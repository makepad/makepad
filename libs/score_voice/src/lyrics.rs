//! Lyrics to sung events: words are pronounced (Kokoro's G2P when the host
//! passes it in, or a small built-in fallback), split into syllables
//! (maximal onsets), matched to notes (melismas with `_`, written syllable
//! breaks with `-`, explicit IPA between slashes: `/lʌv/`, syllables split
//! with dots `/hə.loʊ/`), and each syllable is shifted earlier by its onset
//! consonants so its vowel lands on the beat. The result is plain data for a
//! player: events that index a table of [`PhonemeSeq`]s.

use crate::harmony::harmonize;
use crate::phoneme::{self, Ph};
use crate::singer::{syllable_timing, PhonemeSeq};
use std::collections::HashMap;
use std::sync::Arc;

/// One note of the line: when, how long (beats), its pitch or a chord
/// (MIDI; the highest is the melody), velocity 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteIn {
    pub beat: f64,
    pub dur: f64,
    pub pitches: Vec<f32>,
    pub vel: f32,
}

/// A chord from the Score, from `beat` on (MIDI pitches, any octave).
#[derive(Clone, Debug, PartialEq)]
pub struct ChordAt {
    pub beat: f64,
    pub pitches: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SingOpts {
    pub bpm: f64,
    /// Put each vowel on its beat by starting the onset consonants early.
    pub lead: bool,
    /// Events never start before this beat (a cycle's start).
    pub min_beat: f64,
    /// Harmony: the chords and how many parts sing (the melody included).
    pub chords: Vec<ChordAt>,
    pub parts: u8,
}

impl Default for SingOpts {
    fn default() -> Self {
        SingOpts { bpm: 120.0, lead: true, min_beat: f64::NEG_INFINITY, chords: Vec::new(), parts: 1 }
    }
}

/// A sung event: the player starts the note at `beat` (its first sound) for
/// `dur` beats, with the syllable `table[syllable]`, on choir part `part`
/// (0 = the melody).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SungEvent {
    pub beat: f64,
    pub dur: f64,
    pub note: f32,
    pub vel: f32,
    pub syllable: u16,
    pub part: u8,
    /// Where its vowel lands (the written beat).
    pub vowel_beat: f64,
}

/// A syllable as written, where it is sung (for the lyric display).
#[derive(Clone, Debug, PartialEq)]
pub struct SyllableSpan {
    pub text: String,
    pub ipa: String,
    pub beat: f64,
    pub dur: f64,
    pub note: f32,
}

#[derive(Clone, Debug)]
pub struct Lowered {
    pub events: Vec<SungEvent>,
    pub table: Arc<[PhonemeSeq]>,
    pub syllables: Vec<SyllableSpan>,
    pub warnings: Vec<String>,
}

/// How a word sounds, as IPA (Kokoro notation accepted).
pub type Pronounce<'a> = &'a dyn Fn(&str) -> String;

/// The built-in fallback pronouncer: a small lyric lexicon, then letter
/// rules. A host with Kokoro passes its G2P instead (`kokoro::pronounce`).
pub fn builtin_pronounce(word: &str) -> String {
    let w = word.to_lowercase();
    if let Some((_, ipa)) = LEXICON.iter().find(|(k, _)| *k == w) {
        return ipa.to_string();
    }
    letters_to_sound(&w)
}

