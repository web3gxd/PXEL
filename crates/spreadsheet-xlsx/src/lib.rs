//! Bounded inspection of an untrusted OOXML package. This first phase-6 slice
//! resolves workbook/sheet relationships into metadata only; no cells are
//! published into the domain projection.

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

pub fn inspect_file(path: &Path) -> Result<PackageInspection, XlsxError> {
    let file = File::open(path).map_err(|_| XlsxError::Io)?;
    let len = file.metadata().map_err(|_| XlsxError::Io)?.len();
    inspect_reader(file, len)
}

pub fn inspect_reader<R: Read + Seek>(
    input: R,
    source_len: u64,
) -> Result<PackageInspection, XlsxError> {
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
    let sheets = parse_sheets(workbook, &workbook_part, &workbook_rels, &parts)?;
    Ok(PackageInspection {
        sheets,
        entry_count,
        uncompressed_bytes: total,
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
    loop {
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err(XlsxError::LimitExceeded);
                }
                validate_attributes(&start)?;
            }
            Event::Empty(start) => validate_attributes(&start)?,
            Event::End(_) => depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?,
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err(XlsxError::UnsupportedActiveContent);
            }
            Event::Text(text) if text.len() > 1024 * 1024 => return Err(XlsxError::LimitExceeded),
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(())
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
    use super::{XlsxError, inspect_reader};
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

    fn inspect(bytes: Vec<u8>) -> Result<super::PackageInspection, XlsxError> {
        let len = bytes.len() as u64;
        inspect_reader(Cursor::new(bytes), len)
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
}
