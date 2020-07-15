#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Span {
    pub loc_id: usize,
    pub start: usize,
    pub end: usize,
}