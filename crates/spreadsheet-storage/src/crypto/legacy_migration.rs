//! Offline, read-only legacy decoding into an unpublished encrypted cluster.
//!
//! The caller freezes legacy workers and supplies a fresh, private destination
//! blob root. No source file is rewritten and no plaintext content is submitted
//! to a destination SQL statement or blob writer. A committed PENDING marker
//! survives any failure; retries require another empty destination.

use crate::{CRYPTO_ENVELOPE_OVERHEAD, CryptoError, PgDatabase};
use bytes::Bytes;
use futures_util::{TryStreamExt, stream};
use serde_json::{Map, Value};
use spreadsheet_application::{BlobStore, BlobStoreError};
use spreadsheet_domain::{
    BlobKey, Checksum,
    workbook::{
        CommittedEvent, DocumentId, DocumentVersion, EventPrincipal, Workbook, WorkspaceId,
    },
};
use spreadsheet_storage_codec as codec;
use sqlx::{
    FromRow, PgPool, Postgres, Row, Transaction,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeSet, future::Future, str::FromStr, time::Duration};
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const LAST_LEGACY_MIGRATION: i64 = 202610020005;
const MAINTENANCE_LOCK: i64 = 1_347_962_188;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
static MIGRATIONS: Migrator = sqlx::migrate!("../../migrations");

// A migration ledger alone cannot detect an out-of-band added column. Never
// pass unknown source fields through a metadata JSON parameter.
const LEGACY_COLUMNS: &[(&str, &str)] = &[
    ("users", "user_id,created_at"),
    ("workspaces", "workspace_id,created_at"),
    (
        "memberships",
        "workspace_id,user_id,role,access_epoch,created_at",
    ),
    ("development_accounts", "account,user_id,workspace_id"),
    (
        "development_sessions",
        "token_hash,user_id,workspace_id,expires_at",
    ),
    (
        "documents",
        "document_id,workspace_id,head_version,access_epoch,state,created_at,preferred_snapshot_id,axis_activation_version",
    ),
    (
        "document_permissions",
        "workspace_id,document_id,user_id,role",
    ),
    (
        "audit_events",
        "event_id,workspace_id,document_id,actor_id,target_user_id,event_type,old_role,new_role,access_epoch,created_at,schema_version",
    ),
    (
        "document_operations",
        "document_id,committed_version,operation_id,actor_id,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,created_at,principal_kind",
    ),
    (
        "document_snapshots",
        "snapshot_id,document_id,snapshot_version,last_axis_change_version,snapshot_schema_version,object_key,byte_length,payload_checksum,state_hash,writer_engine_version,writer_compatibility_profile,state,created_at,verified_at,activated_at,cleanup_attempted_at",
    ),
    (
        "export_audit_events",
        "event_id,workspace_id,document_id,actor_id,document_version,payload_checksum,payload_size,created_at",
    ),
];

fn legacy_column_type(column: &str) -> &'static str {
    match column {
        "user_id"
        | "workspace_id"
        | "document_id"
        | "event_id"
        | "operation_id"
        | "actor_id"
        | "target_user_id"
        | "snapshot_id"
        | "object_key"
        | "preferred_snapshot_id" => "uuid",
        "created_at" | "expires_at" | "verified_at" | "activated_at" | "cleanup_attempted_at" => {
            "timestamptz"
        }
        "token_hash"
        | "payload"
        | "payload_checksum"
        | "command_fingerprint"
        | "canonical_event_hash"
        | "state_hash" => "bytea",
        "access_epoch"
        | "head_version"
        | "axis_activation_version"
        | "committed_version"
        | "snapshot_version"
        | "last_axis_change_version"
        | "byte_length"
        | "document_version"
        | "payload_size" => "int8",
        "schema_version" | "snapshot_schema_version" => "int4",
        _ => "text",
    }
}

#[derive(Debug, Error)]
pub enum LegacyMigrationError {
    #[error("legacy migration database access failed")]
    Database,
    #[error("source is not the supported legacy schema")]
    UnsupportedSource,
    #[error("source and destination must be distinct PostgreSQL clusters")]
    SameCluster,
    #[error("destination must be empty and fully migrated")]
    DestinationNotEmpty,
    #[error("storage maintenance is already in progress")]
    MaintenanceConflict,
    #[error("legacy journal or document head is invalid")]
    InvalidJournal,
    #[error("legacy snapshot metadata or object is invalid")]
    InvalidSnapshot,
    #[error("legacy export audit metadata is invalid")]
    InvalidExportAudit,
    #[error("legacy snapshot object storage failed")]
    Blob,
    #[error("migration encryption failed")]
    Encryption,
    #[error("destination verification failed")]
    Verification,
}

