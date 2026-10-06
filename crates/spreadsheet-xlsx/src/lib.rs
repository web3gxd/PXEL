//! Bounded inspection and normalized scalar extraction from an OOXML package.
//! Parsed data is never published directly into the domain projection.

mod export;
mod normalized;
mod styles;
#[cfg(feature = "worker-protocol")]
pub mod worker_protocol;

pub use export::export_scalar_xlsx;
pub use normalized::{
    CompatibilityIssue, CompatibilityIssueCode, NormalizedBorder, NormalizedBorderLine,
    NormalizedCell, NormalizedCellValue, NormalizedHorizontal, NormalizedMerge,
    NormalizedNumberFormat, NormalizedSheet, NormalizedStyle, NormalizedUnderline,
    NormalizedVertical, NormalizedWorkbook,
};

use quick_xml::{Reader, XmlVersion, events::Event};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};
use thiserror::Error;
use zip::{CompressionMethod, ZipArchive};

const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_XML_DEPTH: usize = 64;
const MAX_XML_ATTRIBUTES: usize = 128;
const MAX_SHEETS: usize = 64;
const MAX_RELATIONSHIPS: usize = 1024;
const MAX_INSPECTION_TIME: Duration = Duration::from_secs(5);
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const OFFICE_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
const SHEET_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
const SHARED_STRING_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";
const STYLES_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
const WORKBOOK_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const OFFICE_REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

