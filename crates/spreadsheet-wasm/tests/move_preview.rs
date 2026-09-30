use serde_json::{Value, json};
use spreadsheet_wasm::{PreviewError, PreviewWorkbook};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn id(n: u64) -> String {
    format!("00000000-0000-0000-0000-{n:012x}")
}
fn at(column: u64) -> Value {
    json!({"sheet_id":id(2),"row_id":id(3),"column_id":id(column)})
}
fn init() -> Value {
    json!({"schema_version":1,"document_id":id(1),"input_version":"3","pending_revision":"0","sheets":[{"id":id(2),"name":"Sheet","rows":[id(3)],"columns":[id(4),id(5),id(6),id(7),id(8),id(9)],"cells":[{"row_id":id(3),"column_id":id(4),"value":{"type":"number","value":7}},{"row_id":id(3),"column_id":id(5),"value":{"type":"number","value":8}},{"row_id":id(3),"column_id":id(7),"value":{"type":"formula","value":{"source":"=A1+1"}}},{"row_id":id(3),"column_id":id(8),"value":{"type":"formula","value":{"source":"=10"}}}]}]})
}
fn command() -> Value {
    json!({"schema_version":1,"document_id":id(1),"input_version":"3","expected_pending_revision":"0","pending_revision":"1","source":at(4),"destination":at(5),"rows":1,"columns":2})
}

#[test]
fn overlapping_move_returns_rendered_inbound_formula_and_only_dirty_results() -> TestResult {
    let mut book = PreviewWorkbook::from_json(&init().to_string())?;
    book.calculate_json()?;
    let output: Value = serde_json::from_str(&book.move_json(&command().to_string())?)?;
    assert_eq!(output["input_version"], "3");
    assert_eq!(output["pending_revision"], "1");
    assert_eq!(output["preview"], true);
    assert_eq!(book.workbook().version().get(), 3);
    let writes = output["move_writes"].as_array().ok_or("writes absent")?;
    let inbound = writes
        .iter()
        .find(|w| w["column_id"] == id(7))
        .ok_or("inbound absent")?;
    assert!(
        inbound["value"]["value"]["source"]
            .as_str()
            .ok_or("source absent")?
            .contains("B1")
    );
    assert_eq!(output["evaluated_cells"], 1);
    assert_eq!(output["results"][0]["value"]["value"], 8.0);
    assert!(!output.to_string().contains("previous"));
    Ok(())
}

#[test]
fn malformed_stale_noop_crosssheet_and_partial_range_are_atomic() -> TestResult {
    let mut base = init();
    base["sheets"][0]["cells"][2]["value"]["value"]["source"] = json!("=SUM(A1:C1)");
    let mut book = PreviewWorkbook::from_json(&base.to_string())?;
    book.calculate_json()?;
    let hash = book.workbook().hash();
    assert_eq!(
        book.move_json(&command().to_string()).err(),
        Some(PreviewError::UnsupportedMoveDependency)
    );
    for (key, value) in [
        ("input_version", json!("2")),
        ("expected_pending_revision", json!("1")),
        ("pending_revision", json!("0")),
        ("rows", json!(0)),
        ("columns", json!(1001)),
        ("destination", at(4)),
        ("unknown", json!(1)),
    ] {
        let mut request = command();
        request[key] = value;
        assert!(book.move_json(&request.to_string()).is_err());
        assert_eq!(book.workbook().hash(), hash);
    }
    let mut request = command();
    request["destination"]["sheet_id"] = json!(id(90));
    assert!(book.move_json(&request.to_string()).is_err());
    assert_eq!(book.workbook().hash(), hash);
    Ok(())
}

#[test]
fn reverse_overlap_and_wholly_moved_range_follow_shared_targets() -> TestResult {
    let mut base = init();
    base["sheets"][0]["cells"][2]["value"]["value"]["source"] = json!("=SUM(A1:B1)");
    let mut book = PreviewWorkbook::from_json(&base.to_string())?;
    book.calculate_json()?;
    let forward: Value = serde_json::from_str(&book.move_json(&command().to_string())?)?;
    assert_eq!(forward["results"][0]["value"]["value"], 15.0);
    let mut reverse = command();
    reverse["source"] = at(5);
    reverse["destination"] = at(4);
    reverse["expected_pending_revision"] = json!("1");
    reverse["pending_revision"] = json!("2");
    let output: Value = serde_json::from_str(&book.move_json(&reverse.to_string())?)?;
    assert_eq!(output["results"][0]["value"]["value"], 15.0);
    let writes = output["move_writes"].as_array().ok_or("writes absent")?;
    assert_eq!(
        writes
            .iter()
            .find(|w| w["column_id"] == id(4))
            .ok_or("value missing")?["value"]["value"],
        7.0
    );
    assert_eq!(
        writes
            .iter()
            .find(|w| w["column_id"] == id(6))
            .ok_or("clear missing")?["value"]["type"],
        "blank"
    );
    Ok(())
}

