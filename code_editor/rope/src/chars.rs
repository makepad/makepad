use {
    crate::{Chunks, Slice},
    std::str,
};

#[derive(Clone, Debug)]
pub struct Chars<'a> {
    chars: Option<str::Chars<'a>>,
    chunks: Chunks<'a>,
}

impl<'a> Chars<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            chars: None,
            chunks: slice.chunks(),
        }
    }
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.chars {
                Some(chars) => match chars.next() {
                    Some(ch) => break Some(ch),
                    None => {
                        self.chars = None;
                        continue;
                    }
                },
                None => match self.chunks.next() {
                    Some(chunk) => {
                        self.chars = Some(chunk.chars());
                        continue;
                    }
                    None => break None,
                },
            }
        }
    }
}
