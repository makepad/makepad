#[derive(Clone, Copy, Debug, Default, Hash, Eq, Ord, PartialOrd, PartialEq)]
pub struct LiveFileId(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct Span {
    pub live_file_id: LiveFileId,
    pub start: u32,
    pub end: u32,
}