#[derive(Debug, Error, Eq, PartialEq)]
pub enum XlsxError {
    #[error("invalid or unsupported XLSX package")]
    InvalidPackage,
    #[error("XLSX package exceeds inspection limits")]
    LimitExceeded,
    #[error("XLSX package contains unsupported active content")]
    UnsupportedActiveContent,
    #[error("XLSX package has an unsupported XML or ZIP feature")]
    UnsupportedFeature,
    #[error("XLSX package I/O failed")]
    Io,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportedSheetHeader {
    pub name: String,
    pub sheet_id: u32,
    pub part: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageInspection {
    pub sheets: Vec<ImportedSheetHeader>,
    pub entry_count: usize,
    pub uncompressed_bytes: u64,
}

#[derive(Clone)]
struct Relationship {
    kind: String,
    target: String,
}

struct LoadedPackage {
    inspection: PackageInspection,
    parts: HashMap<String, Vec<u8>>,
    started: Instant,
    workbook_part: String,
    workbook_rels: HashMap<String, Relationship>,
    styles_part: Option<String>,
}

pub fn inspect_file(path: &Path) -> Result<PackageInspection, XlsxError> {
    let file = File::open(path).map_err(|_| XlsxError::Io)?;
    let len = file.metadata().map_err(|_| XlsxError::Io)?.len();
    inspect_reader(file, len)
}

pub fn normalize_file(path: &Path) -> Result<NormalizedWorkbook, XlsxError> {
    let file = File::open(path).map_err(|_| XlsxError::Io)?;
    let len = file.metadata().map_err(|_| XlsxError::Io)?.len();
    normalize_reader(file, len)
}

pub fn inspect_reader<R: Read + Seek>(
    input: R,
    source_len: u64,
) -> Result<PackageInspection, XlsxError> {
    Ok(load_package(input, source_len)?.inspection)
}

/// Extracts a bounded, non-domain workbook. It does not publish an import.
pub fn normalize_reader<R: Read + Seek>(
    input: R,
    source_len: u64,
) -> Result<NormalizedWorkbook, XlsxError> {
    let package = load_package(input, source_len)?;
    normalized::extract(&package)
}

fn load_package<R: Read + Seek>(input: R, source_len: u64) -> Result<LoadedPackage, XlsxError> {
    let mut input = input;
    let actual_len = input.seek(SeekFrom::End(0)).map_err(|_| XlsxError::Io)?;
    if actual_len != source_len || actual_len == 0 || actual_len > MAX_ARCHIVE_BYTES {
        return Err(XlsxError::LimitExceeded);
    }
    input.seek(SeekFrom::Start(0)).map_err(|_| XlsxError::Io)?;
    let started = Instant::now();
    let mut archive = ZipArchive::new(input).map_err(|_| XlsxError::InvalidPackage)?;
    if archive.is_empty() || archive.len() > MAX_ENTRIES {
        return Err(XlsxError::LimitExceeded);
    }
    let entry_count = archive.len();
    let mut parts = HashMap::<String, Vec<u8>>::with_capacity(entry_count);
    let mut seen = HashSet::<String>::with_capacity(entry_count);
    let mut total = 0_u64;
    for index in 0..entry_count {
        if started.elapsed() > MAX_INSPECTION_TIME {
            return Err(XlsxError::LimitExceeded);
        }
        let mut file = archive
            .by_index(index)
            .map_err(|_| XlsxError::InvalidPackage)?;
        let raw = std::str::from_utf8(file.name_raw())
            .map_err(|_| XlsxError::InvalidPackage)?
            .to_owned();
        let name = if file.is_dir() {
            raw.strip_suffix('/').ok_or(XlsxError::InvalidPackage)?
        } else {
            &raw
        };
        validate_part_name(name)?;
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(XlsxError::InvalidPackage);
        }
        if contains_active_part(name) {
            return Err(XlsxError::UnsupportedActiveContent);
        }
        if file.encrypted()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(XlsxError::UnsupportedFeature);
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(XlsxError::InvalidPackage);
        }
        if file.size() > MAX_ENTRY_BYTES
            || (file.compressed_size() == 0 && file.size() != 0)
            || file.size() > file.compressed_size().saturating_mul(100)
        {
            return Err(XlsxError::LimitExceeded);
        }
        let mut body = Vec::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if started.elapsed() > MAX_INSPECTION_TIME {
                return Err(XlsxError::LimitExceeded);
            }
            let count = file
                .read(&mut buffer)
                .map_err(|_| XlsxError::InvalidPackage)?;
            if count == 0 {
                break;
            }
            let size = body
                .len()
                .checked_add(count)
                .ok_or(XlsxError::LimitExceeded)?;
            if size as u64 > MAX_ENTRY_BYTES {
                return Err(XlsxError::LimitExceeded);
            }
            body.extend_from_slice(&buffer[..count]);
        }
        if body.len() as u64 != file.size() {
            return Err(XlsxError::InvalidPackage);
        }
        total = total
            .checked_add(body.len() as u64)
            .ok_or(XlsxError::LimitExceeded)?;
        if total > MAX_TOTAL_BYTES {
            return Err(XlsxError::LimitExceeded);
        }
        if !file.is_dir() {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                validate_xml(&body)?;
            }
            parts.insert(name.to_owned(), body);
        }
    }
    let content_types = parts
        .get("[Content_Types].xml")
        .ok_or(XlsxError::InvalidPackage)?;
    if std::str::from_utf8(content_types)
        .map_err(|_| XlsxError::InvalidPackage)?
        .to_ascii_lowercase()
        .contains("macroenabled")
    {
        return Err(XlsxError::UnsupportedActiveContent);
    }
    for (name, body) in &parts {
        if name.ends_with(".rels") {
            relationships(body)?;
        }
    }
    let root_rels = relationships(parts.get("_rels/.rels").ok_or(XlsxError::InvalidPackage)?)?;
    let workbook_target = root_rels
        .values()
        .filter(|rel| rel.kind == OFFICE_REL)
        .collect::<Vec<_>>();
    if workbook_target.len() != 1 {
        return Err(XlsxError::InvalidPackage);
    }
    let workbook_part = resolve_target("", &workbook_target[0].target)?;
    let workbook = parts.get(&workbook_part).ok_or(XlsxError::InvalidPackage)?;
    let workbook_rels_part = relationship_part(&workbook_part)?;
    let workbook_rels = relationships(
        parts
            .get(&workbook_rels_part)
            .ok_or(XlsxError::InvalidPackage)?,
    )?;
    let style_relationships = workbook_rels
        .values()
        .filter(|rel| rel.kind == STYLES_REL)
        .collect::<Vec<_>>();
    if style_relationships.len() > 1 {
        return Err(XlsxError::InvalidPackage);
    }
    let workbook_base = workbook_part.rsplit_once('/').map_or("", |(base, _)| base);
    let styles_part = style_relationships
        .first()
        .map(|rel| resolve_target(workbook_base, &rel.target))
        .transpose()?;
    if styles_part
        .as_ref()
        .is_some_and(|part| !parts.contains_key(part))
    {
        return Err(XlsxError::InvalidPackage);
    }
    let sheets = parse_sheets(workbook, &workbook_part, &workbook_rels, &parts)?;
    Ok(LoadedPackage {
        inspection: PackageInspection {
            sheets,
            entry_count,
            uncompressed_bytes: total,
        },
        parts,
        started,
        workbook_part,
        workbook_rels,
        styles_part,
    })
}

fn validate_part_name(name: &str) -> Result<(), XlsxError> {
    if name.is_empty()
        || name.len() > 255
        || !name.is_ascii()
        || name.contains(['\\', ':', '%', '?', '#', '\0'])
        || name.starts_with('/')
        || name.ends_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(())
}

fn contains_active_part(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("vbaproject")
        || lower.starts_with("xl/externallinks/")
        || lower.starts_with("xl/embeddings/")
        || lower.starts_with("xl/pivottables/")
        || lower == "xl/connections.xml"
        || lower.starts_with("customxml/")
}

