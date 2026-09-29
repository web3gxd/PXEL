#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use spreadsheet_wasm::{PreviewError, PreviewWorkbook};
use std::sync::atomic::AtomicBool;

const DOC: &str = "00000000-0000-0000-0000-000000000001";
const SHEET: &str = "00000000-0000-0000-0000-000000000002";
const ROW: &str = "00000000-0000-0000-0000-000000000003";
const A: &str = "00000000-0000-0000-0000-000000000004";
const B: &str = "00000000-0000-0000-0000-000000000005";
const C: &str = "00000000-0000-0000-0000-000000000006";
const SHEET_TWO: &str = "00000000-0000-0000-0000-000000000007";
const ROW_TWO: &str = "00000000-0000-0000-0000-000000000008";
const COLUMN_TWO: &str = "00000000-0000-0000-0000-000000000009";
fn init() -> Value {
    json!({"schema_version":1,"document_id":DOC,"input_version":"3","pending_revision":"0","sheets":[{"id":SHEET,"name":"Лист 1","rows":[ROW],"columns":[A,B,C],"cells":[{"row_id":ROW,"column_id":A,"value":{"type":"number","value":2}},{"row_id":ROW,"column_id":B,"value":{"type":"formula","value":{"source":"=A1+1"}}},{"row_id":ROW,"column_id":C,"value":{"type":"formula","value":{"source":"=7"}}}]}]})
}
fn cell(column: &str, value: Value) -> Value {
    json!({"sheet_id":SHEET,"row_id":ROW,"column_id":column,"value":value})
}
fn patch(cells: Vec<Value>) -> Value {
    json!({"schema_version":1,"document_id":DOC,"expected_input_version":"3","input_version":"3","expected_pending_revision":"0","pending_revision":"1","cells":cells})
}
fn calculate(book: &mut PreviewWorkbook) -> Value {
    serde_json::from_str(&book.calculate_json().unwrap()).unwrap()
}

#[test]
fn incremental_preview_is_separate_and_has_shared_core_hash() {
    let mut book = PreviewWorkbook::from_json(&init().to_string()).unwrap();
    let initial = calculate(&mut book);
    assert_eq!(initial["full"], true);
    assert_eq!(initial["evaluated_cells"], 2);
    book.apply_json(&patch(vec![cell(A, json!({"type":"number","value":8}))]).to_string())
        .unwrap();
    let updated = calculate(&mut book);
    assert_eq!(updated["preview"], true);
    assert_eq!(updated["full"], false);
    assert_eq!(updated["input_version"], "3");
    assert_eq!(updated["pending_revision"], "1");
    assert_eq!(updated["evaluated_cells"], 1);
    assert_eq!(updated["results"][0]["value"]["value"], 9.0);
    let mut core = spreadsheet_formula::calculation::CalculationProjection::from_workbook(
        book.workbook(),
        Default::default(),
    )
    .unwrap();
    core.calculate(book.workbook(), Default::default(), &AtomicBool::new(false))
        .unwrap();
    let hash = core
        .result_hash()
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(updated["normalized_result_hash"], hash);
}

#[test]
fn two_sheet_preview_recalculates_cross_sheet_dependency() {
    let mut input = init();
    input["schema_version"] = json!(2);
    input["sheets"][0]["cells"][1]["value"]["value"]["source"] = json!("='Sheet 2'!A1+1");
    input["sheets"].as_array_mut().unwrap().push(json!({
        "id": SHEET_TWO, "name": "Sheet 2", "rows": [ROW_TWO], "columns": [COLUMN_TWO],
        "cells": [{"row_id":ROW_TWO,"column_id":COLUMN_TWO,"value":{"type":"number","value":4}}]
    }));
    let mut book = PreviewWorkbook::from_json(&input.to_string()).unwrap();
    let first = calculate(&mut book);
    assert_eq!(first["evaluated_cells"], 2);
    assert_eq!(first["results"][0]["value"]["value"], 5.0);
    let mut update = patch(vec![json!({
        "sheet_id":SHEET_TWO,"row_id":ROW_TWO,"column_id":COLUMN_TWO,
        "value":{"type":"number","value":9}
    })]);
    update["schema_version"] = json!(1);
    book.apply_json(&update.to_string()).unwrap();
    let second = calculate(&mut book);
    assert_eq!(second["evaluated_cells"], 1);
    assert_eq!(second["results"][0]["value"]["value"], 10.0);
    let mut native = spreadsheet_formula::calculation::CalculationProjection::from_workbook(
        book.workbook(),
        Default::default(),
    )
    .unwrap();
    native
        .calculate(book.workbook(), Default::default(), &AtomicBool::new(false))
        .unwrap();
    let native_hash = native
        .result_hash()
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(second["normalized_result_hash"], native_hash);
}

