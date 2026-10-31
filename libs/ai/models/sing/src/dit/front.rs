//! The DiT voice's front end: a scored line (notes, the words they sing) ->
//! word-level notes (a word's first note carries its whole ARPAbet spelling,
//! its other notes are slurs, gaps are rests) -> note tokens and the frame ->
//! token map the encoder expands to 50 frames per second.
//!
//! The token layout follows the reference implementation's data processor
//! exactly (the model was trained on it): per note `<BOW>`, the note's
//! phones (an English word's phones then `<SEP>`), `<EOW>`; within a note the
//! frames cycle through its phones, the first frame is `<BOW>` and the last
//! `<EOW>`.

use std::collections::HashMap;

/// Output frames per second (24 kHz, hop 480).
pub const FPS: f64 = 50.0;

/// Note kinds the model knows.
pub const REST: u32 = 1;
pub const LYRIC: u32 = 2;
pub const SLUR: u32 = 3;

/// One word-level note: its length in seconds, `<SP>` (a rest) or an
/// English word's ARPAbet phones joined by `-` behind `en_`, its MIDI pitch
/// (0 for a rest) and its kind.
#[derive(Clone, Debug, PartialEq)]
pub struct DitNote {
    pub dur: f64,
    pub phoneme: String,
    pub pitch: u32,
    pub kind: u32,
}

/// The encoder's input for one line: per token its phone id, pitch and kind,
/// and per output frame the token it reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tokens {
    pub phoneme: Vec<u32>,
    pub pitch: Vec<u32>,
    pub kind: Vec<u32>,
    pub mel2note: Vec<u32>,
}

impl Tokens {
    pub fn frames(&self) -> usize {
        self.mel2note.len()
    }
}

/// A note of a score with the index of the word it sings (seconds from the
/// line start; the start is where the vowel sounds).
#[derive(Clone, Copy, Debug)]
pub struct ScoreNote {
    pub start: f32,
    pub dur: f32,
    pub midi: f32,
    pub word: usize,
}

const VOWELS: [&str; 15] = ["AA", "AE", "AH", "AO", "AW", "AY", "EH", "ER", "EY", "IH", "IY", "OW", "OY", "UH", "UW"];

fn is_vowel(p: &str) -> bool {
    p.len() >= 2 && VOWELS.contains(&&p[..2])
}

