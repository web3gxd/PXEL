//! Ordered document sessions; persistence remains behind application ports.
mod axis_history;
mod bootstrap;
pub mod calculation;
#[cfg(test)]
mod historical_recovery_tests;
#[cfg(test)]
mod watch_tests;
use calculation::{CalculatedState, CalculationHandle};
use spreadsheet_application::journal::{
    AuthenticatedActor, CommitOutcome, CommitReceipt, CommitRequest, DocumentAccess, JournalError,
    OperationJournal, SystemCommitRequest,
};
use spreadsheet_domain::workbook::{
    CalculationContext, CellContent, CellWriteKind, Command, CommandPayload, CommittedEvent,
    DocumentId, DocumentVersion, DomainError, EventPrincipal, Operation, OperationId, Workbook,
};
use std::{
    collections::HashMap, future::Future, num::NonZeroU64, pin::Pin, sync::Arc, time::Duration,
};
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot, watch};

const QUEUE_CAPACITY: usize = 64;
const MAX_SESSIONS: usize = 64;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("document queue is full")]
    QueueFull,
    #[error("session unavailable; retry with the same operation identity")]
    Unavailable,
    #[error("command actor does not match authenticated identity")]
    ActorMismatch,
    #[error("base version is ahead of server head")]
    BaseVersionAhead,
    #[error("invalid client operation")]
    InvalidCommand,
    #[error("formula base version is outside the available axis history")]
    RebaseWindowExceeded,
    #[error("range copy source version is no longer current")]
    SourceVersionChanged,
    #[error("formula could not be normalized")]
    InvalidFormula,
    #[error("document invariant failed")]
    Domain(#[from] DomainError),
    #[error("journal operation failed")]
    Journal(#[from] JournalError),
}

pub enum Submission {
    Committed {
        event: Box<CommittedEvent>,
        receipt: CommitReceipt,
        actor_id: spreadsheet_domain::workbook::ActorId,
    },
    AlreadyCommitted(CommitReceipt),
}

#[derive(Clone)]
pub struct HeadUpdate {
    pub version: DocumentVersion,
    pub event: Option<Arc<CommittedEvent>>,
}

enum Request {
    Open {
        identity: AuthenticatedActor,
        reply: oneshot::Sender<Result<DocumentVersion, SessionError>>,
    },
    Timer {
        identity: AuthenticatedActor,
    },
    ReadHead {
        identity: AuthenticatedActor,
        reply: oneshot::Sender<Result<DocumentVersion, SessionError>>,
    },
    Read {
        identity: AuthenticatedActor,
        reply: oneshot::Sender<Result<Workbook, SessionError>>,
    },
    Submit {
        identity: AuthenticatedActor,
        command: Command,
        reply: oneshot::Sender<Result<Submission, SessionError>>,
    },
    AdvanceContext {
        identity: AuthenticatedActor,
        operation_id: OperationId,
        sampled_context: CalculationContext,
        reply: oneshot::Sender<Result<CommitReceipt, SessionError>>,
    },
}

#[derive(Clone)]
pub struct DocumentSession {
    sender: mpsc::Sender<Request>,
    calculated: watch::Receiver<Option<Arc<CalculatedState>>>,
    head_updates: watch::Receiver<HeadUpdate>,
}

impl DocumentSession {
    /// Internal subscription transition. Ordinary reads/replay never renew context.
    pub async fn opened(
        &self,
        identity: AuthenticatedActor,
    ) -> Result<DocumentVersion, SessionError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request::Open { identity, reply })
            .map_err(queue_error)?;
        tokio::time::timeout(Duration::from_secs(10), receiver)
            .await
            .map_err(|_| SessionError::Unavailable)?
            .map_err(|_| SessionError::Unavailable)?
    }
    /// Internal heartbeat admission for an authenticated active subscription.
    /// Enqueueing is neither a commit nor a permission lease.
    pub fn request_timer(&self, identity: AuthenticatedActor) -> Result<(), SessionError> {
        self.sender
            .try_send(Request::Timer { identity })
            .map_err(queue_error)
    }
    /// Registers before the actor authorization barrier. Notifications are
    /// coalescing hints only; callers must recheck access before every send.
    pub async fn watch_head(
        &self,
        identity: AuthenticatedActor,
    ) -> Result<watch::Receiver<HeadUpdate>, SessionError> {
        let receiver = self.head_updates.clone();
        self.authorized_head(identity).await?;
        Ok(receiver)
    }
    /// Rechecks access through the document actor and returns its refreshed head,
    /// even while calculation is pending. This is a per-send check, not a lease.
    pub async fn head(
        &self,
        identity: AuthenticatedActor,
    ) -> Result<DocumentVersion, SessionError> {
        self.authorized_head(identity).await
    }
    /// Registers for calculation completion before the actor authorization barrier.
    /// This is a short-lived notification handle, not a durable/live subscription
    /// or permission lease. Call `calculated(identity)` again before every outbound
    /// payload to recheck current access and head; never send a borrowed watch value
    /// directly. A result already present must be checked before waiting for changes.
    pub async fn watch_calculation(
        &self,
        identity: AuthenticatedActor,
    ) -> Result<watch::Receiver<Option<Arc<CalculatedState>>>, SessionError> {
        let receiver = self.calculated.clone();
        self.authorized_head(identity).await?;
        Ok(receiver)
    }
    pub async fn calculated(
        &self,
        identity: AuthenticatedActor,
    ) -> Result<Option<Arc<CalculatedState>>, SessionError> {
        let head = self.authorized_head(identity).await?;
        Ok(self
            .calculated
            .borrow()
            .clone()
            .filter(|state| state.input_version == head))
    }
    async fn authorized_head(
        &self,
        identity: AuthenticatedActor,
    ) -> Result<DocumentVersion, SessionError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request::ReadHead { identity, reply })
            .map_err(queue_error)?;
        tokio::time::timeout(Duration::from_secs(10), receiver)
            .await
            .map_err(|_| SessionError::Unavailable)?
            .map_err(|_| SessionError::Unavailable)?
    }
    pub async fn read(&self, identity: AuthenticatedActor) -> Result<Workbook, SessionError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request::Read { identity, reply })
            .map_err(queue_error)?;
        tokio::time::timeout(Duration::from_secs(10), receiver)
            .await
            .map_err(|_| SessionError::Unavailable)?
            .map_err(|_| SessionError::Unavailable)?
    }

    pub async fn submit(
        &self,
        identity: AuthenticatedActor,
        command: Command,
    ) -> Result<Submission, SessionError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request::Submit {
                identity,
                command,
                reply,
            })
            .map_err(queue_error)?;
        // A timeout never claims rejection: COMMIT may have succeeded. Retry same ID.
        tokio::time::timeout(Duration::from_secs(10), receiver)
            .await
            .map_err(|_| SessionError::Unavailable)?
            .map_err(|_| SessionError::Unavailable)?
    }
    /// Internal trigger path. Callers sample time/seed/ID once and never accept
    /// these fields from an untrusted document command.
    pub async fn advance_context(
        &self,
        identity: AuthenticatedActor,
        operation_id: OperationId,
        sampled_context: CalculationContext,
    ) -> Result<CommitReceipt, SessionError> {
        let (reply, receiver) = oneshot::channel();
        self.sender
            .try_send(Request::AdvanceContext {
                identity,
                operation_id,
                sampled_context,
                reply,
            })
            .map_err(queue_error)?;
        tokio::time::timeout(Duration::from_secs(10), receiver)
            .await
            .map_err(|_| SessionError::Unavailable)?
            .map_err(|_| SessionError::Unavailable)?
    }
}

