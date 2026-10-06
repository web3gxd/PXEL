use super::*;
use spreadsheet_domain::sort::{SortKey, SortRange};

pub(super) fn encode_range(range: &SortRange) -> Result<v1::SortRangeIntent, CodecError> {
    range.validate_shape().map_err(|_| CodecError::Invalid)?;
    Ok(v1::SortRangeIntent {
        source: Some(addr(&range.source)),
        rows: range.rows,
        columns: range.columns,
        header_rows: range.header_rows,
        keys: range
            .keys
            .iter()
            .map(|key| v1::SortKey {
                column_id: key.column.as_bytes().to_vec(),
                descending: key.descending,
            })
            .collect(),
    })
}

pub(super) fn decode_range(range: v1::SortRangeIntent) -> Result<SortRange, CodecError> {
    let range = SortRange {
        source: decode_addr(range.source.ok_or(CodecError::Invalid)?)?,
        rows: range.rows,
        columns: range.columns,
        header_rows: range.header_rows,
        keys: range
            .keys
            .into_iter()
            .map(|key| {
                Ok(SortKey {
                    column: ColumnId::from_bytes(id(&key.column_id)?)
                        .map_err(|_| CodecError::Invalid)?,
                    descending: key.descending,
                })
            })
            .collect::<Result<_, CodecError>>()?,
    };
    range.validate_shape().map_err(|_| CodecError::Invalid)?;
    Ok(range)
}

pub(super) fn hash_range(hash: &mut Sha256, range: &SortRange) -> Result<(), CodecError> {
    range.validate_shape().map_err(|_| CodecError::Invalid)?;
    hash.update(range.source.sheet.as_bytes());
    hash.update(range.source.row.as_bytes());
    hash.update(range.source.column.as_bytes());
    hash.update(range.rows.to_le_bytes());
    hash.update(range.columns.to_le_bytes());
    hash.update(range.header_rows.to_le_bytes());
    hash.update((range.keys.len() as u32).to_le_bytes());
    for key in &range.keys {
        hash.update(key.column.as_bytes());
        hash.update([u8::from(key.descending)]);
    }
    Ok(())
}
