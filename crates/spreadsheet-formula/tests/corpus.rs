use serde_json::{Value as Json, json};
use spreadsheet_formula::*;

struct Cells {
    cells: Json,
    sheet: String,
}
fn decode(value: &Json) -> Value {
    match value["type"].as_str() {
        Some("number") => value["value"]
            .as_f64()
            .map_or(Value::Error(FormulaError::Value), Value::Number),
        Some("boolean") => value["value"]
            .as_bool()
            .map_or(Value::Error(FormulaError::Value), Value::Boolean),
        Some("text") => value["value"]
            .as_str()
            .map_or(Value::Error(FormulaError::Value), |s| Value::Text(s.into())),
        Some("error") => Value::Error(
            [
                FormulaError::Null,
                FormulaError::Div0,
                FormulaError::Value,
                FormulaError::Ref,
                FormulaError::Name,
                FormulaError::Num,
                FormulaError::Na,
                FormulaError::Spill,
                FormulaError::Cycle,
            ]
            .into_iter()
            .find(|e| Some(e.code()) == value["value"].as_str())
            .unwrap_or(FormulaError::Value),
        ),
        _ => Value::Blank,
    }
}
impl ReferenceResolver for Cells {
    fn resolve_cell(&self, reference: &CellReference) -> Value {
        let mut number = reference.column + 1;
        let mut letters = Vec::new();
        while number > 0 {
            letters.push(char::from(b'A' + ((number - 1) % 26) as u8));
            number = (number - 1) / 26;
        }
        let column: String = letters.into_iter().rev().collect();
        let key = format!(
            "{}!{}{}",
            reference.sheet.as_ref().unwrap_or(&self.sheet),
            column,
            reference.row + 1
        );
        decode(&self.cells[&key])
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
                    row,
                    column,
                    ..range.start.clone()
                }))?;
            }
        }
        Ok(())
    }
}
#[test]
fn all_shared_profile_fixtures_match_without_excel_verification_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let corpus: Json =
        serde_json::from_str(include_str!("../../../fixtures/formulas/profile-v1.json"))?;
    assert_eq!(corpus["verified_in_excel"], false);
    let cases = corpus["cases"].as_array().ok_or("cases missing")?;
    assert!(cases.len() >= 72);
    for case in cases {
        let formula = case["formula"].as_str().ok_or("formula missing")?;
        let sheet = case["origin"]
            .as_str()
            .and_then(|s| s.rsplit_once('!'))
            .map(|(s, _)| s.to_owned())
            .ok_or("origin missing")?;
        let resolver = Cells {
            cells: case["cells"].clone(),
            sheet,
        };
        let ast = parse(formula)?.ast;
        let normalized = canonical(&ast)?;
        assert_eq!(canonical(&parse(&normalized)?.ast)?, normalized);
        let actual = match evaluate(&ast, &resolver, &mut EvaluationBudget::default()) {
            Ok(Value::Blank) => json!({"type":"blank"}),
            Ok(Value::Number(value)) => json!({"type":"number","value":value}),
            Ok(Value::Boolean(value)) => json!({"type":"boolean","value":value}),
            Ok(Value::Text(value)) => json!({"type":"text","value":value}),
            Ok(Value::Error(error)) => json!({"type":"error","value":error.code()}),
            Err(error) => json!({"type":"diagnostic","value":format!("{error:?}")}),
            _ => return Err("unexpected value kind".into()),
        };
        if actual["type"] == "number" && case["expected"]["type"] == "number" {
            assert_eq!(
                actual["value"].as_f64(),
                case["expected"]["value"].as_f64(),
                "fixture {}",
                case["id"]
            );
        } else {
            assert_eq!(actual, case["expected"], "fixture {}", case["id"]);
        }
    }
    Ok(())
}
