use {std::ops::Range, super::{input, input::Input}};

impl<'a> Input<'a> for &'a str {
    type Cursor = Cursor<'a>;

    fn slice(&self, range: Range<usize>) -> &'a str {
        &self[range]
    }

    fn cursor_start(&self) -> Self::Cursor {
        Cursor {
            str: &self,
            index: 0,
        }
    }

    fn cursor_end(&self) -> Self::Cursor {
        Cursor {
            str: self,
            index: self.len(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    str: &'a str,
    index: usize,
}

impl<'a> input::Cursor for Cursor<'a> {
    fn is_start(&self) -> bool {
        self.index == 0
    }

    fn is_end(&self) -> bool {
        self.index == self.str.len()
    }

    fn index(&self) -> usize {
        self.index
    }

    fn current_byte(&mut self) -> Option<u8> {
        self.str.as_bytes()[self.index..].first().copied()
    }

    fn current_char(&mut self) -> Option<char> {
        self.str[self.index..].chars().next()
    }

    fn prev_byte(&mut self) -> Option<u8> {
        self.str.as_bytes()[..self.index].last().copied()
    }

    fn prev_char(&mut self) -> Option<char> {
        self.str[..self.index].chars().next_back()
    }

    fn move_next_byte(&mut self) -> bool {
        if self.is_end() {
            return false;
        }
        self.index += 1;
        true
    }

    fn move_next_char(&mut self) -> bool {
        if self.move_next_byte() {
            while !self.str.is_char_boundary(self.index) {
                self.move_next_byte();
            }
            true
        } else {
            false
        }
    }

    fn move_prev_byte(&mut self) -> bool {
        if self.is_start() {
            return false;
        }
        self.index -= 1;
        true
    }
    
    fn move_prev_char(&mut self) -> bool {
        if self.move_prev_byte() {
            while !self.str.is_char_boundary(self.index) {
                self.move_prev_byte();
            }
            true
        } else {
            false
        }
    }
}
