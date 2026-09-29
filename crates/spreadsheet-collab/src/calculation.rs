//! One coalescing calculation lane per actor; CPU work never runs on an async executor thread.
use spreadsheet_domain::workbook::{
    CalculationContext, CellAddress, DocumentId, DocumentVersion, Operation, Workbook,
};
use spreadsheet_formula::{
    Value,
    calculation::{CalculationError, CalculationLimits, CalculationProjection},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Notify, Semaphore, watch};

pub use spreadsheet_formula::calculation::{COMPATIBILITY_PROFILE, ENGINE_VERSION};

pub enum CalculationOutcome {
    Complete {
        results: BTreeMap<CellAddress, Value>,
        hash: [u8; 32],
        evaluated_cells: usize,
    },
    Failed(CalculationError),
}
pub struct CalculatedState {
    pub document_id: DocumentId,
    pub input_version: DocumentVersion,
    pub context: Option<CalculationContext>,
    pub outcome: CalculationOutcome,
}
struct Pending {
    latest: Option<Arc<Workbook>>,
    changed: BTreeSet<CellAddress>,
    rebuild: bool,
    head: DocumentVersion,
    cancel: Arc<AtomicBool>,
    stopped: bool,
}
struct Shared {
    pending: Mutex<Pending>,
    notify: Notify,
    shutdown: Notify,
    output: watch::Sender<Option<Arc<CalculatedState>>>,
}
pub(crate) struct CalculationHandle(Arc<Shared>);
impl Drop for CalculationHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl CalculationHandle {
    pub fn start(slots: Arc<Semaphore>) -> (Self, watch::Receiver<Option<Arc<CalculatedState>>>) {
        let (output, receiver) = watch::channel(None);
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending {
                latest: None,
                changed: BTreeSet::new(),
                rebuild: false,
                head: DocumentVersion::ZERO,
                cancel: Arc::new(AtomicBool::new(false)),
                stopped: false,
            }),
            notify: Notify::new(),
            shutdown: Notify::new(),
            output,
        });
        tokio::spawn(worker(shared.clone(), slots));
        (Self(shared), receiver)
    }

    /// Latest workbook plus a bounded union of changed identities; no queue of workbook copies.
    pub fn publish(&self, book: Arc<Workbook>, operation: &Operation) {
        let Ok(mut pending) = self.0.pending.lock() else {
            tracing::error!(event = "calculation_lane_poisoned");
            return;
        };
        pending.cancel.store(true, Ordering::Relaxed);
        pending.head = book.version();
        pending.latest = Some(book);
        match operation {
            Operation::SetCellValue { address, .. }
            | Operation::SetCellFormula { address, .. }
            | Operation::ClearCell { address } => {
                pending.changed.insert(*address);
            }
            Operation::CreateWorkbook { .. } => pending.rebuild = true,
            Operation::ExtendRows { .. } => pending.rebuild = true,
            Operation::InsertRows { .. } | Operation::InsertRowsRebased { .. } => {
                pending.rebuild = true
            }
            Operation::AdvanceCalculationEpoch { .. } => pending.rebuild = true,
        }
        if pending.changed.len() > 100_000 {
            pending.changed.clear();
            pending.rebuild = true;
        }
        self.0.notify.notify_one();
    }

    pub fn stop(&self) {
        if let Ok(mut pending) = self.0.pending.lock() {
            pending.stopped = true;
            pending.cancel.store(true, Ordering::Relaxed);
        }
        self.0.notify.notify_one();
        self.0.shutdown.notify_one();
    }
}

