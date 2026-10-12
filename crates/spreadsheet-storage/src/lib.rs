//! PostgreSQL and private filesystem adapters. No collaborative operation commits yet.

mod blob_backend;
mod crypto;
pub use crypto::{
    BackupError, BackupReport, MAX_BACKUP_MANIFEST_BYTES, decrypt_backup, verify_backup,
};
mod filesystem;
mod maintenance;
mod postgres;
pub use crypto::{
    CRYPTO_ENVELOPE_OVERHEAD, CryptoError, LocalKeyProvider, RestoreReport, RotationReport,
    ciphertext_limit,
};
#[cfg(feature = "legacy-migration")]
pub use crypto::{LegacyMigrationError, LegacyMigrationReport};
pub use maintenance::{PgMaintenanceGuard, PgMaintenanceInfo};
#[cfg(feature = "s3")]
mod s3;

pub use blob_backend::ConfiguredBlobStore;
pub use filesystem::LocalFilesystemBlobStore;
pub use postgres::{AccessEpochChange, AccessEpochListener, DatabaseError, PgDatabase};
#[cfg(feature = "s3")]
pub use s3::S3BlobStore;

mod development_sessions;
mod local_accounts;
pub use local_accounts::{
    LocalAccount, LocalAccountError, LocalCredential, LocalSession, valid_local_password,
    valid_local_username,
};
mod disclosure;
mod personal_views;
mod privacy;
pub use personal_views::{
    ColumnFilter, FilterPredicate, FilterValue, NumberFilterOperator, PersonalFilter,
    PersonalFilters, PersonalView, PersonalViewError, TextFilterOperator,
};
pub use privacy::{PrivacyPolicy, PrivacyRules};
mod exports;
pub use disclosure::{
    DisclosureAction, DisclosureAuditPage, DisclosureError, DisclosureFormat, DisclosurePolicy,
    DisclosureReceipt, DisclosureRules, DisclosureStamp,
};
pub use exports::ExportAuditError;
mod activity;
mod journal;
/// Enforce the durable event codec budget before offering an import preview.
pub fn validate_import_event_size(
    event: &spreadsheet_domain::workbook::CommittedEvent,
) -> Result<usize, spreadsheet_application::journal::JournalError> {
    spreadsheet_storage_codec::encode(event)
        .map(|bytes| bytes.len())
        .map_err(|_| spreadsheet_application::journal::JournalError::PayloadTooLarge)
}
pub use activity::{ActivityEntry, ActivityPage};
pub use development_sessions::DevelopmentSession;
mod permissions;
pub use permissions::{PermissionError, PermissionReceipt, PermissionRole};
mod snapshots;
pub use snapshots::{RecoveryResult, SnapshotCleanupReport, SnapshotError, SnapshotReceipt};
mod shares;
pub use shares::{
    GuestSession, GuestSessionSeed, ShareAttempt, ShareAttemptOutcome, ShareCreateReceipt,
    ShareError, ShareLifetime, ShareLinkSummary, ShareMutationReceipt, ShareOwnerNotification,
    ShareRole, ShareSecretKind, ShareSecretPhc, ShareStatus,
};
mod catalog;
mod comments;
pub use catalog::{
    CatalogEntry, CatalogError, CatalogLease, CatalogPage, LayoutPreference, LibraryLayout,
    LibraryPreference, Metadata, valid_title,
};
pub use comments::{CommentAddress, CommentEntry, CommentError, CommentPage};
pub use journal::DocumentAccessLease;

#[cfg(test)]
mod recovery_benchmark;
