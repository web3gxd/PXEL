//! Pure, bounded sparse projection. Canonical hash format is `PXEL-DOMAIN-1`.
pub use crate::formula::FormulaSource;
use crate::formula::{AxisReferenceMode, BoundCellReference, Formula, Node};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
};
use thiserror::Error;

pub const MAX_ROWS: usize = 100_000;
pub const MAX_COLUMNS: usize = 1_000;
pub const MAX_CELLS: usize = 100_000;
pub const MAX_RETIRED_ROWS: usize = 1_000_000;
pub const MAX_RETIRED_COLUMNS: usize = 1_000_000;
pub const MAX_CELL_VALUE_BATCH_TEXT_BYTES: usize = 512 * 1024;
pub const MAX_TEXT_BYTES: usize = 32 * 1024;
pub const MAX_WORKBOOK_TEXT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_WORKBOOK_AST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("sheet does not exist")]
    MissingSheet,
    #[error("row does not exist")]
    MissingRow,
    #[error("column does not exist")]
    MissingColumn,
    #[error("invalid cell value")]
    InvalidValue,
    #[error("invalid bound formula")]
    InvalidFormula,
    #[error("structural operation would require unsupported formula transformation")]
    UnsupportedStructuralDependency,
    #[error("invalid or duplicate axis identity")]
    InvalidAxis,
    #[error("document version conflict")]
    VersionConflict,
    #[error("canonical state hash mismatch")]
    HashMismatch,
    #[error("invalid identity")]
    InvalidId,
    #[error("document limit exceeded")]
    LimitExceeded,
    #[error("invalid calculation context")]
    InvalidCalculationContext,
    #[error("cell content no longer matches the committed preimage")]
    PreimageConflict,
}

