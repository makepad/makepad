//! Mandarin Chinese to Kokoro phoneme symbols, in-tree: no Python, no models.
//!
//! Two fronts, one per Kokoro release, because the two releases were trained on
//! different symbol sets:
//!
//! - [`Variant::V10`], Kokoro v1.0 (the 8 zf_/zm_ voices): pinyin → IPA with
//!   tone arrows (`transcription.py` of misaki), words cut by the dictionary,
//!   no tone sandhi (the release did not apply any).
//! - [`Variant::V11`], Kokoro v1.1-zh (zf_001.. / zm_009..): pinyin → bopomofo
//!   plus tone digits (`zh_frontend.py` of misaki), with tone sandhi, erhua and
//!   the word merges that feed them.
//!
//! Misaki is Apache-2.0 (Hexgrad); its Mandarin front ends are adapted from
//! PaddleSpeech (Apache-2.0) and pinyin-to-ipa (MIT). The data is pinyin-data,
//! phrase-pinyin-data and jieba's dictionary (all MIT); see CREDITS.md.

pub mod number;
pub mod pinyin;
pub mod sandhi;
pub mod segment;

use super::phonemize_with_sources as english;

/// Which Kokoro release the phonemes are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    V10,
    V11,
}

/// A symbol with the byte offset (in the text) of the word it sounds.
pub type Phoneme = (char, Option<usize>);

const PUNCT: &str = ";:,.!?—…\"()“”";

