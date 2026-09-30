use spreadsheet_domain::workbook::{
    ActorId, CalculationContext, Command, CommittedEvent, DocumentId, DocumentVersion, OperationId,
    Workbook, WorkspaceId,
};
use std::future::Future;
use thiserror::Error;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedActor {
    pub actor_id: ActorId,
    pub workspace_id: WorkspaceId,
}
#[derive(Clone, Copy, Debug)]
pub struct DocumentAccess {
    pub head: DocumentVersion,
    pub document_access_epoch: u64,
    pub membership_access_epoch: u64,
    pub can_edit: bool,
}
#[derive(Clone, Debug)]
pub struct CommitReceipt {
    pub operation_id: OperationId,
    pub committed_version: DocumentVersion,
    pub canonical_event_hash: [u8; 32],
    pub current_head_version: DocumentVersion,
}
pub struct CommitRequest {
    /// Internal confirmed projection for structural/move history and lifecycle
    /// guards. The journal verifies its version/hash against the locked durable head.
    pub inverse_projection: Option<std::sync::Arc<Workbook>>,
    pub identity: AuthenticatedActor,
    pub expected_head: DocumentVersion,
    pub expected_access: DocumentAccess,
    pub command: Command,
    pub event: CommittedEvent,
}
/// Prepared, internal-only event. The caller must validate and apply it to a
/// private candidate before asking the journal to advance the durable head.
pub struct SystemCommitRequest {
    pub identity: AuthenticatedActor,
    pub expected_access: DocumentAccess,
    pub expected_head: DocumentVersion,
    pub event: CommittedEvent,
}
#[derive(Debug)]
pub enum CommitOutcome {
    Committed(CommitReceipt),
    AlreadyCommitted(CommitReceipt),
}
#[derive(Debug, Error)]
pub enum JournalError {
    #[error("document access denied")]
    Unauthorized,
    #[error("document not found")]
    NotFound,
    #[error("document changed; retry preparation")]
    Conflict,
    #[error("operation identifier was reused with different content")]
    IdempotencyConflict,
    #[error("journal unavailable")]
    Unavailable,
    #[error("journal data invalid")]
    Corrupt,
    #[error("journal schema unsupported")]
    UnsupportedSchema,
    #[error("durable event exceeds the size limit")]
    PayloadTooLarge,
    #[error("undo target changed or is unavailable")]
    UndoConflict,
    #[error("operation cannot be undone")]
    NotUndoable,
    #[error("undo target is outside the guarded history window")]
    UndoWindowExceeded,
}
pub trait OperationJournal: Send + Sync {
    /// Last calculation system event through a recovered projection's head.
    fn calculation_epoch_version(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        through: DocumentVersion,
    ) -> impl Future<Output = Result<Option<DocumentVersion>, JournalError>> + Send;
    fn quarantine(
        &self,
        document_id: DocumentId,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;
    fn access(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
    ) -> impl Future<Output = Result<DocumentAccess, JournalError>> + Send;
    fn create(
        &self,
        identity: AuthenticatedActor,
        event: CommittedEvent,
    ) -> impl Future<Output = Result<CommitReceipt, JournalError>> + Send;
    fn load_events(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        after: DocumentVersion,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<CommittedEvent>, JournalError>> + Send;
    fn load_undo_target(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        target_operation_id: OperationId,
    ) -> impl Future<Output = Result<CommittedEvent, JournalError>> + Send;
    fn commit(
        &self,
        request: &CommitRequest,
    ) -> impl Future<Output = Result<CommitOutcome, JournalError>> + Send;
    fn commit_system(
        &self,
        request: &SystemCommitRequest,
    ) -> impl Future<Output = Result<CommitOutcome, JournalError>> + Send;
    /// Resolve an uncertain internal trigger before admitting another epoch.
    fn system_receipt(
        &self,
        identity: AuthenticatedActor,
        document_id: DocumentId,
        operation_id: OperationId,
        sampled_context: &CalculationContext,
    ) -> impl Future<Output = Result<Option<CommitReceipt>, JournalError>> + Send;
    fn receipt(
        &self,
        identity: AuthenticatedActor,
        command: &Command,
    ) -> impl Future<Output = Result<Option<CommitReceipt>, JournalError>> + Send;
}
