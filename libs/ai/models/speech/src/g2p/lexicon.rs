//! Binary search over misaki's packed IPA lexicon.
//!
//! The data is embedded rather than loaded: 2.8MB is small next to the 312MB of
//! weights, and it means a build has no runtime file to lose.

use super::packed::Packed;

static LEXICON: Packed = Packed::new(include_bytes!("../../data/us_lexicon.bin"));

fn lookup_exact(word: &str) -> Option<&'static str> {
    LEXICON.get(word)
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
