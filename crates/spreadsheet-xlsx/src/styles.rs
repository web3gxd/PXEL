//! Bounded, relationship-resolved parser for the profile-1 OOXML cell styles.

use super::{LoadedPackage, XlsxError};
use crate::{
    NormalizedBorder, NormalizedBorderLine, NormalizedHorizontal, NormalizedNumberFormat,
    NormalizedStyle, NormalizedUnderline, NormalizedVertical,
};
use quick_xml::{Reader, XmlVersion, events::Event};
use std::collections::HashMap;

const MAX_STYLE_TABLE: usize = 4096;
const MAX_STYLE_NODES: usize = 200_000;
type ParsedFont = (NormalizedStyle, bool);
type ParsedFill = (Option<[u8; 3]>, bool);
type ParsedBorder = ([Option<NormalizedBorder>; 4], bool);

#[derive(Clone, Debug)]
pub(super) struct StyleEntry {
    pub style: Option<NormalizedStyle>,
    pub unsupported: bool,
}

#[derive(Clone, Debug)]
pub(super) struct StyleTable {
    pub entries: Vec<StyleEntry>,
}

#[derive(Default)]
struct Node {
    name: String,
    attrs: HashMap<String, String>,
    text: String,
    children: Vec<Node>,
}

pub(super) fn parse(package: &LoadedPackage) -> Result<Option<StyleTable>, XlsxError> {
    let Some(part) = &package.styles_part else {
        return Ok(None);
    };
    let bytes = package.parts.get(part).ok_or(XlsxError::InvalidPackage)?;
    let root = tree(bytes)?;
    if root.name != "styleSheet"
        || root
            .attrs
            .get("xmlns")
            .is_none_or(|ns| ns != super::WORKBOOK_NS)
    {
        return Err(XlsxError::InvalidPackage);
    }
    for child in &root.children {
        if !matches!(
            child.name.as_str(),
            "numFmts"
                | "fonts"
                | "fills"
                | "borders"
                | "cellStyleXfs"
                | "cellXfs"
                | "cellStyles"
                | "dxfs"
                | "tableStyles"
                | "colors"
                | "extLst"
        ) {
            return Err(XlsxError::UnsupportedFeature);
        }
    }
    let custom_formats = parse_formats(one(&root, "numFmts")?)?;
    let fonts = parse_fonts(one(&root, "fonts")?)?;
    let fills = parse_fills(one(&root, "fills")?)?;
    let borders = parse_borders(one(&root, "borders")?)?;
    for (section, child) in [
        ("cellStyleXfs", "xf"),
        ("cellStyles", "cellStyle"),
        ("dxfs", "dxf"),
        ("tableStyles", "tableStyle"),
    ] {
        let _ = table(one(&root, section)?, child)?;
    }
    let xfs = one(&root, "cellXfs")?.ok_or(XlsxError::InvalidPackage)?;
    let xf_nodes = table(Some(xfs), "xf")?;
    let mut entries = Vec::with_capacity(xf_nodes.len());
    for xf in xf_nodes {
        entries.push(parse_xf(xf, &fonts, &fills, &borders, &custom_formats)?);
    }
    if entries.is_empty() {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(Some(StyleTable { entries }))
}

fn tree(bytes: &[u8]) -> Result<Node, XlsxError> {
    let text = std::str::from_utf8(bytes).map_err(|_| XlsxError::InvalidPackage)?;
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut count = 0usize;
    loop {
        match reader.read_event().map_err(|_| XlsxError::InvalidPackage)? {
            Event::Start(start) => {
                count = count.checked_add(1).ok_or(XlsxError::LimitExceeded)?;
                if count > MAX_STYLE_NODES || stack.len() >= 64 {
                    return Err(XlsxError::LimitExceeded);
                }
                stack.push(Node {
                    name: start.name().as_ref().to_owned(),
                    attrs: super::attributes(&start)?,
                    ..Node::default()
                });
            }
            Event::Empty(start) => {
                count = count.checked_add(1).ok_or(XlsxError::LimitExceeded)?;
                if count > MAX_STYLE_NODES {
                    return Err(XlsxError::LimitExceeded);
                }
                let node = Node {
                    name: start.name().as_ref().to_owned(),
                    attrs: super::attributes(&start)?,
                    ..Node::default()
                };
                attach(&mut stack, &mut root, node)?;
            }
            Event::Text(value) => {
                if let Some(node) = stack.last_mut() {
                    node.text
                        .push_str(&value.xml_content(XmlVersion::Implicit1_0));
                } else if !value.as_ref().trim().is_empty() {
                    return Err(XlsxError::InvalidPackage);
                }
            }
            Event::GeneralRef(reference) => {
                let value = super::resolve_xml_reference(&reference)?;
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&value);
                }
            }
            Event::End(_) => {
                let node = stack.pop().ok_or(XlsxError::InvalidPackage)?;
                attach(&mut stack, &mut root, node)?;
            }
            Event::DocType(_) => return Err(XlsxError::UnsupportedActiveContent),
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            _ => return Err(XlsxError::UnsupportedFeature),
        }
    }
    if !stack.is_empty() {
        return Err(XlsxError::InvalidPackage);
    }
    root.ok_or(XlsxError::InvalidPackage)
}

