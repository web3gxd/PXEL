use super::{
    MAX_ARCHIVE_BYTES, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, NormalizedCellValue, NormalizedWorkbook,
    OFFICE_REL, OFFICE_REL_NS, REL_NS, SHEET_REL, WORKBOOK_NS, XlsxError,
};
use std::io::{Cursor, Write};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const WORKBOOK_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const WORKSHEET_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";

/// Serialize the supported scalar IR profile. No domain IDs or calculated caches are exported.
pub fn export_scalar_xlsx(workbook: &NormalizedWorkbook) -> Result<Vec<u8>, XlsxError> {
    workbook.validate()?;
    if !workbook.issues.is_empty() {
        return Err(XlsxError::UnsupportedFeature);
    }
    let mut parts = Vec::with_capacity(workbook.sheets.len() + 4);
    parts.push((
        "[Content_Types].xml".to_owned(),
        content_types(workbook.sheets.len()),
    ));
    parts.push(("_rels/.rels".to_owned(), root_relationships()));
    parts.push(("xl/workbook.xml".to_owned(), workbook_xml(workbook)));
    parts.push((
        "xl/_rels/workbook.xml.rels".to_owned(),
        workbook_relationships(workbook.sheets.len()),
    ));
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        parts.push((
            format!("xl/worksheets/sheet{}.xml", index + 1),
            sheet_xml(sheet)?,
        ));
    }
    let mut total = 0_u64;
    for (_, content) in &parts {
        let len = u64::try_from(content.len()).map_err(|_| XlsxError::LimitExceeded)?;
        if len > MAX_ENTRY_BYTES {
            return Err(XlsxError::LimitExceeded);
        }
        total = total.checked_add(len).ok_or(XlsxError::LimitExceeded)?;
    }
    if total > MAX_TOTAL_BYTES {
        return Err(XlsxError::LimitExceeded);
    }
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, content) in parts {
        zip.start_file(name, options).map_err(|_| XlsxError::Io)?;
        zip.write_all(content.as_bytes())
            .map_err(|_| XlsxError::Io)?;
    }
    let bytes = zip.finish().map_err(|_| XlsxError::Io)?.into_inner();
    if u64::try_from(bytes.len()).map_err(|_| XlsxError::LimitExceeded)? > MAX_ARCHIVE_BYTES {
        return Err(XlsxError::LimitExceeded);
    }
    Ok(bytes)
}

fn content_types(sheets: usize) -> String {
    let mut xml = format!(
        "<Types xmlns=\"{TYPES_NS}\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"{WORKBOOK_TYPE}\"/>"
    );
    for index in 1..=sheets {
        xml.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{index}.xml\" ContentType=\"{WORKSHEET_TYPE}\"/>"
        ));
    }
    xml.push_str("</Types>");
    xml
}

fn root_relationships() -> String {
    format!(
        "<Relationships xmlns=\"{REL_NS}\"><Relationship Id=\"r1\" Type=\"{OFFICE_REL}\" Target=\"xl/workbook.xml\"/></Relationships>"
    )
}

fn workbook_xml(workbook: &NormalizedWorkbook) -> String {
    let mut xml = format!("<workbook xmlns=\"{WORKBOOK_NS}\" xmlns:r=\"{OFFICE_REL_NS}\"><sheets>");
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        xml.push_str("<sheet name=\"");
        xml_escape_into(&mut xml, &sheet.name);
        xml.push_str(&format!(
            "\" sheetId=\"{}\" r:id=\"r{}\"/>",
            index + 1,
            index + 1
        ));
    }
    xml.push_str("</sheets></workbook>");
    xml
}

fn workbook_relationships(sheets: usize) -> String {
    let mut xml = format!("<Relationships xmlns=\"{REL_NS}\">");
    for index in 1..=sheets {
        xml.push_str(&format!(
            "<Relationship Id=\"r{index}\" Type=\"{SHEET_REL}\" Target=\"worksheets/sheet{index}.xml\"/>"
        ));
    }
    xml.push_str("</Relationships>");
    xml
}