fn validate_xml(bytes: &[u8]) -> Result<(), XlsxError> {
    let text = std::str::from_utf8(bytes).map_err(|_| XlsxError::UnsupportedFeature)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut depth = 0_usize;
    let mut roots = 0_usize;
    loop {
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                if depth == 0 {
                    roots += 1;
                    if roots > 1 {
                        return Err(XlsxError::InvalidPackage);
                    }
                }
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(XlsxError::LimitExceeded);
                }
                validate_attributes(&start)?;
            }
            Event::Empty(start) => {
                if depth == 0 {
                    roots += 1;
                    if roots > 1 {
                        return Err(XlsxError::InvalidPackage);
                    }
                }
                validate_attributes(&start)?;
            }
            Event::End(_) => depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?,
            Event::DocType(_) => {
                return Err(XlsxError::UnsupportedActiveContent);
            }
            Event::GeneralRef(reference) => {
                resolve_xml_reference(&reference)?;
            }
            Event::Text(text) if text.len() > 1024 * 1024 => return Err(XlsxError::LimitExceeded),
            Event::Text(text) if depth == 0 && !text.as_ref().trim().is_empty() => {
                return Err(XlsxError::InvalidPackage);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || roots != 1 {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(())
}

fn resolve_xml_reference(reference: &quick_xml::events::BytesRef<'_>) -> Result<String, XlsxError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| XlsxError::InvalidPackage)?
    {
        if character.is_control() && !matches!(character, '\t' | '\n' | '\r') {
            return Err(XlsxError::InvalidPackage);
        }
        return Ok(character.to_string());
    }
    quick_xml::escape::resolve_predefined_entity(reference.as_ref())
        .map(str::to_owned)
        .ok_or(XlsxError::UnsupportedActiveContent)
}

fn validate_attributes(start: &quick_xml::events::BytesStart<'_>) -> Result<(), XlsxError> {
    let mut count = 0_usize;
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|_| XlsxError::InvalidPackage)?;
        count += 1;
        if count > MAX_XML_ATTRIBUTES || attribute.value.len() > 16 * 1024 {
            return Err(XlsxError::LimitExceeded);
        }
        attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| XlsxError::InvalidPackage)?;
    }
    Ok(())
}

fn attributes(
    start: &quick_xml::events::BytesStart<'_>,
) -> Result<HashMap<String, String>, XlsxError> {
    let mut values = HashMap::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|_| XlsxError::InvalidPackage)?;
        let key = attribute.key.as_ref();
        let value = attribute
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|_| XlsxError::InvalidPackage)?;
        if values.insert(key.to_owned(), value.into_owned()).is_some() {
            return Err(XlsxError::InvalidPackage);
        }
    }
    Ok(values)
}

fn relationships(bytes: &[u8]) -> Result<HashMap<String, Relationship>, XlsxError> {
    let text = std::str::from_utf8(bytes).map_err(|_| XlsxError::InvalidPackage)?;
    let mut reader = Reader::from_str(text);
    let mut depth = 0_usize;
    let mut rels = HashMap::new();
    let mut root_seen = false;
    loop {
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                depth += 1;
                if depth == 1 {
                    let attrs = attributes(&start)?;
                    if start.name().as_ref() != "Relationships"
                        || attrs.get("xmlns").is_none_or(|ns| ns != REL_NS)
                    {
                        return Err(XlsxError::InvalidPackage);
                    }
                    root_seen = true;
                } else if depth == 2 && start.name().as_ref() == "Relationship" {
                    insert_relationship(&mut rels, &start)?;
                } else {
                    return Err(XlsxError::InvalidPackage);
                }
            }
            Event::Empty(start) if depth == 1 && start.name().as_ref() == "Relationship" => {
                insert_relationship(&mut rels, &start)?;
            }
            Event::Empty(_) => return Err(XlsxError::InvalidPackage),
            Event::End(_) => depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?,
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || !root_seen {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(rels)
}

fn insert_relationship(
    rels: &mut HashMap<String, Relationship>,
    start: &quick_xml::events::BytesStart<'_>,
) -> Result<(), XlsxError> {
    if rels.len() >= MAX_RELATIONSHIPS {
        return Err(XlsxError::LimitExceeded);
    }
    let attrs = attributes(start)?;
    if attrs
        .get("TargetMode")
        .is_some_and(|mode| mode.eq_ignore_ascii_case("External"))
    {
        return Err(XlsxError::UnsupportedActiveContent);
    }
    let id = attrs.get("Id").ok_or(XlsxError::InvalidPackage)?;
    let kind = attrs.get("Type").ok_or(XlsxError::InvalidPackage)?;
    let target = attrs.get("Target").ok_or(XlsxError::InvalidPackage)?;
    if id.is_empty() || kind.is_empty() || target.is_empty() || attrs.contains_key("TargetMode") {
        return Err(XlsxError::InvalidPackage);
    }
    if rels
        .insert(
            id.clone(),
            Relationship {
                kind: kind.clone(),
                target: target.clone(),
            },
        )
        .is_some()
    {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(())
}

fn resolve_target(base: &str, target: &str) -> Result<String, XlsxError> {
    if target.starts_with('/')
        || target.contains(['\\', ':', '%', '?', '#', '\0'])
        || !target.is_ascii()
    {
        return Err(XlsxError::InvalidPackage);
    }
    let mut path = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').map(str::to_owned).collect::<Vec<_>>()
    };
    for component in target.split('/') {
        match component {
            "" | "." => return Err(XlsxError::InvalidPackage),
            ".." => {
                path.pop().ok_or(XlsxError::InvalidPackage)?;
            }
            segment => path.push(segment.to_owned()),
        }
    }
    let result = path.join("/");
    validate_part_name(&result)?;
    Ok(result)
}

fn relationship_part(part: &str) -> Result<String, XlsxError> {
    let (directory, file) = part.rsplit_once('/').unwrap_or(("", part));
    let name = if directory.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{directory}/_rels/{file}.rels")
    };
    validate_part_name(&name)?;
    Ok(name)
}

