//! The text transcription: a song as a dense, readable piano roll an AI
//! can read and write, instead of MIDI bytes or event lists.
//!
//! ```text
//! # Take On Me -- a-ha (1985)
//! tempo 169 | meter 4/4 | key A major | grid 16
//! form intro A4, verse_1 B8, chorus_1 C8
//! part drums drums kit:analog_hybrid amp=0.85 pan=0
//! part bass bass preset:fm_bass amp=0.8 pan=0
//! part melody lead* preset:saw_lead amp=0.8 pan=0
//! == verse_1 [B] bars 5-12
//! 5 | F#m . . . | D . . . |
//!   melody F#4 ~ . F#4 . D4 ~ . B3 ~ . E4 . E4 ~ .
//!   bass   F#1 . F#2 . F#1 . F#2 . D1 . D2 . D1 . D2 .
//!   drums  K x... x... x... x... | S .... x... .... x... | H xxxx xxxx xxxx xxxx
//! 7-8 = 5-6
//! ```
//!
//! The grid is four steps per beat (sixteenths in 4/4). A pitched part's
//! line has a token per step: a note (`A4`, `C#5`, `Bb3`) or a chord
//! (`A3+C#4+E4`) starts there, `~` holds what sounds, `.` is silence; a
//! note lasts until the next token that is not `~`. `!` after a note is
//! loud, `?` soft. A drum part has a row per drum (`K` kick, `S` snare, `H`
//! closed hat, `O` open hat, `P` pedal hat, `R` rim, `C` clap, `T1`-`T3`
//! toms high to low, `FT` floor tom, `CR` crash, `RD` ride, `BL` ride
//! bell, `CB` cowbell, `SH` shaker, `CG` conga, `N54` any other GM note),
//! grouped a beat at a time: `x` hit, `X` accent, `o` ghost. The chord line
//! names the harmony a beat at a time (`.` continues); it is analysis, read
//! from the notes, and ignored when the text becomes a Score. A part that
//! is silent for a bar has no line; `7-8 = 5-6` says bars 7-8 repeat 5-6.
//!
//! [`to_text`] writes an [`Arrangement`] (an imported MIDI file, or a Score
//! played back by [`from_score`]); [`from_text`] reads one back, so a song
//! written in this form becomes a Score ([`crate::arrangement::to_score`]).
//! Velocities read as three levels, times as sixteenths, pitches as
//! semitones, and pedal and program changes are left out: a transcription,
//! not a recording; text written from text is the same text.

use crate::arrangement::{self, Arrangement, Note, Part, Section};
use crate::gm::{self, Role, Voice};
use std::collections::BTreeMap;
use std::fmt::Write;

/// Steps per beat (a quarter note).
pub const STEPS: f64 = 4.0;

/// What a transcription shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    /// Only these parts (names, or roles: `drums`, `bass`, `melody`).
    pub parts: Option<Vec<String>>,
    /// Only bars `from..=to` (1-based).
    pub bars: Option<(usize, usize)>,
    pub chords: bool,
    /// The pitched parts' lines.
    pub notes: bool,
    pub drums: bool,
    /// Repeated bars as `7-8 = 5-6`.
    pub collapse: bool,
}

impl Default for Detail {
    fn default() -> Self {
        Detail { parts: None, bars: None, chords: true, notes: true, drums: true, collapse: true }
    }
}

impl Detail {
    /// `full`, `chords` (the chord line only), `drums`, `melody` (the tune
    /// and the chords), `bass`.
    pub fn named(name: &str) -> Detail {
        match name {
            "chords" => Detail { notes: false, drums: false, ..Detail::default() },
            "drums" => Detail { chords: false, notes: false, ..Detail::default() },
            "melody" => Detail { parts: Some(vec!["melody".into()]), drums: false, ..Detail::default() },
            "bass" => Detail { parts: Some(vec!["bass".into()]), drums: false, ..Detail::default() },
            _ => Detail::default(),
        }
    }

    fn shows(&self, p: &Part) -> bool {
        let kind = if p.role == Role::Drums { self.drums } else { self.notes };
        kind && self.parts.as_ref().is_none_or(|names| names.iter().any(|n| *n == p.name || *n == p.role.name() || (n == "melody" && p.melody) || p.name.starts_with(&format!("{n}_"))))
    }
}

const SHARPS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLATS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

fn pitch(key: u8, flats: bool) -> String {
    let names = if flats { FLATS } else { SHARPS };
    format!("{}{}", names[key as usize % 12], key as i32 / 12 - 1)
}

fn parse_pitch(text: &str) -> Option<u8> {
    let b = text.as_bytes();
    let letter = match b.first()?.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let (acc, rest) = match b.get(1) {
        Some(b'#') | Some(b's') => (1, &text[2..]),
        Some(b'b') if text.len() > 2 && (b[2] == b'-' || b[2].is_ascii_digit()) => (-1, &text[2..]),
        _ => (0, &text[1..]),
    };
    let octave: i32 = rest.parse().ok()?;
    let key = (octave + 1) * 12 + letter + acc;
    (0..=127).contains(&key).then_some(key as u8)
}

/// Drum rows: label and GM note.
const DRUMS: [(&str, u8); 17] = [
    ("K", 36), ("S", 38), ("R", 37), ("C", 39), ("H", 42), ("P", 44), ("O", 46), ("T1", 50), ("T2", 47), ("T3", 45), ("FT", 43), ("CR", 49), ("RD", 51), ("BL", 53), ("CB", 56), ("SH", 70), ("CG", 63),
];

/// The row a GM percussion note is written on (close pieces share one).
fn drum_label(note: u8) -> String {
    let near = match note {
        35 => 36,
        40 => 38,
        41 => 43,
        48 => 47,
        52 | 55 | 57 => 49,
        59 => 51,
        n => n,
    };
    DRUMS.iter().find(|d| d.1 == near).map_or(format!("N{note}"), |d| d.0.to_string())
}

