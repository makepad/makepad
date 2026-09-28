//! Scores and the sequencer that plays them.
//!
//! A [`Score`] is a list of notes in beats; a [`Sequencer`] turns it into
//! sample-accurate note-on/off events for any [`crate::Instrument`] on the
//! audio thread, without allocating.
//!
//! # The compact text format
//!
//! Made to be written by hand (or by an AI composing a game's music) in one
//! line. Tokens are separated by whitespace:
//!
//! | token | meaning |
//! |---|---|
//! | `bpm=120` | tempo (default 120) |
//! | `loop` | repeat forever |
//! | `len=16` | explicit length in beats (default: rounded up to whole 4-beat bars) |
//! | `vel=90` | velocity 1..127 for the following notes (default 100) |
//! | `C4` `F#5` `Bb3` `60` | a note by name (C4 = middle C = 60) or MIDI number |
//! | `C4:2` `E4:0.5` `G4:1/2` | a note lasting 2 / ½ / ½ beats |
//! | `r` `-` `r:2` | a rest |
//! | `[C4 E4 G4]:2` | a chord |
//! | `x` `x:1/2` | repeat the previous note or chord |
//! | `kick` `snare` `hat` ... | drum names (General MIDI) for drum kits |
//! | `\|` | bar line (ignored, for readability) |
//! | `;` | starts a PARALLEL track (also from beat 0) |
//!
//! A duration, once given, sticks: `C4:1/2 D4 E4` is three half-beat notes.
//! Drum names: `kick` 36, `snare` 38, `rim`/`side` 37, `clap` 39,
//! `hat`/`hihat` 42, `openhat`/`ohat` 46, `pedalhat` 44, `tom1`/`hitom` 50,
//! `tom2`/`midtom` 47, `tom3`/`lowtom` 45, `floortom` 43, `crash` 49,
//! `ride` 51, `bell`/`ridebell` 53.
//!
//! Examples:
//! - melody: `bpm=140 loop C5:1/2 E5 G5 E5 | C5:1 r:1`
//! - chords with a bass line: `bpm=90 loop [C4 E4 G4]:4 | [A3 C4 E4]:4 ; C2:2 G2 | A1:2 E2`
//! - a beat: `bpm=110 loop kick:1 snare kick kick:1/2 x snare:1 ; hat:1/2 x x x x x x x`
//!
//! Anything the parser does not know is an error naming the token: a typo
//! is reported, never played as something else.

use crate::dsp::note_number;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScoreNote {
    pub beat: f32,
    pub dur: f32,
    pub note: u8,
    pub vel: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub bpm: f32,
    /// Sorted by beat.
    pub notes: Vec<ScoreNote>,
    pub length_beats: f32,
    pub looping: bool,
}

/// General MIDI percussion numbers by name.
pub fn drum_note(name: &str) -> Option<u8> {
    Some(match name {
        "kick" | "bd" => 36,
        "snare" | "sd" => 38,
        "rim" | "side" | "sidestick" => 37,
        "clap" => 39,
        "hat" | "hihat" | "hh" => 42,
        "openhat" | "ohat" | "oh" => 46,
        "pedalhat" => 44,
        "tom1" | "hitom" => 50,
        "tom2" | "midtom" => 47,
        "tom3" | "lowtom" => 45,
        "floortom" => 43,
        "crash" => 49,
        "ride" => 51,
        "bell" | "ridebell" => 53,
        _ => return None,
    })
}

fn parse_note(token: &str) -> Option<u8> {
    let lower = token.to_ascii_lowercase();
    if let Some(n) = drum_note(&lower) {
        return Some(n);
    }
    if let Ok(n) = token.parse::<u32>() {
        return (n <= 127).then_some(n as u8);
    }
    note_number(token)
}

