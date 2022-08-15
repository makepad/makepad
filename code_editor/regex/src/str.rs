use crate::cursor::Cursor;

#[derive(Clone, Copy)]
pub struct StrCursor<'a> {
    string: &'a str,
    byte_position: usize,
}

impl<'a> StrCursor<'a> {
    pub fn new(string: &'a str) -> Self {
        Self {
            string,
            byte_position: 0,
        }
    }
}

impl<'a> Cursor for StrCursor<'a> {
    fn is_at_front(&self) -> bool {
        self.byte_position == 0
    }

    fn is_at_back(&self) -> bool {
        self.byte_position == self.string.len()
    }

    fn byte_position(&self) -> usize {
        self.byte_position
    }

    fn current_char(&self) -> Option<char> {
        self.string[self.byte_position..].chars().next()
    }

    fn move_next_char(&mut self) {
        self.byte_position += utf8_char_width(self.string.as_bytes()[self.byte_position]);
    }

    fn move_prev_char(&mut self) {
        loop {
            self.byte_position -= 1;
            if self.string.is_char_boundary(self.byte_position) {
                break;
            }
        }
    }
}

#[inline]
pub(crate) fn utf8_char_width(byte: u8) -> usize {
    match byte {
        byte if byte < 0x80 => 1,
        byte if byte < 0xe0 => 2,
        byte if byte < 0xf0 => 3,
        _ => 4,
    }
}
