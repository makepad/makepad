//! Melody variants: a loop's part (its phrase: the cycles it walks through
//! from the first it plays until they repeat, 32 bars at most, frozen as
//! notes) rewritten into variants a person auditions, keeps, applies or
//! merges bar by bar. The procedural ones are here and seeded (the same
//! seed, the same variant): variations, more syncopated, simpler, darker,
//! an answer phrase, re-harmonised, busier, humanised. An AI's variants
//! come back in the same form ([`parse_notes`]). Applying is one edit: a
//! loop with code plays the phrase from its cycle on (and its own code
//! before it, and after it when the phrase does not repeat); a pattern or
//! a list becomes a `notes:` list.

use crate::model::ScoreDoc;
use crate::source;
use crate::theory;
use makepad_score_player::presets::DRUM_SLOTS;

/// One note of a part: from `beat` (in the cycle) for `dur` beats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub beat: f64,
    pub note: f32,
    pub dur: f32,
    pub vel: f32,
    /// A drum hit (`note` is its kit slot).
    pub hit: bool,
}

/// A loop's part: its notes over `beats` (its phrase: `beats / cycle`
/// cycles of the loop from cycle number `first`; `repeats`: the phrase
/// comes round again for good, else the loop's own code follows it).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub beats: f64,
    pub beats_per_bar: f64,
    pub notes: Vec<Note>,
    pub cycle: f64,
    pub first: u32,
    pub repeats: bool,
}

impl Part {
    pub fn bars(&self) -> usize {
        (self.beats / self.beats_per_bar.max(1.0)).ceil().max(1.0) as usize
    }

    /// The notes as a score writes them: `[[beat note dur vel] …]`.
    pub fn text(&self) -> String {
        let items: Vec<String> = self.notes.iter().map(|n| {
            let note = if n.hit {
                format!("@{}", DRUM_SLOTS.get(n.note as usize).copied().unwrap_or("kick"))
            } else {
                format!("@{}", theory::note_name(n.note))
            };
            format!("[{} {note} {} {}]", source::fmt_num(n.beat, 3), source::fmt_num(n.dur as f64, 3), source::fmt_num(n.vel as f64, 2))
        }).collect();
        format!("[{}]", items.join(" "))
    }
}

/// What a loop plays in its first cycle, frozen (generated loops included).
pub fn part_of(src: &str, node: &str) -> Result<Part, String> {
    let doc = ScoreDoc::load(src).map_err(|d| d.first().map_or("the score does not load".to_string(), |d| d.message.clone()))?;
    if doc.loop_def(node).is_none() {
        return Err(format!("`{node}` is not a loop"));
    }
    // Its phrase: the cycles it walks through before they repeat (32 bars
    // at most), so a melody written bar by bar is varied whole.
    let phrase = crate::render::phrase_notes(src, node, 32.0 * doc.beats_per_bar()).ok_or_else(|| format!("`{node}` plays nothing in the score"))?;
    let (len, notes, durs) = (phrase.beats, phrase.notes, phrase.durs);
    let notes = notes.iter().enumerate().map(|(i, (beat, note, vel, hit))| Note {
        beat: *beat,
        note: *note,
        dur: durs.get(i).copied().filter(|d| *d > 0.0).unwrap_or(0.25),
        vel: *vel,
        hit: *hit,
    }).collect();
    Ok(Part { beats: len.max(1.0), beats_per_bar: doc.beats_per_bar(), notes, cycle: phrase.cycle.max(0.25), first: phrase.first, repeats: phrase.repeats })
}

/// The `fn(i) { … }` of a loop's code: its parameter and its body.
fn code_of(src: &str, node: &str) -> Option<(String, String)> {
    let outline = source::outline(src);
    let ctor = outline.node(node)?;
    let text = ctor.field("code")?.value.text(src).trim().to_string();
    let open = text.find('(')?;
    let close = open + text[open..].find(')')?;
    let param = text[open + 1..close].trim().to_string();
    let body_open = close + text[close..].find('{')?;
    let body_close = text.rfind('}')?;
    (body_close > body_open).then(|| (if param.is_empty() { "i".to_string() } else { param }, text[body_open + 1..body_close].to_string()))
}

