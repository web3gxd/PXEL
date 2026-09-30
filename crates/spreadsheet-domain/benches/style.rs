use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::{style::*, workbook::*};
use std::{collections::BTreeMap, hint::black_box};

fn run(c: &mut Criterion) -> Result<(), Box<dyn std::error::Error>> {
    let border = Border::new(BorderLine::Thin, Rgb::new([20, 30, 40]));
    let patch = vec![
        StyleChange::FontFamily(Change::Set(FontFamily::new("Noto Sans".into())?)),
        StyleChange::FontSize(Change::Set(FontSize::new(1200)?)),
        StyleChange::FontBold(Change::Set(true)),
        StyleChange::FontItalic(Change::Set(true)),
        StyleChange::FontUnderline(Change::Set(Underline::Single)),
        StyleChange::FontStrike(Change::Set(true)),
        StyleChange::FontColor(Change::Set(Rgb::new([40, 50, 60]))),
        StyleChange::Fill(Change::Set(Rgb::new([220, 230, 240]))),
        StyleChange::BorderTop(Change::Set(border)),
        StyleChange::BorderRight(Change::Set(border)),
        StyleChange::BorderBottom(Change::Set(border)),
        StyleChange::BorderLeft(Change::Set(border)),
        StyleChange::Horizontal(Change::Set(HorizontalAlignment::Center)),
        StyleChange::Vertical(Change::Set(VerticalAlignment::Top)),
        StyleChange::Wrap(Change::Set(true)),
        StyleChange::NumberFormat(Change::Set(NumberFormat::Decimal2)),
    ];
    let default = CellStyle::default();
    let rich = default.apply(&patch)?;
    let single = [StyleChange::FontBold(Change::Set(true))];
    c.bench_function("style/apply-one-leaf", |b| {
        b.iter(|| black_box(default.apply(black_box(&single))))
    });
    c.bench_function("style/apply-sixteen-leaves", |b| {
        b.iter(|| black_box(default.apply(black_box(&patch))))
    });
    c.bench_function("style/canonical-rich-style", |b| {
        let mut bytes = Vec::new();
        b.iter(|| {
            bytes.clear();
            rich.append_to(&mut bytes);
            black_box(&bytes);
        });
    });
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
    let compact = default.apply(&[
        StyleChange::FontBold(Change::Set(true)),
        StyleChange::Fill(Change::Set(Rgb::new([220, 230, 240]))),
    ])?;
    c.bench_function("style/build-100k-sparse-map", |b| {
        b.iter(|| {
            let map: BTreeMap<_, _> = addresses
                .iter()
                .map(|address| (*address, compact.clone()))
                .collect();
            black_box(map);
        });
    });
    let map: BTreeMap<_, _> = addresses
        .iter()
        .map(|address| (*address, compact.clone()))
        .collect();
    c.bench_function("style/canonical-100k-sparse-map", |b| {
        let mut bytes = Vec::new();
        b.iter(|| {
            bytes.clear();
            for (address, style) in &map {
                bytes.extend_from_slice(address.row.as_bytes());
                bytes.extend_from_slice(address.column.as_bytes());
                style.append_to(&mut bytes);
            }
            black_box(&bytes);
        });
    });
    // Value layout only: this excludes BTree nodes, capacity and allocator overhead.
    eprintln!(
        "style fixture: entries={}, CellStyle value bytes={}, address bytes={}",
        map.len(),
        std::mem::size_of::<CellStyle>(),
        std::mem::size_of::<CellAddress>()
    );
    Ok(())
}

fn bench(c: &mut Criterion) {
    if let Err(error) = run(c) {
        eprintln!("style benchmark setup failed: {error}");
        std::process::exit(1);
    }
}
criterion_group!(benches, bench);
criterion_main!(benches);