#[test]
fn moved_formula_keeps_target_and_rebinds_inbound_reference() -> TestResult {
    let mut base = init();
    base["sheets"][0]["cells"][1]["value"] = json!({"type":"formula","value":{"source":"=A1+1"}});
    base["sheets"][0]["cells"][2]["value"] = json!({"type":"formula","value":{"source":"=B1+1"}});
    let mut book = PreviewWorkbook::from_json(&base.to_string())?;
    book.calculate_json()?;
    let mut request = command();
    request["source"] = at(5);
    request["destination"] = at(6);
    request["columns"] = json!(1);
    let output: Value = serde_json::from_str(&book.move_json(&request.to_string())?)?;
    let writes = output["move_writes"].as_array().ok_or("writes absent")?;
    assert!(
        writes
            .iter()
            .find(|w| w["column_id"] == id(6))
            .ok_or("moved formula missing")?["value"]["value"]["source"]
            .as_str()
            .ok_or("source missing")?
            .contains("A1")
    );
    assert!(
        writes
            .iter()
            .find(|w| w["column_id"] == id(7))
            .ok_or("inbound formula missing")?["value"]["value"]["source"]
            .as_str()
            .ok_or("source missing")?
            .contains("C1")
    );
    let results = output["results"].as_array().ok_or("results missing")?;
    assert_eq!(
        results
            .iter()
            .find(|v| v["column_id"] == id(6))
            .ok_or("moved result missing")?["value"]["value"],
        8.0
    );
    assert_eq!(
        results
            .iter()
            .find(|v| v["column_id"] == id(7))
            .ok_or("inbound result missing")?["value"]["value"],
        9.0
    );
    Ok(())
}

#[test]
fn calculation_error_rolls_back_authored_state_and_pending_revision() -> TestResult {
    let rows: Vec<_> = (100..1100).map(id).collect();
    let cells:Vec<_>=rows.iter().take(300).map(|row|json!({"row_id":row,"column_id":id(6),"value":{"type":"formula","value":{"source":"=SUM(A1:A1000)+SUM(A1:A1000)+SUM(A1:A1000)+SUM(A1:A1000)"}}})).collect();
    let base = json!({"schema_version":1,"document_id":id(1),"input_version":"3","pending_revision":"0","sheets":[{"id":id(2),"name":"Sheet","rows":rows,"columns":[id(4),id(5),id(6),id(7),id(8)],"cells":cells}]});
    let mut book = PreviewWorkbook::from_json(&base.to_string())?;
    let hash = book.workbook().hash();
    let mut request = command();
    request["source"] = at(7);
    request["destination"] = at(8);
    request["source"]["row_id"] = json!(id(100));
    request["destination"]["row_id"] = json!(id(100));
    request["columns"] = json!(1);
    assert_eq!(
        book.move_json(&request.to_string()).err(),
        Some(PreviewError::CalculationLimitExceeded)
    );
    assert_eq!(book.workbook().hash(), hash);
    // The same pending identity still reaches evaluation; it was not consumed.
    assert_eq!(
        book.move_json(&request.to_string()).err(),
        Some(PreviewError::CalculationLimitExceeded)
    );
    Ok(())
}

#[test]
fn oversized_result_rolls_back_and_preserves_previous_result_cache() -> TestResult {
    let rows: Vec<_> = (100..850).map(id).collect();
    let text = "x".repeat(6000);
    let mut cells =
        vec![json!({"row_id":rows[0],"column_id":id(4),"value":{"type":"text","value":text}})];
    for row in &rows {
        cells.push(json!({"row_id":row,"column_id":id(6),"value":{"type":"formula","value":{"source":"=$A$1"}}}));
    }
    let base = json!({"schema_version":1,"document_id":id(1),"input_version":"3","pending_revision":"0","sheets":[{"id":id(2),"name":"Sheet","rows":rows,"columns":[id(4),id(5),id(6)],"cells":cells}]});
    let mut book = PreviewWorkbook::from_json(&base.to_string())?;
    let initial: Value = serde_json::from_str(&book.calculate_json()?)?;
    let hash = book.workbook().hash();
    let mut request = command();
    request["source"]["row_id"] = json!(id(100));
    request["destination"]["row_id"] = json!(id(100));
    request["columns"] = json!(1);
    assert_eq!(
        book.move_json(&request.to_string()).err(),
        Some(PreviewError::LimitExceeded)
    );
    assert_eq!(book.workbook().hash(), hash);
    let after: Value = serde_json::from_str(&book.calculate_json()?)?;
    assert_eq!(after["pending_revision"], "0");
    assert_eq!(
        after["normalized_result_hash"],
        initial["normalized_result_hash"]
    );
    Ok(())
}

#[test]
fn volatile_context_and_old_formula_source_survive_move_preview() -> TestResult {
    let mut base = init();
    base["schema_version"] = json!(3);
    base["calculation_context"] = json!({"schema_version":1,"context_id":id(90),"epoch":"1","effective_timestamp_utc_ms":"86400000","utc_offset_minutes":180,"civil_day_since_unix_epoch":1,"random_seed":"01".repeat(32),"engine_version":spreadsheet_formula::calculation::ENGINE_VERSION,"compatibility_profile":spreadsheet_formula::calculation::COMPATIBILITY_PROFILE,"reason":"explicit"});
    base["sheets"][0]["cells"][2]["value"]["value"]["source"] = json!("=A1+NOW()");
    let mut book = PreviewWorkbook::from_json(&base.to_string())?;
    let initial: Value = serde_json::from_str(&book.calculate_json()?)?;
    let output: Value = serde_json::from_str(&book.move_json(&command().to_string())?)?;
    let before = initial["results"]
        .as_array()
        .ok_or("results missing")?
        .iter()
        .find(|v| v["column_id"] == id(7))
        .ok_or("formula missing")?;
    assert_eq!(output["results"][0]["value"], before["value"]);
    use spreadsheet_domain::workbook::CellContent;
    let stored = book.workbook().sheets()[0]
        .cells()
        .values()
        .filter_map(|c| match c {
            CellContent::Formula(f) if f.source() == "=A1+NOW()" => Some(f),
            _ => None,
        })
        .next()
        .ok_or("authored source lost")?;
    assert_eq!(stored.bound_at_version().get(), 3);
    assert_eq!(
        book.workbook()
            .calculation_context()
            .ok_or("context lost")?
            .epoch(),
        1
    );
    Ok(())
}
