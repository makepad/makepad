//! Cell style attributes. Port of ghostty `src/terminal/style.zig` (the
//! Style value type; the ref-counted style set is unnecessary here — cells
//! store their style inline).

use crate::term::color::{Palette, Rgb};

/// Underline styles, values match SGR 4:x subparams (ghostty sgr.zig
/// `Attribute.Underline`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Underline {
    #[default]
    None = 0,
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
}

impl Underline {
    pub fn from_param(v: u16) -> Option<Self> {
        Some(match v {
            0 => Underline::None,
            1 => Underline::Single,
            2 => Underline::Double,
            3 => Underline::Curly,
            4 => Underline::Dotted,
            5 => Underline::Dashed,
            _ => return None,
        })
    }
}

/// A style color: unset (use terminal default), palette-indexed (tracks
/// palette changes), or direct RGB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StyleColor {
    #[default]
    None,
    Palette(u8),
    Rgb(Rgb),
}

impl StyleColor {
    pub fn resolve(self, palette: &Palette, default: Rgb) -> Rgb {
        match self {
            StyleColor::None => default,
            StyleColor::Palette(i) => palette[i as usize],
            StyleColor::Rgb(rgb) => rgb,
        }
    }

    pub fn resolve_opt(self, palette: &Palette) -> Option<Rgb> {
        match self {
            StyleColor::None => None,
            StyleColor::Palette(i) => Some(palette[i as usize]),
            StyleColor::Rgb(rgb) => Some(rgb),
        }
    }
}

/// The style attributes for a cell (ghostty `Style`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg_color: StyleColor,
    pub bg_color: StyleColor,
    pub underline_color: StyleColor,
    pub flags: StyleFlags,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StyleFlags(pub u16);

impl StyleFlags {
    pub const BOLD: u16 = 1 << 0;
    pub const ITALIC: u16 = 1 << 1;
    pub const FAINT: u16 = 1 << 2;
    pub const BLINK: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const INVISIBLE: u16 = 1 << 5;
    pub const STRIKETHROUGH: u16 = 1 << 6;
    pub const OVERLINE: u16 = 1 << 7;
    // Bits 8..11: Underline enum.
    const UNDERLINE_SHIFT: u16 = 8;
    const UNDERLINE_MASK: u16 = 0x7 << Self::UNDERLINE_SHIFT;

    #[inline]
    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    #[inline]
    pub fn set(&mut self, flag: u16, on: bool) {
        if on {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }

    #[inline]
    pub fn underline(self) -> Underline {
        match (self.0 & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT {
            1 => Underline::Single,
            2 => Underline::Double,
            3 => Underline::Curly,
            4 => Underline::Dotted,
            5 => Underline::Dashed,
            _ => Underline::None,
        }
    }

    #[inline]
    pub fn set_underline(&mut self, u: Underline) {
        self.0 = (self.0 & !Self::UNDERLINE_MASK) | ((u as u16) << Self::UNDERLINE_SHIFT);
    }
}

impl Style {
    #[inline]
    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }

    /// Foreground for rendering: applies inverse and invisible are the
    /// renderer's concern; bold-brightening is a renderer policy too.
    pub fn fg(&self, palette: &Palette, default_fg: Rgb) -> Rgb {
        self.fg_color.resolve(palette, default_fg)
    }

    pub fn bg(&self, palette: &Palette) -> Option<Rgb> {
        self.bg_color.resolve_opt(palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underline_bits() {
        let mut f = StyleFlags::default();
        f.set(StyleFlags::BOLD, true);
        f.set_underline(Underline::Curly);
        assert!(f.has(StyleFlags::BOLD));
        assert_eq!(f.underline(), Underline::Curly);
        f.set_underline(Underline::None);
        assert_eq!(f.underline(), Underline::None);
        assert!(f.has(StyleFlags::BOLD));
    }
}
