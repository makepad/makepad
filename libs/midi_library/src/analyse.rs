//! A MIDI file read into parts (a track's notes on one channel) with what
//! a cover needs: each note's velocity, expression and pitch bend at its
//! start, the sustain pedal, the part's program, volume and pan, its role;
//! the tempo map and the metre. And what the index lists: tempo, key,
//! length, roles and a quality score.

use crate::gm::{self, Role};
use makepad_midi_file::{ChannelMessage, Division, EventKind, MetaEvent, TempoMap};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub on: u64,
    pub off: u64,
    pub key: u8,
    pub vel: u8,
    /// Expression (CC 11) when it starts, 0..1.
    pub expr: f32,
    /// Pitch bend when it starts, in semitones.
    pub bend: f32,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub track: usize,
    pub channel: u8,
    /// The track's name, if it has one.
    pub name: String,
    /// The program when its first note plays.
    pub program: u8,
    /// Program changes after its first note: (tick, program).
    pub changes: Vec<(u64, u8)>,
    pub drums: bool,
    pub notes: Vec<Note>,
    /// Sustain pedal moves (tick, 0..127), redundant ones dropped.
    pub pedal: Vec<(u64, u8)>,
    /// Channel volume (CC 7) the part mostly plays at.
    pub volume: u8,
    /// Pan (CC 10) when it starts: 0 left, 64 centre, 127 right.
    pub pan: u8,
    /// Modulation wheel (CC 1) moves (not played by a cover).
    pub mod_moves: usize,
    /// Notes that start under a pitch bend.
    pub bent: usize,
    pub role: Role,
    /// The tune: the part the ear follows (a sung line in the original).
    pub melody: bool,
}

#[derive(Clone, Debug)]
pub struct Song {
    pub ppq: u32,
    pub tempo: TempoMap,
    /// Time signatures as (tick, numerator, denominator).
    pub sigs: Vec<(u64, u8, u32)>,
    /// The key signature meta, if the file has one: (sharps or flats, minor).
    pub key_sig: Option<(i8, bool)>,
    pub parts: Vec<Part>,
    /// The tick the last note ends.
    pub end: u64,
}

impl Song {
    pub fn seconds(&self) -> f64 {
        self.tempo.ticks_to_seconds(self.end)
    }

    pub fn beat(&self, tick: u64) -> f64 {
        tick as f64 / self.ppq as f64
    }

    pub fn sig_at(&self, tick: u64) -> (u8, u32) {
        let i = self.sigs.partition_point(|s| s.0 <= tick).saturating_sub(1);
        self.sigs.get(i).map_or((4, 4), |s| (s.1, s.2))
    }

    /// The measures as (start tick, length in ticks) to the end.
    pub fn measures(&self) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut tick = 0u64;
        while tick < self.end.max(1) {
            let (num, den) = self.sig_at(tick);
            let len = ((self.ppq as f64 * 4.0 * num as f64 / den as f64).round() as u64).max(1);
            out.push((tick, len));
            tick += len;
        }
        out
    }

    /// The bpm at `tick`.
    pub fn bpm_at(&self, tick: u64) -> f64 {
        let i = self.tempo.changes.partition_point(|c| c.tick <= tick).saturating_sub(1);
        self.tempo.changes.get(i).map_or(120.0, |c| 60e6 / c.microseconds_per_quarter.max(1) as f64)
    }

    /// The tempo the song mostly plays at (by time).
    pub fn main_bpm(&self) -> f64 {
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for (i, c) in self.tempo.changes.iter().enumerate() {
            let from = c.tick.min(self.end);
            let to = self.tempo.changes.get(i + 1).map_or(self.end, |n| n.tick.min(self.end));
            if to > from {
                spans.push((60e6 / c.microseconds_per_quarter.max(1) as f64, (to - from) as f64));
            }
        }
        if spans.is_empty() {
            return self.bpm_at(0);
        }
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let half = spans.iter().map(|s| s.1).sum::<f64>() / 2.0;
        let mut acc = 0.0;
        spans.iter().find(|s| {
            acc += s.1;
            acc >= half
        }).map_or(120.0, |s| s.0)
    }
}

#[derive(Clone, Copy, Default)]
struct ChannelState {
    program: u8,
    volume: u8,
    expression: u8,
    pan: u8,
    /// 14-bit, 8192 centre.
    bend: u16,
    /// Bend range in semitones (RPN 0).
    range: f32,
    rpn: (u8, u8),
    sustain: u8,
}