async fn worker(shared: Arc<Shared>, slots: Arc<Semaphore>) {
    let mut projection: Option<CalculationProjection> = None;
    loop {
        shared.notify.notified().await;
        let task = {
            let Ok(mut pending) = shared.pending.lock() else {
                return;
            };
            if pending.stopped {
                return;
            }
            pending.latest.take().map(|book| {
                let changed = std::mem::take(&mut pending.changed);
                let rebuild = std::mem::take(&mut pending.rebuild);
                let cancel = Arc::new(AtomicBool::new(false));
                pending.cancel = cancel.clone();
                (book, changed, rebuild, cancel)
            })
        };
        let Some((book, changed, rebuild, cancel)) = task else {
            continue;
        };
        let version = book.version();
        let document = book.document_id();
        let context = book.calculation_context().cloned();
        let permit = tokio::select! {
            result = slots.clone().acquire_owned() => match result { Ok(permit) => permit, Err(_) => return },
            _ = shared.shutdown.notified() => return,
        };
        let previous = projection.take();
        let job = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let limits = CalculationLimits::default();
            let mut projection = match (previous, rebuild) {
                (Some(mut projection), false) => {
                    projection.reconcile(&book, &changed, limits)?;
                    projection
                }
                _ => CalculationProjection::from_workbook(&book, limits)?,
            };
            let outcome = match projection.calculate(&book, limits, &cancel) {
                Ok(patch) => match projection.result_hash() {
                    Ok(hash) => CalculationOutcome::Complete {
                        results: projection.results().clone(),
                        hash,
                        evaluated_cells: patch.evaluated_cells,
                    },
                    Err(error) => CalculationOutcome::Failed(error),
                },
                Err(error) => CalculationOutcome::Failed(error),
            };
            Ok::<_, CalculationError>((projection, outcome))
        })
        .await;
        let outcome = match job {
            Ok(Ok((next, outcome))) => {
                projection = Some(next);
                outcome
            }
            Ok(Err(error)) => CalculationOutcome::Failed(error),
            Err(_) => CalculationOutcome::Failed(CalculationError::InvalidProjection),
        };
        let Ok(pending) = shared.pending.lock() else {
            return;
        };
        if pending.stopped {
            return;
        }
        if pending.head == version {
            shared.output.send_replace(Some(Arc::new(CalculatedState {
                document_id: document,
                input_version: version,
                context,
                outcome,
            })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn queued_versions_coalesce_without_losing_dependency_updates()
    -> Result<(), Box<dyn std::error::Error>> {
        use spreadsheet_domain::workbook::*;
        let row = RowId::from_bytes([3; 16])?;
        let left = ColumnId::from_bytes([4; 16])?;
        let right = ColumnId::from_bytes([5; 16])?;
        let sheet = SheetId::from_bytes([2; 16])?;
        let seed = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet1".into(),
                rows: vec![row],
                columns: vec![left, right],
            },
        };
        let mut book = Workbook::empty(DocumentId::from_bytes([1; 16])?)
            .candidate(&seed, DocumentVersion::new(1)?)?;
        let origin = CellAddress {
            sheet,
            row,
            column: right,
        };
        let formula = Operation::SetCellFormula {
            address: origin,
            formula: spreadsheet_formula::binder::bind("=A1+1", &book, origin)?,
        };
        book = book.candidate(&formula, DocumentVersion::new(2)?)?;
        let slots = Arc::new(Semaphore::new(0));
        let (handle, mut output) = CalculationHandle::start(slots.clone());
        handle.publish(Arc::new(book.clone()), &seed);
        for version in 3_u32..=50 {
            let operation = Operation::SetCellValue {
                address: CellAddress {
                    sheet,
                    row,
                    column: left,
                },
                value: CellValue::Number(f64::from(version)),
            };
            book = book.candidate(&operation, DocumentVersion::new(u64::from(version))?)?;
            handle.publish(Arc::new(book.clone()), &operation);
            tokio::task::yield_now().await;
        }
        slots.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                output.changed().await?;
                if let Some(state) = output.borrow().as_ref() {
                    assert_eq!(state.input_version.get(), 50);
                    if let CalculationOutcome::Complete { results, hash, .. } = &state.outcome {
                        assert_eq!(results.get(&origin), Some(&Value::Number(51.0)));
                        let mut direct = CalculationProjection::from_workbook(
                            &book,
                            CalculationLimits::default(),
                        )?;
                        direct.calculate(
                            &book,
                            CalculationLimits::default(),
                            &AtomicBool::new(false),
                        )?;
                        assert_eq!(*hash, direct.result_hash()?);
                    } else {
                        return Err("coalesced calculation failed".into());
                    }
                    return Ok::<(), Box<dyn std::error::Error>>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn dropping_idle_lane_closes_output_without_an_orphan_task()
    -> Result<(), Box<dyn std::error::Error>> {
        let (handle, mut output) = CalculationHandle::start(Arc::new(Semaphore::new(1)));
        drop(handle);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), output.changed())
                .await?
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_cancels_waiting_for_cpu_admission() -> Result<(), Box<dyn std::error::Error>>
    {
        use spreadsheet_domain::workbook::*;
        let id = DocumentId::from_bytes([1; 16])?;
        let operation = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes([2; 16])?,
                name: "Sheet1".into(),
                rows: vec![RowId::from_bytes([3; 16])?],
                columns: vec![ColumnId::from_bytes([4; 16])?],
            },
        };
        let book = Workbook::empty(id).candidate(&operation, DocumentVersion::new(1)?)?;
        let (handle, mut output) = CalculationHandle::start(Arc::new(Semaphore::new(0)));
        handle.publish(Arc::new(book), &operation);
        // Let the worker drain the input and wait for a permit which will never arrive.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        drop(handle);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), output.changed())
                .await?
                .is_err()
        );
        Ok(())
    }
}