fn drum_note(label: &str) -> Option<u8> {
    DRUMS.iter().find(|d| d.0 == label).map(|d| d.1).or_else(|| label.strip_prefix('N')?.parse().ok())
}

/// Three loudness levels: soft, normal, loud.
fn level_of(vel: f64) -> u8 {
    if vel >= 0.85 {
        2
    } else if vel < 0.45 {
        0
    } else {
        1
    }
}

const LEVEL_VEL: [f64; 3] = [0.35, 0.7, 0.95];

// ------------------------------------------------------------------ chords

const QUALITIES: [(&str, &[u8]); 14] = [
    ("", &[0, 4, 7]),
    ("m", &[0, 3, 7]),
    ("7", &[0, 4, 7, 10]),
    ("maj7", &[0, 4, 7, 11]),
    ("m7", &[0, 3, 7, 10]),
    ("6", &[0, 4, 7, 9]),
    ("m6", &[0, 3, 7, 9]),
    ("dim", &[0, 3, 6]),
    ("m7b5", &[0, 3, 6, 10]),
    ("aug", &[0, 4, 8]),
    ("sus4", &[0, 5, 7]),
    ("sus2", &[0, 2, 7]),
    ("add9", &[0, 2, 4, 7]),
    ("5", &[0, 7]),
];

/// The chord of pitch-class weights (and the lowest pitch class), or `N`.
pub fn chord_name(weights: &[f64; 12], bass: Option<u8>, flats: bool) -> String {
    let total: f64 = weights.iter().sum();
    if total <= 1e-9 {
        return "N".into();
    }
    let names = if flats { FLATS } else { SHARPS };
    let mut best = (f64::MIN, 0usize, 0usize);
    for root in 0..12 {
        for (q, (_, tones)) in QUALITIES.iter().enumerate() {
            let inside: f64 = tones.iter().map(|t| weights[(root + *t as usize) % 12]).sum();
            let outside = total - inside;
            // Every tone of the chord should sound; the root weighs most.
            let missing = tones.iter().filter(|t| weights[(root + **t as usize) % 12] < total * 0.04).count() as f64;
            let mut score = inside - outside * 0.6 - missing * total * 0.25 + weights[root] * 0.3 - tones.len() as f64 * total * 0.02;
            if bass == Some(root as u8) {
                score += total * 0.12;
            }
            if score > best.0 {
                best = (score, root, q);
            }
        }
    }
    let mut name = format!("{}{}", names[best.1], QUALITIES[best.2].0);
    if let Some(b) = bass {
        if b as usize != best.1 && QUALITIES[best.2].1.iter().any(|t| (best.1 + *t as usize) % 12 == b as usize) {
            name.push('/');
            name.push_str(names[b as usize]);
        }
    }
    name
}

/// The chords of `bar`, a beat at a time (`.` continues the beat before).
fn chord_line(a: &Arrangement, bar: usize) -> String {
    let start = a.bars[bar];
    let end = a.bars[bar + 1];
    let flats = a.flats();
    let mut out: Vec<String> = Vec::new();
    let mut last = String::new();
    let mut beat = start;
    // The tune is left out: its passing notes are not the harmony.
    let harmony: Vec<&Part> = a.parts.iter().filter(|p| p.role != Role::Drums && !p.melody).collect();
    let sources: Vec<&Part> = if harmony.is_empty() { a.parts.iter().filter(|p| p.role != Role::Drums).collect() } else { harmony };
    while beat < end - 1e-9 {
        let to = (beat + 1.0).min(end);
        let mut w = [0f64; 12];
        let mut low: Option<(u8, f64)> = None;
        for p in &sources {
            for n in &p.notes {
                let overlap = (n.beat + n.dur).min(to) - n.beat.max(beat);
                if overlap > 1e-6 {
                    let k = n.key.round().clamp(0.0, 127.0) as u8;
                    w[k as usize % 12] += overlap;
                    if low.is_none_or(|l| k < l.0) {
                        low = Some((k, overlap));
                    }
                }
            }
        }
        let name = chord_name(&w, low.map(|l| l.0 % 12), flats);
        out.push(if name == last && !out.is_empty() { ".".into() } else { name.clone() });
        last = name;
        beat = to;
    }
    out.join(" ")
}

// ------------------------------------------------------------------ writing

/// A part's steps for a bar range, as onset groups: step -> (keys, level,
/// steps held).
fn groups(p: &Part) -> BTreeMap<i64, (Vec<u8>, u8, i64, String)> {
    let mut out: BTreeMap<i64, (Vec<u8>, u8, i64, String)> = BTreeMap::new();
    for n in &p.notes {
        let s = (n.beat * STEPS).round() as i64;
        let len = ((n.dur * STEPS).round() as i64).max(1);
        let k = n.key.round().clamp(0.0, 127.0) as u8;
        let g = out.entry(s).or_insert((Vec::new(), 0, 0, String::new()));
        if !g.0.contains(&k) {
            g.0.push(k);
        }
        if g.3.is_empty() {
            if let Some(fx) = &n.fx {
                g.3 = fx_marks(fx);
            }
        }
        g.1 = g.1.max(level_of(n.vel));
        g.2 = g.2.max(len);
    }
    // A group ends where the next begins.
    let starts: Vec<i64> = out.keys().copied().collect();
    for (i, s) in starts.iter().enumerate() {
        if let Some(next) = starts.get(i + 1) {
            let g = out.get_mut(s).unwrap();
            g.2 = g.2.min(next - s);
        }
        out.get_mut(s).unwrap().0.sort();
    }
    out
}

