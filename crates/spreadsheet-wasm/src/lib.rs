//! Thin fixture and stateful preview boundaries over the shared pure Rust engine.
mod stateful;
use serde::Deserialize;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use spreadsheet_formula::{
    EvaluationBudget, EvaluationError, ReferenceResolver,
    ast::{CellReference, DateSystem, FormulaError, RangeReference, Value},
    evaluate, parse,
};
pub use stateful::{PreviewError, PreviewWorkbook, WasmWorkbook};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use wasm_bindgen::prelude::*;

const MAX_CELLS_JSON_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 32 * 1024;
const MAX_CELLS: usize = 100_000;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("fixture input exceeds limits")]
    LimitExceeded,
    #[error("invalid fixture JSON schema")]
    InvalidJson,
    #[error("duplicate or invalid fixture coordinate")]
    InvalidCoordinate,
    #[error("formula parse error at bytes {start}..{end}")]
    Parse { start: usize, end: usize },
    #[error("{0}")]
    Evaluation(EvaluationError),
    #[error("result serialization failed")]
    Serialization,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureCell {
    #[serde(default)]
    sheet: Option<String>,
    row: u32,
    column: u32,
    value: FixtureValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureInput {
    schema_version: u32,
    current_sheet: String,
    sheets: Vec<String>,
    cells: Vec<FixtureCell>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FixturePayload {
    Workbook(FixtureInput),
    Cells(Vec<FixtureCell>),
}

#[derive(Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "lowercase",
    deny_unknown_fields
)]
enum FixtureValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(String),
}

impl TryFrom<FixtureValue> for Value {
    type Error = AdapterError;
    fn try_from(value: FixtureValue) -> Result<Self, AdapterError> {
        Ok(match value {
            FixtureValue::Blank => Self::Blank,
            FixtureValue::Number(n) => Self::Number(if n == 0.0 { 0.0 } else { n }),
            FixtureValue::Text(s) => Self::Text(s),
            FixtureValue::Boolean(b) => Self::Boolean(b),
            FixtureValue::Error(code) => Self::Error(match code.as_str() {
                "#NULL!" => FormulaError::Null,
                "#DIV/0!" => FormulaError::Div0,
                "#VALUE!" => FormulaError::Value,
                "#REF!" => FormulaError::Ref,
                "#NAME?" => FormulaError::Name,
                "#NUM!" => FormulaError::Num,
                "#N/A" => FormulaError::Na,
                "#SPILL!" => FormulaError::Spill,
                "#CYCLE!" => FormulaError::Cycle,
                _ => return Err(AdapterError::InvalidJson),
            }),
        })
    }
}

struct FixtureResolver {
    cells: BTreeMap<(String, u32, u32), Value>,
    sheets: BTreeSet<String>,
    current_sheet: String,
}

impl ReferenceResolver for FixtureResolver {
    fn resolve_cell(&self, reference: &CellReference) -> Value {
        let sheet = reference.sheet.as_deref().unwrap_or(&self.current_sheet);
        if !self.sheets.contains(sheet) {
            return Value::Error(FormulaError::Ref);
        }
        self.cells
            .get(&(sheet.to_owned(), reference.row, reference.column))
            .cloned()
            .unwrap_or(Value::Blank)
    }

    fn visit_range(
        &self,
        range: &RangeReference,
        visitor: &mut dyn FnMut(Value) -> Result<(), EvaluationError>,
    ) -> Result<(), EvaluationError> {
        for row in range.start.row.min(range.end.row)..=range.start.row.max(range.end.row) {
            for column in
                range.start.column.min(range.end.column)..=range.start.column.max(range.end.column)
            {
                visitor(self.resolve_cell(&CellReference {
                    sheet: range.start.sheet.clone(),
                    row,
                    column,
                    row_absolute: false,
                    column_absolute: false,
                }))?;
            }
        }
        Ok(())
    }
}

fn length(hash: &mut Sha256, value: usize) -> Result<(), AdapterError> {
    hash.update(
        u64::try_from(value)
            .map_err(|_| AdapterError::LimitExceeded)?
            .to_le_bytes(),
    );
    Ok(())
}

