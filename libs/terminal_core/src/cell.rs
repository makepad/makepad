use crate::color::Rgb;

/// Terminal cell color
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    #[default]
    Default,
    Palette(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn resolve(&self, palette: &[Rgb; 256], default: Rgb) -> Rgb {
        match *self {
            Color::Default => default,
            Color::Palette(idx) => palette[idx as usize],
            Color::Rgb(r, g, b) => Rgb::new(r, g, b),
        }
    }
}

/// Style flags packed into u16
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StyleFlags(pub u16);

impl StyleFlags {
    pub const BOLD: u16 = 0x0001;
    pub const ITALIC: u16 = 0x0002;
    pub const FAINT: u16 = 0x0004;
    pub const BLINK: u16 = 0x0008;
    pub const INVERSE: u16 = 0x0010;
    pub const INVISIBLE: u16 = 0x0020;
    pub const STRIKETHROUGH: u16 = 0x0040;
    pub const OVERLINE: u16 = 0x0080;
    // Underline occupies bits 8-10 (3 bits for style)
    pub const UNDERLINE_SHIFT: u16 = 8;
    pub const UNDERLINE_MASK: u16 = 0x0700;

    pub fn has(&self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    pub fn set(&mut self, flag: u16) {
        self.0 |= flag;
    }

    pub fn clear(&mut self, flag: u16) {
        self.0 &= !flag;
    }

    pub fn underline(&self) -> u8 {
        ((self.0 & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT) as u8
    }

    pub fn set_underline(&mut self, style: u8) {
        self.0 = (self.0 & !Self::UNDERLINE_MASK) | ((style as u16 & 0x7) << Self::UNDERLINE_SHIFT);
    }
}

/// Cell text style
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub flags: StyleFlags,
}

impl Style {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Cell flags
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellFlags(pub u8);

impl CellFlags {
    pub const WIDE: u8 = 0x01;
    pub const WIDE_TAIL: u8 = 0x02;

    pub fn has(&self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

/// A single terminal cell
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub codepoint: char,
    pub style: Style,
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            codepoint: ' ',
            style: Style::default(),
            flags: CellFlags::default(),
        }
    }
}

impl Cell {
    pub fn clear(&mut self) {
        self.codepoint = ' ';
        self.style = Style::default();
        self.flags = CellFlags::default();
    }

    pub fn clear_with_style(&mut self, style: Style) {
        self.codepoint = ' ';
        self.style = Style {
            fg: Color::Default,
            bg: style.bg,
            flags: StyleFlags::default(),
        };
        self.flags = CellFlags::default();
    }
}
