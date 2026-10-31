//! What happens to a song between its import and its examples: who it is
//! (the split is by song, not file), whether it is a duplicate, the bars a
//! window takes, and the deterministic augmentations (transposition within
//! each role's range, the tempo relabelled).

use makepad_midi_library::analyse;
use makepad_midi_library::arrangement::{Arrangement, Section};
use makepad_midi_library::gm::Role;
use makepad_midi_library::roll::STEPS;
use std::collections::HashSet;

/// FNV-1a, 64 bits: every deterministic choice and every signature.
pub fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn hash_str(parts: &[&str]) -> u64 {
    let mut joined = String::new();
    for p in parts {
        joined.push_str(p);
        joined.push('\u{1f}');
    }
    hash(joined.as_bytes())
}

// ------------------------------------------------------------------ identity

/// Lowercase words, without bracketed asides, featured artists, versions
/// and punctuation: `Take On Me (Live)` and `take on me` are one song.
pub fn norm_name(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth > 0 => {}
            c if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            '&' => out.push_str(" and "),
            _ => out.push(' '),
        }
    }
    let words: Vec<&str> = out.split_whitespace().collect();
    let cut = words.iter().position(|w| matches!(*w, "feat" | "featuring" | "ft" | "remix" | "remastered" | "live" | "version" | "edit" | "mix")).unwrap_or(words.len());
    let words = &words[..cut.max(1).min(words.len())];
    let words: Vec<&str> = words.iter().copied().filter(|w| *w != "the").collect();
    words.join(" ")
}

/// The song a file is a version of: artist and title, their words run
/// together (`Selfcontrol` and `Self Control`, `Youve` and `You've` are
/// one).
pub fn identity(artist: &str, title: &str) -> String {
    format!("{}|{}", norm_name(artist).replace(' ', ""), norm_name(title).replace(' ', ""))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Split {
    Train,
    Val,
    Test,
}

impl Split {
    pub const ALL: [Split; 3] = [Split::Train, Split::Val, Split::Test];

    pub fn name(self) -> &'static str {
        match self {
            Split::Train => "train",
            Split::Val => "val",
            Split::Test => "test",
        }
    }

    pub fn parse(s: &str) -> Option<Split> {
        Split::ALL.into_iter().find(|x| x.name() == s)
    }
}

/// A song identity's split: its hash in 1000 buckets, `val` and `test`
/// per mille each at the top. Every version of a song lands together.
pub fn split_of(identity: &str, val: u32, test: u32) -> Split {
    let bucket = (hash_str(&["split", identity]) % 1000) as u32;
    if bucket >= 1000 - test {
        Split::Test
    } else if bucket >= 1000 - test - val {
        Split::Val
    } else {
        Split::Train
    }
}

// ------------------------------------------------------------------ keys

const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// "A minor" as (tonic pitch class, minor).
pub fn parse_key(key: &str) -> Option<(u8, bool)> {
    let (tonic, mode) = key.split_once(' ')?;
    let pc = NAMES.iter().position(|n| *n == tonic).or_else(|| {
        const SHARPS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        SHARPS.iter().position(|n| *n == tonic)
    })? as u8;
    Some((pc, mode == "minor"))
}

pub fn key_name(pc: u8, minor: bool) -> String {
    format!("{} {}", NAMES[pc as usize % 12], if minor { "minor" } else { "major" })
}

/// The key of an arrangement's notes (as `midi_library` finds keys).
pub fn key_of(a: &Arrangement) -> String {
    analyse::key_of_histogram(&pc_histogram(a, None))
}

/// Duration-weighted pitch classes of the pitched parts (bars `from..to`).
pub fn pc_histogram(a: &Arrangement, bars: Option<(usize, usize)>) -> [f64; 12] {
    let (lo, hi) = bars.map_or((f64::MIN, f64::MAX), |(f, t)| (a.bars[f.min(a.bar_count())], a.bars[t.min(a.bar_count())]));
    let mut hist = [0f64; 12];
    for p in a.parts.iter().filter(|p| p.role != Role::Drums) {
        for n in p.notes.iter().filter(|n| n.beat >= lo - 1e-9 && n.beat < hi - 1e-9) {
            hist[n.key.round().clamp(0.0, 127.0) as usize % 12] += n.dur.max(0.05);
        }
    }
    hist
}