fn parse_dur(text: &str) -> Option<f32> {
    let v = match text.split_once('/') {
        Some((a, b)) => {
            let a: f32 = a.trim().parse().ok()?;
            let b: f32 = b.trim().parse().ok()?;
            if b == 0.0 {
                return None;
            }
            a / b
        }
        None => text.trim().parse().ok()?,
    };
    (v.is_finite() && v > 0.0 && v <= 1024.0).then_some(v)
}

fn round_up_bars(end: f32) -> f32 {
    ((end / 4.0).ceil() * 4.0).max(4.0)
}

impl Score {
    /// Parse the compact text format (see the module docs).
    pub fn parse(text: &str) -> Result<Score, String> {
        let mut bpm = 120.0f32;
        let mut looping = false;
        let mut explicit_len: Option<f32> = None;
        let mut notes = Vec::new();
        for track in text.split(';') {
            let mut t = 0.0f32;
            let mut dur = 1.0f32;
            let mut vel = 100u8;
            let mut prev: Vec<u8> = Vec::new();
            let mut chord: Option<Vec<u8>> = None;
            for token in track.split_whitespace() {
                if token == "|" {
                    continue;
                }
                if let Some(v) = token.strip_prefix("bpm=") {
                    bpm = v.parse::<f32>().ok().filter(|b| b.is_finite() && *b >= 1.0 && *b <= 999.0)
                        .ok_or_else(|| format!("bad tempo '{token}' (bpm=1..999)"))?;
                    continue;
                }
                if let Some(v) = token.strip_prefix("vel=") {
                    vel = v.parse::<f32>().ok().filter(|v| v.is_finite())
                        .map(|v| v.clamp(1.0, 127.0) as u8)
                        .ok_or_else(|| format!("bad velocity '{token}' (vel=1..127)"))?;
                    continue;
                }
                if let Some(v) = token.strip_prefix("len=") {
                    explicit_len = Some(parse_dur(v).ok_or_else(|| format!("bad length '{token}'"))?);
                    continue;
                }
                if token == "loop" {
                    looping = true;
                    continue;
                }
                // Chords: `[C4 E4 G4]:2` arrives as `[C4`, `E4`, `G4]:2`.
                let mut body = token;
                let opens = body.starts_with('[');
                if opens {
                    if chord.is_some() {
                        return Err(format!("nested chord at '{token}'"));
                    }
                    chord = Some(Vec::new());
                    body = &body[1..];
                }
                if let Some(ch) = chord.as_mut() {
                    let (inner, closes, rest) = match body.find(']') {
                        Some(i) => (&body[..i], true, &body[i + 1..]),
                        None => (body, false, ""),
                    };
                    if !inner.is_empty() {
                        let n = parse_note(inner).ok_or_else(|| format!("unknown note '{inner}' in chord"))?;
                        ch.push(n);
                    }
                    if closes {
                        if let Some(d) = rest.strip_prefix(':') {
                            dur = parse_dur(d).ok_or_else(|| format!("bad duration in '{token}'"))?;
                        } else if !rest.is_empty() {
                            return Err(format!("unexpected '{rest}' after chord in '{token}'"));
                        }
                        let ch = chord.take().unwrap_or_default();
                        if ch.is_empty() {
                            return Err(format!("empty chord at '{token}'"));
                        }
                        for &n in &ch {
                            notes.push(ScoreNote { beat: t, dur, note: n, vel });
                        }
                        prev = ch;
                        t += dur;
                    }
                    continue;
                }
                let (name, d) = match body.split_once(':') {
                    Some((n, d)) => (n, Some(d)),
                    None => (body, None),
                };
                if let Some(d) = d {
                    dur = parse_dur(d).ok_or_else(|| format!("bad duration in '{token}'"))?;
                }
                match name {
                    "r" | "-" => t += dur,
                    "x" => {
                        if prev.is_empty() {
                            return Err(format!("'{token}' repeats nothing"));
                        }
                        for &n in &prev {
                            notes.push(ScoreNote { beat: t, dur, note: n, vel });
                        }
                        t += dur;
                    }
                    _ => {
                        let n = parse_note(name).ok_or_else(|| format!("unknown token '{token}'"))?;
                        notes.push(ScoreNote { beat: t, dur, note: n, vel });
                        prev.clear();
                        prev.push(n);
                        t += dur;
                    }
                }
            }
            if chord.is_some() {
                return Err("unclosed chord '['".into());
            }
        }
        let mut score = Score::from_notes(bpm, notes, looping);
        if let Some(len) = explicit_len {
            score.length_beats = len;
        }
        Ok(score)
    }

