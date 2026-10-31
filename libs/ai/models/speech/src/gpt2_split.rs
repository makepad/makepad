//! The GPT-2 / tiktoken pre-tokenizer both whisper-vocab BPEs here share
//! (Whisper's own text encoder and IndexTTS's): text into the pieces BPE
//! merges within, never across.
//!
//! The reference pattern is
//! `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`,
//! hand-rolled (no regex dependency). `\p{N}` is `char::is_numeric`; `\p{L}`
//! is `char::is_alphabetic` minus [`ALPHABETIC_NOT_LETTER`], the code points
//! std calls alphabetic that are not letters: combining marks (Thai and
//! Devanagari vowel signs, Arabic harakat), letter numbers, circled letters.
//! Without that table a Thai or Hindi word splits differently from tiktoken.
//! Pieces always tile the input exactly.

/// Hand-rolled GPT-2 pre-tokenizer:
/// `'s|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+`
pub(crate) struct Gpt2Splitter<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Gpt2Splitter<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }
}

fn is_letter(c: char) -> bool {
    c.is_alphabetic() && {
        let c = c as u32;
        let index = ALPHABETIC_NOT_LETTER.partition_point(|(_, last)| *last < c);
        ALPHABETIC_NOT_LETTER.get(index).map_or(true, |(first, _)| c < *first)
    }
}

