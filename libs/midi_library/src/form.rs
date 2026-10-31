//! A song's form found from repetition: the bars compared by harmony
//! (duration-weighted pitch classes), the tune (its notes on the sixteenth
//! grid) and the groove (drum onsets); the song cut into sections of four
//! bars or more where the music changes; sections that sound alike given
//! one letter; and the letters named (intro, verse, chorus, bridge, solo,
//! outro) by where they fall, how often they come back and how full they
//! are.

use crate::analyse::Song;

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// A Score identifier: `intro`, `verse_1`, `chorus_2`.
    pub name: String,
    /// The kind without its number (`verse`).
    pub kind: String,
    /// Measures `from..to` of [`Song::measures`].
    pub from: usize,
    pub to: usize,
    /// Which sections sound alike (the same letter).
    pub letter: char,
    /// Parts playing per bar, on average.
    pub energy: f64,
}

/// What one bar sounds like.
#[derive(Clone, Default)]
struct Bar {
    chroma: [f64; 12],
    /// The tune's notes as (sixteenth, key).
    tune: Vec<(u8, u8)>,
    drums: [f64; 16],
    /// Parts with a note in the bar.
    parts: usize,
}

fn cos(a: &[f64], b: &[f64]) -> Option<f64> {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    (na > 0.0 && nb > 0.0).then(|| dot / (na * nb))
}

fn bars(song: &Song, measures: &[(u64, u64)]) -> Vec<Bar> {
    let mut out = vec![Bar::default(); measures.len()];
    let bar_of = |tick: u64| measures.partition_point(|m| m.0 <= tick).saturating_sub(1);
    for part in &song.parts {
        let mut seen = vec![false; measures.len()];
        for n in &part.notes {
            let b = bar_of(n.on);
            if b >= out.len() {
                continue;
            }
            seen[b] = true;
            let (start, len) = measures[b];
            let slot = (((n.on - start) as f64 / len as f64) * 16.0).floor().clamp(0.0, 15.0) as u8;
            if part.drums {
                out[b].drums[slot as usize] += n.vel as f64 / 127.0;
                continue;
            }
            // Harmony: the note's length inside each bar it covers.
            let mut at = n.on;
            let mut bar = b;
            while at < n.off && bar < measures.len() {
                let bar_end = measures[bar].0 + measures[bar].1;
                let until = n.off.min(bar_end);
                out[bar].chroma[n.key as usize % 12] += (until - at) as f64;
                at = until;
                bar += 1;
            }
            if part.melody {
                out[b].tune.push((slot, n.key));
            }
        }
        for (b, s) in seen.iter().enumerate() {
            if *s {
                out[b].parts += 1;
            }
        }
    }
    out
}

fn bar_sim(a: &Bar, b: &Bar) -> f64 {
    let mut total = 0.0;
    let mut weight = 0.0;
    match cos(&a.chroma, &b.chroma) {
        Some(c) => {
            total += 0.45 * c;
            weight += 0.45;
        }
        None if a.chroma.iter().sum::<f64>() == 0.0 && b.chroma.iter().sum::<f64>() == 0.0 => {}
        None => weight += 0.45,
    }
    if !a.tune.is_empty() || !b.tune.is_empty() {
        let common = a.tune.iter().filter(|n| b.tune.contains(n)).count() as f64;
        let union = (a.tune.len() + b.tune.len()) as f64 - common;
        total += 0.4 * common / union.max(1.0);
        weight += 0.4;
    }
    match cos(&a.drums, &b.drums) {
        Some(c) => {
            total += 0.15 * c;
            weight += 0.15;
        }
        None if a.drums.iter().sum::<f64>() == 0.0 && b.drums.iter().sum::<f64>() == 0.0 => {}
        None => weight += 0.15,
    }
    // Two empty bars are alike; a full one and an empty one are not.
    if weight == 0.0 { 1.0 } else { total / weight }
}

/// How alike bars `a..a+len` and `b..b+len` are, 0..1.
fn span_sim(bars: &[Bar], a: usize, b: usize, len: usize) -> f64 {
    let n = len.min(bars.len().saturating_sub(a)).min(bars.len().saturating_sub(b));
    if n == 0 {
        return 0.0;
    }
    (0..n).map(|k| bar_sim(&bars[a + k], &bars[b + k])).sum::<f64>() / n as f64
}

