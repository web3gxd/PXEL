use super::*;

pub(super) fn encode(operation: &Operation) -> Result<v1::MoveCompensation, CodecError> {
    move_codec::validate(operation, None)?;
    let Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes,
        undoes,
        undoes_version,
    } = operation
    else {
        return Err(CodecError::Invalid);
    };
    Ok(v1::MoveCompensation {
        source: Some(addr(source)),
        destination: Some(addr(destination)),
        rows: *rows,
        columns: *columns,
        writes: move_codec::encode_writes(writes)?,
        undoes_operation_id: undoes.as_bytes().to_vec(),
        undoes_version: undoes_version.get(),
    })
}
pub(super) fn decode(comp: v1::MoveCompensation) -> Result<Operation, CodecError> {
    let undoes =
        OperationId::from_bytes(id(&comp.undoes_operation_id)?).map_err(|_| CodecError::Invalid)?;
    let undoes_version =
        DocumentVersion::new(comp.undoes_version).map_err(|_| CodecError::Invalid)?;
    let moved = move_codec::decode(v1::MoveRange {
        source: comp.source,
        destination: comp.destination,
        rows: comp.rows,
        columns: comp.columns,
        writes: comp.writes,
    })?;
    let Operation::MoveRange {
        source,
        destination,
        rows,
        columns,
        writes,
    } = moved
    else {
        return Err(CodecError::Invalid);
    };
    let operation = Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes,
        undoes,
        undoes_version,
    };
    move_codec::validate(&operation, None)?;
    Ok(operation)
}
pub(super) fn hash(h: &mut Sha256, operation: &Operation) -> Result<(), CodecError> {
    move_codec::validate(operation, None)?;
    let Operation::MoveCompensation {
        source,
        destination,
        rows,
        columns,
        writes,
        undoes,
        undoes_version,
    } = operation
    else {
        return Err(CodecError::Invalid);
    };
    h.update([22]);
    move_codec::hash_geometry(h, *source, *destination, *rows, *columns);
    move_codec::hash_writes(h, writes)?;
    h.update(undoes.as_bytes());
    h.update(undoes_version.get().to_le_bytes());
    Ok(())
}