fn queue_error<T>(error: mpsc::error::TrySendError<T>) -> SessionError {
    match error {
        mpsc::error::TrySendError::Full(_) => SessionError::QueueFull,
        mpsc::error::TrySendError::Closed(_) => SessionError::Unavailable,
    }
}

pub trait SessionDirectory: Send + Sync {
    fn session(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
    ) -> impl Future<Output = Result<DocumentSession, SessionError>> + Send;
}

pub struct InProcessSessionDirectory<J> {
    journal: J,
    recovery: Option<RecoveryFn>,
    historical: Option<HistoricalRecovery>,
    snapshots: Option<SnapshotSchedule>,
    context_sampler: Option<bootstrap::Sampler>,
    sessions: Arc<Mutex<HashMap<DocumentId, DocumentSession>>>,
    calculation_slots: Arc<Semaphore>,
    recovery_slots: Arc<Semaphore>,
}

type RecoveryFuture = Pin<Box<dyn Future<Output = Result<Workbook, JournalError>> + Send>>;
type RecoveryFn = Arc<dyn Fn(AuthenticatedActor, DocumentId) -> RecoveryFuture + Send + Sync>;
type HistoricalFn =
    Arc<dyn Fn(AuthenticatedActor, DocumentId, DocumentVersion) -> RecoveryFuture + Send + Sync>;
