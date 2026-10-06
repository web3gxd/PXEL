use prost::Message;
use serde_json::{Value, json};
use spreadsheet_domain::style::TextOverflow;
use spreadsheet_domain::workbook::*;
use spreadsheet_wasm::{PreviewError, PreviewWorkbook};
const DOC: &str = "00000000-0000-0000-0000-000000000001";
const SHEET: &str = "00000000-0000-0000-0000-000000000002";
const ROW: &str = "00000000-0000-0000-0000-000000000003";
const COL: &str = "00000000-0000-0000-0000-000000000004";
fn style() -> Value {
    json!({"font_family":"Arial","font_size_hundredth_points":1000,"font_bold":true,"font_italic":false,"font_underline":"none","font_strike":false,"font_color":[0,0,0],"fill":[255,255,255],"border_top":null,"border_right":null,"border_bottom":null,"border_left":null,"horizontal":"general","vertical":"bottom","text_overflow":"clip","number_format":"general"})
}
fn init() -> Value {
    json!({"schema_version":4,"document_id":DOC,"input_version":"3","pending_revision":"0","last_axis_change_version":"1","retired_row_ids":[],"retired_column_ids":[],"row_activation_floors":[],"column_activation_floors":[],"sheets":[{"id":SHEET,"name":"Sheet1","rows":[ROW],"columns":[COL],"cells":[],"styles":[{"row_id":ROW,"column_id":COL,"style":style()}]}]})
}
fn patch() -> Value {
    json!({"schema_version":2,"document_id":DOC,"expected_input_version":"3","input_version":"3","expected_pending_revision":"0","pending_revision":"1","cells":[],"styles":[{"sheet_id":SHEET,"row_id":ROW,"column_id":COL,"changes":[{"property":"font_italic","value":true}]}]})
}
#[test]
fn init_five_preserves_layout_metadata_and_rejects_downgrades()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = init();
    input["schema_version"] = json!(5);
    input["sheets"][0]["frozen_rows"] = json!(1);
    input["sheets"][0]["row_heights"] = json!([{"axis_id":ROW,"size_css_px":42}]);
    input["sheets"][0]["column_widths"] = json!([]);
    input["sheets"][0]["merges"] = json!([]);
    let mut worker = PreviewWorkbook::from_json(&input.to_string())?;
    let sheet = &worker.workbook().sheets()[0];
    assert_eq!(sheet.frozen_rows(), 1);
    assert_eq!(
        sheet.row_heights().values().copied().collect::<Vec<_>>(),
        [42]
    );
    let before: Value = serde_json::from_str(&worker.calculate_json()?)?;
    let after: Value = serde_json::from_str(&worker.patch_json(&patch().to_string())?)?;
    assert_eq!(worker.workbook().sheets()[0].frozen_rows(), 1);
    assert_eq!(
        after["normalized_result_hash"],
        before["normalized_result_hash"]
    );
    input["schema_version"] = json!(4);
    assert!(PreviewWorkbook::from_json(&input.to_string()).is_err());
    input["schema_version"] = json!(5);
    input["sheets"][0]["frozen_rows"] = json!(1001);
    assert!(PreviewWorkbook::from_json(&input.to_string()).is_err());
    Ok(())
}
#[test]
fn init_four_style_hash_is_authored_independent_of_calculation()
-> Result<(), Box<dyn std::error::Error>> {
    let mut worker = PreviewWorkbook::from_json(&init().to_string())?;
    let before: Value = serde_json::from_str(&worker.calculate_json()?)?;
    assert_eq!(before["schema_version"], 2);
    let after: Value = serde_json::from_str(&worker.patch_json(&patch().to_string())?)?;
    assert_eq!(
        after["normalized_result_hash"],
        before["normalized_result_hash"]
    );
    assert_ne!(
        after["normalized_authored_state_hash"],
        before["normalized_authored_state_hash"]
    );
    assert_eq!(after["evaluated_cells"], 0);
    Ok(())
}
#[test]
fn invalid_style_patch_and_cancellation_are_atomic() -> Result<(), Box<dyn std::error::Error>> {
    let mut worker = PreviewWorkbook::from_json(&init().to_string())?;
    let before: Value = serde_json::from_str(&worker.calculate_json()?)?;
    let mut invalid = patch();
    invalid["styles"][0]["changes"][0]["value"] = json!("bad");
    assert!(worker.patch_json(&invalid.to_string()).is_err());
    assert_eq!(
        worker.patch_with_cancellation(
            &patch().to_string(),
            &std::sync::atomic::AtomicBool::new(true)
        ),
        Err(PreviewError::Cancelled)
    );
    let after: Value = serde_json::from_str(&worker.calculate_json()?)?;
    assert_eq!(after["pending_revision"], "0");
    assert_eq!(
        after["normalized_authored_state_hash"],
        before["normalized_authored_state_hash"]
    );
    worker.patch_json(&patch().to_string())?;
    Ok(())
}

