//! Characters and phrases to pinyin, and a pinyin syllable to its parts.
//!
//! The data is pinyin-data's usual reading of each character and
//! phrase-pinyin-data's readings of polyphonic words (both MIT), packed by
//! `tools/build_zh_lexicon.py`. A word is read the way pypinyin reads one:
//! longest known phrase first, then each character's usual reading.

use super::super::data;

const MAX_PHRASE_CHARS: usize = 12;

/// One syllable per character of `word`, as `hang2` (tone digit last, ü as v,
/// neutral 5); `None` for a character with no known reading.
pub fn syllables(word: &str) -> Vec<Option<&'static str>> {
    let chars: Vec<char> = word.chars().collect();
    let mut out: Vec<Option<&'static str>> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let mut matched = false;
        let longest = (i + MAX_PHRASE_CHARS).min(chars.len());
        for j in (i + 2..=longest).rev() {
            let phrase: String = chars[i..j].iter().collect();
            if let Some(reading) = data::pinyin().get(&phrase) {
                let parts: Vec<&'static str> = reading.split(' ').collect();
                if parts.len() == j - i {
                    out.extend(parts.into_iter().map(Some));
                    i = j;
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        out.push(data::pinyin().get(&chars[i].to_string()));
        i += 1;
    }
    out
}

/// A syllable split the way pypinyin's strict initials and finals do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parts {
    /// `b`, `zh`, ...; empty when the syllable has none (`y`/`w` do not count).
    pub initial: &'static str,
    /// The final with the spellings pypinyin's strict mode gives: `iou`, `uei`,
    /// `uen`, `v`, `ve`, `van`, `vn`, `ia`, `uo`, ... or the syllable itself
    /// for a syllabic consonant (`m`, `n`, `ng`, `hm`, `hng`).
    pub final_: String,
    /// 1..=5, 5 neutral.
    pub tone: u8,
}

const INITIALS: [&str; 21] = [
    "zh", "ch", "sh", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r", "z", "c", "s",
];

pub fn split(syllable: &'static str) -> Option<Parts> {
    let tone = syllable.chars().last()?.to_digit(10)? as u8;
    let base = &syllable[..syllable.len() - 1];
    if matches!(base, "m" | "n" | "ng" | "hm" | "hng" | "ê" | "er" | "o") {
        return Some(Parts { initial: "", final_: base.to_string(), tone });
    }
    if let Some(rest) = base.strip_prefix('y') {
        let final_ = if rest.starts_with('i') {
            rest.to_string()
        } else if let Some(after) = rest.strip_prefix('u') {
            format!("v{after}")
        } else {
            format!("i{rest}")
        };
        return Some(Parts { initial: "", final_, tone });
    }
    if let Some(rest) = base.strip_prefix('w') {
        let final_ = if rest.starts_with('u') { rest.to_string() } else { format!("u{rest}") };
        return Some(Parts { initial: "", final_, tone });
    }
    let initial = INITIALS.iter().find(|i| base.starts_with(**i)).copied().unwrap_or("");
    let rest = &base[initial.len()..];
    if rest.is_empty() {
        return None;
    }
    let final_ = match (initial, rest) {
        ("j" | "q" | "x", r) if r.starts_with('u') => format!("v{}", &r[1..]),
        (_, "iu") => "iou".to_string(),
        (_, "ui") => "uei".to_string(),
        (_, "un") => "uen".to_string(),
        (_, r) => r.to_string(),
    };
    Some(Parts { initial, final_, tone })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(word: &str) -> Vec<&'static str> {
        syllables(word).into_iter().map(|s| s.unwrap_or("?")).collect()
    }

    #[test]
    fn characters_and_polyphones() {
        assert_eq!(read("你好"), vec!["ni3", "hao3"]);
        assert_eq!(read("银行"), vec!["yin2", "hang2"]);
        assert_eq!(read("行走"), vec!["xing2", "zou3"]);
        assert_eq!(read("重庆"), vec!["chong2", "qing4"]);
        assert_eq!(read("地"), vec!["de5"]);
        assert_eq!(read("行号"), vec!["hang2", "hao4"]);
    }

    fn parts(s: &'static str) -> (&'static str, String, u8) {
        let p = split(s).unwrap();
        (p.initial, p.final_, p.tone)
    }

    #[test]
    fn syllables_split_like_pypinyin_strict() {
        assert_eq!(parts("hang2"), ("h", "ang".into(), 2));
        assert_eq!(parts("zhi1"), ("zh", "i".into(), 1));
        assert_eq!(parts("liu2"), ("l", "iou".into(), 2));
        assert_eq!(parts("gui4"), ("g", "uei".into(), 4));
        assert_eq!(parts("lun2"), ("l", "uen".into(), 2));
        assert_eq!(parts("ju3"), ("j", "v".into(), 3));
        assert_eq!(parts("xue2"), ("x", "ve".into(), 2));
        assert_eq!(parts("nv3"), ("n", "v".into(), 3));
        assert_eq!(parts("yi1"), ("", "i".into(), 1));
        assert_eq!(parts("ya1"), ("", "ia".into(), 1));
        assert_eq!(parts("you3"), ("", "iou".into(), 3));
        assert_eq!(parts("yuan2"), ("", "van".into(), 2));
        assert_eq!(parts("wu3"), ("", "u".into(), 3));
        assert_eq!(parts("wo3"), ("", "uo".into(), 3));
        assert_eq!(parts("wen2"), ("", "uen".into(), 2));
        assert_eq!(parts("er2"), ("", "er".into(), 2));
        assert_eq!(parts("bo1"), ("b", "o".into(), 1));
    }
}
