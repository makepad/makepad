//! Letter-to-sound fallback for words no lexicon has.
//!
//! This exists because a child names things. "Pacman", "Gummer", "Zorblax" are
//! not in any dictionary, and a TTS that skips them, or spells them out, breaks
//! the illusion. The rules below are ordinary American English spelling
//! conventions; they are approximate by nature, and only ever run on the words
//! that missed the 90k-entry lexicon.

/// Longest match wins, so multi-letter graphemes precede their prefixes.
const RULES: &[(&str, &str)] = &[
    ("igh", "I"),
    ("tch", "ʧ"),
    ("dge", "ʤ"),
    ("ch", "ʧ"),
    ("sh", "ʃ"),
    ("th", "θ"),
    ("ph", "f"),
    ("wh", "w"),
    ("ck", "k"),
    ("ng", "ŋ"),
    ("qu", "kw"),
    ("ee", "i"),
    ("ea", "i"),
    ("oo", "u"),
    ("ou", "W"),
    ("ow", "W"),
    ("oi", "Y"),
    ("oy", "Y"),
    ("ai", "A"),
    ("ay", "A"),
    ("oa", "O"),
    ("ar", "ɑɹ"),
    ("or", "ɔɹ"),
    ("er", "ɚ"),
    ("ir", "ɚ"),
    ("ur", "ɚ"),
    ("a", "æ"),
    ("e", "ɛ"),
    ("i", "ɪ"),
    ("o", "ɑ"),
    ("u", "ʌ"),
    ("y", "i"),
    ("b", "b"),
    ("c", "k"),
    ("d", "d"),
    ("f", "f"),
    ("g", "ɡ"),
    ("h", "h"),
    ("j", "ʤ"),
    ("k", "k"),
    ("l", "l"),
    ("m", "m"),
    ("n", "n"),
    ("p", "p"),
    ("r", "ɹ"),
    ("s", "s"),
    ("t", "t"),
    ("v", "v"),
    ("w", "w"),
    ("x", "ks"),
    ("z", "z"),
];

const VOWEL_PHONEMES: &[char] = &[
    'æ', 'ɛ', 'ɪ', 'ɑ', 'ʌ', 'i', 'u', 'A', 'I', 'O', 'W', 'Y', 'ɚ', 'ɔ', 'ə',
];

fn is_vowel(letter: char) -> bool {
    matches!(letter, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// Collapse doubled consonants: "gummer" behaves like "gumer".
fn collapse(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut previous = '\0';
    for letter in word.chars() {
        if letter == previous && !is_vowel(letter) {
            continue;
        }
        out.push(letter);
        previous = letter;
    }
    out
}

/// Silent final `e` lengthens the preceding vowel: "cape" -> /keɪp/, not /kæpɛ/.
fn apply_magic_e(word: &str) -> String {
    let letters: Vec<char> = word.chars().collect();
    let len = letters.len();
    if len < 3 || letters[len - 1] != 'e' || is_vowel(letters[len - 2]) {
        return word.to_string();
    }
    let Some(vowel_at) = (0..len - 2).rev().find(|i| is_vowel(letters[*i])) else {
        return word.to_string();
    };
    let long = match letters[vowel_at] {
        'a' => 'A',
        'e' => 'i',
        'i' => 'I',
        'o' => 'O',
        'u' => 'u',
        _ => return word.to_string(),
    };
    // Mark the long vowel with its phoneme directly; the rules pass it through.
    let mut out: String = letters[..vowel_at].iter().collect();
    out.push(long);
    out.extend(&letters[vowel_at + 1..len - 1]);
    out
}

/// Guess IPA for an out-of-vocabulary word.
pub fn phonemize(word: &str) -> String {
    let lower = word.to_lowercase();
    let prepared = apply_magic_e(&collapse(&lower));

    let mut phonemes = String::new();
    let mut rest = prepared.as_str();
    'outer: while !rest.is_empty() {
        // Long-vowel markers inserted by `apply_magic_e` pass straight through.
        let head = rest.chars().next().unwrap();
        if matches!(head, 'A' | 'I' | 'O' | 'W' | 'Y') {
            phonemes.push(head);
            rest = &rest[head.len_utf8()..];
            continue;
        }
        for (grapheme, phoneme) in RULES {
            if let Some(remainder) = rest.strip_prefix(grapheme) {
                phonemes.push_str(phoneme);
                rest = remainder;
                continue 'outer;
            }
        }
        // Unknown character (a digit, an apostrophe): skip it.
        rest = &rest[head.len_utf8()..];
    }

    stress(&phonemes)
}

/// Put primary stress on the first vowel. Crude, but a stressless word sounds
/// robotic in a way listeners notice immediately.
fn stress(phonemes: &str) -> String {
    let Some(at) = phonemes.char_indices().find_map(|(index, ch)| {
        VOWEL_PHONEMES.contains(&ch).then_some(index)
    }) else {
        return phonemes.to_string();
    };
    let mut out = String::with_capacity(phonemes.len() + 2);
    out.push_str(&phonemes[..at]);
    out.push('ˈ');
    out.push_str(&phonemes[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::phonemize;
    use crate::g2p::vocab;

    #[test]
    fn invented_words_produce_valid_symbols() {
        for word in ["pacman", "gummer", "zorblax", "blorp"] {
            let ipa = phonemize(word);
            assert!(!ipa.is_empty(), "{word} produced nothing");
            for symbol in ipa.chars() {
                assert!(
                    vocab::token(symbol).is_some(),
                    "{word} -> {ipa}: symbol {symbol:?} is not in Kokoro's vocab"
                );
            }
        }
    }

    #[test]
    fn magic_e_lengthens() {
        assert!(phonemize("cape").contains('A'));
        assert!(phonemize("cap").contains('æ'));
    }
}