fn parse_sheets(
    workbook: &[u8],
    workbook_part: &str,
    rels: &HashMap<String, Relationship>,
    parts: &HashMap<String, Vec<u8>>,
) -> Result<Vec<ImportedSheetHeader>, XlsxError> {
    let text = std::str::from_utf8(workbook).map_err(|_| XlsxError::InvalidPackage)?;
    let mut reader = Reader::from_str(text);
    let mut depth = 0_usize;
    let mut sheets = Vec::new();
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let mut root_seen = false;
    let mut sheets_seen = false;
    let mut in_sheets = false;
    let base = workbook_part
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    loop {
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                depth += 1;
                if depth == 1 {
                    let attrs = attributes(&start)?;
                    if start.name().as_ref() != "workbook"
                        || attrs.get("xmlns").is_none_or(|ns| ns != WORKBOOK_NS)
                        || attrs.get("xmlns:r").is_none_or(|ns| ns != OFFICE_REL_NS)
                    {
                        return Err(XlsxError::InvalidPackage);
                    }
                    root_seen = true;
                } else if depth == 2 && start.name().as_ref() == "sheets" {
                    sheets_seen = true;
                    in_sheets = true;
                } else if depth == 3 && start.name().as_ref() == "sheet" {
                    return Err(XlsxError::InvalidPackage);
                }
            }
            Event::Empty(start) if depth == 2 && start.name().as_ref() == "sheet" => {
                if !in_sheets {
                    return Err(XlsxError::InvalidPackage);
                }
                add_sheet(&start, base, rels, parts, &mut sheets, &mut names, &mut ids)?;
            }
            Event::End(end) => {
                if depth == 2 && end.name().as_ref() == "sheets" {
                    in_sheets = false;
                }
                depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 || !root_seen || !sheets_seen || sheets.is_empty() {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(sheets)
}

fn add_sheet(
    start: &quick_xml::events::BytesStart<'_>,
    base: &str,
    rels: &HashMap<String, Relationship>,
    parts: &HashMap<String, Vec<u8>>,
    sheets: &mut Vec<ImportedSheetHeader>,
    names: &mut HashSet<String>,
    ids: &mut HashSet<u32>,
) -> Result<(), XlsxError> {
    if sheets.len() >= MAX_SHEETS {
        return Err(XlsxError::LimitExceeded);
    }
    let attrs = attributes(start)?;
    let name = attrs.get("name").ok_or(XlsxError::InvalidPackage)?;
    let id: u32 = attrs
        .get("sheetId")
        .ok_or(XlsxError::InvalidPackage)?
        .parse()
        .map_err(|_| XlsxError::InvalidPackage)?;
    let rel_id = attrs.get("r:id").ok_or(XlsxError::InvalidPackage)?;
    if name.is_empty()
        || name.chars().count() > 31
        || id == 0
        || !names.insert(name.to_lowercase())
        || !ids.insert(id)
    {
        return Err(XlsxError::InvalidPackage);
    }
    let rel = rels.get(rel_id).ok_or(XlsxError::InvalidPackage)?;
    if rel.kind != SHEET_REL {
        return Err(XlsxError::InvalidPackage);
    }
    let part = resolve_target(base, &rel.target)?;
    if !parts.contains_key(&part) {
        return Err(XlsxError::InvalidPackage);
    }
    sheets.push(ImportedSheetHeader {
        name: name.clone(),
        sheet_id: id,
        part,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        CompatibilityIssue, CompatibilityIssueCode, NormalizedBorderLine, NormalizedCellValue,
        NormalizedHorizontal, NormalizedMerge, NormalizedNumberFormat, NormalizedVertical,
        XlsxError, inspect_reader, normalize_reader,
    };
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};

    const CONTENT_TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>"#;
    const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r0" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    const WORKBOOK: &str = r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Budget" sheetId="1" r:id="r1"/><sheet name="Plan" sheetId="2" r:id="r2"/></sheets></workbook>"#;
    const WORKBOOK_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="r2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#;
    const SHEET: &str = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#;

    fn package(overrides: &[(&str, &str)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut files = vec![
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", WORKBOOK),
            ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
            ("xl/worksheets/sheet1.xml", SHEET),
            ("xl/worksheets/sheet2.xml", SHEET),
        ];
        for (name, body) in overrides {
            if let Some(entry) = files.iter_mut().find(|entry| entry.0 == *name) {
                entry.1 = body;
            } else {
                files.push((name, body));
            }
        }
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in files {
            writer.start_file(name, options)?;
            writer.write_all(body.as_bytes())?;
        }
        Ok(writer.finish()?.into_inner())
    }

    fn styled_package(sheet: &str, styles: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let rels = WORKBOOK_RELS.replace("</Relationships>", "<Relationship Id=\"styles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>");
        package(&[
            ("xl/_rels/workbook.xml.rels", &rels),
            ("xl/styles.xml", styles),
            ("xl/worksheets/sheet1.xml", sheet),
        ])
    }

    fn inspect(bytes: Vec<u8>) -> Result<super::PackageInspection, XlsxError> {
        let len = bytes.len() as u64;
        inspect_reader(Cursor::new(bytes), len)
    }

    fn normalize(bytes: Vec<u8>) -> Result<super::NormalizedWorkbook, XlsxError> {
        let len = bytes.len() as u64;
        normalize_reader(Cursor::new(bytes), len)
    }

    #[test]
    fn committed_xlsx_fixture_has_five_scalar_cells() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = include_bytes!("../../../fixtures/xlsx/minimal_scalars.xlsx");
        let workbook = normalize(bytes.to_vec())?;
        assert_eq!(workbook.sheets.len(), 2);
        assert!(workbook.issues.is_empty());
        assert_eq!(
            workbook
                .sheets
                .iter()
                .map(|sheet| sheet.cells.len())
                .sum::<usize>(),
            5
        );
        assert_eq!(
            workbook.sheets[0].cells[3].value,
            NormalizedCellValue::Formula("A1+B1".into())
        );
        Ok(())
    }

    #[test]
    fn resolves_two_sheets_without_importing_cells() -> Result<(), Box<dyn std::error::Error>> {
        let result = inspect(package(&[])?)?;
        assert_eq!(result.sheets.len(), 2);
        assert_eq!(result.sheets[0].name, "Budget");
        assert_eq!(result.sheets[0].part, "xl/worksheets/sheet1.xml");
        assert_eq!(result.sheets[1].name, "Plan");
        assert_eq!(result.entry_count, 6);
        Ok(())
    }

    #[test]
    fn rejects_path_traversal_and_external_relationship() -> Result<(), Box<dyn std::error::Error>>
    {
        let traversal = inspect(package(&[("../outside.xml", SHEET)])?);
        assert_eq!(traversal, Err(XlsxError::InvalidPackage));
        let external = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="x" Type="custom" Target="https://example.invalid/out" TargetMode="External"/></Relationships>"#;
        let result = inspect(package(&[(
            "xl/worksheets/_rels/sheet1.xml.rels",
            external,
        )])?);
        assert_eq!(result, Err(XlsxError::UnsupportedActiveContent));
        Ok(())
    }

    #[test]
    fn rejects_dtd_and_duplicate_sheet_ids() -> Result<(), Box<dyn std::error::Error>> {
        let result = inspect(package(&[(
            "xl/worksheets/sheet1.xml",
            "<!DOCTYPE worksheet><worksheet/>",
        )])?);
        assert_eq!(result, Err(XlsxError::UnsupportedActiveContent));
        let duplicate = WORKBOOK.replace("sheetId=\"2\"", "sheetId=\"1\"");
        let result = inspect(package(&[("xl/workbook.xml", &duplicate)])?);
        assert_eq!(result, Err(XlsxError::InvalidPackage));
        Ok(())
    }

    #[test]
    fn rejects_hostile_xml_depth_and_attribute_counts() -> Result<(), Box<dyn std::error::Error>> {
        let nested = format!("{}{}", "<n>".repeat(65), "</n>".repeat(65));
        assert_eq!(
            inspect(package(&[("xl/worksheets/sheet1.xml", &nested)])?),
            Err(XlsxError::LimitExceeded)
        );

        let attributes = (0..129)
            .map(|index| format!(" a{index}=\"x\""))
            .collect::<String>();
        let xml = format!("<properties{attributes}/>");
        assert_eq!(
            inspect(package(&[("docProps/core.xml", &xml)])?),
            Err(XlsxError::LimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn rejects_case_insensitive_duplicate_parts_and_entry_overflow()
    -> Result<(), Box<dyn std::error::Error>> {
        let duplicate = package(&[("[content_types].xml", CONTENT_TYPES)])?;
        assert_eq!(inspect(duplicate), Err(XlsxError::InvalidPackage));

        let mut extras = Vec::with_capacity(251);
        for index in 0..251 {
            let name = format!("extra/{index}.bin");
            extras.push((name, "".to_owned()));
        }
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in [
            ("[Content_Types].xml".to_owned(), CONTENT_TYPES.to_owned()),
            ("_rels/.rels".to_owned(), ROOT_RELS.to_owned()),
            ("xl/workbook.xml".to_owned(), WORKBOOK.to_owned()),
            (
                "xl/_rels/workbook.xml.rels".to_owned(),
                WORKBOOK_RELS.to_owned(),
            ),
            ("xl/worksheets/sheet1.xml".to_owned(), SHEET.to_owned()),
            ("xl/worksheets/sheet2.xml".to_owned(), SHEET.to_owned()),
        ]
        .into_iter()
        .chain(extras)
        {
            writer.start_file(name, options)?;
            writer.write_all(body.as_bytes())?;
        }
        assert_eq!(
            inspect(writer.finish()?.into_inner()),
            Err(XlsxError::LimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn rejects_high_ratio_deflate_entries() -> Result<(), Box<dyn std::error::Error>> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let stored =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", WORKBOOK),
            ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
            ("xl/worksheets/sheet1.xml", SHEET),
            ("xl/worksheets/sheet2.xml", SHEET),
        ] {
            writer.start_file(name, stored)?;
            writer.write_all(body.as_bytes())?;
        }
        let deflated =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("extra/repetitive.bin", deflated)?;
        writer.write_all("a".repeat(256 * 1024).as_bytes())?;
        assert_eq!(
            inspect(writer.finish()?.into_inner()),
            Err(XlsxError::LimitExceeded)
        );
        Ok(())
    }

    #[test]
    fn rejects_zip_symlink_entries() -> Result<(), Box<dyn std::error::Error>> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", WORKBOOK),
            ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
            ("xl/worksheets/sheet1.xml", SHEET),
            ("xl/worksheets/sheet2.xml", SHEET),
        ] {
            writer.start_file(name, options)?;
            writer.write_all(body.as_bytes())?;
        }
        writer.add_symlink("extra/link", "../../outside", options)?;
        assert_eq!(
            inspect(writer.finish()?.into_inner()),
            Err(XlsxError::InvalidPackage)
        );
        Ok(())
    }

    #[test]
    fn extracts_scalar_cells_and_formula_source_without_cached_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let rels = WORKBOOK_RELS.replace("</Relationships>", "<Relationship Id=\"r3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/></Relationships>");
        let strings = r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>hello &amp; world</t></si></sst>"#;
        let sheet1 = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>-0</v></c><c r="C1" t="b"><v>1</v></c><c r="D1"><f>A1+B1</f><v>999</v></c></row></sheetData></worksheet>"#;
        let sheet2 = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="4"><c r="Z4" t="inlineStr"><is><t>world&#33;</t></is></c></row></sheetData></worksheet>"#;
        let workbook = normalize(package(&[
            ("xl/_rels/workbook.xml.rels", &rels),
            ("xl/sharedStrings.xml", strings),
            ("xl/worksheets/sheet1.xml", sheet1),
            ("xl/worksheets/sheet2.xml", sheet2),
        ])?)?;
        assert_eq!(workbook.sheets.len(), 2);
        assert_eq!(workbook.sheets[0].cells.len(), 4);
        assert_eq!(
            workbook.sheets[0].cells[0].value,
            NormalizedCellValue::Text("hello & world".into())
        );
        assert_eq!(
            workbook.sheets[0].cells[1].value,
            NormalizedCellValue::Number(0.0)
        );
        assert_eq!(
            workbook.sheets[0].cells[2].value,
            NormalizedCellValue::Boolean(true)
        );
        assert_eq!(
            workbook.sheets[0].cells[3].value,
            NormalizedCellValue::Formula("A1+B1".into())
        );
        assert_eq!(workbook.sheets[1].cells[0].column, 26);
        assert_eq!(workbook.sheets[1].cells[0].row, 4);
        assert_eq!(
            workbook.sheets[1].cells[0].value,
            NormalizedCellValue::Text("world!".into())
        );
        Ok(())
    }

    #[test]
    fn parses_bounded_ooxml_merge_ranges_and_rejects_duplicates()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/><mergeCells count="1"><mergeCell ref="A1:C2"/></mergeCells></worksheet>"#;
        let workbook = normalize(package(&[("xl/worksheets/sheet1.xml", valid)])?)?;
        assert_eq!(
            workbook.sheets[0].merges,
            vec![NormalizedMerge {
                first_row: 1,
                last_row: 2,
                first_column: 1,
                last_column: 3,
            }]
        );
        let duplicate = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/><mergeCells count="2"><mergeCell ref="A1:B2"/><mergeCell ref="A1:B2"/></mergeCells></worksheet>"#;
        assert_eq!(
            normalize(package(&[("xl/worksheets/sheet1.xml", duplicate)])?),
            Err(XlsxError::InvalidPackage)
        );
        Ok(())
    }

    #[test]
    fn invalid_shared_index_duplicate_address_and_unsupported_rich_text_fail_atomically()
    -> Result<(), Box<dyn std::error::Error>> {
        let missing_shared = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            normalize(package(&[("xl/worksheets/sheet1.xml", missing_shared)])?),
            Err(XlsxError::InvalidPackage)
        );
        let duplicate = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>1</v></c><c r="A1"><v>2</v></c></row></sheetData></worksheet>"#;
        assert_eq!(
            normalize(package(&[("xl/worksheets/sheet1.xml", duplicate)])?),
            Err(XlsxError::InvalidPackage)
        );
        let rich = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><r><t>styled</t></r></is></c></row></sheetData></worksheet>"#;
        assert_eq!(
            normalize(package(&[("xl/worksheets/sheet1.xml", rich)])?),
            Err(XlsxError::UnsupportedFeature)
        );
        let unknown_entity = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>&unknown;</t></is></c></row></sheetData></worksheet>"#;
        assert_eq!(
            normalize(package(&[("xl/worksheets/sheet1.xml", unknown_entity)])?),
            Err(XlsxError::UnsupportedActiveContent)
        );
        Ok(())
    }

    #[test]
    fn repeated_shared_string_cannot_expand_ir_without_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let rels = WORKBOOK_RELS.replace("</Relationships>", "<Relationship Id=\"r3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings\" Target=\"sharedStrings.xml\"/></Relationships>");
        let strings = format!(
            "<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><si><t>{}</t></si></sst>",
            "x".repeat(1024 * 1024)
        );
        let rows = (1..=33)
            .map(|row| format!("<row r=\"{row}\"><c r=\"A{row}\" t=\"s\"><v>0</v></c></row>"))
            .collect::<String>();
        let sheet = format!(
            "<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>{rows}</sheetData></worksheet>"
        );
        let result = normalize(package(&[
            ("xl/_rels/workbook.xml.rels", &rels),
            ("xl/sharedStrings.xml", &strings),
            ("xl/worksheets/sheet1.xml", &sheet),
        ])?);
        assert_eq!(result, Err(XlsxError::LimitExceeded));
        Ok(())
    }

    #[test]
    fn reports_metadata_and_formatting_that_scalar_import_would_drop()
    -> Result<(), Box<dyn std::error::Error>> {
        let workbook = WORKBOOK.replace("</workbook>", "<definedNames/></workbook>");
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1" ht="30"><c r="A1" s="1"><v>3</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#;
        let rels = WORKBOOK_RELS.replace("</Relationships>", "<Relationship Id=\"styles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>");
        let styles = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font><name val="Arial"/><sz val="10"/></font><font><b/><color theme="4"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/></border></borders><cellXfs count="2"><xf fontId="0" fillId="0" borderId="0" xfId="0"/><xf fontId="1" fillId="0" borderId="0" xfId="0"/></cellXfs></styleSheet>"#;
        let result = normalize(package(&[
            ("xl/workbook.xml", &workbook),
            ("xl/_rels/workbook.xml.rels", &rels),
            ("xl/styles.xml", styles),
            ("xl/worksheets/sheet1.xml", sheet),
            ("docProps/core.xml", "<properties/>"),
        ])?)?;
        assert_eq!(
            result.sheets[0].cells[0].value,
            NormalizedCellValue::Number(3.0)
        );
        assert_eq!(result.sheets[0].styles[0].bold, Some(true));
        assert_eq!(
            result.issues,
            vec![
                CompatibilityIssue {
                    code: CompatibilityIssueCode::UnsupportedPackagePart,
                    sheet_index: None
                },
                CompatibilityIssue {
                    code: CompatibilityIssueCode::WorkbookMetadata,
                    sheet_index: None
                },
                CompatibilityIssue {
                    code: CompatibilityIssueCode::RowFormatting,
                    sheet_index: Some(0)
                },
                CompatibilityIssue {
                    code: CompatibilityIssueCode::CellFormatting,
                    sheet_index: Some(0)
                },
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
        );
        assert!(result.validate().is_ok());
        Ok(())
    }

    #[test]
    fn parses_supported_style_properties_and_a_styled_blank_cell()
    -> Result<(), Box<dyn std::error::Error>> {
        let styles = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts><fonts count="2"><font><name val="Arial"/><sz val="10"/></font><font><name val="Calibri"/><sz val="12.5"/><b/><i/><u val="double"/><strike/><color rgb="FF112233"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFABCDEF"/></patternFill></fill></fills><borders count="2"><border><left/><right/><top/><bottom/></border><border><left style="thin"><color rgb="FF010203"/></left><right style="medium"><color rgb="FF040506"/></right><top style="dashed"><color rgb="FF070809"/></top><bottom style="double"><color rgb="FF0A0B0C"/></bottom></border></borders><cellXfs count="4"><xf fontId="0" fillId="0" borderId="0" xfId="0"/><xf fontId="1" fillId="1" borderId="0" xfId="0"/><xf fontId="0" fillId="0" borderId="0" numFmtId="164" xfId="0"><alignment horizontal="center" vertical="top" wrapText="1"/></xf><xf fontId="1" fillId="0" borderId="1" numFmtId="2" xfId="0"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1" t="inlineStr"><is><t>styled</t></is></c></row><row r="2"><c r="B2" s="2"/><c r="C2" s="3"><v>1</v></c></row></sheetData></worksheet>"#;
        let workbook = normalize(styled_package(sheet, styles)?)?;
        assert!(workbook.issues.is_empty());
        assert_eq!(workbook.sheets[0].cells.len(), 2);
        assert_eq!(workbook.sheets[0].styles.len(), 3);
        assert_eq!(workbook.sheets[0].styles[0].bold, Some(true));
        assert_eq!(
            workbook.sheets[0].styles[0].font_color,
            Some([0x11, 0x22, 0x33])
        );
        assert_eq!(workbook.sheets[0].styles[0].fill, Some([0xAB, 0xCD, 0xEF]));
        assert_eq!(
            workbook.sheets[0].styles[0].font_family.as_deref(),
            Some("Calibri")
        );
        assert_eq!(
            workbook.sheets[0].styles[0].font_size_hundredth_points,
            Some(1250)
        );
        assert_eq!(workbook.sheets[0].styles[0].italic, Some(true));
        assert_eq!(
            workbook.sheets[0].styles[0].underline,
            Some(super::NormalizedUnderline::Double)
        );
        assert_eq!(workbook.sheets[0].styles[0].strike, Some(true));
        assert_eq!(
            workbook.sheets[0].styles[1].horizontal,
            Some(NormalizedHorizontal::Center)
        );
        assert_eq!(
            workbook.sheets[0].styles[1].vertical,
            Some(NormalizedVertical::Top)
        );
        assert_eq!(workbook.sheets[0].styles[1].wrap, Some(true));
        assert_eq!(
            workbook.sheets[0].styles[1].number_format,
            Some(NormalizedNumberFormat::DateIso)
        );
        assert_eq!(
            (
                workbook.sheets[0].styles[1].row,
                workbook.sheets[0].styles[1].column
            ),
            (2, 2)
        );
        assert_eq!(
            workbook.sheets[0].styles[2]
                .border_left
                .ok_or("left border missing")?
                .line,
            NormalizedBorderLine::Thin
        );
        assert_eq!(
            workbook.sheets[0].styles[2]
                .border_right
                .ok_or("right border missing")?
                .line,
            NormalizedBorderLine::Medium
        );
        assert_eq!(
            workbook.sheets[0].styles[2]
                .border_top
                .ok_or("top border missing")?
                .line,
            NormalizedBorderLine::Dashed
        );
        assert_eq!(
            workbook.sheets[0].styles[2]
                .border_bottom
                .ok_or("bottom border missing")?
                .line,
            NormalizedBorderLine::Double
        );
        assert_eq!(
            workbook.sheets[0].styles[2].number_format,
            Some(NormalizedNumberFormat::Decimal2)
        );
        Ok(())
    }

    #[test]
    fn malformed_style_relationship_indices_counts_and_duplicate_formats_reject()
    -> Result<(), Box<dyn std::error::Error>> {
        let styles = r#"<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font/></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellXfs count="1"><xf/></cellXfs></styleSheet>"#;
        let invalid_index = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"/></row></sheetData></worksheet>"#;
        assert_eq!(
            normalize(styled_package(invalid_index, styles)?),
            Err(XlsxError::InvalidPackage)
        );
        let without_relationship = package(&[
            ("xl/styles.xml", styles),
            ("xl/worksheets/sheet1.xml", invalid_index),
        ])?;
        assert_eq!(
            normalize(without_relationship),
            Err(XlsxError::InvalidPackage)
        );
        let bad_count = styles.replace("fonts count=\"1\"", "fonts count=\"2\"");
        assert_eq!(
            normalize(styled_package(invalid_index, &bad_count)?),
            Err(XlsxError::InvalidPackage)
        );
        let duplicate_formats = styles.replace("<fonts count=", "<numFmts count=\"2\"><numFmt numFmtId=\"164\" formatCode=\"0\"/><numFmt numFmtId=\"164\" formatCode=\"0.00\"/></numFmts><fonts count=");
        assert_eq!(
            normalize(styled_package(invalid_index, &duplicate_formats)?),
            Err(XlsxError::InvalidPackage)
        );
        Ok(())
    }
}
