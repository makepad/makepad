//! The tracker's model and its edits: the score's loops as channels, its
//! sections as patterns, and every cell mapped onto the exact bytes of the
//! source that say it. Source in, splices out: an edit never rewrites more
//! than the values it changes.
//!
//! What maps onto cells, both ways:
//! - `hits: "x..o"`: a step per character (whitespace and `|` are layout);
//!   a cell is its character (`x` `X` `o` `1`..`9` hits, `.` rests).
//! - `notes: [[beat note dur vel {opts}] …]`: an entry per note (inline, a
//!   list of voices, or voices named by `let rh = […]`), `[beat @program n]`
//!   and `[beat @pedal v]` in the effect column.
//!
//! What does not, shown read only from what it plays (the composer run off
//! the UI thread): `code: fn(i) {…}` bodies, entries written as expressions,
//! patterns built by expressions, and loops that play a live input.
//!
//! A loop cycles on its own length from the song's first beat, through the
//! sections it rests in, so the cell at a song beat is the loop's step at
//! that beat modulo its length: every repeat of a step is the same bytes.

use makepad_score_live::model::{InstKind, ScoreDoc};
use makepad_score_live::source::{self, Splice, Tok, Token};
use makepad_score_live::theory;
use makepad_score_player::presets;
use std::collections::HashMap;

/// Sub-columns a tracker channel splits its chords into.
pub const MAX_LANES: u8 = 8;
/// The effect column's lane.
pub const FX_LANE: u8 = 255;
/// The longest a read-only part is played through for its cells.
pub const MAX_BEATS: f64 = 4096.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Tracker,
    Roll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EvKind {
    Note,
    Hit,
    Program(u8),
    Pedal,
}

/// Where an event is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Src {
    /// Not in the source as a cell (a code body's note).
    None,
    /// A step of `hits:`.
    Step(usize),
    /// An entry of a note list.
    Entry(EntrySrc),
}

/// An entry's bytes: the brackets, and each value written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntrySrc {
    pub list: u16,
    /// Index in its list, in source order.
    pub index: u32,
    pub start: usize,
    pub end: usize,
    pub beat: (usize, usize),
    pub note: (usize, usize),
    pub dur: Option<(usize, usize)>,
    pub vel: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ev {
    /// Loop-local beats (from the start of its cycle); song beats for a
    /// read-only part.
    pub beat: f64,
    pub pitch: f32,
    pub dur: f32,
    pub vel: f32,
    pub kind: EvKind,
    pub lane: u8,
    pub src: Src,
    /// The cell's note text (`C-4`, `KCK`, `P12`) and velocity (`80`).
    pub text: String,
    pub vel_text: String,
    /// The note's options (`{pan …}`), shown in the effect column.
    pub opts: bool,
}

