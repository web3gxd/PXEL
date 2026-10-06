//! Public style commands. This adapter never depends on persisted codecs.
use crate::v1;
use prost::Message;
use spreadsheet_domain::style::{self as d, Change, StyleChange};
use thiserror::Error;

pub const STYLE_SCHEMA_VERSION: u32 = 1;
pub const STYLE_PROFILE_VERSION: u32 = 2;
pub const MAX_STYLE_PATCH_BYTES: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StyleCodecError {
    #[error("unsupported wire style schema or profile")]
    UnsupportedSchema,
    #[error("invalid wire style patch")]
    Invalid,
    #[error("wire style patch exceeds limit")]
    TooLarge,
}

type Result<T> = std::result::Result<T, StyleCodecError>;

macro_rules! change_decoder {
    ($name:ident, $wire:ident, $module:ident, $domain:ty, $decode:expr) => {
        fn $name(input: v1::$wire) -> Result<Change<$domain>> {
            match input.action.ok_or(StyleCodecError::Invalid)? {
                v1::$module::Action::Set(value) => ($decode)(value).map(Change::Set),
                v1::$module::Action::Reset(true) => Ok(Change::Reset),
                v1::$module::Action::Reset(false) => Err(StyleCodecError::Invalid),
            }
        }
    };
}
fn color(bytes: Vec<u8>) -> Result<d::Rgb> {
    Ok(d::Rgb::new(
        bytes.try_into().map_err(|_| StyleCodecError::Invalid)?,
    ))
}
fn border(input: v1::StyleBorder) -> Result<d::Border> {
    let tag = u8::try_from(input.line).map_err(|_| StyleCodecError::Invalid)?;
    Ok(d::Border::new(
        d::BorderLine::try_from(tag).map_err(|_| StyleCodecError::Invalid)?,
        color(input.color)?,
    ))
}
fn font_size(value: u32) -> Result<d::FontSize> {
    d::FontSize::new(u16::try_from(value).map_err(|_| StyleCodecError::Invalid)?)
        .map_err(|_| StyleCodecError::Invalid)
}

// Domain's default-valued enums start at zero; wire zero is always unspecified.
fn shifted_enum<T: TryFrom<u8>>(value: i32) -> Result<T> {
    let tag = value
        .checked_sub(1)
        .and_then(|v| u8::try_from(v).ok())
        .ok_or(StyleCodecError::Invalid)?;
    T::try_from(tag).map_err(|_| StyleCodecError::Invalid)
}
change_decoder!(
    family_change,
    FontFamilyChange,
    font_family_change,
    d::FontFamily,
    |value| d::FontFamily::new(value).map_err(|_| StyleCodecError::Invalid)
);
change_decoder!(
    size_change,
    FontSizeChange,
    font_size_change,
    d::FontSize,
    font_size
);
change_decoder!(
    bool_change,
    BooleanStyleChange,
    boolean_style_change,
    bool,
    Ok
);
change_decoder!(
    color_change,
    ColorStyleChange,
    color_style_change,
    d::Rgb,
    color
);
change_decoder!(
    underline_change,
    UnderlineChange,
    underline_change,
    d::Underline,
    shifted_enum::<d::Underline>
);
change_decoder!(
    border_change,
    BorderStyleChange,
    border_style_change,
    d::Border,
    border
);
change_decoder!(
    horizontal_change,
    HorizontalStyleChange,
    horizontal_style_change,
    d::HorizontalAlignment,
    shifted_enum::<d::HorizontalAlignment>
);
change_decoder!(
    vertical_change,
    VerticalStyleChange,
    vertical_style_change,
    d::VerticalAlignment,
    shifted_enum::<d::VerticalAlignment>
);
change_decoder!(
    format_change,
    NumberFormatChange,
    number_format_change,
    d::NumberFormat,
    shifted_enum::<d::NumberFormat>
);
change_decoder!(
    text_overflow_change,
    TextOverflowChange,
    text_overflow_change,
    d::TextOverflow,
    shifted_enum::<d::TextOverflow>
);

