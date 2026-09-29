use crate::PgDatabase;
use futures_util::TryStreamExt;
use spreadsheet_application::journal::*;
use spreadsheet_domain::workbook::*;
use spreadsheet_storage_codec as codec;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
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
const MAX_IMPORT_CELLS: usize = 1_000;
const MAX_IMPORT_ENCODED_BYTES: usize = 32 * 1024 * 1024;
struct StoredEventRow<'a> {
    document: DocumentId,
    committed_version: i64,
    operation_id: Uuid,
    actor_id: Option<Uuid>,
    principal_kind: &'a str,
    schema_version: i32,
    payload: &'a [u8],
    payload_checksum: &'a [u8],
    canonical_event_hash: &'a [u8],
}
fn verified_event(row: StoredEventRow<'_>) -> Result<CommittedEvent, JournalError> {
    if !matches!(row.schema_version, 1..=15) {
        return Err(JournalError::UnsupportedSchema);
    }
    if codec::checksum(row.payload).as_slice() != row.payload_checksum {
        return Err(JournalError::Corrupt);
    }
    let event = codec::decode(row.payload).map_err(|error| match error {
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
        || codec::event_hash(&event).map_err(invalid)?.as_slice() != row.canonical_event_hash
    {
        return Err(JournalError::Corrupt);
    }
    Ok(event)
}
fn undo_target_address(
    event: &CommittedEvent,
    actor: ActorId,
) -> Result<(CellAddress, Option<CellContent>), JournalError> {
    if event.principal != EventPrincipal::User(actor) {
        return Err(JournalError::NotUndoable);
    }
    match &event.operation {
        Operation::CellWrite {
            address,
            previous,
            kind: Some(_),
            undoes: None,
            undoes_version: None,
            ..
        } => Ok((*address, previous.clone())),
        _ => Err(JournalError::NotUndoable),
    }
}
fn touches_address(operation: &Operation, address: CellAddress) -> bool {
    match operation {
        Operation::SetCellValue {
            address: target, ..
        }
        | Operation::ClearCell { address: target }
        | Operation::SetCellFormula {
            address: target, ..
        }
        | Operation::CellWrite {
            address: target, ..
        } => *target == address,
        Operation::SetCellValues { writes } => writes.iter().any(|write| write.address == address),
        Operation::CopyRange { writes } => writes.iter().any(|write| write.address == address),
        Operation::DeleteRows { sheet, rows } => {
            *sheet == address.sheet && rows.contains(&address.row)
        }
        Operation::DeleteColumns { sheet, columns } => {
            *sheet == address.sheet && columns.contains(&address.column)
        }
        Operation::CreateWorkbook { .. }
        | Operation::AddSheet { .. }
        | Operation::ExtendRows { .. }
        | Operation::InsertRows { .. }
        | Operation::InsertRowsRebased { .. }
        | Operation::InsertColumnsRebased { .. }
        | Operation::AdvanceCalculationEpoch { .. } => false,
    }
}
async fn access_locked(
    tx: &mut Transaction<'_, Postgres>,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<DocumentAccess, JournalError> {
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
async fn existing(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    operation: OperationId,
    fingerprint: [u8; 32],
    head: DocumentVersion,
) -> Result<Option<CommitReceipt>, JournalError> {
    let row=sqlx::query!("SELECT committed_version, command_fingerprint, canonical_event_hash FROM document_operations WHERE document_id=$1 AND operation_id=$2",uuid(document.as_bytes()),uuid(operation.as_bytes())).fetch_optional(&mut **tx).await.map_err(error)?;
    row.map(|r| {
        if r.command_fingerprint != fingerprint {
            return Err(JournalError::IdempotencyConflict);
        }
        Ok(CommitReceipt {
            operation_id: operation,
            committed_version: version(r.committed_version)?,
            canonical_event_hash: r
                .canonical_event_hash
                .try_into()
                .map_err(|_| JournalError::Corrupt)?,
            current_head_version: head,
        })
    })
    .transpose()
}
async fn undo_target_locked(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target_id: OperationId,
    actor: ActorId,
    head: DocumentVersion,
) -> Result<(CommittedEvent, CellAddress, Option<CellContent>), JournalError> {
    let row = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND operation_id=$2",
        uuid(document.as_bytes()),
        uuid(target_id.as_bytes())
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(error)?
    .ok_or(JournalError::UndoConflict)?;
    let event = verified_event(StoredEventRow {
        document,
        committed_version: row.committed_version,
        operation_id: row.operation_id,
        actor_id: row.actor_id,
        principal_kind: &row.principal_kind,
        schema_version: row.schema_version,
        payload: &row.payload,
        payload_checksum: &row.payload_checksum,
        canonical_event_hash: &row.canonical_event_hash,
    })?;
    let (address, previous) = undo_target_address(&event, actor)?;
    if head
        .get()
        .checked_sub(event.version.get())
        .ok_or(JournalError::Corrupt)?
        > UNDO_WINDOW
    {
        return Err(JournalError::UndoWindowExceeded);
    }
    Ok((event, address, previous))
}
async fn guard_undo_tail(
    tx: &mut Transaction<'_, Postgres>,
    document: DocumentId,
    target: &CommittedEvent,
    address: CellAddress,
    head: DocumentVersion,
) -> Result<(), JournalError> {
    let mut rows = sqlx::query!(
        "SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 AND committed_version<=$3 ORDER BY committed_version",
        uuid(document.as_bytes()),
        number(target.version.get())?,
        number(head.get())?
    )
    .fetch(&mut **tx);
    let mut expected = target.version.get();
    while let Some(row) = rows.try_next().await.map_err(error)? {
        expected = expected.checked_add(1).ok_or(JournalError::Corrupt)?;
        let event = verified_event(StoredEventRow {
            document,
            committed_version: row.committed_version,
            operation_id: row.operation_id,
            actor_id: row.actor_id,
            principal_kind: &row.principal_kind,
            schema_version: row.schema_version,
            payload: &row.payload,
            payload_checksum: &row.payload_checksum,
            canonical_event_hash: &row.canonical_event_hash,
        })?;
        if event.version.get() != expected {
            return Err(JournalError::Corrupt);
        }
        if touches_address(&event.operation, address) {
            return Err(JournalError::UndoConflict);
        }
    }
    if expected != head.get() {
        return Err(JournalError::Corrupt);
    }
    Ok(())
}
async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    event: &CommittedEvent,
    fingerprint: [u8; 32],
) -> Result<CommitReceipt, JournalError> {
    let payload = codec::encode(event).map_err(|error| match error {
        codec::CodecError::EventTooLarge => JournalError::PayloadTooLarge,
        _ => JournalError::Corrupt,
    })?;
    let checksum = codec::checksum(&payload);
    let hash = codec::event_hash(event).map_err(invalid)?;
    let (actor, principal_kind) = match event.principal {
        EventPrincipal::User(actor) => (Some(uuid(actor.as_bytes())), "user"),
        EventPrincipal::System => (None, "system"),
    };
    sqlx::query!(
        "INSERT INTO document_operations(document_id,committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        uuid(event.document_id.as_bytes()), number(event.version.get())?,
        uuid(event.operation_id.as_bytes()), actor, principal_kind,
        i32::try_from(codec::schema_version(&event.operation)).map_err(|_|JournalError::Corrupt)?,
        payload, &checksum[..], &fingerprint[..], &hash[..]
    ).execute(&mut **tx).await.map_err(error)?;
    Ok(CommitReceipt {
        operation_id: event.operation_id,
        committed_version: event.version,
        canonical_event_hash: hash,
        current_head_version: event.version,
    })
}
impl OperationJournal for PgDatabase {
    async fn quarantine(&self, document_id: DocumentId) -> Result<(), JournalError> {
        sqlx::query!(
            "UPDATE documents SET state='quarantined' WHERE document_id=$1",
            uuid(document_id.as_bytes())
        )
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }
    async fn access(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
    ) -> Result<DocumentAccess, JournalError> {
        let mut tx = self.pool.begin().await.map_err(error)?;
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
        let mut tx = self.pool.begin().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, command.document_id).await?;
        let result = existing(
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
        let mut tx = self.pool.begin().await.map_err(error)?;
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
        let receipt = insert(&mut tx, &event, codec::genesis_fingerprint(&event)).await?;
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
            Operation::CopyRange { .. } => match &c.operation {
                CommandPayload::CopyRange { .. } => c.operation.clone(),
                _ => return Err(JournalError::Corrupt),
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
        let mut tx = self.pool.begin().await.map_err(error)?;
        let access = access_locked(&mut tx, request.identity, c.document_id).await?;
        if !access.can_edit {
            return Err(JournalError::Unauthorized);
        }
        if let Some(receipt) = existing(
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
        if let Operation::CellWrite {
            address,
            content,
            previous,
            kind: None,
            undoes: Some(target_id),
            undoes_version: Some(target_version),
        } = &e.operation
        {
            let (target, target_address, target_previous) = undo_target_locked(
                &mut tx,
                c.document_id,
                *target_id,
                request.identity.actor_id,
                access.head,
            )
            .await?;
            if target.version != *target_version
                || target_address != *address
                || target_previous != *content
                || *previous
                    != match &target.operation {
                        Operation::CellWrite { content, .. } => content.clone(),
                        _ => return Err(JournalError::NotUndoable),
                    }
            {
                return Err(JournalError::UndoConflict);
            }
            guard_undo_tail(&mut tx, c.document_id, &target, *address, access.head).await?;
        }
        let updated=sqlx::query!("UPDATE documents SET head_version=head_version+1 WHERE document_id=$1 AND head_version=$2 RETURNING head_version",uuid(c.document_id.as_bytes()),number(request.expected_head.get())?).fetch_optional(&mut *tx).await.map_err(error)?;
        if updated.is_none() {
            return Err(JournalError::Conflict);
        }
        sqlx::query!("SET LOCAL synchronous_commit = on")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        let receipt = insert(&mut tx, e, fingerprint).await?;
        tx.commit().await.map_err(error)?;
        Ok(CommitOutcome::Committed(receipt))
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
        let mut tx = self.pool.begin().await.map_err(error)?;
        let row = sqlx::query!(
            "SELECT head_version, state FROM documents WHERE document_id=$1 FOR UPDATE",
            uuid(event.document_id.as_bytes())
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?
        .ok_or(JournalError::NotFound)?;
        if row.state != "active" {
            return Err(JournalError::Unavailable);
        }
        let head = version(row.head_version)?;
        if let Some(receipt) = existing(
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
        if head != request.expected_head {
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
        let receipt = insert(&mut tx, event, fingerprint).await?;
        tx.commit().await.map_err(error)?;
        Ok(CommitOutcome::Committed(receipt))
    }
    async fn load_undo_target(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        target_operation_id: OperationId,
    ) -> Result<CommittedEvent, JournalError> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, document_id).await?;
        if !access.can_edit {
            return Err(JournalError::Unauthorized);
        }
        let (target, _, _) = undo_target_locked(
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
        let mut tx = self.pool.begin().await.map_err(error)?;
        let access = access_locked(&mut tx, identity, document_id).await?;
        let rows=sqlx::query!("SELECT committed_version, operation_id, actor_id, principal_kind, schema_version, payload, payload_checksum, canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version>$2 ORDER BY committed_version LIMIT $3",uuid(document_id.as_bytes()),number(after.get())?,i64::from(limit)).fetch_all(&mut *tx).await.map_err(error)?;
        let mut events = Vec::with_capacity(rows.len());
        let mut expected = after.get();
        for r in rows {
            let e = verified_event(StoredEventRow {
                document: document_id,
                committed_version: r.committed_version,
                operation_id: r.operation_id,
                actor_id: r.actor_id,
                principal_kind: &r.principal_kind,
                schema_version: r.schema_version,
                payload: &r.payload,
                payload_checksum: &r.payload_checksum,
                canonical_event_hash: &r.canonical_event_hash,
            })?;
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
        if events.is_empty() || events.len() > MAX_IMPORT_CELLS + 64 {
            return Err(JournalError::Corrupt);
        }
        let genesis = events.first().ok_or(JournalError::Corrupt)?;
        let last = events.last().ok_or(JournalError::Corrupt)?;
        let principal = EventPrincipal::User(identity.actor_id);
        let document_id = genesis.document_id;
        let mut seen = std::collections::BTreeSet::new();
        let mut candidate = Workbook::empty(document_id);
        let mut hashes = Vec::with_capacity(events.len());
        let mut written_cells = 0_usize;
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
                )
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
            written_cells = written_cells
                .checked_add(match &event.operation {
                    Operation::SetCellValues { writes } => writes.len(),
                    Operation::SetCellFormula { .. } => 1,
                    _ => 0,
                })
                .ok_or(JournalError::PayloadTooLarge)?;
            if written_cells > MAX_IMPORT_CELLS {
                return Err(JournalError::PayloadTooLarge);
            }
            let encoded = codec::encode(event).map_err(|error| match error {
                codec::CodecError::EventTooLarge => JournalError::PayloadTooLarge,
                _ => JournalError::Corrupt,
            })?;
            encoded_bytes = encoded_bytes
                .checked_add(encoded.len())
                .ok_or(JournalError::PayloadTooLarge)?;
            if encoded_bytes > MAX_IMPORT_ENCODED_BYTES {
                return Err(JournalError::PayloadTooLarge);
            }
            candidate
                .apply_event(event)
                .map_err(|_| JournalError::Corrupt)?;
            hashes.push(codec::event_hash(event).map_err(invalid)?);
        }
        let mut tx = self.pool.begin().await.map_err(error)?;
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
                "SELECT committed_version,operation_id,canonical_event_hash FROM document_operations WHERE document_id=$1 AND committed_version<=$2 ORDER BY committed_version",
                uuid(document_id.as_bytes()),
                number(last.version.get())?
            )
            .fetch_all(&mut *tx)
            .await
            .map_err(error)?;
            if rows.len() != events.len()
                || rows
                    .iter()
                    .zip(events.iter().zip(&hashes))
                    .any(|(row, (event, hash))| {
                        row.committed_version != event.version.get() as i64
                            || row.operation_id != uuid(event.operation_id.as_bytes())
                            || row.canonical_event_hash.as_slice() != hash
                    })
            {
                return Err(JournalError::IdempotencyConflict);
            }
            tx.commit().await.map_err(error)?;
            return Ok(CommitReceipt {
                operation_id: last.operation_id,
                committed_version: last.version,
                canonical_event_hash: *hashes.last().ok_or(JournalError::Corrupt)?,
                current_head_version: access.head,
            });
        }
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
            receipt = Some(insert(&mut tx, event, fingerprint).await?);
        }
        tx.commit().await.map_err(error)?;
        receipt.ok_or(JournalError::Corrupt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    #[sqlx::test(migrations = false)]
    async fn imported_events_publish_atomically_and_retry_exactly(
        pool: sqlx::PgPool,
    ) -> TestResult {
        let (db, identity, _) = fixture(pool).await?;
        let document = DocumentId::from_bytes([21; 16])?;
        let sheet = SheetId::from_bytes([22; 16])?;
        let row = RowId::from_bytes([23; 16])?;
        let column = ColumnId::from_bytes([24; 16])?;
        let genesis_operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Imported".into(),
                rows: vec![row],
                columns: vec![column],
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
        let events = [genesis.clone(), content.clone()];
        let receipt = db.create_imported(identity, &events).await?;
        assert_eq!(receipt.committed_version, DocumentVersion::new(2)?);
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
        sqlx::query("CREATE FUNCTION fail_import_second_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.committed_version=2 THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$")
            .execute(&db.pool).await?;
        sqlx::query("CREATE TRIGGER fail_import_second_event BEFORE INSERT ON document_operations FOR EACH ROW EXECUTE FUNCTION fail_import_second_event()")
            .execute(&db.pool).await?;
        let other = DocumentId::from_bytes([27; 16])?;
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
    async fn fixture(
        pool: sqlx::PgPool,
    ) -> Result<(PgDatabase, AuthenticatedActor, Workbook), Box<dyn std::error::Error>> {
        let db = PgDatabase { pool };
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
            _ => return Err("invalid guarded-write test fixture".into()),
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
