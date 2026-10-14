//! A read-only map packed by `tools/build_lexicon.py` / `build_zh_lexicon.py`
//! (the MKLEX format), searched in place: no parsing, no allocation.

pub(crate) const MAGIC: &[u8] = b"MKLEX\0\0\0";
const ENTRY_SIZE: usize = 8;

/// Sorted `key -> value` strings in one byte slice (a table [`super::data`] loaded).
#[derive(Clone, Copy)]
pub struct Packed(&'static [u8]);

impl Packed {
    pub const fn new(data: &'static [u8]) -> Self {
        Self(data)
    }

    pub fn len(&self) -> usize {
        debug_assert_eq!(&self.0[..MAGIC.len()], MAGIC, "bad lexicon magic");
        u32::from_le_bytes(self.0[8..12].try_into().unwrap()) as usize
    }

    fn blob_start(&self) -> usize {
        12 + self.len() * ENTRY_SIZE
    }

    /// The NUL-terminated string at `offset` within the blob.
    fn string_at(&self, offset: usize) -> &'static str {
        let start = self.blob_start() + offset;
        let end = start + self.0[start..].iter().position(|b| *b == 0).unwrap_or(0);
        std::str::from_utf8(&self.0[start..end]).unwrap_or("")
    }

    fn entry(&self, index: usize) -> (&'static str, &'static str) {
        let at = 12 + index * ENTRY_SIZE;
        let key = u32::from_le_bytes(self.0[at..at + 4].try_into().unwrap()) as usize;
        let value = u32::from_le_bytes(self.0[at + 4..at + 8].try_into().unwrap()) as usize;
        (self.string_at(key), self.string_at(value))
    }

    pub fn get(&self, key: &str) -> Option<&'static str> {
        let (mut low, mut high) = (0usize, self.len());
        while low < high {
            let mid = (low + high) / 2;
            let (candidate, value) = self.entry(mid);
            match candidate.cmp(key) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Some(value),
            }
        }
        None
    }
}
