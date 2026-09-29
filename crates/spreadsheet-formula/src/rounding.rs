//! Versioned shortest-decimal quantization; see ADR-060, not Excel bitwise parity.
use crate::{
    FormulaError, Value,
    evaluator::{EvaluationBudget, EvaluationError, number},
};

pub(crate) enum Mode {
    Nearest,
    Away,
    Toward,
}

pub(crate) fn round(
    value: f64,
    precision: f64,
    mode: Mode,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    let precision = precision.trunc();
    if value == 0.0 || precision >= 324.0 {
        return Ok(number(value));
    }
    if precision <= -309.0 {
        return Ok(match mode {
            Mode::Away => Value::Error(FormulaError::Num),
            _ => number(0.0),
        });
    }
    // Finite inputs are validated by numeric coercion, and precision is now bounded.
    let precision = precision as i32;
    let source = value.abs().to_string();
    for _ in source.bytes() {
        budget.charge()?;
    }
    let (mantissa, exponent) = if let Some((mantissa, exponent)) = source.split_once(['e', 'E']) {
        (
            mantissa,
            exponent
                .parse::<i32>()
                .map_err(|_| EvaluationError::InvalidAst)?,
        )
    } else {
        (source.as_str(), 0)
    };
    let point = mantissa.find('.').unwrap_or(mantissa.len()) as i32 + exponent;
    let mut digits: Vec<u8> = mantissa.bytes().filter(|byte| *byte != b'.').collect();
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(EvaluationError::InvalidAst);
    }
    let cut = point + precision;
    if cut >= digits.len() as i32 {
        return Ok(number(value));
    }
    let retained = cut.max(0) as usize;
    let discarded = digits.get(retained..).ok_or(EvaluationError::InvalidAst)?;
    let increment = match mode {
        Mode::Nearest => cut >= 0 && discarded.first().is_some_and(|digit| *digit >= b'5'),
        Mode::Away => discarded.iter().any(|digit| *digit != b'0'),
        Mode::Toward => false,
    };
    digits.truncate(retained);
    if digits.is_empty() {
        digits.push(b'0');
    }
    if increment {
        let mut carry = true;
        for digit in digits.iter_mut().rev() {
            budget.charge()?;
            if *digit < b'9' {
                *digit += 1;
                carry = false;
                break;
            }
            *digit = b'0';
        }
        if carry {
            digits.insert(0, b'1');
        }
    }
    let coefficient = String::from_utf8(digits).map_err(|_| EvaluationError::InvalidAst)?;
    let output = format!("{coefficient}e{}", -precision);
    for _ in output.bytes() {
        budget.charge()?;
    }
    let rounded = output
        .parse::<f64>()
        .map_err(|_| EvaluationError::InvalidAst)?;
    Ok(number(if value.is_sign_negative() {
        -rounded
    } else {
        rounded
    }))
}