/// Explicit versioned value encoding, never a hash of JSON or protobuf bytes.
fn normalized(value: &Value, hash: &mut Sha256, depth: usize) -> Result<Json, AdapterError> {
    if depth > 128 {
        return Err(AdapterError::LimitExceeded);
    }
    Ok(match value {
        Value::Blank => {
            hash.update([0]);
            json!({"type":"blank"})
        }
        Value::Number(n) if !n.is_finite() => {
            return normalized(&Value::Error(FormulaError::Num), hash, depth);
        }
        Value::Number(n) => {
            let n = if *n == 0.0 { 0.0 } else { *n };
            hash.update([1]);
            hash.update(n.to_bits().to_le_bytes());
            json!({"type":"number","value":n})
        }
        Value::Text(text) => {
            hash.update([2]);
            length(hash, text.len())?;
            hash.update(text.as_bytes());
            json!({"type":"text","value":text})
        }
        Value::Boolean(value) => {
            hash.update([3, u8::from(*value)]);
            json!({"type":"boolean","value":value})
        }
        Value::Error(error) => {
            hash.update([4]);
            length(hash, error.code().len())?;
            hash.update(error.code().as_bytes());
            json!({"type":"error","value":error.code()})
        }
        Value::DateTime { serial, system } => {
            if !serial.is_finite() {
                return normalized(&Value::Error(FormulaError::Num), hash, depth);
            }
            let serial = if *serial == 0.0 { 0.0 } else { *serial };
            let system = match system {
                DateSystem::Excel1900 => 0u8,
                DateSystem::Excel1904 => 1u8,
            };
            hash.update([5, system]);
            hash.update(serial.to_bits().to_le_bytes());
            json!({"type":"datetime","value":{"serial":serial,"date_system":if system==0 {1900} else {1904}}})
        }
        Value::Array {
            rows,
            columns,
            values,
        } => {
            if values.len() > MAX_CELLS
                || u64::from(*rows) * u64::from(*columns) != values.len() as u64
            {
                return Err(AdapterError::LimitExceeded);
            }
            hash.update([6]);
            hash.update(rows.to_le_bytes());
            hash.update(columns.to_le_bytes());
            let values = values
                .iter()
                .map(|v| normalized(v, hash, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            json!({"type":"array","value":{"rows":rows,"columns":columns,"values":values}})
        }
    })
}

/// Shared native/WASM fixture evaluation, not the incremental workbook transport.
pub fn evaluate_fixture(source: &str, cells_json: &str) -> Result<String, AdapterError> {
    if source.len() > MAX_SOURCE_BYTES || cells_json.len() > MAX_CELLS_JSON_BYTES {
        return Err(AdapterError::LimitExceeded);
    }
    let payload: FixturePayload =
        serde_json::from_str(cells_json).map_err(|_| AdapterError::InvalidJson)?;
    let (input, current_sheet, mut sheets, explicit_catalog) = match payload {
        FixturePayload::Cells(cells) => (
            cells,
            "Sheet1".to_owned(),
            BTreeSet::from(["Sheet1".to_owned()]),
            false,
        ),
        FixturePayload::Workbook(input) => {
            if input.schema_version != 1
                || input.sheets.len() > 1024
                || input.sheets.iter().any(|s| s.is_empty() || s.len() > 1024)
            {
                return Err(AdapterError::InvalidJson);
            }
            let sheets: BTreeSet<_> = input.sheets.iter().cloned().collect();
            if sheets.len() != input.sheets.len() || !sheets.contains(&input.current_sheet) {
                return Err(AdapterError::InvalidJson);
            }
            (input.cells, input.current_sheet, sheets, true)
        }
    };
    if input.len() > MAX_CELLS {
        return Err(AdapterError::LimitExceeded);
    }
    let mut cells = BTreeMap::new();
    for cell in input {
        if cell.row >= 1_048_576
            || cell.column >= 16_384
            || cell
                .sheet
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 1024)
        {
            return Err(AdapterError::InvalidCoordinate);
        }
        let sheet = cell.sheet.unwrap_or_else(|| current_sheet.clone());
        if explicit_catalog && !sheets.contains(&sheet) {
            return Err(AdapterError::InvalidCoordinate);
        }
        sheets.insert(sheet.clone());
        if cells
            .insert((sheet, cell.row, cell.column), Value::try_from(cell.value)?)
            .is_some()
        {
            return Err(AdapterError::InvalidCoordinate);
        }
    }
    let parsed = parse(source).map_err(|e| AdapterError::Parse {
        start: e.span.start,
        end: e.span.end,
    })?;
    let mut hash = Sha256::new();
    hash.update(b"PXEL-FIXTURE-RESULT\0\x01");
    let result = match evaluate(
        &parsed.ast,
        &FixtureResolver {
            cells,
            sheets,
            current_sheet,
        },
        &mut EvaluationBudget::new(1_000_000),
    ) {
        Ok(value) => normalized(&value, &mut hash, 0)?,
        Err(error) => {
            let code = match error {
                EvaluationError::CalculationLimitExceeded => "CalculationLimitExceeded",
                EvaluationError::Cancelled => "Cancelled",
                EvaluationError::UnsupportedCompatibilityFeature => {
                    "UnsupportedCompatibilityFeature"
                }
                EvaluationError::InvalidAst => "InvalidAst",
                EvaluationError::InvalidResolverResult => "InvalidResolverResult",
            };
            hash.update([7]);
            length(&mut hash, code.len())?;
            hash.update(code.as_bytes());
            json!({"type":"diagnostic","value":code})
        }
    };
    let result_hash = hash
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    serde_json::to_string(
        &json!({"schema_version":1,"result":result,"normalized_result_hash":result_hash}),
    )
    .map_err(|_| AdapterError::Serialization)
}

#[wasm_bindgen(js_name = evaluate)]
pub fn evaluate_wasm(source: &str, cells_json: &str) -> Result<String, JsValue> {
    evaluate_fixture(source, cells_json).map_err(|error| JsValue::from_str(&error.to_string()))
}

/// Clipboard text transformation is the same pure Rust logic in native and WASM.
/// The server still parses and binds the returned source when it is committed.
#[wasm_bindgen(js_name = copyFormulaText)]
pub fn copy_formula_text_wasm(
    source: &str,
    from_row: u32,
    from_column: u32,
    to_row: u32,
    to_column: u32,
) -> Result<String, JsValue> {
    spreadsheet_formula::copy_formula_text(source, (from_row, from_column), (to_row, to_column))
        .map_err(|_| JsValue::from_str("InvalidFormulaCopy"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_sheet_catalog_distinguishes_blank_and_ref_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = r#"{"schema_version":1,"current_sheet":"Sheet1","sheets":["Sheet1","Empty"],"cells":[]}"#;
        let blank: Json = serde_json::from_str(&evaluate_fixture("=Empty!A1", input)?)?;
        let missing: Json = serde_json::from_str(&evaluate_fixture("=Missing!A1", input)?)?;
        assert_eq!(blank["result"]["type"], "blank");
        assert_eq!(missing["result"], json!({"type":"error","value":"#REF!"}));
        Ok(())
    }
    #[test]
    fn fixture_uses_shared_engine_and_normalizes_zero() -> Result<(), Box<dyn std::error::Error>> {
        let result: Json = serde_json::from_str(&evaluate_fixture(
            "=A1+2",
            r#"[{"row":0,"column":0,"value":{"type":"number","value":40}}]"#,
        )?)?;
        assert_eq!(result["result"]["value"], 42.0);
        assert_eq!(
            evaluate_fixture("=-0", "[]")?,
            evaluate_fixture("=0", "[]")?
        );
        Ok(())
    }
    #[test]
    fn input_boundary_rejects_ambiguous_or_oversized_data() {
        assert!(matches!(
            evaluate_fixture("=1", &" ".repeat(MAX_CELLS_JSON_BYTES + 1)),
            Err(AdapterError::LimitExceeded)
        ));
        assert!(matches!(
            evaluate_fixture(
                "=1",
                r#"[{"row":0,"column":0,"extra":true,"value":{"type":"blank"}}]"#
            ),
            Err(AdapterError::InvalidJson)
        ));
        assert!(matches!(
            evaluate_fixture(
                "=1",
                r#"[{"row":0,"column":0,"value":{"type":"blank"}},{"row":0,"column":0,"value":{"type":"blank"}}]"#
            ),
            Err(AdapterError::InvalidCoordinate)
        ));
    }
}
