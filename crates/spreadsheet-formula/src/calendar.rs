//! ADR-063: bounded Gregorian serial arithmetic, including Excel's fake day.
use crate::{
    Ast, DateSystem, FormulaError, MAX_SOURCE_BYTES, NodeId, ReferenceResolver, Value,
    evaluator::{EvaluationBudget, EvaluationError, eval, numeric},
};

const MONTH_START: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const BASE_1900: i64 = 693595;
const BASE_1904: i64 = 695055;
fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
fn year_start(year: i64) -> Option<i64> {
    let y = year.checked_sub(1)?;
    y.checked_mul(365)?
        .checked_add(y.div_euclid(4))?
        .checked_sub(y.div_euclid(100))?
        .checked_add(y.div_euclid(400))
}
fn maximum(system: DateSystem) -> i64 {
    match system {
        DateSystem::Excel1900 => 2958465,
        DateSystem::Excel1904 => 2957003,
    }
}
fn integer(n: f64) -> Result<i64, FormulaError> {
    let n = n.trunc();
    if !n.is_finite() || n < i64::MIN as f64 || n >= i64::MAX as f64 {
        Err(FormulaError::Num)
    } else {
        Ok(n as i64)
    }
}
fn serial(year: i64, month: i64, day: i64, system: DateSystem) -> Option<i64> {
    if !(0..10000).contains(&year) {
        return None;
    }
    let year = if year < 1900 { year + 1900 } else { year };
    let month = month.checked_sub(1)?;
    let year = year.checked_add(month.div_euclid(12))?;
    let month = month.rem_euclid(12) as usize;
    let ordinal = year_start(year)?
        .checked_add(MONTH_START[month])?
        .checked_add(i64::from(month >= 2 && leap(year)))?;
    let first = match system {
        DateSystem::Excel1900 => ordinal
            .checked_sub(BASE_1900)?
            .checked_add(1)?
            .checked_add(i64::from(ordinal >= BASE_1900 + 59))?,
        DateSystem::Excel1904 => ordinal.checked_sub(BASE_1904)?,
    };
    let value = first.checked_add(day.checked_sub(1)?)?;
    (0..=maximum(system)).contains(&value).then_some(value)
}
fn parts(
    serial: i64,
    system: DateSystem,
    budget: &mut EvaluationBudget,
) -> Result<(i64, i64, i64), EvaluationError> {
    if system == DateSystem::Excel1900 {
        if serial == 0 {
            return Ok((1900, 1, 0));
        }
        if serial == 60 {
            return Ok((1900, 2, 29));
        }
    }
    let ordinal = match system {
        DateSystem::Excel1900 => BASE_1900 + serial - if serial < 60 { 1 } else { 2 },
        DateSystem::Excel1904 => BASE_1904 + serial,
    };
    let (mut low, mut high) = (1900, 10000);
    while low + 1 < high {
        budget.charge()?;
        let mid = low + (high - low) / 2;
        // Search years are bounded; failure would be an internal contract violation.
        let start = year_start(mid).ok_or(EvaluationError::InvalidAst)?;
        if start <= ordinal {
            low = mid;
        } else {
            high = mid;
        }
    }
    let mut day = ordinal - year_start(low).ok_or(EvaluationError::InvalidAst)?;
    let lengths = [
        31,
        if leap(low) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for (month, length) in lengths.into_iter().enumerate() {
        budget.charge()?;
        if day < length {
            return Ok((low, month as i64 + 1, day + 1));
        }
        day -= length;
    }
    Err(EvaluationError::InvalidAst)
}
fn scalar(
    value: Value,
    budget: &mut EvaluationBudget,
) -> Result<Result<f64, FormulaError>, EvaluationError> {
    if let Value::Text(input) = &value {
        if input.len() > MAX_SOURCE_BYTES {
            return Err(EvaluationError::CalculationLimitExceeded);
        }
        for _ in input.chars() {
            budget.charge()?;
        }
    }
    Ok(numeric(value))
}
pub(crate) fn call(
    ast: &Ast,
    name: &str,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    let arity = if name == "DATE" { 3 } else { 1 };
    if args.len() != arity {
        return Ok(Value::Error(FormulaError::Value));
    }
    let mut values = [0i64; 3];
    let mut system = resolver.date_system();
    for (index, arg) in args.iter().enumerate() {
        let value = eval(ast, *arg, resolver, budget, depth)?;
        if name != "DATE"
            && let Value::DateTime {
                system: supplied, ..
            } = &value
        {
            system = *supplied;
        }
        let n = match scalar(value, budget)? {
            Ok(n) => n,
            Err(e) => return Ok(Value::Error(e)),
        };
        // Fractions on the final supported date remain valid, negative serials do not.
        if name != "DATE" && (n < 0.0 || n >= (maximum(system) + 1) as f64) {
            return Ok(Value::Error(FormulaError::Num));
        }
        values[index] = match integer(n) {
            Ok(n) => n,
            Err(e) => return Ok(Value::Error(e)),
        };
    }
    budget.charge()?;
    if name == "DATE" {
        return Ok(match serial(values[0], values[1], values[2], system) {
            Some(n) => Value::Number(n as f64),
            None => Value::Error(FormulaError::Num),
        });
    }
    let (year, month, day) = parts(values[0], system, budget)?;
    Ok(Value::Number(match name {
        "YEAR" => year,
        "MONTH" => month,
        "DAY" => day,
        _ => return Err(EvaluationError::InvalidAst),
    } as f64))
}