#[test]
fn overflow_style_change_is_hashed_by_the_preview_worker() -> Result<(), Box<dyn std::error::Error>>
{
    let mut worker = PreviewWorkbook::from_json(&init().to_string())?;
    let before: Value = serde_json::from_str(&worker.calculate_json()?)?;
    let mut input = patch();
    input["styles"][0]["changes"] = json!([{"property":"text_overflow","value":"overflow"}]);
    let after: Value = serde_json::from_str(&worker.patch_json(&input.to_string())?)?;
    assert_ne!(
        before["normalized_authored_state_hash"],
        after["normalized_authored_state_hash"]
    );
    assert_eq!(
        worker.workbook().sheets()[0]
            .styles()
            .values()
            .next()
            .map(|style| style.text_overflow()),
        Some(TextOverflow::Overflow)
    );
    assert_eq!(after["evaluated_cells"], 0);
    Ok(())
}

#[test]
fn patch_two_missing_value_duplicate_leaves_and_legacy_downgrades_fail()
-> Result<(), Box<dyn std::error::Error>> {
    let mut worker = PreviewWorkbook::from_json(&init().to_string())?;
    let mut input = patch();
    input["styles"][0]["changes"][0] = json!({"property":"font_bold"});
    assert!(worker.patch_json(&input.to_string()).is_err());
    let mut input = patch();
    input["styles"][0]["changes"] =
        json!([{"property":"font_bold","value":true},{"property":"font_bold","value":false}]);
    assert!(worker.patch_json(&input.to_string()).is_err());
    assert!(worker.apply_json(&patch().to_string()).is_err());
    let mut input = patch();
    input["schema_version"] = json!(1);
    assert!(worker.patch_json(&input.to_string()).is_err());
    assert_eq!(worker.workbook().version().get(), 3);
    Ok(())
}

