use super::{
    LoadedPackage, MAX_INSPECTION_TIME, SHARED_STRING_REL, WORKBOOK_NS, XlsxError, attributes,
    resolve_target, resolve_xml_reference,
};
use quick_xml::{Reader, XmlVersion, events::Event};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

const MAX_COMPATIBILITY_ISSUES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum CompatibilityIssueCode {
    UnsupportedPackagePart,
    WorkbookMetadata,
    WorksheetMetadata,
    RowFormatting,
    CellFormatting,
}

/// Bounded report without imported names or cell contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityIssue {
    pub code: CompatibilityIssueCode,
    /// Zero-based sheet index; None identifies a workbook/package issue.
    pub sheet_index: Option<u16>,
}

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
    /// Versioned worker boundary schema. New output is always schema 3.
    pub schema_version: u16,
    pub sheets: Vec<NormalizedSheet>,
    pub issues: Vec<CompatibilityIssue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedSheet {
    pub name: String,
    pub cells: Vec<NormalizedCell>,
    /// Independent sparse cell styles. Entries may refer to blank cells.
    #[serde(default)]
    pub styles: Vec<NormalizedStyle>,
    /// Inclusive, one-based OOXML coordinates, sorted in canonical order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merges: Vec<NormalizedMerge>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedMerge {
    pub first_row: u32,
    pub last_row: u32,
    pub first_column: u32,
    pub last_column: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedStyle {
    pub row: u32,
    pub column: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size_hundredth_points: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<NormalizedUnderline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_top: Option<NormalizedBorder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_right: Option<NormalizedBorder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_bottom: Option<NormalizedBorder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_left: Option<NormalizedBorder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizontal: Option<NormalizedHorizontal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical: Option<NormalizedVertical>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_format: Option<NormalizedNumberFormat>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedUnderline {
    Single,
    Double,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedHorizontal {
    General,
    Left,
    Center,
    Right,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedVertical {
    Top,
    Center,
    Bottom,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedBorderLine {
    Thin,
    Medium,
    Thick,
    Dashed,
    Dotted,
    Double,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedBorder {
    pub line: NormalizedBorderLine,
    pub color: [u8; 3],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NormalizedNumberFormat {
    General,
    Integer,
    Decimal2,
    Percent0,
    Percent2,
    Scientific2,
    DateIso,
}

impl NormalizedStyle {
    pub fn is_default(&self) -> bool {
        self.font_family
            .as_deref()
            .is_none_or(|value| value == "Arial")
            && self
                .font_size_hundredth_points
                .is_none_or(|value| value == 1000)
            && self.bold.is_none_or(|value| !value)
            && self.italic.is_none_or(|value| !value)
            && self.underline.is_none()
            && self.strike.is_none_or(|value| !value)
            && self.font_color.is_none_or(|value| value == [0, 0, 0])
            && self.fill.is_none_or(|value| value == [255, 255, 255])
            && self.border_top.is_none()
            && self.border_right.is_none()
            && self.border_bottom.is_none()
            && self.border_left.is_none()
            && self
                .horizontal
                .is_none_or(|value| value == NormalizedHorizontal::General)
            && self
                .vertical
                .is_none_or(|value| value == NormalizedVertical::Bottom)
            && self.wrap.is_none_or(|value| !value)
            && self
                .number_format
                .is_none_or(|value| value == NormalizedNumberFormat::General)
    }

    fn is_canonical(&self) -> bool {
        !self.is_default()
            && self.font_family.as_ref().is_none_or(|value| {
                valid_xml_text(value)
                    && !value.is_empty()
                    && value.len() <= 128
                    && value.trim() == value
                    && !value.chars().any(char::is_control)
                    && value != "Arial"
            })
            && self
                .font_size_hundredth_points
                .is_none_or(|value| (100..=20000).contains(&value) && value != 1000)
            && self.bold != Some(false)
            && self.italic != Some(false)
            && self.strike != Some(false)
            && self.font_color != Some([0, 0, 0])
            && self.fill != Some([255, 255, 255])
            && self.horizontal != Some(NormalizedHorizontal::General)
            && self.vertical != Some(NormalizedVertical::Bottom)
            && self.wrap != Some(false)
            && self.number_format != Some(NormalizedNumberFormat::General)
    }
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
#[serde(deny_unknown_fields)]
pub enum NormalizedCellValue {
    Number(f64),
    Text(String),
    Boolean(bool),
    /// Source without the leading '='; server binding must parse and validate it.
    Formula(String),
    /// Formula copied from a shared-formula master using relative-reference rules.
    FormulaCopy {
        master_row: u32,
        master_column: u32,
    },
}

impl NormalizedWorkbook {
    /// Revalidate child output at the parent boundary before any import step.
    pub fn validate(&self) -> Result<(), XlsxError> {
        if self.schema_version != 3 {
            return Err(XlsxError::InvalidPackage);
        }
        if self.sheets.is_empty() || self.sheets.len() > 64 {
            return Err(XlsxError::LimitExceeded);
        }
        if self.issues.len() > MAX_COMPATIBILITY_ISSUES
            || self.issues.iter().any(|issue| {
                let global = matches!(
                    issue.code,
                    CompatibilityIssueCode::UnsupportedPackagePart
                        | CompatibilityIssueCode::WorkbookMetadata
                );
                global != issue.sheet_index.is_none()
                    || issue
                        .sheet_index
                        .is_some_and(|index| usize::from(index) >= self.sheets.len())
            })
            || !self.issues.windows(2).all(|pair| pair[0] < pair[1])
        {
            return Err(XlsxError::InvalidPackage);
        }
        let mut names = HashSet::new();
        let mut total_cells = 0_usize;
        let mut total_styles = 0_usize;
        let mut total_text_bytes = 0_usize;
        let mut total_merged_cells = 0usize;
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
                    NormalizedCellValue::FormulaCopy {
                        master_row,
                        master_column,
                    } => {
                        if *master_row == 0
                            || *master_row > MAX_ROW
                            || *master_column == 0
                            || *master_column > MAX_COLUMN
                        {
                            return Err(XlsxError::InvalidPackage);
                        }
                    }
                    NormalizedCellValue::Number(_) | NormalizedCellValue::Boolean(_) => {}
                }
            }
            let formulas = sheet
                .cells
                .iter()
                .filter_map(|cell| match &cell.value {
                    NormalizedCellValue::Formula(_) => Some(((cell.row, cell.column), false)),
                    NormalizedCellValue::FormulaCopy { .. } => {
                        Some(((cell.row, cell.column), true))
                    }
                    _ => None,
                })
                .collect::<HashMap<_, _>>();
            for cell in &sheet.cells {
                if let NormalizedCellValue::FormulaCopy {
                    master_row,
                    master_column,
                } = &cell.value
                    && (formulas.get(&(*master_row, *master_column)) != Some(&false)
                        || (*master_row, *master_column) == (cell.row, cell.column))
                {
                    return Err(XlsxError::InvalidPackage);
                }
            }
            let mut previous_style = None;
            for style in &sheet.styles {
                let address = (style.row, style.column);
                if style.row == 0
                    || style.row > MAX_ROW
                    || style.column == 0
                    || style.column > MAX_COLUMN
                    || previous_style.is_some_and(|last| last >= address)
                    || !style.is_canonical()
                {
                    return Err(XlsxError::InvalidPackage);
                }
                previous_style = Some(address);
                total_styles = total_styles
                    .checked_add(1)
                    .ok_or(XlsxError::LimitExceeded)?;
                if total_styles > MAX_CELLS {
                    return Err(XlsxError::LimitExceeded);
                }
                total_text_bytes = total_text_bytes
                    .checked_add(style.font_family.as_ref().map_or(0, String::len))
                    .ok_or(XlsxError::LimitExceeded)?;
                if total_text_bytes > MAX_IR_TEXT_BYTES {
                    return Err(XlsxError::LimitExceeded);
                }
            }
            let union_count = sheet
                .cells
                .iter()
                .map(|cell| (cell.row, cell.column))
                .chain(sheet.styles.iter().map(|style| (style.row, style.column)))
                .collect::<HashSet<_>>()
                .len();
            if union_count > MAX_CELLS {
                return Err(XlsxError::LimitExceeded);
            }
            let authored_cells = sheet
                .cells
                .iter()
                .map(|cell| (cell.row, cell.column))
                .collect::<HashSet<_>>();
            let authored_styles = sheet
                .styles
                .iter()
                .map(|style| (style.row, style.column))
                .collect::<HashSet<_>>();
            let mut covered = HashSet::new();
            let mut previous_merge = None;
            for merge in &sheet.merges {
                if merge.first_row == 0
                    || merge.first_row > merge.last_row
                    || merge.last_row > MAX_ROW
                    || merge.first_column == 0
                    || merge.first_column > merge.last_column
                    || merge.last_column > MAX_COLUMN
                    || previous_merge.is_some_and(|last| last >= *merge)
                {
                    return Err(XlsxError::InvalidPackage);
                }
                previous_merge = Some(*merge);
                let height = u64::from(merge.last_row - merge.first_row + 1);
                let width = u64::from(merge.last_column - merge.first_column + 1);
                let area = height.checked_mul(width).ok_or(XlsxError::LimitExceeded)?;
                if !(2..=10_000).contains(&area) {
                    return Err(XlsxError::InvalidPackage);
                }
                total_merged_cells = total_merged_cells
                    .checked_add(usize::try_from(area).map_err(|_| XlsxError::LimitExceeded)?)
                    .ok_or(XlsxError::LimitExceeded)?;
                if total_merged_cells > 100_000 {
                    return Err(XlsxError::LimitExceeded);
                }
                for row in merge.first_row..=merge.last_row {
                    for column in merge.first_column..=merge.last_column {
                        if !covered.insert((row, column)) {
                            return Err(XlsxError::InvalidPackage);
                        }
                        if (row != merge.first_row || column != merge.first_column)
                            && (authored_cells.contains(&(row, column))
                                || authored_styles.contains(&(row, column)))
                        {
                            return Err(XlsxError::InvalidPackage);
                        }
                    }
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

#[cfg(test)]
mod ir_tests {
    use super::*;

    fn workbook(styles: Vec<NormalizedStyle>) -> NormalizedWorkbook {
        NormalizedWorkbook {
            schema_version: 3,
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                cells: vec![],
                styles,
                merges: vec![],
            }],
            issues: vec![],
        }
    }

    fn bold(row: u32, column: u32) -> NormalizedStyle {
        NormalizedStyle {
            row,
            column,
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
        }
    }

    #[test]
    fn parent_ir_accepts_sparse_blank_style_and_rejects_untrusted_variants() {
        assert!(workbook(vec![bold(1, 1)]).validate().is_ok());
        let mut invalid = workbook(vec![bold(1, 1)]);
        invalid.schema_version = 1;
        assert_eq!(invalid.validate(), Err(XlsxError::InvalidPackage));
        let invalid = workbook(vec![bold(1, 1), bold(1, 1)]);
        assert_eq!(invalid.validate(), Err(XlsxError::InvalidPackage));
        let invalid = workbook(vec![bold(2, 1), bold(1, 1)]);
        assert_eq!(invalid.validate(), Err(XlsxError::InvalidPackage));
        let mut default = bold(1, 1);
        default.bold = Some(false);
        assert_eq!(
            workbook(vec![default]).validate(),
            Err(XlsxError::InvalidPackage)
        );
    }

    #[test]
    fn style_union_budget_does_not_double_count_shared_content_address() {
        let mut value = workbook(vec![bold(1, 1)]);
        value.sheets[0].cells.push(NormalizedCell {
            row: 1,
            column: 1,
            value: NormalizedCellValue::Text("x".into()),
        });
        assert!(value.validate().is_ok());
    }

    #[test]
    fn parent_ir_only_accepts_formula_copies_pointing_to_direct_formulas() {
        let mut value = workbook(vec![]);
        value.sheets[0].cells = vec![
            NormalizedCell {
                row: 1,
                column: 1,
                value: NormalizedCellValue::Formula("A1*2".into()),
            },
            NormalizedCell {
                row: 2,
                column: 1,
                value: NormalizedCellValue::FormulaCopy {
                    master_row: 1,
                    master_column: 1,
                },
            },
        ];
        assert!(value.validate().is_ok());
        value.sheets[0].cells[1].value = NormalizedCellValue::FormulaCopy {
            master_row: 2,
            master_column: 1,
        };
        assert_eq!(value.validate(), Err(XlsxError::InvalidPackage));
        value.sheets[0].cells[1].value = NormalizedCellValue::FormulaCopy {
            master_row: 1,
            master_column: 2,
        };
        assert_eq!(value.validate(), Err(XlsxError::InvalidPackage));
    }

    #[test]
    fn parent_ir_rejects_overlapping_merges_and_hidden_non_anchor_data() {
        let mut value = workbook(vec![]);
        value.sheets[0].merges = vec![
            NormalizedMerge {
                first_row: 1,
                last_row: 2,
                first_column: 1,
                last_column: 2,
            },
            NormalizedMerge {
                first_row: 2,
                last_row: 3,
                first_column: 2,
                last_column: 3,
            },
        ];
        assert_eq!(value.validate(), Err(XlsxError::InvalidPackage));
        value.sheets[0].merges = vec![NormalizedMerge {
            first_row: 1,
            last_row: 2,
            first_column: 1,
            last_column: 2,
        }];
        value.sheets[0].cells.push(NormalizedCell {
            row: 2,
            column: 1,
            value: NormalizedCellValue::Text("hidden".into()),
        });
        assert_eq!(value.validate(), Err(XlsxError::InvalidPackage));
    }
}

#[derive(Default)]
struct CellDraft {
    row: u32,
    column: u32,
    kind: String,
    style_index: Option<usize>,
    value: String,
    inline_text: String,
    formula: String,
    saw_value: bool,
    saw_formula: bool,
    saw_inline: bool,
    shared_formula: Option<SharedFormulaTag>,
}

#[derive(Clone, Debug)]
struct SharedFormulaTag {
    index: u32,
    range: Option<NormalizedMerge>,
}

#[derive(Clone, Copy)]
struct SharedFormulaMaster {
    row: u32,
    column: u32,
    range: NormalizedMerge,
}

enum ParsedCellValue {
    Value(NormalizedCellValue),
    SharedFollower(u32),
}

#[derive(Clone, Copy)]
enum TextField {
    Value,
    Formula,
    Inline,
}

pub(super) fn extract(package: &LoadedPackage) -> Result<NormalizedWorkbook, XlsxError> {
    let shared = shared_strings(package)?;
    let style_table = super::styles::parse(package)?;
    let mut sheets = Vec::with_capacity(package.inspection.sheets.len());
    let mut unsupported_sheets = Vec::new();
    let mut total_cells = 0_usize;
    let mut total_text_bytes = 0_usize;
    for (index, header) in package.inspection.sheets.iter().enumerate() {
        let body = package
            .parts
            .get(&header.part)
            .ok_or(XlsxError::InvalidPackage)?;
        let (cells, styles, merges, unsupported_cell_style) = parse_sheet(
            body,
            &shared,
            style_table.as_ref(),
            package,
            &mut total_cells,
            &mut total_text_bytes,
        )?;
        if unsupported_cell_style {
            unsupported_sheets.push(index);
        }
        sheets.push(NormalizedSheet {
            name: header.name.clone(),
            cells,
            styles,
            merges,
        });
    }
    let mut issues = scan_compatibility(package)?;
    for index in unsupported_sheets {
        issues.push(CompatibilityIssue {
            code: CompatibilityIssueCode::CellFormatting,
            sheet_index: Some(u16::try_from(index).map_err(|_| XlsxError::LimitExceeded)?),
        });
    }
    issues.sort_unstable();
    issues.dedup();
    let workbook = NormalizedWorkbook {
        schema_version: 3,
        sheets,
        issues,
    };
    workbook.validate()?;
    Ok(workbook)
}

#[cfg(test)]
mod excel_golden_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn decodes_ooxml_escapes_and_normalizes_cell_newlines() -> Result<(), XlsxError> {
        assert_eq!(
            decode_ooxml_text("first_x000D_\nsecond_x000D_third", true)?,
            "first\nsecond\nthird"
        );
        assert_eq!(decode_ooxml_text("emoji _xD83D__xDE00_", true)?, "emoji 😀");
        assert_eq!(decode_ooxml_text("_x005F_x000D_", true)?, "_x000D_");
        assert_eq!(
            decode_ooxml_text("literal _xnope_", true)?,
            "literal _xnope_"
        );
        assert_eq!(decode_ooxml_text("a\r\nb\rc", true)?, "a\nb\nc");
        assert_eq!(decode_ooxml_text("a\r\nb\rc", false)?, "a\r\nb\rc");
        Ok(())
    }

    #[test]
    fn rejects_unpaired_ooxml_surrogate_escapes() {
        assert!(matches!(
            decode_ooxml_text("_xD83D_", true),
            Err(XlsxError::InvalidPackage)
        ));
        assert!(matches!(
            decode_ooxml_text("_xDE00_", true),
            Err(XlsxError::InvalidPackage)
        ));
    }

    #[test]
    fn microsoft_excel_16_profile_fixture_normalizes() -> Result<(), XlsxError> {
        let bytes = include_bytes!("../../../fixtures/xlsx/excel-profile1.xlsx");
        let package = crate::load_package(Cursor::new(bytes.as_slice()), bytes.len() as u64)?;
        let shared = shared_strings(&package)?;
        let style_table = crate::styles::parse(&package)?;
        let mut sheets = Vec::with_capacity(package.inspection.sheets.len());
        let mut unsupported_sheets = Vec::new();
        let mut total_cells = 0;
        let mut total_text_bytes = 0;
        for (index, sheet) in package.inspection.sheets.iter().enumerate() {
            let body = package
                .parts
                .get(&sheet.part)
                .ok_or(XlsxError::InvalidPackage)?;
            let (cells, styles, merges, unsupported_style) = parse_sheet(
                body,
                &shared,
                style_table.as_ref(),
                &package,
                &mut total_cells,
                &mut total_text_bytes,
            )?;
            if unsupported_style {
                unsupported_sheets.push(index);
            }
            sheets.push(NormalizedSheet {
                name: sheet.name.clone(),
                cells,
                styles,
                merges,
            });
        }
        let mut issues = scan_compatibility(&package)?;
        for index in unsupported_sheets {
            issues.push(CompatibilityIssue {
                code: CompatibilityIssueCode::CellFormatting,
                sheet_index: Some(u16::try_from(index).map_err(|_| XlsxError::LimitExceeded)?),
            });
        }
        issues.sort_unstable();
        issues.dedup();
        let candidate = NormalizedWorkbook {
            schema_version: 3,
            sheets,
            issues,
        };
        let validation = candidate.validate();
        assert!(
            validation.is_ok(),
            "Excel parent IR rejected: sheet counts={:?}, styles/merges={:?}, issues={:?}, result={validation:?}",
            candidate
                .sheets
                .iter()
                .map(|sheet| (sheet.cells.len(), sheet.styles.len(), sheet.merges.len()))
                .collect::<Vec<_>>(),
            candidate
                .sheets
                .iter()
                .map(|sheet| (&sheet.name, &sheet.styles, &sheet.merges))
                .collect::<Vec<_>>(),
            candidate.issues
        );
        let workbook = extract(&package);
        assert!(workbook.is_ok(), "Excel workbook IR rejected: {workbook:?}");
        let workbook = workbook?;
        assert_eq!(workbook, candidate);
        assert_eq!(workbook.sheets.len(), 2);
        assert_eq!(workbook.sheets[0].name, "Отчёт Ω");
        assert_eq!(workbook.sheets[1].name, "Data");
        assert_eq!(
            workbook.sheets[0].cells,
            vec![
                NormalizedCell {
                    row: 1,
                    column: 1,
                    value: NormalizedCellValue::Text("Archive".into()),
                },
                NormalizedCell {
                    row: 1,
                    column: 2,
                    value: NormalizedCellValue::Number(2026.0),
                },
                NormalizedCell {
                    row: 2,
                    column: 1,
                    value: NormalizedCellValue::Text("snowman ☃".into()),
                },
            ]
        );
        let data = &workbook.sheets[1];
        assert_eq!(
            data.cells
                .iter()
                .map(|cell| (cell.row, cell.column))
                .collect::<Vec<_>>(),
            vec![
                (1, 1),
                (1, 2),
                (2, 1),
                (2, 2),
                (2, 3),
                (3, 1),
                (3, 2),
                (3, 3),
                (5, 1)
            ]
        );
        assert_eq!(
            data.cells[3].value,
            NormalizedCellValue::Text("Question line 1\nQuestion line 2 | literal".into())
        );
        assert_eq!(
            data.cells[4].value,
            NormalizedCellValue::Formula("SUM(1,2)".into())
        );
        assert_eq!(
            data.cells[6].value,
            NormalizedCellValue::Text("Quoted \"text\", Unicode Ω ☃\nSecond line".into())
        );
        assert_eq!(
            data.cells[7].value,
            NormalizedCellValue::Formula("1+2".into())
        );
        assert_eq!(
            data.merges,
            vec![NormalizedMerge {
                first_row: 5,
                last_row: 5,
                first_column: 1,
                last_column: 2,
            }]
        );
        assert!(
            !data
                .styles
                .iter()
                .any(|style| style.row == 5 && style.column == 2)
        );
        let question_style = data
            .styles
            .iter()
            .find(|style| style.row == 2 && style.column == 2)
            .ok_or(XlsxError::InvalidPackage)?;
        assert_eq!(question_style.font_size_hundredth_points, Some(1200));
        assert_eq!(question_style.bold, Some(true));
        assert_eq!(question_style.italic, Some(true));
        assert_eq!(question_style.underline, Some(NormalizedUnderline::Single));
        assert_eq!(question_style.strike, Some(true));
        assert_eq!(question_style.font_color, Some([40, 50, 60]));
        assert_eq!(question_style.fill, Some([220, 230, 240]));
        assert_eq!(
            question_style.horizontal,
            Some(NormalizedHorizontal::Center)
        );
        assert_eq!(question_style.vertical, Some(NormalizedVertical::Center));
        assert_eq!(question_style.wrap, Some(true));
        assert_eq!(
            question_style.number_format,
            Some(NormalizedNumberFormat::Decimal2)
        );
        assert!(workbook.issues.contains(&CompatibilityIssue {
            code: CompatibilityIssueCode::UnsupportedPackagePart,
            sheet_index: None,
        }));
        assert!(workbook.issues.contains(&CompatibilityIssue {
            code: CompatibilityIssueCode::WorkbookMetadata,
            sheet_index: None,
        }));

        let mut expected = workbook.clone();
        expected.issues.clear();
        let exported = crate::export_scalar_xlsx(&expected)?;
        let exported_again =
            crate::normalize_reader(Cursor::new(exported.as_slice()), exported.len() as u64)?;
        assert_eq!(exported_again, expected);
        Ok(())
    }

    #[test]
    fn microsoft_excel_shared_formula_groups_expand_to_validated_copy_references()
    -> Result<(), XlsxError> {
        let bytes = include_bytes!("../../../fixtures/xlsx/excel-shared-formulas.xlsx");
        let workbook = extract(&crate::load_package(
            Cursor::new(bytes.as_slice()),
            bytes.len() as u64,
        )?)?;
        let sheet = workbook.sheets.first().ok_or(XlsxError::InvalidPackage)?;
        let formula = |row, column| {
            sheet
                .cells
                .iter()
                .find(|cell| cell.row == row && cell.column == column)
                .map(|cell| &cell.value)
        };
        assert!(matches!(
            formula(2, 5),
            Some(NormalizedCellValue::Formula(_))
        ));
        assert_eq!(
            formula(3, 5),
            Some(&NormalizedCellValue::FormulaCopy {
                master_row: 2,
                master_column: 5,
            })
        );
        assert_eq!(
            formula(6, 7),
            Some(&NormalizedCellValue::FormulaCopy {
                master_row: 2,
                master_column: 7,
            })
        );
        assert!(matches!(
            formula(1, 5),
            Some(NormalizedCellValue::Formula(_))
        ));
        Ok(())
    }
}

fn scan_compatibility(package: &LoadedPackage) -> Result<Vec<CompatibilityIssue>, XlsxError> {
    let mut issues = BTreeSet::new();
    let mut supported_parts = HashSet::from([
        "[Content_Types].xml".to_owned(),
        "_rels/.rels".to_owned(),
        package.workbook_part.clone(),
        super::relationship_part(&package.workbook_part)?,
    ]);
    for sheet in &package.inspection.sheets {
        supported_parts.insert(sheet.part.clone());
    }
    if let Some(styles_part) = &package.styles_part {
        supported_parts.insert(styles_part.clone());
    }
    let base = package
        .workbook_part
        .rsplit_once('/')
        .map_or("", |(base, _)| base);
    for relationship in package.workbook_rels.values() {
        if relationship.kind == SHARED_STRING_REL {
            supported_parts.insert(resolve_target(base, &relationship.target)?);
        }
    }
    if package
        .parts
        .keys()
        .any(|part| !supported_parts.contains(part))
    {
        issues.insert(CompatibilityIssue {
            code: CompatibilityIssueCode::UnsupportedPackagePart,
            sheet_index: None,
        });
    }
    let workbook = package
        .parts
        .get(&package.workbook_part)
        .ok_or(XlsxError::InvalidPackage)?;
    scan_xml_features(workbook, None, &mut issues)?;
    for (index, sheet) in package.inspection.sheets.iter().enumerate() {
        let sheet_index = u16::try_from(index).map_err(|_| XlsxError::LimitExceeded)?;
        let body = package
            .parts
            .get(&sheet.part)
            .ok_or(XlsxError::InvalidPackage)?;
        scan_xml_features(body, Some(sheet_index), &mut issues)?;
    }
    if issues.len() > MAX_COMPATIBILITY_ISSUES {
        return Err(XlsxError::LimitExceeded);
    }
    Ok(issues.into_iter().collect())
}

fn scan_xml_features(
    body: &[u8],
    sheet_index: Option<u16>,
    issues: &mut BTreeSet<CompatibilityIssue>,
) -> Result<(), XlsxError> {
    let text = std::str::from_utf8(body).map_err(|_| XlsxError::InvalidPackage)?;
    let mut reader = Reader::from_str(text);
    let mut depth = 0_usize;
    loop {
        let event = reader.read_event().map_err(|_| XlsxError::InvalidPackage)?;
        let (element, element_depth) = match &event {
            Event::Start(start) => {
                depth = depth.checked_add(1).ok_or(XlsxError::LimitExceeded)?;
                (Some(start), depth)
            }
            Event::Empty(start) => (Some(start), depth + 1),
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(XlsxError::InvalidPackage)?;
                (None, depth)
            }
            Event::Eof => break,
            _ => (None, depth),
        };
        let Some(element) = element else { continue };
        let name = element.name();
        let tag = name.as_ref();
        let code = match sheet_index {
            None if element_depth == 2 && tag != "sheets" => {
                Some(CompatibilityIssueCode::WorkbookMetadata)
            }
            None if element_depth == 3 && tag == "sheet" => {
                let attrs = attributes(element)?;
                attrs
                    .keys()
                    .any(|key| !matches!(key.as_str(), "name" | "sheetId" | "r:id"))
                    .then_some(CompatibilityIssueCode::WorkbookMetadata)
            }
            Some(_)
                if element_depth == 2
                    && !matches!(tag, "sheetData" | "dimension" | "mergeCells") =>
            {
                Some(CompatibilityIssueCode::WorksheetMetadata)
            }
            Some(_) if element_depth == 3 && tag == "row" => {
                let attrs = attributes(element)?;
                attrs
                    .keys()
                    .any(|key| key != "r")
                    .then_some(CompatibilityIssueCode::RowFormatting)
            }
            Some(_) if element_depth == 4 && tag == "c" => {
                let attrs = attributes(element)?;
                attrs
                    .keys()
                    .any(|key| !matches!(key.as_str(), "r" | "t" | "s"))
                    .then_some(CompatibilityIssueCode::CellFormatting)
            }
            _ => None,
        };
        if let Some(code) = code {
            issues.insert(CompatibilityIssue { code, sheet_index });
        }
    }
    Ok(())
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
                        let decoded = decode_ooxml_text(
                            &current.take().ok_or(XlsxError::InvalidPackage)?,
                            true,
                        )?;
                        if decoded.len() > MAX_TEXT_BYTES {
                            return Err(XlsxError::LimitExceeded);
                        }
                        strings.push(decoded);
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

type ParsedSheet = (
    Vec<NormalizedCell>,
    Vec<NormalizedStyle>,
    Vec<NormalizedMerge>,
    bool,
);

fn parse_sheet(
    body: &[u8],
    shared: &[String],
    style_table: Option<&super::styles::StyleTable>,
    package: &LoadedPackage,
    total_cells: &mut usize,
    total_text_bytes: &mut usize,
) -> Result<ParsedSheet, XlsxError> {
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
    let mut styles = Vec::new();
    let mut unsupported_style = false;
    let mut merges = Vec::new();
    let mut merge_cells_seen = false;
    let mut in_merge_cells = false;
    let mut declared_merge_count = None;
    let mut addresses = HashSet::new();
    let mut shared_formula_masters = HashMap::<u32, SharedFormulaMaster>::new();
    let mut shared_formula_followers = Vec::<(u32, u32, u32)>::new();
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
                    (2, "sheetData") if root && !data_seen && !merge_cells_seen => {
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
                            style_index: cell_style_index(&attrs, style_table)?,
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
                        let cell = draft.as_mut().ok_or(XlsxError::InvalidPackage)?;
                        if cell.saw_formula {
                            return Err(XlsxError::InvalidPackage);
                        }
                        cell.shared_formula = parse_shared_formula_tag(&attrs)?;
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
                    (2, "mergeCells") if root && data_seen && !in_data && !merge_cells_seen => {
                        let attrs = attributes(&start)?;
                        if attrs.keys().any(|key| key != "count") {
                            return Err(XlsxError::UnsupportedFeature);
                        }
                        declared_merge_count = attrs
                            .get("count")
                            .map(|value| {
                                value
                                    .parse::<usize>()
                                    .map_err(|_| XlsxError::InvalidPackage)
                            })
                            .transpose()?;
                        if declared_merge_count.is_some_and(|count| count > 10_000) {
                            return Err(XlsxError::LimitExceeded);
                        }
                        merge_cells_seen = true;
                        in_merge_cells = true;
                    }
                    (3, "mergeCell") if in_merge_cells => {
                        let attrs = attributes(&start)?;
                        if attrs.len() != 1 {
                            return Err(XlsxError::UnsupportedFeature);
                        }
                        let reference = attrs.get("ref").ok_or(XlsxError::InvalidPackage)?;
                        if merges.len() >= 10_000 {
                            return Err(XlsxError::LimitExceeded);
                        }
                        merges.push(parse_merge_reference(reference)?);
                    }
                    _ if in_merge_cells => return Err(XlsxError::UnsupportedFeature),
                    _ if draft.is_some() || in_data => return Err(XlsxError::UnsupportedFeature),
                    _ => {}
                }
            }
            Event::Empty(start) => {
                if depth == 4 && start.name().as_ref() == "f" && draft.is_some() {
                    let attrs = attributes(&start)?;
                    let cell = draft.as_mut().ok_or(XlsxError::InvalidPackage)?;
                    if cell.saw_formula {
                        return Err(XlsxError::InvalidPackage);
                    }
                    cell.shared_formula = parse_shared_formula_tag(&attrs)?;
                    cell.saw_formula = true;
                } else if depth == 3 && start.name().as_ref() == "c" {
                    let attrs = attributes(&start)?;
                    let (row, column) = parse_a1(attrs.get("r").ok_or(XlsxError::InvalidPackage)?)?;
                    if Some(row) != current_row || !addresses.insert((row, column)) {
                        return Err(XlsxError::InvalidPackage);
                    }
                    count_cell(total_cells)?;
                    if let Some(style_index) = cell_style_index(&attrs, style_table)? {
                        append_style(
                            style_table,
                            style_index,
                            row,
                            column,
                            &mut styles,
                            &mut unsupported_style,
                        )?;
                    }
                } else if depth == 1 && start.name().as_ref() == "sheetData" {
                    if data_seen {
                        return Err(XlsxError::InvalidPackage);
                    }
                    data_seen = true;
                } else if depth == 1
                    && start.name().as_ref() == "mergeCells"
                    && data_seen
                    && !merge_cells_seen
                {
                    let attrs = attributes(&start)?;
                    if attrs.len() != 1 || attrs.get("count").is_none_or(|count| count != "0") {
                        return Err(XlsxError::InvalidPackage);
                    }
                    merge_cells_seen = true;
                    declared_merge_count = Some(0);
                } else if depth == 2 && start.name().as_ref() == "mergeCell" && in_merge_cells {
                    let attrs = attributes(&start)?;
                    if attrs.len() != 1 {
                        return Err(XlsxError::UnsupportedFeature);
                    }
                    if merges.len() >= 10_000 {
                        return Err(XlsxError::LimitExceeded);
                    }
                    merges.push(parse_merge_reference(
                        attrs.get("ref").ok_or(XlsxError::InvalidPackage)?,
                    )?);
                } else if draft.is_some() || in_data || in_merge_cells {
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
                        if let Some(style_index) = cell.style_index {
                            append_style(
                                style_table,
                                style_index,
                                cell.row,
                                cell.column,
                                &mut styles,
                                &mut unsupported_style,
                            )?;
                        }
                        if let Some(value) = finish_cell(&cell, shared)? {
                            match value {
                                ParsedCellValue::SharedFollower(index) => {
                                    shared_formula_followers.push((cell.row, cell.column, index));
                                }
                                ParsedCellValue::Value(value) => {
                                    let bytes = match &value {
                                        NormalizedCellValue::Text(value)
                                        | NormalizedCellValue::Formula(value) => value.len(),
                                        NormalizedCellValue::FormulaCopy { .. }
                                        | NormalizedCellValue::Number(_)
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
                                    if let Some(tag) = cell.shared_formula {
                                        let range = tag.range.ok_or(XlsxError::InvalidPackage)?;
                                        if (range.first_row, range.first_column)
                                            != (cell.row, cell.column)
                                            || shared_formula_masters
                                                .insert(
                                                    tag.index,
                                                    SharedFormulaMaster {
                                                        row: cell.row,
                                                        column: cell.column,
                                                        range,
                                                    },
                                                )
                                                .is_some()
                                        {
                                            return Err(XlsxError::InvalidPackage);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    (3, "row") => current_row = None,
                    (2, "sheetData") => in_data = false,
                    (2, "mergeCells") => {
                        if declared_merge_count.is_some_and(|count| count != merges.len()) {
                            return Err(XlsxError::InvalidPackage);
                        }
                        in_merge_cells = false;
                    }
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
    for (row, column, index) in shared_formula_followers {
        let master = shared_formula_masters
            .get(&index)
            .ok_or(XlsxError::InvalidPackage)?;
        if (row, column) == (master.row, master.column)
            || row < master.range.first_row
            || row > master.range.last_row
            || column < master.range.first_column
            || column > master.range.last_column
        {
            return Err(XlsxError::InvalidPackage);
        }
        cells.push(NormalizedCell {
            row,
            column,
            value: NormalizedCellValue::FormulaCopy {
                master_row: master.row,
                master_column: master.column,
            },
        });
    }
    cells.sort_unstable_by_key(|cell| (cell.row, cell.column));
    styles.sort_unstable_by_key(|style| (style.row, style.column));
    merges.sort_unstable();
    Ok((cells, styles, merges, unsupported_style))
}

fn parse_merge_reference(reference: &str) -> Result<NormalizedMerge, XlsxError> {
    let (first, last) = reference.split_once(':').ok_or(XlsxError::InvalidPackage)?;
    if last.contains(':') {
        return Err(XlsxError::InvalidPackage);
    }
    let (first_row, first_column) = parse_a1(first)?;
    let (last_row, last_column) = parse_a1(last)?;
    if first_row > last_row || first_column > last_column {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(NormalizedMerge {
        first_row,
        last_row,
        first_column,
        last_column,
    })
}

fn cell_style_index(
    attrs: &HashMap<String, String>,
    style_table: Option<&super::styles::StyleTable>,
) -> Result<Option<usize>, XlsxError> {
    let Some(table) = style_table else {
        return if attrs.contains_key("s") {
            Err(XlsxError::InvalidPackage)
        } else {
            Ok(None)
        };
    };
    let index = attrs.get("s").map_or(Ok(0), |raw| {
        raw.parse::<usize>().map_err(|_| XlsxError::InvalidPackage)
    })?;
    if index >= table.entries.len() {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(Some(index))
}

fn append_style(
    style_table: Option<&super::styles::StyleTable>,
    index: usize,
    row: u32,
    column: u32,
    styles: &mut Vec<NormalizedStyle>,
    unsupported: &mut bool,
) -> Result<(), XlsxError> {
    let entry = style_table
        .and_then(|table| table.entries.get(index))
        .ok_or(XlsxError::InvalidPackage)?;
    *unsupported |= entry.unsupported;
    if let Some(mut style) = entry.style.clone() {
        style.row = row;
        style.column = column;
        if styles.len() >= MAX_CELLS {
            return Err(XlsxError::LimitExceeded);
        }
        styles.push(style);
    }
    Ok(())
}

fn count_cell(total_cells: &mut usize) -> Result<(), XlsxError> {
    *total_cells = total_cells.checked_add(1).ok_or(XlsxError::LimitExceeded)?;
    if *total_cells > MAX_CELLS {
        return Err(XlsxError::LimitExceeded);
    }
    Ok(())
}

fn parse_shared_formula_tag(
    attrs: &HashMap<String, String>,
) -> Result<Option<SharedFormulaTag>, XlsxError> {
    if attrs.is_empty() {
        return Ok(None);
    }
    if attrs.get("t").is_none_or(|kind| kind != "shared")
        || attrs
            .keys()
            .any(|key| !matches!(key.as_str(), "t" | "si" | "ref"))
    {
        return Err(XlsxError::UnsupportedFeature);
    }
    let index = attrs
        .get("si")
        .ok_or(XlsxError::InvalidPackage)?
        .parse::<u32>()
        .map_err(|_| XlsxError::InvalidPackage)?;
    let range = attrs
        .get("ref")
        .map(|value| parse_merge_reference(value))
        .transpose()?;
    if range.is_none() && attrs.contains_key("ref") {
        return Err(XlsxError::InvalidPackage);
    }
    Ok(Some(SharedFormulaTag { index, range }))
}

fn finish_cell(cell: &CellDraft, shared: &[String]) -> Result<Option<ParsedCellValue>, XlsxError> {
    if cell.saw_formula {
        if cell.kind == "inlineStr" || cell.kind == "s" || cell.saw_inline {
            return Err(XlsxError::InvalidPackage);
        }
        if let Some(tag) = &cell.shared_formula {
            if cell.formula.is_empty() {
                if tag.range.is_some() {
                    return Err(XlsxError::InvalidPackage);
                }
                return Ok(Some(ParsedCellValue::SharedFollower(tag.index)));
            }
            if tag.range.is_none() {
                return Err(XlsxError::InvalidPackage);
            }
        } else if cell.formula.is_empty() {
            return Err(XlsxError::InvalidPackage);
        }
        let formula = decode_ooxml_text(&cell.formula, false)?;
        if formula.len() > MAX_FORMULA_BYTES {
            return Err(XlsxError::LimitExceeded);
        }
        return Ok(Some(ParsedCellValue::Value(NormalizedCellValue::Formula(
            formula,
        ))));
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
            let text = decode_ooxml_text(&cell.inline_text, true)?;
            if text.len() > MAX_TEXT_BYTES {
                return Err(XlsxError::LimitExceeded);
            }
            NormalizedCellValue::Text(text)
        }
        "" | "n" if !cell.saw_value && !cell.saw_inline => return Ok(None),
        _ => return Err(XlsxError::UnsupportedFeature),
    };
    Ok(Some(ParsedCellValue::Value(value)))
}

/// Decodes SpreadsheetML's `_xHHHH_` escapes after XML entities have been decoded.
/// `_x005F_` quotes the following underscore, so its tail is deliberately left literal.
fn decode_ooxml_text(input: &str, normalize_newlines: bool) -> Result<String, XlsxError> {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut offset = 0_usize;
    while offset < bytes.len() {
        if bytes.get(offset..offset.saturating_add(2)) == Some(b"_x") {
            let token_end = offset.checked_add(7).ok_or(XlsxError::InvalidPackage)?;
            if let Some(token) = input.get(offset..token_end)
                && token.as_bytes().get(6) == Some(&b'_')
                && token
                    .as_bytes()
                    .get(2..6)
                    .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
            {
                let high = parse_ooxml_escape(token)?;
                if high == 0x005f {
                    output.push('_');
                    offset = token_end;
                    continue;
                }
                let codepoint = if (0xd800..=0xdbff).contains(&high) {
                    let next_end = token_end.checked_add(7).ok_or(XlsxError::InvalidPackage)?;
                    let next = input
                        .get(token_end..next_end)
                        .ok_or(XlsxError::InvalidPackage)?;
                    let low = parse_ooxml_escape(next)?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return Err(XlsxError::InvalidPackage);
                    }
                    offset = next_end;
                    0x10000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
                } else {
                    if (0xdc00..=0xdfff).contains(&high) {
                        return Err(XlsxError::InvalidPackage);
                    }
                    offset = token_end;
                    u32::from(high)
                };
                output.push(char::from_u32(codepoint).ok_or(XlsxError::InvalidPackage)?);
                continue;
            }
        }
        let character = input
            .get(offset..)
            .and_then(|tail| tail.chars().next())
            .ok_or(XlsxError::InvalidPackage)?;
        output.push(character);
        offset = offset
            .checked_add(character.len_utf8())
            .ok_or(XlsxError::InvalidPackage)?;
    }
    if normalize_newlines {
        let mut normalized = String::with_capacity(output.len());
        let mut characters = output.chars().peekable();
        while let Some(character) = characters.next() {
            if character == '\r' {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                normalized.push('\n');
            } else {
                normalized.push(character);
            }
        }
        Ok(normalized)
    } else {
        Ok(output)
    }
}

fn parse_ooxml_escape(token: &str) -> Result<u16, XlsxError> {
    if token.len() != 7 || !token.starts_with("_x") || !token.ends_with('_') {
        return Err(XlsxError::InvalidPackage);
    }
    u16::from_str_radix(token.get(2..6).ok_or(XlsxError::InvalidPackage)?, 16)
        .map_err(|_| XlsxError::InvalidPackage)
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
