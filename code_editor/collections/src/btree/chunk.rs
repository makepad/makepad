use super::Info;

pub trait Chunk: Clone {
    type Info: Info;

    const MAX_LEN: usize;

    fn new() -> Self;
    fn is_empty(&self) -> bool;
    fn len(&self) -> usize;
    fn info(&self) -> Self::Info;
    fn move_left(&mut self, other: &mut Self, end: usize);
    fn move_right(&mut self, other: &mut Self, end: usize);
}
