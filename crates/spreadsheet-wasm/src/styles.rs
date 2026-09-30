//! Strict JSON profile shared with the public authored projection.
use crate::PreviewError;
use serde::Deserialize;
use spreadsheet_domain::style::*;
macro_rules! enums {
    ($name:ident,$domain:ident,$($v:ident),+) => {
        #[derive(Deserialize)] #[serde(rename_all="snake_case")]
        pub(crate) enum $name {$($v),+}
        impl From<$name> for $domain {fn from(value:$name)->Self {match value {$($name::$v=>Self::$v),+}}}
    }
}
enums!(UnderlineInput, Underline, None, Single, Double);
enums!(
    HorizontalInput,
    HorizontalAlignment,
    General,
    Left,
    Center,
    Right
);
enums!(VerticalInput, VerticalAlignment, Top, Center, Bottom);
enums!(
    LineInput, BorderLine, Thin, Medium, Thick, Dashed, Dotted, Double
);
enums!(
    NumberInput,
    NumberFormat,
    General,
    Integer,
    Decimal2,
    Percent0,
    Percent2,
    Scientific2,
    DateIso
);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BorderInput {
    line: LineInput,
    color: [u8; 3],
}
impl From<BorderInput> for Border {
    fn from(v: BorderInput) -> Self {
        Self::new(v.line.into(), Rgb::new(v.color))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StyleInput {
    font_family: String,
    font_size_hundredth_points: u16,
    font_bold: bool,
    font_italic: bool,
    font_underline: UnderlineInput,
    font_strike: bool,
    font_color: [u8; 3],
    fill: [u8; 3],
    border_top: Required<BorderInput>,
    border_right: Required<BorderInput>,
    border_bottom: Required<BorderInput>,
    border_left: Required<BorderInput>,
    horizontal: HorizontalInput,
    vertical: VerticalInput,
    wrap: bool,
    number_format: NumberInput,
}
fn change<T>(v: Option<T>) -> Change<T> {
    match v {
        Some(v) => Change::Set(v),
        None => Change::Reset,
    }
}
impl StyleInput {
    pub(crate) fn decode(self) -> Result<CellStyle, PreviewError> {
        let patch = vec![
            StyleChange::FontFamily(Change::Set(
                FontFamily::new(self.font_family).map_err(|_| PreviewError::InvalidProjection)?,
            )),
            StyleChange::FontSize(Change::Set(
                FontSize::new(self.font_size_hundredth_points)
                    .map_err(|_| PreviewError::InvalidProjection)?,
            )),
            StyleChange::FontBold(Change::Set(self.font_bold)),
            StyleChange::FontItalic(Change::Set(self.font_italic)),
            StyleChange::FontUnderline(Change::Set(self.font_underline.into())),
            StyleChange::FontStrike(Change::Set(self.font_strike)),
            StyleChange::FontColor(Change::Set(Rgb::new(self.font_color))),
            StyleChange::Fill(Change::Set(Rgb::new(self.fill))),
            StyleChange::BorderTop(change(self.border_top.0.map(Into::into))),
            StyleChange::BorderRight(change(self.border_right.0.map(Into::into))),
            StyleChange::BorderBottom(change(self.border_bottom.0.map(Into::into))),
            StyleChange::BorderLeft(change(self.border_left.0.map(Into::into))),
            StyleChange::Horizontal(Change::Set(self.horizontal.into())),
            StyleChange::Vertical(Change::Set(self.vertical.into())),
            StyleChange::Wrap(Change::Set(self.wrap)),
            StyleChange::NumberFormat(Change::Set(self.number_format.into())),
        ];
        CellStyle::default()
            .apply(&patch)
            .map_err(|_| PreviewError::InvalidProjection)
    }
}
#[derive(Deserialize)]
pub(crate) struct Required<T>(Option<T>);
#[derive(Deserialize)]
#[serde(
    tag = "property",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum ChangeInput {
    FontFamily(Required<String>),
    FontSizeHundredthPoints(Required<u16>),
    FontBold(Required<bool>),
    FontItalic(Required<bool>),
    FontUnderline(Required<UnderlineInput>),
    FontStrike(Required<bool>),
    FontColor(Required<[u8; 3]>),
    Fill(Required<[u8; 3]>),
    BorderTop(Required<BorderInput>),
    BorderRight(Required<BorderInput>),
    BorderBottom(Required<BorderInput>),
    BorderLeft(Required<BorderInput>),
    Horizontal(Required<HorizontalInput>),
    Vertical(Required<VerticalInput>),
    Wrap(Required<bool>),
    NumberFormat(Required<NumberInput>),
}
impl ChangeInput {
    pub(crate) fn decode(self) -> Result<StyleChange, PreviewError> {
        Ok(match self {
            Self::FontFamily(Required(v)) => StyleChange::FontFamily(change(
                v.map(FontFamily::new)
                    .transpose()
                    .map_err(|_| PreviewError::InvalidProjection)?,
            )),
            Self::FontSizeHundredthPoints(Required(v)) => StyleChange::FontSize(change(
                v.map(FontSize::new)
                    .transpose()
                    .map_err(|_| PreviewError::InvalidProjection)?,
            )),
            Self::FontBold(Required(v)) => StyleChange::FontBold(change(v)),
            Self::FontItalic(Required(v)) => StyleChange::FontItalic(change(v)),
            Self::FontUnderline(Required(v)) => {
                StyleChange::FontUnderline(change(v.map(Into::into)))
            }
            Self::FontStrike(Required(v)) => StyleChange::FontStrike(change(v)),
            Self::FontColor(Required(v)) => StyleChange::FontColor(change(v.map(Rgb::new))),
            Self::Fill(Required(v)) => StyleChange::Fill(change(v.map(Rgb::new))),
            Self::BorderTop(Required(v)) => StyleChange::BorderTop(change(v.map(Into::into))),
            Self::BorderRight(Required(v)) => StyleChange::BorderRight(change(v.map(Into::into))),
            Self::BorderBottom(Required(v)) => StyleChange::BorderBottom(change(v.map(Into::into))),
            Self::BorderLeft(Required(v)) => StyleChange::BorderLeft(change(v.map(Into::into))),
            Self::Horizontal(Required(v)) => StyleChange::Horizontal(change(v.map(Into::into))),
            Self::Vertical(Required(v)) => StyleChange::Vertical(change(v.map(Into::into))),
            Self::Wrap(Required(v)) => StyleChange::Wrap(change(v)),
            Self::NumberFormat(Required(v)) => StyleChange::NumberFormat(change(v.map(Into::into))),
        })
    }
}