/// A pronunciation in the TTS IPA notation (the speech G2P's: stress marks
/// before the stressed vowel, capital-letter diphthongs) as ARPAbet phones
/// with stress digits. `spelling` breaks the flap's tie (`ɾ` is D where the
/// word is spelt with a d and no t, else T).
pub fn arpa_from_ipa(pron: &str, spelling: &str) -> Vec<String> {
    let chars: Vec<char> = pron.chars().collect();
    let flap = if spelling.contains('d') && !spelling.contains('t') { "D" } else { "T" };
    let mut out: Vec<String> = Vec::new();
    let mut stress: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let vowel = |base: &str, stress: &mut Option<char>, out: &mut Vec<String>| {
            let d = match stress.take() {
                Some('ˈ') => '1',
                Some('ˌ') => '2',
                _ => '0',
            };
            out.push(format!("{base}{d}"));
        };
        match c {
            'ˈ' | 'ˌ' => stress = Some(c),
            'ː' | '.' | '-' | ' ' => {}
            'A' => vowel("EY", &mut stress, &mut out),
            'I' => vowel("AY", &mut stress, &mut out),
            'O' | 'Q' => vowel("OW", &mut stress, &mut out),
            'W' => vowel("AW", &mut stress, &mut out),
            'Y' => vowel("OY", &mut stress, &mut out),
            'æ' => vowel("AE", &mut stress, &mut out),
            'ɑ' | 'ɒ' | 'a' => vowel("AA", &mut stress, &mut out),
            'ɔ' => vowel("AO", &mut stress, &mut out),
            'ɛ' | 'e' => vowel("EH", &mut stress, &mut out),
            'ɪ' => vowel("IH", &mut stress, &mut out),
            'i' => vowel("IY", &mut stress, &mut out),
            'u' => vowel("UW", &mut stress, &mut out),
            'ʊ' => vowel("UH", &mut stress, &mut out),
            'o' => vowel("OW", &mut stress, &mut out),
            'ʌ' => vowel("AH", &mut stress, &mut out),
            'ɜ' | 'ɝ' | 'ɚ' => {
                vowel("ER", &mut stress, &mut out);
                if next == Some('ɹ') {
                    i += 1;
                }
            }
            'ə' => {
                if next == Some('ɹ') {
                    vowel("ER", &mut stress, &mut out);
                    i += 1;
                } else {
                    vowel("AH", &mut stress, &mut out);
                }
            }
            'ᵊ' | 'ɐ' => vowel("AH", &mut stress, &mut out),
            'ᵻ' | 'ɨ' => vowel("IH", &mut stress, &mut out),
            _ => {
                let p = match c {
                    'b' => "B",
                    'd' => "D",
                    'f' => "F",
                    'h' | 'x' | 'ç' => "HH",
                    'j' | 'y' => "Y",
                    'k' | 'c' => "K",
                    'l' | 'ɫ' => "L",
                    'm' => "M",
                    'n' => "N",
                    'p' => "P",
                    's' => "S",
                    't' | 'ʔ' => "T",
                    'v' => "V",
                    'w' => "W",
                    'z' => "Z",
                    'ð' => "DH",
                    'ŋ' => "NG",
                    'ɡ' | 'g' => "G",
                    'ɹ' | 'r' | 'ɻ' => "R",
                    'ʃ' => "SH",
                    'ʒ' => "ZH",
                    'θ' => "TH",
                    'ʤ' => "JH",
                    'ʧ' => "CH",
                    'ɾ' | 'T' => flap,
                    _ => "",
                };
                if !p.is_empty() {
                    out.push(p.to_string());
                }
            }
        }
        i += 1;
    }
    // The dictionary writes a vowel before r as IY R (here, near).
    for i in 0..out.len().saturating_sub(1) {
        if out[i].starts_with("IH") && out[i + 1] == "R" && !out.get(i + 2).map(|p| is_vowel(p)).unwrap_or(false) {
            let d = out[i].chars().last().unwrap();
            out[i] = format!("IY{d}");
        }
    }
    // A weak be-/re-/de- prefix is IH0 in the dictionary (believe, remember).
    let sp = spelling.as_bytes();
    if sp.len() > 3 && matches!(&sp[..2], b"be" | b"re" | b"de") && out.len() > 2 && out[1] == "AH0" && !is_vowel(&out[0]) {
        out[1] = "IH0".into();
    }
    // Secondary stress alone is the word's primary stress.
    if !out.iter().any(|p| p.ends_with('1')) {
        if let Some(p) = out.iter_mut().find(|p| p.ends_with('2')) {
            p.pop();
            p.push('1');
        }
    }
    // A word with no stress mark (the lexicon leaves monosyllables bare):
    // its full vowel takes primary stress, a schwa stays unstressed.
    if !out.iter().any(|p| p.ends_with('1') || p.ends_with('2')) {
        let vowels: Vec<usize> = out.iter().enumerate().filter(|(_, p)| is_vowel(p)).map(|(i, _)| i).collect();
        if vowels.len() == 1 {
            let v = &mut out[vowels[0]];
            let full = !(v.starts_with("AH") && pron.contains('ə'));
            if full {
                v.pop();
                v.push('1');
            }
        }
    }
    if out.is_empty() {
        out.push("AH0".into());
    }
    out
}

