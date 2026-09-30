//! Versioned durable envelope. Hashes use explicit semantic encoding, never protobuf bytes.
mod formula_codec;
mod move_codec;
mod move_compensation_codec;
#[cfg(test)]
mod move_compensation_tests;
#[cfg(test)]
mod move_tests;
mod range_codec;
#[cfg(test)]
mod range_tests;
mod snapshot_codec;
use prost::Message;
use sha2::{Digest, Sha256};
pub use snapshot_codec::{MAX_SNAPSHOT_BYTES, decode_snapshot, encode_snapshot};
use spreadsheet_domain::workbook::*;
use thiserror::Error;
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/pxel.storage.v1.rs"));
}
#[derive(Debug, Error)]
pub enum CodecError {
    #[error("unsupported event schema")]
    UnsupportedSchema,
    #[error("durable event exceeds the maximum encoded size")]
    EventTooLarge,
    #[error("snapshot exceeds the maximum encoded size")]
    SnapshotTooLarge,
    #[error("invalid durable event")]
    Invalid,
}
fn id<const N: usize>(b: &[u8]) -> Result<[u8; N], CodecError> {
    b.try_into().map_err(|_| CodecError::Invalid)
}
fn addr(a: &CellAddress) -> v1::CellAddress {
    v1::CellAddress {
        sheet_id: a.sheet.as_bytes().to_vec(),
        row_id: a.row.as_bytes().to_vec(),
        column_id: a.column.as_bytes().to_vec(),
    }
}
fn decode_addr(a: v1::CellAddress) -> Result<CellAddress, CodecError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes(id(&a.sheet_id)?).map_err(|_| CodecError::Invalid)?,
        row: RowId::from_bytes(id(&a.row_id)?).map_err(|_| CodecError::Invalid)?,
        column: ColumnId::from_bytes(id(&a.column_id)?).map_err(|_| CodecError::Invalid)?,
    })
}
fn encode_content(content: &CellContent) -> Result<v1::CellContent, CodecError> {
    let kind = match content {
        CellContent::Value(value) => {
            let value = value
                .clone()
                .normalized()
                .map_err(|_| CodecError::Invalid)?;
            v1::cell_content::Kind::Value(v1::CellValue {
                kind: Some(match value {
                    CellValue::Blank => return Err(CodecError::Invalid),
                    CellValue::Number(value) => v1::cell_value::Kind::Number(value),
                    CellValue::Text(value) => v1::cell_value::Kind::Text(value),
                    CellValue::Boolean(value) => v1::cell_value::Kind::Boolean(value),
                }),
            })
        }
        CellContent::Formula(formula) => {
            v1::cell_content::Kind::Formula(formula_codec::encode(formula))
        }
    };
    Ok(v1::CellContent { kind: Some(kind) })
}
fn decode_content(
    content: v1::CellContent,
    address: CellAddress,
) -> Result<CellContent, CodecError> {
    match content.kind.ok_or(CodecError::Invalid)? {
        v1::cell_content::Kind::Value(value) => {
            let value = match value.kind.ok_or(CodecError::Invalid)? {
                v1::cell_value::Kind::Blank(_) => return Err(CodecError::Invalid),
                v1::cell_value::Kind::Number(value) => CellValue::Number(value),
                v1::cell_value::Kind::Text(value) => CellValue::Text(value),
                v1::cell_value::Kind::Boolean(value) => CellValue::Boolean(value),
            };
            Ok(CellContent::Value(
                value.normalized().map_err(|_| CodecError::Invalid)?,
            ))
        }
        v1::cell_content::Kind::Formula(value) => {
            let formula = formula_codec::decode(value)?;
            if formula.origin() != address {
                return Err(CodecError::Invalid);
            }
            Ok(CellContent::Formula(formula))
        }
    }
}
fn encode_optional_content(
    content: &Option<CellContent>,
) -> Result<Option<v1::CellContent>, CodecError> {
    content.as_ref().map(encode_content).transpose()
}
fn hash_content(h: &mut Sha256, content: &Option<CellContent>) -> Result<(), CodecError> {
    match content {
        None => h.update([0]),
        Some(CellContent::Value(value)) => {
            h.update([1]);
            match value
                .clone()
                .normalized()
                .map_err(|_| CodecError::Invalid)?
            {
                CellValue::Blank => return Err(CodecError::Invalid),
                CellValue::Number(value) => {
                    h.update([1]);
                    h.update(value.to_bits().to_le_bytes());
                }
                CellValue::Text(value) => {
                    h.update([2]);
                    field(h, value.as_bytes());
                }
                CellValue::Boolean(value) => h.update([3, u8::from(value)]),
            }
        }
        Some(CellContent::Formula(formula)) => {
            h.update([2]);
            h.update(formula.semantic_hash());
        }
    }
    Ok(())
}
fn validate_copy_writes(writes: &[ContentWrite]) -> Result<(), CodecError> {
    if writes.is_empty()
        || writes.len() > 1_000
        || writes
            .iter()
            .map(|write| write.address)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != writes.len()
        || writes
            .iter()
            .try_fold(0usize, |total, write| {
                total.checked_add(write.content.as_ref().map_or(0, CellContent::text_bytes))
            })
            .ok_or(CodecError::Invalid)?
            > MAX_CELL_VALUE_BATCH_TEXT_BYTES
    {
        return Err(CodecError::Invalid);
    }
    Ok(())
}
fn encode_sheet(sheet: &SheetSeed) -> v1::CreateWorkbook {
    v1::CreateWorkbook {
        sheet_id: sheet.id.as_bytes().to_vec(),
        name: sheet.name.clone(),
        rows: sheet.rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
        columns: sheet
            .columns
            .iter()
            .map(|id| id.as_bytes().to_vec())
            .collect(),
    }
}
fn decode_sheet(sheet: v1::CreateWorkbook) -> Result<SheetSeed, CodecError> {
    Ok(SheetSeed {
        id: SheetId::from_bytes(id(&sheet.sheet_id)?).map_err(|_| CodecError::Invalid)?,
        name: sheet.name,
        rows: sheet
            .rows
            .iter()
            .map(|value| RowId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
            .collect::<Result<_, _>>()?,
        columns: sheet
            .columns
            .iter()
            .map(|value| ColumnId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
            .collect::<Result<_, _>>()?,
    })
}
fn operation(o: &Operation) -> Result<v1::Operation, CodecError> {
    use v1::operation::Kind;
    Ok(v1::Operation {
        kind: Some(match o {
            Operation::CreateWorkbook { sheet } => Kind::CreateWorkbook(encode_sheet(sheet)),
            Operation::AddSheet { sheet } => Kind::AddSheet(encode_sheet(sheet)),
            Operation::ExtendRows { sheet, rows } => Kind::ExtendRows(v1::ExtendRows {
                sheet_id: sheet.as_bytes().to_vec(),
                rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            }),
            Operation::InsertRows {
                sheet,
                before,
                rows,
            } => Kind::InsertRows(v1::InsertRows {
                sheet_id: sheet.as_bytes().to_vec(),
                before_row_id: before.as_bytes().to_vec(),
                rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            }),
            Operation::InsertRowsRebased {
                sheet,
                before,
                rows,
            } => Kind::InsertRowsRebased(v1::InsertRows {
                sheet_id: sheet.as_bytes().to_vec(),
                before_row_id: before.as_bytes().to_vec(),
                rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            }),
            Operation::DeleteRows { sheet, rows } => Kind::DeleteRows(v1::DeleteRows {
                sheet_id: sheet.as_bytes().to_vec(),
                rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            }),
            Operation::InsertColumnsRebased {
                sheet,
                before,
                columns,
            } => Kind::InsertColumnsRebased(v1::InsertColumns {
                sheet_id: sheet.as_bytes().to_vec(),
                before_column_id: before.as_bytes().to_vec(),
                columns: columns.iter().map(|id| id.as_bytes().to_vec()).collect(),
            }),
            Operation::DeleteColumns { sheet, columns } => Kind::DeleteColumns(v1::DeleteColumns {
                sheet_id: sheet.as_bytes().to_vec(),
                columns: columns.iter().map(|id| id.as_bytes().to_vec()).collect(),
            }),
            Operation::SetCellFormula { address, formula } => {
                Kind::SetCellFormula(v1::SetCellFormula {
                    address: Some(addr(address)),
                    formula: Some(formula_codec::encode(formula)),
                })
            }
            Operation::AdvanceCalculationEpoch { context } => {
                Kind::AdvanceCalculationEpoch(v1::AdvanceCalculationEpoch {
                    context_id: context.context_id().as_bytes().to_vec(),
                    calculation_epoch: context.epoch(),
                    effective_timestamp_utc_ms: context.effective_timestamp_utc_ms(),
                    utc_offset_minutes: i32::from(context.utc_offset_minutes()),
                    civil_day_since_unix_epoch: context.civil_day_since_unix_epoch(),
                    random_seed: context.random_seed().to_vec(),
                    engine_version: context.engine_version().into(),
                    compatibility_profile: context.compatibility_profile().into(),
                    reason: context.reason() as i32,
                })
            }
            Operation::ClearCell { address } => Kind::ClearCell(addr(address)),
            Operation::SetCellValue { address, value } => {
                let value = value
                    .clone()
                    .normalized()
                    .map_err(|_| CodecError::Invalid)?;
                Kind::SetCellValue(v1::SetCellValue {
                    address: Some(addr(address)),
                    value: Some(v1::CellValue {
                        kind: Some(match value {
                            CellValue::Blank => v1::cell_value::Kind::Blank(true),
                            CellValue::Number(v) => v1::cell_value::Kind::Number(v),
                            CellValue::Text(v) => v1::cell_value::Kind::Text(v),
                            CellValue::Boolean(v) => v1::cell_value::Kind::Boolean(v),
                        }),
                    }),
                })
            }
            Operation::SetCellValues { writes } => {
                if writes.is_empty()
                    || writes.len() > 1_000
                    || writes
                        .iter()
                        .map(|write| write.address)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != writes.len()
                    || writes
                        .iter()
                        .map(|write| match &write.value {
                            CellValue::Text(value) => value.len(),
                            _ => 0,
                        })
                        .sum::<usize>()
                        > 512 * 1024
                {
                    return Err(CodecError::Invalid);
                }
                Kind::SetCellValues(v1::SetCellValues {
                    writes: writes
                        .iter()
                        .map(|write| {
                            let value = write
                                .value
                                .clone()
                                .normalized()
                                .map_err(|_| CodecError::Invalid)?;
                            Ok(v1::SetCellValue {
                                address: Some(addr(&write.address)),
                                value: Some(v1::CellValue {
                                    kind: Some(match value {
                                        CellValue::Blank => v1::cell_value::Kind::Blank(true),
                                        CellValue::Number(v) => v1::cell_value::Kind::Number(v),
                                        CellValue::Text(v) => v1::cell_value::Kind::Text(v),
                                        CellValue::Boolean(v) => v1::cell_value::Kind::Boolean(v),
                                    }),
                                }),
                            })
                        })
                        .collect::<Result<_, CodecError>>()?,
                })
            }
            Operation::MoveRange { .. } => Kind::MoveRange(move_codec::encode(o)?),
            Operation::MoveCompensation { .. } => {
                Kind::MoveCompensation(move_compensation_codec::encode(o)?)
            }
            Operation::RangeWrite { undoes, .. } => {
                let range = range_codec::encode(o)?;
                if undoes.is_some() {
                    Kind::CompensatingRangeWrite(range)
                } else {
                    Kind::AuthoredRangeWrite(range)
                }
            }
            Operation::CopyRange { writes } => {
                validate_copy_writes(writes)?;
                Kind::CopyRange(v1::CopyRange {
                    writes: writes
                        .iter()
                        .map(|write| {
                            Ok(v1::ContentWrite {
                                address: Some(addr(&write.address)),
                                content: encode_optional_content(&write.content)?,
                            })
                        })
                        .collect::<Result<_, CodecError>>()?,
                })
            }
            Operation::CellWrite {
                address,
                content,
                previous,
                kind,
                undoes,
                undoes_version,
            } => {
                let authored_kind = match (kind, undoes, undoes_version) {
                    (Some(CellWriteKind::SetValue), None, None) => 1,
                    (Some(CellWriteKind::Clear), None, None) => 2,
                    (Some(CellWriteKind::SetFormula), None, None) => 3,
                    (None, Some(_), Some(_)) => 0,
                    _ => return Err(CodecError::Invalid),
                };
                match kind {
                    Some(CellWriteKind::SetValue)
                        if matches!(content, Some(CellContent::Formula(_))) =>
                    {
                        return Err(CodecError::Invalid);
                    }
                    Some(CellWriteKind::Clear) if content.is_some() => {
                        return Err(CodecError::Invalid);
                    }
                    Some(CellWriteKind::SetFormula)
                        if !matches!(content, Some(CellContent::Formula(_))) =>
                    {
                        return Err(CodecError::Invalid);
                    }
                    _ => {}
                }
                let write = v1::CellWrite {
                    address: Some(addr(address)),
                    content: encode_optional_content(content)?,
                    previous: encode_optional_content(previous)?,
                    undoes_operation_id: undoes.map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
                    undoes_version: undoes_version.map_or(0, DocumentVersion::get),
                    authored_kind,
                };
                if undoes.is_some() {
                    Kind::CompensatingCellWrite(write)
                } else {
                    Kind::AuthoredCellWrite(write)
                }
            }
        }),
    })
}
fn decode_operation(o: v1::Operation) -> Result<Operation, CodecError> {
    use v1::operation::Kind;
    Ok(match o.kind.ok_or(CodecError::Invalid)? {
        Kind::MoveRange(moved) => move_codec::decode(moved)?,
        Kind::MoveCompensation(comp) => move_compensation_codec::decode(comp)?,
        Kind::AuthoredRangeWrite(range) => range_codec::decode(range, false)?,
        Kind::CompensatingRangeWrite(range) => range_codec::decode(range, true)?,
        Kind::AdvanceCalculationEpoch(c) => Operation::AdvanceCalculationEpoch {
            context: CalculationContext::new(
                CalculationContextId::from_bytes(id(&c.context_id)?)
                    .map_err(|_| CodecError::Invalid)?,
                c.calculation_epoch,
                c.effective_timestamp_utc_ms,
                i16::try_from(c.utc_offset_minutes).map_err(|_| CodecError::Invalid)?,
                c.civil_day_since_unix_epoch,
                id(&c.random_seed)?,
                c.engine_version,
                c.compatibility_profile,
                match c.reason {
                    1 => CalculationReason::Open,
                    2 => CalculationReason::Explicit,
                    3 => CalculationReason::Timer,
                    4 => CalculationReason::VolatileDependency,
                    5 => CalculationReason::EngineUpgrade,
                    _ => return Err(CodecError::Invalid),
                },
            )
            .map_err(|_| CodecError::Invalid)?,
        },
        Kind::CreateWorkbook(sheet) => Operation::CreateWorkbook {
            sheet: decode_sheet(sheet)?,
        },
        Kind::AddSheet(sheet) => Operation::AddSheet {
            sheet: decode_sheet(sheet)?,
        },
        Kind::ExtendRows(extension) => {
            if extension.rows.is_empty() || extension.rows.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            let rows: Vec<RowId> = extension
                .rows
                .iter()
                .map(|value| RowId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?;
            if rows
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != rows.len()
            {
                return Err(CodecError::Invalid);
            }
            Operation::ExtendRows {
                sheet: SheetId::from_bytes(id(&extension.sheet_id)?)
                    .map_err(|_| CodecError::Invalid)?,
                rows,
            }
        }
        kind @ (Kind::InsertRows(_) | Kind::InsertRowsRebased(_)) => {
            let rebased = matches!(kind, Kind::InsertRowsRebased(_));
            let insertion = match kind {
                Kind::InsertRows(insertion) | Kind::InsertRowsRebased(insertion) => insertion,
                _ => return Err(CodecError::Invalid),
            };
            if insertion.rows.is_empty() || insertion.rows.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            let rows: Vec<RowId> = insertion
                .rows
                .iter()
                .map(|value| RowId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?;
            if rows
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != rows.len()
            {
                return Err(CodecError::Invalid);
            }
            let sheet =
                SheetId::from_bytes(id(&insertion.sheet_id)?).map_err(|_| CodecError::Invalid)?;
            let before = RowId::from_bytes(id(&insertion.before_row_id)?)
                .map_err(|_| CodecError::Invalid)?;
            if rebased {
                Operation::InsertRowsRebased {
                    sheet,
                    before,
                    rows,
                }
            } else {
                Operation::InsertRows {
                    sheet,
                    before,
                    rows,
                }
            }
        }
        Kind::DeleteRows(deletion) => {
            if deletion.rows.is_empty() || deletion.rows.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            let rows: Vec<RowId> = deletion
                .rows
                .iter()
                .map(|value| RowId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?;
            if rows
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != rows.len()
            {
                return Err(CodecError::Invalid);
            }
            Operation::DeleteRows {
                sheet: SheetId::from_bytes(id(&deletion.sheet_id)?)
                    .map_err(|_| CodecError::Invalid)?,
                rows,
            }
        }
        Kind::InsertColumnsRebased(insertion) => {
            if insertion.columns.is_empty() || insertion.columns.len() > 100 {
                return Err(CodecError::Invalid);
            }
            let columns: Vec<ColumnId> = insertion
                .columns
                .iter()
                .map(|value| ColumnId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?;
            if columns
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != columns.len()
            {
                return Err(CodecError::Invalid);
            }
            Operation::InsertColumnsRebased {
                sheet: SheetId::from_bytes(id(&insertion.sheet_id)?)
                    .map_err(|_| CodecError::Invalid)?,
                before: ColumnId::from_bytes(id(&insertion.before_column_id)?)
                    .map_err(|_| CodecError::Invalid)?,
                columns,
            }
        }
        Kind::DeleteColumns(deletion) => {
            if deletion.columns.is_empty() || deletion.columns.len() > 100 {
                return Err(CodecError::Invalid);
            }
            let columns: Vec<ColumnId> = deletion
                .columns
                .iter()
                .map(|value| ColumnId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?;
            if columns
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != columns.len()
            {
                return Err(CodecError::Invalid);
            }
            Operation::DeleteColumns {
                sheet: SheetId::from_bytes(id(&deletion.sheet_id)?)
                    .map_err(|_| CodecError::Invalid)?,
                columns,
            }
        }
        Kind::SetCellFormula(v) => {
            let address = decode_addr(v.address.ok_or(CodecError::Invalid)?)?;
            let formula = formula_codec::decode(v.formula.ok_or(CodecError::Invalid)?)?;
            if formula.origin() != address {
                return Err(CodecError::Invalid);
            }
            Operation::SetCellFormula { address, formula }
        }
        Kind::ClearCell(a) => Operation::ClearCell {
            address: decode_addr(a)?,
        },
        Kind::SetCellValue(v) => Operation::SetCellValue {
            address: decode_addr(v.address.ok_or(CodecError::Invalid)?)?,
            value: match v.value.and_then(|v| v.kind).ok_or(CodecError::Invalid)? {
                v1::cell_value::Kind::Blank(true) => CellValue::Blank,
                v1::cell_value::Kind::Blank(false) => return Err(CodecError::Invalid),
                v1::cell_value::Kind::Number(n) => CellValue::Number(n),
                v1::cell_value::Kind::Text(s) => CellValue::Text(s),
                v1::cell_value::Kind::Boolean(b) => CellValue::Boolean(b),
            }
            .normalized()
            .map_err(|_| CodecError::Invalid)?,
        },
        Kind::SetCellValues(batch) => {
            if batch.writes.is_empty() || batch.writes.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            let writes = batch
                .writes
                .into_iter()
                .map(|write| {
                    let address = decode_addr(write.address.ok_or(CodecError::Invalid)?)?;
                    let value = match write
                        .value
                        .and_then(|v| v.kind)
                        .ok_or(CodecError::Invalid)?
                    {
                        v1::cell_value::Kind::Blank(true) => CellValue::Blank,
                        v1::cell_value::Kind::Blank(false) => return Err(CodecError::Invalid),
                        v1::cell_value::Kind::Number(n) => CellValue::Number(n),
                        v1::cell_value::Kind::Text(s) => CellValue::Text(s),
                        v1::cell_value::Kind::Boolean(b) => CellValue::Boolean(b),
                    }
                    .normalized()
                    .map_err(|_| CodecError::Invalid)?;
                    Ok(CellWrite { address, value })
                })
                .collect::<Result<Vec<_>, CodecError>>()?;
            if writes
                .iter()
                .map(|write| write.address)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != writes.len()
            {
                return Err(CodecError::Invalid);
            }
            Operation::SetCellValues { writes }
        }
        Kind::CopyRange(batch) => {
            if batch.writes.is_empty() || batch.writes.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            let writes = batch
                .writes
                .into_iter()
                .map(|write| {
                    let address = decode_addr(write.address.ok_or(CodecError::Invalid)?)?;
                    let content = write
                        .content
                        .map(|content| decode_content(content, address))
                        .transpose()?;
                    Ok(ContentWrite { address, content })
                })
                .collect::<Result<Vec<_>, CodecError>>()?;
            validate_copy_writes(&writes)?;
            Operation::CopyRange { writes }
        }
        kind @ (Kind::AuthoredCellWrite(_) | Kind::CompensatingCellWrite(_)) => {
            let compensating = matches!(kind, Kind::CompensatingCellWrite(_));
            let write = match kind {
                Kind::AuthoredCellWrite(write) | Kind::CompensatingCellWrite(write) => write,
                _ => return Err(CodecError::Invalid),
            };
            let address = decode_addr(write.address.ok_or(CodecError::Invalid)?)?;
            let content = write
                .content
                .map(|content| decode_content(content, address))
                .transpose()?;
            let previous = write
                .previous
                .map(|content| decode_content(content, address))
                .transpose()?;
            let (kind, undoes, undoes_version) = if compensating {
                if write.authored_kind != 0
                    || write.undoes_version == 0
                    || write.undoes_operation_id.is_empty()
                {
                    return Err(CodecError::Invalid);
                }
                (
                    None,
                    Some(
                        OperationId::from_bytes(id(&write.undoes_operation_id)?)
                            .map_err(|_| CodecError::Invalid)?,
                    ),
                    Some(
                        DocumentVersion::new(write.undoes_version)
                            .map_err(|_| CodecError::Invalid)?,
                    ),
                )
            } else {
                if !write.undoes_operation_id.is_empty() || write.undoes_version != 0 {
                    return Err(CodecError::Invalid);
                }
                (
                    Some(match write.authored_kind {
                        1 => CellWriteKind::SetValue,
                        2 => CellWriteKind::Clear,
                        3 => CellWriteKind::SetFormula,
                        _ => return Err(CodecError::Invalid),
                    }),
                    None,
                    None,
                )
            };
            match kind {
                Some(CellWriteKind::SetValue)
                    if matches!(content, Some(CellContent::Formula(_))) =>
                {
                    return Err(CodecError::Invalid);
                }
                Some(CellWriteKind::Clear) if content.is_some() => {
                    return Err(CodecError::Invalid);
                }
                Some(CellWriteKind::SetFormula)
                    if !matches!(content, Some(CellContent::Formula(_))) =>
                {
                    return Err(CodecError::Invalid);
                }
                _ => {}
            }
            Operation::CellWrite {
                address,
                content,
                previous,
                kind,
                undoes,
                undoes_version,
            }
        }
    })
}
pub fn schema_version(operation: &Operation) -> u32 {
    match operation {
        Operation::MoveCompensation { .. } => 20,
        Operation::MoveRange { .. } => 19,
        Operation::RangeWrite {
            undoes: Some(_), ..
        } => 18,
        Operation::RangeWrite { .. } => 17,
        Operation::CellWrite {
            kind: Some(CellWriteKind::SetFormula),
            ..
        } => 16,
        Operation::AddSheet { .. } => 15,
        Operation::CopyRange { .. } => 14,
        Operation::CellWrite {
            content: Some(CellContent::Formula(_)),
            undoes: Some(_),
            ..
        } => 13,
        Operation::CellWrite {
            undoes: Some(_), ..
        } => 12,
        Operation::CellWrite { .. } => 11,
        Operation::ExtendRows { .. } => 4,
        Operation::InsertRows { .. } => 5,
        Operation::InsertRowsRebased { .. } => 6,
        Operation::DeleteRows { .. } => 7,
        Operation::InsertColumnsRebased { .. } => 8,
        Operation::DeleteColumns { .. } => 9,
        Operation::SetCellValues { .. } => 10,
        Operation::AdvanceCalculationEpoch { .. } => 3,
        Operation::SetCellFormula { .. } => 2,
        _ => 1,
    }
}
pub fn encode(event: &CommittedEvent) -> Result<Vec<u8>, CodecError> {
    move_codec::validate(&event.operation, Some(event.version))?;
    range_codec::validate(&event.operation, Some(event.version))?;
    if let Operation::CellWrite {
        kind: Some(CellWriteKind::SetFormula),
        content,
        previous,
        ..
    } = &event.operation
        && content.iter().chain(previous.iter()).any(|content| {
            matches!(content, CellContent::Formula(formula) if formula.bound_at_version() >= event.version)
        })
    {
        return Err(CodecError::Invalid);
    }
    let (actor_id, principal_kind) = match (event.principal, &event.operation) {
        (EventPrincipal::User(actor), Operation::AdvanceCalculationEpoch { .. }) => {
            let _ = actor;
            return Err(CodecError::Invalid);
        }
        (EventPrincipal::User(actor), _) => (actor.as_bytes().to_vec(), 0),
        (EventPrincipal::System, Operation::AdvanceCalculationEpoch { .. }) => (Vec::new(), 2),
        (EventPrincipal::System, _) => return Err(CodecError::Invalid),
    };
    let envelope = v1::EventEnvelope {
        schema_version: schema_version(&event.operation),
        document_id: event.document_id.as_bytes().to_vec(),
        operation_id: event.operation_id.as_bytes().to_vec(),
        actor_id,
        committed_version: event.version.get(),
        state_hash: event.state_hash.to_vec(),
        operation: Some(operation(&event.operation)?),
        principal_kind,
    };
    encode_envelope(envelope)
}
fn encode_envelope(envelope: v1::EventEnvelope) -> Result<Vec<u8>, CodecError> {
    if envelope.encoded_len() > 4 * 1024 * 1024 {
        return Err(CodecError::EventTooLarge);
    }
    Ok(envelope.encode_to_vec())
}
pub fn decode(bytes: &[u8]) -> Result<CommittedEvent, CodecError> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(CodecError::Invalid);
    }
    let e = v1::EventEnvelope::decode(bytes).map_err(|_| CodecError::Invalid)?;
    if !matches!(e.schema_version, 1..=20) {
        return Err(CodecError::UnsupportedSchema);
    }
    if e.committed_version == 0 {
        return Err(CodecError::Invalid);
    }
    let operation = decode_operation(e.operation.ok_or(CodecError::Invalid)?)?;
    move_codec::validate(
        &operation,
        Some(DocumentVersion::new(e.committed_version).map_err(|_| CodecError::Invalid)?),
    )?;
    range_codec::validate(
        &operation,
        Some(DocumentVersion::new(e.committed_version).map_err(|_| CodecError::Invalid)?),
    )?;
    if schema_version(&operation) != e.schema_version {
        return Err(CodecError::UnsupportedSchema);
    }
    let principal = match (
        e.schema_version,
        e.principal_kind,
        e.actor_id.as_slice(),
        &operation,
    ) {
        (
            1 | 2 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20,
            0,
            actor,
            Operation::AdvanceCalculationEpoch { .. },
        ) => {
            let _ = actor;
            return Err(CodecError::Invalid);
        }
        (
            1 | 2 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20,
            0,
            actor,
            _,
        ) => {
            EventPrincipal::User(ActorId::from_bytes(id(actor)?).map_err(|_| CodecError::Invalid)?)
        }
        (3, 2, [], Operation::AdvanceCalculationEpoch { .. }) => EventPrincipal::System,
        _ => return Err(CodecError::Invalid),
    };
    if matches!(&operation,Operation::SetCellFormula{formula,..} if formula.bound_at_version().get()>=e.committed_version)
    {
        return Err(CodecError::Invalid);
    }
    if let Operation::CellWrite {kind:Some(CellWriteKind::SetFormula),content,previous,..} = &operation
        && content.iter().chain(previous.iter()).any(|content|matches!(content,CellContent::Formula(formula) if formula.bound_at_version().get()>=e.committed_version))
    {
        return Err(CodecError::Invalid);
    }
    if matches!(&operation, Operation::CellWrite { undoes_version: Some(version), .. } if version.get() >= e.committed_version)
    {
        return Err(CodecError::Invalid);
    }
    Ok(CommittedEvent {
        document_id: DocumentId::from_bytes(id(&e.document_id)?)
            .map_err(|_| CodecError::Invalid)?,
        operation_id: OperationId::from_bytes(id(&e.operation_id)?)
            .map_err(|_| CodecError::Invalid)?,
        principal,
        version: DocumentVersion::new(e.committed_version).map_err(|_| CodecError::Invalid)?,
        operation,
        state_hash: id(&e.state_hash)?,
    })
}
pub fn checksum(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn field(h: &mut Sha256, b: &[u8]) {
    h.update((b.len() as u64).to_le_bytes());
    h.update(b)
}
fn semantic_operation(h: &mut Sha256, o: &Operation) -> Result<(), CodecError> {
    match o {
        Operation::AdvanceCalculationEpoch { context } => {
            h.update([4]);
            h.update(context.context_id().as_bytes());
            h.update(context.epoch().to_le_bytes());
            h.update(context.effective_timestamp_utc_ms().to_le_bytes());
            h.update(context.utc_offset_minutes().to_le_bytes());
            h.update(context.civil_day_since_unix_epoch().to_le_bytes());
            h.update(context.random_seed());
            field(h, context.engine_version().as_bytes());
            field(h, context.compatibility_profile().as_bytes());
            h.update([context.reason() as u8]);
        }
        Operation::CreateWorkbook { sheet } => {
            h.update([0]);
            h.update(sheet.id.as_bytes());
            field(h, sheet.name.as_bytes());
            h.update((sheet.rows.len() as u64).to_le_bytes());
            for id in &sheet.rows {
                h.update(id.as_bytes())
            }
            h.update((sheet.columns.len() as u64).to_le_bytes());
            for id in &sheet.columns {
                h.update(id.as_bytes())
            }
        }
        Operation::AddSheet { sheet } => {
            h.update([16]);
            h.update(sheet.id.as_bytes());
            field(h, sheet.name.as_bytes());
            h.update((sheet.rows.len() as u64).to_le_bytes());
            for id in &sheet.rows {
                h.update(id.as_bytes());
            }
            h.update((sheet.columns.len() as u64).to_le_bytes());
            for id in &sheet.columns {
                h.update(id.as_bytes());
            }
        }
        Operation::ExtendRows { sheet, rows } => {
            h.update([5]);
            h.update(sheet.as_bytes());
            h.update((rows.len() as u64).to_le_bytes());
            for row in rows {
                h.update(row.as_bytes());
            }
        }
        Operation::InsertRows {
            sheet,
            before,
            rows,
        }
        | Operation::InsertRowsRebased {
            sheet,
            before,
            rows,
        } => {
            h.update([if matches!(o, Operation::InsertRowsRebased { .. }) {
                7
            } else {
                6
            }]);
            h.update(sheet.as_bytes());
            h.update(before.as_bytes());
            h.update((rows.len() as u64).to_le_bytes());
            for row in rows {
                h.update(row.as_bytes());
            }
        }
        Operation::DeleteRows { sheet, rows } => {
            h.update([8]);
            h.update(sheet.as_bytes());
            h.update((rows.len() as u64).to_le_bytes());
            for row in rows {
                h.update(row.as_bytes());
            }
        }
        Operation::InsertColumnsRebased {
            sheet,
            before,
            columns,
        } => {
            h.update([9]);
            h.update(sheet.as_bytes());
            h.update(before.as_bytes());
            h.update((columns.len() as u64).to_le_bytes());
            for column in columns {
                h.update(column.as_bytes());
            }
        }
        Operation::DeleteColumns { sheet, columns } => {
            h.update([10]);
            h.update(sheet.as_bytes());
            h.update((columns.len() as u64).to_le_bytes());
            for column in columns {
                h.update(column.as_bytes());
            }
        }
        Operation::SetCellFormula { address, formula } => {
            h.update([3]);
            h.update(address.sheet.as_bytes());
            h.update(address.row.as_bytes());
            h.update(address.column.as_bytes());
            h.update(formula.semantic_hash());
        }
        Operation::ClearCell { address } => {
            h.update([1]);
            h.update(address.sheet.as_bytes());
            h.update(address.row.as_bytes());
            h.update(address.column.as_bytes())
        }
        Operation::SetCellValue { address, value } => {
            h.update([2]);
            h.update(address.sheet.as_bytes());
            h.update(address.row.as_bytes());
            h.update(address.column.as_bytes());
            match value
                .clone()
                .normalized()
                .map_err(|_| CodecError::Invalid)?
            {
                CellValue::Blank => h.update([0]),
                CellValue::Number(n) => {
                    h.update([1]);
                    h.update(n.to_bits().to_le_bytes())
                }
                CellValue::Text(s) => {
                    h.update([2]);
                    field(h, s.as_bytes())
                }
                CellValue::Boolean(b) => h.update([3, u8::from(b)]),
            }
        }
        Operation::SetCellValues { writes } => {
            if writes.is_empty() || writes.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            h.update([11]);
            h.update((writes.len() as u64).to_le_bytes());
            for write in writes {
                h.update(write.address.sheet.as_bytes());
                h.update(write.address.row.as_bytes());
                h.update(write.address.column.as_bytes());
                match write
                    .value
                    .clone()
                    .normalized()
                    .map_err(|_| CodecError::Invalid)?
                {
                    CellValue::Blank => h.update([0]),
                    CellValue::Number(n) => {
                        h.update([1]);
                        h.update(n.to_bits().to_le_bytes());
                    }
                    CellValue::Text(s) => {
                        h.update([2]);
                        field(h, s.as_bytes());
                    }
                    CellValue::Boolean(b) => h.update([3, u8::from(b)]),
                }
            }
        }
        Operation::MoveRange { .. } => move_codec::hash(h, o)?,
        Operation::MoveCompensation { .. } => move_compensation_codec::hash(h, o)?,
        Operation::RangeWrite { .. } => range_codec::hash(h, o)?,
        Operation::CopyRange { writes } => {
            validate_copy_writes(writes)?;
            h.update([15]);
            h.update((writes.len() as u64).to_le_bytes());
            for write in writes {
                h.update(write.address.sheet.as_bytes());
                h.update(write.address.row.as_bytes());
                h.update(write.address.column.as_bytes());
                hash_content(h, &write.content)?;
            }
        }
        Operation::CellWrite {
            address,
            content,
            previous,
            kind,
            undoes,
            undoes_version,
        } => {
            let tag = match (kind, undoes, undoes_version) {
                (Some(CellWriteKind::SetValue), None, None) => 12,
                (Some(CellWriteKind::Clear), None, None) => 13,
                (Some(CellWriteKind::SetFormula), None, None) => 17,
                (None, Some(_), Some(_)) => 14,
                _ => return Err(CodecError::Invalid),
            };
            h.update([tag]);
            h.update(address.sheet.as_bytes());
            h.update(address.row.as_bytes());
            h.update(address.column.as_bytes());
            hash_content(h, content)?;
            hash_content(h, previous)?;
            if let (Some(undoes), Some(version)) = (undoes, undoes_version) {
                h.update(undoes.as_bytes());
                h.update(version.get().to_le_bytes());
            }
        }
    }
    Ok(())
}
pub fn command_fingerprint(c: &Command) -> Result<[u8; 32], CodecError> {
    let mut h = Sha256::new();
    h.update(b"PXEL-COMMAND-1");
    h.update(c.document_id.as_bytes());
    h.update(c.operation_id.as_bytes());
    h.update(c.actor_id.as_bytes());
    h.update(c.base_version.get().to_le_bytes());
    h.update(c.client_sequence.to_le_bytes());
    match &c.operation {
        CommandPayload::CreateWorkbook { sheet } => semantic_operation(
            &mut h,
            &Operation::CreateWorkbook {
                sheet: sheet.clone(),
            },
        )?,
        CommandPayload::AddSheet { sheet } => semantic_operation(
            &mut h,
            &Operation::AddSheet {
                sheet: sheet.clone(),
            },
        )?,
        CommandPayload::ExtendRows { sheet, rows } => semantic_operation(
            &mut h,
            &Operation::ExtendRows {
                sheet: *sheet,
                rows: rows.clone(),
            },
        )?,
        CommandPayload::InsertRows {
            sheet,
            before,
            rows,
        } => semantic_operation(
            &mut h,
            &Operation::InsertRows {
                sheet: *sheet,
                before: *before,
                rows: rows.clone(),
            },
        )?,
        CommandPayload::DeleteRows { sheet, rows } => semantic_operation(
            &mut h,
            &Operation::DeleteRows {
                sheet: *sheet,
                rows: rows.clone(),
            },
        )?,
        CommandPayload::InsertColumns {
            sheet,
            before,
            columns,
        } => semantic_operation(
            &mut h,
            &Operation::InsertColumnsRebased {
                sheet: *sheet,
                before: *before,
                columns: columns.clone(),
            },
        )?,
        CommandPayload::DeleteColumns { sheet, columns } => semantic_operation(
            &mut h,
            &Operation::DeleteColumns {
                sheet: *sheet,
                columns: columns.clone(),
            },
        )?,
        CommandPayload::SetCellValue { address, value } => semantic_operation(
            &mut h,
            &Operation::SetCellValue {
                address: *address,
                value: value.clone(),
            },
        )?,
        CommandPayload::SetCellValues { writes } => semantic_operation(
            &mut h,
            &Operation::SetCellValues {
                writes: writes.clone(),
            },
        )?,
        CommandPayload::CopyRange {
            source,
            destination,
            rows,
            columns,
        } => {
            h.update([15]);
            for address in [source, destination] {
                h.update(address.sheet.as_bytes());
                h.update(address.row.as_bytes());
                h.update(address.column.as_bytes());
            }
            h.update(rows.to_le_bytes());
            h.update(columns.to_le_bytes());
        }
        CommandPayload::ClearCell { address } => {
            semantic_operation(&mut h, &Operation::ClearCell { address: *address })?
        }
        CommandPayload::SetCellFormula { address, source } => {
            h.update([3]);
            h.update(address.sheet.as_bytes());
            h.update(address.row.as_bytes());
            h.update(address.column.as_bytes());
            field(&mut h, source.as_str().as_bytes());
        }
        CommandPayload::UndoCell {
            target_operation_id,
        } => {
            h.update([14]);
            h.update(target_operation_id.as_bytes());
        }
        CommandPayload::MoveRange {
            source,
            destination,
            rows,
            columns,
        } => {
            move_codec::validate_geometry(*source, *destination, *rows, *columns)?;
            h.update([17]);
            move_codec::hash_geometry(&mut h, *source, *destination, *rows, *columns);
        }
    }
    Ok(h.finalize().into())
}
pub fn system_fingerprint(e: &CommittedEvent) -> Result<[u8; 32], CodecError> {
    let Operation::AdvanceCalculationEpoch { context } = &e.operation else {
        return Err(CodecError::Invalid);
    };
    if e.principal != EventPrincipal::System {
        return Err(CodecError::Invalid);
    }
    Ok(system_trigger_fingerprint(
        e.document_id,
        e.operation_id,
        context,
    ))
}
/// The same persisted idempotency key without constructing a synthetic event.
pub fn system_trigger_fingerprint(
    document_id: DocumentId,
    operation_id: OperationId,
    context: &CalculationContext,
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"PXEL-SYSTEM-COMMAND-1");
    h.update(document_id.as_bytes());
    h.update(operation_id.as_bytes());
    h.update([context.reason() as u8]);
    if context.reason() != CalculationReason::Explicit {
        h.update(context.context_id().as_bytes());
        h.update(context.effective_timestamp_utc_ms().to_le_bytes());
        h.update(context.utc_offset_minutes().to_le_bytes());
        h.update(context.civil_day_since_unix_epoch().to_le_bytes());
        h.update(context.random_seed());
        field(&mut h, context.engine_version().as_bytes());
        field(&mut h, context.compatibility_profile().as_bytes());
    }
    h.finalize().into()
}
pub fn event_hash(e: &CommittedEvent) -> Result<[u8; 32], CodecError> {
    move_codec::validate(&e.operation, Some(e.version))?;
    range_codec::validate(&e.operation, Some(e.version))?;
    let mut h = Sha256::new();
    match (e.principal, &e.operation) {
        (EventPrincipal::User(_), Operation::AdvanceCalculationEpoch { .. })
        | (EventPrincipal::System, Operation::CreateWorkbook { .. })
        | (EventPrincipal::System, Operation::AddSheet { .. })
        | (EventPrincipal::System, Operation::SetCellValue { .. })
        | (EventPrincipal::System, Operation::SetCellFormula { .. })
        | (EventPrincipal::System, Operation::ExtendRows { .. })
        | (EventPrincipal::System, Operation::InsertRows { .. })
        | (EventPrincipal::System, Operation::InsertRowsRebased { .. })
        | (EventPrincipal::System, Operation::DeleteRows { .. })
        | (EventPrincipal::System, Operation::InsertColumnsRebased { .. })
        | (EventPrincipal::System, Operation::DeleteColumns { .. })
        | (EventPrincipal::System, Operation::SetCellValues { .. })
        | (EventPrincipal::System, Operation::CopyRange { .. })
        | (EventPrincipal::System, Operation::RangeWrite { .. })
        | (EventPrincipal::System, Operation::MoveRange { .. })
        | (EventPrincipal::System, Operation::MoveCompensation { .. })
        | (EventPrincipal::System, Operation::CellWrite { .. })
        | (EventPrincipal::System, Operation::ClearCell { .. }) => return Err(CodecError::Invalid),
        (EventPrincipal::User(_), _) => h.update(b"PXEL-EVENT-1"),
        (EventPrincipal::System, _) => h.update(b"PXEL-EVENT-SYSTEM-1"),
    }
    h.update(e.document_id.as_bytes());
    h.update(e.operation_id.as_bytes());
    if let EventPrincipal::User(actor) = e.principal {
        h.update(actor.as_bytes());
    }
    h.update(e.version.get().to_le_bytes());
    h.update(e.state_hash);
    semantic_operation(&mut h, &e.operation)?;
    Ok(h.finalize().into())
}
pub fn genesis_fingerprint(e: &CommittedEvent) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"PXEL-CREATE-1");
    h.update(e.document_id.as_bytes());
    h.update(e.operation_id.as_bytes());
    if let EventPrincipal::User(actor) = e.principal {
        h.update(actor.as_bytes());
    }
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn system_event() -> Result<CommittedEvent, Box<dyn std::error::Error>> {
        Ok(CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([8; 16])?,
            principal: EventPrincipal::System,
            version: DocumentVersion::new(2)?,
            operation: Operation::AdvanceCalculationEpoch {
                context: CalculationContext::new(
                    CalculationContextId::from_bytes([9; 16])?,
                    1,
                    1_700_000_000_000,
                    180,
                    19_676,
                    [10; 32],
                    "pxel-formula-0.1.0".into(),
                    "pxel-excel-subset-v1".into(),
                    CalculationReason::Explicit,
                )?,
            },
            state_hash: [11; 32],
        })
    }
    #[test]
    fn system_event_schema_is_distinct_and_rejects_forged_principal()
    -> Result<(), Box<dyn std::error::Error>> {
        let event = system_event()?;
        let bytes = encode(&event)?;
        let envelope = v1::EventEnvelope::decode(bytes.as_slice())?;
        assert_eq!(envelope.schema_version, 3);
        assert_eq!(envelope.principal_kind, 2);
        assert!(envelope.actor_id.is_empty());
        assert_eq!(decode(&bytes)?, event);
        let mut retried = event.clone();
        retried.version = DocumentVersion::new(3)?;
        retried.state_hash = [12; 32];
        if let Operation::AdvanceCalculationEpoch { context } = &mut retried.operation {
            *context = context.with_epoch(2)?;
        }
        assert_eq!(system_fingerprint(&retried)?, system_fingerprint(&event)?);
        assert_ne!(event_hash(&retried)?, event_hash(&event)?);
        let mut forged = event.clone();
        forged.principal = EventPrincipal::User(ActorId::from_bytes([13; 16])?);
        assert!(matches!(encode(&forged), Err(CodecError::Invalid)));
        let mut malformed = envelope.clone();
        malformed.actor_id = [13; 16].to_vec();
        assert!(matches!(
            decode(&malformed.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        malformed = envelope.clone();
        malformed.principal_kind = 0;
        assert!(matches!(
            decode(&malformed.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        malformed = envelope;
        if let Some(v1::operation::Kind::AdvanceCalculationEpoch(context)) = malformed
            .operation
            .as_mut()
            .and_then(|operation| operation.kind.as_mut())
        {
            context.reason = 999;
        } else {
            return Err("missing context".into());
        }
        assert!(matches!(
            decode(&malformed.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        Ok(())
    }
    fn event() -> Result<CommittedEvent, Box<dyn std::error::Error>> {
        Ok(CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::SetCellValue {
                address: CellAddress {
                    sheet: SheetId::from_bytes([4; 16])?,
                    row: RowId::from_bytes([5; 16])?,
                    column: ColumnId::from_bytes([6; 16])?,
                },
                value: CellValue::Number(0.0),
            },
            state_hash: [7; 32],
        })
    }
    #[test]
    fn envelope_roundtrip_preserves_semantic_hash() -> Result<(), Box<dyn std::error::Error>> {
        let e = event()?;
        let bytes = encode(&e)?;
        assert_eq!(decode(&bytes)?, e);
        assert_eq!(event_hash(&decode(&bytes)?)?, event_hash(&e)?);
        Ok(())
    }
    #[test]
    fn semantic_hash_ignores_protobuf_encoding_order() -> Result<(), Box<dyn std::error::Error>> {
        let e = event()?;
        let mut bytes = encode(&e)?;
        bytes.extend_from_slice(&[0x80, 0x06, 0x01]);
        let decoded = decode(&bytes)?;
        assert_ne!(checksum(&bytes), checksum(&encode(&e)?));
        assert_eq!(event_hash(&decoded)?, event_hash(&e)?);
        Ok(())
    }
    #[test]
    fn decoder_rejects_unknown_schema_missing_operation_bad_id_and_nonfinite()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid = v1::EventEnvelope::decode(encode(&event()?)?.as_slice())?;
        let mut e = valid.clone();
        e.schema_version = 99;
        assert!(matches!(
            decode(&e.encode_to_vec()),
            Err(CodecError::UnsupportedSchema)
        ));
        let mut e = valid.clone();
        e.operation = None;
        assert!(matches!(
            decode(&e.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        let mut e = valid.clone();
        e.document_id = vec![0; 16];
        assert!(matches!(
            decode(&e.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        let mut e = valid;
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::SetCellValue(ref mut v)),
        }) = e.operation
        {
            v.value = Some(v1::CellValue {
                kind: Some(v1::cell_value::Kind::Number(f64::NAN)),
            })
        }
        assert!(matches!(
            decode(&e.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        Ok(())
    }
    #[test]
    fn command_fingerprint_binds_intent_and_normalizes_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let e = event()?;
        let mut c = Command {
            document_id: e.document_id,
            operation_id: e.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: match e.operation {
                Operation::SetCellValue { address, value } => {
                    CommandPayload::SetCellValue { address, value }
                }
                _ => return Err("unexpected fixture".into()),
            },
        };
        let hash = command_fingerprint(&c)?;
        if let CommandPayload::SetCellValue { value, .. } = &mut c.operation {
            *value = CellValue::Number(-0.0)
        }
        assert_eq!(command_fingerprint(&c)?, hash);
        c.client_sequence = 2;
        assert_ne!(command_fingerprint(&c)?, hash);
        Ok(())
    }

    #[test]
    fn encoder_refuses_payload_that_replay_would_reject() {
        let envelope = v1::EventEnvelope {
            document_id: vec![0; 4 * 1024 * 1024 + 1],
            ..Default::default()
        };
        assert!(matches!(
            encode_envelope(envelope),
            Err(CodecError::EventTooLarge)
        ));
    }
}

#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;
    proptest! {
     #[test]
     fn arbitrary_bounded_operation_roundtrip(text in ".{0,256}",number in any::<i32>(),boolean in any::<bool>(),kind in 0u8..5) {
      let result=(||->Result<(),Box<dyn std::error::Error>>{
       let address=CellAddress{sheet:SheetId::from_bytes([4;16])?,row:RowId::from_bytes([5;16])?,column:ColumnId::from_bytes([6;16])?};
       let value=match kind {0=>CellValue::Blank,1=>CellValue::Number(f64::from(number)),2=>CellValue::Text(text),_=>CellValue::Boolean(boolean)};
       let operation=if kind==4 {Operation::ClearCell{address}}else{Operation::SetCellValue{address,value}};
       let e=CommittedEvent{document_id:DocumentId::from_bytes([1;16])?,operation_id:OperationId::from_bytes([2;16])?,principal:EventPrincipal::User(ActorId::from_bytes([3;16])?),version:DocumentVersion::new(2)?,operation,state_hash:[7;32]};
       let restored=decode(&encode(&e)?)?;if restored!=e || event_hash(&restored)?!=event_hash(&e)?{return Err("roundtrip mismatch".into())}Ok(())
      })();
      prop_assert!(result.is_ok());
     }
    }
}

#[cfg(test)]
mod formula_tests {
    use super::*;
    use spreadsheet_domain::formula as d;
    use spreadsheet_wire_protocol::formula as wire;
    fn fixture() -> Result<d::Formula, Box<dyn std::error::Error>> {
        let origin = CellAddress {
            sheet: SheetId::from_bytes([1; 16])?,
            row: RowId::from_bytes([2; 16])?,
            column: ColumnId::from_bytes([3; 16])?,
        };
        let r = d::BoundCellReference {
            target_sheet: origin.sheet,
            target_row: origin.row,
            target_column: origin.column,
            row_mode: d::AxisReferenceMode::Relative { original_delta: -1 },
            column_mode: d::AxisReferenceMode::Absolute,
        };
        let nodes = vec![
            d::Node::Number(2.0),
            d::Node::Text("fixture".into()),
            d::Node::Boolean(true),
            d::Node::Error(d::FormulaError::Na),
            d::Node::Missing,
            d::Node::Cell(r),
            d::Node::Range(d::BoundRangeReference { start: r, end: r }),
            d::Node::InvalidReference(d::ReferenceError::DeletedTarget),
            d::Node::Unary {
                op: d::UnaryOp::Minus,
                operand: d::NodeId(0),
            },
            d::Node::Binary {
                op: d::BinaryOp::Add,
                left: d::NodeId(1),
                right: d::NodeId(2),
            },
            d::Node::Call {
                name: d::FunctionName::new("FIXTURE".into())?,
                arguments: (3..10).map(d::NodeId).collect(),
            },
        ];
        Ok(d::Formula::new(
            "=FIXTURE(#N/A,,A1,A1:A1,#REF!,-2,\"fixture\"+TRUE)".into(),
            "=FIXTURE(#N/A,,A1,A1:A1,#REF!,-2,\"fixture\"+TRUE)".into(),
            d::BoundAst::new(1, d::NodeId(10), nodes)?,
            origin,
            DocumentVersion::new(1)?,
        )?)
    }
    #[test]
    fn bound_formula_roundtrips_both_independent_dtos_with_identical_semantics()
    -> Result<(), Box<dyn std::error::Error>> {
        let f = fixture()?;
        let storage = formula_codec::decode(formula_codec::encode(&f))?;
        let transport = wire::decode_formula(wire::encode_formula(&f))?;
        assert_eq!(f, storage);
        assert_eq!(f, transport);
        assert_eq!(storage.semantic_hash(), transport.semantic_hash());
        let e = CommittedEvent {
            document_id: DocumentId::from_bytes([8; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([9; 16])?),
            operation_id: OperationId::from_bytes([10; 16])?,
            version: DocumentVersion::new(2)?,
            operation: Operation::SetCellFormula {
                address: f.origin(),
                formula: f,
            },
            state_hash: [11; 32],
        };
        let envelope = v1::EventEnvelope::decode(encode(&e)?.as_slice())?;
        assert_eq!(envelope.schema_version, 2);
        assert_eq!(decode(&envelope.encode_to_vec())?, e);
        let mut old = envelope;
        old.schema_version = 1;
        assert!(matches!(
            decode(&old.encode_to_vec()),
            Err(CodecError::UnsupportedSchema)
        ));
        Ok(())
    }
    #[test]
    fn strict_formula_decoder_rejects_unknown_versions_enums_and_invalid_nodes()
    -> Result<(), Box<dyn std::error::Error>> {
        let valid = formula_codec::encode(&fixture()?);
        let mut bad = valid.clone();
        bad.compatibility_profile = 99;
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::UnsupportedSchema)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.schema_version = 99;
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::UnsupportedSchema)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.nodes[3].kind =
            Some(v1::formula_node::Kind::Error(99));
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::UnsupportedSchema)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.nodes[8].kind =
            Some(v1::formula_node::Kind::Unary(v1::UnaryNode {
                operator: 99,
                operand: 0,
            }));
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::UnsupportedSchema)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.nodes[0].kind = None;
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::Invalid)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.nodes[0].kind =
            Some(v1::formula_node::Kind::Number(f64::NAN));
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::Invalid)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.root = 0;
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::Invalid)
        ));
        let mut bad = valid.clone();
        bad.ast.as_mut().ok_or("missing ast")?.nodes[8].kind =
            Some(v1::formula_node::Kind::Unary(v1::UnaryNode {
                operator: 1,
                operand: 8,
            }));
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::Invalid)
        ));
        let mut bad = valid;
        bad.ast.as_mut().ok_or("missing ast")?.nodes = vec![
            v1::FormulaNode {
                kind: Some(v1::formula_node::Kind::Missing(true))
            };
            d::MAX_NODES + 1
        ];
        assert!(matches!(
            formula_codec::decode(bad),
            Err(CodecError::Invalid)
        ));
        let mut bad = wire::encode_formula(&fixture()?);
        bad.compatibility_profile = 99;
        assert!(matches!(
            wire::decode_formula(bad),
            Err(wire::FormulaCodecError::UnsupportedSchema)
        ));
        Ok(())
    }
    #[test]
    fn formula_command_fingerprint_depends_on_intent_not_bound_tree()
    -> Result<(), Box<dyn std::error::Error>> {
        let f = fixture()?;
        let mut c = Command {
            document_id: DocumentId::from_bytes([8; 16])?,
            actor_id: ActorId::from_bytes([9; 16])?,
            operation_id: OperationId::from_bytes([10; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: CommandPayload::SetCellFormula {
                address: f.origin(),
                source: FormulaSource::new(f.source().to_owned())?,
            },
        };
        let hash = command_fingerprint(&c)?;
        if let CommandPayload::SetCellFormula { source, .. } = &mut c.operation {
            *source = FormulaSource::new("=1+2".into())?
        }
        assert_ne!(command_fingerprint(&c)?, hash);
        Ok(())
    }
    #[test]
    fn insert_rows_schema_round_trips_and_fingerprint_binds_anchor()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([4; 16])?;
        let before = RowId::from_bytes([5; 16])?;
        let rows = vec![RowId::from_bytes([6; 16])?, RowId::from_bytes([7; 16])?];
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::InsertRows {
                sheet,
                before,
                rows: rows.clone(),
            },
            state_hash: [8; 32],
        };
        let encoded = encode(&event)?;
        let envelope = v1::EventEnvelope::decode(encoded.as_slice())?;
        assert_eq!(envelope.schema_version, 5);
        assert_eq!(decode(&encoded)?, event);
        let rebased = CommittedEvent {
            operation: Operation::InsertRowsRebased {
                sheet,
                before,
                rows: rows.clone(),
            },
            ..event.clone()
        };
        let rebased_bytes = encode(&rebased)?;
        let rebased_envelope = v1::EventEnvelope::decode(rebased_bytes.as_slice())?;
        assert_eq!(rebased_envelope.schema_version, 6);
        assert_eq!(decode(&rebased_bytes)?, rebased);
        assert_ne!(event_hash(&event)?, event_hash(&rebased)?);
        let mut command = Command {
            document_id: event.document_id,
            operation_id: event.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: CommandPayload::InsertRows {
                sheet,
                before,
                rows,
            },
        };
        let fingerprint = command_fingerprint(&command)?;
        if let CommandPayload::InsertRows { before, .. } = &mut command.operation {
            *before = RowId::from_bytes([9; 16])?;
        }
        assert_ne!(command_fingerprint(&command)?, fingerprint);
        Ok(())
    }
    #[test]
    fn delete_rows_schema_round_trips_and_fingerprint_binds_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([4; 16])?;
        let rows = vec![RowId::from_bytes([5; 16])?, RowId::from_bytes([6; 16])?];
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::DeleteRows {
                sheet,
                rows: rows.clone(),
            },
            state_hash: [7; 32],
        };
        let bytes = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            7
        );
        assert_eq!(decode(&bytes)?, event);
        let mut command = Command {
            document_id: event.document_id,
            operation_id: event.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: CommandPayload::DeleteRows { sheet, rows },
        };
        let fingerprint = command_fingerprint(&command)?;
        if let CommandPayload::DeleteRows { rows, .. } = &mut command.operation {
            rows[0] = RowId::from_bytes([8; 16])?;
        }
        assert_ne!(command_fingerprint(&command)?, fingerprint);
        Ok(())
    }
    #[test]
    fn insert_columns_schema_round_trips_and_fingerprint_binds_anchor()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([4; 16])?;
        let before = ColumnId::from_bytes([5; 16])?;
        let columns = vec![ColumnId::from_bytes([6; 16])?];
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::InsertColumnsRebased {
                sheet,
                before,
                columns: columns.clone(),
            },
            state_hash: [7; 32],
        };
        let bytes = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            8
        );
        assert_eq!(decode(&bytes)?, event);
        let mut command = Command {
            document_id: event.document_id,
            operation_id: event.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: CommandPayload::InsertColumns {
                sheet,
                before,
                columns,
            },
        };
        let fingerprint = command_fingerprint(&command)?;
        if let CommandPayload::InsertColumns { before, .. } = &mut command.operation {
            *before = ColumnId::from_bytes([8; 16])?;
        }
        assert_ne!(command_fingerprint(&command)?, fingerprint);
        Ok(())
    }
    #[test]
    fn delete_columns_schema_round_trips_and_fingerprint_binds_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([4; 16])?;
        let columns = vec![ColumnId::from_bytes([5; 16])?];
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::DeleteColumns {
                sheet,
                columns: columns.clone(),
            },
            state_hash: [7; 32],
        };
        let bytes = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            9
        );
        assert_eq!(decode(&bytes)?, event);
        let mut command = Command {
            document_id: event.document_id,
            operation_id: event.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: CommandPayload::DeleteColumns { sheet, columns },
        };
        let fingerprint = command_fingerprint(&command)?;
        if let CommandPayload::DeleteColumns { columns, .. } = &mut command.operation {
            columns[0] = ColumnId::from_bytes([8; 16])?;
        }
        assert_ne!(command_fingerprint(&command)?, fingerprint);
        Ok(())
    }
    #[test]
    fn value_batch_schema_round_trips_and_fingerprint_binds_every_write()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([4; 16])?;
        let address = CellAddress {
            sheet,
            row: RowId::from_bytes([5; 16])?,
            column: ColumnId::from_bytes([6; 16])?,
        };
        let writes = vec![CellWrite {
            address,
            value: CellValue::Number(42.0),
        }];
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::SetCellValues {
                writes: writes.clone(),
            },
            state_hash: [7; 32],
        };
        let bytes = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            10
        );
        assert_eq!(decode(&bytes)?, event);
        let mut command = Command {
            document_id: event.document_id,
            operation_id: event.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: CommandPayload::SetCellValues { writes },
        };
        let fingerprint = command_fingerprint(&command)?;
        if let CommandPayload::SetCellValues { writes } = &mut command.operation {
            writes[0].value = CellValue::Number(43.0);
        }
        assert_ne!(command_fingerprint(&command)?, fingerprint);
        Ok(())
    }
    #[test]
    fn authored_and_compensating_cell_writes_are_distinct_durable_schemas()
    -> Result<(), Box<dyn std::error::Error>> {
        let document_id = DocumentId::from_bytes([1; 16])?;
        let actor = ActorId::from_bytes([3; 16])?;
        let address = CellAddress {
            sheet: SheetId::from_bytes([4; 16])?,
            row: RowId::from_bytes([5; 16])?,
            column: ColumnId::from_bytes([6; 16])?,
        };
        let target_operation_id = OperationId::from_bytes([2; 16])?;
        let authored = CommittedEvent {
            document_id,
            operation_id: target_operation_id,
            principal: EventPrincipal::User(actor),
            version: DocumentVersion::new(2)?,
            operation: Operation::CellWrite {
                address,
                content: Some(CellContent::Value(CellValue::Number(8.0))),
                previous: Some(CellContent::Value(CellValue::Number(3.0))),
                kind: Some(CellWriteKind::SetValue),
                undoes: None,
                undoes_version: None,
            },
            state_hash: [7; 32],
        };
        let bytes = encode(&authored)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            11
        );
        assert_eq!(decode(&bytes)?, authored);
        let compensation = CommittedEvent {
            operation_id: OperationId::from_bytes([9; 16])?,
            version: DocumentVersion::new(3)?,
            operation: Operation::CellWrite {
                address,
                content: Some(CellContent::Value(CellValue::Number(3.0))),
                previous: Some(CellContent::Value(CellValue::Number(8.0))),
                kind: None,
                undoes: Some(target_operation_id),
                undoes_version: Some(DocumentVersion::new(2)?),
            },
            ..authored.clone()
        };
        let bytes = encode(&compensation)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            12
        );
        assert_eq!(decode(&bytes)?, compensation);
        assert_ne!(event_hash(&authored)?, event_hash(&compensation)?);

        let mut malformed = v1::EventEnvelope::decode(bytes.as_slice())?;
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::CompensatingCellWrite(write)),
        }) = malformed.operation.as_mut()
        {
            write.undoes_version = 0;
        } else {
            return Err("unexpected compensation DTO".into());
        }
        assert!(matches!(
            decode(&malformed.encode_to_vec()),
            Err(CodecError::Invalid)
        ));

        let mut retry = Command {
            document_id,
            operation_id: compensation.operation_id,
            actor_id: actor,
            base_version: DocumentVersion::new(2)?,
            client_sequence: 1,
            operation: CommandPayload::UndoCell {
                target_operation_id,
            },
        };
        let fingerprint = command_fingerprint(&retry)?;
        retry.operation = CommandPayload::UndoCell {
            target_operation_id: OperationId::from_bytes([8; 16])?,
        };
        assert_ne!(command_fingerprint(&retry)?, fingerprint);
        Ok(())
    }
    #[test]
    fn authored_formula_write_has_schema_sixteen_and_rejects_downgrade()
    -> Result<(), Box<dyn std::error::Error>> {
        let formula = fixture()?;
        let address = formula.origin();
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([7; 16])?,
            operation_id: OperationId::from_bytes([8; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([9; 16])?),
            version: DocumentVersion::new(4)?,
            operation: Operation::CellWrite {
                address,
                content: Some(CellContent::Formula(formula.clone())),
                previous: Some(CellContent::Value(CellValue::Number(7.))),
                kind: Some(CellWriteKind::SetFormula),
                undoes: None,
                undoes_version: None,
            },
            state_hash: [11; 32],
        };
        let encoded = encode(&event)?;
        let dto = v1::EventEnvelope::decode(encoded.as_slice())?;
        assert_eq!(dto.schema_version, 16);
        assert_eq!(decode(&encoded)?, event);
        assert_eq!(event_hash(&decode(&encoded)?)?, event_hash(&event)?);
        let future_binding = CommittedEvent {
            version: formula.bound_at_version(),
            ..event.clone()
        };
        assert!(matches!(encode(&future_binding), Err(CodecError::Invalid)));
        let legacy = CommittedEvent {
            operation: Operation::SetCellFormula { address, formula },
            ..event.clone()
        };
        assert_ne!(event_hash(&event)?, event_hash(&legacy)?);
        let mut old = dto.clone();
        old.schema_version = 11;
        assert!(matches!(
            decode(&old.encode_to_vec()),
            Err(CodecError::UnsupportedSchema)
        ));
        let mut bad = dto.clone();
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::AuthoredCellWrite(write)),
        }) = bad.operation.as_mut()
        {
            write.authored_kind = 1;
        } else {
            return Err("unexpected authored DTO".into());
        }
        assert!(matches!(
            decode(&bad.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        let mut future = dto;
        if let Some(v1::Operation {
            kind: Some(v1::operation::Kind::AuthoredCellWrite(write)),
        }) = future.operation.as_mut()
        {
            if let Some(v1::CellContent {
                kind: Some(v1::cell_content::Kind::Formula(formula)),
            }) = write.content.as_mut()
            {
                formula.bound_at_version = 4;
            } else {
                return Err("unexpected formula DTO".into());
            }
        } else {
            return Err("unexpected authored DTO".into());
        }
        assert!(matches!(
            decode(&future.encode_to_vec()),
            Err(CodecError::Invalid)
        ));
        let malformed = CommittedEvent {
            operation: Operation::CellWrite {
                address,
                content: Some(CellContent::Value(CellValue::Number(7.))),
                previous: None,
                kind: Some(CellWriteKind::SetFormula),
                undoes: None,
                undoes_version: None,
            },
            ..event
        };
        assert!(matches!(encode(&malformed), Err(CodecError::Invalid)));
        Ok(())
    }
    #[test]
    fn formula_preimage_compensation_roundtrips_only_in_schema_thirteen()
    -> Result<(), Box<dyn std::error::Error>> {
        let formula = fixture()?;
        let address = formula.origin();
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([7; 16])?,
            operation_id: OperationId::from_bytes([8; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([9; 16])?),
            version: DocumentVersion::new(4)?,
            operation: Operation::CellWrite {
                address,
                content: Some(CellContent::Formula(formula)),
                previous: Some(CellContent::Value(CellValue::Number(12.0))),
                kind: None,
                undoes: Some(OperationId::from_bytes([10; 16])?),
                undoes_version: Some(DocumentVersion::new(3)?),
            },
            state_hash: [11; 32],
        };
        let bytes = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            13
        );
        assert_eq!(decode(&bytes)?, event);
        let mut old_schema = v1::EventEnvelope::decode(bytes.as_slice())?;
        old_schema.schema_version = 12;
        assert!(matches!(
            decode(&old_schema.encode_to_vec()),
            Err(CodecError::UnsupportedSchema)
        ));
        Ok(())
    }
    #[test]
    fn frozen_mixed_copy_roundtrips_only_in_schema_fourteen()
    -> Result<(), Box<dyn std::error::Error>> {
        let formula = fixture()?;
        let address = formula.origin();
        let value_address = CellAddress {
            column: ColumnId::from_bytes([4; 16])?,
            ..address
        };
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([7; 16])?,
            operation_id: OperationId::from_bytes([8; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([9; 16])?),
            version: DocumentVersion::new(4)?,
            operation: Operation::CopyRange {
                writes: vec![
                    ContentWrite {
                        address,
                        content: Some(CellContent::Formula(formula)),
                    },
                    ContentWrite {
                        address: value_address,
                        content: Some(CellContent::Value(CellValue::Number(3.0))),
                    },
                ],
            },
            state_hash: [11; 32],
        };
        let bytes = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(bytes.as_slice())?.schema_version,
            14
        );
        assert_eq!(decode(&bytes)?, event);
        let mut old_schema = v1::EventEnvelope::decode(bytes.as_slice())?;
        old_schema.schema_version = 13;
        assert!(matches!(
            decode(&old_schema.encode_to_vec()),
            Err(CodecError::UnsupportedSchema)
        ));
        Ok(())
    }
    #[test]
    fn add_sheet_has_own_durable_schema_and_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
        let event = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: Operation::AddSheet {
                sheet: SheetSeed {
                    id: SheetId::from_bytes([4; 16])?,
                    name: "Plan".into(),
                    rows: vec![RowId::from_bytes([5; 16])?],
                    columns: vec![ColumnId::from_bytes([6; 16])?],
                },
            },
            state_hash: [7; 32],
        };
        let encoded = encode(&event)?;
        assert_eq!(
            v1::EventEnvelope::decode(encoded.as_slice())?.schema_version,
            15
        );
        assert_eq!(decode(&encoded)?, event);
        assert_eq!(event_hash(&decode(&encoded)?)?, event_hash(&event)?);
        let mut old = v1::EventEnvelope::decode(encoded.as_slice())?;
        old.schema_version = 14;
        assert!(matches!(
            decode(&old.encode_to_vec()),
            Err(CodecError::UnsupportedSchema)
        ));
        Ok(())
    }
    #[test]
    fn legacy_hash_golden_is_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let operation = Operation::SetCellValue {
            address: CellAddress {
                sheet: SheetId::from_bytes([4; 16])?,
                row: RowId::from_bytes([5; 16])?,
                column: ColumnId::from_bytes([6; 16])?,
            },
            value: CellValue::Number(0.0),
        };
        let e = CommittedEvent {
            document_id: DocumentId::from_bytes([1; 16])?,
            operation_id: OperationId::from_bytes([2; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([3; 16])?),
            version: DocumentVersion::new(2)?,
            operation: operation.clone(),
            state_hash: [7; 32],
        };
        let c = Command {
            document_id: e.document_id,
            operation_id: e.operation_id,
            actor_id: ActorId::from_bytes([3; 16])?,
            base_version: DocumentVersion::new(1)?,
            client_sequence: 1,
            operation: match operation {
                Operation::SetCellValue { address, value } => {
                    CommandPayload::SetCellValue { address, value }
                }
                _ => return Err("unexpected fixture".into()),
            },
        };
        let hex = |b: [u8; 32]| {
            b.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            hex(event_hash(&e)?),
            "3a5fa3b37cabace0dfc2598dee69c5d4185f57cc9794eb1fdbfee1a7831fdffd"
        );
        assert_eq!(
            hex(command_fingerprint(&c)?),
            "10e140f7a048de47a9b35a4e4a0e70ca136e5db40b0c5e7ab71ebb529fcf38f7"
        );
        assert_eq!(
            v1::EventEnvelope::decode(encode(&e)?.as_slice())?.schema_version,
            1
        );
        Ok(())
    }
}