#[derive(Clone)]
struct HistoricalRecovery {
    recover: HistoricalFn,
    slots: Arc<Semaphore>,
}
type SnapshotEnqueue = Arc<dyn Fn(Arc<Workbook>) + Send + Sync>;
#[derive(Clone)]
struct SnapshotSchedule {
    interval: NonZeroU64,
    enqueue: SnapshotEnqueue,
}
struct ActorOptions {
    snapshots: Option<SnapshotSchedule>,
    historical: Option<HistoricalRecovery>,
    context_sampler: Option<bootstrap::Sampler>,
}

fn maybe_snapshot(schedule: Option<&SnapshotSchedule>, workbook: &Arc<Workbook>) {
    if let Some(schedule) = schedule {
        let version = workbook.version().get();
        if version > 1 && (version - 1).is_multiple_of(schedule.interval.get()) {
            (schedule.enqueue)(workbook.clone());
        }
    }
}

impl<J: Clone> Clone for InProcessSessionDirectory<J> {
    fn clone(&self) -> Self {
        Self {
            journal: self.journal.clone(),
            recovery: self.recovery.clone(),
            historical: self.historical.clone(),
            snapshots: self.snapshots.clone(),
            context_sampler: self.context_sampler.clone(),
            sessions: self.sessions.clone(),
            calculation_slots: self.calculation_slots.clone(),
            recovery_slots: self.recovery_slots.clone(),
        }
    }
}

impl<J> InProcessSessionDirectory<J> {
    pub fn new(journal: J) -> Self {
        Self {
            journal,
            recovery: None,
            historical: None,
            snapshots: None,
            context_sampler: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            calculation_slots: Arc::new(Semaphore::new(2)),
            recovery_slots: Arc::new(Semaphore::new(4)),
        }
    }

    /// Bootstraps actors from a verified projection. The callback is invoked
    /// outside the directory lock; full journal replay remains the fallback
    /// when no callback is configured.
    pub fn with_recovery<F, Fut>(journal: J, recovery: F) -> Self
    where
        F: Fn(AuthenticatedActor, DocumentId) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Workbook, JournalError>> + Send + 'static,
    {
        let mut directory = Self::new(journal);
        directory.recovery = Some(Arc::new(move |identity, document| {
            Box::pin(recovery(identity, document))
        }));
        directory
    }

    /// Recover exact historical projections only when delayed binding misses
    /// the actor cache. Shares the bounded recovery admission with startup.
    pub fn with_historical_recovery<F, Fut>(mut self, recovery: F) -> Self
    where
        F: Fn(AuthenticatedActor, DocumentId, DocumentVersion) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Workbook, JournalError>> + Send + 'static,
    {
        self.historical = Some(HistoricalRecovery {
            recover: Arc::new(move |identity, document, version| {
                Box::pin(recovery(identity, document, version))
            }),
            slots: self.recovery_slots.clone(),
        });
        self
    }

    /// Snapshot scheduling observes only newly committed actor states. The
    /// callback must enqueue without blocking or claiming durable publication.
    pub fn with_snapshot_enqueue<F>(mut self, interval: NonZeroU64, enqueue: F) -> Self
    where
        F: Fn(Arc<Workbook>) + Send + Sync + 'static,
    {
        self.snapshots = Some(SnapshotSchedule {
            interval,
            enqueue: Arc::new(enqueue),
        });
        self
    }
    /// Trusted server clock/seed boundary; never a wire or persisted DTO.
    pub fn with_context_sampler<F>(mut self, sampler: F) -> Self
    where
        F: Fn(
                spreadsheet_domain::workbook::CalculationReason,
            ) -> Result<(OperationId, CalculationContext), SessionError>
            + Send
            + Sync
            + 'static,
    {
        self.context_sampler = Some(Arc::new(sampler));
        self
    }
}

impl<J: OperationJournal + Clone + Send + Sync + 'static> SessionDirectory
    for InProcessSessionDirectory<J>
{
    async fn session(
        &self,
        identity: AuthenticatedActor,
        document: DocumentId,
    ) -> Result<DocumentSession, SessionError> {
        self.journal.access(identity, document).await?;
        {
            let mut sessions = self.sessions.lock().await;
            sessions.retain(|_, session| !session.sender.is_closed());
            if let Some(session) = sessions.get(&document) {
                return Ok(session.clone());
            }
        }
        let _recovery_permit = self
            .recovery_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| SessionError::QueueFull)?;
        let initial = match &self.recovery {
            Some(recovery) => {
                tokio::time::timeout(Duration::from_secs(10), recovery(identity, document))
                    .await
                    .map_err(|_| SessionError::Unavailable)??
            }
            None => Workbook::empty(document),
        };
        if initial.document_id() != document {
            return Err(JournalError::Corrupt.into());
        }
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, session| !session.sender.is_closed());
        if let Some(session) = sessions.get(&document) {
            return Ok(session.clone());
        }
        if sessions.len() >= MAX_SESSIONS {
            return Err(SessionError::QueueFull);
        }
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        let (calculation, calculated) = CalculationHandle::start(self.calculation_slots.clone());
        let (head_sender, head_updates) = watch::channel(HeadUpdate {
            version: DocumentVersion::ZERO,
            event: None,
        });
        let session = DocumentSession {
            sender,
            calculated,
            head_updates,
        };
        sessions.insert(document, session.clone());
        tokio::spawn(run(
            self.journal.clone(),
            initial,
            receiver,
            calculation,
            head_sender,
            ActorOptions {
                snapshots: self.snapshots.clone(),
                historical: self.historical.clone(),
                context_sampler: self.context_sampler.clone(),
            },
        ));
        Ok(session)
    }
}

