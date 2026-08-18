//! Suffix morphology, because the lexicon stores lemmas.
//!
//! `game` is in misaki's dictionary; `games` is not. Without this, every plural
//! and past tense falls through to letter-to-sound and comes out wrong —
//! "games" as `ɡˈæmɛs` rather than `ɡˈAmz`. misaki does the same thing at
//! runtime; the rules are the regular English ones.

use super::lexicon;

const SIBILANTS: [char; 6] = ['s', 'z', 'ʃ', 'ʒ', 'ʧ', 'ʤ'];
const VOICELESS: [char; 5] = ['p', 't', 'k', 'f', 'θ'];

/// Last sound of a pronunciation, ignoring stress and length marks.
fn final_sound(ipa: &str) -> Option<char> {
    ipa.chars().rev().find(|c| !matches!(c, 'ˈ' | 'ˌ' | 'ː'))
}

/// `-s`: /ɪz/ after a sibilant, /s/ after a voiceless sound, /z/ otherwise.
fn plural_suffix(stem: &str) -> &'static str {
    match final_sound(stem) {
        Some(sound) if SIBILANTS.contains(&sound) => "ɪz",
        Some(sound) if VOICELESS.contains(&sound) => "s",
        _ => "z",
    }
}

/// `-ed`: /ɪd/ after /t/ or /d/, /t/ after a voiceless sound, /d/ otherwise.
fn past_suffix(stem: &str) -> &'static str {
    match final_sound(stem) {
        Some('t') | Some('d') => "ɪd",
        Some(sound) if VOICELESS.contains(&sound) => "t",
        _ => "d",
    }
}

/// Undo a doubled final consonant: "stopped" -> "stop", "running" -> "run".
fn undouble(stem: &str) -> Option<String> {
    let letters: Vec<char> = stem.chars().collect();
    let len = letters.len();
    if len >= 3 && letters[len - 1] == letters[len - 2] && !"aeiou".contains(letters[len - 1]) {
        Some(letters[..len - 1].iter().collect())
    } else {
        None
    }
}

fn first_hit(candidates: &[String]) -> Option<&'static str> {
    candidates.iter().find_map(|stem| lexicon::lookup(stem))
}

/// Pronounce an inflected form by finding its lemma and appending the suffix.
pub fn inflected(word: &str) -> Option<String> {
    let lower = word.to_lowercase();

    // Possessive and contraction: "player's" behaves like a plural.
    if let Some(stem) = lower.strip_suffix("'s").or_else(|| lower.strip_suffix('\'')) {
        let base = lexicon::lookup(stem)?;
        return Some(format!("{base}{}", plural_suffix(base)));
    }

    if let Some(stem) = lower.strip_suffix("ing") {
        let mut candidates = vec![stem.to_string(), format!("{stem}e")];
        candidates.extend(undouble(stem));
        if let Some(base) = first_hit(&candidates) {
            return Some(format!("{base}ɪŋ"));
        }
    }

    if let Some(stem) = lower.strip_suffix("ed") {
        // "carried" -> "carry"
        let mut candidates = vec![format!("{stem}e"), stem.to_string()];
        if let Some(without_i) = stem.strip_suffix('i') {
            candidates.push(format!("{without_i}y"));
        }
        candidates.extend(undouble(stem));
        if let Some(base) = first_hit(&candidates) {
            return Some(format!("{base}{}", past_suffix(base)));
        }
    }

    if let Some(stem) = lower.strip_suffix('s') {
        // "boxes" -> "box"; "flies" -> "fly"; "games" -> "game"
        let mut candidates = vec![stem.to_string()];
        if let Some(without_e) = stem.strip_suffix('e') {
            candidates.push(without_e.to_string());
            if let Some(without_i) = without_e.strip_suffix('i') {
                candidates.push(format!("{without_i}y"));
            }
        }
        if let Some(base) = first_hit(&candidates) {
            return Some(format!("{base}{}", plural_suffix(base)));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::inflected;

    #[test]
    fn plurals_take_the_right_sibilant() {
        // game -> ɡˈAm, voiced final: /z/
        assert_eq!(inflected("games").as_deref(), Some("ɡˈAmz"));
        // point -> pˈYnt, voiceless final: /s/
        assert_eq!(inflected("points").as_deref(), Some("pˈYnts"));
        // box -> bˈɑks, sibilant final: /ɪz/
        assert!(inflected("boxes").unwrap().ends_with("ɪz"));
    }

    #[test]
    fn past_tense_agrees_in_voicing() {
        // score -> skˈɔɹ, voiced: /d/
        assert_eq!(inflected("scored").as_deref(), Some("skˈɔɹd"));
        // jump -> ʤˈʌmp, voiceless: /t/
        assert_eq!(inflected("jumped").as_deref(), Some("ʤˈʌmpt"));
        // want -> ends /t/: /ɪd/
        assert!(inflected("wanted").unwrap().ends_with("ɪd"));
    }

    #[test]
    fn progressive_handles_dropped_e_and_doubling() {
        assert!(inflected("making").unwrap().ends_with("ɪŋ"));
        assert!(inflected("stopping").unwrap().ends_with("ɪŋ"));
        assert!(inflected("jumping").unwrap().ends_with("ɪŋ"));
    }

    #[test]
    fn non_inflected_words_are_left_alone() {
        assert_eq!(inflected("gummer"), None);
    }
}
