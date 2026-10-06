use crate::PgDatabase;
use futures_util::TryStreamExt;
use spreadsheet_application::journal::*;
use spreadsheet_domain::workbook::*;
use spreadsheet_storage_codec as codec;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
mod axis_guard;
#[cfg(test)]
mod axis_tests;
mod discovery;
#[cfg(test)]
mod discovery_tests;
mod merge_guard;
#[cfg(test)]
mod merge_tests;
mod move_guard;
#[cfg(test)]
mod move_tests;
#[cfg(test)]
mod range_history_tests;
#[cfg(test)]
mod sheet_settings_tests;
#[cfg(test)]
mod sort_tests;
mod style_guard;
#[cfg(test)]
mod style_tests;
#[cfg(test)]
mod timer_admission_benchmark;
fn uuid(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}
fn error(e: sqlx::Error) -> JournalError {
    match e {
        sqlx::Error::Database(e)
            if matches!(
                e.code().as_deref(),
                Some("40001" | "40P01" | "55P03" | "23505")
            ) =>
        {
            JournalError::Conflict
        }
        _ => JournalError::Unavailable,
    }
}
fn invalid(_: codec::CodecError) -> JournalError {
    JournalError::Corrupt
}
fn version(n: i64) -> Result<DocumentVersion, JournalError> {
    DocumentVersion::new(u64::try_from(n).map_err(|_| JournalError::Corrupt)?)
        .map_err(|_| JournalError::Corrupt)
}
fn number(n: u64) -> Result<i64, JournalError> {
    i64::try_from(n).map_err(|_| JournalError::Corrupt)
}
const UNDO_WINDOW: u64 = 10_000;
const MAX_UNDO_TAIL_BYTES: usize = 64 * 1024 * 1024;
const MAX_UNDO_ADDRESS_TOUCHES: usize = 100_000;
const MAX_IMPORT_CELLS: usize = 1_000;
const MAX_IMPORT_ENCODED_BYTES: usize = 32 * 1024 * 1024;
fn encrypted_plaintext_length(length: usize) -> Result<usize, JournalError> {
    length
        .checked_sub(crate::crypto::ENVELOPE_OVERHEAD)
        .filter(|n| *n <= 4 * 1024 * 1024)
        .ok_or(JournalError::Corrupt)
}
pub(crate) struct StoredEventRow<'a> {
    pub(crate) document: DocumentId,
    pub(crate) committed_version: i64,
    pub(crate) operation_id: Uuid,
    pub(crate) actor_id: Option<Uuid>,
    pub(crate) principal_kind: &'a str,
    pub(crate) schema_version: i32,
    pub(crate) payload: &'a [u8],
    pub(crate) payload_checksum: &'a [u8],
    pub(crate) canonical_event_hash: &'a [u8],
}
fn crypto_error(error: crate::CryptoError) -> JournalError {
    match error {
        crate::CryptoError::UnsupportedVersion => JournalError::UnsupportedSchema,
        crate::CryptoError::LimitExceeded { .. } => JournalError::PayloadTooLarge,
        crate::CryptoError::Database(_)
        | crate::CryptoError::NonceExhausted
        | crate::CryptoError::KeyProvider(
            spreadsheet_application::keys::KeyProviderError::KeyUnavailable
            | spreadsheet_application::keys::KeyProviderError::ProviderUnavailable
            | spreadsheet_application::keys::KeyProviderError::RandomnessUnavailable,
        ) => JournalError::Unavailable,
        _ => JournalError::Corrupt,
    }
}
pub(crate) async fn verified_event(
    db: &PgDatabase,
    row: StoredEventRow<'_>,
) -> Result<CommittedEvent, JournalError> {
    verified_event_cached(db, &mut crate::crypto::ReadKeyCache::default(), row).await
}

pub(crate) async fn verified_event_cached(
    db: &PgDatabase,
    keys: &mut crate::crypto::ReadKeyCache,
    row: StoredEventRow<'_>,
) -> Result<CommittedEvent, JournalError> {
    if !matches!(row.schema_version, 1..=42) {
        return Err(JournalError::UnsupportedSchema);
    }
    if codec::checksum(row.payload).as_slice() != row.payload_checksum {
        return Err(JournalError::Corrupt);
    }
    let stored_version = version(row.committed_version)?.get();
    let schema = u32::try_from(row.schema_version).map_err(|_| JournalError::Corrupt)?;
    let payload = db
        .open_record_cached(
            keys,
            row.document,
            stored_version,
            "journal",
            row.operation_id,
            schema,
            row.payload,
            4 * 1024 * 1024,
        )
        .await
        .map_err(crypto_error)?;
    let hash = db
        .open_record_cached(
            keys,
            row.document,
            stored_version,
            "journal-hash",
            row.operation_id,
            schema,
            row.canonical_event_hash,
            32,
        )
        .await
        .map_err(crypto_error)?;
    let event = codec::decode(&payload).map_err(|error| match error {
        codec::CodecError::UnsupportedSchema => JournalError::UnsupportedSchema,
        _ => JournalError::Corrupt,
    })?;
    if codec::schema_version(&event.operation)
        != u32::try_from(row.schema_version).map_err(|_| JournalError::Corrupt)?
        || event.document_id != row.document
        || event.version != version(row.committed_version)?
        || event.operation_id.as_bytes() != row.operation_id.as_bytes()
        || match event.principal {
            EventPrincipal::User(actor) => {
                row.principal_kind != "user"
                    || row.actor_id.as_ref().map(Uuid::as_bytes) != Some(actor.as_bytes())
            }
            EventPrincipal::System => row.principal_kind != "system" || row.actor_id.is_some(),
        }
        || codec::event_hash(&event).map_err(invalid)?.as_slice() != hash.as_slice()
    {
        return Err(JournalError::Corrupt);
    }
    Ok(event)
}
fn target_writes(
    event: &CommittedEvent,
    actor: ActorId,
) -> Result<Vec<GuardedContentWrite>, JournalError> {
    if event.principal != EventPrincipal::User(actor) || !event.operation.is_inverse_target() {
        return Err(JournalError::NotUndoable);
    }
    match &event.operation {
        Operation::CellWrite {
            address,
            content,
            previous,
            ..
        } => Ok(vec![GuardedContentWrite {
            address: *address,
            content: content.clone(),
            previous: previous.clone(),
        }]),
        Operation::RangeWrite { writes, .. }
        | Operation::MoveRange { writes, .. }
        | Operation::MoveCompensation { writes, .. } => Ok(writes.clone()),
        Operation::CellBatchWrite { content_writes, .. } => Ok(content_writes.clone()),
        Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => {
            Ok(change.writes.clone())
        }
        _ => Err(JournalError::NotUndoable),
    }
}
fn touched_addresses(
    operation: &Operation,
    targets: &std::collections::BTreeMap<CellAddress, Vec<(OperationId, DocumentVersion)>>,
) -> Vec<CellAddress> {
    match operation {
        Operation::SetCellValue { address, .. }
        | Operation::ClearCell { address }
        | Operation::SetCellFormula { address, .. }
        | Operation::CellWrite { address, .. } => targets
            .contains_key(address)
            .then_some(*address)
            .into_iter()
            .collect(),
        Operation::SetCellValues { writes } => writes
            .iter()
            .map(|w| w.address)
            .filter(|a| targets.contains_key(a))
            .collect(),
        Operation::CopyRange { writes } => writes
            .iter()
            .map(|w| w.address)
            .filter(|a| targets.contains_key(a))
            .collect(),
        Operation::RangeWrite { writes, .. }
        | Operation::MoveRange { writes, .. }
        | Operation::MoveCompensation { writes, .. } => writes
            .iter()
            .map(|w| w.address)
            .filter(|a| targets.contains_key(a))
            .collect(),
        Operation::CellBatchWrite { content_writes, .. } => content_writes
            .iter()
            .map(|w| w.address)
            .filter(|a| targets.contains_key(a))
            .collect(),
        Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => change
            .writes
            .iter()
            .map(|w| w.address)
            .filter(|a| targets.contains_key(a))
            .collect(),
        Operation::DeleteRows { sheet, rows } => targets
            .keys()
            .copied()
            .filter(|a| a.sheet == *sheet && rows.contains(&a.row))
            .collect(),
        Operation::DeleteColumns { sheet, columns } => targets
            .keys()
            .copied()
            .filter(|a| a.sheet == *sheet && columns.contains(&a.column))
            .collect(),
        Operation::CreateWorkbook { .. }
        | Operation::AddSheet { .. }
        | Operation::ExtendRows { .. }
        | Operation::InsertRows { .. }
        | Operation::InsertRowsRebased { .. }
        | Operation::InsertColumnsRebased { .. }
        | Operation::AdvanceCalculationEpoch { .. }
        | Operation::StyleWrite { .. }
        | Operation::StyleCompensation { .. }
        | Operation::AxisSizeWrite { .. }
        | Operation::SheetSettingsWrite { .. }
        | Operation::CopySheet { .. }
        | Operation::FrozenRowsWrite { .. }
        | Operation::MergeWrite { .. } => Vec::new(),
    }
}
fn inverse_shape_matches(target: &Operation, inverse: &Operation) -> bool {
    match (target, inverse) {
        (Operation::CellWrite { .. }, Operation::CellWrite { .. })
        | (Operation::RangeWrite { .. }, Operation::RangeWrite { .. }) => true,
        (Operation::CellBatchWrite { .. }, Operation::CellBatchWrite { .. }) => true,
        (
            Operation::MergeWrite {
                sheet: a,
                change: ac,
                ..
            },
            Operation::MergeWrite {
                sheet: b,
                change: bc,
                ..
            },
        ) => a == b && ac.range == bc.range && ac.merged != bc.merged,
        (
            Operation::MoveRange {
                source: a,
                destination: b,
                rows: r,
                columns: c,
                ..
            }
            | Operation::MoveCompensation {
                source: a,
                destination: b,
                rows: r,
                columns: c,
                ..
            },
            Operation::MoveCompensation {
                source: x,
                destination: y,
                rows: s,
                columns: d,
                ..
            },
        ) => a == x && b == y && r == s && c == d,
        _ => false,
    }
}
async fn access_locked(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<DocumentAccess, JournalError> {
    let identity_kind =
        sqlx::query_scalar::<_, String>("SELECT identity_kind FROM users WHERE user_id=$1")
            .bind(uuid(identity.actor_id.as_bytes()))
            .fetch_optional(&mut **tx)
            .await
            .map_err(error)?
            .ok_or(JournalError::Unauthorized)?;
    if identity_kind == "share_guest" {
        return crate::shares::guest_access_locked(tx, identity, document).await;
    }
    if identity_kind != "member" {
        return Err(JournalError::Unauthorized);
    }
    let membership = sqlx::query!(
        "SELECT access_epoch FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
        uuid(identity.workspace_id.as_bytes()),
        uuid(identity.actor_id.as_bytes())
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::Unauthorized)?;
    let doc=sqlx::query!("SELECT head_version, access_epoch, state FROM documents WHERE workspace_id=$1 AND document_id=$2 FOR UPDATE",uuid(identity.workspace_id.as_bytes()),uuid(document.as_bytes())).fetch_optional(&mut **tx).await.map_err(error)?.ok_or(JournalError::NotFound)?;
    if doc.state != "active" {
        return Err(JournalError::Unavailable);
    }
    let permission = sqlx::query!(
        "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2",
        uuid(document.as_bytes()),
        uuid(identity.actor_id.as_bytes())
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::Unauthorized)?;
    Ok(DocumentAccess {
        head: version(doc.head_version)?,
        document_access_epoch: u64::try_from(doc.access_epoch)
            .map_err(|_| JournalError::Corrupt)?,
        membership_access_epoch: u64::try_from(membership.access_epoch)
            .map_err(|_| JournalError::Corrupt)?,
        can_edit: permission.role != "viewer",
    })
}

/// Authorization snapshot whose shared PostgreSQL row locks remain held while
/// the caller sends one bounded document payload. This extends the delivery
/// fence across server processes, where an in-memory RwLock is insufficient.
pub struct DocumentAccessLease {
    transaction: Transaction<'static, Postgres>,
    access: DocumentAccess,
}

impl DocumentAccessLease {
    pub fn access(&self) -> DocumentAccess {
        self.access
    }

    pub async fn release(self) -> Result<(), JournalError> {
        self.transaction.commit().await.map_err(error)
    }
}

pub(crate) async fn access_for_delivery_locked(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<DocumentAccess, JournalError> {
    let identity_kind =
        sqlx::query_scalar::<_, String>("SELECT identity_kind FROM users WHERE user_id=$1")
            .bind(uuid(identity.actor_id.as_bytes()))
            .fetch_optional(&mut **tx)
            .await
            .map_err(error)?
            .ok_or(JournalError::Unauthorized)?;
    if identity_kind == "share_guest" {
        return crate::shares::guest_delivery_access_locked(tx, identity, document).await;
    }
    if identity_kind != "member" {
        return Err(JournalError::Unauthorized);
    }
    let membership = sqlx::query!(
        "SELECT access_epoch FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
        uuid(identity.workspace_id.as_bytes()),
        uuid(identity.actor_id.as_bytes())
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::Unauthorized)?;
    let doc = sqlx::query_as::<_, (i64, i64, String)>(
        "SELECT head_version,access_epoch,state FROM documents WHERE workspace_id=$1 AND document_id=$2 FOR SHARE",
    )
    .bind(uuid(identity.workspace_id.as_bytes()))
    .bind(uuid(document.as_bytes()))
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::NotFound)?;
    if doc.2 != "active" {
        return Err(JournalError::Unavailable);
    }
    let permission = sqlx::query_scalar::<_, String>(
        "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2 FOR SHARE",
    )
    .bind(uuid(document.as_bytes()))
    .bind(uuid(identity.actor_id.as_bytes()))
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::Unauthorized)?;
    Ok(DocumentAccess {
        head: version(doc.0)?,
        document_access_epoch: u64::try_from(doc.1).map_err(|_| JournalError::Corrupt)?,
        membership_access_epoch: u64::try_from(membership.access_epoch)
            .map_err(|_| JournalError::Corrupt)?,
        can_edit: permission != "viewer",
    })
}

impl PgDatabase {
    pub async fn lease_delivery_access(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
    ) -> Result<DocumentAccessLease, JournalError> {
        let mut transaction = self.pool.begin().await.map_err(error)?;
        self.guard_transaction(&mut transaction)
            .await
            .map_err(error)?;
        let access = access_for_delivery_locked(&mut transaction, identity, document).await?;
        Ok(DocumentAccessLease {
            transaction,
            access,
        })
    }