async fn run<J: OperationJournal>(
    journal: J,
    initial: Workbook,
    mut receiver: mpsc::Receiver<Request>,
    calculation: CalculationHandle,
    head_sender: watch::Sender<HeadUpdate>,
    options: ActorOptions,
) {
    let ActorOptions {
        snapshots,
        historical,
        context_sampler,
    } = options;
    let mut bootstrap = bootstrap::Bootstrap::new(context_sampler);
    let mut axes = axis_history::AxisHistory::new(&initial);
    let mut workbook = Arc::new(initial);
    if workbook.version() != DocumentVersion::ZERO {
        calculation.publish_full(workbook.clone());
    }
    while let Ok(Some(request)) =
        tokio::time::timeout(Duration::from_secs(60), receiver.recv()).await
    {
        match request {
            Request::Open { identity, reply } => {
                let before = workbook.version();
                let result = async {
                    refresh(&journal, identity, &mut workbook, &calculation, &mut axes).await?;
                    let previous = workbook
                        .calculation_context()
                        .map(|context| context.context_id());
                    bootstrap
                        .ensure(&journal, identity, &mut workbook, &calculation, &mut axes)
                        .await?;
                    let renew = previous
                        == workbook
                            .calculation_context()
                            .map(|context| context.context_id());
                    bootstrap
                        .opened(
                            &journal,
                            identity,
                            &mut workbook,
                            &calculation,
                            &mut axes,
                            renew,
                        )
                        .await?;
                    Ok(workbook.version())
                }
                .await;
                if workbook.version() > before {
                    maybe_snapshot(snapshots.as_ref(), &workbook);
                }
                let _ = reply.send(result);
            }
            Request::Timer { identity } => {
                let before = workbook.version();
                let result = async {
                    refresh(&journal, identity, &mut workbook, &calculation, &mut axes).await?;
                    bootstrap
                        .ensure(&journal, identity, &mut workbook, &calculation, &mut axes)
                        .await?;
                    bootstrap
                        .timer(&journal, identity, &mut workbook, &calculation, &mut axes)
                        .await
                }
                .await;
                if result.is_err() {
                    tracing::warn!(event = "calculation_timer_pending");
                }
                if workbook.version() > before {
                    maybe_snapshot(snapshots.as_ref(), &workbook);
                }
            }
            Request::ReadHead { identity, reply } => {
                let result = refresh(&journal, identity, &mut workbook, &calculation, &mut axes)
                    .await
                    .map(|_| workbook.version());
                let _ = reply.send(result);
            }
            Request::Read { identity, reply } => {
                let result = async {
                    refresh(&journal, identity, &mut workbook, &calculation, &mut axes).await?;
                    let before = workbook.version();
                    bootstrap
                        .ensure(&journal, identity, &mut workbook, &calculation, &mut axes)
                        .await?;
                    if workbook.version() > before {
                        maybe_snapshot(snapshots.as_ref(), &workbook);
                    }
                    Ok(workbook.as_ref().clone())
                }
                .await;
                let _ = reply.send(result);
            }
            Request::Submit {
                identity,
                command,
                reply,
            } => {
                let result = submit(
                    &journal,
                    identity,
                    &mut workbook,
                    command,
                    &calculation,
                    &mut axes,
                    historical.as_ref(),
                )
                .await;
                if let Ok(Submission::Committed { event, .. }) = &result {
                    head_sender.send_replace(HeadUpdate {
                        version: event.version,
                        event: Some(Arc::new((**event).clone())),
                    });
                    maybe_snapshot(snapshots.as_ref(), &workbook);
                    bootstrap.edited(&workbook, &event.operation);
                }
                let committed = matches!(&result, Ok(Submission::Committed { .. }));
                let _ = reply.send(result);
                if committed {
                    let before = workbook.version();
                    if bootstrap
                        .ensure(&journal, identity, &mut workbook, &calculation, &mut axes)
                        .await
                        .is_err()
                    {
                        tracing::warn!(event = "calculation_bootstrap_pending");
                    } else if workbook.version() > before {
                        maybe_snapshot(snapshots.as_ref(), &workbook);
                    }
                }
            }
            Request::AdvanceContext {
                identity,
                operation_id,
                sampled_context,
                reply,
            } => {
                let before = workbook.version();
                let result = advance_context(
                    &journal,
                    identity,
                    &mut workbook,
                    bootstrap::ContextAdvance {
                        operation_id,
                        context: &sampled_context,
                        admission: bootstrap::Admission::Explicit,
                    },
                    &calculation,
                    &mut axes,
                )
                .await
                .and_then(|receipt| receipt.ok_or(SessionError::InvalidCommand));
                if result.is_ok() && workbook.version() > before {
                    maybe_snapshot(snapshots.as_ref(), &workbook);
                }
                let _ = reply.send(result);
            }
        }
        if head_sender.borrow().version != workbook.version() {
            head_sender.send_replace(HeadUpdate {
                version: workbook.version(),
                event: None,
            });
        }
    }
    calculation.stop();
}

