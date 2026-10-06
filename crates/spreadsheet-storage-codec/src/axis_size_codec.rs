use super::{CodecError, v1};
use sha2::Digest;
use spreadsheet_domain::{
    axis_size::valid_size,
    workbook::{AxisSizeChange, AxisSizeTarget, ColumnId, RowId, SheetId},
};
use std::collections::BTreeSet;

pub(super) fn encode(
    sheet: SheetId,
    writes: &[AxisSizeChange],
) -> Result<v1::AxisSizeWrite, CodecError> {
    validate(writes)?;
    Ok(v1::AxisSizeWrite {
        sheet_id: sheet.as_bytes().to_vec(),
        writes: writes
            .iter()
            .map(|write| {
                let (axis_kind, axis_id) = match write.target {
                    AxisSizeTarget::Row(id) => (1, id.as_bytes().to_vec()),
                    AxisSizeTarget::Column(id) => (2, id.as_bytes().to_vec()),
                };
                v1::AxisSizeChange {
                    axis_kind,
                    axis_id,
                    size_css_px: write.size,
                    previous_css_px: write.previous,
                }
            })
            .collect(),
    })
}

pub(super) fn decode(
    value: v1::AxisSizeWrite,
) -> Result<(SheetId, Vec<AxisSizeChange>), CodecError> {
    let sheet = SheetId::from_bytes(
        value
            .sheet_id
            .as_slice()
            .try_into()
            .map_err(|_| CodecError::Invalid)?,
    )
    .map_err(|_| CodecError::Invalid)?;
    let writes = value
        .writes
        .into_iter()
        .map(|write| {
            let bytes: [u8; 16] = write
                .axis_id
                .as_slice()
                .try_into()
                .map_err(|_| CodecError::Invalid)?;
            let target = match write.axis_kind {
                1 => {
                    AxisSizeTarget::Row(RowId::from_bytes(bytes).map_err(|_| CodecError::Invalid)?)
                }
                2 => AxisSizeTarget::Column(
                    ColumnId::from_bytes(bytes).map_err(|_| CodecError::Invalid)?,
                ),
                _ => return Err(CodecError::Invalid),
            };
            Ok(AxisSizeChange {
                target,
                size: write.size_css_px,
                previous: write.previous_css_px,
            })
        })
        .collect::<Result<Vec<_>, CodecError>>()?;
    validate(&writes)?;
    Ok((sheet, writes))
}

pub(super) fn validate(writes: &[AxisSizeChange]) -> Result<(), CodecError> {
    if writes.is_empty() || writes.len() > spreadsheet_domain::axis_size::MAX_AXIS_SIZE_WRITES {
        return Err(CodecError::Invalid);
    }
    let mut seen = BTreeSet::new();
    for write in writes {
        if !seen.insert(write.target)
            || write.size == write.previous
            || write
                .size
                .is_some_and(|size| !valid_size(write.target, size))
            || write
                .previous
                .is_some_and(|size| !valid_size(write.target, size))
        {
            return Err(CodecError::Invalid);
        }
    }
    Ok(())
}

pub(super) fn hash(
    hash: &mut sha2::Sha256,
    sheet: SheetId,
    writes: &[AxisSizeChange],
) -> Result<(), CodecError> {
    validate(writes)?;
    hash.update([25]);
    hash.update(sheet.as_bytes());
    hash.update((writes.len() as u64).to_le_bytes());
    for write in writes {
        match write.target {
            AxisSizeTarget::Row(id) => {
                hash.update([1]);
                hash.update(id.as_bytes());
            }
            AxisSizeTarget::Column(id) => {
                hash.update([2]);
                hash.update(id.as_bytes());
            }
        }
        hash_option(hash, write.previous);
        hash_option(hash, write.size);
    }
    Ok(())
}

pub(super) fn hash_command(
    hash: &mut sha2::Sha256,
    sheet: SheetId,
    writes: &[(AxisSizeTarget, Option<u32>)],
) -> Result<(), CodecError> {
    if writes.is_empty() || writes.len() > spreadsheet_domain::axis_size::MAX_AXIS_SIZE_WRITES {
        return Err(CodecError::Invalid);
    }
    hash.update([25]);
    hash.update(sheet.as_bytes());
    hash.update((writes.len() as u64).to_le_bytes());
    let mut seen = BTreeSet::new();
    for (target, size) in writes {
        if !seen.insert(*target) || size.is_some_and(|value| !valid_size(*target, value)) {
            return Err(CodecError::Invalid);
        }
        match target {
            AxisSizeTarget::Row(id) => {
                hash.update([1]);
                hash.update(id.as_bytes());
            }
            AxisSizeTarget::Column(id) => {
                hash.update([2]);
                hash.update(id.as_bytes());
            }
        }
        hash_option(hash, *size);
    }
    Ok(())
}

fn hash_option(hash: &mut sha2::Sha256, value: Option<u32>) {
    match value {
        Some(value) => {
            hash.update([1]);
            hash.update(value.to_le_bytes());
        }
        None => hash.update([0]),
    }
}