/// `src` with loop `node` playing `part` (as a `notes:` list; its code,
/// pattern and note go), checked to load.
pub fn apply(src: &str, node: &str, part: &Part) -> Result<String, String> {
    if let Some((param, body)) = code_of(src, node) {
        return apply_code(src, node, part, &param, &body);
    }
    let mut next = src.to_string();
    for key in ["code", "hits", "note", "step"] {
        if let Ok(splice) = source::remove_field(&next, node, key) {
            next = source::apply(&next, &splice);
        }
    }
    for (key, value) in [("notes", part.text()), ("beats", source::fmt_num(part.beats, 3))] {
        let splice = source::set_field(&next, node, key, &value)?;
        next = source::apply(&next, &splice);
    }
    ScoreDoc::load(&next).map_err(|d| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "))?;
    Ok(next)
}

/// A loop with code made to play `part`: its cycles from `first` on take
/// the phrase's notes, cycle by cycle; before it (and after it, when the
/// phrase does not repeat) the loop's own code plays as it did.
fn apply_code(src: &str, node: &str, part: &Part, param: &str, body: &str) -> Result<String, String> {
    let cycles = ((part.beats / part.cycle).round() as usize).max(1);
    let slices: Vec<String> = (0..cycles).map(|k| {
        let (t0, t1) = (k as f64 * part.cycle, (k + 1) as f64 * part.cycle);
        let notes = part.notes.iter().filter(|n| n.beat >= t0 - 1e-9 && n.beat < t1 - 1e-9).map(|n| Note { beat: n.beat - t0, ..*n }).collect();
        Part { notes, ..part.clone() }.text()
    }).collect();
    // Its own names (a body may use `phrase`, `k` or `n` for its own).
    let within = if part.repeats { String::new() } else { format!(" && vary_k < {cycles}") };
    let cycle = source::fmt_num(part.cycle, 3);
    let own = if body.trim().is_empty() { format!("sleep({cycle} - beat())") } else { body.trim().to_string() };
    let code = format!(
        "fn({param}) {{\n        let vary_k = {param} - {first}\n        if vary_k >= 0{within} {{\n            let vary_cycles = [{phrase}]\n            for vary_n in vary_cycles[vary_k % {cycles}] {{\n                if vary_n[0] > beat() {{ sleep(vary_n[0] - beat()) }}\n                play(vary_n[1], {{dur: vary_n[2] amp: vary_n[3]}})\n            }}\n            sleep({cycle} - beat())\n        }} else {{\n            {own}\n        }}\n    }}",
        first = part.first,
        phrase = slices.join("\n                "),
    );
    let mut next = src.to_string();
    for key in ["hits", "note", "step", "notes"] {
        if let Ok(splice) = source::remove_field(&next, node, key) {
            next = source::apply(&next, &splice);
        }
    }
    for (key, value) in [("code", code), ("beats", cycle.clone())] {
        let splice = source::set_field(&next, node, key, &value)?;
        next = source::apply(&next, &splice);
    }
    ScoreDoc::load(&next).map_err(|d| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; "))?;
    if let Some(error) = crate::render::loop_error(&next, node) {
        return Err(format!("`{node}` would not play: {error}"));
    }
    Ok(next)
}

/// Bar by bar from the parts chosen (`choice[bar]`: an index into
/// `parts`); a bar past the choices is the first part's.
pub fn merge(parts: &[&Part], choice: &[usize]) -> Part {
    let base = parts[0];
    let bar = base.beats_per_bar;
    let mut notes = Vec::new();
    for b in 0..base.bars() {
        let from = parts.get(choice.get(b).copied().unwrap_or(0)).copied().unwrap_or(base);
        let (t0, t1) = (b as f64 * bar, (b + 1) as f64 * bar);
        notes.extend(from.notes.iter().filter(|n| n.beat >= t0 - 1e-9 && n.beat < t1 - 1e-9).copied());
    }
    Part { notes, ..base.clone() }
}

/// The procedural variants, by name.
pub const KINDS: [(&str, &str); 8] = [
    ("variations", "4 variations"),
    ("syncopated", "More syncopated"),
    ("simpler", "Simpler"),
    ("darker", "Darker"),
    ("answer", "Answer phrase"),
    ("reharmonise", "Re-harmonise"),
    ("busier", "Busier"),
    ("humanise", "Humanise"),
];

/// A small deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x2545_F491_4F6C_DD1D)
    }

    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The part's key: the root and scale (of major and natural minor) that
/// hold most of its notes, its lowest best-fitting note the root.
pub fn key_of(part: &Part) -> (f32, &'static [i32]) {
    let pcs: Vec<i32> = part.notes.iter().filter(|n| !n.hit).map(|n| (n.note.round() as i32).rem_euclid(12)).collect();
    let mut best = (0i32, theory::scale_kind("minor").unwrap(), -1i32);
    for root in 0..12 {
        for kind in ["minor", "major"] {
            let scale = theory::scale_kind(kind).unwrap();
            let fit = pcs.iter().filter(|pc| scale.contains(&((*pc - root).rem_euclid(12)))).count() as i32 * 4
                + pcs.iter().filter(|pc| **pc == root).count() as i32;
            if fit > best.2 {
                best = (root, scale, fit);
            }
        }
    }
    let low = part.notes.iter().filter(|n| !n.hit).map(|n| n.note).fold(f32::MAX, f32::min);
    let low = if low == f32::MAX { 48.0 } else { low };
    let root = low - ((low.round() as i32 - best.0).rem_euclid(12)) as f32;
    (root, best.1)
}

/// `note` moved by `steps` degrees of the scale on `root` (snapped first).
fn degree(note: f32, steps: i32, root: f32, scale: &[i32]) -> f32 {
    let rel = (note - root).round() as i32;
    let octave = rel.div_euclid(12);
    let pc = rel.rem_euclid(12);
    let mut idx = scale.iter().position(|s| *s >= pc).unwrap_or(scale.len() - 1) as i32;
    if scale[idx as usize] != pc && idx > 0 && pc - scale[idx as usize - 1] < scale[idx as usize] - pc {
        idx -= 1;
    }
    let total = octave * scale.len() as i32 + idx + steps;
    let (o, i) = (total.div_euclid(scale.len() as i32), total.rem_euclid(scale.len() as i32));
    root + (o * 12 + scale[i as usize]) as f32
}

/// The grid the part is written on (the smallest step between onsets).
fn grid(part: &Part) -> f64 {
    let mut beats: Vec<f64> = part.notes.iter().map(|n| n.beat).collect();
    beats.sort_by(f64::total_cmp);
    beats.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 1e-3).fold(0.5f64, f64::min).max(0.125)
}