async fn refresh<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    workbook: &mut Arc<Workbook>,
    calculation: &CalculationHandle,
    axes: &mut axis_history::AxisHistory,
) -> Result<DocumentAccess, SessionError> {
    let result = replay_to_head(journal, identity, workbook, calculation, axes).await;
    if matches!(
        &result,
        Err(SessionError::Domain(_))
            | Err(SessionError::Journal(
                JournalError::Corrupt | JournalError::UnsupportedSchema
            ))
    ) {
        journal.quarantine(workbook.document_id()).await?;
        return Err(JournalError::Corrupt.into());
    }
    result
}

async fn replay_to_head<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    workbook: &mut Arc<Workbook>,
    calculation: &CalculationHandle,
    axes: &mut axis_history::AxisHistory,
) -> Result<DocumentAccess, SessionError> {
    let access = journal.access(identity, workbook.document_id()).await?;
    if access.head < workbook.version() {
        return Err(JournalError::Corrupt.into());
    }
    while workbook.version() < access.head {
        let events = journal
            .load_events(identity, workbook.document_id(), workbook.version(), 1000)
            .await?;
        if events.is_empty() {
            return Err(JournalError::Corrupt.into());
        }
        for event in events {
            if event.version > access.head {
                break;
            }
            Arc::make_mut(workbook).apply_event(&event)?;
            let checkpoint = if axis_history::AxisHistory::structural(&event.operation) {
                axes.prepare(workbook, &event.operation)
            } else {
                None
            };
            axes.publish(workbook, &event.operation, checkpoint);
            calculation.publish(workbook.clone(), &event.operation);
        }
    }
    Ok(access)
}

async fn restore_axis_history<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    head: &Workbook,
    base: DocumentVersion,
    recovery: &HistoricalRecovery,
) -> Result<axis_history::AxisHistory, SessionError> {
    if base == DocumentVersion::ZERO
        || base > head.version()
        || head.version().get().saturating_sub(base.get()) > axis_history::MAX_VERSIONS
    {
        return Err(SessionError::RebaseWindowExceeded);
    }
    let _permit = recovery
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| SessionError::QueueFull)?;
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut book = (recovery.recover)(identity, head.document_id(), base).await?;
        if book.document_id() != head.document_id() || book.version() != base {
            return Err(JournalError::Corrupt.into());
        }
        let mut axes = axis_history::AxisHistory::new(&book);
        while book.version() < head.version() {
            let count = (head.version().get() - book.version().get()).min(128) as u32;
            let events = journal
                .load_events(identity, head.document_id(), book.version(), count)
                .await?;
            if events.is_empty() {
                return Err(JournalError::Corrupt.into());
            }
            for event in events {
                if event.version > head.version() {
                    return Err(JournalError::Corrupt.into());
                }
                book.apply_event(&event)?;
                let checkpoint = if axis_history::AxisHistory::structural(&event.operation) {
                    axes.prepare(&book, &event.operation)
                } else {
                    None
                };
                axes.publish(&book, &event.operation, checkpoint);
                tokio::task::yield_now().await;
            }
        }
        if book.hash() != head.hash() {
            return Err(JournalError::Corrupt.into());
        }
        Ok(axes)
    })
    .await
    .map_err(|_| SessionError::Unavailable)?
}

