//! Immutable normalized bound formula data. No parser, evaluator or transport types.
use crate::workbook::{CellAddress, ColumnId, DocumentVersion, RowId, SheetId};
use std::collections::BTreeSet;
use std::fmt;
use thiserror::Error;
mod hash;

pub const AST_SCHEMA_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
pub const MAX_CANONICAL_BYTES: usize = 64 * 1024;
pub const MAX_AST_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_LITERAL_BYTES: usize = 32 * 1024;
pub const MAX_NODES: usize = 4096;
pub const MAX_DEPTH: usize = 128;
pub const MAX_CALL_ARGUMENTS: usize = 255;

/// Untrusted client intent text; server binding produces a separate Formula.
#[derive(Clone, PartialEq, Eq)]
pub struct FormulaSource(String);
impl FormulaSource {
    pub fn new(source: String) -> Result<Self, FormulaValidationError> {
        if source.len() > MAX_SOURCE_BYTES {
            return Err(FormulaValidationError::LimitExceeded);
        }
        if source.len() < 2 || !source.starts_with('=') {
            return Err(FormulaValidationError::InvalidSource);
        }
        Ok(Self(source))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for FormulaSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FormulaSource([redacted])")
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum FormulaValidationError {
    #[error("unsupported formula AST schema")]
    UnsupportedSchema,
    #[error("invalid formula AST structure")]
    InvalidStructure,
    #[error("formula data exceeds configured limits")]
    LimitExceeded,
    #[error("formula number is nonfinite")]
    NonFiniteNumber,
    #[error("function identifier is not normalized")]
    InvalidFunctionName,
    #[error("invalid bound reference")]
    InvalidReference,
    #[error("invalid formula source or canonical text")]
    InvalidSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityProfile {
    ExcelSubsetV1,
}
impl CompatibilityProfile {
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::ExcelSubsetV1 => "pxel-excel-subset-v1",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);
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
pub enum AxisReferenceMode {
    Absolute,
    /// Delta is relative to the formula origin in bound_at_version's axis projection.
    /// Structural transforms must recompute it; it is not a permanent original offset.
    Relative {
        original_delta: i64,
    },
}
impl AxisReferenceMode {
    fn validate(self, maximum: u64) -> Result<(), FormulaValidationError> {
        match self {
            Self::Relative { original_delta } if original_delta.unsigned_abs() > maximum => {
                Err(FormulaValidationError::InvalidReference)
            }
            _ => Ok(()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundCellReference {
    pub target_sheet: SheetId,
    pub target_row: RowId,
    pub target_column: ColumnId,
    pub row_mode: AxisReferenceMode,
    pub column_mode: AxisReferenceMode,
}
impl BoundCellReference {
    pub fn validate(&self) -> Result<(), FormulaValidationError> {
        // Excel address-space bounds, independent of this workbook's allocated axes.
        self.row_mode.validate(1_048_575)?;
        self.column_mode.validate(16_383)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundRangeReference {
    pub start: BoundCellReference,
    pub end: BoundCellReference,
}
impl BoundRangeReference {
    pub fn validate(&self) -> Result<(), FormulaValidationError> {
        self.start.validate()?;
        self.end.validate()?;
        if self.start.target_sheet != self.end.target_sheet {
            return Err(FormulaValidationError::InvalidReference);
        }
        // Rectangle geometry, range cardinality and live IDs require binder axis context.
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceError {
    DeletedTarget,
    OutOfBounds,
    MissingSheet,
}
#[derive(Clone, PartialEq, Eq)]
pub struct FunctionName(String);
impl FunctionName {
    pub fn new(name: String) -> Result<Self, FormulaValidationError> {
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().enumerate().all(|(index, b)| {
                if index == 0 {
                    b.is_ascii_uppercase() || b == b'_'
                } else {
                    b.is_ascii_uppercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.')
                }
            })
        {
            return Err(FormulaValidationError::InvalidFunctionName);
        }
        Ok(Self(name))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for FunctionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FunctionName([redacted])")
    }
}
#[derive(Clone, PartialEq)]
pub enum Node {
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(FormulaError),
    Missing,
    Cell(BoundCellReference),
    Range(BoundRangeReference),
    InvalidReference(ReferenceError),
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
        name: FunctionName,
        arguments: Vec<NodeId>,
    },
}
impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Number(_) => "Number([redacted])",
            Self::Text(_) => "Text([redacted])",
            Self::Boolean(_) => "Boolean([redacted])",
            Self::Error(_) => "Error",
            Self::Missing => "Missing",
            Self::Cell(_) => "BoundCell",
            Self::Range(_) => "BoundRange",
            Self::InvalidReference(_) => "InvalidReference",
            Self::Unary { .. } => "Unary",
            Self::Binary { .. } => "Binary",
            Self::Call { .. } => "Call([redacted])",
        })
    }
}
#[derive(Clone, PartialEq)]
pub struct BoundAst {
    schema_version: u32,
    root: NodeId,
    nodes: Vec<Node>,
    text_bytes: usize,
}
impl BoundAst {
    /// Validates representation only. The server binder must additionally establish
    /// that these stable references resolve against the authorized base projection.
    pub fn new(
        schema_version: u32,
        root: NodeId,
        mut nodes: Vec<Node>,
    ) -> Result<Self, FormulaValidationError> {
        if schema_version != AST_SCHEMA_VERSION {
            return Err(FormulaValidationError::UnsupportedSchema);
        }
        if nodes.is_empty() || nodes.len() > MAX_NODES {
            return Err(FormulaValidationError::LimitExceeded);
        }
        if root.0 as usize != nodes.len() - 1 {
            return Err(FormulaValidationError::InvalidStructure);
        }
        let mut depths = vec![1usize; nodes.len()];
        let mut parents = vec![0u8; nodes.len()];
        let mut text_bytes = 0usize;
        for (index, node) in nodes.iter_mut().enumerate() {
            let mut child = |id: NodeId| -> Result<(), FormulaValidationError> {
                let position = id.0 as usize;
                if position >= index {
                    return Err(FormulaValidationError::InvalidStructure);
                }
                let Some(count) = parents.get_mut(position) else {
                    return Err(FormulaValidationError::InvalidStructure);
                };
                if *count != 0 {
                    return Err(FormulaValidationError::InvalidStructure);
                }
                *count = 1;
                let child_depth = *depths
                    .get(position)
                    .ok_or(FormulaValidationError::InvalidStructure)?;
                let depth = depths
                    .get_mut(index)
                    .ok_or(FormulaValidationError::InvalidStructure)?;
                *depth = (*depth).max(child_depth + 1);
                if *depth > MAX_DEPTH {
                    return Err(FormulaValidationError::LimitExceeded);
                }
                Ok(())
            };
            match node {
                Node::Number(n) => {
                    if !n.is_finite() {
                        return Err(FormulaValidationError::NonFiniteNumber);
                    }
                    if *n == 0.0 {
                        *n = 0.0
                    }
                }
                Node::Text(text) => {
                    if text.len() > MAX_LITERAL_BYTES {
                        return Err(FormulaValidationError::LimitExceeded);
                    }
                    text_bytes = text_bytes
                        .checked_add(text.len())
                        .ok_or(FormulaValidationError::LimitExceeded)?;
                }
                Node::Cell(reference) => reference.validate()?,
                Node::Range(reference) => reference.validate()?,
                Node::Unary { operand, .. } => child(*operand)?,
                Node::Binary { left, right, .. } => {
                    child(*left)?;
                    child(*right)?;
                }
                Node::Call { name, arguments } => {
                    if arguments.len() > MAX_CALL_ARGUMENTS {
                        return Err(FormulaValidationError::LimitExceeded);
                    }
                    text_bytes = text_bytes
                        .checked_add(name.as_str().len())
                        .ok_or(FormulaValidationError::LimitExceeded)?;
                    for id in arguments {
                        child(*id)?;
                    }
                }
                Node::Boolean(_) | Node::Error(_) | Node::Missing | Node::InvalidReference(_) => {}
            }
            if text_bytes > MAX_AST_TEXT_BYTES {
                return Err(FormulaValidationError::LimitExceeded);
            }
        }
        if parents
            .iter()
            .take(nodes.len() - 1)
            .any(|count| *count != 1)
        {
            return Err(FormulaValidationError::InvalidStructure);
        }
        Ok(Self {
            schema_version,
            root,
            nodes,
            text_bytes,
        })
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn root(&self) -> NodeId {
        self.root
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn text_bytes(&self) -> usize {
        self.text_bytes
    }
}
impl fmt::Debug for BoundAst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundAst")
            .field("schema_version", &self.schema_version)
            .field("node_count", &self.nodes.len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq)]
pub struct Formula {
    source: String,
    canonical: String,
    ast: BoundAst,
    origin: CellAddress,
    bound_at_version: DocumentVersion,
    profile: CompatibilityProfile,
}
impl Formula {
    /// Rebase a surviving formula after deletion. Deleted scalar targets become
    /// typed REF nodes; ranges keep the surviving portion of their old span.
    pub fn rebase_deleted_rows(
        &self,
        next_version: DocumentVersion,
        origin_row_position: u32,
        deleted_sheet: SheetId,
        deleted_rows: &BTreeSet<RowId>,
        row_position: impl Fn(SheetId, RowId) -> Option<u32>,
        range_survivors: impl Fn(RowId, RowId) -> Option<(RowId, RowId)>,
    ) -> Result<Self, FormulaValidationError> {
        let mut nodes = self.ast.nodes().to_vec();
        let rebase = |reference: &mut BoundCellReference| -> Result<(), FormulaValidationError> {
            if let AxisReferenceMode::Relative { .. } = reference.row_mode {
                let target = row_position(reference.target_sheet, reference.target_row)
                    .ok_or(FormulaValidationError::InvalidReference)?;
                reference.row_mode = AxisReferenceMode::Relative {
                    original_delta: i64::from(target) - i64::from(origin_row_position),
                };
            }
            Ok(())
        };
        for node in &mut nodes {
            match node {
                Node::Cell(reference) => {
                    if reference.target_sheet == deleted_sheet
                        && deleted_rows.contains(&reference.target_row)
                    {
                        *node = Node::InvalidReference(ReferenceError::DeletedTarget);
                    } else {
                        rebase(reference)?;
                    }
                }
                Node::Range(range) => {
                    if range.start.target_sheet == deleted_sheet {
                        if let Some((start, end)) =
                            range_survivors(range.start.target_row, range.end.target_row)
                        {
                            range.start.target_row = start;
                            range.end.target_row = end;
                        } else {
                            *node = Node::InvalidReference(ReferenceError::DeletedTarget);
                            continue;
                        }
                    }
                    rebase(&mut range.start)?;
                    rebase(&mut range.end)?;
                }
                _ => {}
            }
        }
        let ast = BoundAst::new(self.ast.schema_version(), self.ast.root(), nodes)?;
        Self::new(
            self.source.clone(),
            self.canonical.clone(),
            ast,
            self.origin,
            next_version,
        )
    }
    /// Rebase only relative row copy offsets after a stable-ID-preserving axis
    /// change. Authored text stays immutable; the current A1 view is rendered
    /// from targets and axes by the formula adapter.
    pub fn rebase_row_offsets(
        &self,
        next_version: DocumentVersion,
        origin_row_position: u32,
        row_position: impl Fn(SheetId, RowId) -> Option<u32>,
    ) -> Result<Self, FormulaValidationError> {
        let mut nodes = self.ast.nodes().to_vec();
        let rebase = |reference: &mut BoundCellReference| -> Result<(), FormulaValidationError> {
            if let AxisReferenceMode::Relative { .. } = reference.row_mode {
                let target = row_position(reference.target_sheet, reference.target_row)
                    .ok_or(FormulaValidationError::InvalidReference)?;
                reference.row_mode = AxisReferenceMode::Relative {
                    original_delta: i64::from(target) - i64::from(origin_row_position),
                };
            }
            Ok(())
        };
        for node in &mut nodes {
            match node {
                Node::Cell(reference) => rebase(reference)?,
                Node::Range(reference) => {
                    rebase(&mut reference.start)?;
                    rebase(&mut reference.end)?;
                }
                _ => {}
            }
        }
        let ast = BoundAst::new(self.ast.schema_version(), self.ast.root(), nodes)?;
        Self::new(
            self.source.clone(),
            self.canonical.clone(),
            ast,
            self.origin,
            next_version,
        )
    }
    /// Architecture-independent conservative charge, not allocator instrumentation.
    /// Nodes cost 256 bytes each, argument handles 4 bytes, plus AST-owned text.
    pub fn ast_memory_bytes(&self) -> usize {
        self.ast.nodes().len() * 256
            + self.ast.text_bytes()
            + self
                .ast
                .nodes()
                .iter()
                .map(|node| match node {
                    Node::Call { arguments, .. } => arguments.len() * 4,
                    _ => 0,
                })
                .sum::<usize>()
    }
    pub fn semantic_hash(&self) -> [u8; 32] {
        hash::semantic_hash(self)
    }
    pub fn new(
        source: String,
        canonical: String,
        ast: BoundAst,
        origin: CellAddress,
        bound_at_version: DocumentVersion,
    ) -> Result<Self, FormulaValidationError> {
        if source.len() > MAX_SOURCE_BYTES || canonical.len() > MAX_CANONICAL_BYTES {
            return Err(FormulaValidationError::LimitExceeded);
        }
        if !source.starts_with('=')
            || source.len() < 2
            || !canonical.starts_with('=')
            || canonical.len() < 2
        {
            return Err(FormulaValidationError::InvalidSource);
        }
        Ok(Self {
            source,
            canonical,
            ast,
            origin,
            bound_at_version,
            profile: CompatibilityProfile::ExcelSubsetV1,
        })
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn ast(&self) -> &BoundAst {
        &self.ast
    }
    pub fn origin(&self) -> CellAddress {
        self.origin
    }
    pub fn bound_at_version(&self) -> DocumentVersion {
        self.bound_at_version
    }
    pub fn profile(&self) -> CompatibilityProfile {
        self.profile
    }
    pub fn text_bytes(&self) -> usize {
        self.source.len() + self.canonical.len() + self.ast.text_bytes()
    }
}
impl fmt::Debug for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Formula")
            .field("ast", &self.ast)
            .field("bound_at_version", &self.bound_at_version)
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}