/// What a chip or tracker note does while it sounds, as marks after its
/// pitch: `:arp=0.4.7` (an arpeggio's offsets), `:vib` (vibrato),
/// `:bend=+2` / `:bend=-12` (where a slide or fall ends, in semitones),
/// `:env` (its level moves: a fade, a volume slide), `:sweep` (a filter
/// sweep). `C4:arp=0.3.7:vib!` is a loud C4 arpeggiating a minor chord
/// with vibrato.
fn fx_marks(fx: &arrangement::NoteFx) -> String {
    let mut out = String::new();
    if fx.arp_n > 0 {
        out.push_str(&format!(":arp={}", fx.arp().iter().map(|o| arrangement::num(*o as f64, 2)).collect::<Vec<_>>().join(".")));
    }
    if fx.vib.1 > 0.0 {
        out.push_str(":vib");
    }
    if let Some(&(_, end)) = fx.bend().last() {
        if end.abs() >= 0.5 {
            out.push_str(&format!(":bend={}{}", if end > 0.0 { "+" } else { "" }, end.round() as i32));
        }
    }
    if fx.level().iter().any(|p| (p.1 - 1.0).abs() > 0.2) {
        out.push_str(":env");
    }
    if fx.cutoff_n > 1 {
        out.push_str(":sweep");
    }
    out
}

/// The marks read back as note options (a transcription: an arpeggio a
/// 64th a step, a gentle vibrato, a slide over a beat, a fall to silence).
fn parse_marks(marks: &str) -> Option<arrangement::NoteFx> {
    let mut fx = arrangement::NoteFx::default();
    for m in marks.split(':').filter(|m| !m.is_empty()) {
        let (k, v) = m.split_once('=').unwrap_or((m, ""));
        match k {
            "arp" => {
                for (i, o) in v.split('.').filter_map(|o| o.parse::<f32>().ok()).take(4).enumerate() {
                    fx.arp[i] = o;
                    fx.arp_n = i as u8 + 1;
                }
                fx.arp_step = 0.0625;
            }
            "vib" => fx.vib = (5.5, 0.25, 0.0),
            "bend" => {
                if let Ok(end) = v.trim_start_matches('+').parse::<f32>() {
                    fx.bend[0] = (0.0, 0.0);
                    fx.bend[1] = (1.0, end);
                    fx.bend_n = 2;
                }
            }
            "env" => {
                fx.level[0] = (0.0, 1.0);
                fx.level[1] = (1.0, 0.3);
                fx.level_n = 2;
            }
            _ => {}
        }
    }
    (!fx.is_plain()).then_some(fx)
}

fn pitched_line(p: &Part, g: &BTreeMap<i64, (Vec<u8>, u8, i64, String)>, from: i64, to: i64, flats: bool) -> Option<String> {
    let mut tokens = Vec::new();
    let mut any = false;
    // What sounds at the bar's start from before it.
    let mut sounding_until = g.range(..from).next_back().map_or(from, |(s, grp)| s + grp.2);
    for step in from..to {
        if let Some((keys, level, len, fx)) = g.get(&step) {
            let mut t = keys.iter().map(|k| pitch(*k, flats)).collect::<Vec<_>>().join("+");
            t.push_str(fx);
            t.push_str(["?", "", "!"][*level as usize]);
            tokens.push(t);
            sounding_until = step + len;
            any = true;
        } else if step < sounding_until {
            tokens.push("~".into());
            any = true;
        } else {
            tokens.push(".".into());
        }
    }
    let _ = p;
    any.then(|| tokens.join(" "))
}

fn drum_line(p: &Part, from: i64, to: i64) -> Option<String> {
    let mut rows: BTreeMap<usize, (String, Vec<u8>)> = BTreeMap::new();
    let width = (to - from) as usize;
    for n in &p.notes {
        let s = (n.beat * STEPS).round() as i64;
        if s < from || s >= to {
            continue;
        }
        let k = n.key.round().clamp(0.0, 127.0) as u8;
        let label = drum_label(k);
        let order = DRUMS.iter().position(|d| d.0 == label).unwrap_or(DRUMS.len() + k as usize);
        let row = rows.entry(order).or_insert_with(|| (label, vec![b'.'; width]));
        let c = [b'o', b'x', b'X'][level_of(n.vel) as usize];
        let at = (s - from) as usize;
        // The loudest hit on a step wins.
        let rank = |c: u8| match c {
            b'X' => 3,
            b'x' => 2,
            b'o' => 1,
            _ => 0,
        };
        if rank(c) > rank(row.1[at]) {
            row.1[at] = c;
        }
    }
    if rows.is_empty() {
        return None;
    }
    let per_beat = STEPS as usize;
    let text: Vec<String> = rows
        .values()
        .map(|(label, steps)| {
            let beats: Vec<String> = steps.chunks(per_beat).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
            format!("{label} {}", beats.join(" "))
        })
        .collect();
    let _ = p;
    Some(text.join(" | "))
}

/// The header lines: title, tempo, meter, key, form and the parts.
fn header(a: &Arrangement, d: &Detail) -> String {
    let mut w = String::new();
    let year = a.year.map_or(String::new(), |y| format!(" ({y})"));
    let by = if a.artist.is_empty() { String::new() } else { format!(" -- {}", a.artist) };
    let _ = writeln!(w, "# {}{by}{year}", a.title);
    let _ = writeln!(w, "tempo {} | meter {}/{} | key {} | grid {}", arrangement::num(a.bpm, 2), a.sig.0, a.sig.1, if a.key.is_empty() { "-" } else { &a.key }, (a.per_bar() * STEPS).round());
    let form: Vec<String> = a.sections.iter().map(|s| format!("{} {}{}", s.name, s.letter, s.to - s.from)).collect();
    let _ = writeln!(w, "form {}", form.join(", "));
    for p in a.parts.iter().filter(|p| d.shows(p)) {
        let _ = writeln!(w, "part {} {}{} {} amp={} pan={}", p.name, p.role.name(), if p.melody { "*" } else { "" }, voice_text(&p.voice), arrangement::num(p.amp, 2), arrangement::num(p.pan, 2));
    }
    if a.parts.iter().any(|p| p.notes.iter().any(|n| n.fx.is_some())) {
        let _ = writeln!(w, "marks :arp=0.4.7 arpeggio offsets | :vib vibrato | :bend=+2 slide/fall end (semitones) | :env volume moves | :sweep filter sweep");
    }
    w
}

