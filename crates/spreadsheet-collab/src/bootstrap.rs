//! First-context admission. Sampling is injected by the application boundary.
use super::*;
use spreadsheet_domain::workbook::CalculationContextId;
use spreadsheet_domain::{formula::Node, workbook::CalculationReason};

#[derive(Clone, Copy)]
pub(crate) enum Admission {
    Explicit,
    Bootstrap,
    Open {
        context_id: CalculationContextId,
    },
    AfterEdit {
        context_id: CalculationContextId,
        watermark: DocumentVersion,
    },
    Timer {
        context_id: CalculationContextId,
        timestamp_ms: i64,
        day: i32,
        offset: i16,
    },
}
impl Admission {
    pub fn permits(self, book: &Workbook) -> bool {
        match self {
            Self::Explicit => true,
            Self::Bootstrap => needed(book).is_some(),
            Self::Open { context_id } => {
                book.calculation_context()
                    .is_some_and(|context| context.context_id() == context_id)
                    && has_volatile(book)
            }
            Self::AfterEdit { context_id, .. } => {
                book.calculation_context()
                    .is_some_and(|context| context.context_id() == context_id)
                    && has_volatile(book)
            }
            Self::Timer {
                context_id,
                timestamp_ms,
                day,
                offset,
            } => {
                book.calculation_context()
                    .is_some_and(|context| context.context_id() == context_id)
                    && timer_due_at(book, timestamp_ms, day, offset)
            }
        }
    }
}
pub(crate) struct ContextAdvance<'a> {
    pub operation_id: OperationId,
    pub context: &'a CalculationContext,
    pub admission: Admission,
}
pub(crate) struct ContextAdvanceOutcome {
    pub receipt: Option<CommitReceipt>,
    /// Only a newly committed event has provenance available at this boundary.
    pub event: Option<Arc<CommittedEvent>>,
}

pub(crate) type Sampler = Arc<
    dyn Fn(CalculationReason) -> Result<(OperationId, CalculationContext), SessionError>
        + Send
        + Sync,
