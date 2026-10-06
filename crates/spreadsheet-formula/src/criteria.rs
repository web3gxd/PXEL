//! Bounded shared criteria and deterministic conditional aggregates.
use crate::{
    Ast, EvaluationBudget, EvaluationError, FormulaError, MAX_DEPTH, Node, NodeId, RangeReference,
    ReferenceResolver, Value,
};
use crate::{
    evaluator::{eval, number},
    index::Selection,
    lookup::{self, Token},
};
use std::cmp::Ordering;

#[derive(Clone, Copy)]
enum Comparison {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
enum Operand {
    Number(f64),
    Boolean(bool),
    Empty,
    Text(Vec<char>, Vec<Token>),
    Error(FormulaError),
}
struct Criterion {
    comparison: Comparison,
    operand: Operand,
}
fn finite_text(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}
fn compile(
    value: Value,
    budget: &mut EvaluationBudget,
) -> Result<Result<Criterion, FormulaError>, EvaluationError> {
    let value = lookup::checked_scalar(value, budget)?;
    let mut comparison = Comparison::Eq;
    let operand = match value {
        Value::Error(code) => return Ok(Err(code)),
        Value::Blank => Operand::Number(0.),
        Value::Number(n) | Value::DateTime { serial: n, .. } if n.is_finite() => Operand::Number(n),
        Value::Number(_) | Value::DateTime { .. } => return Ok(Err(FormulaError::Num)),
        Value::Boolean(value) => Operand::Boolean(value),
        Value::Text(text) => {
            if text.chars().count() > 255 {
                return Err(EvaluationError::UnsupportedCompatibilityFeature);
            }
            let mut payload = text.as_str();
            for (prefix, operator) in [
                ("<>", Comparison::Ne),
                ("<=", Comparison::Le),
                (">=", Comparison::Ge),
                ("=", Comparison::Eq),
                ("<", Comparison::Lt),
                (">", Comparison::Gt),
            ] {
                if let Some(tail) = payload.strip_prefix(prefix) {
                    comparison = operator;
                    payload = tail;
                    break;
                }
            }
            if payload.is_empty() {
                Operand::Empty
            } else if let Some(n) = finite_text(payload) {
                Operand::Number(n)
            } else if payload.eq_ignore_ascii_case("TRUE") {
                Operand::Boolean(true)
            } else if payload.eq_ignore_ascii_case("FALSE") {
                Operand::Boolean(false)
            } else if let Some(code) = [
                FormulaError::Null,
                FormulaError::Div0,
                FormulaError::Value,
                FormulaError::Ref,
                FormulaError::Name,
                FormulaError::Num,
                FormulaError::Na,
                FormulaError::Spill,
                FormulaError::Cycle,
            ]
            .into_iter()
            .find(|code| code.code() == payload)
            {
                Operand::Error(code)
            } else {
                let text = lookup::folded(payload, budget)?;
                let tokens = lookup::pattern(&text, budget)?;
                Operand::Text(text, tokens)
            }
        }
        Value::Array { .. } => return Err(EvaluationError::UnsupportedCompatibilityFeature),
    };
    Ok(Ok(Criterion {
        comparison,
        operand,
    }))
}
fn compare(comparison: Comparison, order: Option<Ordering>) -> bool {
    match comparison {
        Comparison::Eq => order == Some(Ordering::Equal),
        Comparison::Ne => order != Some(Ordering::Equal),
        Comparison::Lt => order == Some(Ordering::Less),
        Comparison::Le => matches!(order, Some(Ordering::Less | Ordering::Equal)),
        Comparison::Gt => order == Some(Ordering::Greater),
        Comparison::Ge => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
    }
}
impl Criterion {
    fn matches(
        &self,
        value: &Value,
        budget: &mut EvaluationBudget,
    ) -> Result<bool, EvaluationError> {
        budget.charge()?;
        let ordering = match &self.operand {
            Operand::Number(target) => {
                let candidate = match value {
                    Value::Number(n) | Value::DateTime { serial: n, .. } => Some(*n),
                    Value::Blank => Some(0.),
                    Value::Text(text) => finite_text(text),
                    _ => None,
                };
                candidate.and_then(|n| n.partial_cmp(target))
            }
            Operand::Boolean(target) => match value {
                Value::Boolean(value) => Some(value.cmp(target)),
                Value::Text(text) if text.eq_ignore_ascii_case("TRUE") => Some(true.cmp(target)),
                Value::Text(text) if text.eq_ignore_ascii_case("FALSE") => Some(false.cmp(target)),
                _ => None,
            },
            Operand::Empty => match value {
                Value::Blank => Some(Ordering::Equal),
                Value::Text(text)
                    if text.is_empty() && !matches!(self.comparison, Comparison::Ne) =>
                {
                    Some(Ordering::Equal)
                }
                Value::Text(_) => Some(Ordering::Greater),
                _ => None,
            },
            Operand::Error(code) => match value {
                Value::Error(value) if value == code => Some(Ordering::Equal),
                _ => None,
            },
            Operand::Text(text, tokens) => {
                let Value::Text(value) = value else {
                    return Ok(matches!(self.comparison, Comparison::Ne));
                };
                let value = lookup::folded(value, budget)?;
                if matches!(self.comparison, Comparison::Eq | Comparison::Ne) {
                    let all_stars = tokens.iter().all(|token| matches!(token, Token::Star));
                    let wildcard_match = lookup::wildcard(tokens, &value, budget)?;
                    let matched = wildcard_match && !(value.is_empty() && all_stars);
                    return Ok(if matches!(self.comparison, Comparison::Ne) {
                        !matched
                    } else {
                        matched
                    });
                }
                Some(lookup::text_order(&value, text, budget)?)
            }
        };
        Ok(compare(self.comparison, ordering))
    }
}
fn reference(
    ast: &Ast,
    id: NodeId,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Result<RangeReference, FormulaError>, EvaluationError> {
    let mut range = match lookup::source(ast, id, resolver, budget, depth)? {
        Selection::Reference(range) => range,
        Selection::Value(Value::Error(code)) => return Ok(Err(code)),
        Selection::Value(Value::Array { .. }) => {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        _ => return Ok(Err(FormulaError::Value)),
    };
    lookup::normalize(&mut range)?;
    Ok(Ok(range))
}
fn literal(ast: &Ast, id: NodeId) -> Option<RangeReference> {
    match ast.nodes.get(id.0)? {
        Node::Cell(cell) => Some(RangeReference {
            start: cell.clone(),
            end: cell.clone(),
        }),
        Node::Range(range) => Some(range.clone()),
        _ => None,
    }
}
fn sum_rectangle(
    keys: &RangeReference,
    source: &RangeReference,
) -> Result<RangeReference, FormulaError> {
    let mut range = source.clone();
    range.end.row = range
        .start
        .row
        .checked_add(keys.end.row - keys.start.row)
        .ok_or(FormulaError::Ref)?;
    range.end.column = range
        .start
        .column
        .checked_add(keys.end.column - keys.start.column)
        .ok_or(FormulaError::Ref)?;
    if range.end.row >= 1_048_576 || range.end.column >= 16_384 {
        return Err(FormulaError::Ref);
    }
    Ok(range)
}
/// Extra range dependencies for literal top-left expansion, without rewriting AST.
pub(crate) fn implicit_sum_ranges(ast: &Ast) -> Result<Vec<RangeReference>, EvaluationError> {
    let mut ranges = Vec::new();
    for node in &ast.nodes {
        let Node::Call { name, arguments } = node else {
            continue;
        };
        if name != "SUMIF" || arguments.len() != 3 {
            continue;
        }
        let (Some(mut keys), Some(mut sums)) =
            (literal(ast, arguments[0]), literal(ast, arguments[2]))
        else {
            continue;
        };
        lookup::normalize(&mut keys)?;
        lookup::normalize(&mut sums)?;
        if let Ok(actual) = sum_rectangle(&keys, &sums)
            && (actual.end.row > sums.end.row || actual.end.column > sums.end.column)
        {
            ranges.push(actual);
        }
    }
    Ok(ranges)
}
fn checked_reference(
    value: Value,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    let value = lookup::checked_scalar(value, budget)?;
    Ok(match value {
        Value::Number(n) if !n.is_finite() => Value::Error(FormulaError::Num),
        Value::DateTime { serial, .. } if !serial.is_finite() => Value::Error(FormulaError::Num),
        value => value,
    })
}
fn add_sum(value: &Value, total: &mut f64, error: &mut Option<FormulaError>) {
    match value {
        Value::Number(n) | Value::DateTime { serial: n, .. } => {
            *total += n;
            if !total.is_finite() && error.is_none() {
                *error = Some(FormulaError::Num);
            }
        }
        Value::Error(code) if error.is_none() => *error = Some(*code),
        _ => {}
    }
}
pub(crate) fn multi_call(
    ast: &Ast,
    args: &[NodeId],
    sum: bool,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.charge()?;
    if depth > MAX_DEPTH {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    let offset = usize::from(sum);
    if args.len() < offset + 2
        || !(args.len() - offset).is_multiple_of(2)
        || (args.len() - offset) / 2 > 127
    {
        return Ok(Value::Error(FormulaError::Value));
    }
    let sums = if sum {
        match reference(ast, args[0], resolver, budget, depth + 1)? {
            Ok(range) => Some(range),
            Err(code) => return Ok(Value::Error(code)),
        }
    } else {
        None
    };
    let dimensions = |range: &RangeReference| {
        (
            range.end.row - range.start.row,
            range.end.column - range.start.column,
        )
    };
    let mut shape = sums.as_ref().map(dimensions);
    let mut pairs = Vec::new();
    for pair in args[offset..].as_chunks::<2>().0 {
        let range = match reference(ast, pair[0], resolver, budget, depth + 1)? {
            Ok(range) => range,
            Err(code) => return Ok(Value::Error(code)),
        };
        let current = dimensions(&range);
        if let Some(shape) = shape {
            if shape != current {
                return Ok(Value::Error(FormulaError::Value));
            }
        } else {
            shape = Some(current);
        }
        let criterion = match compile(eval(ast, pair[1], resolver, budget, depth + 1)?, budget)? {
            Ok(criterion) => criterion,
            Err(code) => return Ok(Value::Error(code)),
        };
        pairs.push((range, criterion));
    }
    let mut matched = Vec::new();
    for (index, (range, criterion)) in pairs.iter().enumerate() {
        let mut visits = 0usize;
        resolver.visit_range(range, &mut |value| {
            budget.charge()?;
            if visits as u64 >= range.cell_count() {
                return Err(EvaluationError::InvalidResolverResult);
            }
            let value = checked_reference(value, budget)?;
            let selected = criterion.matches(&value, budget)?;
            if index == 0 {
                matched.push(selected);
            } else {
                let entry = matched
                    .get_mut(visits)
                    .ok_or(EvaluationError::InvalidResolverResult)?;
                *entry &= selected;
            }
            visits += 1;
            Ok(())
        })?;
        if visits as u64 != range.cell_count() {
            return Err(EvaluationError::InvalidResolverResult);
        }
    }
    if let Some(sums) = sums {
        let mut total = 0.;
        let mut error = None;
        let mut visits = 0usize;
        resolver.visit_range(&sums, &mut |value| {
            budget.charge()?;
            let selected = *matched
                .get(visits)
                .ok_or(EvaluationError::InvalidResolverResult)?;
            visits += 1;
            let value = checked_reference(value, budget)?;
            if selected {
                if let Value::Boolean(value) = value {
                    add_sum(&Value::Number(f64::from(value)), &mut total, &mut error);
                } else {
                    add_sum(&value, &mut total, &mut error);
                }
            }
            Ok(())
        })?;
        if visits != matched.len() {
            return Err(EvaluationError::InvalidResolverResult);
        }
        Ok(error.map_or_else(|| number(total), Value::Error))
    } else {
        let mut count = 0usize;
        for selected in matched {
            budget.charge()?;
            count += usize::from(selected);
        }
        Ok(Value::Number(count as f64))
    }
}
pub(crate) fn call(
    ast: &Ast,
    args: &[NodeId],
    sum: bool,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.charge()?;
    if depth > MAX_DEPTH {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    if args.len() != 2 && !(sum && args.len() == 3) {
        return Ok(Value::Error(FormulaError::Value));
    }
    let keys = match reference(ast, args[0], resolver, budget, depth + 1)? {
        Ok(range) => range,
        Err(code) => return Ok(Value::Error(code)),
    };
    let criterion = match compile(eval(ast, args[1], resolver, budget, depth + 1)?, budget)? {
        Ok(criterion) => criterion,
        Err(code) => return Ok(Value::Error(code)),
    };
    let sums = if let Some(id) = args
        .get(2)
        .filter(|id| !matches!(ast.nodes.get(id.0), Some(Node::Missing)))
    {
        let source = match reference(ast, *id, resolver, budget, depth + 1)? {
            Ok(range) => range,
            Err(code) => return Ok(Value::Error(code)),
        };
        let actual = match sum_rectangle(&keys, &source) {
            Ok(range) => range,
            Err(code) => return Ok(Value::Error(code)),
        };
        if (actual.end.row > source.end.row || actual.end.column > source.end.column)
            && (literal(ast, args[0]).is_none() || literal(ast, *id).is_none())
        {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        Some(actual)
    } else {
        None
    };
    let mut visits = 0;
    let mut count = 0;
    let mut total = 0.;
    let mut error = None;
    let mut matched = Vec::new();
    resolver.visit_range(&keys, &mut |value| {
        budget.charge()?;
        visits += 1;
        if visits > keys.cell_count() {
            return Err(EvaluationError::InvalidResolverResult);
        }
        let value = checked_reference(value, budget)?;
        let selected = criterion.matches(&value, budget)?;
        if selected {
            count += 1;
            if sum && sums.is_none() {
                add_sum(&value, &mut total, &mut error);
            }
        }
        if sums.is_some() {
            matched.push(selected);
        }
        Ok(())
    })?;
    if visits != keys.cell_count() {
        return Err(EvaluationError::InvalidResolverResult);
    }
    if let Some(sums) = sums {
        let mut visits = 0usize;
        resolver.visit_range(&sums, &mut |value| {
            budget.charge()?;
            let selected = *matched
                .get(visits)
                .ok_or(EvaluationError::InvalidResolverResult)?;
            visits += 1;
            let value = checked_reference(value, budget)?;
            if selected {
                add_sum(&value, &mut total, &mut error);
            }
            Ok(())
        })?;
        if visits != matched.len() {
            return Err(EvaluationError::InvalidResolverResult);
        }
    }
    Ok(if sum {
        error.map_or_else(|| number(total), Value::Error)
    } else {
        Value::Number(count as f64)
    })
}
