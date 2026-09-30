//! Bounded, content-free axis checkpoints for binding delayed formula commands.
use crate::SessionError;
use spreadsheet_domain::{formula::Formula, workbook::*};
use std::collections::{BTreeSet, VecDeque};

pub(crate) const MAX_VERSIONS: u64 = 10_000;
const MAX_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct Checkpoint {
    version: DocumentVersion,
    seeds: Vec<SheetSeed>,
    transition: Option<Operation>,
    bytes: usize,
}
pub(crate) struct AxisHistory {
    checkpoints: VecDeque<Checkpoint>,
    bytes: usize,
    relocation_floor: DocumentVersion,
}
impl AxisHistory {
    pub(crate) fn covers(&self, base: DocumentVersion) -> bool {
        base >= self.relocation_floor
            && self
                .checkpoints
                .front()
                .is_some_and(|entry| entry.version <= base)
    }
    pub(crate) fn new(book: &Workbook) -> Self {
        let mut history = Self {
            checkpoints: VecDeque::new(),
            bytes: 0,
            relocation_floor: DocumentVersion::ZERO,
        };
        if let Some(checkpoint) = Self::checkpoint(book, None) {
            history.bytes = checkpoint.bytes;
            history.checkpoints.push_back(checkpoint);
        }
        history
    }
    fn checkpoint(book: &Workbook, transition: Option<Operation>) -> Option<Checkpoint> {
        if book.version() == DocumentVersion::ZERO {
            return None;
        }
        // Account generously for vector buffers, names, entries and transition IDs.
        let mut bytes = 1024usize;
        for sheet in book.sheets() {
            bytes = bytes
                .checked_add(512)?
                .checked_add(sheet.name().len())?
                .checked_add(
                    sheet
                        .rows()
                        .len()
                        .checked_add(sheet.columns().len())?
                        .checked_mul(32)?,
                )?;
        }
        if let Some(operation) = &transition {
            bytes = bytes.checked_add(match operation {
                Operation::CreateWorkbook { sheet } | Operation::AddSheet { sheet } => {
                    (sheet.rows.len() + sheet.columns.len())
                        .checked_mul(32)?
                        .checked_add(sheet.name.len())?
                }
                Operation::ExtendRows { rows, .. }
                | Operation::InsertRows { rows, .. }
                | Operation::InsertRowsRebased { rows, .. }
                | Operation::DeleteRows { rows, .. } => rows.len().checked_mul(32)?,
                Operation::InsertColumnsRebased { columns, .. }
                | Operation::DeleteColumns { columns, .. } => columns.len().checked_mul(32)?,
                _ => return None,
            })?;
        }
        if bytes > MAX_BYTES {
            return None;
        }
        Some(Checkpoint {
            version: book.version(),
            bytes,
            transition,
            seeds: book
                .sheets()
                .iter()
                .map(|sheet| SheetSeed {
                    id: sheet.id(),
                    name: sheet.name().to_owned(),
                    rows: sheet.rows().to_vec(),
                    columns: sheet.columns().to_vec(),
                })
                .collect(),
        })
    }
    pub(crate) fn structural(operation: &Operation) -> bool {
        matches!(
            operation,
            Operation::CreateWorkbook { .. }
                | Operation::AddSheet { .. }
                | Operation::ExtendRows { .. }
                | Operation::InsertRows { .. }
                | Operation::InsertRowsRebased { .. }
                | Operation::DeleteRows { .. }
                | Operation::InsertColumnsRebased { .. }
                | Operation::DeleteColumns { .. }
        )
    }
    /// Prepare allocations before COMMIT; publication only moves prepared buffers.
    pub(crate) fn prepare(&mut self, book: &Workbook, operation: &Operation) -> Option<Checkpoint> {
        if self.checkpoints.try_reserve(1).is_err() {
            return None;
        }
        Self::checkpoint(book, Some(operation.clone()))
    }
    pub(crate) fn publish(
        &mut self,
        book: &Workbook,
        operation: &Operation,
        checkpoint: Option<Checkpoint>,
    ) {
        if matches!(operation, Operation::MoveRange { .. }) {
            self.relocation_floor = book.version();
        }
        if Self::structural(operation) {
            if let Some(checkpoint) = checkpoint {
                self.bytes += checkpoint.bytes;
                self.checkpoints.push_back(checkpoint);
            } else {
                self.checkpoints.clear();
                self.bytes = 0;
            }
        }
        let oldest = book.version().get().saturating_sub(MAX_VERSIONS);
        while self.bytes > MAX_BYTES
            || self
                .checkpoints
                .get(1)
                .is_some_and(|entry| entry.version.get() <= oldest)
        {
            let Some(entry) = self.checkpoints.pop_front() else {
                break;
            };
            self.bytes -= entry.bytes;
        }
    }
    pub(crate) fn bind(
        &self,
        source: &str,
        head: &Workbook,
        address: CellAddress,
        base: DocumentVersion,
    ) -> Result<Formula, SessionError> {
        head.cell_content(address)?; // Deleted origins have typed domain errors.
        if base < self.relocation_floor {
            return Err(SessionError::SourceVersionChanged);
        }
        if base == DocumentVersion::ZERO
            || base > head.version()
            || head.version().get().saturating_sub(base.get()) > MAX_VERSIONS
        {
            return Err(SessionError::RebaseWindowExceeded);
        }
        // A fresh actor restores move history even when axes did not change.
        if !self.covers(base) {
            return Err(SessionError::RebaseWindowExceeded);
        }
        if base >= head.last_axis_change_version() {
            let bound = spreadsheet_formula::binder::bind(source, head, address)
                .map_err(|_| SessionError::InvalidFormula)?;
            return Formula::new(
                bound.source().to_owned(),
                bound.canonical().to_owned(),
                bound.ast().clone(),
                address,
                base,
            )
            .map_err(|_| SessionError::InvalidFormula);
        }
        let index = self
            .checkpoints
            .iter()
            .rposition(|entry| entry.version <= base)
            .ok_or(SessionError::RebaseWindowExceeded)?;
        let entry = self
            .checkpoints
            .get(index)
            .ok_or(SessionError::RebaseWindowExceeded)?;
        let mut work = 1_000_000usize;
        charge(&mut work, axis_count(entry).saturating_mul(2))?;
        let mut axes = Workbook::hydrate_sheets(head.document_id(), base, entry.seeds.clone(), [])?;
        let mut formula = spreadsheet_formula::binder::bind(source, &axes, address)
            .map_err(|_| SessionError::InvalidFormula)?;
        for entry in self.checkpoints.iter().skip(index + 1) {
            let ranges = formula
                .ast()
                .nodes()
                .iter()
                .filter(|node| matches!(node, spreadsheet_domain::formula::Node::Range(_)))
                .count();
            let deletion_axis = match entry.transition.as_ref() {
                Some(Operation::DeleteRows { sheet, .. }) => axes
                    .sheets()
                    .iter()
                    .find(|current| current.id() == *sheet)
                    .map_or(0, |sheet| sheet.rows().len()),
                Some(Operation::DeleteColumns { sheet, .. }) => axes
                    .sheets()
                    .iter()
                    .find(|current| current.id() == *sheet)
                    .map_or(0, |sheet| sheet.columns().len()),
                _ => 0,
            };
            charge(
                &mut work,
                axis_count(entry)
                    .saturating_mul(2)
                    .saturating_add(formula.ast().nodes().len().saturating_mul(8))
                    .saturating_add(
                        deletion_axis.saturating_mul(ranges.saturating_mul(4).saturating_add(2)),
                    ),
            )?;
            let next = Workbook::hydrate_sheets(
                head.document_id(),
                entry.version,
                entry.seeds.clone(),
                [],
            )?;
            let sheet = next
                .sheets()
                .iter()
                .find(|sheet| sheet.id() == address.sheet)
                .ok_or(DomainError::MissingSheet)?;
            let row = sheet
                .row_position(address.row)
                .ok_or(DomainError::MissingRow)?;
            let column = sheet
                .column_position(address.column)
                .ok_or(DomainError::MissingColumn)?;
            let rows = |id, row| {
                next.sheets()
                    .iter()
                    .find(|sheet| sheet.id() == id)
                    .and_then(|sheet| sheet.row_position(row))
            };
            let columns = |id, column| {
                next.sheets()
                    .iter()
                    .find(|sheet| sheet.id() == id)
                    .and_then(|sheet| sheet.column_position(column))
            };
            formula = match entry
                .transition
                .as_ref()
                .ok_or(SessionError::RebaseWindowExceeded)?
            {
                Operation::InsertRows { .. } | Operation::InsertRowsRebased { .. } => {
                    formula.rebase_row_offsets(entry.version, row, rows)
                }
                Operation::InsertColumnsRebased { .. } => {
                    formula.rebase_column_offsets(entry.version, column, columns)
                }
                Operation::DeleteRows {
                    sheet,
                    rows: deleted,
                } => {
                    let old = axes
                        .sheets()
                        .iter()
                        .find(|current| current.id() == *sheet)
                        .ok_or(DomainError::MissingSheet)?;
                    let deleted: BTreeSet<_> = deleted.iter().copied().collect();
                    formula.rebase_deleted_rows(
                        entry.version,
                        row,
                        *sheet,
                        &deleted,
                        rows,
                        |start, end| survivors(old.rows(), start, end, |id| !deleted.contains(&id)),
                    )
                }
                Operation::DeleteColumns {
                    sheet,
                    columns: deleted,
                } => {
                    let old = axes
                        .sheets()
                        .iter()
                        .find(|current| current.id() == *sheet)
                        .ok_or(DomainError::MissingSheet)?;
                    let deleted: BTreeSet<_> = deleted.iter().copied().collect();
                    formula.rebase_deleted_columns(
                        entry.version,
                        column,
                        *sheet,
                        &deleted,
                        columns,
                        |start, end| {
                            survivors(old.columns(), start, end, |id| !deleted.contains(&id))
                        },
                    )
                }
                Operation::CreateWorkbook { .. }
                | Operation::AddSheet { .. }
                | Operation::ExtendRows { .. } => Ok(formula),
                _ => return Err(SessionError::RebaseWindowExceeded),
            }
            .map_err(|_| SessionError::InvalidFormula)?;
            axes = next;
        }
        Formula::new(
            formula.source().to_owned(),
            formula.canonical().to_owned(),
            formula.ast().clone(),
            address,
            base,
        )
        .map_err(|_| SessionError::InvalidFormula)
    }
}
fn axis_count(entry: &Checkpoint) -> usize {
    entry.seeds.iter().fold(0usize, |total, sheet| {
        total
            .saturating_add(sheet.rows.len())
            .saturating_add(sheet.columns.len())
    })
}
fn charge(work: &mut usize, cost: usize) -> Result<(), SessionError> {
    *work = work
        .checked_sub(cost)
        .ok_or(SessionError::RebaseWindowExceeded)?;
    Ok(())
}
fn survivors<T: Copy + PartialEq>(
    axis: &[T],
    start: T,
    end: T,
    alive: impl Fn(T) -> bool,
) -> Option<(T, T)> {
    let first = axis.iter().position(|id| *id == start)?;
    let last = axis.iter().position(|id| *id == end)?;
    let mut values = axis
        .get(first.min(last)..=first.max(last))?
        .iter()
        .copied()
        .filter(|id| alive(*id));
    let low = values.next()?;
    let high = values.last().unwrap_or(low);
    Some(if first <= last {
        (low, high)
    } else {
        (high, low)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    type ResultTest = Result<(), Box<dyn std::error::Error>>;
    fn seed() -> Result<SheetSeed, DomainError> {
        Ok(SheetSeed {
            id: SheetId::from_bytes([30; 16])?,
            name: "Sheet1".into(),
            rows: (10..15)
                .map(|id| RowId::from_bytes([id; 16]))
                .collect::<Result<_, _>>()?,
            columns: (20..23)
                .map(|id| ColumnId::from_bytes([id; 16]))
                .collect::<Result<_, _>>()?,
        })
    }
    fn fixture() -> Result<(Workbook, AxisHistory, CellAddress), DomainError> {
        let seed = seed()?;
        let address = CellAddress {
            sheet: seed.id,
            row: seed.rows[4],
            column: seed.columns[2],
        };
        let book = Workbook::empty(DocumentId::from_bytes([31; 16])?).candidate(
            &Operation::CreateWorkbook { sheet: seed },
            DocumentVersion::new(1)?,
        )?;
        let history = AxisHistory::new(&book);
        Ok((book, history, address))
    }
    fn edit(book: &mut Workbook, history: &mut AxisHistory, operation: Operation) -> ResultTest {
        let candidate =
            book.candidate(&operation, DocumentVersion::new(book.version().get() + 1)?)?;
        let prepared = history.prepare(&candidate, &operation);
        history.publish(&candidate, &operation, prepared);
        *book = candidate;
        Ok(())
    }
    #[test]
    fn delayed_formula_uses_original_targets_through_insert_and_delete() -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        edit(
            &mut book,
            &mut history,
            Operation::InsertRowsRebased {
                sheet: origin.sheet,
                before: seed()?.rows[0],
                rows: vec![RowId::from_bytes([40; 16])?],
            },
        )?;
        let bound = history.bind("=A2", &book, origin, DocumentVersion::new(1)?)?;
        assert_eq!(
            spreadsheet_formula::bound::render_current(&bound, &book)?,
            "=A3"
        );
        edit(
            &mut book,
            &mut history,
            Operation::DeleteRows {
                sheet: origin.sheet,
                rows: vec![seed()?.rows[0]],
            },
        )?;
        let bound = history.bind("=SUM(A1:A3)", &book, origin, DocumentVersion::new(1)?)?;
        assert_eq!(
            spreadsheet_formula::bound::render_current(&bound, &book)?,
            "=SUM(A2:A3)"
        );
        let invalid = history.bind("=A1", &book, origin, DocumentVersion::new(1)?)?;
        assert_eq!(
            spreadsheet_formula::bound::render_current(&invalid, &book)?,
            "=#REF!"
        );
        Ok(())
    }
    #[test]
    fn column_modes_and_deleted_origins_remain_typed() -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        edit(
            &mut book,
            &mut history,
            Operation::InsertColumnsRebased {
                sheet: origin.sheet,
                before: seed()?.columns[0],
                columns: vec![ColumnId::from_bytes([41; 16])?],
            },
        )?;
        let bound = history.bind("=$A2+B$2", &book, origin, DocumentVersion::new(1)?)?;
        assert_eq!(
            spreadsheet_formula::bound::render_current(&bound, &book)?,
            "=($B2+C$2)"
        );
        edit(
            &mut book,
            &mut history,
            Operation::DeleteColumns {
                sheet: origin.sheet,
                columns: vec![seed()?.columns[0]],
            },
        )?;
        let bound = history.bind("=SUM(A1:B1)", &book, origin, DocumentVersion::new(1)?)?;
        assert_eq!(
            spreadsheet_formula::bound::render_current(&bound, &book)?,
            "=SUM(B1:B1)"
        );
        edit(
            &mut book,
            &mut history,
            Operation::DeleteRows {
                sheet: origin.sheet,
                rows: vec![origin.row],
            },
        )?;
        assert!(matches!(
            history.bind("=A1", &book, origin, DocumentVersion::new(1)?),
            Err(SessionError::Domain(DomainError::MissingRow))
        ));
        Ok(())
    }
    #[test]
    fn restart_and_window_eviction_do_not_bind_against_the_wrong_axes() -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        edit(
            &mut book,
            &mut history,
            Operation::InsertRowsRebased {
                sheet: origin.sheet,
                before: seed()?.rows[0],
                rows: vec![RowId::from_bytes([40; 16])?],
            },
        )?;
        let restored = AxisHistory::new(&book);
        assert!(matches!(
            restored.bind("=A1", &book, origin, DocumentVersion::new(1)?),
            Err(SessionError::RebaseWindowExceeded)
        ));
        assert!(charge(&mut 1, 2).is_err());
        Ok(())
    }
    #[test]
    fn real_checkpoint_pressure_evicts_old_axes_and_keeps_the_memory_account_bounded() -> ResultTest
    {
        let (mut book, mut history, origin) = fixture()?;
        for index in 0..1200u128 {
            edit(
                &mut book,
                &mut history,
                Operation::InsertRowsRebased {
                    sheet: origin.sheet,
                    before: seed()?.rows[0],
                    rows: vec![RowId::from_bytes((index + 1000).to_be_bytes())?],
                },
            )?;
            assert!(history.bytes <= MAX_BYTES);
            assert_eq!(
                history.bytes,
                history.checkpoints.iter().map(|entry| entry.bytes).sum()
            );
        }
        assert!(matches!(
            history.bind("=A1", &book, origin, DocumentVersion::new(1)?),
            Err(SessionError::RebaseWindowExceeded)
        ));
        Ok(())
    }
    #[test]
    #[ignore = "release timing harness; excludes PostgreSQL and calculation"]
    fn historical_formula_binding_benchmark() -> ResultTest {
        let mut seed = seed()?;
        seed.rows = (100..1100u128)
            .map(|id| RowId::from_bytes(id.to_be_bytes()))
            .collect::<Result<_, _>>()?;
        let origin = CellAddress {
            sheet: seed.id,
            row: seed.rows[900],
            column: seed.columns[2],
        };
        let anchor = seed.rows[0];
        let mut book = Workbook::empty(DocumentId::from_bytes([31; 16])?).candidate(
            &Operation::CreateWorkbook { sheet: seed },
            DocumentVersion::new(1)?,
        )?;
        let mut history = AxisHistory::new(&book);
        for index in 0..100u128 {
            edit(
                &mut book,
                &mut history,
                Operation::InsertRowsRebased {
                    sheet: origin.sheet,
                    before: anchor,
                    rows: vec![RowId::from_bytes((index + 5000).to_be_bytes())?],
                },
            )?;
        }
        let mut samples = Vec::new();
        for _ in 0..105 {
            let start = std::time::Instant::now();
            let bound = history.bind("=SUM(A1:A3)", &book, origin, DocumentVersion::new(1)?)?;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(
                spreadsheet_formula::bound::render_current(&bound, &book)?,
                "=SUM(A101:A103)"
            );
            samples.push(elapsed);
        }
        samples.drain(..5);
        samples.sort_by(f64::total_cmp);
        println!(
            "axis_rebase_samples=100 checkpoints={} accounted_bytes={} p50_ms={:.3} p95_ms={:.3} max_ms={:.3}",
            history.checkpoints.len(),
            history.bytes,
            samples[49],
            samples[94],
            samples[99]
        );
        Ok(())
    }
}
