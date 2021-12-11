use{
    std::fmt,
    crate::live_id::LiveFileId,
};

#[derive(Clone, Copy, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct TextPos {
    pub line: u32,
    pub column: u32
}

#[derive(Clone, Copy, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct Span {
    pub file_id: LiveFileId,
    pub start: TextPos,
    pub end: TextPos
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Span(start:{}, end:{}, file_id:{})", self.start, self.end, self.file_id.to_index())
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Span(start:{}, end:{}, file_id:{})", self.start, self.end, self.file_id.to_index())
    }
}

impl fmt::Display for TextPos {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

impl fmt::Debug for TextPos {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

