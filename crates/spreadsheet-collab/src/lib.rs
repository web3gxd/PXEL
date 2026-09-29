//! Ordered document sessions; persistence remains behind application ports.
pub mod calculation;
#[cfg(test)]
mod watch_tests;
use calculation::{CalculatedState, CalculationHandle};
use spreadsheet_application::journal::{
    AuthenticatedActor, CommitOutcome, CommitReceipt, CommitRequest, DocumentAccess, JournalError,
    OperationJournal, SystemCommitRequest,
};
use spreadsheet_domain::workbook::{
    CalculationContext, Command, CommandPayload, CommittedEvent, DocumentId, DocumentVersion,
    DomainError, EventPrincipal, Operation, OperationId, Workbook,
};
use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};
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
    sessions: Arc<Mutex<HashMap<DocumentId, DocumentSession>>>,
    calculation_slots: Arc<Semaphore>,
}

impl<J: Clone> Clone for InProcessSessionDirectory<J> {
    fn clone(&self) -> Self {
        Self {
            journal: self.journal.clone(),
            sessions: self.sessions.clone(),
            calculation_slots: self.calculation_slots.clone(),
        }
    }
}

impl<J> InProcessSessionDirectory<J> {
    pub fn new(journal: J) -> Self {
        Self {
            journal,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            calculation_slots: Arc::new(Semaphore::new(2)),
        }
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
            document,
            receiver,
            calculation,
            head_sender,
        ));
        Ok(session)
    }
}

async fn run<J: OperationJournal>(
    journal: J,
    document: DocumentId,
    mut receiver: mpsc::Receiver<Request>,
    calculation: CalculationHandle,
    head_sender: watch::Sender<HeadUpdate>,
) {
    let mut workbook = Arc::new(Workbook::empty(document));
    while let Ok(Some(request)) =
        tokio::time::timeout(Duration::from_secs(60), receiver.recv()).await
    {
        match request {
            Request::ReadHead { identity, reply } => {
                let result = refresh(&journal, identity, &mut workbook, &calculation)
                    .await
                    .map(|_| workbook.version());
                let _ = reply.send(result);
            }
            Request::Read { identity, reply } => {
                let result = refresh(&journal, identity, &mut workbook, &calculation)
                    .await
                    .map(|_| workbook.as_ref().clone());
                let _ = reply.send(result);
            }
            Request::Submit {
                identity,
                command,
                reply,
            } => {
                let result = submit(&journal, identity, &mut workbook, command, &calculation).await;
                if let Ok(Submission::Committed { event, .. }) = &result {
                    head_sender.send_replace(HeadUpdate {
                        version: event.version,
                        event: Some(Arc::new((**event).clone())),
                    });
                }
                let _ = reply.send(result);
            }
            Request::AdvanceContext {
                identity,
                operation_id,
                sampled_context,
                reply,
            } => {
                let result = advance_context(
                    &journal,
                    identity,
                    &mut workbook,
                    operation_id,
                    &sampled_context,
                    &calculation,
                )
                .await;
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
) -> Result<DocumentAccess, SessionError> {
    let result = replay_to_head(journal, identity, workbook, calculation).await;
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
            calculation.publish(workbook.clone(), &event.operation);
        }
    }
    Ok(access)
}

async fn submit<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    workbook: &mut Arc<Workbook>,
    command: Command,
    calculation: &CalculationHandle,
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
        let access = refresh(journal, identity, workbook, calculation).await?;
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
        let operation = normalize(&command, workbook)?;
        let candidate = Arc::new(workbook.candidate(&operation, next)?);
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
    operation_id: OperationId,
    sampled_context: &CalculationContext,
    calculation: &CalculationHandle,
) -> Result<CommitReceipt, SessionError> {
    for attempt in 0..3 {
        let access = refresh(journal, identity, workbook, calculation).await?;
        if !access.can_edit {
            return Err(JournalError::Unauthorized.into());
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
            expected_head: workbook.version(),
            event: event.clone(),
        };
        match journal.commit_system(&request).await {
            Ok(CommitOutcome::Committed(receipt)) => {
                *workbook = candidate;
                calculation.publish(workbook.clone(), &event.operation);
                tracing::info!(event = "calculation_epoch_advanced", version = next.get());
                return Ok(receipt);
            }
            Ok(CommitOutcome::AlreadyCommitted(receipt)) => {
                refresh(journal, identity, workbook, calculation).await?;
                return Ok(receipt);
            }
            Err(JournalError::Conflict) if attempt < 2 => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(JournalError::Conflict.into())
}

/// Stable-ID value commands may cross an axis change. Formula text is bound only
/// when the client's base is at least the latest axis-change version; historical
/// axis projections are not yet available for an older formula command.
fn normalize(command: &Command, workbook: &Workbook) -> Result<Operation, SessionError> {
    match &command.operation {
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
        CommandPayload::SetCellValue { address, value } => Ok(Operation::SetCellValue {
            address: *address,
            value: value.normalized()?,
        }),
        CommandPayload::ClearCell { address } => Ok(Operation::ClearCell { address: *address }),
        CommandPayload::SetCellFormula { address, source } => {
            if command.base_version == DocumentVersion::ZERO
                || command.base_version < workbook.last_axis_change_version()
                || workbook
                    .version()
                    .get()
                    .saturating_sub(command.base_version.get())
                    > 10_000
            {
                return Err(SessionError::RebaseWindowExceeded);
            }
            let current = spreadsheet_formula::binder::bind(source.as_str(), workbook, *address)
                .map_err(|_| SessionError::InvalidFormula)?;
            let formula = spreadsheet_domain::formula::Formula::new(
                current.source().to_owned(),
                current.canonical().to_owned(),
                current.ast().clone(),
                *address,
                command.base_version,
            )
            .map_err(|_| SessionError::InvalidFormula)?;
            Ok(Operation::SetCellFormula {
                address: *address,
                formula,
            })
        }
        CommandPayload::CreateWorkbook { .. } => Err(SessionError::InvalidCommand),
    }
}