/// A note list's place in the source.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteList {
    /// The byte after its `[`.
    pub open: usize,
    /// Its entries' (start, end) in source order.
    pub entries: Vec<(usize, usize)>,
    /// The whitespace between two entries (what an insert repeats).
    pub sep: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Form {
    /// `hits:`: each step's byte, its character and the step length.
    Hits { steps: Vec<usize>, chars: Vec<u8>, step: f64 },
    Notes { lists: Vec<NoteList> },
    /// Read only, and why.
    Fixed(&'static str),
}

impl Form {
    pub fn editable(&self) -> bool {
        !matches!(self, Form::Fixed(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub name: String,
    /// Its `label:`, else the name.
    pub label: String,
    pub key: u64,
    pub play: Option<String>,
    /// The instrument in a few words (`saw_lead`, `kit · analog_hybrid`).
    pub inst_text: String,
    pub amp: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub sends: Vec<(String, f32)>,
    /// Its `color:` atom (a palette name), if any.
    pub color: Option<String>,
    /// Its cycle in beats.
    pub len: f64,
    pub form: Form,
    /// Sorted by beat.
    pub events: Vec<Ev>,
    /// Events are song beats (a read-only part played through).
    pub absolute: bool,
    pub lanes: u8,
    pub has_fx: bool,
    /// The pitch range a roll shows (whole octaves).
    pub lo: f32,
    pub hi: f32,
    /// The view its part suggests (mostly chords: the roll).
    pub auto_view: View,
    pub drums: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub name: String,
    /// Its first start in the song (None: outside the arrangement).
    pub start: Option<f64>,
    /// Every start in the song.
    pub starts: Vec<f64>,
    pub len: f64,
    /// The loops it plays (all of them for a score of loops).
    pub loops: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackModel {
    pub bar: f64,
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    /// The score's instrument nodes, and its effects (send targets).
    pub insts: Vec<String>,
    pub fx: Vec<String>,
    pub song_len: f64,
    /// No arrangement: one pattern, the loops' cycle.
    pub cycle: bool,
}

impl TrackModel {
    pub fn channel(&self, name: &str) -> Option<usize> {
        self.channels.iter().position(|c| c.name == name)
    }
}

impl Channel {
    /// The events between loop-local `from` and `to` beats (indices).
    pub fn range(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let a = self.events.partition_point(|e| e.beat < from - 1e-9);
        let b = self.events.partition_point(|e| e.beat < to - 1e-9);
        a..b.max(a)
    }

    /// The loop-local beat at song beat `song` (a read-only part keeps
    /// song beats).
    pub fn local(&self, song: f64) -> f64 {
        if self.absolute || self.len <= 0.0 { song } else { song.rem_euclid(self.len) }
    }
}

/// Tracker note text: `C-4`, `C#4`.
pub fn note_text(pitch: f32) -> String {
    const NAMES: [&str; 12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];
    let n = pitch.round() as i32;
    let octave = n.div_euclid(12) - 1;
    if (0..=9).contains(&octave) { format!("{}{}", NAMES[n.rem_euclid(12) as usize], octave) } else { format!("{n:>3}") }
}

/// A General MIDI drum's three letters.
pub fn drum_text(gm: u8) -> &'static str {
    match gm {
        35 | 36 => "KCK",
        37 => "RIM",
        38 | 40 => "SNR",
        39 => "CLP",
        42 => "CHH",
        44 => "PHH",
        46 => "OHH",
        41 | 43 => "FTM",
        45 | 47 => "LTM",
        48 | 50 => "HTM",
        49 | 57 => "CRS",
        51 | 59 => "RID",
        53 => "BEL",
        56 => "COW",
        70 => "SHK",
        60..=64 => "CNG",
        _ => "DRM",
    }
}

fn vel_text(vel: f32) -> String {
    format!("{:02}", (vel * 99.0).round().clamp(0.0, 99.0) as u32)
}

/// A hit's character for a velocity (as `theory::pattern` reads them back).
pub fn hit_char(vel: f32) -> u8 {
    if (vel - 1.0).abs() < 0.05 {
        b'X'
    } else if (vel - 0.8).abs() < 0.05 {
        b'x'
    } else if (vel - 0.45).abs() < 0.03 {
        b'o'
    } else {
        b'0' + (vel * 10.0).round().clamp(1.0, 9.0) as u8
    }
}

fn tok_text<'a>(src: &'a str, t: &Token) -> &'a str {
    &src[t.start..t.end]
}

fn is_punct(src: &str, t: Option<&Token>, p: &str) -> bool {
    t.is_some_and(|t| t.kind == Tok::Punct && tok_text(src, t) == p)
}

/// The index of the token closing the bracket at `open`.
fn close_of(src: &str, toks: &[Token], open: usize) -> Option<usize> {
    let (o, c) = match tok_text(src, &toks[open]) {
        "[" => ("[", "]"),
        "{" => ("{", "}"),
        _ => ("(", ")"),
    };
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate().skip(open) {
        if t.kind != Tok::Punct {
            continue;
        }
        let s = tok_text(src, t);
        if s == o {
            depth += 1;
        } else if s == c {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// The first token at or after byte `at`.
fn tok_at(toks: &[Token], at: usize) -> usize {
    toks.partition_point(|t| t.start < at)
}

/// A value read off an entry element.
enum Elem {
    Num(f64),
    Name(String),
    Opts,
}

/// One entry `[beat note dur vel {opts}]` from token `open` (its `[`): its
/// values with their bytes, or None when it is written with expressions.
fn entry(src: &str, toks: &[Token], open: usize, close: usize) -> Option<Vec<(Elem, (usize, usize))>> {
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < close {
        let t = toks[i];
        if t.kind == Tok::Comment || is_punct(src, Some(&t), ",") {
            i += 1;
            continue;
        }
        // A negative number is `-` then the number, nothing between.
        if is_punct(src, Some(&t), "-") && toks.get(i + 1).is_some_and(|n| n.kind == Tok::Num && n.start == t.end) {
            let n = toks[i + 1];
            out.push((Elem::Num(-tok_text(src, &n).parse::<f64>().ok()?), (t.start, n.end)));
            i += 2;
        } else if t.kind == Tok::Num {
            out.push((Elem::Num(tok_text(src, &t).parse::<f64>().ok()?), (t.start, t.end)));
            i += 1;
        } else if t.kind == Tok::Id {
            out.push((Elem::Name(tok_text(src, &t)[1..].to_string()), (t.start, t.end)));
            i += 1;
        } else if is_punct(src, Some(&t), "{") {
            let c = close_of(src, toks, i)?;
            out.push((Elem::Opts, (t.start, toks[c].end)));
            i = c + 1;
        } else {
            return None;
        }
        // An operator after a value: an expression.
        if toks.get(i).is_some_and(|n| n.kind == Tok::Punct && i < close && !matches!(tok_text(src, n), "," | "{" | "-")) {
            return None;
        }
    }
    Some(out)
}

/// `let name = [` in the source: the token of its `[`.
fn named_list(src: &str, toks: &[Token], name: &str) -> Option<usize> {
    (0..toks.len().saturating_sub(3)).find(|&i| {
        toks[i].kind == Tok::Ident && tok_text(src, &toks[i]) == "let" && toks[i + 1].kind == Tok::Ident && tok_text(src, &toks[i + 1]) == name && is_punct(src, toks.get(i + 2), "=") && is_punct(src, toks.get(i + 3), "[")
    }).map(|i| i + 3)
}

/// The list of entries opened at token `open` as a [`NoteList`], with its
/// entries parsed; None when one of them is not plain values.
fn note_list(src: &str, toks: &[Token], open: usize) -> Option<(NoteList, Vec<Vec<(Elem, (usize, usize))>>)> {
    let close = close_of(src, toks, open)?;
    let mut list = NoteList { open: toks[open].end, entries: Vec::new(), sep: String::new() };
    let mut parsed = Vec::new();
    let mut i = open + 1;
    while i < close {
        let t = toks[i];
        if t.kind == Tok::Comment || is_punct(src, Some(&t), ",") {
            i += 1;
            continue;
        }
        if !is_punct(src, Some(&t), "[") {
            return None;
        }
        let c = close_of(src, toks, i)?;
        parsed.push(entry(src, toks, i, c)?);
        list.entries.push((t.start, toks[c].end));
        i = c + 1;
    }
    list.sep = match list.entries.as_slice() {
        [.., a, b] => src[a.1..b.0].to_string(),
        _ => " ".to_string(),
    };
    // A comment between two entries is not a separator to repeat.
    if list.sep.contains("//") || list.sep.trim().len() > 1 {
        list.sep = " ".to_string();
    }
    Some((list, parsed))
}

/// The voices of `notes:` whose value starts at byte `at`: inline entries,
/// inline voices or voices named by `let`.
fn voices(src: &str, toks: &[Token], at: usize) -> Option<Vec<(NoteList, Vec<Vec<(Elem, (usize, usize))>>)>> {
    let open = tok_at(toks, at);
    if !is_punct(src, toks.get(open), "[") {
        return None;
    }
    let close = close_of(src, toks, open)?;
    // A list of entries: its first item opens a bracket whose first item is not a bracket.
    let first = (open + 1..close).find(|&i| toks[i].kind != Tok::Comment);
    let Some(first) = first else { return note_list(src, toks, open).map(|l| vec![l]) };
    let entries = is_punct(src, toks.get(first), "[") && !is_punct(src, toks.get(first + 1), "[") && !is_punct(src, toks.get(first + 1), "]");
    if entries {
        return note_list(src, toks, open).map(|l| vec![l]);
    }
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < close {
        let t = toks[i];
        if t.kind == Tok::Comment || is_punct(src, Some(&t), ",") {
            i += 1;
            continue;
        }
        if is_punct(src, Some(&t), "[") {
            out.push(note_list(src, toks, i)?);
            i = close_of(src, toks, i)? + 1;
        } else if t.kind == Tok::Ident {
            let named = named_list(src, toks, tok_text(src, &t))?;
            out.push(note_list(src, toks, named)?);
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

fn inst_text(doc: &ScoreDoc, outline: &source::Outline, src: &str, inst: &str) -> String {
    let Some(ctor) = outline.node(inst) else { return inst.to_string() };
    let what = ["preset", "kit", "kind", "lib", "program", "osc"].iter().find_map(|k| ctor.field(k)).map(|f| f.value.text(src).trim().trim_start_matches('@').trim_matches('"').to_string());
    let _ = doc;
    match what {
        Some(w) if w != inst => format!("{inst} · {w}"),
        _ => inst.to_string(),
    }
}

/// Assign sub-columns: a note takes the first lane free at its beat.
fn assign_lanes(events: &mut [Ev]) -> u8 {
    let mut ends: Vec<f64> = Vec::new();
    for e in events.iter_mut() {
        if !matches!(e.kind, EvKind::Note | EvKind::Hit) {
            e.lane = FX_LANE;
            continue;
        }
        let end = e.beat + (e.dur as f64).max(1e-3);
        match ends.iter().position(|x| *x <= e.beat + 1e-6) {
            Some(l) => {
                ends[l] = end;
                e.lane = l as u8;
            }
            None if ends.len() < MAX_LANES as usize => {
                ends.push(end);
                e.lane = (ends.len() - 1) as u8;
            }
            None => {
                // Every lane busy: the one free soonest.
                let l = ends.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(l, _)| l);
                ends[l] = end;
                e.lane = l as u8;
            }
        }
    }
    ends.len().max(1) as u8
}

/// The view a part suggests: the roll when most of its notes are chords.
fn auto_view(events: &[Ev]) -> View {
    let notes: Vec<&Ev> = events.iter().filter(|e| e.kind == EvKind::Note).collect();
    if notes.len() < 4 {
        return View::Tracker;
    }
    let chorded = notes.iter().enumerate().filter(|(i, e)| {
        notes.get(i.wrapping_sub(1)).is_some_and(|p| (p.beat - e.beat).abs() < 1e-3) || notes.get(i + 1).is_some_and(|n| (n.beat - e.beat).abs() < 1e-3)
    }).count();
    if chorded * 2 > notes.len() { View::Roll } else { View::Tracker }
}

fn finish(c: &mut Channel) {
    c.events.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    c.lanes = assign_lanes(&mut c.events);
    c.has_fx = c.events.iter().any(|e| e.lane == FX_LANE || e.opts);
    let pitched: Vec<f32> = c.events.iter().filter(|e| e.kind == EvKind::Note).map(|e| e.pitch).collect();
    let (lo, hi) = pitched.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let (mut lo, mut hi) = if pitched.is_empty() { (48.0, 72.0) } else { ((lo / 12.0).floor() * 12.0, (hi / 12.0).floor() * 12.0 + 11.0) };
    while hi - lo < 23.0 {
        lo = (lo - 12.0).max(0.0);
        hi = (hi + 12.0).min(127.0).max(lo + 23.0);
    }
    c.lo = lo;
    c.hi = hi.min(127.0);
    c.auto_view = if c.drums { View::Tracker } else { auto_view(&c.events) };
}

/// The model of `doc` (loaded from `src`). Read-only parts come in later
/// from [`played_parts`] through [`TrackModel::set_played`].
pub fn build(doc: &ScoreDoc, src: &str) -> TrackModel {
    let outline = source::outline(src);
    let toks = &outline.tokens;
    let bar = doc.beats_per_bar().max(1.0);
    let mut m = TrackModel { bar, ..TrackModel::default() };
    m.insts = doc.insts.iter().map(|i| i.name.clone()).collect();
    m.fx = doc.fx.iter().map(|f| f.name.clone()).collect();
    for l in &doc.loops {
        let ctor = outline.node(&l.name);
        let field = |key: &str| ctor.and_then(|c| c.field(key));
        let inst = l.play.as_deref().and_then(|p| doc.inst(p));
        let drums = matches!(inst.map(|i| &i.kind), Some(InstKind::Drums { .. })) || matches!(inst.map(|i| &i.kind), Some(InstKind::Native { native, .. }) if native == "drums") || l.hits.as_ref().is_some_and(|h| h.drum.is_some());
        let mut c = Channel {
            name: l.name.clone(),
            label: field("label").map(|f| f.value.text(src).trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| l.name.clone()),
            key: makepad_score_live::key(&l.name),
            play: l.play.clone(),
            inst_text: l.play.as_deref().map_or_else(|| "no instrument".to_string(), |p| inst_text(doc, &outline, src, p)),
            amp: l.amp,
            pan: l.pan,
            mute: l.mute,
            solo: l.solo,
            sends: l.sends.clone(),
            color: field("color").map(|f| f.value.text(src).trim().trim_start_matches('@').trim_matches('"').to_string()),
            len: 0.0,
            form: Form::Fixed("plays its code"),
            events: Vec::new(),
            absolute: false,
            lanes: 1,
            has_fx: false,
            lo: 48.0,
            hi: 71.0,
            auto_view: View::Tracker,
            drums,
        };
        // The hits, and their bytes when the field is one string.
        let mut hits_form = None;
        if let Some(h) = &l.hits {
            let (pitch, text) = match h.drum {
                Some((_, gm)) => (gm as f32, drum_text(gm).to_string()),
                None => (h.note, note_text(h.note)),
            };
            if let Some(f) = field("hits") {
                let t = tok_at(toks, f.value.start);
                if toks.get(t).is_some_and(|t| t.kind == Tok::Str && t.start == f.value.start && t.end == f.value.end) {
                    let inner = f.value.start + 1..f.value.end.saturating_sub(1);
                    let mut steps = Vec::new();
                    let mut chars = Vec::new();
                    for at in inner {
                        let b = src.as_bytes()[at];
                        if !b.is_ascii_whitespace() && b != b'|' {
                            steps.push(at);
                            chars.push(b);
                        }
                    }
                    if steps.len() == h.steps.len() {
                        hits_form = Some(Form::Hits { steps, chars, step: h.step });
                    }
                }
            }
            for (i, vel) in h.steps.iter().enumerate() {
                if let Some(vel) = vel {
                    let kind = if h.drum.is_some() { EvKind::Hit } else { EvKind::Note };
                    let src_of = if hits_form.is_some() { Src::Step(i) } else { Src::None };
                    c.events.push(Ev { beat: i as f64 * h.step, pitch, dur: (h.step * 0.9) as f32, vel: *vel, kind, lane: 0, src: src_of, text: text.clone(), vel_text: vel_text(*vel), opts: false });
                }
            }
            c.len = l.beats.unwrap_or(h.steps.len() as f64 * h.step);
            c.form = hits_form.clone().unwrap_or(Form::Fixed("its pattern is an expression"));
        }
        if l.code.is_some() {
            // Code plays too: the whole part is what it plays, read only.
            c.form = Form::Fixed("plays its code");
            c.events.clear();
        } else if l.notes.is_some() {
            let parsed = field("notes").and_then(|f| voices(src, toks, f.value.start));
            let notes = l.notes.as_deref().unwrap_or(&[]);
            let end = notes.iter().map(|n| n.beat + n.dur as f64).fold(0.0f64, f64::max);
            c.len = l.beats.unwrap_or_else(|| ((end / bar).ceil() * bar).max(bar));
            match parsed {
                Some(lists) => {
                    let mut events = Vec::new();
                    for (li, (_, entries)) in lists.iter().enumerate() {
                        for (ei, values) in entries.iter().enumerate() {
                            if let Some(ev) = entry_event(&lists[li].0, li as u16, ei as u32, values, drums) {
                                events.push(ev);
                            }
                        }
                    }
                    c.events = events;
                    c.form = Form::Notes { lists: lists.into_iter().map(|(l, _)| l).collect() };
                }
                None => {
                    c.form = Form::Fixed("its notes are written with expressions");
                    c.events = notes.iter().map(|n| list_event(n, drums)).collect();
                }
            }
        } else if l.hits.is_none() {
            c.form = Form::Fixed(if l.midi_in.is_some() { "plays a MIDI keyboard" } else { "plays a live input" });
        }
        if c.len <= 0.0 {
            c.len = l.beats.unwrap_or(bar);
        }
        finish(&mut c);
        m.channels.push(c);
    }
    // Patterns: the arrangement's sections in order (each once), then the
    // sections it does not play; a score of loops is one pattern.
    if doc.arrange.is_empty() || doc.sections.is_empty() {
        m.cycle = true;
        let len = m.channels.iter().filter(|c| !c.absolute).map(|c| c.len).fold(0.0, f64::max).max(bar);
        let len = (len / bar).ceil() * bar;
        m.song_len = len;
        m.patterns.push(Pattern { name: "loops".into(), start: Some(0.0), starts: vec![0.0], len, loops: doc.loops.iter().map(|l| l.name.clone()).collect() });
    } else {
        let mut at = 0.0;
        let mut starts: HashMap<&str, Vec<f64>> = HashMap::new();
        for name in &doc.arrange {
            let Some(s) = doc.sections.iter().find(|s| &s.name == name) else { continue };
            starts.entry(s.name.as_str()).or_default().push(at);
            at += s.bars * bar;
        }
        m.song_len = at;
        let order = doc.arrange.iter().filter_map(|n| doc.sections.iter().find(|s| &s.name == n)).chain(doc.sections.iter());
        for s in order {
            if m.patterns.iter().any(|p| p.name == s.name) {
                continue;
            }
            let st = starts.get(s.name.as_str()).cloned().unwrap_or_default();
            m.patterns.push(Pattern { name: s.name.clone(), start: st.first().copied(), starts: st, len: (s.bars * bar).max(bar / 4.0), loops: s.loops.clone() });
        }
    }
    m
}

fn entry_event(list: &NoteList, li: u16, ei: u32, values: &[(Elem, (usize, usize))], drums: bool) -> Option<Ev> {
    let (beat, beat_span) = match values.first()? {
        (Elem::Num(b), s) => (*b, *s),
        _ => return None,
    };
    let (note_elem, note_span) = values.get(1)?;
    let span = list.entries[ei as usize];
    let mut src = EntrySrc { list: li, index: ei, start: span.0, end: span.1, beat: beat_span, note: *note_span, dur: None, vel: None };
    let num = |k: usize| values.get(k).and_then(|(e, s)| if let Elem::Num(n) = e { Some((*n, *s)) } else { None });
    let opts = values.iter().any(|(e, _)| matches!(e, Elem::Opts));
    match note_elem {
        Elem::Name(n) if n == "program" => {
            let p = num(2).map_or(0.0, |v| v.0).clamp(0.0, 127.0) as u8;
            src.vel = num(2).map(|v| v.1);
            return Some(Ev { beat, pitch: 0.0, dur: 0.0, vel: 0.0, kind: EvKind::Program(p), lane: FX_LANE, src: Src::Entry(src), text: format!("P{p:02}"), vel_text: String::new(), opts: false });
        }
        Elem::Name(n) if n == "pedal" => {
            let v = num(2).map_or(0.0, |v| v.0).clamp(0.0, 1.0) as f32;
            src.vel = num(2).map(|v| v.1);
            return Some(Ev { beat, pitch: 0.0, dur: 0.0, vel: v, kind: EvKind::Pedal, lane: FX_LANE, src: Src::Entry(src), text: format!("S{}", vel_text(v)), vel_text: String::new(), opts: false });
        }
        _ => {}
    }
    let pitch = match note_elem {
        Elem::Num(n) => *n as f32,
        Elem::Name(n) => theory::note_number(n).or_else(|| presets::gm_drum(n).map(|g| g as f32))?,
        Elem::Opts => return None,
    };
    let dur = num(2);
    let vel = num(3);
    src.dur = dur.map(|d| d.1);
    src.vel = vel.map(|v| v.1);
    let (dur, vel) = (dur.map_or(0.5, |d| d.0).clamp(0.0, 4096.0) as f32, vel.map_or(0.8, |v| v.0).clamp(0.0, 1.0) as f32);
    let hit = drums && pitch >= 27.0 && pitch <= 87.0;
    let text = if hit { drum_text(pitch as u8).to_string() } else { note_text(pitch) };
    Some(Ev { beat, pitch, dur, vel, kind: if hit { EvKind::Hit } else { EvKind::Note }, lane: 0, src: Src::Entry(src), text, vel_text: vel_text(vel), opts })
}

fn list_event(n: &makepad_score_live::model::ListNote, drums: bool) -> Ev {
    use makepad_score_live::model::ListKind;
    match n.kind {
        ListKind::Program(p) => Ev { beat: n.beat, pitch: 0.0, dur: 0.0, vel: 0.0, kind: EvKind::Program(p), lane: FX_LANE, src: Src::None, text: format!("P{p:02}"), vel_text: String::new(), opts: false },
        ListKind::Pedal => Ev { beat: n.beat, pitch: 0.0, dur: 0.0, vel: n.vel, kind: EvKind::Pedal, lane: FX_LANE, src: Src::None, text: format!("S{}", vel_text(n.vel)), vel_text: String::new(), opts: false },
        ListKind::Note => {
            let hit = drums;
            Ev { beat: n.beat, pitch: n.note, dur: n.dur, vel: n.vel, kind: if hit { EvKind::Hit } else { EvKind::Note }, lane: 0, src: Src::None, text: if hit { drum_text(n.note as u8).to_string() } else { note_text(n.note) }, vel_text: vel_text(n.vel), opts: n.opts != 0 }
        }
    }
}

/// What the read-only parts play, per loop, in song beats: the composer
/// run over the song once, without sound. Off the UI thread.
pub type Played = HashMap<String, Vec<(f64, f32, f32, f32, bool)>>;

pub fn played_parts(source: &str) -> Played {
    use makepad_score_live::compose::Composer;
    let Ok(loaded) = makepad_score_live::model::load(source) else { return Played::default() };
    let fixed: Vec<String> = loaded.doc.loops.iter().filter(|l| l.code.is_some()).map(|l| l.name.clone()).collect();
    let mut out = Played::default();
    if fixed.is_empty() {
        return out;
    }
    let doc = &loaded.doc;
    let bar = doc.beats_per_bar().max(1.0);
    let end = if doc.arrange.is_empty() {
        let longest = doc.loops.iter().filter_map(|l| l.beats).fold(0.0, f64::max).max(4.0 * bar);
        (longest / bar).ceil() * bar
    } else {
        doc.arrange.iter().filter_map(|n| doc.sections.iter().find(|s| &s.name == n)).map(|s| s.bars * bar).sum()
    }.min(MAX_BEATS);
    let mut composer = Composer::new(loaded);
    let mut events = Vec::new();
    let mut until = 0.0;
    while until < end {
        until = (until + bar).min(end);
        composer.pump(until, &mut events);
        events.clear();
        for p in composer.history.drain(..) {
            if p.beat < end && fixed.contains(&p.loop_name) {
                // A hit's note is its kit slot: shown as its GM drum.
                let note = if p.hit { presets::DRUM_SLOTS.get(p.note as usize).and_then(|n| presets::gm_drum(n)).unwrap_or(36) as f32 } else { p.note };
                out.entry(p.loop_name.clone()).or_default().push((p.beat, note, p.dur, p.vel, p.hit));
            }
        }
    }
    out
}

impl TrackModel {
    /// The read-only parts' notes as played.
    pub fn set_played(&mut self, played: &Played) {
        for c in &mut self.channels {
            if !matches!(c.form, Form::Fixed("plays its code")) {
                continue;
            }
            c.absolute = true;
            c.len = self.song_len.max(self.bar);
            c.events = played.get(&c.name).map(|notes| notes.iter().map(|&(beat, pitch, dur, vel, hit)| {
                let text = if hit { drum_text(pitch as u8).to_string() } else { note_text(pitch) };
                Ev { beat, pitch, dur, vel, kind: if hit { EvKind::Hit } else { EvKind::Note }, lane: 0, src: Src::None, text, vel_text: vel_text(vel), opts: false }
            }).collect()).unwrap_or_default();
            finish(c);
        }
    }
}

// ------------------------------------------------------------ edits

/// Order splices for one change: disjoint, an insert at a deletion's start
/// kept after it; inserts at one place merged in the order given.
pub fn normalize(mut splices: Vec<Splice>) -> Result<Vec<Splice>, String> {
    // Inserts at one place, merged (stable: in the order given).
    let mut merged: Vec<Splice> = Vec::with_capacity(splices.len());
    for s in splices.drain(..) {
        if s.start == s.end {
            if let Some(m) = merged.iter_mut().find(|m| m.start == s.start && m.end == s.end) {
                m.text.push_str(&s.text);
                continue;
            }
        }
        merged.push(s);
    }
    // Ascending; at one start the insert first (it lands before what the
    // deletion leaves).
    merged.sort_by(|a, b| a.start.cmp(&b.start).then((a.end - a.start).cmp(&(b.end - b.start))));
    for w in merged.windows(2) {
        if w[0].end > w[1].start {
            return Err("two edits of one change overlap".into());
        }
    }
    Ok(merged)
}

/// Apply normalized splices (back to front).
pub fn apply_all(src: &str, splices: &[Splice]) -> String {
    let mut out = src.to_string();
    for s in splices.iter().rev() {
        out.replace_range(s.start..s.end, &s.text);
    }
    out
}

/// `@c4`, or the drum's name on a drum part.
pub fn note_atom(pitch: f32, drums: bool) -> String {
    if drums {
        let name = match pitch.round() as u8 {
            36 | 35 => "kick",
            38 | 40 => "snare",
            37 => "rim",
            39 => "clap",
            42 => "hat",
            44 => "pedalhat",
            46 => "ohat",
            49 | 57 => "crash",
            51 | 59 => "ride",
            53 => "bell",
            56 => "cowbell",
            70 => "shaker",
            43 | 41 => "floortom",
            45 | 47 => "tom3",
            48 | 50 => "tom1",
            _ => "",
        };
        if !name.is_empty() {
            return format!("@{name}");
        }
    }
    format!("@{}", theory::note_name(pitch))
}

fn num(v: f64) -> String {
    source::fmt_num(v, 4)
}

/// A new entry's text.
pub fn entry_text(beat: f64, pitch: f32, dur: f64, vel: f32, drums: bool) -> String {
    format!("[{} {} {} {}]", num(beat), note_atom(pitch, drums), num(dur), source::fmt_num(vel as f64, 2))
}

/// Set a hits step's character.
pub fn set_step(c: &Channel, step: usize, ch: u8) -> Option<Splice> {
    let Form::Hits { steps, chars, .. } = &c.form else { return None };
    let at = *steps.get(step)?;
    (chars[step] != ch).then(|| Splice { start: at, end: at + 1, text: (ch as char).to_string() })
}

/// Insert a note into list `list` of a notes part, at its place by beat.
pub fn insert_entry(c: &Channel, list: usize, text: String, beat: f64) -> Option<Splice> {
    let Form::Notes { lists } = &c.form else { return None };
    let l = lists.get(list)?;
    // The first entry (in source order) after this beat.
    let beats: Vec<f64> = c.events.iter().filter_map(|e| match e.src {
        Src::Entry(s) if s.list as usize == list => Some((s.index, e.beat)),
        _ => None,
    }).fold(vec![f64::NAN; l.entries.len()], |mut v, (i, b)| {
        v[i as usize] = b;
        v
    });
    let next = (0..l.entries.len()).find(|&i| beats[i] > beat + 1e-9);
    Some(match next {
        Some(0) => Splice { start: l.entries[0].0, end: l.entries[0].0, text: format!("{text}{}", l.sep) },
        Some(k) => Splice { start: l.entries[k - 1].1, end: l.entries[k - 1].1, text: format!("{}{text}", l.sep) },
        None => match l.entries.last() {
            Some(last) => Splice { start: last.1, end: last.1, text: format!("{}{text}", l.sep) },
            None => Splice { start: l.open, end: l.open, text },
        },
    })
}

/// Remove an entry with the space before it.
pub fn delete_entry(src: &str, c: &Channel, e: &EntrySrc) -> Option<Splice> {
    let Form::Notes { lists } = &c.form else { return None };
    let l = lists.get(e.list as usize)?;
    let before = if e.index == 0 { l.open } else { l.entries[e.index as usize - 1].1 };
    let start = if e.index == 0 {
        // The first entry takes the space after it instead.
        let next = l.entries.get(1).map_or(e.end, |n| n.0);
        return Some(Splice { start: e.start, end: if l.entries.len() > 1 { next } else { e.end }, text: String::new() });
    } else {
        before
    };
    let _ = src;
    Some(Splice { start, end: e.end, text: String::new() })
}

/// Replace a value of an entry (dur, vel) that may not be written: the
/// missing ones up to it are written with their defaults.
pub fn set_entry_value(e: &EntrySrc, ev: &Ev, dur: Option<f64>, vel: Option<f32>) -> Vec<Splice> {
    let mut out = Vec::new();
    match (e.dur, e.vel) {
        (Some(d), Some(v)) => {
            if let Some(dur) = dur {
                out.push(Splice { start: d.0, end: d.1, text: num(dur) });
            }
            if let Some(vel) = vel {
                out.push(Splice { start: v.0, end: v.1, text: source::fmt_num(vel as f64, 2) });
            }
        }
        (Some(d), None) => {
            if let Some(dur) = dur {
                out.push(Splice { start: d.0, end: d.1, text: num(dur) });
            }
            if let Some(vel) = vel {
                out.push(Splice { start: d.1, end: d.1, text: format!(" {}", source::fmt_num(vel as f64, 2)) });
            }
        }
        (None, _) => {
            let dur = dur.unwrap_or(ev.dur as f64);
            let text = match vel {
                Some(vel) => format!(" {} {}", num(dur), source::fmt_num(vel as f64, 2)),
                None => format!(" {}", num(dur)),
            };
            out.push(Splice { start: e.note.1, end: e.note.1, text });
        }
    }
    out
}

/// The splices that change `ev` (an entry or a step) to `to` (beat, pitch,
/// dur, vel): only the values that differ.
pub fn change_event(c: &Channel, ev: &Ev, beat: f64, pitch: f32, dur: f64, vel: f32) -> Vec<Splice> {
    match ev.src {
        Src::Step(i) => {
            let ch = hit_char(vel);
            set_step(c, i, ch).into_iter().collect()
        }
        Src::Entry(e) => {
            let mut out = Vec::new();
            if (beat - ev.beat).abs() > 1e-9 {
                out.push(Splice { start: e.beat.0, end: e.beat.1, text: num(beat.max(0.0)) });
            }
            if matches!(ev.kind, EvKind::Note | EvKind::Hit) && (pitch - ev.pitch).abs() > 1e-3 {
                out.push(Splice { start: e.note.0, end: e.note.1, text: note_atom(pitch, c.drums) });
            }
            let dur_changed = (dur - ev.dur as f64).abs() > 1e-6;
            let vel_changed = (vel - ev.vel).abs() > 1e-3;
            if dur_changed || vel_changed {
                out.extend(set_entry_value(&e, ev, dur_changed.then_some(dur), vel_changed.then_some(vel)));
            }
            out
        }
        Src::None => Vec::new(),
    }
}

/// `sends: {room: 0.3}` with one send set (0 removes it).
pub fn sends_text(sends: &[(String, f32)], fx: &str, amount: f32) -> String {
    let mut out: Vec<(String, f32)> = sends.iter().filter(|(n, _)| n != fx).cloned().collect();
    if amount > 0.001 {
        out.push((fx.to_string(), amount));
    }
    let parts: Vec<String> = out.iter().map(|(n, a)| format!("{n}: {}", source::fmt_num(*a as f64, 2))).collect();
    format!("{{{}}}", parts.join(" "))
}

/// The slot counts a hit pattern can be split into: straight and triplet.
pub const SLOTS: [usize; 6] = [8, 12, 16, 24, 32, 64];

/// A loop's hit pattern: its slots, and the bytes of `hits:` and `step:`.
pub struct HitsAt {
    pub chars: Vec<u8>,
    pub step: f64,
    /// The `hits:` value's span (quotes included) and `step:`'s, if written.
    pub hits: (usize, usize),
    pub step_at: Option<(usize, usize)>,
    pub grouped: Option<char>,
}

pub fn hits_at(src: &str, name: &str) -> Result<HitsAt, String> {
    let outline = source::outline(src);
    let ctor = outline.node(name).ok_or_else(|| format!("no loop `{name}`"))?;
    let f = ctor.field("hits").ok_or_else(|| format!("`{name}` has no hits pattern"))?;
    let text = f.value.text(src);
    if !(text.len() >= 2 && text.starts_with('"') && text.ends_with('"') && !text[1..text.len() - 1].contains('"')) {
        return Err(format!("`{name}`'s hits are an expression: change it in the code"));
    }
    let inner = &text[1..text.len() - 1];
    let chars: Vec<u8> = inner.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'|').collect();
    let grouped = if inner.contains('|') { Some('|') } else if inner.contains(' ') { Some(' ') } else { None };
    let (step, step_at) = match ctor.field("step") {
        Some(sf) => (sf.value.text(src).trim().parse::<f64>().map_err(|_| format!("`{name}`'s step is an expression: change it in the code"))?, Some((sf.value.start, sf.value.end))),
        None => (0.25, None),
    };
    Ok(HitsAt { chars, step, hits: (f.value.start, f.value.end), step_at, grouped })
}

/// Split loop `name`'s hit pattern into `slots` over the same length: every
/// hit keeps its time (`x...` in 16 is `x.......` in 32). Refused when a hit
/// would fall between the new slots. One splice (one undo step) for the
/// pattern and its `step:`.
pub fn resample_hits(src: &str, name: &str, slots: usize) -> Result<Splice, String> {
    let h = hits_at(src, name)?;
    let n = h.chars.len().max(1);
    if slots == n {
        return Err(format!("`{name}` has {n} slots already"));
    }
    let len = n as f64 * h.step;
    let step = len / slots as f64;
    if step < 1.0 / 64.0 - 1e-9 {
        return Err(format!("{slots} slots over {} beats is finer than 1/64 beat", source::fmt_num(len, 3)));
    }
    let mut out = vec![b'.'; slots];
    for (i, c) in h.chars.iter().enumerate() {
        if matches!(c, b'.' | b'-' | b'_') {
            continue;
        }
        let j = i as f64 * h.step / step;
        if (j - j.round()).abs() > 1e-6 {
            return Err(format!("a hit at beat {} falls between {slots} slots: clear it first or pick a finer split", source::fmt_num(i as f64 * h.step, 3)));
        }
        out[j.round() as usize] = *c;
    }
    // Grouped as it was: a group per beat when a beat is whole slots.
    let per_beat = slots as f64 / len.max(1e-9);
    let mut text = String::with_capacity(slots * 2 + 2);
    text.push('"');
    for (k, c) in out.iter().enumerate() {
        if let Some(sep) = h.grouped {
            if k > 0 && (per_beat - per_beat.round()).abs() < 1e-9 && per_beat >= 2.0 && k % per_beat.round() as usize == 0 {
                text.push(sep);
            }
        }
        text.push(*c as char);
    }
    text.push('"');
    let step_text = source::fmt_num(step, 6);
    let mut splices = vec![Splice { start: h.hits.0, end: h.hits.1, text }];
    match h.step_at {
        Some(at) => splices.push(Splice { start: at.0, end: at.1, text: step_text }),
        None if (step - 0.25).abs() > 1e-12 => splices.push(Splice { start: h.hits.1, end: h.hits.1, text: format!(" step: {step_text}") }),
        None => {}
    }
    let splices = normalize(splices)?;
    // One splice from the first change to the last (the bytes between kept).
    let (a, b) = (splices[0].start, splices[splices.len() - 1].end);
    let changed = apply_all(&src[a..b], &splices.iter().map(|s| Splice { start: s.start - a, end: s.end - a, text: s.text.clone() }).collect::<Vec<_>>());
    Ok(Splice { start: a, end: b, text: changed })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "use mod.score.*\nlet clock = Clock{bpm: 120 sig: [4 4]}\nlet kit = Drums{kit: @analog_hybrid}\nlet keys = Synth{preset: @epiano}\nlet piano = Instrument{kind: @piano}\nlet kick = Loop{play: kit hits: \"x...|x.o.\" note: @kick}\nlet line = Loop{play: keys notes: [[0 @c4 1 0.5] [1 @e4] [2 @g4 0.5 0.7]] beats: 4}\nlet rh = [\n    [0 @c5 1 0.5]\n    [1 @d5 1 0.5]\n]\nlet lh = [[0 @c3 2 0.4] [0 @g3 2 0.4] [0 @pedal 1]]\nlet p = Loop{play: piano beats: 4 notes: [rh lh]}\nlet pad = Loop{play: keys beats: 4 code: fn(i) { play(@c3, {dur: 4}) sleep(4) }}\nScore{clock kit keys piano kick line p pad}\n";

    fn model() -> TrackModel {
        let doc = makepad_score_live::model::load(SRC).unwrap().doc;
        build(&doc, SRC)
    }

    fn edit(src: &str, splices: Vec<Splice>) -> String {
        apply_all(src, &normalize(splices).unwrap())
    }

    #[test]
    fn tracker_maps_hits_and_note_lists_onto_their_bytes() {
        let m = model();
        let kick = &m.channels[m.channel("kick").unwrap()];
        let Form::Hits { steps, chars, .. } = &kick.form else { panic!("{:?}", kick.form) };
        assert_eq!(steps.len(), 8);
        assert_eq!(chars, b"x...x.o.");
        assert_eq!(kick.len, 2.0);
        assert_eq!(kick.events.len(), 3);
        assert_eq!(kick.events[0].text, "KCK");
        let line = &m.channels[m.channel("line").unwrap()];
        assert!(matches!(line.form, Form::Notes { .. }));
        assert_eq!(line.events.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["C-4", "E-4", "G-4"]);
        assert_eq!(line.events[1].dur, 0.5);
        let p = &m.channels[m.channel("p").unwrap()];
        let Form::Notes { lists } = &p.form else { panic!() };
        assert_eq!(lists.len(), 2);
        assert_eq!(lists[0].sep, "\n    ");
        assert!(p.events.iter().any(|e| e.kind == EvKind::Pedal && e.lane == FX_LANE));
        // The two left-hand notes at 0 are a chord: two lanes.
        assert!(p.lanes >= 3, "{}", p.lanes);
        let pad = &m.channels[m.channel("pad").unwrap()];
        assert!(!pad.form.editable());
        assert_eq!(m.patterns.len(), 1);
        assert!(m.cycle);
    }

    #[test]
    fn tracker_edits_splice_only_the_values() {
        let m = model();
        let kick = &m.channels[m.channel("kick").unwrap()];
        let s = edit(SRC, vec![set_step(kick, 1, b'X').unwrap()]);
        assert_eq!(s, SRC.replace("\"x...|x.o.\"", "\"xX..|x.o.\""));
        let line = &m.channels[m.channel("line").unwrap()];
        // E4 to F4, its velocity written (its dur first, as it was).
        let e = &line.events[1];
        let s = edit(SRC, change_event(line, e, e.beat, 65.0, e.dur as f64, 0.9));
        assert!(s.contains("[1 @f4 0.5 0.9]"), "{s}");
        // An insert between two entries, one at the end.
        let a = insert_entry(line, 0, entry_text(1.5, 62.0, 0.5, 0.8, false), 1.5).unwrap();
        let b = insert_entry(line, 0, entry_text(3.0, 64.0, 1.0, 0.8, false), 3.0).unwrap();
        let s = edit(SRC, vec![a, b]);
        assert!(s.contains("notes: [[0 @c4 1 0.5] [1 @e4] [1.5 @d4 0.5 0.8] [2 @g4 0.5 0.7] [3 @e4 1 0.8]]"), "{s}");
        // Deleting the first and inserting before it, in one change.
        let Src::Entry(first) = line.events[0].src else { panic!() };
        let d = delete_entry(SRC, line, &first).unwrap();
        let i = insert_entry(line, 0, entry_text(0.0, 59.0, 1.0, 0.8, false), -1.0).unwrap();
        let s = edit(SRC, vec![d, i]);
        assert!(s.contains("notes: [[0 @b3 1 0.8] [1 @e4] [2 @g4 0.5 0.7]]"), "{s}");
        // A named voice keeps its layout.
        let p = &m.channels[m.channel("p").unwrap()];
        let i = insert_entry(p, 0, entry_text(2.0, 76.0, 1.0, 0.5, false), 2.0).unwrap();
        let s = edit(SRC, vec![i]);
        assert!(s.contains("    [1 @d5 1 0.5]\n    [2 @e5 1 0.5]\n]"), "{s}");
        // Every edit still loads.
        makepad_score_live::model::load(&s).unwrap();
    }

    #[test]
    fn tracker_sends_and_fields() {
        assert_eq!(sends_text(&[("room".into(), 0.3)], "echo", 0.25), "{room: 0.3 echo: 0.25}");
        assert_eq!(sends_text(&[("room".into(), 0.3)], "room", 0.0), "{}");
        let s = apply_all(SRC, &[source::set_field(SRC, "kick", "amp", "0.6").unwrap()]);
        assert!(s.contains("note: @kick amp: 0.6}"), "{s}");
    }

    #[test]
    fn tracker_hits_split_keeps_every_hit_in_time() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120 sig: [4 4]}\nlet kit = Drums{kit: @analog_hybrid}\nlet kick = Loop{play: kit hits: \"x...x...x...x...\" note: @kick}\nScore{clock kit kick}\n";
        let loads = |s: &str| {
            if let Err(d) = makepad_score_live::model::load(s) {
                panic!("does not load: {d:?}\n{s}");
            }
        };
        loads(src);
        // Finer: 16 to 32, the step halves.
        let s = apply_all(src, &[resample_hits(src, "kick", 32).unwrap()]);
        assert!(s.contains("hits: \"x.......x.......x.......x.......\" step: 0.125 note: @kick"), "{s}");
        loads(&s);
        // And back: 32 to 16, the written step goes back to 0.25.
        let back = apply_all(&s, &[resample_hits(&s, "kick", 16).unwrap()]);
        assert!(back.contains("hits: \"x...x...x...x...\" step: 0.25 note: @kick"), "{back}");
        // Triplets: 16 to 12, the beats stay on the beat.
        let t = apply_all(src, &[resample_hits(src, "kick", 12).unwrap()]);
        assert!(t.contains("hits: \"x..x..x..x..\" step: 0.3333"), "{t}");
        loads(&t);
        // Coarser refused when a hit falls between: an off-beat at 0.25.
        let off = src.replace("x...x...", "xx..x...");
        let e = resample_hits(&off, "kick", 8).unwrap_err();
        assert!(e.contains("falls between"), "{e}");
        // Coarser allowed when every hit lands: 16 to 8.
        let c = apply_all(src, &[resample_hits(src, "kick", 8).unwrap()]);
        assert!(c.contains("hits: \"x.x.x.x.\" step: 0.5"), "{c}");
        loads(&c);
        // Groups kept: a separator per beat.
        let g = src.replace("x...x...x...x...", "x... x... x... x...");
        let gs = apply_all(&g, &[resample_hits(&g, "kick", 32).unwrap()]);
        assert!(gs.contains("hits: \"x....... x....... x....... x.......\""), "{gs}");
    }
}
