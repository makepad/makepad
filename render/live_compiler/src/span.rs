#[derive(Clone, Copy, Debug, Default, Hash, Eq, Ord, PartialOrd, PartialEq)]
pub struct LiveBodyId(pub usize);

impl LiveBodyId{
    pub fn as_index(&self)->usize{self.0}
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct Span {
    pub live_body_id: LiveBodyId,
    pub start: usize,
    pub end: usize,
}
