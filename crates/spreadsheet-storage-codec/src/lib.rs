//! Versioned durable envelope. Hashes use explicit semantic encoding, never protobuf bytes.
mod formula_codec;
use prost::Message;
use sha2::{Digest, Sha256};
use spreadsheet_domain::workbook::*;
use thiserror::Error;
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/pxel.storage.v1.rs"));
}
#[derive(Debug, Error)]
pub enum CodecError {
    #[error("unsupported event schema")]
    UnsupportedSchema,
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
fn operation(o: &Operation) -> Result<v1::Operation, CodecError> {
    use v1::operation::Kind;
    Ok(v1::Operation {
        kind: Some(match o {
            Operation::CreateWorkbook { sheet } => Kind::CreateWorkbook(v1::CreateWorkbook {
                sheet_id: sheet.id.as_bytes().to_vec(),
                name: sheet.name.clone(),
                rows: sheet.rows.iter().map(|i| i.as_bytes().to_vec()).collect(),
                columns: sheet
                    .columns
                    .iter()
                    .map(|i| i.as_bytes().to_vec())
                    .collect(),
            }),
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
        }),
    })
}
fn decode_operation(o: v1::Operation) -> Result<Operation, CodecError> {
    use v1::operation::Kind;
    Ok(match o.kind.ok_or(CodecError::Invalid)? {
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
        Kind::CreateWorkbook(s) => Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes(id(&s.sheet_id)?).map_err(|_| CodecError::Invalid)?,
                name: s.name,
                rows: s
                    .rows
                    .iter()
                    .map(|v| RowId::from_bytes(id(v)?).map_err(|_| CodecError::Invalid))
                    .collect::<Result<_, _>>()?,
                columns: s
                    .columns
                    .iter()
                    .map(|v| ColumnId::from_bytes(id(v)?).map_err(|_| CodecError::Invalid))
                    .collect::<Result<_, _>>()?,
            },
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
    })
}
pub fn schema_version(operation: &Operation) -> u32 {
    match operation {
        Operation::ExtendRows { .. } => 4,
        Operation::InsertRows { .. } => 5,
        Operation::InsertRowsRebased { .. } => 6,
        Operation::DeleteRows { .. } => 7,
        Operation::InsertColumnsRebased { .. } => 8,
        Operation::DeleteColumns { .. } => 9,
        Operation::AdvanceCalculationEpoch { .. } => 3,
        Operation::SetCellFormula { .. } => 2,
        _ => 1,
    }
}
pub fn encode(event: &CommittedEvent) -> Result<Vec<u8>, CodecError> {
    let (actor_id, principal_kind) = match (event.principal, &event.operation) {
        (EventPrincipal::User(actor), Operation::AdvanceCalculationEpoch { .. }) => {
            let _ = actor;
            return Err(CodecError::Invalid);
        }
        (EventPrincipal::User(actor), _) => (actor.as_bytes().to_vec(), 0),
        (EventPrincipal::System, Operation::AdvanceCalculationEpoch { .. }) => (Vec::new(), 2),
        (EventPrincipal::System, _) => return Err(CodecError::Invalid),
    };
    Ok(v1::EventEnvelope {
        schema_version: schema_version(&event.operation),
        document_id: event.document_id.as_bytes().to_vec(),
        operation_id: event.operation_id.as_bytes().to_vec(),
        actor_id,
        committed_version: event.version.get(),
        state_hash: event.state_hash.to_vec(),
        operation: Some(operation(&event.operation)?),
        principal_kind,
    }
    .encode_to_vec())
}
pub fn decode(bytes: &[u8]) -> Result<CommittedEvent, CodecError> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(CodecError::Invalid);
    }
    let e = v1::EventEnvelope::decode(bytes).map_err(|_| CodecError::Invalid)?;
    if !matches!(e.schema_version, 1..=9) {
        return Err(CodecError::UnsupportedSchema);
    }
    if e.committed_version == 0 {
        return Err(CodecError::Invalid);
    }
    let operation = decode_operation(e.operation.ok_or(CodecError::Invalid)?)?;
    if schema_version(&operation) != e.schema_version {
        return Err(CodecError::UnsupportedSchema);
    }
    let principal = match (
        e.schema_version,
        e.principal_kind,
        e.actor_id.as_slice(),
        &operation,
    ) {
        (1 | 2 | 4 | 5 | 6 | 7 | 8 | 9, 0, actor, Operation::AdvanceCalculationEpoch { .. }) => {
            let _ = actor;
            return Err(CodecError::Invalid);
        }
        (1 | 2 | 4 | 5 | 6 | 7 | 8 | 9, 0, actor, _) => {
            EventPrincipal::User(ActorId::from_bytes(id(actor)?).map_err(|_| CodecError::Invalid)?)
        }
        (3, 2, [], Operation::AdvanceCalculationEpoch { .. }) => EventPrincipal::System,
        _ => return Err(CodecError::Invalid),
    };
    if matches!(&operation,Operation::SetCellFormula{formula,..} if formula.bound_at_version().get()>=e.committed_version)
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
    }
    Ok(h.finalize().into())
}
pub fn system_fingerprint(e: &CommittedEvent) -> Result<[u8; 32], CodecError> {
    if e.principal != EventPrincipal::System
        || !matches!(e.operation, Operation::AdvanceCalculationEpoch { .. })
    {
        return Err(CodecError::Invalid);
    }
    let mut h = Sha256::new();
    h.update(b"PXEL-SYSTEM-COMMAND-1");
    h.update(e.document_id.as_bytes());
    h.update(e.operation_id.as_bytes());
    if let Operation::AdvanceCalculationEpoch { context } = &e.operation {
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
    }
    Ok(h.finalize().into())
}
pub fn event_hash(e: &CommittedEvent) -> Result<[u8; 32], CodecError> {
    let mut h = Sha256::new();
    match (e.principal, &e.operation) {
        (EventPrincipal::User(_), Operation::AdvanceCalculationEpoch { .. })
        | (EventPrincipal::System, Operation::CreateWorkbook { .. })
        | (EventPrincipal::System, Operation::SetCellValue { .. })
        | (EventPrincipal::System, Operation::SetCellFormula { .. })
        | (EventPrincipal::System, Operation::ExtendRows { .. })
        | (EventPrincipal::System, Operation::InsertRows { .. })
        | (EventPrincipal::System, Operation::InsertRowsRebased { .. })
        | (EventPrincipal::System, Operation::DeleteRows { .. })
        | (EventPrincipal::System, Operation::InsertColumnsRebased { .. })
        | (EventPrincipal::System, Operation::DeleteColumns { .. })
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
