//! MIDI to Score: a recorded performance (a Standard MIDI File) written as
//! score code a person can read and edit. Not a dump: one modelled grand,
//! the hands (or voices) as named note lists with a comment per bar naming
//! its dynamic, the sustain pedal as its own list, and the piece's form as
//! Sections that carry the performer's tempo, point by point, from their
//! start. `check` plays the result through the composer and measures it
//! against the file: every note's start and end, and every pedal move.

use crate::compose::Composer;
use makepad_midi_file::{ChannelMessage, Division, EventKind as MidiEvent, MidiFile, TempoMap};
use makepad_audio_engine::feed::{TEMPO_MAX, TEMPO_MIN};
use makepad_score_player::EventKind;
use std::fmt::Write;

/// How chromatic notes are spelled: `@cs4` or `@db4`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spelling {
    Sharps,
    Flats,
}

/// A hand or a voice: the note list `name`, made of these tracks.
#[derive(Clone, Debug)]
pub struct VoiceSpec {
    pub name: &'static str,
    pub tracks: Vec<usize>,
    pub about: &'static str,
}

/// A section of the form, from the MIDI measure `measure` (0-based, as
/// the file counts them) to the next one.
#[derive(Clone, Debug)]
pub struct SectionSpec {
    pub name: &'static str,
    pub measure: usize,
    pub about: &'static str,
    pub spelling: Option<Spelling>,
}

/// What a transcription needs besides the file: names, the form, the
/// voices, and how the bars are numbered.
#[derive(Clone, Debug)]
pub struct Piece {
    /// The Score's label.
    pub label: &'static str,
    pub composer: &'static str,
    /// The full title (work, key, catalogue number).
    pub work: &'static str,
    /// Lines on the piece and its form, for the header.
    pub about: Vec<&'static str>,
    pub voices: Vec<VoiceSpec>,
    pub sections: Vec<SectionSpec>,
    pub spelling: Spelling,
    /// Bar numbers: from measure `.0` on, the bar is `.1` + measures since
    /// (a pickup measure is bar 0; a second movement restarts at 1).
    pub numbering: Vec<(usize, i32)>,
    /// The performance's credit and licence, first in the header.
    pub credit: Vec<&'static str>,
    /// The terms the score is published under (`Score{rights: …}`).
    pub rights: Option<crate::model::ScoreRights>,
}

/// The rights of a piano-midi.de adaptation, for the library.
pub fn piano_midi_de_rights() -> crate::model::ScoreRights {
    crate::model::ScoreRights {
        license: "CC-BY-SA-3.0-DE".into(),
        credits: "Performance by Bernd Krueger, piano-midi.de (CC BY-SA 3.0 DE); transcribed into a score by Makepad".into(),
        source: "http://www.piano-midi.de".into(),
        terms: "https://creativecommons.org/licenses/by-sa/3.0/de/".into(),
    }
}

/// The attribution every transcription of the piano-midi.de performances
/// carries (CC BY-SA 3.0 DE binds adaptations).
pub const PIANO_MIDI_DE: [&str; 8] = [
    "Performed and sequenced by Bernd Krueger, piano-midi.de (http://www.piano-midi.de).",
    "This score is an adaptation of that performance: its notes, dynamics, pedalling and",
    "tempo are transcribed from his MIDI file. Like the performance, it is licensed under",
    "the Creative Commons Attribution-ShareAlike 3.0 Germany licence (CC BY-SA 3.0 DE):",
    "https://creativecommons.org/licenses/by-sa/3.0/de/ -- share it and adapt it, with",
    "this credit, under the same licence. The composition itself is in the public domain.",
    "",
    "",
];

/// Seconds of silence a performance may keep before its first note; more
/// is played faster (the tempo there is not music).
const LEAD_SECONDS: f64 = 0.6;
/// How long the last bar rings after the music, in seconds.
const RING_SECONDS: f64 = 2.0;
/// Entries on one line of a note list.
const PER_LINE: usize = 6;

/// Dynamic marks by MIDI velocity: below 40 pp, below 56 p, and so on.
const MARKS: [(u8, &str); 6] = [(40, "pp"), (56, "p"), (72, "mp"), (88, "mf"), (104, "f"), (128, "ff")];

pub fn mark(velocity: f64) -> &'static str {
    MARKS[mark_index(velocity)].1
}

