//! The Arrange view's model and its edits. A lane per loop (its role and
//! instrument in plain words, its pitch range or drum rows), the
//! arrangement's sections in beats (repeats marked), what each section
//! plays per loop (the arrangement played through without sound), and a
//! chord per bar fitted to the notes. Every edit is source in, source out,
//! touching only the fields it changes (`loops:`, `bars:`, `arrange:`,
//! `mute:`/`solo:`, a copied `Section`).

use makepad_score_live::compose::Composer;
use makepad_score_live::model::{self, InstKind, ScoreDoc};
use makepad_score_live::source::{self, Ctor, Splice};
use makepad_score_live::{instruments, theory};
use makepad_score_player::presets::DRUM_SLOTS;
use std::collections::HashMap;

/// What a loop does in the song, for its lane's name and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Drums,
    Bass,
    Keys,
    Pad,
    Arp,
    Lead,
    Vocals,
    Other,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Drums => "Drums",
            Role::Bass => "Bass",
            Role::Keys => "Keys",
            Role::Pad => "Pad",
            Role::Arp => "Arp",
            Role::Lead => "Lead",
            Role::Vocals => "Vocals",
            Role::Other => "Part",
        }
    }
}

/// A note or hit of a clip: beats from its section's start; `pitch` is
/// the MIDI note, or the kit slot for a hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipNote {
    pub beat: f64,
    pub pitch: f32,
    pub dur: f32,
    pub vel: f32,
    pub hit: bool,
}

/// What the arrangement plays: per (section, its occurrence in the
/// arrangement, loop) the notes, from the section's start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipNotes {
    pub map: HashMap<(String, usize, String), Vec<ClipNote>>,
}

impl ClipNotes {
    pub fn get(&self, section: &str, occurrence: usize, lane: &str) -> &[ClipNote] {
        self.map.get(&(section.to_string(), occurrence, lane.to_string())).map_or(&[], Vec::as_slice)
    }
}

/// One entry of `arrange:`.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    /// Its place in `arrange:`, and how many times it played before.
    pub index: usize,
    pub occurrence: usize,
    /// Beats from the song's start, and its length in beats and bars.
    pub start: f64,
    pub len: f64,
    pub bars: f64,
    pub loops: Vec<String>,
    /// Its place in a run of the same section played back to back, and
    /// the run's length (1: no repeat).
    pub run: (usize, usize),
    /// Played before, not just before it: a reprise.
    pub again: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub name: String,
    pub key: u64,
    pub role: Role,
    /// "Bass · reese", "Kick · 808".
    pub title: String,
    pub drums: bool,
    /// A drum lane's kit slots, top row first.
    pub rows: Vec<f32>,
    /// A melodic lane's pitch range (inclusive), at least an octave.
    pub low: f32,
    pub high: f32,
    pub mute: bool,
    pub solo: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArrangeModel {
    pub source: String,
    pub beats_per_bar: f64,
    pub sections: Vec<Section>,
    pub lanes: Vec<Lane>,
    pub notes: ClipNotes,
    /// (first bar, bars, name): a chord held over bars.
    pub chords: Vec<(usize, usize, String)>,
    /// The arrangement's length in beats.
    pub total: f64,
}

impl ArrangeModel {
    pub fn section_at(&self, beat: f64) -> Option<usize> {
        self.sections.iter().position(|s| beat >= s.start && beat < s.start + s.len)
    }
}

/// The longest arrangement played through for its notes.
const MAX_BEATS: f64 = 2048.0;

/// `arrange:` as sections: (name, occurrence, start beat, beats).
fn spans(doc: &ScoreDoc) -> Vec<(String, usize, f64, f64)> {
    let bar = doc.beats_per_bar();
    let mut start = 0.0;
    let mut out: Vec<(String, usize, f64, f64)> = Vec::new();
    for name in &doc.arrange {
        let Some(section) = doc.sections.iter().find(|s| &s.name == name) else { continue };
        let occurrence = out.iter().filter(|(n, ..)| n == name).count();
        let len = section.bars * bar;
        out.push((name.clone(), occurrence, start, len));
        start += len;
    }
    out
}

