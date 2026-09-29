use super::{
    LoadedPackage, MAX_INSPECTION_TIME, SHARED_STRING_REL, WORKBOOK_NS, XlsxError, attributes,
    resolve_target, resolve_xml_reference,
};
use quick_xml::{Reader, XmlVersion, events::Event};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_CELLS: usize = 100_000;
const MAX_SHARED_STRINGS: usize = 100_000;
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_FORMULA_BYTES: usize = 8 * 1024;
const MAX_IR_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_ROW: u32 = 1_048_576;
const MAX_COLUMN: u32 = 16_384;

/// XLSX-specific intermediate representation. No stable domain IDs are assigned here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedWorkbook {
    pub sheets: Vec<NormalizedSheet>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedSheet {
    pub name: String,
    pub cells: Vec<NormalizedCell>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedCell {
    /// One-based OOXML row index.
    pub row: u32,
    /// One-based OOXML column index.
    pub column: u32,
    pub value: NormalizedCellValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NormalizedCellValue {
    Number(f64),
    Text(String),
    Boolean(bool),
    /// Source without the leading '='; server binding must parse and validate it.
    Formula(String),
}

impl NormalizedWorkbook {
    /// Revalidate child output at the parent boundary before any import step.
    pub fn validate(&self) -> Result<(), XlsxError> {
        if self.sheets.is_empty() || self.sheets.len() > 64 {
            return Err(XlsxError::LimitExceeded);
        }
        let mut names = HashSet::new();
        let mut total_cells = 0_usize;
        let mut total_text_bytes = 0_usize;
        for sheet in &self.sheets {
            if sheet.name.is_empty()
                || sheet.name.chars().count() > 31
                || !names.insert(sheet.name.to_lowercase())
                || !valid_xml_text(&sheet.name)
                || sheet
                    .name
                    .chars()
                    .any(|character| "[]:*?/\\".contains(character))
                || sheet.name.starts_with('\'')
                || sheet.name.ends_with('\'')
            {
                return Err(XlsxError::InvalidPackage);
            }
            let mut previous = None;
            for cell in &sheet.cells {
                let address = (cell.row, cell.column);
                if cell.row == 0
                    || cell.row > MAX_ROW
                    || cell.column == 0
                    || cell.column > MAX_COLUMN
                    || previous.is_some_and(|last| last >= address)
                {
                    return Err(XlsxError::InvalidPackage);
                }
                previous = Some(address);
                total_cells = total_cells.checked_add(1).ok_or(XlsxError::LimitExceeded)?;
                if total_cells > MAX_CELLS {
                    return Err(XlsxError::LimitExceeded);
                }
                match &cell.value {
                    NormalizedCellValue::Number(value)
                        if !value.is_finite() || value.is_sign_negative() && *value == 0.0 =>
                    {
                        return Err(XlsxError::InvalidPackage);
                    }
                    NormalizedCellValue::Text(value) | NormalizedCellValue::Formula(value) => {
                        let formula = matches!(cell.value, NormalizedCellValue::Formula(_));
                        let limit = if formula {
                            MAX_FORMULA_BYTES
                        } else {
                            MAX_TEXT_BYTES
                        };
                        if value.len() > limit {
                            return Err(XlsxError::LimitExceeded);
                        }
                        if formula && value.is_empty() {
                            return Err(XlsxError::InvalidPackage);
                        }
                        if !valid_xml_text(value) || formula && value.starts_with('=') {
                            return Err(XlsxError::InvalidPackage);
                        }
                        total_text_bytes = total_text_bytes
                            .checked_add(value.len())
                            .ok_or(XlsxError::LimitExceeded)?;
                        if total_text_bytes > MAX_IR_TEXT_BYTES {
                            return Err(XlsxError::LimitExceeded);
                        }
                    }
                    NormalizedCellValue::Number(_) | NormalizedCellValue::Boolean(_) => {}
                }
            }
        }
        Ok(())
    }
}

fn valid_xml_text(value: &str) -> bool {
    value.chars().all(|character| {
        matches!(character, '\t' | '\n' | '\r')
            || character >= '\u{20}' && !matches!(character, '\u{fffe}' | '\u{ffff}')
    })
}

#[derive(Default)]
struct CellDraft {
    row: u32,
    column: u32,
    kind: String,
    value: String,
    inline_text: String,
    formula: String,
    saw_value: bool,
    saw_formula: bool,
    saw_inline: bool,
}

#[derive(Clone, Copy)]
enum TextField {
    Value,
    Formula,
    Inline,
}

pub(super) fn extract(package: &LoadedPackage) -> Result<NormalizedWorkbook, XlsxError> {
    let shared = shared_strings(package)?;
    let mut sheets = Vec::with_capacity(package.inspection.sheets.len());
    let mut total_cells = 0_usize;
    let mut total_text_bytes = 0_usize;
    for header in &package.inspection.sheets {
        let body = package
            .parts
            .get(&header.part)
            .ok_or(XlsxError::InvalidPackage)?;
        let cells = parse_sheet(
            body,
            &shared,
            package,
            &mut total_cells,
            &mut total_text_bytes,
        )?;
        sheets.push(NormalizedSheet {
            name: header.name.clone(),
            cells,
        });
    }
    Ok(NormalizedWorkbook { sheets })
}

fn shared_strings(package: &LoadedPackage) -> Result<Vec<String>, XlsxError> {
    let matches = package
        .workbook_rels
        .values()
        .filter(|rel| rel.kind == SHARED_STRING_REL)
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(XlsxError::InvalidPackage);
    }
    let Some(rel) = matches.first() else {
        return Ok(Vec::new());
    };
    let base = package
        .workbook_part
        .rsplit_once('/')
        .map_or("", |(base, _)| base);
    let path = resolve_target(base, &rel.target)?;
    let body = package.parts.get(&path).ok_or(XlsxError::InvalidPackage)?;
    let text = std::str::from_utf8(body).map_err(|_| XlsxError::InvalidPackage)?;
    let mut reader = Reader::from_str(text);
    let mut depth = 0_usize;
    let mut root = false;
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut strings = Vec::new();
    loop {
        check_budget(package)?;
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                depth += 1;
                match (depth, start.name().as_ref()) {
                    (1, "sst") => {
                        let attrs = attributes(&start)?;
                        if attrs.get("xmlns").is_none_or(|ns| ns != WORKBOOK_NS) {
                            return Err(XlsxError::InvalidPackage);
                        }
                        root = true;
                    }
                    (2, "si") if root => current = Some(String::new()),
                    (3, "t") if current.is_some() => in_text = true,
                    _ => return Err(XlsxError::UnsupportedFeature),
                }
            }
            Event::Text(value) if in_text => {
                let current = current.as_mut().ok_or(XlsxError::InvalidPackage)?;
                append_bounded(
                    current,
                    &value.xml_content(XmlVersion::Implicit1_0),
                    MAX_TEXT_BYTES,
                )?;
            }
            Event::GeneralRef(reference) if in_text => {
                let current = current.as_mut().ok_or(XlsxError::InvalidPackage)?;
                append_bounded(current, &resolve_xml_reference(&reference)?, MAX_TEXT_BYTES)?;
            }
            Event::End(end) => {
                match (depth, end.name().as_ref()) {
                    (3, "t") => in_text = false,
                    (2, "si") => {
                        if strings.len() >= MAX_SHARED_STRINGS {
                            return Err(XlsxError::LimitExceeded);
                        }
                        strings.push(current.take().ok_or(XlsxError::InvalidPackage)?);
                    }
                    (1, "sst") => {}
                    _ => return Err(XlsxError::InvalidPackage),
                }
                depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?;
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::Text(_) => {}
            _ => return Err(XlsxError::UnsupportedFeature),
        }
    }
    if depth != 0 || !root || current.is_some() {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(strings)
}

fn parse_sheet(
    body: &[u8],
    shared: &[String],
    package: &LoadedPackage,
    total_cells: &mut usize,
    total_text_bytes: &mut usize,
) -> Result<Vec<NormalizedCell>, XlsxError> {
    let text = std::str::from_utf8(body).map_err(|_| XlsxError::InvalidPackage)?;
    let mut reader = Reader::from_str(text);
    let mut depth = 0_usize;
    let mut root = false;
    let mut data_seen = false;
    let mut in_data = false;
    let mut current_row = None;
    let mut draft: Option<CellDraft> = None;
    let mut field = None;
    let mut cells = Vec::new();
    let mut addresses = HashSet::new();
    loop {
        check_budget(package)?;
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                depth += 1;
                match (depth, start.name().as_ref()) {
                    (1, "worksheet") => {
                        let attrs = attributes(&start)?;
                        if attrs.get("xmlns").is_none_or(|ns| ns != WORKBOOK_NS) {
                            return Err(XlsxError::InvalidPackage);
                        }
                        root = true;
                    }
                    (2, "sheetData") if root && !data_seen => {
                        data_seen = true;
                        in_data = true;
                    }
                    (3, "row") if in_data => {
                        let attrs = attributes(&start)?;
                        let row = attrs
                            .get("r")
                            .ok_or(XlsxError::InvalidPackage)?
                            .parse::<u32>()
                            .map_err(|_| XlsxError::InvalidPackage)?;
                        if row == 0 || row > MAX_ROW {
                            return Err(XlsxError::LimitExceeded);
                        }
                        current_row = Some(row);
                    }
                    (4, "c") if current_row.is_some() => {
                        let attrs = attributes(&start)?;
                        let (row, column) =
                            parse_a1(attrs.get("r").ok_or(XlsxError::InvalidPackage)?)?;
                        if Some(row) != current_row {
                            return Err(XlsxError::InvalidPackage);
                        }
                        if !addresses.insert((row, column)) {
                            return Err(XlsxError::InvalidPackage);
                        }
                        count_cell(total_cells)?;
                        draft = Some(CellDraft {
                            row,
                            column,
                            kind: attrs.get("t").cloned().unwrap_or_default(),
                            ..CellDraft::default()
                        });
                    }
                    (5, "v") if draft.is_some() => {
                        let cell = draft.as_mut().ok_or(XlsxError::InvalidPackage)?;
                        if cell.saw_value {
                            return Err(XlsxError::InvalidPackage);
                        }
                        cell.saw_value = true;
                        field = Some(TextField::Value);
                    }
                    (5, "f") if draft.is_some() => {
                        let attrs = attributes(&start)?;
                        if !attrs.is_empty() {
                            return Err(XlsxError::UnsupportedFeature);
                        }
                        let cell = draft.as_mut().ok_or(XlsxError::InvalidPackage)?;
                        if cell.saw_formula {
                            return Err(XlsxError::InvalidPackage);
                        }
                        cell.saw_formula = true;
                        field = Some(TextField::Formula);
                    }
                    (5, "is") if draft.is_some() => {
                        let cell = draft.as_mut().ok_or(XlsxError::InvalidPackage)?;
                        if cell.saw_inline {
                            return Err(XlsxError::InvalidPackage);
                        }
                        cell.saw_inline = true;
                    }
                    (6, "t") if draft.as_ref().is_some_and(|cell| cell.saw_inline) => {
                        field = Some(TextField::Inline)
                    }
                    _ if draft.is_some() || in_data => return Err(XlsxError::UnsupportedFeature),
                    _ => {}
                }
            }
            Event::Empty(start) => {
                if depth == 3 && start.name().as_ref() == "c" {
                    let attrs = attributes(&start)?;
                    let (row, column) = parse_a1(attrs.get("r").ok_or(XlsxError::InvalidPackage)?)?;
                    if Some(row) != current_row || !addresses.insert((row, column)) {
                        return Err(XlsxError::InvalidPackage);
                    }
                    count_cell(total_cells)?;
                } else if depth == 1 && start.name().as_ref() == "sheetData" {
                    if data_seen {
                        return Err(XlsxError::InvalidPackage);
                    }
                    data_seen = true;
                } else if draft.is_some() || in_data {
                    return Err(XlsxError::UnsupportedFeature);
                }
            }
            Event::Text(value) => {
                if let (Some(cell), Some(field)) = (draft.as_mut(), field) {
                    let value = value.xml_content(XmlVersion::Implicit1_0);
                    match field {
                        TextField::Value => {
                            append_bounded(&mut cell.value, &value, MAX_TEXT_BYTES)?
                        }
                        TextField::Formula => {
                            append_bounded(&mut cell.formula, &value, MAX_FORMULA_BYTES)?
                        }
                        TextField::Inline => {
                            append_bounded(&mut cell.inline_text, &value, MAX_TEXT_BYTES)?
                        }
                    }
                }
            }
            Event::GeneralRef(reference) => {
                let text = resolve_xml_reference(&reference)?;
                if let (Some(cell), Some(field)) = (draft.as_mut(), field) {
                    match field {
                        TextField::Value => append_bounded(&mut cell.value, &text, MAX_TEXT_BYTES)?,
                        TextField::Formula => {
                            append_bounded(&mut cell.formula, &text, MAX_FORMULA_BYTES)?
                        }
                        TextField::Inline => {
                            append_bounded(&mut cell.inline_text, &text, MAX_TEXT_BYTES)?
                        }
                    }
                }
            }
            Event::End(end) => {
                match (depth, end.name().as_ref()) {
                    (6, "t") | (5, "v" | "f") => field = None,
                    (4, "c") => {
                        let cell = draft.take().ok_or(XlsxError::InvalidPackage)?;
                        if let Some(value) = finish_cell(&cell, shared)? {
                            let bytes = match &value {
                                NormalizedCellValue::Text(value)
                                | NormalizedCellValue::Formula(value) => value.len(),
                                NormalizedCellValue::Number(_)
                                | NormalizedCellValue::Boolean(_) => 0,
                            };
                            *total_text_bytes = total_text_bytes
                                .checked_add(bytes)
                                .ok_or(XlsxError::LimitExceeded)?;
                            if *total_text_bytes > MAX_IR_TEXT_BYTES {
                                return Err(XlsxError::LimitExceeded);
                            }
                            cells.push(NormalizedCell {
                                row: cell.row,
                                column: cell.column,
                                value,
                            });
                        }
                    }
                    (3, "row") => current_row = None,
                    (2, "sheetData") => in_data = false,
                    _ => {}
                }
                depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || !root || !data_seen || draft.is_some() || current_row.is_some() {
        return Err(XlsxError::InvalidPackage);
    }
    cells.sort_unstable_by_key(|cell| (cell.row, cell.column));
    Ok(cells)
}

fn count_cell(total_cells: &mut usize) -> Result<(), XlsxError> {
    *total_cells = total_cells.checked_add(1).ok_or(XlsxError::LimitExceeded)?;
    if *total_cells > MAX_CELLS {
        return Err(XlsxError::LimitExceeded);
    }
    Ok(())
}

fn finish_cell(
    cell: &CellDraft,
    shared: &[String],
) -> Result<Option<NormalizedCellValue>, XlsxError> {
    if cell.saw_formula {
        if cell.formula.is_empty()
            || cell.kind == "inlineStr"
            || cell.kind == "s"
            || cell.saw_inline
        {
            return Err(XlsxError::InvalidPackage);
        }
        return Ok(Some(NormalizedCellValue::Formula(cell.formula.clone())));
    }
    let value = match cell.kind.as_str() {
        "" | "n" if cell.saw_value => {
            let number = cell
                .value
                .parse::<f64>()
                .map_err(|_| XlsxError::InvalidPackage)?;
            if !number.is_finite() {
                return Err(XlsxError::InvalidPackage);
            }
            NormalizedCellValue::Number(if number == 0.0 { 0.0 } else { number })
        }
        "s" if cell.saw_value => {
            let index = cell
                .value
                .parse::<usize>()
                .map_err(|_| XlsxError::InvalidPackage)?;
            NormalizedCellValue::Text(shared.get(index).ok_or(XlsxError::InvalidPackage)?.clone())
        }
        "b" if cell.saw_value => match cell.value.as_str() {
            "0" => NormalizedCellValue::Boolean(false),
            "1" => NormalizedCellValue::Boolean(true),
            _ => return Err(XlsxError::InvalidPackage),
        },
        "inlineStr" if cell.saw_inline && !cell.saw_value => {
            NormalizedCellValue::Text(cell.inline_text.clone())
        }
        "" | "n" if !cell.saw_value && !cell.saw_inline => return Ok(None),
        _ => return Err(XlsxError::UnsupportedFeature),
    };
    Ok(Some(value))
}

fn append_bounded(output: &mut String, text: &str, limit: usize) -> Result<(), XlsxError> {
    if output
        .len()
        .checked_add(text.len())
        .is_none_or(|len| len > limit)
    {
        return Err(XlsxError::LimitExceeded);
    }
    output.push_str(text);
    Ok(())
}

fn parse_a1(address: &str) -> Result<(u32, u32), XlsxError> {
    let mut column = 0_u32;
    let mut split = 0;
    for byte in address.bytes() {
        if byte.is_ascii_uppercase() {
            column = column
                .checked_mul(26)
                .and_then(|number| number.checked_add(u32::from(byte - b'A' + 1)))
                .ok_or(XlsxError::LimitExceeded)?;
            split += 1;
        } else {
            break;
        }
    }
    if split == 0 || column == 0 || column > MAX_COLUMN {
        return Err(XlsxError::LimitExceeded);
    }
    let row = address
        .get(split..)
        .ok_or(XlsxError::InvalidPackage)?
        .parse::<u32>()
        .map_err(|_| XlsxError::InvalidPackage)?;
    if row == 0 || row > MAX_ROW {
        return Err(XlsxError::LimitExceeded);
    }
    Ok((row, column))
}

fn check_budget(package: &LoadedPackage) -> Result<(), XlsxError> {
    if package.started.elapsed() > MAX_INSPECTION_TIME {
        Err(XlsxError::LimitExceeded)
    } else {
        Ok(())
    }
}