const LEXICON: &[(&str, &str)] = &[
    ("a", "ə"), ("ah", "ɑ"), ("all", "ɔl"), ("amazing", "əmeɪzɪŋ"), ("and", "ænd"), ("are", "ɑɹ"), ("away", "əweɪ"), ("be", "bi"),
    ("blue", "blu"), ("bright", "bɹaɪt"), ("can", "kæn"), ("day", "deɪ"), ("dream", "dɹim"), ("fire", "faɪɝ"), ("fly", "flaɪ"),
    ("free", "fɹi"), ("go", "ɡoʊ"), ("grace", "ɡɹeɪs"), ("hallelujah", "hæləlujə"), ("heart", "hɑɹt"), ("hello", "həloʊ"),
    ("hold", "hoʊld"), ("home", "hoʊm"), ("how", "haʊ"), ("i", "aɪ"), ("in", "ɪn"), ("is", "ɪz"), ("la", "lɑ"), ("let", "lɛt"),
    ("light", "laɪt"), ("love", "lʌv"), ("me", "mi"), ("mm", "m"), ("moon", "mun"), ("my", "maɪ"), ("never", "nɛvɝ"), ("night", "naɪt"),
    ("oh", "oʊ"), ("on", "ɑn"), ("ooh", "u"), ("rain", "ɹeɪn"), ("shining", "ʃaɪnɪŋ"), ("sing", "sɪŋ"), ("sky", "skaɪ"),
    ("song", "sɔŋ"), ("sound", "saʊnd"), ("stars", "stɑɹz"), ("sun", "sʌn"), ("sweet", "swit"), ("that", "ðæt"), ("the", "ðə"),
    ("to", "tu"), ("tonight", "tənaɪt"), ("we", "wi"), ("with", "wɪð"), ("yeah", "jɛə"), ("you", "ju"), ("your", "jɔɹ"),
    ("saved", "seɪvd"), ("wretch", "ɹɛʧ"), ("like", "laɪk"), ("shine", "ʃaɪn"), ("young", "jʌŋ"), ("was", "wʌz"), ("lost", "lɔst"),
    ("found", "faʊnd"), ("now", "naʊ"), ("blind", "blaɪnd"), ("see", "si"), ("but", "bʌt"), ("once", "wʌns"),
];

fn letters_to_sound(word: &str) -> String {
    let w: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    let mut out = String::new();
    let mut i = 0;
    let vowel = |c: char| "aeiou".contains(c);
    while i < w.len() {
        let rest: String = w[i..].iter().collect();
        let (ipa, n) = if rest.starts_with("igh") {
            ("aɪ", 3)
        } else if let Some((_, ipa)) = [("sh", "ʃ"), ("ch", "ʧ"), ("th", "θ"), ("ng", "ŋ"), ("ph", "f"), ("ck", "k"), ("ee", "i"), ("ea", "i"), ("oo", "u"), ("ai", "eɪ"), ("ay", "eɪ"), ("ou", "aʊ"), ("ow", "aʊ"), ("oa", "oʊ"), ("qu", "kw")]
            .iter()
            .find(|(g, _)| rest.starts_with(g))
        {
            (*ipa, 2)
        } else {
            let c = w[i];
            // A vowel before one consonant and a final e is long.
            let magic = vowel(c) && i + 2 == w.len() - 1 && w[w.len() - 1] == 'e' && !vowel(w[i + 1]);
            let ipa = match c {
                'a' if magic => "eɪ",
                'e' if magic => "i",
                'i' if magic => "aɪ",
                'o' if magic => "oʊ",
                'u' if magic => "u",
                'a' => "æ",
                'e' if i + 1 == w.len() && i > 0 => "",
                'e' => "ɛ",
                'i' => "ɪ",
                'o' => "ɑ",
                'u' => "ʌ",
                'y' if i == 0 => "j",
                'y' => "i",
                'c' | 'q' => "k",
                'x' => "ks",
                'j' => "ʤ",
                'r' => "ɹ",
                'g' => "ɡ",
                c if "bdfhklmnpstvwz".contains(c) => {
                    out.push(c);
                    i += 1;
                    continue;
                }
                _ => "",
            };
            (ipa, 1)
        };
        out.push_str(ipa);
        i += n;
    }
    out
}

fn is_diphthong(a: Ph, b: Ph) -> bool {
    let (a, b) = (phoneme::info(a).ipa, phoneme::info(b).ipa);
    matches!(a, "e" | "a" | "o" | "ɔ" | "ə") && matches!(b, "ɪ" | "ʊ")
}

