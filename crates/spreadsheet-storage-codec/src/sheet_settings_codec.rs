use super::*;
use spreadsheet_domain::workbook::{CellShiftAxis, SheetSettings, SheetSettingsPatch};
pub(super) fn encode(s: &SheetSettings) -> v1::SheetSettings {
    v1::SheetSettings {
        hidden: s.hidden,
        default_column_width: s.default_column_width,
        tab_color: s.tab_color,
        hidden_rows: s
            .hidden_rows
            .iter()
            .map(|id| id.as_bytes().to_vec())
            .collect(),
        hidden_columns: s
            .hidden_columns
            .iter()
            .map(|id| id.as_bytes().to_vec())
            .collect(),
    }
}
pub(super) fn decode(s: v1::SheetSettings, name: String) -> Result<SheetSettings, CodecError> {
    let rows = s.hidden_rows.len();
    let columns = s.hidden_columns.len();
    let result = SheetSettings {
        name,
        hidden: s.hidden,
        default_column_width: s.default_column_width,
        tab_color: s.tab_color,
        hidden_rows: s
            .hidden_rows
            .iter()
            .map(|v| RowId::from_bytes(id(v)?).map_err(|_| CodecError::Invalid))
            .collect::<Result<_, _>>()?,
        hidden_columns: s
            .hidden_columns
            .iter()
            .map(|v| ColumnId::from_bytes(id(v)?).map_err(|_| CodecError::Invalid))
            .collect::<Result<_, _>>()?,
    };
    if result.hidden_rows.len() != rows
        || result.hidden_columns.len() != columns
        || !(24..=2000).contains(&result.default_column_width)
        || result.tab_color.is_some_and(|v| v > 0xffffff)
    {
        return Err(CodecError::Invalid);
    }
    Ok(result)
}
pub(super) fn hash_patch(h: &mut Sha256, p: &SheetSettingsPatch) {
    h.update([u8::from(p.name.is_some())]);
    if let Some(v) = &p.name {
        field(h, v.as_bytes());
    }
    h.update([u8::from(p.index.is_some())]);
    if let Some(v) = p.index {
        h.update(v.to_be_bytes());
    }
    h.update([u8::from(p.hidden.is_some())]);
    if let Some(v) = p.hidden {
        h.update([u8::from(v)]);
    }
    h.update([u8::from(p.default_column_width.is_some())]);
    if let Some(v) = p.default_column_width {
        h.update(v.to_be_bytes());
    }
    h.update([u8::from(p.tab_color.is_some())]);
    if let Some(v) = p.tab_color {
        h.update([u8::from(v.is_some())]);
        if let Some(v) = v {
            h.update(v.to_be_bytes());
        }
    }
    h.update((p.visibility.len() as u64).to_be_bytes());
    for (target, hidden) in &p.visibility {
        match target {
            spreadsheet_domain::workbook::AxisSizeTarget::Row(id) => {
                h.update([1]);
                h.update(id.as_bytes());
            }
            spreadsheet_domain::workbook::AxisSizeTarget::Column(id) => {
                h.update([2]);
                h.update(id.as_bytes());
            }
        }
        h.update([u8::from(*hidden)]);
    }
}
pub(super) fn shift(
    source: CellAddress,
    rows: u32,
    columns: u32,
    axis: CellShiftAxis,
    insert: bool,
) -> v1::ShiftCellsIntent {
    v1::ShiftCellsIntent {
        source: Some(addr(&source)),
        rows,
        columns,
        axis_kind: match axis {
            CellShiftAxis::Row => 1,
            CellShiftAxis::Column => 2,
        },
        insert,
    }
}
pub(super) fn decode_shift(v: v1::ShiftCellsIntent) -> Result<CellBatchIntent, CodecError> {
    if v.rows == 0 || v.columns == 0 {
        return Err(CodecError::Invalid);
    }
    Ok(CellBatchIntent::ShiftCells {
        source: decode_addr(v.source.ok_or(CodecError::Invalid)?)?,
        rows: v.rows,
        columns: v.columns,
        axis: match v.axis_kind {
            1 => CellShiftAxis::Row,
            2 => CellShiftAxis::Column,
            _ => return Err(CodecError::Invalid),
        },
        insert: v.insert,
    })
}

pub(super) fn validate_operation(op: &Operation) -> Result<(), CodecError> {
    if let Operation::SheetSettingsWrite {
        previous,
        settings,
        previous_index,
        index,
        ..
    } = op
    {
        for s in [previous, settings] {
            if s.name.is_empty()
                || s.name.chars().count() > 31
                || !(24..=2000).contains(&s.default_column_width)
                || s.tab_color.is_some_and(|v| v > 0xffffff)
                || s.hidden_rows.len() > spreadsheet_domain::workbook::MAX_RETIRED_ROWS
                || s.hidden_columns.len() > spreadsheet_domain::workbook::MAX_RETIRED_COLUMNS
            {
                return Err(CodecError::Invalid);
            }
        }
        if *previous_index >= 64
            || *index >= 64
            || (previous == settings && previous_index == index)
        {
            return Err(CodecError::Invalid);
        }
    }
    if let Operation::CopySheet { sheet, index, .. } = op
        && (*index >= 64
            || sheet.rows.is_empty()
            || sheet.columns.is_empty()
            || sheet.rows.len() > spreadsheet_domain::workbook::MAX_ROWS
            || sheet.columns.len() > spreadsheet_domain::workbook::MAX_COLUMNS
            || sheet.name.trim().is_empty()
            || sheet.name.chars().count() > 31)
    {
        return Err(CodecError::Invalid);
    }
    Ok(())
}