/// The song's sections over `measures` (from the first measure to the
/// last; the caller trims silence before it).
pub fn sections(song: &Song, measures: &[(u64, u64)]) -> Vec<Section> {
    let bars = bars(song, measures);
    let n = bars.len();
    if n == 0 {
        return Vec::new();
    }
    // Boundaries every 4 bars where the next 4 differ from the last 4.
    const BLOCK: usize = 4;
    let mut novelty: Vec<(usize, f64)> = Vec::new();
    let mut b = BLOCK;
    while b + 2 <= n {
        let sim = span_sim(&bars, b - BLOCK, b, BLOCK);
        let before = bars[b - BLOCK..b].iter().map(|x| x.parts).sum::<usize>() as f64 / BLOCK as f64;
        let after = bars[b..(b + BLOCK).min(n)].iter().map(|x| x.parts).sum::<usize>() as f64 / (b + BLOCK).min(n).saturating_sub(b).max(1) as f64;
        novelty.push((b, (1.0 - sim) + (after - before).abs() * 0.15));
        b += BLOCK;
    }
    let mut sorted: Vec<f64> = novelty.iter().map(|v| v.1).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let threshold = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0).max(0.2);
    let mut starts: Vec<usize> = vec![0];
    for &(b, v) in &novelty {
        if v >= threshold {
            starts.push(b);
        }
    }
    // A section is at most 16 bars: a longer one is cut at its most
    // changing boundary, again until none is.
    loop {
        let mut cut = None;
        for (i, &s) in starts.iter().enumerate() {
            let e = starts.get(i + 1).copied().unwrap_or(n);
            if e - s > 16 {
                let best = novelty.iter().filter(|(b, _)| *b > s + 3 && *b + 4 <= e).max_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0).unwrap_or(s + 8);
                cut = Some(best);
                break;
            }
        }
        match cut {
            Some(c) => {
                starts.push(c);
                starts.sort();
            }
            None => break,
        }
    }
    // The tail shorter than 4 bars joins the section before it.
    if starts.len() > 1 && n - starts[starts.len() - 1] < 4 {
        starts.pop();
    }
    let spans: Vec<(usize, usize)> = starts.iter().enumerate().map(|(i, &s)| (s, starts.get(i + 1).copied().unwrap_or(n))).collect();
    // Letters: a section takes the letter of the first one it sounds like.
    let mut letters: Vec<char> = Vec::new();
    let mut firsts: Vec<(char, usize, usize)> = Vec::new();
    for &(s, e) in &spans {
        let len = e - s;
        let like = firsts.iter().find(|(_, fs, fe)| {
            let flen = fe - fs;
            let shorter = len.min(flen);
            shorter * 2 >= len.max(flen) && span_sim(&bars, *fs, s, shorter) >= 0.72
        });
        match like {
            Some((c, _, _)) => letters.push(*c),
            None => {
                let c = (b'A' + (firsts.len() as u8).min(25)) as char;
                firsts.push((c, s, e));
                letters.push(c);
            }
        }
    }
    let energy: Vec<f64> = spans.iter().map(|&(s, e)| bars[s..e].iter().map(|b| b.parts as f64).sum::<f64>() / (e - s).max(1) as f64).collect();
    let count = |c: char| letters.iter().filter(|l| **l == c).count();
    let mean_energy = |c: char| {
        let v: Vec<f64> = letters.iter().zip(&energy).filter(|(l, _)| **l == c).map(|(_, e)| *e).collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    let has_tune = |i: usize| bars[spans[i].0..spans[i].1].iter().any(|b| !b.tune.is_empty());
    // The chorus: the fullest letter that comes back; the verse: another
    // that comes back, mostly before the chorus.
    let repeated: Vec<char> = firsts.iter().map(|f| f.0).filter(|c| count(*c) >= 2).collect();
    let chorus = repeated.iter().copied().max_by(|a, b| (mean_energy(*a) + count(*a) as f64 * 0.1).total_cmp(&(mean_energy(*b) + count(*b) as f64 * 0.1)));
    let first_chorus = chorus.and_then(|c| letters.iter().position(|l| *l == c)).unwrap_or(usize::MAX);
    let verse = repeated.iter().copied().filter(|c| Some(*c) != chorus).min_by_key(|c| letters.iter().position(|l| l == c).unwrap_or(usize::MAX)).filter(|c| letters.iter().position(|l| l == c).unwrap_or(usize::MAX) < first_chorus || count(*c) >= 2);
    let mut kinds: Vec<&str> = Vec::new();
    for (i, &c) in letters.iter().enumerate() {
        let kind = if Some(c) == chorus {
            "chorus"
        } else if Some(c) == verse {
            "verse"
        } else if count(c) >= 2 {
            // Comes back each time just before the chorus.
            let before_chorus = letters.iter().enumerate().filter(|(_, l)| **l == c).all(|(j, _)| letters.get(j + 1) == chorus.as_ref());
            if before_chorus { "pre_chorus" } else { "interlude" }
        } else if i == 0 {
            "intro"
        } else if i + 1 == letters.len() {
            "outro"
        } else if !has_tune(i) && energy[i] >= 2.0 {
            "solo"
        } else {
            "bridge"
        };
        kinds.push(kind);
    }
    let mut out = Vec::new();
    for (i, &(s, e)) in spans.iter().enumerate() {
        let kind = kinds[i];
        let total = kinds.iter().filter(|k| **k == kind).count();
        let nth = kinds[..=i].iter().filter(|k| **k == kind).count();
        let name = if total > 1 { format!("{kind}_{nth}") } else { kind.to_string() };
        out.push(Section { name, kind: kind.to_string(), from: s, to: e, letter: letters[i], energy: energy[i] });
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::analyse::{Note, Part, Song};
    use crate::gm::Role;

    fn part(melody: bool, drums: bool, notes: Vec<Note>) -> Part {
        Part { track: 0, channel: 0, name: String::new(), program: 0, changes: vec![], drums, notes, pedal: vec![], volume: 100, pan: 64, mod_moves: 0, bent: 0, role: if drums { Role::Drums } else { Role::Keys }, melody }
    }

    /// 120 bpm throughout.
    pub fn tempo_120(ppq: u16) -> makepad_midi_file::TempoMap {
        use makepad_midi_file::*;
        let file = MidiFile { header: Header { format: Format::SingleTrack, track_count: 1, division: Division::TicksPerQuarter(ppq), extra_data: Vec::new() }, tracks: vec![Track { events: Vec::new(), trailing_data: Vec::new() }], unknown_chunks: Vec::new() };
        file.tempo_map().unwrap()
    }

    /// verse (A, 8 bars) chorus (B, 8 bars) verse chorus, with a 4-bar intro.
    #[test]
    fn finds_verses_and_choruses() {
        let ppq = 480u64;
        let bar = ppq * 4;
        let mut tune = Vec::new();
        let mut chords = Vec::new();
        let mut kit = Vec::new();
        let plan = [('i', 4), ('v', 8), ('c', 8), ('v', 8), ('c', 8)];
        let mut b = 0u64;
        for (kind, len) in plan {
            for k in 0..len {
                let at = (b + k) * bar;
                let (root, line): (u8, [u8; 4]) = match kind {
                    'i' => (48, [0, 0, 0, 0]),
                    'v' => (if k % 2 == 0 { 45 } else { 41 }, [69, 67, 64, 67]),
                    _ => (if k % 2 == 0 { 48 } else { 43 }, [76, 79, 81, 79]),
                };
                for (j, key) in [root, root + 4, root + 7].into_iter().enumerate() {
                    chords.push(Note { on: at + j as u64, off: at + bar - 10, key, vel: 70, expr: 1.0, bend: 0.0 });
                }
                if kind != 'i' {
                    for (q, key) in line.into_iter().enumerate() {
                        tune.push(Note { on: at + q as u64 * ppq, off: at + q as u64 * ppq + ppq - 20, key, vel: 90, expr: 1.0, bend: 0.0 });
                    }
                    for q in 0..8u64 {
                        kit.push(Note { on: at + q * ppq / 2, off: at + q * ppq / 2 + 30, key: 42, vel: 80, expr: 1.0, bend: 0.0 });
                    }
                    if kind == 'c' {
                        for q in 0..4u64 {
                            kit.push(Note { on: at + q * ppq, off: at + q * ppq + 30, key: 36, vel: 110, expr: 1.0, bend: 0.0 });
                        }
                    }
                }
            }
            b += len;
        }
        kit.sort_by_key(|n| n.on);
        let song = Song {
            ppq: ppq as u32,
            tempo: tempo_120(480),
            sigs: vec![(0, 4, 4)],
            key_sig: None,
            parts: vec![part(true, false, tune), part(false, false, chords), part(false, true, kit)],
            end: b * bar,
        };
        let s = sections(&song, &song.measures());
        let names: Vec<(&str, usize, usize)> = s.iter().map(|s| (s.name.as_str(), s.from, s.to)).collect();
        assert_eq!(names, vec![("intro", 0, 4), ("verse_1", 4, 12), ("chorus_1", 12, 20), ("verse_2", 20, 28), ("chorus_2", 28, 36)], "{s:#?}");
    }
}