// ------------------------------------------------------------------ windows

/// Bars `from..to` of `a` as a song of their own: notes that start in
/// them (held at most to the window's end), the sections cut to them, the
/// tempo at the window's start.
pub fn slice(a: &Arrangement, from: usize, to: usize) -> Arrangement {
    let to = to.min(a.bar_count());
    let from = from.min(to);
    let start = a.bars[from];
    let end = a.bars[to];
    let mut out = a.clone();
    out.bars = a.bars[from..=to].iter().map(|b| b - start).collect();
    for p in &mut out.parts {
        p.notes.retain(|n| n.beat >= start - 1e-9 && n.beat < end - 1e-9);
        for n in &mut p.notes {
            n.beat -= start;
            n.dur = n.dur.min(end - start - n.beat).max(1.0 / STEPS);
        }
        p.pedal.clear();
        p.programs.clear();
    }
    out.parts.retain(|p| !p.notes.is_empty());
    out.sections = a.sections.iter().filter(|s| s.to > from && s.from < to).map(|s| Section { from: s.from.max(from) - from, to: s.to.min(to) - from, ..s.clone() }).collect();
    out.bpm = a.bpm_at(start);
    out.tempo = a.tempo.iter().filter(|t| t.0 > start + 1e-9 && t.0 < end - 1e-9).map(|t| (t.0 - start, t.1)).collect();
    out
}

/// The bars each section starts at (window starts), ascending.
pub fn section_starts(a: &Arrangement) -> Vec<usize> {
    let mut starts: Vec<usize> = a.sections.iter().map(|s| s.from).collect();
    starts.push(0);
    starts.sort();
    starts.dedup();
    starts
}

/// Up to `n` windows of `len` bars, each starting on a section start where
/// it can, not overlapping, spread over the song in order.
pub fn windows(a: &Arrangement, len: usize, n: usize, salt: &str) -> Vec<usize> {
    let bars = a.bar_count();
    if bars < len || n == 0 {
        return Vec::new();
    }
    let mut starts: Vec<usize> = section_starts(a).into_iter().filter(|s| s + len <= bars).collect();
    if starts.is_empty() {
        starts.push(0);
    }
    // Rotate by the salt so different tasks prefer different sections.
    let rot = (hash_str(&["win", salt]) % starts.len() as u64) as usize;
    starts.rotate_left(rot);
    let mut out: Vec<usize> = Vec::new();
    for s in starts {
        if out.len() == n {
            break;
        }
        if out.iter().all(|o| s + len <= *o || *o + len <= s) {
            out.push(s);
        }
    }
    out.sort();
    out
}

// ------------------------------------------------------------------ augmentation

/// The range a role's notes stay in when transposed (MIDI keys).
pub fn role_range(role: Role, melody: bool) -> (f32, f32) {
    if melody {
        return (48.0, 96.0);
    }
    match role {
        Role::Drums => (0.0, 127.0),
        Role::Bass => (24.0, 67.0),
        Role::Lead => (48.0, 100.0),
        Role::Fx => (21.0, 108.0),
        _ => (33.0, 100.0),
    }
}

/// The shifts (semitones, -6..=6 but 0) that keep every part in its range.
pub fn shifts(a: &Arrangement) -> Vec<i32> {
    (-6..=6)
        .filter(|s| *s != 0)
        .filter(|s| {
            a.parts.iter().filter(|p| p.role != Role::Drums).all(|p| {
                let (lo, hi) = role_range(p.role, p.melody);
                p.notes.iter().all(|n| n.key + *s as f32 >= lo.min(n.key) && n.key + *s as f32 <= hi.max(n.key))
            })
        })
        .collect()
}

/// `a` moved by `shift` semitones (drums stay), its key renamed.
pub fn transpose(a: &Arrangement, shift: i32) -> Arrangement {
    let mut out = a.clone();
    for p in out.parts.iter_mut().filter(|p| p.role != Role::Drums) {
        for n in &mut p.notes {
            n.key += shift as f32;
        }
    }
    if let Some((pc, minor)) = parse_key(&a.key) {
        out.key = key_name(((pc as i32 + shift).rem_euclid(12)) as u8, minor);
    }
    out
}