fn attach(stack: &mut [Node], root: &mut Option<Node>, node: Node) -> Result<(), XlsxError> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(node);
    } else if root.replace(node).is_some() {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(())
}

fn one<'a>(root: &'a Node, name: &str) -> Result<Option<&'a Node>, XlsxError> {
    let mut matching = root.children.iter().filter(|node| node.name == name);
    let first = matching.next();
    if matching.next().is_some() {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(first)
}

fn table<'a>(node: Option<&'a Node>, child: &str) -> Result<Vec<&'a Node>, XlsxError> {
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let values = node
        .children
        .iter()
        .filter(|item| item.name == child)
        .collect::<Vec<_>>();
    if values.len() > MAX_STYLE_TABLE {
        return Err(XlsxError::LimitExceeded);
    }
    if let Some(declared) = node.attrs.get("count") {
        let declared = declared
            .parse::<usize>()
            .map_err(|_| XlsxError::InvalidPackage)?;
        if declared != values.len() {
            return Err(XlsxError::InvalidPackage);
        }
    }
    Ok(values)
}

fn parse_formats(
    node: Option<&Node>,
) -> Result<HashMap<u32, Option<NormalizedNumberFormat>>, XlsxError> {
    let mut result = HashMap::new();
    for item in table(node, "numFmt")? {
        if !item.children.is_empty() {
            return Err(XlsxError::InvalidPackage);
        }
        let id = number(item, "numFmtId", 0)?;
        let code = item
            .attrs
            .get("formatCode")
            .ok_or(XlsxError::InvalidPackage)?;
        let mapped = match code.as_str() {
            "0" => Some(NormalizedNumberFormat::Integer),
            "0.00" => Some(NormalizedNumberFormat::Decimal2),
            "0%" => Some(NormalizedNumberFormat::Percent0),
            "0.00%" => Some(NormalizedNumberFormat::Percent2),
            "0.00E+00" => Some(NormalizedNumberFormat::Scientific2),
            "yyyy-mm-dd" => Some(NormalizedNumberFormat::DateIso),
            _ => None,
        };
        if id < 164 || result.insert(id, mapped).is_some() {
            return Err(XlsxError::InvalidPackage);
        }
    }
    Ok(result)
}

fn parse_fonts(node: Option<&Node>) -> Result<Vec<ParsedFont>, XlsxError> {
    let values = table(node, "font")?;
    if values.is_empty() {
        return Err(XlsxError::InvalidPackage);
    }
    values.into_iter().map(parse_font).collect()
}

fn parse_font(node: &Node) -> Result<ParsedFont, XlsxError> {
    let mut style = empty_style();
    let mut unsupported = false;
    let mut seen = std::collections::HashSet::new();
    for item in &node.children {
        if !seen.insert(item.name.as_str()) {
            return Err(XlsxError::InvalidPackage);
        }
        match item.name.as_str() {
            "name" => style.font_family = Some(required_attr(item, "val")?.to_owned()),
            "sz" => {
                style.font_size_hundredth_points =
                    Some(parse_hundredths(required_attr(item, "val")?)?)
            }
            "b" => style.bold = Some(bool_value(item)?),
            "i" => style.italic = Some(bool_value(item)?),
            "strike" => style.strike = Some(bool_value(item)?),
            "u" => {
                style.underline = match item
                    .attrs
                    .get("val")
                    .map(String::as_str)
                    .unwrap_or("single")
                {
                    "single" => Some(NormalizedUnderline::Single),
                    "double" => Some(NormalizedUnderline::Double),
                    _ => {
                        unsupported = true;
                        None
                    }
                }
            }
            "color" => match color(item)? {
                (Some(value), false) => style.font_color = Some(value),
                (_, true) => unsupported = true,
                _ => {}
            },
            "family" | "scheme" | "vertAlign" | "charset" | "condense" | "extend" | "outline"
            | "shadow" | "cancel" => {}
            _ => unsupported = true,
        }
    }
    normalize(&mut style);
    Ok((style, unsupported))
}