async fn submit<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    workbook: &mut Arc<Workbook>,
    command: Command,
    calculation: &CalculationHandle,
    axes: &mut axis_history::AxisHistory,
    historical: Option<&HistoricalRecovery>,
) -> Result<Submission, SessionError> {
    if command.actor_id != identity.actor_id {
        return Err(SessionError::ActorMismatch);
    }
    if command.document_id != workbook.document_id()
        || command.client_sequence == 0
        || matches!(command.operation, CommandPayload::CreateWorkbook { .. })
    {
        return Err(SessionError::InvalidCommand);
    }
    for attempt in 0..3 {
        let access = refresh(journal, identity, workbook, calculation, axes).await?;
        if !access.can_edit {
            return Err(JournalError::Unauthorized.into());
        }
        if let Some(receipt) = journal.receipt(identity, &command).await? {
            return Ok(Submission::AlreadyCommitted(receipt));
        }
        if command.base_version > workbook.version() {
            return Err(SessionError::BaseVersionAhead);
        }
        let next = DocumentVersion::new(
            workbook
                .version()
                .get()
                .checked_add(1)
                .ok_or(SessionError::InvalidCommand)?,
        )?;
        let operation = match &command.operation {
            CommandPayload::UndoCell {
                target_operation_id,
            } => {
                let target = journal
                    .load_undo_target(identity, command.document_id, *target_operation_id)
                    .await?;
                let Operation::CellWrite {
                    address,
                    content,
                    previous,
                    undoes: None,
                    undoes_version: None,
                    kind: Some(_),
                } = target.operation
                else {
                    return Err(JournalError::NotUndoable.into());
                };
                // A historical bound formula was not present in the workbook
                // while this value occupied its cell, so axis edits could not
                // rebase it. Restore it only when the axes have not changed.
                if matches!(previous, Some(CellContent::Formula(_)))
                    && workbook.last_axis_change_version() > target.version
                {
                    return Err(JournalError::NotUndoable.into());
                }
                Operation::CellWrite {
                    address,
                    content: previous,
                    previous: content,
                    undoes: Some(*target_operation_id),
                    undoes_version: Some(target.version),
                    kind: None,
                }
            }
            _ => match normalize(&command, workbook, axes) {
                Err(SessionError::RebaseWindowExceeded)
                    if matches!(command.operation, CommandPayload::SetCellFormula { .. })
                        && !axes.covers(command.base_version) =>
                {
                    let recovery = historical.ok_or(SessionError::RebaseWindowExceeded)?;
                    *axes = restore_axis_history(
                        journal,
                        identity,
                        workbook,
                        command.base_version,
                        recovery,
                    )
                    .await?;
                    normalize(&command, workbook, axes)?
                }
                result => result?,
            },
        };
        let candidate = match workbook.candidate(&operation, next) {
            Ok(candidate) => Arc::new(candidate),
            Err(
                DomainError::PreimageConflict
                | DomainError::MissingSheet
                | DomainError::MissingRow
                | DomainError::MissingColumn,
            ) if matches!(command.operation, CommandPayload::UndoCell { .. }) => {
                return Err(JournalError::UndoConflict.into());
            }
            Err(error) => return Err(error.into()),
        };
        if matches!(
            operation,
            Operation::InsertRowsRebased { .. }
                | Operation::DeleteRows { .. }
                | Operation::InsertColumnsRebased { .. }
                | Operation::DeleteColumns { .. }
        ) {
            for sheet in candidate.sheets() {
                for content in sheet.cells().values() {
                    if let spreadsheet_domain::workbook::CellContent::Formula(formula) = content {
                        spreadsheet_formula::bound::render_current(formula, &candidate)
                            .map_err(|_| SessionError::InvalidFormula)?;
                    }
                }
            }
        }
        let axis_checkpoint = if axis_history::AxisHistory::structural(&operation) {
            axes.prepare(&candidate, &operation)
        } else {
            None
        };
        let event = CommittedEvent {
            document_id: command.document_id,
            operation_id: command.operation_id,
            principal: EventPrincipal::User(command.actor_id),
            version: next,
            operation,
            state_hash: candidate.hash(),
        };
        let request = CommitRequest {
            identity,
            expected_head: workbook.version(),
            expected_access: access,
            command: command.clone(),
            event: event.clone(),
        };
        let event = Box::new(event);
        match journal.commit(&request).await {
            Ok(CommitOutcome::Committed(receipt)) => {
                // The only post-COMMIT state transition is this infallible prepared swap.
                *workbook = candidate;
                axes.publish(workbook, &event.operation, axis_checkpoint);
                calculation.publish(workbook.clone(), &event.operation);
                tracing::info!(event = "operation_committed", version = next.get());
                return Ok(Submission::Committed {
                    event,
                    receipt,
                    actor_id: command.actor_id,
                });
            }
            Ok(CommitOutcome::AlreadyCommitted(receipt)) => {
                return Ok(Submission::AlreadyCommitted(receipt));
            }
            Err(JournalError::Conflict) if attempt < 2 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(JournalError::Conflict.into())
}

async fn advance_context<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    workbook: &mut Arc<Workbook>,
    trigger: bootstrap::ContextAdvance<'_>,
    calculation: &CalculationHandle,
    axes: &mut axis_history::AxisHistory,
) -> Result<Option<CommitReceipt>, SessionError> {
    let bootstrap::ContextAdvance {
        operation_id,
        context: sampled_context,
        admission,
    } = trigger;
    for attempt in 0..3 {
        let access = refresh(journal, identity, workbook, calculation, axes).await?;
        if sampled_context.reason() == spreadsheet_domain::workbook::CalculationReason::Explicit
            && !access.can_edit
        {
            return Err(JournalError::Unauthorized.into());
        }
        if let Some(receipt) = journal
            .system_receipt(
                identity,
                workbook.document_id(),
                operation_id,
                sampled_context,
            )
            .await?
        {
            if receipt.current_head_version > workbook.version() {
                refresh(journal, identity, workbook, calculation, axes).await?;
            }
            return Ok(Some(receipt));
        }
        if !admission.permits(workbook) {
            return Ok(None);
        }
        let next = DocumentVersion::new(
            workbook
                .version()
                .get()
                .checked_add(1)
                .ok_or(SessionError::InvalidCommand)?,
        )?;
        let next_epoch = workbook
            .calculation_context()
            .map_or(Some(1), |context| context.epoch().checked_add(1))
            .ok_or(SessionError::InvalidCommand)?;
        let operation = Operation::AdvanceCalculationEpoch {
            context: sampled_context.with_epoch(next_epoch)?,
        };
        let candidate = Arc::new(workbook.candidate(&operation, next)?);
        let event = CommittedEvent {
            document_id: workbook.document_id(),
            operation_id,
            principal: EventPrincipal::System,
            version: next,
            operation,
            state_hash: candidate.hash(),
        };
        let request = SystemCommitRequest {
            identity,
            expected_access: access,
            expected_head: workbook.version(),
            event: event.clone(),
        };
        match journal.commit_system(&request).await {
            Ok(CommitOutcome::Committed(receipt)) => {
                *workbook = candidate;
                axes.publish(workbook, &event.operation, None);
                calculation.publish(workbook.clone(), &event.operation);
                tracing::info!(event = "calculation_epoch_advanced", version = next.get());
                return Ok(Some(receipt));
            }
            Ok(CommitOutcome::AlreadyCommitted(receipt)) => {
                refresh(journal, identity, workbook, calculation, axes).await?;
                return Ok(Some(receipt));
            }
            Err(JournalError::Conflict) if attempt < 2 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(JournalError::Conflict.into())
}

/// Formula text is bound against the bounded historical axes the client saw.
fn normalize(
    command: &Command,
    workbook: &Workbook,
    axes: &axis_history::AxisHistory,
) -> Result<Operation, SessionError> {
    match &command.operation {
        CommandPayload::AddSheet { sheet } => Ok(Operation::AddSheet {
            sheet: sheet.clone(),
        }),
        CommandPayload::ExtendRows { sheet, rows } => Ok(Operation::ExtendRows {
            sheet: *sheet,
            rows: rows.clone(),
        }),
        CommandPayload::InsertRows {
            sheet,
            before,
            rows,
        } => Ok(Operation::InsertRowsRebased {
            sheet: *sheet,
            before: *before,
            rows: rows.clone(),
        }),
        CommandPayload::DeleteRows { sheet, rows } => Ok(Operation::DeleteRows {
            sheet: *sheet,
            rows: rows.clone(),
        }),
        CommandPayload::InsertColumns {
            sheet,
            before,
            columns,
        } => Ok(Operation::InsertColumnsRebased {
            sheet: *sheet,
            before: *before,
            columns: columns.clone(),
        }),
        CommandPayload::DeleteColumns { sheet, columns } => Ok(Operation::DeleteColumns {
            sheet: *sheet,
            columns: columns.clone(),
        }),
        CommandPayload::SetCellValue { address, value } => {
            let value = value.normalized()?;
            Ok(Operation::CellWrite {
                address: *address,
                content: (value != spreadsheet_domain::workbook::CellValue::Blank)
                    .then_some(CellContent::Value(value)),
                previous: workbook.cell_content(*address)?.cloned(),
                undoes: None,
                undoes_version: None,
                kind: Some(CellWriteKind::SetValue),
            })
        }
        CommandPayload::SetCellValues { writes } => Ok(Operation::SetCellValues {
            writes: writes
                .iter()
                .map(|write| {
                    Ok(spreadsheet_domain::workbook::CellWrite {
                        address: write.address,
                        value: write.value.clone().normalized()?,
                    })
                })
                .collect::<Result<_, SessionError>>()?,
        }),
        CommandPayload::CopyRange {
            source,
            destination,
            rows,
            columns,
        } => {
            if command.base_version != workbook.version() {
                return Err(SessionError::SourceVersionChanged);
            }
            let count = rows
                .checked_mul(*columns)
                .ok_or(SessionError::InvalidCommand)?;
            if count == 0 || count > 1_000 || source.sheet != destination.sheet {
                return Err(SessionError::InvalidCommand);
            }
            let sheet = workbook
                .sheets()
                .iter()
                .find(|sheet| sheet.id() == source.sheet)
                .ok_or(DomainError::MissingSheet)?;
            let source_row = sheet
                .row_position(source.row)
                .ok_or(DomainError::MissingRow)?;
            let source_column = sheet
                .column_position(source.column)
                .ok_or(DomainError::MissingColumn)?;
            let destination_row = sheet
                .row_position(destination.row)
                .ok_or(DomainError::MissingRow)?;
            let destination_column = sheet
                .column_position(destination.column)
                .ok_or(DomainError::MissingColumn)?;
            let mut writes = Vec::with_capacity(count as usize);
            for row_offset in 0..*rows {
                for column_offset in 0..*columns {
                    let sr = source_row
                        .checked_add(row_offset)
                        .ok_or(SessionError::InvalidCommand)?;
                    let sc = source_column
                        .checked_add(column_offset)
                        .ok_or(SessionError::InvalidCommand)?;
                    let dr = destination_row
                        .checked_add(row_offset)
                        .ok_or(SessionError::InvalidCommand)?;
                    let dc = destination_column
                        .checked_add(column_offset)
                        .ok_or(SessionError::InvalidCommand)?;
                    let source_address = spreadsheet_domain::workbook::CellAddress {
                        sheet: source.sheet,
                        row: *sheet
                            .rows()
                            .get(sr as usize)
                            .ok_or(DomainError::MissingRow)?,
                        column: *sheet
                            .columns()
                            .get(sc as usize)
                            .ok_or(DomainError::MissingColumn)?,
                    };
                    let destination_address = spreadsheet_domain::workbook::CellAddress {
                        sheet: destination.sheet,
                        row: *sheet
                            .rows()
                            .get(dr as usize)
                            .ok_or(DomainError::MissingRow)?,
                        column: *sheet
                            .columns()
                            .get(dc as usize)
                            .ok_or(DomainError::MissingColumn)?,
                    };
                    let content = match workbook.cell_content(source_address)? {
                        Some(CellContent::Value(value)) => Some(CellContent::Value(value.clone())),
                        Some(CellContent::Formula(formula)) => {
                            let current =
                                spreadsheet_formula::bound::render_current(formula, workbook)
                                    .map_err(|_| SessionError::InvalidFormula)?;
                            let copied = spreadsheet_formula::copy_formula_text(
                                &current,
                                (sr, sc),
                                (dr, dc),
                            )
                            .map_err(|_| SessionError::InvalidFormula)?;
                            let bound = spreadsheet_formula::binder::bind(
                                &copied,
                                workbook,
                                destination_address,
                            )
                            .map_err(|_| SessionError::InvalidFormula)?;
                            Some(CellContent::Formula(bound))
                        }
                        None => None,
                    };
                    writes.push(spreadsheet_domain::workbook::ContentWrite {
                        address: destination_address,
                        content,
                    });
                }
            }
            Ok(Operation::CopyRange { writes })
        }
        CommandPayload::ClearCell { address } => Ok(Operation::CellWrite {
            address: *address,
            content: None,
            previous: workbook.cell_content(*address)?.cloned(),
            undoes: None,
            undoes_version: None,
            kind: Some(CellWriteKind::Clear),
        }),
        CommandPayload::SetCellFormula { address, source } => {
            let formula = axes.bind(source.as_str(), workbook, *address, command.base_version)?;
            Ok(Operation::SetCellFormula {
                address: *address,
                formula,
            })
        }
        CommandPayload::CreateWorkbook { .. } | CommandPayload::UndoCell { .. } => {
            Err(SessionError::InvalidCommand)
        }
    }
}
