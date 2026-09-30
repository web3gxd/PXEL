//! Deterministic bounded lookup comparisons; no locale or host dependencies.
use crate::{
    Ast, EvaluationBudget, EvaluationError, FormulaError, MAX_DEPTH, MAX_RANGE_CELLS,
    MAX_SOURCE_BYTES, Node, NodeId, RangeReference, ReferenceResolver, Value,
    evaluator::{eval, logical, number, numeric},
    index::Selection,
};
use std::cmp::Ordering;

fn folded(text: &str, budget: &mut EvaluationBudget) -> Result<Vec<char>, EvaluationError> {
    if text.len() > MAX_SOURCE_BYTES {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    let mut result = Vec::new();
    for ch in text.chars() {
        budget.charge()?;
        for lower in ch.to_lowercase() {
            budget.charge()?;
            if result.len() >= MAX_SOURCE_BYTES {
                return Err(EvaluationError::CalculationLimitExceeded);
            }
            result.push(lower);
        }
    }
    Ok(result)
}

#[derive(Clone, Copy)]
enum Token {
    Literal(char),
    Any,
    Star,
}
fn pattern(text: &[char], budget: &mut EvaluationBudget) -> Result<Vec<Token>, EvaluationError> {
    let mut result = Vec::new();
    let mut chars = text.iter().copied().peekable();
    while let Some(ch) = chars.next() {
        budget.charge()?;
        let token = match ch {
            '*' => Token::Star,
            '?' => Token::Any,
            '~' if matches!(chars.peek(), Some('*' | '?' | '~')) => {
                budget.charge()?;
                Token::Literal(chars.next().ok_or(EvaluationError::InvalidAst)?)
            }
            ch => Token::Literal(ch),
        };
        result.push(token);
    }
    Ok(result)
}
fn wildcard(
    tokens: &[Token],
    text: &[char],
    budget: &mut EvaluationBudget,
) -> Result<bool, EvaluationError> {
    let mut literals_only = true;
    for token in tokens {
        budget.charge()?;
        literals_only &= matches!(token, Token::Literal(_));
    }
    if literals_only {
        if tokens.len() != text.len() {
            return Ok(false);
        }
        for (token, ch) in tokens.iter().zip(text) {
            budget.charge()?;
            if !matches!(token, Token::Literal(literal) if literal == ch) {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    let mut prior = vec![false; text.len() + 1];
    let mut next = vec![false; text.len() + 1];
    prior[0] = true;
    for token in tokens {
        budget.charge()?;
        next[0] = matches!(token, Token::Star) && prior[0];
        for (offset, ch) in text.iter().enumerate() {
            budget.charge()?;
            next[offset + 1] = match token {
                Token::Star => prior[offset + 1] || next[offset],
                Token::Any => prior[offset],
                Token::Literal(literal) => prior[offset] && literal == ch,
            };
        }
        std::mem::swap(&mut prior, &mut next);
    }
    Ok(prior[text.len()])
}
fn text_order(
    left: &[char],
    right: &[char],
    budget: &mut EvaluationBudget,
) -> Result<Ordering, EvaluationError> {
    for (a, b) in left.iter().zip(right) {
        budget.charge()?;
        if a != b {
            return Ok(a.cmp(b));
        }
    }
    Ok(left.len().cmp(&right.len()))
}

enum Key {
    Number(f64),
    Text(Vec<char>),
    Boolean(bool),
    Blank,
    Error(FormulaError),
}
fn key(value: Value, budget: &mut EvaluationBudget) -> Result<Key, EvaluationError> {
    Ok(match value {
        Value::Number(n) | Value::DateTime { serial: n, .. } if n.is_finite() => {
            Key::Number(if n == 0.0 { 0.0 } else { n })
        }
        Value::Number(_) | Value::DateTime { .. } => Key::Error(FormulaError::Num),
        Value::Text(text) => Key::Text(folded(&text, budget)?),
        Value::Boolean(value) => Key::Boolean(value),
        Value::Blank => Key::Blank,
        Value::Error(error) => Key::Error(error),
        Value::Array { .. } => return Err(EvaluationError::UnsupportedCompatibilityFeature),
    })
}
fn order(
    left: &Key,
    right: &Key,
    budget: &mut EvaluationBudget,
) -> Result<Ordering, EvaluationError> {
    budget.charge()?;
    Ok(match (left, right) {
        (Key::Number(a), Key::Number(b)) => a
            .partial_cmp(b)
            .ok_or(EvaluationError::InvalidResolverResult)?,
        (Key::Text(a), Key::Text(b)) => text_order(a, b, budget)?,
        (Key::Boolean(a), Key::Boolean(b)) => a.cmp(b),
        (Key::Number(_), _) | (Key::Text(_), Key::Boolean(_)) => Ordering::Less,
        (_, Key::Number(_)) | (Key::Boolean(_), Key::Text(_)) => Ordering::Greater,
        _ => return Err(EvaluationError::InvalidResolverResult),
    })
}
fn source(
    ast: &Ast,
    id: NodeId,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Selection, EvaluationError> {
    budget.charge()?;
    Ok(
        match ast.nodes.get(id.0).ok_or(EvaluationError::InvalidAst)? {
            Node::Cell(cell) => Selection::Reference(RangeReference {
                start: cell.clone(),
                end: cell.clone(),
            }),
            Node::Range(range) => Selection::Reference(range.clone()),
            Node::Call { name, arguments } if name == "INDEX" => {
                crate::index::select(ast, arguments, resolver, budget, depth + 1)?
            }
            _ => Selection::Value(eval(ast, id, resolver, budget, depth + 1)?),
        },
    )
}
pub(crate) fn call(
    ast: &Ast,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.charge()?;
    if depth > MAX_DEPTH {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    if !(2..=3).contains(&args.len()) {
        return Ok(Value::Error(FormulaError::Value));
    }
    let lookup = key(eval(ast, args[0], resolver, budget, depth + 1)?, budget)?;
    let lookup = match lookup {
        Key::Error(error) => return Ok(Value::Error(error)),
        Key::Blank => Key::Number(0.0),
        value => value,
    };
    let mode = if let Some(id) = args.get(2) {
        let value = eval(ast, *id, resolver, budget, depth + 1)?;
        if let Value::Text(text) = &value {
            if text.len() > MAX_SOURCE_BYTES {
                return Err(EvaluationError::CalculationLimitExceeded);
            }
            for _ in text.chars() {
                budget.charge()?;
            }
        }
        if matches!(value, Value::Array { .. }) {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        match numeric(value) {
            Ok(n) if n == -1.0 || n == 0.0 || n == 1.0 => n as i8,
            Ok(_) => return Ok(Value::Error(FormulaError::Value)),
            Err(error) => return Ok(Value::Error(error)),
        }
    } else {
        1
    };
    let mut range = match source(ast, args[1], resolver, budget, depth)? {
        Selection::Reference(range) => range,
        Selection::Value(Value::Error(error)) => return Ok(Value::Error(error)),
        Selection::Value(Value::Array { .. }) => {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        Selection::Value(_) => return Ok(Value::Error(FormulaError::Value)),
    };
    let (rows, columns) = normalize(&mut range)?;
    if rows != 1 && columns != 1 {
        return Ok(Value::Error(FormulaError::Value));
    }
    search(&range, lookup, mode, resolver, budget)
}
fn normalize(range: &mut RangeReference) -> Result<(u32, u32), EvaluationError> {
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
    let (row, col) = (
        range.start.row.min(range.end.row),
        range.start.column.min(range.end.column),
    );
    range.end.row = range.start.row.max(range.end.row);
    range.end.column = range.start.column.max(range.end.column);
    range.start.row = row;
    range.start.column = col;
    Ok((range.end.row - row + 1, range.end.column - col + 1))
}

fn checked_scalar(value: Value, budget: &mut EvaluationBudget) -> Result<Value, EvaluationError> {
    if matches!(value, Value::Array { .. }) {
        return Err(EvaluationError::UnsupportedCompatibilityFeature);
    }
    if let Value::Text(text) = &value {
        if text.len() > MAX_SOURCE_BYTES {
            return Err(EvaluationError::CalculationLimitExceeded);
        }
        for _ in text.chars() {
            budget.charge()?;
        }
    }
    Ok(value)
}
pub(crate) fn table_call(
    ast: &Ast,
    args: &[NodeId],
    vertical: bool,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.charge()?;
    if depth > MAX_DEPTH {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    if !(3..=4).contains(&args.len()) {
        return Ok(Value::Error(FormulaError::Value));
    }
    let lookup = match key(eval(ast, args[0], resolver, budget, depth + 1)?, budget)? {
        Key::Error(error) => return Ok(Value::Error(error)),
        Key::Blank => Key::Number(0.0),
        value => value,
    };
    let mut table = match source(ast, args[1], resolver, budget, depth)? {
        Selection::Reference(range) => range,
        Selection::Value(Value::Error(error)) => return Ok(Value::Error(error)),
        Selection::Value(Value::Array { .. }) => {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        Selection::Value(_) => return Ok(Value::Error(FormulaError::Value)),
    };
    let (rows, columns) = normalize(&mut table)?;
    let index = checked_scalar(eval(ast, args[2], resolver, budget, depth + 1)?, budget)?;
    let index = match numeric(index) {
        Ok(n) if n >= 1.0 => n.trunc(),
        Ok(_) => return Ok(Value::Error(FormulaError::Value)),
        Err(error) => return Ok(Value::Error(error)),
    };
    let extent = if vertical { columns } else { rows };
    if index > f64::from(extent) {
        return Ok(Value::Error(FormulaError::Ref));
    }
    let approximate = if let Some(id) = args.get(3) {
        let mode = checked_scalar(eval(ast, *id, resolver, budget, depth + 1)?, budget)?;
        match logical(mode) {
            Ok(value) => value,
            Err(error) => return Ok(Value::Error(error)),
        }
    } else {
        true
    };
    let mut axis = table.clone();
    if vertical {
        axis.end.column = axis.start.column;
    } else {
        axis.end.row = axis.start.row;
    }
    let position = match search(&axis, lookup, i8::from(approximate), resolver, budget)? {
        Value::Number(n) => n as u32,
        value => return Ok(value),
    };
    // Search cardinality and index bounds prove offsets fit within the table.
    let mut selected = table.start;
    if vertical {
        selected.row += position - 1;
        selected.column += index as u32 - 1;
    } else {
        selected.row += index as u32 - 1;
        selected.column += position - 1;
    }
    budget.charge()?;
    let value = checked_scalar(resolver.resolve_cell(&selected), budget)?;
    Ok(match value {
        Value::Blank => Value::Number(0.0),
        Value::Number(n) => number(n),
        Value::DateTime { serial, system } if serial.is_finite() => Value::DateTime {
            serial: if serial == 0.0 { 0.0 } else { serial },
            system,
        },
        Value::DateTime { .. } => Value::Error(FormulaError::Num),
        value => value,
    })
}
fn search(
    range: &RangeReference,
    lookup: Key,
    mode: i8,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    let expected = range.cell_count();
    let tokens = if let (0, Key::Text(text)) = (mode, &lookup) {
        Some(pattern(text, budget)?)
    } else {
        None
    };
    let mut visits = 0u64;
    let mut found = None;
    let mut previous = None;
    let mut error = None;
    let mut unsorted = false;
    resolver.visit_range(range, &mut |value| {
        budget.charge()?;
        visits += 1;
        if visits > expected {
            return Err(EvaluationError::InvalidResolverResult);
        }
        let candidate = key(value, budget)?;
        if mode == 0 && found.is_some() {
            return Ok(());
        }
        if let Key::Error(code) = candidate {
            if error.is_none() {
                error = Some(code);
            }
            return Ok(());
        }
        if matches!(candidate, Key::Blank) || error.is_some() {
            return Ok(());
        }
        if mode == 0 {
            let matched = if let (Some(tokens), Key::Text(text)) = (&tokens, &candidate) {
                wildcard(tokens, text, budget)?
            } else {
                order(&candidate, &lookup, budget)? == Ordering::Equal
            };
            if matched {
                found = Some(visits);
            }
        } else {
            if let Some(prior) = &previous {
                let comparison = order(prior, &candidate, budget)?;
                if (mode == 1 && comparison == Ordering::Greater)
                    || (mode == -1 && comparison == Ordering::Less)
                {
                    unsorted = true;
                }
            }
            let comparison = order(&candidate, &lookup, budget)?;
            if (mode == 1 && comparison != Ordering::Greater)
                || (mode == -1 && comparison != Ordering::Less)
            {
                found = Some(visits);
            }
            previous = Some(candidate);
        }
        Ok(())
    })?;
    if visits != expected {
        return Err(EvaluationError::InvalidResolverResult);
    }
    Ok(if let Some(code) = error {
        Value::Error(code)
    } else if unsorted {
        Value::Error(FormulaError::Na)
    } else {
        found.map_or(Value::Error(FormulaError::Na), |n| Value::Number(n as f64))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    fn oracle(tokens: &[Token], text: &[char]) -> bool {
        let Some((first, tail)) = tokens.split_first() else {
            return text.is_empty();
        };
        match first {
            Token::Star => {
                oracle(tail, text)
                    || text
                        .split_first()
                        .is_some_and(|(_, rest)| oracle(tokens, rest))
            }
            Token::Any => text
                .split_first()
                .is_some_and(|(_, rest)| oracle(tail, rest)),
            Token::Literal(ch) => text
                .split_first()
                .is_some_and(|(actual, rest)| actual == ch && oracle(tail, rest)),
        }
    }
    proptest! {
        #[test]
        fn wildcard_dp_matches_small_exhaustive_branch_oracle(source in "[ab?*]{0,7}", text in "[ab]{0,7}") {
            let mut budget=EvaluationBudget::default();
            let tokens=pattern(&source.chars().collect::<Vec<_>>(),&mut budget).map_err(|e|TestCaseError::fail(e.to_string()))?;
            let text=text.chars().collect::<Vec<_>>();
            let actual=wildcard(&tokens,&text,&mut budget).map_err(|e|TestCaseError::fail(e.to_string()))?;
            prop_assert_eq!(actual,oracle(&tokens,&text));
        }
    }
    #[test]
    fn wildcard_escape_and_unicode_contract() -> Result<(), EvaluationError> {
        for (source, value, expected) in [
            ("a*?", "AB", true),
            ("a?", "a😀", true),
            ("~*", "*", true),
            ("~~", "~", true),
            ("~x", "~x", true),
            ("x~", "x~", true),
            ("İ", "i\u{307}", true),
            ("*x*y", "ayxxby", true),
            ("*x*y", "ayxxbz", false),
            ("*", "", true),
        ] {
            let mut budget = EvaluationBudget::default();
            let normalized = folded(source, &mut budget)?;
            let tokens = pattern(&normalized, &mut budget)?;
            let normalized = folded(value, &mut budget)?;
            assert_eq!(wildcard(&tokens, &normalized, &mut budget)?, expected);
        }
        Ok(())
    }
}
