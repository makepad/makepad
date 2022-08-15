pub trait Cursor {
    fn is_at_front(&self) -> bool;
    fn is_at_back(&self) -> bool;
    fn byte_position(&self) -> usize;
    fn current_char(&self) -> Option<char>;
    fn move_next_char(&mut self);
    fn move_prev_char(&mut self);
}