fn sheet_xml(sheet: &super::NormalizedSheet) -> Result<String, XlsxError> {
    let mut xml = format!("<worksheet xmlns=\"{WORKBOOK_NS}\"><sheetData>");
    let mut open_row = None;
    for cell in &sheet.cells {
        if open_row != Some(cell.row) {
            if open_row.is_some() {
                xml.push_str("</row>");
            }
            xml.push_str(&format!("<row r=\"{}\">", cell.row));
            open_row = Some(cell.row);
        }
        let address = format!("{}{}", column_label(cell.column)?, cell.row);
        match &cell.value {
            NormalizedCellValue::Number(value) => {
                xml.push_str(&format!("<c r=\"{address}\"><v>{value}</v></c>"));
            }
            NormalizedCellValue::Boolean(value) => {
                xml.push_str(&format!(
                    "<c r=\"{address}\" t=\"b\"><v>{}</v></c>",
                    u8::from(*value)
                ));
            }
            NormalizedCellValue::Text(value) => {
                xml.push_str(&format!(
                    "<c r=\"{address}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">"
                ));
                xml_escape_into(&mut xml, value);
                xml.push_str("</t></is></c>");
            }
            NormalizedCellValue::Formula(value) => {
                xml.push_str(&format!("<c r=\"{address}\"><f>"));
                xml_escape_into(&mut xml, value);
                xml.push_str("</f></c>");
            }
        }
    }
    if open_row.is_some() {
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData></worksheet>");
    Ok(xml)
}

fn column_label(mut column: u32) -> Result<String, XlsxError> {
    if column == 0 || column > 16_384 {
        return Err(XlsxError::InvalidPackage);
    }
    let mut letters = Vec::with_capacity(3);
    while column > 0 {
        column -= 1;
        let digit = u8::try_from(column % 26).map_err(|_| XlsxError::InvalidPackage)?;
        letters.push(char::from(b'A' + digit));
        column /= 26;
    }
    Ok(letters.into_iter().rev().collect())
}

fn xml_escape_into(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NormalizedCell, NormalizedSheet, normalize_reader};

    #[test]
    fn scalar_fixture_roundtrips_through_own_package_reader() -> Result<(), XlsxError> {
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/xlsx/minimal_scalars.xlsx"
        ))
        .map_err(|_| XlsxError::Io)?;
        let source = normalize_reader(Cursor::new(&fixture), fixture.len() as u64)?;
        let exported = export_scalar_xlsx(&source)?;
        let reimported = normalize_reader(Cursor::new(&exported), exported.len() as u64)?;
        assert_eq!(reimported, source);
        Ok(())
    }

    #[test]
    fn escaped_text_and_xfd_coordinate_roundtrip() -> Result<(), XlsxError> {
        let source = NormalizedWorkbook {
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "R&D 'Plan' Q".to_owned(),
                cells: vec![
                    NormalizedCell {
                        row: 1,
                        column: 1,
                        value: NormalizedCellValue::Text(" <&>\"'\nю ".to_owned()),
                    },
                    NormalizedCell {
                        row: 2,
                        column: 16_384,
                        value: NormalizedCellValue::Formula("A1&\"x\"".to_owned()),
                    },
                ],
            }],
        };
        let bytes = export_scalar_xlsx(&source)?;
        assert_eq!(
            normalize_reader(Cursor::new(&bytes), bytes.len() as u64)?,
            source
        );
        Ok(())
    }

    #[test]
    fn oversized_entry_fails_before_zip_publication() {
        let source = NormalizedWorkbook {
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Big".to_owned(),
                cells: (1..=9)
                    .map(|row| NormalizedCell {
                        row,
                        column: 1,
                        value: NormalizedCellValue::Text("x".repeat(1024 * 1024)),
                    })
                    .collect(),
            }],
        };
        assert_eq!(export_scalar_xlsx(&source), Err(XlsxError::LimitExceeded));
    }

    #[test]
    fn invalid_xml_and_excel_names_are_rejected_before_encoding() {
        let mut source = NormalizedWorkbook {
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Bad/Name".to_owned(),
                cells: vec![],
            }],
        };
        assert_eq!(export_scalar_xlsx(&source), Err(XlsxError::InvalidPackage));
        source.sheets[0].name = "Valid".to_owned();
        source.sheets[0].cells.push(NormalizedCell {
            row: 1,
            column: 1,
            value: NormalizedCellValue::Text("bad\u{0}text".to_owned()),
        });
        assert_eq!(export_scalar_xlsx(&source), Err(XlsxError::InvalidPackage));
    }
}
