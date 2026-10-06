//! Authored sheet presentation metadata and atomic sheet duplication.
use crate::formula::{BoundAst, Formula, Node};
use crate::workbook::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, PartialEq, Eq)]
pub struct SheetSettings {
    pub name: String,
    pub hidden: bool,
    pub default_column_width: u32,
    pub tab_color: Option<u32>,
    pub hidden_rows: BTreeSet<RowId>,
    pub hidden_columns: BTreeSet<ColumnId>,
}
impl std::fmt::Debug for SheetSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SheetSettings")
            .field("hidden", &self.hidden)
            .finish_non_exhaustive()
    }
}
impl Default for SheetSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            hidden: false,
            default_column_width: 112,
            tab_color: None,
            hidden_rows: BTreeSet::new(),
            hidden_columns: BTreeSet::new(),
        }
    }
}
#[derive(Clone, Default, PartialEq)]
pub struct SheetSettingsPatch {
    pub name: Option<String>,
    pub index: Option<u32>,
    pub hidden: Option<bool>,
    pub default_column_width: Option<u32>,
    pub tab_color: Option<Option<u32>>,
    pub visibility: Vec<(AxisSizeTarget, bool)>,
}
impl std::fmt::Debug for SheetSettingsPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SheetSettingsPatch([redacted])")
    }
}
pub fn validate(sheet: &Sheet, settings: &SheetSettings) -> Result<(), DomainError> {
    if settings.name.is_empty()
        || settings.name.chars().count() > 31
        || !(24..=2000).contains(&settings.default_column_width)
        || settings.tab_color.is_some_and(|c| c > 0xffffff)
        || settings
            .hidden_rows
            .iter()
            .filter(|id| sheet.row_positions.contains_key(id))
            .count()
            >= sheet.rows.len()
        || settings
            .hidden_columns
            .iter()
            .filter(|id| sheet.column_positions.contains_key(id))
            .count()
            >= sheet.columns.len()
    {
        return Err(DomainError::InvalidAxis);
    }
    if settings.hidden_rows.len() > MAX_RETIRED_ROWS
        || settings.hidden_columns.len() > MAX_RETIRED_COLUMNS
    {
        return Err(DomainError::LimitExceeded);
    }
    for range in &sheet.merges {
        let (top, bottom, left, right) = sheet.merge_bounds(*range)?;
        if settings.hidden_rows.iter().any(|id| {
            sheet
                .row_position(*id)
                .is_some_and(|p| (top..=bottom).contains(&p))
        }) || settings.hidden_columns.iter().any(|id| {
            sheet
                .column_position(*id)
                .is_some_and(|p| (left..=right).contains(&p))
        }) {
            return Err(DomainError::MergeConflict);
        }
    }
    Ok(())
}
pub fn prepare(
    book: &Workbook,
    id: SheetId,
    patch: &SheetSettingsPatch,
) -> Result<Operation, DomainError> {
    let index = book
        .sheets
        .iter()
        .position(|s| s.id == id)
        .ok_or(DomainError::MissingSheet)?;
    let target = &book.sheets[index];
    let previous = target.settings.clone();
    let mut settings = previous.clone();
    if let Some(name) = &patch.name {
        if name.trim().is_empty()
            || name
                .chars()
                .any(|c| c.is_control() || "[]:*?/\\".contains(c))
        {
            return Err(DomainError::InvalidAxis);
        }
        settings.name = name.clone();
    }
    if let Some(v) = patch.hidden {
        settings.hidden = v;
    }
    if let Some(v) = patch.default_column_width {
        settings.default_column_width = v;
    }
    if let Some(v) = patch.tab_color {
        settings.tab_color = v;
    }
    if patch.visibility.len() > 1000 {
        return Err(DomainError::LimitExceeded);
    }
    let mut seen = BTreeSet::new();
    for (axis, hidden) in &patch.visibility {
        if match axis {
            AxisSizeTarget::Row(id) => !target.row_positions.contains_key(id),
            AxisSizeTarget::Column(id) => !target.column_positions.contains_key(id),
        } {
            return Err(DomainError::InvalidAxis);
        }
        if !seen.insert(*axis) {
            return Err(DomainError::InvalidAxis);
        }
        match axis {
            AxisSizeTarget::Row(id) => {
                if *hidden {
                    settings.hidden_rows.insert(*id);
                } else {
                    settings.hidden_rows.remove(id);
                }
            }
            AxisSizeTarget::Column(id) => {
                if *hidden {
                    settings.hidden_columns.insert(*id);
                } else {
                    settings.hidden_columns.remove(id);
                }
            }
        }
    }
    Ok(Operation::SheetSettingsWrite {
        sheet: id,
        previous,
        settings,
        previous_index: index as u32,
        index: patch.index.unwrap_or(index as u32),
    })
}
pub(crate) fn candidate(
    book: &Workbook,
    op: &Operation,
    version: DocumentVersion,
) -> Result<Workbook, DomainError> {
    match op {
        Operation::SheetSettingsWrite {
            sheet,
            previous,
            settings,
            previous_index,
            index,
        } => {
            let actual = book
                .sheets
                .iter()
                .position(|s| s.id == *sheet)
                .ok_or(DomainError::MissingSheet)?;
            if actual as u32 != *previous_index || book.sheets[actual].settings != *previous {
                return Err(DomainError::PreimageConflict);
            }
            validate(&book.sheets[actual], settings)?;
            if settings.hidden_rows.iter().any(|id| {
                !book.sheets[actual].row_positions.contains_key(id)
                    && !book.retired_row_ids.contains(id)
            }) || settings.hidden_columns.iter().any(|id| {
                !book.sheets[actual].column_positions.contains_key(id)
                    && !book.retired_column_ids.contains(id)
            }) {
                return Err(DomainError::InvalidAxis);
            }
            if *index as usize >= book.sheets.len()
                || (settings == previous && index == previous_index)
                || book.sheets.iter().any(|s| {
                    s.id != *sheet && s.name.to_lowercase() == settings.name.to_lowercase()
                })
                || (settings.hidden
                    && book
                        .sheets
                        .iter()
                        .filter(|s| s.id != *sheet)
                        .all(|s| s.settings.hidden))
            {
                return Err(DomainError::InvalidAxis);
            }
            let mut next = book.clone();
            let mut target = next.sheets.remove(actual);
            target.name = settings.name.clone();
            target.settings = settings.clone();
            next.sheets.insert(*index as usize, target);
            next.version = version;
            Ok(next)
        }
        Operation::CopySheet {
            source,
            sheet,
            index,
        } => {
            let current = book
                .sheets
                .iter()
                .find(|s| s.id == *source)
                .ok_or(DomainError::MissingSheet)?;
            if sheet.name.trim().is_empty()
                || sheet
                    .name
                    .chars()
                    .any(|c| c.is_control() || "[]:*?/\\".contains(c))
                || sheet.rows.len() != current.rows.len()
                || sheet.columns.len() != current.columns.len()
                || *index as usize > book.sheets.len()
            {
                return Err(DomainError::InvalidAxis);
            }
            let mut next = book.candidate(
                &Operation::AddSheet {
                    sheet: sheet.clone(),
                },
                version,
            )?;
            let target = next.sheets.last_mut().ok_or(DomainError::MissingSheet)?;
            let rows: BTreeMap<_, _> = current
                .rows
                .iter()
                .copied()
                .zip(sheet.rows.iter().copied())
                .collect();
            let columns: BTreeMap<_, _> = current
                .columns
                .iter()
                .copied()
                .zip(sheet.columns.iter().copied())
                .collect();
            let address = |a: CellAddress| -> Result<CellAddress, DomainError> {
                Ok(CellAddress {
                    sheet: sheet.id,
                    row: *rows.get(&a.row).ok_or(DomainError::MissingRow)?,
                    column: *columns.get(&a.column).ok_or(DomainError::MissingColumn)?,
                })
            };
            let reference =
                |r: &mut crate::formula::BoundCellReference| -> Result<(), DomainError> {
                    if r.target_sheet == *source {
                        r.target_sheet = sheet.id;
                        if let Some(id) = rows.get(&r.target_row) {
                            r.target_row = *id;
                        } else if !book.retired_row_ids.contains(&r.target_row) {
                            return Err(DomainError::InvalidFormula);
                        }
                        if let Some(id) = columns.get(&r.target_column) {
                            r.target_column = *id;
                        } else if !book.retired_column_ids.contains(&r.target_column) {
                            return Err(DomainError::InvalidFormula);
                        }
                    }
                    Ok(())
                };
            for (a, c) in &current.cells {
                let a = address(*a)?;
                let c = match c {
                    CellContent::Value(v) => CellContent::Value(v.clone()),
                    CellContent::Formula(f) => {
                        let mut nodes = f.ast().nodes().to_vec();
                        for node in &mut nodes {
                            match node {
                                Node::Cell(r) => reference(r)?,
                                Node::Range(r) => {
                                    reference(&mut r.start)?;
                                    reference(&mut r.end)?;
                                }
                                _ => {}
                            }
                        }
                        let ast = BoundAst::new(f.ast().schema_version(), f.ast().root(), nodes)
                            .map_err(|_| DomainError::InvalidFormula)?;
                        CellContent::Formula(
                            Formula::new(f.source().into(), f.canonical().into(), ast, a, version)
                                .map_err(|_| DomainError::InvalidFormula)?,
                        )
                    }
                };
                target.cells.insert(a, c);
            }
            target.styles = current
                .styles
                .iter()
                .map(|(a, s)| Ok((address(*a)?, s.clone())))
                .collect::<Result<_, DomainError>>()?;
            target.row_heights = current
                .row_heights
                .iter()
                .map(|(id, v)| Ok((*rows.get(id).ok_or(DomainError::MissingRow)?, *v)))
                .collect::<Result<_, DomainError>>()?;
            target.column_widths = current
                .column_widths
                .iter()
                .map(|(id, v)| Ok((*columns.get(id).ok_or(DomainError::MissingColumn)?, *v)))
                .collect::<Result<_, DomainError>>()?;
            target.merges = current
                .merges
                .iter()
                .map(|r| {
                    Ok(crate::merge::MergeRange::new(
                        *rows.get(&r.first_row()).ok_or(DomainError::MissingRow)?,
                        *rows.get(&r.last_row()).ok_or(DomainError::MissingRow)?,
                        *columns
                            .get(&r.first_column())
                            .ok_or(DomainError::MissingColumn)?,
                        *columns
                            .get(&r.last_column())
                            .ok_or(DomainError::MissingColumn)?,
                    ))
                })
                .collect::<Result<_, DomainError>>()?;
            target.frozen_rows = current.frozen_rows;
            target.settings = current.settings.clone();
            target.settings.name = sheet.name.clone();
            target.settings.hidden = false;
            target.settings.hidden_rows = current
                .settings
                .hidden_rows
                .iter()
                .filter_map(|id| rows.get(id).copied())
                .collect();
            target.settings.hidden_columns = current
                .settings
                .hidden_columns
                .iter()
                .filter_map(|id| columns.get(id).copied())
                .collect();
            next.last_axis_change_version = version;
            let copied = next.sheets.pop().ok_or(DomainError::MissingSheet)?;
            next.sheets.insert(*index as usize, copied);
            // Hydration independently validates aggregate budgets, identities and bound references.
            Workbook::hydrate_snapshot(next.snapshot_image()?)
        }
        _ => Err(DomainError::InvalidAxis),
    }
}
pub(crate) fn hash(book: &Workbook, hash: &mut Sha256) {
    if book.sheets.iter().all(|s| {
        !s.settings.hidden
            && s.settings.default_column_width == 112
            && s.settings.tab_color.is_none()
            && s.settings.hidden_rows.is_empty()
            && s.settings.hidden_columns.is_empty()
    }) {
        return;
    }
    hash.update(b"PXEL-SHEET-SETTINGS-1\0");
    for sheet in &book.sheets {
        let s = &sheet.settings;
        hash.update(sheet.id.as_bytes());
        hash.update([u8::from(s.hidden)]);
        hash.update(s.default_column_width.to_be_bytes());
        match s.tab_color {
            Some(v) => {
                hash.update([1]);
                hash.update(v.to_be_bytes());
            }
            None => hash.update([0]),
        }
        hash.update((s.hidden_rows.len() as u64).to_be_bytes());
        for id in &s.hidden_rows {
            hash.update(id.as_bytes());
        }
        hash.update((s.hidden_columns.len() as u64).to_be_bytes());
        for id in &s.hidden_columns {
            hash.update(id.as_bytes());
        }
    }
}
