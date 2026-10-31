//! The phoneme inventory: English IPA in the TTS notation (the same symbols
//! Score's singer parses; diphthongs are two vowels), plus silence and breath.
//! A phoneme is a `u8` index into [`SYMBOLS`]; the model's embedding table has
//! [`TABLE`] rows so the inventory can grow without a new checkpoint shape.

pub type Ph = u8;

pub const TABLE: usize = 64;

pub const SYMBOLS: [&str; 45] = [
    "<pad>", "SP", "AP", // silence, audible breath
    "i", "ɪ", "e", "ɛ", "æ", "a", "ɑ", "ɒ", "ɔ", "o", "ʊ", "u", "ʌ", "ə", "ɜ", "ɝ", // vowels 3..=18
    "p", "b", "t", "d", "k", "ɡ", "f", "v", "θ", "ð", "s", "z", "ʃ", "ʒ", "h", "m", "n", "ŋ", "l", "ɹ", "w", "j", "ʧ",
    "ʤ", "ɾ", "ʔ",
];

pub const PAD: Ph = 0;
pub const SP: Ph = 1;
pub const AP: Ph = 2;

pub fn id(sym: &str) -> Option<Ph> {
    SYMBOLS.iter().position(|s| *s == sym).map(|i| i as Ph)
}

pub fn symbol(ph: Ph) -> &'static str {
    SYMBOLS.get(ph as usize).copied().unwrap_or("?")
}

pub fn is_vowel(ph: Ph) -> bool {
    (3..=18).contains(&ph)
}

pub fn is_voiced(ph: Ph) -> bool {
    if is_vowel(ph) {
        return true;
    }
    matches!(symbol(ph), "b" | "d" | "ɡ" | "v" | "ð" | "z" | "ʒ" | "m" | "n" | "ŋ" | "l" | "ɹ" | "w" | "j" | "ʤ" | "ɾ")
}

/// Sung consonant length in seconds (onset or coda), before the duration
/// model is trained or when it is off.
pub fn rule_len(ph: Ph, coda: bool) -> f32 {
    let base = match symbol(ph) {
        "p" | "t" | "k" => if coda { 0.07 } else { 0.08 },
        "b" | "d" | "ɡ" => 0.05,
        "f" | "θ" | "s" | "ʃ" => if coda { 0.11 } else { 0.1 },
        "v" | "ð" | "z" | "ʒ" => 0.07,
        "h" => 0.07,
        "m" | "n" | "ŋ" => if coda { 0.09 } else { 0.07 },
        "l" | "ɹ" => 0.06,
        "w" | "j" => 0.05,
        "ʧ" | "ʤ" => 0.11,
        "ɾ" => 0.025,
        "ʔ" => 0.03,
        _ => 0.06,
    };
    base
}

/// Parse a pronunciation in the TTS IPA notation (stress and length marks
/// skipped; the capital-letter diphthongs split in two vowels).
pub fn parse(pron: &str) -> Vec<Ph> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        if let Some(p) = id(s) {
            out.push(p)
        }
    };
    for ch in pron.chars() {
        match ch {
            'A' => {
                push("e");
                push("ɪ")
            }
            'I' => {
                push("a");
                push("ɪ")
            }
            'O' => {
                push("o");
                push("ʊ")
            }
            'W' => {
                push("a");
                push("ʊ")
            }
            'Y' => {
                push("ɔ");
                push("ɪ")
            }
            'Q' => {
                push("ə");
                push("ʊ")
            }
            'ᵊ' | 'ɐ' | 'ɨ' | 'ᵻ' => push("ə"),
            'ɚ' => push("ɝ"),
            'r' | 'ɻ' => push("ɹ"),
            'g' => push("ɡ"),
            'y' => push("j"),
            'ʦ' => {
                push("t");
                push("s")
            }
            'T' => push("ɾ"),
            'ɫ' => push("l"),
            'c' => push("k"),
            'ç' | 'x' | 'χ' => push("h"),
            _ => {
                let mut b = [0u8; 4];
                push(ch.encode_utf8(&mut b));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_classes() {
        let p = parse("həlˈO");
        assert_eq!(p.iter().map(|p| symbol(*p)).collect::<String>(), "həloʊ");
        assert!(is_vowel(id("ɝ").unwrap()) && !is_vowel(id("p").unwrap()));
        assert!(is_voiced(id("m").unwrap()) && !is_voiced(id("s").unwrap()));
        assert!(SYMBOLS.len() <= TABLE);
    }
}
