use {
    crate::{ChunksRev, Slice},
    std::str,
};

#[derive(Clone, Debug)]
pub struct BytesRev<'a> {
    bytes: Option<str::Bytes<'a>>,
    chunks_rev: ChunksRev<'a>,
}

impl<'a> BytesRev<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            bytes: None,
            chunks_rev: slice.chunks_rev(),
        }
    }
}

impl<'a> Iterator for BytesRev<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.bytes {
                Some(bytes) => match bytes.next_back() {
                    Some(byte) => break Some(byte),
                    None => {
                        self.bytes = None;
                        continue;
                    }
                },
                None => match self.chunks_rev.next() {
                    Some(chunk) => {
                        self.bytes = Some(chunk.bytes());
                        continue;
                    }
                    None => break None,
                },
            }
        }
    }
}