/// Read a Standard MIDI File into parts. SMPTE-timed files are refused.
pub fn read(bytes: &[u8]) -> Result<Song, String> {
    let options = makepad_midi_file::ParseOptions { require_end_of_track: false, require_declared_track_count: false };
    let file = makepad_midi_file::parse_with_options(bytes, options).map_err(|e| format!("{e:?}"))?;
    let ppq = match file.header.division {
        Division::TicksPerQuarter(ppq) if ppq > 0 => ppq as u32,
        _ => return Err("SMPTE-timed MIDI files are not read".into()),
    };
    let tempo = file.tempo_map().map_err(|e| format!("{e:?}"))?;
    // Every event of every track, in time order (a stable sort keeps each
    // track's own order at one tick).
    let mut events: Vec<(u64, usize, &EventKind)> = Vec::new();
    let mut names: Vec<String> = vec![String::new(); file.tracks.len()];
    let mut sigs: Vec<(u64, u8, u32)> = Vec::new();
    let mut key_sig = None;
    for (t, track) in file.tracks.iter().enumerate() {
        for e in &track.events {
            match &e.kind {
                EventKind::Meta(MetaEvent::SequenceOrTrackName(name)) if names[t].is_empty() => {
                    names[t] = String::from_utf8_lossy(name).trim().to_string();
                }
                EventKind::Meta(MetaEvent::TimeSignature(sig)) => {
                    if let Some(den) = sig.denominator().filter(|d| *d > 0 && *d <= 64 && sig.numerator > 0) {
                        sigs.push((e.tick, sig.numerator, den));
                    }
                }
                EventKind::Meta(MetaEvent::KeySignature(k)) if key_sig.is_none() => key_sig = Some((k.sharps_flats, k.is_minor)),
                EventKind::Channel(_) => events.push((e.tick, t, &e.kind)),
                _ => {}
            }
        }
    }
    events.sort_by_key(|e| e.0);
    sigs.sort_by_key(|s| s.0);
    sigs.dedup_by(|b, a| a.0 == b.0 && {
        *a = *b;
        true
    });
    if sigs.first().is_none_or(|s| s.0 > 0) {
        sigs.insert(0, (0, 4, 4));
    }
    let mut state = [ChannelState { volume: 100, expression: 127, pan: 64, bend: 8192, range: 2.0, rpn: (127, 127), ..Default::default() }; 16];
    // Parts by (track, channel); a sounding note per (part, key), oldest first.
    let mut parts: Vec<Part> = Vec::new();
    let mut part_of = std::collections::HashMap::<(usize, u8), usize>::new();
    let mut open: std::collections::HashMap<(usize, u8), Vec<usize>> = std::collections::HashMap::new();
    let mut volumes: Vec<Vec<(u8, u64)>> = Vec::new();
    let mut end = 0u64;
    for &(tick, track, kind) in &events {
        let EventKind::Channel(c) = kind else { continue };
        let ch = c.channel & 15;
        let s = &mut state[ch as usize];
        match c.message {
            ChannelMessage::ProgramChange { program } => {
                s.program = program & 127;
                if let Some(&p) = part_of.get(&(track, ch)) {
                    if parts[p].changes.last().map_or(parts[p].program, |c| c.1) != s.program {
                        parts[p].changes.push((tick, s.program));
                    }
                }
            }
            ChannelMessage::ControlChange { controller, value } => {
                match controller {
                    1 => {
                        if let Some(&p) = part_of.get(&(track, ch)) {
                            parts[p].mod_moves += 1;
                        }
                    }
                    7 => s.volume = value,
                    10 => s.pan = value,
                    11 => s.expression = value,
                    64 => {
                        s.sustain = value;
                        if let Some(&p) = part_of.get(&(track, ch)) {
                            parts[p].pedal.push((tick, value));
                        }
                    }
                    101 => s.rpn.0 = value,
                    100 => s.rpn.1 = value,
                    6 if s.rpn == (0, 0) => s.range = (value as f32).clamp(1.0, 24.0),
                    _ => {}
                }
                if controller == 7 {
                    if let Some(&p) = part_of.get(&(track, ch)) {
                        volumes[p].push((value, tick));
                    }
                }
            }
            ChannelMessage::PitchBend { value } => s.bend = value,
            ChannelMessage::NoteOn { key, velocity } if velocity > 0 => {
                let p = *part_of.entry((track, ch)).or_insert_with(|| {
                    parts.push(Part {
                        track,
                        channel: ch,
                        name: names.get(track).cloned().unwrap_or_default(),
                        program: s.program,
                        changes: Vec::new(),
                        drums: ch == 9,
                        notes: Vec::new(),
                        pedal: if s.sustain >= 64 { vec![(tick, s.sustain)] } else { Vec::new() },
                        volume: s.volume,
                        pan: s.pan,
                        mod_moves: 0,
                        bent: 0,
                        role: if ch == 9 { Role::Drums } else { gm::role(s.program) },
                        melody: false,
                    });
                    volumes.push(vec![(s.volume, tick)]);
                    parts.len() - 1
                });
                let bend = (s.bend as f32 - 8192.0) / 8192.0 * s.range;
                if bend.abs() >= 0.25 {
                    parts[p].bent += 1;
                }
                parts[p].notes.push(Note { on: tick, off: tick, key: key & 127, vel: velocity.min(127), expr: s.expression as f32 / 127.0, bend });
                open.entry((p, key & 127)).or_default().push(parts[p].notes.len() - 1);
            }
            ChannelMessage::NoteOn { key, .. } | ChannelMessage::NoteOff { key, .. } => {
                if let Some(&p) = part_of.get(&(track, ch)) {
                    if let Some(stack) = open.get_mut(&(p, key & 127)) {
                        if !stack.is_empty() {
                            let n = stack.remove(0);
                            parts[p].notes[n].off = tick;
                            end = end.max(tick);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // Notes never let go end where their track's last event is (or a beat on).
    for ((p, _), stack) in open {
        for n in stack {
            let note = &mut parts[p].notes[n];
            note.off = note.on + ppq as u64;
            end = end.max(note.off);
        }
    }
    parts.retain(|p| !p.notes.is_empty());
    for part in parts.iter_mut() {
        part.notes.sort_by_key(|n| (n.on, n.key));
        for n in &mut part.notes {
            // A drum hit or a zero-length note still sounds.
            if n.off <= n.on {
                n.off = n.on + (ppq as u64 / 8).max(1);
            }
            end = end.max(n.off);
        }
        let mut at = 0u8;
        part.pedal.retain(|&(_, v)| {
            let down = if v >= 64 { 127 } else { 0 };
            std::mem::replace(&mut at, down) != down
        });
    }
    // The volume a part mostly plays at, weighted by time.
    for (part, vols) in parts.iter_mut().zip(volumes.iter()) {
        let mut spans: Vec<(u8, u64)> = Vec::new();
        for (i, (v, t)) in vols.iter().enumerate() {
            let to = vols.get(i + 1).map_or(end, |n| n.1);
            spans.push((*v, to.saturating_sub(*t)));
        }
        spans.sort_by_key(|s| s.0);
        let half = spans.iter().map(|s| s.1).sum::<u64>() / 2;
        let mut acc = 0;
        part.volume = spans.iter().find(|s| {
            acc += s.1;
            acc >= half
        }).map_or(part.volume, |s| s.0);
    }
    let mut song = Song { ppq, tempo, sigs, key_sig, parts, end };
    mark_melody(&mut song);
    Ok(song)
}

/// The share of a part's notes that start while none of its other notes
/// sound (1 for a single line, near 0 for chords).
fn monophony(part: &Part, ppq: u32) -> f64 {
    // A legato overlap of up to a 32nd still reads as one line.
    let slack = ppq as u64 / 8;
    let mut alone = 0usize;
    let mut sounding_until = 0u64;
    for (i, n) in part.notes.iter().enumerate() {
        let chord = part.notes.get(i + 1).is_some_and(|m| m.on == n.on) || (i > 0 && part.notes[i - 1].on == n.on);
        if n.on + slack >= sounding_until && !chord {
            alone += 1;
        }
        sounding_until = sounding_until.max(n.off);
    }
    alone as f64 / part.notes.len().max(1) as f64
}

/// Find the tune: a mostly single-line part, high, present through much
/// of the song, or a part whose name says it is the melody or the vocal.
fn mark_melody(song: &mut Song) {
    let end = song.end.max(1) as f64;
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in song.parts.iter().enumerate() {
        if p.drums || p.notes.len() < 24 {
            continue;
        }
        let name = p.name.to_lowercase();
        let named = ["melody", "vocal", "voice", "lead", "sing", "vox", "tune", "melodie"].iter().any(|w| name.contains(w));
        let mean = p.notes.iter().map(|n| n.key as f64).sum::<f64>() / p.notes.len() as f64;
        if mean < 52.0 && !named {
            continue;
        }
        let span = (p.notes.last().unwrap().off - p.notes[0].on) as f64 / end;
        let score = monophony(p, song.ppq) * 2.0 + span + (mean - 52.0).clamp(0.0, 24.0) / 24.0 + if named { 2.0 } else { 0.0 } - if p.role == Role::Bass { 3.0 } else { 0.0 };
        if best.is_none_or(|b| score > b.1) {
            best = Some((i, score));
        }
    }
    if let Some((i, score)) = best {
        if score > 1.6 {
            song.parts[i].melody = true;
        }
    }
}

/// What the index lists about a song.
#[derive(Clone, Debug, PartialEq)]
pub struct Features {
    pub bpm: f64,
    /// "A minor", "Eb major".
    pub key: String,
    /// "4/4".
    pub sig: String,
    pub parts: usize,
    pub programs: Vec<u8>,
    pub roles: Vec<Role>,
    pub seconds: f64,
    pub notes: usize,
    pub melody: bool,
    /// 0..100: a full arrangement (drums, bass, harmony, a tune, long
    /// enough, expressive) scores high, a sketch low.
    pub quality: u8,
}

const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// The key by Krumhansl-Schmuckler on the duration-weighted pitch classes
/// (the file's key signature is often missing or left at C).
pub fn key(song: &Song) -> String {
    let mut hist = [0f64; 12];
    for p in song.parts.iter().filter(|p| !p.drums) {
        for n in &p.notes {
            hist[n.key as usize % 12] += (n.off - n.on) as f64;
        }
    }
    key_of_histogram(&hist)
}

/// The key of duration-weighted pitch classes ("" when there are none).
pub fn key_of_histogram(hist: &[f64; 12]) -> String {
    const MAJOR: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
    const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];
    if hist.iter().sum::<f64>() <= 0.0 {
        return String::new();
    }
    let corr = |profile: &[f64; 12], tonic: usize| {
        let mx = hist.iter().sum::<f64>() / 12.0;
        let my = profile.iter().sum::<f64>() / 12.0;
        let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
        for i in 0..12 {
            let x = hist[(i + tonic) % 12] - mx;
            let y = profile[i] - my;
            sxy += x * y;
            sxx += x * x;
            syy += y * y;
        }
        sxy / (sxx * syy).sqrt().max(1e-12)
    };
    let mut best = (f64::MIN, 0, false);
    for tonic in 0..12 {
        for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
            let r = corr(profile, tonic);
            if r > best.0 {
                best = (r, tonic, minor);
            }
        }
    }
    format!("{} {}", NAMES[best.1], if best.2 { "minor" } else { "major" })
}

pub fn features(song: &Song) -> Features {
    let mut programs: Vec<u8> = song.parts.iter().filter(|p| !p.drums).map(|p| p.program).collect();
    programs.sort();
    programs.dedup();
    let mut roles: Vec<Role> = song.parts.iter().map(|p| p.role).collect();
    roles.sort();
    roles.dedup();
    let notes: usize = song.parts.iter().map(|p| p.notes.len()).sum();
    let seconds = song.seconds();
    let melody = song.parts.iter().any(|p| p.melody);
    let (num, den) = song.sig_at(0);
    let has = |r: Role| roles.contains(&r);
    let mut q = 0f64;
    q += if has(Role::Drums) { 15.0 } else { 0.0 };
    q += if has(Role::Bass) { 15.0 } else { 0.0 };
    let harmony = [Role::Keys, Role::Guitar, Role::Pads, Role::Strings].iter().filter(|r| has(**r)).count();
    q += (harmony as f64 * 7.5).min(15.0);
    q += if melody { 15.0 } else { 0.0 };
    q += ((seconds - 60.0) / 120.0).clamp(0.0, 1.0) * 15.0;
    q += ((song.parts.len() as f64 - 2.0) / 5.0).clamp(0.0, 1.0) * 10.0;
    // Expressive velocities (a step-entered sketch has one velocity).
    let vels: Vec<f64> = song.parts.iter().flat_map(|p| p.notes.iter().map(|n| n.vel as f64)).collect();
    let mean = vels.iter().sum::<f64>() / vels.len().max(1) as f64;
    let sd = (vels.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vels.len().max(1) as f64).sqrt();
    q += (sd / 12.0).clamp(0.0, 1.0) * 10.0;
    q += ((notes as f64 / seconds.max(1.0)) / 6.0).clamp(0.0, 1.0) * 5.0;
    Features {
        bpm: (song.main_bpm() * 10.0).round() / 10.0,
        key: key(song),
        sig: format!("{num}/{den}"),
        parts: song.parts.len(),
        programs,
        roles,
        seconds: seconds.round(),
        notes,
        melody,
        quality: q.round().clamp(0.0, 100.0) as u8,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use makepad_midi_file::*;

    /// A small band: drums, a bass, chords, and a melody line, 8 bars at
    /// 100 bpm.
    pub fn band() -> Vec<u8> {
        let ch = |tick: u64, channel: u8, message: ChannelMessage| TrackEvent { tick, kind: makepad_midi_file::EventKind::Channel(ChannelEvent { channel, message }) };
        let meta = |tick: u64, m: MetaEvent| TrackEvent { tick, kind: makepad_midi_file::EventKind::Meta(m) };
        let mut tracks = vec![Track { events: vec![meta(0, MetaEvent::SetTempo(600_000)), meta(0, MetaEvent::TimeSignature(TimeSignature::default())), meta(7680, MetaEvent::EndOfTrack)], trailing_data: Vec::new() }];
        let mut part = |name: &str, channel: u8, program: u8, notes: Vec<(u64, u64, u8, u8)>| {
            let mut ev = vec![meta(0, MetaEvent::SequenceOrTrackName(name.as_bytes().to_vec())), ch(0, channel, ChannelMessage::ProgramChange { program }), ch(0, channel, ChannelMessage::ControlChange { controller: 7, value: 100 })];
            let mut offs = Vec::new();
            for (on, len, key, vel) in notes {
                ev.push(ch(on, channel, ChannelMessage::NoteOn { key, velocity: vel }));
                offs.push(ch(on + len, channel, ChannelMessage::NoteOff { key, velocity: 0 }));
            }
            ev.extend(offs);
            ev.sort_by_key(|e| e.tick);
            ev.push(meta(7680, MetaEvent::EndOfTrack));
            tracks.push(Track { events: ev, trailing_data: Vec::new() });
        };
        let bars = 0..8u64;
        part("Drums", 9, 0, bars.clone().flat_map(|b| (0..4u64).flat_map(move |q| [(b * 960 + q * 240, 60, if q % 2 == 0 { 36 } else { 38 }, 100), (b * 960 + q * 240, 60, 42, 70 + (q as u8) * 5)])).collect());
        part("Bass", 1, 33, bars.clone().flat_map(|b| (0..4u64).map(move |q| (b * 960 + q * 240, 220, if b % 2 == 0 { 45 } else { 41 }, 90 + q as u8))).collect());
        part("Piano", 2, 0, bars.clone().flat_map(|b| [57u8, 60, 64].map(|k| (b * 960, 900, if b % 2 == 0 { k } else { k - 4 }, 70)).to_vec()).collect());
        part("Melody", 3, 73, bars.clone().flat_map(|b| (0..4u64).map(move |q| (b * 960 + q * 240, 200, [76u8, 74, 72, 74][q as usize], 80 + (q as u8) * 6))).collect());
        let file = MidiFile { header: Header { format: Format::Parallel, track_count: tracks.len() as u16, division: Division::TicksPerQuarter(240), extra_data: Vec::new() }, tracks, unknown_chunks: Vec::new() };
        file.to_bytes().unwrap()
    }

    #[test]
    fn reads_parts_roles_and_features() {
        let song = read(&band()).unwrap();
        assert_eq!(song.parts.len(), 4);
        let roles: Vec<Role> = song.parts.iter().map(|p| p.role).collect();
        assert!(roles.contains(&Role::Drums) && roles.contains(&Role::Bass) && roles.contains(&Role::Keys));
        let melody: Vec<&str> = song.parts.iter().filter(|p| p.melody).map(|p| p.name.as_str()).collect();
        assert_eq!(melody, vec!["Melody"]);
        let f = features(&song);
        assert_eq!(f.bpm, 100.0);
        assert_eq!(f.sig, "4/4");
        assert_eq!(f.parts, 4);
        assert!(f.key == "A minor" || f.key == "C major", "{}", f.key);
        assert!((f.seconds - 19.0).abs() <= 1.0, "{}", f.seconds);
        assert!(f.quality >= 50, "{}", f.quality);
        assert_eq!(song.measures().len(), 8);
    }
}
