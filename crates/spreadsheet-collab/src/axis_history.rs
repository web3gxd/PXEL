//! Bounded, content-free axis checkpoints for binding delayed formula commands.
use crate::SessionError;
use spreadsheet_domain::{formula::Formula, workbook::*};
use std::collections::{BTreeSet, VecDeque};

pub(crate) const MAX_VERSIONS: u64 = 10_000;
const MAX_BYTES: usize = 16 * 1024 * 1024;

// No cell values, formula sources, or frozen preimages enter checkpoints.
enum AxisTransition {
    Passive,
    Change { sheet: SheetId, delta: AxisDelta },
}
impl AxisTransition {
    fn from_operation(operation: &Operation) -> Option<Self> {
        let (sheet, delta) = match operation {
            Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => {
                (change.sheet, change.delta.clone())
            }
            Operation::InsertRows {
                sheet,
                before,
                rows,
            }
            | Operation::InsertRowsRebased {
                sheet,
                before,
                rows,
            } => (
                *sheet,
                AxisDelta::Rows {
                    removed: vec![],
                    inserted: rows
                        .iter()
                        .map(|row| RowPlacement {
                            row: *row,
                            before: Some(*before),
                        })
                        .collect(),
                },
            ),
            Operation::DeleteRows { sheet, rows } => (
                *sheet,
                AxisDelta::Rows {
                    removed: rows.clone(),
                    inserted: vec![],
                },
            ),
            Operation::InsertColumnsRebased {
                sheet,
                before,
                columns,
            } => (
                *sheet,
                AxisDelta::Columns {
                    removed: vec![],
                    inserted: columns
                        .iter()
                        .map(|column| ColumnPlacement {
                            column: *column,
                            before: Some(*before),
                        })
                        .collect(),
                },
            ),
            Operation::DeleteColumns { sheet, columns } => (
                *sheet,
                AxisDelta::Columns {
                    removed: columns.clone(),
                    inserted: vec![],
                },
            ),
            Operation::CreateWorkbook { .. }
            | Operation::AddSheet { .. }
            | Operation::CopySheet { .. }
            | Operation::SheetSettingsWrite { .. }
            | Operation::ExtendRows { .. } => return Some(Self::Passive),
            _ => return None,
        };
        Some(Self::Change { sheet, delta })
    }
}

