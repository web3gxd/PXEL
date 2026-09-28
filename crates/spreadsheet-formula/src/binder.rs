//! Server-side binding against an explicit axis projection, including its version.
//! Callers must supply the client's base projection, never silently substitute head.
use crate::{ast as syntax, canonical, parse};
use spreadsheet_domain::{
    formula as bound,
    workbook::{CellAddress, Workbook},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BindingError {
    #[error(transparent)]
    Syntax(#[from] syntax::ParseError),
    #[error(transparent)]
    Invalid(#[from] bound::FormulaValidationError),
    #[error("formula origin does not exist in the supplied axis projection")]
    MissingOrigin,
    #[error("formula range exceeds configured limits")]
    RangeLimit,
}

fn id(id: syntax::NodeId) -> Result<bound::NodeId, BindingError> {
    Ok(bound::NodeId(u32::try_from(id.0).map_err(|_| {
        bound::FormulaValidationError::LimitExceeded
    })?))
}

pub fn bind(
    source: &str,
    axes: &Workbook,
    origin: CellAddress,
) -> Result<bound::Formula, BindingError> {
    let origin_sheet = axes
        .sheets()
        .iter()
        .find(|sheet| sheet.id() == origin.sheet)
        .ok_or(BindingError::MissingOrigin)?;
    let origin_row = origin_sheet
        .row_position(origin.row)
        .ok_or(BindingError::MissingOrigin)?;
    let origin_column = origin_sheet
        .column_position(origin.column)
        .ok_or(BindingError::MissingOrigin)?;
    let diagnostic = parse(source)?;
    let text = canonical(&diagnostic.ast)?;
    let reference = |reference: &syntax::CellReference| -> Result<bound::BoundCellReference, bound::ReferenceError> {
        let sheet = if let Some(name) = &reference.sheet {
            axes.sheets().iter().find(|sheet| sheet.name().to_lowercase() == name.to_lowercase())
                .ok_or(bound::ReferenceError::MissingSheet)?
        } else { origin_sheet };
        let row = sheet.rows().get(reference.row as usize).copied().ok_or(bound::ReferenceError::OutOfBounds)?;
        let column = sheet.columns().get(reference.column as usize).copied().ok_or(bound::ReferenceError::OutOfBounds)?;
        Ok(bound::BoundCellReference {
            target_sheet: sheet.id(), target_row: row, target_column: column,
            row_mode: if reference.row_absolute { bound::AxisReferenceMode::Absolute }
                else { bound::AxisReferenceMode::Relative { original_delta: i64::from(reference.row) - i64::from(origin_row) } },
            column_mode: if reference.column_absolute { bound::AxisReferenceMode::Absolute }
                else { bound::AxisReferenceMode::Relative { original_delta: i64::from(reference.column) - i64::from(origin_column) } },
        })
    };
    let mut nodes = Vec::with_capacity(diagnostic.ast.nodes.len());
    for node in &diagnostic.ast.nodes {
        nodes.push(match node {
            syntax::Node::Number(value) => bound::Node::Number(*value),
            syntax::Node::Text(value) => bound::Node::Text(value.clone()),
            syntax::Node::Boolean(value) => bound::Node::Boolean(*value),
            syntax::Node::Missing => bound::Node::Missing,
            syntax::Node::Error(error) => bound::Node::Error(match error {
                syntax::FormulaError::Null => bound::FormulaError::Null,
                syntax::FormulaError::Div0 => bound::FormulaError::Div0,
                syntax::FormulaError::Value => bound::FormulaError::Value,
                syntax::FormulaError::Ref => bound::FormulaError::Ref,
                syntax::FormulaError::Name => bound::FormulaError::Name,
                syntax::FormulaError::Num => bound::FormulaError::Num,
                syntax::FormulaError::Na => bound::FormulaError::Na,
                syntax::FormulaError::Spill => bound::FormulaError::Spill,
                syntax::FormulaError::Cycle => bound::FormulaError::Cycle,
            }),
            syntax::Node::Cell(cell) => match reference(cell) {
                Ok(cell) => bound::Node::Cell(cell),
                Err(error) => bound::Node::InvalidReference(error),
            },
            syntax::Node::Range(range) => {
                if range.cell_count() > syntax::MAX_RANGE_CELLS {
                    return Err(BindingError::RangeLimit);
                }
                match (reference(&range.start), reference(&range.end)) {
                    (Ok(start), Ok(end)) => {
                        bound::Node::Range(bound::BoundRangeReference { start, end })
                    }
                    (Err(error), _) | (_, Err(error)) => bound::Node::InvalidReference(error),
                }
            }
            syntax::Node::Unary { op, operand } => bound::Node::Unary {
                op: match op {
                    syntax::UnaryOp::Plus => bound::UnaryOp::Plus,
                    syntax::UnaryOp::Minus => bound::UnaryOp::Minus,
                    syntax::UnaryOp::Percent => bound::UnaryOp::Percent,
                },
                operand: id(*operand)?,
            },
            syntax::Node::Binary { op, left, right } => bound::Node::Binary {
                op: match op {
                    syntax::BinaryOp::Add => bound::BinaryOp::Add,
                    syntax::BinaryOp::Subtract => bound::BinaryOp::Subtract,
                    syntax::BinaryOp::Multiply => bound::BinaryOp::Multiply,
                    syntax::BinaryOp::Divide => bound::BinaryOp::Divide,
                    syntax::BinaryOp::Power => bound::BinaryOp::Power,
                    syntax::BinaryOp::Concat => bound::BinaryOp::Concat,
                    syntax::BinaryOp::Equal => bound::BinaryOp::Equal,
                    syntax::BinaryOp::NotEqual => bound::BinaryOp::NotEqual,
                    syntax::BinaryOp::Less => bound::BinaryOp::Less,
                    syntax::BinaryOp::LessEqual => bound::BinaryOp::LessEqual,
                    syntax::BinaryOp::Greater => bound::BinaryOp::Greater,
                    syntax::BinaryOp::GreaterEqual => bound::BinaryOp::GreaterEqual,
                },
                left: id(*left)?,
                right: id(*right)?,
            },
            syntax::Node::Call { name, arguments } => bound::Node::Call {
                name: bound::FunctionName::new(name.clone())?,
                arguments: arguments
                    .iter()
                    .copied()
                    .map(id)
                    .collect::<Result<_, _>>()?,
            },
        });
    }
    let ast = bound::BoundAst::new(bound::AST_SCHEMA_VERSION, id(diagnostic.ast.root)?, nodes)?;
    Ok(bound::Formula::new(
        source.to_owned(),
        text,
        ast,
        origin,
        axes.version(),
    )?)
}