>;
pub(crate) struct Bootstrap {
    pub sampler: Option<Sampler>,
    pending: Option<(OperationId, CalculationContext, Admission)>,
    inspected: Option<DocumentVersion>,
    required_edit: Option<DocumentVersion>,
    pending_timer: Option<(OperationId, CalculationContext, Admission)>,
    pending_open: Option<(OperationId, CalculationContext, Admission)>,
    head_event: Option<Arc<CommittedEvent>>,
}
impl Bootstrap {
    pub fn new(sampler: Option<Sampler>) -> Self {
        Self {
            sampler,
            pending: None,
            inspected: None,
            required_edit: None,
            pending_timer: None,
            pending_open: None,
            head_event: None,
        }
    }
    pub fn take_head_event(&mut self) -> Option<Arc<CommittedEvent>> {
        self.head_event.take()
    }
    fn record_advance(&mut self, outcome: ContextAdvanceOutcome) -> Option<CommitReceipt> {
        if outcome.event.is_some() {
            self.head_event = outcome.event;
        }
        outcome.receipt
    }
    pub fn edited(&mut self, book: &Workbook, operation: &Operation) {
        if affects_calculation(operation) && (self.pending.is_some() || has_volatile(book)) {
            self.required_edit = Some(book.version());
        }
    }
    pub async fn opened<J: OperationJournal>(
        &mut self,
        journal: &J,
        identity: AuthenticatedActor,
        workbook: &mut Arc<Workbook>,
        calculation: &CalculationHandle,
        axes: &mut axis_history::AxisHistory,
        renew: bool,
    ) -> Result<(), SessionError> {
        // Resolve an uncertain Open before checking whether a new one is needed.
        if self.pending_open.is_none() {
            if !renew || !has_volatile(workbook) {
                return Ok(());
            }
            let Some(sampler) = &self.sampler else {
                return Ok(());
            };
            let Some(previous) = workbook.calculation_context() else {
                return Err(SessionError::InvalidCommand);
            };
            let (id, context) = sampler(CalculationReason::Open)?;
            if context.reason() != CalculationReason::Open
                || context.engine_version() != calculation::ENGINE_VERSION
                || context.compatibility_profile() != calculation::COMPATIBILITY_PROFILE
                || context.utc_offset_minutes() != previous.utc_offset_minutes()
            {
                return Err(SessionError::InvalidCommand);
            }
            self.pending_open = Some((
                id,
                context,
                Admission::Open {
                    context_id: previous.context_id(),
                },
            ));
        }
        let Some((id, context, admission)) = &self.pending_open else {
            return Err(SessionError::InvalidCommand);
        };
        let outcome = advance_context(
            journal,
            identity,
            workbook,
            ContextAdvance {
                operation_id: *id,
                context,
                admission: *admission,
            },
            calculation,
            axes,
        )
        .await?;
        self.record_advance(outcome);
        self.ensure(journal, identity, workbook, calculation, axes)
            .await?;
        self.pending_open = None;
        Ok(())
    }
    pub async fn timer<J: OperationJournal>(
        &mut self,
        journal: &J,
        identity: AuthenticatedActor,
        workbook: &mut Arc<Workbook>,
        calculation: &CalculationHandle,
        axes: &mut axis_history::AxisHistory,
    ) -> Result<(), SessionError> {
        if self.pending_timer.is_none() {
            let (now, today) = clock_consumers(workbook);
            if !now && !today {
                return Ok(());
            }
            let Some(sampler) = &self.sampler else {
                return Ok(());
            };
            let Some(previous) = workbook.calculation_context() else {
                return Err(SessionError::InvalidCommand);
            };
            let (id, context) = sampler(CalculationReason::Timer)?;
            if context.reason() != CalculationReason::Timer
                || context.engine_version() != calculation::ENGINE_VERSION
                || context.compatibility_profile() != calculation::COMPATIBILITY_PROFILE
                || context.utc_offset_minutes() != previous.utc_offset_minutes()
            {
                return Err(SessionError::InvalidCommand);
            }
            if !timer_due_at(
                workbook,
                context.effective_timestamp_utc_ms(),
                context.civil_day_since_unix_epoch(),
                context.utc_offset_minutes(),
            ) {
                return Ok(());
            }
            let admission = Admission::Timer {
                context_id: previous.context_id(),
                timestamp_ms: context.effective_timestamp_utc_ms(),
                day: context.civil_day_since_unix_epoch(),
                offset: context.utc_offset_minutes(),
            };
            self.pending_timer = Some((id, context, admission));
        }
        let Some((id, context, admission)) = &self.pending_timer else {
            return Err(SessionError::InvalidCommand);
        };
        let outcome = advance_context(
            journal,
            identity,
            workbook,
            ContextAdvance {
                operation_id: *id,
                context,
                admission: *admission,
            },
            calculation,
            axes,
        )
        .await?;
        self.record_advance(outcome);
        self.pending_timer = None;
        // A refreshed external epoch may be followed by a newer content edit.
        // Resolve its durable ordering before declaring this head inspected.
        self.ensure(journal, identity, workbook, calculation, axes)
            .await
    }
    pub async fn ensure<J: OperationJournal>(
        &mut self,
        journal: &J,
        identity: AuthenticatedActor,
        workbook: &mut Arc<Workbook>,
        calculation: &CalculationHandle,
        axes: &mut axis_history::AxisHistory,
    ) -> Result<(), SessionError> {
        let Some(sampler) = self.sampler.clone() else {
            return Ok(());
        };
        // Recover admission intent from durable input history after an actor restart.
        // Stop at the first relevant edit; each decoded page remains bounded.
        if self.inspected != Some(workbook.version())
            && needed(workbook).is_none()
            && has_volatile(workbook)
        {
            self.required_edit = recover_edit(journal, identity, workbook).await?;
        }
        // At most an older uncertain trigger plus one trigger for newer edits.
        for _ in 0..2 {
            if self.pending.is_none() {
                if self.required_edit.is_none()
                    && self.inspected == Some(workbook.version())
                    && needed(workbook).is_none()
                {
                    return Ok(());
                }
                let trigger = if let Some(reason) = needed(workbook) {
                    Some((reason, Admission::Bootstrap))
                } else if let Some(watermark) =
                    self.required_edit.filter(|_| has_volatile(workbook))
                {
                    workbook.calculation_context().map(|context| {
                        (
                            CalculationReason::VolatileDependency,
                            Admission::AfterEdit {
                                context_id: context.context_id(),
                                watermark,
                            },
                        )
                    })
                } else {
                    None
                };
                let Some((reason, admission)) = trigger else {
                    self.required_edit = None;
                    self.inspected = Some(workbook.version());
                    return Ok(());
                };
                let sampled = sampler(reason)?;
                if sampled.1.reason() != reason
                    || sampled.1.engine_version() != calculation::ENGINE_VERSION
                    || sampled.1.compatibility_profile() != calculation::COMPATIBILITY_PROFILE
                {
                    return Err(SessionError::InvalidCommand);
                }
                self.pending = Some((sampled.0, sampled.1, admission));
            }
            let Some((id, context, admission)) = &self.pending else {
                return Err(SessionError::InvalidCommand);
            };
            let admission = *admission;
            let prepared_head = workbook.version();
            let outcome = advance_context(
                journal,
                identity,
                workbook,
                ContextAdvance {
                    operation_id: *id,
                    context,
                    admission,
                },
                calculation,
                axes,
            )
            .await?;
            let receipt = self.record_advance(outcome);
            if workbook.version() != prepared_head {
                // Refresh/receipt/CAS retry may discover an epoch followed by a
                // newer edit. Resolve that ordering before caching this head.
                self.required_edit = recover_edit(journal, identity, workbook).await?;
            } else {
                let covered = self.required_edit.is_some_and(|required| {
                    receipt.as_ref().is_some_and(|receipt| receipt.committed_version > required)
                        || (receipt.is_none() && matches!(admission, Admission::AfterEdit { watermark, .. } if watermark >= required))
                });
                if covered || !has_volatile(workbook) {
                    self.required_edit = None;
                }
            }
            self.pending = None;
            self.inspected = Some(workbook.version());
            if self.required_edit.is_none() && needed(workbook).is_none() {
                return Ok(());
            }
        }
        Err(SessionError::Unavailable)
    }
}
async fn recover_edit<J: OperationJournal>(
    journal: &J,
    identity: AuthenticatedActor,
    book: &Workbook,
) -> Result<Option<DocumentVersion>, SessionError> {
    if needed(book).is_some() || !has_volatile(book) {
        return Ok(None);
    }
    let mut after = journal
        .calculation_epoch_version(identity, book.document_id(), book.version())
        .await?
        .ok_or(JournalError::Corrupt)?;
    let start = after;
    while after < book.version() {
        if after.get().saturating_sub(start.get()) >= 4096 {
            return Err(SessionError::Unavailable);
        }
        let events = journal
            .load_events(identity, book.document_id(), after, 128)
            .await?;
        if events.is_empty() {
            return Err(JournalError::Corrupt.into());
        }
        for event in events {
            if event.version > book.version() {
                break;
            }
            if event.version.get()
                != after
                    .get()
                    .checked_add(1)
                    .ok_or(SessionError::InvalidCommand)?
            {
                return Err(JournalError::Corrupt.into());
            }
            after = event.version;
            if affects_calculation(&event.operation) {
                return Ok(Some(book.version()));
            }
        }
        tokio::task::yield_now().await;
    }
    Ok(None)
}
fn formula_is_volatile(formula: &spreadsheet_domain::formula::Formula) -> bool {
    formula.ast().nodes().iter().any(|node| matches!(node, Node::Call { name, .. } if matches!(name.as_str(), "NOW" | "TODAY" | "RAND")))
}
pub(crate) fn needed(book: &Workbook) -> Option<CalculationReason> {
    if book.calculation_context().is_some_and(|context| {
        context.engine_version() == calculation::ENGINE_VERSION
            && context.compatibility_profile() == calculation::COMPATIBILITY_PROFILE
    }) {
        return None;
    }
    if !has_volatile(book) {
        return None;
    }
    Some(if book.calculation_context().is_some() {
        CalculationReason::EngineUpgrade
    } else {
        CalculationReason::Open
    })
}
fn has_volatile(book: &Workbook) -> bool {
    book.sheets().iter().any(|sheet| sheet.cells().values().any(|content| matches!(content, CellContent::Formula(formula) if formula_is_volatile(formula))))
}
fn affects_calculation(operation: &Operation) -> bool {
    !matches!(
        operation,
        Operation::CreateWorkbook { .. }
            | Operation::AdvanceCalculationEpoch { .. }
            | Operation::ExtendRows { .. }
            | Operation::StyleWrite { .. }
            | Operation::StyleCompensation { .. }
    )
}
fn clock_consumers(book: &Workbook) -> (bool, bool) {
    let mut now = false;
    let mut today = false;
    for sheet in book.sheets() {
        for content in sheet.cells().values() {
            if let CellContent::Formula(formula) = content {
                for node in formula.ast().nodes() {
                    if let Node::Call { name, .. } = node {
                        now |= name.as_str() == "NOW";
                        today |= name.as_str() == "TODAY";
                    }
                }
            }
        }
    }
    (now, today)
}
fn timer_due_at(book: &Workbook, timestamp_ms: i64, day: i32, offset: i16) -> bool {
    let Some(context) = book.calculation_context() else {
        return false;
    };
    if offset != context.utc_offset_minutes() || timestamp_ms < context.effective_timestamp_utc_ms()
    {
        return false;
    }
    let (now, today) = clock_consumers(book);
    (now && timestamp_ms
        .checked_sub(context.effective_timestamp_utc_ms())
        .is_some_and(|elapsed| elapsed >= 60_000))
        || (today && day != context.civil_day_since_unix_epoch())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::{
        ActorId, CalculationContextId, CellAddress, ColumnId, RowId, SheetId, SheetSeed,
        WorkspaceId,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn fixture(source: &str) -> Result<Workbook, Box<dyn std::error::Error>> {
        let at = CellAddress {
            sheet: SheetId::from_bytes([1; 16])?,
            row: RowId::from_bytes([2; 16])?,
            column: ColumnId::from_bytes([3; 16])?,
        };
        let seed = SheetSeed {
            id: at.sheet,
            name: "Sheet1".into(),
            rows: vec![at.row],
            columns: vec![at.column],
        };
        let book = Workbook::empty(DocumentId::from_bytes([4; 16])?).candidate(
            &Operation::CreateWorkbook { sheet: seed },
            DocumentVersion::new(1)?,
        )?;
        Ok(book.candidate(
            &Operation::SetCellFormula {
                address: at,
                formula: spreadsheet_formula::binder::bind(source, &book, at)?,
            },
            DocumentVersion::new(2)?,
        )?)
    }
    #[test]
    fn formatting_and_its_inverse_do_not_advance_volatile_epoch()
    -> Result<(), Box<dyn std::error::Error>> {
        use spreadsheet_domain::{
            style::{Change, StyleChange},
            workbook::{GuardedStyleWrite, OperationId, StyleWriteKind},
        };
        let book = fixture("=RAND()")?;
        let sheet = &book.sheets()[0];
        let at = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[0],
            column: sheet.columns()[0],
        };
        let original = spreadsheet_domain::style_write::prepare_style_write(
            &book,
            &[at],
            &[StyleChange::FontBold(Change::Set(true))],
            StyleWriteKind::Cell,
        )?;
        assert!(!affects_calculation(&original));
        let Operation::StyleWrite { kind, writes } = original else {
            return Err("expected style write".into());
        };
        let inverse = Operation::StyleCompensation {
            kind,
            writes: writes
                .into_iter()
                .map(|w| GuardedStyleWrite {
                    address: w.address,
                    previous: w.changes,
                    changes: w.previous,
                })
                .collect(),
            undoes: OperationId::from_bytes([9; 16])?,
            undoes_version: DocumentVersion::new(3)?,
        };
        assert!(!affects_calculation(&inverse));
        Ok(())
    }
    fn context(reason: CalculationReason, engine: &str) -> Result<CalculationContext, DomainError> {
        CalculationContext::new(
            CalculationContextId::from_bytes([5; 16])?,
            1,
            0,
            0,
            0,
            [6; 32],
            engine.into(),
            calculation::COMPATIBILITY_PROFILE.into(),
            reason,
        )
    }
    #[test]
    #[ignore = "release timer classification harness; excludes queues, database and calculation"]
    fn timer_classification_benchmark() -> Result<(), Box<dyn std::error::Error>> {
        use spreadsheet_domain::workbook::CellValue;
        use std::{hint::black_box, time::Instant};
        for cell_count in [1_000usize, 10_000, 100_000] {
            let seed = SheetSeed {
                id: SheetId::from_bytes([1; 16])?,
                name: "Sheet1".into(),
                rows: (1..=cell_count)
                    .map(|n| RowId::from_bytes((n as u128).to_be_bytes()))
                    .collect::<Result<Vec<_>, _>>()?,
                columns: vec![ColumnId::from_bytes([3; 16])?],
            };
            let address = CellAddress {
                sheet: seed.id,
                row: seed.rows[0],
                column: seed.columns[0],
            };
            let cells = seed
                .rows
                .iter()
                .map(|row| {
                    (
                        CellAddress {
                            row: *row,
                            ..address
                        },
                        CellContent::Value(CellValue::Number(1.0)),
                    )
                })
                .collect::<Vec<_>>();
            let static_book = Workbook::hydrate(
                DocumentId::from_bytes([4; 16])?,
                DocumentVersion::new(1)?,
                seed,
                cells,
            )?;
            let clock_book = static_book
                .candidate(
                    &Operation::SetCellFormula {
                        address,
                        formula: spreadsheet_formula::binder::bind(
                            "=NOW()",
                            &static_book,
                            address,
                        )?,
                    },
                    DocumentVersion::new(2)?,
                )?
                .candidate(
                    &Operation::AdvanceCalculationEpoch {
                        context: context(CalculationReason::Explicit, calculation::ENGINE_VERSION)?,
                    },
                    DocumentVersion::new(3)?,
                )?;
            for (name, book, expected) in [
                ("static", &static_book, (false, false)),
                ("one_now", &clock_book, (true, false)),
            ] {
                assert_eq!(clock_consumers(book), expected);
                let mut samples = Vec::with_capacity(1_100);
                for _ in 0..1_100 {
                    let start = Instant::now();
                    black_box(clock_consumers(black_box(book)));
                    samples.push(start.elapsed().as_secs_f64() * 1_000_000.0);
                }
                samples.drain(..100);
                samples.sort_by(f64::total_cmp);
                println!(
                    "timer_classification cells={cell_count} profile={name} samples=1000 p50_us={:.3} p95_us={:.3} max_us={:.3}",
                    samples[499], samples[949], samples[999]
                );
            }
            assert!(timer_due_at(&clock_book, 60_000, 0, 0));
        }
        Ok(())
    }
    #[test]
    fn open_admission_renews_only_its_observed_volatile_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let admission = Admission::Open {
            context_id: CalculationContextId::from_bytes([5; 16])?,
        };
        for (source, expected) in [
            ("=RAND()", true),
            ("=NOW()", true),
            ("=TODAY()", true),
            ("=1+2", false),
            ("=\"RAND()\"", false),
        ] {
            let book = fixture(source)?.candidate(
                &Operation::AdvanceCalculationEpoch {
                    context: context(CalculationReason::Explicit, calculation::ENGINE_VERSION)?,
                },
                DocumentVersion::new(3)?,
            )?;
            assert_eq!(admission.permits(&book), expected);
            assert!(
                !Admission::Open {
                    context_id: CalculationContextId::from_bytes([10; 16])?
                }
                .permits(&book)
            );
        }
        assert!(!admission.permits(&fixture("=RAND()")?));
        Ok(())
    }
    #[test]
    fn timer_policy_only_advances_clock_consumers_at_due_boundaries()
    -> Result<(), Box<dyn std::error::Error>> {
        for (source, minute, midnight) in [
            ("=NOW()", true, true),
            ("=TODAY()", false, true),
            ("=RAND()", false, false),
            ("=\"NOW()\"", false, false),
        ] {
            let book = fixture(source)?.candidate(
                &Operation::AdvanceCalculationEpoch {
                    context: context(CalculationReason::Explicit, calculation::ENGINE_VERSION)?,
                },
                DocumentVersion::new(3)?,
            )?;
            assert!(!timer_due_at(&book, 59_999, 0, 0));
            assert_eq!(timer_due_at(&book, 60_000, 0, 0), minute);
            assert_eq!(timer_due_at(&book, 86_400_000, 1, 0), midnight);
            assert!(!timer_due_at(&book, 86_400_000, 1, 60));
        }
        let previous = CalculationContext::new(
            CalculationContextId::from_bytes([10; 16])?,
            1,
            60_000,
            0,
            0,
            [11; 32],
            calculation::ENGINE_VERSION.into(),
            calculation::COMPATIBILITY_PROFILE.into(),
            CalculationReason::Explicit,
        )?;
        let book = fixture("=NOW()")?.candidate(
            &Operation::AdvanceCalculationEpoch { context: previous },
            DocumentVersion::new(3)?,
        )?;
        assert!(!timer_due_at(&book, 59_999, 0, 0));
        assert!(!timer_due_at(&book, 119_999, 0, 0));
        assert!(timer_due_at(&book, 120_000, 0, 0));
        Ok(())
    }
    #[test]
    fn classification_uses_calls_and_retains_compatible_context()
    -> Result<(), Box<dyn std::error::Error>> {
        for source in ["=RAND()", "=NOW()", "=TODAY()", "=IF(FALSE,RAND(),7)"] {
            let book = fixture(source)?;
            assert_eq!(needed(&book), Some(CalculationReason::Open));
            let current = book.candidate(
                &Operation::AdvanceCalculationEpoch {
                    context: context(CalculationReason::Explicit, calculation::ENGINE_VERSION)?,
                },
                DocumentVersion::new(3)?,
            )?;
            assert_eq!(needed(&current), None);
            let outdated = book.candidate(
                &Operation::AdvanceCalculationEpoch {
                    context: context(CalculationReason::Explicit, "old-engine")?,
                },
                DocumentVersion::new(3)?,
            )?;
            assert_eq!(needed(&outdated), Some(CalculationReason::EngineUpgrade));
        }
        for source in ["=1+2", "=\"RAND()\"", "=RANDOM()"] {
            assert_eq!(needed(&fixture(source)?), None);
        }
        Ok(())
    }
    #[tokio::test]
    async fn pending_bootstrap_failure_preserves_sampled_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let mut bootstrap = Bootstrap::new(Some(Arc::new(move |reason| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok((
                OperationId::from_bytes([7; 16])?,
                context(reason, calculation::ENGINE_VERSION)?,
            ))
        })));
        let mut book = Arc::new(fixture("=RAND()")?);
        let before = book.hash();
        let mut axes = axis_history::AxisHistory::new(&book);
        let (calculation, _) = CalculationHandle::start(Arc::new(Semaphore::new(1)));
        let identity = AuthenticatedActor {
            actor_id: ActorId::from_bytes([8; 16])?,
            workspace_id: WorkspaceId::from_bytes([9; 16])?,
        };
        let journal = crate::historical_recovery_tests::RejectJournal::default();
        for _ in 0..2 {
            assert!(
                bootstrap
                    .ensure(&journal, identity, &mut book, &calculation, &mut axes)
                    .await
                    .is_err()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                bootstrap
                    .pending
                    .as_ref()
                    .map(|(id, context, _)| (*id, context.random_seed())),
                Some((OperationId::from_bytes([7; 16])?, &[6; 32]))
            );
            assert_eq!(book.hash(), before);
        }
        calculation.stop();
        Ok(())
    }
}
