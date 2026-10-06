use super::{
    MAX_ARCHIVE_BYTES, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES, NormalizedBorder, NormalizedBorderLine,
    NormalizedCell, NormalizedCellValue, NormalizedHorizontal, NormalizedNumberFormat,
    NormalizedStyle, NormalizedUnderline, NormalizedVertical, NormalizedWorkbook, OFFICE_REL,
    OFFICE_REL_NS, REL_NS, SHEET_REL, WORKBOOK_NS, XlsxError,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Write},
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const WORKBOOK_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml";
const WORKSHEET_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
const STYLES_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml";

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct StyleKey {
    font_family: Option<String>,
    font_size: Option<u16>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<NormalizedUnderline>,
    strike: Option<bool>,
    font_color: Option<[u8; 3]>,
    fill: Option<[u8; 3]>,
    borders: [Option<NormalizedBorder>; 4],
    horizontal: Option<NormalizedHorizontal>,
    vertical: Option<NormalizedVertical>,
    wrap: Option<bool>,
    number_format: Option<NormalizedNumberFormat>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
struct FontKey {
    family: Option<String>,
    size: Option<u16>,
    bold: bool,
    italic: bool,
    underline: Option<NormalizedUnderline>,
    strike: bool,
    color: Option<[u8; 3]>,
}

#[derive(Clone, Debug)]
struct StyleTables {
    style_ids: BTreeMap<StyleKey, u32>,
    font_ids: BTreeMap<FontKey, u32>,
    fill_ids: BTreeMap<[u8; 3], u32>,
    border_ids: BTreeMap<[Option<NormalizedBorder>; 4], u32>,
}

impl StyleTables {
    fn new(workbook: &NormalizedWorkbook) -> Result<Self, XlsxError> {
        let mut styles = BTreeSet::new();
        for sheet in &workbook.sheets {
            for style in &sheet.styles {
                styles.insert(style_key(style));
            }
        }
        let mut fonts = BTreeSet::from([FontKey::default()]);
        let mut fills = BTreeSet::new();
        let mut borders = BTreeSet::from([[None; 4]]);
        for style in &styles {
            fonts.insert(font_key(style));
            if let Some(fill) = style.fill {
                fills.insert(fill);
            }
            borders.insert(style.borders);
        }
        let mut font_ids = BTreeMap::from([(FontKey::default(), 0)]);
        for font in fonts.into_iter().filter(|font| *font != FontKey::default()) {
            let id = u32::try_from(font_ids.len()).map_err(|_| XlsxError::LimitExceeded)?;
            font_ids.insert(font, id);
        }
        let mut fill_ids = BTreeMap::new();
        for fill in fills {
            let id = u32::try_from(fill_ids.len() + 2).map_err(|_| XlsxError::LimitExceeded)?;
            fill_ids.insert(fill, id);
        }
        let mut border_ids = BTreeMap::from([([None; 4], 0)]);
        for border in borders.into_iter().filter(|border| *border != [None; 4]) {
            let id = u32::try_from(border_ids.len()).map_err(|_| XlsxError::LimitExceeded)?;
            border_ids.insert(border, id);
        }
        let mut style_ids = BTreeMap::new();
        for style in styles {
            let id = u32::try_from(style_ids.len() + 1).map_err(|_| XlsxError::LimitExceeded)?;
            style_ids.insert(style, id);
        }
        Ok(Self {
            style_ids,
            font_ids,
            fill_ids,
            border_ids,
        })
    }
}

fn style_key(style: &NormalizedStyle) -> StyleKey {
    StyleKey {
        font_family: style.font_family.clone(),
        font_size: style.font_size_hundredth_points,
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strike,
        font_color: style.font_color,
        fill: style.fill,
        borders: [
            style.border_top,
            style.border_right,
            style.border_bottom,
            style.border_left,
        ],
        horizontal: style.horizontal,
        vertical: style.vertical,
        wrap: style.wrap,
        number_format: style.number_format,
    }
}

fn font_key(style: &StyleKey) -> FontKey {
    FontKey {
        family: style.font_family.clone(),
        size: style.font_size,
        bold: style.bold.unwrap_or(false),
        italic: style.italic.unwrap_or(false),
        underline: style.underline,
        strike: style.strike.unwrap_or(false),
        color: style.font_color,
    }
}

/// Serialize the bounded normalized profile. No domain IDs or calculated caches are exported.
pub fn export_scalar_xlsx(workbook: &NormalizedWorkbook) -> Result<Vec<u8>, XlsxError> {
    workbook.validate()?;
    if !workbook.issues.is_empty() {
        return Err(XlsxError::UnsupportedFeature);
    }
    let style_tables = StyleTables::new(workbook)?;
    let has_styles = !style_tables.style_ids.is_empty();
    let mut parts = Vec::with_capacity(workbook.sheets.len() + if has_styles { 5 } else { 4 });
    parts.push((
        "[Content_Types].xml".to_owned(),
        content_types(workbook.sheets.len(), has_styles),
    ));
    parts.push(("_rels/.rels".to_owned(), root_relationships()));
    parts.push(("xl/workbook.xml".to_owned(), workbook_xml(workbook)));
    parts.push((
        "xl/_rels/workbook.xml.rels".to_owned(),
        workbook_relationships(workbook.sheets.len(), has_styles),
    ));
    if has_styles {
        parts.push(("xl/styles.xml".to_owned(), styles_xml(&style_tables)?));
    }
    for (index, sheet) in workbook.sheets.iter().enumerate() {
        parts.push((
            format!("xl/worksheets/sheet{}.xml", index + 1),
            sheet_xml(sheet, &style_tables)?,
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

fn content_types(sheets: usize, has_styles: bool) -> String {
    let mut xml = format!(
        "<Types xmlns=\"{TYPES_NS}\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"{WORKBOOK_TYPE}\"/>"
    );
    for index in 1..=sheets {
        xml.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{index}.xml\" ContentType=\"{WORKSHEET_TYPE}\"/>"
        ));
    }
    if has_styles {
        xml.push_str(&format!(
            "<Override PartName=\"/xl/styles.xml\" ContentType=\"{STYLES_TYPE}\"/>"
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

fn workbook_relationships(sheets: usize, has_styles: bool) -> String {
    let mut xml = format!("<Relationships xmlns=\"{REL_NS}\">");
    for index in 1..=sheets {
        xml.push_str(&format!(
            "<Relationship Id=\"r{index}\" Type=\"{SHEET_REL}\" Target=\"worksheets/sheet{index}.xml\"/>"
        ));
    }
    if has_styles {
        xml.push_str(&format!("<Relationship Id=\"r{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>", sheets + 1));
    }
    xml.push_str("</Relationships>");
    xml
}

fn sheet_xml(sheet: &super::NormalizedSheet, tables: &StyleTables) -> Result<String, XlsxError> {
    let mut xml = format!("<worksheet xmlns=\"{WORKBOOK_NS}\"><sheetData>");
    let mut open_row = None;
    let mut records = BTreeMap::<(u32, u32), (Option<&NormalizedCell>, Option<u32>)>::new();
    for cell in &sheet.cells {
        records.entry((cell.row, cell.column)).or_default().0 = Some(cell);
    }
    for style in &sheet.styles {
        let id = *tables
            .style_ids
            .get(&style_key(style))
            .ok_or(XlsxError::InvalidPackage)?;
        records.entry((style.row, style.column)).or_default().1 = Some(id);
    }
    for ((row, column), (cell, style_id)) in records {
        if open_row != Some(row) {
            if open_row.is_some() {
                xml.push_str("</row>");
            }
            xml.push_str(&format!("<row r=\"{row}\">"));
            open_row = Some(row);
        }
        let address = format!("{}{}", column_label(column)?, row);
        let style_attribute = style_id.map_or_else(String::new, |id| format!(" s=\"{id}\""));
        let Some(cell) = cell else {
            xml.push_str(&format!("<c r=\"{address}\"{style_attribute}/>"));
            continue;
        };
        match &cell.value {
            NormalizedCellValue::Number(value) => {
                xml.push_str(&format!(
                    "<c r=\"{address}\"{style_attribute}><v>{value}</v></c>"
                ));
            }
            NormalizedCellValue::Boolean(value) => {
                xml.push_str(&format!(
                    "<c r=\"{address}\"{style_attribute} t=\"b\"><v>{}</v></c>",
                    u8::from(*value)
                ));
            }
            NormalizedCellValue::Text(value) => {
                xml.push_str(&format!(
                    "<c r=\"{address}\"{style_attribute} t=\"inlineStr\"><is><t xml:space=\"preserve\">"
                ));
                xml_escape_into(&mut xml, value);
                xml.push_str("</t></is></c>");
            }
            NormalizedCellValue::Formula(value) => {
                xml.push_str(&format!("<c r=\"{address}\"{style_attribute}><f>"));
                xml_escape_into(&mut xml, value);
                xml.push_str("</f></c>");
            }
            NormalizedCellValue::FormulaCopy { .. } => return Err(XlsxError::UnsupportedFeature),
        }
    }
    if open_row.is_some() {
        xml.push_str("</row>");
    }
    xml.push_str("</sheetData>");
    if !sheet.merges.is_empty() {
        xml.push_str(&format!("<mergeCells count=\"{}\">", sheet.merges.len()));
        for merge in &sheet.merges {
            xml.push_str(&format!(
                "<mergeCell ref=\"{}{}:{}{}\"/>",
                column_label(merge.first_column)?,
                merge.first_row,
                column_label(merge.last_column)?,
                merge.last_row,
            ));
        }
        xml.push_str("</mergeCells>");
    }
    xml.push_str("</worksheet>");
    Ok(xml)
}

fn styles_xml(tables: &StyleTables) -> Result<String, XlsxError> {
    let mut xml = format!("<styleSheet xmlns=\"{WORKBOOK_NS}\">");
    let has_date = tables
        .style_ids
        .keys()
        .any(|style| style.number_format == Some(NormalizedNumberFormat::DateIso));
    if has_date {
        xml.push_str(
            "<numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"yyyy-mm-dd\"/></numFmts>",
        );
    }

    let mut fonts = tables.font_ids.iter().collect::<Vec<_>>();
    fonts.sort_by_key(|(_, id)| **id);
    xml.push_str(&format!("<fonts count=\"{}\">", fonts.len()));
    for (font, _) in fonts {
        xml.push_str("<font><name val=\"");
        xml_escape_into(&mut xml, font.family.as_deref().unwrap_or("Arial"));
        let size = font.size.unwrap_or(1000);
        xml.push_str(&format!(
            "\"/><sz val=\"{}.{:02}\"/>",
            size / 100,
            size % 100
        ));
        if font.bold {
            xml.push_str("<b/>");
        }
        if font.italic {
            xml.push_str("<i/>");
        }
        if let Some(underline) = font.underline {
            xml.push_str(match underline {
                NormalizedUnderline::Single => "<u val=\"single\"/>",
                NormalizedUnderline::Double => "<u val=\"double\"/>",
            });
        }
        if font.strike {
            xml.push_str("<strike/>");
        }
        if let Some(color) = font.color {
            xml.push_str(&format!("<color rgb=\"FF{}\"/>", rgb(color)));
        }
        xml.push_str("</font>");
    }
    xml.push_str("</fonts>");

    let mut fills = tables.fill_ids.iter().collect::<Vec<_>>();
    fills.sort_by_key(|(_, id)| **id);
    xml.push_str(&format!("<fills count=\"{}\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill>", fills.len() + 2));
    for (color, _) in fills {
        xml.push_str(&format!(
            "<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF{}\"/></patternFill></fill>",
            rgb(*color)
        ));
    }
    xml.push_str("</fills>");

    let mut borders = tables
        .border_ids
        .iter()
        .filter(|(_, id)| **id != 0)
        .collect::<Vec<_>>();
    borders.sort_by_key(|(_, id)| **id);
    xml.push_str(&format!("<borders count=\"{}\">", borders.len() + 1));
    xml.push_str("<border><left/><right/><top/><bottom/><diagonal/></border>");
    for (border, _) in borders {
        xml.push_str("<border>");
        for (name, edge) in ["top", "right", "bottom", "left"].iter().zip(border.iter()) {
            if let Some(edge) = edge {
                xml.push_str(&format!(
                    "<{name} style=\"{}\"><color rgb=\"FF{}\"/></{name}>",
                    border_name(edge.line),
                    rgb(edge.color)
                ));
            } else {
                xml.push_str(&format!("<{name}/>"));
            }
        }
        xml.push_str("<diagonal/></border>");
    }
    xml.push_str("</borders>");

    xml.push_str("<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>");
    let mut styles = tables.style_ids.iter().collect::<Vec<_>>();
    styles.sort_by_key(|(_, id)| **id);
    xml.push_str(&format!("<cellXfs count=\"{}\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>", styles.len() + 1));
    for (style, _) in styles {
        let font_id = *tables
            .font_ids
            .get(&font_key(style))
            .ok_or(XlsxError::InvalidPackage)?;
        let fill_id = style
            .fill
            .map_or(0, |fill| tables.fill_ids.get(&fill).copied().unwrap_or(0));
        let border_key = style.borders;
        let border_id = tables.border_ids.get(&border_key).copied().unwrap_or(0);
        let format_id = match style.number_format {
            None | Some(NormalizedNumberFormat::General) => 0,
            Some(NormalizedNumberFormat::Integer) => 1,
            Some(NormalizedNumberFormat::Decimal2) => 2,
            Some(NormalizedNumberFormat::Percent0) => 9,
            Some(NormalizedNumberFormat::Percent2) => 10,
            Some(NormalizedNumberFormat::Scientific2) => 11,
            Some(NormalizedNumberFormat::DateIso) => 164,
        };
        xml.push_str(&format!("<xf numFmtId=\"{format_id}\" fontId=\"{font_id}\" fillId=\"{fill_id}\" borderId=\"{border_id}\" xfId=\"0\""));
        if style.horizontal.is_some() || style.vertical.is_some() || style.wrap.unwrap_or(false) {
            xml.push_str(" applyAlignment=\"1\"><alignment");
            if let Some(value) = style.horizontal {
                xml.push_str(&format!(" horizontal=\"{}\"", horizontal_name(value)));
            }
            if let Some(value) = style.vertical {
                xml.push_str(&format!(" vertical=\"{}\"", vertical_name(value)));
            }
            if style.wrap.unwrap_or(false) {
                xml.push_str(" wrapText=\"1\"");
            }
            xml.push_str("/></xf>");
        } else {
            xml.push_str("/>");
        }
    }
    xml.push_str("</cellXfs></styleSheet>");
    Ok(xml)
}

fn rgb(value: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", value[0], value[1], value[2])
}
fn border_name(value: NormalizedBorderLine) -> &'static str {
    match value {
        NormalizedBorderLine::Thin => "thin",
        NormalizedBorderLine::Medium => "medium",
        NormalizedBorderLine::Thick => "thick",
        NormalizedBorderLine::Dashed => "dashed",
        NormalizedBorderLine::Dotted => "dotted",
        NormalizedBorderLine::Double => "double",
    }
}
fn horizontal_name(value: NormalizedHorizontal) -> &'static str {
    match value {
        NormalizedHorizontal::General => "general",
        NormalizedHorizontal::Left => "left",
        NormalizedHorizontal::Center => "center",
        NormalizedHorizontal::Right => "right",
    }
}
fn vertical_name(value: NormalizedVertical) -> &'static str {
    match value {
        NormalizedVertical::Top => "top",
        NormalizedVertical::Center => "center",
        NormalizedVertical::Bottom => "bottom",
    }
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
    use crate::{
        NormalizedBorder, NormalizedCell, NormalizedSheet, NormalizedStyle, normalize_reader,
    };

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
    fn deterministic_style_roundtrip_includes_styled_blank_formula_and_values()
    -> Result<(), XlsxError> {
        let rich = NormalizedStyle {
            row: 1,
            column: 1,
            font_family: Some("Calibri".into()),
            font_size_hundredth_points: Some(1200),
            bold: Some(true),
            italic: Some(true),
            underline: Some(NormalizedUnderline::Single),
            strike: Some(true),
            font_color: Some([1, 2, 3]),
            fill: Some([4, 5, 6]),
            border_top: Some(NormalizedBorder {
                line: NormalizedBorderLine::Thin,
                color: [10, 11, 12],
            }),
            border_right: Some(NormalizedBorder {
                line: NormalizedBorderLine::Medium,
                color: [13, 14, 15],
            }),
            border_bottom: Some(NormalizedBorder {
                line: NormalizedBorderLine::Dashed,
                color: [16, 17, 18],
            }),
            border_left: Some(NormalizedBorder {
                line: NormalizedBorderLine::Double,
                color: [19, 20, 21],
            }),
            horizontal: Some(NormalizedHorizontal::Left),
            vertical: Some(NormalizedVertical::Top),
            wrap: Some(true),
            number_format: Some(NormalizedNumberFormat::Decimal2),
        };
        let blank = NormalizedStyle {
            row: 2,
            column: 2,
            font_family: None,
            font_size_hundredth_points: None,
            bold: None,
            italic: None,
            underline: None,
            strike: None,
            font_color: None,
            fill: None,
            border_top: None,
            border_right: None,
            border_bottom: None,
            border_left: None,
            horizontal: Some(NormalizedHorizontal::Center),
            vertical: None,
            wrap: None,
            number_format: Some(NormalizedNumberFormat::DateIso),
        };
        let formula_style = NormalizedStyle {
            row: 3,
            column: 3,
            font_family: None,
            font_size_hundredth_points: None,
            bold: Some(true),
            italic: None,
            underline: None,
            strike: None,
            font_color: None,
            fill: None,
            border_top: None,
            border_right: None,
            border_bottom: None,
            border_left: None,
            horizontal: None,
            vertical: None,
            wrap: None,
            number_format: None,
        };
        let format_style = |row, column, number_format| NormalizedStyle {
            row,
            column,
            font_family: None,
            font_size_hundredth_points: None,
            bold: None,
            italic: None,
            underline: None,
            strike: None,
            font_color: None,
            fill: None,
            border_top: None,
            border_right: None,
            border_bottom: None,
            border_left: None,
            horizontal: None,
            vertical: None,
            wrap: None,
            number_format: Some(number_format),
        };
        let source = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![
                NormalizedSheet {
                    name: "Styled".into(),
                    cells: vec![
                        NormalizedCell {
                            row: 1,
                            column: 1,
                            value: NormalizedCellValue::Text("value".into()),
                        },
                        NormalizedCell {
                            row: 3,
                            column: 3,
                            value: NormalizedCellValue::Formula("A1".into()),
                        },
                        NormalizedCell {
                            row: 4,
                            column: 1,
                            value: NormalizedCellValue::Number(1.0),
                        },
                        NormalizedCell {
                            row: 4,
                            column: 2,
                            value: NormalizedCellValue::Number(0.25),
                        },
                        NormalizedCell {
                            row: 4,
                            column: 3,
                            value: NormalizedCellValue::Number(0.25),
                        },
                        NormalizedCell {
                            row: 4,
                            column: 4,
                            value: NormalizedCellValue::Number(1.0),
                        },
                    ],
                    styles: vec![
                        rich.clone(),
                        blank,
                        formula_style,
                        format_style(4, 1, NormalizedNumberFormat::Integer),
                        format_style(4, 2, NormalizedNumberFormat::Percent0),
                        format_style(4, 3, NormalizedNumberFormat::Percent2),
                        format_style(4, 4, NormalizedNumberFormat::Scientific2),
                    ],
                    merges: vec![],
                },
                NormalizedSheet {
                    name: "Other".into(),
                    cells: vec![],
                    styles: vec![rich],
                    merges: vec![],
                },
            ],
        };
        let first = export_scalar_xlsx(&source)?;
        let second = export_scalar_xlsx(&source)?;
        assert_eq!(first, second);
        let decoded = normalize_reader(Cursor::new(&first), first.len() as u64)?;
        assert_eq!(decoded, source);
        assert_eq!(decoded.sheets.len(), 2);
        assert_eq!(decoded.sheets[0].styles.len(), 7);
        assert_eq!(decoded.sheets[0].cells.len(), 6);
        Ok(())
    }

    #[test]
    fn escaped_text_and_xfd_coordinate_roundtrip() -> Result<(), XlsxError> {
        let source = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "R&D 'Plan' Q".to_owned(),
                styles: vec![],
                merges: vec![],
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
    fn merged_cells_roundtrip_through_ooxml() -> Result<(), XlsxError> {
        let source = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Merged".into(),
                cells: vec![NormalizedCell {
                    row: 1,
                    column: 1,
                    value: NormalizedCellValue::Text("header".into()),
                }],
                styles: vec![],
                merges: vec![crate::NormalizedMerge {
                    first_row: 1,
                    last_row: 2,
                    first_column: 1,
                    last_column: 3,
                }],
            }],
        };
        let bytes = export_scalar_xlsx(&source)?;
        let decoded = crate::normalize_reader(Cursor::new(&bytes), bytes.len() as u64)?;
        assert_eq!(decoded, source);
        Ok(())
    }

    #[test]
    fn oversized_entry_fails_before_zip_publication() {
        let source = NormalizedWorkbook {
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Big".to_owned(),
                styles: vec![],
                merges: vec![],
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
            schema_version: 3,
            issues: vec![],
            sheets: vec![NormalizedSheet {
                name: "Bad/Name".to_owned(),
                cells: vec![],
                styles: vec![],
                merges: vec![],
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
