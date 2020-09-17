#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct LiveBodyId(pub usize);

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct Span {
    pub live_body_id: LiveBodyId,
    pub start: usize,
    pub end: usize,
}
