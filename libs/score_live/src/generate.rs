//! Code generators: a loop's part written from a few musical choices, as
//! score code a person can read and keep editing (an arpeggio, a chord
//! progression, a Euclidean rhythm, ratchets) or the part it has rewritten
//! (transposed, moved into a scale, thinned or filled, humanised, given a
//! velocity curve). An editor panel's generator widgets call these, preview
//! the result by audition and write it as one edit. Every generator is a
//! pure function of its choices (and of the part it rewrites): a `seed`
//! picks the same "random" order every time.
//!
//! [`GENERATORS`] lists each with its parameters (what a widget draws);
//! [`generate`] answers the field edits on the loop.

use crate::model::ScoreDoc;
use crate::source;
use crate::theory;

/// What a parameter is.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    /// One of these names.
    Choice(&'static [&'static str]),
    /// A number in `min..=max` moving by `step`.
    Number { min: f64, max: f64, step: f64 },
    /// A note name (`a2`).
    Note,
    /// Words (`"1 5 6 4"`).
    Text,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamKind,
    /// Its value when none is given, as text.
    pub default: &'static str,
}

const fn choice(name: &'static str, options: &'static [&'static str], default: &'static str) -> Param {
    Param { name, kind: ParamKind::Choice(options), default }
}

const fn number(name: &'static str, min: f64, max: f64, step: f64, default: &'static str) -> Param {
    Param { name, kind: ParamKind::Number { min, max, step }, default }
}

const fn note(name: &'static str, default: &'static str) -> Param {
    Param { name, kind: ParamKind::Note, default }
}

const fn text(name: &'static str, default: &'static str) -> Param {
    Param { name, kind: ParamKind::Text, default }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Generator {
    pub name: &'static str,
    pub label: &'static str,
    /// What it does, in a line.
    pub about: &'static str,
    pub params: &'static [Param],
}

const ARP_PATTERNS: [&str; 6] = ["up", "down", "updown", "downup", "random", "chord"];
const RATES: [&str; 6] = ["1", "2", "3", "4", "6", "8"];
const VOICINGS: [&str; 4] = ["close", "open", "drop2", "spread"];
const RHYTHMS: [&str; 4] = ["whole", "half", "stabs", "pulse"];
const CURVES: [&str; 6] = ["flat", "rise", "fall", "accent", "swell", "random"];

pub const GENERATORS: [Generator; 9] = [
    Generator {
        name: "arp",
        label: "Arpeggio",
        about: "an arpeggio over a chord: pattern, rate (steps a beat), octaves, gate, swing, ratchets, a velocity curve",
        params: &[
            note("root", "a2"),
            choice("chord", &theory::CHORD_KINDS, "minor"),
            choice("pattern", &ARP_PATTERNS, "up"),
            choice("rate", &RATES, "4"),
            number("octaves", 1.0, 4.0, 1.0, "2"),
            number("gate", 0.1, 1.0, 0.05, "0.5"),
            number("swing", 0.0, 0.5, 0.01, "0"),
            number("repeats", 1.0, 4.0, 1.0, "1"),
            choice("curve", &CURVES, "accent"),
            number("beats", 1.0, 16.0, 1.0, "4"),
            number("seed", 0.0, 999.0, 1.0, "1"),
        ],
    },
    Generator {
        name: "chords",
        label: "Chord progression",
        about: "a progression on scale degrees in a key: chord size, voicing, inversion, rhythm, bars per chord",
        params: &[
            note("root", "c3"),
            choice("scale", &theory::SCALE_KINDS, "minor"),
            text("degrees", "1 6 3 7"),
            choice("size", &["triad", "seventh", "ninth"], "triad"),
            choice("voicing", &VOICINGS, "close"),
            number("inversion", 0.0, 3.0, 1.0, "0"),
            choice("rhythm", &RHYTHMS, "half"),
            number("bars", 1.0, 4.0, 1.0, "1"),
            choice("curve", &CURVES, "flat"),
        ],
    },
    Generator {
        name: "euclid",
        label: "Euclidean rhythm",
        about: "hits spread evenly over steps, rotated; ratchets repeat each hit",
        params: &[
            number("hits", 0.0, 32.0, 1.0, "5"),
            number("steps", 1.0, 32.0, 1.0, "16"),
            number("rotate", 0.0, 31.0, 1.0, "0"),
            choice("rate", &RATES, "4"),
            number("repeats", 1.0, 4.0, 1.0, "1"),
            choice("curve", &CURVES, "accent"),
        ],
    },
    Generator {
        name: "transpose",
        label: "Transpose",
        about: "every note of the part moved by semitones and octaves",
        params: &[number("semitones", -12.0, 12.0, 1.0, "0"), number("octaves", -3.0, 3.0, 1.0, "0")],
    },
    Generator {
        name: "scale",
        label: "Scale",
        about: "every note of the part moved to the nearest note of a scale on a root",
        params: &[note("root", "c4"), choice("scale", &theory::SCALE_KINDS, "minor")],
    },
    Generator {
        name: "density",
        label: "Density",
        about: "a hit pattern thinned (below 0.5, downbeats kept last) or filled (above 0.5, ghost notes between)",
        params: &[number("amount", 0.0, 1.0, 0.05, "0.5"), number("seed", 0.0, 999.0, 1.0, "1")],
    },
    Generator {
        name: "humanize",
        label: "Humanise",
        about: "timing (beats) and velocity nudged, the same every time",
        params: &[number("time", 0.0, 0.05, 0.001, "0.01"), number("vel", 0.0, 0.5, 0.01, "0.1")],
    },
    Generator {
        name: "velocity",
        label: "Velocity curve",
        about: "a hit pattern's velocities shaped over its steps",
        params: &[choice("curve", &CURVES, "accent"), number("amount", 0.0, 1.0, 0.05, "0.6"), number("seed", 0.0, 999.0, 1.0, "1")],
    },
    Generator {
        name: "swing",
        label: "Swing",
        about: "the loop's swing (every second step late)",
        params: &[number("amount", 0.0, 0.5, 0.01, "0.12")],
    },
];

/// The generator named `name`.
pub fn find(name: &str) -> Option<&'static Generator> {
    GENERATORS.iter().find(|g| g.name == name)
}

/// One field of the loop: set to code text, or removed (`None`).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldEdit {
    pub field: String,
    pub value: Option<String>,
}