fn mark_index(velocity: f64) -> usize {
    MARKS.iter().position(|(below, _)| velocity < *below as f64).unwrap_or(MARKS.len() - 1)
}

/// The mark a bar reads after `previous`: it moves only when the bar's
/// mean velocity is clearly past the boundary (a bar hovering on it keeps
/// the mark it had).
fn next_mark(previous: Option<usize>, velocity: f64) -> usize {
    let index = mark_index(velocity);
    match previous {
        Some(p) if index > p && velocity < MARKS[p].0 as f64 + 4.0 => p,
        Some(p) if index < p && velocity >= MARKS[index].0 as f64 - 4.0 => p,
        _ => index,
    }
}

/// A note name as the score writes it (`cs4`, `eb5`; 60 is `c4`).
pub fn note_name(key: u8, spelling: Spelling) -> String {
    const SHARPS: [&str; 12] = ["c", "cs", "d", "ds", "e", "f", "fs", "g", "gs", "a", "as", "b"];
    const FLATS: [&str; 12] = ["c", "db", "d", "eb", "e", "f", "gb", "g", "ab", "a", "bb", "b"];
    let names = if spelling == Spelling::Sharps { SHARPS } else { FLATS };
    format!("{}{}", names[key as usize % 12], key as i32 / 12 - 1)
}