pub fn decode_patch(patch: v1::StylePatch) -> Result<Vec<StyleChange>> {
    if patch.schema_version != STYLE_SCHEMA_VERSION
        || !matches!(patch.profile_version, 1 | STYLE_PROFILE_VERSION)
    {
        return Err(StyleCodecError::UnsupportedSchema);
    }
    if patch.changes.len() > d::STYLE_PROPERTY_COUNT || patch.encoded_len() > MAX_STYLE_PATCH_BYTES
    {
        return Err(StyleCodecError::TooLarge);
    }
    if patch.changes.is_empty() {
        return Err(StyleCodecError::Invalid);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut changes = Vec::with_capacity(patch.changes.len());
    for change in patch.changes {
        use v1::style_change::Kind as K;
        let change = match change.kind.ok_or(StyleCodecError::Invalid)? {
            K::FontFamily(value) => StyleChange::FontFamily(family_change(value)?),
            K::FontSize(value) => StyleChange::FontSize(size_change(value)?),
            K::FontBold(value) => StyleChange::FontBold(bool_change(value)?),
            K::FontItalic(value) => StyleChange::FontItalic(bool_change(value)?),
            K::FontUnderline(value) => StyleChange::FontUnderline(underline_change(value)?),
            K::FontStrike(value) => StyleChange::FontStrike(bool_change(value)?),
            K::FontColor(value) => StyleChange::FontColor(color_change(value)?),
            K::Fill(value) => StyleChange::Fill(color_change(value)?),
            K::BorderTop(value) => StyleChange::BorderTop(border_change(value)?),
            K::BorderRight(value) => StyleChange::BorderRight(border_change(value)?),
            K::BorderBottom(value) => StyleChange::BorderBottom(border_change(value)?),
            K::BorderLeft(value) => StyleChange::BorderLeft(border_change(value)?),
            K::Horizontal(value) => StyleChange::Horizontal(horizontal_change(value)?),
            K::Vertical(value) => StyleChange::Vertical(vertical_change(value)?),
            K::Wrap(value) => match bool_change(value)? {
                Change::Set(true) => StyleChange::TextOverflow(Change::Set(d::TextOverflow::Wrap)),
                Change::Set(false) => StyleChange::TextOverflow(Change::Set(d::TextOverflow::Clip)),
                Change::Reset => StyleChange::TextOverflow(Change::Reset),
            },
            K::NumberFormat(value) => StyleChange::NumberFormat(format_change(value)?),
            K::TextOverflow(value) => {
                if patch.profile_version < STYLE_PROFILE_VERSION {
                    return Err(StyleCodecError::Invalid);
                }
                StyleChange::TextOverflow(text_overflow_change(value)?)
            }
        };
        if !seen.insert(change.property()) {
            return Err(StyleCodecError::Invalid);
        }
        changes.push(change);
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::style::{CellStyle, Change, StyleProperty};

    fn patch(changes: Vec<v1::style_change::Kind>) -> v1::StylePatch {
        v1::StylePatch {
            schema_version: STYLE_SCHEMA_VERSION,
            profile_version: STYLE_PROFILE_VERSION,
            changes: changes
                .into_iter()
                .map(|kind| v1::StyleChange { kind: Some(kind) })
                .collect(),
        }
    }

    fn bold(value: bool) -> v1::style_change::Kind {
        v1::style_change::Kind::FontBold(v1::BooleanStyleChange {
            action: Some(v1::boolean_style_change::Action::Set(value)),
        })
    }

    fn all_leaves() -> Vec<v1::style_change::Kind> {
        use v1::style_change::Kind as K;
        let boolean = v1::BooleanStyleChange {
            action: Some(v1::boolean_style_change::Action::Set(true)),
        };
        let color = v1::ColorStyleChange {
            action: Some(v1::color_style_change::Action::Set(vec![12, 34, 56])),
        };
        let border = v1::BorderStyleChange {
            action: Some(v1::border_style_change::Action::Set(v1::StyleBorder {
                line: 1,
                color: vec![65, 43, 21],
            })),
        };
        vec![
            K::FontFamily(v1::FontFamilyChange {
                action: Some(v1::font_family_change::Action::Set("Noto Sans".into())),
            }),
            K::FontSize(v1::FontSizeChange {
                action: Some(v1::font_size_change::Action::Set(1250)),
            }),
            K::FontBold(boolean),
            K::FontItalic(boolean),
            K::FontUnderline(v1::UnderlineChange {
                action: Some(v1::underline_change::Action::Set(3)),
            }),
            K::FontStrike(boolean),
            K::FontColor(color.clone()),
            K::Fill(color),
            K::BorderTop(border.clone()),
            K::BorderRight(border.clone()),
            K::BorderBottom(border.clone()),
            K::BorderLeft(border),
            K::Horizontal(v1::HorizontalStyleChange {
                action: Some(v1::horizontal_style_change::Action::Set(3)),
            }),
            K::Vertical(v1::VerticalStyleChange {
                action: Some(v1::vertical_style_change::Action::Set(1)),
            }),
            K::Wrap(boolean),
            K::NumberFormat(v1::NumberFormatChange {
                action: Some(v1::number_format_change::Action::Set(7)),
            }),
        ]
    }

    #[test]
    fn all_sixteen_typed_leaves_decode_without_losing_the_footprint()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let decoded = decode_patch(patch(all_leaves()))?;
        assert_eq!(decoded.len(), 16);
        for (index, change) in decoded.iter().enumerate() {
            assert_eq!(change.property() as usize, index + 1);
        }
        let style = CellStyle::default().apply(&decoded)?;
        assert_eq!(style.font_family(), "Noto Sans");
        assert_eq!(style.font_size().get(), 1250);
        assert_eq!(style.font_color().channels(), [12, 34, 56]);
        assert!(style.font_bold());
        assert_eq!(
            style.number_format(),
            spreadsheet_domain::style::NumberFormat::DateIso
        );
        assert_eq!(
            decode_patch(patch(vec![bold(false)]))?,
            vec![StyleChange::FontBold(Change::Set(false))]
        );
        Ok(())
    }

    #[test]
    fn reset_is_present_and_never_confused_with_omission_or_false() -> Result<()> {
        let reset = v1::style_change::Kind::FontBold(v1::BooleanStyleChange {
            action: Some(v1::boolean_style_change::Action::Reset(true)),
        });
        assert_eq!(
            decode_patch(patch(vec![reset]))?,
            vec![StyleChange::FontBold(Change::Reset)]
        );
        for invalid in [None, Some(v1::boolean_style_change::Action::Reset(false))] {
            assert_eq!(
                decode_patch(patch(vec![v1::style_change::Kind::FontBold(
                    v1::BooleanStyleChange { action: invalid }
                )])),
                Err(StyleCodecError::Invalid)
            );
        }
        Ok(())
    }

    #[test]
    fn every_reset_and_binary_roundtrip_preserves_typed_properties() -> Result<()> {
        use v1::style_change::Kind as K;
        let boolean = v1::BooleanStyleChange {
            action: Some(v1::boolean_style_change::Action::Reset(true)),
        };
        let color = v1::ColorStyleChange {
            action: Some(v1::color_style_change::Action::Reset(true)),
        };
        let border = v1::BorderStyleChange {
            action: Some(v1::border_style_change::Action::Reset(true)),
        };
        let reset = patch(vec![
            K::FontFamily(v1::FontFamilyChange {
                action: Some(v1::font_family_change::Action::Reset(true)),
            }),
            K::FontSize(v1::FontSizeChange {
                action: Some(v1::font_size_change::Action::Reset(true)),
            }),
            K::FontBold(boolean),
            K::FontItalic(boolean),
            K::FontUnderline(v1::UnderlineChange {
                action: Some(v1::underline_change::Action::Reset(true)),
            }),
            K::FontStrike(boolean),
            K::FontColor(color.clone()),
            K::Fill(color),
            K::BorderTop(border.clone()),
            K::BorderRight(border.clone()),
            K::BorderBottom(border.clone()),
            K::BorderLeft(border),
            K::Horizontal(v1::HorizontalStyleChange {
                action: Some(v1::horizontal_style_change::Action::Reset(true)),
            }),
            K::Vertical(v1::VerticalStyleChange {
                action: Some(v1::vertical_style_change::Action::Reset(true)),
            }),
            K::Wrap(boolean),
            K::NumberFormat(v1::NumberFormatChange {
                action: Some(v1::number_format_change::Action::Reset(true)),
            }),
        ]);
        let default = CellStyle::default();
        for input in [patch(all_leaves()), reset.clone()] {
            let bytes = input.encode_to_vec();
            let decoded =
                v1::StylePatch::decode(bytes.as_slice()).map_err(|_| StyleCodecError::Invalid)?;
            assert_eq!(decode_patch(decoded)?, decode_patch(input)?);
        }
        for change in decode_patch(reset)? {
            assert_eq!(change, default.change(change.property()));
        }
        // Unknown future oneof semantics decode to None and fail as a whole patch.
        let unknown = v1::StyleChange::decode([0x8a, 0x01, 0x00].as_slice())
            .map_err(|_| StyleCodecError::Invalid)?;
        assert_eq!(
            decode_patch(v1::StylePatch {
                changes: vec![unknown],
                ..patch(vec![])
            }),
            Err(StyleCodecError::Invalid)
        );
        Ok(())
    }

    #[test]
    fn unknown_versions_empty_and_duplicate_properties_are_atomic_errors() {
        let valid = patch(vec![bold(true)]);
        for (schema_version, profile_version) in [(0, 1), (2, 1), (1, 0), (1, 3)] {
            assert_eq!(
                decode_patch(v1::StylePatch {
                    schema_version,
                    profile_version,
                    ..valid.clone()
                }),
                Err(StyleCodecError::UnsupportedSchema)
            );
        }
        assert_eq!(decode_patch(patch(vec![])), Err(StyleCodecError::Invalid));
        assert_eq!(
            decode_patch(patch(vec![bold(true), bold(false)])),
            Err(StyleCodecError::Invalid)
        );
        assert_eq!(
            decode_patch(patch(vec![bold(true); 17])),
            Err(StyleCodecError::TooLarge)
        );
        let mut missing = valid;
        missing.changes.push(v1::StyleChange { kind: None });
        assert_eq!(decode_patch(missing), Err(StyleCodecError::Invalid));
    }

    #[test]
    fn legacy_wrap_and_profile_two_overflow_decode_to_the_same_style_leaf() {
        use v1::style_change::Kind as K;
        let legacy = v1::StylePatch {
            schema_version: STYLE_SCHEMA_VERSION,
            profile_version: 1,
            changes: vec![v1::StyleChange {
                kind: Some(K::Wrap(v1::BooleanStyleChange {
                    action: Some(v1::boolean_style_change::Action::Set(true)),
                })),
            }],
        };
        assert_eq!(
            decode_patch(legacy),
            Ok(vec![StyleChange::TextOverflow(Change::Set(
                d::TextOverflow::Wrap
            ))])
        );

        let overflow = v1::StylePatch {
            schema_version: STYLE_SCHEMA_VERSION,
            profile_version: STYLE_PROFILE_VERSION,
            changes: vec![v1::StyleChange {
                kind: Some(K::TextOverflow(v1::TextOverflowChange {
                    action: Some(v1::text_overflow_change::Action::Set(3)),
                })),
            }],
        };
        assert_eq!(
            decode_patch(overflow.clone()),
            Ok(vec![StyleChange::TextOverflow(Change::Set(
                d::TextOverflow::Overflow
            ))])
        );
        let mut downgraded = overflow;
        downgraded.profile_version = 1;
        assert_eq!(decode_patch(downgraded), Err(StyleCodecError::Invalid));
    }

    #[test]
    fn typed_font_rgb_and_closed_enums_reject_malformed_input() {
        use v1::style_change::Kind as K;
        let mut invalid = vec![];
        for value in [0, 99, 20001, u32::MAX] {
            invalid.push(K::FontSize(v1::FontSizeChange {
                action: Some(v1::font_size_change::Action::Set(value)),
            }));
        }
        for value in ["", " Arial", "Arial ", "line\nbreak", &"x".repeat(129)] {
            invalid.push(K::FontFamily(v1::FontFamilyChange {
                action: Some(v1::font_family_change::Action::Set(value.to_owned())),
            }));
        }
        for value in [vec![], vec![1, 2], vec![1, 2, 3, 4]] {
            invalid.push(K::Fill(v1::ColorStyleChange {
                action: Some(v1::color_style_change::Action::Set(value)),
            }));
        }
        for value in [0, -1, 255, i32::MAX] {
            invalid.push(K::FontUnderline(v1::UnderlineChange {
                action: Some(v1::underline_change::Action::Set(value)),
            }));
            invalid.push(K::Horizontal(v1::HorizontalStyleChange {
                action: Some(v1::horizontal_style_change::Action::Set(value)),
            }));
            invalid.push(K::Vertical(v1::VerticalStyleChange {
                action: Some(v1::vertical_style_change::Action::Set(value)),
            }));
            invalid.push(K::NumberFormat(v1::NumberFormatChange {
                action: Some(v1::number_format_change::Action::Set(value)),
            }));
            invalid.push(K::BorderTop(v1::BorderStyleChange {
                action: Some(v1::border_style_change::Action::Set(v1::StyleBorder {
                    line: value,
                    color: vec![1, 2, 3],
                })),
            }));
        }
        for kind in invalid {
            assert_eq!(
                decode_patch(patch(vec![bold(true), kind])),
                Err(StyleCodecError::Invalid)
            );
        }
        let large = K::FontFamily(v1::FontFamilyChange {
            action: Some(v1::font_family_change::Action::Set(
                "x".repeat(MAX_STYLE_PATCH_BYTES),
            )),
        });
        assert_eq!(
            decode_patch(patch(vec![large])),
            Err(StyleCodecError::TooLarge)
        );
        assert_eq!(StyleProperty::NumberFormat as u8, 16);
    }
}
