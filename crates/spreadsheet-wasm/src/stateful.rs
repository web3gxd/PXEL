//! Mutable browser preview. Source binding is valid only for the immutable-axis profile.
use crate::styles::{ChangeInput, StyleInput};
use prost::Message;
use serde::{Deserialize, Serialize};
use spreadsheet_domain::{
    formula::Formula,
    workbook::{
        CalculationContext, CalculationContextId, CalculationReason, CellAddress, CellContent,
        CellValue, ColumnId, DocumentId, DocumentVersion, DomainError, Operation, RowId, SheetId,
        SheetSeed, Workbook,
    },
};
use spreadsheet_formula::{
    DateSystem, EvaluationError, Value,
    binder::bind,
    calculation::{
        COMPATIBILITY_PROFILE, CalculationError, CalculationLimits, CalculationProjection,
        ENGINE_VERSION,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use thiserror::Error;
use wasm_bindgen::prelude::*;

const MAX_INITIAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_PATCH_BYTES: usize = 1024 * 1024;
const MAX_PATCH_CELLS: usize = 1000;
const MAX_MOVE_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PreviewError {
    #[error("InvalidJson")]
    InvalidJson,
    #[error("LimitExceeded")]
    LimitExceeded,
    #[error("InvalidIdentity")]
    InvalidIdentity,
    #[error("StaleRevision")]
    StaleRevision,
    #[error("UnsupportedProfile")]
    UnsupportedProfile,
    #[error("UnsupportedMoveDependency")]
    UnsupportedMoveDependency,
    #[error("InvalidFormula")]
    InvalidFormula,
    #[error("InvalidProjection")]
    InvalidProjection,
    #[error("CalculationLimitExceeded")]
    CalculationLimitExceeded,
    #[error("Cancelled")]
    Cancelled,
}
impl PreviewError {
    fn js(self) -> JsValue {
        JsValue::from_str(&format!("{{\"code\":\"{self}\",\"retryable\":false}}"))
    }
}
impl From<DomainError> for PreviewError {
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::LimitExceeded => Self::LimitExceeded,
            DomainError::VersionConflict => Self::StaleRevision,
            DomainError::InvalidId => Self::InvalidIdentity,
            _ => Self::InvalidProjection,
        }
    }
}
impl From<CalculationError> for PreviewError {
    fn from(error: CalculationError) -> Self {
        match error {
            CalculationError::VersionMismatch => Self::StaleRevision,
            CalculationError::DependencyLimit
            | CalculationError::Evaluation(EvaluationError::CalculationLimitExceeded) => {
                Self::CalculationLimitExceeded
            }
            CalculationError::Evaluation(EvaluationError::Cancelled) => Self::Cancelled,
            CalculationError::Evaluation(EvaluationError::UnsupportedCompatibilityFeature) => {
                Self::UnsupportedProfile
            }
            _ => Self::InvalidProjection,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveAddress {
    sheet_id: String,
    row_id: String,
    column_id: String,
}
impl MoveAddress {
    fn decode(&self) -> Result<CellAddress, PreviewError> {
        address(&self.sheet_id, &self.row_id, &self.column_id)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveInput {
    schema_version: u32,
    document_id: String,
    input_version: String,
    expected_pending_revision: String,
    pending_revision: String,
    source: MoveAddress,
    destination: MoveAddress,
    rows: u32,
    columns: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Initialize {
    schema_version: u32,
    #[serde(default, deserialize_with = "present")]
    last_axis_change_version: Option<String>,
    #[serde(default, deserialize_with = "present")]
    retired_row_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    retired_column_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    row_activation_floors: Option<Vec<RowFloor>>,
    #[serde(default, deserialize_with = "present")]
    column_activation_floors: Option<Vec<ColumnFloor>>,
    #[serde(default)]
    calculation_context: Option<ContextInput>,
    document_id: String,
    input_version: String,
    pending_revision: String,
    sheets: Vec<SheetInput>,
}
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RowFloor {
    row_id: String,
    version: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnFloor {
    column_id: String,
    version: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StyledCellInput {
    row_id: String,
    column_id: String,
    style: StyleInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StyleMutation {
    sheet_id: String,
    row_id: String,
    column_id: String,
    changes: Vec<ChangeInput>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextInput {
    schema_version: u32,
    context_id: String,
    epoch: String,
    effective_timestamp_utc_ms: String,
    utc_offset_minutes: i16,
    civil_day_since_unix_epoch: i32,
    random_seed: String,
    engine_version: String,
    compatibility_profile: String,
    reason: String,
}
impl ContextInput {
    fn decode(self, version: DocumentVersion) -> Result<CalculationContext, PreviewError> {
        if self.schema_version != 1
            || self.engine_version != ENGINE_VERSION
            || self.compatibility_profile != COMPATIBILITY_PROFILE
        {
            return Err(PreviewError::UnsupportedProfile);
        }
        let epoch = revision(&self.epoch)?;
        if epoch == 0 || epoch > version.get() {
            return Err(PreviewError::InvalidProjection);
        }
        if self.effective_timestamp_utc_ms.is_empty()
            || self.effective_timestamp_utc_ms.len() > 19
            || !self
                .effective_timestamp_utc_ms
                .bytes()
                .all(|b| b.is_ascii_digit())
        {
            return Err(PreviewError::InvalidJson);
        }
        let timestamp = self
            .effective_timestamp_utc_ms
            .parse::<i64>()
            .map_err(|_| PreviewError::InvalidJson)?;
        if self.random_seed.len() != 64
            || !self
                .random_seed
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(PreviewError::InvalidJson);
        }
        let mut seed = [0u8; 32];
        for (index, part) in self
            .random_seed
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
        {
            let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            seed[index] = (digit(part[0]) << 4) | digit(part[1]);
        }
        let reason = match self.reason.as_str() {
            "open" => CalculationReason::Open,
            "explicit" => CalculationReason::Explicit,
            "timer" => CalculationReason::Timer,
            "volatile_dependency" => CalculationReason::VolatileDependency,
            "engine_upgrade" => CalculationReason::EngineUpgrade,
            _ => return Err(PreviewError::InvalidJson),
        };
        Ok(CalculationContext::new(
            CalculationContextId::from_bytes(identity(&self.context_id)?)?,
            epoch,
            timestamp,
            self.utc_offset_minutes,
            self.civil_day_since_unix_epoch,
            seed,
            self.engine_version,
            self.compatibility_profile,
            reason,
        )?)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SheetInput {
    #[serde(default, deserialize_with = "present")]
    hidden: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    default_column_width: Option<u32>,
    #[serde(default, deserialize_with = "present")]
    tab_color: Option<Option<u32>>,
    #[serde(default, deserialize_with = "present")]
    hidden_rows: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    hidden_columns: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    frozen_rows: Option<u32>,
    #[serde(default, deserialize_with = "present")]
    row_heights: Option<Vec<SizeInput>>,
    #[serde(default, deserialize_with = "present")]
    column_widths: Option<Vec<SizeInput>>,
    #[serde(default, deserialize_with = "present")]
    merges: Option<Vec<MergeInput>>,
    #[serde(default, deserialize_with = "present")]
    styles: Option<Vec<StyledCellInput>>,
    id: String,
    name: String,
    rows: Vec<String>,
    columns: Vec<String>,
    cells: Vec<CellInput>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeInput {
    axis_id: String,
    size_css_px: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeInput {
    first_row_id: String,
    last_row_id: String,
    first_column_id: String,
    last_column_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CellInput {
    row_id: String,
    column_id: String,
    value: InputValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mutation {
    sheet_id: String,
    row_id: String,
    column_id: String,
    value: InputValue,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    #[serde(default, deserialize_with = "present")]
    styles: Option<Vec<StyleMutation>>,
    schema_version: u32,
    document_id: String,
    expected_input_version: String,
    input_version: String,
    expected_pending_revision: String,
    pending_revision: String,
    cells: Vec<Mutation>,
}
#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "lowercase",
    deny_unknown_fields
)]
enum InputValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Formula(FormulaInput),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FormulaInput {
    #[serde(default, deserialize_with = "present")]
    bound_formula_wire_hex: Option<String>,
    source: String,
    #[serde(default)]
    canonical: Option<String>,
    #[serde(default)]
    ast_schema: Option<u32>,
    #[serde(default)]
    compatibility_profile: Option<String>,
    #[serde(default)]
    bound_at_version: Option<String>,
}
fn revision(value: &str) -> Result<u64, PreviewError> {
    if value.is_empty()
        || value.len() > 19
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(PreviewError::InvalidJson);
    }
    let n = value
        .parse::<u64>()
        .map_err(|_| PreviewError::InvalidJson)?;
    if n > i64::MAX as u64 {
        return Err(PreviewError::LimitExceeded);
    }
    Ok(n)
}
fn identity(value: &str) -> Result<[u8; 16], PreviewError> {
    if value.len() != 36 || !value.is_ascii() {
        return Err(PreviewError::InvalidIdentity);
    }
    let mut bytes = [0; 16];
    let mut index = 0;
    for (position, byte) in value.bytes().enumerate() {
        if [8, 13, 18, 23].contains(&position) {
            if byte != b'-' {
                return Err(PreviewError::InvalidIdentity);
            }
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return Err(PreviewError::InvalidIdentity),
        };
        let slot = bytes
            .get_mut(index / 2)
            .ok_or(PreviewError::InvalidIdentity)?;
        *slot = (*slot << 4) | digit;
        index += 1;
    }
    if bytes == [0; 16] {
        return Err(PreviewError::InvalidIdentity);
    }
    Ok(bytes)
}
fn id_string(bytes: &[u8; 16]) -> String {
    let h = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}
fn address(sheet: &str, row: &str, column: &str) -> Result<CellAddress, PreviewError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes(identity(sheet)?)?,
        row: RowId::from_bytes(identity(row)?)?,
        column: ColumnId::from_bytes(identity(column)?)?,
    })
}

fn cell_content(
    value: InputValue,
    at: CellAddress,
    book: &Workbook,
    target_version: DocumentVersion,
    bound_mode: u8,
) -> Result<CellContent, PreviewError> {
    Ok(match value {
        InputValue::Blank => CellContent::Value(CellValue::Blank),
        InputValue::Number(n) => CellContent::Value(CellValue::Number(n).normalized()?),
        InputValue::Text(text) => CellContent::Value(CellValue::Text(text).normalized()?),
        InputValue::Boolean(b) => CellContent::Value(CellValue::Boolean(b)),
        InputValue::Formula(input) => {
            if let Some(hex) = &input.bound_formula_wire_hex {
                if bound_mode == 0 {
                    return Err(PreviewError::UnsupportedProfile);
                }
                if hex.len() > 2 * 1024 * 1024 {
                    return Err(PreviewError::LimitExceeded);
                }
                if hex.is_empty()
                    || hex.len() % 2 != 0
                    || !hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(PreviewError::InvalidFormula);
                }
                let mut bytes = Vec::with_capacity(hex.len() / 2);
                for pair in hex.as_bytes().as_chunks::<2>().0 {
                    let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
                    bytes.push((digit(pair[0]) << 4) | digit(pair[1]));
                }
                let dto = spreadsheet_wire_protocol::v1::BoundFormula::decode(bytes.as_slice())
                    .map_err(|_| PreviewError::InvalidFormula)?;
                let formula = spreadsheet_wire_protocol::formula::decode_formula(dto)
                    .map_err(|_| PreviewError::InvalidFormula)?;
                if formula.origin() != at
                    || formula.source() != input.source
                    || input.canonical.as_deref() != Some(formula.canonical())
                    || input.ast_schema != Some(formula.ast().schema_version())
                    || input.compatibility_profile.as_deref()
                        != Some(formula.profile().identifier())
                    || input
                        .bound_at_version
                        .as_deref()
                        .map(revision)
                        .transpose()?
                        != Some(formula.bound_at_version().get())
                    || formula.bound_at_version() > target_version
                {
                    return Err(PreviewError::InvalidFormula);
                }
                return Ok(CellContent::Formula(formula));
            }
            if bound_mode == 2 {
                return Err(PreviewError::InvalidFormula);
            }

            if input.source.len() > 32 * 1024
                || input
                    .canonical
                    .as_ref()
                    .is_some_and(|s| s.len() > 128 * 1024)
            {
                return Err(PreviewError::LimitExceeded);
            }
            if input.ast_schema.is_some_and(|v| v != 1)
                || input
                    .compatibility_profile
                    .as_ref()
                    .is_some_and(|p| p != COMPATIBILITY_PROFILE)
            {
                return Err(PreviewError::UnsupportedProfile);
            }
            let formula =
                bind(&input.source, book, at).map_err(|_| PreviewError::InvalidFormula)?;
            if input
                .canonical
                .as_ref()
                .is_some_and(|s| s != formula.canonical())
            {
                return Err(PreviewError::UnsupportedProfile);
            }
            let base = match input.bound_at_version {
                Some(version) => DocumentVersion::new(revision(&version)?)?,
                None => book.version(),
            };
            if base == DocumentVersion::ZERO || base > target_version {
                return Err(PreviewError::InvalidFormula);
            }
            CellContent::Formula(
                Formula::new(
                    formula.source().to_owned(),
                    formula.canonical().to_owned(),
                    formula.ast().clone(),
                    at,
                    base,
                )
                .map_err(|_| PreviewError::InvalidFormula)?,
            )
        }
    })
}

fn retain_budget(content: &CellContent, retained: &mut (usize, usize)) -> Result<(), PreviewError> {
    retained.0 = retained
        .0
        .checked_add(content.text_bytes())
        .filter(|n| *n <= spreadsheet_domain::workbook::MAX_WORKBOOK_TEXT_BYTES)
        .ok_or(PreviewError::LimitExceeded)?;
    retained.1 = retained
        .1
        .checked_add(content.ast_memory_bytes())
        .filter(|n| *n <= spreadsheet_domain::workbook::MAX_WORKBOOK_AST_BYTES)
        .ok_or(PreviewError::LimitExceeded)?;
    Ok(())
}

#[derive(Serialize)]
struct AddressOutput {
    sheet_id: String,
    row_id: String,
    column_id: String,
}
impl From<CellAddress> for AddressOutput {
    fn from(at: CellAddress) -> Self {
        Self {
            sheet_id: id_string(at.sheet.as_bytes()),
            row_id: id_string(at.row.as_bytes()),
            column_id: id_string(at.column.as_bytes()),
        }
    }
}
#[derive(Serialize)]
struct CellOutput {
    #[serde(flatten)]
    address: AddressOutput,
    value: OutputValue,
}
#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
enum OutputValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(&'static str),
    DateTime {
        serial: f64,
        system: &'static str,
    },
    Array {
        rows: u32,
        columns: u32,
        values: Vec<OutputValue>,
    },
}
impl From<&Value> for OutputValue {
    fn from(value: &Value) -> Self {
        match value {
            Value::Blank => Self::Blank,
            Value::Number(n) => Self::Number(*n),
            Value::Text(text) => Self::Text(text.clone()),
            Value::Boolean(b) => Self::Boolean(*b),
            Value::Error(e) => Self::Error(e.code()),
            Value::DateTime { serial, system } => Self::DateTime {
                serial: *serial,
                system: match system {
                    DateSystem::Excel1900 => "1900",
                    DateSystem::Excel1904 => "1904",
                },
            },
            Value::Array {
                rows,
                columns,
                values,
            } => Self::Array {
                rows: *rows,
                columns: *columns,
                values: values.iter().map(Self::from).collect(),
            },
        }
    }
}
#[derive(Serialize)]
struct CalculationOutput {
    schema_version: u32,
    preview: bool,
    document_id: String,
    input_version: String,
    pending_revision: String,
    formula_engine_version: &'static str,
    compatibility_profile: &'static str,
    complete: bool,
    full: bool,
    results: Vec<CellOutput>,
    cleared: Vec<AddressOutput>,
    normalized_result_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    normalized_authored_state_hash: Option<String>,
    evaluated_cells: usize,
    dirty_cells: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    move_writes: Option<Vec<MoveWriteOutput>>,
}

#[derive(Serialize)]
struct MoveWriteOutput {
    #[serde(flatten)]
    address: AddressOutput,
    value: AuthoredOutputValue,
}
#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
enum AuthoredOutputValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Formula(FormulaTextOutput),
}
#[derive(Serialize)]
struct FormulaTextOutput {
    source: String,
}
struct CappedOutput(Vec<u8>);
impl std::io::Write for CappedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_MOVE_OUTPUT_BYTES {
            return Err(std::io::Error::other("preview output limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Native callable API used unchanged by the browser wrapper and differential tests.
#[derive(Clone)]
pub struct PreviewWorkbook {
    output_schema: u32,
    workbook: Workbook,
    calculation: CalculationProjection,
    pending_revision: u64,
    first_calculation: bool,
}
impl PreviewWorkbook {
    pub fn patch_json(&mut self, json: &str) -> Result<String, PreviewError> {
        self.patch_with_cancellation(json, &AtomicBool::new(false))
    }
    pub fn patch_with_cancellation(
        &mut self,
        json: &str,
        cancelled: &AtomicBool,
    ) -> Result<String, PreviewError> {
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(PreviewError::Cancelled);
        }
        if json.len() > MAX_PATCH_BYTES {
            return Err(PreviewError::LimitExceeded);
        }
        let patch: Patch = serde_json::from_str(json).map_err(|_| PreviewError::InvalidJson)?;
        if patch.schema_version != 2 || self.output_schema != 2 || patch.styles.is_none() {
            return Err(PreviewError::UnsupportedProfile);
        }
        if DocumentId::from_bytes(identity(&patch.document_id)?)? != self.workbook.document_id() {
            return Err(PreviewError::InvalidIdentity);
        }
        let version = DocumentVersion::new(revision(&patch.input_version)?)?;
        let pending = revision(&patch.pending_revision)?;
        if revision(&patch.expected_input_version)? != self.workbook.version().get()
            || revision(&patch.expected_pending_revision)? != self.pending_revision
            || version < self.workbook.version()
            || pending <= self.pending_revision
        {
            return Err(PreviewError::StaleRevision);
        }
        let styles = patch.styles.ok_or(PreviewError::InvalidProjection)?;
        if patch.cells.len().saturating_add(styles.len()) > MAX_PATCH_CELLS {
            return Err(PreviewError::LimitExceeded);
        }
        let mut changed = BTreeSet::new();
        let mut operations = Vec::new();
        let mut retained = (0usize, 0usize);
        for cell in patch.cells {
            let at = address(&cell.sheet_id, &cell.row_id, &cell.column_id)?;
            if !changed.insert(at) {
                return Err(PreviewError::InvalidProjection);
            }
            let content = cell_content(cell.value, at, &self.workbook, version, 1)?;
            retain_budget(&content, &mut retained)?;
            operations.push(match content {
                CellContent::Value(CellValue::Blank) => Operation::ClearCell { address: at },
                CellContent::Value(value) => Operation::SetCellValue { address: at, value },
                CellContent::Formula(formula) => Operation::SetCellFormula {
                    address: at,
                    formula,
                },
            });
        }
        let mut style_changes = Vec::new();
        for style in styles {
            if style.changes.is_empty() || style.changes.len() > 16 {
                return Err(PreviewError::InvalidProjection);
            }
            style_changes.push((
                address(&style.sheet_id, &style.row_id, &style.column_id)?,
                style
                    .changes
                    .into_iter()
                    .map(ChangeInput::decode)
                    .collect::<Result<Vec<_>, _>>()?,
            ));
        }
        let candidate =
            self.workbook
                .preview_authored_candidate(&operations, &style_changes, version)?;
        let delta = self.calculation.prepare_reconcile(
            &candidate,
            &changed,
            CalculationLimits::default(),
        )?;
        let mut temporary = Self {
            output_schema: 2,
            workbook: candidate,
            calculation: self.calculation.clone(),
            pending_revision: pending,
            first_calculation: self.first_calculation,
        };
        temporary.calculation.apply(delta);
        let output = temporary.calculate_output(cancelled)?;
        let mut bytes = CappedOutput(Vec::new());
        serde_json::to_writer(&mut bytes, &output).map_err(|_| PreviewError::LimitExceeded)?;
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(PreviewError::Cancelled);
        }
        let result = String::from_utf8(bytes.0).map_err(|_| PreviewError::InvalidProjection)?;
        temporary.first_calculation = false;
        *self = temporary;
        Ok(result)
    }
    /// Runs the shared move planner and incremental evaluator on a private
    /// candidate. Any validation, calculation or output-limit failure rolls back.
    pub fn move_json(&mut self, json: &str) -> Result<String, PreviewError> {
        use spreadsheet_formula::relocation::{MoveError, MoveRequest, plan_move};
        if json.len() > MAX_PATCH_BYTES {
            return Err(PreviewError::LimitExceeded);
        }
        let input: MoveInput = serde_json::from_str(json).map_err(|_| PreviewError::InvalidJson)?;
        if input.schema_version != 1 {
            return Err(PreviewError::UnsupportedProfile);
        }
        if DocumentId::from_bytes(identity(&input.document_id)?)? != self.workbook.document_id() {
            return Err(PreviewError::InvalidIdentity);
        }
        let pending = revision(&input.pending_revision)?;
        if revision(&input.input_version)? != self.workbook.version().get()
            || revision(&input.expected_pending_revision)? != self.pending_revision
            || pending <= self.pending_revision
        {
            return Err(PreviewError::StaleRevision);
        }
        let source = input.source.decode()?;
        let destination = input.destination.decode()?;
        let plan = plan_move(
            &self.workbook,
            MoveRequest {
                source,
                destination,
                rows: input.rows,
                columns: input.columns,
            },
        )
        .map_err(|error| match error {
            MoveError::PartialRange => PreviewError::UnsupportedMoveDependency,
            MoveError::LimitExceeded => PreviewError::LimitExceeded,
            MoveError::Cancelled => PreviewError::Cancelled,
            _ => PreviewError::InvalidProjection,
        })?;
        let changed = plan.writes.iter().map(|write| write.address).collect();
        let operation = Operation::MoveRange {
            source,
            destination,
            rows: input.rows,
            columns: input.columns,
            writes: plan.writes,
        };
        let candidate = self.workbook.preview_move_candidate(&operation)?;
        let delta = self.calculation.prepare_reconcile(
            &candidate,
            &changed,
            CalculationLimits::default(),
        )?;
        let mut temporary = Self {
            output_schema: self.output_schema,
            workbook: candidate,
            calculation: self.calculation.clone(),
            pending_revision: pending,
            first_calculation: self.first_calculation,
        };
        temporary.calculation.apply(delta);
        let Operation::MoveRange { writes, .. } = &operation else {
            return Err(PreviewError::InvalidProjection);
        };
        let mut move_writes = Vec::with_capacity(writes.len());
        let mut output_bytes = 0usize;
        for write in writes {
            let value = match &write.content {
                None | Some(CellContent::Value(CellValue::Blank)) => AuthoredOutputValue::Blank,
                Some(CellContent::Value(CellValue::Number(number))) => {
                    AuthoredOutputValue::Number(*number)
                }
                Some(CellContent::Value(CellValue::Boolean(value))) => {
                    AuthoredOutputValue::Boolean(*value)
                }
                Some(CellContent::Value(CellValue::Text(text))) => {
                    output_bytes = output_bytes.saturating_add(text.len());
                    AuthoredOutputValue::Text(text.clone())
                }
                Some(CellContent::Formula(formula)) => {
                    let source =
                        spreadsheet_formula::bound::render_current(formula, &temporary.workbook)
                            .map_err(|_| PreviewError::InvalidFormula)?;
                    output_bytes = output_bytes.saturating_add(source.len());
                    AuthoredOutputValue::Formula(FormulaTextOutput { source })
                }
            };
            output_bytes = output_bytes.saturating_add(128);
            if output_bytes > MAX_MOVE_OUTPUT_BYTES {
                return Err(PreviewError::LimitExceeded);
            }
            move_writes.push(MoveWriteOutput {
                address: write.address.into(),
                value,
            });
        }
        let mut output = temporary.calculate_output(&AtomicBool::new(false))?;
        output.move_writes = Some(move_writes);
        let mut bytes = CappedOutput(Vec::new());
        serde_json::to_writer(&mut bytes, &output).map_err(|_| PreviewError::LimitExceeded)?;
        let json = String::from_utf8(bytes.0).map_err(|_| PreviewError::InvalidProjection)?;
        temporary.first_calculation = false;
        *self = temporary;
        Ok(json)
    }
    pub fn from_json(json: &str) -> Result<Self, PreviewError> {
        if json.len() > MAX_INITIAL_BYTES {
            return Err(PreviewError::LimitExceeded);
        }
        let init: Initialize = serde_json::from_str(json).map_err(|_| PreviewError::InvalidJson)?;
        if !matches!(init.schema_version, 1..=6)
            || init.sheets.is_empty()
            || init.sheets.len() > 64
            || (init.schema_version == 1 && init.sheets.len() != 1)
            || (init.schema_version < 4
                && ((init.schema_version == 3) != init.calculation_context.is_some()))
        {
            return Err(PreviewError::UnsupportedProfile);
        }
        let exact = init.schema_version >= 4;
        let layout = init.schema_version >= 5;
        let settings_present = init.schema_version == 6;
        let metadata_present = init.last_axis_change_version.is_some()
            && init.retired_row_ids.is_some()
            && init.retired_column_ids.is_some()
            && init.row_activation_floors.is_some()
            && init.column_activation_floors.is_some();
        let metadata_any = init.last_axis_change_version.is_some()
            || init.retired_row_ids.is_some()
            || init.retired_column_ids.is_some()
            || init.row_activation_floors.is_some()
            || init.column_activation_floors.is_some();
        if (exact && !metadata_present)
            || (!exact && metadata_any)
            || init.sheets.iter().any(|s| s.styles.is_some() != exact)
            || init.sheets.iter().any(|s| {
                (s.hidden.is_some() != settings_present)
                    || (s.default_column_width.is_some() != settings_present)
                    || (s.tab_color.is_some() != settings_present)
                    || (s.hidden_rows.is_some() != settings_present)
                    || (s.hidden_columns.is_some() != settings_present)
                    || (s.frozen_rows.is_some() != layout)
                    || (s.row_heights.is_some() != layout)
                    || (s.column_widths.is_some() != layout)
                    || (s.merges.is_some() != layout)
            })
        {
            return Err(PreviewError::UnsupportedProfile);
        }
        let pending_revision = revision(&init.pending_revision)?;
        if pending_revision != 0 {
            return Err(PreviewError::StaleRevision);
        }
        let document_id = DocumentId::from_bytes(identity(&init.document_id)?)?;
        let version = DocumentVersion::new(revision(&init.input_version)?)?;
        let context = init
            .calculation_context
            .map(|context| context.decode(version))
            .transpose()?;
        let mut seeds = Vec::with_capacity(init.sheets.len());
        let mut sheet_cells = Vec::with_capacity(init.sheets.len());
        let mut total_cells = 0usize;
        let mut all_styles = Vec::new();
        let mut layouts = Vec::new();
        let mut settings_entries = Vec::new();
        for sheet in init.sheets {
            if settings_present {
                let rows = sheet
                    .hidden_rows
                    .as_ref()
                    .ok_or(PreviewError::InvalidProjection)?;
                let columns = sheet
                    .hidden_columns
                    .as_ref()
                    .ok_or(PreviewError::InvalidProjection)?;
                let settings = spreadsheet_domain::workbook::SheetSettings {
                    name: sheet.name.clone(),
                    hidden: sheet.hidden.ok_or(PreviewError::InvalidProjection)?,
                    default_column_width: sheet
                        .default_column_width
                        .ok_or(PreviewError::InvalidProjection)?,
                    tab_color: sheet.tab_color.ok_or(PreviewError::InvalidProjection)?,
                    hidden_rows: rows
                        .iter()
                        .map(|v| Ok(RowId::from_bytes(identity(v)?)?))
                        .collect::<Result<BTreeSet<_>, PreviewError>>()?,
                    hidden_columns: columns
                        .iter()
                        .map(|v| Ok(ColumnId::from_bytes(identity(v)?)?))
                        .collect::<Result<BTreeSet<_>, PreviewError>>()?,
                };
                if settings.hidden_rows.len() != rows.len()
                    || settings.hidden_columns.len() != columns.len()
                {
                    return Err(PreviewError::InvalidProjection);
                }
                settings_entries.push((SheetId::from_bytes(identity(&sheet.id)?)?, settings));
            }
            if layout {
                let sheet_id = SheetId::from_bytes(identity(&sheet.id)?)?;
                let rows = sheet
                    .row_heights
                    .ok_or(PreviewError::InvalidProjection)?
                    .into_iter()
                    .map(|s| Ok((RowId::from_bytes(identity(&s.axis_id)?)?, s.size_css_px)))
                    .collect::<Result<Vec<_>, PreviewError>>()?;
                let columns = sheet
                    .column_widths
                    .ok_or(PreviewError::InvalidProjection)?
                    .into_iter()
                    .map(|s| Ok((ColumnId::from_bytes(identity(&s.axis_id)?)?, s.size_css_px)))
                    .collect::<Result<Vec<_>, PreviewError>>()?;
                let merges = sheet
                    .merges
                    .ok_or(PreviewError::InvalidProjection)?
                    .into_iter()
                    .map(|m| {
                        Ok(spreadsheet_domain::MergeRange::new(
                            RowId::from_bytes(identity(&m.first_row_id)?)?,
                            RowId::from_bytes(identity(&m.last_row_id)?)?,
                            ColumnId::from_bytes(identity(&m.first_column_id)?)?,
                            ColumnId::from_bytes(identity(&m.last_column_id)?)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, PreviewError>>()?;
                layouts.push((
                    sheet_id,
                    sheet.frozen_rows.ok_or(PreviewError::InvalidProjection)?,
                    rows,
                    columns,
                    merges,
                ));
            }
            if let Some(styles) = sheet.styles {
                if all_styles.len() + styles.len() > 100_000 {
                    return Err(PreviewError::LimitExceeded);
                }
                for style in styles {
                    all_styles.push((
                        address(&sheet.id, &style.row_id, &style.column_id)?,
                        style.style.decode()?,
                    ));
                }
            }
            total_cells = total_cells
                .checked_add(sheet.cells.len())
                .ok_or(PreviewError::LimitExceeded)?;
            if sheet.rows.len() > 100_000 || sheet.columns.len() > 1000 || total_cells > 100_000 {
                return Err(PreviewError::LimitExceeded);
            }
            seeds.push(SheetSeed {
                id: SheetId::from_bytes(identity(&sheet.id)?)?,
                name: sheet.name,
                rows: sheet
                    .rows
                    .iter()
                    .map(|id| Ok(RowId::from_bytes(identity(id)?)?))
                    .collect::<Result<_, PreviewError>>()?,
                columns: sheet
                    .columns
                    .iter()
                    .map(|id| Ok(ColumnId::from_bytes(identity(id)?)?))
                    .collect::<Result<_, PreviewError>>()?,
            });
            sheet_cells.push((sheet.id, sheet.cells));
        }
        let axes =
            Workbook::hydrate_sheets(document_id, version, seeds.clone(), std::iter::empty())?;
        let mut addresses = BTreeSet::new();
        let mut cells = Vec::with_capacity(total_cells);
        let mut retained = (0usize, 0usize);
        for (sheet_id, input_cells) in sheet_cells {
            for cell in input_cells {
                let at = address(&sheet_id, &cell.row_id, &cell.column_id)?;
                if !addresses.insert(at) {
                    return Err(PreviewError::InvalidProjection);
                }
                let content =
                    cell_content(cell.value, at, &axes, version, if exact { 2 } else { 0 })?;
                retain_budget(&content, &mut retained)?;
                cells.push((at, content));
            }
        }
        let mut workbook = match context {
            Some(context) => {
                Workbook::hydrate_sheets_with_context(document_id, version, seeds, cells, context)?
            }
            None => Workbook::hydrate_sheets(document_id, version, seeds, cells)?,
        };
        if exact {
            let mut image = workbook.snapshot_image()?;
            for (id, frozen_rows, rows, columns, merges) in layouts {
                let sheet = image
                    .sheets
                    .iter_mut()
                    .find(|s| s.sheet.id == id)
                    .ok_or(PreviewError::InvalidProjection)?;
                sheet.frozen_rows = frozen_rows;
                sheet.row_heights = rows;
                sheet.column_widths = columns;
                sheet.merges = merges;
            }
            for (id, settings) in settings_entries {
                image
                    .sheets
                    .iter_mut()
                    .find(|s| s.sheet.id == id)
                    .ok_or(PreviewError::InvalidProjection)?
                    .settings = settings;
            }
            image.last_axis_change_version = DocumentVersion::new(revision(
                init.last_axis_change_version
                    .as_deref()
                    .ok_or(PreviewError::InvalidProjection)?,
            )?)?;
            image.retired_rows = init
                .retired_row_ids
                .ok_or(PreviewError::InvalidProjection)?
                .iter()
                .map(|s| Ok(RowId::from_bytes(identity(s)?)?))
                .collect::<Result<_, PreviewError>>()?;
            image.retired_columns = init
                .retired_column_ids
                .ok_or(PreviewError::InvalidProjection)?
                .iter()
                .map(|s| Ok(ColumnId::from_bytes(identity(s)?)?))
                .collect::<Result<_, PreviewError>>()?;
            image.row_activation_floors = init
                .row_activation_floors
                .ok_or(PreviewError::InvalidProjection)?
                .iter()
                .map(|f| {
                    Ok((
                        RowId::from_bytes(identity(&f.row_id)?)?,
                        DocumentVersion::new(revision(&f.version)?)?,
                    ))
                })
                .collect::<Result<_, PreviewError>>()?;
            image.column_activation_floors = init
                .column_activation_floors
                .ok_or(PreviewError::InvalidProjection)?
                .iter()
                .map(|f| {
                    Ok((
                        ColumnId::from_bytes(identity(&f.column_id)?)?,
                        DocumentVersion::new(revision(&f.version)?)?,
                    ))
                })
                .collect::<Result<_, PreviewError>>()?;
            for (at, style) in all_styles {
                image
                    .sheets
                    .iter_mut()
                    .find(|s| s.sheet.id == at.sheet)
                    .ok_or(PreviewError::InvalidProjection)?
                    .styles
                    .push((at, style));
            }
            workbook = Workbook::hydrate_snapshot(image)?;
        }
        let calculation =
            CalculationProjection::from_workbook(&workbook, CalculationLimits::default())?;
        Ok(Self {
            output_schema: if exact { 2 } else { 1 },
            workbook,
            calculation,
            pending_revision,
            first_calculation: true,
        })
    }
    pub fn apply_json(&mut self, json: &str) -> Result<(), PreviewError> {
        if json.len() > MAX_PATCH_BYTES {
            return Err(PreviewError::LimitExceeded);
        }
        let patch: Patch = serde_json::from_str(json).map_err(|_| PreviewError::InvalidJson)?;
        if patch.schema_version != 1 || patch.styles.is_some() || self.output_schema != 1 {
            return Err(PreviewError::UnsupportedProfile);
        }
        if DocumentId::from_bytes(identity(&patch.document_id)?)? != self.workbook.document_id() {
            return Err(PreviewError::InvalidIdentity);
        }
        let version = DocumentVersion::new(revision(&patch.input_version)?)?;
        let pending = revision(&patch.pending_revision)?;
        if revision(&patch.expected_input_version)? != self.workbook.version().get()
            || revision(&patch.expected_pending_revision)? != self.pending_revision
            || version < self.workbook.version()
            || pending <= self.pending_revision
        {
            return Err(PreviewError::StaleRevision);
        }
        if patch.cells.len() > MAX_PATCH_CELLS {
            return Err(PreviewError::LimitExceeded);
        }
        let mut changed = BTreeSet::new();
        let mut operations = Vec::with_capacity(patch.cells.len());
        let mut retained = (0usize, 0usize);
        for cell in patch.cells {
            let at = address(&cell.sheet_id, &cell.row_id, &cell.column_id)?;
            if !changed.insert(at) {
                return Err(PreviewError::InvalidProjection);
            }
            let content = cell_content(cell.value, at, &self.workbook, version, 0)?;
            retain_budget(&content, &mut retained)?;
            operations.push(match content {
                CellContent::Value(CellValue::Blank) => Operation::ClearCell { address: at },
                CellContent::Value(value) => Operation::SetCellValue { address: at, value },
                CellContent::Formula(formula) => Operation::SetCellFormula {
                    address: at,
                    formula,
                },
            });
        }
        let candidate = self.workbook.preview_candidate(&operations, version)?;
        let delta = self.calculation.prepare_reconcile(
            &candidate,
            &changed,
            CalculationLimits::default(),
        )?;
        self.calculation.apply(delta);
        self.workbook = candidate;
        self.pending_revision = pending;
        Ok(())
    }
    pub fn calculate_json(&mut self) -> Result<String, PreviewError> {
        self.calculate_with_cancellation(&AtomicBool::new(false))
    }
    pub fn calculate_with_cancellation(
        &mut self,
        cancelled: &AtomicBool,
    ) -> Result<String, PreviewError> {
        if self.output_schema == 2 {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(PreviewError::Cancelled);
            }
            let mut temporary = self.clone();
            let output = temporary.calculate_output(cancelled)?;
            let mut bytes = CappedOutput(Vec::new());
            serde_json::to_writer(&mut bytes, &output).map_err(|_| PreviewError::LimitExceeded)?;
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(PreviewError::Cancelled);
            }
            let result = String::from_utf8(bytes.0).map_err(|_| PreviewError::InvalidProjection)?;
            temporary.first_calculation = false;
            *self = temporary;
            return Ok(result);
        }
        let output = self.calculate_output(cancelled)?;
        let json = serde_json::to_string(&output).map_err(|_| PreviewError::InvalidProjection)?;
        self.first_calculation = false;
        Ok(json)
    }
    fn calculate_output(
        &mut self,
        cancelled: &AtomicBool,
    ) -> Result<CalculationOutput, PreviewError> {
        let patch =
            self.calculation
                .calculate(&self.workbook, CalculationLimits::default(), cancelled)?;
        let hash = self.calculation.result_hash()?;
        let values: &BTreeMap<CellAddress, Value> = if self.first_calculation {
            self.calculation.results()
        } else {
            &patch.changed_results
        };
        let output = CalculationOutput {
            schema_version: self.output_schema,
            normalized_authored_state_hash: (self.output_schema == 2).then(|| {
                self.workbook
                    .hash()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            }),
            preview: true,
            document_id: id_string(self.workbook.document_id().as_bytes()),
            input_version: self.workbook.version().get().to_string(),
            pending_revision: self.pending_revision.to_string(),
            formula_engine_version: ENGINE_VERSION,
            compatibility_profile: COMPATIBILITY_PROFILE,
            complete: true,
            full: self.first_calculation,
            results: values
                .iter()
                .map(|(at, value)| CellOutput {
                    address: (*at).into(),
                    value: OutputValue::from(value),
                })
                .collect(),
            cleared: patch
                .cleared_results
                .into_iter()
                .map(AddressOutput::from)
                .collect(),
            normalized_result_hash: hash.iter().map(|b| format!("{b:02x}")).collect(),
            evaluated_cells: patch.evaluated_cells,
            dirty_cells: patch.dirty_cells,
            move_writes: None,
        };
        Ok(output)
    }
    pub fn workbook(&self) -> &Workbook {
        &self.workbook
    }
}

#[wasm_bindgen]
pub struct WasmWorkbook {
    inner: PreviewWorkbook,
}
#[wasm_bindgen]
impl WasmWorkbook {
    #[wasm_bindgen(constructor)]
    pub fn new(json: &str) -> Result<WasmWorkbook, JsValue> {
        PreviewWorkbook::from_json(json)
            .map(|inner| Self { inner })
            .map_err(PreviewError::js)
    }
    pub fn apply(&mut self, json: &str) -> Result<(), JsValue> {
        self.inner.apply_json(json).map_err(PreviewError::js)
    }
    pub fn patch(&mut self, json: &str) -> Result<String, JsValue> {
        self.inner.patch_json(json).map_err(PreviewError::js)
    }
    pub fn calculate(&mut self) -> Result<String, JsValue> {
        self.inner.calculate_json().map_err(PreviewError::js)
    }
    pub fn move_preview(&mut self, json: &str) -> Result<String, JsValue> {
        self.inner.move_json(json).map_err(PreviewError::js)
    }
}