fn legal_onset(cs: &[Ph]) -> bool {
    let s: String = cs.iter().map(|p| phoneme::info(*p).ipa).collect();
    match cs.len() {
        0 => true,
        1 => s != "ŋ",
        2 => [
            "sp", "st", "sk", "sm", "sn", "sl", "sw", "pl", "pɹ", "bl", "bɹ", "tɹ", "dɹ", "kl", "kɹ", "ɡl", "ɡɹ", "fl", "fɹ", "θɹ", "ʃɹ",
            "tw", "dw", "kw", "ɡw", "pj", "bj", "kj", "fj", "mj", "hj", "vj",
        ]
        .contains(&s.as_str()),
        3 => ["spl", "spɹ", "stɹ", "skɹ", "skw", "skj", "spj"].contains(&s.as_str()),
        _ => false,
    }
}

/// Split a word's phonemes into syllables: one per vowel (a diphthong is
/// one), the consonants between two vowels going to the later syllable as
/// far as they make a legal English onset.
pub fn syllabify(phones: &[Ph]) -> Vec<Vec<Ph>> {
    // Nucleus spans.
    let mut nuclei: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < phones.len() {
        if phoneme::is_vowel(phones[i]) {
            let end = if i + 1 < phones.len() && is_diphthong(phones[i], phones[i + 1]) { i + 2 } else { i + 1 };
            nuclei.push((i, end));
            i = end;
        } else {
            i += 1;
        }
    }
    if nuclei.is_empty() {
        return if phones.is_empty() { Vec::new() } else { vec![phones.to_vec()] };
    }
    let mut cuts = vec![0usize];
    for w in nuclei.windows(2) {
        let (from, to) = (w[0].1, w[1].0);
        let mut cut = to;
        while cut > from && legal_onset(&phones[cut - 1..to]) {
            cut -= 1;
        }
        cuts.push(cut);
    }
    cuts.push(phones.len());
    cuts.windows(2).map(|w| phones[w[0]..w[1]].to_vec()).collect()
}

enum Item {
    Syllable { phones: Vec<Ph>, text: String },
    Melisma,
}

fn tokenize(lyrics: &str, pronounce: Pronounce, warnings: &mut Vec<String>) -> Vec<Item> {
    let mut items = Vec::new();
    for token in lyrics.split_whitespace() {
        if token.chars().all(|c| c == '_' || c == '~') {
            for _ in token.chars() {
                items.push(Item::Melisma);
            }
            continue;
        }
        if let Some(ipa) = token.strip_prefix('/').and_then(|t| t.strip_suffix('/')) {
            for part in ipa.split('.') {
                items.push(Item::Syllable { phones: phoneme::parse(part), text: part.to_string() });
            }
            continue;
        }
        let written: Vec<String> = token
            .split('-')
            .map(|p| p.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>())
            .filter(|p| !p.is_empty())
            .collect();
        if written.is_empty() {
            continue;
        }
        let word: String = written.concat();
        let phones = phoneme::parse(&pronounce(&word));
        if phones.is_empty() {
            warnings.push(format!("no pronunciation for {word:?}"));
            continue;
        }
        let syllables = syllabify(&phones);
        let texts: Vec<String> = if written.len() == syllables.len() {
            written
        } else {
            // Written breaks that do not match: share the letters out.
            let chars: Vec<char> = word.chars().collect();
            let n = syllables.len().max(1);
            (0..n).map(|k| chars[k * chars.len() / n..(k + 1) * chars.len() / n].iter().collect()).collect()
        };
        for (phones, text) in syllables.into_iter().zip(texts) {
            items.push(Item::Syllable { phones, text });
        }
    }
    items
}