fn voice_text(v: &Voice) -> String {
    match v {
        Voice::Preset(name) => format!("preset:{name}"),
        Voice::Gm(program) => format!("gm:{program}"),
        Voice::Kit(kit) => format!("kit:{kit}"),
        Voice::ModelledKit => "kit:modelled".into(),
        Voice::GmKit(program) => format!("kit:gm{program}"),
        Voice::Other(about) => format!("other:{}", about.replace(' ', "_")),
        // The node as written, its spaces as `+` (a node never has one).
        Voice::Node(node) => format!("node:{}", node.replace(' ', "+")),
    }
}

fn parse_voice(text: &str) -> Voice {
    let (kind, value) = text.split_once(':').unwrap_or((text, ""));
    let sounds = makepad_score_live::palette::sounds::sounds();
    let preset = |name: &str| sounds.patch(name).map(|p| p.name.as_str());
    match kind {
        "preset" => preset(value).map_or(Voice::Gm(0), Voice::Preset),
        "gm" => Voice::Gm(value.parse().unwrap_or(0)),
        "kit" => match value {
            "modelled" => Voice::ModelledKit,
            gm if gm.starts_with("gm") => Voice::GmKit(gm[2..].parse().unwrap_or(0)),
            other => sounds.kit(other).map(|k| k.name.as_str()).map_or(Voice::ModelledKit, Voice::Kit),
        },
        "node" => Voice::Node(value.replace('+', " ")),
        _ => Voice::Other(value.replace('_', " ")),
    }
}

/// A played note's options as a [`NoteFx`](arrangement::NoteFx), when it
/// has any (`pan` is its pan from the loop's).
fn note_fx(o: &makepad_score_player::NoteOpts, pan: f32) -> Option<arrangement::NoteFx> {
    let mut fx = arrangement::NoteFx { pan: if pan.abs() > 1e-4 { pan } else { 0.0 }, ..Default::default() };
    for (i, p) in o.bend.pts[..o.bend.n as usize].iter().enumerate().take(arrangement::FX_POINTS) {
        fx.bend[i] = (p[0], p[1]);
        fx.bend_n = i as u8 + 1;
    }
    for (i, p) in o.env.pts[..o.env.n as usize].iter().enumerate().take(arrangement::FX_POINTS) {
        fx.level[i] = (p[0], p[1]);
        fx.level_n = i as u8 + 1;
    }
    for (i, p) in o.cutoff_env.pts[..o.cutoff_env.n as usize].iter().enumerate().take(arrangement::FX_POINTS) {
        fx.cutoff[i] = (p[0], p[1]);
        fx.cutoff_n = i as u8 + 1;
    }
    if o.vib_depth > 0.0 {
        fx.vib = (o.vib_rate, o.vib_depth, o.vib_delay);
    }
    fx.arp_n = o.arp_n.min(4);
    fx.arp[..fx.arp_n as usize].copy_from_slice(&o.arp[..fx.arp_n as usize]);
    fx.arp_step = o.arp_step;
    if o.start.is_finite() {
        fx.start = o.start.max(0.0);
    }
    (!fx.is_plain()).then_some(fx)
}

/// The transcription of `a` at `d`'s detail.
pub fn to_text(a: &Arrangement, d: &Detail) -> String {
    let mut w = header(a, d);
    let flats = a.flats();
    let shown: Vec<&Part> = a.parts.iter().filter(|p| d.shows(p)).collect();
    let name_width = shown.iter().map(|p| p.name.len()).max().unwrap_or(4);
    let grouped: Vec<BTreeMap<i64, (Vec<u8>, u8, i64, String)>> = shown.iter().map(|p| if p.role == Role::Drums { BTreeMap::new() } else { groups(p) }).collect();
    let (first, last) = d.bars.map_or((0, a.bar_count()), |(f, t)| (f.saturating_sub(1).min(a.bar_count()), t.min(a.bar_count())));
    let standard = a.per_bar();
    // Each bar's lines (without its number), for the repeats.
    let mut bodies: Vec<String> = Vec::with_capacity(last.saturating_sub(first));
    let mut heads: Vec<String> = Vec::new();
    for bar in first..last {
        let from = (a.bars[bar] * STEPS).round() as i64;
        let to = (a.bars[bar + 1] * STEPS).round() as i64;
        let len = a.bars[bar + 1] - a.bars[bar];
        let meter = if (len - standard).abs() > 1e-6 { format!(" ({}/4)", arrangement::num(len, 2)) } else { String::new() };
        let mut head = meter;
        if d.chords {
            head.push_str(&format!(" | {} |", chord_line(a, bar)));
        }
        let tempo: Vec<String> = a.tempo.iter().filter(|t| t.0 >= a.bars[bar] - 1e-9 && t.0 < a.bars[bar + 1] - 1e-9).map(|t| {
            let at = t.0 - a.bars[bar];
            if at.abs() < 1e-9 { format!(" tempo {}", arrangement::num(t.1, 2)) } else { format!(" tempo {}@{}", arrangement::num(t.1, 2), arrangement::num(at, 3)) }
        }).collect();
        head.push_str(&tempo.concat());
        let mut body = String::new();
        for (p, g) in shown.iter().zip(&grouped) {
            let line = if p.role == Role::Drums { drum_line(p, from, to) } else { pitched_line(p, g, from, to, flats) };
            if let Some(line) = line {
                let _ = writeln!(body, "  {:<name_width$} {line}", p.name);
            }
        }
        heads.push(head);
        bodies.push(body);
    }
    let section_at = |bar: usize| a.sections.iter().find(|s| s.from == bar);
    let mut i = 0;
    while i < bodies.len() {
        let bar = first + i;
        if let Some(s) = section_at(bar) {
            let _ = writeln!(w, "== {} [{}] bars {}-{}", s.name, s.letter, s.from + 1, s.to);
        }
        // A run of bars that repeats an earlier run (within this section's
        // bars, not across a section start).
        if d.collapse && !bodies[i].is_empty() {
            let mut best: Option<(usize, usize)> = None;
            for j in 0..i {
                let mut n = 0;
                while i + n < bodies.len() && j + n < i && bodies[j + n] == bodies[i + n] && heads[j + n] == heads[i + n] && (n == 0 || section_at(first + i + n).is_none()) {
                    n += 1;
                }
                if n > 0 && best.is_none_or(|b| n > b.1) {
                    best = Some((j, n));
                }
            }
            if let Some((j, n)) = best {
                let span = |a: usize, n: usize| if n == 1 { format!("{}", a + 1) } else { format!("{}-{}", a + 1, a + n) };
                let _ = writeln!(w, "{} = {}", span(bar, n), span(first + j, n));
                i += n;
                continue;
            }
        }
        let _ = writeln!(w, "{}{}", bar + 1, heads[i]);
        w.push_str(&bodies[i]);
        i += 1;
    }
    w
}

