#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::new(0, 0, 0, 255);
    pub const RED: Self = Self::new(128, 0, 0, 255);
    pub const GREEN: Self = Self::new(0, 128, 0, 255);
    pub const YELLOW: Self = Self::new(128, 128, 0, 255);
    pub const BLUE: Self = Self::new(0, 0, 128, 255);
    pub const MAGENTA: Self = Self::new(128, 0, 128, 255);
    pub const CYAN: Self = Self::new(0, 128, 128, 255);
    pub const WHITE: Self = Self::new(192, 192, 192, 255);
    pub const BRIGHT_BLACK: Self = Self::new(128, 128, 128, 255);
    pub const BRIGHT_RED: Self = Self::new(255, 0, 0, 255);
    pub const BRIGHT_GREEN: Self = Self::new(0, 255, 0, 255);
    pub const BRIGHT_YELLOW: Self = Self::new(255, 255, 0, 255);
    pub const BRIGHT_BLUE: Self = Self::new(0, 0, 255, 255);
    pub const BRIGHT_MAGENTA: Self = Self::new(255, 0, 255, 255);
    pub const BRIGHT_CYAN: Self = Self::new(0, 255, 255, 255);
    pub const BRIGHT_WHITE: Self = Self::new(255, 255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}