fn id(n: u8) -> [u8; 16] {
    let mut v = [0; 16];
    v[15] = n;
    v
}
#[test]
fn exact_bound_ast_after_insertion_preserves_actual_authored_hash()
-> Result<(), Box<dyn std::error::Error>> {
    let document = DocumentId::from_bytes(id(1))?;
    let sheet = SheetId::from_bytes(id(2))?;
    let row = RowId::from_bytes(id(3))?;
    let col = ColumnId::from_bytes(id(4))?;
    let b = ColumnId::from_bytes(id(5))?;
    let mut book = Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet1".into(),
                rows: vec![row],
                columns: vec![col, b],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    let a = CellAddress {
        sheet,
        row,
        column: col,
    };
    let at = CellAddress { column: b, ..a };
    book = book.candidate(
        &Operation::SetCellValue {
            address: a,
            value: CellValue::Number(2.0),
        },
        DocumentVersion::new(2)?,
    )?;
    let formula = spreadsheet_formula::binder::bind("=A1+1", &book, at)?;
    book = book.candidate(
        &Operation::SetCellFormula {
            address: at,
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    book = book.candidate(
        &Operation::InsertRowsRebased {
            sheet,
            before: row,
            rows: vec![RowId::from_bytes(id(6))?],
        },
        DocumentVersion::new(4)?,
    )?;
    let Some(CellContent::Formula(formula)) = book.cell_content(at)? else {
        return Err("missing formula".into());
    };
    let hex = spreadsheet_wire_protocol::formula::encode_formula(formula)
        .encode_to_vec()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let mut input = init();
    input["input_version"] = json!("4");
    input["last_axis_change_version"] = json!("4");
    input["sheets"][0]["styles"] = json!([]);
    input["sheets"][0]["rows"] = json!(["00000000-0000-0000-0000-000000000006", ROW]);
    input["sheets"][0]["columns"] = json!([COL, "00000000-0000-0000-0000-000000000005"]);
    input["sheets"][0]["cells"] = json!([{"row_id":ROW,"column_id":COL,"value":{"type":"number","value":2}},{"row_id":ROW,"column_id":"00000000-0000-0000-0000-000000000005","value":{"type":"formula","value":{"source":formula.source(),"canonical":formula.canonical(),"ast_schema":1,"compatibility_profile":formula.profile().identifier(),"bound_at_version":formula.bound_at_version().get().to_string(),"bound_formula_wire_hex":hex}}}]);
    let mut worker = PreviewWorkbook::from_json(&input.to_string())?;
    assert_eq!(worker.workbook().hash(), book.hash());
    let output: Value = serde_json::from_str(&worker.calculate_json()?)?;
    assert_eq!(output["results"][0]["value"]["value"], 3.0);
    input["sheets"][0]["cells"][1]["value"]["value"]["source"] = json!("=A1+2");
    assert!(matches!(
        PreviewWorkbook::from_json(&input.to_string()),
        Err(PreviewError::InvalidFormula)
    ));
    Ok(())
}

#[test]
fn restored_metadata_multisheet_and_old_schema_boundaries_are_validated()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = init();
    input["retired_row_ids"] = json!([ROW]);
    input["row_activation_floors"] = json!([{"row_id":ROW,"version":"2"}]);
    input["last_axis_change_version"] = json!("2");
    input["sheets"].as_array_mut().ok_or("missing sheets")?.push(json!({"id":"00000000-0000-0000-0000-000000000007","name":"Second","rows":["00000000-0000-0000-0000-000000000008"],"columns":["00000000-0000-0000-0000-000000000009"],"cells":[],"styles":[]}));
    let worker = PreviewWorkbook::from_json(&input.to_string())?;
    assert_eq!(worker.workbook().sheets().len(), 2);
    assert_eq!(
        worker
            .workbook()
            .row_activation_floor(RowId::from_bytes(id(3))?)
            .get(),
        2
    );
    let mut malformed = input.clone();
    malformed["last_axis_change_version"] = json!("1");
    assert!(PreviewWorkbook::from_json(&malformed.to_string()).is_err());
    malformed = input.clone();
    malformed["row_activation_floors"] = json!([]);
    assert!(PreviewWorkbook::from_json(&malformed.to_string()).is_err());
    for schema in [1, 2, 3] {
        let mut old = input.clone();
        old["schema_version"] = json!(schema);
        assert!(PreviewWorkbook::from_json(&old.to_string()).is_err());
    }
    Ok(())
}

#[test]
fn patch_output_budget_rolls_back_authored_projection_cache_and_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let mut input = init();
    let columns = std::iter::once(COL.to_owned())
        .chain((10..150).map(|n| format!("00000000-0000-0000-0000-{n:012x}")))
        .collect::<Vec<_>>();
    input["sheets"][0]["columns"] = json!(columns);
    let mut worker = PreviewWorkbook::from_json(&input.to_string())?;
    let before = worker.workbook().hash();
    let mut input = patch();
    let cells=columns.iter().enumerate().map(|(index,column)|json!({"sheet_id":SHEET,"row_id":ROW,"column_id":column,"value":if index==0 {json!({"type":"text","value":"x".repeat(32768)})}else{json!({"type":"formula","value":{"source":"=A1"}})}})).collect::<Vec<_>>();
    input["cells"] = json!(cells);
    assert_eq!(
        worker.patch_json(&input.to_string()),
        Err(PreviewError::LimitExceeded)
    );
    assert_eq!(worker.workbook().hash(), before);
    let output: Value = serde_json::from_str(&worker.calculate_json()?)?;
    assert_eq!(output["pending_revision"], "0");
    assert_eq!(output["evaluated_cells"], 0);
    assert_eq!(output["results"], json!([]));
    worker.patch_json(&patch().to_string())?;
    Ok(())
}

#[test]
fn shared_styles_fixture_is_valid_and_context_is_not_advanced()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture: Value = serde_json::from_str(include_str!("../fixtures/styles-v4.json"))?;
    let mut input = fixture[0]["init"].clone();
    input["calculation_context"] = json!({"schema_version":1,"context_id":DOC,"epoch":"1","effective_timestamp_utc_ms":"86400000","utc_offset_minutes":0,"civil_day_since_unix_epoch":1,"random_seed":"01".repeat(32),"engine_version":spreadsheet_formula::calculation::ENGINE_VERSION,"compatibility_profile":spreadsheet_formula::calculation::COMPATIBILITY_PROFILE,"reason":"explicit"});
    let mut worker = PreviewWorkbook::from_json(&input.to_string())?;
    let original = worker.workbook().calculation_context().cloned();
    worker.calculate_json()?;
    worker.patch_json(&fixture[0]["patches"][0].to_string())?;
    assert_eq!(worker.workbook().calculation_context(), original.as_ref());
    Ok(())
}