/// Inclusive ranges of code points that `char::is_alphabetic` accepts but
/// Unicode's `\p{L}` does not, sorted. Generated (Python `unicodedata` 16.0
/// for the categories, filtered by this toolchain's `is_alphabetic`):
/// every code point whose category is not `L*` and that `is_alphabetic`
/// accepts, merged into runs.
const ALPHABETIC_NOT_LETTER: &[(u32, u32)] = &[
    (0x0345, 0x0345), (0x0363, 0x036f), (0x05b0, 0x05bd), (0x05bf, 0x05bf), (0x05c1, 0x05c2),
    (0x05c4, 0x05c5), (0x05c7, 0x05c7), (0x0610, 0x061a), (0x064b, 0x0657), (0x0659, 0x065f),
    (0x0670, 0x0670), (0x06d6, 0x06dc), (0x06e1, 0x06e4), (0x06e7, 0x06e8), (0x06ed, 0x06ed),
    (0x0711, 0x0711), (0x0730, 0x073f), (0x07a6, 0x07b0), (0x0816, 0x0817), (0x081b, 0x0823),
    (0x0825, 0x0827), (0x0829, 0x082c), (0x088f, 0x088f), (0x0897, 0x0897), (0x08d4, 0x08df),
    (0x08e3, 0x08e9), (0x08f0, 0x0903), (0x093a, 0x093b), (0x093e, 0x094c), (0x094e, 0x094f),
    (0x0955, 0x0957), (0x0962, 0x0963), (0x0981, 0x0983), (0x09be, 0x09c4), (0x09c7, 0x09c8),
    (0x09cb, 0x09cc), (0x09d7, 0x09d7), (0x09e2, 0x09e3), (0x0a01, 0x0a03), (0x0a3e, 0x0a42),
    (0x0a47, 0x0a48), (0x0a4b, 0x0a4c), (0x0a51, 0x0a51), (0x0a70, 0x0a71), (0x0a75, 0x0a75),
    (0x0a81, 0x0a83), (0x0abe, 0x0ac5), (0x0ac7, 0x0ac9), (0x0acb, 0x0acc), (0x0ae2, 0x0ae3),
    (0x0afa, 0x0afc), (0x0b01, 0x0b03), (0x0b3e, 0x0b44), (0x0b47, 0x0b48), (0x0b4b, 0x0b4c),
    (0x0b56, 0x0b57), (0x0b62, 0x0b63), (0x0b82, 0x0b82), (0x0bbe, 0x0bc2), (0x0bc6, 0x0bc8),
    (0x0bca, 0x0bcc), (0x0bd7, 0x0bd7), (0x0c00, 0x0c04), (0x0c3e, 0x0c44), (0x0c46, 0x0c48),
    (0x0c4a, 0x0c4c), (0x0c55, 0x0c56), (0x0c5c, 0x0c5c), (0x0c62, 0x0c63), (0x0c81, 0x0c83),
    (0x0cbe, 0x0cc4), (0x0cc6, 0x0cc8), (0x0cca, 0x0ccc), (0x0cd5, 0x0cd6), (0x0cdc, 0x0cdc),
    (0x0ce2, 0x0ce3), (0x0cf3, 0x0cf3), (0x0d00, 0x0d03), (0x0d3e, 0x0d44), (0x0d46, 0x0d48),
    (0x0d4a, 0x0d4c), (0x0d57, 0x0d57), (0x0d62, 0x0d63), (0x0d81, 0x0d83), (0x0dcf, 0x0dd4),
    (0x0dd6, 0x0dd6), (0x0dd8, 0x0ddf), (0x0df2, 0x0df3), (0x0e31, 0x0e31), (0x0e34, 0x0e3a),
    (0x0e4d, 0x0e4d), (0x0eb1, 0x0eb1), (0x0eb4, 0x0eb9), (0x0ebb, 0x0ebc), (0x0ecd, 0x0ecd),
    (0x0f71, 0x0f83), (0x0f8d, 0x0f97), (0x0f99, 0x0fbc), (0x102b, 0x1036), (0x1038, 0x1038),
    (0x103b, 0x103e), (0x1056, 0x1059), (0x105e, 0x1060), (0x1062, 0x1064), (0x1067, 0x106d),
    (0x1071, 0x1074), (0x1082, 0x108d), (0x108f, 0x108f), (0x109a, 0x109d), (0x16ee, 0x16f0),
    (0x1712, 0x1713), (0x1732, 0x1733), (0x1752, 0x1753), (0x1772, 0x1773), (0x17b6, 0x17c8),
    (0x1885, 0x1886), (0x18a9, 0x18a9), (0x1920, 0x192b), (0x1930, 0x1938), (0x1a17, 0x1a1b),
    (0x1a55, 0x1a5e), (0x1a61, 0x1a74), (0x1abf, 0x1ac0), (0x1acc, 0x1ace), (0x1b00, 0x1b04),
    (0x1b35, 0x1b43), (0x1b80, 0x1b82), (0x1ba1, 0x1ba9), (0x1bac, 0x1bad), (0x1be7, 0x1bf1),
    (0x1c24, 0x1c36), (0x1dd3, 0x1df4), (0x2160, 0x2182), (0x2185, 0x2188), (0x24b6, 0x24e9),
    (0x2de0, 0x2dff), (0x3007, 0x3007), (0x3021, 0x3029), (0x3038, 0x303a), (0xa674, 0xa67b),
    (0xa69e, 0xa69f), (0xa6e6, 0xa6ef), (0xa7ce, 0xa7cf), (0xa7d2, 0xa7d2), (0xa7d4, 0xa7d4),
    (0xa7f1, 0xa7f1), (0xa802, 0xa802), (0xa80b, 0xa80b), (0xa823, 0xa827), (0xa880, 0xa881),
    (0xa8b4, 0xa8c3), (0xa8c5, 0xa8c5), (0xa8ff, 0xa8ff), (0xa926, 0xa92a), (0xa947, 0xa952),
    (0xa980, 0xa983), (0xa9b4, 0xa9bf), (0xa9e5, 0xa9e5), (0xaa29, 0xaa36), (0xaa43, 0xaa43),
    (0xaa4c, 0xaa4d), (0xaa7b, 0xaa7d), (0xaab0, 0xaab0), (0xaab2, 0xaab4), (0xaab7, 0xaab8),
    (0xaabe, 0xaabe), (0xaaeb, 0xaaef), (0xaaf5, 0xaaf5), (0xabe3, 0xabea), (0xfb1e, 0xfb1e),
    (0x10140, 0x10174), (0x10341, 0x10341), (0x1034a, 0x1034a), (0x10376, 0x1037a),
    (0x103d1, 0x103d5), (0x10940, 0x10959), (0x10a01, 0x10a03), (0x10a05, 0x10a06),
    (0x10a0c, 0x10a0f), (0x10d24, 0x10d27), (0x10d69, 0x10d69), (0x10eab, 0x10eac),
    (0x10ec5, 0x10ec7), (0x10efa, 0x10efc), (0x11000, 0x11002), (0x11038, 0x11045),
    (0x11073, 0x11074), (0x11080, 0x11082), (0x110b0, 0x110b8), (0x110c2, 0x110c2),
    (0x11100, 0x11102), (0x11127, 0x11132), (0x11145, 0x11146), (0x11180, 0x11182),
    (0x111b3, 0x111bf), (0x111ce, 0x111cf), (0x1122c, 0x11234), (0x11237, 0x11237),
    (0x1123e, 0x1123e), (0x11241, 0x11241), (0x112df, 0x112e8), (0x11300, 0x11303),
    (0x1133e, 0x11344), (0x11347, 0x11348), (0x1134b, 0x1134c), (0x11357, 0x11357),
    (0x11362, 0x11363), (0x113b8, 0x113c0), (0x113c2, 0x113c2), (0x113c5, 0x113c5),
    (0x113c7, 0x113ca), (0x113cc, 0x113cd), (0x11435, 0x11441), (0x11443, 0x11445),
    (0x114b0, 0x114c1), (0x115af, 0x115b5), (0x115b8, 0x115be), (0x115dc, 0x115dd),
    (0x11630, 0x1163e), (0x11640, 0x11640), (0x116ab, 0x116b5), (0x1171d, 0x1172a),
    (0x1182c, 0x11838), (0x11930, 0x11935), (0x11937, 0x11938), (0x1193b, 0x1193c),
    (0x11940, 0x11940), (0x11942, 0x11942), (0x119d1, 0x119d7), (0x119da, 0x119df),
    (0x119e4, 0x119e4), (0x11a01, 0x11a0a), (0x11a35, 0x11a39), (0x11a3b, 0x11a3e),
    (0x11a51, 0x11a5b), (0x11a8a, 0x11a97), (0x11b60, 0x11b67), (0x11c2f, 0x11c36),
    (0x11c38, 0x11c3e), (0x11c92, 0x11ca7), (0x11ca9, 0x11cb6), (0x11d31, 0x11d36),
    (0x11d3a, 0x11d3a), (0x11d3c, 0x11d3d), (0x11d3f, 0x11d41), (0x11d43, 0x11d43),
    (0x11d47, 0x11d47), (0x11d8a, 0x11d8e), (0x11d90, 0x11d91), (0x11d93, 0x11d96),
    (0x11db0, 0x11ddb), (0x11ef3, 0x11ef6), (0x11f00, 0x11f01), (0x11f03, 0x11f03),
    (0x11f34, 0x11f3a), (0x11f3e, 0x11f40), (0x12400, 0x1246e), (0x1611e, 0x1612e),
    (0x16ea0, 0x16eb8), (0x16ebb, 0x16ed3), (0x16f4f, 0x16f4f), (0x16f51, 0x16f87),
    (0x16f8f, 0x16f92), (0x16ff0, 0x16ff6), (0x187f8, 0x187ff), (0x18d09, 0x18d1e),
    (0x18d80, 0x18df2), (0x1bc9e, 0x1bc9e), (0x1e000, 0x1e006), (0x1e008, 0x1e018),
    (0x1e01b, 0x1e021), (0x1e023, 0x1e024), (0x1e026, 0x1e02a), (0x1e08f, 0x1e08f),
    (0x1e6c0, 0x1e6de), (0x1e6e0, 0x1e6f5), (0x1e6fe, 0x1e6ff), (0x1e947, 0x1e947),
    (0x1f130, 0x1f149), (0x1f150, 0x1f169), (0x1f170, 0x1f189), (0x2b73a, 0x2b73f),
    (0x2cea2, 0x2cead), (0x323b0, 0x33479),
];

