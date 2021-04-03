use std::fmt;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct LiveFileId(pub u16);

#[derive(Clone, Copy, Default, Eq, Ord, PartialOrd, PartialEq)]
pub struct Span {
   store: u64,
}

impl Span {
    pub fn new(live_file_id: LiveFileId, start: usize, end: usize)->Self{
        Span {
            store:
            (((live_file_id.0 as u64) & 0xffff) << 48) |
            (((start as u64) & 0xffffff) << 24) |
            (((end as u64) & 0xffffff) << 0)
        }
    }
    pub fn start(&self)->usize{
        ((self.store>>24)&0xffffff) as usize
    }
    pub fn end(&self)->usize{
        (self.store&0xffffff) as usize
    }
    pub fn len(&self)->usize{
        self.end() - self.start()
    }    
    
    pub fn live_file_id(&self)->LiveFileId{
        LiveFileId(((self.store>>48)&0xffff) as u16)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Span(start:{}, end:{}, live_file_id:{})", self.start(), self.end(), self.live_file_id().0)
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Span(start:{}, end:{}, live_file_id:{})", self.start(), self.end(), self.live_file_id().0)
    }
}

