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
    fn charge(&mut self) -> Result<(), EvaluationError> {
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
fn number(n: f64) -> Value {
    if !n.is_finite() {
        Value::Error(FormulaError::Num)
    } else {
        Value::Number(if n == 0.0 { 0.0 } else { n })
    }
}
fn numeric(value: Value) -> Result<f64, FormulaError> {
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
fn text(value: Value) -> Result<String, FormulaError> {
    match value {
        Value::Blank => Ok(String::new()),
        Value::Number(n) if n.is_finite() => Ok(if n == 0.0 { "0".into() } else { n.to_string() }),
        Value::Text(s) => Ok(s),
        Value::Boolean(b) => Ok(if b { "TRUE".into() } else { "FALSE".into() }),
        Value::Error(e) => Err(e),
        _ => Err(FormulaError::Value),
    }
}
fn eval(
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
            binary(*op, left, right)?
        }
        Node::Call { name, arguments } => {
            if name == "IF" {
                conditional(ast, arguments, resolver, budget, depth + 1)?
            } else if matches!(name.as_str(), "SUM" | "AVERAGE" | "MIN" | "MAX") {
                aggregate(ast, name, arguments, resolver, budget, depth + 1)?
            } else {
                Value::Error(FormulaError::Name)
            }
        }
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
fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, EvaluationError> {
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
            if b.fract() != 0.0 || b.abs() > 1024.0 {
                return Err(EvaluationError::UnsupportedCompatibilityFeature);
            }
            if a == 0.0 && b == 0.0 {
                return Ok(Value::Error(FormulaError::Num));
            }
            if a == 0.0 && b < 0.0 {
                return Ok(Value::Error(FormulaError::Div0));
            }
            let mut exponent = b.abs() as u32;
            let mut base = if b < 0.0 { 1.0 / a } else { a };
            if !base.is_finite() {
                return Ok(Value::Error(FormulaError::Num));
            }
            let mut result = 1.0;
            while exponent > 0 {
                if exponent & 1 != 0 {
                    result *= base;
                }
                exponent >>= 1;
                if exponent > 0 {
                    base *= base;
                }
            }
            number(result)
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