    pub async fn lease_owner_delivery_access(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
    ) -> Result<DocumentAccessLease, JournalError> {
        let mut lease = self.lease_delivery_access(identity, document).await?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM document_permissions WHERE document_id=$1 AND user_id=$2 FOR SHARE",
        )
        .bind(uuid(document.as_bytes()))
        .bind(uuid(identity.actor_id.as_bytes()))
        .fetch_optional(&mut *lease.transaction)
        .await
        .map_err(error)?;
        if role.as_deref() != Some("owner") {
            return Err(JournalError::Unauthorized);
        }
        Ok(lease)
    }
}
async fn existing(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    operation: OperationId,
    fingerprint: [u8; 32],
    head: DocumentVersion,
) -> Result<Option<CommitReceipt>, JournalError> {
    let row=sqlx::query!("SELECT committed_version, schema_version, command_fingerprint, canonical_event_hash FROM document_operations WHERE document_id=$1 AND operation_id=$2",uuid(document.as_bytes()),uuid(operation.as_bytes())).fetch_optional(&mut **tx).await.map_err(error)?;
    if let Some(r) = row {
        let stored_version = version(r.committed_version)?.get();
        let schema = u32::try_from(r.schema_version).map_err(|_| JournalError::Corrupt)?;
        let stored_fingerprint = db
            .open_record(
                document,
                stored_version,
                "journal-fingerprint",
                uuid(operation.as_bytes()),
                schema,
                &r.command_fingerprint,
                32,
            )
            .await
            .map_err(crypto_error)?;
        let stored_hash = db
            .open_record(
                document,
                stored_version,
                "journal-hash",
                uuid(operation.as_bytes()),
                schema,
                &r.canonical_event_hash,
                32,
            )
            .await
            .map_err(crypto_error)?;
        if stored_fingerprint.as_slice() != fingerprint {
            return Err(JournalError::IdempotencyConflict);
        }
        Ok(Some(CommitReceipt {
            operation_id: operation,
            committed_version: version(r.committed_version)?,
            canonical_event_hash: stored_hash
                .as_slice()
                .try_into()
                .map_err(|_| JournalError::Corrupt)?,
            current_head_version: head,
        }))
    } else {
        Ok(None)
    }
}
async fn undo_target_locked(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target_id: OperationId,
    actor: ActorId,
    head: DocumentVersion,
) -> Result<CommittedEvent, JournalError> {
    let row = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND operation_id=$2",
        uuid(document.as_bytes()),
        uuid(target_id.as_bytes())
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::UndoConflict)?;
    let event = verified_event(
        db,
        StoredEventRow {
            document,
            committed_version: row.committed_version,
            operation_id: row.operation_id,
            actor_id: row.actor_id,
            principal_kind: &row.principal_kind,
            schema_version: row.schema_version,
            payload: &row.payload,
            payload_checksum: &row.payload_checksum,
            canonical_event_hash: &row.canonical_event_hash,
        },
    )
    .await?;
    if event.principal != EventPrincipal::User(actor) || !event.operation.is_inverse_target() {
        return Err(JournalError::NotUndoable);
    }
    if head
        .get()
        .checked_sub(event.version.get())
        .ok_or(JournalError::Corrupt)?
        > UNDO_WINDOW
    {
        return Err(JournalError::UndoWindowExceeded);
    }
    Ok(event)
}
async fn guard_undo_tail(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target: &CommittedEvent,
    head: DocumentVersion,
) -> Result<(), JournalError> {
    let EventPrincipal::User(actor) = target.principal else {
        return Err(JournalError::NotUndoable);
    };
    let writes = target_writes(target, actor)?;
    let formula_history = writes.iter().any(|write| {
        write
            .content
            .iter()
            .chain(write.previous.iter())
            .any(|c| matches!(c, CellContent::Formula(_)))
    });
    let mut effective_writes = writes
        .iter()
        .map(|write| (write.address, Vec::<(OperationId, DocumentVersion)>::new()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut rows = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 AND committed_version<=$3 ORDER BY committed_version",
        uuid(document.as_bytes()),
        number(target.version.get())?,
        number(head.get())?
    )
    .fetch(&mut **tx);
    let mut expected = target.version.get();
    let mut payload_bytes = 0usize;
    let mut address_touches = 0usize;
    let mut movement_history = Vec::new();
    while let Some(row) = rows.try_next().await.map_err(error)? {
        payload_bytes = payload_bytes
            .checked_add(encrypted_plaintext_length(row.payload.len())?)
            .filter(|n| *n <= MAX_UNDO_TAIL_BYTES)
            .ok_or(JournalError::UndoWindowExceeded)?;
        expected = expected.checked_add(1).ok_or(JournalError::Corrupt)?;
        let event = verified_event(
            db,
            StoredEventRow {
                document,
                committed_version: row.committed_version,
                operation_id: row.operation_id,
                actor_id: row.actor_id,
                principal_kind: &row.principal_kind,
                schema_version: row.schema_version,
                payload: &row.payload,
                payload_checksum: &row.payload_checksum,
                canonical_event_hash: &row.canonical_event_hash,
            },
        )
        .await?;
        if event.version.get() != expected {
            return Err(JournalError::Corrupt);
        }
        if matches!(event.operation, Operation::MergeWrite { sheet, .. }
            if effective_writes.keys().any(|address| address.sheet == sheet))
        {
            return Err(JournalError::UndoConflict);
        }
        for address in touched_addresses(&event.operation, &effective_writes) {
            address_touches = address_touches
                .checked_add(1)
                .filter(|n| *n <= MAX_UNDO_ADDRESS_TOUCHES)
                .ok_or(JournalError::UndoWindowExceeded)?;
            if event.principal != target.principal || !event.operation.is_inverse_target() {
                return Err(JournalError::UndoConflict);
            }
            let stack = effective_writes
                .get_mut(&address)
                .ok_or(JournalError::Corrupt)?;
            let cancelled = match &event.operation {
                Operation::CellWrite {
                    undoes: Some(id),
                    undoes_version: Some(version),
                    ..
                }
                | Operation::RangeWrite {
                    undoes: Some(id),
                    undoes_version: Some(version),
                    ..
                }
                | Operation::CellBatchWrite {
                    undoes: Some(id),
                    undoes_version: Some(version),
                    ..
                }
                | Operation::MoveCompensation {
                    undoes: id,
                    undoes_version: version,
                    ..
                }
                | Operation::AxisCompensation {
                    undoes: id,
                    undoes_version: version,
                    ..
                } => stack.last() == Some(&(*id, *version)),
                _ => false,
            };
            if cancelled {
                stack.pop();
            } else {
                stack.push((event.operation_id, event.version));
            }
        }
        if formula_history
            && event.operation.changes_axes()
            && !matches!(
                event.operation,
                Operation::AxisEdit { .. } | Operation::AxisCompensation { .. }
            )
        {
            return Err(JournalError::NotUndoable);
        }
        let changes_formula_binding_context = matches!(
            event.operation,
            Operation::MoveRange { .. }
                | Operation::MoveCompensation { .. }
                | Operation::AxisEdit { .. }
                | Operation::AxisCompensation { .. }
        ) || matches!(
            &event.operation,
            Operation::CellBatchWrite {
                intent: spreadsheet_domain::workbook::CellBatchIntent::MoveRange { .. }
                    | spreadsheet_domain::workbook::CellBatchIntent::ShiftCells { .. },
                ..
            }
        );
        if formula_history && changes_formula_binding_context {
            if event.principal != target.principal {
                return Err(JournalError::NotUndoable);
            }
            let cancelled = match event.operation {
                Operation::MoveCompensation {
                    undoes,
                    undoes_version,
                    ..
                }
                | Operation::AxisCompensation {
                    undoes,
                    undoes_version,
                    ..
                } => movement_history.last() == Some(&(undoes, undoes_version)),
                Operation::CellBatchWrite {
                    intent:
                        spreadsheet_domain::workbook::CellBatchIntent::MoveRange { .. }
                        | spreadsheet_domain::workbook::CellBatchIntent::ShiftCells { .. },
                    undoes: Some(undoes),
                    undoes_version: Some(undoes_version),
                    ..
                } => movement_history.last() == Some(&(undoes, undoes_version)),
                _ => false,
            };
            if cancelled {
                movement_history.pop();
            } else {
                movement_history.push((event.operation_id, event.version));
            }
        }
    }
    if expected != head.get() {
        return Err(JournalError::Corrupt);
    }
    if effective_writes.values().any(|stack| !stack.is_empty()) {
        return Err(JournalError::UndoConflict);
    }
    if !movement_history.is_empty() {
        return Err(JournalError::NotUndoable);
    }
    Ok(())
}
async fn insert(
    db: &PgDatabase,
    tx: &mut Transaction<'_, Postgres>,
    event: &CommittedEvent,
    fingerprint: [u8; 32],
) -> Result<CommitReceipt, JournalError> {
    let payload = codec::encode(event).map_err(|error| match error {
        codec::CodecError::EventTooLarge => JournalError::PayloadTooLarge,
        _ => JournalError::Corrupt,
    })?;
    let hash = codec::event_hash(event).map_err(invalid)?;
    let schema = codec::schema_version(&event.operation);
    let object = uuid(event.operation_id.as_bytes());
    let payload = db
        .seal_record_guarded(
            event.document_id,
            event.version.get(),
            "journal",
            object,
            schema,
            &payload,
            4 * 1024 * 1024,
        )
        .await
        .map_err(crypto_error)?;
    let stored_fingerprint = db
        .seal_record_guarded(
            event.document_id,
            event.version.get(),
            "journal-fingerprint",
            object,
            schema,
            &fingerprint,
            32,
        )
        .await
        .map_err(crypto_error)?;
    let stored_hash = db
        .seal_record_guarded(
            event.document_id,
            event.version.get(),
            "journal-hash",
            object,
            schema,
            &hash,
            32,
        )
        .await
        .map_err(crypto_error)?;
    let checksum = codec::checksum(&payload);
    let (actor, principal_kind) = match event.principal {
        EventPrincipal::User(actor) => (Some(uuid(actor.as_bytes())), "user"),
        EventPrincipal::System => (None, "system"),
    };
    sqlx::query!(
        "INSERT INTO document_operations(document_id,committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        uuid(event.document_id.as_bytes()), number(event.version.get())?,
        uuid(event.operation_id.as_bytes()), actor, principal_kind,
        i32::try_from(codec::schema_version(&event.operation)).map_err(|_|JournalError::Corrupt)?,
        payload, &checksum[..], stored_fingerprint, stored_hash
    ).execute(&mut **tx).await.map_err(error)?;
    Ok(CommitReceipt {
        operation_id: event.operation_id,
        committed_version: event.version,
        canonical_event_hash: hash,
        current_head_version: event.version,
    })
}
impl OperationJournal for PgDatabase {
    async fn undo_history(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
    ) -> Result<spreadsheet_application::history::UndoHistory, JournalError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            discovery::discover(self, identity, document_id),
        )
        .await
        .map_err(|_| JournalError::Unavailable)?
    }
    async fn calculation_epoch_version(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        through: DocumentVersion,
    ) -> Result<Option<DocumentVersion>, JournalError> {
        let access = self.access(identity, document_id).await?;
        if through > access.head {
            return Err(JournalError::Conflict);
        }
        let row = sqlx::query!("SELECT committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,canonical_event_hash FROM document_operations WHERE document_id = $1 AND committed_version <= $2 AND principal_kind = 'system' ORDER BY committed_version DESC LIMIT 1", uuid(document_id.as_bytes()), number(through.get())?)
            .fetch_optional(&self.pool).await.map_err(error)?;
        if let Some(row) = row {
            let event = verified_event(
                self,
                StoredEventRow {
                    document: document_id,
                    committed_version: row.committed_version,
                    operation_id: row.operation_id,
                    actor_id: row.actor_id,
                    principal_kind: &row.principal_kind,
                    schema_version: row.schema_version,
                    payload: &row.payload,
                    payload_checksum: &row.payload_checksum,
                    canonical_event_hash: &row.canonical_event_hash,
                },
            )
            .await?;
            if !matches!(event.operation, Operation::AdvanceCalculationEpoch { .. }) {
                return Err(JournalError::Corrupt);
            }
            Ok(Some(event.version))
        } else {
            Ok(None)
        }
    }
    async fn quarantine(&self, document_id: DocumentId) -> Result<(), JournalError> {
        let mut tx = self.begin_guarded().await.map_err(error)?;
        sqlx::query!(
            "UPDATE documents SET state='quarantined' WHERE document_id=$1",
            uuid(document_id.as_bytes())
        )
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        tx.commit().await.map_err(error)?;
        Ok(())
    }
    async fn access(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
    ) -> Result<DocumentAccess, JournalError> {
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let result = access_locked(&mut tx, identity, document_id).await?;
        tx.commit().await.map_err(error)?;
        Ok(result)
    }
    async fn receipt(
        &self,
        identity: AuthenticatedActor,
        command: &Command,
    ) -> Result<Option<CommitReceipt>, JournalError> {
        if identity.actor_id != command.actor_id {
            return Err(JournalError::Unauthorized);
        }
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, command.document_id).await?;
        let result = existing(
            self,
            &mut tx,
            command.document_id,
            command.operation_id,
            codec::command_fingerprint(command).map_err(invalid)?,
            access.head,
        )
        .await?;
        tx.commit().await.map_err(error)?;
        Ok(result)
    }
    async fn create(
        &self,
        identity: AuthenticatedActor,
        event: CommittedEvent,
    ) -> Result<CommitReceipt, JournalError> {
        if event.principal != EventPrincipal::User(identity.actor_id) {
            return Err(JournalError::Unauthorized);
        }
        if event.version.get() != 1 || !matches!(event.operation, Operation::CreateWorkbook { .. })
        {
            return Err(JournalError::Corrupt);
        }
        let mut candidate = Workbook::empty(event.document_id);
        candidate
            .apply_event(&event)
            .map_err(|_| JournalError::Corrupt)?;
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let member = sqlx::query!(
            "SELECT role FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR UPDATE",
            uuid(identity.workspace_id.as_bytes()),
            uuid(identity.actor_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?
        .ok_or(JournalError::Unauthorized)?;
        if member.role != "owner" && member.role != "member" {
            return Err(JournalError::Unauthorized);
        }
        let already = sqlx::query!(
            "SELECT workspace_id FROM documents WHERE document_id=$1",
            uuid(event.document_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?;
        if already.is_some() {
            let access = access_locked(&mut tx, identity, event.document_id).await?;
            let receipt = existing(
                self,
                &mut tx,
                event.document_id,
                event.operation_id,
                codec::genesis_fingerprint(&event),
                access.head,
            )
            .await?
            .ok_or(JournalError::IdempotencyConflict)?;
            tx.commit().await.map_err(error)?;
            return Ok(receipt);
        }
        self.ensure_document_key_guarded(identity.workspace_id, event.document_id)
            .await
            .map_err(crypto_error)?;
        sqlx::query!(
            "INSERT INTO documents(document_id,workspace_id,head_version) VALUES($1,$2,1)",
            uuid(event.document_id.as_bytes()),
            uuid(identity.workspace_id.as_bytes())
        )
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        sqlx::query!("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'owner')",uuid(identity.workspace_id.as_bytes()),uuid(event.document_id.as_bytes()),uuid(identity.actor_id.as_bytes())).execute(&mut *tx).await.map_err(error)?;
        sqlx::query!("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let receipt = insert(self, &mut tx, &event, codec::genesis_fingerprint(&event)).await?;
        tx.commit().await.map_err(error)?;
        Ok(receipt)
    }
    async fn commit(&self, request: &CommitRequest) -> Result<CommitOutcome, JournalError> {
        let c = &request.command;
        let e = &request.event;
        if request.identity.actor_id != c.actor_id
            || e.principal != EventPrincipal::User(c.actor_id)
        {
            return Err(JournalError::Unauthorized);
        }
        if c.document_id != e.document_id
            || c.operation_id != e.operation_id
            || c.base_version.get() > request.expected_head.get()
            || e.version.get()
                != request
                    .expected_head
                    .get()
                    .checked_add(1)
                    .ok_or(JournalError::Corrupt)?
            || matches!(e.operation, Operation::CreateWorkbook { .. })
            || matches!(e.operation, Operation::AdvanceCalculationEpoch { .. })
        {
            return Err(JournalError::Corrupt);
        }
        let canonical_payload = match &e.operation {
            Operation::SheetSettingsWrite { sheet, .. } => match &c.operation {
                CommandPayload::SetSheetSettings { sheet: id, .. } if id == sheet => {
                    c.operation.clone()
                }
                _ => return Err(JournalError::Corrupt),
            },
            Operation::CopySheet {
                source,
                sheet,
                index,
            } => CommandPayload::CopySheet {
                source: *source,
                sheet: sheet.clone(),
                index: *index,
            },
            Operation::CellBatchWrite {
                intent:
                    CellBatchIntent::ShiftCells {
                        source,
                        rows,
                        columns,
                        axis,
                        insert,
                    },
                undoes: None,
                ..
            } => CommandPayload::ShiftCells {
                source: *source,
                rows: *rows,
                columns: *columns,
                axis: *axis,
                insert: *insert,
            },
            Operation::FrozenRowsWrite { sheet, rows, .. } => {
                let expected = CommandPayload::SetFrozenRows {
                    sheet: *sheet,
                    rows: *rows,
                };
                if c.operation != expected {
                    return Err(JournalError::Corrupt);
                }
                expected
            }
            Operation::AxisSizeWrite { sheet, writes } => {
                let expected = CommandPayload::SetAxisSizes {
                    sheet: *sheet,
                    writes: writes
                        .iter()
                        .map(|write| (write.target, write.size))
                        .collect(),
                };
                if c.operation != expected {
                    return Err(JournalError::Corrupt);
                }
                expected
            }
            Operation::MergeWrite {
                sheet,
                change,
                undoes,
                ..
            } => match undoes {
                Some(id) => CommandPayload::UndoCell {
                    target_operation_id: *id,
                },
                None => CommandPayload::SetMergeRange {
                    sheet: *sheet,
                    range: change.range,
                    merged: change.merged,
                },
            },
            Operation::StyleWrite { .. } => match &c.operation {
                CommandPayload::SetCellStyle { .. } | CommandPayload::SetRangeStyle { .. } => {
                    c.operation.clone()
                }
                _ => return Err(JournalError::Corrupt),
            },
            Operation::StyleCompensation { undoes, .. } => CommandPayload::UndoCell {
                target_operation_id: *undoes,
            },
            Operation::AxisEdit { .. } => {
                axis_guard::legacy(&c.operation)?;
                c.operation.clone()
            }
            Operation::AxisCompensation { undoes, .. } => CommandPayload::UndoCell {
                target_operation_id: *undoes,
            },
            Operation::CreateWorkbook { .. } | Operation::AdvanceCalculationEpoch { .. } => {
                return Err(JournalError::Corrupt);
            }
            Operation::AddSheet { sheet } => CommandPayload::AddSheet {
                sheet: sheet.clone(),
            },
            Operation::SetCellValue { address, value } => CommandPayload::SetCellValue {
                address: *address,
                value: value.clone(),
            },
            Operation::SetCellValues { writes } => CommandPayload::SetCellValues {
                writes: writes.clone(),
            },
            Operation::CellBatchWrite {
                undoes: Some(undoes),
                ..
            } => CommandPayload::UndoCell {
                target_operation_id: *undoes,
            },
            Operation::CellBatchWrite {
                intent: spreadsheet_domain::workbook::CellBatchIntent::SortRange { range, .. },
                undoes: None,
                ..
            } => match &c.operation {
                CommandPayload::SortRange { range: selected }
                    if range == selected && c.base_version == request.expected_head =>
                {
                    c.operation.clone()
                }
                _ => return Err(JournalError::Corrupt),
            },
            Operation::CellBatchWrite {
                intent: spreadsheet_domain::workbook::CellBatchIntent::SetValues,
                undoes: None,
                ..
            } => match &c.operation {
                CommandPayload::SetCellBatch { .. } => c.operation.clone(),
                _ => return Err(JournalError::Corrupt),
            },
            Operation::CellBatchWrite {
                intent:
                    spreadsheet_domain::workbook::CellBatchIntent::CopyRange {
                        source,
                        destination,
                        rows,
                        columns,
                    },
                undoes: None,
                ..
            } => match &c.operation {
                CommandPayload::CopyRange {
                    source: command_source,
                    destination: command_destination,
                    rows: command_rows,
                    columns: command_columns,
                    repeat: None,
                } if source == command_source
                    && destination == command_destination
                    && rows == command_rows
                    && columns == command_columns =>
                {
                    c.operation.clone()
                }
                _ => return Err(JournalError::Corrupt),
            },
            Operation::CellBatchWrite {
                intent:
                    spreadsheet_domain::workbook::CellBatchIntent::MoveRange {
                        source,
                        destination,
                        rows,
                        columns,
                    },
                undoes: None,
                ..
            } => match &c.operation {
                CommandPayload::MoveRange {
                    source: command_source,
                    destination: command_destination,
                    rows: command_rows,
                    columns: command_columns,
                } if source == command_source
                    && destination == command_destination
                    && rows == command_rows
                    && columns == command_columns =>
                {
                    c.operation.clone()
                }
                _ => return Err(JournalError::Corrupt),
            },
            Operation::CopyRange { .. } => match &c.operation {
                CommandPayload::CopyRange { .. } => c.operation.clone(),
                _ => return Err(JournalError::Corrupt),
            },
            Operation::RangeWrite {
                writes,
                kind,
                undoes,
                undoes_version,
                ..
            } => match (kind, undoes, undoes_version) {
                (Some(RangeWriteKind::SetValues), None, None) => CommandPayload::SetCellValues {
                    writes: writes
                        .iter()
                        .map(|w| {
                            Ok(CellWrite {
                                address: w.address,
                                value: match &w.content {
                                    Some(CellContent::Value(value)) => value.clone(),
                                    None => CellValue::Blank,
                                    _ => return Err(JournalError::Corrupt),
                                },
                            })
                        })
                        .collect::<Result<_, JournalError>>()?,
                },
                (Some(RangeWriteKind::Copy), None, None)
                    if matches!(c.operation, CommandPayload::CopyRange { .. }) =>
                {
                    c.operation.clone()
                }
                (None, Some(id), Some(_)) => CommandPayload::UndoCell {
                    target_operation_id: *id,
                },
                _ => return Err(JournalError::Corrupt),
            },
            Operation::MoveRange {
                source,
                destination,
                rows,
                columns,
                ..
            } => CommandPayload::MoveRange {
                source: *source,
                destination: *destination,
                rows: *rows,
                columns: *columns,
            },
            Operation::MoveCompensation { undoes, .. } => CommandPayload::UndoCell {
                target_operation_id: *undoes,
            },
            Operation::ExtendRows { sheet, rows } => CommandPayload::ExtendRows {
                sheet: *sheet,
                rows: rows.clone(),
            },
            Operation::InsertRows {
                sheet,
                before,
                rows,
            }
            | Operation::InsertRowsRebased {
                sheet,
                before,
                rows,
            } => CommandPayload::InsertRows {
                sheet: *sheet,
                before: *before,
                rows: rows.clone(),
            },
            Operation::DeleteRows { sheet, rows } => CommandPayload::DeleteRows {
                sheet: *sheet,
                rows: rows.clone(),
            },
            Operation::InsertColumnsRebased {
                sheet,
                before,
                columns,
            } => CommandPayload::InsertColumns {
                sheet: *sheet,
                before: *before,
                columns: columns.clone(),
            },
            Operation::DeleteColumns { sheet, columns } => CommandPayload::DeleteColumns {
                sheet: *sheet,
                columns: columns.clone(),
            },
            Operation::ClearCell { address } => CommandPayload::ClearCell { address: *address },
            Operation::CellWrite {
                address,
                content,
                kind,
                undoes,
                undoes_version,
                ..
            } => match (kind, undoes, undoes_version, content) {
                (Some(CellWriteKind::SetValue), None, None, Some(CellContent::Value(value))) => {
                    CommandPayload::SetCellValue {
                        address: *address,
                        value: value.clone(),
                    }
                }
                (Some(CellWriteKind::SetValue), None, None, None) => CommandPayload::SetCellValue {
                    address: *address,
                    value: CellValue::Blank,
                },
                (Some(CellWriteKind::Clear), None, None, None) => {
                    CommandPayload::ClearCell { address: *address }
                }
                (
                    Some(CellWriteKind::SetFormula),
                    None,
                    None,
                    Some(CellContent::Formula(formula)),
                ) => {
                    if formula.origin() != *address
                        || formula.bound_at_version() > request.expected_head
                    {
                        return Err(JournalError::Corrupt);
                    }
                    CommandPayload::SetCellFormula {
                        address: *address,
                        source: FormulaSource::new(formula.source().to_owned())
                            .map_err(|_| JournalError::Corrupt)?,
                    }
                }
                (None, Some(target_operation_id), Some(_), _) => CommandPayload::UndoCell {
                    target_operation_id: *target_operation_id,
                },
                _ => return Err(JournalError::Corrupt),
            },
            Operation::SetCellFormula { address, formula } => {
                if formula.origin() != *address
                    || formula.bound_at_version() > request.expected_head
                {
                    return Err(JournalError::Corrupt);
                }
                CommandPayload::SetCellFormula {
                    address: *address,
                    source: FormulaSource::new(formula.source().to_owned())
                        .map_err(|_| JournalError::Corrupt)?,
                }
            }
        };
        let canonical = Command {
            operation: canonical_payload,
            ..c.clone()
        };
        if codec::command_fingerprint(c).map_err(invalid)?
            != codec::command_fingerprint(&canonical).map_err(invalid)?
        {
            return Err(JournalError::Corrupt);
        }
        let fingerprint = codec::command_fingerprint(c).map_err(invalid)?;
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let access = access_locked(&mut tx, request.identity, c.document_id).await?;
        self.deny_anonymized_guest_write(&mut tx, request.identity, c.document_id)
            .await?;
        let identity_kind =
            sqlx::query_scalar::<_, String>("SELECT identity_kind FROM users WHERE user_id=$1")
                .bind(uuid(request.identity.actor_id.as_bytes()))
                .fetch_one(&mut *tx)
                .await
                .map_err(error)?;
        if identity_kind == "share_guest" {
            crate::shares::guest_command_floor_locked(
                &mut tx,
                request.identity,
                c.document_id,
                c.base_version,
            )
            .await?;
        }
        if !access.can_edit {
            return Err(JournalError::Unauthorized);
        }
        if let Some(receipt) = existing(
            self,
            &mut tx,
            c.document_id,
            c.operation_id,
            fingerprint,
            access.head,
        )
        .await?
        {
            tx.commit().await.map_err(error)?;
            return Ok(CommitOutcome::AlreadyCommitted(receipt));
        }
        if access.head != request.expected_head
            || access.document_access_epoch != request.expected_access.document_access_epoch
            || access.membership_access_epoch != request.expected_access.membership_access_epoch
        {
            return Err(JournalError::Conflict);
        }
        // Resolve durable identities before enforcing exact source versions.
        // Late copy/cut retries reuse receipts without rereading their sources.
        if matches!(
            c.operation,
            CommandPayload::CopyRange { .. }
                | CommandPayload::MoveRange { .. }
                | CommandPayload::ShiftCells { .. }
                | CommandPayload::CopySheet { .. }
                | CommandPayload::SetSheetSettings { .. }
        ) && c.base_version != access.head
        {
            return Err(JournalError::Conflict);
        }
        if matches!(
            e.operation,
            Operation::SheetSettingsWrite { .. }
                | Operation::CopySheet { .. }
                | Operation::CellBatchWrite {
                    intent: CellBatchIntent::ShiftCells { .. },
                    ..
                }
        ) {
            let projection = request
                .inverse_projection
                .as_deref()
                .ok_or(JournalError::Corrupt)?;
            axis_guard::verified_projection(self, &mut tx, c.document_id, access.head, projection)
                .await?;
            if let CommandPayload::SetSheetSettings { sheet, changes } = &c.operation
                && spreadsheet_domain::sheet_settings::prepare(projection, *sheet, changes)
                    .map_err(|_| JournalError::Corrupt)?
                    != e.operation
            {
                return Err(JournalError::Corrupt);
            }
            if projection
                .candidate(&e.operation, e.version)
                .map_err(|_| JournalError::Corrupt)?
                .hash()
                != e.state_hash
            {
                return Err(JournalError::Corrupt);
            }
        }
        let activation: i64 = sqlx::query_scalar!(
            "SELECT axis_activation_version FROM documents WHERE document_id=$1",
            uuid(c.document_id.as_bytes())
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(error)?;
        if activation < 0
            || u64::try_from(activation).map_err(|_| JournalError::Corrupt)? > access.head.get()
        {
            return Err(JournalError::Corrupt);
        }
        if activation > 0
            || matches!(
                e.operation,
                Operation::AxisEdit { .. } | Operation::AxisCompensation { .. }
            )
        {
            let projection = request
                .inverse_projection
                .as_deref()
                .ok_or(JournalError::UndoConflict)?;
            axis_guard::verified_projection(self, &mut tx, c.document_id, access.head, projection)
                .await?;
            if spreadsheet_domain::lifecycle::max_activation_version(projection).get()
                != u64::try_from(activation).map_err(|_| JournalError::Corrupt)?
            {
                return Err(JournalError::Corrupt);
            }
            axis_guard::check_floors(projection, c)?;
            axis_guard::check_formulas(projection, c, &e.operation)?;
        }
        if matches!(e.operation, Operation::StyleWrite { .. })
            || matches!(&e.operation, Operation::CellBatchWrite { style_writes, undoes: None, .. } if !style_writes.is_empty())
        {
            style_guard::validate(self, &mut tx, request, access.head, None).await?;
        }
        if matches!(e.operation, Operation::AxisEdit { .. }) {
            axis_guard::validate(self, &mut tx, request, access.head, None).await?;
        }
        if matches!(e.operation, Operation::MergeWrite { undoes: None, .. }) {
            merge_guard::validate(self, &mut tx, request, access.head, None).await?;
        }
        let inverse_target = match &e.operation {
            Operation::CellWrite {
                kind: None,
                undoes: Some(id),
                undoes_version: Some(version),
                ..
            }
            | Operation::RangeWrite {
                kind: None,
                undoes: Some(id),
                undoes_version: Some(version),
                ..
            }
            | Operation::MoveCompensation {
                undoes: id,
                undoes_version: version,
                ..
            }
            | Operation::StyleCompensation {
                undoes: id,
                undoes_version: version,
                ..
            }
            | Operation::CellBatchWrite {
                undoes: Some(id),
                undoes_version: Some(version),
                ..
            }
            | Operation::AxisCompensation {
                undoes: id,
                undoes_version: version,
                ..
            }
            | Operation::MergeWrite {
                undoes: Some(id),
                undoes_version: Some(version),
                ..
            } => Some((*id, *version)),
            _ => None,
        };
        if let Some((target_id, target_version)) = inverse_target {
            let target = undo_target_locked(
                self,
                &mut tx,
                c.document_id,
                target_id,
                request.identity.actor_id,
                access.head,
            )
            .await?;
            if matches!(e.operation, Operation::StyleCompensation { .. }) {
                style_guard::validate(self, &mut tx, request, access.head, Some(&target)).await?;
            } else if matches!(e.operation, Operation::AxisCompensation { .. }) {
                axis_guard::validate(self, &mut tx, request, access.head, Some(&target)).await?;
            } else if matches!(e.operation, Operation::MergeWrite { .. }) {
                merge_guard::validate(self, &mut tx, request, access.head, Some(&target)).await?;
            } else {
                if matches!(e.operation, Operation::CellBatchWrite { .. }) {
                    style_guard::validate(self, &mut tx, request, access.head, Some(&target))
                        .await?;
                }
                let original = target_writes(&target, request.identity.actor_id)?;
                let inverse = target_writes(e, request.identity.actor_id)?;
                if target.version != target_version
                    || original.len() != inverse.len()
                    || !inverse_shape_matches(&target.operation, &e.operation)
                    || original.iter().zip(&inverse).any(|(a, b)| {
                        a.address != b.address || a.content != b.previous || a.previous != b.content
                    })
                {
                    return Err(JournalError::UndoConflict);
                }
                guard_undo_tail(self, &mut tx, c.document_id, &target, access.head).await?;
                if matches!(e.operation, Operation::MoveCompensation { .. })
                    || matches!(
                        &e.operation,
                        Operation::CellBatchWrite {
                            intent: spreadsheet_domain::workbook::CellBatchIntent::MoveRange { .. }
                                | spreadsheet_domain::workbook::CellBatchIntent::ShiftCells { .. },
                            undoes: Some(_),
                            ..
                        }
                    )
                {
                    let projection = request
                        .inverse_projection
                        .as_deref()
                        .ok_or(JournalError::Corrupt)?;
                    move_guard::validate(
                        self,
                        &mut tx,
                        c.document_id,
                        &target,
                        access.head,
                        projection,
                    )
                    .await?;
                }
            }
        }
        let activation_update = match &e.operation {
            Operation::AxisCompensation { change, .. } => match &change.delta {
                AxisDelta::Rows { inserted, .. } if !inserted.is_empty() => e.version.get(),
                AxisDelta::Columns { inserted, .. } if !inserted.is_empty() => e.version.get(),
                _ => 0,
            },
            _ => 0,
        };
        let updated: Option<i64> = sqlx::query_scalar!("UPDATE documents SET head_version=head_version+1, axis_activation_version=GREATEST(axis_activation_version,$3) WHERE document_id=$1 AND head_version=$2 RETURNING head_version", uuid(c.document_id.as_bytes()), number(request.expected_head.get())?, number(activation_update)?)
            .fetch_optional(&mut *tx).await.map_err(error)?;
        if updated.is_none() {
            return Err(JournalError::Conflict);
        }
        sqlx::query!("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let receipt = insert(self, &mut tx, e, fingerprint).await?;
        tx.commit().await.map_err(error)?;
        Ok(CommitOutcome::Committed(receipt))
    }
    async fn system_receipt(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        operation_id: OperationId,
        sampled_context: &CalculationContext,
    ) -> Result<Option<CommitReceipt>, JournalError> {
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, document_id).await?;
        if sampled_context.reason() == CalculationReason::Explicit && !access.can_edit {
            return Err(JournalError::Unauthorized);
        }
        self.deny_anonymized_guest_write(&mut tx, identity, document_id)
            .await?;
        let fingerprint =
            codec::system_trigger_fingerprint(document_id, operation_id, sampled_context);
        let receipt = existing(
            self,
            &mut tx,
            document_id,
            operation_id,
            fingerprint,
            access.head,
        )
        .await?;
        tx.commit().await.map_err(error)?;
        Ok(receipt)
    }
    async fn commit_system(
        &self,
        request: &SystemCommitRequest,
    ) -> Result<CommitOutcome, JournalError> {
        let event = &request.event;
        if event.principal != EventPrincipal::System
            || !matches!(event.operation, Operation::AdvanceCalculationEpoch { .. })
            || event.version.get()
                != request
                    .expected_head
                    .get()
                    .checked_add(1)
                    .ok_or(JournalError::Corrupt)?
        {
            return Err(JournalError::Corrupt);
        }
        let fingerprint = codec::system_fingerprint(event).map_err(invalid)?;
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let access = access_locked(&mut tx, request.identity, event.document_id).await?;
        self.deny_anonymized_guest_write(&mut tx, request.identity, event.document_id)
            .await?;
        if matches!(&event.operation, Operation::AdvanceCalculationEpoch { context } if context.reason() == CalculationReason::Explicit)
            && !access.can_edit
        {
            return Err(JournalError::Unauthorized);
        }
        let head = access.head;
        if let Some(receipt) = existing(
            self,
            &mut tx,
            event.document_id,
            event.operation_id,
            fingerprint,
            head,
        )
        .await?
        {
            tx.commit().await.map_err(error)?;
            return Ok(CommitOutcome::AlreadyCommitted(receipt));
        }
        if head != request.expected_head
            || access.document_access_epoch != request.expected_access.document_access_epoch
            || access.membership_access_epoch != request.expected_access.membership_access_epoch
        {
            return Err(JournalError::Conflict);
        }
        let updated = sqlx::query!(
            "UPDATE documents SET head_version=head_version+1 WHERE document_id=$1 AND head_version=$2 RETURNING head_version",
            uuid(event.document_id.as_bytes()),
            number(request.expected_head.get())?
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?;
        if updated.is_none() {
            return Err(JournalError::Conflict);
        }
        sqlx::query!("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let receipt = insert(self, &mut tx, event, fingerprint).await?;
        tx.commit().await.map_err(error)?;
        Ok(CommitOutcome::Committed(receipt))
    }
    async fn load_undo_target(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        target_operation_id: OperationId,
    ) -> Result<CommittedEvent, JournalError> {
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, document_id).await?;
        if !access.can_edit {
            return Err(JournalError::Unauthorized);
        }
        let target = undo_target_locked(
            self,
            &mut tx,
            document_id,
            target_operation_id,
            identity.actor_id,
            access.head,
        )
        .await?;
        tx.commit().await.map_err(error)?;
        Ok(target)
    }
    async fn load_events(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        after: DocumentVersion,
        limit: u32,
    ) -> Result<Vec<CommittedEvent>, JournalError> {
        if limit == 0 || limit > 1000 {
            return Err(JournalError::Corrupt);
        }
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, document_id).await?;
        let rows=sqlx::query!("SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 ORDER BY committed_version LIMIT $3",uuid(document_id.as_bytes()),number(after.get())?,i64::from(limit)).fetch_all(&mut *tx).await.map_err(error)?;
        let mut events = Vec::with_capacity(rows.len());
        let mut expected = after.get();
        for r in rows {
            let e = verified_event(
                self,
                StoredEventRow {
                    document: document_id,
                    committed_version: r.committed_version,
                    operation_id: r.operation_id,
                    actor_id: r.actor_id,
                    principal_kind: &r.principal_kind,
                    schema_version: r.schema_version,
                    payload: &r.payload,
                    payload_checksum: &r.payload_checksum,
                    canonical_event_hash: &r.canonical_event_hash,
                },
            )
            .await?;
            expected = expected.checked_add(1).ok_or(JournalError::Corrupt)?;
            if e.version.get() != expected {
                return Err(JournalError::Corrupt);
            }
            events.push(e)
        }
        if after > access.head
            || expected
                != access
                    .head
                    .get()
                    .min(after.get().saturating_add(u64::from(limit)))
        {
            return Err(JournalError::Corrupt);
        }
        tx.commit().await.map_err(error)?;
        Ok(events)
    }
}

impl PgDatabase {
    /// Publish a new imported document only after every prepared event has been
    /// validated against its predecessor. No partial workbook is visible.
    pub async fn create_imported(
        &self,
        identity: AuthenticatedActor,
        events: &[CommittedEvent],
    ) -> Result<CommitReceipt, JournalError> {
        self.publish_import(identity, events, None).await
    }
    pub async fn create_pxel_imported(
        &self,
        identity: AuthenticatedActor,
        events: &[CommittedEvent],
        personal: &crate::PersonalFilters,
        privacy: &crate::PrivacyRules,
        title: &str,
    ) -> Result<CommitReceipt, JournalError> {
        self.publish_import(identity, events, Some((personal, privacy, title)))
            .await
    }
    async fn publish_import(
        &self,
        identity: AuthenticatedActor,
        events: &[CommittedEvent],
        settings: Option<(&crate::PersonalFilters, &crate::PrivacyRules, &str)>,
    ) -> Result<CommitReceipt, JournalError> {
        let cells_limit = if settings.is_some() {
            MAX_CELLS
        } else {
            MAX_IMPORT_CELLS
        };
        let total_bytes_limit = if settings.is_some() {
            96 * 1024 * 1024
        } else {
            MAX_IMPORT_ENCODED_BYTES
        };
        // At most one formula and one style event per imported address, plus
        // one value batch, bounded merge events, and 64 sheet-creation events.
        if events.is_empty() || events.len() > cells_limit * 2 + 10_000 + 65 {
            return Err(JournalError::Corrupt);
        }
        let genesis = events.first().ok_or(JournalError::Corrupt)?;
        let last = events.last().ok_or(JournalError::Corrupt)?;
        let principal = EventPrincipal::User(identity.actor_id);
        let document_id = genesis.document_id;
        let mut seen = std::collections::BTreeSet::new();
        let mut candidate = Workbook::empty(document_id);
        let mut hashes = Vec::with_capacity(events.len());
        let mut imported_addresses = std::collections::BTreeSet::new();
        let mut encoded_bytes = 0_usize;
        for (index, event) in events.iter().enumerate() {
            let permitted = if index == 0 {
                matches!(event.operation, Operation::CreateWorkbook { .. })
            } else {
                matches!(
                    event.operation,
                    Operation::AddSheet { .. }
                        | Operation::SetCellValues { .. }
                        | Operation::SetCellFormula { .. }
                        | Operation::StyleWrite { .. }
                        | Operation::MergeWrite {
                            undoes: None,
                            undoes_version: None,
                            ..
                        }
                ) || (settings.is_some()
                    && matches!(
                        event.operation,
                        Operation::AxisSizeWrite { .. }
                            | Operation::SheetSettingsWrite { .. }
                            | Operation::CopySheet { .. }
                            | Operation::FrozenRowsWrite { .. }
                            | Operation::CellBatchWrite {
                                intent: CellBatchIntent::SetValues,
                                undoes: None,
                                undoes_version: None,
                                ..
                            }
                    ))
            };
            if !permitted
                || event.document_id != document_id
                || event.principal != principal
                || event.version.get()
                    != u64::try_from(index + 1).map_err(|_| JournalError::Corrupt)?
                || !seen.insert(event.operation_id)
            {
                return Err(JournalError::Corrupt);
            }
            match &event.operation {
                Operation::SetCellValues { writes } => {
                    imported_addresses.extend(writes.iter().map(|write| write.address));
                }
                Operation::SetCellFormula { address, .. } => {
                    imported_addresses.insert(*address);
                }
                Operation::StyleWrite { writes, .. } => {
                    imported_addresses.extend(writes.iter().map(|write| write.address));
                }
                Operation::CellBatchWrite {
                    content_writes,
                    style_writes,
                    ..
                } => {
                    imported_addresses.extend(content_writes.iter().map(|w| w.address));
                    imported_addresses.extend(style_writes.iter().map(|w| w.address));
                }
                _ => {}
            }
            if imported_addresses.len() > cells_limit {
                return Err(JournalError::PayloadTooLarge);
            }
            let encoded = codec::encode(event).map_err(|error| match error {
                codec::CodecError::EventTooLarge => JournalError::PayloadTooLarge,
                _ => JournalError::Corrupt,
            })?;
            encoded_bytes = encoded_bytes
                .checked_add(encoded.len())
                .ok_or(JournalError::PayloadTooLarge)?;
            if encoded_bytes > total_bytes_limit {
                return Err(JournalError::PayloadTooLarge);
            }
            candidate
                .apply_event(event)
                .map_err(|_| JournalError::Corrupt)?;
            hashes.push(codec::event_hash(event).map_err(invalid)?);
        }
        if let Some((personal, privacy, title)) = settings {
            if !crate::valid_title(title) {
                return Err(JournalError::Corrupt);
            }
            personal
                .validate_geometry(&candidate)
                .map_err(|_| JournalError::Corrupt)?;
            privacy.kinds().map_err(|_| JournalError::Corrupt)?;
        }
        let mut tx = self.begin_guarded().await.map_err(error)?;
        let member = sqlx::query!(
            "SELECT role FROM memberships WHERE workspace_id=$1 AND user_id=$2 FOR UPDATE",
            uuid(identity.workspace_id.as_bytes()),
            uuid(identity.actor_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?
        .ok_or(JournalError::Unauthorized)?;
        if member.role != "owner" && member.role != "member" {
            return Err(JournalError::Unauthorized);
        }
        let already = sqlx::query!(
            "SELECT workspace_id FROM documents WHERE document_id=$1",
            uuid(document_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?;
        if already.is_some() {
            let access = access_locked(&mut tx, identity, document_id).await?;
            let rows = sqlx::query!(
                "SELECT committed_version,operation_id,schema_version,canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version<=$2 ORDER BY committed_version",
                uuid(document_id.as_bytes()),
                number(last.version.get())?
            )
            .fetch_all(&mut *tx)
            .await
            .map_err(error)?;
            if rows.len() != events.len() {
                return Err(JournalError::IdempotencyConflict);
            }
            for (row, (event, hash)) in rows.iter().zip(events.iter().zip(&hashes)) {
                let stored_hash = self
                    .open_record(
                        document_id,
                        version(row.committed_version)?.get(),
                        "journal-hash",
                        row.operation_id,
                        u32::try_from(row.schema_version).map_err(|_| JournalError::Corrupt)?,
                        &row.canonical_event_hash,
                        32,
                    )
                    .await
                    .map_err(crypto_error)?;
                if row.committed_version != number(event.version.get())?
                    || row.operation_id != uuid(event.operation_id.as_bytes())
                    || stored_hash.as_slice() != hash
                {
                    return Err(JournalError::IdempotencyConflict);
                }
            }
            tx.commit().await.map_err(error)?;
            return Ok(CommitReceipt {
                operation_id: last.operation_id,
                committed_version: last.version,
                canonical_event_hash: *hashes.last().ok_or(JournalError::Corrupt)?,
                current_head_version: access.head,
            });
        }
        self.ensure_document_key_guarded(identity.workspace_id, document_id)
            .await
            .map_err(crypto_error)?;
        sqlx::query!(
            "INSERT INTO documents(document_id,workspace_id,head_version) VALUES($1,$2,$3)",
            uuid(document_id.as_bytes()),
            uuid(identity.workspace_id.as_bytes()),
            number(last.version.get())?
        )
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        sqlx::query!("INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'owner')",uuid(identity.workspace_id.as_bytes()),uuid(document_id.as_bytes()),uuid(identity.actor_id.as_bytes())).execute(&mut *tx).await.map_err(error)?;
        sqlx::query!("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let mut receipt = None;
        for (index, event) in events.iter().enumerate() {
            let fingerprint = if index == 0 {
                codec::genesis_fingerprint(event)
            } else {
                *hashes.get(index).ok_or(JournalError::Corrupt)?
            };
            receipt = Some(insert(self, &mut tx, event, fingerprint).await?);
        }
        if let Some((personal, privacy, title)) = settings {
            self.initial_metadata(&mut tx, document_id, title)
                .await
                .map_err(|_| JournalError::Corrupt)?;
            let user = uuid(identity.actor_id.as_bytes());
            let plain = zeroize::Zeroizing::new(
                serde_json::to_vec(personal).map_err(|_| JournalError::Corrupt)?,
            );
            let payload = self
                .seal_record_guarded(document_id, 1, "personal-view", user, 1, &plain, 128 * 1024)
                .await
                .map_err(crypto_error)?;
            sqlx::query("INSERT INTO personal_document_views(document_id,user_id,revision,payload) VALUES($1,$2,1,$3)").bind(uuid(document_id.as_bytes())).bind(user).bind(payload).execute(&mut *tx).await.map_err(error)?;
            let plain = zeroize::Zeroizing::new(
                serde_json::to_vec(privacy).map_err(|_| JournalError::Corrupt)?,
            );
            let payload = self
                .seal_record_guarded(
                    document_id,
                    1,
                    "privacy-policy",
                    uuid(document_id.as_bytes()),
                    1,
                    &plain,
                    128 * 1024,
                )
                .await
                .map_err(crypto_error)?;
            sqlx::query("INSERT INTO document_privacy_policies(document_id,revision,payload) VALUES($1,1,$2)").bind(uuid(document_id.as_bytes())).bind(payload).execute(&mut *tx).await.map_err(error)?;
        }
        tx.commit().await.map_err(error)?;
        receipt.ok_or(JournalError::Corrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct ReceiptRaceJournal {
        database: PgDatabase,
        event: std::sync::Arc<SystemCommitRequest>,
        pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
        followup: Option<std::sync::Arc<CommitRequest>>,
    }
    macro_rules! delegate_journal {
        ($name:ident ($($arg:ident: $ty:ty),*) -> $output:ty) => {
            async fn $name(&self,$($arg:$ty),*) -> Result<$output,JournalError> { self.database.$name($($arg),*).await }
        };
    }
    impl OperationJournal for ReceiptRaceJournal {
        async fn calculation_epoch_version(
            &self,
            identity: AuthenticatedActor,
            document_id: DocumentId,
            through: DocumentVersion,
        ) -> Result<Option<DocumentVersion>, JournalError> {
            self.database
                .calculation_epoch_version(identity, document_id, through)
                .await
        }
        delegate_journal!(quarantine(document:DocumentId) -> ());
        delegate_journal!(access(identity:AuthenticatedActor,document:DocumentId) -> DocumentAccess);
        delegate_journal!(undo_history(identity:AuthenticatedActor,document:DocumentId) -> spreadsheet_application::history::UndoHistory);
        delegate_journal!(create(identity:AuthenticatedActor,event:CommittedEvent) -> CommitReceipt);
        delegate_journal!(load_events(identity:AuthenticatedActor,document:DocumentId,after:DocumentVersion,limit:u32) -> Vec<CommittedEvent>);
        delegate_journal!(load_undo_target(identity:AuthenticatedActor,document:DocumentId,target:OperationId) -> CommittedEvent);
        delegate_journal!(commit(request:&CommitRequest) -> CommitOutcome);
        delegate_journal!(commit_system(request:&SystemCommitRequest) -> CommitOutcome);
        delegate_journal!(receipt(identity:AuthenticatedActor,command:&Command) -> Option<CommitReceipt>);
        async fn system_receipt(
            &self,
            identity: AuthenticatedActor,
            document: DocumentId,
            operation: OperationId,
            context: &CalculationContext,
        ) -> Result<Option<CommitReceipt>, JournalError> {
            if self
                .pending
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.database.commit_system(&self.event).await?;
                if let Some(request) = &self.followup {
                    self.database.commit(request).await?;
                }
            }
            self.database
                .system_receipt(identity, document, operation, context)
                .await
        }
    }
    #[sqlx::test(migrations = false)]
    async fn bootstrap_receipt_race_refreshes_before_returning_projection(
        pool: sqlx::PgPool,
    ) -> TestResult {
        use spreadsheet_collab::SessionDirectory;
        use std::sync::{Arc, atomic::AtomicBool};
        let (database, identity, book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let mut formula = request(&database, identity, &book, 109).await?;
        formula.command.operation = CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new("=NOW()".into())?,
        };
        formula.event.operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind("=NOW()", &book, address)?,
        };
        let authored = book.candidate(&formula.event.operation, DocumentVersion::new(2)?)?;
        formula.event.state_hash = authored.hash();
        database.commit(&formula).await?;
        let context = CalculationContext::new(
            CalculationContextId::from_bytes([110; 16])?,
            1,
            86400000,
            0,
            1,
            [111; 32],
            spreadsheet_formula::calculation::ENGINE_VERSION.into(),
            spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
            CalculationReason::Open,
        )?;
        let operation = Operation::AdvanceCalculationEpoch {
            context: context.clone(),
        };
        let expected = authored.candidate(&operation, DocumentVersion::new(3)?)?;
        let operation_id = OperationId::from_bytes([112; 16])?;
        let journal = ReceiptRaceJournal {
            followup: None,
            database: database.clone(),
            pending: Arc::new(AtomicBool::new(true)),
            event: Arc::new(SystemCommitRequest {
                identity,
                expected_access: database.access(identity, book.document_id()).await?,
                expected_head: authored.version(),
                event: CommittedEvent {
                    document_id: book.document_id(),
                    operation_id,
                    principal: EventPrincipal::System,
                    version: expected.version(),
                    operation,
                    state_hash: expected.hash(),
                },
            }),
        };
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(journal)
            .with_context_sampler(move |_| Ok((operation_id, context.clone())));
        let session = directory.session(identity, book.document_id()).await?;
        let returned = session.read(identity).await?;
        assert_eq!(returned.version().get(), 3);
        assert_eq!(returned.hash(), expected.hash());
        assert_eq!(session.head(identity).await?.get(), 3);
        assert_eq!(
            database
                .access(identity, book.document_id())
                .await?
                .head
                .get(),
            3
        );
        Ok(())
    }
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    #[sqlx::test(migrations = false)]
    async fn imported_events_publish_atomically_and_retry_exactly(
        pool: sqlx::PgPool,
    ) -> TestResult {
        use spreadsheet_domain::{
            style::{Change, StyleChange},
            style_write::prepare_style_write,
        };
        let (db, identity, _) = fixture(pool).await?;
        let document = DocumentId::from_bytes([21; 16])?;
        let sheet = SheetId::from_bytes([22; 16])?;
        let row = RowId::from_bytes([23; 16])?;
        let column = ColumnId::from_bytes([24; 16])?;
        let second_column = ColumnId::from_bytes([27; 16])?;
        let genesis_operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Imported".into(),
                rows: vec![row],
                columns: vec![column, second_column],
            },
        };
        let first =
            Workbook::empty(document).candidate(&genesis_operation, DocumentVersion::new(1)?)?;
        let genesis = CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([25; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: first.version(),
            operation: genesis_operation,
            state_hash: first.hash(),
        };
        let address = CellAddress { sheet, row, column };
        let write = |value| Operation::SetCellValues {
            writes: vec![CellWrite {
                address,
                value: CellValue::Number(value),
            }],
        };
        let content_operation = write(7.0);
        let second = first.candidate(&content_operation, DocumentVersion::new(2)?)?;
        let content = CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([26; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: second.version(),
            operation: content_operation,
            state_hash: second.hash(),
        };
        let style_operation = prepare_style_write(
            &second,
            &[address],
            &[StyleChange::FontBold(Change::Set(true))],
            StyleWriteKind::Cell,
        )?;
        let third = second.candidate(&style_operation, DocumentVersion::new(3)?)?;
        let style = CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([28; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: third.version(),
            operation: style_operation,
            state_hash: third.hash(),
        };
        let merge_operation = Operation::MergeWrite {
            sheet,
            change: spreadsheet_domain::merge::MergeChange {
                range: spreadsheet_domain::merge::MergeRange::new(row, row, column, second_column),
                merged: true,
                previous: false,
            },
            undoes: None,
            undoes_version: None,
        };
        let fourth = third.candidate(&merge_operation, DocumentVersion::new(4)?)?;
        let merge = CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([29; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: fourth.version(),
            operation: merge_operation,
            state_hash: fourth.hash(),
        };
        let events = [genesis.clone(), content.clone(), style, merge];
        let receipt = db.create_imported(identity, &events).await?;
        assert_eq!(receipt.committed_version, DocumentVersion::new(4)?);
        assert_eq!(
            db.load_events(identity, document, DocumentVersion::ZERO, 10)
                .await?,
            events
        );
        let retry = db.create_imported(identity, &events).await?;
        assert_eq!(retry.canonical_event_hash, receipt.canonical_event_hash);
        let changed_operation = write(8.0);
        let changed = first.candidate(&changed_operation, DocumentVersion::new(2)?)?;
        let changed_content = CommittedEvent {
            operation: changed_operation,
            state_hash: changed.hash(),
            ..content
        };
        assert!(matches!(
            db.create_imported(identity, &[genesis.clone(), changed_content])
                .await,
            Err(JournalError::IdempotencyConflict)
        ));
        sqlx::query("CREATE FUNCTION fail_import_style_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.committed_version=3 THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$")
            .execute(&db.pool).await?;
        sqlx::query("CREATE TRIGGER fail_import_style_event BEFORE INSERT ON document_operations FOR EACH ROW EXECUTE FUNCTION fail_import_style_event()")
            .execute(&db.pool).await?;
        let other = DocumentId::from_bytes([29; 16])?;
        let mut failed_events = events;
        let mut candidate = Workbook::empty(other);
        for event in &mut failed_events {
            event.document_id = other;
            candidate = candidate.candidate(&event.operation, event.version)?;
            event.state_hash = candidate.hash();
        }
        assert!(db.create_imported(identity, &failed_events).await.is_err());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE document_id=$1")
            .bind(uuid(other.as_bytes()))
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(count, 0);
        Ok(())
    }
    pub(super) async fn fixture(
        pool: sqlx::PgPool,
    ) -> Result<(PgDatabase, AuthenticatedActor, Workbook), Box<dyn std::error::Error>> {
        let db = PgDatabase::for_test(pool)?;
        db.migrate().await?;
        let identity = AuthenticatedActor {
            actor_id: ActorId::from_bytes([1; 16])?,
            workspace_id: WorkspaceId::from_bytes([2; 16])?,
        };
        db.issue_development_session([3; 32], identity.actor_id, identity.workspace_id)
            .await?;
        let doc = DocumentId::from_bytes([4; 16])?;
        let operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes([5; 16])?,
                name: "Test".into(),
                rows: vec![RowId::from_bytes([6; 16])?],
                columns: vec![ColumnId::from_bytes([7; 16])?],
            },
        };
        let workbook = Workbook::empty(doc).candidate(&operation, DocumentVersion::new(1)?)?;
        db.create(
            identity,
            CommittedEvent {
                document_id: doc,
                operation_id: OperationId::from_bytes([8; 16])?,
                principal: EventPrincipal::User(identity.actor_id),
                version: workbook.version(),
                operation,
                state_hash: workbook.hash(),
            },
        )
        .await?;
        Ok((db, identity, workbook))
    }
    async fn request(
        db: &PgDatabase,
        identity: AuthenticatedActor,
        w: &Workbook,
        id: u8,
    ) -> Result<CommitRequest, Box<dyn std::error::Error>> {
        let operation = Operation::SetCellValue {
            address: CellAddress {
                sheet: SheetId::from_bytes([5; 16])?,
                row: RowId::from_bytes([6; 16])?,
                column: ColumnId::from_bytes([7; 16])?,
            },
            value: CellValue::Text("retained".into()),
        };
        let next = DocumentVersion::new(w.version().get() + 1)?;
        let candidate = w.candidate(&operation, next)?;
        let operation_id = OperationId::from_bytes([id; 16])?;
        Ok(CommitRequest {
            inverse_projection: None,
            identity,
            expected_head: w.version(),
            expected_access: db.access(identity, w.document_id()).await?,
            command: Command {
                operation_id,
                document_id: w.document_id(),
                actor_id: identity.actor_id,
                base_version: w.version(),
                client_sequence: 1,
                operation: match &operation {
                    Operation::SetCellValue { address, value } => CommandPayload::SetCellValue {
                        address: *address,
                        value: value.clone(),
                    },
                    _ => return Err("unexpected fixture".into()),
                },
            },
            event: CommittedEvent {
                document_id: w.document_id(),
                operation_id,
                principal: EventPrincipal::User(identity.actor_id),
                version: next,
                operation,
                state_hash: candidate.hash(),
            },
        })
    }
    async fn guarded_write_request(
        db: &PgDatabase,
        identity: AuthenticatedActor,
        book: &Workbook,
        id: u8,
        content: Option<CellContent>,
        previous: Option<CellContent>,
        undoes: Option<(OperationId, DocumentVersion)>,
    ) -> Result<CommitRequest, Box<dyn std::error::Error>> {
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let (kind, undoes_id, undoes_version, payload) = match (&content, undoes) {
            (_, Some((target, target_version))) => (
                None,
                Some(target),
                Some(target_version),
                CommandPayload::UndoCell {
                    target_operation_id: target,
                },
            ),
            (Some(CellContent::Value(value)), None) => (
                Some(CellWriteKind::SetValue),
                None,
                None,
                CommandPayload::SetCellValue {
                    address,
                    value: value.clone(),
                },
            ),
            (None, None) => (
                Some(CellWriteKind::Clear),
                None,
                None,
                CommandPayload::ClearCell { address },
            ),
            (Some(CellContent::Formula(formula)), None) => (
                Some(CellWriteKind::SetFormula),
                None,
                None,
                CommandPayload::SetCellFormula {
                    address,
                    source: FormulaSource::new(formula.source().to_owned())?,
                },
            ),
        };
        let operation = Operation::CellWrite {
            address,
            content,
            previous,
            kind,
            undoes: undoes_id,
            undoes_version,
        };
        let next = DocumentVersion::new(book.version().get() + 1)?;
        let candidate = book.candidate(&operation, next)?;
        let operation_id = OperationId::from_bytes([id; 16])?;
        Ok(CommitRequest {
            inverse_projection: None,
            identity,
            expected_head: book.version(),
            expected_access: db.access(identity, book.document_id()).await?,
            command: Command {
                operation_id,
                document_id: book.document_id(),
                actor_id: identity.actor_id,
                base_version: book.version(),
                client_sequence: 1,
                operation: payload,
            },
            event: CommittedEvent {
                document_id: book.document_id(),
                operation_id,
                principal: EventPrincipal::User(identity.actor_id),
                version: next,
                operation,
                state_hash: candidate.hash(),
            },
        })
    }
    #[sqlx::test(migrations = false)]
    async fn same_cell_history_undo_redo_chain_replays_exactly(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, mut book) = fixture(pool).await?;
        let number = |value| Some(CellContent::Value(CellValue::Number(value)));
        let actions = [
            (121, number(1.), None, None),
            (122, number(2.), number(1.), None),
            (123, number(1.), number(2.), Some((122, 3))),
            (124, None, number(1.), Some((121, 2))),
            (125, number(1.), None, Some((124, 5))),
            (126, number(2.), number(1.), Some((123, 4))),
            (127, number(1.), number(2.), Some((126, 7))),
            (128, None, number(1.), Some((125, 6))),
        ];
        for (id, content, previous, target) in actions {
            let undoes = target
                .map(|(id, version)| {
                    Ok::<_, DomainError>((
                        OperationId::from_bytes([id; 16])?,
                        DocumentVersion::new(version)?,
                    ))
                })
                .transpose()?;
            if let Some((target_id, target_version)) = undoes {
                let loaded = db
                    .load_undo_target(identity, book.document_id(), target_id)
                    .await?;
                assert_eq!(loaded.version, target_version);
            }
            let request =
                guarded_write_request(&db, identity, &book, id, content, previous, undoes).await?;
            assert!(
                matches!(db.commit(&request).await?, CommitOutcome::Committed(_)),
                "action {id}"
            );
            book.apply_event(&request.event)?;
            let hash = book.hash();
            assert!(matches!(
                db.commit(&request).await?,
                CommitOutcome::AlreadyCommitted(_)
            ));
            assert_eq!(book.hash(), hash);
        }
        let mut replay = Workbook::empty(book.document_id());
        for event in db
            .load_events(identity, book.document_id(), DocumentVersion::ZERO, 1000)
            .await?
        {
            replay.apply_event(&event)?;
        }
        assert_eq!(replay.hash(), book.hash());
        assert_eq!(replay.version().get(), 9);
        let wrong_version = guarded_write_request(
            &db,
            identity,
            &book,
            129,
            number(1.),
            None,
            Some((
                OperationId::from_bytes([128; 16])?,
                DocumentVersion::new(8)?,
            )),
        )
        .await?;
        assert!(matches!(
            db.commit(&wrong_version).await,
            Err(JournalError::UndoConflict)
        ));
        assert_eq!(
            db.access(identity, book.document_id()).await?.head,
            book.version()
        );
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn formula_undo_transaction_rejects_axis_changes_without_actor(
        pool: sqlx::PgPool,
    ) -> TestResult {
        use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
        let (db, identity, book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let formula = Some(CellContent::Formula(Formula::new(
            "=42".into(),
            "=42".into(),
            BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.)])?,
            address,
            book.version(),
        )?));
        let original =
            guarded_write_request(&db, identity, &book, 111, formula.clone(), None, None).await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        let after = book.candidate(&original.event.operation, original.event.version)?;
        let rows = vec![RowId::from_bytes([112; 16])?];
        let operation = Operation::ExtendRows {
            sheet: address.sheet,
            rows: rows.clone(),
        };
        let next = DocumentVersion::new(3)?;
        let extended = after.candidate(&operation, next)?;
        let operation_id = OperationId::from_bytes([113; 16])?;
        let extension = CommitRequest {
            inverse_projection: None,
            identity,
            expected_head: after.version(),
            expected_access: db.access(identity, book.document_id()).await?,
            command: Command {
                operation_id,
                document_id: book.document_id(),
                actor_id: identity.actor_id,
                base_version: after.version(),
                client_sequence: 2,
                operation: CommandPayload::ExtendRows {
                    sheet: address.sheet,
                    rows,
                },
            },
            event: CommittedEvent {
                document_id: book.document_id(),
                operation_id,
                principal: EventPrincipal::User(identity.actor_id),
                version: next,
                operation,
                state_hash: extended.hash(),
            },
        };
        assert!(matches!(
            db.commit(&extension).await?,
            CommitOutcome::Committed(_)
        ));
        let inverse = guarded_write_request(
            &db,
            identity,
            &extended,
            114,
            None,
            formula,
            Some((original.event.operation_id, original.event.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&inverse).await,
            Err(JournalError::NotUndoable)
        ));
        assert_eq!(db.access(identity, book.document_id()).await?.head, next);
        assert!(db.receipt(identity, &inverse.command).await?.is_none());
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn authored_formula_undo_is_durable_and_equal_rewrite_conflicts(
        pool: sqlx::PgPool,
    ) -> TestResult {
        use spreadsheet_domain::formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId};
        let (db, identity, book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let formula = Some(CellContent::Formula(Formula::new(
            "=42".into(),
            "=42".into(),
            BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.)])?,
            address,
            book.version(),
        )?));
        let original =
            guarded_write_request(&db, identity, &book, 101, formula.clone(), None, None).await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        let target = db
            .load_undo_target(identity, book.document_id(), original.event.operation_id)
            .await?;
        assert_eq!(target.operation, original.event.operation);
        let after = book.candidate(&target.operation, target.version)?;
        let undo = guarded_write_request(
            &db,
            identity,
            &after,
            102,
            None,
            formula.clone(),
            Some((target.operation_id, target.version)),
        )
        .await?;
        let rewrite = guarded_write_request(
            &db,
            identity,
            &after,
            103,
            formula.clone(),
            formula.clone(),
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&rewrite).await?,
            CommitOutcome::Committed(_)
        ));
        let after_rewrite = after.candidate(&rewrite.event.operation, rewrite.event.version)?;
        let stale_inverse = guarded_write_request(
            &db,
            identity,
            &after_rewrite,
            104,
            None,
            formula.clone(),
            Some((target.operation_id, target.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&stale_inverse).await,
            Err(JournalError::UndoConflict)
        ));
        let inverse = guarded_write_request(
            &db,
            identity,
            &after_rewrite,
            105,
            formula.clone(),
            formula,
            Some((rewrite.event.operation_id, rewrite.event.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&inverse).await?,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            db.commit(&inverse).await?,
            CommitOutcome::AlreadyCommitted(_)
        ));
        assert!(db.receipt(identity, &undo.command).await?.is_none());
        let mut replay = Workbook::empty(book.document_id());
        for event in db
            .load_events(identity, book.document_id(), DocumentVersion::ZERO, 1000)
            .await?
        {
            replay.apply_event(&event)?;
        }
        assert_eq!(replay.hash(), inverse.event.state_hash);
        assert_eq!(replay.version().get(), 4);
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn guarded_undo_commits_new_version_and_replays_after_retry(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let original = guarded_write_request(
            &db,
            identity,
            &book,
            81,
            Some(CellContent::Value(CellValue::Text("first".into()))),
            None,
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        let target = db
            .load_undo_target(identity, book.document_id(), original.event.operation_id)
            .await?;
        assert_eq!(target.version.get(), 2);
        let after_original = book.candidate(&original.event.operation, target.version)?;
        let undo = guarded_write_request(
            &db,
            identity,
            &after_original,
            82,
            None,
            Some(CellContent::Value(CellValue::Text("first".into()))),
            Some((original.event.operation_id, target.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&undo).await?,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            db.commit(&undo).await?,
            CommitOutcome::AlreadyCommitted(_)
        ));
        let mut reused_id = undo.command.clone();
        reused_id.operation = CommandPayload::UndoCell {
            target_operation_id: OperationId::from_bytes([83; 16])?,
        };
        assert!(matches!(
            db.receipt(identity, &reused_id).await,
            Err(JournalError::IdempotencyConflict)
        ));
        let mut replay = Workbook::empty(book.document_id());
        for event in db
            .load_events(identity, book.document_id(), DocumentVersion::ZERO, 1000)
            .await?
        {
            replay.apply_event(&event)?;
        }
        assert_eq!(replay.hash(), undo.event.state_hash);
        assert_eq!(replay.version().get(), 3);
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn blank_value_is_distinct_from_clear_in_schema_eleven(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let mut blank = guarded_write_request(&db, identity, &book, 87, None, None, None).await?;
        if let Operation::CellWrite { kind, .. } = &mut blank.event.operation {
            *kind = Some(CellWriteKind::SetValue);
        }
        if let CommandPayload::ClearCell { address } = blank.command.operation {
            blank.command.operation = CommandPayload::SetCellValue {
                address,
                value: CellValue::Blank,
            };
        } else {
            return Err("expected clear fixture".into());
        }
        assert!(matches!(
            db.commit(&blank).await?,
            CommitOutcome::Committed(_)
        ));
        let event = db
            .load_undo_target(identity, book.document_id(), blank.event.operation_id)
            .await?;
        assert!(matches!(
            event.operation,
            Operation::CellWrite {
                kind: Some(CellWriteKind::SetValue),
                content: None,
                ..
            }
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn undo_tail_guard_rejects_write_away_and_back_to_same_value(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let original = guarded_write_request(
            &db,
            identity,
            &book,
            83,
            Some(CellContent::Value(CellValue::Text("first".into()))),
            None,
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        let after_original = book.candidate(&original.event.operation, original.event.version)?;
        let away = guarded_write_request(
            &db,
            identity,
            &after_original,
            84,
            Some(CellContent::Value(CellValue::Text("second".into()))),
            Some(CellContent::Value(CellValue::Text("first".into()))),
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&away).await?,
            CommitOutcome::Committed(_)
        ));
        let after_away = after_original.candidate(&away.event.operation, away.event.version)?;
        let back = guarded_write_request(
            &db,
            identity,
            &after_away,
            85,
            Some(CellContent::Value(CellValue::Text("first".into()))),
            Some(CellContent::Value(CellValue::Text("second".into()))),
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&back).await?,
            CommitOutcome::Committed(_)
        ));
        let after_back = after_away.candidate(&back.event.operation, back.event.version)?;
        let undo = guarded_write_request(
            &db,
            identity,
            &after_back,
            86,
            None,
            Some(CellContent::Value(CellValue::Text("first".into()))),
            Some((original.event.operation_id, original.event.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&undo).await,
            Err(JournalError::UndoConflict)
        ));
        assert_eq!(db.access(identity, book.document_id()).await?.head.get(), 4);
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn undo_rejects_foreign_target_and_foreign_same_value_write(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, owner, book) = fixture(pool).await?;
        let original = guarded_write_request(
            &db,
            owner,
            &book,
            88,
            Some(CellContent::Value(CellValue::Text("same".into()))),
            None,
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        let other = AuthenticatedActor {
            actor_id: ActorId::from_bytes([89; 16])?,
            workspace_id: owner.workspace_id,
        };
        sqlx::query!(
            "INSERT INTO users(user_id) VALUES($1)",
            uuid(other.actor_id.as_bytes())
        )
        .execute(&db.pool)
        .await?;
        sqlx::query!(
            "INSERT INTO memberships(workspace_id,user_id,role) VALUES($1,$2,'member')",
            uuid(other.workspace_id.as_bytes()),
            uuid(other.actor_id.as_bytes())
        )
        .execute(&db.pool)
        .await?;
        sqlx::query!(
            "INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES($1,$2,$3,'editor')",
            uuid(other.workspace_id.as_bytes()),
            uuid(book.document_id().as_bytes()),
            uuid(other.actor_id.as_bytes())
        )
        .execute(&db.pool)
        .await?;
        assert!(matches!(
            db.load_undo_target(other, book.document_id(), original.event.operation_id)
                .await,
            Err(JournalError::NotUndoable)
        ));
        let after_original = book.candidate(&original.event.operation, original.event.version)?;
        let same = guarded_write_request(
            &db,
            other,
            &after_original,
            91,
            Some(CellContent::Value(CellValue::Text("same".into()))),
            Some(CellContent::Value(CellValue::Text("same".into()))),
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&same).await?,
            CommitOutcome::Committed(_)
        ));
        let after_same = after_original.candidate(&same.event.operation, same.event.version)?;
        let foreign_inverse = guarded_write_request(
            &db,
            other,
            &after_same,
            95,
            Some(CellContent::Value(CellValue::Text("same".into()))),
            Some(CellContent::Value(CellValue::Text("same".into()))),
            Some((same.event.operation_id, same.event.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&foreign_inverse).await?,
            CommitOutcome::Committed(_)
        ));
        let after_same = after_same.candidate(
            &foreign_inverse.event.operation,
            foreign_inverse.event.version,
        )?;
        let undo = guarded_write_request(
            &db,
            owner,
            &after_same,
            92,
            None,
            Some(CellContent::Value(CellValue::Text("same".into()))),
            Some((original.event.operation_id, original.event.version)),
        )
        .await?;
        assert!(matches!(
            db.commit(&undo).await,
            Err(JournalError::UndoConflict)
        ));
        Ok(())
    }
    /// Run with `cargo test -p spreadsheet-storage cancelled_cell_history_scan_timing -- --ignored --nocapture`.
    /// Real database sample of inverse-pair reduction; not a latency percentile.
    #[sqlx::test(migrations = false)]
    #[ignore]
    async fn cancelled_cell_history_scan_timing(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, mut book) = fixture(pool).await?;
        let settings: (String, String, String) = sqlx::query_as(
            "SELECT current_setting('synchronous_commit'), current_setting('fsync'), current_setting('full_page_writes')",
        ).fetch_one(&db.pool).await?;
        assert_eq!(settings, ("on".into(), "on".into(), "on".into()));
        let value = |n| Some(CellContent::Value(CellValue::Number(n)));
        let original =
            guarded_write_request(&db, identity, &book, 131, value(7.), None, None).await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        book.apply_event(&original.event)?;
        const PAIRS: u128 = 500;
        for index in 0..PAIRS {
            let mut write =
                guarded_write_request(&db, identity, &book, 132, value(8.), value(7.), None)
                    .await?;
            let operation_id = OperationId::from_bytes((30_000 + index * 2).to_be_bytes())?;
            write.command.operation_id = operation_id;
            write.event.operation_id = operation_id;
            assert!(matches!(
                db.commit(&write).await?,
                CommitOutcome::Committed(_)
            ));
            book.apply_event(&write.event)?;
            let mut inverse = guarded_write_request(
                &db,
                identity,
                &book,
                133,
                value(7.),
                value(8.),
                Some((operation_id, write.event.version)),
            )
            .await?;
            let inverse_id = OperationId::from_bytes((30_001 + index * 2).to_be_bytes())?;
            inverse.command.operation_id = inverse_id;
            inverse.event.operation_id = inverse_id;
            assert!(matches!(
                db.commit(&inverse).await?,
                CommitOutcome::Committed(_)
            ));
            book.apply_event(&inverse.event)?;
        }
        let inverse = guarded_write_request(
            &db,
            identity,
            &book,
            134,
            None,
            value(7.),
            Some((original.event.operation_id, original.event.version)),
        )
        .await?;
        let started = std::time::Instant::now();
        assert!(matches!(
            db.commit(&inverse).await?,
            CommitOutcome::Committed(_)
        ));
        let elapsed = started.elapsed();
        book.apply_event(&inverse.event)?;
        let mut replay = Workbook::empty(book.document_id());
        while replay.version() < book.version() {
            let events = db
                .load_events(identity, book.document_id(), replay.version(), 1000)
                .await?;
            if events.is_empty() {
                return Err("history benchmark replay gap".into());
            }
            for event in events {
                replay.apply_event(&event)?;
            }
        }
        assert_eq!(book.hash(), replay.hash());
        eprintln!(
            "cancelled_cell_history_scan_timing: pairs={PAIRS}, tail_events={}, commit_wall_ms={:.3}",
            PAIRS * 2,
            elapsed.as_secs_f64() * 1000.
        );
        Ok(())
    }
    /// Run with `cargo test -p spreadsheet-storage undo_tail_scan_timing -- --ignored --nocapture`.
    /// Reports one real PostgreSQL commit sample; it is not a latency percentile.
    #[sqlx::test(migrations = false)]
    #[ignore]
    async fn undo_tail_scan_timing(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, mut book) = fixture(pool).await?;
        let original = guarded_write_request(
            &db,
            identity,
            &book,
            93,
            Some(CellContent::Value(CellValue::Text("target".into()))),
            None,
            None,
        )
        .await?;
        assert!(matches!(
            db.commit(&original).await?,
            CommitOutcome::Committed(_)
        ));
        book.apply_event(&original.event)?;
        const TAIL_EVENTS: u128 = 1_000;
        for index in 0..TAIL_EVENTS {
            let row = RowId::from_bytes((10_000 + index).to_be_bytes())?;
            let operation = Operation::ExtendRows {
                sheet: SheetId::from_bytes([5; 16])?,
                rows: vec![row],
            };
            let next = DocumentVersion::new(book.version().get() + 1)?;
            let candidate = book.candidate(&operation, next)?;
            let operation_id = OperationId::from_bytes((20_000 + index).to_be_bytes())?;
            let request = CommitRequest {
                inverse_projection: None,
                identity,
                expected_head: book.version(),
                expected_access: db.access(identity, book.document_id()).await?,
                command: Command {
                    operation_id,
                    document_id: book.document_id(),
                    actor_id: identity.actor_id,
                    base_version: book.version(),
                    client_sequence: index as u64 + 2,
                    operation: CommandPayload::ExtendRows {
                        sheet: SheetId::from_bytes([5; 16])?,
                        rows: vec![row],
                    },
                },
                event: CommittedEvent {
                    document_id: book.document_id(),
                    operation_id,
                    principal: EventPrincipal::User(identity.actor_id),
                    version: next,
                    operation,
                    state_hash: candidate.hash(),
                },
            };
            assert!(matches!(
                db.commit(&request).await?,
                CommitOutcome::Committed(_)
            ));
            book = candidate;
        }
        let undo = guarded_write_request(
            &db,
            identity,
            &book,
            94,
            None,
            Some(CellContent::Value(CellValue::Text("target".into()))),
            Some((original.event.operation_id, original.event.version)),
        )
        .await?;
        let started = std::time::Instant::now();
        assert!(matches!(
            db.commit(&undo).await?,
            CommitOutcome::Committed(_)
        ));
        eprintln!(
            "undo_tail_scan_timing: tail_events={TAIL_EVENTS}, commit_wall_ms={:.3}",
            started.elapsed().as_secs_f64() * 1_000.0
        );
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn journal_payload_fingerprints_and_hashes_are_authenticated_ciphertext(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let req = request(&db, identity, &book, 29).await?;
        db.commit(&req).await?;
        let row = sqlx::query!("SELECT payload,payload_checksum,command_fingerprint,canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version=2",uuid(book.document_id().as_bytes())).fetch_one(&db.pool).await?;
        assert!(
            !row.payload
                .windows(b"retained".len())
                .any(|bytes| bytes == b"retained")
        );
        assert!(codec::decode(&row.payload).is_err());
        assert_eq!(
            row.command_fingerprint.len(),
            32 + crate::crypto::ENVELOPE_OVERHEAD
        );
        assert_eq!(
            row.canonical_event_hash.len(),
            32 + crate::crypto::ENVELOPE_OVERHEAD
        );
        assert_ne!(
            row.command_fingerprint.as_slice(),
            codec::command_fingerprint(&req.command)?.as_slice()
        );
        assert_ne!(
            row.canonical_event_hash.as_slice(),
            codec::event_hash(&req.event)?.as_slice()
        );
        assert_eq!(
            codec::checksum(&row.payload).as_slice(),
            row.payload_checksum
        );
        let events = db
            .load_events(identity, book.document_id(), DocumentVersion::ZERO, 10)
            .await?;
        assert_eq!(events.last(), Some(&req.event));
        // Isolated administrator simulates an altered ciphertext with a matching
        // outer checksum. AEAD must still reject before codec/publication.
        let mut changed = row.payload;
        let last = changed.last_mut().ok_or("empty ciphertext")?;
        *last ^= 1;
        sqlx::raw_sql(
            "ALTER TABLE document_operations DISABLE TRIGGER document_operations_immutable",
        )
        .execute(&db.pool)
        .await?;
        sqlx::query("UPDATE document_operations SET payload=$1,payload_checksum=$2 WHERE document_id=$3 AND committed_version=2").bind(&changed).bind(codec::checksum(&changed).as_slice()).bind(uuid(book.document_id().as_bytes())).execute(&db.pool).await?;
        sqlx::raw_sql(
            "ALTER TABLE document_operations ENABLE TRIGGER document_operations_immutable",
        )
        .execute(&db.pool)
        .await?;
        assert!(matches!(
            db.load_events(identity, book.document_id(), DocumentVersion::ZERO, 10)
                .await,
            Err(JournalError::Corrupt)
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn retry_100_times_is_one_event_and_replay_survives_new_adapter(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let r = request(&db, identity, &w, 9).await?;
        assert!(matches!(db.commit(&r).await?, CommitOutcome::Committed(_)));
        for _ in 0..100 {
            assert!(matches!(
                db.commit(&r).await?,
                CommitOutcome::AlreadyCommitted(_)
            ));
        }
        let restarted = PgDatabase {
            pool: db.pool.clone(),
            ..db.clone()
        };
        let events = restarted
            .load_events(identity, w.document_id(), DocumentVersion::ZERO, 1000)
            .await?;
        assert_eq!(events.len(), 2);
        let mut restored = Workbook::empty(w.document_id());
        for event in &events {
            restored.apply_event(event)?
        }
        assert_eq!(restored.hash(), r.event.state_hash);
        let receipt = db
            .receipt(identity, &r.command)
            .await?
            .ok_or("missing receipt")?;
        assert_eq!(receipt.committed_version.get(), 2);
        let mut changed = r;
        changed.command.client_sequence = 2;
        assert!(matches!(
            db.commit(&changed).await,
            Err(JournalError::IdempotencyConflict)
        ));
        assert!(matches!(
            db.receipt(identity, &changed.command).await,
            Err(JournalError::IdempotencyConflict)
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn concurrent_expected_head_writers_have_one_winner(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let a = request(&db, identity, &w, 9).await?;
        let b = request(&db, identity, &w, 10).await?;
        let (a, b) = tokio::join!(db.commit(&a), db.commit(&b));
        assert!(matches!(
            (&a, &b),
            (Ok(CommitOutcome::Committed(_)), Err(JournalError::Conflict))
                | (Err(JournalError::Conflict), Ok(CommitOutcome::Committed(_)))
        ));
        assert_eq!(db.access(identity, w.document_id()).await?.head.get(), 2);
        assert_eq!(
            db.load_events(identity, w.document_id(), DocumentVersion::ZERO, 1000)
                .await?
                .len(),
            2
        );
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn authorization_precedes_duplicate_and_epoch_change_requires_retry(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let r = request(&db, identity, &w, 9).await?;
        sqlx::raw_sql("UPDATE documents SET access_epoch=access_epoch+1")
            .execute(&db.pool)
            .await?;
        assert!(matches!(db.commit(&r).await, Err(JournalError::Conflict)));
        let r = request(&db, identity, &w, 9).await?;
        db.commit(&r).await?;
        sqlx::raw_sql("UPDATE document_permissions SET role='viewer'")
            .execute(&db.pool)
            .await?;
        assert!(matches!(
            db.commit(&r).await,
            Err(JournalError::Unauthorized)
        ));
        sqlx::raw_sql("DELETE FROM document_permissions")
            .execute(&db.pool)
            .await?;
        assert!(matches!(
            db.receipt(identity, &r.command).await,
            Err(JournalError::Unauthorized)
        ));
        let other = AuthenticatedActor {
            actor_id: ActorId::from_bytes([11; 16])?,
            workspace_id: WorkspaceId::from_bytes([12; 16])?,
        };
        db.issue_development_session([13; 32], other.actor_id, other.workspace_id)
            .await?;
        assert!(matches!(
            db.load_events(other, w.document_id(), DocumentVersion::ZERO, 1000)
                .await,
            Err(JournalError::NotFound)
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn failure_after_head_update_rolls_back_entire_transaction(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let r = request(&db, identity, &w, 9).await?;
        sqlx::raw_sql("CREATE FUNCTION fail_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test injected failure'; END $$; CREATE TRIGGER injected_failure AFTER INSERT ON document_operations FOR EACH ROW EXECUTE FUNCTION fail_event();").execute(&db.pool).await?;
        assert!(matches!(
            db.commit(&r).await,
            Err(JournalError::Unavailable)
        ));
        assert_eq!(db.access(identity, w.document_id()).await?.head.get(), 1);
        assert_eq!(
            db.load_events(identity, w.document_id(), DocumentVersion::ZERO, 1000)
                .await?
                .len(),
            1
        );
        sqlx::raw_sql("DROP TRIGGER injected_failure ON document_operations")
            .execute(&db.pool)
            .await?;
        assert!(matches!(db.commit(&r).await?, CommitOutcome::Committed(_)));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn append_only_and_checksum_corruption_are_detected(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        assert!(
            sqlx::raw_sql("DELETE FROM document_operations")
                .execute(&db.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::raw_sql("UPDATE document_operations SET schema_version=2")
                .execute(&db.pool)
                .await
                .is_err()
        );
        // Only this isolated superuser test bypasses immutability to emulate damaged storage.
        sqlx::raw_sql("ALTER TABLE document_operations DISABLE TRIGGER document_operations_immutable; UPDATE document_operations SET payload_checksum=decode(repeat('00',32),'hex'); ALTER TABLE document_operations ENABLE TRIGGER document_operations_immutable;").execute(&db.pool).await?;
        assert!(matches!(
            db.load_events(identity, w.document_id(), DocumentVersion::ZERO, 1000)
                .await,
            Err(JournalError::Corrupt)
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn create_retry_uses_client_intent_and_session_survives_adapter_restart(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let mut genesis = db
            .load_events(identity, w.document_id(), DocumentVersion::ZERO, 1)
            .await?
            .remove(0);
        if let Operation::CreateWorkbook { sheet } = &mut genesis.operation {
            sheet.rows = vec![RowId::from_bytes([99; 16])?]
        }
        genesis.state_hash = Workbook::empty(w.document_id())
            .candidate(&genesis.operation, genesis.version)?
            .hash();
        assert_eq!(
            db.create(identity, genesis).await?.committed_version.get(),
            1
        );
        let restarted = PgDatabase {
            pool: db.pool.clone(),
            ..db.clone()
        };
        let session = restarted
            .find_development_session([3; 32])
            .await?
            .ok_or("session lost")?;
        assert_eq!(session.identity, identity);
        assert!(session.remaining_seconds <= 3600);
        assert!(restarted.find_development_session([4; 32]).await?.is_none());
        let stored = sqlx::query!("SELECT token_hash FROM development_sessions")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(stored.token_hash, vec![3; 32]);
        db.quarantine(w.document_id()).await?;
        assert!(matches!(
            db.access(identity, w.document_id()).await,
            Err(JournalError::Unavailable)
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn held_document_lock_times_out_and_commit_recovers(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let request = request(&db, identity, &w, 9).await?;
        let bounded = PgDatabase {
            pool: sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect_with(crate::postgres::bounded_options(
                    (*db.pool.connect_options()).clone(),
                ))
                .await?,
            ..db.clone()
        };
        let settings=sqlx::query!(r#"SELECT current_setting('statement_timeout') AS "statement!", current_setting('lock_timeout') AS "lock!", current_setting('idle_in_transaction_session_timeout') AS "idle!""#)
            .fetch_one(&bounded.pool).await?;
        assert_eq!(
            (
                settings.statement.as_str(),
                settings.lock.as_str(),
                settings.idle.as_str()
            ),
            ("5s", "2s", "10s")
        );
        let mut held = db.pool.begin().await?;
        sqlx::query!(
            "SELECT document_id FROM documents WHERE document_id=$1 FOR UPDATE",
            uuid(w.document_id().as_bytes())
        )
        .fetch_one(&mut *held)
        .await?;
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), bounded.commit(&request))
                .await?;
        assert!(matches!(result, Err(JournalError::Conflict)));
        held.rollback().await?;
        assert!(matches!(
            bounded.commit(&request).await?,
            CommitOutcome::Committed(_)
        ));
        assert_eq!(
            bounded.access(identity, w.document_id()).await?.head.get(),
            2
        );
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn named_development_account_retains_document_after_session_expiry(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        sqlx::raw_sql("UPDATE development_accounts SET account='local'; UPDATE development_sessions SET expires_at=now()-interval '1 second'")
            .execute(&db.pool).await?;
        assert!(db.find_development_session([3; 32]).await?.is_none());
        let actor = ActorId::from_bytes([20; 16])?;
        let workspace = WorkspaceId::from_bytes([21; 16])?;
        let renewed = db
            .issue_development_account_session([22; 32], actor, workspace, "local")
            .await?;
        assert_eq!(renewed.identity, identity);
        assert_eq!(
            db.access(renewed.identity, w.document_id())
                .await?
                .head
                .get(),
            1
        );
        assert_eq!(
            db.find_development_session([22; 32])
                .await?
                .ok_or("missing renewed session")?
                .identity,
            identity
        );
        let other = db
            .issue_development_account_session([23; 32], actor, workspace, "other_account")
            .await?;
        assert_ne!(other.identity, identity);
        assert!(matches!(
            db.access(other.identity, w.document_id()).await,
            Err(JournalError::NotFound)
        ));
        for alias in [
            "",
            "space name",
            "../path",
            "русский",
            "123456789012345678901234567890123",
        ] {
            assert!(matches!(
                db.issue_development_account_session([24; 32], actor, workspace, alias)
                    .await,
                Err(crate::DatabaseError::InvalidConfiguration)
            ));
        }
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn formula_commit_persists_bound_ast_replays_after_restart_and_deduplicates(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, w) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let formula = spreadsheet_formula::binder::bind("=SUM(1,2)*3", &w, address)?;
        let semantic_hash = formula.semantic_hash();
        let source = FormulaSource::new(formula.source().to_owned())?;
        let operation = Operation::SetCellFormula { address, formula };
        let next = DocumentVersion::new(2)?;
        let candidate = w.candidate(&operation, next)?;
        let operation_id = OperationId::from_bytes([40; 16])?;
        let request = CommitRequest {
            inverse_projection: None,
            identity,
            expected_head: w.version(),
            expected_access: db.access(identity, w.document_id()).await?,
            command: Command {
                operation_id,
                document_id: w.document_id(),
                actor_id: identity.actor_id,
                base_version: w.version(),
                client_sequence: 1,
                operation: CommandPayload::SetCellFormula { address, source },
            },
            event: CommittedEvent {
                document_id: w.document_id(),
                operation_id,
                principal: EventPrincipal::User(identity.actor_id),
                version: next,
                operation,
                state_hash: candidate.hash(),
            },
        };
        assert!(matches!(
            db.commit(&request).await?,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            db.commit(&request).await?,
            CommitOutcome::AlreadyCommitted(_)
        ));
        let metadata=sqlx::query!("SELECT schema_version FROM document_operations WHERE document_id=$1 AND operation_id=$2",uuid(w.document_id().as_bytes()),uuid(operation_id.as_bytes())).fetch_one(&db.pool).await?;
        assert_eq!(metadata.schema_version, 2);
        let restarted = PgDatabase {
            pool: db.pool.clone(),
            ..db.clone()
        };
        let events = restarted
            .load_events(identity, w.document_id(), DocumentVersion::ZERO, 1000)
            .await?;
        assert_eq!(events.len(), 2);
        let mut restored = Workbook::empty(w.document_id());
        for event in events {
            restored.apply_event(&event)?
        }
        assert_eq!(restored.hash(), candidate.hash());
        let stored = restored
            .sheets()
            .first()
            .and_then(|s| s.cells().get(&address))
            .ok_or("missing formula")?;
        if let CellContent::Formula(formula) = stored {
            assert_eq!(formula.source(), "=SUM(1,2)*3");
            assert_eq!(formula.semantic_hash(), semantic_hash);
            assert_eq!(
                spreadsheet_formula::bound::evaluate_bound(
                    formula,
                    &restored,
                    &std::collections::BTreeMap::new(),
                    &mut spreadsheet_formula::EvaluationBudget::default()
                )?,
                spreadsheet_formula::Value::Number(9.0)
            );
        } else {
            return Err("formula changed to a literal".into());
        }
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn system_epoch_is_durable_idempotent_and_has_no_user_actor(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let context = CalculationContext::new(
            CalculationContextId::from_bytes([41; 16])?,
            1,
            1_700_000_000_000,
            180,
            19_676,
            [42; 32],
            "pxel-formula-0.1.0".into(),
            "pxel-excel-subset-v1".into(),
            CalculationReason::Explicit,
        )?;
        let operation = Operation::AdvanceCalculationEpoch { context };
        let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
        let request = SystemCommitRequest {
            identity,
            expected_access: db.access(identity, book.document_id()).await?,
            expected_head: book.version(),
            event: CommittedEvent {
                document_id: book.document_id(),
                operation_id: OperationId::from_bytes([43; 16])?,
                principal: EventPrincipal::System,
                version: candidate.version(),
                operation,
                state_hash: candidate.hash(),
            },
        };
        assert!(matches!(
            db.commit_system(&request).await?,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            db.commit_system(&request).await?,
            CommitOutcome::AlreadyCommitted(_)
        ));
        let metadata = sqlx::query!(
            "SELECT actor_id, principal_kind, schema_version FROM document_operations WHERE document_id=$1 AND operation_id=$2",
            uuid(book.document_id().as_bytes()),
            uuid(request.event.operation_id.as_bytes())
        )
        .fetch_one(&db.pool)
        .await?;
        assert!(metadata.actor_id.is_none());
        assert_eq!(metadata.principal_kind, "system");
        assert_eq!(metadata.schema_version, 3);
        let restarted = PgDatabase {
            pool: db.pool.clone(),
            ..db.clone()
        };
        let events = restarted
            .load_events(identity, book.document_id(), DocumentVersion::ZERO, 1000)
            .await?;
        assert_eq!(events.len(), 2);
        let mut restored = Workbook::empty(book.document_id());
        for event in events {
            restored.apply_event(&event)?;
        }
        assert_eq!(restored.hash(), candidate.hash());
        assert_eq!(
            restored
                .calculation_context()
                .map(CalculationContext::epoch),
            Some(1)
        );
        let duplicate_http_trigger = SystemCommitRequest {
            identity,
            expected_access: request.expected_access,
            event: CommittedEvent {
                operation: Operation::AdvanceCalculationEpoch {
                    context: CalculationContext::new(
                        CalculationContextId::from_bytes([46; 16])?,
                        1,
                        1_700_000_000_000,
                        180,
                        19_676,
                        [47; 32],
                        "pxel-formula-0.1.0".into(),
                        "pxel-excel-subset-v1".into(),
                        CalculationReason::Explicit,
                    )?,
                },
                ..request.event.clone()
            },
            expected_head: request.expected_head,
        };
        assert!(matches!(
            db.commit_system(&duplicate_http_trigger).await?,
            CommitOutcome::AlreadyCommitted(_)
        ));
        let collision = SystemCommitRequest {
            identity,
            expected_access: request.expected_access,
            event: CommittedEvent {
                operation: Operation::AdvanceCalculationEpoch {
                    context: CalculationContext::new(
                        CalculationContextId::from_bytes([44; 16])?,
                        1,
                        1_700_000_000_000,
                        180,
                        19_676,
                        [45; 32],
                        "pxel-formula-0.1.0".into(),
                        "pxel-excel-subset-v1".into(),
                        CalculationReason::Timer,
                    )?,
                },
                ..request.event.clone()
            },
            ..request
        };
        assert!(matches!(
            db.commit_system(&collision).await,
            Err(JournalError::IdempotencyConflict)
        ));
        Ok(())
    }

    async fn bootstrap_external_progress_case(pool: sqlx::PgPool, mode: u8) -> TestResult {
        use spreadsheet_collab::SessionDirectory;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let (db, identity, book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let mut formula = request(&db, identity, &book, 101).await?;
        formula.command.operation = CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new("=NOW()".into())?,
        };
        formula.event.operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind("=NOW()", &book, address)?,
        };
        let authored = book.candidate(&formula.event.operation, DocumentVersion::new(2)?)?;
        formula.event.state_hash = authored.hash();
        db.commit(&formula).await?;
        sqlx::raw_sql("CREATE FUNCTION fail_bootstrap_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.principal_kind='system' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_bootstrap_event BEFORE INSERT ON document_operations FOR EACH ROW EXECUTE FUNCTION fail_bootstrap_event()") .execute(&db.pool).await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(db.clone())
            .with_context_sampler(move |reason| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok((
                    OperationId::from_bytes([102; 16])?,
                    CalculationContext::new(
                        CalculationContextId::from_bytes([103; 16])?,
                        1,
                        0,
                        0,
                        0,
                        [104; 32],
                        spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                        spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                        reason,
                    )?,
                ))
            });
        let session = directory.session(identity, book.document_id()).await?;
        assert!(session.read(identity).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(db.access(identity, book.document_id()).await?.head.get(), 2);
        sqlx::raw_sql("DROP TRIGGER fail_bootstrap_event ON document_operations")
            .execute(&db.pool)
            .await?;
        let expected = if mode == 1 {
            let mut clear = request(&db, identity, &authored, 105).await?;
            clear.command.operation = CommandPayload::ClearCell { address };
            clear.event.operation = Operation::ClearCell { address };
            let candidate = authored.candidate(&clear.event.operation, DocumentVersion::new(3)?)?;
            clear.event.state_hash = candidate.hash();
            db.commit(&clear).await?;
            candidate
        } else {
            let resolved = mode == 2;
            let operation = Operation::AdvanceCalculationEpoch {
                context: CalculationContext::new(
                    CalculationContextId::from_bytes([if resolved { 103 } else { 106 }; 16])?,
                    1,
                    if resolved { 0 } else { 86400000 },
                    0,
                    if resolved { 0 } else { 1 },
                    [if resolved { 104 } else { 107 }; 32],
                    spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                    spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                    if resolved {
                        CalculationReason::Open
                    } else {
                        CalculationReason::Explicit
                    },
                )?,
            };
            let candidate = authored.candidate(&operation, DocumentVersion::new(3)?)?;
            db.commit_system(&SystemCommitRequest {
                identity,
                expected_access: db.access(identity, book.document_id()).await?,
                expected_head: authored.version(),
                event: CommittedEvent {
                    document_id: book.document_id(),
                    operation_id: OperationId::from_bytes([if resolved { 102 } else { 108 }; 16])?,
                    principal: EventPrincipal::System,
                    version: candidate.version(),
                    operation,
                    state_hash: candidate.hash(),
                },
            })
            .await?;
            candidate
        };
        let refreshed = session.read(identity).await?;
        assert_eq!(refreshed.hash(), expected.hash());
        assert_eq!(refreshed.version().get(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(db.access(identity, book.document_id()).await?.head.get(), 3);
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn bootstrap_pending_is_superseded_by_external_context(pool: sqlx::PgPool) -> TestResult {
        bootstrap_external_progress_case(pool, 0).await
    }
    #[sqlx::test(migrations = false)]
    async fn bootstrap_pending_is_cancelled_after_last_volatile_formula_removed(
        pool: sqlx::PgPool,
    ) -> TestResult {
        bootstrap_external_progress_case(pool, 1).await
    }
    #[sqlx::test(migrations = false)]
    async fn bootstrap_pending_receipt_resolves_without_another_epoch(
        pool: sqlx::PgPool,
    ) -> TestResult {
        bootstrap_external_progress_case(pool, 2).await
    }
    async fn volatile_edit_recovery_case(pool: sqlx::PgPool, mode: u8) -> TestResult {
        use spreadsheet_collab::SessionDirectory;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let pending = mode == 1;
        let (db, identity, mut book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        async fn formula_request(
            db: &PgDatabase,
            identity: AuthenticatedActor,
            book: &Workbook,
            address: CellAddress,
            source: &str,
            id: u8,
        ) -> Result<CommitRequest, Box<dyn std::error::Error>> {
            let mut write = request(db, identity, book, id).await?;
            write.command.operation = CommandPayload::SetCellFormula {
                address,
                source: spreadsheet_domain::formula::FormulaSource::new(source.into())?,
            };
            write.event.operation = Operation::SetCellFormula {
                address,
                formula: spreadsheet_formula::binder::bind(source, book, address)?,
            };
            write.event.state_hash = book
                .candidate(&write.event.operation, write.event.version)?
                .hash();
            Ok(write)
        }
        let first = formula_request(&db, identity, &book, address, "=RAND()", 120).await?;
        db.commit(&first).await?;
        book.apply_event(&first.event)?;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let sampler = move |reason| {
            let count = counter.fetch_add(1, Ordering::SeqCst) as u8;
            Ok((
                OperationId::from_bytes([150 + count; 16])?,
                CalculationContext::new(
                    CalculationContextId::from_bytes([160 + count; 16])?,
                    1,
                    0,
                    0,
                    0,
                    [170 + count; 32],
                    spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                    spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                    reason,
                )?,
            ))
        };
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(db.clone())
            .with_context_sampler(sampler.clone());
        let session = directory.session(identity, book.document_id()).await?;
        book = session.read(identity).await?;
        assert_eq!(book.version().get(), 3);
        let edit = formula_request(&db, identity, &book, address, "=RAND()+1", 121).await?;
        if pending {
            sqlx::raw_sql("CREATE FUNCTION fail_edit_epoch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.principal_kind='system' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$; CREATE TRIGGER fail_edit_epoch BEFORE INSERT ON document_operations FOR EACH ROW EXECUTE FUNCTION fail_edit_epoch()") .execute(&db.pool).await?;
            assert!(matches!(
                session.submit(identity, edit.command.clone()).await?,
                spreadsheet_collab::Submission::Committed { .. }
            ));
            assert_eq!(session.head(identity).await?.get(), 4);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            sqlx::raw_sql("DROP TRIGGER fail_edit_epoch ON document_operations")
                .execute(&db.pool)
                .await?;
            // Refresh the expected canonical CellWrite chosen by the actor.
            for event in db
                .load_events(identity, book.document_id(), book.version(), 128)
                .await?
            {
                book.apply_event(&event)?;
            }
            let operation = Operation::AdvanceCalculationEpoch {
                context: CalculationContext::new(
                    CalculationContextId::from_bytes([140; 16])?,
                    2,
                    86400000,
                    0,
                    1,
                    [141; 32],
                    spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                    spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                    CalculationReason::Explicit,
                )?,
            };
            let candidate = book.candidate(&operation, DocumentVersion::new(5)?)?;
            db.commit_system(&SystemCommitRequest {
                identity,
                expected_access: db.access(identity, book.document_id()).await?,
                expected_head: book.version(),
                event: CommittedEvent {
                    document_id: book.document_id(),
                    operation_id: OperationId::from_bytes([142; 16])?,
                    principal: EventPrincipal::System,
                    version: candidate.version(),
                    operation,
                    state_hash: candidate.hash(),
                },
            })
            .await?;
            book = candidate;
            let later = formula_request(&db, identity, &book, address, "=RAND()+2", 122).await?;
            db.commit(&later).await?;
            book.apply_event(&later.event)?;
            book = session.read(identity).await?;
            assert_eq!(book.version().get(), 7);
            assert_eq!(
                book.calculation_context().ok_or("context missing")?.epoch(),
                3
            );
            assert_eq!(calls.load(Ordering::SeqCst), 3);
        } else {
            // Durable user event with no matching epoch: simulate the crash gap.
            db.commit(&edit).await?;
            book.apply_event(&edit.event)?;
        }
        let restored = if mode == 2 {
            // Inject E2 then edit B only after ensure's initial recovery scan,
            // during advance_context receipt lookup/CAS retry.
            let operation = Operation::AdvanceCalculationEpoch {
                context: CalculationContext::new(
                    CalculationContextId::from_bytes([140; 16])?,
                    2,
                    86400000,
                    0,
                    1,
                    [141; 32],
                    spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                    spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                    CalculationReason::Explicit,
                )?,
            };
            let candidate = book.candidate(&operation, DocumentVersion::new(5)?)?;
            let later =
                formula_request(&db, identity, &candidate, address, "=RAND()+2", 122).await?;
            let mut later = later;
            later.expected_access.head = candidate.version();
            let journal = ReceiptRaceJournal {
                database: db.clone(),
                pending: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                followup: Some(Arc::new(later)),
                event: Arc::new(SystemCommitRequest {
                    identity,
                    expected_access: db.access(identity, book.document_id()).await?,
                    expected_head: book.version(),
                    event: CommittedEvent {
                        document_id: book.document_id(),
                        operation_id: OperationId::from_bytes([142; 16])?,
                        principal: EventPrincipal::System,
                        version: candidate.version(),
                        operation,
                        state_hash: candidate.hash(),
                    },
                }),
            };
            spreadsheet_collab::InProcessSessionDirectory::new(journal)
                .with_context_sampler(sampler)
                .session(identity, book.document_id())
                .await?
                .read(identity)
                .await?
        } else {
            spreadsheet_collab::InProcessSessionDirectory::new(db.clone())
                .with_context_sampler(sampler)
                .session(identity, book.document_id())
                .await?
                .read(identity)
                .await?
        };
        assert_eq!(restored.version().get(), if mode != 0 { 7 } else { 5 });
        assert_eq!(
            restored
                .calculation_context()
                .ok_or("context missing")?
                .epoch(),
            if mode != 0 { 3 } else { 2 }
        );
        assert_eq!(calls.load(Ordering::SeqCst), if mode != 0 { 3 } else { 2 });
        assert_eq!(
            db.calculation_epoch_version(identity, book.document_id(), restored.version())
                .await?,
            Some(restored.version())
        );
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn volatile_edit_restart_recovers_missing_epoch(pool: sqlx::PgPool) -> TestResult {
        volatile_edit_recovery_case(pool, 0).await
    }
    #[derive(Clone)]
    struct TimerAckLossJournal {
        database: PgDatabase,
        lose: std::sync::Arc<std::sync::atomic::AtomicBool>,
        reason: CalculationReason,
        recovery_fault: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    }
    impl OperationJournal for TimerAckLossJournal {
        async fn calculation_epoch_version(
            &self,
            identity: AuthenticatedActor,
            document: DocumentId,
            through: DocumentVersion,
        ) -> Result<Option<DocumentVersion>, JournalError> {
            if self
                .recovery_fault
                .as_ref()
                .is_some_and(|fault| fault.swap(false, std::sync::atomic::Ordering::SeqCst))
            {
                return Err(JournalError::Unavailable);
            }
            self.database
                .calculation_epoch_version(identity, document, through)
                .await
        }
        delegate_journal!(quarantine(document:DocumentId) -> ());
        delegate_journal!(access(identity:AuthenticatedActor,document:DocumentId) -> DocumentAccess);
        delegate_journal!(undo_history(identity:AuthenticatedActor,document:DocumentId) -> spreadsheet_application::history::UndoHistory);
        delegate_journal!(create(identity:AuthenticatedActor,event:CommittedEvent) -> CommitReceipt);
        delegate_journal!(load_events(identity:AuthenticatedActor,document:DocumentId,after:DocumentVersion,limit:u32) -> Vec<CommittedEvent>);
        delegate_journal!(load_undo_target(identity:AuthenticatedActor,document:DocumentId,target:OperationId) -> CommittedEvent);
        delegate_journal!(commit(request:&CommitRequest) -> CommitOutcome);
        delegate_journal!(receipt(identity:AuthenticatedActor,command:&Command) -> Option<CommitReceipt>);
        delegate_journal!(system_receipt(identity:AuthenticatedActor,document:DocumentId,operation:OperationId,context:&CalculationContext) -> Option<CommitReceipt>);
        async fn commit_system(
            &self,
            request: &SystemCommitRequest,
        ) -> Result<CommitOutcome, JournalError> {
            let outcome = self.database.commit_system(request).await?;
            if matches!(&request.event.operation, Operation::AdvanceCalculationEpoch { context } if context.reason() == CalculationReason::Open && context.epoch() > 1)
                && let Some(fault) = &self.recovery_fault
            {
                fault.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            if matches!(&request.event.operation, Operation::AdvanceCalculationEpoch { context } if context.reason() == self.reason)
                && self.lose.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(JournalError::Unavailable);
            }
            Ok(outcome)
        }
    }
    async fn timer_epoch_case(pool: sqlx::PgPool, source: &str, mode: u8) -> TestResult {
        use spreadsheet_collab::SessionDirectory;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
        };
        let (db, identity, book) = fixture(pool).await?;
        let lose_ack = mode != 0;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let mut formula = request(&db, identity, &book, 180).await?;
        formula.command.operation = CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new(source.into())?,
        };
        formula.event.operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind(source, &book, address)?,
        };
        formula.event.state_hash = book
            .candidate(&formula.event.operation, formula.event.version)?
            .hash();
        db.commit(&formula).await?;
        let clock = Arc::new(AtomicI64::new(0));
        let sampled_clock = clock.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let journal = TimerAckLossJournal {
            database: db.clone(),
            lose: Arc::new(AtomicBool::new(lose_ack)),
            reason: CalculationReason::Timer,
            recovery_fault: None,
        };
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(journal)
            .with_context_sampler(move |reason| {
                let count = counter.fetch_add(1, Ordering::SeqCst) as u8;
                let timestamp = sampled_clock.load(Ordering::SeqCst);
                Ok((
                    OperationId::from_bytes([190 + count; 16])?,
                    CalculationContext::new(
                        CalculationContextId::from_bytes([200 + count; 16])?,
                        1,
                        timestamp,
                        0,
                        (timestamp / 86_400_000) as i32,
                        [210 + count; 32],
                        spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                        spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                        reason,
                    )?,
                ))
            });
        let session = directory.session(identity, book.document_id()).await?;
        assert_eq!(session.read(identity).await?.version().get(), 3);
        clock.store(59_999, Ordering::SeqCst);
        session.request_timer(identity)?;
        assert_eq!(session.head(identity).await?.get(), 3);
        clock.store(60_000, Ordering::SeqCst);
        session.request_timer(identity)?;
        let first_head = session.head(identity).await?;
        assert_eq!(first_head.get(), if source == "=NOW()" { 4 } else { 3 });
        if mode == 2 {
            let command = Command {
                operation_id: OperationId::from_bytes([230; 16])?,
                document_id: book.document_id(),
                actor_id: identity.actor_id,
                base_version: first_head,
                client_sequence: 1,
                operation: CommandPayload::ClearCell { address },
            };
            assert!(matches!(
                session.submit(identity, command).await?,
                spreadsheet_collab::Submission::Committed { .. }
            ));
            assert_eq!(session.head(identity).await?.get(), 5);
            let before = calls.load(Ordering::SeqCst);
            clock.store(120_000, Ordering::SeqCst);
            session.request_timer(identity)?;
            assert_eq!(session.head(identity).await?.get(), 5);
            assert_eq!(calls.load(Ordering::SeqCst), before);
            assert_eq!(
                session
                    .read(identity)
                    .await?
                    .calculation_context()
                    .ok_or("context missing")?
                    .epoch(),
                2
            );
            return Ok(());
        }
        // Another subscription requests the same interval: persisted time coalesces it.
        let before_retry_samples = calls.load(Ordering::SeqCst);
        if lose_ack {
            clock.store(120_000, Ordering::SeqCst);
        }
        session.request_timer(identity)?;
        assert_eq!(session.head(identity).await?, first_head);
        if lose_ack {
            assert_eq!(calls.load(Ordering::SeqCst), before_retry_samples);
            // The first retry resolved the retained ID without resampling.
            assert_eq!(
                session
                    .read(identity)
                    .await?
                    .calculation_context()
                    .ok_or("context missing")?
                    .epoch(),
                2
            );
            assert!(
                !db.load_events(identity, book.document_id(), DocumentVersion::new(3)?, 128)
                    .await?
                    .iter()
                    .any(|event| event.version.get() > 4)
            );
        }
        clock.store(86_400_000, Ordering::SeqCst);
        session.request_timer(identity)?;
        let restored = session.read(identity).await?;
        assert_eq!(
            restored.version().get(),
            match source {
                "=NOW()" => 5,
                "=TODAY()" => 4,
                _ => 3,
            }
        );
        if source != "=RAND()" {
            assert_eq!(
                restored
                    .calculation_context()
                    .ok_or("context missing")?
                    .reason(),
                CalculationReason::Timer
            );
            let calculated = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Some(state) = session.calculated(identity).await? {
                        break Ok::<_, spreadsheet_collab::SessionError>(state);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await??;
            match &calculated.outcome {
                spreadsheet_collab::calculation::CalculationOutcome::Complete {
                    results, ..
                } => assert_eq!(
                    results.get(&address),
                    Some(&spreadsheet_formula::Value::Number(25570.0))
                ),
                _ => return Err("timer calculation failed".into()),
            }
        } else {
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn open_compatible_rand_renews_but_reads_do_not(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 0).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_bootstrap_reuses_first_epoch(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 3).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_static_book_has_no_epoch(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=1+2", 0).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_lost_commit_response_keeps_sample(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 1).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_deleted_consumer_resolves_pending_receipt(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 2).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_viewer_allowed_then_revocation_stops_renewal(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 4).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_recovers_edit_and_reuses_its_epoch(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 5).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_cross_tenant_identity_cannot_renew(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 6).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_post_commit_recovery_failure_retains_receipt(pool: sqlx::PgPool) -> TestResult {
        open_epoch_case(pool, "=RAND()", 7).await
    }
    async fn open_epoch_case(pool: sqlx::PgPool, source: &str, mode: u8) -> TestResult {
        use spreadsheet_collab::SessionDirectory;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let (db, identity, book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let mut formula = request(&db, identity, &book, 180).await?;
        formula.command.operation = CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new(source.into())?,
        };
        formula.event.operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind(source, &book, address)?,
        };
        formula.event.state_hash = book
            .candidate(&formula.event.operation, formula.event.version)?
            .hash();
        db.commit(&formula).await?;
        let lose = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(TimerAckLossJournal {
            database: db.clone(),
            lose: lose.clone(),
            reason: CalculationReason::Open,
            recovery_fault: (mode == 7).then(|| Arc::new(AtomicBool::new(false))),
        })
        .with_context_sampler(move |reason| {
            let count = counter.fetch_add(1, Ordering::SeqCst) as u8;
            Ok((
                OperationId::from_bytes([190 + count; 16])?,
                CalculationContext::new(
                    CalculationContextId::from_bytes([200 + count; 16])?,
                    1,
                    i64::from(count) * 1000,
                    0,
                    0,
                    [210 + count; 32],
                    spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                    spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                    reason,
                )?,
            ))
        });
        let session = directory.session(identity, book.document_id()).await?;
        if mode == 3 {
            assert_eq!(session.opened(identity).await?.get(), 3);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                session
                    .read(identity)
                    .await?
                    .calculation_context()
                    .ok_or("missing context")?
                    .epoch(),
                1
            );
            return Ok(());
        }
        let initial = session.read(identity).await?;
        if source == "=1+2" {
            assert_eq!(session.opened(identity).await?.get(), 2);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(
                session
                    .read(identity)
                    .await?
                    .calculation_context()
                    .is_none()
            );
            return Ok(());
        }
        assert_eq!(initial.version().get(), 3);
        if mode == 5 {
            let mut edit = request(&db, identity, &initial, 220).await?;
            edit.command.operation = CommandPayload::SetCellFormula {
                address,
                source: spreadsheet_domain::formula::FormulaSource::new("=RAND()+1".into())?,
            };
            edit.event.operation = Operation::SetCellFormula {
                address,
                formula: spreadsheet_formula::binder::bind("=RAND()+1", &initial, address)?,
            };
            edit.event.state_hash = initial
                .candidate(&edit.event.operation, edit.event.version)?
                .hash();
            db.commit(&edit).await?;
            assert_eq!(session.opened(identity).await?.get(), 5);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(
                session
                    .read(identity)
                    .await?
                    .calculation_context()
                    .ok_or("missing context")?
                    .reason(),
                CalculationReason::VolatileDependency
            );
            return Ok(());
        }
        if mode == 6 {
            assert!(
                session
                    .opened(AuthenticatedActor {
                        workspace_id: WorkspaceId::from_bytes([222; 16])?,
                        ..identity
                    })
                    .await
                    .is_err()
            );
            assert_eq!(db.access(identity, book.document_id()).await?.head.get(), 3);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            return Ok(());
        }
        lose.store(mode == 1 || mode == 2, Ordering::SeqCst);
        if mode == 4 {
            sqlx::raw_sql("UPDATE document_permissions SET role='viewer'")
                .execute(&db.pool)
                .await?;
        }
        if mode == 1 || mode == 2 || mode == 7 {
            assert!(session.opened(identity).await.is_err());
            assert_eq!(db.access(identity, book.document_id()).await?.head.get(), 4);
            if mode == 2 {
                session
                    .submit(
                        identity,
                        Command {
                            operation_id: OperationId::from_bytes([230; 16])?,
                            document_id: book.document_id(),
                            actor_id: identity.actor_id,
                            base_version: DocumentVersion::new(4)?,
                            client_sequence: 1,
                            operation: CommandPayload::ClearCell { address },
                        },
                    )
                    .await?;
            }
        }
        assert_eq!(
            session.opened(identity).await?.get(),
            if mode == 2 { 5 } else { 4 }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let renewed = session.read(identity).await?;
        assert_eq!(
            renewed
                .calculation_context()
                .ok_or("missing context")?
                .epoch(),
            2
        );
        assert_eq!(
            renewed
                .calculation_context()
                .ok_or("missing context")?
                .reason(),
            CalculationReason::Open
        );
        assert_ne!(renewed.calculation_context(), initial.calculation_context());
        assert_eq!(session.read(identity).await?.hash(), renewed.hash());
        assert_eq!(session.head(identity).await?, renewed.version());
        if mode == 4 {
            sqlx::raw_sql("DELETE FROM document_permissions")
                .execute(&db.pool)
                .await?;
            assert!(session.opened(identity).await.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn timer_now_threshold_and_subscriber_coalescing(pool: sqlx::PgPool) -> TestResult {
        timer_epoch_case(pool, "=NOW()", 0).await
    }
    #[sqlx::test(migrations = false)]
    async fn timer_today_only_advances_at_civil_midnight(pool: sqlx::PgPool) -> TestResult {
        timer_epoch_case(pool, "=TODAY()", 0).await
    }
    #[sqlx::test(migrations = false)]
    async fn timer_rand_only_never_samples_clock(pool: sqlx::PgPool) -> TestResult {
        timer_epoch_case(pool, "=RAND()", 0).await
    }
    #[sqlx::test(migrations = false)]
    async fn timer_lost_commit_ack_resolves_same_id(pool: sqlx::PgPool) -> TestResult {
        timer_epoch_case(pool, "=NOW()", 1).await
    }
    #[sqlx::test(migrations = false)]
    async fn timer_pending_receipt_resolves_after_clock_formula_deleted(
        pool: sqlx::PgPool,
    ) -> TestResult {
        timer_epoch_case(pool, "=NOW()", 2).await
    }
    #[sqlx::test(migrations = false)]
    async fn timer_cas_race_preserves_edit_after_external_epoch(pool: sqlx::PgPool) -> TestResult {
        clock_open_cas_race(pool, false, true).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_cas_race_preserves_edit_after_external_epoch(pool: sqlx::PgPool) -> TestResult {
        clock_open_cas_race(pool, true, true).await
    }
    #[sqlx::test(migrations = false)]
    async fn open_new_external_epoch_supersedes_candidate(pool: sqlx::PgPool) -> TestResult {
        clock_open_cas_race(pool, true, false).await
    }
    async fn clock_open_cas_race(
        pool: sqlx::PgPool,
        opening: bool,
        edit_after: bool,
    ) -> TestResult {
        use spreadsheet_collab::SessionDirectory;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let (db, identity, book) = fixture(pool).await?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([5; 16])?,
            row: RowId::from_bytes([6; 16])?,
            column: ColumnId::from_bytes([7; 16])?,
        };
        let mut formula = request(&db, identity, &book, 232).await?;
        formula.command.operation = CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new("=NOW()".into())?,
        };
        formula.event.operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind("=NOW()", &book, address)?,
        };
        formula.event.state_hash = book
            .candidate(&formula.event.operation, formula.event.version)?
            .hash();
        db.commit(&formula).await?;
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(db.clone())
            .with_context_sampler(|reason| {
                Ok((
                    OperationId::from_bytes([233; 16])?,
                    CalculationContext::new(
                        CalculationContextId::from_bytes([234; 16])?,
                        1,
                        0,
                        0,
                        0,
                        [235; 32],
                        spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                        spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                        reason,
                    )?,
                ))
            });
        let initialized = directory
            .session(identity, book.document_id())
            .await?
            .read(identity)
            .await?;
        let operation = Operation::AdvanceCalculationEpoch {
            context: CalculationContext::new(
                CalculationContextId::from_bytes([236; 16])?,
                2,
                60_000,
                0,
                0,
                [237; 32],
                spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                CalculationReason::Explicit,
            )?,
        };
        let external = initialized.candidate(&operation, DocumentVersion::new(4)?)?;
        let mut edit = request(&db, identity, &external, 238).await?;
        edit.command.operation = CommandPayload::SetCellFormula {
            address,
            source: spreadsheet_domain::formula::FormulaSource::new("=NOW()+1".into())?,
        };
        edit.event.operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind("=NOW()+1", &external, address)?,
        };
        edit.event.state_hash = external
            .candidate(&edit.event.operation, edit.event.version)?
            .hash();
        let journal = ReceiptRaceJournal {
            database: db.clone(),
            pending: Arc::new(AtomicBool::new(true)),
            followup: edit_after.then(|| Arc::new(edit)),
            event: Arc::new(SystemCommitRequest {
                identity,
                expected_head: initialized.version(),
                expected_access: db.access(identity, book.document_id()).await?,
                event: CommittedEvent {
                    document_id: book.document_id(),
                    operation_id: OperationId::from_bytes([239; 16])?,
                    principal: EventPrincipal::System,
                    version: external.version(),
                    operation,
                    state_hash: external.hash(),
                },
            }),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let directory = spreadsheet_collab::InProcessSessionDirectory::new(journal)
            .with_context_sampler(move |reason| {
                let count = counter.fetch_add(1, Ordering::SeqCst) as u8;
                Ok((
                    OperationId::from_bytes([240 + count; 16])?,
                    CalculationContext::new(
                        CalculationContextId::from_bytes([245 + count; 16])?,
                        1,
                        60_000,
                        0,
                        0,
                        [250 + count; 32],
                        spreadsheet_formula::calculation::ENGINE_VERSION.into(),
                        spreadsheet_formula::calculation::COMPATIBILITY_PROFILE.into(),
                        reason,
                    )?,
                ))
            });
        let session = directory.session(identity, book.document_id()).await?;
        if opening {
            session.opened(identity).await?;
        } else {
            session.request_timer(identity)?;
        }
        assert_eq!(
            session.head(identity).await?.get(),
            if edit_after { 6 } else { 4 }
        );
        let restored = session.read(identity).await?;
        assert_eq!(
            restored
                .calculation_context()
                .ok_or("context missing")?
                .epoch(),
            if edit_after { 3 } else { 2 }
        );
        assert_eq!(
            restored
                .calculation_context()
                .ok_or("context missing")?
                .reason(),
            if edit_after {
                CalculationReason::VolatileDependency
            } else {
                CalculationReason::Explicit
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), if edit_after { 2 } else { 1 });
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn volatile_edit_newer_external_edit_outlives_old_pending_context(
        pool: sqlx::PgPool,
    ) -> TestResult {
        volatile_edit_recovery_case(pool, 1).await
    }
    #[sqlx::test(migrations = false)]
    async fn volatile_edit_refresh_race_recovers_newer_edit_after_external_epoch(
        pool: sqlx::PgPool,
    ) -> TestResult {
        volatile_edit_recovery_case(pool, 2).await
    }
    #[sqlx::test(migrations = false)]
    async fn system_trigger_checks_role_epochs_and_revocation_inside_commit(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let make = |reason| -> Result<Operation, DomainError> {
            Ok(Operation::AdvanceCalculationEpoch {
                context: CalculationContext::new(
                    CalculationContextId::from_bytes([92; 16])?,
                    1,
                    0,
                    0,
                    0,
                    [93; 32],
                    "pxel-formula-0.16.0".into(),
                    "pxel-excel-subset-v1".into(),
                    reason,
                )?,
            })
        };
        let operation = make(CalculationReason::Explicit)?;
        let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
        let mut request = SystemCommitRequest {
            identity,
            expected_access: db.access(identity, book.document_id()).await?,
            expected_head: book.version(),
            event: CommittedEvent {
                document_id: book.document_id(),
                operation_id: OperationId::from_bytes([94; 16])?,
                principal: EventPrincipal::System,
                version: candidate.version(),
                operation,
                state_hash: candidate.hash(),
            },
        };
        sqlx::raw_sql("UPDATE documents SET access_epoch=access_epoch+1")
            .execute(&db.pool)
            .await?;
        assert!(matches!(
            db.commit_system(&request).await,
            Err(JournalError::Conflict)
        ));
        assert_eq!(
            db.access(identity, book.document_id()).await?.head,
            book.version()
        );
        request.expected_access = db.access(identity, book.document_id()).await?;
        sqlx::raw_sql("UPDATE document_permissions SET role='viewer'")
            .execute(&db.pool)
            .await?;
        assert!(matches!(
            db.commit_system(&request).await,
            Err(JournalError::Unauthorized)
        ));
        request.event.operation = make(CalculationReason::Open)?;
        request.event.state_hash = book
            .candidate(&request.event.operation, DocumentVersion::new(2)?)?
            .hash();
        assert!(matches!(
            db.commit_system(&request).await?,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            db.commit_system(&request).await?,
            CommitOutcome::AlreadyCommitted(_)
        ));
        sqlx::raw_sql("DELETE FROM document_permissions")
            .execute(&db.pool)
            .await?;
        assert!(matches!(
            db.commit_system(&request).await,
            Err(JournalError::Unauthorized)
        ));
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn system_trigger_cannot_cross_tenant_after_preflight(pool: sqlx::PgPool) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let operation = Operation::AdvanceCalculationEpoch {
            context: CalculationContext::new(
                CalculationContextId::from_bytes([95; 16])?,
                1,
                0,
                0,
                0,
                [96; 32],
                "pxel-formula-0.16.0".into(),
                "pxel-excel-subset-v1".into(),
                CalculationReason::Open,
            )?,
        };
        let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
        let request = SystemCommitRequest {
            identity: AuthenticatedActor {
                workspace_id: WorkspaceId::from_bytes([97; 16])?,
                ..identity
            },
            expected_access: db.access(identity, book.document_id()).await?,
            expected_head: book.version(),
            event: CommittedEvent {
                document_id: book.document_id(),
                operation_id: OperationId::from_bytes([98; 16])?,
                principal: EventPrincipal::System,
                version: candidate.version(),
                operation,
                state_hash: candidate.hash(),
            },
        };
        assert!(matches!(
            db.commit_system(&request).await,
            Err(JournalError::Unauthorized) | Err(JournalError::NotFound)
        ));
        assert_eq!(
            db.access(identity, book.document_id()).await?.head,
            book.version()
        );
        Ok(())
    }
    #[sqlx::test(migrations = false)]
    async fn system_cas_retry_reuses_sampled_context_after_user_commit(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, book) = fixture(pool).await?;
        let context = CalculationContext::new(
            CalculationContextId::from_bytes([50; 16])?,
            1,
            1_700_000_000_000,
            180,
            19_676,
            [51; 32],
            "pxel-formula-0.1.0".into(),
            "pxel-excel-subset-v1".into(),
            CalculationReason::Explicit,
        )?;
        let operation_id = OperationId::from_bytes([52; 16])?;
        let operation = Operation::AdvanceCalculationEpoch {
            context: context.clone(),
        };
        let before_user = book.candidate(&operation, DocumentVersion::new(2)?)?;
        let stale = SystemCommitRequest {
            identity,
            expected_access: db.access(identity, book.document_id()).await?,
            expected_head: book.version(),
            event: CommittedEvent {
                document_id: book.document_id(),
                operation_id,
                principal: EventPrincipal::System,
                version: DocumentVersion::new(2)?,
                operation: operation.clone(),
                state_hash: before_user.hash(),
            },
        };
        let user = request(&db, identity, &book, 53).await?;
        assert!(matches!(
            db.commit(&user).await?,
            CommitOutcome::Committed(_)
        ));
        assert!(matches!(
            db.commit_system(&stale).await,
            Err(JournalError::Conflict)
        ));
        let mut after_user = book.clone();
        after_user.apply_event(&user.event)?;
        let candidate = after_user.candidate(&operation, DocumentVersion::new(3)?)?;
        let retry = SystemCommitRequest {
            identity,
            expected_access: db.access(identity, book.document_id()).await?,
            expected_head: after_user.version(),
            event: CommittedEvent {
                version: DocumentVersion::new(3)?,
                state_hash: candidate.hash(),
                ..stale.event.clone()
            },
        };
        assert_eq!(
            codec::system_fingerprint(&retry.event)?,
            codec::system_fingerprint(&stale.event)?
        );
        assert!(matches!(
            db.commit_system(&retry).await?,
            CommitOutcome::Committed(_)
        ));
        let mut restored = Workbook::empty(book.document_id());
        for event in db
            .load_events(identity, book.document_id(), DocumentVersion::ZERO, 1000)
            .await?
        {
            restored.apply_event(&event)?;
        }
        assert_eq!(restored.hash(), candidate.hash());
        assert_eq!(
            restored
                .calculation_context()
                .map(CalculationContext::context_id),
            Some(context.context_id())
        );
        Ok(())
    }
}
