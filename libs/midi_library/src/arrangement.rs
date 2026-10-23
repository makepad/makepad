//! An arrangement: what a song is, between its sources and the Score that
//! plays it. Parts (each a role, an instrument and its notes in beats),
//! the bars, the sections and the tempo. A MIDI file is imported into one
//! ([`crate::import`]), the text transcription reads and writes one
//! ([`crate::roll`]), a Score is read back into one by playing it
//! ([`crate::roll::from_score`]), and [`to_score`] writes one as a readable
//! Score with its mix and master chain.

use crate::gm::{self, Role, Voice};
use std::fmt::Write;

/// A note: `key` a MIDI pitch (fractional when bent) or, on a drum part,
/// the GM percussion note; beats from the song's start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub beat: f64,
    pub key: f32,
    pub dur: f64,
    /// 0..1.
    pub vel: f64,
    /// What it does while it sounds (tracker and chip music).
    pub fx: Option<NoteFx>,
}

/// The most breakpoints a note's bend or level keeps.
pub const FX_POINTS: usize = 8;

/// What a note does while it sounds, as the Score's note options write it
/// (`[beat note dur vel, {bend vib arp env cutoff_env pan start}]`): its pitch bend
/// and level over time (a portamento, a volume slide, an envelope), a
/// vibrato, an arpeggio, its pan and where its sample starts. Tracker and
/// chip music is made of these.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct NoteFx {
    /// `[beats from the note's start, semitones]`.
    pub bend: [(f32, f32); FX_POINTS],
    pub bend_n: u8,
    /// `[beats, level]`: the level over the note, times `vel`.
    pub level: [(f32, f32); FX_POINTS],
    pub level_n: u8,
    /// `[beats, Hz]`: the filter's cutoff over the note (a SID sweep).
    pub cutoff: [(f32, f32); FX_POINTS],
    pub cutoff_n: u8,
    /// (rate Hz, depth semitones, delay beats); depth 0 is none.
    pub vib: (f32, f32, f32),
    /// Semitone offsets stepped every `arp_step` beats.
    pub arp: [f32; 4],
    pub arp_n: u8,
    pub arp_step: f32,
    /// -1..1 (0 is the part's).
    pub pan: f32,
    /// Where a sample starts, 0..1 of it.
    pub start: f32,
}

impl NoteFx {
    pub fn bend(&self) -> &[(f32, f32)] {
        &self.bend[..self.bend_n as usize]
    }

    pub fn level(&self) -> &[(f32, f32)] {
        &self.level[..self.level_n as usize]
    }

    pub fn cutoff(&self) -> &[(f32, f32)] {
        &self.cutoff[..self.cutoff_n as usize]
    }

    pub fn arp(&self) -> &[f32] {
        &self.arp[..self.arp_n as usize]
    }

    /// Nothing to say.
    pub fn is_plain(&self) -> bool {
        self.bend_n == 0 && self.level_n == 0 && self.cutoff_n == 0 && self.vib.1 <= 0.0 && self.arp_n == 0 && self.pan == 0.0 && self.start <= 0.0
    }

