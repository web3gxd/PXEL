//! PostgreSQL and private filesystem adapters. No collaborative operation commits yet.

mod blob_backend;
mod filesystem;
mod postgres;
#[cfg(feature = "s3")]
mod s3;

pub use blob_backend::ConfiguredBlobStore;
pub use filesystem::LocalFilesystemBlobStore;
pub use postgres::{DatabaseError, PgDatabase};
#[cfg(feature = "s3")]
pub use s3::S3BlobStore;

mod development_sessions;
mod exports;
pub use exports::ExportAuditError;
mod journal;
pub use development_sessions::DevelopmentSession;
mod permissions;
pub use permissions::{PermissionError, PermissionReceipt, PermissionRole};
mod snapshots;
pub use snapshots::{RecoveryResult, SnapshotCleanupReport, SnapshotError, SnapshotReceipt};

#[cfg(test)]
mod recovery_benchmark;
