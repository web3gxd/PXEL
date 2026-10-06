//! Sparse formatting overrides, independent of authored cell content.
//! Sizes are hundredths of a point. Fonts are names, never font-loading URLs.
use std::fmt;
use thiserror::Error;

pub const DEFAULT_PROFILE_VERSION: u16 = 1;
pub const OVERFLOW_PROFILE_VERSION: u16 = 2;
pub const STYLE_PROPERTY_COUNT: usize = 16;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum StyleError {
    #[error("style patch must contain at least one property")]
    EmptyPatch,
    #[error("style patch exceeds the property limit")]
    TooManyProperties,
    #[error("style patch repeats a property")]
    DuplicateProperty,
    #[error("invalid font family name")]
    InvalidFontFamily,
    #[error("font size is outside this profile")]
    InvalidFontSize,
    #[error("unknown style enum value")]
    InvalidEnum,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Change<T> {
    Set(T),
    Reset,
}
#[derive(Clone, Eq, PartialEq)]
pub struct FontFamily(String);
impl FontFamily {
    pub fn new(name: String) -> Result<Self, StyleError> {
        if name.is_empty()
            || name.len() > 128
            || name.trim() != name
            || name.chars().any(char::is_control)
        {
            return Err(StyleError::InvalidFontFamily);
        }
        Ok(Self(name))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for FontFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontFamily")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FontSize(u16);
impl FontSize {
    pub fn new(hundredth_points: u16) -> Result<Self, StyleError> {
        if !(100..=20000).contains(&hundredth_points) {
            return Err(StyleError::InvalidFontSize);
        }
        Ok(Self(hundredth_points))
    }
    pub const fn get(self) -> u16 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb([u8; 3]);
impl Rgb {
    pub const fn new(channels: [u8; 3]) -> Self {
        Self(channels)
    }
    pub const fn channels(self) -> [u8; 3] {
        self.0
    }
}
macro_rules! closed_enum {
    ($name:ident { $($variant:ident = $tag:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
        #[repr(u8)]
        pub enum $name { $($variant = $tag),+ }
        impl TryFrom<u8> for $name {
            type Error = StyleError;
            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value { $($tag => Ok(Self::$variant),)+ _ => Err(StyleError::InvalidEnum) }
            }
        }
    };
}
closed_enum!(Underline { None=0, Single=1, Double=2 });
closed_enum!(TextOverflow { Clip=0, Wrap=1, Overflow=2 });
closed_enum!(HorizontalAlignment { General=0, Left=1, Center=2, Right=3 });
closed_enum!(VerticalAlignment { Top=0, Center=1, Bottom=2 });
closed_enum!(BorderLine { Thin=1, Medium=2, Thick=3, Dashed=4, Dotted=5, Double=6 });
closed_enum!(NumberFormat { General=0, Integer=1, Decimal2=2, Percent0=3, Percent2=4, Scientific2=5, DateIso=6 });
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Border {
    line: BorderLine,
    color: Rgb,
}
impl Border {
    pub const fn new(line: BorderLine, color: Rgb) -> Self {
        Self { line, color }
    }
    pub const fn line(self) -> BorderLine {
        self.line
    }
    pub const fn color(self) -> Rgb {
        self.color
    }
}

closed_enum!(StyleProperty {
    FontFamily=1,
    FontSize=2,
    FontBold=3,
    FontItalic=4,
    FontUnderline=5,
    FontStrike=6,
    FontColor=7,
    Fill=8,
    BorderTop=9,
    BorderRight=10,
    BorderBottom=11,
    BorderLeft=12,
    Horizontal=13,
    Vertical=14,
    // Tag 15 was the v1 boolean `wrap`; v2 extends it to clip/wrap/overflow.
    TextOverflow=15,
    NumberFormat=16,
});
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StyleChange {
    FontFamily(Change<FontFamily>),
    FontSize(Change<FontSize>),
    FontBold(Change<bool>),
    FontItalic(Change<bool>),
    FontUnderline(Change<Underline>),
    FontStrike(Change<bool>),
    FontColor(Change<Rgb>),
    Fill(Change<Rgb>),
    BorderTop(Change<Border>),
    BorderRight(Change<Border>),
    BorderBottom(Change<Border>),
    BorderLeft(Change<Border>),
    Horizontal(Change<HorizontalAlignment>),
    Vertical(Change<VerticalAlignment>),
    TextOverflow(Change<TextOverflow>),
    NumberFormat(Change<NumberFormat>),
}
impl StyleChange {
    pub const fn property(&self) -> StyleProperty {
        match self {
            Self::FontFamily(..) => StyleProperty::FontFamily,
            Self::FontSize(..) => StyleProperty::FontSize,
            Self::FontBold(..) => StyleProperty::FontBold,
            Self::FontItalic(..) => StyleProperty::FontItalic,
            Self::FontUnderline(..) => StyleProperty::FontUnderline,
            Self::FontStrike(..) => StyleProperty::FontStrike,
            Self::FontColor(..) => StyleProperty::FontColor,
            Self::Fill(..) => StyleProperty::Fill,
            Self::BorderTop(..) => StyleProperty::BorderTop,
            Self::BorderRight(..) => StyleProperty::BorderRight,
            Self::BorderBottom(..) => StyleProperty::BorderBottom,
            Self::BorderLeft(..) => StyleProperty::BorderLeft,
            Self::Horizontal(..) => StyleProperty::Horizontal,
            Self::Vertical(..) => StyleProperty::Vertical,
            Self::TextOverflow(..) => StyleProperty::TextOverflow,
            Self::NumberFormat(..) => StyleProperty::NumberFormat,
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CellStyle {
    font_family: Option<FontFamily>,
    font_size: Option<FontSize>,
    font_bold: Option<bool>,
    font_italic: Option<bool>,
    font_underline: Option<Underline>,
    font_strike: Option<bool>,
    font_color: Option<Rgb>,
    fill: Option<Rgb>,
    border_top: Option<Border>,
    border_right: Option<Border>,
    border_bottom: Option<Border>,
    border_left: Option<Border>,
    horizontal: Option<HorizontalAlignment>,
    vertical: Option<VerticalAlignment>,
    text_overflow: Option<TextOverflow>,
    number_format: Option<NumberFormat>,
}
impl CellStyle {
    /// Exact property preimage, retaining Reset for an inherited default.
    pub fn change(&self, property: StyleProperty) -> StyleChange {
        if let Some(change) = self
            .normalized_changes()
            .into_iter()
            .find(|c| c.property() == property)
        {
            return change;
        }
        match property {
            StyleProperty::FontFamily => StyleChange::FontFamily(Change::Reset),
            StyleProperty::FontSize => StyleChange::FontSize(Change::Reset),
            StyleProperty::FontBold => StyleChange::FontBold(Change::Reset),
            StyleProperty::FontItalic => StyleChange::FontItalic(Change::Reset),
            StyleProperty::FontUnderline => StyleChange::FontUnderline(Change::Reset),
            StyleProperty::FontStrike => StyleChange::FontStrike(Change::Reset),
            StyleProperty::FontColor => StyleChange::FontColor(Change::Reset),
            StyleProperty::Fill => StyleChange::Fill(Change::Reset),
            StyleProperty::BorderTop => StyleChange::BorderTop(Change::Reset),
            StyleProperty::BorderRight => StyleChange::BorderRight(Change::Reset),
            StyleProperty::BorderBottom => StyleChange::BorderBottom(Change::Reset),
            StyleProperty::BorderLeft => StyleChange::BorderLeft(Change::Reset),
            StyleProperty::Horizontal => StyleChange::Horizontal(Change::Reset),
            StyleProperty::Vertical => StyleChange::Vertical(Change::Reset),
            StyleProperty::TextOverflow => StyleChange::TextOverflow(Change::Reset),
            StyleProperty::NumberFormat => StyleChange::NumberFormat(Change::Reset),
        }
    }

    pub fn font_family(&self) -> &str {
        self.font_family
            .as_ref()
            .map_or("Arial", FontFamily::as_str)
    }
    pub fn font_size(&self) -> FontSize {
        self.font_size.unwrap_or(FontSize(1000))
    }
    pub fn font_bold(&self) -> bool {
        self.font_bold.unwrap_or(false)
    }
    pub fn font_italic(&self) -> bool {
        self.font_italic.unwrap_or(false)
    }
    pub fn font_underline(&self) -> Underline {
        self.font_underline.unwrap_or(Underline::None)
    }
    pub fn font_strike(&self) -> bool {
        self.font_strike.unwrap_or(false)
    }
    pub fn font_color(&self) -> Rgb {
        self.font_color.unwrap_or(Rgb([0, 0, 0]))
    }
    pub fn fill(&self) -> Rgb {
        self.fill.unwrap_or(Rgb([255, 255, 255]))
    }
    pub const fn border_top(&self) -> Option<Border> {
        self.border_top
    }
    pub const fn border_right(&self) -> Option<Border> {
        self.border_right
    }
    pub const fn border_bottom(&self) -> Option<Border> {
        self.border_bottom
    }
    pub const fn border_left(&self) -> Option<Border> {
        self.border_left
    }
    pub fn horizontal(&self) -> HorizontalAlignment {
        self.horizontal.unwrap_or(HorizontalAlignment::General)
    }
    pub fn vertical(&self) -> VerticalAlignment {
        self.vertical.unwrap_or(VerticalAlignment::Bottom)
    }
    pub fn text_overflow(&self) -> TextOverflow {
        self.text_overflow.unwrap_or(TextOverflow::Clip)
    }
    pub fn wrap(&self) -> bool {
        self.text_overflow() == TextOverflow::Wrap
    }
    pub fn number_format(&self) -> NumberFormat {
        self.number_format.unwrap_or(NumberFormat::General)
    }
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    /// Validate the whole property mask before cloning or applying anything.
    /// Equal-value writes remain visible to the caller; journal history owns them.
    pub fn apply(&self, changes: &[StyleChange]) -> Result<Self, StyleError> {
        if changes.is_empty() {
            return Err(StyleError::EmptyPatch);
        }
        if changes.len() > STYLE_PROPERTY_COUNT {
            return Err(StyleError::TooManyProperties);
        }
        let mut seen = 0u16;
        for change in changes {
            let bit = 1u16 << (change.property() as u8 - 1);
            if seen & bit != 0 {
                return Err(StyleError::DuplicateProperty);
            }
            seen |= bit;
        }
        let mut candidate = self.clone();
        for change in changes {
            match change {
                StyleChange::FontFamily(change) => {
                    candidate.font_family = match change {
                        Change::Set(value) if value.as_str() != "Arial" => Some(value.clone()),
                        _ => None,
                    }
                }
                StyleChange::FontSize(change) => {
                    candidate.font_size = override_value(change, FontSize(1000))
                }
                StyleChange::FontBold(change) => {
                    candidate.font_bold = override_value(change, false)
                }
                StyleChange::FontItalic(change) => {
                    candidate.font_italic = override_value(change, false)
                }
                StyleChange::FontUnderline(change) => {
                    candidate.font_underline = override_value(change, Underline::None)
                }
                StyleChange::FontStrike(change) => {
                    candidate.font_strike = override_value(change, false)
                }
                StyleChange::FontColor(change) => {
                    candidate.font_color = override_value(change, Rgb([0, 0, 0]))
                }
                StyleChange::Fill(change) => {
                    candidate.fill = override_value(change, Rgb([255, 255, 255]))
                }
                StyleChange::BorderTop(change) => {
                    candidate.border_top = match change {
                        Change::Set(value) => Some(*value),
                        Change::Reset => None,
                    }
                }
                StyleChange::BorderRight(change) => {
                    candidate.border_right = match change {
                        Change::Set(value) => Some(*value),
                        Change::Reset => None,
                    }
                }
                StyleChange::BorderBottom(change) => {
                    candidate.border_bottom = match change {
                        Change::Set(value) => Some(*value),
                        Change::Reset => None,
                    }
                }
                StyleChange::BorderLeft(change) => {
                    candidate.border_left = match change {
                        Change::Set(value) => Some(*value),
                        Change::Reset => None,
                    }
                }
                StyleChange::Horizontal(change) => {
                    candidate.horizontal = override_value(change, HorizontalAlignment::General)
                }
                StyleChange::Vertical(change) => {
                    candidate.vertical = override_value(change, VerticalAlignment::Bottom)
                }
                StyleChange::TextOverflow(change) => {
                    candidate.text_overflow = override_value(change, TextOverflow::Clip)
                }
                StyleChange::NumberFormat(change) => {
                    candidate.number_format = override_value(change, NumberFormat::General)
                }
            }
        }
        Ok(candidate)
    }
    /// Only non-default Set overrides, in frozen property-tag order. Empty means
    /// default; it is not a valid user patch passed to `apply`.
    pub fn normalized_changes(&self) -> Vec<StyleChange> {
        let mut changes = Vec::new();
        if let Some(value) = &self.font_family {
            changes.push(StyleChange::FontFamily(Change::Set(value.clone())));
        }
        if let Some(value) = self.font_size {
            changes.push(StyleChange::FontSize(Change::Set(value)));
        }
        if let Some(value) = self.font_bold {
            changes.push(StyleChange::FontBold(Change::Set(value)));
        }
        if let Some(value) = self.font_italic {
            changes.push(StyleChange::FontItalic(Change::Set(value)));
        }
        if let Some(value) = self.font_underline {
            changes.push(StyleChange::FontUnderline(Change::Set(value)));
        }
        if let Some(value) = self.font_strike {
            changes.push(StyleChange::FontStrike(Change::Set(value)));
        }
        if let Some(value) = self.font_color {
            changes.push(StyleChange::FontColor(Change::Set(value)));
        }
        if let Some(value) = self.fill {
            changes.push(StyleChange::Fill(Change::Set(value)));
        }
        if let Some(value) = self.border_top {
            changes.push(StyleChange::BorderTop(Change::Set(value)));
        }
        if let Some(value) = self.border_right {
            changes.push(StyleChange::BorderRight(Change::Set(value)));
        }
        if let Some(value) = self.border_bottom {
            changes.push(StyleChange::BorderBottom(Change::Set(value)));
        }
        if let Some(value) = self.border_left {
            changes.push(StyleChange::BorderLeft(Change::Set(value)));
        }
        if let Some(value) = self.horizontal {
            changes.push(StyleChange::Horizontal(Change::Set(value)));
        }
        if let Some(value) = self.vertical {
            changes.push(StyleChange::Vertical(Change::Set(value)));
        }
        if let Some(value) = self.text_overflow {
            changes.push(StyleChange::TextOverflow(Change::Set(value)));
        }
        if let Some(value) = self.number_format {
            changes.push(StyleChange::NumberFormat(Change::Set(value)));
        }
        changes
    }
    /// Canonical semantic bytes, independent of protobuf and platform. Existing
    /// clip/wrap styles retain v1 bytes; overflow uses the profile2 prefix.
    /// Prefix, u16 BE override count, then fixed property tags and typed values.
    pub fn append_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(if self.text_overflow == Some(TextOverflow::Overflow) {
            b"PXEL-STYLE-2\0"
        } else {
            b"PXEL-STYLE-1\0"
        });
        let count = [
            self.font_family.is_some(),
            self.font_size.is_some(),
            self.font_bold.is_some(),
            self.font_italic.is_some(),
            self.font_underline.is_some(),
            self.font_strike.is_some(),
            self.font_color.is_some(),
            self.fill.is_some(),
            self.border_top.is_some(),
            self.border_right.is_some(),
            self.border_bottom.is_some(),
            self.border_left.is_some(),
            self.horizontal.is_some(),
            self.vertical.is_some(),
            self.text_overflow.is_some(),
            self.number_format.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        out.extend_from_slice(&(count as u16).to_be_bytes());
        if let Some(value) = &self.font_family {
            out.push(1);
            out.extend_from_slice(&(value.as_str().len() as u16).to_be_bytes());
            out.extend_from_slice(value.as_str().as_bytes());
        }
        if let Some(value) = &self.font_size {
            out.push(2);
            out.extend_from_slice(&value.get().to_be_bytes());
        }
        if let Some(value) = &self.font_bold {
            out.push(3);
            out.push(u8::from(*value));
        }
        if let Some(value) = &self.font_italic {
            out.push(4);
            out.push(u8::from(*value));
        }
        if let Some(value) = &self.font_underline {
            out.push(5);
            out.push(*value as u8);
        }
        if let Some(value) = &self.font_strike {
            out.push(6);
            out.push(u8::from(*value));
        }
        if let Some(value) = &self.font_color {
            out.push(7);
            out.extend_from_slice(&value.channels());
        }
        if let Some(value) = &self.fill {
            out.push(8);
            out.extend_from_slice(&value.channels());
        }
        if let Some(value) = &self.border_top {
            out.push(9);
            out.push(value.line() as u8);
            out.extend_from_slice(&value.color().channels());
        }
        if let Some(value) = &self.border_right {
            out.push(10);
            out.push(value.line() as u8);
            out.extend_from_slice(&value.color().channels());
        }
        if let Some(value) = &self.border_bottom {
            out.push(11);
            out.push(value.line() as u8);
            out.extend_from_slice(&value.color().channels());
        }
        if let Some(value) = &self.border_left {
            out.push(12);
            out.push(value.line() as u8);
            out.extend_from_slice(&value.color().channels());
        }
        if let Some(value) = &self.horizontal {
            out.push(13);
            out.push(*value as u8);
        }
        if let Some(value) = &self.vertical {
            out.push(14);
            out.push(*value as u8);
        }
        if let Some(value) = &self.text_overflow {
            out.push(15);
            out.push(*value as u8);
        }
        if let Some(value) = &self.number_format {
            out.push(16);
            out.push(*value as u8);
        }
    }
}
fn override_value<T: Copy + Eq>(change: &Change<T>, default: T) -> Option<T> {
    match change {
        Change::Set(value) if *value != default => Some(*value),
        _ => None,
    }
}
