use super::*;
use spreadsheet_domain::workbook::{
    ActorId, CalculationContextId, CalculationReason, ColumnId, RowId, SheetId, SheetSeed,
    WorkspaceId,
};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

// Controlled application-port double for actor publication tests. PostgreSQL
// durability and the real HTTP/WS path are covered by integration tests.
#[derive(Clone)]
struct ContextJournal {
    state: Arc<Mutex<(Workbook, Vec<CommittedEvent>)>>,
    fail_after_commit: bool,
    fail_epoch_once: Arc<AtomicBool>,
}
impl ContextJournal {
    async fn last_event(&self) -> TestResult<CommittedEvent> {
        self.state
            .lock()
            .await
            .1
            .last()
            .cloned()
            .ok_or_else(|| "missing event".into())
    }
    async fn append(
        &self,
        expected: DocumentVersion,
        event: &CommittedEvent,
    ) -> Result<CommitReceipt, JournalError> {
        let mut state = self.state.lock().await;
        if state.0.version() != expected {
            return Err(JournalError::Conflict);
        }
        state
            .0
            .apply_event(event)
            .map_err(|_| JournalError::Corrupt)?;
        state.1.push(event.clone());
        Ok(receipt(event, state.0.version()))
    }
}
fn receipt(event: &CommittedEvent, head: DocumentVersion) -> CommitReceipt {
    CommitReceipt {
        operation_id: event.operation_id,
        committed_version: event.version,
        canonical_event_hash: [0; 32],
        current_head_version: head,
    }
}
macro_rules! unavailable {
    ($name:ident ($($arg:ident: $ty:ty),*) -> $output:ty) => {
        async fn $name(&self, $($arg: $ty),*) -> Result<$output, JournalError> { Err(JournalError::Unavailable) }
    };
}
impl OperationJournal for ContextJournal {
    unavailable!(undo_history(_identity:AuthenticatedActor,_document:DocumentId) -> spreadsheet_application::history::UndoHistory);
    unavailable!(quarantine(_document:DocumentId) -> ());
    unavailable!(create(_identity:AuthenticatedActor,_event:CommittedEvent) -> CommitReceipt);
    unavailable!(load_undo_target(_identity:AuthenticatedActor,_document:DocumentId,_target:OperationId) -> CommittedEvent);
    async fn access(
        &self,
        _: AuthenticatedActor,
        _: DocumentId,
    ) -> Result<DocumentAccess, JournalError> {
        Ok(DocumentAccess {
            head: self.state.lock().await.0.version(),
            document_access_epoch: 1,
            membership_access_epoch: 1,
            can_edit: true,
        })
    }
    async fn load_events(
        &self,
        _: AuthenticatedActor,
        _: DocumentId,
        after: DocumentVersion,
        limit: u32,
    ) -> Result<Vec<CommittedEvent>, JournalError> {
        Ok(self
            .state
            .lock()
            .await
            .1
            .iter()
            .filter(|event| event.version > after)
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn calculation_epoch_version(
        &self,
        _: AuthenticatedActor,
        _: DocumentId,
        through: DocumentVersion,
    ) -> Result<Option<DocumentVersion>, JournalError> {
        if self.fail_epoch_once.swap(false, Ordering::SeqCst) {
            return Err(JournalError::Unavailable);
        }
        Ok(self
            .state
            .lock()
            .await
            .1
            .iter()
            .rev()
            .find(|event| event.version <= through && event.principal == EventPrincipal::System)
            .map(|event| event.version))
    }
    async fn receipt(
        &self,
        _: AuthenticatedActor,
        _: &Command,
    ) -> Result<Option<CommitReceipt>, JournalError> {
        Ok(None)
    }
    async fn system_receipt(
        &self,
        _: AuthenticatedActor,
        _: DocumentId,
        id: OperationId,
        _: &CalculationContext,
    ) -> Result<Option<CommitReceipt>, JournalError> {
        let state = self.state.lock().await;
        Ok(state
            .1
            .iter()
            .find(|event| event.operation_id == id)
            .map(|event| receipt(event, state.0.version())))
    }
    async fn commit(&self, request: &CommitRequest) -> Result<CommitOutcome, JournalError> {
        Ok(CommitOutcome::Committed(
            self.append(request.expected_head, &request.event).await?,
        ))
    }
    async fn commit_system(
        &self,
        request: &SystemCommitRequest,
    ) -> Result<CommitOutcome, JournalError> {
        let receipt = self.append(request.expected_head, &request.event).await?;
        if self.fail_after_commit {
            self.fail_epoch_once.store(true, Ordering::SeqCst);
        }
        Ok(CommitOutcome::Committed(receipt))
    }
}
fn fixture(volatile: bool) -> TestResult<(ContextJournal, AuthenticatedActor)> {
    let identity = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    let document_id = DocumentId::from_bytes([3; 16])?;
    let sheet = SheetSeed {
        id: SheetId::from_bytes([4; 16])?,
        name: "Sheet1".into(),
        rows: vec![RowId::from_bytes([5; 16])?],
        columns: vec![ColumnId::from_bytes([6; 16])?],
    };
    let operation = Operation::CreateWorkbook {
        sheet: sheet.clone(),
    };
    let mut book = Workbook::empty(document_id).candidate(&operation, DocumentVersion::new(1)?)?;
    let mut events = vec![CommittedEvent {
        document_id,
        operation_id: OperationId::from_bytes([7; 16])?,
        principal: EventPrincipal::User(identity.actor_id),
        version: book.version(),
        operation,
        state_hash: book.hash(),
    }];
    if volatile {
        let address = spreadsheet_domain::workbook::CellAddress {
            sheet: sheet.id,
            row: sheet.rows[0],
            column: sheet.columns[0],
        };
        let operation = Operation::SetCellFormula {
            address,
            formula: spreadsheet_formula::binder::bind("=NOW()", &book, address)?,
        };
        book = book.candidate(&operation, DocumentVersion::new(2)?)?;
        events.push(CommittedEvent {
            document_id,
            operation_id: OperationId::from_bytes([8; 16])?,
            principal: EventPrincipal::User(identity.actor_id),
            version: book.version(),
            operation,
            state_hash: book.hash(),
        });
    }
    Ok((
        ContextJournal {
            state: Arc::new(Mutex::new((book, events))),
            fail_after_commit: false,
            fail_epoch_once: Arc::new(AtomicBool::new(false)),
        },
        identity,
    ))
}
fn context(id: u8, reason: CalculationReason) -> Result<CalculationContext, DomainError> {
    CalculationContext::new(
        CalculationContextId::from_bytes([id; 16])?,
        1,
        if reason == CalculationReason::Timer {
            60_000
        } else {
            0
        },
        0,
        0,
        [id; 32],
        calculation::ENGINE_VERSION.into(),
        calculation::COMPATIBILITY_PROFILE.into(),
        reason,
    )
}
async fn assert_system_head(
    receiver: &mut watch::Receiver<HeadUpdate>,
    journal: &ContextJournal,
    version: u64,
    reason: CalculationReason,
) -> TestResult {
    tokio::time::timeout(Duration::from_secs(2), async {
        while receiver.borrow().version.get() < version {
            receiver.changed().await?;
        }
        Ok::<_, watch::error::RecvError>(())
    })
    .await??;
    let update = receiver.borrow().clone();
    assert_eq!(update.version.get(), version);
    let event = update.event.ok_or("missing exact system provenance")?;
    assert_eq!(event.as_ref(), &journal.last_event().await?);
    assert_eq!(event.principal, EventPrincipal::System);
    assert!(
        matches!(&event.operation, Operation::AdvanceCalculationEpoch { context } if context.reason()==reason)
    );
    Ok(())
}

#[tokio::test]
async fn explicit_context_head_retains_exact_event_and_unknown_receipt_refresh_stays_unknown()
-> TestResult {
    let (journal, identity) = fixture(false)?;
    let document = journal.state.lock().await.0.document_id();
    let directory = InProcessSessionDirectory::new(journal.clone());
    let session = directory.session(identity, document).await?;
    let mut heads = session.watch_head(identity).await?;
    let id = OperationId::from_bytes([20; 16])?;
    let sampled = context(21, CalculationReason::Explicit)?;
    session
        .advance_context(identity, id, sampled.clone())
        .await?;
    assert_system_head(&mut heads, &journal, 2, CalculationReason::Explicit).await?;
    let book = session.read(identity).await?;
    let operation = Operation::AdvanceCalculationEpoch {
        context: context(22, CalculationReason::Explicit)?.with_epoch(2)?,
    };
    let candidate = book.candidate(&operation, DocumentVersion::new(3)?)?;
    let external = SystemCommitRequest {
        identity,
        expected_access: journal.access(identity, document).await?,
        expected_head: book.version(),
        event: CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([23; 16])?,
            principal: EventPrincipal::System,
            version: candidate.version(),
            operation,
            state_hash: candidate.hash(),
        },
    };
    journal.commit_system(&external).await?;
    let retried = session.advance_context(identity, id, sampled).await?;
    assert_eq!(retried.committed_version.get(), 2);
    tokio::time::timeout(Duration::from_secs(2), async {
        while heads.borrow().version.get() < 3 {
            heads.changed().await?;
        }
        Ok::<_, watch::error::RecvError>(())
    })
    .await??;
    assert!(heads.borrow().event.is_none());
    Ok(())
}

#[tokio::test]
async fn bootstrap_open_timer_and_edit_epochs_publish_the_exact_latest_system_event() -> TestResult
{
    let (journal, identity) = fixture(true)?;
    let document = journal.state.lock().await.0.document_id();
    let samples = Arc::new(AtomicU8::new(30));
    let next = samples.clone();
    let directory =
        InProcessSessionDirectory::new(journal.clone()).with_context_sampler(move |reason| {
            let id = next.fetch_add(1, Ordering::SeqCst);
            Ok((OperationId::from_bytes([id; 16])?, context(id, reason)?))
        });
    let session = directory.session(identity, document).await?;
    let mut heads = session.watch_head(identity).await?;
    session.read(identity).await?;
    assert_system_head(&mut heads, &journal, 3, CalculationReason::Open).await?;
    session.opened(identity).await?;
    assert_system_head(&mut heads, &journal, 4, CalculationReason::Open).await?;
    session.request_timer(identity)?;
    assert_system_head(&mut heads, &journal, 5, CalculationReason::Timer).await?;
    let book = session.read(identity).await?;
    let sheet = &book.sheets()[0];
    let command = Command {
        document_id: document,
        operation_id: OperationId::from_bytes([40; 16])?,
        actor_id: identity.actor_id,
        base_version: book.version(),
        client_sequence: 1,
        operation: CommandPayload::SetCellFormula {
            address: spreadsheet_domain::workbook::CellAddress {
                sheet: sheet.id(),
                row: sheet.rows()[0],
                column: sheet.columns()[0],
            },
            source: spreadsheet_domain::formula::FormulaSource::new("=NOW()+1".into())?,
        },
    };
    assert!(matches!(
        session.submit(identity, command).await?,
        Submission::Committed { .. }
    ));
    assert_system_head(
        &mut heads,
        &journal,
        7,
        CalculationReason::VolatileDependency,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn bootstrap_post_commit_failure_still_publishes_exact_system_event() -> TestResult {
    let (mut journal, identity) = fixture(true)?;
    journal.fail_after_commit = true;
    let document = journal.state.lock().await.0.document_id();
    let directory =
        InProcessSessionDirectory::new(journal.clone()).with_context_sampler(|reason| {
            Ok((OperationId::from_bytes([50; 16])?, context(51, reason)?))
        });
    let session = directory.session(identity, document).await?;
    let mut heads = session.watch_head(identity).await?;
    assert!(matches!(
        session.read(identity).await,
        Err(SessionError::Journal(JournalError::Unavailable))
    ));
    assert_system_head(&mut heads, &journal, 3, CalculationReason::Open).await?;
    assert_eq!(session.read(identity).await?.version().get(), 3);
    assert_eq!(journal.state.lock().await.1.len(), 3);
    Ok(())
}
