//! Contiguous reference INDEX profile. References remain intermediate only.
use crate::{
    Ast, FormulaError, MAX_DEPTH, MAX_RANGE_CELLS, MAX_SOURCE_BYTES, Node, NodeId, RangeReference,
    ReferenceResolver, Value,
    evaluator::{EvaluationBudget, EvaluationError, eval, number, numeric},
};
enum Selection {
    Reference(RangeReference),
    Value(Value),
}
fn selector(
    ast: &Ast,
    id: NodeId,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Result<f64, FormulaError>, EvaluationError> {
    let value = eval(ast, id, resolver, budget, depth)?;
    if let Value::Text(text) = &value {
        if text.len() > MAX_SOURCE_BYTES {
            return Err(EvaluationError::CalculationLimitExceeded);
        }
        for _ in text.chars() {
            budget.charge()?;
        }
    }
    Ok(numeric(value).and_then(|n| {
        if n < 0.0 {
            Err(FormulaError::Value)
        } else {
            Ok(n.trunc())
        }
    }))
}
fn select(
    ast: &Ast,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Selection, EvaluationError> {
    budget.charge()?;
    if depth > MAX_DEPTH {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    if !(2..=4).contains(&args.len()) {
        return Ok(Selection::Value(Value::Error(FormulaError::Value)));
    }
    let mut source = match ast
        .nodes
        .get(args[0].0)
        .ok_or(EvaluationError::InvalidAst)?
    {
        Node::Cell(cell) => Selection::Reference(RangeReference {
            start: cell.clone(),
            end: cell.clone(),
        }),
        Node::Range(range) => Selection::Reference(range.clone()),
        Node::Call { name, arguments } if name == "INDEX" => {
            select(ast, arguments, resolver, budget, depth + 1)?
        }
        _ => Selection::Value(eval(ast, args[0], resolver, budget, depth + 1)?),
    };
    if let Selection::Value(Value::Error(_)) = source {
        return Ok(source);
    }
    let (rows, columns) = match &mut source {
        Selection::Reference(range) => {
            if range.start.sheet != range.end.sheet
                || range.start.row >= 1_048_576
                || range.end.row >= 1_048_576
                || range.start.column >= 16_384
                || range.end.column >= 16_384
            {
                return Err(EvaluationError::InvalidAst);
            }
            if range.cell_count() > MAX_RANGE_CELLS {
                return Err(EvaluationError::CalculationLimitExceeded);
            }
            let (first_row, last_row) = (
                range.start.row.min(range.end.row),
                range.start.row.max(range.end.row),
            );
            let (first_column, last_column) = (
                range.start.column.min(range.end.column),
                range.start.column.max(range.end.column),
            );
            range.start.row = first_row;
            range.start.column = first_column;
            range.end.row = last_row;
            range.end.column = last_column;
            (last_row - first_row + 1, last_column - first_column + 1)
        }
        Selection::Value(Value::Array { .. }) => {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        Selection::Value(_) => (1, 1),
    };
    // Ambiguous compact horizontal/2D defaults need independent Excel goldens.
    if args.len() == 2 && columns > 1 {
        return Err(EvaluationError::UnsupportedCompatibilityFeature);
    }
    let row = match selector(ast, args[1], resolver, budget, depth + 1)? {
        Ok(n) => n,
        Err(error) => return Ok(Selection::Value(Value::Error(error))),
    };
    let column = if let Some(id) = args.get(2) {
        match selector(ast, *id, resolver, budget, depth + 1)? {
            Ok(n) => n,
            Err(error) => return Ok(Selection::Value(Value::Error(error))),
        }
    } else {
        1.0
    };
    if let Some(id) = args.get(3) {
        let area = if matches!(ast.nodes.get(id.0), Some(Node::Missing)) {
            1.0
        } else {
            match selector(ast, *id, resolver, budget, depth + 1)? {
                Ok(n) => n,
                Err(error) => return Ok(Selection::Value(Value::Error(error))),
            }
        };
        if area != 1.0 {
            return Ok(Selection::Value(Value::Error(FormulaError::Ref)));
        }
    }
    if row > f64::from(rows) || column > f64::from(columns) {
        return Ok(Selection::Value(Value::Error(FormulaError::Ref)));
    }
    if let Selection::Reference(range) = &mut source {
        if row != 0.0 {
            range.start.row += row as u32 - 1;
            range.end.row = range.start.row;
        }
        if column != 0.0 {
            range.start.column += column as u32 - 1;
            range.end.column = range.start.column;
        }
    }
    Ok(source)
}
pub(crate) fn call(
    ast: &Ast,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    let value = match select(ast, args, resolver, budget, depth)? {
        Selection::Reference(range) => {
            if range.cell_count() != 1 {
                return Err(EvaluationError::UnsupportedCompatibilityFeature);
            }
            budget.charge()?;
            resolver.resolve_cell(&range.start)
        }
        Selection::Value(value) => value,
    };
    Ok(match value {
        Value::Blank => Value::Number(0.0),
        Value::Number(n) => number(n),
        Value::Array { .. } => return Err(EvaluationError::UnsupportedCompatibilityFeature),
        value => value,
    })
}
/// False means non-INDEX argument; reference coercion is preserved for selected cells.
pub(crate) fn visit(
    ast: &Ast,
    id: NodeId,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
    visitor: &mut dyn FnMut(Value, bool) -> Result<(), EvaluationError>,
) -> Result<bool, EvaluationError> {
    let Node::Call { name, arguments } = ast.nodes.get(id.0).ok_or(EvaluationError::InvalidAst)?
    else {
        return Ok(false);
    };
    if name != "INDEX" {
        return Ok(false);
    }
    match select(ast, arguments, resolver, budget, depth)? {
        Selection::Value(value) => visitor(value, false)?,
        Selection::Reference(range) => {
            let expected = range.cell_count();
            let mut visits = 0u64;
            resolver.visit_range(&range, &mut |value| {
                budget.charge()?;
                visits += 1;
                if visits > expected {
                    return Err(EvaluationError::InvalidResolverResult);
                }
                if matches!(value, Value::Array { .. }) {
                    return Err(EvaluationError::UnsupportedCompatibilityFeature);
                }
                visitor(value, true)
            })?;
            if visits != expected {
                return Err(EvaluationError::InvalidResolverResult);
            }
        }
    }
    Ok(true)
}