pub(crate) struct Checkpoint {
    version: DocumentVersion,
    seeds: Vec<SheetSeed>,
    transition: Option<AxisTransition>,
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
    fn checkpoint(book: &Workbook, transition: Option<AxisTransition>) -> Option<Checkpoint> {
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
        if let Some(transition) = &transition {
            bytes = bytes.checked_add(match transition {
                AxisTransition::Passive => 0,
                AxisTransition::Change {
                    delta: AxisDelta::Rows { removed, inserted },
                    ..
                } => removed
                    .len()
                    .checked_mul(32)?
                    .checked_add(inserted.len().checked_mul(64)?)?,
                AxisTransition::Change {
                    delta: AxisDelta::Columns { removed, inserted },
                    ..
                } => removed
                    .len()
                    .checked_mul(32)?
                    .checked_add(inserted.len().checked_mul(64)?)?,
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
                | Operation::CopySheet { .. }
                | Operation::SheetSettingsWrite { .. }
                | Operation::ExtendRows { .. }
                | Operation::InsertRows { .. }
                | Operation::InsertRowsRebased { .. }
                | Operation::DeleteRows { .. }
                | Operation::InsertColumnsRebased { .. }
                | Operation::DeleteColumns { .. }
                | Operation::AxisEdit { .. }
                | Operation::AxisCompensation { .. }
        )
    }
    /// Prepare allocations before COMMIT; publication only moves prepared buffers.
    pub(crate) fn prepare(&mut self, book: &Workbook, operation: &Operation) -> Option<Checkpoint> {
        if self.checkpoints.try_reserve(1).is_err() {
            return None;
        }
        Self::checkpoint(book, Some(AxisTransition::from_operation(operation)?))
    }
    pub(crate) fn publish(
        &mut self,
        book: &Workbook,
        operation: &Operation,
        checkpoint: Option<Checkpoint>,
    ) {
        if matches!(
            operation,
            Operation::MoveRange { .. }
                | Operation::MoveCompensation { .. }
                | Operation::CellBatchWrite {
                    intent: CellBatchIntent::MoveRange { .. } | CellBatchIntent::ShiftCells { .. },
                    ..
                }
        ) {
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
        if head.address_admission_floor(address)? > base {
            return Err(SessionError::SourceVersionChanged);
        }
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
            check_activation(head, head, &bound, base)?;
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
        let activation_count = head
            .row_activation_floors()
            .count()
            .saturating_add(head.column_activation_floors().count());
        charge(&mut work, activation_count)?;
        charge(&mut work, axis_count(entry).saturating_mul(2))?;
        let mut axes = Workbook::hydrate_sheets(head.document_id(), base, entry.seeds.clone(), [])?;
        let mut formula = spreadsheet_formula::binder::bind(source, &axes, address)
            .map_err(|_| SessionError::InvalidFormula)?;
        check_activation(head, &axes, &formula, base)?;
        for entry in self.checkpoints.iter().skip(index + 1) {
            let ranges = formula
                .ast()
                .nodes()
                .iter()
                .filter(|node| matches!(node, spreadsheet_domain::formula::Node::Range(_)))
                .count();
            // A restore can add a retired ID inside a range, and a later delete
            // can erase it again. Check every intermediate axis image before
            // the next lossy transform, not just initial and final endpoints.
            charge(&mut work, ranges.saturating_mul(activation_count))?;
            check_activation(head, &axes, &formula, base)?;
            let deletion_axis = match entry.transition.as_ref() {
                Some(AxisTransition::Change {
                    sheet,
                    delta: AxisDelta::Rows { removed, .. },
                }) if !removed.is_empty() => axes
                    .sheets()
                    .iter()
                    .find(|current| current.id() == *sheet)
                    .map_or(0, |sheet| sheet.rows().len()),
                Some(AxisTransition::Change {
                    sheet,
                    delta: AxisDelta::Columns { removed, .. },
                }) if !removed.is_empty() => axes
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
                AxisTransition::Change {
                    delta: AxisDelta::Rows { removed, .. },
                    ..
                } if removed.is_empty() => formula.rebase_row_offsets(entry.version, row, rows),
                AxisTransition::Change {
                    delta: AxisDelta::Columns { removed, .. },
                    ..
                } if removed.is_empty() => {
                    formula.rebase_column_offsets(entry.version, column, columns)
                }
                AxisTransition::Change {
                    sheet,
                    delta:
                        AxisDelta::Rows {
                            removed: deleted, ..
                        },
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
                AxisTransition::Change {
                    sheet,
                    delta:
                        AxisDelta::Columns {
                            removed: deleted, ..
                        },
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
                AxisTransition::Passive => Ok(formula),
            }
            .map_err(|_| SessionError::InvalidFormula)?;
            axes = next;
        }
        check_activation(head, head, &formula, base)?;
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
fn check_activation(
    head: &Workbook,
    axes: &Workbook,
    formula: &Formula,
    base: DocumentVersion,
) -> Result<(), SessionError> {
    spreadsheet_domain::lifecycle::check_formula_activation(head, axes, formula, base).map_err(
        |error| match error {
            DomainError::VersionConflict => SessionError::SourceVersionChanged,
            other => SessionError::Domain(other),
        },
    )
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
    fn normalized_move_advances_formula_binding_relocation_floor() -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        let next = book.candidate(
            &Operation::SetCellValue {
                address: origin,
                value: CellValue::Number(1.0),
            },
            DocumentVersion::new(2)?,
        )?;
        let operation = Operation::CellBatchWrite {
            intent: CellBatchIntent::MoveRange {
                source: origin,
                destination: CellAddress {
                    column: seed()?.columns[1],
                    ..origin
                },
                rows: 1,
                columns: 1,
            },
            content_writes: Vec::new(),
            style_writes: Vec::new(),
            undoes: None,
            undoes_version: None,
        };
        history.publish(&next, &operation, None);
        book = next;

        assert!(matches!(
            history.bind("=A1", &book, origin, DocumentVersion::new(1)?),
            Err(SessionError::SourceVersionChanged)
        ));
        Ok(())
    }
    fn axis_edit(
        book: &mut Workbook,
        history: &mut AxisHistory,
        legacy: Operation,
    ) -> Result<CommittedEvent, Box<dyn std::error::Error>> {
        let version = DocumentVersion::new(book.version().get() + 1)?;
        let operation = spreadsheet_domain::axis_edit::prepare_axis_edit(book, &legacy, version)?;
        edit(book, history, operation.clone())?;
        Ok(CommittedEvent {
            document_id: book.document_id(),
            operation_id: OperationId::from_bytes([version.get() as u8 + 80; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([79; 16])?),
            version,
            operation,
            state_hash: book.hash(),
        })
    }
    #[test]
    fn frozen_transitions_bind_delayed_targets_and_never_retain_cell_payloads() -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        axis_edit(
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
        let value_address = CellAddress {
            sheet: origin.sheet,
            row: seed()?.rows[0],
            column: seed()?.columns[0],
        };
        edit(
            &mut book,
            &mut history,
            Operation::SetCellValue {
                address: value_address,
                value: CellValue::Text("x".repeat(32768)),
            },
        )?;
        let target = axis_edit(
            &mut book,
            &mut history,
            Operation::DeleteRows {
                sheet: origin.sheet,
                rows: vec![seed()?.rows[0]],
            },
        )?;
        let bound = history.bind("=A1", &book, origin, DocumentVersion::new(1)?)?;
        assert_eq!(
            spreadsheet_formula::bound::render_current(&bound, &book)?,
            "=#REF!"
        );
        let Some(checkpoint) = history.checkpoints.back() else {
            return Err("missing checkpoint".into());
        };
        assert!(checkpoint.bytes < 32768);
        assert!(matches!(
            checkpoint.transition,
            Some(AxisTransition::Change {
                delta: AxisDelta::Rows { .. },
                ..
            })
        ));
        let mut altered = target.operation.clone();
        if let Operation::AxisEdit { change } = &mut altered {
            change.writes.clear();
        }
        let Some(empty_payload) = history.prepare(&book, &altered) else {
            return Err("missing prepared".into());
        };
        assert_eq!(
            empty_payload.bytes,
            history
                .checkpoints
                .back()
                .ok_or("missing checkpoint")?
                .bytes
        );
        Ok(())
    }
    #[test]
    fn revived_origins_targets_and_range_interiors_reject_old_base_after_reconstruction()
    -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        let target = axis_edit(
            &mut book,
            &mut history,
            Operation::DeleteRows {
                sheet: origin.sheet,
                rows: vec![seed()?.rows[1]],
            },
        )?;
        let inverse = spreadsheet_domain::axis_edit::prepare_axis_inverse(
            &book,
            &target,
            DocumentVersion::new(3)?,
        )?;
        edit(&mut book, &mut history, inverse.clone())?;
        let assert_rejections = |head: &Workbook, history: &AxisHistory| -> ResultTest {
            let revived = CellAddress {
                sheet: origin.sheet,
                row: seed()?.rows[1],
                column: origin.column,
            };
            for (source, at) in [("=A2", origin), ("=SUM(A1:A3)", origin), ("=A1", revived)] {
                assert!(matches!(
                    history.bind(source, head, at, DocumentVersion::new(1)?),
                    Err(SessionError::SourceVersionChanged)
                ));
            }
            let unchanged = history.bind("=A1", head, origin, DocumentVersion::new(1)?)?;
            assert_eq!(
                spreadsheet_formula::bound::render_current(&unchanged, head)?,
                "=A1"
            );
            // Bound while deleted: the old A1:A2 now spans restored interior A2.
            assert!(matches!(
                history.bind("=SUM(A1:A2)", head, origin, DocumentVersion::new(2)?),
                Err(SessionError::SourceVersionChanged)
            ));
            Ok(())
        };
        assert_rejections(&book, &history)?;
        let (mut restored, mut fresh, _) = fixture()?;
        edit(&mut restored, &mut fresh, target.operation)?;
        edit(&mut restored, &mut fresh, inverse)?;
        assert_eq!(restored.hash(), book.hash());
        assert_rejections(&restored, &fresh)?;
        axis_edit(
            &mut book,
            &mut history,
            Operation::DeleteRows {
                sheet: origin.sheet,
                rows: vec![seed()?.rows[1]],
            },
        )?;
        assert!(matches!(
            history.bind("=SUM(A1:A2)", &book, origin, DocumentVersion::new(2)?),
            Err(SessionError::SourceVersionChanged)
        ));
        Ok(())
    }
    #[test]
    fn column_reactivation_is_checked_before_deleted_reference_identity_is_lost() -> ResultTest {
        let (mut book, mut history, origin) = fixture()?;
        let target = axis_edit(
            &mut book,
            &mut history,
            Operation::DeleteColumns {
                sheet: origin.sheet,
                columns: vec![seed()?.columns[1]],
            },
        )?;
        let inverse = spreadsheet_domain::axis_edit::prepare_axis_inverse(
            &book,
            &target,
            DocumentVersion::new(3)?,
        )?;
        edit(&mut book, &mut history, inverse)?;
        assert!(matches!(
            history.bind("=$B$1", &book, origin, DocumentVersion::new(1)?),
            Err(SessionError::SourceVersionChanged)
        ));
        assert!(matches!(
            history.bind("=SUM(A1:C1)", &book, origin, DocumentVersion::new(1)?),
            Err(SessionError::SourceVersionChanged)
        ));
        assert!(
            history
                .bind("=$A$1", &book, origin, DocumentVersion::new(1)?)
                .is_ok()
        );
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
