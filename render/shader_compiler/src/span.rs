#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Span {
    pub file_id: usize,
    pub start: usize,
    pub end: usize,
}