// Preserve classifications without retaining SQL messages, connection URLs,
// paths, row bodies or provider internals in boundary errors.
impl From<sqlx::Error> for LegacyMigrationError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<CryptoError> for LegacyMigrationError {
    fn from(_: CryptoError) -> Self {
        Self::Encryption
    }
}
impl From<BlobStoreError> for LegacyMigrationError {
    fn from(_: BlobStoreError) -> Self {
        Self::Blob
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct LegacyMigrationReport {
    pub metadata_rows: u64,
    pub documents: u64,
    pub operation_records: u64,
    pub snapshot_records: u64,
    pub export_audit_records: u64,
    /// Missing REJECTED/UPLOADING blobs become metadata-only tombstones:
    /// zero external checksum, original length plus envelope overhead, sealed
    /// original state hash, unchanged identity/status/timestamps, no blob.
    pub missing_inactive_blobs: u64,
}

struct MetadataTable {
    select: &'static str,
    insert: &'static str,
}
// SQL identifiers are literal, reviewed whitelist entries. JSON parameters
// contain only metadata, never operation/snapshot/export content fingerprints.
const METADATA: &[MetadataTable] = &[
    MetadataTable {
        // New destination identity classifications retain their member default;
        // compare only the exact legacy metadata fields on both sides.
        select: "SELECT jsonb_build_object('user_id',t.user_id,'created_at',t.created_at)::text FROM users t ORDER BY user_id",
        insert: "INSERT INTO users(user_id,created_at) SELECT user_id,created_at FROM json_populate_record(NULL::users,$1::json)",
    },
    MetadataTable {
        select: "SELECT to_jsonb(t)::text FROM workspaces t ORDER BY workspace_id",
        insert: "INSERT INTO workspaces SELECT * FROM json_populate_record(NULL::workspaces,$1::json)",
    },
    MetadataTable {
        select: "SELECT to_jsonb(t)::text FROM memberships t ORDER BY workspace_id,user_id",
        insert: "INSERT INTO memberships SELECT * FROM json_populate_record(NULL::memberships,$1::json)",
    },
    MetadataTable {
        select: "SELECT to_jsonb(t)::text FROM development_accounts t ORDER BY account",
        insert: "INSERT INTO development_accounts SELECT * FROM json_populate_record(NULL::development_accounts,$1::json)",
    },
    MetadataTable {
        select: "SELECT to_jsonb(t)::text FROM development_sessions t ORDER BY token_hash",
        insert: "INSERT INTO development_sessions SELECT * FROM json_populate_record(NULL::development_sessions,$1::json)",
    },
    MetadataTable {
        select: "SELECT (to_jsonb(t)||jsonb_build_object('preferred_snapshot_id',NULL))::text FROM documents t ORDER BY document_id",
        insert: "INSERT INTO documents SELECT * FROM json_populate_record(NULL::documents,$1::json)",
    },
    MetadataTable {
        select: "SELECT to_jsonb(t)::text FROM document_permissions t ORDER BY document_id,user_id",
        insert: "INSERT INTO document_permissions SELECT * FROM json_populate_record(NULL::document_permissions,$1::json)",
    },
    MetadataTable {
        select: "SELECT to_jsonb(t)::text FROM audit_events t ORDER BY event_id",
        insert: "INSERT INTO audit_events SELECT * FROM json_populate_record(NULL::audit_events,$1::json)",
    },
];

#[derive(FromRow)]
struct DocumentRow {
    document_id: Uuid,
    workspace_id: Uuid,
    head_version: i64,
    preferred_snapshot_id: Option<Uuid>,
}

#[derive(FromRow)]
struct OperationRow {
    committed_version: i64,
    operation_id: Uuid,
    actor_id: Option<Uuid>,
    principal_kind: String,
    schema_version: i32,
    payload: Vec<u8>,
    payload_checksum: Vec<u8>,
    command_fingerprint: Vec<u8>,
    canonical_event_hash: Vec<u8>,
    metadata: String,
}
impl Drop for OperationRow {
    fn drop(&mut self) {
        self.payload.zeroize();
        self.payload_checksum.zeroize();
        self.command_fingerprint.zeroize();
        self.canonical_event_hash.zeroize();
        self.metadata.zeroize();
    }
}
const OPERATION: &str = "SELECT committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,(to_jsonb(o)-ARRAY['payload','payload_checksum','command_fingerprint','canonical_event_hash'])::text AS metadata FROM document_operations o WHERE document_id=$1 AND committed_version=$2";

#[derive(FromRow)]
struct SnapshotRow {
    snapshot_id: Uuid,
    snapshot_version: i64,
    last_axis_change_version: i64,
    snapshot_schema_version: i32,
    object_key: Uuid,
    byte_length: i64,
    payload_checksum: Vec<u8>,
    state_hash: Vec<u8>,
    writer_engine_version: String,
    writer_compatibility_profile: String,
    state: String,
    metadata: String,
}
const SNAPSHOTS: &str = "SELECT snapshot_id,snapshot_version,last_axis_change_version,snapshot_schema_version,object_key,byte_length,payload_checksum,state_hash,writer_engine_version,writer_compatibility_profile,state,(to_jsonb(s)-ARRAY['byte_length','payload_checksum','state_hash'])::text AS metadata FROM document_snapshots s WHERE document_id=$1 AND snapshot_version=$2 AND ($3::uuid IS NULL OR snapshot_id>$3) ORDER BY snapshot_id LIMIT 128";
const ONE_SNAPSHOT: &str = "SELECT snapshot_id,snapshot_version,last_axis_change_version,snapshot_schema_version,object_key,byte_length,payload_checksum,state_hash,writer_engine_version,writer_compatibility_profile,state,(to_jsonb(s)-ARRAY['byte_length','payload_checksum','state_hash'])::text AS metadata FROM document_snapshots s WHERE snapshot_id=$1";

#[derive(FromRow)]
struct ExportRow {
    event_id: Uuid,
    document_version: i64,
    payload_checksum: Vec<u8>,
    payload_size: i64,
    metadata: String,
}
const EXPORTS: &str = "SELECT event_id,document_version,payload_checksum,payload_size,(to_jsonb(e)-ARRAY['payload_checksum','payload_size'])::text AS metadata FROM export_audit_events e WHERE document_id=$1 AND ($2::uuid IS NULL OR event_id>$2) ORDER BY event_id LIMIT 128";
const ONE_EXPORT: &str = "SELECT event_id,document_version,payload_checksum,payload_size,(to_jsonb(e)-ARRAY['payload_checksum','payload_size'])::text AS metadata FROM export_audit_events e WHERE event_id=$1";

fn document_id(id: Uuid) -> Result<DocumentId, LegacyMigrationError> {
    DocumentId::from_bytes(*id.as_bytes()).map_err(|_| LegacyMigrationError::InvalidJournal)
}
fn version(value: i64) -> Result<DocumentVersion, LegacyMigrationError> {
    u64::try_from(value)
        .ok()
        .and_then(|n| DocumentVersion::new(n).ok())
        .ok_or(LegacyMigrationError::InvalidJournal)
}
fn bytes32(value: &[u8]) -> Result<[u8; 32], LegacyMigrationError> {
    value
        .try_into()
        .map_err(|_| LegacyMigrationError::Verification)
}
fn json_object(metadata: &str) -> Result<Map<String, Value>, LegacyMigrationError> {
    match serde_json::from_str(metadata).map_err(|_| LegacyMigrationError::Verification)? {
        Value::Object(value) => Ok(value),
        _ => Err(LegacyMigrationError::Verification),
    }
}
fn bytea(bytes: &[u8]) -> Value {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(2 + bytes.len() * 2);
    value.push_str("\\x");
    for byte in bytes {
        value.push(char::from(HEX[(byte >> 4) as usize]));
        value.push(char::from(HEX[(byte & 15) as usize]));
    }
    Value::String(value)
}
fn encoded_json(value: Map<String, Value>) -> Result<Zeroizing<String>, LegacyMigrationError> {
    serde_json::to_string(&value)
        .map(Zeroizing::new)
        .map_err(|_| LegacyMigrationError::Verification)
}

fn legacy_event(
    document: DocumentId,
    row: &OperationRow,
) -> Result<CommittedEvent, LegacyMigrationError> {
    if !(1..=35).contains(&row.schema_version)
        || row.payload.len() > MAX_EVENT_BYTES
        || row.command_fingerprint.len() != 32
        || row.canonical_event_hash.len() != 32
        || codec::checksum(&row.payload).as_slice() != row.payload_checksum
    {
        return Err(LegacyMigrationError::InvalidJournal);
    }
    let event = codec::decode(&row.payload).map_err(|_| LegacyMigrationError::InvalidJournal)?;
    if codec::schema_version(&event.operation) != row.schema_version as u32
        || event.document_id != document
        || event.version != version(row.committed_version)?
        || event.operation_id.as_bytes() != row.operation_id.as_bytes()
        || match event.principal {
            EventPrincipal::User(actor) => {
                row.principal_kind != "user"
                    || row.actor_id.as_ref().map(Uuid::as_bytes) != Some(actor.as_bytes())
            }
            EventPrincipal::System => row.principal_kind != "system" || row.actor_id.is_some(),
        }
        || codec::event_hash(&event)
            .map_err(|_| LegacyMigrationError::InvalidJournal)?
            .as_slice()
            != row.canonical_event_hash
    {
        return Err(LegacyMigrationError::InvalidJournal);
    }
    Ok(event)
}

async fn durable_insert(
    db: &PgDatabase,
    sql: &'static str,
    metadata: &str,
) -> Result<(), LegacyMigrationError> {
    let mut tx = db.crypto_pool.begin().await?;
    sqlx::query("SET LOCAL synchronous_commit=on")
        .execute(&mut *tx)
        .await?;
    sqlx::query(sql).bind(metadata).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn keep_fence_alive<T>(
    fence: &mut Transaction<'_, Postgres>,
    work: impl Future<Output = Result<T, LegacyMigrationError>>,
) -> Result<T, LegacyMigrationError> {
    tokio::pin!(work);
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = interval.tick() => { sqlx::query("SELECT 1").execute(&mut **fence).await?; }
        }
    }
}

impl PgDatabase {
    /// Validates the exact supported source without changing its schema/data.
    pub async fn inspect_legacy_source(
        source_url: &str,
    ) -> Result<crate::PgMaintenanceInfo, LegacyMigrationError> {
        let information = Self::inspect_storage_cluster(source_url).await?;
        let pool = legacy_read_only_pool(source_url).await?;
        let result = async {
            let mut tx = pool.begin().await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx)
                .await?;
            verify_schema(&mut tx, true).await?;
            tx.rollback().await?;
            Ok::<_, LegacyMigrationError>(information)
        }
        .await;
        pool.close().await;
        result
    }