fn within(n: &Note, region: Option<(f64, f64)>) -> bool {
    region.is_none_or(|(a, b)| n.beat >= a - 1e-9 && n.beat < b - 1e-9)
}

/// Variants of `part` of a kind (one, or `n` for "variations"), seeded;
/// only the notes in `region` (beats) change. Each with its label.
pub fn vary(part: &Part, kind: &str, n: usize, seed: u64, region: Option<(f64, f64)>) -> Result<Vec<(String, Part)>, String> {
    let (root, scale) = key_of(part);
    let g = grid(part);
    let one = |label: &str, f: &dyn Fn(&mut Rng, &Part) -> Vec<Note>, s: u64| -> (String, Part) {
        let mut rng = Rng::new(seed.wrapping_add(s));
        let mut notes = f(&mut rng, part);
        notes.sort_by(|a, b| a.beat.total_cmp(&b.beat));
        notes.retain(|n| n.beat >= 0.0 && n.beat < part.beats - 1e-9);
        (label.to_string(), Part { notes, ..part.clone() })
    };
    // Each transform leaves the notes outside the region as they are.
    let keep_out = |p: &Part| -> Vec<Note> { p.notes.iter().filter(|n| !within(n, region)).copied().collect() };
    let inside = |p: &Part| -> Vec<Note> { p.notes.iter().filter(|n| within(n, region)).copied().collect() };
    let variation = |rng: &mut Rng, p: &Part| -> Vec<Note> {
        let mut out = keep_out(p);
        for mut n in inside(p) {
            if !n.hit {
                let r = rng.next();
                if r < 0.3 {
                    n.note = degree(n.note, if rng.next() < 0.5 { 1 } else { -1 }, root, scale);
                } else if r < 0.4 {
                    n.note = degree(n.note, if rng.next() < 0.5 { 2 } else { -2 }, root, scale);
                } else if r < 0.45 {
                    n.note += if n.note < root + 12.0 { 12.0 } else { -12.0 };
                }
            }
            if rng.next() < 0.15 {
                // A rest instead (never the first note of a bar).
                if (n.beat / p.beats_per_bar).fract() > 1e-6 {
                    continue;
                }
            }
            n.vel = (n.vel * (0.85 + 0.3 * rng.next() as f32)).clamp(0.1, 1.0);
            out.push(n);
        }
        out
    };
    Ok(match kind {
        "variations" => (0..n.max(1)).map(|k| {
            let (_, p) = one("", &variation, k as u64 * 7919);
            (format!("Variation {}", k + 1), p)
        }).collect(),
        "syncopated" => vec![one("More syncopated", &|rng, p| {
            let mut out = keep_out(p);
            let onsets: Vec<f64> = p.notes.iter().map(|n| n.beat).collect();
            for mut n in inside(p) {
                let on_beat = (n.beat / (2.0 * g)).fract() < 1e-6;
                // Pushed only into a step that is free (a dense line stays).
                let free = !onsets.iter().any(|b| (b - (n.beat - g)).abs() < 1e-6);
                if on_beat && free && (n.beat / p.beats_per_bar).fract() > 1e-6 && rng.next() < 0.6 {
                    // Pushed: a grid step early, held over the beat.
                    n.beat -= g;
                    n.dur += g as f32;
                }
                out.push(n);
            }
            out
        }, 1)],
        "simpler" => vec![one("Simpler", &|rng, p| {
            let mut out = keep_out(p);
            let mut kept: Vec<Note> = Vec::new();
            for n in inside(p) {
                let strong = (n.beat).fract() < 1e-6;
                if strong || rng.next() < 0.35 {
                    kept.push(n);
                }
            }
            // Each note held to the next.
            for i in 0..kept.len() {
                if !kept[i].hit {
                    let next = kept.get(i + 1).map_or(p.beats, |m| m.beat);
                    kept[i].dur = ((next - kept[i].beat) * 0.9).max(kept[i].dur as f64) as f32;
                }
            }
            out.extend(kept);
            out
        }, 2)],
        "darker" => vec![one("Darker", &|_rng, p| {
            let minor = theory::scale_kind("phrygian").unwrap();
            let mut out = keep_out(p);
            for mut n in inside(p) {
                if !n.hit {
                    // Into the darker mode on the root, the top down an octave.
                    n.note = degree(n.note, 0, root, minor);
                    if n.note > root + 12.0 {
                        n.note -= 12.0;
                    }
                }
                n.vel *= 0.85;
                out.push(n);
            }
            out
        }, 3)],
        "answer" => vec![one("Answer phrase", &|_rng, p| {
            // The phrase's contour turned over around its first note, the
            // last note brought home to the root.
            let mut out = keep_out(p);
            let notes = inside(p);
            let first = notes.iter().find(|n| !n.hit).map_or(root, |n| n.note);
            let last = notes.iter().rposition(|n| !n.hit);
            for (i, mut n) in notes.into_iter().enumerate() {
                if !n.hit {
                    let steps = ((first - n.note) / 2.0).round() as i32;
                    n.note = degree(first, steps, root, scale);
                    if Some(i) == last {
                        n.note = root + 12.0 * ((n.note - root) / 12.0).round();
                    }
                }
                out.push(n);
            }
            out
        }, 4)],
        "reharmonise" => vec![one("Re-harmonised", &|_rng, p| {
            // The relative key's mode (minor ↔ major on the same notes moved).
            let other: &[i32] = if scale == theory::scale_kind("minor").unwrap() { theory::scale_kind("dorian").unwrap() } else { theory::scale_kind("mixolydian").unwrap() };
            let mut out = keep_out(p);
            let moved = inside(p).iter().any(|n| !n.hit && degree(n.note, 0, root, other) != n.note);
            for mut n in inside(p) {
                if !n.hit {
                    n.note = if moved {
                        degree(n.note, 0, root, other)
                    } else {
                        // The mode changes nothing here: each bar over another
                        // chord of the key instead (i, iv, vi, v), its shape kept.
                        let bar = (n.beat / p.beats_per_bar).floor() as usize;
                        degree(n.note, [0, 3, 5, 4][bar % 4], root, scale)
                    };
                }
                out.push(n);
            }
            out
        }, 5)],
        "busier" => vec![one("Busier", &|rng, p| {
            let mut out = keep_out(p);
            let notes = inside(p);
            for (i, n) in notes.iter().enumerate() {
                out.push(*n);
                let next = notes.get(i + 1);
                let gap = next.map_or(p.beats, |m| m.beat) - n.beat;
                if gap >= 2.0 * g && rng.next() < 0.7 {
                    let mut pass = *n;
                    pass.beat = n.beat + gap / 2.0;
                    pass.dur = (gap / 2.0 * 0.8) as f32;
                    pass.vel = n.vel * 0.7;
                    if !n.hit {
                        let target = next.filter(|m| !m.hit).map_or(n.note, |m| m.note);
                        pass.note = degree(n.note, if target >= n.note { 1 } else { -1 }, root, scale);
                    }
                    out.push(pass);
                }
            }
            out
        }, 6)],
        "humanise" => vec![one("Humanised", &|rng, p| {
            let mut out = keep_out(p);
            for mut n in inside(p) {
                n.beat = (n.beat + (rng.next() - 0.5) * 0.04).max(0.0);
                n.vel = (n.vel * (0.8 + 0.35 * rng.next() as f32)).clamp(0.1, 1.0);
                out.push(n);
            }
            out
        }, 7)],
        other => return Err(format!("no variant `{other}` (there are {})", KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(", "))),
    })
}

