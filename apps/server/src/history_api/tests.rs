use super::*;
use spreadsheet_domain::{
    axis_size::{AxisSizeChange, AxisSizeTarget},
    merge::{MergeChange, MergeRange},
    style::{Change, Rgb, StyleChange},
    style_write::{self, StyleWriteKind},
};
#[test]
fn stable_id_diff_covers_every_category_and_structural_formula_context()
-> Result<(), Box<dyn std::error::Error>> {
    let doc = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let rows = (3..6)
        .map(|n| RowId::from_bytes([n; 16]))
        .collect::<Result<Vec<_>, _>>()?;
    let columns = (6..8)
        .map(|n| ColumnId::from_bytes([n; 16]))
        .collect::<Result<Vec<_>, _>>()?;
    let mut book = Workbook::empty(doc).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Diff".into(),
                rows: rows.clone(),
                columns: columns.clone(),
            },
        },
        DocumentVersion::new(1)?,
    )?;
    let cell = |r: usize, c: usize| CellAddress {
        sheet,
        row: rows[r],
        column: columns[c],
    };
    book = book.candidate(
        &Operation::SetCellValue {
            address: cell(0, 0),
            value: CellValue::Number(7.),
        },
        DocumentVersion::new(2)?,
    )?;
    let formula = spreadsheet_formula::binder::bind("=A1*2", &book, cell(0, 1))?;
    book = book.candidate(
        &Operation::SetCellFormula {
            address: cell(0, 1),
            formula,
        },
        DocumentVersion::new(3)?,
    )?;
    let before = book.clone();
    let mutate = |book: &Workbook, operation: Operation| {
        book.candidate(&operation, DocumentVersion::new(book.version().get() + 1)?)
    };
    book = mutate(
        &book,
        Operation::SetCellValue {
            address: cell(0, 0),
            value: CellValue::Number(8.),
        },
    )?;
    let style = style_write::prepare_style_write(
        &book,
        &[cell(0, 1)],
        &[
            StyleChange::FontBold(Change::Set(true)),
            StyleChange::Fill(Change::Set(Rgb::new([200, 230, 210]))),
        ],
        StyleWriteKind::Cell,
    )?;
    book = mutate(&book, style)?;
    book = mutate(
        &book,
        Operation::AxisSizeWrite {
            sheet,
            writes: vec![AxisSizeChange {
                target: AxisSizeTarget::Row(rows[0]),
                size: Some(48),
                previous: None,
            }],
        },
    )?;
    book = mutate(
        &book,
        Operation::FrozenRowsWrite {
            sheet,
            rows: 1,
            previous: 0,
        },
    )?;
    book = mutate(
        &book,
        Operation::InsertRowsRebased {
            sheet,
            before: rows[0],
            rows: vec![RowId::from_bytes([8; 16])?],
        },
    )?;
    book = mutate(
        &book,
        Operation::MergeWrite {
            sheet,
            change: MergeChange {
                range: MergeRange::new(rows[2], rows[2], columns[0], columns[1]),
                merged: true,
                previous: false,
            },
            undoes: None,
            undoes_version: None,
        },
    )?;
    book = mutate(
        &book,
        Operation::AddSheet {
            sheet: SheetSeed {
                id: SheetId::from_bytes([9; 16])?,
                name: "Extra".into(),
                rows: vec![RowId::from_bytes([10; 16])?],
                columns: vec![ColumnId::from_bytes([11; 16])?],
            },
        },
    )?;
    let hashes = (before.hash(), book.hash());
    let result = diff(&before, &book).map_err(|e| e.1)?;
    for category in [
        "value",
        "formula",
        "calculated",
        "style",
        "size",
        "merge",
        "sheet",
        "structure",
        "layout",
    ] {
        assert!(
            result.changes.iter().any(|c| c.category == category),
            "{category}"
        );
    }
    let styles = result
        .changes
        .iter()
        .filter(|c| c.category == "style")
        .collect::<Vec<_>>();
    assert_eq!(styles.len(), 2);
    assert!(styles.iter().any(|c| c.property == "font_bold"
        && c.before == Some(json!(false))
        && c.after == Some(json!(true))));
    let formula = result
        .changes
        .iter()
        .find(|c| c.category == "formula")
        .ok_or("formula diff")?;
    assert!(formula.structural_context);
    assert_eq!(
        formula.before.as_ref().and_then(|v| v.get("current_text")),
        Some(&json!("=(A1*2)"))
    );
    assert_eq!(
        formula.after.as_ref().and_then(|v| v.get("current_text")),
        Some(&json!("=(A2*2)"))
    );
    let calculated = result
        .changes
        .iter()
        .find(|c| c.category == "calculated")
        .ok_or("calculated diff")?;
    assert_eq!(
        calculated.before,
        Some(json!({"type":"number","value":14.}))
    );
    assert_eq!(calculated.after, Some(json!({"type":"number","value":16.})));
    assert_eq!(before.hash(), hashes.0);
    assert_eq!(book.hash(), hashes.1);
    assert!(before.calculation_context().is_none());
    assert!(book.calculation_context().is_none());
    assert!(diff(&book, &book).map_err(|e| e.1)?.changes.is_empty());
    let reverse = diff(&book, &before).map_err(|e| e.1)?;
    assert_eq!(reverse.changes.len(), result.changes.len());
    Ok(())
}

#[test]
fn settings_only_versions_have_per_leaf_historical_diff() -> Result<(), Box<dyn std::error::Error>>
{
    let seed = SheetSeed {
        id: SheetId::from_bytes([2; 16])?,
        name: "History".into(),
        rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?],
        columns: vec![
            ColumnId::from_bytes([5; 16])?,
            ColumnId::from_bytes([6; 16])?,
        ],
    };
    let book = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
        &Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
        DocumentVersion::new(1)?,
    )?;
    let changes = spreadsheet_domain::workbook::SheetSettingsPatch {
        default_column_width: Some(160),
        tab_color: Some(Some(0x123456)),
        visibility: vec![
            (AxisSizeTarget::Row(seed.rows[1]), true),
            (AxisSizeTarget::Column(seed.columns[1]), true),
        ],
        ..Default::default()
    };
    let next = book.candidate(
        &spreadsheet_domain::sheet_settings::prepare(&book, seed.id, &changes)?,
        DocumentVersion::new(2)?,
    )?;
    let before = comparable(&book).map_err(|error| error.1)?;
    let after = comparable(&next).map_err(|error| error.1)?;
    let changed = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, _)| key.property.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        changed,
        BTreeSet::from([
            "default_column_width",
            "tab_color",
            "row_hidden",
            "column_hidden"
        ])
    );
    Ok(())
}
