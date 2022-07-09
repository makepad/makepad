use std::{ops::Range, slice::Iter};

pub struct CharRanges<'a> {
    iter: Iter<'a, ([u8; 3], [u8; 3])>,
}

impl<'a> CharRanges<'a> {
    pub(super) fn new(table: &'a [([u8; 3], [u8; 3])]) -> Self {
        Self {
            iter: table.iter(),
        }
    }
}

impl<'a> Iterator for CharRanges<'a> {
    type Item = Range<u32>;

    fn next(&mut self) -> Option<Self::Item> {
        let (first_bytes, last_bytes) = self.iter.next()?;
        let first = u32::from_be_bytes([0, first_bytes[0], first_bytes[1], first_bytes[2]]);
        let last = u32::from_be_bytes([0, last_bytes[0], last_bytes[1], last_bytes[2]]);
        Some(first..last)    
    }
}