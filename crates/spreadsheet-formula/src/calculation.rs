//! Incremental, deterministic calculation projection. Cached results are derived state.
use crate::{
    EvaluationBudget, EvaluationError, FormulaError, Value,
    bound::{BoundDependencies, bound_dependencies, evaluate_bound_with_overlay},
    dependency::{DependencyGraph, DependencyLimitExceeded},
    range_index::RangeIndex,
};
use sha2::{Digest, Sha256};
use spreadsheet_domain::workbook::{
    CalculationContext, CellAddress, CellContent, DocumentId, DocumentVersion, Operation, SheetId,
    Workbook,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::atomic::{AtomicBool, Ordering},
};
use thiserror::Error;

pub const ENGINE_VERSION: &str = "pxel-formula-0.14.0";
pub const COMPATIBILITY_PROFILE: &str = "pxel-excel-subset-v1";

#[derive(Clone, Copy, Debug)]
pub struct CalculationLimits {
    pub graph_work: usize,
    pub evaluation_work: u64,
    pub max_result_bytes: usize,
}
impl Default for CalculationLimits {
    fn default() -> Self {
        Self {
            graph_work: 1_000_000,
            evaluation_work: 1_000_000,
            max_result_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CalculationError {
    #[error("calculation projection version mismatch")]
    VersionMismatch,
    #[error("calculation projection invariant failed")]
    InvalidProjection,
    #[error("calculation dependency work limit exceeded")]
    DependencyLimit,
    #[error(transparent)]
    Evaluation(#[from] EvaluationError),
}
impl From<DependencyLimitExceeded> for CalculationError {
    fn from(_: DependencyLimitExceeded) -> Self {
        Self::DependencyLimit
    }
}
fn charge(work: &mut usize, units: usize) -> Result<(), CalculationError> {
    *work = work
        .checked_sub(units)
        .ok_or(CalculationError::DependencyLimit)?;
    Ok(())
}
fn position(book: &Workbook, cell: CellAddress) -> Result<(SheetId, u32, u32), CalculationError> {
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == cell.sheet)
        .ok_or(CalculationError::InvalidProjection)?;
    Ok((
        cell.sheet,
        sheet
            .row_position(cell.row)
            .ok_or(CalculationError::InvalidProjection)?,
        sheet
            .column_position(cell.column)
            .ok_or(CalculationError::InvalidProjection)?,
    ))
}
fn content(book: &Workbook, cell: CellAddress) -> Option<&CellContent> {
    book.sheets()
        .iter()
        .find(|s| s.id() == cell.sheet)?
        .cells()
        .get(&cell)
}

#[derive(Clone, Default)]
struct Structure {
    graph: DependencyGraph,
    dependencies: BTreeMap<CellAddress, BoundDependencies>,
    formula_positions: BTreeMap<(SheetId, u32, u32), CellAddress>,
    ranges: RangeIndex,
    edges: usize,
}
impl Structure {
    fn rebuild_ranges(&mut self, work: &mut usize) -> Result<(), CalculationError> {
        let mut ranges = Vec::new();
        for deps in self.dependencies.values() {
            charge(work, 1 + deps.ranges.len())?;
            ranges.extend(deps.ranges.iter().copied());
        }
        self.ranges = RangeIndex::build(ranges)?;
        Ok(())
    }
    fn replace_edges(
        &mut self,
        cell: CellAddress,
        work: &mut usize,
    ) -> Result<(), CalculationError> {
        let deps = self
            .dependencies
            .get(&cell)
            .ok_or(CalculationError::InvalidProjection)?;
        charge(work, deps.direct.len())?;
        let mut references = deps.direct.clone();
        for range in &deps.ranges {
            for ((_, _, column), address) in self
                .formula_positions
                .range((range.sheet, range.first_row, 0)..=(range.sheet, range.last_row, u32::MAX))
            {
                charge(work, 1)?;
                if *column >= range.first_column && *column <= range.last_column {
                    references.insert(*address);
                }
            }
        }
        let previous = self.graph.dependencies(cell).map_or(0, BTreeSet::len);
        self.edges = self
            .edges
            .checked_sub(previous)
            .and_then(|n| n.checked_add(references.len()))
            .filter(|n| *n <= 1_000_000)
            .ok_or(CalculationError::DependencyLimit)?;
        self.graph.replace(cell, references);
        Ok(())
    }
    fn dirty(
        &self,
        book: &Workbook,
        seeds: impl IntoIterator<Item = CellAddress>,
        work: &mut usize,
    ) -> Result<BTreeSet<CellAddress>, CalculationError> {
        let mut dirty = BTreeSet::new();
        let mut queue = VecDeque::new();
        for cell in seeds {
            charge(work, 1)?;
            if dirty.insert(cell) {
                queue.push_back(cell);
            }
        }
        while let Some(cell) = queue.pop_front() {
            if let Some(users) = self.graph.dependents(&cell) {
                for user in users {
                    charge(work, 1)?;
                    if dirty.insert(*user) {
                        queue.push_back(*user);
                    }
                }
            }
            let (sheet, row, column) = position(book, cell)?;
            for user in self.ranges.affected(sheet, row, column, work)? {
                if dirty.insert(user) {
                    queue.push_back(user);
                }
            }
        }
        Ok(dirty)
    }
}

/// Prepared against a validated candidate workbook. Applying this delta is infallible.
pub struct CalculationDelta {
    version: DocumentVersion,
    context: Option<CalculationContext>,
    structure: Option<Structure>,
    dirty: BTreeSet<CellAddress>,
    cleared: BTreeSet<CellAddress>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalculationPatch {
    pub input_version: DocumentVersion,
    pub changed_results: BTreeMap<CellAddress, Value>,
    pub cleared_results: BTreeSet<CellAddress>,
    pub evaluated_cells: usize,
    pub dirty_cells: usize,
    pub graph_work_used: usize,
    pub evaluation_work_used: u64,
}

#[derive(Clone)]
pub struct CalculationProjection {
    document_id: DocumentId,
    input_version: DocumentVersion,
    context: Option<CalculationContext>,
    calculated_version: Option<DocumentVersion>,
    structure: Structure,
    results: BTreeMap<CellAddress, Value>,
    result_sizes: BTreeMap<CellAddress, usize>,
    result_bytes: usize,
    dirty: BTreeSet<CellAddress>,
    cleared: BTreeSet<CellAddress>,
}
impl CalculationProjection {
    pub fn from_workbook(
        book: &Workbook,
        limits: CalculationLimits,
    ) -> Result<Self, CalculationError> {
        if book.calculation_context().is_some_and(|context| {
            context.engine_version() != ENGINE_VERSION
                || context.compatibility_profile() != COMPATIBILITY_PROFILE
        }) {
            return Err(CalculationError::InvalidProjection);
        }
        let mut structure = Structure::default();
        let mut work = limits.graph_work.min(1_000_000);
        for sheet in book.sheets() {
            for (address, value) in sheet.cells() {
                charge(&mut work, 1)?;
                if let CellContent::Formula(formula) = value {
                    charge(&mut work, formula.ast().nodes().len())?;
                    structure
                        .dependencies
                        .insert(*address, bound_dependencies(formula, book)?);
                    structure
                        .formula_positions
                        .insert(position(book, *address)?, *address);
                }
            }
        }
        structure.rebuild_ranges(&mut work)?;
        let cells: Vec<_> = structure.dependencies.keys().copied().collect();
        for cell in &cells {
            structure.replace_edges(*cell, &mut work)?;
        }
        Ok(Self {
            document_id: book.document_id(),
            input_version: book.version(),
            context: book.calculation_context().cloned(),
            calculated_version: None,
            structure,
            results: BTreeMap::new(),
            result_sizes: BTreeMap::new(),
            result_bytes: 0,
            dirty: cells.into_iter().collect(),
            cleared: BTreeSet::new(),
        })
    }
    pub fn input_version(&self) -> DocumentVersion {
        self.input_version
    }
    pub fn calculated_version(&self) -> Option<DocumentVersion> {
        self.calculated_version
    }
    pub fn results(&self) -> &BTreeMap<CellAddress, Value> {
        &self.results
    }
    /// Hash only a fully calculated input version, never a partially updated cache.
    pub fn result_hash(&self) -> Result<[u8; 32], CalculationError> {
        if self.calculated_version != Some(self.input_version) || !self.dirty.is_empty() {
            return Err(CalculationError::VersionMismatch);
        }
        let mut hash = Sha256::new();
        hash.update(b"PXEL-CALCULATED\0\x01");
        hash.update(self.document_id.as_bytes());
        hash.update(self.input_version.get().to_le_bytes());
        hash_text(&mut hash, ENGINE_VERSION);
        hash_text(&mut hash, COMPATIBILITY_PROFILE);
        hash.update((self.results.len() as u64).to_le_bytes());
        for (address, value) in &self.results {
            hash.update(address.sheet.as_bytes());
            hash.update(address.row.as_bytes());
            hash.update(address.column.as_bytes());
            hash_value(&mut hash, value);
        }
        if let Some(context) = &self.context {
            hash.update(b"PXEL-CALCULATED-CONTEXT-1\0");
            hash.update(context.context_id().as_bytes());
            hash.update(context.epoch().to_le_bytes());
        }
        Ok(hash.finalize().into())
    }
    pub fn result_bytes(&self) -> usize {
        self.result_bytes
    }
    pub fn dirty_count(&self) -> usize {
        self.dirty.len()
    }

    pub fn prepare(
        &self,
        book: &Workbook,
        operation: &Operation,
        limits: CalculationLimits,
    ) -> Result<CalculationDelta, CalculationError> {
        if book.version().get()
            != self
                .input_version
                .get()
                .checked_add(1)
                .ok_or(CalculationError::VersionMismatch)?
        {
            return Err(CalculationError::VersionMismatch);
        }
        let address = match operation {
            Operation::SetCellValue { address, .. }
            | Operation::ClearCell { address }
            | Operation::SetCellFormula { address, .. }
            | Operation::CellWrite { address, .. } => *address,
            Operation::SetCellValues { writes } => {
                let changed = writes.iter().map(|write| write.address).collect();
                return self.prepare_reconcile(book, &changed, limits);
            }
            Operation::RangeWrite { writes, .. }
            | Operation::MoveRange { writes, .. }
            | Operation::MoveCompensation { writes, .. } => {
                let changed = writes.iter().map(|write| write.address).collect();
                return self.prepare_reconcile(book, &changed, limits);
            }
            Operation::CopyRange { writes } => {
                let changed = writes.iter().map(|write| write.address).collect();
                return self.prepare_reconcile(book, &changed, limits);
            }
            Operation::CreateWorkbook { .. } | Operation::AddSheet { .. } => {
                return Err(CalculationError::InvalidProjection);
            }
            // Axis changes replace the graph; the caller must rebuild from the new workbook.
            Operation::ExtendRows { .. } => return Err(CalculationError::InvalidProjection),
            Operation::InsertRows { .. }
            | Operation::InsertRowsRebased { .. }
            | Operation::DeleteRows { .. }
            | Operation::InsertColumnsRebased { .. } => {
                return Err(CalculationError::InvalidProjection);
            }
            Operation::DeleteColumns { .. }
            | Operation::AxisEdit { .. }
            | Operation::AxisCompensation { .. } => {
                return Err(CalculationError::InvalidProjection);
            }
            Operation::AdvanceCalculationEpoch { .. }
            | Operation::StyleWrite { .. }
            | Operation::StyleCompensation { .. } => {
                return self.prepare_reconcile(book, &BTreeSet::new(), limits);
            }
        };
        self.prepare_reconcile(book, &BTreeSet::from([address]), limits)
    }
    /// The caller supplies the exact union of changed cells and guarantees unchanged axes.
    /// Skipped intermediate formula edits are replaced by their latest authoritative content.
    pub fn prepare_reconcile(
        &self,
        book: &Workbook,
        changed: &BTreeSet<CellAddress>,
        limits: CalculationLimits,
    ) -> Result<CalculationDelta, CalculationError> {
        if book.document_id() != self.document_id || book.version() < self.input_version {
            return Err(CalculationError::VersionMismatch);
        }
        if book.calculation_context().is_some_and(|context| {
            context.engine_version() != ENGINE_VERSION
                || context.compatibility_profile() != COMPATIBILITY_PROFILE
        }) {
            return Err(CalculationError::InvalidProjection);
        }
        if changed.len() > 100_000 {
            return Err(CalculationError::DependencyLimit);
        }
        let mut work = limits.graph_work.min(1_000_000);
        let mut geometry = false;
        let mut cleared = BTreeSet::new();
        for address in changed {
            charge(&mut work, 1)?;
            position(book, *address)?;
            let old = self.structure.dependencies.contains_key(address);
            let new = matches!(content(book, *address), Some(CellContent::Formula(_)));
            geometry |= old || new;
            if old && !new {
                cleared.insert(*address);
            }
        }
        let mut replacement = None;
        if geometry {
            charge(
                &mut work,
                self.structure.edges + self.structure.dependencies.len(),
            )?;
            let mut next = self.structure.clone();
            for address in changed {
                let p = position(book, *address)?;
                let previous = next.graph.dependencies(*address).map_or(0, BTreeSet::len);
                next.edges = next
                    .edges
                    .checked_sub(previous)
                    .ok_or(CalculationError::InvalidProjection)?;
                next.graph.remove(*address);
                next.dependencies.remove(address);
                next.formula_positions.remove(&p);
                if let Some(CellContent::Formula(formula)) = content(book, *address) {
                    charge(&mut work, formula.ast().nodes().len())?;
                    next.dependencies
                        .insert(*address, bound_dependencies(formula, book)?);
                    next.formula_positions.insert(p, *address);
                }
            }
            next.rebuild_ranges(&mut work)?;
            let mut affected = BTreeSet::new();
            for address in changed {
                let p = position(book, *address)?;
                affected.extend(next.ranges.affected(p.0, p.1, p.2, &mut work)?);
                if next.dependencies.contains_key(address) {
                    affected.insert(*address);
                }
            }
            for formula in affected {
                next.replace_edges(formula, &mut work)?;
            }
            replacement = Some(next);
        }
        let structure = replacement.as_ref().unwrap_or(&self.structure);
        let mut dirty = structure.dirty(book, changed.iter().copied(), &mut work)?;
        // A persisted epoch changes evaluation inputs even without cell edits.
        // Reconciliation may coalesce the system event with subsequent edits.
        if self.context.as_ref() != book.calculation_context() {
            charge(&mut work, structure.dependencies.len())?;
            dirty.extend(structure.dependencies.keys().copied());
        }
        Ok(CalculationDelta {
            version: book.version(),
            context: book.calculation_context().cloned(),
            structure: replacement,
            dirty,
            cleared,
        })
    }
    pub fn reconcile(
        &mut self,
        book: &Workbook,
        changed: &BTreeSet<CellAddress>,
        limits: CalculationLimits,
    ) -> Result<(), CalculationError> {
        let delta = self.prepare_reconcile(book, changed, limits)?;
        self.apply(delta);
        Ok(())
    }
    pub fn apply(&mut self, delta: CalculationDelta) {
        self.input_version = delta.version;
        self.context = delta.context;
        if let Some(structure) = delta.structure {
            self.structure = structure;
        }
        self.dirty.extend(delta.dirty);
        for cell in delta.cleared {
            self.cleared.insert(cell);
        }
    }
    pub fn calculate(
        &mut self,
        book: &Workbook,
        limits: CalculationLimits,
        cancelled: &AtomicBool,
    ) -> Result<CalculationPatch, CalculationError> {
        if book.document_id() != self.document_id || book.version() != self.input_version {
            return Err(CalculationError::VersionMismatch);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(EvaluationError::Cancelled.into());
        }
        let initial_graph = limits.graph_work.min(1_000_000);
        let mut graph_work = initial_graph;
        let order = self
            .structure
            .graph
            .schedule(&self.dirty, &mut graph_work)?;
        let initial_evaluation = limits.evaluation_work.min(1_000_000);
        let mut budget = EvaluationBudget::new(initial_evaluation);
        let mut overlay = BTreeMap::new();
        let mut evaluated = 0;
        let mut result_bytes = self.result_bytes;
        let mut sizes = BTreeMap::new();
        let cleared: BTreeSet<_> = self
            .cleared
            .iter()
            .filter(|cell| !self.structure.dependencies.contains_key(cell))
            .copied()
            .collect();
        // Primitive replacements shadow the old generation's formula cache only
        // inside this calculation. Failed work leaves that old cache intact.
        for cell in &cleared {
            let value = match content(book, *cell) {
                Some(CellContent::Value(spreadsheet_domain::workbook::CellValue::Number(n))) => {
                    Value::Number(*n)
                }
                Some(CellContent::Value(spreadsheet_domain::workbook::CellValue::Text(text))) => {
                    Value::Text(text.clone())
                }
                Some(CellContent::Value(spreadsheet_domain::workbook::CellValue::Boolean(b))) => {
                    Value::Boolean(*b)
                }
                _ => Value::Blank,
            };
            result_bytes = result_bytes
                .checked_sub(self.result_sizes.get(cell).copied().unwrap_or(0))
                .ok_or(CalculationError::InvalidProjection)?;
            overlay.insert(*cell, value);
        }
        for cell in order.ready {
            if cancelled.load(Ordering::Relaxed) {
                return Err(EvaluationError::Cancelled.into());
            }
            let Some(CellContent::Formula(formula)) = content(book, cell) else {
                return Err(CalculationError::InvalidProjection);
            };
            let value =
                evaluate_bound_with_overlay(formula, book, &self.results, &overlay, &mut budget)?;
            let size = result_size(&value)?;
            result_bytes = result_bytes
                .checked_sub(self.result_sizes.get(&cell).copied().unwrap_or(0))
                .and_then(|n| n.checked_add(size))
                .filter(|n| *n <= limits.max_result_bytes.min(32 * 1024 * 1024))
                .ok_or(EvaluationError::CalculationLimitExceeded)?;
            sizes.insert(cell, size);
            overlay.insert(cell, value);
            evaluated += 1;
        }
        for cell in order.blocked {
            let value = Value::Error(FormulaError::Cycle);
            let size = result_size(&value)?;
            result_bytes = result_bytes
                .checked_sub(self.result_sizes.get(&cell).copied().unwrap_or(0))
                .and_then(|n| n.checked_add(size))
                .filter(|n| *n <= limits.max_result_bytes.min(32 * 1024 * 1024))
                .ok_or(EvaluationError::CalculationLimitExceeded)?;
            sizes.insert(cell, size);
            overlay.insert(cell, value);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(EvaluationError::Cancelled.into());
        }
        for cell in &cleared {
            overlay.remove(cell);
        }
        let patch = CalculationPatch {
            input_version: self.input_version,
            changed_results: overlay.clone(),
            cleared_results: cleared.clone(),
            evaluated_cells: evaluated,
            dirty_cells: self.dirty.len(),
            graph_work_used: initial_graph - graph_work,
            evaluation_work_used: initial_evaluation - budget.remaining(),
        };
        for cell in cleared {
            self.results.remove(&cell);
            self.result_sizes.remove(&cell);
        }
        self.results.extend(overlay);
        self.result_sizes.extend(sizes);
        self.result_bytes = result_bytes;
        self.calculated_version = Some(self.input_version);
        self.dirty.clear();
        self.cleared.clear();
        Ok(patch)
    }
}

fn result_size(value: &Value) -> Result<usize, CalculationError> {
    let mut pending = vec![(value, 0usize)];
    // Conservative per-cell allowance for stable key, B-tree bookkeeping and
    // the companion size index, in addition to each value's owned allocation.
    let mut bytes = 128usize;
    let mut count = 0;
    while let Some((value, depth)) = pending.pop() {
        count += 1;
        if count > 100_000 || depth > 128 {
            return Err(EvaluationError::CalculationLimitExceeded.into());
        }
        bytes = bytes
            .checked_add(64)
            .ok_or(EvaluationError::CalculationLimitExceeded)?;
        match value {
            Value::Number(n) | Value::DateTime { serial: n, .. } if !n.is_finite() => {
                return Err(EvaluationError::InvalidResolverResult.into());
            }
            Value::Text(text) => {
                bytes = bytes
                    .checked_add(text.len())
                    .ok_or(EvaluationError::CalculationLimitExceeded)?
            }
            Value::Array {
                rows,
                columns,
                values,
            } => {
                if *rows == 0
                    || *columns == 0
                    || u64::from(*rows) * u64::from(*columns) != values.len() as u64
                {
                    return Err(EvaluationError::InvalidResolverResult.into());
                }
                if values.len() > 100_000
                    || pending
                        .len()
                        .saturating_add(values.len())
                        .saturating_add(count)
                        > 100_000
                {
                    return Err(EvaluationError::CalculationLimitExceeded.into());
                }
                pending.extend(values.iter().map(|v| (v, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(bytes)
}

fn hash_text(hash: &mut Sha256, text: &str) {
    hash.update((text.len() as u64).to_le_bytes());
    hash.update(text.as_bytes());
}
fn hash_number(hash: &mut Sha256, n: f64) {
    hash.update((if n == 0.0 { 0.0 } else { n }).to_bits().to_le_bytes());
}
fn hash_value(hash: &mut Sha256, value: &Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Blank => hash.update([0]),
            Value::Number(n) => {
                hash.update([1]);
                hash_number(hash, *n);
            }
            Value::Text(text) => {
                hash.update([2]);
                hash_text(hash, text);
            }
            Value::Boolean(value) => hash.update([3, u8::from(*value)]),
            Value::Error(error) => {
                hash.update([4]);
                hash_text(hash, error.code());
            }
            Value::DateTime { serial, system } => {
                hash.update([
                    5,
                    match system {
                        crate::DateSystem::Excel1900 => 0,
                        crate::DateSystem::Excel1904 => 1,
                    },
                ]);
                hash_number(hash, *serial);
            }
            Value::Array {
                rows,
                columns,
                values,
            } => {
                hash.update([6]);
                hash.update(rows.to_le_bytes());
                hash.update(columns.to_le_bytes());
                hash.update((values.len() as u64).to_le_bytes());
                pending.extend(values.iter().rev());
            }
        }
    }
}