/// Lower a sung line: notes and lyrics to events, the syllable table and
/// the display spans.
pub fn lower_sing(notes: &[NoteIn], lyrics: &str, opts: &SingOpts, pronounce: Pronounce) -> Lowered {
    let mut warnings = Vec::new();
    let items = tokenize(lyrics, pronounce, &mut warnings);
    // Groups: a syllable and the notes it is sung on.
    let mut groups: Vec<(Vec<Ph>, String, Vec<usize>)> = Vec::new();
    let mut next = 0usize;
    for (n, _) in notes.iter().enumerate() {
        match items.get(next) {
            Some(Item::Melisma) => {
                next += 1;
                if let Some(last) = groups.last_mut() {
                    last.2.push(n);
                } else {
                    warnings.push("a melisma before the first syllable".into());
                }
            }
            Some(Item::Syllable { phones, text }) => {
                next += 1;
                groups.push((phones.clone(), text.clone(), vec![n]));
            }
            None => {
                if let Some(last) = groups.last_mut() {
                    last.2.push(n);
                }
            }
        }
    }
    let left = items[next.min(items.len())..].iter().filter(|i| matches!(i, Item::Syllable { .. })).count();
    if left > 0 {
        warnings.push(format!("{left} syllable(s) left without notes"));
    }

    // One PhonemeSeq per sung note.
    let mut seqs: Vec<(usize, PhonemeSeq)> = Vec::new();
    let mut spans = Vec::new();
    for (phones, text, group) in &groups {
        let nuc = phones.iter().position(|p| phoneme::is_vowel(*p));
        let k = group.len();
        for (j, n) in group.iter().enumerate() {
            let seq_phones: Vec<Ph> = match (nuc, k) {
                (_, 1) | (None, _) => phones.clone(),
                (Some(v), _) if j == 0 => phones[..=v].to_vec(),
                (Some(v), _) if j + 1 == k => phones[v..].to_vec(),
                (Some(v), _) => vec![phones[v]],
            };
            let mut flags = if j > 0 { PhonemeSeq::CONT | PhonemeSeq::LEGATO } else { 0 };
            if j == 0 && *n > 0 {
                let prev = &notes[n - 1];
                if prev.beat + prev.dur >= notes[*n].beat - 1e-6 {
                    flags |= PhonemeSeq::LEGATO;
                }
            }
            seqs.push((*n, PhonemeSeq::new(&seq_phones, flags)));
        }
        let first = &notes[group[0]];
        let last = &notes[*group.last().unwrap()];
        spans.push(SyllableSpan {
            text: text.clone(), ipa: phoneme::to_ipa(phones), beat: first.beat, dur: last.beat + last.dur - first.beat,
            note: first.pitches.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        });
    }

    // The table, one entry per distinct syllable.
    let mut index: HashMap<PhonemeSeq, u16> = HashMap::new();
    let mut table = Vec::new();
    let mut ids = Vec::with_capacity(seqs.len());
    for (_, seq) in &seqs {
        let id = *index.entry(*seq).or_insert_with(|| {
            table.push(*seq);
            (table.len() - 1) as u16
        });
        ids.push(id);
    }

    // Start times: each vowel on its beat, onsets before it.
    let secs = |beats: f64| (beats * 60.0 / opts.bpm.max(1.0)) as f32;
    let beats = |secs: f32| secs as f64 * opts.bpm.max(1.0) / 60.0;
    let count = seqs.len();
    let mut starts = vec![0.0f64; count];
    for (i, (n, seq)) in seqs.iter().enumerate() {
        let note = &notes[*n];
        let mut lead = 0.0;
        if opts.lead {
            for _ in 0..2 {
                let (onset, _) = syllable_timing(seq, secs(note.dur + lead));
                lead = beats(onset);
            }
        }
        let mut start = (note.beat - lead).max(opts.min_beat);
        if i > 0 {
            let prev = &notes[seqs[i - 1].0];
            start = start.max(prev.beat + prev.dur.min(note.beat - prev.beat) * 0.5);
        }
        starts[i] = start.min(note.beat);
    }
    let mut events = Vec::new();
    let mut prev_harmony: Vec<f32> = Vec::new();
    for (i, (n, _)) in seqs.iter().enumerate() {
        let note = &notes[*n];
        let end = note.beat + note.dur;
        let dur = match starts.get(i + 1) {
            Some(next) if *next <= end + 1e-6 => next - starts[i],
            _ => end - starts[i],
        };
        let mut pitches = note.pitches.clone();
        pitches.sort_by(|a, b| b.partial_cmp(a).unwrap());
        if pitches.len() == 1 && opts.parts > 1 {
            if let Some(chord) = opts.chords.iter().rev().find(|c| c.beat <= note.beat + 1e-9).or(opts.chords.first()) {
                let parts = harmonize(pitches[0], &chord.pitches, opts.parts as usize, &prev_harmony);
                prev_harmony = parts.clone();
                pitches = parts;
            }
        }
        for (part, pitch) in pitches.iter().enumerate() {
            events.push(SungEvent { beat: starts[i], dur, note: *pitch, vel: note.vel, syllable: ids[i], part: part as u8, vowel_beat: note.beat });
        }
    }
    Lowered { events, table: table.into(), syllables: spans, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipa(s: &[Vec<Ph>]) -> Vec<String> {
        s.iter().map(|p| phoneme::to_ipa(p)).collect()
    }

    #[test]
    fn syllables_take_maximal_legal_onsets() {
        assert_eq!(ipa(&syllabify(&phoneme::parse("əmeɪzɪŋ"))), ["ə", "meɪ", "zɪŋ"]);
        assert_eq!(ipa(&syllabify(&phoneme::parse("mɪstɹi"))), ["mɪ", "stɹi"]);
        assert_eq!(ipa(&syllabify(&phoneme::parse("hæləlujə"))), ["hæ", "lə", "lu", "jə"]);
        assert_eq!(ipa(&syllabify(&phoneme::parse("ʃaɪnɪŋ"))), ["ʃaɪ", "nɪŋ"]);
    }

    #[test]
    fn notes_and_lyrics_align_with_melismas_and_leads() {
        let n = |beat: f64, p: f32| NoteIn { beat, dur: 1.0, pitches: vec![p], vel: 0.8 };
        let notes = [n(0.0, 60.0), n(1.0, 62.0), n(2.0, 64.0), n(3.0, 65.0)];
        let out = lower_sing(&notes, "sing a _ song", &SingOpts { min_beat: -1.0, ..SingOpts::default() }, &builtin_pronounce);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(out.events.len(), 4);
        // "a" spans two notes: the second continues its vowel.
        let seq = out.table[out.events[2].syllable as usize];
        assert!(seq.flags & PhonemeSeq::CONT != 0);
        // "sing" starts before beat 0 (its s), its vowel on the beat.
        assert!(out.events[0].beat < 0.0 && out.events[0].vowel_beat == 0.0);
        // Legato durations: each event runs to the next one's start.
        for w in out.events.windows(2) {
            assert!((w[0].beat + w[0].dur - w[1].beat).abs() < 1e-9);
        }
        assert_eq!(out.syllables.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["sing", "a", "song"]);
        assert!((out.syllables[1].dur - 2.0).abs() < 1e-9);
    }

    #[test]
    fn explicit_ipa_and_hyphens() {
        let n = |beat: f64| NoteIn { beat, dur: 0.5, pitches: vec![67.0], vel: 0.8 };
        let notes = [n(0.0), n(0.5), n(1.0)];
        let out = lower_sing(&notes, "/hə.loʊ/ you", &SingOpts::default(), &builtin_pronounce);
        assert_eq!(out.syllables.iter().map(|s| s.ipa.as_str()).collect::<Vec<_>>(), ["hə", "loʊ", "ju"]);
        let out = lower_sing(&notes[..2], "hel-lo", &SingOpts::default(), &builtin_pronounce);
        assert_eq!(out.syllables.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["hel", "lo"]);
    }
}