/// A number with at most `places` decimals, without trailing zeros.
fn num(x: f64, places: usize) -> String {
    let s = format!("{x:.places$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

#[derive(Clone, Copy, Debug)]
struct Note {
    on: u64,
    off: u64,
    key: u8,
    vel: u8,
}

/// The file read into what a transcription writes.
struct Perf {
    ppq: f64,
    tempo: TempoMap,
    /// Measures as (start tick, length in ticks, beats per bar).
    measures: Vec<(u64, u64, (u8, u32))>,
    voices: Vec<Vec<Note>>,
    /// Pedal moves as (tick, value 0..127), redundant ones dropped.
    pedal: Vec<(u64, u8)>,
    end: u64,
}

fn read(bytes: &[u8], piece: &Piece) -> Result<(MidiFile, Perf), String> {
    let file = makepad_midi_file::parse(bytes).map_err(|e| format!("{e:?}"))?;
    let ppq = match file.header.division {
        Division::TicksPerQuarter(ppq) => ppq as f64,
        _ => return Err("SMPTE-timed MIDI files are not supported".into()),
    };
    let tempo = file.tempo_map().map_err(|e| format!("{e:?}"))?;
    let sigs = file.time_signature_map().map_err(|e| format!("{e:?}"))?;
    let sequences = file.paired_notes().map_err(|e| format!("{e:?}"))?;
    let mut voices = vec![Vec::new(); piece.voices.len()];
    let mut end = 0u64;
    for sequence in &sequences {
        for n in &sequence.notes {
            let Some(v) = piece.voices.iter().position(|v| v.tracks.contains(&n.track)) else {
                return Err(format!("track {} has notes but no voice", n.track));
            };
            voices[v].push(Note { on: n.tick_on, off: n.tick_off.max(n.tick_on), key: n.key, vel: n.velocity_on });
            end = end.max(n.tick_off);
        }
    }
    for v in &mut voices {
        v.sort_by_key(|n| (n.on, n.key));
    }
    let mut moves: Vec<(u64, u8)> = Vec::new();
    for track in &file.tracks {
        for event in &track.events {
            if let MidiEvent::Channel(c) = &event.kind {
                if let ChannelMessage::ControlChange { controller: 64, value } = c.message {
                    moves.push((event.tick, value));
                }
            }
        }
    }
    moves.sort_by_key(|m| m.0);
    let mut pedal: Vec<(u64, u8)> = Vec::new();
    for (tick, value) in moves {
        // Two moves at one tick keep the last.
        match pedal.last_mut() {
            Some(last) if last.0 == tick => last.1 = value,
            _ => pedal.push((tick, value)),
        }
        end = end.max(tick);
    }
    // A move to where the pedal already is (up at the start, or the same
    // value again) is no move.
    let mut at = 0u8;
    pedal.retain(|&(_, value)| std::mem::replace(&mut at, value) != value);
    let mut measures = Vec::new();
    let mut tick = 0u64;
    while tick <= end {
        let sig = sigs.at_tick(tick);
        let den = sig.denominator().unwrap_or(4);
        let len = (ppq * 4.0 * sig.numerator as f64 / den as f64).round() as u64;
        measures.push((tick, len.max(1), (sig.numerator, den)));
        tick += len.max(1);
    }
    Ok((file, Perf { ppq, tempo, measures, voices, pedal, end }))
}

impl Perf {
    fn beat(&self, tick: u64) -> f64 {
        tick as f64 / self.ppq
    }

    fn measure_of(&self, tick: u64) -> usize {
        self.measures.partition_point(|m| m.0 <= tick).saturating_sub(1)
    }

    fn first_onset(&self) -> u64 {
        self.voices.iter().filter_map(|v| v.first()).map(|n| n.on).min().unwrap_or(0)
    }
}

/// The bar number the piece gives measure `m`.
fn bar_number(piece: &Piece, m: usize) -> i32 {
    let (from, first) = piece.numbering.iter().rev().find(|(from, _)| *from <= m).copied().unwrap_or((0, 1));
    first + (m - from) as i32
}

/// The tempo points the score plays, (beat, bpm) as written: the file's
/// tempo from its first note (a long silence before it is shortened), the
/// bpm rounded to three places with the rounding carried forward, so the
/// time of every beat stays within a hair of the file's.
fn tempo_points(perf: &Perf) -> Vec<(f64, f64)> {
    let first = perf.first_onset();
    let bpm_at = |tick: u64| {
        let i = perf.tempo.changes.partition_point(|c| c.tick <= tick).saturating_sub(1);
        60e6 / perf.tempo.changes[i].microseconds_per_quarter as f64
    };
    let mut points: Vec<(u64, f64)> = Vec::new();
    let lead_beats = perf.beat(first);
    if first > 0 && perf.tempo.ticks_to_seconds(first) > LEAD_SECONDS {
        points.push((0, (lead_beats * 60.0 / LEAD_SECONDS).clamp(TEMPO_MIN, TEMPO_MAX)));
    }
    points.push((if points.is_empty() { 0 } else { first }, bpm_at(if points.is_empty() { 0 } else { first })));
    for change in &perf.tempo.changes {
        if change.tick > points.last().map_or(0, |p| p.0) {
            points.push((change.tick, 60e6 / change.microseconds_per_quarter as f64));
        }
    }
    points.dedup_by(|b, a| (a.1 - b.1).abs() < 1e-9);
    // Error diffusion: each segment's written bpm makes the score's clock
    // meet the file's at the segment's end.
    // The target is the file's own time of each point's tick (the lead-in
    // shifts it by a constant), so neither the bpm's nor the beat's
    // rounding adds up.
    let lead_in = points.len() > 1 && points[0].0 == 0 && first > 0 && points[1].0 == first;
    let mut out = Vec::with_capacity(points.len());
    let mut written = 0.0f64;
    let mut offset = None;
    for (i, &(tick, bpm)) in points.iter().enumerate() {
        let beat = (perf.beat(tick) * 1e4).round() / 1e4;
        if !(lead_in && i == 0) && offset.is_none() {
            offset = Some(written - perf.tempo.ticks_to_seconds(tick));
        }
        let rounded = match points.get(i + 1) {
            Some(&(next, _)) => {
                let len = (perf.beat(next) * 1e4).round() / 1e4 - beat;
                let want = match offset {
                    Some(offset) => perf.tempo.ticks_to_seconds(next) + offset - written,
                    None => len * 60.0 / bpm,
                };
                let r = ((len * 60.0 / want.max(1e-6)) * 1e3).round() / 1e3;
                let r = r.clamp(TEMPO_MIN, TEMPO_MAX);
                written += len * 60.0 / r;
                r
            }
            None => (bpm * 1e3).round() / 1e3,
        };
        out.push((beat, rounded.clamp(TEMPO_MIN, TEMPO_MAX)));
    }
    out
}

/// A Section's `bars:` for `beats` beats: a whole number, or the exact
/// fraction as arithmetic (`5 + 1/3`).
fn bars_text(beats: f64, per_bar: f64) -> String {
    let bars = beats / per_bar;
    if (bars - bars.round()).abs() < 1e-9 {
        return num(bars.round(), 0);
    }
    // Beats are whole quarters or halves here; find a small denominator.
    for den in 2..=48u32 {
        let n = bars * den as f64;
        if (n - n.round()).abs() < 1e-6 {
            let whole = (bars.floor()) as i64;
            let rest = (n.round() as i64) - whole * den as i64;
            return format!("{whole} + {rest}/{den}");
        }
    }
    num(bars, 9)
}

/// Write `bytes` (a MIDI performance) as score code.
pub fn transcribe(bytes: &[u8], piece: &Piece) -> Result<String, String> {
    let (_, perf) = read(bytes, piece)?;
    let points = tempo_points(&perf);
    let (num0, den0) = perf.measures.first().map_or((4, 4), |m| m.2);
    let per_bar = num0 as f64 * 4.0 / den0 as f64;
    let last_measure = perf.measure_of(perf.end);
    // The music ends on the half beat after its last event (the last
    // bar may be cut short: a fermata's empty rest would only be silence).
    let half = perf.ppq as u64 / 2;
    let music_end = ((perf.end + half) / half * half).min(perf.measures[last_measure].0 + perf.measures[last_measure].1);
    let music_beats = perf.beat(music_end);
    let ring_bpm = (per_bar * 60.0 / RING_SECONDS).clamp(TEMPO_MIN, TEMPO_MAX);
    let total_beats = music_beats + per_bar;
    // The notional tempo: the median over the beats played.
    let clock_bpm = {
        let mut spans: Vec<(f64, f64)> = points.windows(2).map(|w| (w[0].1, w[1].0 - w[0].0)).collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let half = spans.iter().map(|s| s.1).sum::<f64>() / 2.0;
        let mut acc = 0.0;
        spans.iter().find(|s| {
            acc += s.1;
            acc >= half
        }).map_or(points[0].1, |s| s.0).round().clamp(20.0, 400.0)
    };
    // Section starts in measures, the last running to the music's end.
    let mut sections: Vec<(usize, usize, &SectionSpec)> = Vec::new();
    for (i, s) in piece.sections.iter().enumerate() {
        let to = piece.sections.get(i + 1).map_or(last_measure + 1, |n| n.measure);
        if s.measure >= to {
            return Err(format!("section `{}` is empty", s.name));
        }
        sections.push((s.measure, to, s));
    }
    if sections.first().is_none_or(|s| s.0 != 0) {
        return Err("the first section starts at measure 0".into());
    }
    let section_of = |m: usize| sections.iter().position(|s| s.0 <= m && m < s.1).unwrap_or(sections.len() - 1);
    let bars_label = |from: usize, to: usize| {
        let (a, b) = (bar_number(piece, from), bar_number(piece, to - 1));
        if a == b { format!("bar {a}") } else { format!("bars {a}-{b}") }
    };

    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "// {} -- {}", piece.label, piece.composer);
    let _ = writeln!(w, "// {}", piece.work);
    let _ = writeln!(w, "//");
    for line in piece.credit.iter().filter(|l| !l.is_empty()) {
        let _ = writeln!(w, "// {line}");
    }
    let _ = writeln!(w, "//");
    for line in &piece.about {
        let _ = writeln!(w, "// {line}");
    }
    let _ = writeln!(w, "//");
    let _ = writeln!(w, "// Reading it: one modelled grand plays the note lists below, merged. An entry is");
    let _ = writeln!(w, "// [beat note dur vel]: beats are quarter notes from the start, dur in quarters, vel");
    let _ = writeln!(w, "// the key's velocity 0..1. Each bar's comment names its dynamic (pp < 0.31 <= p <");
    let _ = writeln!(w, "// 0.44 <= mp < 0.57 <= mf < 0.69 <= f < 0.82 <= ff). [beat @pedal 1] puts the sustain");
    let _ = writeln!(w, "// pedal down, [beat @pedal 0] lifts it. Each Section's `tempo` is the performer's");
    let _ = writeln!(w, "// rubato: [beat bpm] from the section's first beat, held to the next point.");
    let _ = writeln!(w, "use mod.score.*");
    let _ = writeln!(w);
    let _ = writeln!(w, "let clock = Clock{{bpm: {} sig: [{} {}]}}", num(clock_bpm, 0), num0, den0);
    let _ = writeln!(w, "let piano = Instrument{{kind: @piano}}");
    let _ = writeln!(w);

    // The voices.
    for (v, spec) in piece.voices.iter().enumerate() {
        let notes = &perf.voices[v];
        let _ = writeln!(w, "/// {}", spec.about);
        let _ = writeln!(w, "let {} = [", spec.name);
        let mut i = 0;
        let mut last_section = usize::MAX;
        let mut level = None;
        while i < notes.len() {
            let m = perf.measure_of(notes[i].on);
            let (start, len, _) = perf.measures[m];
            let j = i + notes[i..].iter().take_while(|n| n.on < start + len).count();
            let bar = &notes[i..j];
            let s = section_of(m);
            let spelling = sections[s].2.spelling.unwrap_or(piece.spelling);
            if s != last_section {
                let (from, to, spec) = sections[s];
                let _ = writeln!(w, "    // -- {}: {} ({}) --", spec.name, spec.about, bars_label(from, to));
                last_section = s;
            }
            let mean = bar.iter().map(|n| n.vel as f64).sum::<f64>() / bar.len() as f64;
            let now = next_mark(level, mean);
            level = Some(now);
            let _ = writeln!(w, "    // bar {} {}", bar_number(piece, m), MARKS[now].1);
            for line in bar.chunks(PER_LINE) {
                let entries: Vec<String> = line.iter().map(|n| {
                    format!("[{} @{} {} {}]", num(perf.beat(n.on), 4), note_name(n.key, spelling), num(perf.beat(n.off - n.on), 4), num(n.vel as f64 / 127.0, 2))
                }).collect();
                let _ = writeln!(w, "    {}", entries.join(" "));
            }
            i = j;
        }
        let _ = writeln!(w, "]");
        let _ = writeln!(w);
    }
    // The pedal.
    let _ = writeln!(w, "/// The sustain pedal: 1 down, 0 up (between is half-pedal).");
    let _ = writeln!(w, "let pedal = [");
    let mut i = 0;
    while i < perf.pedal.len() {
        let m = perf.measure_of(perf.pedal[i].0);
        let (start, len, _) = perf.measures[m];
        let j = i + perf.pedal[i..].iter().take_while(|p| p.0 < start + len).count();
        let entries: Vec<String> = perf.pedal[i..j].iter().map(|(tick, value)| format!("[{} @pedal {}]", num(perf.beat(*tick), 4), num(*value as f64 / 127.0, 2))).collect();
        let _ = writeln!(w, "    // bar {}", bar_number(piece, m));
        for line in entries.chunks(PER_LINE * 2) {
            let _ = writeln!(w, "    {}", line.join(" "));
        }
        i = j;
    }
    let _ = writeln!(w, "]");
    let _ = writeln!(w);
    let names: Vec<&str> = piece.voices.iter().map(|v| v.name).chain(["pedal"]).collect();
    let _ = writeln!(w, "/// The whole performance, once through (the sections below shape its time).");
    let _ = writeln!(w, "let performance = Loop{{play: piano amp: 1 beats: {} notes: [{}]}}", num(total_beats, 4), names.join(" "));
    let _ = writeln!(w);

    // The sections and their tempo.
    for (from, to, spec) in &sections {
        let start = perf.beat(perf.measures[*from].0);
        let end = perf.beat(if *to <= last_measure { perf.measures[*to].0 } else { music_end });
        let _ = writeln!(w, "/// {}: {}", bars_label(*from, *to), spec.about);
        let _ = writeln!(w, "let {} = Section{{bars: {} loops: [performance] tempo: [", spec.name, bars_text(end - start, per_bar));
        let mine: Vec<&(f64, f64)> = points.iter().filter(|p| p.0 >= start - 1e-9 && p.0 < end - 1e-9).collect();
        // The tempo playing when the section starts, if no point is on it.
        let mut lines: Vec<(usize, Vec<String>)> = Vec::new();
        if mine.first().is_none_or(|p| p.0 > start + 1e-9) {
            let before = points.iter().rev().find(|p| p.0 <= start + 1e-9).map_or(points[0].1, |p| p.1);
            lines.push((*from, vec![format!("[0 {}]", num(before, 3))]));
        }
        for p in mine {
            let m = perf.measure_of((p.0 * perf.ppq).round() as u64);
            let entry = format!("[{} {}]", num(p.0 - start, 4), num(p.1, 3));
            match lines.last_mut() {
                Some((lm, entries)) if *lm == m => entries.push(entry),
                _ => lines.push((m, vec![entry])),
            }
        }
        for (m, entries) in lines {
            let _ = writeln!(w, "    // bar {}", bar_number(piece, m));
            for line in entries.chunks(PER_LINE * 2) {
                let _ = writeln!(w, "    {}", line.join(" "));
            }
        }
        let _ = writeln!(w, "]}}");
        let _ = writeln!(w);
    }
    let _ = writeln!(w, "/// The last chord rings out.");
    let _ = writeln!(w, "let ring = Section{{bars: 1 loops: [performance] next: @stop tempo: [[0 {}]]}}", num(ring_bpm, 3));
    let _ = writeln!(w);
    let arrange: Vec<&str> = sections.iter().map(|s| s.2.name).chain(["ring"]).collect();
    let rights = piece.rights.as_ref().map_or(String::new(), |r| {
        format!("\n    rights: {{license: {:?} credits: {:?} source: {:?} terms: {:?}}}", r.license, r.credits, r.source, r.terms)
    });
    let _ = writeln!(w, "Score{{label: \"{}\"{rights}\n    arrange: [{}]", piece.label, arrange.join(" "));
    let _ = writeln!(w, "    clock piano {} performance", names.join(" "));
    let _ = writeln!(w, "    {}}}", arrange.join(" "));
    Ok(out)
}

/// How closely a score plays a performance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Check {
    pub notes: usize,
    pub pedal_moves: usize,
    /// The largest differences from the file, in seconds, after the
    /// shortened lead-in: note starts, note ends, pedal moves.
    pub onset: f64,
    pub release: f64,
    pub pedal: f64,
    /// The largest velocity difference in MIDI steps.
    pub velocity: u8,
    /// The score's length to the last note's end, in seconds.
    pub seconds: f64,
}

