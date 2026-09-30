use proptest::prelude::*;
use spreadsheet_domain::style::*;
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn frozen_defaults_and_reset_remove_sparse_overrides() -> TestResult {
    let default = CellStyle::default();
    assert!(default.is_default());
    assert_eq!(default.font_family(), "Arial");
    assert_eq!(default.font_size().get(), 1000);
    assert_eq!(default.font_color(), Rgb::new([0; 3]));
    assert_eq!(default.fill(), Rgb::new([255; 3]));
    assert_eq!(default.horizontal(), HorizontalAlignment::General);
    assert_eq!(default.vertical(), VerticalAlignment::Bottom);
    assert_eq!(default.number_format(), NumberFormat::General);
    assert_eq!(default.border_top(), None);
    let bold = default.apply(&[StyleChange::FontBold(Change::Set(true))])?;
    assert!(bold.font_bold());
    assert!(!bold.is_default());
    assert_eq!(
        bold.apply(&[StyleChange::FontBold(Change::Set(false))])?,
        default
    );
    assert_eq!(
        bold.apply(&[StyleChange::FontBold(Change::Reset)])?,
        default
    );
    assert!(
        default
            .apply(&[
                StyleChange::FontFamily(Change::Set(FontFamily::new("Arial".into())?)),
                StyleChange::FontSize(Change::Set(FontSize::new(1000)?)),
                StyleChange::Fill(Change::Set(Rgb::new([255; 3])))
            ])?
            .is_default()
    );
    Ok(())
}
#[test]
fn patch_errors_are_typed_and_do_not_mutate_existing_style() -> TestResult {
    let style = CellStyle::default().apply(&[StyleChange::FontItalic(Change::Set(true))])?;
    assert_eq!(style.apply(&[]), Err(StyleError::EmptyPatch));
    assert_eq!(
        style.apply(&[
            StyleChange::FontBold(Change::Set(true)),
            StyleChange::FontBold(Change::Reset)
        ]),
        Err(StyleError::DuplicateProperty)
    );
    assert!(style.font_italic());
    assert!(!style.font_bold());
    Ok(())
}
#[test]
fn independent_leaves_commute_and_same_property_uses_order() -> TestResult {
    let base = CellStyle::default();
    let bold = StyleChange::FontBold(Change::Set(true));
    let italic = StyleChange::FontItalic(Change::Set(true));
    let fill = StyleChange::Fill(Change::Set(Rgb::new([12, 34, 56])));
    assert_eq!(
        base.apply(std::slice::from_ref(&bold))?
            .apply(&[italic.clone(), fill.clone()])?,
        base.apply(&[italic, fill])?.apply(&[bold])?
    );
    let left = base
        .apply(&[StyleChange::FontBold(Change::Set(true))])?
        .apply(&[StyleChange::FontBold(Change::Reset)])?;
    let right = base
        .apply(&[StyleChange::FontBold(Change::Reset)])?
        .apply(&[StyleChange::FontBold(Change::Set(true))])?;
    assert!(left.is_default());
    assert!(right.font_bold());
    Ok(())
}
#[test]
fn family_and_size_reject_invalid_input_at_construction() -> TestResult {
    for invalid in ["", " Arial", "Arial ", "Arial\nX", "A\u{7f}B"] {
        assert_eq!(
            FontFamily::new(invalid.into()),
            Err(StyleError::InvalidFontFamily)
        );
    }
    assert!(FontFamily::new("Я".repeat(64)).is_ok());
    assert!(FontFamily::new("Я".repeat(65)).is_err());
    for size in [0, 99, 20001, u16::MAX] {
        assert_eq!(FontSize::new(size), Err(StyleError::InvalidFontSize));
    }
    assert_eq!(FontSize::new(100)?.get(), 100);
    assert_eq!(FontSize::new(20000)?.get(), 20000);
    Ok(())
}
#[test]
fn sparse_canonical_encoding_is_fixed_and_patch_order_independent() -> TestResult {
    let style = CellStyle::default().apply(&[
        StyleChange::Fill(Change::Set(Rgb::new([1, 2, 3]))),
        StyleChange::FontBold(Change::Set(true)),
    ])?;
    assert_eq!(
        style
            .normalized_changes()
            .iter()
            .map(StyleChange::property)
            .collect::<Vec<_>>(),
        vec![StyleProperty::FontBold, StyleProperty::Fill]
    );
    let mut bytes = vec![];
    style.append_to(&mut bytes);
    let mut expected = b"PXEL-STYLE-1\0".to_vec();
    expected.extend_from_slice(&[0, 2, 3, 1, 8, 1, 2, 3]);
    assert_eq!(bytes, expected);
    let reconstructed = CellStyle::default().apply(&style.normalized_changes())?;
    assert_eq!(reconstructed, style);
    Ok(())
}
#[test]
fn all_leaf_tags_are_distinct_and_typed_border_edges_are_atomic() -> TestResult {
    let border = Border::new(BorderLine::Double, Rgb::new([20, 30, 40]));
    let changes = [
        StyleChange::FontFamily(Change::Set(FontFamily::new("Serif".into())?)),
        StyleChange::FontSize(Change::Set(FontSize::new(1200)?)),
        StyleChange::FontBold(Change::Set(true)),
        StyleChange::FontItalic(Change::Set(true)),
        StyleChange::FontUnderline(Change::Set(Underline::Double)),
        StyleChange::FontStrike(Change::Set(true)),
        StyleChange::FontColor(Change::Set(Rgb::new([1, 2, 3]))),
        StyleChange::Fill(Change::Set(Rgb::new([4, 5, 6]))),
        StyleChange::BorderTop(Change::Set(border)),
        StyleChange::BorderRight(Change::Set(border)),
        StyleChange::BorderBottom(Change::Set(border)),
        StyleChange::BorderLeft(Change::Set(border)),
        StyleChange::Horizontal(Change::Set(HorizontalAlignment::Right)),
        StyleChange::Vertical(Change::Set(VerticalAlignment::Top)),
        StyleChange::Wrap(Change::Set(true)),
        StyleChange::NumberFormat(Change::Set(NumberFormat::DateIso)),
    ];
    let style = CellStyle::default().apply(&changes)?;
    assert_eq!(style.normalized_changes(), changes);
    assert_eq!(
        style
            .normalized_changes()
            .iter()
            .map(|c| c.property() as u8)
            .collect::<Vec<_>>(),
        (1..=16).collect::<Vec<_>>()
    );
    assert_eq!(style.font_size().get(), 1200);
    assert_eq!(style.font_underline(), Underline::Double);
    assert!(style.font_strike());
    assert!(style.wrap());
    assert_eq!(style.border_top(), Some(border));
    assert_eq!(style.border_right(), Some(border));
    assert_eq!(style.border_bottom(), Some(border));
    assert_eq!(style.border_left(), Some(border));
    let cleared = style.apply(&[StyleChange::BorderTop(Change::Reset)])?;
    assert_eq!(cleared.border_top(), None);
    assert_eq!(cleared.border_right(), Some(border));
    assert!(StyleProperty::try_from(0).is_err());
    assert!(StyleProperty::try_from(17).is_err());
    assert!(Underline::try_from(3).is_err());
    assert!(HorizontalAlignment::try_from(4).is_err());
    assert!(VerticalAlignment::try_from(3).is_err());
    assert!(BorderLine::try_from(0).is_err());
    assert!(NumberFormat::try_from(7).is_err());
    Ok(())
}
proptest! {
    #[test]
    fn normalization_is_idempotent(bold in any::<bool>(),italic in any::<bool>(),color in any::<[u8;3]>()) {
        let patch=[StyleChange::FontBold(Change::Set(bold)),StyleChange::FontItalic(Change::Set(italic)),StyleChange::FontColor(Change::Set(Rgb::new(color)))];
        let style=CellStyle::default().apply(&patch)?;
        prop_assert_eq!(&style.apply(&patch)?,&style);
        let normalized=style.normalized_changes();
        if normalized.is_empty(){prop_assert!(style.is_default());}else{prop_assert_eq!(CellStyle::default().apply(&normalized)?,style);}
    }
}
