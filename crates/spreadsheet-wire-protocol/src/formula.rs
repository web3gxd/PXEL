//! Public wire formula DTO adapter. Independent of the durable storage codec.
use crate::v1;
use spreadsheet_domain::workbook::CellAddress;
use thiserror::Error;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormulaCodecError {
    #[error("unsupported wire formula schema")]
    UnsupportedSchema,
    #[error("invalid wire formula")]
    Invalid,
}
use FormulaCodecError as CodecError;
fn id<const N: usize>(bytes: &[u8]) -> Result<[u8; N], CodecError> {
    bytes.try_into().map_err(|_| CodecError::Invalid)
}
fn addr(a: &CellAddress) -> v1::CellAddress {
    v1::CellAddress {
        sheet_id: a.sheet.as_bytes().to_vec(),
        row_id: a.row.as_bytes().to_vec(),
        column_id: a.column.as_bytes().to_vec(),
    }
}
fn decode_addr(a: v1::CellAddress) -> Result<CellAddress, CodecError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes(id(&a.sheet_id)?).map_err(|_| CodecError::Invalid)?,
        row: RowId::from_bytes(id(&a.row_id)?).map_err(|_| CodecError::Invalid)?,
        column: ColumnId::from_bytes(id(&a.column_id)?).map_err(|_| CodecError::Invalid)?,
    })
}
use spreadsheet_domain::{
    formula as d,
    workbook::{ColumnId, DocumentVersion, RowId, SheetId},
};
fn invalid(error: d::FormulaValidationError) -> CodecError {
    match error {
        d::FormulaValidationError::UnsupportedSchema => CodecError::UnsupportedSchema,
        _ => CodecError::Invalid,
    }
}
macro_rules! enum_mapping {
 ($encode:ident,$decode:ident,$kind:ident,{$($variant:ident=$number:literal),+$(,)?})=>{
  fn $encode(value:d::$kind)->i32{match value{$(d::$kind::$variant=>$number),+}}
  fn $decode(value:i32)->Result<d::$kind,CodecError>{match value{$($number=>Ok(d::$kind::$variant)),+,_=>Err(CodecError::UnsupportedSchema)}}
 };
}
enum_mapping!(error,decode_error,FormulaError,{Null=1,Div0=2,Value=3,Ref=4,Name=5,Num=6,Na=7,Spill=8,Cycle=9});
enum_mapping!(reference_error,decode_reference_error,ReferenceError,{DeletedTarget=1,OutOfBounds=2,MissingSheet=3});
enum_mapping!(unary,decode_unary,UnaryOp,{Plus=1,Minus=2,Percent=3});
enum_mapping!(binary,decode_binary,BinaryOp,{Add=1,Subtract=2,Multiply=3,Divide=4,Power=5,Concat=6,Equal=7,NotEqual=8,Less=9,LessEqual=10,Greater=11,GreaterEqual=12});
fn mode(value: d::AxisReferenceMode) -> v1::AxisReferenceMode {
    v1::AxisReferenceMode {
        kind: Some(match value {
            d::AxisReferenceMode::Absolute => v1::axis_reference_mode::Kind::Absolute(true),
            d::AxisReferenceMode::Relative { original_delta } => {
                v1::axis_reference_mode::Kind::RelativeDelta(original_delta)
            }
        }),
    }
}
fn decode_mode(value: Option<v1::AxisReferenceMode>) -> Result<d::AxisReferenceMode, CodecError> {
    match value.and_then(|v| v.kind) {
        Some(v1::axis_reference_mode::Kind::Absolute(true)) => Ok(d::AxisReferenceMode::Absolute),
        Some(v1::axis_reference_mode::Kind::RelativeDelta(original_delta)) => {
            Ok(d::AxisReferenceMode::Relative { original_delta })
        }
        _ => Err(CodecError::Invalid),
    }
}
fn reference(value: &d::BoundCellReference) -> v1::BoundCellReference {
    v1::BoundCellReference {
        sheet_id: value.target_sheet.as_bytes().to_vec(),
        row_id: value.target_row.as_bytes().to_vec(),
        column_id: value.target_column.as_bytes().to_vec(),
        row_mode: Some(mode(value.row_mode)),
        column_mode: Some(mode(value.column_mode)),
    }
}
fn decode_reference(value: v1::BoundCellReference) -> Result<d::BoundCellReference, CodecError> {
    let result = d::BoundCellReference {
        target_sheet: SheetId::from_bytes(id(&value.sheet_id)?).map_err(|_| CodecError::Invalid)?,
        target_row: RowId::from_bytes(id(&value.row_id)?).map_err(|_| CodecError::Invalid)?,
        target_column: ColumnId::from_bytes(id(&value.column_id)?)
            .map_err(|_| CodecError::Invalid)?,
        row_mode: decode_mode(value.row_mode)?,
        column_mode: decode_mode(value.column_mode)?,
    };
    result.validate().map_err(invalid)?;
    Ok(result)
}
fn node(value: &d::Node) -> v1::FormulaNode {
    use v1::formula_node::Kind;
    v1::FormulaNode {
        kind: Some(match value {
            d::Node::Number(n) => Kind::Number(*n),
            d::Node::Text(t) => Kind::Text(t.clone()),
            d::Node::Boolean(b) => Kind::Boolean(*b),
            d::Node::Error(e) => Kind::Error(error(*e)),
            d::Node::Missing => Kind::Missing(true),
            d::Node::Cell(r) => Kind::Cell(reference(r)),
            d::Node::Range(r) => Kind::Range(v1::BoundRangeReference {
                start: Some(reference(&r.start)),
                end: Some(reference(&r.end)),
            }),
            d::Node::InvalidReference(e) => Kind::InvalidReference(reference_error(*e)),
            d::Node::Unary { op, operand } => Kind::Unary(v1::UnaryNode {
                operator: unary(*op),
                operand: operand.0,
            }),
            d::Node::Binary { op, left, right } => Kind::Binary(v1::BinaryNode {
                operator: binary(*op),
                left: left.0,
                right: right.0,
            }),
            d::Node::Call { name, arguments } => Kind::Call(v1::CallNode {
                name: name.as_str().to_owned(),
                arguments: arguments.iter().map(|id| id.0).collect(),
            }),
        }),
    }
}
fn decode_node(value: v1::FormulaNode) -> Result<d::Node, CodecError> {
    use v1::formula_node::Kind;
    Ok(match value.kind.ok_or(CodecError::Invalid)? {
        Kind::Number(n) => d::Node::Number(n),
        Kind::Text(t) => d::Node::Text(t),
        Kind::Boolean(b) => d::Node::Boolean(b),
        Kind::Error(e) => d::Node::Error(decode_error(e)?),
        Kind::Missing(true) => d::Node::Missing,
        Kind::Missing(false) => return Err(CodecError::Invalid),
        Kind::Cell(r) => d::Node::Cell(decode_reference(r)?),
        Kind::Range(r) => d::Node::Range(d::BoundRangeReference {
            start: decode_reference(r.start.ok_or(CodecError::Invalid)?)?,
            end: decode_reference(r.end.ok_or(CodecError::Invalid)?)?,
        }),
        Kind::InvalidReference(e) => d::Node::InvalidReference(decode_reference_error(e)?),
        Kind::Unary(v) => d::Node::Unary {
            op: decode_unary(v.operator)?,
            operand: d::NodeId(v.operand),
        },
        Kind::Binary(v) => d::Node::Binary {
            op: decode_binary(v.operator)?,
            left: d::NodeId(v.left),
            right: d::NodeId(v.right),
        },
        Kind::Call(v) => {
            if v.arguments.len() > d::MAX_CALL_ARGUMENTS {
                return Err(CodecError::Invalid);
            }
            d::Node::Call {
                name: d::FunctionName::new(v.name).map_err(invalid)?,
                arguments: v.arguments.into_iter().map(d::NodeId).collect(),
            }
        }
    })
}
pub fn encode_formula(formula: &d::Formula) -> v1::BoundFormula {
    v1::BoundFormula {
        source: formula.source().to_owned(),
        canonical: formula.canonical().to_owned(),
        ast: Some(v1::BoundAst {
            schema_version: formula.ast().schema_version(),
            root: formula.ast().root().0,
            nodes: formula.ast().nodes().iter().map(node).collect(),
        }),
        origin: Some(addr(&formula.origin())),
        bound_at_version: formula.bound_at_version().get(),
        compatibility_profile: match formula.profile() {
            d::CompatibilityProfile::ExcelSubsetV1 => 1,
        },
    }
}
pub fn decode_formula(value: v1::BoundFormula) -> Result<d::Formula, CodecError> {
    if value.compatibility_profile != 1 {
        return Err(CodecError::UnsupportedSchema);
    }
    if value.source.len() > d::MAX_SOURCE_BYTES || value.canonical.len() > d::MAX_CANONICAL_BYTES {
        return Err(CodecError::Invalid);
    }
    let ast = value.ast.ok_or(CodecError::Invalid)?;
    if ast.schema_version != d::AST_SCHEMA_VERSION {
        return Err(CodecError::UnsupportedSchema);
    }
    if ast.nodes.len() > d::MAX_NODES {
        return Err(CodecError::Invalid);
    }
    let nodes = ast
        .nodes
        .into_iter()
        .map(decode_node)
        .collect::<Result<Vec<_>, _>>()?;
    d::Formula::new(
        value.source,
        value.canonical,
        d::BoundAst::new(ast.schema_version, d::NodeId(ast.root), nodes).map_err(invalid)?,
        decode_addr(value.origin.ok_or(CodecError::Invalid)?)?,
        DocumentVersion::new(value.bound_at_version).map_err(|_| CodecError::Invalid)?,
    )
    .map_err(invalid)
}