fn parse_fills(node: Option<&Node>) -> Result<Vec<ParsedFill>, XlsxError> {
    let values = table(node, "fill")?;
    if values.is_empty() {
        return Err(XlsxError::InvalidPackage);
    }
    values
        .into_iter()
        .map(|fill| {
            let mut result = None;
            let mut unsupported = false;
            if fill
                .children
                .iter()
                .filter(|child| child.name == "patternFill")
                .count()
                > 1
            {
                return Err(XlsxError::InvalidPackage);
            }
            for child in &fill.children {
                if child.name != "patternFill" {
                    unsupported = true;
                    continue;
                }
                match child
                    .attrs
                    .get("patternType")
                    .map(String::as_str)
                    .unwrap_or("none")
                {
                    "none" | "gray125" => {}
                    "solid" => {
                        if let Some(fg) = one(child, "fgColor")? {
                            match color(fg)? {
                                (Some(value), false) => result = Some(value),
                                (_, true) => unsupported = true,
                                _ => {}
                            }
                        } else {
                            unsupported = true;
                        }
                        if one(child, "bgColor")?.is_some() {
                            unsupported = true;
                        }
                    }
                    _ => unsupported = true,
                }
            }
            Ok((result, unsupported))
        })
        .collect()
}

fn parse_borders(node: Option<&Node>) -> Result<Vec<ParsedBorder>, XlsxError> {
    let values = table(node, "border")?;
    if values.is_empty() {
        return Err(XlsxError::InvalidPackage);
    }
    values
        .into_iter()
        .map(|border| {
            let mut sides = [None; 4];
            let mut unsupported = border.children.iter().any(|child| {
                !matches!(
                    child.name.as_str(),
                    "top" | "right" | "bottom" | "left" | "diagonal"
                ) || child.name == "diagonal"
                    && (!child.attrs.is_empty() || !child.children.is_empty())
            }) || border
                .attrs
                .keys()
                .any(|key| !matches!(key.as_str(), "diagonalUp" | "diagonalDown" | "outline"))
                || ["diagonalUp", "diagonalDown"].iter().any(|key| {
                    border
                        .attrs
                        .get(*key)
                        .is_some_and(|value| value == "1" || value == "true")
                });
            for (index, name) in ["top", "right", "bottom", "left"].iter().enumerate() {
                if let Some(side) = one(border, name)?
                    && let Some(line) = side.attrs.get("style")
                {
                    let mapped = match line.as_str() {
                        "thin" => Some(NormalizedBorderLine::Thin),
                        "medium" => Some(NormalizedBorderLine::Medium),
                        "thick" => Some(NormalizedBorderLine::Thick),
                        "dashed" => Some(NormalizedBorderLine::Dashed),
                        "dotted" => Some(NormalizedBorderLine::Dotted),
                        "double" => Some(NormalizedBorderLine::Double),
                        _ => None,
                    };
                    if let Some(line) = mapped {
                        if let Some(color_node) = one(side, "color")? {
                            match color(color_node)? {
                                (Some(color), false) => {
                                    sides[index] = Some(NormalizedBorder { line, color })
                                }
                                (_, true) => unsupported = true,
                                _ => unsupported = true,
                            }
                        } else {
                            unsupported = true;
                        }
                    } else {
                        unsupported = true;
                    }
                }
            }
            Ok((sides, unsupported))
        })
        .collect()
}

