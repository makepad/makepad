//! Grapheme to phoneme: English text in, Kokoro phoneme tokens out.
//!
//! Kokoro's ONNX graph takes `input_ids`, not text, so this stage is a hard
//! prerequisite rather than a convenience. Upstream uses `misaki`, which falls
//! back to espeak-ng — a C dependency with its own licence. Instead we embed
//! misaki's own 90k-entry IPA lexicon (Apache-2.0, already in Kokoro's symbol
//! set) and hand-write letter-to-sound rules for the rest.

pub mod lexicon;
pub mod lts;
pub mod morph;
pub mod vocab;

/// Pronounce one word: the lexicon, then regular morphology, then guesswork.
pub fn pronounce(word: &str) -> String {
    if let Some(ipa) = lexicon::lookup(word) {
        return ipa.to_string();
    }
    if let Some(ipa) = morph::inflected(word) {
        return ipa;
    }
    lts::phonemize(word)
}

/// Kokoro accepts at most 510 phoneme tokens, plus a zero at each end.
pub const MAX_TOKENS: usize = 510;

enum Token {
    Word(String),
    Punct(char),
}

const ONES: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// Spell a number so the lexicon can pronounce it. Beyond a million, read the
/// digits — nobody says "four hundred billion" about a game score.
pub fn spell_number(value: u64) -> String {
    match value {
        0..=19 => ONES[value as usize].to_string(),
        20..=99 => {
            let (tens, ones) = (value / 10, value % 10);
            if ones == 0 {
                TENS[tens as usize].to_string()
            } else {
                format!("{} {}", TENS[tens as usize], ONES[ones as usize])
            }
        }
        100..=999 => {
            let (hundreds, rest) = (value / 100, value % 100);
            if rest == 0 {
                format!("{} hundred", ONES[hundreds as usize])
            } else {
                format!("{} hundred {}", ONES[hundreds as usize], spell_number(rest))
            }
        }
        1_000..=999_999 => {
            let (thousands, rest) = (value / 1000, value % 1000);
            if rest == 0 {
                format!("{} thousand", spell_number(thousands))
            } else {
                format!("{} thousand {}", spell_number(thousands), spell_number(rest))
            }
        }
        _ => value
            .to_string()
            .chars()
            .map(|digit| ONES[digit.to_digit(10).unwrap_or(0) as usize])
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut digits = String::new();

    let flush_word = |word: &mut String, tokens: &mut Vec<Token>| {
        if !word.is_empty() {
            tokens.push(Token::Word(std::mem::take(word)));
        }
    };
    let flush_digits = |digits: &mut String, tokens: &mut Vec<Token>| {
        if digits.is_empty() {
            return;
        }
        let spelled = digits
            .parse::<u64>()
            .map(spell_number)
            .unwrap_or_else(|_| std::mem::take(digits));
        digits.clear();
        for part in spelled.split_whitespace() {
            tokens.push(Token::Word(part.to_string()));
        }
    };

    for ch in text.chars() {
        if ch.is_ascii_digit() {
            flush_word(&mut word, &mut tokens);
            digits.push(ch);
            continue;
        }
        flush_digits(&mut digits, &mut tokens);

        if ch.is_alphabetic() || ch == '\'' {
            word.push(ch);
            continue;
        }
        flush_word(&mut word, &mut tokens);

        // Keep only punctuation the model actually has a token for.
        if vocab::token(ch).is_some() && ch != ' ' {
            tokens.push(Token::Punct(ch));
        }
    }
    flush_digits(&mut digits, &mut tokens);
    flush_word(&mut word, &mut tokens);
    tokens
}

const VOWEL_PHONEMES: &str = "aæɑɐɒɔəɚɛɜeiɪɨoʊuʌyøœɯɤAIOWYᵊᵻ";

fn is_vowel_phoneme(sound: char) -> bool {
    VOWEL_PHONEMES.contains(sound)
}

fn first_sound(ipa: &str) -> Option<char> {
    ipa.chars().find(|c| !matches!(c, 'ˈ' | 'ˌ' | 'ː'))
}

/// A few function words are pronounced by context, not by dictionary.
///
/// The gold lexicon stores the citation forms — `a` as `A` (the letter `eɪ`) and
/// `the` as `ði` — because misaki overrides them at runtime. Left alone, "a
/// squishy blob" comes out as "eɪ squishy blob".
fn function_word(word: &str, next_sound: Option<char>) -> Option<&'static str> {
    let followed_by_vowel = next_sound.is_some_and(is_vowel_phoneme);
    match word {
        "a" => Some("ɐ"),
        "the" => Some(if followed_by_vowel { "ði" } else { "ðə" }),
        "to" => Some(if followed_by_vowel { "tu" } else { "tə" }),
        _ => None,
    }
}

/// English text to a Kokoro phoneme string.
pub fn phonemize(text: &str) -> String {
    let tokens = tokenize(text);

    let mut sounds: Vec<Option<String>> = tokens
        .iter()
        .map(|token| match token {
            Token::Word(word) => Some(pronounce(word)),
            Token::Punct(_) => None,
        })
        .collect();

    // Second pass: some words depend on the sound that follows them.
    for index in 0..tokens.len() {
        let Token::Word(word) = &tokens[index] else {
            continue;
        };
        let next_sound = sounds[index + 1..]
            .iter()
            .flatten()
            .next()
            .and_then(|ipa| first_sound(ipa));
        if let Some(replacement) = function_word(&word.to_lowercase(), next_sound) {
            sounds[index] = Some(replacement.to_string());
        }
    }

    let mut out = String::new();
    for (token, sound) in tokens.iter().zip(&sounds) {
        match (token, sound) {
            (Token::Word(_), Some(ipa)) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(ipa);
            }
            (Token::Punct(ch), _) => out.push(*ch),
            _ => {}
        }
    }
    out
}

/// Phoneme token ids, zero-padded at both ends the way Kokoro expects.
pub fn tokens(text: &str) -> Vec<u16> {
    let phonemes = phonemize(text);
    let mut ids = Vec::with_capacity(phonemes.len() + 2);
    ids.push(0);
    for symbol in phonemes.chars() {
        if ids.len() > MAX_TOKENS {
            break;
        }
        if let Some(id) = vocab::token(symbol) {
            ids.push(id);
        }
    }
    ids.push(0);
    ids
}

#[cfg(test)]
mod tests {
    use super::{phonemize, tokens, vocab, MAX_TOKENS};

    #[test]
    fn every_emitted_symbol_is_in_vocab() {
        let text = "Hi! I make games with you. Escape the Gummer, 42 times?";
        for symbol in phonemize(text).chars() {
            assert!(vocab::token(symbol).is_some(), "stray symbol {symbol:?}");
        }
    }

    #[test]
    fn tokens_are_padded_and_bounded() {
        let ids = tokens("hello");
        assert_eq!(ids.first(), Some(&0));
        assert_eq!(ids.last(), Some(&0));

        let long = "hello ".repeat(500);
        assert!(tokens(&long).len() <= MAX_TOKENS + 2);
    }

    #[test]
    fn numbers_are_spoken_not_spelled() {
        assert!(phonemize("42").len() > 3);
    }

    #[test]
    fn function_words_follow_context() {
        // The article, not the letter `eɪ`.
        assert!(phonemize("a blob").starts_with('ɐ'));
        // `ðə` before a consonant, `ði` before a vowel.
        assert!(phonemize("the game").starts_with("ðə"));
        assert!(phonemize("the apple").starts_with("ði"));
    }
}
