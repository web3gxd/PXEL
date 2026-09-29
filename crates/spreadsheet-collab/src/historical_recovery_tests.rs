use super::*;
use spreadsheet_domain::workbook::{ActorId, ColumnId, RowId, SheetId, SheetSeed, WorkspaceId};
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// A rejecting boundary double, never a production persistence implementation.
#[derive(Default)]
pub(crate) struct RejectJournal(AtomicUsize);
macro_rules! reject {
    ($name:ident ($($arg:ident: $ty:ty),*) -> $output:ty) => {
        async fn $name(&self, $($arg: $ty),*) -> Result<$output, JournalError> {
            Err(JournalError::Unavailable)
        }
    };
}
impl OperationJournal for RejectJournal {
    reject!(quarantine(_document: DocumentId) -> ());
    reject!(access(_identity: AuthenticatedActor, _document: DocumentId) -> DocumentAccess);
    reject!(create(_identity: AuthenticatedActor, _event: CommittedEvent) -> CommitReceipt);
    reject!(load_undo_target(_identity: AuthenticatedActor, _document: DocumentId, _target: OperationId) -> CommittedEvent);
    reject!(commit(_request: &CommitRequest) -> CommitOutcome);
    reject!(commit_system(_request: &SystemCommitRequest) -> CommitOutcome);
    reject!(system_receipt(_identity: AuthenticatedActor, _document: DocumentId, _operation: OperationId, _context: &CalculationContext) -> Option<CommitReceipt>);
    reject!(receipt(_identity: AuthenticatedActor, _command: &Command) -> Option<CommitReceipt>);
    async fn load_events(
        &self,
        _identity: AuthenticatedActor,
        _document: DocumentId,
        _after: DocumentVersion,
        _limit: u32,
    ) -> Result<Vec<CommittedEvent>, JournalError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(JournalError::Unauthorized)
    }
}
fn fixture() -> Result<(AuthenticatedActor, Workbook, Workbook), DomainError> {
    let identity = AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    };
    let sheet = SheetSeed {
        id: SheetId::from_bytes([3; 16])?,
        name: "Sheet1".into(),
        rows: vec![RowId::from_bytes([4; 16])?],
        columns: vec![ColumnId::from_bytes([5; 16])?],
    };
    let base = Workbook::empty(DocumentId::from_bytes([6; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: sheet.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let head = base.candidate(
        &Operation::ExtendRows {
            sheet: sheet.id,
            rows: vec![RowId::from_bytes([7; 16])?],
        },
        DocumentVersion::new(2)?,
    )?;
    Ok((identity, base, head))
}
fn recovery(book: Workbook, slots: Arc<Semaphore>, calls: Arc<AtomicUsize>) -> HistoricalRecovery {
    HistoricalRecovery {
        slots,
        recover: Arc::new(move |_, _, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            let book = book.clone();
            Box::pin(async move { Ok(book) })
        }),
    }
}

#[tokio::test]
async fn exhausted_admission_never_loads_history_and_retains_head() -> TestResult {
    let (identity, base, head) = fixture()?;
    let hash = head.hash();
    let calls = Arc::new(AtomicUsize::new(0));
    let slots = Arc::new(Semaphore::new(1));
    let permit = slots.clone().acquire_owned().await?;
    let recovery = recovery(base.clone(), slots.clone(), calls.clone());
    let journal = RejectJournal::default();
    assert!(matches!(
        restore_axis_history(&journal, identity, &head, base.version(), &recovery).await,
        Err(SessionError::QueueFull)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(journal.0.load(Ordering::SeqCst), 0);
    assert_eq!(head.hash(), hash);
    drop(permit);
    assert_eq!(slots.available_permits(), 1);
    Ok(())
}

#[tokio::test]
async fn invalid_base_rejects_before_callback_or_journal() -> TestResult {
    let (identity, base, head) = fixture()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let recovery = recovery(base, Arc::new(Semaphore::new(1)), calls.clone());
    let journal = RejectJournal::default();
    for version in [DocumentVersion::ZERO, DocumentVersion::new(3)?] {
        assert!(matches!(
            restore_axis_history(&journal, identity, &head, version, &recovery).await,
            Err(SessionError::RebaseWindowExceeded)
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(journal.0.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn wrong_recovered_version_is_corrupt_before_tail_read() -> TestResult {
    let (identity, base, head) = fixture()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let slots = Arc::new(Semaphore::new(1));
    let recovery = recovery(head.clone(), slots.clone(), calls.clone());
    let journal = RejectJournal::default();
    assert!(matches!(
        restore_axis_history(&journal, identity, &head, base.version(), &recovery).await,
        Err(SessionError::Journal(JournalError::Corrupt))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(journal.0.load(Ordering::SeqCst), 0);
    assert_eq!(slots.available_permits(), 1);
    Ok(())
}

#[tokio::test]
async fn revoked_tail_read_rejects_without_mutating_head_or_leaking_permit() -> TestResult {
    let (identity, base, head) = fixture()?;
    let hash = head.hash();
    let slots = Arc::new(Semaphore::new(1));
    let recovery = recovery(base.clone(), slots.clone(), Arc::new(AtomicUsize::new(0)));
    let journal = RejectJournal::default();
    assert!(matches!(
        restore_axis_history(&journal, identity, &head, base.version(), &recovery).await,
        Err(SessionError::Journal(JournalError::Unauthorized))
    ));
    assert_eq!(journal.0.load(Ordering::SeqCst), 1);
    assert_eq!(head.hash(), hash);
    assert_eq!(slots.available_permits(), 1);
    Ok(())
}

#[tokio::test]
async fn cancelling_recovery_releases_admission_without_reading_tail() -> TestResult {
    let (identity, base, head) = fixture()?;
    let slots = Arc::new(Semaphore::new(1));
    let started = Arc::new(tokio::sync::Notify::new());
    let notification = started.clone();
    let recovery = HistoricalRecovery {
        slots: slots.clone(),
        recover: Arc::new(move |_, _, _| {
            notification.notify_one();
            Box::pin(std::future::pending())
        }),
    };
    let journal = Arc::new(RejectJournal::default());
    let observed = journal.clone();
    let task = tokio::spawn(async move {
        restore_axis_history(journal.as_ref(), identity, &head, base.version(), &recovery).await
    });
    tokio::time::timeout(Duration::from_secs(1), started.notified()).await?;
    assert_eq!(slots.available_permits(), 0);
    task.abort();
    assert!(task.await.is_err_and(|error| error.is_cancelled()));
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(observed.0.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn same_version_with_wrong_domain_hash_is_rejected() -> TestResult {
    let (identity, base, _) = fixture()?;
    let sheet = &base.sheets()[0];
    let wrong = Workbook::hydrate_sheets(
        base.document_id(),
        base.version(),
        vec![SheetSeed {
            id: sheet.id(),
            name: "Changed".into(),
            rows: sheet.rows().to_vec(),
            columns: sheet.columns().to_vec(),
        }],
        [],
    )?;
    let journal = RejectJournal::default();
    let slots = Arc::new(Semaphore::new(1));
    let recovery = recovery(wrong, slots.clone(), Arc::new(AtomicUsize::new(0)));
    assert!(matches!(
        restore_axis_history(&journal, identity, &base, base.version(), &recovery).await,
        Err(SessionError::Journal(JournalError::Corrupt))
    ));
    assert_eq!(journal.0.load(Ordering::SeqCst), 0);
    assert_eq!(slots.available_permits(), 1);
    Ok(())
}

#[tokio::test]
async fn timeout_drops_pending_recovery_and_releases_permit() -> TestResult {
    let (identity, base, head) = fixture()?;
    let hash = head.hash();
    let slots = Arc::new(Semaphore::new(1));
    let recovery = HistoricalRecovery {
        slots: slots.clone(),
        recover: Arc::new(|_, _, _| Box::pin(std::future::pending())),
    };
    let journal = RejectJournal::default();
    let result = tokio::time::timeout(
        Duration::from_secs(12),
        restore_axis_history(&journal, identity, &head, base.version(), &recovery),
    )
    .await?;
    assert!(matches!(result, Err(SessionError::Unavailable)));
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(journal.0.load(Ordering::SeqCst), 0);
    assert_eq!(head.hash(), hash);
    Ok(())
}