/// What each section of the arrangement plays per loop: the composer run
/// bar by bar over the arrangement once, its notes sorted into sections.
/// Off the UI thread: it runs the loops' code.
pub fn clip_notes(source: &str) -> ClipNotes {
    let Ok(loaded) = model::load(source) else { return ClipNotes::default() };
    let spans = spans(&loaded.doc);
    let bar = loaded.doc.beats_per_bar().max(1.0);
    let end = spans.last().map_or(0.0, |(_, _, start, len)| start + len).min(MAX_BEATS);
    let mut out = ClipNotes::default();
    if end <= 0.0 {
        return out;
    }
    let mut composer = Composer::new(loaded);
    let mut events = Vec::new();
    let mut until = 0.0;
    while until < end {
        until = (until + bar).min(end);
        composer.pump(until, &mut events);
        events.clear();
        for played in composer.history.drain(..) {
            let Some((name, occurrence, start, _)) = spans.iter().find(|(_, _, start, len)| played.beat >= *start - 1e-9 && played.beat < start + len - 1e-9) else { continue };
            out.map.entry((name.clone(), *occurrence, played.loop_name.clone())).or_default().push(ClipNote { beat: played.beat - start, pitch: played.note, dur: played.dur, vel: played.vel, hit: played.hit });
        }
    }
    for notes in out.map.values_mut() {
        notes.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    }
    out
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// `saw_lead` reads "saw lead", `analog_80s` "808".
fn words(name: &str) -> String {
    let name = name.trim().trim_start_matches('@').trim_matches('"');
    let name = name.strip_prefix("tr").filter(|rest| rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty()).unwrap_or(name);
    name.replace('_', " ")
}

/// A drum slot in plain words.
fn slot_word(slot: f32) -> String {
    let name = DRUM_SLOTS.get(slot.max(0.0) as usize).copied().unwrap_or("perc");
    match name {
        "kick" => "Kick",
        "snare" => "Snare",
        "clap" => "Clap",
        "hat" => "Hats",
        "ohat" => "Open hat",
        "rim" => "Rim",
        "tom_lo" | "tom_mid" | "tom_hi" => "Toms",
        "crash" => "Crash",
        "ride" => "Ride",
        "shaker" => "Shaker",
        "cowbell" => "Cowbell",
        "conga" => "Conga",
        "snap" => "Snap",
        "zap" => "Zap",
        _ => "Perc",
    }
    .to_string()
}

/// The note or slot a hover names: `b3`, `kick`.
pub fn pitch_name(pitch: f32, hit: bool) -> String {
    if hit {
        DRUM_SLOTS.get(pitch.max(0.0) as usize).map_or_else(|| format!("slot {pitch}"), |s| s.replace('_', " "))
    } else {
        let name = theory::note_name(pitch);
        let mut chars = name.chars();
        let letter = chars.next().map(|c| c.to_ascii_uppercase()).unwrap_or('?');
        let rest: String = chars.collect();
        format!("{letter}{}", rest.replacen('s', "#", 1))
    }
}

/// Top row first: cymbals and hats over the snare, the kick at the bottom.
fn drum_height(slot: f32) -> i32 {
    match DRUM_SLOTS.get(slot.max(0.0) as usize).copied().unwrap_or("") {
        "kick" => 0,
        "tom_lo" => 1,
        "snare" => 2,
        "rim" => 3,
        "clap" => 4,
        "snap" => 5,
        "tom_mid" => 6,
        "tom_hi" => 7,
        "conga" => 8,
        "cowbell" => 9,
        "zap" => 10,
        "shaker" => 11,
        "hat" => 12,
        "ohat" => 13,
        "ride" => 14,
        "crash" => 15,
        _ => 16,
    }
}

/// The instrument a loop plays, in plain words: its preset, kit or
/// program.
fn instrument_words(doc: &ScoreDoc, outline: &source::Outline, src: &str, inst: &str) -> String {
    let Some(def) = doc.inst(inst) else { return words(inst) };
    let field = |key: &str| outline.node(inst).and_then(|n| n.field(key)).map(|f| words(f.value.text(src)));
    match &def.kind {
        InstKind::Drums { kit, .. } => words(kit),
        InstKind::Synth { .. } => field("preset").or_else(|| field("osc").map(|o| format!("{o} synth"))).unwrap_or_else(|| "synth".into()),
        InstKind::Native { native, preset, program, .. } => match native.as_str() {
            "piano" => preset.as_deref().map_or_else(|| "grand piano".into(), |p| format!("{} piano", words(p))),
            "drums" => "acoustic kit".into(),
            _ => instruments::GM_PROGRAMS.get(*program as usize).map_or_else(|| "soundfont".into(), |p| p.to_lowercase()),
        },
        InstKind::Sampler { .. } => "sampler".into(),
        InstKind::Ironfish { .. } => field("program").map_or_else(|| "ironfish".into(), |p| format!("ironfish {p}")),
        InstKind::Morph { .. } => field("preset").map_or_else(|| "morph".into(), |p| format!("morph {p}")),
        InstKind::Shader { shader, .. } => shader.lib.as_deref().map_or_else(|| "shader synth".into(), words),
        InstKind::Voice { profile, .. } => format!("{} voice", words(profile)),
        InstKind::Input { .. } => "audio in".into(),
        InstKind::Midi { channel, .. } => format!("MIDI ch {channel}"),
    }
}

/// A loop's role: by its instrument (a kit plays drums, a voice sings),
/// else by the words in its name and instrument, else by what it plays
/// (low notes are bass, held chords a pad, stacked short ones keys, fast
/// high ones an arp, a single line a lead).
fn role_of(kind: Option<&InstKind>, drum_hits: bool, words: &str, notes: &[ClipNote]) -> Role {
    match kind {
        Some(InstKind::Drums { .. }) => return Role::Drums,
        Some(InstKind::Native { native, .. }) if native == "drums" => return Role::Drums,
        Some(InstKind::Voice { .. }) => return Role::Vocals,
        _ if drum_hits => return Role::Drums,
        _ => {}
    }
    let pitched: Vec<&ClipNote> = notes.iter().filter(|n| !n.hit).collect();
    let pitch = median(pitched.iter().map(|n| n.pitch).collect());
    let dur = median(pitched.iter().map(|n| n.dur).collect()).unwrap_or(0.0);
    let onsets = {
        let mut beats: Vec<i64> = pitched.iter().map(|n| (n.beat * 64.0).round() as i64).collect();
        beats.dedup();
        beats.len().max(1)
    };
    let stack = pitched.len() as f32 / onsets as f32;
    let has = |list: &[&str]| list.iter().any(|w| words.contains(w));
    if has(&["bass", "sub", "808", "reese"]) || pitch.is_some_and(|p| p < 50.0) {
        Role::Bass
    } else if has(&["pad", "string", "choir", "drone"]) {
        Role::Pad
    } else if has(&["arp", "pluck", "blip", "chip", "bell"]) {
        Role::Arp
    } else if has(&["lead", "hook", "melody"]) {
        Role::Lead
    } else if has(&["piano", "keys", "organ", "chord", "rhodes"]) {
        Role::Keys
    } else if pitch.is_none() {
        Role::Other
    } else if stack >= 2.5 {
        if dur >= 2.0 { Role::Pad } else { Role::Keys }
    } else if dur <= 0.3 && pitch.is_some_and(|p| p >= 70.0) {
        Role::Arp
    } else {
        Role::Lead
    }
}

/// The model of a loaded score, with the notes (from [`clip_notes`]; any
/// older ones do until the new ones come: they are keyed by section).
pub fn build(doc: &ScoreDoc, src: &str, notes: ClipNotes) -> ArrangeModel {
    let outline = source::outline(src);
    let bar = doc.beats_per_bar().max(1.0);
    let spans = spans(doc);
    let sections: Vec<Section> = spans
        .iter()
        .enumerate()
        .map(|(index, (name, occurrence, start, len))| {
            let def = doc.sections.iter().find(|s| &s.name == name);
            let run_start = (0..=index).rev().take_while(|i| spans[*i].0 == *name).last().unwrap_or(index);
            let run_end = (index..spans.len()).take_while(|i| spans[*i].0 == *name).last().unwrap_or(index);
            Section {
                name: name.clone(),
                index,
                occurrence: *occurrence,
                start: *start,
                len: *len,
                bars: len / bar,
                loops: def.map(|d| d.loops.clone()).unwrap_or_default(),
                run: (index - run_start, run_end - run_start + 1),
                again: spans[..run_start].iter().any(|(n, ..)| n == name),
            }
        })
        .collect();
    let mut lanes: Vec<Lane> = doc
        .loops
        .iter()
        .filter(|l| l.stinger.is_none())
        .map(|l| {
            let played: Vec<ClipNote> = notes.map.iter().filter(|((_, _, lane), _)| *lane == l.name).flat_map(|(_, v)| v.iter().copied()).collect();
            let kind = l.play.as_deref().and_then(|p| doc.inst(p)).map(|i| &i.kind);
            let drum_hits = l.hits.as_ref().is_some_and(|h| h.drum.is_some());
            let instrument = l.play.as_deref().map_or_else(|| "no instrument".to_string(), |p| instrument_words(doc, &outline, src, p));
            let role = role_of(kind, drum_hits, &format!("{} {}", l.name, instrument).to_lowercase(), &played);
            let drums = role == Role::Drums;
            let mut rows: Vec<f32> = played.iter().filter(|n| n.hit).map(|n| n.pitch).collect();
            if rows.is_empty() {
                if let Some((slot, _)) = l.hits.as_ref().and_then(|h| h.drum) {
                    rows.push(slot as f32);
                }
            }
            rows.sort_by(|a, b| drum_height(*b).cmp(&drum_height(*a)).then(a.total_cmp(b)));
            rows.dedup();
            let pitches: Vec<f32> = played.iter().filter(|n| !n.hit).map(|n| n.pitch).collect();
            let (mut low, mut high) = pitches.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
            if low > high {
                (low, high) = (60.0, 60.0);
            }
            // At least an octave, the notes in the middle.
            let span = high - low;
            if span < 11.0 {
                let pad = (11.0 - span) / 2.0;
                low = (low - pad.ceil()).max(0.0);
                high = low + 11.0;
            }
            let title = if drums {
                let what = if rows.len() == 1 { slot_word(rows[0]) } else { "Drums".to_string() };
                format!("{what} · {instrument}")
            } else {
                format!("{} · {instrument}", role.name())
            };
            Lane { name: l.name.clone(), key: makepad_score_live::key(&l.name), role, title, drums, rows, low, high, mute: l.mute, solo: l.solo }
        })
        .collect();
    lanes.sort_by_key(|l| l.role);
    let total = sections.last().map_or(0.0, |s| s.start + s.len);
    let chords = chords(&sections, &lanes, &notes, bar, total);
    ArrangeModel { source: src.to_string(), beats_per_bar: bar, sections, lanes, notes, chords, total }
}

const PITCH_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The chord kinds fitted, with their suffixes.
const CHORDS: [(&str, &str); 8] = [("major", ""), ("minor", "m"), ("7", "7"), ("maj7", "maj7"), ("m7", "m7"), ("dim", "dim"), ("sus4", "sus4"), ("sus2", "sus2")];

/// The chord that best fits the pitch classes' weights (`bass`: the
/// lowest class sounding): the chord's tones count, tones outside it
/// count against, four-note chords pay a little for their extra tone, and
/// a root in the bass helps. None: too little sounds, or nothing fits.
pub fn fit_chord(weights: &[f32; 12], bass: Option<usize>) -> Option<String> {
    let total: f32 = weights.iter().sum();
    if total <= 1e-3 || weights.iter().filter(|w| **w > total * 0.05).count() < 2 {
        return None;
    }
    let mut best: Option<(f32, usize, usize)> = None;
    for root in 0..12 {
        for (k, (kind, _)) in CHORDS.iter().enumerate() {
            let tones = theory::chord_kind(kind).unwrap_or(&[]);
            let mut inside = [false; 12];
            for t in tones {
                inside[(root + *t as usize) % 12] = true;
            }
            let hit: f32 = (0..12).filter(|pc| inside[*pc]).map(|pc| weights[pc]).sum();
            let miss = total - hit;
            // Every tone of the chord should sound a little.
            let absent = (0..12).filter(|pc| inside[*pc] && weights[*pc] < total * 0.04).count() as f32;
            let mut score = hit - 0.8 * miss - absent * 0.25 * total - (tones.len() as f32 - 3.0).max(0.0) * 0.06 * total;
            if bass == Some(root) {
                score += 0.2 * total;
            }
            if best.is_none_or(|(b, ..)| score > b + 1e-6) {
                best = Some((score, root, k));
            }
        }
    }
    let (score, root, k) = best?;
    (score > 0.15 * total).then(|| format!("{}{}", PITCH_NAMES[root], CHORDS[k].1))
}

/// A chord per bar from every pitched lane, bars holding the same chord
/// joined.
fn chords(sections: &[Section], lanes: &[Lane], notes: &ClipNotes, bar: f64, total: f64) -> Vec<(usize, usize, String)> {
    let bars = (total / bar).ceil().max(0.0) as usize;
    let mut weights = vec![[0f32; 12]; bars];
    let mut lowest: Vec<Option<f32>> = vec![None; bars];
    for section in sections {
        for lane in lanes.iter().filter(|l| !l.drums && !l.mute && section.loops.contains(&l.name)) {
            for n in notes.get(&section.name, section.occurrence, &lane.name).iter().filter(|n| !n.hit) {
                let start = section.start + n.beat;
                let end = start + (n.dur as f64).max(0.25);
                let first = (start / bar).floor().max(0.0) as usize;
                for b in first..bars {
                    let (b0, b1) = (b as f64 * bar, (b + 1) as f64 * bar);
                    if b0 >= end {
                        break;
                    }
                    let overlap = (end.min(b1) - start.max(b0)).max(0.0) as f32;
                    if overlap <= 0.0 {
                        continue;
                    }
                    weights[b][(n.pitch.round() as i32).rem_euclid(12) as usize] += overlap * n.vel.clamp(0.2, 1.5);
                    if lowest[b].is_none_or(|l| n.pitch < l) {
                        lowest[b] = Some(n.pitch);
                    }
                }
            }
        }
    }
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    for b in 0..bars {
        let Some(name) = fit_chord(&weights[b], lowest[b].map(|p| (p.round() as i32).rem_euclid(12) as usize)) else { continue };
        match out.last_mut() {
            Some((first, len, last)) if *last == name && *first + *len == b => *len += 1,
            _ => out.push((b, 1, name)),
        }
    }
    out
}

// The edits.

/// The names in a node's list field (`loops: [a b]`; `""` is the root).
pub fn list_of(src: &str, node: &str, key: &str) -> Vec<String> {
    let outline = source::outline(src);
    let ctor = if node.is_empty() { outline.root.as_ref() } else { outline.node(node) };
    ctor.and_then(|c| c.field(key))
        .map(|f| f.value.text(src).trim().trim_start_matches('[').trim_end_matches(']').split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

fn set_list(src: &str, node: &str, key: &str, items: &[String]) -> Result<String, String> {
    Ok(source::apply(src, &source::set_field(src, node, key, &format!("[{}]", items.join(" ")))?))
}

/// `loop` added to `section`'s loops (nothing if it plays there).
pub fn add_clip(src: &str, section: &str, lane: &str) -> Result<String, String> {
    let mut loops = list_of(src, section, "loops");
    if loops.iter().any(|l| l == lane) {
        return Ok(src.to_string());
    }
    loops.push(lane.to_string());
    set_list(src, section, "loops", &loops)
}

/// `loop` taken out of `section`'s loops.
pub fn remove_clip(src: &str, section: &str, lane: &str) -> Result<String, String> {
    let mut loops = list_of(src, section, "loops");
    loops.retain(|l| l != lane);
    set_list(src, section, "loops", &loops)
}

/// `loop` moved (or copied) from one section to another.
pub fn move_clip(src: &str, from: &str, to: &str, lane: &str, copy: bool) -> Result<String, String> {
    if from == to {
        return Ok(src.to_string());
    }
    let added = add_clip(src, to, lane)?;
    if copy { Ok(added) } else { remove_clip(&added, from, lane) }
}

/// A section's length in bars (at least one).
pub fn set_bars(src: &str, section: &str, bars: f64) -> Result<String, String> {
    Ok(source::apply(src, &source::set_field(src, section, "bars", &source::fmt_num(bars.max(1.0), 2))?))
}

/// `mute:` or `solo:` on a loop: written true, or taken out.
pub fn set_flag(src: &str, lane: &str, key: &str, on: bool) -> Result<String, String> {
    if on {
        Ok(source::apply(src, &source::set_field(src, lane, key, "true")?))
    } else if source::outline(src).node(lane).and_then(|n| n.field(key)).is_some() {
        Ok(source::apply(src, &source::remove_field(src, lane, key)?))
    } else {
        Ok(src.to_string())
    }
}

/// `base`, or `base_2`, `base_3`… : a name no node has.
fn free_name(src: &str, base: &str) -> String {
    let outline = source::outline(src);
    if outline.node(base).is_none() {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|name| outline.node(name).is_none()).unwrap_or_default()
}

/// A node's value as written: `Section{…}`.
fn node_text<'a>(src: &'a str, ctor: &Ctor) -> &'a str {
    let head = &src[ctor.span.start..ctor.body.start];
    let at = head.find('=').map_or(ctor.body.start, |eq| ctor.span.start + eq + 1);
    src[at..ctor.span.end].trim_start()
}

/// `let name = text` on the line after `after`'s statement, listed in the
/// root right after it.
fn insert_after(src: &str, after: &str, name: &str, text: &str) -> Result<String, String> {
    let outline = source::outline(src);
    let ctor = outline.node(after).ok_or_else(|| format!("no node `{after}`"))?;
    let root = outline.root.as_ref().ok_or("the score has no final `Score{…}`")?;
    // The root comes after every node: list there first, offsets before it hold.
    let member = match root.fields.iter().find(|f| f.bare && f.value.text(src) == after).or_else(|| root.fields.iter().rev().find(|f| f.bare)) {
        Some(field) => Splice { start: field.value.end, end: field.value.end, text: format!(" {name}") },
        None => {
            let before = src[..root.body.end - 1].trim_end().len();
            Splice { start: before, end: before, text: format!(" {name}") }
        }
    };
    let listed = source::apply(src, &member);
    Ok(source::apply(&listed, &Splice { start: ctor.span.end, end: ctor.span.end, text: format!("\nlet {name} = {text}") }))
}

/// A section split in two after `at` bars: it keeps the first part, a copy
/// (`name_b`) the rest and the `next:`, and follows it wherever the
/// arrangement plays it.
pub fn split_section(src: &str, section: &str, at: f64) -> Result<String, String> {
    let outline = source::outline(src);
    let ctor = outline.node(section).ok_or_else(|| format!("no section `{section}`"))?;
    let bars: f64 = ctor.field("bars").and_then(|f| f.value.text(src).trim().parse().ok()).unwrap_or(4.0);
    let at = at.round();
    if at < 1.0 || at > bars - 1.0 {
        return Err(format!("`{section}` has {} bars: split it after bar 1 to {}", source::fmt_num(bars, 2), source::fmt_num(bars - 1.0, 2)));
    }
    let name = free_name(src, &format!("{section}_b"));
    let text = node_text(src, ctor).to_string();
    let mut out = insert_after(src, section, &name, &text)?;
    out = set_bars(&out, &name, bars - at)?;
    out = set_bars(&out, section, at)?;
    if source::outline(&out).node(section).and_then(|n| n.field("next")).is_some() {
        out = source::apply(&out, &source::remove_field(&out, section, "next")?);
    }
    let arrange: Vec<String> = list_of(&out, "", "arrange").into_iter().flat_map(|s| if s == section { vec![s, name.clone()] } else { vec![s] }).collect();
    set_list(&out, "", "arrange", &arrange)
}

/// The section at `index` of the arrangement copied (`name_2`), the copy
/// played right after it.
pub fn duplicate_section(src: &str, index: usize) -> Result<String, String> {
    let mut arrange = list_of(src, "", "arrange");
    let section = arrange.get(index).cloned().ok_or("no such section in the arrangement")?;
    let outline = source::outline(src);
    let ctor = outline.node(&section).ok_or_else(|| format!("no section `{section}`"))?;
    let name = free_name(src, &format!("{section}_2"));
    let out = insert_after(src, &section, &name, node_text(src, ctor))?;
    arrange.insert(index + 1, name);
    set_list(&out, "", "arrange", &arrange)
}

/// The section at `index` played once more right after itself.
pub fn repeat_section(src: &str, index: usize, to: usize) -> Result<String, String> {
    let mut arrange = list_of(src, "", "arrange");
    let section = arrange.get(index).cloned().ok_or("no such section in the arrangement")?;
    arrange.insert(to.min(arrange.len()), section);
    set_list(src, "", "arrange", &arrange)
}

/// The section at `index` out of the arrangement, and out of the score
/// when it played nowhere else and nothing names it.
pub fn delete_section(src: &str, index: usize) -> Result<String, String> {
    let mut arrange = list_of(src, "", "arrange");
    if index >= arrange.len() {
        return Err("no such section in the arrangement".into());
    }
    let section = arrange.remove(index);
    let out = set_list(src, "", "arrange", &arrange)?;
    if arrange.contains(&section) {
        return Ok(out);
    }
    Ok(source::remove_node(&out, &section).unwrap_or(out))
}

/// The arrangement with the section at `from` moved to `to` (its place
/// once moved).
pub fn move_section(src: &str, from: usize, to: usize) -> Result<String, String> {
    let mut arrange = list_of(src, "", "arrange");
    if from >= arrange.len() {
        return Err("no such section in the arrangement".into());
    }
    let section = arrange.remove(from);
    arrange.insert(to.min(arrange.len()), section);
    set_list(src, "", "arrange", &arrange)
}

/// The one change from `old` to `new`: what differs between their common
/// head and tail.
pub fn splice_of(old: &str, new: &str) -> Splice {
    let head = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    let mut head = head;
    while !old.is_char_boundary(head) || !new.is_char_boundary(head) {
        head -= 1;
    }
    let max_tail = old.len().min(new.len()) - head;
    let mut tail = old.bytes().rev().zip(new.bytes().rev()).take(max_tail).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
        tail -= 1;
    }
    Splice { start: head, end: old.len() - tail, text: new[head..new.len() - tail].to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "use mod.score.*\nlet clock = Clock{bpm: 120 sig: [4 4]}\nlet kit = Drums{kit: @analog_hybrid}\nlet reese = Synth{preset: @reese mono: true}\nlet glass = Synth{preset: @glass_pad}\nlet lead = Synth{preset: @saw_lead mono: true}\n\nlet bass = Loop{play: reese beats: 4\n    code: fn(i) { play(@a1, {dur: 1}) sleep(1) play(@a1, {dur: 1}) sleep(1) play(@e2, {dur: 1}) sleep(1) play(@a1, {dur: 1}) sleep(1) }}\nlet pad = Loop{play: glass beats: 4\n    code: fn(i) { play([@a3 @c4 @e4], {dur: 3.9}) sleep(4) }}\nlet hook = Loop{play: lead beats: 4\n    code: fn(i) { play(@e5, {dur: 0.5}) sleep(1) play(@c5, {dur: 0.5}) sleep(3) }}\nlet kick = Loop{play: kit hits: \"x...x...x...x...\" note: @kick step: 0.25}\nlet hats = Loop{play: kit hits: \"..x...x...x...x.\" note: @hat step: 0.25}\n\nlet intro = Section{bars: 2 loops: [pad kick]}\nlet verse = Section{bars: 4 loops: [bass pad kick hats]}\nlet chorus = Section{bars: 4 loops: [bass pad hook kick hats] next: @stop}\n\nScore{label: \"T\" clock kit reese glass lead\n    bass pad hook kick hats\n    intro verse chorus\n    arrange: [intro verse verse chorus]}\n";

    fn model(src: &str) -> ArrangeModel {
        let doc = model::load(src).map_err(|d| format!("{d:?}")).unwrap().doc;
        build(&doc, src, clip_notes(src))
    }

    fn loads(src: &str) -> ScoreDoc {
        model::load(src).map_err(|d| format!("{d:?}")).unwrap().doc
    }

    #[test]
    fn arrange_lanes_name_their_role_and_instrument() {
        let m = model(SRC);
        let titles: Vec<(&str, &str)> = m.lanes.iter().map(|l| (l.name.as_str(), l.title.as_str())).collect();
        assert_eq!(titles, [("kick", "Kick · analog hybrid"), ("hats", "Hats · analog hybrid"), ("bass", "Bass · reese"), ("pad", "Pad · glass pad"), ("hook", "Lead · saw lead")]);
        let bass = m.lanes.iter().find(|l| l.name == "bass").unwrap();
        // a1..e2 (33..40) fitted to an octave around them.
        assert!(bass.low <= 33.0 && bass.high >= 40.0 && bass.high - bass.low == 11.0, "{} {}", bass.low, bass.high);
        let kick = m.lanes.iter().find(|l| l.name == "kick").unwrap();
        assert!(kick.drums && kick.rows.len() == 1);
        // By what it plays, with nothing in the words.
        let held = [0.0, 0.0, 0.0].map(|_| ClipNote { beat: 0.0, pitch: 64.0, dur: 4.0, vel: 1.0, hit: false });
        assert_eq!(role_of(None, false, "x synth", &held), Role::Pad);
        let fast: Vec<ClipNote> = (0..8).map(|i| ClipNote { beat: i as f64 * 0.25, pitch: 84.0, dur: 0.1, vel: 1.0, hit: false }).collect();
        assert_eq!(role_of(None, false, "x synth", &fast), Role::Arp);
        assert_eq!(role_of(None, false, "x synth", &fast[..1].iter().map(|n| ClipNote { pitch: 40.0, ..*n }).collect::<Vec<_>>()), Role::Bass);
    }

    #[test]
    fn arrange_sections_mark_repeats_and_hold_their_notes() {
        let m = model(SRC);
        let s: Vec<(&str, f64, f64, (usize, usize), usize)> = m.sections.iter().map(|s| (s.name.as_str(), s.start, s.bars, s.run, s.occurrence)).collect();
        assert_eq!(s, [("intro", 0.0, 2.0, (0, 1), 0), ("verse", 8.0, 4.0, (0, 2), 0), ("verse", 24.0, 4.0, (1, 2), 1), ("chorus", 40.0, 4.0, (0, 1), 0)]);
        assert_eq!(m.total, 56.0);
        // The hook plays only in the chorus: two notes a bar, from its start.
        assert!(m.notes.get("verse", 0, "hook").is_empty());
        let hook = m.notes.get("chorus", 0, "hook");
        assert_eq!(hook.len(), 8);
        assert_eq!((hook[0].beat, hook[0].pitch), (0.0, 76.0));
        // Each kick hit, four a bar, in the second verse too.
        assert_eq!(m.notes.get("verse", 1, "kick").len(), 16);
        assert!(m.notes.get("verse", 1, "kick").iter().all(|n| n.hit));
        assert_eq!(pitch_name(76.0, false), "E5");
        assert_eq!(pitch_name(61.0, false), "C#4");
    }

    #[test]
    fn arrange_chords_fit_the_bars() {
        let m = model(SRC);
        // A minor (a c e over a in the bass) through the song, one span.
        assert_eq!(m.chords, [(0, 14, "Am".to_string())]);
        let mut w = [0f32; 12];
        for pc in [7, 11, 2, 5] {
            w[pc] = 1.0;
        }
        assert_eq!(fit_chord(&w, Some(7)).as_deref(), Some("G7"));
        let mut w = [0f32; 12];
        for pc in [0, 4, 7] {
            w[pc] = 1.0;
        }
        assert_eq!(fit_chord(&w, Some(0)).as_deref(), Some("C"));
        assert_eq!(fit_chord(&[0.0; 12], None), None);
    }

    #[test]
    fn arrange_clip_edits_write_the_loops() {
        let out = remove_clip(SRC, "verse", "hats").unwrap();
        assert!(out.contains("let verse = Section{bars: 4 loops: [bass pad kick]}"));
        let out = move_clip(SRC, "chorus", "intro", "hook", false).unwrap();
        assert_eq!(loads(&out).sections.iter().map(|s| s.loops.clone()).collect::<Vec<_>>()[0], ["pad", "kick", "hook"]);
        assert!(out.contains("loops: [bass pad kick hats] next: @stop"));
        let out = move_clip(SRC, "chorus", "intro", "hook", true).unwrap();
        assert!(out.contains("loops: [bass pad hook kick hats] next: @stop") && out.contains("loops: [pad kick hook]"));
        assert_eq!(add_clip(SRC, "intro", "pad").unwrap(), SRC);
        let muted = set_flag(SRC, "kick", "mute", true).unwrap();
        assert!(loads(&muted).loop_def("kick").unwrap().mute);
        assert_eq!(set_flag(&muted, "kick", "mute", false).unwrap(), SRC);
    }

    #[test]
    fn arrange_section_edits_rewrite_the_arrangement() {
        let out = set_bars(SRC, "intro", 3.0).unwrap();
        assert!(out.contains("let intro = Section{bars: 3 loops: [pad kick]}"));
        let out = move_section(SRC, 3, 0).unwrap();
        assert!(out.contains("arrange: [chorus intro verse verse]"));
        let out = repeat_section(SRC, 0, 1).unwrap();
        assert!(out.contains("arrange: [intro intro verse verse chorus]"));
        // One verse out: the node stays for the other.
        let out = delete_section(SRC, 1).unwrap();
        assert!(out.contains("arrange: [intro verse chorus]") && out.contains("let verse = "));
        // The intro out: it played nowhere else, so it goes.
        let out = delete_section(SRC, 0).unwrap();
        assert!(out.contains("arrange: [verse verse chorus]") && !out.contains("intro"));
        assert!(loads(&out).sections.len() == 2);
    }

    #[test]
    fn arrange_split_and_duplicate_copy_the_section() {
        let out = split_section(SRC, "chorus", 1.0).unwrap();
        assert!(out.contains("let chorus = Section{bars: 1 loops: [bass pad hook kick hats]}\nlet chorus_b = Section{bars: 3 loops: [bass pad hook kick hats] next: @stop}"), "{out}");
        assert!(out.contains("intro verse chorus chorus_b\n") && out.contains("arrange: [intro verse verse chorus chorus_b]"));
        let doc = loads(&out);
        assert_eq!(doc.arrangement_beats(), 56.0);
        // Every place it plays splits.
        let out = split_section(SRC, "verse", 2.0).unwrap();
        assert!(out.contains("arrange: [intro verse verse_b verse verse_b chorus]"));
        assert!(split_section(SRC, "intro", 2.0).is_err());
        let out = duplicate_section(SRC, 0).unwrap();
        assert!(out.contains("let intro_2 = Section{bars: 2 loops: [pad kick]}") && out.contains("arrange: [intro intro_2 verse verse chorus]"));
        assert_eq!(loads(&out).sections.len(), 4);
    }

    #[test]
    fn arrange_splices_are_the_change_alone() {
        let new = set_bars(SRC, "intro", 3.0).unwrap();
        let s = splice_of(SRC, &new);
        assert_eq!(s.text, "3");
        assert_eq!(source::apply(SRC, &s), new);
        let s = splice_of("aé b", "aé cb");
        assert_eq!(source::apply("aé b", &s), "aé cb");
        assert_eq!(splice_of("same", "same").text, "");
    }
}
