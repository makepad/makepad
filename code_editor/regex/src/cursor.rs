pub trait Cursor {
    fn peek_char(&self) -> Option<char>;
    fn skip_char(&mut self);
}