    /// Fresh destination preflight precedes applying even schema migrations.
    pub async fn inspect_legacy_destination(
        target_url: &str,
    ) -> Result<crate::PgMaintenanceInfo, LegacyMigrationError> {
        let information = Self::inspect_storage_cluster(target_url).await?;
        if information.has_application_data {
            return Err(LegacyMigrationError::DestinationNotEmpty);
        }
        let pool = legacy_read_only_pool(target_url).await?;
        let result=async {
            let mut names:Vec<&str>=LEGACY_COLUMNS.iter().map(|(table,_)|*table).collect();
            names.extend(["_sqlx_migrations","document_crypto_keys","crypto_nonce_reservations","crypto_key_state","crypto_restore_runs","crypto_restore_state","crypto_restore_documents","storage_migration_state","share_links","share_guest_sessions","share_audit_events","share_owner_notifications","document_comment_heads","document_comment_events","disclosure_policies","disclosure_audit_events","personal_document_views","document_privacy_policies","document_metadata","document_library_preferences","library_layout_preferences"]);
            let unknown:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_tables WHERE schemaname='public' AND NOT(tablename=ANY($1)))").bind(names).fetch_one(&pool).await?;
            if unknown {return Err(LegacyMigrationError::DestinationNotEmpty);}
            Ok::<_,LegacyMigrationError>(information)
        }.await;
        pool.close().await;
        result
    }

    /// Source must be the exact pre-encryption schema through 202610020005.
    /// Legacy workers must be frozen by the caller throughout this operation.
    /// Destination must be an unpublished, distinct cluster with empty tables
    /// and a fresh private blob root. A failed destination stays PENDING.
    pub async fn migrate_legacy<S: BlobStore, T: BlobStore>(
        &self,
        source_url: &str,
        source_blobs: &S,
        target_blobs: &T,
    ) -> Result<LegacyMigrationReport, LegacyMigrationError> {
        let source = legacy_read_only_pool(source_url).await?;
        let result =
            Box::pin(self.migrate_legacy_pool(&source, source_blobs, target_blobs, true)).await;
        source.close().await;
        result
    }

    /// Checks the persisted target again after the caller has restarted it and
    /// constructed a fresh adapter. This method never starts/stops PostgreSQL.
    pub async fn verify_legacy_after_restart<S: BlobStore, T: BlobStore>(
        &self,
        source_url: &str,
        source_blobs: &S,
        target_blobs: &T,
        report: &LegacyMigrationReport,
    ) -> Result<(), LegacyMigrationError> {
        // The caller keeps the restarted destination unpublished and read-only
        // until an operator explicitly opens writes after this comparison.
        let read_only:bool=sqlx::query_scalar("SELECT current_setting('default_transaction_read_only')='on' AND current_setting('transaction_read_only')='on'").fetch_one(&self.crypto_pool).await?;
        if !read_only {
            return Err(LegacyMigrationError::Verification);
        }
        self.readiness()
            .await
            .map_err(|_| LegacyMigrationError::Verification)?;
        Box::pin(self.encryption_readiness()).await?;
        let pool = legacy_read_only_pool(source_url).await?;
        let result=async {
            let mut source=pool.begin().await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *source).await?;
            sqlx::query("SET LOCAL idle_in_transaction_session_timeout=0").execute(&mut *source).await?;
            verify_schema(&mut source,true).await?;
            verify_clusters(&mut source,self).await?;
            let mut fence=self.pool.begin().await?;
            sqlx::query("SET LOCAL idle_in_transaction_session_timeout=0").execute(&mut *fence).await?;
            let acquired:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(MAINTENANCE_LOCK).fetch_one(&mut *fence).await?;
            if !acquired {return Err(LegacyMigrationError::MaintenanceConflict);}
            let complete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM storage_migration_state WHERE singleton=true AND state='COMPLETE')").fetch_one(&mut *fence).await?;
            if !complete {return Err(LegacyMigrationError::Verification);}
            keep_fence_alive(&mut fence,Box::pin(self.verify_migrated(&mut source,source_blobs,target_blobs,report))).await?;
            source.rollback().await?;
            fence.rollback().await?;
            Ok::<_,LegacyMigrationError>(())
        }.await;
        pool.close().await;
        result
    }

    async fn migrate_legacy_pool<S: BlobStore, T: BlobStore>(
        &self,
        source: &PgPool,
        source_blobs: &S,
        target_blobs: &T,
        require_distinct_cluster: bool,
    ) -> Result<LegacyMigrationReport, LegacyMigrationError> {
        let mut source_tx = source.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *source_tx)
            .await?;
        sqlx::query("SET LOCAL idle_in_transaction_session_timeout=0")
            .execute(&mut *source_tx)
            .await?;
        sqlx::query("SET LOCAL TimeZone='UTC'")
            .execute(&mut *source_tx)
            .await?;
        // Legacy application processes do not implement this fence; the caller
        // still has to stop those workers. It also excludes supported tooling.
        let source_fenced =
            sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock_shared($1)")
                .bind(MAINTENANCE_LOCK)
                .fetch_one(&mut *source_tx)
                .await?;
        if !source_fenced {
            return Err(LegacyMigrationError::MaintenanceConflict);
        }
        verify_schema(&mut source_tx, true).await?;
        if require_distinct_cluster {
            verify_clusters(&mut source_tx, self).await?;
        }
        let mut fence = self.pool.begin().await?;
        sqlx::query("SET LOCAL idle_in_transaction_session_timeout=0")
            .execute(&mut *fence)
            .await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *fence)
            .await?;
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1)")
            .bind(MAINTENANCE_LOCK)
            .fetch_one(&mut *fence)
            .await?;
        if !acquired {
            return Err(LegacyMigrationError::MaintenanceConflict);
        }
        verify_schema(&mut fence, false).await?;
        self.require_empty_migration_target().await?;
        // The marker and exclusive fence commit on the SAME connection. If that
        // connection dies before commit, no destination content has been copied.
        sqlx::query("INSERT INTO storage_migration_state(singleton,state) VALUES(true,'PENDING')")
            .execute(&mut *fence)
            .await?;
        fence.commit().await?;
        // PENDING rejects ordinary writers while the exclusive fence is renewed.
        let mut fence = self.pool.begin().await?;
        sqlx::query("SET LOCAL idle_in_transaction_session_timeout=0")
            .execute(&mut *fence)
            .await?;
        sqlx::query("SET LOCAL synchronous_commit=on")
            .execute(&mut *fence)
            .await?;
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1)")
            .bind(MAINTENANCE_LOCK)
            .fetch_one(&mut *fence)
            .await?;
        if !acquired {
            return Err(LegacyMigrationError::MaintenanceConflict);
        }
        let work = self.migrate_legacy_locked(&mut source_tx, source_blobs, target_blobs);
        let result = keep_fence_alive(&mut fence, Box::pin(work)).await;
        match result {
            Ok(report) => {
                source_tx.rollback().await?;
                let changed=sqlx::query("UPDATE storage_migration_state SET state='COMPLETE',completed_at=now() WHERE singleton=true AND state='PENDING'")
                    .execute(&mut *fence).await?.rows_affected();
                if changed != 1 {
                    return Err(LegacyMigrationError::Verification);
                }
                fence.commit().await?;
                Ok(report)
            }
            Err(error) => {
                source_tx.rollback().await?;
                fence.rollback().await?;
                Err(error)
            }
        }
    }

    async fn migrate_legacy_locked<S: BlobStore, T: BlobStore>(
        &self,
        source: &mut Transaction<'_, Postgres>,
        source_blobs: &S,
        target_blobs: &T,
    ) -> Result<LegacyMigrationReport, LegacyMigrationError> {
        let mut report = LegacyMigrationReport::default();
        for table in METADATA {
            let mut rows = sqlx::query_scalar::<_, String>(table.select).fetch(&mut **source);
            while let Some(metadata) = rows.try_next().await? {
                let metadata = Zeroizing::new(metadata);
                durable_insert(self, table.insert, &metadata).await?;
                report.metadata_rows += 1;
            }
        }
        let mut after = None;
        loop {
            let documents = sqlx::query_as::<_, DocumentRow>("SELECT document_id,workspace_id,head_version,preferred_snapshot_id FROM documents WHERE $1::uuid IS NULL OR document_id>$1 ORDER BY document_id LIMIT 128")
                .bind(after).fetch_all(&mut **source).await?;
            if documents.is_empty() {
                break;
            }
            for document in documents {
                self.migrate_document(source, source_blobs, target_blobs, &document, &mut report)
                    .await?;
                after = Some(document.document_id);
                report.documents += 1;
            }
        }
        Box::pin(self.verify_migrated(source, source_blobs, target_blobs, &report)).await?;
        // The outer exclusive transaction activates only after this succeeds.
        Ok(report)
    }

    async fn require_empty_migration_target(&self) -> Result<(), LegacyMigrationError> {
        // A verified encrypted rollback archive may already have burned nonce
        // reservations in this fresh destination. Retain that durable ledger.
        let nonempty = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users) OR EXISTS(SELECT 1 FROM workspaces) OR EXISTS(SELECT 1 FROM memberships) OR EXISTS(SELECT 1 FROM development_accounts) OR EXISTS(SELECT 1 FROM development_sessions) OR EXISTS(SELECT 1 FROM documents) OR EXISTS(SELECT 1 FROM document_permissions) OR EXISTS(SELECT 1 FROM audit_events) OR EXISTS(SELECT 1 FROM document_operations) OR EXISTS(SELECT 1 FROM document_snapshots) OR EXISTS(SELECT 1 FROM export_audit_events) OR EXISTS(SELECT 1 FROM document_crypto_keys) OR EXISTS(SELECT 1 FROM crypto_key_state) OR EXISTS(SELECT 1 FROM crypto_restore_runs) OR EXISTS(SELECT 1 FROM storage_migration_state)")
            .fetch_one(&self.crypto_pool).await?;
        if nonempty {
            return Err(LegacyMigrationError::DestinationNotEmpty);
        }
        // These destination-only tables may be introduced by a later supported
        // migration. Their rows must never be mixed into a legacy import.
        for table in [
            "share_links",
            "share_guest_sessions",
            "share_audit_events",
            "share_owner_notifications",
            "document_comment_heads",
            "document_comment_events",
        ] {
            let present: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(format!("public.{table}"))
                .fetch_one(&self.crypto_pool)
                .await?;
            if present
                && sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(format!(
                    "SELECT EXISTS(SELECT 1 FROM {table})"
                )))
                .fetch_one(&self.crypto_pool)
                .await?
            {
                return Err(LegacyMigrationError::DestinationNotEmpty);
            }
        }
        Ok(())
    }

    async fn migrate_document<S: BlobStore, T: BlobStore>(
        &self,
        source: &mut Transaction<'_, Postgres>,
        source_blobs: &S,
        target_blobs: &T,
        row: &DocumentRow,
        report: &mut LegacyMigrationReport,
    ) -> Result<(), LegacyMigrationError> {
        let document = document_id(row.document_id)?;
        let workspace = WorkspaceId::from_bytes(*row.workspace_id.as_bytes())
            .map_err(|_| LegacyMigrationError::InvalidJournal)?;
        let head = version(row.head_version)?;
        self.ensure_document_key_guarded(workspace, document)
            .await?;
        let mut book = Workbook::empty(document);
        for n in 1..=head.get() {
            let operation = sqlx::query_as::<_, OperationRow>(OPERATION)
                .bind(row.document_id)
                .bind(n as i64)
                .fetch_optional(&mut **source)
                .await?
                .ok_or(LegacyMigrationError::InvalidJournal)?;
            let event = legacy_event(document, &operation)?;
            book.apply_event(&event)
                .map_err(|_| LegacyMigrationError::InvalidJournal)?;
            Box::pin(self.migrate_operation(document, &operation)).await?;
            report.operation_records += 1;
            Box::pin(self.snapshots_at_version(
                source,
                source_blobs,
                target_blobs,
                &book,
                report,
                false,
            ))
            .await?;
        }
        let bad = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM document_operations WHERE document_id=$1 AND committed_version>$2) OR EXISTS(SELECT 1 FROM document_snapshots WHERE document_id=$1 AND snapshot_version>$2) OR EXISTS(SELECT 1 FROM export_audit_events WHERE document_id=$1 AND document_version>$2)")
            .bind(row.document_id).bind(row.head_version).fetch_one(&mut **source).await?;
        if bad || book.version() != head {
            return Err(LegacyMigrationError::InvalidJournal);
        }
        self.migrate_exports(source, row, false)
            .await
            .map(|count| report.export_audit_records += count)?;
        if let Some(preferred) = row.preferred_snapshot_id {
            let active = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM document_snapshots WHERE document_id=$1 AND snapshot_id=$2 AND state='ACTIVE' AND payload_checksum<>decode(repeat('00',32),'hex'))")
                .bind(row.document_id).bind(preferred).fetch_one(&self.crypto_pool).await?;
            if !active {
                return Err(LegacyMigrationError::InvalidSnapshot);
            }
            let mut tx = self.crypto_pool.begin().await?;
            sqlx::query("SET LOCAL synchronous_commit=on")
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE documents SET preferred_snapshot_id=$2 WHERE document_id=$1")
                .bind(row.document_id)
                .bind(preferred)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    async fn migrate_operation(
        &self,
        document: DocumentId,
        row: &OperationRow,
    ) -> Result<(), LegacyMigrationError> {
        let n = version(row.committed_version)?.get();
        let schema =
            u32::try_from(row.schema_version).map_err(|_| LegacyMigrationError::InvalidJournal)?;
        let payload = self
            .seal_record_for_migration(
                document,
                n,
                "journal",
                row.operation_id,
                schema,
                &row.payload,
                MAX_EVENT_BYTES,
            )
            .await?;
        let fingerprint = self
            .seal_record_for_migration(
                document,
                n,
                "journal-fingerprint",
                row.operation_id,
                schema,
                &row.command_fingerprint,
                32,
            )
            .await?;
        let hash = self
            .seal_record_for_migration(
                document,
                n,
                "journal-hash",
                row.operation_id,
                schema,
                &row.canonical_event_hash,
                32,
            )
            .await?;
        let mut fields = json_object(&row.metadata)?;
        fields.insert("payload".into(), bytea(&payload));
        fields.insert("payload_checksum".into(), bytea(&codec::checksum(&payload)));
        fields.insert("command_fingerprint".into(), bytea(&fingerprint));
        fields.insert("canonical_event_hash".into(), bytea(&hash));
        durable_insert(self,"INSERT INTO document_operations SELECT * FROM json_populate_record(NULL::document_operations,$1::json)",&encoded_json(fields)?).await
    }
}