fn parse_xf(
    xf: &Node,
    fonts: &[(NormalizedStyle, bool)],
    fills: &[(Option<[u8; 3]>, bool)],
    borders: &[([Option<NormalizedBorder>; 4], bool)],
    formats: &HashMap<u32, Option<NormalizedNumberFormat>>,
) -> Result<StyleEntry, XlsxError> {
    let font_id = number(xf, "fontId", 0)? as usize;
    let fill_id = number(xf, "fillId", 0)? as usize;
    let border_id = number(xf, "borderId", 0)? as usize;
    let format_id = number(xf, "numFmtId", 0)?;
    let (mut style, font_issue) = fonts
        .get(font_id)
        .cloned()
        .ok_or(XlsxError::InvalidPackage)?;
    let mut unsupported = false;
    let (fill, fill_issue) = *fills.get(fill_id).ok_or(XlsxError::InvalidPackage)?;
    style.fill = fill;
    let (sides, border_issue) = borders
        .get(border_id)
        .cloned()
        .ok_or(XlsxError::InvalidPackage)?;
    [
        style.border_top,
        style.border_right,
        style.border_bottom,
        style.border_left,
    ] = sides;
    let apply_font = xf
        .attrs
        .get("applyFont")
        .map(|value| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    let apply_fill = xf
        .attrs
        .get("applyFill")
        .map(|value| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    let apply_border = xf
        .attrs
        .get("applyBorder")
        .map(|value| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    let apply_number = xf
        .attrs
        .get("applyNumberFormat")
        .map(|value| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    let apply_alignment = xf
        .attrs
        .get("applyAlignment")
        .map(|value| parse_bool(value))
        .transpose()?
        .unwrap_or(true);
    if !apply_font {
        style.font_family = None;
        style.font_size_hundredth_points = None;
        style.bold = None;
        style.italic = None;
        style.underline = None;
        style.strike = None;
        style.font_color = None;
    }
    if apply_font {
        unsupported |= font_issue;
    }
    if !apply_fill {
        style.fill = None;
    } else {
        unsupported |= fill_issue;
    }
    if !apply_border {
        style.border_top = None;
        style.border_right = None;
        style.border_bottom = None;
        style.border_left = None;
    } else {
        unsupported |= border_issue;
    }
    if !apply_number || format_id == 0 {
        style.number_format = None;
    } else if let Some(mapped) = builtin_format(format_id) {
        style.number_format = Some(mapped);
    } else if format_id < 164 {
        unsupported = true;
    } else if let Some(mapped) = formats.get(&format_id) {
        style.number_format = *mapped;
        unsupported |= mapped.is_none();
    } else {
        return Err(XlsxError::InvalidPackage);
    }
    if xf
        .attrs
        .get("xfId")
        .is_some_and(|value| value.parse::<u32>() != Ok(0))
    {
        unsupported = true;
    }
    if xf.attrs.keys().any(|key| {
        !matches!(
            key.as_str(),
            "fontId"
                | "fillId"
                | "borderId"
                | "numFmtId"
                | "xfId"
                | "applyFont"
                | "applyFill"
                | "applyBorder"
                | "applyNumberFormat"
                | "applyAlignment"
                | "applyProtection"
                | "pivotButton"
                | "quotePrefix"
        )
    }) {
        return Err(XlsxError::InvalidPackage);
    }
    for flag in [
        "applyFont",
        "applyFill",
        "applyBorder",
        "applyNumberFormat",
        "applyAlignment",
        "applyProtection",
        "pivotButton",
        "quotePrefix",
    ] {
        if let Some(value) = xf.attrs.get(flag) {
            let enabled = parse_bool(value)?;
            if matches!(flag, "pivotButton" | "quotePrefix") && enabled {
                unsupported = true;
            }
            if !enabled && flag == "applyProtection" && one(xf, "protection")?.is_some() {
                unsupported = true;
            }
        }
    }
    if let Some(alignment) = one(xf, "alignment")? {
        if xf.children.iter().any(|child| child.name != "alignment") {
            unsupported = true;
        }
        let values = &alignment.attrs;
        if values.keys().any(|key| {
            !matches!(
                key.as_str(),
                "horizontal"
                    | "vertical"
                    | "wrapText"
                    | "shrinkToFit"
                    | "indent"
                    | "relativeIndent"
                    | "justifyLastLine"
                    | "readingOrder"
                    | "textRotation"
            )
        }) {
            return Err(XlsxError::InvalidPackage);
        }
    }
    for child in &xf.children {
        if child.name == "alignment" && apply_alignment {
            match child.attrs.get("horizontal").map(String::as_str) {
                Some("general") => {}
                Some("left") => style.horizontal = Some(NormalizedHorizontal::Left),
                Some("center") => style.horizontal = Some(NormalizedHorizontal::Center),
                Some("right") => style.horizontal = Some(NormalizedHorizontal::Right),
                Some(_) => unsupported = true,
                None => {}
            }
            match child.attrs.get("vertical").map(String::as_str) {
                Some("top") => style.vertical = Some(NormalizedVertical::Top),
                Some("center") => style.vertical = Some(NormalizedVertical::Center),
                Some("bottom") => {}
                Some(_) => unsupported = true,
                None => {}
            }
            if let Some(value) = child.attrs.get("wrapText") {
                style.wrap = Some(parse_bool(value)?);
            }
            if child
                .attrs
                .keys()
                .any(|key| !matches!(key.as_str(), "horizontal" | "vertical" | "wrapText"))
            {
                unsupported = true;
            }
        } else if child.name == "protection" {
            if xf
                .attrs
                .get("applyProtection")
                .is_some_and(|value| value == "1" || value == "true")
            {
                unsupported = true;
            }
        } else if child.name != "alignment" {
            unsupported = true;
        }
    }
    normalize(&mut style);
    Ok(StyleEntry {
        style: (!style.is_default()).then_some(style),
        unsupported,
    })
}

fn builtin_format(id: u32) -> Option<NormalizedNumberFormat> {
    match id {
        1 => Some(NormalizedNumberFormat::Integer),
        2 => Some(NormalizedNumberFormat::Decimal2),
        9 => Some(NormalizedNumberFormat::Percent0),
        10 => Some(NormalizedNumberFormat::Percent2),
        11 => Some(NormalizedNumberFormat::Scientific2),
        _ => None,
    }
}

fn normalize(style: &mut NormalizedStyle) {
    if style.font_family.as_deref() == Some("Arial") {
        style.font_family = None;
    }
    if style.font_size_hundredth_points == Some(1000) {
        style.font_size_hundredth_points = None;
    }
    if style.bold == Some(false) {
        style.bold = None;
    }
    if style.italic == Some(false) {
        style.italic = None;
    }
    if style.strike == Some(false) {
        style.strike = None;
    }
    if style.font_color == Some([0, 0, 0]) {
        style.font_color = None;
    }
    if style.fill == Some([255, 255, 255]) {
        style.fill = None;
    }
    if style.horizontal == Some(NormalizedHorizontal::General) {
        style.horizontal = None;
    }
    if style.vertical == Some(NormalizedVertical::Bottom) {
        style.vertical = None;
    }
    if style.wrap == Some(false) {
        style.wrap = None;
    }
    if style.number_format == Some(NormalizedNumberFormat::General) {
        style.number_format = None;
    }
}

fn empty_style() -> NormalizedStyle {
    NormalizedStyle {
        row: 1,
        column: 1,
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
        number_format: None,
    }
}

fn number(node: &Node, key: &str, default: u32) -> Result<u32, XlsxError> {
    node.attrs.get(key).map_or(Ok(default), |value| {
        value.parse().map_err(|_| XlsxError::InvalidPackage)
    })
}
fn required_attr<'a>(node: &'a Node, key: &str) -> Result<&'a str, XlsxError> {
    node.attrs
        .get(key)
        .map(String::as_str)
        .ok_or(XlsxError::InvalidPackage)
}
fn parse_bool(value: &str) -> Result<bool, XlsxError> {
    match value {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(XlsxError::InvalidPackage),
    }
}
fn bool_value(node: &Node) -> Result<bool, XlsxError> {
    parse_bool(node.attrs.get("val").map(String::as_str).unwrap_or("1"))
}
fn parse_hundredths(value: &str) -> Result<u16, XlsxError> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(XlsxError::InvalidPackage);
    }
    let whole = whole
        .parse::<u32>()
        .map_err(|_| XlsxError::InvalidPackage)?;
    let fraction = match fraction.len() {
        0 => 0,
        1 => {
            fraction
                .parse::<u32>()
                .map_err(|_| XlsxError::InvalidPackage)?
                * 10
        }
        _ => fraction
            .parse::<u32>()
            .map_err(|_| XlsxError::InvalidPackage)?,
    };
    let value = whole
        .checked_mul(100)
        .and_then(|value| value.checked_add(fraction))
        .ok_or(XlsxError::InvalidPackage)?;
    u16::try_from(value).map_err(|_| XlsxError::InvalidPackage)
}
fn color(node: &Node) -> Result<(Option<[u8; 3]>, bool), XlsxError> {
    if node.attrs.len() != 1 {
        return Ok((None, true));
    }
    let Some(value) = node.attrs.get("rgb") else {
        return Ok((None, true));
    };
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(XlsxError::InvalidPackage);
    }
    if !value.starts_with("FF") {
        return Ok((None, true));
    }
    let mut rgb = [0; 3];
    for index in 0..3 {
        rgb[index] = u8::from_str_radix(&value[2 + index * 2..4 + index * 2], 16)
            .map_err(|_| XlsxError::InvalidPackage)?;
    }
    Ok((Some(rgb), false))
}
