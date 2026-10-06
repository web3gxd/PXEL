use spreadsheet_domain::{style::*, workbook::*};
use std::{collections::BTreeMap, error::Error, hint::black_box, time::Duration};

fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "styles".to_owned());
    if !["axis-only", "styles", "rich-styles"].contains(&mode.as_str()) {
        return Err(format!("unsupported probe mode: {mode}").into());
    }

    let sheet = SheetId::from_bytes([1; 16])?;
    let column = ColumnId::from_bytes([2; 16])?;
    let addresses = (1..=100_000u128)
        .map(|index| {
            Ok(CellAddress {
                sheet,
                row: RowId::from_bytes(index.to_be_bytes())?,
                column,
            })
        })
        .collect::<Result<Vec<_>, DomainError>>()?;
    let compact_style = CellStyle::default().apply(&[
        StyleChange::FontBold(Change::Set(true)),
        StyleChange::Fill(Change::Set(Rgb::new([220, 230, 240]))),
    ])?;
    let rich_styles = if mode == "rich-styles" {
        ["Arial", "Calibri", "Noto Sans", "Times New Roman"]
            .into_iter()
            .enumerate()
            .map(|(variant, family)| rich_style(family, variant))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let styles = (mode != "axis-only").then(|| {
        addresses
            .iter()
            .enumerate()
            .map(|(index, address)| {
                let style = if mode == "rich-styles" {
                    &rich_styles[index % rich_styles.len()]
                } else {
                    &compact_style
                };
                (*address, style.clone())
            })
            .collect::<BTreeMap<_, _>>()
    });

    println!(
        "ready mode={mode} addresses={} styled_cells={}",
        addresses.len(),
        styles.as_ref().map_or(0, BTreeMap::len)
    );
    std::thread::sleep(Duration::from_secs(3));
    black_box((&addresses, &styles));
    Ok(())
}

fn rich_style(family: &str, variant: usize) -> Result<CellStyle, StyleError> {
    let channel = variant as u8 * 40;
    let border = Border::new(
        [
            BorderLine::Thin,
            BorderLine::Medium,
            BorderLine::Dashed,
            BorderLine::Dotted,
        ][variant],
        Rgb::new([channel, 80, 180]),
    );
    CellStyle::default().apply(&[
        StyleChange::FontFamily(Change::Set(FontFamily::new(family.to_owned())?)),
        StyleChange::FontSize(Change::Set(FontSize::new(1200 + variant as u16 * 100)?)),
        StyleChange::FontBold(Change::Set(variant.is_multiple_of(2))),
        StyleChange::FontItalic(Change::Set(!variant.is_multiple_of(2))),
        StyleChange::FontUnderline(Change::Set(if variant.is_multiple_of(2) {
            Underline::Single
        } else {
            Underline::Double
        })),
        StyleChange::FontStrike(Change::Set(variant == 3)),
        StyleChange::FontColor(Change::Set(Rgb::new([30, channel, 90]))),
        StyleChange::Fill(Change::Set(Rgb::new([220, channel, 240]))),
        StyleChange::BorderTop(Change::Set(border)),
        StyleChange::BorderRight(Change::Set(border)),
        StyleChange::BorderBottom(Change::Set(border)),
        StyleChange::BorderLeft(Change::Set(border)),
        StyleChange::Horizontal(Change::Set(
            [
                HorizontalAlignment::Left,
                HorizontalAlignment::Center,
                HorizontalAlignment::Right,
                HorizontalAlignment::General,
            ][variant],
        )),
        StyleChange::Vertical(Change::Set(
            [
                VerticalAlignment::Top,
                VerticalAlignment::Center,
                VerticalAlignment::Bottom,
                VerticalAlignment::Top,
            ][variant],
        )),
        StyleChange::TextOverflow(Change::Set(if variant.is_multiple_of(2) {
            TextOverflow::Wrap
        } else {
            TextOverflow::Clip
        })),
        StyleChange::NumberFormat(Change::Set(
            [
                NumberFormat::Integer,
                NumberFormat::Decimal2,
                NumberFormat::Percent0,
                NumberFormat::DateIso,
            ][variant],
        )),
    ])
}
