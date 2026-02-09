use {super::char::CharExt, std::ops::Range};

pub trait Input<'a> {
    type Cursor: Cursor;

    fn slice(&self, range: Range<usize>) -> Self;
    
    fn cursor_start(&self) -> Self::Cursor;

    fn cursor_end(&self) -> Self::Cursor;
}

pub trait Cursor: Clone {
    fn is_start(&self) -> bool;

    fn is_end(&self) -> bool;

    fn index(&self) -> usize;

    fn current_byte(&mut self) -> Option<u8>;

    fn current_char(&mut self) -> Option<char>;

    fn prev_byte(&mut self) -> Option<u8>;

    fn prev_char(&mut self) -> Option<char>;
    
    fn move_next_byte(&mut self) -> bool;

    fn move_next_char(&mut self) -> bool;

    fn move_prev_byte(&mut self) -> bool;
    
    fn move_prev_char(&mut self) -> bool;

    fn is_line_start(&mut self) -> bool {
        self.prev_char().map_or(true, |c| c == '\n')
    }

    fn is_line_end(&mut self) -> bool {
        self.current_char().map_or(true, |c| c == '\n')
    }

    fn is_word_boundary(&mut self) -> bool {
        self.prev_char().map(|c| c.is_word()) != self.current_char().map(|c| c.is_word())
    }

    fn rev(self) -> RevCursor<Self>
    where
        Self: Sized,
    {
        RevCursor { cursor: self }
    }
}

#[derive(Clone, Debug)]
pub struct RevCursor<C> {
    cursor: C,
}

impl<C: Cursor> Cursor for RevCursor<C> {
    fn is_start(&self) -> bool {
        self.cursor.is_end()
    }

    fn is_end(&self) -> bool {
        self.cursor.is_start()
    }

    fn index(&self) -> usize {
        self.cursor.index()
    }

    fn current_byte(&mut self) -> Option<u8> {
        self.cursor.prev_byte()
    }

    fn current_char(&mut self) -> Option<char> {
        self.cursor.prev_char()
    }

    fn prev_byte(&mut self) -> Option<u8> {
        self.cursor.current_byte()
    }

    fn prev_char(&mut self) -> Option<char> {
        self.cursor.current_char()
    }

    fn move_next_byte(&mut self) -> bool {
        self.cursor.move_prev_byte()
    }

    fn move_next_char(&mut self) -> bool {
        self.cursor.move_prev_char()
    }

    fn move_prev_byte(&mut self) -> bool {
        self.cursor.move_next_byte()
    }
    
    fn move_prev_char(&mut self) -> bool {
        self.cursor.move_next_char()
    }
}
