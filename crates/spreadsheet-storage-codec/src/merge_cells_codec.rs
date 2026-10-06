use super::*;

fn range_operation(writes: &[GuardedContentWrite]) -> Operation {
    Operation::RangeWrite {
        writes: writes.to_vec(),
        kind: Some(RangeWriteKind::Copy),
        undoes: None,
        undoes_version: None,
    }
}

pub(super) fn validate(op: &Operation, version: Option<DocumentVersion>) -> Result<(), CodecError> {
    let Operation::MergeCellsWrite {
        sheet,
        change,
        content_writes,
        style_writes,
        undoes,
        undoes_version,
    } = op
    else {
        return Ok(());
    };
    if change.merged == change.previous
        || undoes.is_some() != undoes_version.is_some()
        || undoes_version.is_some_and(|v| {
            v == DocumentVersion::ZERO || version.is_some_and(|current| v >= current)
        })
        || content_writes.len() > 1_000
        || style_writes.len() > 1_000
        || content_writes.iter().any(|w| w.address.sheet != *sheet)
        || style_writes.iter().any(|w| w.address.sheet != *sheet)
        || content_writes
            .windows(2)
            .any(|p| p[0].address >= p[1].address)
    {
        return Err(CodecError::Invalid);
    }
    let targets = content_writes
        .iter()
        .map(|write| write.address)
        .chain(style_writes.iter().map(|write| write.address))
        .collect::<std::collections::BTreeSet<_>>();
    if targets.len() > 1_000 {
        return Err(CodecError::Invalid);
    }
    if undoes.is_none()
        && (!change.merged
            || change.previous
            || content_writes.iter().any(|write| write.content.is_some())
            || style_writes
                .iter()
                .flat_map(|write| &write.changes)
                .any(|change| {
                    *change
                        != spreadsheet_domain::style::CellStyle::default().change(change.property())
                }))
    {
        return Err(CodecError::Invalid);
    }
    if !content_writes.is_empty() {
        range_codec::validate(&range_operation(content_writes), version)?;
    }
    if !style_writes.is_empty() {
        style_codec::validate(
            &Operation::StyleWrite {
                kind: StyleWriteKind::Range,
                writes: style_writes.clone(),
            },
            version,
        )?;
    }
    Ok(())
}

pub(super) fn encode(op: &Operation) -> Result<v1::MergeCellsWrite, CodecError> {
    validate(op, None)?;
    let Operation::MergeCellsWrite {
        sheet,
        change,
        content_writes,
        style_writes,
        undoes,
        undoes_version,
    } = op
    else {
        return Err(CodecError::Invalid);
    };
    Ok(v1::MergeCellsWrite {
        merge: Some(v1::MergeWrite {
            sheet_id: sheet.as_bytes().to_vec(),
            range: Some(encode_merge_range(change.range)),
            merged: change.merged,
            previous: change.previous,
            undoes_operation_id: undoes.map_or_else(Vec::new, |id| id.as_bytes().to_vec()),
            undoes_version: undoes_version.map_or(0, DocumentVersion::get),
        }),
        content_writes: if content_writes.is_empty() {
            Vec::new()
        } else {
            range_codec::encode(&range_operation(content_writes))?.writes
        },
        styles: if style_writes.is_empty() {
            None
        } else {
            Some(style_codec::encode_write(
                StyleWriteKind::Range,
                style_writes,
            )?)
        },
    })
}

pub(super) fn decode(dto: v1::MergeCellsWrite) -> Result<Operation, CodecError> {
    if dto.content_writes.len() > 1_000
        || dto.styles.as_ref().is_some_and(|s| s.writes.len() > 1_000)
    {
        return Err(CodecError::Invalid);
    }
    let Operation::MergeWrite {
        sheet,
        change,
        undoes,
        undoes_version,
    } = decode_operation(v1::Operation {
        kind: Some(v1::operation::Kind::MergeWrite(
            dto.merge.ok_or(CodecError::Invalid)?,
        )),
    })?
    else {
        return Err(CodecError::Invalid);
    };
    let content_writes = if dto.content_writes.is_empty() {
        Vec::new()
    } else {
        let Operation::RangeWrite { writes, .. } = range_codec::decode(
            v1::RangeWrite {
                writes: dto.content_writes,
                undoes_operation_id: Vec::new(),
                undoes_version: 0,
                authored_kind: 2,
            },
            false,
        )?
        else {
            return Err(CodecError::Invalid);
        };
        writes
    };
    let style_writes = if let Some(styles) = dto.styles {
        let (kind, writes) = style_codec::decode_write(styles)?;
        if kind != StyleWriteKind::Range {
            return Err(CodecError::Invalid);
        }
        writes
    } else {
        Vec::new()
    };
    let op = Operation::MergeCellsWrite {
        sheet,
        change,
        content_writes,
        style_writes,
        undoes,
        undoes_version,
    };
    validate(&op, None)?;
    Ok(op)
}

pub(super) fn hash(h: &mut Sha256, op: &Operation) -> Result<(), CodecError> {
    validate(op, None)?;
    let Operation::MergeCellsWrite {
        sheet,
        change,
        content_writes,
        style_writes,
        undoes,
        undoes_version,
    } = op
    else {
        return Err(CodecError::Invalid);
    };
    h.update([schema_version(op) as u8]);
    h.update(sheet.as_bytes());
    h.update(change.range.first_row().as_bytes());
    h.update(change.range.last_row().as_bytes());
    h.update(change.range.first_column().as_bytes());
    h.update(change.range.last_column().as_bytes());
    h.update([u8::from(change.merged), u8::from(change.previous)]);
    h.update((content_writes.len() as u64).to_le_bytes());
    if !content_writes.is_empty() {
        range_codec::hash(h, &range_operation(content_writes))?;
    }
    h.update((style_writes.len() as u64).to_le_bytes());
    if !style_writes.is_empty() {
        style_codec::hash(
            h,
            &Operation::StyleWrite {
                kind: StyleWriteKind::Range,
                writes: style_writes.clone(),
            },
        )?;
    }
    if let (Some(id), Some(version)) = (undoes, undoes_version) {
        h.update(id.as_bytes());
        h.update(version.get().to_le_bytes());
    }
    Ok(())
}
