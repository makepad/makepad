pub trait Input {
    type Cursor: Cursor;

    fn cursor_front(&self) -> Self::Cursor;
    fn cursor_back(&self) -> Self::Cursor;
}

pub trait Cursor {
    fn peek_next_char(&self) -> Option<char>;
    fn peek_prev_char(&self) -> Option<char>;
    fn skip_next_char(&mut self);
    fn skip_prev_char(&mut self);

    fn rev(self) -> Rev<Self>
    where
        Self: Sized,
    {
        Rev { cursor: self }
    }
}

pub struct Rev<C> {
    cursor: C,
}

impl<C: Cursor> Cursor for Rev<C> {
    fn peek_next_char(&self) -> Option<char> {
        self.cursor.peek_prev_char()
    }

    fn peek_prev_char(&self) -> Option<char> {
        self.cursor.peek_next_char()
    }

    fn skip_next_char(&mut self) {
        self.cursor.skip_prev_char()
    }

    fn skip_prev_char(&mut self) {
        self.cursor.skip_next_char()
    }
}