macro_rules! identity {
    ($($name:ident),+) => {$ (
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        pub struct $name([u8; 16]);
        impl $name {
            pub fn from_bytes(bytes: [u8;16]) -> Result<Self, DomainError> {
                if bytes == [0;16] { Err(DomainError::InvalidId) } else { Ok(Self(bytes)) }
            }
            pub const fn as_bytes(&self) -> &[u8;16] { &self.0 }
        }
    )+};
}
identity!(
    DocumentId,
    SheetId,
    RowId,
    ColumnId,
    ActorId,
    WorkspaceId,
    OperationId,
    SnapshotId,
    AuthorizationChangeId,
    CalculationContextId
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct DocumentVersion(u64);
impl DocumentVersion {
    pub const ZERO: Self = Self(0);
    pub fn new(value: u64) -> Result<Self, DomainError> {
        if value > i64::MAX as u64 {
            Err(DomainError::LimitExceeded)
        } else {
            Ok(Self(value))
        }
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CellAddress {
    pub sheet: SheetId,
    pub row: RowId,
    pub column: ColumnId,
}

/// A persisted input to volatile evaluation. No clock or random sampling occurs
/// while applying or replaying this value.
#[derive(Clone, Eq, PartialEq)]
pub struct CalculationContext {
    context_id: CalculationContextId,
    epoch: u64,
    effective_timestamp_utc_ms: i64,
    utc_offset_minutes: i16,
    civil_day_since_unix_epoch: i32,
    random_seed: [u8; 32],
    engine_version: String,
    compatibility_profile: String,
    reason: CalculationReason,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalculationReason {
    Open = 1,
    Explicit = 2,
    Timer = 3,
    VolatileDependency = 4,
    EngineUpgrade = 5,
}

impl fmt::Debug for CalculationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CalculationContext")
            .field("context_id", &self.context_id)
            .field("epoch", &self.epoch)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl CalculationContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context_id: CalculationContextId,
        epoch: u64,
        effective_timestamp_utc_ms: i64,
        utc_offset_minutes: i16,
        civil_day_since_unix_epoch: i32,
        random_seed: [u8; 32],
        engine_version: String,
        compatibility_profile: String,
        reason: CalculationReason,
    ) -> Result<Self, DomainError> {
        let offset_ms = i64::from(utc_offset_minutes) * 60_000;
        let calculated_day = effective_timestamp_utc_ms
            .checked_add(offset_ms)
            .map(|ms| ms.div_euclid(86_400_000))
            .and_then(|day| i32::try_from(day).ok());
        let valid_label = |label: &str| {
            !label.is_empty()
                && label.len() <= 64
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        };
        if epoch == 0
            || effective_timestamp_utc_ms < 0
            || !(-840..=840).contains(&utc_offset_minutes)
            || calculated_day != Some(civil_day_since_unix_epoch)
            || !valid_label(&engine_version)
            || !valid_label(&compatibility_profile)
        {
            return Err(DomainError::InvalidCalculationContext);
        }
        Ok(Self {
            context_id,
            epoch,
            effective_timestamp_utc_ms,
            utc_offset_minutes,
            civil_day_since_unix_epoch,
            random_seed,
            engine_version,
            compatibility_profile,
            reason,
        })
    }
    pub const fn context_id(&self) -> CalculationContextId {
        self.context_id
    }
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
    pub const fn effective_timestamp_utc_ms(&self) -> i64 {
        self.effective_timestamp_utc_ms
    }
    pub const fn utc_offset_minutes(&self) -> i16 {
        self.utc_offset_minutes
    }
    pub const fn civil_day_since_unix_epoch(&self) -> i32 {
        self.civil_day_since_unix_epoch
    }
    pub const fn random_seed(&self) -> &[u8; 32] {
        &self.random_seed
    }
    pub fn engine_version(&self) -> &str {
        &self.engine_version
    }
    pub fn compatibility_profile(&self) -> &str {
        &self.compatibility_profile
    }
    pub const fn reason(&self) -> CalculationReason {
        self.reason
    }
    pub fn with_epoch(&self, epoch: u64) -> Result<Self, DomainError> {
        Self::new(
            self.context_id,
            epoch,
            self.effective_timestamp_utc_ms,
            self.utc_offset_minutes,
            self.civil_day_since_unix_epoch,
            self.random_seed,
            self.engine_version.clone(),
            self.compatibility_profile.clone(),
            self.reason,
        )
    }
}

#[derive(Clone, PartialEq)]
pub enum CellValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
}
impl fmt::Debug for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Blank => "Blank",
            Self::Number(_) => "Number([redacted])",
            Self::Text(_) => "Text([redacted])",
            Self::Boolean(_) => "Boolean([redacted])",
        })
    }
}
impl CellValue {
    fn text_bytes(&self) -> usize {
        match self {
            Self::Text(value) => value.len(),
            _ => 0,
        }
    }
    pub fn validate(&self) -> Result<(), DomainError> {
        match self {
            Self::Number(n) if !n.is_finite() => Err(DomainError::InvalidValue),
            Self::Text(t) if t.len() > MAX_TEXT_BYTES => Err(DomainError::LimitExceeded),
            _ => Ok(()),
        }
    }
    pub fn normalized(&self) -> Result<Self, DomainError> {
        self.validate()?;
        Ok(match self {
            Self::Number(n) if *n == 0.0 => Self::Number(0.0),
            _ => self.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellContent {
    Value(CellValue),
    Formula(Formula),
}
impl CellContent {
    pub fn text_bytes(&self) -> usize {
        match self {
            Self::Value(value) => value.text_bytes(),
            Self::Formula(formula) => formula.text_bytes(),
        }
    }
    pub fn ast_memory_bytes(&self) -> usize {
        match self {
            Self::Value(_) => 0,
            Self::Formula(formula) => formula.ast_memory_bytes(),
        }
    }
}
impl From<CellValue> for CellContent {
    fn from(value: CellValue) -> Self {
        Self::Value(value)
    }
}

#[derive(Clone, PartialEq)]
pub struct SheetSeed {
    pub id: SheetId,
    pub name: String,
    pub rows: Vec<RowId>,
    pub columns: Vec<ColumnId>,
}
// Names are user content and must not be rendered by accidental Debug logging.
impl fmt::Debug for SheetSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SheetSeed")
            .field("id", &self.id)
            .field("rows", &self.rows.len())
            .field("columns", &self.columns.len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct CellWrite {
    pub address: CellAddress,
    pub value: CellValue,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ContentWrite {
    pub address: CellAddress,
    pub content: Option<CellContent>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct GuardedContentWrite {
    pub address: CellAddress,
    pub content: Option<CellContent>,
    pub previous: Option<CellContent>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeWriteKind {
    SetValues,
    Copy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellWriteKind {
    SetValue,
    Clear,
    SetFormula,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    CreateWorkbook {
        sheet: SheetSeed,
    },
    AddSheet {
        sheet: SheetSeed,
    },
    ExtendRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    InsertRows {
        sheet: SheetId,
        before: RowId,
        rows: Vec<RowId>,
    },
    /// Persisted schema 6: same axis action plus deterministic rebasing of
    /// every bound formula's relative row offset at the new document version.
    InsertRowsRebased {
        sheet: SheetId,
        before: RowId,
        rows: Vec<RowId>,
    },
    DeleteRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    InsertColumnsRebased {
        sheet: SheetId,
        before: ColumnId,
        columns: Vec<ColumnId>,
    },
    DeleteColumns {
        sheet: SheetId,
        columns: Vec<ColumnId>,
    },
    SetCellValue {
        address: CellAddress,
        value: CellValue,
    },
    SetCellValues {
        writes: Vec<CellWrite>,
    },
    /// Frozen source contents, including transformed formulas and blank cells.
    /// Replay never reads the source range again.
    CopyRange {
        writes: Vec<ContentWrite>,
    },
    /// Frozen move footprint and exact preimages. Not an undo target in this schema.
    MoveRange {
        source: CellAddress,
        destination: CellAddress,
        rows: u32,
        columns: u32,
        writes: Vec<GuardedContentWrite>,
    },
    /// Exact frozen inverse; journal admission verifies provenance and history.
    MoveCompensation {
        source: CellAddress,
        destination: CellAddress,
        rows: u32,
        columns: u32,
        writes: Vec<GuardedContentWrite>,
        undoes: OperationId,
        undoes_version: DocumentVersion,
    },
    /// Frozen destination contents and exact preimages in one atomic event.
    RangeWrite {
        writes: Vec<GuardedContentWrite>,
        kind: Option<RangeWriteKind>,
        undoes: Option<OperationId>,
        undoes_version: Option<DocumentVersion>,
    },
    SetCellFormula {
        address: CellAddress,
        formula: Formula,
    },
    ClearCell {
        address: CellAddress,
    },
    /// A guarded, replayable content write. `previous` is the exact content
    /// observed before the write; `undoes` identifies a compensated user event.
    CellWrite {
        address: CellAddress,
        content: Option<CellContent>,
        previous: Option<CellContent>,
        kind: Option<CellWriteKind>,
        undoes: Option<OperationId>,
        undoes_version: Option<DocumentVersion>,
    },
    AdvanceCalculationEpoch {
        context: CalculationContext,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub enum CommandPayload {
    CreateWorkbook {
        sheet: SheetSeed,
    },
    AddSheet {
        sheet: SheetSeed,
    },
    ExtendRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    InsertRows {
        sheet: SheetId,
        before: RowId,
        rows: Vec<RowId>,
    },
    DeleteRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    InsertColumns {
        sheet: SheetId,
        before: ColumnId,
        columns: Vec<ColumnId>,
    },
    DeleteColumns {
        sheet: SheetId,
        columns: Vec<ColumnId>,
    },
    SetCellValue {
        address: CellAddress,
        value: CellValue,
    },
    SetCellValues {
        writes: Vec<CellWrite>,
    },
    CopyRange {
        source: CellAddress,
        destination: CellAddress,
        rows: u32,
        columns: u32,
    },
    MoveRange {
        source: CellAddress,
        destination: CellAddress,
        rows: u32,
        columns: u32,
    },
    SetCellFormula {
        address: CellAddress,
        source: FormulaSource,
    },
    ClearCell {
        address: CellAddress,
    },
    UndoCell {
        target_operation_id: OperationId,
    },
}
impl Operation {
    pub const fn is_cell_inverse_target(&self) -> bool {
        matches!(
            self,
            Self::CellWrite {
                kind: Some(_),
                undoes: None,
                undoes_version: None,
                ..
            } | Self::CellWrite {
                kind: None,
                undoes: Some(_),
                undoes_version: Some(_),
                ..
            }
        )
    }
    pub const fn is_inverse_target(&self) -> bool {
        self.is_cell_inverse_target()
            || matches!(self, Self::MoveRange { .. } | Self::MoveCompensation { .. })
            || matches!(
                self,
                Self::RangeWrite {
                    kind: Some(_),
                    undoes: None,
                    undoes_version: None,
                    ..
                } | Self::RangeWrite {
                    kind: None,
                    undoes: Some(_),
                    undoes_version: Some(_),
                    ..
                }
            )
    }
    pub const fn changes_axes(&self) -> bool {
        matches!(
            self,
            Self::CreateWorkbook { .. }
                | Self::AddSheet { .. }
                | Self::ExtendRows { .. }
                | Self::InsertRows { .. }
                | Self::InsertRowsRebased { .. }
                | Self::DeleteRows { .. }
                | Self::InsertColumnsRebased { .. }
                | Self::DeleteColumns { .. }
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub operation_id: OperationId,
    pub document_id: DocumentId,
    pub actor_id: ActorId,
    pub base_version: DocumentVersion,
    pub client_sequence: u64,
    pub operation: CommandPayload,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventPrincipal {
    User(ActorId),
    System,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CommittedEvent {
    pub document_id: DocumentId,
    pub operation_id: OperationId,
    pub principal: EventPrincipal,
    pub version: DocumentVersion,
    pub operation: Operation,
    pub state_hash: [u8; 32],
}

#[derive(Clone)]
pub struct Sheet {
    id: SheetId,
    name: String,
    rows: Vec<RowId>,
    columns: Vec<ColumnId>,
    row_positions: HashMap<RowId, u32>,
    column_positions: HashMap<ColumnId, u32>,
    cells: BTreeMap<CellAddress, CellContent>,
}

/// Versioned storage codecs encode this authored image; calculated values are absent.
/// It is not a transport DTO and must be authenticated against the journal head.
#[derive(Clone)]
pub struct SnapshotImage {
    pub document_id: DocumentId,
    pub version: DocumentVersion,
    pub last_axis_change_version: DocumentVersion,
    pub sheets: Vec<SnapshotSheetImage>,
    pub retired_rows: Vec<RowId>,
    pub retired_columns: Vec<ColumnId>,
    pub calculation_context: Option<CalculationContext>,
}

#[derive(Clone)]
pub struct SnapshotSheetImage {
    pub sheet: SheetSeed,
    pub cells: Vec<(CellAddress, CellContent)>,
}
impl Sheet {
    pub const fn id(&self) -> SheetId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn rows(&self) -> &[RowId] {
        &self.rows
    }
    pub fn columns(&self) -> &[ColumnId] {
        &self.columns
    }
    pub fn row_position(&self, id: RowId) -> Option<u32> {
        self.row_positions.get(&id).copied()
    }
    pub fn column_position(&self, id: ColumnId) -> Option<u32> {
        self.column_positions.get(&id).copied()
    }
    pub fn cells(&self) -> &BTreeMap<CellAddress, CellContent> {
        &self.cells
    }
    fn from_seed(seed: &SheetSeed) -> Result<Self, DomainError> {
        if seed.rows.len() > MAX_ROWS || seed.columns.len() > MAX_COLUMNS {
            return Err(DomainError::LimitExceeded);
        }
        if seed.rows.is_empty()
            || seed.columns.is_empty()
            || seed.name.is_empty()
            || seed.name.chars().count() > 31
            || seed
                .name
                .chars()
                .any(|c| c.is_control() || "[]:*?/\\".contains(c))
        {
            return Err(DomainError::InvalidAxis);
        }
        let row_positions: HashMap<_, _> = seed
            .rows
            .iter()
            .copied()
            .enumerate()
            .map(|(position, id)| (id, position as u32))
            .collect();
        let column_positions: HashMap<_, _> = seed
            .columns
            .iter()
            .copied()
            .enumerate()
            .map(|(position, id)| (id, position as u32))
            .collect();
        if row_positions.len() != seed.rows.len() || column_positions.len() != seed.columns.len() {
            return Err(DomainError::InvalidAxis);
        }
        Ok(Self {
            id: seed.id,
            name: seed.name.clone(),
            rows: seed.rows.clone(),
            columns: seed.columns.clone(),
            row_positions,
            column_positions,
            cells: BTreeMap::new(),
        })
    }
    fn validate_address(&self, address: CellAddress) -> Result<(), DomainError> {
        if !self.row_positions.contains_key(&address.row) {
            return Err(DomainError::MissingRow);
        }
        if !self.column_positions.contains_key(&address.column) {
            return Err(DomainError::MissingColumn);
        }
        Ok(())
    }
    fn validate_row_extension(&self, new_rows: &[RowId]) -> Result<(), DomainError> {
        if new_rows.is_empty() || new_rows.len() > 1_000 {
            return Err(DomainError::LimitExceeded);
        }
        if self.rows.len().saturating_add(new_rows.len()) > MAX_ROWS {
            return Err(DomainError::LimitExceeded);
        }
        let mut seen = BTreeSet::new();
        if new_rows
            .iter()
            .any(|row| self.row_positions.contains_key(row) || !seen.insert(*row))
        {
            return Err(DomainError::InvalidAxis);
        }
        Ok(())
    }
    fn validate_column_extension(&self, columns: &[ColumnId]) -> Result<(), DomainError> {
        if columns.is_empty() || columns.len() > 100 {
            return Err(DomainError::LimitExceeded);
        }
        if self.columns.len().saturating_add(columns.len()) > MAX_COLUMNS {
            return Err(DomainError::LimitExceeded);
        }
        let mut seen = BTreeSet::new();
        if columns
            .iter()
            .any(|id| self.column_positions.contains_key(id) || !seen.insert(*id))
        {
            return Err(DomainError::InvalidAxis);
        }
        Ok(())
    }
    fn validate_column_deletion(
        &self,
        columns: &[ColumnId],
    ) -> Result<BTreeSet<ColumnId>, DomainError> {
        if columns.is_empty() || columns.len() > 100 {
            return Err(DomainError::LimitExceeded);
        }
        let mut deleted = BTreeSet::new();
        for column in columns {
            if !self.column_positions.contains_key(column) {
                return Err(DomainError::MissingColumn);
            }
            if !deleted.insert(*column) {
                return Err(DomainError::InvalidAxis);
            }
        }
        if deleted.len() >= self.columns.len() {
            return Err(DomainError::InvalidAxis);
        }
        Ok(deleted)
    }
    fn validate_row_deletion(&self, rows: &[RowId]) -> Result<BTreeSet<RowId>, DomainError> {
        if rows.is_empty() || rows.len() > 1_000 {
            return Err(DomainError::LimitExceeded);
        }
        let mut deleted = BTreeSet::new();
        for row in rows {
            if !self.row_positions.contains_key(row) {
                return Err(DomainError::MissingRow);
            }
            if !deleted.insert(*row) {
                return Err(DomainError::InvalidAxis);
            }
        }
        if deleted.len() >= self.rows.len() {
            return Err(DomainError::InvalidAxis);
        }
        Ok(deleted)
    }
}

#[derive(Clone)]
pub struct Workbook {
    document_id: DocumentId,
    version: DocumentVersion,
    // Derived by replay; not part of the canonical authored hash.
    last_axis_change_version: DocumentVersion,
    sheets: Vec<Sheet>,
    retired_row_ids: BTreeSet<RowId>,
    retired_column_ids: BTreeSet<ColumnId>,
    calculation_context: Option<CalculationContext>,
    // Derived from cell contents; never persisted or included in canonical hashes.
    text_bytes: usize,
    ast_memory_bytes: usize,
}
impl Workbook {
    pub fn empty(document_id: DocumentId) -> Self {
        Self {
            document_id,
            version: DocumentVersion::ZERO,
            last_axis_change_version: DocumentVersion::ZERO,
            sheets: Vec::new(),
            retired_row_ids: BTreeSet::new(),
            retired_column_ids: BTreeSet::new(),
            calculation_context: None,
            text_bytes: 0,
            ast_memory_bytes: 0,
        }
    }
    pub const fn document_id(&self) -> DocumentId {
        self.document_id
    }
    pub const fn version(&self) -> DocumentVersion {
        self.version
    }
    pub const fn last_axis_change_version(&self) -> DocumentVersion {
        self.last_axis_change_version
    }
    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }
    pub fn cell_content(&self, address: CellAddress) -> Result<Option<&CellContent>, DomainError> {
        let sheet = self
            .sheets
            .iter()
            .find(|sheet| sheet.id == address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        sheet.validate_address(address)?;
        Ok(sheet.cells.get(&address))
    }
    pub fn calculation_context(&self) -> Option<&CalculationContext> {
        self.calculation_context.as_ref()
    }
    pub const fn text_bytes(&self) -> usize {
        self.text_bytes
    }
    pub const fn ast_memory_bytes(&self) -> usize {
        self.ast_memory_bytes
    }
    pub fn snapshot_image(&self) -> Result<SnapshotImage, DomainError> {
        if self.version == DocumentVersion::ZERO || self.sheets.is_empty() || self.sheets.len() > 64
        {
            return Err(DomainError::VersionConflict);
        }
        Ok(SnapshotImage {
            document_id: self.document_id,
            version: self.version,
            last_axis_change_version: self.last_axis_change_version,
            sheets: self
                .sheets
                .iter()
                .map(|sheet| SnapshotSheetImage {
                    sheet: SheetSeed {
                        id: sheet.id,
                        name: sheet.name.clone(),
                        rows: sheet.rows.clone(),
                        columns: sheet.columns.clone(),
                    },
                    cells: sheet.cells.iter().map(|(a, c)| (*a, c.clone())).collect(),
                })
                .collect(),
            retired_rows: self.retired_row_ids.iter().copied().collect(),
            retired_columns: self.retired_column_ids.iter().copied().collect(),
            calculation_context: self.calculation_context.clone(),
        })
    }
    pub fn restore_snapshot(
        image: SnapshotImage,
        expected_hash: [u8; 32],
    ) -> Result<Self, DomainError> {
        if image.last_axis_change_version == DocumentVersion::ZERO
            || image.last_axis_change_version > image.version
            || image.retired_rows.len() > MAX_RETIRED_ROWS
            || image.retired_columns.len() > MAX_RETIRED_COLUMNS
        {
            return Err(DomainError::InvalidAxis);
        }
        let seeds = image
            .sheets
            .iter()
            .map(|sheet| sheet.sheet.clone())
            .collect::<Vec<_>>();
        let mut workbook = Self::hydrate_sheets(
            image.document_id,
            image.version,
            seeds,
            image.sheets.into_iter().flat_map(|sheet| sheet.cells),
        )?;
        let retired_row_count = image.retired_rows.len();
        let retired_column_count = image.retired_columns.len();
        let retired_rows: BTreeSet<_> = image.retired_rows.into_iter().collect();
        let retired_columns: BTreeSet<_> = image.retired_columns.into_iter().collect();
        if retired_rows.len() > MAX_RETIRED_ROWS
            || retired_columns.len() > MAX_RETIRED_COLUMNS
            || retired_rows.iter().any(|id| {
                workbook
                    .sheets
                    .iter()
                    .any(|sheet| sheet.row_positions.contains_key(id))
            })
            || retired_columns.iter().any(|id| {
                workbook
                    .sheets
                    .iter()
                    .any(|sheet| sheet.column_positions.contains_key(id))
            })
        {
            return Err(DomainError::InvalidAxis);
        }
        // A duplicate ID must not be accepted even if the expected hash is absent
        // from a later storage format or generated by a faulty writer.
        if retired_rows.len() != retired_row_count || retired_columns.len() != retired_column_count
        {
            return Err(DomainError::InvalidAxis);
        }
        workbook.last_axis_change_version = image.last_axis_change_version;
        workbook.retired_row_ids = retired_rows;
        workbook.retired_column_ids = retired_columns;
        workbook.calculation_context = image.calculation_context;
        if workbook.hash() != expected_hash {
            return Err(DomainError::HashMismatch);
        }
        Ok(workbook)
    }
    /// Bounded fixture loader for tests and benchmarks; absent from default builds.
    /// Validates identities/values just like commands but avoids quadratic replay setup.
    #[cfg(feature = "test-support")]
    pub fn from_fixture(
        document_id: DocumentId,
        version: DocumentVersion,
        seed: SheetSeed,
        cells: impl IntoIterator<Item = (CellAddress, CellValue)>,
    ) -> Result<Self, DomainError> {
        Self::from_content_fixture(
            document_id,
            version,
            seed,
            cells
                .into_iter()
                .map(|(address, value)| (address, CellContent::Value(value))),
        )
    }
    #[cfg(feature = "test-support")]
    pub fn from_content_fixture(
        document_id: DocumentId,
        version: DocumentVersion,
        seed: SheetSeed,
        cells: impl IntoIterator<Item = (CellAddress, CellContent)>,
    ) -> Result<Self, DomainError> {
        Self::hydrate(document_id, version, seed, cells)
    }
    /// Builds a bounded authored projection, not an authenticated server state.
    /// Callers must independently authenticate its source/version. Formula ASTs
    /// and metadata are validated; calculated results are never hydrated here.
    pub fn hydrate(
        document_id: DocumentId,
        version: DocumentVersion,
        seed: SheetSeed,
        cells: impl IntoIterator<Item = (CellAddress, CellContent)>,
    ) -> Result<Self, DomainError> {
        Self::hydrate_sheets(document_id, version, std::iter::once(seed), cells)
    }
    /// Hydrates ordered sheets together so cross-sheet bound references can be validated.
    pub fn hydrate_sheets(
        document_id: DocumentId,
        version: DocumentVersion,
        seeds: impl IntoIterator<Item = SheetSeed>,
        cells: impl IntoIterator<Item = (CellAddress, CellContent)>,
    ) -> Result<Self, DomainError> {
        if version == DocumentVersion::ZERO {
            return Err(DomainError::VersionConflict);
        }
        let mut sheets = Vec::new();
        let mut sheet_ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut row_ids = BTreeSet::new();
        let mut column_ids = BTreeSet::new();
        for seed in seeds {
            if sheets.len() >= 64
                || !sheet_ids.insert(seed.id)
                || !names.insert(seed.name.to_lowercase())
                || seed.rows.iter().any(|id| !row_ids.insert(*id))
                || seed.columns.iter().any(|id| !column_ids.insert(*id))
            {
                return Err(DomainError::InvalidAxis);
            }
            sheets.push(Sheet::from_seed(&seed)?);
        }
        if sheets.is_empty() {
            return Err(DomainError::MissingSheet);
        }
        let mut workbook = Self {
            document_id,
            version,
            last_axis_change_version: version,
            sheets,
            retired_row_ids: BTreeSet::new(),
            retired_column_ids: BTreeSet::new(),
            calculation_context: None,
            text_bytes: 0,
            ast_memory_bytes: 0,
        };
        let mut seen = BTreeSet::new();
        for (index, (address, value)) in cells.into_iter().enumerate() {
            if index >= MAX_CELLS {
                return Err(DomainError::LimitExceeded);
            }
            if !seen.insert(address) {
                return Err(DomainError::InvalidAxis);
            }
            let value = match value {
                CellContent::Value(value) => CellContent::Value(value.normalized()?),
                CellContent::Formula(formula) => {
                    workbook.validate_formula(address, &formula)?;
                    CellContent::Formula(formula)
                }
            };
            let (text_bytes, ast_memory_bytes) =
                workbook.updated_budgets(address, value.text_bytes(), value.ast_memory_bytes())?;
            if value != CellContent::Value(CellValue::Blank) {
                let sheet = workbook
                    .sheets
                    .iter_mut()
                    .find(|sheet| sheet.id == address.sheet)
                    .ok_or(DomainError::MissingSheet)?;
                if sheet.cells.insert(address, value).is_some() {
                    return Err(DomainError::InvalidAxis);
                }
            }
            workbook.text_bytes = text_bytes;
            workbook.ast_memory_bytes = ast_memory_bytes;
        }
        Ok(workbook)
    }
    /// Hydrates an authenticated projection context without inventing a journal event.
    /// This is an adapter entry point, not a canonical write path.
    pub fn hydrate_sheets_with_context(
        document_id: DocumentId,
        version: DocumentVersion,
        seeds: impl IntoIterator<Item = SheetSeed>,
        cells: impl IntoIterator<Item = (CellAddress, CellContent)>,
        context: CalculationContext,
    ) -> Result<Self, DomainError> {
        if context.epoch() > version.get() {
            return Err(DomainError::InvalidCalculationContext);
        }
        let mut book = Self::hydrate_sheets(document_id, version, seeds, cells)?;
        book.calculation_context = Some(context);
        Ok(book)
    }
    pub fn candidate(
        &self,
        operation: &Operation,
        next_version: DocumentVersion,
    ) -> Result<Self, DomainError> {
        if self.version.get().checked_add(1) != Some(next_version.get()) {
            return Err(DomainError::VersionConflict);
        }
        if let Operation::MoveCompensation {
            source,
            destination,
            rows,
            columns,
            writes,
            undoes_version,
            ..
        } = operation
        {
            if *undoes_version == DocumentVersion::ZERO || *undoes_version > self.version {
                return Err(DomainError::VersionConflict);
            }
            if source.sheet != destination.sheet || source == destination {
                return Err(DomainError::InvalidAxis);
            }
            if writes.is_empty() || writes.len() > 10_000 {
                return Err(DomainError::LimitExceeded);
            }
            let required: BTreeSet<_> = self
                .move_rectangle(*source, *rows, *columns)?
                .into_iter()
                .chain(self.move_rectangle(*destination, *rows, *columns)?)
                .collect();
            let mut seen = BTreeSet::new();
            let mut bytes = [0usize; 2];
            let mut operations = Vec::with_capacity(writes.len());
            for write in writes {
                if !seen.insert(write.address) {
                    return Err(DomainError::InvalidAxis);
                }
                for (index, content) in [&write.content, &write.previous].into_iter().enumerate() {
                    bytes[index] = bytes[index]
                        .checked_add(content.as_ref().map_or(0, CellContent::text_bytes))
                        .filter(|n| *n <= MAX_CELL_VALUE_BATCH_TEXT_BYTES)
                        .ok_or(DomainError::LimitExceeded)?;
                }
                let kind = Some(match &write.content {
                    None => CellWriteKind::Clear,
                    Some(CellContent::Value(_)) => CellWriteKind::SetValue,
                    Some(CellContent::Formula(_)) => CellWriteKind::SetFormula,
                });
                let cell = Operation::CellWrite {
                    address: write.address,
                    content: write.content.clone(),
                    previous: write.previous.clone(),
                    kind,
                    undoes: None,
                    undoes_version: None,
                };
                self.validate_guarded_write(&cell)?;
                operations.push(cell);
            }
            if !required.is_subset(&seen) {
                return Err(DomainError::InvalidAxis);
            }
            return self.preview_candidate(&operations, next_version);
        }
        if let Operation::MoveRange {
            source,
            destination,
            rows,
            columns,
            writes,
        } = operation
        {
            return self.move_candidate(
                *source,
                *destination,
                *rows,
                *columns,
                writes,
                next_version,
            );
        }
        if let Operation::RangeWrite {
            writes,
            kind,
            undoes,
            undoes_version,
        } = operation
        {
            if !operation.is_inverse_target()
                || undoes_version.is_some_and(|v| v == DocumentVersion::ZERO || v > self.version)
            {
                return Err(DomainError::VersionConflict);
            }
            if writes.is_empty() || writes.len() > 1_000 {
                return Err(DomainError::LimitExceeded);
            }
            for previous in [false, true] {
                let bytes = writes
                    .iter()
                    .try_fold(0usize, |total, write| {
                        let content = if previous {
                            &write.previous
                        } else {
                            &write.content
                        };
                        total.checked_add(content.as_ref().map_or(0, CellContent::text_bytes))
                    })
                    .ok_or(DomainError::LimitExceeded)?;
                if bytes > MAX_CELL_VALUE_BATCH_TEXT_BYTES {
                    return Err(DomainError::LimitExceeded);
                }
            }
            let operations = writes
                .iter()
                .map(|write| {
                    if *kind == Some(RangeWriteKind::SetValues)
                        && matches!(write.content, Some(CellContent::Formula(_)))
                    {
                        return Err(DomainError::InvalidValue);
                    }
                    let cell_kind = if kind.is_none() {
                        None
                    } else {
                        Some(match write.content {
                            Some(CellContent::Formula(_)) => CellWriteKind::SetFormula,
                            Some(CellContent::Value(_)) => CellWriteKind::SetValue,
                            None => CellWriteKind::Clear,
                        })
                    };
                    Ok(Operation::CellWrite {
                        address: write.address,
                        content: write.content.clone(),
                        previous: write.previous.clone(),
                        kind: cell_kind,
                        undoes: *undoes,
                        undoes_version: *undoes_version,
                    })
                })
                .collect::<Result<Vec<_>, DomainError>>()?;
            return self.preview_candidate(&operations, next_version);
        }
        if let Operation::SetCellValues { writes } = operation {
            if writes.is_empty() || writes.len() > 1_000 {
                return Err(DomainError::LimitExceeded);
            }
            let text_bytes = writes
                .iter()
                .try_fold(0usize, |total, write| {
                    total.checked_add(write.value.text_bytes())
                })
                .ok_or(DomainError::LimitExceeded)?;
            if text_bytes > MAX_CELL_VALUE_BATCH_TEXT_BYTES {
                return Err(DomainError::LimitExceeded);
            }
            let operations = writes
                .iter()
                .map(|write| Operation::SetCellValue {
                    address: write.address,
                    value: write.value.clone(),
                })
                .collect::<Vec<_>>();
            return self.preview_candidate(&operations, next_version);
        }
        if let Operation::CopyRange { writes } = operation {
            if writes.is_empty() || writes.len() > 1_000 {
                return Err(DomainError::LimitExceeded);
            }
            let text_bytes = writes
                .iter()
                .try_fold(0usize, |total, write| {
                    total.checked_add(write.content.as_ref().map_or(0, CellContent::text_bytes))
                })
                .ok_or(DomainError::LimitExceeded)?;
            if text_bytes > MAX_CELL_VALUE_BATCH_TEXT_BYTES {
                return Err(DomainError::LimitExceeded);
            }
            let operations = writes
                .iter()
                .map(|write| match &write.content {
                    Some(CellContent::Value(value)) => Operation::SetCellValue {
                        address: write.address,
                        value: value.clone(),
                    },
                    Some(CellContent::Formula(formula)) => Operation::SetCellFormula {
                        address: write.address,
                        formula: formula.clone(),
                    },
                    None => Operation::ClearCell {
                        address: write.address,
                    },
                })
                .collect::<Vec<_>>();
            return self.preview_candidate(&operations, next_version);
        }
        let budgets = self.operation_budgets(operation)?;
        let mut next = self.clone();
        next.apply_operation(operation, budgets, next_version)?;
        next.version = next_version;
        if operation.changes_axes() {
            next.last_axis_change_version = next_version;
        }
        if matches!(
            operation,
            Operation::InsertRowsRebased { .. }
                | Operation::DeleteRows { .. }
                | Operation::InsertColumnsRebased { .. }
                | Operation::DeleteColumns { .. }
        ) {
            for sheet in &next.sheets {
                for (address, content) in &sheet.cells {
                    if let CellContent::Formula(formula) = content {
                        next.validate_formula(*address, formula)?;
                    }
                }
            }
        }
        Ok(next)
    }
    /// Validated move preview with the original confirmed version retained.
    /// This never establishes server authority or publishes a synthetic version.
    pub fn preview_move_candidate(&self, operation: &Operation) -> Result<Self, DomainError> {
        if !matches!(operation, Operation::MoveRange { .. }) {
            return Err(DomainError::InvalidValue);
        }
        let next_version = DocumentVersion::new(
            self.version
                .get()
                .checked_add(1)
                .ok_or(DomainError::LimitExceeded)?,
        )?;
        let mut candidate = self.candidate(operation, next_version)?;
        candidate.version = self.version;
        Ok(candidate)
    }
    /// Atomic preview-only patch over immutable axes. Does not establish server
    /// authority or commit any operation. Equal versions represent pending edits;
    /// larger versions allow coalesced confirmed patches supplied by the adapter.
    /// Durable writers must use `candidate`/`apply_event` with strict next versions.
    /// Duplicate coordinates are rejected, including clear/blank operations.
    pub fn preview_candidate(
        &self,
        operations: &[Operation],
        target_version: DocumentVersion,
    ) -> Result<Self, DomainError> {
        if target_version < self.version || self.sheets.is_empty() {
            return Err(DomainError::VersionConflict);
        }
        if operations.len() > MAX_CELLS {
            return Err(DomainError::LimitExceeded);
        }
        let mut seen = BTreeSet::new();
        for operation in operations {
            let address = match operation {
                Operation::CreateWorkbook { .. } => return Err(DomainError::VersionConflict),
                Operation::AddSheet { .. } => return Err(DomainError::VersionConflict),
                Operation::ExtendRows { .. } => return Err(DomainError::VersionConflict),
                Operation::InsertRows { .. } => return Err(DomainError::VersionConflict),
                Operation::InsertRowsRebased { .. } => return Err(DomainError::VersionConflict),
                Operation::DeleteRows { .. } => return Err(DomainError::VersionConflict),
                Operation::InsertColumnsRebased { .. } => {
                    return Err(DomainError::VersionConflict);
                }
                Operation::DeleteColumns { .. } => return Err(DomainError::VersionConflict),
                Operation::AdvanceCalculationEpoch { .. } => {
                    return Err(DomainError::VersionConflict);
                }
                Operation::SetCellValue { address, .. }
                | Operation::SetCellFormula { address, .. }
                | Operation::ClearCell { address }
                | Operation::CellWrite { address, .. } => *address,
                Operation::SetCellValues { .. } => return Err(DomainError::InvalidAxis),
                Operation::CopyRange { .. }
                | Operation::RangeWrite { .. }
                | Operation::MoveRange { .. }
                | Operation::MoveCompensation { .. } => {
                    return Err(DomainError::InvalidAxis);
                }
            };
            if !seen.insert(address) {
                return Err(DomainError::InvalidAxis);
            }
            if matches!(operation, Operation::CellWrite { .. }) {
                self.validate_guarded_write(operation)?;
            }
        }
        let mut next = self.clone();
        next.version = target_version;
        // Release all replaced allocations in this private candidate first.
        // Atomic batches must be limited by their final state, not by the order
        // in which additions and removals happen to appear in the payload.
        for address in seen {
            let clear = Operation::ClearCell { address };
            let budgets = next.operation_budgets(&clear)?;
            next.apply_operation(&clear, budgets, target_version)?;
        }
        for operation in operations {
            // The guard was checked against the original projection above.
            // Clearing candidate destinations must not invalidate its preimage.
            let replacement;
            let operation = if let Operation::CellWrite {
                address, content, ..
            } = operation
            {
                replacement = match content {
                    Some(CellContent::Value(value)) => Operation::SetCellValue {
                        address: *address,
                        value: value.clone(),
                    },
                    Some(CellContent::Formula(formula)) => Operation::SetCellFormula {
                        address: *address,
                        formula: formula.clone(),
                    },
                    None => Operation::ClearCell { address: *address },
                };
                &replacement
            } else {
                operation
            };
            let budgets = next.operation_budgets(operation)?;
            next.apply_operation(operation, budgets, target_version)?;
        }
        Ok(next)
    }
    fn move_rectangle(
        &self,
        origin: CellAddress,
        rows: u32,
        columns: u32,
    ) -> Result<Vec<CellAddress>, DomainError> {
        if rows == 0 || columns == 0 || u64::from(rows) * u64::from(columns) > 1_000 {
            return Err(DomainError::LimitExceeded);
        }
        let sheet = self
            .sheets
            .iter()
            .find(|sheet| sheet.id == origin.sheet)
            .ok_or(DomainError::MissingSheet)?;
        let row = sheet
            .row_position(origin.row)
            .ok_or(DomainError::MissingRow)? as usize;
        let column = sheet
            .column_position(origin.column)
            .ok_or(DomainError::MissingColumn)? as usize;
        let row_end = row
            .checked_add(rows as usize)
            .ok_or(DomainError::LimitExceeded)?;
        let column_end = column
            .checked_add(columns as usize)
            .ok_or(DomainError::LimitExceeded)?;
        let rows = sheet
            .rows
            .get(row..row_end)
            .ok_or(DomainError::MissingRow)?;
        let columns = sheet
            .columns
            .get(column..column_end)
            .ok_or(DomainError::MissingColumn)?;
        Ok(rows
            .iter()
            .flat_map(|row| {
                columns.iter().map(move |column| CellAddress {
                    sheet: origin.sheet,
                    row: *row,
                    column: *column,
                })
            })
            .collect())
    }
    fn move_candidate(
        &self,
        source: CellAddress,
        destination: CellAddress,
        rows: u32,
        columns: u32,
        writes: &[GuardedContentWrite],
        next_version: DocumentVersion,
    ) -> Result<Self, DomainError> {
        if source.sheet != destination.sheet || source == destination {
            return Err(DomainError::InvalidAxis);
        }
        if writes.is_empty() || writes.len() > 10_000 {
            return Err(DomainError::LimitExceeded);
        }
        let source = self.move_rectangle(source, rows, columns)?;
        let destination = self.move_rectangle(destination, rows, columns)?;
        let remap: BTreeMap<_, _> = source
            .iter()
            .copied()
            .zip(destination.iter().copied())
            .collect();
        let first = *source.first().ok_or(DomainError::InvalidAxis)?;
        let last = *source.last().ok_or(DomainError::InvalidAxis)?;
        let (r1, c1) = self.move_cell_position(first)?;
        let (r2, c2) = self.move_cell_position(last)?;
        let source_bounds = (first.sheet, r1, r2, c1, c2);
        let mut by_address = BTreeMap::new();
        let mut incoming = 0usize;
        let mut previous = 0usize;
        for write in writes {
            if by_address.insert(write.address, write).is_some() {
                return Err(DomainError::InvalidAxis);
            }
            incoming = incoming
                .checked_add(write.content.as_ref().map_or(0, CellContent::text_bytes))
                .ok_or(DomainError::LimitExceeded)?;
            previous = previous
                .checked_add(write.previous.as_ref().map_or(0, CellContent::text_bytes))
                .ok_or(DomainError::LimitExceeded)?;
            if incoming > MAX_CELL_VALUE_BATCH_TEXT_BYTES
                || previous > MAX_CELL_VALUE_BATCH_TEXT_BYTES
            {
                return Err(DomainError::LimitExceeded);
            }
        }
        let source_set: BTreeSet<_> = source.iter().copied().collect();
        let destination_set: BTreeSet<_> = destination.iter().copied().collect();
        for address in source_set.union(&destination_set) {
            if !by_address.contains_key(address) {
                return Err(DomainError::InvalidAxis);
            }
        }
        for (from, to) in source.iter().zip(&destination) {
            let old = &by_address
                .get(from)
                .ok_or(DomainError::InvalidAxis)?
                .previous;
            let new = &by_address.get(to).ok_or(DomainError::InvalidAxis)?.content;
            match (old, new) {
                (None, None) => {}
                (Some(CellContent::Value(before)), Some(CellContent::Value(after)))
                    if before == after => {}
                (Some(CellContent::Formula(before)), Some(CellContent::Formula(formula))) => {
                    if formula.origin() != *to
                        || formula.bound_at_version() >= next_version
                        || before.source() != formula.source()
                        || before.canonical() != formula.canonical()
                    {
                        return Err(DomainError::InvalidFormula);
                    }
                    self.validate_moved_formula(before, formula, &remap, source_bounds)?;
                }
                _ => return Err(DomainError::InvalidValue),
            }
        }
        // Replay must reject an omitted inbound rewrite as well as a forged
        // included one. Comparing an unchanged formula to its expected mapping
        // detects both affected scalar targets and unsupported partial ranges.
        let mut visited_nodes = 0usize;
        for sheet in &self.sheets {
            for (address, content) in &sheet.cells {
                if source_set.contains(address) || destination_set.contains(address) {
                    continue;
                }
                if let CellContent::Formula(formula) = content {
                    visited_nodes = visited_nodes
                        .checked_add(formula.ast().nodes().len())
                        .filter(|nodes| *nodes <= 1_000_000)
                        .ok_or(DomainError::LimitExceeded)?;
                    if !by_address.contains_key(address) {
                        self.validate_moved_formula(formula, formula, &remap, source_bounds)?;
                    }
                }
            }
        }
        let mut operations = Vec::with_capacity(writes.len());
        for write in writes {
            if source_set.contains(&write.address)
                && !destination_set.contains(&write.address)
                && write.content.is_some()
            {
                return Err(DomainError::InvalidValue);
            }
            if !source_set.contains(&write.address) && !destination_set.contains(&write.address) {
                match (&write.previous, &write.content) {
                    (Some(CellContent::Formula(old)), Some(CellContent::Formula(new)))
                        if old.ast() != new.ast()
                            && old.source() == new.source()
                            && old.canonical() == new.canonical() =>
                    {
                        self.validate_moved_formula(old, new, &remap, source_bounds)?;
                    }
                    _ => return Err(DomainError::InvalidFormula),
                }
            }
            let kind = Some(match &write.content {
                None => CellWriteKind::Clear,
                Some(CellContent::Value(_)) => CellWriteKind::SetValue,
                Some(CellContent::Formula(_)) => CellWriteKind::SetFormula,
            });
            let operation = Operation::CellWrite {
                address: write.address,
                content: write.content.clone(),
                previous: write.previous.clone(),
                kind,
                undoes: None,
                undoes_version: None,
            };
            self.validate_guarded_write(&operation)?;
            operations.push(operation);
        }
        self.preview_candidate(&operations, next_version)
    }
    fn move_cell_position(&self, address: CellAddress) -> Result<(u32, u32), DomainError> {
        let sheet = self
            .sheets
            .iter()
            .find(|sheet| sheet.id == address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        Ok((
            sheet
                .row_position(address.row)
                .ok_or(DomainError::MissingRow)?,
            sheet
                .column_position(address.column)
                .ok_or(DomainError::MissingColumn)?,
        ))
    }
    fn expected_move_reference(
        &self,
        reference: &BoundCellReference,
        origin: CellAddress,
        remap: &BTreeMap<CellAddress, CellAddress>,
    ) -> Result<BoundCellReference, DomainError> {
        let old = CellAddress {
            sheet: reference.target_sheet,
            row: reference.target_row,
            column: reference.target_column,
        };
        // Exactly one lookup: overlapping destinations must not move twice.
        let target = remap.get(&old).copied().unwrap_or(old);
        let (row, column) = self.move_cell_position(target)?;
        let (origin_row, origin_column) = self.move_cell_position(origin)?;
        Ok(BoundCellReference {
            target_sheet: target.sheet,
            target_row: target.row,
            target_column: target.column,
            row_mode: match reference.row_mode {
                AxisReferenceMode::Absolute => AxisReferenceMode::Absolute,
                AxisReferenceMode::Relative { .. } => AxisReferenceMode::Relative {
                    original_delta: i64::from(row) - i64::from(origin_row),
                },
            },
            column_mode: match reference.column_mode {
                AxisReferenceMode::Absolute => AxisReferenceMode::Absolute,
                AxisReferenceMode::Relative { .. } => AxisReferenceMode::Relative {
                    original_delta: i64::from(column) - i64::from(origin_column),
                },
            },
        })
    }
    fn validate_moved_formula(
        &self,
        old: &Formula,
        new: &Formula,
        remap: &BTreeMap<CellAddress, CellAddress>,
        source_bounds: (SheetId, u32, u32, u32, u32),
    ) -> Result<(), DomainError> {
        if old.source() != new.source()
            || old.canonical() != new.canonical()
            || old.profile() != new.profile()
            || old.ast().schema_version() != new.ast().schema_version()
            || old.ast().root() != new.ast().root()
            || old.ast().nodes().len() != new.ast().nodes().len()
        {
            return Err(DomainError::InvalidFormula);
        }
        for (before, after) in old.ast().nodes().iter().zip(new.ast().nodes()) {
            let valid = match (before, after) {
                (Node::Cell(before), Node::Cell(after)) => {
                    self.expected_move_reference(before, new.origin(), remap)? == *after
                }
                (Node::Range(before), Node::Range(after)) => {
                    let (ra, ca) = self.reference_position(&before.start)?;
                    let (rb, cb) = self.reference_position(&before.end)?;
                    let (sheet, r1, r2, c1, c2) = source_bounds;
                    let intersects = before.start.target_sheet == sheet
                        && ra.min(rb) <= r2
                        && ra.max(rb) >= r1
                        && ca.min(cb) <= c2
                        && ca.max(cb) >= c1;
                    let contained = ra.min(rb) >= r1
                        && ra.max(rb) <= r2
                        && ca.min(cb) >= c1
                        && ca.max(cb) <= c2;
                    if intersects && !contained {
                        return Err(DomainError::InvalidFormula);
                    }
                    self.expected_move_reference(&before.start, new.origin(), remap)? == after.start
                        && self.expected_move_reference(&before.end, new.origin(), remap)?
                            == after.end
                }
                _ => before == after,
            };
            if !valid {
                return Err(DomainError::InvalidFormula);
            }
        }
        Ok(())
    }
    fn operation_budgets(&self, operation: &Operation) -> Result<(usize, usize), DomainError> {
        // Check the incoming value and aggregate budget before cloning the existing
        // projection. Replacements subtract their former allocation in constant time.
        Ok(match operation {
            Operation::CreateWorkbook { .. } => (self.text_bytes, self.ast_memory_bytes),
            Operation::AddSheet { sheet } => {
                self.validate_new_sheet(sheet)?;
                (self.text_bytes, self.ast_memory_bytes)
            }
            Operation::ExtendRows { sheet, rows } => {
                self.validate_new_rows(rows)?;
                self.sheets
                    .iter()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?
                    .validate_row_extension(rows)?;
                (self.text_bytes, self.ast_memory_bytes)
            }
            Operation::InsertRows {
                sheet,
                before,
                rows,
            } => {
                self.validate_new_rows(rows)?;
                let target = self
                    .sheets
                    .iter()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                target
                    .row_position(*before)
                    .ok_or(DomainError::MissingRow)?;
                target.validate_row_extension(rows)?;
                if self.sheets.iter().any(|sheet| {
                    sheet
                        .cells
                        .values()
                        .any(|cell| matches!(cell, CellContent::Formula(_)))
                }) {
                    return Err(DomainError::UnsupportedStructuralDependency);
                }
                (self.text_bytes, self.ast_memory_bytes)
            }
            Operation::InsertRowsRebased {
                sheet,
                before,
                rows,
            } => {
                self.validate_new_rows(rows)?;
                let target = self
                    .sheets
                    .iter()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                target
                    .row_position(*before)
                    .ok_or(DomainError::MissingRow)?;
                target.validate_row_extension(rows)?;
                (self.text_bytes, self.ast_memory_bytes)
            }
            Operation::DeleteRows { sheet, rows } => {
                let target = self
                    .sheets
                    .iter()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                let deleted = target.validate_row_deletion(rows)?;
                if self.retired_row_ids.len().saturating_add(deleted.len()) > MAX_RETIRED_ROWS {
                    return Err(DomainError::LimitExceeded);
                }
                let removed_text = target
                    .cells
                    .iter()
                    .filter(|(address, _)| deleted.contains(&address.row))
                    .map(|(_, content)| content.text_bytes())
                    .sum::<usize>();
                let removed_ast = target
                    .cells
                    .iter()
                    .filter(|(address, _)| deleted.contains(&address.row))
                    .map(|(_, content)| content.ast_memory_bytes())
                    .sum::<usize>();
                (
                    self.text_bytes
                        .checked_sub(removed_text)
                        .ok_or(DomainError::InvalidFormula)?,
                    self.ast_memory_bytes
                        .checked_sub(removed_ast)
                        .ok_or(DomainError::InvalidFormula)?,
                )
            }
            Operation::InsertColumnsRebased {
                sheet,
                before,
                columns,
            } => {
                self.validate_new_columns(columns)?;
                let target = self
                    .sheets
                    .iter()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                target.validate_column_extension(columns)?;
                target
                    .column_position(*before)
                    .ok_or(DomainError::MissingColumn)?;
                (self.text_bytes, self.ast_memory_bytes)
            }
            Operation::DeleteColumns { sheet, columns } => {
                let target = self
                    .sheets
                    .iter()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                let deleted = target.validate_column_deletion(columns)?;
                if self.retired_column_ids.len().saturating_add(deleted.len()) > MAX_RETIRED_COLUMNS
                {
                    return Err(DomainError::LimitExceeded);
                }
                let removed_text = target
                    .cells
                    .iter()
                    .filter(|(address, _)| deleted.contains(&address.column))
                    .map(|(_, content)| content.text_bytes())
                    .sum::<usize>();
                let removed_ast = target
                    .cells
                    .iter()
                    .filter(|(address, _)| deleted.contains(&address.column))
                    .map(|(_, content)| content.ast_memory_bytes())
                    .sum::<usize>();
                (
                    self.text_bytes
                        .checked_sub(removed_text)
                        .ok_or(DomainError::InvalidFormula)?,
                    self.ast_memory_bytes
                        .checked_sub(removed_ast)
                        .ok_or(DomainError::InvalidFormula)?,
                )
            }
            Operation::AdvanceCalculationEpoch { context } => {
                let expected_epoch = self
                    .calculation_context
                    .as_ref()
                    .map_or(Some(1), |previous| previous.epoch().checked_add(1));
                if self.sheets.is_empty()
                    || expected_epoch != Some(context.epoch())
                    || self
                        .calculation_context
                        .as_ref()
                        .is_some_and(|previous| previous.context_id() == context.context_id())
                {
                    return Err(DomainError::InvalidCalculationContext);
                }
                (self.text_bytes, self.ast_memory_bytes)
            }
            Operation::SetCellValue { address, value } => {
                value.validate()?;
                self.updated_budgets(*address, value.text_bytes(), 0)?
            }
            Operation::SetCellValues { .. } => return Err(DomainError::InvalidAxis),
            Operation::CopyRange { .. }
            | Operation::RangeWrite { .. }
            | Operation::MoveRange { .. }
            | Operation::MoveCompensation { .. } => {
                return Err(DomainError::InvalidAxis);
            }
            Operation::SetCellFormula { address, formula } => {
                self.validate_formula(*address, formula)?;
                self.updated_budgets(*address, formula.text_bytes(), formula.ast_memory_bytes())?
            }
            Operation::ClearCell { address } => self.updated_budgets(*address, 0, 0)?,
            Operation::CellWrite {
                address, content, ..
            } => {
                self.validate_guarded_write(operation)?;
                self.updated_budgets(
                    *address,
                    content.as_ref().map_or(0, CellContent::text_bytes),
                    content.as_ref().map_or(0, CellContent::ast_memory_bytes),
                )?
            }
        })
    }
    fn validate_guarded_write(&self, operation: &Operation) -> Result<(), DomainError> {
        let Operation::CellWrite {
            address,
            content,
            previous,
            kind,
            undoes,
            undoes_version,
        } = operation
        else {
            return Err(DomainError::InvalidValue);
        };
        if undoes.is_some() != undoes_version.is_some()
            || undoes_version.is_some_and(|version| version > self.version)
            || (kind.is_some() == undoes.is_some())
        {
            return Err(DomainError::VersionConflict);
        }
        match kind {
            Some(CellWriteKind::SetValue)
                if content
                    .as_ref()
                    .is_some_and(|content| !matches!(content, CellContent::Value(_))) =>
            {
                return Err(DomainError::InvalidValue);
            }
            Some(CellWriteKind::Clear) if content.is_some() => {
                return Err(DomainError::InvalidValue);
            }
            Some(CellWriteKind::SetFormula)
                if !matches!(content, Some(CellContent::Formula(_))) =>
            {
                return Err(DomainError::InvalidValue);
            }
            _ => {}
        }
        if self.cell_content(*address)? != previous.as_ref() {
            return Err(DomainError::PreimageConflict);
        }
        match content {
            Some(CellContent::Value(CellValue::Blank)) => {
                return Err(DomainError::InvalidValue);
            }
            Some(CellContent::Value(value)) => {
                value.validate()?;
            }
            Some(CellContent::Formula(formula)) => {
                self.validate_formula(*address, formula)?;
            }
            None => {}
        }
        Ok(())
    }
    fn validate_new_rows(&self, rows: &[RowId]) -> Result<(), DomainError> {
        if rows.iter().any(|row| {
            self.retired_row_ids.contains(row)
                || self
                    .sheets
                    .iter()
                    .any(|sheet| sheet.row_positions.contains_key(row))
        }) {
            return Err(DomainError::InvalidAxis);
        }
        Ok(())
    }
    fn validate_new_sheet(&self, seed: &SheetSeed) -> Result<(), DomainError> {
        if self.sheets.is_empty()
            || self.sheets.len() >= 64
            || self.sheets.iter().any(|sheet| {
                sheet.id == seed.id || sheet.name.to_lowercase() == seed.name.to_lowercase()
            })
        {
            return Err(DomainError::InvalidAxis);
        }
        Sheet::from_seed(seed)?;
        self.validate_new_rows(&seed.rows)?;
        self.validate_new_columns(&seed.columns)?;
        Ok(())
    }
    fn validate_new_columns(&self, columns: &[ColumnId]) -> Result<(), DomainError> {
        if columns.iter().any(|id| {
            self.retired_column_ids.contains(id)
                || self
                    .sheets
                    .iter()
                    .any(|sheet| sheet.column_positions.contains_key(id))
        }) {
            return Err(DomainError::InvalidAxis);
        }
        Ok(())
    }
    fn apply_operation(
        &mut self,
        operation: &Operation,
        (text_bytes, ast_memory_bytes): (usize, usize),
        next_version: DocumentVersion,
    ) -> Result<(), DomainError> {
        match operation {
            Operation::AdvanceCalculationEpoch { context } => {
                self.calculation_context = Some(context.clone());
            }
            Operation::CreateWorkbook { sheet } => {
                if self.version != DocumentVersion::ZERO || !self.sheets.is_empty() {
                    return Err(DomainError::VersionConflict);
                }
                self.sheets.push(Sheet::from_seed(sheet)?);
            }
            Operation::AddSheet { sheet } => {
                self.validate_new_sheet(sheet)?;
                self.sheets.push(Sheet::from_seed(sheet)?);
            }
            Operation::ExtendRows { sheet, rows } => {
                let target = self
                    .sheets
                    .iter_mut()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                target.validate_row_extension(rows)?;
                for row in rows {
                    target.row_positions.insert(*row, target.rows.len() as u32);
                    target.rows.push(*row);
                }
            }
            Operation::InsertRows {
                sheet,
                before,
                rows,
            }
            | Operation::InsertRowsRebased {
                sheet,
                before,
                rows,
            } => {
                let target = self
                    .sheets
                    .iter_mut()
                    .find(|current| current.id == *sheet)
                    .ok_or(DomainError::MissingSheet)?;
                target.validate_row_extension(rows)?;
                let position = target
                    .row_position(*before)
                    .ok_or(DomainError::MissingRow)? as usize;
                target.rows.splice(position..position, rows.iter().copied());
                target.row_positions = target
                    .rows
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(index, id)| (id, index as u32))
                    .collect();
                if matches!(operation, Operation::InsertRowsRebased { .. }) {
                    let mut replacements = Vec::new();
                    for (index, sheet) in self.sheets.iter().enumerate() {
                        for (address, content) in &sheet.cells {
                            let CellContent::Formula(formula) = content else {
                                continue;
                            };
                            let origin_position = sheet
                                .row_position(address.row)
                                .ok_or(DomainError::MissingRow)?;
                            let rebased = formula
                                .rebase_row_offsets(next_version, origin_position, |id, row| {
                                    self.sheets
                                        .iter()
                                        .find(|sheet| sheet.id == id)
                                        .and_then(|sheet| sheet.row_position(row))
                                })
                                .map_err(|_| DomainError::InvalidFormula)?;
                            replacements.push((index, *address, rebased));
                        }
                    }
                    for (index, address, formula) in replacements {
                        let sheet = self
                            .sheets
                            .get_mut(index)
                            .ok_or(DomainError::MissingSheet)?;
                        sheet.cells.insert(address, CellContent::Formula(formula));
                    }
                }
            }
            Operation::DeleteRows { sheet, rows } => {
                self.apply_row_deletion(*sheet, rows, next_version)?;
            }
            Operation::InsertColumnsRebased {
                sheet,
                before,
                columns,
            } => {
                self.apply_column_insertion(*sheet, *before, columns, next_version)?;
            }
            Operation::DeleteColumns { sheet, columns } => {
                self.apply_column_deletion(*sheet, columns, next_version)?;
            }
            Operation::SetCellValue { address, value } => {
                let normalized = value.normalized()?;
                let sheet = self
                    .sheets
                    .iter_mut()
                    .find(|s| s.id == address.sheet)
                    .ok_or(DomainError::MissingSheet)?;
                sheet.validate_address(*address)?;
                if normalized == CellValue::Blank {
                    sheet.cells.remove(address);
                } else {
                    if sheet.cells.len() >= MAX_CELLS && !sheet.cells.contains_key(address) {
                        return Err(DomainError::LimitExceeded);
                    }
                    sheet.cells.insert(*address, CellContent::Value(normalized));
                }
            }
            Operation::SetCellValues { .. } => return Err(DomainError::InvalidAxis),
            Operation::CopyRange { .. }
            | Operation::RangeWrite { .. }
            | Operation::MoveRange { .. }
            | Operation::MoveCompensation { .. } => {
                return Err(DomainError::InvalidAxis);
            }
            Operation::SetCellFormula { address, formula } => {
                let sheet = self
                    .sheets
                    .iter_mut()
                    .find(|sheet| sheet.id == address.sheet)
                    .ok_or(DomainError::MissingSheet)?;
                if sheet.cells.len() >= MAX_CELLS && !sheet.cells.contains_key(address) {
                    return Err(DomainError::LimitExceeded);
                }
                sheet
                    .cells
                    .insert(*address, CellContent::Formula(formula.clone()));
            }
            Operation::ClearCell { address } => {
                let sheet = self
                    .sheets
                    .iter_mut()
                    .find(|s| s.id == address.sheet)
                    .ok_or(DomainError::MissingSheet)?;
                sheet.validate_address(*address)?;
                sheet.cells.remove(address);
            }
            Operation::CellWrite {
                address, content, ..
            } => {
                let sheet = self
                    .sheets
                    .iter_mut()
                    .find(|sheet| sheet.id == address.sheet)
                    .ok_or(DomainError::MissingSheet)?;
                if let Some(content) = content {
                    if sheet.cells.len() >= MAX_CELLS && !sheet.cells.contains_key(address) {
                        return Err(DomainError::LimitExceeded);
                    }
                    let normalized = match content {
                        CellContent::Value(value) => CellContent::Value(value.normalized()?),
                        CellContent::Formula(formula) => CellContent::Formula(formula.clone()),
                    };
                    sheet.cells.insert(*address, normalized);
                } else {
                    sheet.cells.remove(address);
                }
            }
        }
        self.text_bytes = text_bytes;
        self.ast_memory_bytes = ast_memory_bytes;
        Ok(())
    }
    fn apply_column_insertion(
        &mut self,
        sheet_id: SheetId,
        before: ColumnId,
        columns: &[ColumnId],
        next_version: DocumentVersion,
    ) -> Result<(), DomainError> {
        let target = self
            .sheets
            .iter_mut()
            .find(|sheet| sheet.id == sheet_id)
            .ok_or(DomainError::MissingSheet)?;
        target.validate_column_extension(columns)?;
        let position = target
            .column_position(before)
            .ok_or(DomainError::MissingColumn)? as usize;
        target
            .columns
            .splice(position..position, columns.iter().copied());
        target.column_positions = target
            .columns
            .iter()
            .copied()
            .enumerate()
            .map(|(position, id)| (id, position as u32))
            .collect();
        let mut replacements = Vec::new();
        for (sheet_index, sheet) in self.sheets.iter().enumerate() {
            for (address, content) in &sheet.cells {
                let CellContent::Formula(formula) = content else {
                    continue;
                };
                let origin_position = sheet
                    .column_position(address.column)
                    .ok_or(DomainError::MissingColumn)?;
                let rebased = formula
                    .rebase_column_offsets(next_version, origin_position, |id, column| {
                        self.sheets
                            .iter()
                            .find(|sheet| sheet.id == id)
                            .and_then(|sheet| sheet.column_position(column))
                    })
                    .map_err(|_| DomainError::InvalidFormula)?;
                replacements.push((sheet_index, *address, rebased));
            }
        }
        for (sheet_index, address, formula) in replacements {
            let sheet = self
                .sheets
                .get_mut(sheet_index)
                .ok_or(DomainError::MissingSheet)?;
            sheet.cells.insert(address, CellContent::Formula(formula));
        }
        Ok(())
    }
    fn apply_column_deletion(
        &mut self,
        sheet_id: SheetId,
        columns: &[ColumnId],
        next_version: DocumentVersion,
    ) -> Result<(), DomainError> {
        let index = self
            .sheets
            .iter()
            .position(|sheet| sheet.id == sheet_id)
            .ok_or(DomainError::MissingSheet)?;
        let sheet = self.sheets.get(index).ok_or(DomainError::MissingSheet)?;
        let deleted = sheet.validate_column_deletion(columns)?;
        let old_columns = sheet.columns.clone();
        let old_positions = sheet.column_positions.clone();
        let mut next_alive = vec![None; old_columns.len()];
        let mut survivor = None;
        for position in (0..old_columns.len()).rev() {
            if !deleted.contains(&old_columns[position]) {
                survivor = Some(old_columns[position]);
            }
            next_alive[position] = survivor;
        }
        let mut previous_alive = vec![None; old_columns.len()];
        survivor = None;
        for (position, column) in old_columns.iter().copied().enumerate() {
            if !deleted.contains(&column) {
                survivor = Some(column);
            }
            previous_alive[position] = survivor;
        }
        let target = self
            .sheets
            .get_mut(index)
            .ok_or(DomainError::MissingSheet)?;
        target.columns.retain(|column| !deleted.contains(column));
        target.column_positions = target
            .columns
            .iter()
            .copied()
            .enumerate()
            .map(|(position, column)| (column, position as u32))
            .collect();
        target
            .cells
            .retain(|address, _| !deleted.contains(&address.column));

        let mut replacements = Vec::new();
        for (sheet_index, sheet) in self.sheets.iter().enumerate() {
            for (address, content) in &sheet.cells {
                let CellContent::Formula(formula) = content else {
                    continue;
                };
                let origin_position = sheet
                    .column_position(address.column)
                    .ok_or(DomainError::MissingColumn)?;
                let transformed = formula
                    .rebase_deleted_columns(
                        next_version,
                        origin_position,
                        sheet_id,
                        &deleted,
                        |id, column| {
                            self.sheets
                                .iter()
                                .find(|sheet| sheet.id == id)
                                .and_then(|sheet| sheet.column_position(column))
                        },
                        |start, end| {
                            let first = *old_positions.get(&start)? as usize;
                            let last = *old_positions.get(&end)? as usize;
                            let low = first.min(last);
                            let high = first.max(last);
                            let low_survivor = next_alive.get(low).copied().flatten()?;
                            let high_survivor = previous_alive.get(high).copied().flatten()?;
                            if (*old_positions.get(&low_survivor)? as usize) > high
                                || (*old_positions.get(&high_survivor)? as usize) < low
                            {
                                return None;
                            }
                            Some(if first <= last {
                                (low_survivor, high_survivor)
                            } else {
                                (high_survivor, low_survivor)
                            })
                        },
                    )
                    .map_err(|_| DomainError::InvalidFormula)?;
                replacements.push((sheet_index, *address, transformed));
            }
        }
        for (sheet_index, address, formula) in replacements {
            let sheet = self
                .sheets
                .get_mut(sheet_index)
                .ok_or(DomainError::MissingSheet)?;
            sheet.cells.insert(address, CellContent::Formula(formula));
        }
        self.retired_column_ids.extend(deleted);
        Ok(())
    }
    fn apply_row_deletion(
        &mut self,
        sheet_id: SheetId,
        rows: &[RowId],
        next_version: DocumentVersion,
    ) -> Result<(), DomainError> {
        let index = self
            .sheets
            .iter()
            .position(|sheet| sheet.id == sheet_id)
            .ok_or(DomainError::MissingSheet)?;
        let sheet = self.sheets.get(index).ok_or(DomainError::MissingSheet)?;
        let deleted = sheet.validate_row_deletion(rows)?;
        let old_rows = sheet.rows.clone();
        let old_positions = sheet.row_positions.clone();
        let mut next_alive = vec![None; old_rows.len()];
        let mut survivor = None;
        for position in (0..old_rows.len()).rev() {
            if !deleted.contains(&old_rows[position]) {
                survivor = Some(old_rows[position]);
            }
            next_alive[position] = survivor;
        }
        let mut previous_alive = vec![None; old_rows.len()];
        survivor = None;
        for (position, row) in old_rows.iter().copied().enumerate() {
            if !deleted.contains(&row) {
                survivor = Some(row);
            }
            previous_alive[position] = survivor;
        }
        let target = self
            .sheets
            .get_mut(index)
            .ok_or(DomainError::MissingSheet)?;
        target.rows.retain(|row| !deleted.contains(row));
        target.row_positions = target
            .rows
            .iter()
            .copied()
            .enumerate()
            .map(|(position, row)| (row, position as u32))
            .collect();
        target
            .cells
            .retain(|address, _| !deleted.contains(&address.row));

        let mut replacements = Vec::new();
        for (sheet_index, sheet) in self.sheets.iter().enumerate() {
            for (address, content) in &sheet.cells {
                let CellContent::Formula(formula) = content else {
                    continue;
                };
                let origin_position = sheet
                    .row_position(address.row)
                    .ok_or(DomainError::MissingRow)?;
                let transformed = formula
                    .rebase_deleted_rows(
                        next_version,
                        origin_position,
                        sheet_id,
                        &deleted,
                        |id, row| {
                            self.sheets
                                .iter()
                                .find(|sheet| sheet.id == id)
                                .and_then(|sheet| sheet.row_position(row))
                        },
                        |start, end| {
                            let first = *old_positions.get(&start)? as usize;
                            let last = *old_positions.get(&end)? as usize;
                            let low = first.min(last);
                            let high = first.max(last);
                            let low_survivor = next_alive.get(low).copied().flatten()?;
                            let high_survivor = previous_alive.get(high).copied().flatten()?;
                            if (*old_positions.get(&low_survivor)? as usize) > high
                                || (*old_positions.get(&high_survivor)? as usize) < low
                            {
                                return None;
                            }
                            Some(if first <= last {
                                (low_survivor, high_survivor)
                            } else {
                                (high_survivor, low_survivor)
                            })
                        },
                    )
                    .map_err(|_| DomainError::InvalidFormula)?;
                replacements.push((sheet_index, *address, transformed));
            }
        }
        for (sheet_index, address, formula) in replacements {
            let sheet = self
                .sheets
                .get_mut(sheet_index)
                .ok_or(DomainError::MissingSheet)?;
            sheet.cells.insert(address, CellContent::Formula(formula));
        }
        self.retired_row_ids.extend(deleted);
        Ok(())
    }
    fn updated_budgets(
        &self,
        address: CellAddress,
        replacement_bytes: usize,
        replacement_ast_bytes: usize,
    ) -> Result<(usize, usize), DomainError> {
        let sheet = self
            .sheets
            .iter()
            .find(|sheet| sheet.id == address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        sheet.validate_address(address)?;
        let previous_bytes = sheet.cells.get(&address).map_or(0, CellContent::text_bytes);
        let previous_ast_bytes = sheet
            .cells
            .get(&address)
            .map_or(0, CellContent::ast_memory_bytes);
        let text_bytes = self
            .text_bytes
            .checked_sub(previous_bytes)
            .and_then(|size| size.checked_add(replacement_bytes))
            .filter(|size| *size <= MAX_WORKBOOK_TEXT_BYTES)
            .ok_or(DomainError::LimitExceeded)?;
        let ast_bytes = self
            .ast_memory_bytes
            .checked_sub(previous_ast_bytes)
            .and_then(|size| size.checked_add(replacement_ast_bytes))
            .filter(|size| *size <= MAX_WORKBOOK_AST_BYTES)
            .ok_or(DomainError::LimitExceeded)?;
        Ok((text_bytes, ast_bytes))
    }
    fn reference_position(
        &self,
        reference: &BoundCellReference,
    ) -> Result<(u32, u32), DomainError> {
        let sheet = self
            .sheets
            .iter()
            .find(|sheet| sheet.id == reference.target_sheet)
            .ok_or(DomainError::MissingSheet)?;
        Ok((
            sheet
                .row_position(reference.target_row)
                .ok_or(DomainError::MissingRow)?,
            sheet
                .column_position(reference.target_column)
                .ok_or(DomainError::MissingColumn)?,
        ))
    }
    fn validate_formula(&self, address: CellAddress, formula: &Formula) -> Result<(), DomainError> {
        if formula.origin() != address || formula.bound_at_version() > self.version {
            return Err(DomainError::InvalidFormula);
        }
        for node in formula.ast().nodes() {
            match node {
                Node::Cell(reference) => {
                    self.reference_position(reference)?;
                }
                Node::Range(range) => {
                    let (r1, c1) = self.reference_position(&range.start)?;
                    let (r2, c2) = self.reference_position(&range.end)?;
                    if (u64::from(r1.abs_diff(r2)) + 1) * (u64::from(c1.abs_diff(c2)) + 1) > 100_000
                    {
                        return Err(DomainError::LimitExceeded);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn apply_event(&mut self, event: &CommittedEvent) -> Result<(), DomainError> {
        if event.document_id != self.document_id {
            return Err(DomainError::InvalidId);
        }
        if matches!(event.principal, EventPrincipal::System)
            != matches!(event.operation, Operation::AdvanceCalculationEpoch { .. })
        {
            return Err(DomainError::InvalidCalculationContext);
        }
        let candidate = self.candidate(&event.operation, event.version)?;
        if candidate.hash() != event.state_hash {
            return Err(DomainError::HashMismatch);
        }
        *self = candidate;
        Ok(())
    }
    /// Length-prefixed UTF-8 and big-endian integers; sheets/axes in visible order,
    /// sparse cells in stable-ID order. No calculated cache or wire bytes.
    pub fn hash(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"PXEL-DOMAIN-1\0");
        hash.update(self.document_id.as_bytes());
        hash.update(self.version.get().to_be_bytes());
        hash.update((self.sheets.len() as u64).to_be_bytes());
        for sheet in &self.sheets {
            hash.update(sheet.id.as_bytes());
            hash.update((sheet.name.len() as u64).to_be_bytes());
            hash.update(sheet.name.as_bytes());
            hash.update((sheet.rows.len() as u64).to_be_bytes());
            for id in &sheet.rows {
                hash.update(id.as_bytes());
            }
            hash.update((sheet.columns.len() as u64).to_be_bytes());
            for id in &sheet.columns {
                hash.update(id.as_bytes());
            }
            hash.update((sheet.cells.len() as u64).to_be_bytes());
            for (address, value) in &sheet.cells {
                hash.update(address.row.as_bytes());
                hash.update(address.column.as_bytes());
                match value {
                    CellContent::Formula(formula) => {
                        hash.update([4]);
                        hash.update(formula.semantic_hash());
                    }
                    CellContent::Value(value) => match value {
                        CellValue::Blank => hash.update([0]),
                        CellValue::Number(n) => {
                            hash.update([1]);
                            hash.update(n.to_bits().to_be_bytes());
                        }
                        CellValue::Text(t) => {
                            hash.update([2]);
                            hash.update((t.len() as u64).to_be_bytes());
                            hash.update(t.as_bytes());
                        }
                        CellValue::Boolean(b) => hash.update([3, u8::from(*b)]),
                    },
                }
            }
        }
        if let Some(context) = &self.calculation_context {
            // Legacy documents never enter this branch, preserving PXEL-DOMAIN-1
            // goldens while context-bearing states gain a domain-separated suffix.
            hash.update(b"PXEL-CONTEXT-1\0");
            hash.update(context.context_id.as_bytes());
            hash.update(context.epoch.to_be_bytes());
            hash.update(context.effective_timestamp_utc_ms.to_be_bytes());
            hash.update(context.utc_offset_minutes.to_be_bytes());
            hash.update(context.civil_day_since_unix_epoch.to_be_bytes());
            hash.update(context.random_seed);
            hash.update((context.engine_version.len() as u64).to_be_bytes());
            hash.update(context.engine_version.as_bytes());
            hash.update((context.compatibility_profile.len() as u64).to_be_bytes());
            hash.update(context.compatibility_profile.as_bytes());
            hash.update([context.reason as u8]);
        }
        if !self.retired_row_ids.is_empty() {
            hash.update(b"PXEL-RETIRED-ROWS-1\0");
            hash.update((self.retired_row_ids.len() as u64).to_be_bytes());
            for id in &self.retired_row_ids {
                hash.update(id.as_bytes());
            }
        }
        if !self.retired_column_ids.is_empty() {
            hash.update(b"PXEL-RETIRED-COLUMNS-1\0");
            hash.update((self.retired_column_ids.len() as u64).to_be_bytes());
            for id in &self.retired_column_ids {
                hash.update(id.as_bytes());
            }
        }
        hash.finalize().into()
    }
}
