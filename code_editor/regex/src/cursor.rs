pub trait Cursor {
    fn is_at_start_of_text(&self) -> bool;
    fn is_at_end_of_text(&self) -> bool;
    fn byte_position(&self) -> usize;
    fn move_to(&mut self, byte_position: usize);
    fn next_byte(&mut self) -> Option<u8>;
    fn prev_byte(&mut self) -> Option<u8>;
    fn next_char(&mut self) -> Option<char>;
    fn prev_char(&mut self) -> Option<char>;

    fn rev(self) -> Rev<Self>
    where
        Self: Sized,
    {
        Rev { cursor: self }
    }
}

impl<'a, T: Cursor> Cursor for &'a mut T {
    fn is_at_start_of_text(&self) -> bool {
        (**self).is_at_start_of_text()
    }

    fn is_at_end_of_text(&self) -> bool {
        (**self).is_at_end_of_text()
    }

    fn byte_position(&self) -> usize {
        (**self).byte_position()
    }

    fn move_to(&mut self, position: usize) {
        (**self).move_to(position)
    }

    fn next_byte(&mut self) -> Option<u8> {
        (**self).next_byte()
    }

    fn prev_byte(&mut self) -> Option<u8> {
        (**self).prev_byte()
    }

    fn next_char(&mut self) -> Option<char> {
        (**self).next_char()
    }

    fn prev_char(&mut self) -> Option<char> {
        (**self).prev_char()
    }
}

#[derive(Clone, Debug)]
pub struct Rev<C> {
    cursor: C,
}

impl<C: Cursor> Cursor for Rev<C> {
    fn is_at_start_of_text(&self) -> bool {
        self.cursor.is_at_end_of_text()
    }

    fn is_at_end_of_text(&self) -> bool {
        self.cursor.is_at_start_of_text()
    }

    fn byte_position(&self) -> usize {
        self.cursor.byte_position()
    }

    fn move_to(&mut self, byte_position: usize) {
        self.cursor.move_to(byte_position)
    }

    fn next_byte(&mut self) -> Option<u8> {
        self.cursor.prev_byte()
    }

    fn prev_byte(&mut self) -> Option<u8> {
        self.cursor.next_byte()
    }

    fn next_char(&mut self) -> Option<char> {
        self.cursor.prev_char()
    }

    fn prev_char(&mut self) -> Option<char> {
        self.cursor.next_char()
    }
}
