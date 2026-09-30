use super::*;
use spreadsheet_domain::{style::*, style_write::MAX_STYLE_TARGETS};

fn canonical(patch: &[StyleChange]) -> Result<Vec<StyleChange>, CodecError> {
    let style = CellStyle::default()
        .apply(patch)
        .map_err(|_| CodecError::Invalid)?;
    let mut properties = patch.iter().map(StyleChange::property).collect::<Vec<_>>();
    properties.sort();
    Ok(properties
        .into_iter()
        .map(|property| style.change(property))
        .collect())
}
fn validate_patch(patch: &[StyleChange]) -> Result<(), CodecError> {
    if canonical(patch)? != patch {
        return Err(CodecError::Invalid);
    }
    Ok(())
}
fn rgb(value: Rgb) -> u32 {
    let [r, g, b] = value.channels();
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}
fn decode_rgb(value: u32) -> Result<Rgb, CodecError> {
    if value > 0xff_ffff {
        return Err(CodecError::Invalid);
    }
    let bytes = value.to_be_bytes();
    Ok(Rgb::new([bytes[1], bytes[2], bytes[3]]))
}
pub(super) fn encode_change(change: &StyleChange) -> v1::StyleChange {
    use v1::style_change::Kind;
    let kind = match change {
        StyleChange::FontFamily(Change::Set(value)) => Kind::FontFamily(value.as_str().into()),
        StyleChange::FontSize(Change::Set(value)) => Kind::FontSize(u32::from(value.get())),
        StyleChange::FontBold(Change::Set(value))
        | StyleChange::FontItalic(Change::Set(value))
        | StyleChange::FontStrike(Change::Set(value))
        | StyleChange::Wrap(Change::Set(value)) => Kind::Boolean(*value),
        StyleChange::FontUnderline(Change::Set(value)) => Kind::Enumeration(*value as u32),
        StyleChange::Horizontal(Change::Set(value)) => Kind::Enumeration(*value as u32),
        StyleChange::Vertical(Change::Set(value)) => Kind::Enumeration(*value as u32),
        StyleChange::NumberFormat(Change::Set(value)) => Kind::Enumeration(*value as u32),
        StyleChange::FontColor(Change::Set(value)) | StyleChange::Fill(Change::Set(value)) => {
            Kind::Rgb(rgb(*value))
        }
        StyleChange::BorderTop(Change::Set(value))
        | StyleChange::BorderRight(Change::Set(value))
        | StyleChange::BorderBottom(Change::Set(value))
        | StyleChange::BorderLeft(Change::Set(value)) => Kind::Border(v1::StyleBorder {
            line: value.line() as u32,
            rgb: rgb(value.color()),
        }),
        StyleChange::FontFamily(Change::Reset)
        | StyleChange::FontSize(Change::Reset)
        | StyleChange::FontBold(Change::Reset)
        | StyleChange::FontItalic(Change::Reset)
        | StyleChange::FontUnderline(Change::Reset)
        | StyleChange::FontStrike(Change::Reset)
        | StyleChange::FontColor(Change::Reset)
        | StyleChange::Fill(Change::Reset)
        | StyleChange::BorderTop(Change::Reset)
        | StyleChange::BorderRight(Change::Reset)
        | StyleChange::BorderBottom(Change::Reset)
        | StyleChange::BorderLeft(Change::Reset)
        | StyleChange::Horizontal(Change::Reset)
        | StyleChange::Vertical(Change::Reset)
        | StyleChange::Wrap(Change::Reset)
        | StyleChange::NumberFormat(Change::Reset) => Kind::Reset(true),
    };
    v1::StyleChange {
        property: change.property() as u32,
        kind: Some(kind),
    }
}
pub(super) fn decode_change(change: v1::StyleChange) -> Result<StyleChange, CodecError> {
    use v1::style_change::Kind;
    let property =
        StyleProperty::try_from(u8::try_from(change.property).map_err(|_| CodecError::Invalid)?)
            .map_err(|_| CodecError::Invalid)?;
    let kind = change.kind.ok_or(CodecError::Invalid)?;
    if matches!(kind, Kind::Reset(true)) {
        return Ok(CellStyle::default().change(property));
    }
    let value = match (property, kind) {
        (StyleProperty::FontFamily, Kind::FontFamily(value)) => StyleChange::FontFamily(
            Change::Set(FontFamily::new(value).map_err(|_| CodecError::Invalid)?),
        ),
        (StyleProperty::FontSize, Kind::FontSize(value)) => StyleChange::FontSize(Change::Set(
            FontSize::new(u16::try_from(value).map_err(|_| CodecError::Invalid)?)
                .map_err(|_| CodecError::Invalid)?,
        )),
        (StyleProperty::FontBold, Kind::Boolean(value)) => {
            StyleChange::FontBold(Change::Set(value))
        }
        (StyleProperty::FontItalic, Kind::Boolean(value)) => {
            StyleChange::FontItalic(Change::Set(value))
        }
        (StyleProperty::FontStrike, Kind::Boolean(value)) => {
            StyleChange::FontStrike(Change::Set(value))
        }
        (StyleProperty::Wrap, Kind::Boolean(value)) => StyleChange::Wrap(Change::Set(value)),
        (StyleProperty::FontUnderline, Kind::Enumeration(value)) => {
            StyleChange::FontUnderline(Change::Set(
                Underline::try_from(u8::try_from(value).map_err(|_| CodecError::Invalid)?)
                    .map_err(|_| CodecError::Invalid)?,
            ))
        }
        (StyleProperty::Horizontal, Kind::Enumeration(value)) => {
            StyleChange::Horizontal(Change::Set(
                HorizontalAlignment::try_from(
                    u8::try_from(value).map_err(|_| CodecError::Invalid)?,
                )
                .map_err(|_| CodecError::Invalid)?,
            ))
        }
        (StyleProperty::Vertical, Kind::Enumeration(value)) => StyleChange::Vertical(Change::Set(
            VerticalAlignment::try_from(u8::try_from(value).map_err(|_| CodecError::Invalid)?)
                .map_err(|_| CodecError::Invalid)?,
        )),
        (StyleProperty::NumberFormat, Kind::Enumeration(value)) => {
            StyleChange::NumberFormat(Change::Set(
                NumberFormat::try_from(u8::try_from(value).map_err(|_| CodecError::Invalid)?)
                    .map_err(|_| CodecError::Invalid)?,
            ))
        }
        (StyleProperty::FontColor, Kind::Rgb(value)) => {
            StyleChange::FontColor(Change::Set(decode_rgb(value)?))
        }
        (StyleProperty::Fill, Kind::Rgb(value)) => {
            StyleChange::Fill(Change::Set(decode_rgb(value)?))
        }
        (property, Kind::Border(value)) => {
            let border = Border::new(
                BorderLine::try_from(u8::try_from(value.line).map_err(|_| CodecError::Invalid)?)
                    .map_err(|_| CodecError::Invalid)?,
                decode_rgb(value.rgb)?,
            );
            match property {
                StyleProperty::BorderTop => StyleChange::BorderTop(Change::Set(border)),
                StyleProperty::BorderRight => StyleChange::BorderRight(Change::Set(border)),
                StyleProperty::BorderBottom => StyleChange::BorderBottom(Change::Set(border)),
                StyleProperty::BorderLeft => StyleChange::BorderLeft(Change::Set(border)),
                _ => return Err(CodecError::Invalid),
            }
        }
        _ => return Err(CodecError::Invalid),
    };
    Ok(value)
}
fn decode_patch(changes: Vec<v1::StyleChange>) -> Result<Vec<StyleChange>, CodecError> {
    if changes.is_empty() || changes.len() > STYLE_PROPERTY_COUNT {
        return Err(CodecError::Invalid);
    }
    let changes = changes
        .into_iter()
        .map(decode_change)
        .collect::<Result<Vec<_>, _>>()?;
    validate_patch(&changes)?;
    Ok(changes)
}
pub(super) fn encode_style(style: &CellStyle) -> v1::CellStyle {
    v1::CellStyle {
        profile_version: u32::from(DEFAULT_PROFILE_VERSION),
        changes: style
            .normalized_changes()
            .iter()
            .map(encode_change)
            .collect(),
    }
}
pub(super) fn decode_style(style: v1::CellStyle) -> Result<CellStyle, CodecError> {
    if style.profile_version != u32::from(DEFAULT_PROFILE_VERSION) {
        return Err(CodecError::UnsupportedSchema);
    }
    let changes = decode_patch(style.changes)?;
    let value = CellStyle::default()
        .apply(&changes)
        .map_err(|_| CodecError::Invalid)?;
    if value.is_default() || value.normalized_changes() != changes {
        return Err(CodecError::Invalid);
    }
    Ok(value)
}
fn validate_writes(kind: StyleWriteKind, writes: &[GuardedStyleWrite]) -> Result<(), CodecError> {
    if writes.is_empty()
        || writes.len() > MAX_STYLE_TARGETS
        || (kind == StyleWriteKind::Cell && writes.len() != 1)
        || writes
            .windows(2)
            .any(|pair| pair[0].address >= pair[1].address)
        || writes
            .iter()
            .any(|write| write.address.sheet != writes[0].address.sheet)
    {
        return Err(CodecError::Invalid);
    }
    for write in writes {
        validate_patch(&write.previous)?;
        validate_patch(&write.changes)?;
        if !write
            .previous
            .iter()
            .map(StyleChange::property)
            .eq(write.changes.iter().map(StyleChange::property))
        {
            return Err(CodecError::Invalid);
        }
    }
    Ok(())
}
pub(super) fn validate(
    operation: &Operation,
    version: Option<DocumentVersion>,
) -> Result<(), CodecError> {
    let (kind, writes) = match operation {
        Operation::StyleWrite { kind, writes } => (*kind, writes),
        Operation::StyleCompensation {
            kind,
            writes,
            undoes_version,
            ..
        } => {
            if *undoes_version == DocumentVersion::ZERO
                || version.is_some_and(|v| *undoes_version >= v)
            {
                return Err(CodecError::Invalid);
            }
            (*kind, writes)
        }
        _ => return Ok(()),
    };
    if version == Some(DocumentVersion::ZERO) {
        return Err(CodecError::Invalid);
    }
    validate_writes(kind, writes)
}
pub(super) fn encode_write(
    kind: StyleWriteKind,
    writes: &[GuardedStyleWrite],
) -> Result<v1::StyleWrite, CodecError> {
    validate_writes(kind, writes)?;
    Ok(v1::StyleWrite {
        kind: kind as u32,
        writes: writes
            .iter()
            .map(|write| v1::GuardedStyleWrite {
                address: Some(addr(&write.address)),
                previous: write.previous.iter().map(encode_change).collect(),
                changes: write.changes.iter().map(encode_change).collect(),
            })
            .collect(),
    })
}
pub(super) fn decode_write(
    write: v1::StyleWrite,
) -> Result<(StyleWriteKind, Vec<GuardedStyleWrite>), CodecError> {
    let kind = match write.kind {
        1 => StyleWriteKind::Cell,
        2 => StyleWriteKind::Range,
        _ => return Err(CodecError::Invalid),
    };
    if write.writes.is_empty() || write.writes.len() > MAX_STYLE_TARGETS {
        return Err(CodecError::Invalid);
    }
    let writes = write
        .writes
        .into_iter()
        .map(|write| {
            Ok(GuardedStyleWrite {
                address: decode_addr(write.address.ok_or(CodecError::Invalid)?)?,
                previous: decode_patch(write.previous)?,
                changes: decode_patch(write.changes)?,
            })
        })
        .collect::<Result<Vec<_>, CodecError>>()?;
    validate_writes(kind, &writes)?;
    Ok((kind, writes))
}
fn hash_patch(hash: &mut Sha256, patch: &[StyleChange]) -> Result<(), CodecError> {
    validate_patch(patch)?;
    hash.update((patch.len() as u32).to_le_bytes());
    for change in patch {
        hash.update([change.property() as u8]);
    }
    let style = CellStyle::default()
        .apply(patch)
        .map_err(|_| CodecError::Invalid)?;
    let mut bytes = Vec::new();
    style.append_to(&mut bytes);
    field(hash, &bytes);
    Ok(())
}
pub(super) fn hash(hash: &mut Sha256, operation: &Operation) -> Result<(), CodecError> {
    validate(operation, None)?;
    let (tag, kind, writes) = match operation {
        Operation::StyleWrite { kind, writes } => (25, *kind, writes),
        Operation::StyleCompensation { kind, writes, .. } => (26, *kind, writes),
        _ => return Err(CodecError::Invalid),
    };
    hash.update([tag, kind as u8]);
    hash.update((writes.len() as u32).to_le_bytes());
    for write in writes {
        hash.update(write.address.sheet.as_bytes());
        hash.update(write.address.row.as_bytes());
        hash.update(write.address.column.as_bytes());
        hash_patch(hash, &write.previous)?;
        hash_patch(hash, &write.changes)?;
    }
    if let Operation::StyleCompensation {
        undoes,
        undoes_version,
        ..
    } = operation
    {
        hash.update(undoes.as_bytes());
        hash.update(undoes_version.get().to_le_bytes());
    }
    Ok(())
}
pub(super) fn hash_command(
    hash: &mut Sha256,
    kind: StyleWriteKind,
    addresses: &[CellAddress],
    patch: &[StyleChange],
) -> Result<(), CodecError> {
    if addresses.is_empty()
        || addresses.len() > MAX_STYLE_TARGETS
        || (kind == StyleWriteKind::Cell && addresses.len() != 1)
    {
        return Err(CodecError::Invalid);
    }
    let mut addresses = addresses.to_vec();
    addresses.sort();
    if addresses.windows(2).any(|pair| pair[0] == pair[1])
        || addresses
            .iter()
            .any(|address| address.sheet != addresses[0].sheet)
    {
        return Err(CodecError::Invalid);
    }
    let patch = canonical(patch)?;
    hash.update([18, kind as u8]);
    hash.update((addresses.len() as u32).to_le_bytes());
    for address in addresses {
        hash.update(address.sheet.as_bytes());
        hash.update(address.row.as_bytes());
        hash.update(address.column.as_bytes());
    }
    hash_patch(hash, &patch)
}