// ------------------------------------------------------------------ reading

/// Read a transcription back into an arrangement.
pub fn from_text(text: &str) -> Result<Arrangement, String> {
    let mut a = Arrangement {
        title: String::new(),
        artist: String::new(),
        year: None,
        genre: String::new(),
        key: String::new(),
        bpm: 120.0,
        sig: (4, 4),
        bars: vec![0.0],
        parts: Vec::new(),
        sections: Vec::new(),
        tempo: Vec::new(),
        notes: vec!["Written from a text transcription.".into()],
        rights: None,
        limiter_gain: 1.6,
        excerpt: false,
    };
    // Bars as written: number -> (length, lines).
    let mut bars: BTreeMap<usize, (f64, Vec<(String, String)>, Vec<(f64, f64)>)> = BTreeMap::new();
    let mut current: Option<usize> = None;
    let mut sections: Vec<(String, char, usize, usize)> = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let err = |m: &str| format!("line {}: {m}: {t}", n + 1);
        if let Some(rest) = t.strip_prefix("# ") {
            let (head, year) = match rest.rsplit_once(" (") {
                Some((h, y)) if y.ends_with(')') && y[..y.len() - 1].parse::<u16>().is_ok() => (h, y[..y.len() - 1].parse().ok()),
                _ => (rest, None),
            };
            let (title, artist) = head.split_once(" -- ").unwrap_or((head, ""));
            a.title = title.trim().into();
            a.artist = artist.trim().into();
            a.year = year;
        } else if t.starts_with("tempo ") && current.is_none() {
            for field in t.split('|').map(str::trim) {
                if let Some(v) = field.strip_prefix("tempo ") {
                    a.bpm = v.trim().parse().map_err(|_| err("tempo is a number"))?;
                } else if let Some(v) = field.strip_prefix("meter ") {
                    let (num, den) = v.trim().split_once('/').ok_or_else(|| err("meter is n/d"))?;
                    a.sig = (num.parse().map_err(|_| err("meter"))?, den.parse().map_err(|_| err("meter"))?);
                } else if let Some(v) = field.strip_prefix("key ") {
                    a.key = if v.trim() == "-" { String::new() } else { v.trim().into() };
                }
            }
        } else if t.starts_with("form ") || t.starts_with("marks ") || t.starts_with("//") {
            continue;
        } else if let Some(rest) = t.strip_prefix("part ") {
            let fields: Vec<&str> = rest.split_whitespace().collect();
            let (Some(name), Some(role)) = (fields.first(), fields.get(1)) else { return Err(err("part <name> <role> <voice>")) };
            let melody = role.ends_with('*');
            let role = Role::parse(role.trim_end_matches('*')).ok_or_else(|| err("no such role"))?;
            let mut part = Part { name: name.to_string(), role, melody, voice: fields.get(2).map_or(Voice::Gm(0), |v| parse_voice(v)), about: "written as text".into(), notes: Vec::new(), pedal: Vec::new(), programs: Vec::new(), amp: arrangement::level(role, melody), pan: 0.0 };
            for f in &fields[2.min(fields.len())..] {
                if let Some(v) = f.strip_prefix("amp=") {
                    part.amp = v.parse().unwrap_or(part.amp);
                } else if let Some(v) = f.strip_prefix("pan=") {
                    part.pan = v.parse().unwrap_or(0.0);
                }
            }
            a.parts.push(part);
        } else if let Some(rest) = t.strip_prefix("== ") {
            let mut words = rest.split_whitespace();
            let name = words.next().ok_or_else(|| err("== <name> [L] bars a-b"))?.to_string();
            let letter = words.next().and_then(|l| l.trim_matches(|c| c == '[' || c == ']').chars().next()).unwrap_or('A');
            let range = words.skip_while(|w| *w != "bars").nth(1).ok_or_else(|| err("bars a-b"))?;
            let (f, to) = range.split_once('-').unwrap_or((range, range));
            sections.push((name, letter, f.parse::<usize>().map_err(|_| err("bars"))?.saturating_sub(1), to.parse().map_err(|_| err("bars"))?));
        } else if t.as_bytes()[0].is_ascii_digit() {
            // `5 | chords |`, `5 (3/4) | ...`, `7-8 = 5-6`, `7 = 5`.
            if let Some((left, right)) = t.split_once(" = ") {
                let span = |s: &str| -> Option<(usize, usize)> {
                    let (a, b) = s.trim().split_once('-').unwrap_or((s.trim(), s.trim()));
                    Some((a.parse().ok()?, b.parse().ok()?))
                };
                let (Some((a0, a1)), Some((b0, _))) = (span(left), span(right)) else { return Err(err("a repeat is `7-8 = 5-6`")) };
                for k in 0..=(a1.saturating_sub(a0)) {
                    let src = bars.get(&(b0 + k)).cloned().ok_or_else(|| err("repeats a bar not written before"))?;
                    bars.insert(a0 + k, src);
                }
                current = None;
                continue;
            }
            let number_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
            let number: usize = t[..number_end].parse().map_err(|_| err("bar number"))?;
            let rest = &t[number_end..];
            let mut len = a.per_bar();
            if let Some(m) = rest.trim_start().strip_prefix('(') {
                if let Some(end) = m.find("/4)") {
                    len = m[..end].parse().map_err(|_| err("a bar's length is (beats/4)"))?;
                }
            }
            let mut tempo = Vec::new();
            for (i, word) in rest.split_whitespace().enumerate() {
                if word == "tempo" {
                    if let Some(v) = rest.split_whitespace().nth(i + 1) {
                        let (bpm, at) = v.split_once('@').unwrap_or((v, "0"));
                        tempo.push((at.parse().unwrap_or(0.0), bpm.parse().map_err(|_| err("tempo"))?));
                    }
                }
            }
            bars.insert(number, (len, Vec::new(), tempo));
            current = Some(number);
        } else if line.starts_with(' ') {
            let bar = current.ok_or_else(|| err("a part line outside a bar"))?;
            let (name, rest) = t.split_once(char::is_whitespace).ok_or_else(|| err("<part> <steps>"))?;
            bars.get_mut(&bar).unwrap().1.push((name.to_string(), rest.trim().to_string()));
        } else {
            return Err(err("not a line of a transcription"));
        }
    }
    // Bar starts (bars not written are silent bars of the standard length).
    let count = bars.keys().last().copied().unwrap_or(0);
    let mut starts = vec![0.0];
    for b in 1..=count {
        let len = bars.get(&b).map_or(a.per_bar(), |x| x.0);
        starts.push(starts[b - 1] + len);
    }
    a.bars = starts;
    // Notes: each part's groups across bars.
    for pi in 0..a.parts.len() {
        let drums = a.parts[pi].role == Role::Drums;
        let name = a.parts[pi].name.clone();
        let mut notes: Vec<Note> = Vec::new();
        // The group sounding: (note indices, start step).
        let mut open: Vec<usize> = Vec::new();
        for b in 1..=count {
            let start = (a.bars[b - 1] * STEPS).round() as i64;
            let end = (a.bars[b] * STEPS).round() as i64;
            let line = bars.get(&b).and_then(|x| x.1.iter().find(|(n, _)| *n == name)).map(|x| x.1.clone());
            if drums {
                let Some(line) = line else { continue };
                for row in line.split('|') {
                    let mut words = row.split_whitespace();
                    let Some(label) = words.next() else { continue };
                    let key = drum_note(label).ok_or_else(|| format!("bar {b}: no drum `{label}`"))?;
                    let steps: Vec<u8> = words.flat_map(|w| w.bytes()).collect();
                    for (i, c) in steps.iter().enumerate() {
                        let level = match c {
                            b'X' => 2,
                            b'x' => 1,
                            b'o' => 0,
                            _ => continue,
                        };
                        notes.push(Note { beat: (start + i as i64) as f64 / STEPS, key: key as f32, dur: 1.0 / STEPS, vel: LEVEL_VEL[level], fx: None });
                    }
                }
                continue;
            }
            let tokens: Vec<String> = line.map(|l| l.split_whitespace().map(str::to_string).collect()).unwrap_or_else(|| vec![".".to_string(); (end - start) as usize]);
            for (i, tok) in tokens.iter().enumerate() {
                let step = start + i as i64;
                match tok.as_str() {
                    "~" => {
                        for &n in &open {
                            notes[n].dur += 1.0 / STEPS;
                        }
                    }
                    "." => open.clear(),
                    t => {
                        open.clear();
                        let (body, level) = match t.strip_suffix('!') {
                            Some(b) => (b, 2),
                            None => match t.strip_suffix('?') {
                                Some(b) => (b, 0),
                                None => (t, 1),
                            },
                        };
                        let (body, marks) = body.split_once(':').map_or((body, ""), |(p, m)| (p, m));
                        let fx = parse_marks(marks);
                        for p in body.split('+') {
                            let key = parse_pitch(p).ok_or_else(|| format!("bar {b}, part {name}: `{p}` is not a pitch (C4, F#3, Bb2)"))?;
                            open.push(notes.len());
                            notes.push(Note { beat: step as f64 / STEPS, key: key as f32, dur: 1.0 / STEPS, vel: LEVEL_VEL[level], fx });
                        }
                    }
                }
            }
        }
        a.parts[pi].notes = notes;
    }
    for (b, (_, _, tempo)) in &bars {
        for (at, bpm) in tempo {
            a.tempo.push((a.bars[b - 1] + at, *bpm));
        }
    }
    a.sections = sections.into_iter().map(|(name, letter, from, to)| Section { name: name.clone(), kind: name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '_').to_string(), letter, from, to: to.min(count) }).collect();
    if a.sections.is_empty() && count > 0 {
        a.sections.push(Section { name: "song".into(), kind: "song".into(), letter: 'A', from: 0, to: count });
    }
    Ok(a)
}