/// Function words in the model's training spelling (the dictionary's first
/// form: weak vowels where it has them), which the TTS lexicon writes
/// differently (`the` before a vowel, `and` with a full vowel).
const FUNCTION_WORDS: [(&str, &str); 88] = [
    ("the", "DH AH0"),
    ("a", "AH0"),
    ("an", "AE1 N"),
    ("and", "AH0 N D"),
    ("of", "AH1 V"),
    ("to", "T UW1"),
    ("for", "F AO1 R"),
    ("in", "IH0 N"),
    ("on", "AA1 N"),
    ("at", "AE1 T"),
    ("with", "W IH1 DH"),
    ("from", "F R AH1 M"),
    ("by", "B AY1"),
    ("as", "AE1 Z"),
    ("is", "IH1 Z"),
    ("are", "AA1 R"),
    ("was", "W AA1 Z"),
    ("were", "W ER0"),
    ("be", "B IY1"),
    ("been", "B IH1 N"),
    ("i", "AY1"),
    ("you", "Y UW1"),
    ("he", "HH IY1"),
    ("she", "SH IY1"),
    ("it", "IH1 T"),
    ("we", "W IY1"),
    ("they", "DH EY1"),
    ("me", "M IY1"),
    ("my", "M AY1"),
    ("your", "Y AO1 R"),
    ("our", "AW1 ER0"),
    ("their", "DH EH1 R"),
    ("his", "HH IH1 Z"),
    ("her", "HH ER0"),
    ("its", "IH1 T S"),
    ("this", "DH IH1 S"),
    ("that", "DH AE1 T"),
    ("these", "DH IY1 Z"),
    ("those", "DH OW1 Z"),
    ("but", "B AH1 T"),
    ("or", "AO1 R"),
    ("so", "S OW1"),
    ("if", "IH1 F"),
    ("not", "N AA1 T"),
    ("no", "N OW1"),
    ("do", "D UW1"),
    ("does", "D AH1 Z"),
    ("did", "D IH1 D"),
    ("have", "HH AE1 V"),
    ("has", "HH AE1 Z"),
    ("had", "HH AE1 D"),
    ("will", "W IH1 L"),
    ("would", "W UH1 D"),
    ("can", "K AE1 N"),
    ("could", "K UH1 D"),
    ("should", "SH UH1 D"),
    ("may", "M EY1"),
    ("might", "M AY1 T"),
    ("must", "M AH1 S T"),
    ("am", "AE1 M"),
    ("all", "AO1 L"),
    ("up", "AH1 P"),
    ("down", "D AW1 N"),
    ("out", "AW1 T"),
    ("over", "OW1 V ER0"),
    ("into", "IH0 N T UW1"),
    ("upon", "AH0 P AA1 N"),
    ("through", "TH R UW1"),
    ("across", "AH0 K R AO1 S"),
    ("around", "ER0 AW1 N D"),
    ("before", "B IH0 F AO1 R"),
    ("after", "AE1 F T ER0"),
    ("under", "AH1 N D ER0"),
    ("above", "AH0 B AH1 V"),
    ("between", "B IH0 T W IY1 N"),
    ("how", "HH AW1"),
    ("what", "W AH1 T"),
    ("when", "W EH1 N"),
    ("where", "W EH1 R"),
    ("why", "W AY1"),
    ("who", "HH UW1"),
    ("there", "DH EH1 R"),
    ("here", "HH IY1 R"),
    ("then", "DH EH1 N"),
    ("than", "DH AE1 N"),
    ("just", "JH AH1 S T"),
    ("like", "L AY1 K"),
    ("oh", "OW1"),
];

/// A word's ARPAbet phones: the function word table, else its TTS IPA
/// pronunciation (`pron`) through [`arpa_from_ipa`].
pub fn arpa_word(word: &str, pron: &str) -> Vec<String> {
    let w = word.to_lowercase();
    if let Some((_, p)) = FUNCTION_WORDS.iter().find(|(k, _)| *k == w) {
        return p.split(' ').map(String::from).collect();
    }
    arpa_from_ipa(pron, &w)
}