    /// A score from explicit notes: sorted, and as long as the last note's
    /// end rounded up to a whole 4-beat bar.
    pub fn from_notes(bpm: f32, mut notes: Vec<ScoreNote>, looping: bool) -> Score {
        notes.retain(|n| n.beat.is_finite() && n.dur.is_finite() && n.beat >= 0.0);
        for n in notes.iter_mut() {
            n.dur = n.dur.max(1.0e-3);
            n.note = n.note.min(127);
            n.vel = n.vel.clamp(1, 127);
        }
        notes.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));
        let end = notes.iter().map(|n| n.beat + n.dur).fold(0.0f32, f32::max);
        let bpm = if bpm.is_finite() { bpm.clamp(1.0, 999.0) } else { 120.0 };
        Score { bpm, notes, length_beats: round_up_bars(end), looping }
    }

    pub fn duration_secs(&self) -> f32 {
        self.length_beats * 60.0 / self.bpm
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeqEvent {
    On { note: u8, vel: u8 },
    Off { note: u8 },
}

/// Most notes one sequencer holds at once; past this the earliest-ending
/// note is released early instead of the table growing.
pub const MAX_HELD: usize = 128;

pub struct Sequencer {
    score: Score,
    bpm: f32,
    beat: f64,
    next: usize,
    held: [(f64, u8); MAX_HELD],
    held_count: usize,
    stopped: bool,
}

impl Sequencer {
    pub fn new(score: Score) -> Self {
        Sequencer {
            bpm: score.bpm,
            score,
            beat: 0.0,
            next: 0,
            held: [(0.0, 0); MAX_HELD],
            held_count: 0,
            stopped: false,
        }
    }

    pub fn score(&self) -> &Score {
        &self.score
    }

    pub fn set_bpm(&mut self, bpm: f32) {
        if bpm.is_finite() {
            self.bpm = bpm.clamp(1.0, 999.0);
        }
    }

    /// Beats played so far within the current pass.
    pub fn beat(&self) -> f64 {
        self.beat
    }

    pub fn finished(&self) -> bool {
        self.stopped || (!self.looping() && self.next >= self.score.notes.len() && self.held_count == 0)
    }

    fn looping(&self) -> bool {
        self.score.looping && self.score.length_beats > 1.0e-3
    }

    fn remove_held(&mut self, index: usize) {
        self.held_count -= 1;
        self.held[index] = self.held[self.held_count];
    }

    fn earliest_held(&self) -> Option<usize> {
        (0..self.held_count).min_by(|&a, &b| {
            self.held[a].0.partial_cmp(&self.held[b].0).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Advance by `frames` at `rate`, emitting every event in the window at
    /// its frame offset, in time order. Offs come before ons at one instant
    /// (a repeated note re-strikes cleanly).
    pub fn advance(&mut self, frames: usize, rate: f32, emit: &mut dyn FnMut(usize, SeqEvent)) {
        if self.stopped || frames == 0 {
            return;
        }
        let bpf = self.bpm as f64 / 60.0 / rate.max(1.0) as f64;
        // `origin` is the beat at frame 0 of this window; a loop shifts it.
        let mut origin = self.beat;
        let mut end = self.beat + frames as f64 * bpf;
        let offset = |beat: f64, origin: f64| -> usize {
            (((beat - origin) / bpf + 1.0e-6).floor().max(0.0) as usize).min(frames - 1)
        };
        loop {
            let on_beat = self.score.notes.get(self.next).map(|n| n.beat as f64);
            let off = self.earliest_held().map(|i| (i, self.held[i].0));
            let loop_at = self.looping().then_some(self.score.length_beats as f64);
            // Earliest of: off, loop point, on (in that priority at ties).
            let mut best: Option<(f64, u8)> = None; // (beat, kind 0=off 1=loop 2=on)
            let mut consider = |b: Option<f64>, kind: u8| {
                if let Some(b) = b {
                    if best.map_or(true, |(bb, bk)| b < bb || (b == bb && kind < bk)) {
                        best = Some((b, kind));
                    }
                }
            };
            consider(off.map(|o| o.1), 0);
            consider(loop_at, 1);
            // A note at or past the loop point never plays.
            consider(on_beat.filter(|b| loop_at.map_or(true, |l| *b < l)), 2);
            let Some((at, kind)) = best else { break };
            if at >= end {
                break;
            }
            let at_frame = offset(at, origin);
            match kind {
                0 => {
                    let (i, _) = off.unwrap_or((0, 0.0));
                    let note = self.held[i].1;
                    self.remove_held(i);
                    emit(at_frame, SeqEvent::Off { note });
                }
                1 => {
                    while self.held_count > 0 {
                        let note = self.held[self.held_count - 1].1;
                        self.held_count -= 1;
                        emit(at_frame, SeqEvent::Off { note });
                    }
                    let len = self.score.length_beats as f64;
                    origin -= len;
                    end -= len;
                    self.next = 0;
                }
                _ => {
                    let n = self.score.notes[self.next];
                    self.next += 1;
                    // Re-strike: release a still-held copy of this note.
                    if let Some(i) = (0..self.held_count).find(|&i| self.held[i].1 == n.note) {
                        self.remove_held(i);
                        emit(at_frame, SeqEvent::Off { note: n.note });
                    }
                    if self.held_count == MAX_HELD {
                        if let Some(i) = self.earliest_held() {
                            let note = self.held[i].1;
                            self.remove_held(i);
                            emit(at_frame, SeqEvent::Off { note });
                        }
                    }
                    self.held[self.held_count] = (at + n.dur as f64, n.note);
                    self.held_count += 1;
                    emit(at_frame, SeqEvent::On { note: n.note, vel: n.vel });
                }
            }
        }
        self.beat = end;
    }

    /// Release every held note now and end the sequence.
    pub fn stop(&mut self, emit: &mut dyn FnMut(usize, SeqEvent)) {
        while self.held_count > 0 {
            self.held_count -= 1;
            emit(0, SeqEvent::Off { note: self.held[self.held_count].1 });
        }
        self.next = self.score.notes.len();
        self.stopped = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(seq: &mut Sequencer, frames: usize, rate: f32) -> Vec<(usize, SeqEvent)> {
        let mut out = Vec::new();
        seq.advance(frames, rate, &mut |f, e| out.push((f, e)));
        out
    }

    #[test]
    fn parses_notes_durations_rests_and_sticky_lengths() {
        let s = Score::parse("bpm=90 C4:1/2 D4 r E4:2 60 x").unwrap();
        assert_eq!(s.bpm, 90.0);
        let beats: Vec<(f32, u8, f32)> = s.notes.iter().map(|n| (n.beat, n.note, n.dur)).collect();
        assert_eq!(beats, vec![(0.0, 60, 0.5), (0.5, 62, 0.5), (1.5, 64, 2.0), (3.5, 60, 2.0), (5.5, 60, 2.0)]);
        assert_eq!(s.length_beats, 8.0);
        assert!(!s.looping);
    }

    #[test]
    fn chords_and_parallel_tracks_start_together() {
        let s = Score::parse("loop [C4 E4 G4]:2 x ; C2:4 | vel=60 kick:1/2").unwrap();
        assert!(s.looping);
        let at0: Vec<u8> = s.notes.iter().filter(|n| n.beat == 0.0).map(|n| n.note).collect();
        assert_eq!(at0.len(), 4);
        assert!(at0.contains(&36) && at0.contains(&60));
        assert_eq!(s.notes.iter().filter(|n| n.beat == 2.0).count(), 3);
        let kick = s.notes.iter().find(|n| n.note == 36 && n.beat > 0.0).unwrap();
        assert_eq!((kick.beat, kick.vel), (4.0, 60));
    }

    #[test]
    fn typos_are_errors_naming_the_token() {
        assert!(Score::parse("C4 Q9").unwrap_err().contains("Q9"));
        assert!(Score::parse("[C4 E4").is_err());
        assert!(Score::parse("C4:0").is_err());
        assert!(Score::parse("x").is_err());
        assert!(Score::parse("bpm=fast").is_err());
    }

    #[test]
    fn events_are_sample_accurate() {
        // 120 bpm at 48 kHz: one beat = 24000 frames.
        let s = Score::parse("r:1 A4:1").unwrap();
        let mut seq = Sequencer::new(s);
        let ev = collect(&mut seq, 48_000 * 2, 48_000.0);
        assert_eq!(ev, vec![(24_000, SeqEvent::On { note: 69, vel: 100 }), (48_000, SeqEvent::Off { note: 69 })]);
        // Same answer in small blocks.
        let mut seq = Sequencer::new(Score::parse("r:1 A4:1").unwrap());
        let mut abs = Vec::new();
        for block in 0..(48_000 * 2 / 256) {
            seq.advance(256, 48_000.0, &mut |f, e| abs.push((block * 256 + f, e)));
        }
        assert_eq!(abs[0].0, 24_000);
        assert_eq!(abs[1].0, 48_000);
        assert!(seq.finished());
    }

    #[test]
    fn a_loop_releases_held_notes_at_the_loop_point_and_restarts() {
        // A note held past the bar end is cut at the loop point.
        let s = Score::parse("loop len=4 C4:8").unwrap();
        let mut seq = Sequencer::new(s);
        let ev = collect(&mut seq, 24_000 * 9, 48_000.0);
        assert_eq!(ev[0], (0, SeqEvent::On { note: 60, vel: 100 }));
        assert_eq!(ev[1], (96_000, SeqEvent::Off { note: 60 }));
        assert_eq!(ev[2], (96_000, SeqEvent::On { note: 60, vel: 100 }));
        assert_eq!(ev[3], (192_000, SeqEvent::Off { note: 60 }));
        assert!(!seq.finished());
    }

    #[test]
    fn every_midi_note_can_be_held_at_once_without_growing() {
        // Held notes are unique per note number (a re-strike releases the
        // old copy first), so the fixed table of MAX_HELD = 128 always fits;
        // the early-release path is a guard, never reached by a real score.
        let mut notes: Vec<ScoreNote> =
            (0..=127u8).map(|i| ScoreNote { beat: 0.0, dur: 4.0, note: i, vel: 90 }).collect();
        notes.push(ScoreNote { beat: 0.0, dur: 4.0, note: 5, vel: 90 });
        let mut seq = Sequencer::new(Score::from_notes(120.0, notes, false));
        let ev = collect(&mut seq, 256, 48_000.0);
        let ons = ev.iter().filter(|e| matches!(e.1, SeqEvent::On { .. })).count();
        let offs: Vec<_> = ev.iter().filter(|e| matches!(e.1, SeqEvent::Off { .. })).collect();
        assert_eq!(ons, 129);
        assert_eq!(offs.len(), 1, "only the re-struck note is released");
        assert_eq!(offs[0].1, SeqEvent::Off { note: 5 });
        let mut rest = Vec::new();
        seq.stop(&mut |f, e| rest.push((f, e)));
        assert_eq!(rest.len(), 128);
        assert!(seq.finished());
    }
}