/// Misaki's punctuation mapping: full-width marks to the ones the model has.
fn map_punctuation(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '、' | '，' => out.push_str(", "),
            '。' | '．' => out.push_str(". "),
            '！' => out.push_str("! "),
            '：' => out.push_str(": "),
            '；' => out.push_str("; "),
            '？' => out.push_str("? "),
            '«' | '《' | '「' | '【' => out.push_str(" “"),
            '»' | '》' | '」' | '】' => out.push_str("” "),
            '（' => out.push_str(" ("),
            '）' => out.push_str(") "),
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

fn is_han(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// Split into Latin runs (`Some`) and everything else (`None`), the way
/// misaki's `([A-Za-z \'-]*[A-Za-z][A-Za-z \'-]*)|([^A-Za-z]+)` does.
fn split_latin(text: &str) -> Vec<(bool, String)> {
    let chars: Vec<char> = text.chars().collect();
    let in_set = |c: char| c.is_ascii_alphabetic() || c == ' ' || c == '\'' || c == '-';
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut end = i;
        while end < chars.len() && in_set(chars[end]) {
            end += 1;
        }
        if chars[i..end].iter().any(|c| c.is_ascii_alphabetic()) {
            out.push((true, chars[i..end].iter().collect()));
            i = end;
        } else {
            let mut end = i;
            while end < chars.len() && !chars[end].is_ascii_alphabetic() {
                end += 1;
            }
            out.push((false, chars[i..end].iter().collect()));
            i = end;
        }
    }
    out
}

/// Chinese (and the English words in it) to phoneme symbols for `variant`.
pub fn phonemize_with_sources(text: &str, variant: Variant) -> Vec<Phoneme> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let text = map_punctuation(&number::spell_numbers(text));
    let mut out: Vec<Phoneme> = Vec::new();
    match variant {
        Variant::V11 => {
            let mut first = true;
            for (is_english, segment) in split_latin(&text) {
                let segment = segment.trim();
                if segment.is_empty() {
                    continue;
                }
                if !first {
                    out.push((' ', None));
                }
                first = false;
                if is_english {
                    out.extend(english(segment).into_iter().map(|(c, _)| (c, None)));
                } else {
                    out.extend(v11::frontend(segment).chars().map(|c| (c, Some(0))));
                }
            }
        }
        Variant::V10 => {
            for (is_english, segment) in split_latin(&text) {
                if is_english {
                    out.extend(english(segment.trim()).into_iter().map(|(c, _)| (c, None)));
                    out.push((' ', None));
                    continue;
                }
                let mut run = String::new();
                for c in segment.chars().chain(std::iter::once('\0')) {
                    if c != '\0' && is_han(c) {
                        run.push(c);
                        continue;
                    }
                    if !run.is_empty() {
                        let words: Vec<String> = segment::cut(&run).iter().map(|w| v10::word(w)).collect();
                        out.extend(words.join(" ").chars().map(|c| (c, Some(0))));
                        run.clear();
                    }
                    if c != '\0' {
                        out.push((c, if c == ' ' { None } else { Some(0) }));
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ v1.1

mod v11 {
    use super::{is_han, pinyin, sandhi, segment, PUNCT};

    fn zh_map(p: &str) -> Option<&'static str> {
        Some(match p {
            "b" => "ㄅ", "p" => "ㄆ", "m" => "ㄇ", "f" => "ㄈ", "d" => "ㄉ", "t" => "ㄊ", "n" => "ㄋ",
            "l" => "ㄌ", "g" => "ㄍ", "k" => "ㄎ", "h" => "ㄏ", "j" => "ㄐ", "q" => "ㄑ", "x" => "ㄒ",
            "zh" => "ㄓ", "ch" => "ㄔ", "sh" => "ㄕ", "r" => "ㄖ", "z" => "ㄗ", "c" => "ㄘ", "s" => "ㄙ",
            "a" => "ㄚ", "o" => "ㄛ", "e" => "ㄜ", "ie" => "ㄝ", "ai" => "ㄞ", "ei" => "ㄟ", "ao" => "ㄠ",
            "ou" => "ㄡ", "an" => "ㄢ", "en" => "ㄣ", "ang" => "ㄤ", "eng" => "ㄥ", "er" => "ㄦ",
            "i" => "ㄧ", "u" => "ㄨ", "v" => "ㄩ", "ii" => "ㄭ", "iii" => "十", "ve" => "月", "ia" => "压",
            "ian" => "言", "iang" => "阳", "iao" => "要", "in" => "阴", "ing" => "应", "iong" => "用",
            "iou" => "又", "ong" => "中", "ua" => "穵", "uai" => "外", "uan" => "万", "uang" => "王",
            "uei" => "为", "uen" => "文", "ueng" => "瓮", "uo" => "我", "van" => "元", "vn" => "云",
            "1" => "1", "2" => "2", "3" => "3", "4" => "4", "5" => "5", "R" => "R",
            _ => return None,
        })
    }

    /// One word's initials and finals (tone digit last), with `i` after
    /// z/c/s as `ii` and after zh/ch/sh/r as `iii`. `None` for a character
    /// without a reading.
    fn initials_finals(word: &str) -> Vec<Option<(String, String)>> {
        let mut out = Vec::new();
        for (c, syllable) in word.chars().zip(pinyin::syllables(word)) {
            // 嗯 is n2: its initial and final may not both be empty.
            let parts = if c == '嗯' { pinyin::split("n2").map(|mut p| {
                p.initial = "";
                p
            }) } else { syllable.and_then(pinyin::split) };
            out.push(parts.map(|p| {
                let mut final_ = format!("{}{}", p.final_, p.tone);
                if p.final_ == "i" {
                    if matches!(p.initial, "z" | "c" | "s") {
                        final_ = format!("ii{}", p.tone);
                    } else if matches!(p.initial, "zh" | "ch" | "sh" | "r") {
                        final_ = format!("iii{}", p.tone);
                    }
                }
                (p.initial.to_string(), final_)
            }));
        }
        out
    }

    struct Tok {
        tag: String,
        phonemes: String,
        whitespace: String,
    }

    pub fn frontend(text: &str) -> String {
        // Word segmentation with a tag per word; whatever is not Chinese is its own word.
        let mut seg: Vec<(String, String)> = Vec::new();
        for word in segment::cut(text) {
            let first = word.chars().next().unwrap_or(' ');
            let pos = if is_han(first) { segment::tag(&word).unwrap_or("x").to_string() } else { "x".to_string() };
            seg.push((word, pos));
        }
        let seg = sandhi::pre_merge(seg);

        let mut tokens: Vec<Tok> = Vec::new();
        for (word, mut pos) in seg {
            if pos == "x" && word.chars().all(is_han) {
                pos = "X".into();
            } else if pos != "x" && PUNCT.contains(word.as_str()) && word.chars().count() == 1 {
                pos = "x".into();
            }
            if pos == "x" || pos == "eng" {
                if !word.trim().is_empty() {
                    let phonemes = if pos == "x" && PUNCT.contains(word.as_str()) && word.chars().count() == 1 { word.clone() } else { String::new() };
                    tokens.push(Tok { tag: pos, phonemes, whitespace: String::new() });
                } else if let Some(last) = tokens.last_mut() {
                    last.whitespace.push_str(&word);
                }
                continue;
            }
            if let Some(last) = tokens.last_mut() {
                if last.tag != "x" && last.tag != "eng" && last.whitespace.is_empty() {
                    last.whitespace = "/".into();
                }
            }

            let pairs = initials_finals(&word);
            let known: Vec<bool> = pairs.iter().map(Option::is_some).collect();
            let mut initials: Vec<String> = pairs.iter().map(|p| p.as_ref().map(|p| p.0.clone()).unwrap_or_default()).collect();
            let mut finals: Vec<String> = pairs.iter().map(|p| p.as_ref().map(|p| p.1.clone()).unwrap_or_else(|| "0".into())).collect();
            sandhi::modified_tone(&word, &pos, &mut finals);
            let (initials_, finals_, known_) = merge_erhua(&mut initials, &mut finals, &known, &word, &pos);

            let mut phones: Vec<String> = Vec::new();
            for ((initial, final_), known) in initials_.iter().zip(&finals_).zip(&known_) {
                if !known {
                    continue;
                }
                if !initial.is_empty() {
                    phones.push(initial.clone());
                }
                if !final_.is_empty() && (final_.len() > 1 || !PUNCT.contains(final_.as_str()) || final_ != initial) {
                    phones.push(final_.clone());
                }
            }
            let joined = phones.join("_").replace("_eR", "_er").replace('R', "_R");
            let mut split = String::new();
            for c in joined.chars() {
                if c.is_ascii_digit() {
                    split.push('_');
                }
                split.push(c);
            }
            let phonemes: String = split.split('_').filter_map(zh_map).collect();
            tokens.push(Tok { tag: pos, phonemes, whitespace: String::new() });
        }
        tokens.iter().map(|t| format!("{}{}", t.phonemes, t.whitespace)).collect()
    }

    fn merge_erhua(
        initials: &mut Vec<String>,
        finals: &mut Vec<String>,
        known: &[bool],
        word: &str,
        pos: &str,
    ) -> (Vec<String>, Vec<String>, Vec<bool>) {
        let chars: Vec<char> = word.chars().collect();
        let n = finals.len();
        if n > 0 && chars.get(n - 1) == Some(&'儿') && finals[n - 1] == "er1" {
            finals[n - 1] = "er2".into();
        }
        let keep = |initials: &Vec<String>, finals: &Vec<String>| (initials.clone(), finals.clone(), known.to_vec());
        if !sandhi::must_erhua().contains(word) && (sandhi::not_erhua().contains(word) || matches!(pos, "a" | "j" | "nr")) {
            return keep(initials, finals);
        }
        if finals.len() != chars.len() {
            return keep(initials, finals);
        }
        let tail: String = chars[chars.len().saturating_sub(2)..].iter().collect();
        let (mut new_initials, mut new_finals, mut new_known) = (Vec::new(), Vec::new(), Vec::new());
        for (i, phn) in finals.iter().enumerate() {
            if i == finals.len() - 1
                && chars[i] == '儿'
                && (phn == "er2" || phn == "er5")
                && !sandhi::not_erhua().contains(tail.as_str())
                && !new_finals.is_empty()
            {
                let last: &mut String = new_finals.last_mut().unwrap();
                let tone = last.pop().unwrap_or('5');
                last.push('R');
                last.push(tone);
            } else {
                new_initials.push(initials[i].clone());
                new_finals.push(phn.clone());
                new_known.push(known[i]);
            }
        }
        (new_initials, new_finals, new_known)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn ni_hao_in_bopomofo() {
            // ni3 hao3 is one word: the first third tone rises.
            assert_eq!(frontend("你好"), "ㄋㄧ2ㄏㄠ3");
        }

        #[test]
        fn words_are_joined_with_a_slash_and_punctuation_kept() {
            assert_eq!(frontend("我爱你"), "我3/ㄞ4/ㄋㄧ3");
            assert_eq!(frontend("你好, 世界!"), "ㄋㄧ2ㄏㄠ3, ㄕ十4ㄐㄝ4!");
        }
    }
}

// ------------------------------------------------------------------ v1.0

mod v10 {
    use super::pinyin;

    fn initial(i: &str) -> &'static str {
        match i {
            "b" => "p", "c" => "ʦʰ", "ch" => "\u{ab67}ʰ", "d" => "t", "f" => "f", "g" => "k", "h" => "x",
            "j" => "ʨ", "k" => "kʰ", "l" => "l", "m" => "m", "n" => "n", "p" => "pʰ", "q" => "ʨʰ", "r" => "ɻ",
            "s" => "s", "sh" => "ʂ", "t" => "tʰ", "x" => "ɕ", "z" => "ʦ", "zh" => "\u{ab67}",
            _ => "",
        }
    }

    /// The final with `0` where the tone goes (the combining marks misaki
    /// writes under diphthongs are dropped: the model has no symbol for them).
    fn final_(initial: &str, f: &str) -> Option<&'static str> {
        Some(match f {
            "a" => "a0", "ai" => "ai0", "an" => "a0n", "ang" => "a0ŋ", "ao" => "au0", "e" => "ɤ0",
            "ei" => "ei0", "en" => "ə0n", "eng" => "ə0ŋ",
            "i" => {
                if matches!(initial, "zh" | "ch" | "sh" | "r" | "z" | "c" | "s") { "ɨ0" } else { "i0" }
            }
            "ia" => "ja0", "ian" => "jɛ0n", "iang" => "ja0ŋ", "iao" => "jau0", "ie" => "je0", "in" => "i0n",
            "iou" => "jou0", "ing" => "i0ŋ", "iong" => "jʊ0ŋ", "ong" => "ʊ0ŋ", "ou" => "ou0", "u" => "u0",
            "uei" => "wei0", "ua" => "wa0", "uai" => "wai0", "uan" => "wa0n", "uen" => "wə0n",
            "uang" => "wa0ŋ", "ueng" => "wə0ŋ", "uo" => "wo0", "o" => "wo0", "v" => "y0", "ve" => "ɥe0",
            "van" => "ɥɛ0n", "vn" => "y0n",
            _ => return None,
        })
    }

    fn tone(t: u8) -> &'static str {
        match t {
            1 => "→",
            2 => "↗",
            3 => "↓",
            4 => "↘",
            _ => "",
        }
    }

    fn syllable(s: Option<&'static str>) -> String {
        let Some(parts) = s.and_then(pinyin::split) else { return String::new() };
        let marks = tone(parts.tone);
        let body = match parts.final_.as_str() {
            "er" => "ɚ0".to_string(),
            "o" if parts.initial.is_empty() => "ɔ0".to_string(),
            "ê" => "ɛ0".to_string(),
            "hm" => "hm0".to_string(),
            "hng" => "hŋ0".to_string(),
            "m" => "m0".to_string(),
            "n" => "n0".to_string(),
            "ng" => "ŋ0".to_string(),
            f => match final_(parts.initial, f) {
                Some(f) => format!("{}{}", initial(parts.initial), f),
                None => return String::new(),
            },
        };
        body.replace('0', marks)
    }

    /// One word as phonemes: its syllables run together.
    pub fn word(word: &str) -> String {
        pinyin::syllables(word).into_iter().map(syllable).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn syllables_in_ipa() {
            assert_eq!(word("你好"), "ni↓xau↓");
            assert_eq!(word("中国"), "\u{ab67}ʊ→ŋkwo↗");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str, v: Variant) -> String {
        phonemize_with_sources(s, v).iter().map(|p| p.0).collect()
    }

    #[test]
    fn v11_applies_sandhi_numbers_and_erhua() {
        assert_eq!(text("你好，世界！", Variant::V11), "ㄋㄧ2ㄏㄠ3, ㄕ十4ㄐㄝ4!");
        // 一 before a fourth tone and before a first tone; 不 before a fourth tone.
        assert_eq!(text("一共一天不怕", Variant::V11), "ㄧ2ㄍ中4/ㄧ4ㄊ言1ㄅㄨ2ㄆㄚ4");
        // 100 -> 一百, 3.5 -> 三点五; the particle 了 is neutral.
        assert_eq!(text("100个", Variant::V11), "ㄧ4ㄅㄞ3ㄍㄜ4");
        assert_eq!(text("花儿开了", Variant::V11), "ㄏ穵1ㄦ2/ㄎㄞ1/ㄌㄜ5");
        assert_eq!(text("小孩儿", Variant::V11), "ㄒ要3ㄏㄞR2");
        // Polyphones follow the phrase.
        assert_eq!(text("银行行长", Variant::V11), "阴2ㄏㄤ2/ㄏㄤ2ㄓㄤ3");
    }

    #[test]
    fn v10_writes_ipa_with_tone_arrows_and_no_sandhi() {
        assert_eq!(text("你好，世界！", Variant::V10), "ni↓xau↓, ʂɨ↘ʨje↘!");
        assert_eq!(text("不怕", Variant::V10), "pu↘pʰa↘");
    }

    #[test]
    fn every_symbol_is_in_its_models_vocab() {
        use crate::g2p::{vocab, vocab_v11};
        let sample = "今天天气很好，我们去公园玩。他有100个朋友，一共花了3.5元。Kokoro 是一系列模型。花儿开了！";
        for c in text(sample, Variant::V11).chars() {
            assert!(vocab_v11::token(c).is_some(), "v1.1 stray {c:?}");
        }
        for c in text(sample, Variant::V10).chars() {
            assert!(vocab::token(c).is_some(), "v1.0 stray {c:?}");
        }
    }

    #[test]
    fn latin_runs_split_the_way_misaki_does() {
        let parts = split_latin("Kokoro 是 TTS 模型");
        let flags: Vec<bool> = parts.iter().map(|p| p.0).collect();
        assert_eq!(flags, vec![true, false, true, false]);
    }
}