// ------------------------------------------------------------------ scores

/// A Score as an arrangement, by playing it: every note its loops play
/// over the arrangement (16 bars without one), each loop a part, its role
/// from its instrument and register. For reading a score in the same form
/// as an imported song (`score_text`); its instruments and code stay in
/// the Score.
pub fn from_score(source: &str) -> Result<Arrangement, String> {
    use makepad_score_live::model::InstKind;
    let loaded = makepad_score_live::model::load(source).map_err(|d| d.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n"))?;
    let doc = loaded.doc.clone();
    let per_bar = doc.beats_per_bar();
    let beats = if doc.arrangement_beats() > 0.0 { doc.arrangement_beats() } else { 16.0 * per_bar };
    let mut composer = makepad_score_live::compose::Composer::new(loaded);
    let mut events = Vec::new();
    composer.pump(beats, &mut events);
    let mut parts: Vec<Part> = Vec::new();
    for l in &doc.loops {
        let key = makepad_score_live::key(&l.name);
        let inst = l.play.as_deref().and_then(|p| doc.inst(p));
        let mut notes: Vec<Note> = Vec::new();
        let mut drums = matches!(inst.map(|i| &i.kind), Some(InstKind::Drums { .. })) || matches!(inst.map(|i| &i.kind), Some(InstKind::Native { native, bank: 128, .. }) if native == "soundfont") || matches!(inst.map(|i| &i.kind), Some(InstKind::Native { native, .. }) if native == "drums");
        // A loop's amp scales its notes' velocities; the part's own are
        // before it.
        let amp = if l.amp > 1e-6 { l.amp } else { 1.0 };
        for e in events.iter().filter(|e| e.loop_key == key && e.beat < beats) {
            match e.kind {
                makepad_score_player::EventKind::Note => notes.push(Note { beat: e.beat, key: e.note, dur: e.dur as f64, vel: (e.vel / amp).clamp(0.0, 1.0) as f64, fx: note_fx(&e.opts, e.pan - l.pan) }),
                makepad_score_player::EventKind::Hit(_, gm) => {
                    drums = true;
                    notes.push(Note { beat: e.beat, key: gm as f32, dur: 0.25, vel: (e.vel / amp).clamp(0.0, 1.0) as f64, fx: None });
                }
                makepad_score_player::EventKind::Sung(..) => notes.push(Note { beat: e.beat, key: e.note, dur: e.dur as f64, vel: (e.vel / amp).clamp(0.0, 1.0) as f64, fx: None }),
                _ => {}
            }
        }
        if notes.is_empty() {
            continue;
        }
        notes.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        let mean = notes.iter().map(|n| n.key as f64).sum::<f64>() / notes.len() as f64;
        let (role, voice, about) = match inst.map(|i| &i.kind) {
            _ if drums => (Role::Drums, Voice::ModelledKit, "drums".to_string()),
            Some(InstKind::Native { native, program, .. }) if native == "soundfont" => (gm::role(*program), Voice::Gm(*program), format!("GM {}", gm::PROGRAMS[*program as usize & 127])),
            Some(InstKind::Native { native, .. }) if native == "piano" => (Role::Keys, Voice::Other("the modelled piano".into()), "piano".into()),
            Some(InstKind::Voice { .. }) => (Role::Lead, Voice::Other("a sung voice".into()), "voice".into()),
            Some(k) => {
                let role = if mean < 50.0 { Role::Bass } else if notes.windows(2).filter(|w| (w[0].beat - w[1].beat).abs() < 1e-6).count() * 3 > notes.len() { Role::Keys } else { Role::Lead };
                let kind = match k {
                    InstKind::Synth { .. } => "a synth",
                    InstKind::Ironfish { .. } => "the Ironfish synth",
                    InstKind::Shader { .. } => "a shader instrument",
                    InstKind::Sampler { .. } => "a sampler",
                    _ => "an instrument",
                };
                (role, Voice::Other(kind.into()), kind.to_string())
            }
            None => (Role::Lead, Voice::Other("an instrument".into()), "an instrument".into()),
        };
        parts.push(Part { name: l.name.clone(), role, melody: matches!(inst.map(|i| &i.kind), Some(InstKind::Voice { .. })), voice, about: format!("the score's loop {} ({about})", l.name), notes, pedal: Vec::new(), programs: Vec::new(), amp: l.amp as f64, pan: l.pan as f64 });
    }
    let mut bars: Vec<f64> = Vec::new();
    let mut b = 0.0;
    while b < beats - 1e-9 {
        bars.push(b);
        b += per_bar;
    }
    bars.push(beats);
    let mut sections = Vec::new();
    let mut at = 0usize;
    for name in &doc.arrange {
        if let Some(s) = doc.sections.iter().find(|s| &s.name == name) {
            let n = s.bars.ceil().max(1.0) as usize;
            let letter = (b'A' + (doc.sections.iter().position(|x| x.name == s.name).unwrap_or(0) as u8).min(25)) as char;
            sections.push(Section { name: s.name.clone(), kind: s.name.clone(), letter, from: at, to: (at + n).min(bars.len() - 1) });
            at += n;
        }
    }
    let mut a = Arrangement {
        title: if doc.label.is_empty() { "Score".into() } else { doc.label.clone() },
        artist: String::new(),
        year: None,
        genre: String::new(),
        key: String::new(),
        bpm: doc.clock.bpm,
        sig: (doc.clock.sig.0 as u8, doc.clock.sig.1),
        bars,
        parts,
        sections,
        tempo: Vec::new(),
        notes: Vec::new(),
        rights: None,
        limiter_gain: 1.6,
        excerpt: false,
    };
    a.key = key_of(&a);
    Ok(a)
}

/// The key of an arrangement's notes (Krumhansl-Schmuckler).
fn key_of(a: &Arrangement) -> String {
    let mut hist = [0f64; 12];
    for p in a.parts.iter().filter(|p| p.role != Role::Drums) {
        for n in &p.notes {
            hist[n.key.round().clamp(0.0, 127.0) as usize % 12] += n.dur.max(0.05);
        }
    }
    crate::analyse::key_of_histogram(&hist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitches_and_drums_read_back() {
        for key in 0..=127u8 {
            for flats in [false, true] {
                assert_eq!(parse_pitch(&pitch(key, flats)), Some(key), "{}", pitch(key, flats));
            }
        }
        assert_eq!(parse_pitch("Bb-1"), Some(10));
        for (label, note) in DRUMS {
            assert_eq!(drum_label(note), label);
            assert_eq!(drum_note(label), Some(note));
        }
        assert_eq!(drum_label(54), "N54");
        assert_eq!(drum_note("N54"), Some(54));
    }

    #[test]
    fn chip_marks_read_back() {
        let fx = |arp: &[f32], vib: bool, bend: f32| {
            let mut f = arrangement::NoteFx::default();
            for (i, o) in arp.iter().enumerate() {
                f.arp[i] = *o;
                f.arp_n = i as u8 + 1;
            }
            f.arp_step = 0.0625;
            if vib {
                f.vib = (6.0, 0.3, 0.0);
            }
            if bend != 0.0 {
                f.bend[0] = (0.0, 0.0);
                f.bend[1] = (0.5, bend);
                f.bend_n = 2;
            }
            Some(f)
        };
        let mut a = Arrangement { title: "Chip".into(), artist: String::new(), year: None, genre: String::new(), key: String::new(), bpm: 120.0, sig: (4, 4), bars: vec![0.0, 4.0], parts: Vec::new(), sections: vec![Section { name: "a".into(), kind: "a".into(), letter: 'A', from: 0, to: 1 }], tempo: Vec::new(), notes: Vec::new(), rights: None, limiter_gain: 1.0, excerpt: false };
        a.parts.push(Part { name: "lead".into(), role: Role::Lead, melody: true, voice: Voice::Node("Synth{osc: @pulse width: 0.25}".into()), about: String::new(), notes: vec![
            Note { beat: 0.0, key: 60.0, dur: 1.0, vel: 0.95, fx: fx(&[0.0, 3.0, 7.0], true, 0.0) },
            Note { beat: 1.0, key: 64.0, dur: 1.0, vel: 0.7, fx: fx(&[], false, -12.0) },
        ], pedal: Vec::new(), programs: Vec::new(), amp: 0.6, pan: 0.0 });
        let text = to_text(&a, &Detail::default());
        assert!(text.contains("C4:arp=0.3.7:vib!") && text.contains("E4:bend=-12") && text.contains("marks :arp"), "{text}");
        let back = from_text(&text).unwrap();
        let n = &back.parts[0].notes;
        assert_eq!(n[0].fx.map(|f| f.arp().to_vec()), Some(vec![0.0, 3.0, 7.0]));
        assert!(n[0].fx.is_some_and(|f| f.vib.1 > 0.0));
        assert_eq!(n[1].fx.map(|f| f.bend()[1].1), Some(-12.0));
        assert!(matches!(&back.parts[0].voice, Voice::Node(node) if node == "Synth{osc: @pulse width: 0.25}"));
        // Written again, the same text.
        assert_eq!(to_text(&back, &Detail::default()), text);
    }

    #[test]
    fn chords_are_named() {
        let w = |pcs: &[usize]| {
            let mut w = [0f64; 12];
            for p in pcs {
                w[*p] = 1.0;
            }
            w
        };
        assert_eq!(chord_name(&w(&[9, 0, 4]), Some(9), false), "Am");
        assert_eq!(chord_name(&w(&[9, 0, 4, 7]), Some(9), false), "Am7");
        assert_eq!(chord_name(&w(&[5, 9, 0]), Some(5), false), "F");
        assert_eq!(chord_name(&w(&[7, 11, 2, 5]), Some(7), false), "G7");
        assert_eq!(chord_name(&w(&[0, 4, 7]), Some(4), false), "C/E");
        assert_eq!(chord_name(&[0.0; 12], None, false), "N");
    }

    /// A MIDI cover survives the round trip: its text, read back and
    /// written again, is the same text, and the Score the text makes plays
    /// the notes the text shows.
    #[test]
    fn a_cover_survives_the_round_trip() {
        let bytes = crate::analyse::tests::band();
        let about = crate::import::About { title: "Test Song".into(), artist: "The Band".into(), year: Some(1985), ..Default::default() };
        let imported = crate::import::import(&bytes, &about, &crate::import::Options::default()).unwrap();
        let text = to_text(&imported.arrangement, &Detail::default());
        let back = from_text(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        let again = to_text(&back, &Detail::default());
        assert_eq!(text, again);
        assert!(text.contains("= "), "repeated bars collapse:\n{text}");
        // The Score made from the text loads and plays every note it shows.
        let score = arrangement::to_score(&back);
        let loaded = makepad_score_live::model::load(&score).unwrap_or_else(|d| panic!("{d:?}\n{score}"));
        let mut composer = makepad_score_live::compose::Composer::new(loaded);
        let mut events = Vec::new();
        composer.pump(64.0, &mut events);
        let played = events.iter().filter(|e| e.kind == makepad_score_player::EventKind::Note).count();
        assert_eq!(played, back.parts.iter().map(|p| p.notes.len()).sum::<usize>());
        assert_eq!(played, imported.arrangement.parts.iter().map(|p| p.notes.len()).sum::<usize>(), "the test band sits on the grid: nothing merges");
        // And the Score read back by playing it says the same.
        let read = from_score(&score).unwrap();
        let only_notes = |t: &str| t.lines().filter(|l| l.starts_with("  ") || l.contains(" = ")).map(str::to_string).collect::<Vec<_>>();
        assert_eq!(only_notes(&to_text(&read, &Detail { chords: false, ..Detail::default() })), only_notes(&to_text(&back, &Detail { chords: false, ..Detail::default() })));
    }

    #[test]
    fn details_pick_parts_and_bars() {
        let bytes = crate::analyse::tests::band();
        let imported = crate::import::import(&bytes, &crate::import::About { title: "T".into(), ..Default::default() }, &crate::import::Options::default()).unwrap();
        let drums = to_text(&imported.arrangement, &Detail::named("drums"));
        assert!(drums.contains("  drums") && !drums.contains("  bass") && !drums.contains(" | Am"), "{drums}");
        let bars = to_text(&imported.arrangement, &Detail { bars: Some((3, 4)), collapse: false, ..Detail::default() });
        assert!(bars.lines().any(|l| l.starts_with("3 |")) && !bars.lines().any(|l| l.starts_with("1 |") || l.starts_with("5 |")), "{bars}");
    }
}