fn is_number(c: char) -> bool {
    c.is_numeric()
}

impl<'a> Iterator for Gpt2Splitter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let rest = &self.text[self.pos..];
        if rest.is_empty() {
            return None;
        }
        let start = self.pos;
        let mut chars = rest.chars();
        let first = chars.next().unwrap();

        // Contractions: 's 't 're 've 'm 'll 'd (case-sensitive, as in the
        // reference pattern; input is lowercased upstream anyway).
        if first == '\'' {
            for suffix in ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"] {
                if rest.starts_with(suffix) {
                    self.pos += suffix.len();
                    return Some(&self.text[start..self.pos]);
                }
            }
        }

        // ` ?\p{L}+`, ` ?\p{N}+`, ` ?[^\s\p{L}\p{N}]+` — one optional leading
        // ASCII space, then a run of one class.
        let (lead_space, class_first) = if first == ' ' {
            match chars.next() {
                Some(c) => (true, c),
                None => {
                    // Lone trailing space: falls through to the whitespace arm.
                    self.pos += 1;
                    return Some(&self.text[start..self.pos]);
                }
            }
        } else {
            (false, first)
        };

        if !class_first.is_whitespace() {
            let class: fn(char) -> bool = if is_letter(class_first) {
                is_letter
            } else if is_number(class_first) {
                is_number
            } else {
                |c: char| !c.is_whitespace() && !is_letter(c) && !is_number(c)
            };
            let mut end = start + lead_space as usize + class_first.len_utf8();
            for c in self.text[end..].chars() {
                if class(c) && !c.is_whitespace() {
                    end += c.len_utf8();
                } else {
                    break;
                }
            }
            self.pos = end;
            return Some(&self.text[start..end]);
        }

        // Whitespace run (first char is whitespace, or the lone-space case
        // above already returned). `\s+(?!\S)` keeps the final whitespace
        // char for the next token when non-space follows.
        let mut end = start;
        for c in self.text[start..].chars() {
            if c.is_whitespace() {
                end += c.len_utf8();
            } else {
                break;
            }
        }
        let followed_by_nonspace = end < self.text.len();
        if followed_by_nonspace {
            // Leave the last whitespace char to prefix the next token
            // (`\s+(?!\S)` semantics) — unless the run is a single char, in
            // which case `\s+` takes it whole.
            let last_len = self.text[start..end].chars().last().unwrap().len_utf8();
            if end - last_len > start {
                end -= last_len;
            }
        }
        self.pos = end;
        Some(&self.text[start..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splitter(text: &str) -> Vec<&str> {
        Gpt2Splitter::new(text).collect()
    }

    #[test]
    fn gpt2_pattern_splits() {
        assert_eq!(splitter("hello world"), vec!["hello", " world"]);
        assert_eq!(splitter("it's 42 items."), vec!["it", "'s", " 42", " items", "."]);
        assert_eq!(splitter("a  b"), vec!["a", " ", " b"]);
        assert_eq!(splitter("a \n"), vec!["a", " \n"]);
        assert_eq!(splitter("ab12cd"), vec!["ab", "12", "cd"]);
        assert_eq!(splitter(" \nX"), vec![" ", "\n", "X"]);
        assert_eq!(splitter("I'M ok?!'s"), vec!["I", "'", "M", " ok", "?!'", "s"]);
        assert_eq!(splitter(" café, naïve"), vec![" café", ",", " naïve"]);
        // Thai: a vowel sign and a tone mark are marks, not letters.
        assert_eq!(splitter("ที่นี่"), vec!["ท", "ี่", "น", "ี่"]);
        // Combining acute (NFD "é") and a Roman numeral (a number, not a letter).
        assert_eq!(splitter("e\u{301}t Ⅻ"), vec!["e", "\u{301}", "t", " Ⅻ"]);
    }

    #[test]
    fn pieces_tile_the_input() {
        for text in ["", " ", "  lead and trail  ", "日本語のテキスト。", "a\t\tb\r\n c", "¿Qué? ¡Sí! 3.14…"] {
            assert_eq!(splitter(text).concat(), text);
        }
    }
}
