pub trait Cursor {
    fn is_at_front(&self) -> bool;
    fn is_at_back(&self) -> bool;
    fn byte_position(&self) -> usize;
    fn current(&self) -> Option<char>;
    fn move_next(&mut self);
    fn move_prev(&mut self);
}
