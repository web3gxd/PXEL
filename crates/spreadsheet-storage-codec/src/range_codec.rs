use super::*;

/// One validation path for encode, decode and semantic hashing.
pub(super) fn validate(
    operation: &Operation,
    version: Option<DocumentVersion>,
) -> Result<(), CodecError> {
    let Operation::RangeWrite {
        writes,
        kind,
        undoes,
        undoes_version,
    } = operation
    else {
        return Ok(());
    };
    match (kind, undoes, undoes_version) {
        (Some(_), None, None) | (None, Some(_), Some(_)) => {}
        _ => return Err(CodecError::Invalid),
    }
    if writes.is_empty()
        || writes.len() > 1_000
        || version == Some(DocumentVersion::ZERO)
        || undoes_version.is_some_and(|target| {
            target == DocumentVersion::ZERO || version.is_some_and(|version| target >= version)
        })
    {
        return Err(CodecError::Invalid);
    }
    let mut addresses = std::collections::BTreeSet::new();
    let mut incoming = 0usize;
    let mut previous = 0usize;
    for write in writes {
        if !addresses.insert(write.address)
            || matches!(kind, Some(RangeWriteKind::SetValues))
                && matches!(write.content, Some(CellContent::Formula(_)))
        {
            return Err(CodecError::Invalid);
        }
        for content in write.content.iter().chain(write.previous.iter()) {
            match content {
                CellContent::Value(CellValue::Blank) => return Err(CodecError::Invalid),
                CellContent::Value(value) => value.validate().map_err(|_| CodecError::Invalid)?,
                CellContent::Formula(formula)
                    if formula.origin() != write.address
                        || version.is_some_and(|version| formula.bound_at_version() >= version) =>
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
pub(super) fn encode(operation: &Operation) -> Result<v1::RangeWrite, CodecError> {
    validate(operation, None)?;
    let Operation::RangeWrite {
        writes,
        kind,
        undoes,
        undoes_version,
    } = operation
    else {
        return Err(CodecError::Invalid);
    };
    Ok(v1::RangeWrite {
        writes: writes
            .iter()
            .map(|write| {
                Ok(v1::GuardedContentWrite {
                    address: Some(addr(&write.address)),
                    content: encode_optional_content(&write.content)?,
                    previous: encode_optional_content(&write.previous)?,
                })
            })
            .collect::<Result<_, CodecError>>()?,
        undoes_operation_id: undoes.map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
        undoes_version: undoes_version.map_or(0, DocumentVersion::get),
        authored_kind: match kind {
            Some(RangeWriteKind::SetValues) => 1,
            Some(RangeWriteKind::Copy) => 2,
            None => 0,
        },
    })
}
pub(super) fn decode(range: v1::RangeWrite, compensating: bool) -> Result<Operation, CodecError> {
    if range.writes.is_empty() || range.writes.len() > 1_000 {
        return Err(CodecError::Invalid);
    }
    let (kind, undoes, undoes_version) = if compensating {
        if range.authored_kind != 0 || range.undoes_version == 0 {
            return Err(CodecError::Invalid);
        }
        (
            None,
            Some(
                OperationId::from_bytes(id(&range.undoes_operation_id)?)
                    .map_err(|_| CodecError::Invalid)?,
            ),
            Some(DocumentVersion::new(range.undoes_version).map_err(|_| CodecError::Invalid)?),
        )
    } else {
        if !range.undoes_operation_id.is_empty() || range.undoes_version != 0 {
            return Err(CodecError::Invalid);
        }
        (
            Some(match range.authored_kind {
                1 => RangeWriteKind::SetValues,
                2 => RangeWriteKind::Copy,
                _ => return Err(CodecError::Invalid),
            }),
            None,
            None,
        )
    };
    let writes = range
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
        .collect::<Result<_, CodecError>>()?;
    let operation = Operation::RangeWrite {
        writes,
        kind,
        undoes,
        undoes_version,
    };
    validate(&operation, None)?;
    Ok(operation)
}
pub(super) fn hash(h: &mut Sha256, operation: &Operation) -> Result<(), CodecError> {
    validate(operation, None)?;
    let Operation::RangeWrite {
        writes,
        kind,
        undoes,
        undoes_version,
    } = operation
    else {
        return Err(CodecError::Invalid);
    };
    h.update([match kind {
        Some(RangeWriteKind::SetValues) => 18,
        Some(RangeWriteKind::Copy) => 19,
        None => 20,
    }]);
    h.update((writes.len() as u64).to_le_bytes());
    for write in writes {
        h.update(write.address.sheet.as_bytes());
        h.update(write.address.row.as_bytes());
        h.update(write.address.column.as_bytes());
        hash_content(h, &write.content)?;
        hash_content(h, &write.previous)?;
    }
    if let (Some(id), Some(version)) = (undoes, undoes_version) {
        h.update(id.as_bytes());
        h.update(version.get().to_le_bytes());
    }
    Ok(())
}
