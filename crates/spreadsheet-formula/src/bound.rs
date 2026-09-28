//! Bridge from durable stable-ID AST to the pure evaluator. Source text is never parsed.
use crate::{
    EvaluationBudget, EvaluationError, ReferenceResolver, Value, ast as syntax, evaluate,
    range_index::RangeDependency,
};
use spreadsheet_domain::{
    formula as bound,
    workbook::{CellAddress, CellContent, CellValue, Sheet, SheetId, Workbook},
};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BoundDependencies {
    pub direct: BTreeSet<CellAddress>,
    pub ranges: Vec<RangeDependency>,
}
fn key(id: SheetId) -> String {
    id.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn sheet(workbook: &Workbook, id: SheetId) -> Option<&Sheet> {
    workbook.sheets().iter().find(|sheet| sheet.id() == id)
}
fn position(workbook: &Workbook, r: &bound::BoundCellReference) -> Option<(u32, u32)> {
    let sheet = sheet(workbook, r.target_sheet)?;
    Some((
        sheet.row_position(r.target_row)?,
        sheet.column_position(r.target_column)?,
    ))
}
fn origin_exists(formula: &bound::Formula, workbook: &Workbook) -> bool {
    let address = formula.origin();
    sheet(workbook, address.sheet).is_some_and(|sheet| {
        sheet.row_position(address.row).is_some() && sheet.column_position(address.column).is_some()
    })
}
fn project_cell(
    workbook: &Workbook,
    r: &bound::BoundCellReference,
) -> Option<syntax::CellReference> {
    let (row, column) = position(workbook, r)?;
    Some(syntax::CellReference {
        sheet: Some(key(r.target_sheet)),
        row,
        column,
        row_absolute: matches!(r.row_mode, bound::AxisReferenceMode::Absolute),
        column_absolute: matches!(r.column_mode, bound::AxisReferenceMode::Absolute),
    })
}
fn project_range(
    workbook: &Workbook,
    r: &bound::BoundRangeReference,
) -> Result<Option<syntax::RangeReference>, EvaluationError> {
    let (Some(start), Some(end)) = (
        project_cell(workbook, &r.start),
        project_cell(workbook, &r.end),
    ) else {
        return Ok(None);
    };
    let range = syntax::RangeReference { start, end };
    if range.cell_count() > syntax::MAX_RANGE_CELLS {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    Ok(Some(range))
}
pub fn bound_dependencies(
    formula: &bound::Formula,
    workbook: &Workbook,
) -> Result<BoundDependencies, EvaluationError> {
    if !origin_exists(formula, workbook) {
        return Err(EvaluationError::InvalidAst);
    }
    let mut dependencies = BoundDependencies::default();
    for node in formula.ast().nodes() {
        match node {
            bound::Node::Cell(r) => {
                if position(workbook, r).is_some() {
                    dependencies.direct.insert(CellAddress {
                        sheet: r.target_sheet,
                        row: r.target_row,
                        column: r.target_column,
                    });
                }
            }
            bound::Node::Range(r) => {
                if let Some(range) = project_range(workbook, r)? {
                    if dependencies.ranges.len() >= 256 {
                        return Err(EvaluationError::CalculationLimitExceeded);
                    }
                    dependencies.ranges.push(RangeDependency {
                        formula: formula.origin(),
                        sheet: r.start.target_sheet,
                        first_row: range.start.row.min(range.end.row),
                        last_row: range.start.row.max(range.end.row),
                        first_column: range.start.column.min(range.end.column),
                        last_column: range.start.column.max(range.end.column),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(dependencies)
}
fn error(e: bound::FormulaError) -> syntax::FormulaError {
    match e {
        bound::FormulaError::Null => syntax::FormulaError::Null,
        bound::FormulaError::Div0 => syntax::FormulaError::Div0,
        bound::FormulaError::Value => syntax::FormulaError::Value,
        bound::FormulaError::Ref => syntax::FormulaError::Ref,
        bound::FormulaError::Name => syntax::FormulaError::Name,
        bound::FormulaError::Num => syntax::FormulaError::Num,
        bound::FormulaError::Na => syntax::FormulaError::Na,
        bound::FormulaError::Spill => syntax::FormulaError::Spill,
        bound::FormulaError::Cycle => syntax::FormulaError::Cycle,
    }
}
fn unary(op: bound::UnaryOp) -> syntax::UnaryOp {
    match op {
        bound::UnaryOp::Plus => syntax::UnaryOp::Plus,
        bound::UnaryOp::Minus => syntax::UnaryOp::Minus,
        bound::UnaryOp::Percent => syntax::UnaryOp::Percent,
    }
}
fn binary(op: bound::BinaryOp) -> syntax::BinaryOp {
    match op {
        bound::BinaryOp::Add => syntax::BinaryOp::Add,
        bound::BinaryOp::Subtract => syntax::BinaryOp::Subtract,
        bound::BinaryOp::Multiply => syntax::BinaryOp::Multiply,
        bound::BinaryOp::Divide => syntax::BinaryOp::Divide,
        bound::BinaryOp::Power => syntax::BinaryOp::Power,
        bound::BinaryOp::Concat => syntax::BinaryOp::Concat,
        bound::BinaryOp::Equal => syntax::BinaryOp::Equal,
        bound::BinaryOp::NotEqual => syntax::BinaryOp::NotEqual,
        bound::BinaryOp::Less => syntax::BinaryOp::Less,
        bound::BinaryOp::LessEqual => syntax::BinaryOp::LessEqual,
        bound::BinaryOp::Greater => syntax::BinaryOp::Greater,
        bound::BinaryOp::GreaterEqual => syntax::BinaryOp::GreaterEqual,
    }
}
fn project(formula: &bound::Formula, workbook: &Workbook) -> Result<syntax::Ast, EvaluationError> {
    let mut nodes = Vec::with_capacity(formula.ast().nodes().len());
    for node in formula.ast().nodes() {
        nodes.push(match node {
            bound::Node::Number(n) => syntax::Node::Number(*n),
            bound::Node::Text(t) => syntax::Node::Text(t.clone()),
            bound::Node::Boolean(b) => syntax::Node::Boolean(*b),
            bound::Node::Error(e) => syntax::Node::Error(error(*e)),
            bound::Node::Missing => syntax::Node::Missing,
            bound::Node::Cell(r) => {
                project_cell(workbook, r).map_or(syntax::Node::InvalidReference, syntax::Node::Cell)
            }
            bound::Node::Range(r) => project_range(workbook, r)?
                .map_or(syntax::Node::InvalidReference, syntax::Node::Range),
            bound::Node::InvalidReference(_) => syntax::Node::InvalidReference,
            bound::Node::Unary { op, operand } => syntax::Node::Unary {
                op: unary(*op),
                operand: syntax::NodeId(operand.0 as usize),
            },
            bound::Node::Binary { op, left, right } => syntax::Node::Binary {
                op: binary(*op),
                left: syntax::NodeId(left.0 as usize),
                right: syntax::NodeId(right.0 as usize),
            },
            bound::Node::Call { name, arguments } => syntax::Node::Call {
                name: name.as_str().to_owned(),
                arguments: arguments
                    .iter()
                    .map(|id| syntax::NodeId(id.0 as usize))
                    .collect(),
            },
        });
    }
    Ok(syntax::Ast {
        schema_version: syntax::AST_SCHEMA_VERSION,
        root: syntax::NodeId(formula.ast().root().0 as usize),
        nodes,
    })
}
fn valid_result(value: &Value) -> bool {
    let mut pending = vec![(value, 0usize)];
    let mut count = 0usize;
    let mut text_bytes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        count += 1;
        if count > 100_000 || depth > 128 {
            return false;
        }
        match value {
            Value::Number(n) | Value::DateTime { serial: n, .. } => {
                if !n.is_finite() {
                    return false;
                }
            }
            Value::Text(text) => {
                text_bytes = text_bytes.saturating_add(text.len());
                if text.len() > bound::MAX_LITERAL_BYTES || text_bytes > bound::MAX_AST_TEXT_BYTES {
                    return false;
                }
            }
            Value::Array {
                rows,
                columns,
                values,
            } => {
                if *rows == 0
                    || *columns == 0
                    || u64::from(*rows) * u64::from(*columns) != values.len() as u64
                    || values.len() > 100_000
                    || pending
                        .len()
                        .saturating_add(values.len())
                        .saturating_add(count)
                        > 100_000
                {
                    return false;
                }
                for item in values {
                    pending.push((item, depth + 1));
                }
            }
            Value::Blank | Value::Boolean(_) | Value::Error(_) => {}
        }
    }
    true
}
struct ProjectionResolver<'a> {
    sheets: BTreeMap<String, &'a Sheet>,
    results: &'a BTreeMap<CellAddress, Value>,
    overlay: &'a BTreeMap<CellAddress, Value>,
    invalid_result: Cell<bool>,
}
impl ProjectionResolver<'_> {
    fn value(&self, sheet: &Sheet, row: u32, column: u32) -> Value {
        let (Some(row), Some(column)) = (
            sheet.rows().get(row as usize),
            sheet.columns().get(column as usize),
        ) else {
            return Value::Error(syntax::FormulaError::Ref);
        };
        let address = CellAddress {
            sheet: sheet.id(),
            row: *row,
            column: *column,
        };
        if let Some(value) = self
            .overlay
            .get(&address)
            .or_else(|| self.results.get(&address))
        {
            if !valid_result(value) {
                self.invalid_result.set(true);
                return Value::Error(syntax::FormulaError::Value);
            }
            return value.clone();
        }
        match sheet.cells().get(&address) {
            None | Some(CellContent::Value(CellValue::Blank)) => Value::Blank,
            Some(CellContent::Value(CellValue::Number(n))) => Value::Number(*n),
            Some(CellContent::Value(CellValue::Text(t))) => Value::Text(t.clone()),
            Some(CellContent::Value(CellValue::Boolean(b))) => Value::Boolean(*b),
            Some(CellContent::Formula(_)) => {
                self.invalid_result.set(true);
                Value::Error(syntax::FormulaError::Value)
            }
        }
    }
}
impl ReferenceResolver for ProjectionResolver<'_> {
    fn resolve_cell(&self, r: &syntax::CellReference) -> Value {
        let Some(sheet) = r.sheet.as_ref().and_then(|key| self.sheets.get(key)) else {
            return Value::Error(syntax::FormulaError::Ref);
        };
        self.value(sheet, r.row, r.column)
    }
    fn visit_range(
        &self,
        r: &syntax::RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        if r.start.sheet != r.end.sheet {
            return Err(EvaluationError::InvalidAst);
        }
        let Some(sheet) = r.start.sheet.as_ref().and_then(|key| self.sheets.get(key)) else {
            return visitor(Value::Error(syntax::FormulaError::Ref));
        };
        for row in r.start.row.min(r.end.row)..=r.start.row.max(r.end.row) {
            for column in r.start.column.min(r.end.column)..=r.start.column.max(r.end.column) {
                let value = self.value(sheet, row, column);
                if self.invalid_result.get() {
                    return Err(EvaluationError::InvalidResolverResult);
                }
                visitor(value)?;
            }
        }
        Ok(())
    }
}
/// `results` belongs to this workbook/input version and contains only formula-cell
/// calculated values. The caller owns provenance and scheduling, not this bridge.
pub fn evaluate_bound(
    formula: &bound::Formula,
    workbook: &Workbook,
    results: &BTreeMap<CellAddress, Value>,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    evaluate_bound_with_overlay(formula, workbook, results, &BTreeMap::new(), budget)
}

/// Evaluates against staged predecessor results without cloning the entire cache.
/// The caller commits the overlay only when the whole calculation succeeds.
pub fn evaluate_bound_with_overlay(
    formula: &bound::Formula,
    workbook: &Workbook,
    results: &BTreeMap<CellAddress, Value>,
    overlay: &BTreeMap<CellAddress, Value>,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    if !origin_exists(formula, workbook) {
        return Err(EvaluationError::InvalidAst);
    }
    let ast = project(formula, workbook)?;
    let resolver = ProjectionResolver {
        sheets: workbook
            .sheets()
            .iter()
            .map(|sheet| (key(sheet.id()), sheet))
            .collect(),
        results,
        overlay,
        invalid_result: Cell::new(false),
    };
    let result = evaluate(&ast, &resolver, budget);
    if resolver.invalid_result.get() {
        return Err(EvaluationError::InvalidResolverResult);
    }
    result
}