fn set(field: &str, value: String) -> FieldEdit {
    FieldEdit { field: field.into(), value: Some(value) }
}

fn remove(field: &str) -> FieldEdit {
    FieldEdit { field: field.into(), value: None }
}

/// A parameter's value: as given, else its default.
struct Args<'a> {
    gen: &'a Generator,
    given: &'a [(String, String)],
}

impl Args<'_> {
    fn text(&self, name: &str) -> String {
        if let Some((_, v)) = self.given.iter().find(|(k, _)| k == name) {
            return v.trim().trim_start_matches('@').trim_matches('"').to_string();
        }
        self.gen.params.iter().find(|p| p.name == name).map_or(String::new(), |p| p.default.to_string())
    }

    fn num(&self, name: &str) -> f64 {
        let value = self.text(name).parse::<f64>().unwrap_or(0.0);
        match self.gen.params.iter().find(|p| p.name == name).map(|p| &p.kind) {
            Some(ParamKind::Number { min, max, step }) => {
                let v = value.clamp(*min, *max);
                if *step >= 1.0 { v.round() } else { v }
            }
            _ => value,
        }
    }

    fn note(&self, name: &str) -> Result<f32, String> {
        let t = self.text(name);
        theory::note_number(&t).ok_or_else(|| format!("`{t}` is not a note"))
    }
}

/// A small deterministic generator (the same seed, the same numbers).
struct Rng(u64);

