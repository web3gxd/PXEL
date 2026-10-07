//! Validated positional bridge; stable domain IDs are assigned by pxel_workbook.
use crate::{documents::CellStyleView, pxel_file::*};
use spreadsheet_domain::{
    style::CellStyle,
    workbook::{MAX_COLUMNS, MAX_ROWS},
};
use spreadsheet_xlsx::{NormalizedCellValue, NormalizedStyle, NormalizedWorkbook};
use std::collections::BTreeMap;

fn invalid<T>(_: T) -> FileError {
    FileError::Invalid
}

#[derive(Clone, serde::Serialize)]
pub struct FormulaIssue {
    pub sheet_index: u16,
    pub function: String,
    pub cells: u32,
}

/// Ask the actual evaluator dispatcher, without executing uploaded arguments.
fn supported_function(name: &str) -> bool {
    use spreadsheet_formula::{AST_SCHEMA_VERSION, Ast, FormulaError, Node, NodeId, Value};
    let ast = Ast {
        schema_version: AST_SCHEMA_VERSION,
        root: NodeId(0),
        nodes: vec![Node::Call {
            name: name.into(),
            arguments: vec![],
        }],
    };
    !matches!(
        spreadsheet_formula::evaluate(
            &ast,
            &spreadsheet_formula::NoReferences,
            &mut spreadsheet_formula::EvaluationBudget::default()
        ),
        Ok(Value::Error(FormulaError::Name))
    )
}

pub fn formula_issues(file: &Book, selection: &[u16]) -> Result<Vec<FormulaIssue>, FileError> {
    let mut capabilities = BTreeMap::new();
    let mut issues = BTreeMap::<(u16, String), u32>::new();
    for (index, sheet) in file.sheets.iter().enumerate() {
        let original = *selection.get(index).ok_or(FileError::Invalid)?;
        for cell in &sheet.cells {
            let Content::Formula(source) = &cell.content else {
                continue;
            };
            let ast = spreadsheet_formula::parse(source).map_err(invalid)?;
            let mut unsupported = std::collections::BTreeSet::new();
            for node in ast.ast.nodes {
                let spreadsheet_formula::Node::Call { name, .. } = node else {
                    continue;
                };
                let supported = *capabilities
                    .entry(name.clone())
                    .or_insert_with(|| supported_function(&name));
                if capabilities.len() > 256 {
                    return Err(FileError::Limit);
                }
                if !supported {
                    unsupported.insert(name);
                }
            }
            for function in unsupported {
                *issues.entry((original, function)).or_default() += 1;
                if issues.len() > 128 {
                    return Err(FileError::Limit);
                }
            }
        }
    }
    Ok(issues
        .into_iter()
        .map(|((sheet_index, function), cells)| FormulaIssue {
            sheet_index,
            function,
            cells,
        })
        .collect())
}

fn properties(style: &NormalizedStyle) -> Result<BTreeMap<String, serde_json::Value>, FileError> {
    let serde_json::Value::Object(mut result) =
        serde_json::to_value(CellStyleView::from(&CellStyle::default())).map_err(invalid)?
    else {
        return Err(FileError::Invalid);
    };
    let serde_json::Value::Object(fields) = serde_json::to_value(style).map_err(invalid)? else {
        return Err(FileError::Invalid);
    };
    for (key, value) in fields {
        let target = match key.as_str() {
            "row" | "column" => continue,
            "bold" => "font_bold",
            "italic" => "font_italic",
            "underline" => "font_underline",
            "strike" => "font_strike",
            "wrap" => {
                let wrap = value.as_bool().ok_or(FileError::Invalid)?;
                result.insert(
                    "text_overflow".into(),
                    serde_json::Value::String(if wrap { "wrap" } else { "clip" }.into()),
                );
                continue;
            }
            other => other,
        };
        if !result.contains_key(target) {
            return Err(FileError::Invalid);
        }
        result.insert(target.to_owned(), value);
    }
    Ok(result.into_iter().collect())
}

