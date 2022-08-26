use crate::cursor::Cursor;

#[derive(Clone, Copy, Debug)]
pub(crate) struct StrCursor<'a> {
    string: &'a str,
    position: usize,
}

impl<'a> StrCursor<'a> {
    pub(crate) fn new(string: &'a str) -> Self {
        Self {
            string,
            position: 0,
        }
    }
}

impl<'a> Cursor for StrCursor<'a> {
    fn is_at_start_of_text(&self) -> bool {
        self.position == 0
    }

    fn is_at_end_of_text(&self) -> bool {
        self.position == self.string.len()
    }

    fn byte_position(&self) -> usize {
        self.position
    }

    fn move_to(&mut self, position: usize) {
        assert!(position <= self.string.len());
        self.position = position;
    }

    fn next_byte(&mut self) -> Option<u8> {
        if self.position == self.string.len() {
            return None;
        }
        let byte = self.string.as_bytes()[self.position];
        self.position += 1;
        Some(byte)
    }

    fn prev_byte(&mut self) -> Option<u8> {
        if self.position == 0 {
            return None;
        }
        self.position -= 1;
        Some(self.string.as_bytes()[self.position])
    }

    fn next_char(&mut self) -> Option<char> {
        if self.position == self.string.len() {
            return None;
        }
        let ch = self.string[self.position..].chars().next().unwrap();
        self.position += ch.len_utf8();
        Some(ch)
    }

    fn prev_char(&mut self) -> Option<char> {
        if self.position == 0 {
            return None;
        }
        let ch = self.string[..self.position].chars().next_back().unwrap();
        self.position -= ch.len_utf8();
        Some(ch)
    }
}
