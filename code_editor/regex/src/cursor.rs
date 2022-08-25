pub trait Cursor {
    fn is_at_start_of_text(&self) -> bool;
    fn is_at_end_of_text(&self) -> bool;
    fn byte_position(&self) -> usize;
    fn current_byte(&self) -> Option<u8>;
    fn current_char(&self) -> Option<char>;
    fn move_next_byte(&mut self);
    fn move_prev_byte(&mut self);
    fn move_next_char(&mut self);
    fn move_prev_char(&mut self);
}
