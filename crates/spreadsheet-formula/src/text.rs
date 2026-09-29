//! ADR-062: bounded Unicode-scalar text profile.
use crate::{
    Ast, FormulaError, MAX_RANGE_CELLS, MAX_SOURCE_BYTES, Node, NodeId, ReferenceResolver, Value,
    evaluator::{EvaluationBudget, EvaluationError, eval, numeric, text},
};

fn scan(input: &str, budget: &mut EvaluationBudget) -> Result<usize, EvaluationError> {
    if input.len() > MAX_SOURCE_BYTES {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    let mut count = 0;
    for _ in input.chars() {
        budget.charge()?;
        count += 1;
    }
    Ok(count)
}
fn append(
    out: &mut String,
    chars: &mut usize,
    error: &mut Option<FormulaError>,
    value: Value,
    budget: &mut EvaluationBudget,
) -> Result<(), EvaluationError> {
    let input = match text(value) {
        Ok(input) => input,
        Err(e) => {
            if error.is_none() {
                *error = Some(e);
            }
            return Ok(());
        }
    };
    let count = scan(&input, budget)?;
    if error.is_some() {
        return Ok(());
    }
    if chars.saturating_add(count) > 32767 {
        *error = Some(FormulaError::Value);
        return Ok(());
    }
    if out.len().saturating_add(input.len()) > MAX_SOURCE_BYTES {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    *chars += count;
    out.push_str(&input);
    Ok(())
}
pub(crate) fn call(
    ast: &Ast,
    name: &str,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    if name == "CONCAT" {
        if args.is_empty() || args.len() > 253 {
            return Ok(Value::Error(FormulaError::Value));
        }
        let mut out = String::new();
        let mut chars = 0;
        let mut error = None;
        for arg in args {
            match ast.nodes.get(arg.0).ok_or(EvaluationError::InvalidAst)? {
                Node::Range(range) => {
                    budget.charge()?;
                    if range.start.sheet != range.end.sheet || range.cell_count() > MAX_RANGE_CELLS
                    {
                        return Err(EvaluationError::CalculationLimitExceeded);
                    }
                    if range.start.row >= 1_048_576
                        || range.end.row >= 1_048_576
                        || range.start.column >= 16_384
                        || range.end.column >= 16_384
                    {
                        return Err(EvaluationError::InvalidAst);
                    }
                    let mut visits = 0u64;
                    resolver.visit_range(range, &mut |value| {
                        budget.charge()?;
                        visits += 1;
                        if visits > range.cell_count() {
                            return Err(EvaluationError::InvalidResolverResult);
                        }
                        append(&mut out, &mut chars, &mut error, value, budget)
                    })?;
                    if visits != range.cell_count() {
                        return Err(EvaluationError::InvalidResolverResult);
                    }
                }
                _ => append(
                    &mut out,
                    &mut chars,
                    &mut error,
                    eval(ast, *arg, resolver, budget, depth)?,
                    budget,
                )?,
            }
        }
        return Ok(error.map_or(Value::Text(out), Value::Error));
    }
    let valid = match name {
        "LEFT" | "RIGHT" => matches!(args.len(), 1 | 2),
        "MID" => args.len() == 3,
        _ => args.len() == 1,
    };
    if !valid {
        return Ok(Value::Error(FormulaError::Value));
    }
    let input = match text(eval(ast, args[0], resolver, budget, depth)?) {
        Ok(v) => v,
        Err(e) => return Ok(Value::Error(e)),
    };
    let length = scan(&input, budget)?;
    if name == "LEN" {
        return Ok(Value::Number(length as f64));
    }
    if matches!(name, "LOWER" | "UPPER") {
        let out = if name == "LOWER" {
            input.to_lowercase()
        } else {
            input.to_uppercase()
        };
        scan(&out, budget)?;
        return Ok(Value::Text(out));
    }
    if name == "TRIM" {
        let mut out = String::new();
        let mut space = false;
        for character in input.chars() {
            budget.charge()?;
            if character == ' ' {
                space = !out.is_empty();
            } else {
                if space {
                    out.push(' ');
                    space = false;
                }
                out.push(character);
            }
        }
        return Ok(Value::Text(out));
    }
    let mut positions = [1.0, 1.0];
    for (index, arg) in args.iter().skip(1).enumerate() {
        let index_value = eval(ast, *arg, resolver, budget, depth)?;
        if let Value::Text(input) = &index_value {
            scan(input, budget)?;
        }
        positions[index] = match numeric(index_value) {
            Ok(v) => v,
            Err(e) => return Ok(Value::Error(e)),
        };
    }
    let count = if name == "MID" {
        positions[1]
    } else {
        positions[0]
    };
    if count < 0.0 || (name == "MID" && positions[0].trunc() < 1.0) {
        return Ok(Value::Error(FormulaError::Value));
    }
    let count = count.trunc().min(length as f64) as usize;
    let start = match name {
        "LEFT" => 0,
        "RIGHT" => length.saturating_sub(count),
        "MID" => (positions[0].trunc() - 1.0).min(length as f64) as usize,
        _ => return Err(EvaluationError::InvalidAst),
    };
    let mut out = String::new();
    for (index, character) in input.chars().enumerate() {
        budget.charge()?;
        if index >= start && index - start < count {
            out.push(character);
        }
    }
    Ok(Value::Text(out))
}
