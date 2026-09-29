//! Fixed portable POWER/^ profile; native and browser share the same implementation.
use crate::{
    FormulaError, Value,
    evaluator::{EvaluationBudget, EvaluationError, number},
};

pub(crate) fn power(
    base: f64,
    exponent: f64,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    // Fixed admission cost for the bounded libm algorithm, including cancellation checks.
    for _ in 0..64 {
        budget.charge()?;
    }
    if base == 0.0 {
        return Ok(if exponent == 0.0 {
            Value::Error(FormulaError::Num)
        } else if exponent < 0.0 {
            Value::Error(FormulaError::Div0)
        } else {
            number(0.0)
        });
    }
    if base < 0.0 && exponent.fract() != 0.0 {
        return Ok(Value::Error(FormulaError::Num));
    }
    Ok(number(libm::pow(base, exponent)))
}