    /// The options map a note list entry writes.
    pub fn text(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        let points = |pts: &[(f32, f32)], places: usize| pts.iter().map(|(b, v)| format!("[{} {}]", num(*b as f64, 4), num(*v as f64, places))).collect::<Vec<_>>().join(" ");
        if self.pan != 0.0 {
            out.push(format!("pan: {}", num(self.pan as f64, 2)));
        }
        if self.bend_n > 0 {
            out.push(format!("bend: [{}]", points(self.bend(), 3)));
        }
        if self.vib.1 > 0.0 {
            let delay = if self.vib.2 > 0.0 { format!(" {}", num(self.vib.2 as f64, 4)) } else { String::new() };
            out.push(format!("vib: [{} {}{delay}]", num(self.vib.0 as f64, 2), num(self.vib.1 as f64, 3)));
        }
        if self.arp_n > 0 {
            out.push(format!("arp: [{}] arp_step: {}", self.arp().iter().map(|v| num(*v as f64, 2)).collect::<Vec<_>>().join(" "), num(self.arp_step as f64, 5)));
        }
        if self.level_n > 0 {
            out.push(format!("env: [{}]", points(self.level(), 3)));
        }
        if self.cutoff_n > 0 {
            out.push(format!("cutoff_env: [{}]", points(self.cutoff(), 0)));
        }
        if self.start > 0.0 {
            out.push(format!("start: {}", num(self.start as f64, 5)));
        }
        format!("{{{}}}", out.join(" "))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// A Score identifier: `drums`, `bass`, `keys_2`, `melody`.
    pub name: String,
    pub role: Role,
    /// The tune.
    pub melody: bool,
    pub voice: Voice,
    /// Where it came from, for its comment ("track 3 \"Bass\", channel 2,
    /// GM 33 Electric Bass (finger)").
    pub about: String,
    pub notes: Vec<Note>,
    /// Sustain pedal moves (beat, down) on a native part.
    pub pedal: Vec<(f64, bool)>,
    /// GM program changes (beat, program) on a SoundFont part.
    pub programs: Vec<(f64, u8)>,
    pub amp: f64,
    pub pan: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub kind: String,
    pub letter: char,
    /// Bars `from..to` (indices into [`Arrangement::bars`]).
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arrangement {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    pub genre: String,
    /// "A minor".
    pub key: String,
    pub bpm: f64,
    pub sig: (u8, u32),
    /// The beat each bar starts at, and the end as the last entry.
    pub bars: Vec<f64>,
    pub parts: Vec<Part>,
    pub sections: Vec<Section>,
    /// Tempo changes after the start, (beat, bpm).
    pub tempo: Vec<(f64, f64)>,
    /// Comment lines for the header (credit, licence, what was not played).
    pub notes: Vec<String>,
    /// `Score{rights: …}` as (license, credits, source, terms).
    pub rights: Option<(String, String, String, String)>,
    pub limiter_gain: f64,
    /// It plays once and stops (an excerpt); else a ring-out bar ends it.
    pub excerpt: bool,
}

impl Arrangement {
    pub fn beats(&self) -> f64 {
        self.bars.last().copied().unwrap_or(0.0)
    }

    pub fn bar_count(&self) -> usize {
        self.bars.len().saturating_sub(1)
    }

    pub fn per_bar(&self) -> f64 {
        self.sig.0 as f64 * 4.0 / self.sig.1 as f64
    }

    /// The bar a beat is in.
    pub fn bar_of(&self, beat: f64) -> usize {
        self.bars.partition_point(|b| *b <= beat + 1e-9).saturating_sub(1).min(self.bar_count().saturating_sub(1))
    }

    pub fn bpm_at(&self, beat: f64) -> f64 {
        self.tempo.iter().rev().find(|t| t.0 <= beat + 1e-9).map_or(self.bpm, |t| t.1)
    }

    /// Whether the key is written with flats.
    pub fn flats(&self) -> bool {
        matches!(self.key.as_str(), "F major" | "Bb major" | "Eb major" | "Ab major" | "Db major" | "D minor" | "G minor" | "C minor" | "F minor" | "Bb minor" | "Eb minor")
    }
}

/// Entries on one line of a note list.
const PER_LINE: usize = 8;

/// A number with at most `places` decimals, without trailing zeros.
pub fn num(x: f64, places: usize) -> String {
    let s = format!("{x:.places$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

/// A note name as the score writes it (`cs4`, `eb5`; 60 is `c4`).
pub fn note_name(key: u8, flats: bool) -> String {
    const SHARPS: [&str; 12] = ["c", "cs", "d", "ds", "e", "f", "fs", "g", "gs", "a", "as", "b"];
    const FLATS: [&str; 12] = ["c", "db", "d", "eb", "e", "f", "gb", "g", "ab", "a", "bb", "b"];
    let names = if flats { FLATS } else { SHARPS };
    format!("{}{}", names[key as usize % 12], key as i32 / 12 - 1)
}

/// A Section's `bars:` for `beats` beats: whole, or exact arithmetic.
pub fn bars_text(beats: f64, per_bar: f64) -> String {
    let bars = beats / per_bar;
    if (bars - bars.round()).abs() < 1e-6 {
        return num(bars.round(), 0);
    }
    for den in 2..=48u32 {
        let n = bars * den as f64;
        if (n - n.round()).abs() < 1e-6 {
            let whole = bars.floor() as i64;
            let rest = n.round() as i64 - whole * den as i64;
            return format!("{whole} + {rest}/{den}");
        }
    }
    num(bars, 6)
}

/// Sends per role: (room, plate, hall, throw, smash).
fn sends(role: Role, melody: bool) -> [f64; 5] {
    if melody {
        return [0.0, 0.22, 0.0, 0.12, 0.0];
    }
    match role {
        Role::Drums => [0.15, 0.0, 0.0, 0.0, 0.3],
        Role::Bass => [0.0; 5],
        Role::Keys | Role::Guitar => [0.0, 0.15, 0.0, 0.0, 0.0],
        Role::Pads | Role::Strings => [0.0, 0.0, 0.35, 0.0, 0.0],
        Role::Brass => [0.0, 0.0, 0.2, 0.0, 0.0],
        Role::Lead => [0.0, 0.2, 0.0, 0.1, 0.0],
        Role::Fx => [0.0, 0.0, 0.4, 0.0, 0.0],
    }
}

/// The base level of a role before the source's own volume (the GM
/// SoundFont plays about 8 dB hotter than the presets at one amp: its
/// parts get 0.4 of this).
pub fn level(role: Role, melody: bool) -> f64 {
    if melody {
        return 0.8;
    }
    match role {
        Role::Drums => 0.85,
        Role::Bass => 0.8,
        Role::Keys | Role::Guitar => 0.55,
        Role::Pads => 0.45,
        Role::Strings | Role::Brass => 0.5,
        Role::Lead => 0.6,
        Role::Fx => 0.35,
    }
}

/// A note as a list writes it: a drum by name (or its GM number), a pitch
/// by name, a bent pitch as a number.
fn note_text(key: f32, drums: bool, flats: bool) -> String {
    if drums {
        let k = key.round().clamp(0.0, 127.0) as u8;
        return gm::drum_name(k).map_or(k.to_string(), |name| format!("@{name}"));
    }
    if (key - key.round()).abs() > 0.01 {
        return num(key as f64, 2);
    }
    format!("@{}", note_name(key.round().clamp(0.0, 127.0) as u8, flats))
}

/// The arrangement as a Score: header comments, the clock, an instrument
/// and a note list and loop per part (marked by section and bar), the mix
/// and master chain, the sections with the tempo map, and `Score{…}`.
pub fn to_score(a: &Arrangement) -> String {
    let mut w = String::new();
    let flats = a.flats();
    let per_bar = a.per_bar();
    let music_beats = a.beats();
    let total_beats = music_beats + if a.excerpt { 0.0 } else { per_bar };
    let year = a.year.map_or(String::new(), |y| format!(" ({y})"));
    let by = if a.artist.is_empty() { String::new() } else { format!(" -- {}", a.artist) };
    let _ = writeln!(w, "// {}{by}{year}", a.title);
    let _ = writeln!(w, "//");
    for line in &a.notes {
        let _ = writeln!(w, "// {line}");
    }
    let _ = writeln!(w, "// {} at {} bpm in {}/{}{}; {} parts.", if a.key.is_empty() { "No key".to_string() } else { a.key.clone() }, num(a.bpm, 1), a.sig.0, a.sig.1, if a.genre.is_empty() { String::new() } else { format!(", {}", a.genre) }, a.parts.len());
    let form: Vec<String> = a.sections.iter().map(|s| format!("{} {} [{}]", s.name, if s.to - s.from == 1 { format!("bar {}", s.from + 1) } else { format!("bars {}-{}", s.from + 1, s.to) }, s.letter)).collect();
    let _ = writeln!(w, "// Form (letters sound alike): {}.", form.join(", "));
    let _ = writeln!(w, "//");
    let _ = writeln!(w, "// Reading it: each part is a loop playing its note list once through. An entry is");
    let _ = writeln!(w, "// [beat note dur vel]: beats are quarter notes from the song's start, dur in quarters,");
    let _ = writeln!(w, "// vel 0..1. Comments mark the sections and bars. [beat @program n] changes a SoundFont");
    let _ = writeln!(w, "// part's GM program, [beat @pedal 1] / [beat @pedal 0] press and lift its sustain pedal.");
    let _ = writeln!(w, "// The mix and master chain follow the parts.");
    let _ = writeln!(w, "use mod.score.*");
    let _ = writeln!(w);
    let _ = writeln!(w, "let clock = Clock{{bpm: {} sig: [{} {}]}}", num(a.bpm, 3), a.sig.0, a.sig.1);
    let _ = writeln!(w);
    for p in &a.parts {
        let _ = writeln!(w, "/// {}{}: {}, on {}.", p.name, if p.melody { " (the tune)" } else { "" }, p.about, p.voice.about());
        let _ = writeln!(w, "let {}_sound = {}", p.name, p.voice.node());
    }
    let _ = writeln!(w);
    let section_of = |bar: usize| a.sections.iter().position(|s| s.from <= bar && bar < s.to);
    let mut loops: Vec<&Part> = Vec::new();
    let sounding = a.parts.iter().filter(|p| !p.notes.is_empty()).count();
    for p in &a.parts {
        if p.notes.is_empty() {
            continue;
        }
        let drums = p.role == Role::Drums;
        let mut entries: Vec<(f64, u8, String)> = p
            .notes
            .iter()
            .map(|n| {
                // The options follow a comma: `0.9 {…}` would read as one value.
                let fx = n.fx.filter(|f| !f.is_plain()).map_or(String::new(), |f| format!(", {}", f.text()));
                (n.beat, 1, format!("[{} {} {} {}{fx}]", num(n.beat, 4), note_text(n.key, drums, flats), num(n.dur, 4), num(n.vel, 2)))
            })
            .collect();
        if p.voice.native() && !drums {
            entries.extend(p.programs.iter().map(|&(b, prog)| (b, 0, format!("[{} @program {prog}]", num(b, 4)))));
            entries.extend(p.pedal.iter().map(|&(b, down)| (b, 0, format!("[{} @pedal {}]", num(b, 4), down as u8))));
        }
        entries.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        let _ = writeln!(w, "let {}_notes = [", p.name);
        let mut last_section = None;
        let mut last_bar = usize::MAX;
        let mut line: Vec<String> = Vec::new();
        for (beat, _, text) in entries {
            let bar = a.bar_of(beat);
            if bar != last_bar {
                if !line.is_empty() {
                    let _ = writeln!(w, "    {}", line.join(" "));
                    line.clear();
                }
                let s = section_of(bar);
                if s != last_section {
                    if let Some(sec) = s.and_then(|s| a.sections.get(s)) {
                        let _ = writeln!(w, "    // -- {} (bars {}-{}) --", sec.name, sec.from + 1, sec.to);
                    }
                    last_section = s;
                }
                let _ = writeln!(w, "    // bar {}", bar + 1);
                last_bar = bar;
            }
            line.push(text);
            if line.len() == PER_LINE {
                let _ = writeln!(w, "    {}", line.join(" "));
                line.clear();
            }
        }
        if !line.is_empty() {
            let _ = writeln!(w, "    {}", line.join(" "));
        }
        let _ = writeln!(w, "]");
        // Every send is a bus in the graph: with many parts (a tracker
        // module's samples) only the tune and the pads keep theirs.
        let keep_sends = sounding <= 12 || p.melody || matches!(p.role, Role::Pads | Role::Strings);
        let send_text: Vec<String> = ["room", "plate", "hall", "throw", "smash"].iter().zip(sends(p.role, p.melody)).filter(|(_, v)| *v > 0.0 && keep_sends).map(|(n, v)| format!("{n}: {}", num(v, 2))).collect();
        let sends = if send_text.is_empty() { String::new() } else { format!(" sends: {{{}}}", send_text.join(" ")) };
        let _ = writeln!(w, "let {} = Loop{{play: {}_sound amp: {} pan: {} beats: {}{sends} notes: [{}_notes]}}", p.name, p.name, num(p.amp, 2), num(p.pan, 2), num(total_beats, 4), p.name);
        let _ = writeln!(w);
        loops.push(p);
    }

    // The mix: returns, buses, the master.
    let of = |roles: &[Role], melody: bool| -> Vec<String> { loops.iter().filter(|p| (roles.contains(&p.role) && !p.melody) || (melody && p.melody)).map(|p| p.name.clone()).collect() };
    let drums = of(&[Role::Drums], false);
    let bass = of(&[Role::Bass], false);
    let music = of(&[Role::Keys, Role::Guitar, Role::Brass, Role::Lead, Role::Fx], true);
    let pads = of(&[Role::Pads, Role::Strings], false);
    let electronic = loops.iter().any(|p| matches!(p.voice, Voice::Kit(_)));
    let mut fx: Vec<&str> = vec!["room", "plate", "hall", "throw", "returns"];
    let mut into_glue: Vec<&str> = vec!["returns"];
    let _ = writeln!(w, "// -- mix: returns, buses and the master --");
    let _ = writeln!(w, "let room = Reverb{{room: 0.35 mix: 1 damp: 9000}}");
    let _ = writeln!(w, "let plate = Reverb{{room: 0.6 mix: 1 damp: 9500}}");
    let _ = writeln!(w, "let hall = Reverb{{room: 0.82 mix: 1 damp: 9000}}");
    let _ = writeln!(w, "let throw = Delay{{time: 0.75 feedback: 0.4 mix: 1 spread: 1}}");
    let _ = writeln!(w, "let returns = Highpass{{in: [room plate hall throw] cutoff: 250}}");
    if !drums.is_empty() {
        let _ = writeln!(w, "let smash = Comp{{threshold: -34 ratio: 12 makeup: 10}}");
        let _ = writeln!(w, "let drum_bus = Comp{{in: [{}] threshold: -16 ratio: 3 attack: 15 release: 150 makeup: 2}}", drums.join(" "));
        let _ = writeln!(w, "let drum_tape = Drive{{in: [drum_bus] amount: 0.15 mix: 0.3}}");
        fx.extend(["smash", "drum_bus", "drum_tape"]);
        into_glue.extend(["drum_tape", "smash"]);
    }
    if !bass.is_empty() {
        let _ = writeln!(w, "let bass_bus = Comp{{in: [{}] threshold: -18 ratio: 3 attack: 10 release: 120 makeup: 1}}", bass.join(" "));
        fx.push("bass_bus");
        if electronic && !drums.is_empty() {
            let _ = writeln!(w, "let bass_duck = Duck{{in: [bass_bus] key: {} depth: 3 release: 0.2}}", drums[0]);
            fx.push("bass_duck");
            into_glue.push("bass_duck");
        } else {
            into_glue.push("bass_bus");
        }
    }
    if !music.is_empty() {
        let _ = writeln!(w, "let music_hp = Highpass{{in: [{}] cutoff: 160}}", music.join(" "));
        fx.push("music_hp");
        into_glue.push("music_hp");
    }
    if !pads.is_empty() {
        let _ = writeln!(w, "let pad_hp = Highpass{{in: [{}] cutoff: 200}}", pads.join(" "));
        let _ = writeln!(w, "let pad_eq = Eq{{in: [pad_hp] b1: [@peak, 500, -3, 1] b2: [@highshelf, 9000, -2]}}");
        fx.extend(["pad_hp", "pad_eq"]);
        into_glue.push("pad_eq");
    }
    let _ = writeln!(w, "let glue = Comp{{in: [{}] threshold: -14 ratio: 1.8 attack: 20 release: 150 knee: 6 makeup: 1}}", into_glue.join(" "));
    let _ = writeln!(w, "let master_eq = Eq{{in: [glue] b1: [@highpass 28] b2: [@lowshelf 90 1] b3: [@peak, 350, -1.5, 0.9] b4: [@highshelf 10000 1.5]}}");
    let _ = writeln!(w, "let master_width = Width{{in: [master_eq] width: 1.1 bass_mono: 140}}");
    let _ = writeln!(w, "let master_peak = Comp{{in: [master_width] threshold: -10 ratio: 5 attack: 1 release: 80 knee: 4 makeup: 0}}");
    let _ = writeln!(w, "let master_limit = Limiter{{in: [master_peak] ceiling: -1.5 gain: {}}}", num(a.limiter_gain, 3));
    let _ = writeln!(w, "let master_lp = Lowpass{{in: [master_limit] cutoff: 16000 res: 0.5}}");
    fx.extend(["glue", "master_eq", "master_width", "master_peak", "master_limit", "master_lp"]);
    let _ = writeln!(w);

    // The form, with the tempo map.
    let loop_names: Vec<&str> = loops.iter().map(|p| p.name.as_str()).collect();
    let _ = writeln!(w, "// -- form --");
    for (i, s) in a.sections.iter().enumerate() {
        let start = a.bars[s.from];
        let end = a.bars[s.to.min(a.bar_count())];
        let bars = bars_text(end - start, per_bar);
        let mut tempo = String::new();
        if !a.tempo.is_empty() {
            let mut points = vec![(0.0, a.bpm_at(start))];
            for &(beat, bpm) in a.tempo.iter().filter(|t| t.0 > start + 1e-9 && t.0 < end - 1e-9) {
                points.push((beat - start, bpm));
            }
            points.dedup_by(|b, a| (a.1 - b.1).abs() < 1e-6);
            let text: Vec<String> = points.iter().map(|(b, bpm)| format!("[{} {}]", num(*b, 4), num(*bpm, 3))).collect();
            tempo = format!(" tempo: [{}]", text.join(" "));
        }
        let next = if a.excerpt && i + 1 == a.sections.len() { " next: @stop" } else { "" };
        let _ = writeln!(w, "/// bars {}-{} [{}]", s.from + 1, s.to, s.letter);
        let _ = writeln!(w, "let {} = Section{{bars: {bars} loops: [{}]{tempo}{next}}}", s.name, loop_names.join(" "));
    }
    let mut arrange: Vec<&str> = a.sections.iter().map(|s| s.name.as_str()).collect();
    if !a.excerpt {
        let _ = writeln!(w, "/// The last chord rings out.");
        let _ = writeln!(w, "let ring = Section{{bars: 1 loops: [{}] next: @stop}}", loop_names.join(" "));
        arrange.push("ring");
    }
    let _ = writeln!(w);
    let label = if a.artist.is_empty() { a.title.clone() } else { format!("{} (cover)", a.title) };
    let _ = writeln!(w, "Score{{label: {:?}", label.replace('"', "'"));
    if let Some((license, credits, source, terms)) = &a.rights {
        let _ = writeln!(w, "    rights: {{license: {license:?} credits: {credits:?} source: {source:?} terms: {terms:?}}}");
    }
    let _ = writeln!(w, "    arrange: [{}]", arrange.join(" "));
    let insts: Vec<String> = a.parts.iter().map(|p| format!("{}_sound", p.name)).collect();
    let _ = writeln!(w, "    clock {}", insts.join(" "));
    let _ = writeln!(w, "    {}", loop_names.join(" "));
    let _ = writeln!(w, "    {}", fx.join(" "));
    let _ = writeln!(w, "    {}}}", arrange.join(" "));
    w
}