async fn legacy_read_only_pool(url: &str) -> Result<PgPool, LegacyMigrationError> {
    let parsed = PgConnectOptions::from_str(url).map_err(|_| LegacyMigrationError::Database)?;
    let options = crate::postgres::restored_options(parsed).options([
        ("default_transaction_read_only", "on"),
        ("idle_in_transaction_session_timeout", "0"),
    ]);
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
        .map_err(Into::into)
}

async fn verify_schema(
    tx: &mut Transaction<'_, Postgres>,
    legacy: bool,
) -> Result<(), LegacyMigrationError> {
    let applied = sqlx::query(
        "SELECT version,checksum,success FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut **tx)
    .await?;
    let expected: Vec<_> = MIGRATIONS
        .iter()
        .filter(|m| !legacy || m.version <= LAST_LEGACY_MIGRATION)
        .collect();
    if applied.len() != expected.len()
        || !applied.iter().zip(expected).all(|(actual, expected)| {
            actual.try_get::<i64, _>("version").ok() == Some(expected.version)
                && actual.try_get::<Vec<u8>, _>("checksum").ok().as_deref()
                    == Some(expected.checksum.as_ref())
                && actual.try_get::<bool, _>("success").ok() == Some(true)
        })
    {
        return Err(if legacy {
            LegacyMigrationError::UnsupportedSource
        } else {
            LegacyMigrationError::DestinationNotEmpty
        });
    }
    if legacy {
        let crypto = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_tables WHERE schemaname='public' AND (tablename LIKE 'crypto\\_%' ESCAPE '\\' OR tablename IN ('document_crypto_keys','storage_migration_state')))").fetch_one(&mut **tx).await?;
        if crypto {
            return Err(LegacyMigrationError::UnsupportedSource);
        }
        let names: Vec<_> = LEGACY_COLUMNS.iter().map(|(name, _)| *name).collect();
        let unknown:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_tables WHERE schemaname='public' AND tablename<>'_sqlx_migrations' AND NOT(tablename=ANY($1)))").bind(&names).fetch_one(&mut **tx).await?;
        if unknown {
            return Err(LegacyMigrationError::UnsupportedSource);
        }
        let actual:Vec<(String,String,String)>=sqlx::query_as("SELECT table_name,column_name,udt_name FROM information_schema.columns WHERE table_schema='public' AND table_name=ANY($1)")
            .bind(names).fetch_all(&mut **tx).await?;
        let actual: BTreeSet<_> = actual.into_iter().collect();
        let expected: BTreeSet<_> = LEGACY_COLUMNS
            .iter()
            .flat_map(|(table, columns)| {
                columns.split(',').map(move |column| {
                    (
                        (*table).to_owned(),
                        column.to_owned(),
                        legacy_column_type(column).to_owned(),
                    )
                })
            })
            .collect();
        if actual != expected {
            return Err(LegacyMigrationError::UnsupportedSource);
        }
    } else {
        let durable:bool=sqlx::query_scalar("SELECT current_setting('fsync')='on' AND current_setting('full_page_writes')='on' AND current_setting('synchronous_commit')='on'")
            .fetch_one(&mut **tx).await?;
        if !durable {
            return Err(LegacyMigrationError::DestinationNotEmpty);
        }
    }
    Ok(())
}

async fn verify_clusters(
    source: &mut Transaction<'_, Postgres>,
    target: &PgDatabase,
) -> Result<(), LegacyMigrationError> {
    let sql =
        "SELECT system_identifier::text,current_setting('data_directory') FROM pg_control_system()";
    let src: (String, String) = sqlx::query_as(sql).fetch_one(&mut **source).await?;
    let dst: (String, String) = sqlx::query_as(sql).fetch_one(&target.crypto_pool).await?;
    if src.0 == dst.0 || src.1 == dst.1 {
        Err(LegacyMigrationError::SameCluster)
    } else {
        Ok(())
    }
}

async fn read_snapshot<B: BlobStore>(
    blobs: &B,
    row: &SnapshotRow,
    encrypted: bool,
) -> Result<Option<Zeroizing<Vec<u8>>>, LegacyMigrationError> {
    let length =
        usize::try_from(row.byte_length).map_err(|_| LegacyMigrationError::InvalidSnapshot)?;
    let limit = codec::MAX_SNAPSHOT_BYTES
        + if encrypted {
            CRYPTO_ENVELOPE_OVERHEAD
        } else {
            0
        };
    if length == 0 || length > limit || row.payload_checksum.len() != 32 {
        return Err(LegacyMigrationError::InvalidSnapshot);
    }
    let key = BlobKey::parse(&row.object_key.to_string())
        .map_err(|_| LegacyMigrationError::InvalidSnapshot)?;
    let download = match blobs.get(&key).await {
        Ok(download) => download,
        Err(BlobStoreError::NotFound) if matches!(row.state.as_str(), "UPLOADING" | "REJECTED") => {
            return Ok(None);
        }
        Err(BlobStoreError::NotFound) => return Err(LegacyMigrationError::InvalidSnapshot),
        Err(error) => return Err(error.into()),
    };
    if download.metadata.size_bytes != length as u64
        || download.metadata.checksum.as_bytes().as_slice() != row.payload_checksum
    {
        return Err(LegacyMigrationError::InvalidSnapshot);
    }
    let mut body = Zeroizing::new(Vec::with_capacity(length));
    let mut chunks = download.stream;
    while let Some(chunk) = chunks.try_next().await? {
        if body.len().saturating_add(chunk.len()) > length {
            return Err(LegacyMigrationError::InvalidSnapshot);
        }
        body.extend_from_slice(&chunk);
    }
    if body.len() != length || codec::checksum(&body).as_slice() != row.payload_checksum {
        return Err(LegacyMigrationError::InvalidSnapshot);
    }
    Ok(Some(body))
}

fn validate_snapshot(
    row: &SnapshotRow,
    plaintext: &[u8],
    book: &Workbook,
) -> Result<(), LegacyMigrationError> {
    if codec::snapshot_schema_version(plaintext)
        .map_err(|_| LegacyMigrationError::InvalidSnapshot)?
        != row.snapshot_schema_version as u32
    {
        return Err(LegacyMigrationError::InvalidSnapshot);
    }
    let image = codec::decode_snapshot(
        plaintext,
        book.document_id(),
        version(row.snapshot_version)?,
        version(row.last_axis_change_version)?,
        bytes32(&row.state_hash)?,
        &row.writer_engine_version,
        &row.writer_compatibility_profile,
    )
    .map_err(|_| LegacyMigrationError::InvalidSnapshot)?;
    if image.hash() != book.hash()
        || image.last_axis_change_version() != book.last_axis_change_version()
    {
        return Err(LegacyMigrationError::InvalidSnapshot);
    }
    Ok(())
}

