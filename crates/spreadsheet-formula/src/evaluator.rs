use crate::ast::*;
use std::cmp::Ordering;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum EvaluationError {
    #[error("calculation work limit exceeded")]
    CalculationLimitExceeded,
    #[error("calculation cancelled")]
    Cancelled,
    #[error("unsupported compatibility feature")]
    UnsupportedCompatibilityFeature,
    #[error("invalid formula AST")]
    InvalidAst,
    #[error("reference resolver violated its contract")]
    InvalidResolverResult,
}
pub struct EvaluationBudget {
    remaining: u64,
    cancelled: bool,
}
impl Default for EvaluationBudget {
    fn default() -> Self {
        Self::new(1_000_000)
    }
}
impl EvaluationBudget {
    pub fn new(units: u64) -> Self {
        Self {
            remaining: units.min(1_000_000),
            cancelled: false,
        }
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
    pub const fn remaining(&self) -> u64 {
        self.remaining
    }
    pub(crate) fn charge(&mut self) -> Result<(), EvaluationError> {
        if self.cancelled {
            return Err(EvaluationError::Cancelled);
        }
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(EvaluationError::CalculationLimitExceeded)?;
        Ok(())
    }
}
/// Trusted adapter must visit all addressed cells in row-major order, including
/// Blank cells, and immediately propagate visitor errors without more work.
pub trait ReferenceResolver {
    /// Persisted calculation inputs supplied by a trusted workbook adapter.
    fn evaluation_context(&self) -> Option<crate::EvaluationContext<'_>> {
        None
    }
    /// Calendar profile supplied by the trusted adapter, never the host locale.
    fn date_system(&self) -> DateSystem {
        DateSystem::Excel1900
    }
    fn resolve_cell(&self, reference: &CellReference) -> Value;
    fn visit_range(
        &self,
        reference: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError>;
}
pub struct NoReferences;
impl ReferenceResolver for NoReferences {
    fn resolve_cell(&self, _: &CellReference) -> Value {
        Value::Error(FormulaError::Ref)
    }
    fn visit_range(
        &self,
        _: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        visitor(Value::Error(FormulaError::Ref))
    }
}
pub fn evaluate(
    ast: &Ast,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    if ast.schema_version != AST_SCHEMA_VERSION || ast.nodes.len() > MAX_NODES {
        return Err(EvaluationError::InvalidAst);
    }
    eval(ast, ast.root, resolver, budget, 0)
}
pub(crate) fn number(n: f64) -> Value {
    if !n.is_finite() {
        Value::Error(FormulaError::Num)
    } else {
        Value::Number(if n == 0.0 { 0.0 } else { n })
    }
}
pub(crate) fn numeric(value: Value) -> Result<f64, FormulaError> {
    match value {
        Value::Blank => Ok(0.0),
        Value::Number(n) | Value::DateTime { serial: n, .. } => {
            if n.is_finite() {
                Ok(n)
            } else {
                Err(FormulaError::Num)
            }
        }
        Value::Boolean(b) => Ok(f64::from(u8::from(b))),
        Value::Text(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or(FormulaError::Value),
        Value::Error(e) => Err(e),
        Value::Array { .. } => Err(FormulaError::Value),
    }
}
fn logical(value: Value) -> Result<bool, FormulaError> {
    match value {
        Value::Blank => Ok(false),
        Value::Boolean(b) => Ok(b),
        Value::Number(n) if n.is_finite() => Ok(n != 0.0),
        Value::Text(s) if s.eq_ignore_ascii_case("TRUE") => Ok(true),
        Value::Text(s) if s.eq_ignore_ascii_case("FALSE") => Ok(false),
        Value::Error(e) => Err(e),
        _ => Err(FormulaError::Value),
    }
}
pub(crate) fn text(value: Value) -> Result<String, FormulaError> {
    match value {
        Value::Blank => Ok(String::new()),
        Value::Number(n) if n.is_finite() => Ok(if n == 0.0 { "0".into() } else { n.to_string() }),
        Value::Text(s) => Ok(s),
        Value::Boolean(b) => Ok(if b { "TRUE".into() } else { "FALSE".into() }),
        Value::Error(e) => Err(e),
        _ => Err(FormulaError::Value),
    }
}
pub(crate) fn eval(
    ast: &Ast,
    id: NodeId,
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.charge()?;
    if depth > MAX_DEPTH {
        return Err(EvaluationError::CalculationLimitExceeded);
    }
    let node = ast.nodes.get(id.0).ok_or(EvaluationError::InvalidAst)?;
    Ok(match node {
        Node::Number(n) => number(*n),
        Node::Text(s) => {
            if s.len() > MAX_SOURCE_BYTES {
                return Err(EvaluationError::CalculationLimitExceeded);
            }
            Value::Text(s.clone())
        }
        Node::Boolean(b) => Value::Boolean(*b),
        Node::Error(e) => Value::Error(*e),
        Node::InvalidReference => Value::Error(FormulaError::Ref),
        Node::Missing => Value::Blank,
        Node::Cell(reference) => {
            budget.charge()?;
            if reference.row >= 1_048_576 || reference.column >= 16_384 {
                return Err(EvaluationError::InvalidAst);
            }
            match resolver.resolve_cell(reference) {
                Value::Number(n) => number(n),
                value => value,
            }
        }
        Node::Range(_) => Value::Error(FormulaError::Value),
        Node::Unary { op, operand } => {
            match numeric(eval(ast, *operand, resolver, budget, depth + 1)?) {
                Ok(n) => number(match op {
                    UnaryOp::Plus => n,
                    UnaryOp::Minus => -n,
                    UnaryOp::Percent => n / 100.0,
                }),
                Err(e) => Value::Error(e),
            }
        }
        Node::Binary { op, left, right } => {
            let left = eval(ast, *left, resolver, budget, depth + 1)?;
            if matches!(left, Value::Error(_)) {
                return Ok(left);
            }
            let right = eval(ast, *right, resolver, budget, depth + 1)?;
            if matches!(right, Value::Error(_)) {
                return Ok(right);
            }
            binary(*op, left, right, budget)?
        }
        Node::Call { name, arguments } => {
            if name == "MATCH" {
                crate::lookup::call(ast, arguments, resolver, budget, depth + 1)?
            } else if name == "INDEX" {
                crate::index::call(ast, arguments, resolver, budget, depth + 1)?
            } else if matches!(
                name.as_str(),
                "CONCAT" | "LEFT" | "RIGHT" | "MID" | "LEN" | "TRIM" | "LOWER" | "UPPER"
            ) {
                crate::text::call(ast, name, arguments, resolver, budget, depth + 1)?
            } else if matches!(name.as_str(), "NOW" | "TODAY" | "RAND") {
                crate::volatile::call(name, arguments, id, resolver, budget)?
            } else if matches!(name.as_str(), "DATE" | "YEAR" | "MONTH" | "DAY") {
                crate::calendar::call(ast, name, arguments, resolver, budget, depth + 1)?
            } else if name == "IF" {
                conditional(ast, arguments, resolver, budget, depth + 1)?
            } else if name == "IFS" {
                conditions(ast, arguments, resolver, budget, depth + 1)?
            } else if name == "IFERROR" {
                if_error(ast, arguments, resolver, budget, depth + 1)?
            } else if matches!(name.as_str(), "AND" | "OR") {
                logical_aggregate(ast, name == "AND", arguments, resolver, budget, depth + 1)?
            } else if name == "NOT" {
                if arguments.len() != 1 {
                    Value::Error(FormulaError::Value)
                } else {
                    match logical(eval(ast, arguments[0], resolver, budget, depth + 1)?) {
                        Ok(value) => Value::Boolean(!value),
                        Err(error) => Value::Error(error),
                    }
                }
            } else if matches!(
                name.as_str(),
                "ABS" | "MOD" | "ROUND" | "ROUNDUP" | "ROUNDDOWN" | "POWER"
            ) {
                scalar_math(ast, name, arguments, resolver, budget, depth + 1)?
            } else if matches!(name.as_str(), "SUM" | "AVERAGE" | "MIN" | "MAX") {
                aggregate(ast, name, arguments, resolver, budget, depth + 1)?
            } else if matches!(name.as_str(), "COUNT" | "COUNTA") {
                count(
                    ast,
                    name == "COUNTA",
                    arguments,
                    resolver,
                    budget,
                    depth + 1,
                )?
            } else {
                Value::Error(FormulaError::Name)
            }
        }
    })
}
fn scalar_math(
    ast: &Ast,
    name: &str,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    if args.len() != if name == "ABS" { 1 } else { 2 } {
        return Ok(Value::Error(FormulaError::Value));
    }
    let value = match numeric(eval(ast, args[0], resolver, budget, depth)?) {
        Ok(value) => value,
        Err(error) => return Ok(Value::Error(error)),
    };
    if name == "ABS" {
        return Ok(number(value.abs()));
    }
    let divisor = match numeric(eval(ast, args[1], resolver, budget, depth)?) {
        Ok(value) => value,
        Err(error) => return Ok(Value::Error(error)),
    };
    if name == "POWER" {
        return crate::power::power(value, divisor, budget);
    }
    let mode = match name {
        "ROUND" => Some(crate::rounding::Mode::Nearest),
        "ROUNDUP" => Some(crate::rounding::Mode::Away),
        "ROUNDDOWN" => Some(crate::rounding::Mode::Toward),
        _ => None,
    };
    if let Some(mode) = mode {
        return crate::rounding::round(value, divisor, mode, budget);
    }
    if divisor == 0.0 {
        return Ok(Value::Error(FormulaError::Div0));
    }
    let quotient = value / divisor;
    if !quotient.is_finite() {
        return Ok(Value::Error(FormulaError::Num));
    }
    // Fixed binary64 order from the profile's n - d * floor(n/d) definition.
    // No platform remainder function, fused operation or epsilon correction.
    let result = value - divisor * quotient.floor();
    if result != 0.0
        && (result.is_sign_negative() != divisor.is_sign_negative()
            || result.abs() >= divisor.abs())
    {
        return Ok(Value::Error(FormulaError::Num));
    }
    Ok(number(result))
}

fn logical_aggregate(
    ast: &Ast,
    all: bool,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    if args.is_empty() || args.len() > 255 {
        return Ok(Value::Error(FormulaError::Value));
    }
    let mut result = all;
    let mut seen = false;
    let mut error = None;
    let mut add = |value: Value, reference: bool| -> Result<(), EvaluationError> {
        if matches!(value, Value::Array { .. }) {
            return Err(EvaluationError::UnsupportedCompatibilityFeature);
        }
        if reference && matches!(value, Value::Blank | Value::Text(_)) {
            return Ok(());
        }
        match logical(value) {
            Ok(value) => {
                seen = true;
                // Do not skip later arguments after a decisive boolean: their
                // spreadsheet errors and resource failures still matter.
                result = if all { result & value } else { result | value };
            }
            Err(value) => {
                if error.is_none() {
                    error = Some(value);
                }
            }
        }
        Ok(())
    };
    for arg in args {
        if crate::index::visit(ast, *arg, resolver, budget, depth, &mut add)? {
            continue;
        }
        match ast.nodes.get(arg.0).ok_or(EvaluationError::InvalidAst)? {
            Node::Range(range) => {
                budget.charge()?;
                if range.start.sheet != range.end.sheet || range.cell_count() > MAX_RANGE_CELLS {
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
                    add(value, true)
                })?;
                if visits != range.cell_count() {
                    return Err(EvaluationError::InvalidResolverResult);
                }
            }
            Node::Cell(_) => add(eval(ast, *arg, resolver, budget, depth)?, true)?,
            _ => add(eval(ast, *arg, resolver, budget, depth)?, false)?,
        }
    }
    Ok(if let Some(error) = error {
        Value::Error(error)
    } else if seen {
        Value::Boolean(result)
    } else {
        Value::Error(FormulaError::Value)
    })
}
fn conditions(
    ast: &Ast,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    let (pairs, remainder) = args.as_chunks::<2>();
    if args.len() < 2 || args.len() > 254 || !remainder.is_empty() {
        return Ok(Value::Error(FormulaError::Value));
    }
    for pair in pairs {
        let condition = match eval(ast, pair[0], resolver, budget, depth)? {
            Value::Boolean(value) => value,
            Value::Error(error) => return Ok(Value::Error(error)),
            _ => return Ok(Value::Error(FormulaError::Value)),
        };
        if condition {
            let value = eval(ast, pair[1], resolver, budget, depth)?;
            return Ok(if value == Value::Blank {
                Value::Number(0.0)
            } else {
                value
            });
        }
    }
    Ok(Value::Error(FormulaError::Na))
}
fn if_error(
    ast: &Ast,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    let [value, fallback] = args else {
        return Ok(Value::Error(FormulaError::Value));
    };
    let value = eval(ast, *value, resolver, budget, depth)?;
    let chosen = if matches!(value, Value::Error(_)) {
        eval(ast, *fallback, resolver, budget, depth)?
    } else {
        value
    };
    Ok(if chosen == Value::Blank {
        Value::Text(String::new())
    } else {
        chosen
    })
}
fn count(
    ast: &Ast,
    counta: bool,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    if args.is_empty() || args.len() > 255 {
        return Ok(Value::Error(FormulaError::Value));
    }
    let mut total = 0u64;
    for arg in args {
        if crate::index::visit(
            ast,
            *arg,
            resolver,
            budget,
            depth,
            &mut |value, reference| {
                if countable(&value, counta, reference)? {
                    total += 1;
                }
                Ok(())
            },
        )? {
            continue;
        }
        match ast.nodes.get(arg.0).ok_or(EvaluationError::InvalidAst)? {
            Node::InvalidReference => return Ok(Value::Error(FormulaError::Ref)),
            Node::Range(range) => {
                budget.charge()?;
                if range.start.sheet != range.end.sheet || range.cell_count() > MAX_RANGE_CELLS {
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
                    visits = visits.saturating_add(1);
                    if visits > range.cell_count() {
                        return Err(EvaluationError::InvalidResolverResult);
                    }
                    if countable(&value, counta, true)? {
                        total += 1;
                    }
                    Ok(())
                })?;
                if visits != range.cell_count() {
                    return Err(EvaluationError::InvalidResolverResult);
                }
            }
            Node::Cell(_) => {
                let value = eval(ast, *arg, resolver, budget, depth)?;
                if countable(&value, counta, true)? {
                    total += 1;
                }
            }
            _ => {
                let value = eval(ast, *arg, resolver, budget, depth)?;
                if countable(&value, counta, false)? {
                    total += 1;
                }
            }
        }
    }
    Ok(Value::Number(total as f64))
}
fn countable(value: &Value, counta: bool, reference: bool) -> Result<bool, EvaluationError> {
    Ok(match value {
        Value::Blank => false,
        Value::Number(_) | Value::DateTime { .. } => true,
        Value::Text(text) => {
            counta || (!reference && text.trim().parse::<f64>().is_ok_and(f64::is_finite))
        }
        Value::Boolean(_) => counta || !reference,
        Value::Error(_) => counta,
        Value::Array { .. } => return Err(EvaluationError::UnsupportedCompatibilityFeature),
    })
}
fn conditional(
    ast: &Ast,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    if !(2..=3).contains(&args.len()) {
        return Ok(Value::Error(FormulaError::Value));
    }
    let test = match logical(eval(ast, args[0], resolver, budget, depth)?) {
        Ok(b) => b,
        Err(e) => return Ok(Value::Error(e)),
    };
    let chosen = if test { args.get(1) } else { args.get(2) };
    let Some(chosen) = chosen else {
        return Ok(Value::Boolean(false));
    };
    let result = eval(ast, *chosen, resolver, budget, depth)?;
    Ok(if result == Value::Blank {
        Value::Number(0.0)
    } else {
        result
    })
}
fn aggregate(
    ast: &Ast,
    name: &str,
    args: &[NodeId],
    resolver: &impl ReferenceResolver,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    if args.is_empty() || args.len() > 255 {
        return Ok(Value::Error(FormulaError::Value));
    }
    let mut state = Aggregate {
        count: 0,
        value: 0.0,
        error: None,
    };
    for arg in args {
        if crate::index::visit(
            ast,
            *arg,
            resolver,
            budget,
            depth,
            &mut |value, reference| {
                state.add(value, reference, name);
                Ok(())
            },
        )? {
            if let Some(error) = state.error {
                return Ok(Value::Error(error));
            }
            continue;
        }
        match ast.nodes.get(arg.0).ok_or(EvaluationError::InvalidAst)? {
            Node::Range(range) => {
                budget.charge()?;
                if range.start.sheet != range.end.sheet || range.cell_count() > MAX_RANGE_CELLS {
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
                let result = resolver.visit_range(range, &mut |value| {
                    budget.charge()?;
                    visits += 1;
                    if visits > range.cell_count() {
                        return Err(EvaluationError::InvalidResolverResult);
                    }
                    state.add(value, true, name);
                    Ok(())
                });
                result?;
                if let Some(error) = state.error {
                    return Ok(Value::Error(error));
                }
                if visits != range.cell_count() {
                    return Err(EvaluationError::InvalidResolverResult);
                }
            }
            Node::Cell(_) => state.add(eval(ast, *arg, resolver, budget, depth)?, true, name),
            _ => state.add(eval(ast, *arg, resolver, budget, depth)?, false, name),
        }
        if let Some(error) = state.error {
            return Ok(Value::Error(error));
        }
    }
    Ok(if name == "AVERAGE" {
        if state.count == 0 {
            Value::Error(FormulaError::Div0)
        } else {
            number(state.value / state.count as f64)
        }
    } else {
        number(state.value)
    })
}
struct Aggregate {
    count: u64,
    value: f64,
    error: Option<FormulaError>,
}
impl Aggregate {
    fn add(&mut self, value: Value, reference: bool, name: &str) {
        if self.error.is_some() {
            return;
        }
        if reference && matches!(value, Value::Blank | Value::Text(_) | Value::Boolean(_)) {
            return;
        }
        match numeric(value) {
            Err(error) => self.error = Some(error),
            Ok(n) => {
                self.value = match name {
                    "MIN" if self.count > 0 => self.value.min(n),
                    "MAX" if self.count > 0 => self.value.max(n),
                    "MIN" | "MAX" => n,
                    _ => self.value + n,
                };
                self.count += 1;
                if !self.value.is_finite() {
                    self.error = Some(FormulaError::Num);
                }
            }
        }
    }
}
fn binary(
    op: BinaryOp,
    left: Value,
    right: Value,
    budget: &mut EvaluationBudget,
) -> Result<Value, EvaluationError> {
    if op == BinaryOp::Concat {
        return Ok(match (text(left), text(right)) {
            (Ok(a), Ok(b)) => {
                if a.len().saturating_add(b.len()) > MAX_SOURCE_BYTES {
                    return Err(EvaluationError::CalculationLimitExceeded);
                }
                Value::Text(a + &b)
            }
            (Err(e), _) | (_, Err(e)) => Value::Error(e),
        });
    }
    if matches!(
        op,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
    ) {
        return Ok(match compare(left, right) {
            Ok(order) => Value::Boolean(match op {
                BinaryOp::Equal => order == Ordering::Equal,
                BinaryOp::NotEqual => order != Ordering::Equal,
                BinaryOp::Less => order == Ordering::Less,
                BinaryOp::LessEqual => order != Ordering::Greater,
                BinaryOp::Greater => order == Ordering::Greater,
                BinaryOp::GreaterEqual => order != Ordering::Less,
                _ => false,
            }),
            Err(e) => Value::Error(e),
        });
    }
    let (a, b) = match (numeric(left), numeric(right)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Ok(Value::Error(e)),
    };
    Ok(match op {
        BinaryOp::Add => number(a + b),
        BinaryOp::Subtract => number(a - b),
        BinaryOp::Multiply => number(a * b),
        BinaryOp::Divide => {
            if b == 0.0 {
                Value::Error(FormulaError::Div0)
            } else {
                number(a / b)
            }
        }
        BinaryOp::Power => {
            return crate::power::power(a, b, budget);
        }
        _ => return Err(EvaluationError::InvalidAst),
    })
}
fn compare(mut left: Value, mut right: Value) -> Result<Ordering, FormulaError> {
    if left == Value::Blank {
        left = match right {
            Value::Text(_) => Value::Text(String::new()),
            Value::Boolean(_) => Value::Boolean(false),
            _ => Value::Number(0.0),
        };
    }
    if right == Value::Blank {
        right = match left {
            Value::Text(_) => Value::Text(String::new()),
            Value::Boolean(_) => Value::Boolean(false),
            _ => Value::Number(0.0),
        };
    }
    match (&left, &right) {
        (Value::Number(a), Value::Number(b)) => a.partial_cmp(b).ok_or(FormulaError::Num),
        (Value::Text(a), Value::Text(b)) => Ok(a.to_uppercase().cmp(&b.to_uppercase())),
        (Value::Boolean(a), Value::Boolean(b)) => Ok(a.cmp(b)),
        (Value::Error(e), _) | (_, Value::Error(e)) => Err(*e),
        _ => {
            let rank = |v: &Value| match v {
                Value::Number(_) => Some(0),
                Value::Text(_) => Some(1),
                Value::Boolean(_) => Some(2),
                _ => None,
            };
            Ok(rank(&left)
                .ok_or(FormulaError::Value)?
                .cmp(&rank(&right).ok_or(FormulaError::Value)?))
        }
    }
}