/// A part written as `[[beat note dur vel] …]` (an AI's answer), read;
/// the cycle's length and bar from `like`.
pub fn parse_notes(text: &str, like: &Part) -> Result<Part, String> {
    // The last list (an answer that corrects itself ends with it).
    let open = text.rfind("[[").ok_or("no `[[beat note dur vel] …]` list")?;
    let close = open + text[open..].find("]]").ok_or("the list is not closed")? + 2;
    let body = &text[open + 1..close - 1];
    let mut notes = Vec::new();
    for item in body.split(']') {
        let item = item.trim().trim_start_matches(['[', ',', ' ']);
        if item.is_empty() {
            continue;
        }
        let words: Vec<&str> = item.split([' ', ',']).filter(|w| !w.is_empty()).collect();
        if words.len() < 2 {
            continue;
        }
        let beat = words[0].parse::<f64>().map_err(|_| format!("`{item}`: a beat is a number"))?;
        let name = words[1].trim_start_matches('@');
        let (note, hit) = match DRUM_SLOTS.iter().position(|s| *s == name) {
            Some(slot) => (slot as f32, true),
            None => (theory::note_number(name).or_else(|| name.parse::<f32>().ok()).ok_or_else(|| format!("`{name}` is not a note"))?, false),
        };
        let dur = words.get(2).and_then(|w| w.parse::<f32>().ok()).unwrap_or(0.25);
        let vel = words.get(3).and_then(|w| w.parse::<f32>().ok()).unwrap_or(0.8);
        if beat >= 0.0 && beat < like.beats {
            notes.push(Note { beat, note, dur, vel, hit });
        }
    }
    if notes.is_empty() {
        return Err("the answer has no notes".into());
    }
    notes.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    Ok(Part { notes, ..like.clone() })
}
