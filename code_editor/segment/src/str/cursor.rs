use crate::cursor::char;

/// A cursor over a `str`.
///
/// A `Cursor` is like an iterator, except that it can freely seek back-and-forth.
pub struct Cursor<'a> {
    string: &'a str,
    position: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(string: &'a str, position: usize) -> Self {
        Self { string, position }
    }
}

impl<'a> char::Cursor for Cursor<'a> {
    fn is_at_start(&self) -> bool {
        self.position == 0
    }

    fn is_at_end(&self) -> bool {
        self.position == self.string.len()
    }

    fn is_at_boundary(&self) -> bool {
        self.string.is_char_boundary(self.position)
    }

    fn position(&self) -> usize {
        self.position
    }

    fn current(&self) -> char {
        self.string[self.position..].chars().next().unwrap()
    }

    fn move_next(&mut self) {
        loop {
            self.position += 1;
            if self.is_at_boundary() {
                break;
            }
        }
    }

    fn move_prev(&mut self) {
        loop {
            self.position -= 1;
            if self.is_at_boundary() {
                break;
            }
        }
    }

    fn set_position(&mut self, position: usize) {
        assert!(position <= self.string.len());
        self.position = position;
    }
}