#[test]
fn malformed_or_duplicate_batch_and_stale_revision_are_atomic() {
    let mut book = PreviewWorkbook::from_json(&init().to_string()).unwrap();
    let before = calculate(&mut book);
    let invalid = patch(vec![
        cell(A, json!({"type":"number","value":90})),
        cell(B, json!({"type":"formula","value":{"source":"=("}})),
    ]);
    assert_eq!(
        book.apply_json(&invalid.to_string()),
        Err(PreviewError::InvalidFormula)
    );
    let duplicate = patch(vec![
        cell(A, json!({"type":"number","value":90})),
        cell(A, json!({"type":"blank"})),
    ]);
    assert_eq!(
        book.apply_json(&duplicate.to_string()),
        Err(PreviewError::InvalidProjection)
    );
    let mut stale = patch(vec![]);
    stale["input_version"] = json!("2");
    assert_eq!(
        book.apply_json(&stale.to_string()),
        Err(PreviewError::StaleRevision)
    );
    let after = calculate(&mut book);
    assert_eq!(after["pending_revision"], "0");
    assert_eq!(
        after["normalized_result_hash"],
        before["normalized_result_hash"]
    );
    book.apply_json(&patch(vec![cell(A, json!({"type":"number","value":3}))]).to_string())
        .unwrap();
    assert_eq!(calculate(&mut book)["results"][0]["value"]["value"], 4.0);
}

#[test]
fn cancellation_preserves_dirty_work_and_clear_emits_tombstone() {
    let mut book = PreviewWorkbook::from_json(&init().to_string()).unwrap();
    assert_eq!(
        book.calculate_with_cancellation(&AtomicBool::new(true)),
        Err(PreviewError::Cancelled)
    );
    assert_eq!(calculate(&mut book)["full"], true);
    book.apply_json(&patch(vec![cell(B, json!({"type":"blank"}))]).to_string())
        .unwrap();
    let result = calculate(&mut book);
    assert_eq!(result["cleared"][0]["column_id"], B);
    assert_eq!(result["results"].as_array().unwrap().len(), 0);
}

#[test]
fn coalesced_version_jump_accepts_formula_metadata_at_new_base() {
    let mut book = PreviewWorkbook::from_json(&init().to_string()).unwrap();
    calculate(&mut book);
    let mut update = patch(vec![cell(
        B,
        json!({"type":"formula","value":{"source":"=A1*3","bound_at_version":"9"}}),
    )]);
    update["input_version"] = json!("10");
    book.apply_json(&update.to_string()).unwrap();
    let result = calculate(&mut book);
    assert_eq!(result["input_version"], "10");
    assert_eq!(result["results"][0]["value"]["value"], 6.0);
    assert_eq!(
        book.apply_json(&update.to_string()),
        Err(PreviewError::StaleRevision)
    );
}

#[test]
fn unknown_identity_schema_and_input_caps_fail_closed() {
    let mut input = init();
    input["schema_version"] = json!(3);
    assert!(matches!(
        PreviewWorkbook::from_json(&input.to_string()),
        Err(PreviewError::UnsupportedProfile)
    ));
    let mut book = PreviewWorkbook::from_json(&init().to_string()).unwrap();
    let invalid = patch(vec![cell(DOC, json!({"type":"blank"}))]);
    assert_eq!(
        book.apply_json(&invalid.to_string()),
        Err(PreviewError::InvalidProjection)
    );
    assert_eq!(
        book.apply_json(&" ".repeat(1024 * 1024 + 1)),
        Err(PreviewError::LimitExceeded)
    );
    assert_eq!(calculate(&mut book)["pending_revision"], "0");
}

#[test]
fn expanded_formula_ast_budget_is_checked_while_hydrating() {
    // Compact source expands to a large bound AST; a JSON byte cap alone is insufficient.
    let mut expression = "1".to_owned();
    for _ in 0..10 {
        expression = format!("({expression}+{expression})");
    }
    let source = format!("={expression}");
    let mut input = init();
    let rows = (100..1100)
        .map(|n| format!("00000000-0000-0000-0000-{n:012x}"))
        .collect::<Vec<_>>();
    input["sheets"][0]["rows"] = json!(rows);
    input["sheets"][0]["cells"] = json!(rows.iter().map(|row| json!({"row_id":row,"column_id":A,"value":{"type":"formula","value":{"source":source}}})).collect::<Vec<_>>());
    let encoded = input.to_string();
    assert!(encoded.len() < 32 * 1024 * 1024);
    assert!(matches!(
        PreviewWorkbook::from_json(&encoded),
        Err(PreviewError::LimitExceeded)
    ));
}
