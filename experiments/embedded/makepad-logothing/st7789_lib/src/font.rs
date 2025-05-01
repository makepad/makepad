use core::ops::Range;

/// Font trait
pub trait Font{
    /// Returns the bit the width of a character in the font.
    fn get_char(&self, c: char) -> Option<(&[u8],u8)>;

    /// Returns the width of a character in the font.
    /// If the character is not found, returns None.
    fn get_width(&self, c: char) -> Option<u8>;

    /// Returns the width of a character in the font.
    fn get_height(&self) -> u8;

    /// Returns the width and height of a character in the font.
    /// It will break lines when meeting a newline character.
    fn measure_text(&self, text: &str) -> (u16,u16){
        let mut width:u16 = 0;
        let mut max_width:u16 = 0;
        let mut height:u16 = 0;
        for c in text.chars(){
            if c == '\n' {
                if width > max_width {
                    max_width = width;
                }
                width = 0;
                height += self.get_height() as u16;
            }else{
                width += self.get_width(c).unwrap_or(0) as u16;
            }
        }
        if width > max_width {
            max_width = width;
        }
        (max_width,height+self.get_height() as u16)
    }
}

/// ROM Fonts
pub struct ROMFont {
    /// the raw data of the font
    data: &'static [u8],
    /// the height of the font
    height: u8,
    /// the width of the font
    width: u8,
    /// the character contains in the font
    range: Range<char>,
}

impl Font for ROMFont {
    fn get_char(&self, c: char) -> Option<(&[u8],u8)> {
        if self.range.contains(&c) {
            let index = c as usize - self.range.start as usize;
            let size = self.width as usize * (self.height / 8 ) as usize;
            let offset = index * size;
            Some((&self.data[offset..offset+size], self.width))
        } else {
            None
        }
    }

    fn get_width(&self, c: char) -> Option<u8> {
        if self.range.contains(&c) {
            Some(self.width)
        } else {
            None
        }
    }

    fn get_height(&self) -> u8 {
        self.height
    }
}

impl ROMFont {
    /// Creates a new ROMFont from the given data.
    pub const fn new(data: &'static [u8], height: u8, width: u8, range: Range<char>) -> ROMFont {
        ROMFont {
            data: data,
            height: height,
            width: width,
            range: range,
        }
    }
}

