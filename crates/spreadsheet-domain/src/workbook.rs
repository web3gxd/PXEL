//! Pure, bounded sparse projection. Canonical hash format is `PXEL-DOMAIN-1`.
pub use crate::formula::FormulaSource;
use crate::formula::{BoundCellReference, Formula, Node};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
};
use thiserror::Error;

pub const MAX_ROWS: usize = 100_000;
pub const MAX_COLUMNS: usize = 1_000;
pub const MAX_CELLS: usize = 100_000;
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
pub enum Operation {
    CreateWorkbook {
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
    SetCellValue {
        address: CellAddress,
        value: CellValue,
    },
    SetCellFormula {
        address: CellAddress,
        formula: Formula,
    },
    ClearCell {
        address: CellAddress,
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
    ExtendRows {
        sheet: SheetId,
        rows: Vec<RowId>,
    },
    InsertRows {
        sheet: SheetId,
        before: RowId,
        rows: Vec<RowId>,
    },
    SetCellValue {
        address: CellAddress,
        value: CellValue,
    },
    SetCellFormula {
        address: CellAddress,
        source: FormulaSource,
    },
    ClearCell {
        address: CellAddress,
    },
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
}

#[derive(Clone)]
pub struct Workbook {
    document_id: DocumentId,
    version: DocumentVersion,
    // Derived by replay; not part of the canonical authored hash.
    last_axis_change_version: DocumentVersion,
    sheets: Vec<Sheet>,
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
    pub fn calculation_context(&self) -> Option<&CalculationContext> {
        self.calculation_context.as_ref()
    }
    pub const fn text_bytes(&self) -> usize {
        self.text_bytes
    }
    pub const fn ast_memory_bytes(&self) -> usize {
        self.ast_memory_bytes
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
        if version == DocumentVersion::ZERO {
            return Err(DomainError::VersionConflict);
        }
        let mut workbook = Self {
            document_id,
            version,
            last_axis_change_version: version,
            sheets: vec![Sheet::from_seed(&seed)?],
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
                    .first_mut()
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
    pub fn candidate(
        &self,
        operation: &Operation,
        next_version: DocumentVersion,
    ) -> Result<Self, DomainError> {
        if self.version.get().checked_add(1) != Some(next_version.get()) {
            return Err(DomainError::VersionConflict);
        }
        let budgets = self.operation_budgets(operation)?;
        let mut next = self.clone();
        next.apply_operation(operation, budgets, next_version)?;
        next.version = next_version;
        if matches!(
            operation,
            Operation::CreateWorkbook { .. }
                | Operation::ExtendRows { .. }
                | Operation::InsertRows { .. }
                | Operation::InsertRowsRebased { .. }
        ) {
            next.last_axis_change_version = next_version;
        }
        if matches!(operation, Operation::InsertRowsRebased { .. }) {
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
                Operation::ExtendRows { .. } => return Err(DomainError::VersionConflict),
                Operation::InsertRows { .. } => return Err(DomainError::VersionConflict),
                Operation::InsertRowsRebased { .. } => return Err(DomainError::VersionConflict),
                Operation::AdvanceCalculationEpoch { .. } => {
                    return Err(DomainError::VersionConflict);
                }
                Operation::SetCellValue { address, .. }
                | Operation::SetCellFormula { address, .. }
                | Operation::ClearCell { address } => *address,
            };
            if !seen.insert(address) {
                return Err(DomainError::InvalidAxis);
            }
        }
        let mut next = self.clone();
        next.version = target_version;
        for operation in operations {
            let budgets = next.operation_budgets(operation)?;
            next.apply_operation(operation, budgets, target_version)?;
        }
        Ok(next)
    }
    fn operation_budgets(&self, operation: &Operation) -> Result<(usize, usize), DomainError> {
        // Check the incoming value and aggregate budget before cloning the existing
        // projection. Replacements subtract their former allocation in constant time.
        Ok(match operation {
            Operation::CreateWorkbook { .. } => (self.text_bytes, self.ast_memory_bytes),
            Operation::ExtendRows { sheet, rows } => {
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
            Operation::SetCellFormula { address, formula } => {
                self.validate_formula(*address, formula)?;
                self.updated_budgets(*address, formula.text_bytes(), formula.ast_memory_bytes())?
            }
            Operation::ClearCell { address } => self.updated_budgets(*address, 0, 0)?,
        })
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
        }
        self.text_bytes = text_bytes;
        self.ast_memory_bytes = ast_memory_bytes;
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
        hash.finalize().into()
    }
}