/// A tempo as words.
pub fn tempo_label(bpm: f64) -> &'static str {
    match bpm {
        b if b < 76.0 => "slow",
        b if b < 100.0 => "relaxed",
        b if b < 118.0 => "mid-tempo",
        b if b < 136.0 => "upbeat",
        b if b < 160.0 => "fast",
        _ => "very fast",
    }
}

// ------------------------------------------------------------------ signatures

/// A song's duplicate signatures: the whole song (every part's onsets,
/// transposed to C, part names dropped) and its core bars (tune and bass
/// onsets as pitch classes on the grid) as a set, for versions of one
/// song.
#[derive(Clone, Debug, Default)]
pub struct Signature {
    pub song: u64,
    pub core: HashSet<u64>,
}

pub fn signature(a: &Arrangement) -> Signature {
    let tonic = parse_key(&a.key).map_or(0, |k| k.0) as i32;
    let bars = a.bar_count();
    let mut full: Vec<Vec<(u8, i64, i32)>> = vec![Vec::new(); bars];
    let mut core: Vec<Vec<(u8, i64, i32)>> = vec![Vec::new(); bars];
    for p in &a.parts {
        let drums = p.role == Role::Drums;
        for n in &p.notes {
            let bar = a.bar_of(n.beat);
            if bar >= bars {
                continue;
            }
            let step = ((n.beat - a.bars[bar]) * STEPS).round() as i64;
            let key = if drums { n.key.round() as i32 } else { n.key.round() as i32 - tonic };
            full[bar].push((p.role as u8, step, key));
            if p.melody || p.role == Role::Bass {
                core[bar].push((p.melody as u8, step, key.rem_euclid(12)));
            }
        }
    }
    let bar_hash = |v: &mut Vec<(u8, i64, i32)>| {
        v.sort();
        v.dedup();
        let mut bytes = Vec::with_capacity(v.len() * 12);
        for (r, s, k) in v.iter() {
            bytes.push(*r);
            bytes.extend_from_slice(&s.to_le_bytes());
            bytes.extend_from_slice(&k.to_le_bytes());
        }
        hash(&bytes)
    };
    let full: Vec<u64> = full.iter_mut().map(bar_hash).collect();
    let mut song_bytes = Vec::new();
    for h in &full {
        song_bytes.extend_from_slice(&h.to_le_bytes());
    }
    let core: HashSet<u64> = core.iter_mut().filter(|v| !v.is_empty()).map(bar_hash).collect();
    Signature { song: hash(&song_bytes), core }
}

/// How alike two core sets are (Jaccard).
pub fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.intersection(b).count() as f64;
    common / ((a.len() + b.len()) as f64 - common)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_splits() {
        assert_eq!(identity("The Beatles", "Hey Jude (Remastered)"), identity("Beatles", "hey jude"));
        assert_eq!(identity("Laura Branigan", "Selfcontrol"), identity("Laura Branigan", "Self Control"));
        assert_eq!(norm_name("Take On Me - Live"), "take on me");
        assert_eq!(norm_name("Simon & Garfunkel"), "simon and garfunkel");
        let id = identity("a-ha", "Take On Me");
        assert_eq!(split_of(&id, 50, 50), split_of(&id, 50, 50));
        let n = 20000;
        let test = (0..n).filter(|i| split_of(&format!("x|{i}"), 50, 50) == Split::Test).count();
        assert!((800..1200).contains(&test), "{test}");
    }

    #[test]
    fn keys_transpose() {
        assert_eq!(parse_key("F# minor"), Some((6, true)));
        assert_eq!(key_name(((9 + 3) % 12) as u8, false), "C major");
        let a: HashSet<u64> = [1, 2, 3, 4].into();
        let b: HashSet<u64> = [3, 4, 5, 6].into();
        assert!((jaccard(&a, &b) - 2.0 / 6.0).abs() < 1e-9);
    }
}