impl PgDatabase {
    async fn snapshots_at_version<S: BlobStore, T: BlobStore>(
        &self,
        source: &mut Transaction<'_, Postgres>,
        source_blobs: &S,
        target_blobs: &T,
        book: &Workbook,
        report: &mut LegacyMigrationReport,
        verify: bool,
    ) -> Result<(), LegacyMigrationError> {
        let mut after = None;
        loop {
            let rows = sqlx::query_as::<_, SnapshotRow>(SNAPSHOTS)
                .bind(Uuid::from_bytes(*book.document_id().as_bytes()))
                .bind(book.version().get() as i64)
                .bind(after)
                .fetch_all(&mut **source)
                .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                if !(1..=spreadsheet_storage_codec::SNAPSHOT_SCHEMA_VERSION as i32)
                    .contains(&row.snapshot_schema_version)
                    || row.state_hash.as_slice() != book.hash()
                    || version(row.last_axis_change_version)? != book.last_axis_change_version()
                {
                    return Err(LegacyMigrationError::InvalidSnapshot);
                }
                let body = read_snapshot(source_blobs, &row, false).await?;
                if let Some(body) = body.as_ref() {
                    validate_snapshot(&row, body, book)?;
                }
                if verify {
                    Box::pin(self.verify_snapshot(
                        target_blobs,
                        &row,
                        body.as_deref().map(Vec::as_slice),
                        book,
                    ))
                    .await?;
                } else {
                    Box::pin(self.migrate_snapshot(
                        target_blobs,
                        &row,
                        body.as_deref().map(Vec::as_slice),
                        book.document_id(),
                    ))
                    .await?;
                }
                report.snapshot_records += 1;
                if body.is_none() {
                    report.missing_inactive_blobs += 1;
                }
                after = Some(row.snapshot_id);
            }
        }
        Ok(())
    }

    async fn migrate_snapshot<T: BlobStore>(
        &self,
        target_blobs: &T,
        row: &SnapshotRow,
        body: Option<&[u8]>,
        document: DocumentId,
    ) -> Result<(), LegacyMigrationError> {
        let n = version(row.snapshot_version)?.get();
        let schema = u32::try_from(row.snapshot_schema_version)
            .map_err(|_| LegacyMigrationError::InvalidSnapshot)?;
        let state_hash = self
            .seal_record_for_migration(
                document,
                n,
                "snapshot-hash",
                row.snapshot_id,
                schema,
                &row.state_hash,
                32,
            )
            .await?;
        let (length, checksum) = if let Some(body) = body {
            let envelope = self
                .seal_record_for_migration(
                    document,
                    n,
                    "snapshot",
                    row.object_key,
                    schema,
                    body,
                    codec::MAX_SNAPSHOT_BYTES,
                )
                .await?;
            let length = envelope.len() as u64;
            let checksum = codec::checksum(&envelope);
            let key = BlobKey::parse(&row.object_key.to_string())
                .map_err(|_| LegacyMigrationError::InvalidSnapshot)?;
            let written = target_blobs
                .put(
                    &key,
                    Box::pin(stream::once(async move { Ok(Bytes::from(envelope)) })),
                    Checksum::from_bytes(checksum),
                )
                .await?;
            if written.metadata.size_bytes != length
                || written.metadata.checksum.as_bytes() != &checksum
            {
                return Err(LegacyMigrationError::Verification);
            }
            (length, checksum)
        } else {
            // Intentionally no placeholder content or original plaintext checksum.
            let length = u64::try_from(row.byte_length)
                .ok()
                .and_then(|n| n.checked_add(CRYPTO_ENVELOPE_OVERHEAD as u64))
                .ok_or(LegacyMigrationError::InvalidSnapshot)?;
            (length, [0; 32])
        };
        let mut fields = json_object(&row.metadata)?;
        fields.insert("byte_length".into(), Value::from(length));
        fields.insert("payload_checksum".into(), bytea(&checksum));
        fields.insert("state_hash".into(), bytea(&state_hash));
        durable_insert(self,"INSERT INTO document_snapshots SELECT * FROM json_populate_record(NULL::document_snapshots,$1::json)",&encoded_json(fields)?).await
    }

    async fn verify_snapshot<T: BlobStore>(
        &self,
        target_blobs: &T,
        source: &SnapshotRow,
        body: Option<&[u8]>,
        book: &Workbook,
    ) -> Result<(), LegacyMigrationError> {
        let destination = sqlx::query_as::<_, SnapshotRow>(ONE_SNAPSHOT)
            .bind(source.snapshot_id)
            .fetch_optional(&self.crypto_pool)
            .await?
            .ok_or(LegacyMigrationError::Verification)?;
        if destination.metadata != source.metadata {
            return Err(LegacyMigrationError::Verification);
        }
        let schema = u32::try_from(source.snapshot_schema_version)
            .map_err(|_| LegacyMigrationError::InvalidSnapshot)?;
        let hash = self
            .open_record(
                book.document_id(),
                book.version().get(),
                "snapshot-hash",
                source.snapshot_id,
                schema,
                &destination.state_hash,
                32,
            )
            .await?;
        if hash.as_slice() != source.state_hash {
            return Err(LegacyMigrationError::Verification);
        }
        let ciphertext = read_snapshot(target_blobs, &destination, true).await?;
        match (body, ciphertext) {
            (Some(original), Some(ciphertext)) => {
                let plaintext = self
                    .open_record(
                        book.document_id(),
                        book.version().get(),
                        "snapshot",
                        source.object_key,
                        schema,
                        &ciphertext,
                        codec::MAX_SNAPSHOT_BYTES,
                    )
                    .await?;
                if plaintext.as_slice() != original {
                    return Err(LegacyMigrationError::Verification);
                }
                validate_snapshot(source, &plaintext, book)?;
            }
            (None, None) => {
                if destination.payload_checksum != [0; 32]
                    || destination.byte_length
                        != source.byte_length + CRYPTO_ENVELOPE_OVERHEAD as i64
                {
                    return Err(LegacyMigrationError::Verification);
                }
            }
            _ => return Err(LegacyMigrationError::Verification),
        }
        Ok(())
    }

    async fn migrate_exports(
        &self,
        source: &mut Transaction<'_, Postgres>,
        document: &DocumentRow,
        verify: bool,
    ) -> Result<u64, LegacyMigrationError> {
        let mut count = 0;
        let mut after = None;
        loop {
            let rows = sqlx::query_as::<_, ExportRow>(EXPORTS)
                .bind(document.document_id)
                .bind(after)
                .fetch_all(&mut **source)
                .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let n = version(row.document_version)?.get();
                if n == 0
                    || row.document_version > document.head_version
                    || row.payload_checksum.len() != 32
                    || !(1..=codec::MAX_SNAPSHOT_BYTES as i64).contains(&row.payload_size)
                {
                    return Err(LegacyMigrationError::InvalidExportAudit);
                }
                let mut protected = Zeroizing::new(Vec::with_capacity(40));
                protected.extend_from_slice(&row.payload_checksum);
                protected.extend_from_slice(&(row.payload_size as u64).to_be_bytes());
                let id = document_id(document.document_id)?;
                if verify {
                    let target = sqlx::query_as::<_, ExportRow>(ONE_EXPORT)
                        .bind(row.event_id)
                        .fetch_optional(&self.crypto_pool)
                        .await?
                        .ok_or(LegacyMigrationError::Verification)?;
                    if target.metadata != row.metadata || target.payload_size != 0 {
                        return Err(LegacyMigrationError::Verification);
                    }
                    let opened = self
                        .open_record(
                            id,
                            n,
                            "export-audit",
                            row.event_id,
                            1,
                            &target.payload_checksum,
                            40,
                        )
                        .await?;
                    if opened.as_slice() != protected.as_slice() {
                        return Err(LegacyMigrationError::Verification);
                    }
                } else {
                    let sealed = self
                        .seal_record_for_migration(
                            id,
                            n,
                            "export-audit",
                            row.event_id,
                            1,
                            &protected,
                            40,
                        )
                        .await?;
                    let mut fields = json_object(&row.metadata)?;
                    fields.insert("payload_checksum".into(), bytea(&sealed));
                    fields.insert("payload_size".into(), Value::from(0));
                    durable_insert(self,"INSERT INTO export_audit_events SELECT * FROM json_populate_record(NULL::export_audit_events,$1::json)",&encoded_json(fields)?).await?;
                }
                after = Some(row.event_id);
                count += 1;
            }
        }
        Ok(count)
    }

    async fn verify_migrated<S: BlobStore, T: BlobStore>(
        &self,
        source: &mut Transaction<'_, Postgres>,
        source_blobs: &S,
        target_blobs: &T,
        expected: &LegacyMigrationReport,
    ) -> Result<(), LegacyMigrationError> {
        let mut actual = LegacyMigrationReport::default();
        for table in METADATA {
            let mut src = sqlx::query_scalar::<_, String>(table.select).fetch(&mut **source);
            let mut dst = sqlx::query_scalar::<_, String>(table.select).fetch(&self.crypto_pool);
            loop {
                match (src.try_next().await?, dst.try_next().await?) {
                    (Some(src), Some(dst)) if src == dst => actual.metadata_rows += 1,
                    (None, None) => break,
                    _ => return Err(LegacyMigrationError::Verification),
                }
            }
        }
        let mut after = None;
        loop {
            let documents=sqlx::query_as::<_,DocumentRow>("SELECT document_id,workspace_id,head_version,preferred_snapshot_id FROM documents WHERE $1::uuid IS NULL OR document_id>$1 ORDER BY document_id LIMIT 128").bind(after).fetch_all(&mut **source).await?;
            if documents.is_empty() {
                break;
            }
            for row in documents {
                let id = document_id(row.document_id)?;
                let mut source_book = Workbook::empty(id);
                let mut destination_book = Workbook::empty(id);
                let source_metadata = sqlx::query_scalar::<_, String>(
                    "SELECT to_jsonb(d)::text FROM documents d WHERE document_id=$1",
                )
                .bind(row.document_id)
                .fetch_one(&mut **source)
                .await?;
                let destination_metadata = sqlx::query_scalar::<_, String>(
                    "SELECT to_jsonb(d)::text FROM documents d WHERE document_id=$1",
                )
                .bind(row.document_id)
                .fetch_one(&self.crypto_pool)
                .await?;
                if source_metadata != destination_metadata {
                    return Err(LegacyMigrationError::Verification);
                }
                for n in 1..=version(row.head_version)?.get() {
                    let original = sqlx::query_as::<_, OperationRow>(OPERATION)
                        .bind(row.document_id)
                        .bind(n as i64)
                        .fetch_optional(&mut **source)
                        .await?
                        .ok_or(LegacyMigrationError::InvalidJournal)?;
                    let target = sqlx::query_as::<_, OperationRow>(OPERATION)
                        .bind(row.document_id)
                        .bind(n as i64)
                        .fetch_optional(&self.crypto_pool)
                        .await?
                        .ok_or(LegacyMigrationError::Verification)?;
                    let event = legacy_event(id, &original)?;
                    source_book
                        .apply_event(&event)
                        .map_err(|_| LegacyMigrationError::InvalidJournal)?;
                    let target_event = Box::pin(crate::journal::verified_event(
                        self,
                        crate::journal::StoredEventRow {
                            document: id,
                            committed_version: target.committed_version,
                            operation_id: target.operation_id,
                            actor_id: target.actor_id,
                            principal_kind: &target.principal_kind,
                            schema_version: target.schema_version,
                            payload: &target.payload,
                            payload_checksum: &target.payload_checksum,
                            canonical_event_hash: &target.canonical_event_hash,
                        },
                    ))
                    .await
                    .map_err(|_| LegacyMigrationError::Verification)?;
                    destination_book
                        .apply_event(&target_event)
                        .map_err(|_| LegacyMigrationError::Verification)?;
                    let schema = u32::try_from(original.schema_version)
                        .map_err(|_| LegacyMigrationError::InvalidJournal)?;
                    let opened = self
                        .open_record(
                            id,
                            n,
                            "journal",
                            original.operation_id,
                            schema,
                            &target.payload,
                            MAX_EVENT_BYTES,
                        )
                        .await?;
                    let fingerprint = self
                        .open_record(
                            id,
                            n,
                            "journal-fingerprint",
                            original.operation_id,
                            schema,
                            &target.command_fingerprint,
                            32,
                        )
                        .await?;
                    if original.metadata != target.metadata
                        || opened.as_slice() != original.payload
                        || fingerprint.as_slice() != original.command_fingerprint
                        || source_book.hash() != destination_book.hash()
                        || source_book.version() != destination_book.version()
                    {
                        return Err(LegacyMigrationError::Verification);
                    }
                    actual.operation_records += 1;
                    Box::pin(self.snapshots_at_version(
                        source,
                        source_blobs,
                        target_blobs,
                        &source_book,
                        &mut actual,
                        true,
                    ))
                    .await?;
                }
                actual.export_audit_records += self.migrate_exports(source, &row, true).await?;
                after = Some(row.document_id);
                actual.documents += 1;
            }
        }
        let counts:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM document_operations),(SELECT count(*) FROM document_snapshots),(SELECT count(*) FROM export_audit_events)").fetch_one(&self.crypto_pool).await?;
        if &actual != expected
            || counts
                != (
                    expected.operation_records as i64,
                    expected.snapshot_records as i64,
                    expected.export_audit_records as i64,
                )
        {
            return Err(LegacyMigrationError::Verification);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalFilesystemBlobStore, LocalKeyProvider};
    use prost::Message;
    use spreadsheet_application::journal::{AuthenticatedActor, OperationJournal};
    use spreadsheet_domain::workbook::{
        ActorId, CellAddress, CellContent, CellValue, CellWriteKind, ColumnId, Operation,
        OperationId, RowId, SheetId, SheetSeed,
    };
    use sqlx::AssertSqlSafe;
    use sqlx::ConnectOptions;
    use std::{path::PathBuf, sync::Arc};

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const MARKER: &str = "legacy-migration-private-history-marker";
    fn uuid(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    struct Fixture {
        target: PgDatabase,
        source_blobs: LocalFilesystemBlobStore,
        target_blobs: Option<LocalFilesystemBlobStore>,
        target_blob_path: PathBuf,
        database_name: Option<String>,
        external_target_url: Option<String>,
        run_cli: bool,
        source_blob_path: PathBuf,
        _source_directory: Option<tempfile::TempDir>,
        target_directory_path: PathBuf,
        _target_directory: Option<tempfile::TempDir>,
        identity: AuthenticatedActor,
        head_hash: [u8; 32],
    }
    impl Fixture {
        async fn new(source: &PgPool) -> Result<Self, Box<dyn std::error::Error>> {
            let legacy = sqlx::migrate::Migrator::with_migrations(
                MIGRATIONS
                    .iter()
                    .filter(|m| m.version <= LAST_LEGACY_MIGRATION)
                    .cloned()
                    .collect(),
            );
            legacy.run(source).await?;
            let external_target_url = std::env::var("PXEL_LEGACY_MIGRATION_TEST_TARGET_URL").ok();
            let database_name = if external_target_url.is_none() {
                Some(format!("pxel_legacy_{}", Uuid::now_v7().simple()))
            } else {
                None
            };
            let target = if let Some(target_url) = external_target_url.as_deref() {
                let keys = Arc::new(LocalKeyProvider::from_environment()?);
                PgDatabase::connect_with_keys(target_url, 5, keys).await?
            } else {
                let database_name = database_name
                    .as_deref()
                    .ok_or("missing isolated target database name")?;
                // Generated ASCII prefix and UUID, never operator-provided SQL.
                sqlx::query(AssertSqlSafe(format!(
                    "CREATE DATABASE \"{database_name}\""
                )))
                .execute(source)
                .await?;
                let pool = PgPoolOptions::new()
                    .max_connections(5)
                    .connect_with(
                        (*source.connect_options())
                            .clone()
                            .database(database_name)
                            .options([("TimeZone", "UTC")])
                            .disable_statement_logging(),
                    )
                    .await?;
                PgDatabase::for_test(pool)?
            };
            target.migrate().await?;
            let run_cli =
                external_target_url.is_some() && std::env::var_os("PXEL_TEST_SERVER_BIN").is_some();
            let (source_blob_path, source_directory, target_directory_path, target_directory) =
                if run_cli {
                    let data_root = std::env::var_os("PXEL_LEGACY_MIGRATION_TEST_DATA_ROOT")
                        .ok_or("missing private CLI fixture directory")?;
                    let data_root = PathBuf::from(data_root);
                    let source = data_root.join("content-source-blobs");
                    let target = data_root.join("content-migration-output");
                    std::fs::create_dir(&source)?;
                    std::fs::create_dir(&target)?;
                    (source, None, target, None)
                } else {
                    let source = tempfile::tempdir()?;
                    let target = tempfile::tempdir()?;
                    (
                        source.path().to_owned(),
                        Some(source),
                        target.path().to_owned(),
                        Some(target),
                    )
                };
            let target_blob_path = if run_cli {
                target_directory_path.join("target-blobs")
            } else {
                target_directory_path.clone()
            };
            let source_blobs =
                LocalFilesystemBlobStore::new(&source_blob_path, codec::MAX_SNAPSHOT_BYTES as u64)
                    .await?;
            let target_blobs = if run_cli {
                None
            } else {
                Some(
                    LocalFilesystemBlobStore::new(
                        &target_blob_path,
                        (codec::MAX_SNAPSHOT_BYTES + CRYPTO_ENVELOPE_OVERHEAD) as u64,
                    )
                    .await?,
                )
            };
            sqlx::raw_sql("INSERT INTO users(user_id,created_at) VALUES('01010101-0101-0101-0101-010101010101','2020-01-01T01:02:03.123456Z'),('06060606-0606-0606-0606-060606060606','2020-01-02T01:02:03.123456Z');
                INSERT INTO workspaces(workspace_id,created_at) VALUES('02020202-0202-0202-0202-020202020202','2020-02-01T01:02:03.123456Z');
                INSERT INTO memberships(workspace_id,user_id,role,access_epoch,created_at) VALUES('02020202-0202-0202-0202-020202020202','01010101-0101-0101-0101-010101010101','owner',7,'2020-03-01T01:02:03.123456Z'),('02020202-0202-0202-0202-020202020202','06060606-0606-0606-0606-060606060606','member',8,'2020-03-02T01:02:03.123456Z');
                INSERT INTO development_accounts(account,user_id,workspace_id) VALUES('migration-fixture','01010101-0101-0101-0101-010101010101','02020202-0202-0202-0202-020202020202');
                INSERT INTO development_sessions(token_hash,user_id,workspace_id,expires_at) VALUES(decode(repeat('08',32),'hex'),'01010101-0101-0101-0101-010101010101','02020202-0202-0202-0202-020202020202','2040-04-01T01:02:03.123456Z');
                INSERT INTO documents(document_id,workspace_id,head_version,access_epoch,created_at) VALUES('03030303-0303-0303-0303-030303030303','02020202-0202-0202-0202-020202020202',3,11,'2020-05-01T01:02:03.123456Z');
                INSERT INTO document_permissions(workspace_id,document_id,user_id,role) VALUES('02020202-0202-0202-0202-020202020202','03030303-0303-0303-0303-030303030303','01010101-0101-0101-0101-010101010101','owner'),('02020202-0202-0202-0202-020202020202','03030303-0303-0303-0303-030303030303','06060606-0606-0606-0606-060606060606','viewer');
                INSERT INTO audit_events(event_id,workspace_id,document_id,actor_id,target_user_id,event_type,old_role,new_role,access_epoch,created_at) VALUES('07070707-0707-0707-0707-070707070707','02020202-0202-0202-0202-020202020202','03030303-0303-0303-0303-030303030303','01010101-0101-0101-0101-010101010101','06060606-0606-0606-0606-060606060606','permission_granted','editor','viewer',11,'2020-06-01T01:02:03.123456Z');")
                .execute(source).await?;
            let identity = AuthenticatedActor {
                actor_id: ActorId::from_bytes([1; 16])?,
                workspace_id: WorkspaceId::from_bytes([2; 16])?,
            };
            let document = DocumentId::from_bytes([3; 16])?;
            let sheet = SheetId::from_bytes([4; 16])?;
            let address = CellAddress {
                sheet,
                row: RowId::from_bytes([5; 16])?,
                column: ColumnId::from_bytes([6; 16])?,
            };
            let mut book = Workbook::empty(document);
            append(
                source,
                &mut book,
                identity.actor_id,
                9,
                Operation::CreateWorkbook {
                    sheet: SheetSeed {
                        id: sheet,
                        name: "Sheet".into(),
                        rows: vec![address.row],
                        columns: vec![address.column],
                    },
                },
            )
            .await?;
            let content = CellContent::Value(CellValue::Text(MARKER.into()));
            append(
                source,
                &mut book,
                identity.actor_id,
                10,
                Operation::CellWrite {
                    address,
                    content: Some(content.clone()),
                    previous: None,
                    kind: Some(CellWriteKind::SetValue),
                    undoes: None,
                    undoes_version: None,
                },
            )
            .await?;
            // This fixture is a pre-encryption database, whose final snapshot
            // writer was schema 6. Keep exercising that reader after new writers
            // are added, instead of giving a legacy database future metadata.
            let encoded = codec::encode_snapshot(&book, "engine-v1", "pxel-v1")?;
            let mut envelope = codec::v1::SnapshotEnvelope::decode(encoded.as_slice())?;
            envelope.snapshot_schema_version = 6;
            for sheet in &mut envelope.sheets {
                sheet.settings = None;
            }
            let snapshot = envelope.encode_to_vec();
            assert_eq!(
                codec::decode_snapshot(
                    &snapshot,
                    book.document_id(),
                    book.version(),
                    book.last_axis_change_version(),
                    book.hash(),
                    "engine-v1",
                    "pxel-v1",
                )?
                .hash(),
                book.hash(),
            );
            let checksum = codec::checksum(&snapshot);
            let key = BlobKey::parse(&uuid(12).to_string())?;
            source_blobs
                .put(
                    &key,
                    Box::pin(stream::once(async move { Ok(Bytes::from(snapshot)) })),
                    Checksum::from_bytes(checksum),
                )
                .await?;
            for (id, state, object) in [(12, "ACTIVE", 12), (13, "REJECTED", 13)] {
                sqlx::query("INSERT INTO document_snapshots(snapshot_id,document_id,snapshot_version,last_axis_change_version,snapshot_schema_version,object_key,byte_length,payload_checksum,state_hash,writer_engine_version,writer_compatibility_profile,state,created_at,verified_at,activated_at,cleanup_attempted_at) VALUES($1,$2,2,1,6,$3,$4,$5,$6,'engine-v1','pxel-v1',$7,'2020-07-01T01:02:03.123456Z','2020-07-02T01:02:03.123456Z','2020-07-03T01:02:03.123456Z','2020-07-04T01:02:03.123456Z')")
                    .bind(uuid(id)).bind(uuid(3)).bind(uuid(object)).bind(source_blobs.head(&key).await?.size_bytes as i64).bind(checksum.as_slice()).bind(book.hash().as_slice()).bind(state).execute(source).await?;
            }
            sqlx::query("UPDATE documents SET preferred_snapshot_id=$1 WHERE document_id=$2")
                .bind(uuid(12))
                .bind(uuid(3))
                .execute(source)
                .await?;
            append(
                source,
                &mut book,
                identity.actor_id,
                11,
                Operation::CellWrite {
                    address,
                    content: None,
                    previous: Some(content),
                    kind: None,
                    undoes: Some(OperationId::from_bytes([10; 16])?),
                    undoes_version: Some(DocumentVersion::new(2)?),
                },
            )
            .await?;
            sqlx::query("INSERT INTO export_audit_events(event_id,workspace_id,document_id,actor_id,document_version,payload_checksum,payload_size,created_at) VALUES($1,$2,$3,$4,2,$5,123,'2020-08-01T01:02:03.123456Z')")
                .bind(uuid(14)).bind(uuid(2)).bind(uuid(3)).bind(uuid(1)).bind(checksum.as_slice()).execute(source).await?;
            Ok(Self {
                target,
                source_blobs,
                target_blobs,
                target_blob_path,
                source_blob_path,
                database_name,
                external_target_url,
                run_cli,
                _source_directory: source_directory,
                target_directory_path,
                _target_directory: target_directory,
                identity,
                head_hash: book.hash(),
            })
        }
        async fn migrate(
            &mut self,
            source: &PgPool,
        ) -> Result<LegacyMigrationReport, LegacyMigrationError> {
            if let Some(target_url) = self.external_target_url.as_deref() {
                let database_name: String = sqlx::query_scalar("SELECT current_database()")
                    .fetch_one(source)
                    .await?;
                let base_url =
                    std::env::var("DATABASE_URL").map_err(|_| LegacyMigrationError::Database)?;
                let mut source_url =
                    url::Url::parse(&base_url).map_err(|_| LegacyMigrationError::Database)?;
                source_url.set_path(&database_name);
                let source_url = source_url.to_string();
                if let Some(server_bin) = std::env::var_os("PXEL_TEST_SERVER_BIN") {
                    let recovery_url = std::env::var("PXEL_RESTORE_DATABASE_URL")
                        .map_err(|_| LegacyMigrationError::Database)?;
                    let mut recovery_url = url::Url::parse(&recovery_url)
                        .map_err(|_| LegacyMigrationError::Database)?;
                    recovery_url.set_path(&database_name);
                    let recovery_url = recovery_url.to_string();
                    let archive = self.target_directory_path.join("rollback.pxel-backup");
                    let verification = self.target_directory_path.join("rollback-verified");
                    let summary_path = self.target_directory_path.join("migration-cli.stdout");
                    let stderr_path = self.target_directory_path.join("migration-cli.stderr");
                    let stdout_file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&summary_path)
                        .map_err(|_| LegacyMigrationError::Database)?;
                    let stderr_file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&stderr_path)
                        .map_err(|_| LegacyMigrationError::Database)?;
                    let output_markers = [
                        MARKER.as_bytes().to_vec(),
                        source_url.as_bytes().to_vec(),
                        target_url.as_bytes().to_vec(),
                        recovery_url.as_bytes().to_vec(),
                    ];
                    let source_blobs = self.source_blob_path.clone();
                    let target_blobs = self.target_blob_path.clone();
                    let target_url = target_url.to_owned();
                    let cli_target_url = target_url.clone();
                    self.target.close().await;
                    let status = tokio::task::spawn_blocking(move || {
                        std::process::Command::new(server_bin)
                            .args(["storage", "migrate-legacy", "--maintenance-confirmed"])
                            .env("PXEL_LEGACY_SOURCE_DATABASE_URL", source_url)
                            .env("DATABASE_URL", cli_target_url)
                            .env("PXEL_RESTORE_DATABASE_URL", recovery_url)
                            .env("PXEL_LEGACY_SOURCE_BLOB_ROOT", source_blobs)
                            .env("PXEL_BLOB_ROOT", target_blobs)
                            .env("PXEL_MIGRATION_ROLLBACK_ARCHIVE", archive)
                            .env("PXEL_MIGRATION_VERIFY_ROOT", verification)
                            .stdout(std::process::Stdio::from(stdout_file))
                            .stderr(std::process::Stdio::from(stderr_file))
                            .status()
                    })
                    .await
                    .map_err(|_| LegacyMigrationError::Database)?
                    .map_err(|_| LegacyMigrationError::Database)?;
                    if !status.success() {
                        return Err(LegacyMigrationError::Database);
                    }
                    let summary = std::fs::read_to_string(summary_path)
                        .map_err(|_| LegacyMigrationError::Verification)?;
                    let stderr = std::fs::read(stderr_path)
                        .map_err(|_| LegacyMigrationError::Verification)?;
                    for output in [summary.as_bytes(), stderr.as_slice()] {
                        for marker in &output_markers {
                            if !marker.is_empty()
                                && output.windows(marker.len()).any(|window| window == marker)
                            {
                                return Err(LegacyMigrationError::Verification);
                            }
                        }
                    }
                    if !summary.contains(
                        "documents=1, journal_records=3, snapshots=2, export_audits=1, metadata_rows=11, missing_inactive_blobs=1",
                    ) {
                        return Err(LegacyMigrationError::Verification);
                    }
                    let keys = Arc::new(
                        LocalKeyProvider::from_environment()
                            .map_err(|_| LegacyMigrationError::Encryption)?,
                    );
                    self.target =
                        PgDatabase::connect_for_restored_cluster(target_url.as_str(), 5, keys)
                            .await
                            .map_err(|_| LegacyMigrationError::Database)?;
                    self.target_blobs = Some(
                        LocalFilesystemBlobStore::new(
                            &self.target_blob_path,
                            (codec::MAX_SNAPSHOT_BYTES + CRYPTO_ENVELOPE_OVERHEAD) as u64,
                        )
                        .await?,
                    );
                    return Ok(LegacyMigrationReport {
                        metadata_rows: 11,
                        documents: 1,
                        operation_records: 3,
                        snapshot_records: 2,
                        export_audit_records: 1,
                        missing_inactive_blobs: 1,
                    });
                }
                PgDatabase::inspect_legacy_source(&source_url).await?;
                PgDatabase::inspect_legacy_destination(target_url).await?;
                return Box::pin(
                    self.target.migrate_legacy(
                        &source_url,
                        &self.source_blobs,
                        self.target_blobs
                            .as_ref()
                            .ok_or(LegacyMigrationError::Blob)?,
                    ),
                )
                .await;
            }
            // SQLx isolated source/target databases share its test cluster. The
            // public API always enforces physical cluster separation separately.
            Box::pin(
                self.target.migrate_legacy_pool(
                    source,
                    &self.source_blobs,
                    self.target_blobs
                        .as_ref()
                        .ok_or(LegacyMigrationError::Blob)?,
                    false,
                ),
            )
            .await
        }
        async fn finish(self, source: &PgPool) -> TestResult {
            self.target.close().await;
            if let Some(database_name) = self.database_name {
                sqlx::query(AssertSqlSafe(format!("DROP DATABASE \"{database_name}\"")))
                    .execute(source)
                    .await?;
            }
            Ok(())
        }
    }
    async fn append(
        pool: &PgPool,
        book: &mut Workbook,
        actor: ActorId,
        id: u8,
        operation: Operation,
    ) -> TestResult {
        let next = DocumentVersion::new(book.version().get() + 1)?;
        let candidate = book.candidate(&operation, next)?;
        let event = CommittedEvent {
            document_id: book.document_id(),
            operation_id: OperationId::from_bytes([id; 16])?,
            principal: EventPrincipal::User(actor),
            version: next,
            operation,
            state_hash: candidate.hash(),
        };
        let payload = Zeroizing::new(codec::encode(&event)?);
        sqlx::query("INSERT INTO document_operations(document_id,committed_version,operation_id,actor_id,principal_kind,schema_version,payload,payload_checksum,command_fingerprint,canonical_event_hash,created_at) VALUES($1,$2,$3,$4,'user',$5,$6,$7,$8,$9,'2020-09-01T01:02:03.123456Z')")
            .bind(Uuid::from_bytes(*event.document_id.as_bytes())).bind(next.get() as i64).bind(uuid(id)).bind(Uuid::from_bytes(*actor.as_bytes())).bind(codec::schema_version(&event.operation) as i32).bind(&*payload).bind(codec::checksum(&payload).as_slice()).bind([id;32].as_slice()).bind(codec::event_hash(&event)?.as_slice()).execute(pool).await?;
        book.apply_event(&event)?;
        Ok(())
    }

    #[sqlx::test(migrations = false)]
    async fn legacy_history_metadata_snapshots_and_export_audits_are_verified_and_encrypted(
        pool: PgPool,
    ) -> TestResult {
        let mut fixture = Box::pin(Fixture::new(&pool)).await?;
        let source_before: Vec<String> = sqlx::query_scalar(
            "SELECT to_jsonb(o)::text FROM document_operations o ORDER BY committed_version",
        )
        .fetch_all(&pool)
        .await?;
        let snapshot_before = read_snapshot(
            &fixture.source_blobs,
            &sqlx::query_as::<_, SnapshotRow>(ONE_SNAPSHOT)
                .bind(uuid(12))
                .fetch_one(&pool)
                .await?,
            false,
        )
        .await?
        .ok_or("missing source fixture")?;
        let report = fixture.migrate(&pool).await?;
        assert_eq!(
            report,
            LegacyMigrationReport {
                metadata_rows: 11,
                documents: 1,
                operation_records: 3,
                snapshot_records: 2,
                export_audit_records: 1,
                missing_inactive_blobs: 1
            }
        );
        assert_eq!(
            source_before,
            sqlx::query_scalar::<_, String>(
                "SELECT to_jsonb(o)::text FROM document_operations o ORDER BY committed_version"
            )
            .fetch_all(&pool)
            .await?
        );
        let source_after = read_snapshot(
            &fixture.source_blobs,
            &sqlx::query_as::<_, SnapshotRow>(ONE_SNAPSHOT)
                .bind(uuid(12))
                .fetch_one(&pool)
                .await?,
            false,
        )
        .await?
        .ok_or("missing source fixture")?;
        assert_eq!(snapshot_before.as_slice(), source_after.as_slice());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM storage_migration_state")
                .fetch_one(&fixture.target.pool)
                .await?,
            "COMPLETE"
        );
        fixture.target.encryption_readiness().await?;
        let has_kind:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='public' AND table_name='users' AND column_name='identity_kind')").fetch_one(&fixture.target.pool).await?;
        if has_kind {
            let all_members: bool = sqlx::query_scalar(
                "SELECT NOT EXISTS(SELECT 1 FROM users WHERE identity_kind<>'member')",
            )
            .fetch_one(&fixture.target.pool)
            .await?;
            assert!(all_members);
        }
        let document = DocumentId::from_bytes([3; 16])?;
        if fixture.run_cli {
            let head: i64 =
                sqlx::query_scalar("SELECT head_version FROM documents WHERE document_id=$1")
                    .bind(uuid(3))
                    .fetch_one(&fixture.target.pool)
                    .await?;
            assert_eq!(head, 3);
        } else {
            let target_blobs = fixture
                .target_blobs
                .as_ref()
                .ok_or("migration fixture has no target blob store")?;
            let recovered = Box::pin(fixture.target.recover_document(
                target_blobs,
                fixture.identity,
                document,
            ))
            .await?;
            assert_eq!(recovered.workbook.version().get(), 3);
            assert_eq!(recovered.workbook.hash(), fixture.head_hash);
            assert_eq!(recovered.snapshot_version, Some(DocumentVersion::new(2)?));
        }
        if !fixture.run_cli {
            let history = fixture
                .target
                .undo_history(fixture.identity, document)
                .await?;
            assert_eq!(history.redo.len(), 1);
        }
        let operations = sqlx::query_as::<_, OperationRow>(OPERATION)
            .bind(uuid(3))
            .bind(2_i64)
            .fetch_one(&fixture.target.pool)
            .await?;
        assert!(
            !operations
                .payload
                .windows(MARKER.len())
                .any(|bytes| bytes == MARKER.as_bytes())
        );
        assert!(codec::decode(&operations.payload).is_err());
        assert_eq!(operations.command_fingerprint.len(), 88);
        assert_eq!(operations.canonical_event_hash.len(), 88);
        let physical = tokio::fs::read(fixture.target_blob_path.join(uuid(12).to_string())).await?;
        assert!(
            !physical
                .windows(MARKER.len())
                .any(|bytes| bytes == MARKER.as_bytes())
        );
        assert!(!fixture.target_blob_path.join(uuid(13).to_string()).exists());
        let tombstone = sqlx::query_as::<_, SnapshotRow>(ONE_SNAPSHOT)
            .bind(uuid(13))
            .fetch_one(&fixture.target.pool)
            .await?;
        assert_eq!(tombstone.state, "REJECTED");
        assert_eq!(tombstone.payload_checksum, [0; 32]);
        let export = sqlx::query_as::<_, ExportRow>(ONE_EXPORT)
            .bind(uuid(14))
            .fetch_one(&fixture.target.pool)
            .await?;
        assert_eq!(export.payload_size, 0);
        assert_eq!(export.payload_checksum.len(), 96);
        assert!(
            sqlx::query("UPDATE export_audit_events SET document_version=1")
                .execute(&fixture.target.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM export_audit_events")
                .execute(&fixture.target.pool)
                .await
                .is_err()
        );
        fixture.finish(&pool).await
    }

    #[sqlx::test(migrations = false)]
    async fn encrypted_rollback_backup_nonce_reservations_survive_migration(
        pool: PgPool,
    ) -> TestResult {
        let mut fixture = Box::pin(Fixture::new(&pool)).await?;
        let mut input = std::io::Cursor::new(Vec::<u8>::new());
        let mut archive = Vec::new();
        fixture
            .target
            .encrypt_backup(b"legacy rollback", &mut input, &mut archive)
            .await?;
        let reserved: Vec<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT physical_key_id,nonce FROM crypto_nonce_reservations ORDER BY physical_key_id,nonce",
        ).fetch_all(&fixture.target.pool).await?;
        assert_eq!(reserved.len(), 3);
        assert_eq!(fixture.migrate(&pool).await?.operation_records, 3);
        for (key, nonce) in reserved {
            let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM crypto_nonce_reservations WHERE physical_key_id=$1 AND nonce=$2)")
                .bind(key).bind(nonce).fetch_one(&fixture.target.pool).await?;
            assert!(retained);
        }
        crate::verify_backup(&*fixture.target.keys, &mut std::io::Cursor::new(archive)).await?;
        fixture.finish(&pool).await
    }

    #[sqlx::test(migrations = false)]
    async fn invalid_legacy_head_keeps_partial_destination_unservable(pool: PgPool) -> TestResult {
        let mut fixture = Box::pin(Fixture::new(&pool)).await?;
        sqlx::query("UPDATE documents SET head_version=4")
            .execute(&pool)
            .await?;
        assert!(matches!(
            fixture.migrate(&pool).await,
            Err(LegacyMigrationError::InvalidJournal)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM storage_migration_state")
                .fetch_one(&fixture.target.pool)
                .await?,
            "PENDING"
        );
        assert!(fixture.target.encryption_readiness().await.is_err());
        assert!(fixture.target.begin_guarded().await.is_err());
        assert!(matches!(
            fixture.migrate(&pool).await,
            Err(LegacyMigrationError::DestinationNotEmpty)
        ));
        let no_plaintext:bool=sqlx::query_scalar("SELECT bool_and(is_encrypted_storage_envelope(payload) AND is_encrypted_storage_envelope(command_fingerprint) AND is_encrypted_storage_envelope(canonical_event_hash)) FROM document_operations").fetch_one(&fixture.target.pool).await?;
        assert!(no_plaintext);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT head_version FROM documents")
                .fetch_one(&pool)
                .await?,
            4
        );
        fixture.finish(&pool).await
    }

    #[sqlx::test(migrations = false)]
    async fn missing_active_snapshot_fails_with_persistent_pending_barrier(
        pool: PgPool,
    ) -> TestResult {
        let mut fixture = Box::pin(Fixture::new(&pool)).await?;
        fixture
            .source_blobs
            .delete(&BlobKey::parse(&uuid(12).to_string())?)
            .await?;
        assert!(matches!(
            fixture.migrate(&pool).await,
            Err(LegacyMigrationError::InvalidSnapshot)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT state FROM storage_migration_state")
                .fetch_one(&fixture.target.pool)
                .await?,
            "PENDING"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_snapshots")
                .fetch_one(&fixture.target.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_snapshots")
                .fetch_one(&pool)
                .await?,
            2
        );
        fixture.finish(&pool).await
    }

    #[sqlx::test(migrations = false)]
    async fn same_cluster_and_nonempty_destinations_are_rejected_before_activation(
        pool: PgPool,
    ) -> TestResult {
        let mut fixture = Box::pin(Fixture::new(&pool)).await?;
        let mut source = pool.begin().await?;
        assert!(matches!(
            verify_clusters(&mut source, &fixture.target).await,
            Err(LegacyMigrationError::SameCluster)
        ));
        source.rollback().await?;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM storage_migration_state")
                .fetch_one(&fixture.target.pool)
                .await?,
            0
        );
        sqlx::query("INSERT INTO users(user_id) VALUES($1)")
            .bind(uuid(99))
            .execute(&fixture.target.pool)
            .await?;
        assert!(matches!(
            fixture.migrate(&pool).await,
            Err(LegacyMigrationError::DestinationNotEmpty)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM storage_migration_state")
                .fetch_one(&fixture.target.pool)
                .await?,
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM crypto_nonce_reservations")
                .fetch_one(&fixture.target.pool)
                .await?,
            0
        );
        // A matching ledger cannot authorize an out-of-band content column.
        sqlx::query("ALTER TABLE users ADD COLUMN unexpected_content text")
            .execute(&pool)
            .await?;
        assert!(matches!(
            fixture.migrate(&pool).await,
            Err(LegacyMigrationError::UnsupportedSource)
        ));
        sqlx::query("ALTER TABLE users DROP COLUMN unexpected_content")
            .execute(&pool)
            .await?;
        sqlx::query("ALTER TABLE users ALTER COLUMN created_at TYPE text USING created_at::text")
            .execute(&pool)
            .await?;
        assert!(matches!(
            fixture.migrate(&pool).await,
            Err(LegacyMigrationError::UnsupportedSource)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM storage_migration_state")
                .fetch_one(&fixture.target.pool)
                .await?,
            0
        );
        fixture.finish(&pool).await
    }
}
