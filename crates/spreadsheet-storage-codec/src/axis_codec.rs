use super::*;
use std::collections::BTreeSet;

struct DeltaIdentity {
    kind: u8,
    removed: BTreeSet<[u8; 16]>,
    inserted: BTreeSet<[u8; 16]>,
}
fn validate_delta(delta: &AxisDelta) -> Result<DeltaIdentity, CodecError> {
    let (kind, limit, removed, inserted) = match delta {
        AxisDelta::Rows { removed, inserted } => (
            1,
            1_000,
            removed.iter().map(|id| *id.as_bytes()).collect::<Vec<_>>(),
            inserted
                .iter()
                .map(|placement| {
                    (
                        *placement.row.as_bytes(),
                        placement.before.map(|id| *id.as_bytes()),
                    )
                })
                .collect::<Vec<_>>(),
        ),
        AxisDelta::Columns { removed, inserted } => (
            2,
            100,
            removed.iter().map(|id| *id.as_bytes()).collect::<Vec<_>>(),
            inserted
                .iter()
                .map(|placement| {
                    (
                        *placement.column.as_bytes(),
                        placement.before.map(|id| *id.as_bytes()),
                    )
                })
                .collect::<Vec<_>>(),
        ),
    };
    if removed.is_empty() == inserted.is_empty() || removed.len() > limit || inserted.len() > limit
    {
        return Err(CodecError::Invalid);
    }
    let removed_ids = removed.iter().copied().collect::<BTreeSet<_>>();
    let inserted_ids = inserted.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>();
    if removed_ids.len() != removed.len()
        || inserted_ids.len() != inserted.len()
        || inserted
            .iter()
            .any(|(_, before)| before.is_some_and(|id| inserted_ids.contains(&id)))
    {
        return Err(CodecError::Invalid);
    }
    Ok(DeltaIdentity {
        kind,
        removed: removed_ids,
        inserted: inserted_ids,
    })
}
pub(super) fn validate(
    operation: &Operation,
    version: Option<DocumentVersion>,
) -> Result<(), CodecError> {
    let change = match operation {
        Operation::AxisEdit { change } => change,
        Operation::AxisCompensation {
            change,
            undoes_version,
            ..
        } => {
            if *undoes_version == DocumentVersion::ZERO
                || version.is_some_and(|version| *undoes_version >= version)
            {
                return Err(CodecError::Invalid);
            }
            change
        }
        _ => return Ok(()),
    };
    if version == Some(DocumentVersion::ZERO)
        || change.before_order_hash == change.after_order_hash
        || change.writes.len() > 10_000
    {
        return Err(CodecError::Invalid);
    }
    let forward = validate_delta(&change.delta)?;
    let inverse = validate_delta(&change.inverse_delta)?;
    if forward.kind != inverse.kind
        || forward.removed != inverse.inserted
        || forward.inserted != inverse.removed
        || change
            .writes
            .windows(2)
            .any(|pair| pair[0].address >= pair[1].address)
    {
        return Err(CodecError::Invalid);
    }
    let mut incoming = 0usize;
    let mut previous = 0usize;
    for write in &change.writes {
        for (content, prior) in write
            .content
            .iter()
            .map(|content| (content, false))
            .chain(write.previous.iter().map(|content| (content, true)))
        {
            match content {
                CellContent::Value(CellValue::Blank) => return Err(CodecError::Invalid),
                CellContent::Value(value) => value.validate().map_err(|_| CodecError::Invalid)?,
                CellContent::Formula(formula)
                    if formula.origin() != write.address
                        || version.is_some_and(|version| {
                            if prior {
                                formula.bound_at_version() >= version
                            } else {
                                formula.bound_at_version() > version
                            }
                        }) =>
                {
                    return Err(CodecError::Invalid);
                }
                CellContent::Formula(_) => {}
            }
        }
        incoming = incoming
            .checked_add(write.content.as_ref().map_or(0, CellContent::text_bytes))
            .ok_or(CodecError::Invalid)?;
        previous = previous
            .checked_add(write.previous.as_ref().map_or(0, CellContent::text_bytes))
            .ok_or(CodecError::Invalid)?;
        if incoming > MAX_CELL_VALUE_BATCH_TEXT_BYTES || previous > MAX_CELL_VALUE_BATCH_TEXT_BYTES
        {
            return Err(CodecError::Invalid);
        }
    }
    Ok(())
}
fn encode_delta(delta: &AxisDelta) -> v1::AxisDelta {
    use v1::axis_delta::Kind;
    v1::AxisDelta {
        kind: Some(match delta {
            AxisDelta::Rows { removed, inserted } => Kind::Rows(v1::RowDelta {
                removed: removed.iter().map(|id| id.as_bytes().to_vec()).collect(),
                inserted: inserted
                    .iter()
                    .map(|placement| v1::RowPlacement {
                        row_id: placement.row.as_bytes().to_vec(),
                        before_row_id: placement
                            .before
                            .map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
                    })
                    .collect(),
            }),
            AxisDelta::Columns { removed, inserted } => Kind::Columns(v1::ColumnDelta {
                removed: removed.iter().map(|id| id.as_bytes().to_vec()).collect(),
                inserted: inserted
                    .iter()
                    .map(|placement| v1::ColumnPlacement {
                        column_id: placement.column.as_bytes().to_vec(),
                        before_column_id: placement
                            .before
                            .map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
                    })
                    .collect(),
            }),
        }),
    }
}
fn decode_delta(delta: v1::AxisDelta) -> Result<AxisDelta, CodecError> {
    use v1::axis_delta::Kind;
    let delta = match delta.kind.ok_or(CodecError::Invalid)? {
        Kind::Rows(delta) => {
            if delta.removed.len() > 1_000 || delta.inserted.len() > 1_000 {
                return Err(CodecError::Invalid);
            }
            AxisDelta::Rows {
                removed: delta
                    .removed
                    .iter()
                    .map(|bytes| RowId::from_bytes(id(bytes)?).map_err(|_| CodecError::Invalid))
                    .collect::<Result<_, _>>()?,
                inserted: delta
                    .inserted
                    .iter()
                    .map(|placement| {
                        Ok(RowPlacement {
                            row: RowId::from_bytes(id(&placement.row_id)?)
                                .map_err(|_| CodecError::Invalid)?,
                            before: if placement.before_row_id.is_empty() {
                                None
                            } else {
                                Some(
                                    RowId::from_bytes(id(&placement.before_row_id)?)
                                        .map_err(|_| CodecError::Invalid)?,
                                )
                            },
                        })
                    })
                    .collect::<Result<_, CodecError>>()?,
            }
        }
        Kind::Columns(delta) => {
            if delta.removed.len() > 100 || delta.inserted.len() > 100 {
                return Err(CodecError::Invalid);
            }
            AxisDelta::Columns {
                removed: delta
                    .removed
                    .iter()
                    .map(|bytes| ColumnId::from_bytes(id(bytes)?).map_err(|_| CodecError::Invalid))
                    .collect::<Result<_, _>>()?,
                inserted: delta
                    .inserted
                    .iter()
                    .map(|placement| {
                        Ok(ColumnPlacement {
                            column: ColumnId::from_bytes(id(&placement.column_id)?)
                                .map_err(|_| CodecError::Invalid)?,
                            before: if placement.before_column_id.is_empty() {
                                None
                            } else {
                                Some(
                                    ColumnId::from_bytes(id(&placement.before_column_id)?)
                                        .map_err(|_| CodecError::Invalid)?,
                                )
                            },
                        })
                    })
                    .collect::<Result<_, CodecError>>()?,
            }
        }
    };
    validate_delta(&delta)?;
    Ok(delta)
}
pub(super) fn encode_change(change: &AxisChange) -> Result<v1::AxisChange, CodecError> {
    Ok(v1::AxisChange {
        sheet_id: change.sheet.as_bytes().to_vec(),
        delta: Some(encode_delta(&change.delta)),
        inverse_delta: Some(encode_delta(&change.inverse_delta)),
        writes: move_codec::encode_writes(&change.writes)?,
        before_order_hash: change.before_order_hash.to_vec(),
        after_order_hash: change.after_order_hash.to_vec(),
    })
}
pub(super) fn decode_change(change: v1::AxisChange) -> Result<AxisChange, CodecError> {
    if change.writes.len() > 10_000 {
        return Err(CodecError::Invalid);
    }
    Ok(AxisChange {
        sheet: SheetId::from_bytes(id(&change.sheet_id)?).map_err(|_| CodecError::Invalid)?,
        delta: decode_delta(change.delta.ok_or(CodecError::Invalid)?)?,
        inverse_delta: decode_delta(change.inverse_delta.ok_or(CodecError::Invalid)?)?,
        writes: change
            .writes
            .into_iter()
            .map(|write| {
                let address = decode_addr(write.address.ok_or(CodecError::Invalid)?)?;
                Ok(GuardedContentWrite {
                    address,
                    content: write
                        .content
                        .map(|content| decode_content(content, address))
                        .transpose()?,
                    previous: write
                        .previous
                        .map(|content| decode_content(content, address))
                        .transpose()?,
                })
            })
            .collect::<Result<_, CodecError>>()?,
        before_order_hash: id(&change.before_order_hash)?,
        after_order_hash: id(&change.after_order_hash)?,
    })
}
fn hash_delta(hash: &mut Sha256, delta: &AxisDelta) {
    match delta {
        AxisDelta::Rows { removed, inserted } => {
            hash.update([1]);
            hash.update((removed.len() as u64).to_le_bytes());
            for row in removed {
                hash.update(row.as_bytes());
            }
            hash.update((inserted.len() as u64).to_le_bytes());
            for placement in inserted {
                hash.update(placement.row.as_bytes());
                hash.update([u8::from(placement.before.is_some())]);
                if let Some(before) = placement.before {
                    hash.update(before.as_bytes());
                }
            }
        }
        AxisDelta::Columns { removed, inserted } => {
            hash.update([2]);
            hash.update((removed.len() as u64).to_le_bytes());
            for column in removed {
                hash.update(column.as_bytes());
            }
            hash.update((inserted.len() as u64).to_le_bytes());
            for placement in inserted {
                hash.update(placement.column.as_bytes());
                hash.update([u8::from(placement.before.is_some())]);
                if let Some(before) = placement.before {
                    hash.update(before.as_bytes());
                }
            }
        }
    }
}
pub(super) fn hash(hash: &mut Sha256, operation: &Operation) -> Result<(), CodecError> {
    validate(operation, None)?;
    let (change, inverse) = match operation {
        Operation::AxisEdit { change } => {
            hash.update([23]);
            (change, None)
        }
        Operation::AxisCompensation {
            change,
            undoes,
            undoes_version,
        } => {
            hash.update([24]);
            (change, Some((undoes, undoes_version)))
        }
        _ => return Err(CodecError::Invalid),
    };
    hash.update(change.sheet.as_bytes());
    hash_delta(hash, &change.delta);
    hash_delta(hash, &change.inverse_delta);
    hash.update(change.before_order_hash);
    hash.update(change.after_order_hash);
    move_codec::hash_writes(hash, &change.writes)?;
    if let Some((undoes, version)) = inverse {
        hash.update(undoes.as_bytes());
        hash.update(version.get().to_le_bytes());
    }
    Ok(())
}