pub fn convert(
    imported: &NormalizedWorkbook,
    selection: &[u16],
    title: String,
) -> Result<Book, FileError> {
    imported.validate().map_err(invalid)?;
    if selection.is_empty()
        || selection.len() > imported.sheets.len()
        || !selection.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(FileError::Invalid);
    }
    let mut sheets = Vec::with_capacity(selection.len());
    for index in selection {
        let sheet = imported
            .sheets
            .get(usize::from(*index))
            .ok_or(FileError::Invalid)?;
        let rows = sheet
            .cells
            .iter()
            .map(|cell| cell.row)
            .chain(sheet.styles.iter().map(|style| style.row))
            .chain(sheet.merges.iter().map(|merge| merge.last_row))
            .max()
            .unwrap_or(0)
            .max(1000);
        let columns = sheet
            .cells
            .iter()
            .map(|cell| cell.column)
            .chain(sheet.styles.iter().map(|style| style.column))
            .chain(sheet.merges.iter().map(|merge| merge.last_column))
            .max()
            .unwrap_or(0)
            .max(26);
        if rows as usize > MAX_ROWS || columns as usize > MAX_COLUMNS {
            return Err(FileError::Limit);
        }
        let masters: BTreeMap<_, _> = sheet
            .cells
            .iter()
            .filter_map(|cell| match &cell.value {
                NormalizedCellValue::Formula(source) => Some(((cell.row, cell.column), source)),
                _ => None,
            })
            .collect();
        let cells = sheet
            .cells
            .iter()
            .map(|cell| {
                let content = match &cell.value {
                    NormalizedCellValue::Number(value) => Content::Number(*value),
                    NormalizedCellValue::Text(value) => {
                        if value.len() > spreadsheet_domain::workbook::MAX_TEXT_BYTES {
                            return Err(FileError::Limit);
                        }
                        Content::Text(value.clone())
                    }
                    NormalizedCellValue::Boolean(value) => Content::Boolean(*value),
                    NormalizedCellValue::Formula(value) => Content::Formula(format!("={value}")),
                    NormalizedCellValue::FormulaCopy {
                        master_row,
                        master_column,
                    } => {
                        let source = masters
                            .get(&(*master_row, *master_column))
                            .ok_or(FileError::Invalid)?;
                        Content::Formula(
                            spreadsheet_formula::copy_formula_text(
                                &format!("={source}"),
                                (*master_row - 1, *master_column - 1),
                                (cell.row - 1, cell.column - 1),
                            )
                            .map_err(invalid)?,
                        )
                    }
                };
                Ok(Cell {
                    row: cell.row - 1,
                    column: cell.column - 1,
                    content,
                })
            })
            .collect::<Result<_, FileError>>()?;
        let styles = sheet
            .styles
            .iter()
            .map(|style| {
                Ok(Style {
                    row: style.row - 1,
                    column: style.column - 1,
                    properties: properties(style)?,
                })
            })
            .collect::<Result<_, FileError>>()?;
        sheets.push(Sheet {
            name: sheet.name.clone(),
            rows,
            columns,
            frozen_rows: 0,
            view: None,
            cells,
            styles,
            row_heights: vec![],
            column_widths: vec![],
            merges: sheet
                .merges
                .iter()
                .map(|merge| Merge {
                    first_row: merge.first_row - 1,
                    last_row: merge.last_row - 1,
                    first_column: merge.first_column - 1,
                    last_column: merge.last_column - 1,
                })
                .collect(),
        });
    }
    let names: BTreeMap<_, _> = sheets
        .iter()
        .enumerate()
        .map(|(index, sheet)| (sheet.name.to_lowercase(), index))
        .collect();
    let mut extents: Vec<_> = sheets
        .iter()
        .map(|sheet| (sheet.rows, sheet.columns))
        .collect();
    for (origin, sheet) in sheets.iter().enumerate() {
        for cell in &sheet.cells {
            let Content::Formula(source) = &cell.content else {
                continue;
            };
            let ast = spreadsheet_formula::parse(source).map_err(invalid)?;
            for node in &ast.ast.nodes {
                let references: &[&spreadsheet_formula::ast::CellReference] = match node {
                    spreadsheet_formula::ast::Node::Cell(reference) => &[reference],
                    spreadsheet_formula::ast::Node::Range(range) => &[&range.start, &range.end],
                    _ => &[],
                };
                for reference in references {
                    let target = match &reference.sheet {
                        Some(name) => *names.get(&name.to_lowercase()).ok_or(FileError::Invalid)?,
                        None => origin,
                    };
                    let rows = reference.row.checked_add(1).ok_or(FileError::Limit)?;
                    let columns = reference.column.checked_add(1).ok_or(FileError::Limit)?;
                    if rows as usize > MAX_ROWS || columns as usize > MAX_COLUMNS {
                        return Err(FileError::Limit);
                    }
                    extents[target].0 = extents[target].0.max(rows);
                    extents[target].1 = extents[target].1.max(columns);
                }
            }
        }
    }
    for (sheet, (rows, columns)) in sheets.iter_mut().zip(extents) {
        sheet.rows = rows;
        sheet.columns = columns;
    }
    Ok(Book {
        schema_version: 1,
        engine_version: spreadsheet_collab::calculation::ENGINE_VERSION.into(),
        compatibility_profile: spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
        title,
        anonymized: false,
        sheets,
        settings: Settings::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::{ActorId, DocumentId};
    use uuid::Uuid;

    #[test]
    fn evaluator_capability_probe_reports_unknown_functions_without_evaluating_arguments() {
        for name in [
            "SUM", "SUMIFS", "NOW", "RAND", "INDEX", "XLOOKUP", "COUNT", "IFERROR", "DATE",
        ] {
            assert!(supported_function(name));
        }
        for name in ["PXELUNKNOWN", "FILTER", "QUERY"] {
            assert!(!supported_function(name));
        }
    }

    #[test]
    fn selected_book_preserves_shared_formulas_styles_and_retry_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        for bytes in [
            include_bytes!("../../../fixtures/xlsx/styled_profile.xlsx").as_slice(),
            include_bytes!("../../../fixtures/xlsx/excel-shared-formulas.xlsx").as_slice(),
        ] {
            let ir = spreadsheet_xlsx::normalize_reader(
                std::io::Cursor::new(bytes),
                bytes.len() as u64,
            )?;
            let selection: Vec<_> = (0..ir.sheets.len() as u16).collect();
            let file = convert(&ir, &selection, "Импорт".into())?;
            let request = Uuid::now_v7();
            let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
            let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
            let first =
                crate::pxel_workbook::prepare_xlsx(&file, document, actor, request, b"digest")?;
            let retry =
                crate::pxel_workbook::prepare_xlsx(&file, document, actor, request, b"digest")?;
            assert_eq!(first.book.hash(), retry.book.hash());
            assert_eq!(
                first
                    .events
                    .iter()
                    .map(|e| e.operation_id)
                    .collect::<Vec<_>>(),
                retry
                    .events
                    .iter()
                    .map(|e| e.operation_id)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                file.sheets.iter().map(|s| s.styles.len()).sum::<usize>(),
                ir.sheets.iter().map(|s| s.styles.len()).sum::<usize>()
            );
            assert!(file.sheets.iter().flat_map(|s| &s.cells).all(|c| !matches!(&c.content, crate::pxel_file::Content::Formula(source) if !source.starts_with('='))));
        }
        Ok(())
    }
    #[test]
    fn mixed_scalar_formula_profile_keeps_heavy_events_bounded_and_replayable()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = include_bytes!("../../../fixtures/xlsx/minimal_scalars.xlsx");
        let mut ir =
            spreadsheet_xlsx::normalize_reader(std::io::Cursor::new(bytes), bytes.len() as u64)?;
        ir.sheets[0].cells = (0..2000)
            .map(|index| spreadsheet_xlsx::NormalizedCell {
                row: index / 500 + 1,
                column: index % 500 + 1,
                value: if index == 1001 {
                    NormalizedCellValue::Formula("1+2".into())
                } else {
                    NormalizedCellValue::Number(f64::from(index))
                },
            })
            .collect();
        let file = convert(&ir, &[0], "Импорт".into())?;
        let prepared = crate::pxel_workbook::prepare_xlsx(
            &file,
            DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?,
            ActorId::from_bytes(*Uuid::now_v7().as_bytes())?,
            Uuid::now_v7(),
            b"mixed-profile",
        )?;
        let mut targets = 0;
        let mut large_scalar = false;
        for event in &prepared.events {
            if let spreadsheet_domain::workbook::Operation::CellBatchWrite {
                content_writes,
                style_writes,
                ..
            } = &event.operation
            {
                let heavy = !style_writes.is_empty()
                    || content_writes.iter().any(|write| {
                        matches!(
                            write.content,
                            Some(spreadsheet_domain::workbook::CellContent::Formula(_))
                        )
                    });
                assert!(content_writes.len() <= if heavy { 128 } else { 1000 });
                large_scalar |= content_writes.len() > 128;
                targets += content_writes.len();
            }
            spreadsheet_storage::validate_import_event_size(event)?;
        }
        assert_eq!(targets, 2000);
        assert!(large_scalar);
        Ok(())
    }

    #[test]
    fn empty_reference_targets_expand_axes_only_within_domain_profile()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = include_bytes!("../../../fixtures/xlsx/minimal_scalars.xlsx");
        let mut ir =
            spreadsheet_xlsx::normalize_reader(std::io::Cursor::new(bytes), bytes.len() as u64)?;
        let formula = ir.sheets[0]
            .cells
            .iter_mut()
            .find(|cell| matches!(cell.value, NormalizedCellValue::Formula(_)))
            .ok_or("missing fixture formula")?;
        formula.value = NormalizedCellValue::Formula("AA2000".into());
        let file = convert(&ir, &[0, 1], "Импорт".into())?;
        assert_eq!(file.sheets[0].rows, 2000);
        assert_eq!(file.sheets[0].columns, 27);
        let formula = ir.sheets[0]
            .cells
            .iter_mut()
            .find(|cell| matches!(cell.value, NormalizedCellValue::Formula(_)))
            .ok_or("missing fixture formula")?;
        formula.value = NormalizedCellValue::Formula("A100001".into());
        assert!(matches!(
            convert(&ir, &[0, 1], "Импорт".into()),
            Err(FileError::Limit)
        ));
        Ok(())
    }

    #[test]
    fn selection_and_missing_cross_sheet_target_fail_atomically()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = include_bytes!("../../../fixtures/xlsx/cross_sheet_formula.xlsx");
        let ir =
            spreadsheet_xlsx::normalize_reader(std::io::Cursor::new(bytes), bytes.len() as u64)?;
        for selection in [vec![], vec![0, 0], vec![1, 0], vec![64]] {
            assert!(convert(&ir, &selection, "Импорт".into()).is_err());
        }
        let all = convert(&ir, &[0, 1], "Импорт".into())?;
        let request = Uuid::now_v7();
        let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        assert!(
            crate::pxel_workbook::prepare_xlsx(&all, document, actor, request, b"digest").is_ok()
        );
        assert!(convert(&ir, &[0], "Импорт".into()).is_err());
        Ok(())
    }
}
