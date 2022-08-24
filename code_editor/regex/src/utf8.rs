use {
    crate::Range,
    std::ops::{Deref, DerefMut},
};

const MAX_LEN: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct Encoder {
    range_stack: Vec<Range<u32>>,
}

impl Encoder {
    pub(crate) fn new() -> Self {
        Self {
            range_stack: Vec::new(),
        }
    }

    pub(crate) fn encode(&mut self, char_range: Range<char>) -> Encode<'_> {
        self.range_stack
            .push(Range::new(char_range.start as u32, char_range.end as u32));
        Encode {
            range_stack: &mut self.range_stack,
        }
    }
}

pub(crate) struct Encode<'a> {
    range_stack: &'a mut Vec<Range<u32>>,
}

impl<'a> Iterator for Encode<'a> {
    type Item = ByteRanges;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(mut range) = self.range_stack.pop() {
            'LOOP: loop {
                if range.end <= 0x7F {
                    return Some(ByteRanges::One([Range::new(
                        range.start as u8,
                        range.end as u8,
                    )]));
                }
                if range.start < 0xE000 && range.end > 0xD7FF {
                    self.range_stack.push(Range::new(0xE000, range.end));
                    range.end = 0xD7FF;
                    continue 'LOOP;
                }
                for index in 1..MAX_LEN {
                    let max = max_scalar(index);
                    if range.start <= max && max < range.end {
                        self.range_stack.push(Range::new(max + 1, range.end));
                        range.end = max;
                        continue 'LOOP;
                    }
                }
                for index in 1..MAX_LEN {
                    let mask = (1 << (6 * index)) - 1;
                    if range.start & !mask != range.end & !mask {
                        if range.start & mask != 0 {
                            self.range_stack
                                .push(Range::new((range.start | mask) + 1, range.end));
                            range.end = range.start | mask;
                            continue 'LOOP;
                        }
                        if range.end & mask != mask {
                            self.range_stack
                                .push(Range::new(range.end & !mask, range.end));
                            range.end = (range.end & !mask) - 1;
                            continue 'LOOP;
                        }
                    }
                }
                let mut start = [0; MAX_LEN];
                let start = char::from_u32(range.start)
                    .unwrap()
                    .encode_utf8(&mut start)
                    .as_bytes();
                let mut end = [0; MAX_LEN];
                let end = char::from_u32(range.end)
                    .unwrap()
                    .encode_utf8(&mut end)
                    .as_bytes();
                return Some(match start.len() {
                    2 => ByteRanges::Two([
                        Range::new(start[0], end[0]),
                        Range::new(start[1], end[1]),
                    ]),
                    3 => ByteRanges::Three([
                        Range::new(start[0], end[0]),
                        Range::new(start[1], end[1]),
                        Range::new(start[2], end[2]),
                    ]),
                    4 => ByteRanges::Four([
                        Range::new(start[0], end[0]),
                        Range::new(start[1], end[1]),
                        Range::new(start[2], end[2]),
                        Range::new(start[3], end[3]),
                    ]),
                    _ => panic!(),
                });
            }
        }
        None
    }
}

impl<'a> Drop for Encode<'a> {
    fn drop(&mut self) {
        self.range_stack.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ByteRanges {
    One([Range<u8>; 1]),
    Two([Range<u8>; 2]),
    Three([Range<u8>; 3]),
    Four([Range<u8>; 4]),
}

impl ByteRanges {
    fn as_slice(&self) -> &[Range<u8>] {
        match self {
            Self::One(byte_ranges) => byte_ranges.as_slice(),
            Self::Two(byte_ranges) => byte_ranges.as_slice(),
            Self::Three(byte_ranges) => byte_ranges.as_slice(),
            Self::Four(byte_ranges) => byte_ranges.as_slice(),
        }
    }

    fn as_mut_slice(&mut self) -> &mut [Range<u8>] {
        match self {
            Self::One(byte_ranges) => byte_ranges.as_mut_slice(),
            Self::Two(byte_ranges) => byte_ranges.as_mut_slice(),
            Self::Three(byte_ranges) => byte_ranges.as_mut_slice(),
            Self::Four(byte_ranges) => byte_ranges.as_mut_slice(),
        }
    }
}

impl Deref for ByteRanges {
    type Target = [Range<u8>];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for ByteRanges {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

fn max_scalar(len: usize) -> u32 {
    match len {
        1 => 0x7F,
        2 => 0x7FF,
        3 => 0xFFFF,
        4 => 0x10FFFF,
        _ => panic!(),
    }
}

mod tests {
    use super::*;

    #[test]
    fn encode() {
        let mut encoder = Encoder::new();
        assert_eq!(
            encoder
                .encode(Range::new('\u{0}', '\u{10FFFF}'))
                .collect::<Vec<_>>(),
            vec![
                ByteRanges::One([Range::new(0x00, 0x7F)]),
                ByteRanges::Two([Range::new(0xC2, 0xDF), Range::new(0x80, 0xBF)]),
                ByteRanges::Three([
                    Range::new(0xE0, 0xE0),
                    Range::new(0xA0, 0xBF),
                    Range::new(0x80, 0xBF)
                ]),
                ByteRanges::Three([
                    Range::new(0xE1, 0xEC),
                    Range::new(0x80, 0xBF),
                    Range::new(0x80, 0xBF)
                ]),
                ByteRanges::Three([
                    Range::new(0xED, 0xED),
                    Range::new(0x80, 0x9F),
                    Range::new(0x80, 0xBF)
                ]),
                ByteRanges::Three([
                    Range::new(0xEE, 0xEF),
                    Range::new(0x80, 0xBF),
                    Range::new(0x80, 0xBF)
                ]),
                ByteRanges::Four([
                    Range::new(0xF0, 0xF0),
                    Range::new(0x90, 0xBF),
                    Range::new(0x80, 0xBF),
                    Range::new(0x80, 0xBF),
                ]),
                ByteRanges::Four([
                    Range::new(0xF1, 0xF3),
                    Range::new(0x80, 0xBF),
                    Range::new(0x80, 0xBF),
                    Range::new(0x80, 0xBF),
                ]),
                ByteRanges::Four([
                    Range::new(0xF4, 0xF4),
                    Range::new(0x80, 0x8F),
                    Range::new(0x80, 0xBF),
                    Range::new(0x80, 0xBF),
                ])
            ]
        );
    }
}