/// Play `source` through the composer and compare it with the file.
pub fn check(bytes: &[u8], piece: &Piece, source: &str) -> Result<Check, String> {
    let (_, perf) = read(bytes, piece)?;
    let loaded = crate::model::load(source).map_err(|d| d.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n"))?;
    let mut composer = Composer::new(loaded);
    let mut events = Vec::new();
    composer.pump(perf.beat(perf.end) + 64.0, &mut events);
    let mut ours: Vec<(u8, f64, f64, u8)> = Vec::new();
    let mut pedal: Vec<(f64, u8)> = Vec::new();
    for e in &events {
        match e.kind {
            EventKind::Note => ours.push((e.note.round() as u8, composer.seconds_at(e.beat), composer.seconds_at(e.beat + e.dur as f64), (e.vel * 127.0).round() as u8)),
            EventKind::Pedal => pedal.push((composer.seconds_at(e.beat), (e.vel * 127.0).round() as u8)),
            _ => {}
        }
    }
    let mut theirs: Vec<(u8, f64, f64, u8)> = perf.voices.iter().flatten().map(|n| (n.key, perf.tempo.ticks_to_seconds(n.on), perf.tempo.ticks_to_seconds(n.off), n.vel)).collect();
    if ours.len() != theirs.len() {
        return Err(format!("{} notes play, the file has {}", ours.len(), theirs.len()));
    }
    let order = |a: &(u8, f64, f64, u8), b: &(u8, f64, f64, u8)| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1));
    ours.sort_by(order);
    theirs.sort_by(order);
    let lead = perf.tempo.ticks_to_seconds(perf.first_onset()) - ours.iter().map(|n| n.1).fold(f64::INFINITY, f64::min);
    let mut c = Check { notes: ours.len(), pedal_moves: pedal.len(), ..Check::default() };
    for (a, b) in ours.iter().zip(&theirs) {
        if a.0 != b.0 {
            return Err(format!("note {} plays where the file has {}", a.0, b.0));
        }
        c.onset = c.onset.max((a.1 + lead - b.1).abs());
        c.release = c.release.max((a.2 + lead - b.2).abs());
        c.velocity = c.velocity.max(a.3.abs_diff(b.3));
        c.seconds = c.seconds.max(a.2);
    }
    if pedal.len() != perf.pedal.len() {
        return Err(format!("{} pedal moves play, the file has {}", pedal.len(), perf.pedal.len()));
    }
    for (a, b) in pedal.iter().zip(&perf.pedal) {
        c.pedal = c.pedal.max((a.0 + lead - perf.tempo.ticks_to_seconds(b.0)).abs());
        c.velocity = c.velocity.max(a.1.abs_diff(b.1));
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_marks_and_numbers() {
        assert_eq!(note_name(60, Spelling::Sharps), "c4");
        assert_eq!(note_name(75, Spelling::Sharps), "ds5");
        assert_eq!(note_name(75, Spelling::Flats), "eb5");
        assert_eq!(note_name(21, Spelling::Flats), "a0");
        for key in 21..=108u8 {
            for spelling in [Spelling::Sharps, Spelling::Flats] {
                assert_eq!(crate::theory::note_number(&note_name(key, spelling)), Some(key as f32), "{}", note_name(key, spelling));
            }
        }
        assert_eq!(mark(30.0), "pp");
        assert_eq!(mark(64.0), "mp");
        assert_eq!(mark(120.0), "ff");
        // p (40..56) holds at 38, moves to pp at 35.
        assert_eq!(next_mark(Some(1), 38.0), 1);
        assert_eq!(next_mark(Some(1), 35.0), 0);
        assert_eq!(next_mark(Some(1), 58.0), 1);
        assert_eq!(next_mark(Some(1), 61.0), 2);
        assert_eq!(num(1.50, 4), "1.5");
        assert_eq!(num(2.0, 4), "2");
        assert_eq!(num(1.0 / 3.0, 4), "0.3333");
        assert_eq!(bars_text(12.0, 6.0), "2");
        assert_eq!(bars_text(32.0, 6.0), "5 + 1/3");
    }
}