/// Onset consonants before a word's first vowel.
fn onset_len(phones: &[String]) -> usize {
    phones.iter().take_while(|p| !is_vowel(p)).count()
}

/// A line's notes as word-level notes: one per score note; the first note of
/// a word (a lyric note) carries the word's phones, its other notes are
/// slurs; gaps are rests. A word's first note starts `lead` seconds per
/// onset consonant before its vowel, into the gap before it. Durations are
/// rounded to 0.1 ms. `end` is where the line's audio ends.
pub fn word_notes(notes: &[ScoreNote], words: &[Vec<String>], end: f64, lead: f64) -> Vec<DitNote> {
    let mut segs: Vec<(f64, f64, String, u32, u32)> = Vec::new();
    let mut prev_end = 0.0f64;
    let mut prev_word: Option<usize> = None;
    for n in notes {
        let (s, e) = (n.start as f64, n.start as f64 + n.dur as f64);
        let ph = &words[n.word];
        let (s2, kind) = if prev_word != Some(n.word) {
            ((s - lead * onset_len(ph) as f64).max(prev_end), LYRIC)
        } else {
            (s.max(prev_end), SLUR)
        };
        if s2 - prev_end > 1e-3 {
            segs.push((prev_end, s2, "<SP>".into(), 0, REST));
        } else if let Some(last) = segs.last_mut() {
            last.1 = s2;
        }
        segs.push((s2, e, format!("en_{}", ph.join("-")), n.midi.round().clamp(0.0, 255.0) as u32, kind));
        prev_end = e;
        prev_word = Some(n.word);
    }
    if end - prev_end > 1e-3 {
        segs.push((prev_end, end, "<SP>".into(), 0, REST));
    }
    segs.into_iter()
        .map(|(a, b, phoneme, pitch, kind)| DitNote { dur: ((b - a) * 1e4).round() / 1e4, phoneme, pitch, kind })
        .collect()
}

/// Round half to even, as numpy does.
fn round_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 {
        2.0 * (x / 2.0).round()
    } else {
        r
    }
}

