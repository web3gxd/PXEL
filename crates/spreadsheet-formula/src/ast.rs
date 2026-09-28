use std::fmt;
use thiserror::Error;

pub const AST_SCHEMA_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
pub const MAX_NODES: usize = 4096;
pub const MAX_DEPTH: usize = 128;
pub const MAX_RANGE_CELLS: u64 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorCode {
    MissingEquals,
    UnexpectedToken,
    UnterminatedString,
    InvalidNumber,
    InvalidReference,
    UnsupportedSyntax,
    LimitExceeded,
    InvalidAst,
}
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("formula syntax error {code:?} at bytes {span:?}")]
pub struct ParseError {
    pub code: ParseErrorCode,
    pub span: Span,
}
impl ParseError {
    pub(crate) fn at(code: ParseErrorCode, start: usize, end: usize) -> Self {
        Self {
            code,
            span: Span { start, end },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
    Percent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Concat,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}
#[derive(Clone, PartialEq, Eq)]
pub struct CellReference {
    pub sheet: Option<String>,
    pub row: u32,
    pub column: u32,
    pub row_absolute: bool,
    pub column_absolute: bool,
}
#[derive(Clone, PartialEq, Eq)]
pub struct RangeReference {
    pub start: CellReference,
    pub end: CellReference,
}
impl RangeReference {
    pub fn cell_count(&self) -> u64 {
        (u64::from(self.start.row.abs_diff(self.end.row)) + 1)
            .saturating_mul(u64::from(self.start.column.abs_diff(self.end.column)) + 1)
    }
}
#[derive(Clone, PartialEq)]
pub enum Node {
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(FormulaError),
    /// Projection-only deleted/invalid bound target; never emitted by the parser.
    InvalidReference,
    Missing,
    Cell(CellReference),
    Range(RangeReference),
    Unary {
        op: UnaryOp,
        operand: NodeId,
    },
    Binary {
        op: BinaryOp,
        left: NodeId,
        right: NodeId,
    },
    Call {
        name: String,
        arguments: Vec<NodeId>,
    },
}
/// Syntax-only normalized arena, not a stable-ID-bound persisted formula.
#[derive(Clone, PartialEq)]
pub struct Ast {
    pub schema_version: u32,
    pub root: NodeId,
    pub nodes: Vec<Node>,
}
#[derive(Clone, PartialEq)]
pub struct DiagnosticAst {
    pub ast: Ast,
    pub spans: Vec<Span>,
}
impl fmt::Debug for Ast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ast")
            .field("schema_version", &self.schema_version)
            .field("nodes", &self.nodes.len())
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for DiagnosticAst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticAst")
            .field("ast", &self.ast)
            .field("spans", &self.spans)
            .finish()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormulaError {
    Null,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    Na,
    Spill,
    Cycle,
}
impl FormulaError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Null => "#NULL!",
            Self::Div0 => "#DIV/0!",
            Self::Value => "#VALUE!",
            Self::Ref => "#REF!",
            Self::Name => "#NAME?",
            Self::Num => "#NUM!",
            Self::Na => "#N/A",
            Self::Spill => "#SPILL!",
            Self::Cycle => "#CYCLE!",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateSystem {
    Excel1900,
    Excel1904,
}
#[derive(Clone, PartialEq)]
pub enum Value {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    DateTime {
        serial: f64,
        system: DateSystem,
    },
    Array {
        rows: u32,
        columns: u32,
        values: Vec<Value>,
    },
    Error(FormulaError),
}
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Blank => "Blank",
            Self::Number(_) => "Number([redacted])",
            Self::Text(_) => "Text([redacted])",
            Self::Boolean(_) => "Boolean([redacted])",
            Self::DateTime { .. } => "DateTime([redacted])",
            Self::Array { .. } => "Array([redacted])",
            Self::Error(_) => "Error",
        })
    }
}