impl Rng {
    fn new(seed: f64) -> Rng {
        Rng((seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }

    fn next(&mut self) -> f64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn num_text(v: f64) -> String {
    source::fmt_num(v, 3)
}

/// The velocity at step `i` of `n` under a curve.
fn curve(name: &str, i: usize, n: usize, amount: f64, rng: &mut Rng) -> f64 {
    let t = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
    let shape = match name {
        "rise" => 0.4 + 0.6 * t,
        "fall" => 1.0 - 0.6 * t,
        "accent" => if i % 4 == 0 { 1.0 } else if i % 2 == 0 { 0.75 } else { 0.55 },
        "swell" => 0.45 + 0.55 * (std::f64::consts::PI * t).sin(),
        "random" => 0.45 + 0.55 * rng.next(),
        _ => 0.8,
    };
    (0.8 + (shape - 0.8) * amount).clamp(0.05, 1.0)
}

fn note_list(notes: &[f32]) -> String {
    format!("[{}]", notes.iter().map(|n| format!("@{}", theory::note_name(*n))).collect::<Vec<_>>().join(" "))
}

/// The edits generator `name` makes on loop `node` of `src` with `given`
/// parameters (others at their defaults).
pub fn generate(src: &str, node: &str, name: &str, given: &[(String, String)]) -> Result<Vec<FieldEdit>, String> {
    let gen = find(name).ok_or_else(|| format!("no generator `{name}` (there are {})", GENERATORS.iter().map(|g| g.name).collect::<Vec<_>>().join(", ")))?;
    let args = Args { gen, given };
    let outline = source::outline(src);
    let ctor = outline.node(node).ok_or_else(|| format!("no node `{node}`"))?;
    if !matches!(ctor.type_name.as_str(), "Loop" | "Stinger") {
        return Err(format!("`{node}` is a {}, not a Loop", ctor.type_name));
    }
    let field = |key: &str| ctor.field(key).map(|f| f.value.text(src).to_string());
    match name {
        "arp" => {
            let root = args.note("root")?;
            let kind = theory::chord_kind(&args.text("chord")).ok_or("no such chord")?;
            let octaves = args.num("octaves") as i32;
            let mut tones: Vec<f32> = Vec::new();
            for o in 0..octaves.max(1) {
                tones.extend(theory::chord(root + 12.0 * o as f32, kind, 0));
            }
            let mut rng = Rng::new(args.num("seed"));
            let order: Vec<f32> = match args.text("pattern").as_str() {
                "down" => tones.iter().rev().copied().collect(),
                "updown" => tones.iter().copied().chain(tones.iter().rev().skip(1).take(tones.len().saturating_sub(2)).copied()).collect(),
                "downup" => tones.iter().rev().copied().chain(tones.iter().skip(1).take(tones.len().saturating_sub(2)).copied()).collect(),
                "random" => {
                    let mut t = tones.clone();
                    for i in (1..t.len()).rev() {
                        let j = (rng.next() * (i + 1) as f64) as usize;
                        t.swap(i, j.min(i));
                    }
                    t
                }
                // Chord tones in the order a player reaches: root, fifth, third, the rest.
                "chord" => {
                    let mut t = tones.clone();
                    t.sort_by_key(|n| (((n - root).rem_euclid(12.0) as i32) * 7 % 12, (*n * 10.0) as i32));
                    t
                }
                _ => tones.clone(),
            };
            let rate = args.num("rate").max(1.0);
            let repeats = args.num("repeats").max(1.0) as usize;
            let beats = args.num("beats").max(1.0);
            let steps = (beats * rate).round() as usize;
            let gate = args.num("gate");
            let curve_name = args.text("curve");
            let step = 1.0 / rate;
            let vels: Vec<String> = (0..steps).map(|i| num_text((curve(&curve_name, i, steps, 1.0, &mut rng) * 100.0).round() / 100.0)).collect();
            let play = if repeats > 1 {
                format!("for r in 0..{repeats} {{ play(notes[s % {}], {{dur: {} vel: vels[s]}}) sleep({}) }}", order.len(), num_text(step / repeats as f64 * gate), num_text(step / repeats as f64))
            } else {
                format!("play(notes[s % {}], {{dur: {} vel: vels[s]}}) sleep({})", order.len(), num_text(step * gate), num_text(step))
            };
            let code = format!(
                "fn(i) {{\n        let notes = {}\n        let vels = [{}]\n        for s in 0..{steps} {{ {play} }}\n    }}",
                note_list(&order),
                vels.join(" ")
            );
            let mut edits = vec![set("code", code), set("beats", num_text(beats)), remove("hits"), remove("notes")];
            let swing = args.num("swing");
            edits.push(if swing > 0.0 { set("swing", num_text(swing)) } else { remove("swing") });
            Ok(edits)
        }
        "chords" => {
            let root = args.note("root")?;
            let scale = theory::scale_kind(&args.text("scale")).ok_or("no such scale")?;
            let size = match args.text("size").as_str() {
                "seventh" => 4,
                "ninth" => 5,
                _ => 3,
            };
            let degrees: Vec<i32> = args.text("degrees").split_whitespace().filter_map(|d| d.parse::<i32>().ok()).filter(|d| (1..=7).contains(d)).collect();
            if degrees.is_empty() {
                return Err("`degrees` are scale degrees 1-7, like \"1 5 6 4\"".into());
            }
            let degree = |d: i32| -> f32 {
                let i = (d - 1).rem_euclid(scale.len() as i32);
                let octave = (d - 1).div_euclid(scale.len() as i32);
                root + (scale[i as usize] + 12 * octave) as f32
            };
            let inversion = args.num("inversion") as i32;
            let voicing = args.text("voicing");
            let chords: Vec<Vec<f32>> = degrees.iter().map(|d| {
                // Stacked thirds of the scale from the degree.
                let mut notes: Vec<f32> = (0..size).map(|k| degree(d - 1 + 1 + 2 * k as i32)).collect();
                for _ in 0..inversion.min(size as i32 - 1) {
                    notes.sort_by(f32::total_cmp);
                    notes[0] += 12.0;
                }
                notes.sort_by(f32::total_cmp);
                match voicing.as_str() {
                    // The second note an octave up.
                    "open" if notes.len() > 2 => notes[1] += 12.0,
                    // The second from the top an octave down.
                    "drop2" if notes.len() > 2 => {
                        let k = notes.len() - 2;
                        notes[k] -= 12.0;
                    }
                    // The root an octave down, the top an octave up.
                    "spread" if notes.len() > 1 => {
                        notes[0] -= 12.0;
                        let last = notes.len() - 1;
                        notes[last] += 12.0;
                    }
                    _ => {}
                }
                notes.sort_by(f32::total_cmp);
                notes
            }).collect();
            let bars = args.num("bars").max(1.0);
            let per = 4.0 * bars;
            let (hits, dur): (Vec<f64>, f64) = match args.text("rhythm").as_str() {
                "whole" => (vec![0.0], per * 0.95),
                "stabs" => ((0..(per as usize)).filter(|b| b % 2 == 1).map(|b| b as f64 + 0.5).collect(), 0.3),
                "pulse" => ((0..(per as usize * 2)).map(|h| h as f64 * 0.5).collect(), 0.4),
                _ => ((0..(per as usize / 2)).map(|h| h as f64 * 2.0).collect(), 1.9),
            };
            let mut rng = Rng::new(1.0);
            let curve_name = args.text("curve");
            let n = hits.len();
            let vels: Vec<String> = (0..n).map(|i| num_text((curve(&curve_name, i, n, 1.0, &mut rng) * 100.0).round() / 100.0)).collect();
            let beats = per * chords.len() as f64;
            // The rests before each hit, and after the last to the chord's end.
            let gaps: Vec<String> = hits.iter().enumerate().map(|(k, h)| num_text(h - if k == 0 { 0.0 } else { hits[k - 1] })).collect();
            let tail = per - hits.last().copied().unwrap_or(0.0);
            let code = format!(
                "fn(i) {{\n        let prog = [{}]\n        let gaps = [{}]\n        let vels = [{}]\n        for c in prog {{\n            for k in 0..{n} {{ sleep(gaps[k]) play(c, {{dur: {} vel: vels[k]}}) }}\n            sleep({})\n        }}\n    }}",
                chords.iter().map(|c| note_list(c)).collect::<Vec<_>>().join(" "),
                gaps.join(" "),
                vels.join(" "),
                num_text(dur),
                num_text(tail),
            );
            Ok(vec![set("code", code), set("beats", num_text(beats)), remove("hits"), remove("notes")])
        }
        "euclid" => {
            let steps = args.num("steps").max(1.0) as i32;
            let hits = theory::euclid(args.num("hits") as i32, steps, args.num("rotate") as i32);
            let rate = args.num("rate").max(1.0);
            let repeats = args.num("repeats").max(1.0) as usize;
            let mut rng = Rng::new(1.0);
            let curve_name = args.text("curve");
            let digits: Vec<char> = hits.iter().enumerate().map(|(i, on)| {
                if !*on {
                    return '.';
                }
                let v = curve(&curve_name, i, hits.len(), 1.0, &mut rng);
                if v >= 0.95 { 'X' } else { char::from(b'0' + ((v * 10.0).round().clamp(1.0, 9.0)) as u8) }
            }).collect();
            let step = 1.0 / rate;
            if repeats > 1 {
                // Ratchets: each hit repeated within its step, in code.
                // The drum the loop plays now: its `note`, else the first
                // hit its code makes.
                let drum = field("note").or_else(|| {
                    let code = field("code")?;
                    let at = code.find("hit(@")? + 4;
                    let end = at + code[at..].find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '@'))?;
                    Some(code[at..end].to_string())
                }).unwrap_or_else(|| "@kick".into());
                let pattern: String = digits.iter().collect();
                let code = format!(
                    "fn(i) {{\n        let pat = steps(\"{pattern}\")\n        for s in 0..{} {{\n            let v = pat[s]\n            if v {{ for r in 0..{repeats} {{ hit({drum}, {{amp: v}}) sleep({}) }} }} else {{ sleep({}) }}\n        }}\n    }}",
                    hits.len(),
                    num_text(step / repeats as f64),
                    num_text(step)
                );
                Ok(vec![set("code", code), set("beats", num_text(hits.len() as f64 * step)), remove("hits"), remove("notes")])
            } else {
                let pattern: String = digits.iter().collect();
                Ok(vec![set("hits", format!("{pattern:?}")), set("step", num_text(step)), remove("code"), remove("notes")])
            }
        }
        "transpose" | "scale" => {
            let shift = args.num("semitones") + 12.0 * args.num("octaves");
            let snap = if name == "scale" {
                let root = args.note("root")?;
                let kind = theory::scale_kind(&args.text("scale")).ok_or("no such scale")?.to_vec();
                Some((root, kind))
            } else {
                None
            };
            let move_note = |n: f32| -> f32 {
                match &snap {
                    Some((root, kind)) => {
                        // The nearest scale note (ties go down).
                        let mut best = n;
                        let mut best_d = f32::MAX;
                        for o in -1..=11 {
                            for s in kind {
                                let c = root.rem_euclid(12.0) + (o * 12 + s) as f32;
                                let d = (c - n).abs();
                                if d < best_d - 1e-3 {
                                    best = c;
                                    best_d = d;
                                }
                            }
                        }
                        best
                    }
                    None => n + shift as f32,
                }
            };
            let mut edits = Vec::new();
            let mut written = false;
            for key in ["code", "notes", "note"] {
                if let Some(text) = field(key) {
                    written |= map_notes(&text, &|n| n) != text || text.contains('@');
                    let next = map_notes(&text, &move_note);
                    if next != text {
                        edits.push(set(key, next));
                    }
                }
            }
            if edits.is_empty() {
                return Err(if written { format!("`{node}`'s written notes are already there: nothing moves") } else { format!("`{node}` writes no notes to move (its notes are computed or it plays a drum)") });
            }
            Ok(edits)
        }
        "density" => {
            let hits = field("hits").ok_or_else(|| format!("`{node}` has no hit pattern (`hits:`)"))?;
            let pattern = hits.trim().trim_matches('"');
            let amount = args.num("amount");
            let mut rng = Rng::new(args.num("seed"));
            let chars: Vec<char> = pattern.chars().collect();
            let steps: Vec<usize> = chars.iter().enumerate().filter(|(_, c)| !c.is_whitespace() && **c != '|').map(|(i, _)| i).collect();
            let mut out = chars.clone();
            for (k, &i) in steps.iter().enumerate() {
                let hit = !matches!(chars[i], '.' | '-' | '_');
                let downbeat = k % 4 == 0;
                let r = rng.next();
                if hit && amount < 0.5 && !downbeat && r < (0.5 - amount) * 2.0 {
                    out[i] = '.';
                } else if !hit && amount > 0.5 && r < (amount - 0.5) * 2.0 {
                    out[i] = 'o';
                }
            }
            let next: String = out.into_iter().collect();
            Ok(vec![set("hits", format!("{next:?}"))])
        }
        "velocity" => {
            let hits = field("hits").ok_or_else(|| format!("`{node}` has no hit pattern (`hits:`)"))?;
            let pattern = hits.trim().trim_matches('"');
            let mut rng = Rng::new(args.num("seed"));
            let curve_name = args.text("curve");
            let amount = args.num("amount");
            let n = pattern.chars().filter(|c| !c.is_whitespace() && *c != '|').count();
            let mut k = 0;
            let next: String = pattern.chars().map(|c| {
                if c.is_whitespace() || c == '|' {
                    return c;
                }
                let i = k;
                k += 1;
                if matches!(c, '.' | '-' | '_') {
                    return c;
                }
                let v = curve(&curve_name, i, n, amount, &mut rng);
                if v >= 0.95 { 'X' } else { char::from(b'0' + ((v * 10.0).round().clamp(1.0, 9.0)) as u8) }
            }).collect();
            Ok(vec![set("hits", format!("{next:?}"))])
        }
        "humanize" => Ok(vec![set("humanize", format!("{{time: {} vel: {}}}", num_text(args.num("time")), num_text(args.num("vel"))))]),
        "swing" => {
            let amount = args.num("amount");
            Ok(vec![if amount > 0.0 { set("swing", num_text(amount)) } else { remove("swing") }])
        }
        _ => Err(format!("no generator `{name}`")),
    }
}

/// `text` with every note id (`@a2`, `@fs3`, `@eb4`) moved by `f`: the
/// numbers, names and code around them as written.
pub fn map_notes(text: &str, f: &dyn Fn(f32) -> f32) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        if b[i] == b'@' && i + 1 < b.len() && (b'a'..=b'g').contains(&b[i + 1].to_ascii_lowercase()) {
            let mut j = i + 2;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'#') {
                j += 1;
            }
            let word = &text[i + 1..j];
            // A note id has an octave digit (`@a2`), so names like `@acid` stay.
            if word.bytes().any(|c| c.is_ascii_digit()) {
                if let Some(n) = theory::note_number(word) {
                    out.push_str(&text[last..i]);
                    out.push('@');
                    out.push_str(&theory::note_name(f(n).clamp(0.0, 127.0)));
                    last = j;
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

/// `src` with generator edits applied to loop `node` (a field set, added
/// or removed), as one new source.
pub fn apply(src: &str, node: &str, edits: &[FieldEdit]) -> Result<String, String> {
    let mut next = src.to_string();
    for edit in edits {
        let splice = match &edit.value {
            Some(value) => source::set_field(&next, node, &edit.field, value)?,
            None => match source::remove_field(&next, node, &edit.field) {
                Ok(splice) => splice,
                Err(_) => continue,
            },
        };
        next = source::apply(&next, &splice);
    }
    ScoreDoc::load(&next).map_err(|d| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "))?;
    Ok(next)
}
