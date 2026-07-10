//! Binary search over misaki's packed IPA lexicon.
//!
//! The data is embedded rather than loaded: 2.8MB is small next to the 312MB of
//! weights, and it means a build has no runtime file to lose.

const DATA: &[u8] = include_bytes!("../../data/us_lexicon.bin");
const MAGIC: &[u8] = b"MKLEX\0\0\0";
const ENTRY_SIZE: usize = 8;

fn count() -> usize {
    debug_assert_eq!(&DATA[..MAGIC.len()], MAGIC, "bad lexicon magic");
    u32::from_le_bytes(DATA[8..12].try_into().unwrap()) as usize
}

fn blob_start() -> usize {
    12 + count() * ENTRY_SIZE
}

/// The NUL-terminated string at `offset` within the blob.
fn string_at(offset: usize) -> &'static str {
    let start = blob_start() + offset;
    let end = start + DATA[start..].iter().position(|b| *b == 0).unwrap_or(0);
    std::str::from_utf8(&DATA[start..end]).unwrap_or("")
}

fn entry(index: usize) -> (&'static str, &'static str) {
    let at = 12 + index * ENTRY_SIZE;
    let key = u32::from_le_bytes(DATA[at..at + 4].try_into().unwrap()) as usize;
    let value = u32::from_le_bytes(DATA[at + 4..at + 8].try_into().unwrap()) as usize;
    (string_at(key), string_at(value))
}

fn lookup_exact(word: &str) -> Option<&'static str> {
    let (mut low, mut high) = (0usize, count());
    while low < high {
        let mid = (low + high) / 2;
        let (key, value) = entry(mid);
        match key.cmp(word) {
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
            std::cmp::Ordering::Equal => return Some(value),
        }
    }
    None
}

/// IPA for `word`, in Kokoro's own symbol set. Tries the word as written, then
/// lowercased — the lexicon holds acronyms like `AA` in upper case.
pub fn lookup(word: &str) -> Option<&'static str> {
    if let Some(found) = lookup_exact(word) {
        return Some(found);
    }
    let lower = word.to_lowercase();
    if lower != word {
        return lookup_exact(&lower);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::lookup;

    #[test]
    fn known_words_resolve() {
        assert_eq!(lookup("hello"), Some("həlˈO"));
        assert_eq!(lookup("game"), Some("ɡˈAm"));
        // Case-insensitive fallback.
        assert_eq!(lookup("Hello"), Some("həlˈO"));
        // A word a child invents is not in any lexicon.
        assert_eq!(lookup("pacman"), None);
    }
}