/// Word-level notes -> tokens (the reference data processor: adjacent rests
/// of one kind and pitch merge, then each note becomes `<BOW>` phones
/// `<EOW>` and the frames cycle through its phones). Unknown phones fail.
pub fn tokens(notes: &[DitNote], phones: &HashMap<String, u32>) -> Result<Tokens, String> {
    // Merge runs of rests.
    let mut merged: Vec<DitNote> = Vec::new();
    for n in notes {
        let ph = if n.phoneme == "<AP>" { "<SP>".to_string() } else { n.phoneme.clone() };
        let kind = if ph == "<SP>" { REST } else { n.kind };
        if let Some(last) = merged.last_mut() {
            if ph == "<SP>" && last.phoneme == "<SP>" && last.kind == kind && last.pitch == n.pitch {
                last.dur += n.dur;
                continue;
            }
        }
        merged.push(DitNote { dur: n.dur, phoneme: ph, pitch: n.pitch, kind });
    }
    let total: f64 = merged.iter().map(|n| n.dur).sum::<f64>() * FPS;
    let frames = total as usize;
    let mut mel2note = vec![0u32; frames];
    let mut locations: Vec<(usize, usize)> = Vec::new();
    let mut new_ph: Vec<&str> = Vec::new();
    let mut origin: Vec<usize> = Vec::new();
    let mut en: Vec<Vec<String>> = Vec::new();
    for n in &merged {
        en.push(match n.phoneme.strip_prefix("en_") {
            Some(rest) => rest.split('-').map(|p| format!("en_{p}")).chain(std::iter::once("<SEP>".to_string())).collect(),
            None => vec![n.phoneme.clone()],
        });
    }
    let mut dur_sum = 0.0f64;
    for (i, n) in merged.iter().enumerate() {
        let at = (round_even(dur_sum * FPS) as usize).min(frames.saturating_sub(1));
        new_ph.push("<BOW>");
        origin.push(i);
        locations.push((at, en[i].len().max(1)));
        for p in &en[i] {
            new_ph.push(p);
            origin.push(i);
        }
        new_ph.push("<EOW>");
        origin.push(i);
        dur_sum += n.dur;
    }
    let mut ph_idx = 1u32;
    for (li, &(mut i, j)) in locations.iter().enumerate() {
        let next = if li + 1 < locations.len() { locations[li + 1].0 } else { frames };
        if i >= frames || i + j > frames {
            break;
        }
        while i < frames && mel2note[i] > 0 {
            i += 1;
        }
        if i >= frames {
            break;
        }
        mel2note[i] = ph_idx;
        let mut k = i + 1;
        while k + j < next {
            for q in 0..j {
                mel2note[k + q] = ph_idx + q as u32 + 1;
            }
            k += j;
        }
        if next >= 1 {
            mel2note[next - 1] = ph_idx + j as u32 + 1;
        }
        ph_idx += j as u32 + 2;
    }
    let mut t = Tokens { mel2note, ..Tokens::default() };
    t.phoneme.push(*phones.get("<PAD>").ok_or("phone set has no <PAD>")?);
    t.pitch.push(0);
    t.kind.push(REST);
    for (p, o) in new_ph.iter().zip(&origin) {
        t.phoneme.push(*phones.get(*p).ok_or_else(|| format!("phone {p} is not in the model's phone set"))?);
        t.pitch.push(merged[*o].pitch.min(255));
        t.kind.push(merged[*o].kind);
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipa_to_arpa() {
        let a = |p: &str, w: &str| arpa_from_ipa(p, w).join(" ");
        assert_eq!(a("nˈʌθɪŋ", "nothing"), "N AH1 TH IH0 NG");
        assert_eq!(a("pɹˈɑməs", "promise"), "P R AA1 M AH0 S");
        assert_eq!(arpa_word("the", "ði").join(" "), "DH AH0");
        assert_eq!(a("hˈɪɹ", "hear"), "HH IY1 R");
        assert_eq!(a("mˈɪɹəɹ", "mirror"), "M IH1 R ER0");
        assert_eq!(a("bənˈiθ", "beneath"), "B IH0 N IY1 TH");
        assert_eq!(a("ˌmi", "me"), "M IY1");
        assert_eq!(a("ʃˈAk", "shake"), "SH EY1 K");
        assert_eq!(a("tˈɛndəɹ", "tender"), "T EH1 N D ER0");
        assert_eq!(a("sˈɪɾi", "city"), "S IH1 T IY0");
        assert_eq!(a("ˈWəɹ", "hour"), "AW1 ER0");
    }

    #[test]
    fn frames_cycle_through_a_notes_phones() {
        let mut phones = HashMap::new();
        for (i, p) in ["<PAD>", "<SP>", "<AP>", "<UNK>", "<BOW>", "<EOW>", "<BOS>", "<EOS>", "<MASK>", "<SEP>", "en_AY1", "en_B"].iter().enumerate() {
            phones.insert(p.to_string(), i as u32);
        }
        let notes = vec![
            DitNote { dur: 0.1, phoneme: "<SP>".into(), pitch: 0, kind: REST },
            DitNote { dur: 0.2, phoneme: "en_B-AY1".into(), pitch: 60, kind: LYRIC },
        ];
        let t = tokens(&notes, &phones).unwrap();
        // <PAD> | <BOW> <SP> <EOW> | <BOW> B AY1 <SEP> <EOW>
        assert_eq!(t.phoneme, vec![0, 4, 1, 5, 4, 11, 10, 9, 5]);
        assert_eq!(t.frames(), 15);
        // Rest frames 0..5: BOW, SP x3, EOW; the word: BOW, (B AY SEP) x3, EOW.
        assert_eq!(t.mel2note, vec![1, 2, 2, 2, 3, 4, 5, 6, 7, 5, 6, 7, 0, 0, 8]);
    }
}