#[cfg(test)]
mod lists {
    use makepad_score_player::EventKind;

    #[test]
    fn hands_merge_and_pedal_reads() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 60}\nlet piano = Instrument{kind: @piano}\nlet rh = [\n    // bar 1\n    [0 @c4 1 0.5] [1 @e4 1 0.5]\n]\nlet lh = [[0 @c3 2 0.4]]\nlet pedal = [[0 @pedal 1] [1.5 @pedal 0.5]]\nlet p = Loop{play: piano amp: 1 beats: 4 notes: [rh lh pedal]}\nlet a = Section{bars: 1 + 1/2 loops: [p] tempo: [[0 60] [1 8]]}\nScore{arrange: [a] clock piano p a}";
        let loaded = crate::model::load(src).unwrap_or_else(|d| panic!("{d:?}"));
        let notes = loaded.doc.loops[0].notes.clone().unwrap();
        assert_eq!(notes.len(), 5);
        assert_eq!(loaded.doc.sections[0].bars, 1.5);
        assert_eq!(loaded.doc.sections[0].tempo, vec![(0.0, 60.0), (1.0, 8.0)]);
    }

    #[test]
    fn section_tempo_points_bend_time_and_code_moves_the_pedal() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet piano = Instrument{kind: @piano}\nlet p = Loop{play: piano beats: 4 code: fn(i) { play(@c4) pedal(1) sleep(1) play(@e4) pedal(0.5) sleep(1) pedal(0) play(@g4) sleep(2) }}\nlet a = Section{bars: 1 loops: [p] tempo: [[0 60] [1 30] [3 6]]}\nlet b = Section{bars: 1 loops: [p] next: @stop tempo: [[2 120]]}\nScore{arrange: [a b] clock piano p a b}";
        let loaded = crate::model::load(src).unwrap_or_else(|d| panic!("{d:?}"));
        let mut composer = crate::compose::Composer::new(loaded);
        let mut events = Vec::new();
        composer.pump(16.0, &mut events);
        let pedal: Vec<(f64, f32)> = events.iter().filter(|e| e.kind == EventKind::Pedal).map(|e| (e.beat, e.vel)).collect();
        assert_eq!(pedal, vec![(0.0, 1.0), (1.0, 0.5), (2.0, 0.0), (4.0, 1.0), (5.0, 0.5), (6.0, 0.0)]);
        // 1 s at 60, 2 beats at 30, a beat at 6 (a fermata), then b's
        // first two beats at 6 and the rest at 120.
        let at = |beat: f64| composer.seconds_at(beat);
        assert!((at(1.0) - 1.0).abs() < 1e-9 && (at(3.0) - 5.0).abs() < 1e-9 && (at(4.0) - 15.0).abs() < 1e-9);
        assert!((at(6.0) - 35.0).abs() < 1e-9 && (at(7.0) - 35.5).abs() < 1e-9, "{}", at(7.0));
        // After `next: @stop` nothing plays, and the sections before it
        // still answer.
        assert!(events.iter().all(|e| e.beat < 8.0));
        assert_eq!(composer.section_at(0.5).as_deref(), Some("a"));
    }
}
