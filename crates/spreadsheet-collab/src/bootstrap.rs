//! First-context admission. Sampling is injected by the application boundary.
use super::*;
use spreadsheet_domain::workbook::CalculationContextId;
use spreadsheet_domain::{formula::Node, workbook::CalculationReason};

#[derive(Clone, Copy)]
pub(crate) enum Admission {
    Explicit,
    Bootstrap,
    AfterEdit {
        context_id: CalculationContextId,
        watermark: DocumentVersion,
    },
}
impl Admission {
    pub fn permits(self, book: &Workbook) -> bool {
        match self {
            Self::Explicit => true,
            Self::Bootstrap => needed(book).is_some(),
            Self::AfterEdit { context_id, .. } => {
                book.calculation_context()
                    .is_some_and(|context| context.context_id() == context_id)
                    && has_volatile(book)
            }
        }
    }
}
pub(crate) struct ContextAdvance<'a> {
    pub operation_id: OperationId,
    pub context: &'a CalculationContext,
    pub admission: Admission,
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
}
impl Bootstrap {
    pub fn new(sampler: Option<Sampler>) -> Self {
        Self {
            sampler,
            pending: None,
            inspected: None,
            required_edit: None,
        }
    }
    pub fn edited(&mut self, book: &Workbook, operation: &Operation) {
        if affects_calculation(operation) && (self.pending.is_some() || has_volatile(book)) {
            self.required_edit = Some(book.version());
        }
    }
    pub async fn ensure<J: OperationJournal>(
        &mut self,
        journal: &J,
        identity: AuthenticatedActor,
        workbook: &mut Arc<Workbook>,
        calculation: &CalculationHandle,
        axes: &mut axis_history::AxisHistory,
    ) -> Result<(), SessionError> {
        let Some(sampler) = &self.sampler else {
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
            let receipt = advance_context(
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
    )
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
