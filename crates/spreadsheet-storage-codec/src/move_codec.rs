use super::*;

pub(super) fn validate_geometry(
    source: CellAddress,
    destination: CellAddress,
    rows: u32,
    columns: u32,
) -> Result<(), CodecError> {
    if source.sheet != destination.sheet
        || source == destination
        || rows == 0
        || columns == 0
        || u64::from(rows) * u64::from(columns) > 1_000
    {
        return Err(CodecError::Invalid);
    }
    Ok(())
}
pub(super) fn validate(
    operation: &Operation,
    version: Option<DocumentVersion>,
) -> Result<(), CodecError> {
    let Operation::MoveRange {
        source,
        destination,
        rows,
        columns,
        writes,
    } = operation
    else {
        return Ok(());
    };
    validate_geometry(*source, *destination, *rows, *columns)?;
    if writes.is_empty() || writes.len() > 10_000 || version == Some(DocumentVersion::ZERO) {
        return Err(CodecError::Invalid);
    }
    let mut addresses = std::collections::BTreeSet::new();
    let mut incoming = 0usize;
    let mut previous = 0usize;
    for write in writes {
        if !addresses.insert(write.address) {
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
pub(super) fn encode(operation: &Operation) -> Result<v1::MoveRange, CodecError> {
    validate(operation, None)?;
    let Operation::MoveRange {
        source,
        destination,
        rows,
        columns,
        writes,
    } = operation
    else {
        return Err(CodecError::Invalid);
    };
    Ok(v1::MoveRange {
        source: Some(addr(source)),
        destination: Some(addr(destination)),
        rows: *rows,
        columns: *columns,
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
    })
}
pub(super) fn decode(moved: v1::MoveRange) -> Result<Operation, CodecError> {
    let source = decode_addr(moved.source.ok_or(CodecError::Invalid)?)?;
    let destination = decode_addr(moved.destination.ok_or(CodecError::Invalid)?)?;
    validate_geometry(source, destination, moved.rows, moved.columns)?;
    if moved.writes.is_empty() || moved.writes.len() > 10_000 {
        return Err(CodecError::Invalid);
    }
    let writes = moved
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
    let operation = Operation::MoveRange {
        source,
        destination,
        rows: moved.rows,
        columns: moved.columns,
        writes,
    };
    validate(&operation, None)?;
    Ok(operation)
}
pub(super) fn hash_geometry(
    h: &mut Sha256,
    source: CellAddress,
    destination: CellAddress,
    rows: u32,
    columns: u32,
) {
    for address in [source, destination] {
        h.update(address.sheet.as_bytes());
        h.update(address.row.as_bytes());
        h.update(address.column.as_bytes());
    }
    h.update(rows.to_le_bytes());
    h.update(columns.to_le_bytes());
}
pub(super) fn hash(h: &mut Sha256, operation: &Operation) -> Result<(), CodecError> {
    validate(operation, None)?;
    let Operation::MoveRange {
        source,
        destination,
        rows,
        columns,
        writes,
    } = operation
    else {
        return Err(CodecError::Invalid);
    };
    h.update([21]);
    hash_geometry(h, *source, *destination, *rows, *columns);
    h.update((writes.len() as u64).to_le_bytes());
    for write in writes {
        h.update(write.address.sheet.as_bytes());
        h.update(write.address.row.as_bytes());
        h.update(write.address.column.as_bytes());
        hash_content(h, &write.content)?;
        hash_content(h, &write.previous)?;
    }
    Ok(())
}
