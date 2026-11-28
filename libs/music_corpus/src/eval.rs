//! The yardstick: a model's answers to held-out examples, scored against
//! the songs they came from. A prediction file is JSONL, one
//! `{"id": "<example id>", "output": "<the model's answer>"}` a line.
//!
//! Per example (0..1, then averaged per task):
//! - `parsed`: the answer reads as a roll (`roll::from_text`) or loads and
//!   plays as a Score (`roll::from_score`);
//! - `key`: the key of its notes is the one asked for (0.5 for the
//!   relative key or a fifth away);
//! - `meter`: the meter asked for, every bar its length;
//! - `bars`: as many bars as asked for;
//! - `density`: notes against the reference (the smaller over the larger);
//! - `pitch`: pitch-class profile against the reference (cosine);
//! - `rhythm`: onset positions in the bar against the reference (cosine);
//! - `notes_f1`: the reference's notes found on the grid (exact step,
//!   pitch or drum), F1: the transcription tasks should reach 1;
//! - `repeat`: how much the answer repeats its own bars, against the
//!   reference (1 - the difference);
//! - `form`: the form `midi_library::form` hears (section letters),
//!   against the reference's (1 - edit distance / length).
//!
//! The baselines a fine-tune has to beat: `heuristic` (a stock groove in the
//! key for `describe`, the last bars again for `continue`, roots, triads and
//! a backbeat from the other parts' harmony for `infill`, the input copied
//! for the Score tasks) and `oracle` (the reference itself: every metric 1,
//! a check of the metrics).

use crate::prep;
use crate::tasks::{self, Task};
use makepad_midi_library::analyse;
use makepad_midi_library::arrangement::{self, Arrangement, Note, Part};
use makepad_midi_library::form;
use makepad_midi_library::gm::{Role, Voice};
use makepad_midi_library::roll::{self, Detail, STEPS};
use makepad_score_live::{LoadError, PrepareControl};
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// A held-out example as the eval needs it.
#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub task: Task,
    pub user: String,
    pub reference: String,
    pub key: String,
    pub meter: String,
    pub bars: usize,
    pub from_bar: usize,
    pub part: String,
    pub role: String,
}

/// Every example of a split, in file order (no augmented copies: those are
/// train only).
pub fn load_cases(dir: &Path, split: &str) -> Result<Vec<Case>, String> {
    let d = dir.join(split);
    let mut files: Vec<_> = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")).collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).map_err(|e| e.to_string())?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v = makepad_strict_json::parse(line.as_bytes()).map_err(|e| format!("{}: {e}", f.display()))?;
            let st = |v: Option<&Value>| v.and_then(|x| x.as_str()).unwrap_or("").to_string();
            let msgs = v.get("messages").and_then(|m| m.as_arr()).unwrap_or(&[]);
            let content = |role: &str| msgs.iter().find(|m| m.get("role").and_then(|r| r.as_str()) == Some(role)).map(|m| st(m.get("content"))).unwrap_or_default();
            let want = v.get("want");
            let Some(task) = Task::parse(&st(v.get("task"))) else { continue };
            out.push(Case {
                id: st(v.get("id")),
                task,
                user: content("user"),
                reference: content("assistant"),
                key: st(want.and_then(|w| w.get("key"))),
                meter: st(want.and_then(|w| w.get("meter"))),
                bars: want.and_then(|w| w.get("bars")).and_then(|b| b.as_i64()).unwrap_or(0) as usize,
                from_bar: want.and_then(|w| w.get("from_bar")).and_then(|b| b.as_i64()).unwrap_or(0) as usize,
                part: st(want.and_then(|w| w.get("part"))),
                role: st(want.and_then(|w| w.get("role"))),
            });
        }
    }
    Ok(out)
}

/// The text after the request's first blank line (the roll or Score it
/// gives).
pub fn given(user: &str) -> &str {
    user.split_once("\n\n").map_or("", |x| x.1)
}

// ------------------------------------------------------------------ reading answers

/// The answer as an arrangement of just what it wrote.
fn read_answer(c: &Case, text: &str, control: &PrepareControl) -> Result<Option<Arrangement>, LoadError> {
    if control.is_cancelled() { return Err(LoadError::Cancelled); }
    match c.task {
        // Not a Score at all: no need to run the engine on it.
        Task::RollToScore if !(text.contains("use mod.score") && text.contains("Score{")) => Ok(None),
        Task::RollToScore => match roll::from_score_controlled(text, control) {
            Ok(score) => Ok(Some(score)),
            Err(LoadError::Problems(_)) => Ok(None),
            Err(cancelled) => Err(cancelled),
        },
        Task::Continue => {
            let Ok(a) = roll::from_text(&format!("{}{}{}", given(&c.user), if given(&c.user).ends_with('\n') { "" } else { "\n" }, text)) else { return Ok(None) };
            let from = c.from_bar.saturating_sub(1);
            Ok(Some(prep::slice(&a, from, a.bar_count().max(from))))
        }
        _ => Ok(roll::from_text(text).ok()),
    }
}

// ------------------------------------------------------------------ metrics

pub const METRICS: [&str; 10] = ["parsed", "key", "meter", "bars", "density", "pitch", "rhythm", "notes_f1", "repeat", "form"];

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na < 1e-12 && nb < 1e-12 {
        1.0
    } else if na < 1e-12 || nb < 1e-12 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn key_score(found: &str, want: &str) -> f64 {
    if want.is_empty() {
        return 1.0;
    }
    let (Some((a, am)), Some((b, bm))) = (prep::parse_key(found), prep::parse_key(want)) else { return 0.0 };
    if a == b && am == bm {
        return 1.0;
    }
    // Relative major/minor, or a fifth either way in the same mode.
    let rel = am != bm && ((am && (a + 3) % 12 == b) || (bm && (b + 3) % 12 == a));
    let fifth = am == bm && ((a + 7) % 12 == b || (b + 7) % 12 == a);
    if rel || fifth {
        0.5
    } else {
        0.0
    }
}

fn onsets(a: &Arrangement) -> HashSet<(bool, i64, i32)> {
    a.parts.iter().flat_map(|p| p.notes.iter().map(move |n| (p.role == Role::Drums, (n.beat * STEPS).round() as i64, n.key.round() as i32))).collect()
}

fn rhythm_profile(a: &Arrangement) -> Vec<f64> {
    let steps = ((a.per_bar() * STEPS).round() as usize).clamp(1, 64);
    let mut h = vec![0f64; steps];
    for p in &a.parts {
        for n in &p.notes {
            let bar = a.bar_of(n.beat);
            let at = ((n.beat - a.bars[bar.min(a.bar_count().saturating_sub(1))]) * STEPS).round() as usize;
            h[at.min(steps - 1)] += 1.0;
        }
    }
    h
}

/// The share of bars that repeat an earlier bar exactly.
fn repeat_share(a: &Arrangement) -> f64 {
    let n = a.bar_count();
    if n == 0 {
        return 0.0;
    }
    let mut bars: Vec<Vec<(usize, i64, i32)>> = vec![Vec::new(); n];
    for (pi, p) in a.parts.iter().enumerate() {
        for x in &p.notes {
            let b = a.bar_of(x.beat);
            if b < n {
                bars[b].push((pi, ((x.beat - a.bars[b]) * STEPS).round() as i64, x.key.round() as i32));
            }
        }
    }
    let mut seen = HashSet::new();
    let mut rep = 0;
    for mut b in bars {
        b.sort();
        if b.is_empty() {
            continue;
        }
        if !seen.insert(b) {
            rep += 1;
        }
    }
    rep as f64 / n as f64
}

/// The arrangement as `midi_library`'s analysed song (for its form).
pub fn song_of(a: &Arrangement) -> analyse::Song {
    use makepad_midi_file::{Division, Format, Header, MidiFile, Track};
    let ppq = 480u32;
    let file = MidiFile { header: Header { format: Format::SingleTrack, track_count: 1, division: Division::TicksPerQuarter(ppq as u16), extra_data: Vec::new() }, tracks: vec![Track { events: Vec::new(), trailing_data: Vec::new() }], unknown_chunks: Vec::new() };
    let tick = |b: f64| (b.max(0.0) * ppq as f64).round() as u64;
    let parts = a
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| analyse::Part {
            track: i,
            channel: if p.role == Role::Drums { 9 } else { i as u8 % 9 },
            name: p.name.clone(),
            program: 0,
            changes: Vec::new(),
            drums: p.role == Role::Drums,
            notes: p.notes.iter().map(|n| analyse::Note { on: tick(n.beat), off: tick(n.beat + n.dur).max(tick(n.beat) + 1), key: n.key.round().clamp(0.0, 127.0) as u8, vel: (n.vel * 127.0).round().clamp(1.0, 127.0) as u8, expr: 1.0, bend: 0.0 }).collect(),
            pedal: Vec::new(),
            volume: 100,
            pan: 64,
            mod_moves: 0,
            bent: 0,
            role: p.role,
            melody: p.melody,
        })
        .collect();
    analyse::Song { ppq, tempo: file.tempo_map().expect("a tempo map of one empty track"), sigs: vec![(0, a.sig.0, a.sig.1)], key_sig: None, parts, end: tick(a.beats()) }
}

fn form_letters(a: &Arrangement) -> Vec<char> {
    let song = song_of(a);
    let measures = song.measures();
    form::sections(&song, &measures).iter().flat_map(|s| std::iter::repeat_n(s.letter, (s.to - s.from).div_ceil(4))).collect()
}

fn edit_similarity(a: &[char], b: &[char]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f64 / a.len().max(b.len()) as f64
}

fn ratio(a: f64, b: f64) -> f64 {
    if a <= 0.0 && b <= 0.0 {
        1.0
    } else if a <= 0.0 || b <= 0.0 {
        0.0
    } else {
        a.min(b) / a.max(b)
    }
}

/// The metrics of one answer.
pub fn score(c: &Case, output: &str, control: &PrepareControl) -> Result<[f64; 10], LoadError> {
    let Some(p) = read_answer(c, output, control)? else { return Ok([0.0; 10]) };
    let Some(r) = read_answer(c, &c.reference, control)? else { return Ok([0.0; 10]) };
    // The key of the music the answer completes (an infilled part with the
    // others).
    let key_of = |a: &Arrangement| {
        if c.task == Task::Infill {
            let mut all = roll::from_text(given(&c.user)).unwrap_or_else(|_| a.clone());
            all.parts.extend(a.parts.iter().cloned());
            prep::key_of(&all)
        } else {
            prep::key_of(a)
        }
    };
    // The key the reference's notes are in (for `describe`, the key asked
    // for: the window's own).
    let want_key = if c.task == Task::Describe && !c.key.is_empty() { c.key.clone() } else { key_of(&r) };
    let key = if c.role == "drums" { 1.0 } else { key_score(&key_of(&p), &want_key) };
    let per_bar = p.per_bar();
    let meter_ok = format!("{}/{}", p.sig.0, p.sig.1) == c.meter && p.bars.windows(2).all(|w| (w[1] - w[0] - per_bar).abs() < 1e-6 || r.bars.windows(2).any(|v| (v[1] - v[0] - (w[1] - w[0])).abs() < 1e-6));
    let bars = ratio(p.bar_count() as f64, r.bar_count() as f64);
    let count = |a: &Arrangement| a.parts.iter().map(|x| x.notes.len()).sum::<usize>() as f64;
    let density = ratio(count(&p), count(&r));
    let pitch = cosine(&prep::pc_histogram(&p, None), &prep::pc_histogram(&r, None));
    let rhythm = cosine(&rhythm_profile(&p), &rhythm_profile(&r));
    let (po, ro) = (onsets(&p), onsets(&r));
    let hit = po.intersection(&ro).count() as f64;
    let f1 = if po.is_empty() && ro.is_empty() { 1.0 } else { 2.0 * hit / (po.len() + ro.len()).max(1) as f64 };
    let repeat = 1.0 - (repeat_share(&p) - repeat_share(&r)).abs();
    let form = edit_similarity(&form_letters(&p), &form_letters(&r));
    Ok([1.0, key, meter_ok as u8 as f64, bars, density, pitch, rhythm, f1, repeat, form])
}

// ------------------------------------------------------------------ baselines

/// A chord's root and whether it is minor, from its name (`Am7`, `C/E`).
fn chord_root(name: &str) -> Option<(u8, bool)> {
    let b = name.as_bytes();
    let letter = match b.first()? {
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
        Some(b'#') => (1, &name[2..]),
        Some(b'b') => (-1, &name[2..]),
        _ => (0, &name[1..]),
    };
    let minor = rest.starts_with('m') && !rest.starts_with("maj");
    Some((((letter + acc + 12) % 12) as u8, minor))
}

/// A part playing a stock figure for its role over `chords` (one (root,
/// minor) per beat).
fn stock_part(name: &str, role: Role, melody: bool, chords: &[(u8, bool)], per_bar: f64) -> Part {
    let mut notes = Vec::new();
    let n = |beat: f64, key: f32, dur: f64, vel: f64| Note { beat, key, dur, vel, fx: None };
    for (i, &(root, minor)) in chords.iter().enumerate() {
        let beat = i as f64;
        let third = if minor { 3 } else { 4 };
        if role == Role::Drums {
            let in_bar = beat % per_bar;
            notes.push(n(beat, if (in_bar as usize) % 2 == 0 { 36.0 } else { 38.0 }, 0.25, 0.7));
            notes.push(n(beat, 42.0, 0.25, 0.7));
            notes.push(n(beat + 0.5, 42.0, 0.25, 0.7));
        } else if melody || role == Role::Lead {
            let k = [0, third, 7, third][i % 4];
            notes.push(n(beat, (72 + root as i32 + k) as f32, 1.0, 0.7));
        } else if role == Role::Bass {
            notes.push(n(beat, (36 + root as i32) as f32, 1.0, 0.7));
        } else {
            for k in [0, third, 7] {
                notes.push(n(beat, (60 + root as i32 + k) as f32, 1.0, 0.7));
            }
        }
    }
    let voice = if role == Role::Drums { Voice::GmKit(0) } else { Voice::Gm(0) };
    Part { name: name.into(), role, melody, voice, about: "baseline".into(), notes, pedal: Vec::new(), programs: Vec::new(), amp: arrangement::level(role, melody), pan: 0.0 }
}

fn empty_arrangement(sig: (u8, u32), bars: usize, bpm: f64, key: &str) -> Arrangement {
    let per_bar = sig.0 as f64 * 4.0 / sig.1 as f64;
    Arrangement {
        title: "Untitled".into(),
        artist: String::new(),
        year: None,
        genre: String::new(),
        key: key.into(),
        bpm,
        sig,
        bars: (0..=bars).map(|b| b as f64 * per_bar).collect(),
        parts: Vec::new(),
        sections: vec![arrangement::Section { name: "song".into(), kind: "song".into(), letter: 'A', from: 0, to: bars }],
        tempo: Vec::new(),
        notes: Vec::new(),
        rights: None,
        limiter_gain: 1.6,
        excerpt: false,
    }
}

/// The heuristic answer to a case.
pub fn heuristic(c: &Case) -> String {
    match c.task {
        Task::Describe => {
            let (num, den) = c.meter.split_once('/').and_then(|(a, b)| Some((a.parse::<u8>().ok()?, b.parse::<u32>().ok()?))).unwrap_or((4, 4));
            let (tonic, minor) = prep::parse_key(&c.key).unwrap_or((0, false));
            let mut a = empty_arrangement((num, den), c.bars.max(1), 120.0, &c.key);
            // I V vi IV (i VI III VII in minor), a chord a bar.
            let degrees: [(u8, bool); 4] = if minor { [(0, true), (8, false), (3, false), (10, false)] } else { [(0, false), (7, false), (9, true), (5, false)] };
            let per_bar = a.per_bar();
            let beats = (a.beats().round() as usize).max(1);
            let chords: Vec<(u8, bool)> = (0..beats).map(|b| {
                let d = degrees[(b as f64 / per_bar) as usize % 4];
                ((tonic + d.0) % 12, d.1)
            }).collect();
            // The parts the request names: `name (role[, the tune])`.
            let parts = c.user.split("Parts: ").nth(1).and_then(|r| r.split(". ").next()).unwrap_or("drums (drums), bass (bass), keys (keys)");
            for item in parts.split("), ") {
                let item = item.trim_end_matches(')');
                let Some((name, role)) = item.split_once(" (") else { continue };
                let melody = role.contains("the tune");
                let role = Role::parse(role.split(',').next().unwrap_or("keys").trim()).unwrap_or(Role::Keys);
                a.parts.push(stock_part(name.trim(), role, melody, &chords, per_bar));
            }
            tasks::roll_text(&a)
        }
        Task::Continue => {
            let Ok(g) = roll::from_text(given(&c.user)) else { return String::new() };
            let k = c.from_bar.saturating_sub(1);
            let m = c.bars;
            let mut a = g.clone();
            let per_bar = a.per_bar();
            a.bars = (0..=k + m).map(|b| b as f64 * per_bar).collect();
            let src_from = k.saturating_sub(m);
            let shift = (k - src_from) as f64 * per_bar;
            for p in &mut a.parts {
                let lo = src_from as f64 * per_bar;
                let hi = k as f64 * per_bar;
                let copy: Vec<Note> = p.notes.iter().filter(|n| n.beat >= lo - 1e-9 && n.beat < hi - 1e-9).map(|n| Note { beat: n.beat + shift, ..*n }).collect();
                p.notes.extend(copy);
            }
            tasks::split_header(&roll::to_text(&a, &Detail { bars: Some((k + 1, k + m)), ..Detail::default() })).1
        }
        Task::Infill => {
            let Ok(g) = roll::from_text(given(&c.user)) else { return String::new() };
            let per_bar = g.per_bar();
            let beats = (g.beats().round() as usize).max(1);
            let flats = g.flats();
            let chords: Vec<(u8, bool)> = (0..beats)
                .map(|b| {
                    let mut w = [0f64; 12];
                    let mut low: Option<u8> = None;
                    for p in g.parts.iter().filter(|p| p.role != Role::Drums) {
                        for n in p.notes.iter().filter(|n| n.beat < b as f64 + 1.0 && n.beat + n.dur > b as f64) {
                            let k = n.key.round().clamp(0.0, 127.0) as u8;
                            w[k as usize % 12] += 1.0;
                            low = Some(low.map_or(k, |l| l.min(k)));
                        }
                    }
                    chord_root(&roll::chord_name(&w, low.map(|l| l % 12), flats)).unwrap_or((0, false))
                })
                .collect();
            let role = match c.role.as_str() {
                "bass" => Role::Bass,
                "drums" => Role::Drums,
                "melody" => Role::Lead,
                _ => Role::Keys,
            };
            let mut a = g.clone();
            a.parts = vec![stock_part(&c.part, role, c.role == "melody", &chords, per_bar)];
            a.bars.truncate(c.bars + 1);
            roll::to_text(&a, &Detail { chords: false, ..Detail::default() })
        }
        // The input, unchanged: what a model that cannot convert gives.
        Task::RollToScore | Task::ScoreToRoll => given(&c.user).to_string(),
    }
}

/// Predictions for `cases` by a baseline (`heuristic` or `oracle`), at most
/// `limit` per task.
pub fn baseline(cases: &[Case], kind: &str, limit: usize) -> String {
    let mut per: BTreeMap<Task, usize> = BTreeMap::new();
    let mut out = String::new();
    for c in cases {
        let n = per.entry(c.task).or_default();
        if *n >= limit {
            continue;
        }
        *n += 1;
        let answer = if kind == "oracle" { c.reference.clone() } else { heuristic(c) };
        out.push_str(&obj(vec![("id", s(c.id.clone())), ("output", s(answer))]).to_json());
        out.push('\n');
    }
    out
}

/// Scores per task (means) and how many answers each has.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub per_task: BTreeMap<Task, (usize, [f64; 10])>,
    /// Predictions whose scoring or requested WAV could not finish.
    pub unfinished: Vec<UnfinishedPrediction>,
    pub missing: usize,
    pub unknown: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnfinishedPrediction {
    pub id: String,
    pub line: Option<u32>,
    pub reason: String,
}

fn render_answer(case: &Case, output: String, control: &PrepareControl) -> Result<Option<makepad_midi_library::cover::Rendered>, LoadError> {
    let source = if case.task == Task::RollToScore {
        output
    } else {
        let Some(arrangement) = read_answer(case, &output, control)? else { return Ok(None) };
        arrangement::to_score(&arrangement)
    };
    match makepad_midi_library::cover::render_checked(&source, -14.0, control) {
        Ok(rendered) => Ok(Some(rendered)),
        Err(makepad_midi_library::cover::RenderError::Load(error @ (LoadError::Unfinished { .. } | LoadError::Memory { .. } | LoadError::Cancelled))) => Err(error),
        Err(_) => Ok(None), // An ordinary unrenderable prediction keeps its metrics.
    }
}

/// Score the predictions in `pred` (JSONL) against `cases`; with `render`,
/// write the first `n` parsed answers as WAV through the Score engine.
pub fn evaluate(cases: &[Case], pred: &str, threads: usize, render: Option<(usize, &Path)>, control: &makepad_score_live::PrepareControl) -> Result<Report, String> {
    let control = &control.with_instruction_budget(crate::EXAMPLE_INSTRUCTION_BUDGET);
    let by_id: BTreeMap<&str, &Case> = cases.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut pairs: Vec<(&Case, String)> = Vec::new();
    let mut rep = Report::default();
    for line in pred.lines().filter(|l| !l.trim().is_empty()) {
        let v = makepad_strict_json::parse(line.as_bytes()).map_err(|e| format!("a prediction line: {e}"))?;
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
        let output = v.get("output").and_then(|x| x.as_str()).unwrap_or("").to_string();
        match by_id.get(id) {
            Some(c) => pairs.push((c, output)),
            None => rep.unknown += 1,
        }
    }
    let scores = crate::corpus::par_map(pairs.len(), threads, control, |i| score(pairs[i].0, &pairs[i].1, control))
        .map_err(|error| error.to_string())?;
    if let Some((_, dir)) = render { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    let mut rendered = 0;
    let answered: HashSet<&str> = pairs.iter().map(|p| p.0.id.as_str()).collect();
    rep.missing = cases.iter().filter(|c| !answered.contains(c.id.as_str())).count();
    for ((case, output), metrics) in pairs.into_iter().zip(scores) {
        let record_unfinished = |rep: &mut Report, error: LoadError, line| {
            eprintln!("{}: {error}", case.id);
            rep.unfinished.push(UnfinishedPrediction { id: case.id.clone(), line, reason: error.to_string() });
        };
        let metrics = match metrics.unwrap_or(Ok([0.0; 10])) {
            Ok(metrics) => metrics,
            Err(error @ (LoadError::Unfinished { line } | LoadError::Memory { line })) => {
                record_unfinished(&mut rep, error, line);
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        if let Some((_, dir)) = render.filter(|(n, _)| rendered < *n && metrics[0] >= 1.0) {
            let result = render_answer(case, output, control);
            if control.is_cancelled() { return Err("the evaluation was cancelled".into()); }
            match result {
                Ok(Some(r)) => {
                    let name: String = case.id.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect();
                    std::fs::write(dir.join(format!("{name}.wav")), makepad_midi_library::cover::wav(&r.song)).map_err(|e| e.to_string())?;
                    rendered += 1;
                }
                Ok(None) => {}
                Err(error @ (LoadError::Unfinished { line } | LoadError::Memory { line })) => {
                    record_unfinished(&mut rep, error, line);
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        let (count, sums) = rep.per_task.entry(case.task).or_default();
        *count += 1;
        for (sum, value) in sums.iter_mut().zip(metrics) { *sum += value; }
    }
    for (count, sums) in rep.per_task.values_mut() {
        for sum in sums { *sum /= *count as f64; }
    }
    Ok(rep)
}

pub fn report_text(rep: &Report) -> String {
    let mut w = format!("{:<14} {:>6}", "task", "n");
    for m in METRICS {
        w.push_str(&format!(" {m:>8}"));
    }
    w.push('\n');
    for (task, (n, m)) in &rep.per_task {
        w.push_str(&format!("{:<14} {n:>6}", task.name()));
        for v in m {
            w.push_str(&format!(" {v:>8.3}"));
        }
        w.push('\n');
    }
    for item in &rep.unfinished {
        w.push_str(&format!("{}: {}\n", item.id, item.reason));
    }
    if rep.missing + rep.unknown > 0 {
        w.push_str(&format!("({} cases without an answer, {} answers to no case)\n", rep.missing, rep.unknown));
    }
    w
}

pub fn report_json(rep: &Report) -> Value {
    let mut tasks: Vec<_> = rep.per_task.iter().map(|(t, (n, m))| (t.name().to_string(), obj(std::iter::once(("n", Value::Int(*n as i64))).chain(METRICS.iter().zip(m).map(|(k, v)| (*k, Value::F64((v * 1000.0).round() / 1000.0)))).collect()))).collect();
    if !rep.unfinished.is_empty() {
        let rows = rep.unfinished.iter().map(|item| obj(vec![
            ("id", s(item.id.clone())), ("line", item.line.map_or(Value::Null, |n| Value::Int(i64::from(n)))),
            ("reason", s(item.reason.clone())),
        ])).collect();
        tasks.push(("unfinished".into(), Value::Arr(rows)));
    }
    Value::Obj(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_forms_compare() {
        assert_eq!(key_score("A minor", "C major"), 0.5);
        assert_eq!(key_score("G major", "C major"), 0.5);
        assert_eq!(key_score("C major", "C major"), 1.0);
        assert_eq!(key_score("F# major", "C major"), 0.0);
        assert_eq!(chord_root("Bbm7"), Some((10, true)));
        assert_eq!(chord_root("Cmaj7"), Some((0, false)));
        assert!((edit_similarity(&['A', 'B', 'A'], &['A', 'B', 'B']) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn interrupted_scores_are_neither_bad_predictions_nor_rejected_examples() {
        let src = "use mod.score.*\nlet c = Clock{}\nlet s = Synth{preset: @sub}\nlet l = Loop{play: s beats: 4 notes: [[0 @a1 1 1]]}\nlet a = Section{bars: 1 loops: [l]}\nScore{arrange: [a] c s l a}";
        let case = Case {
            id: "one".into(), task: Task::RollToScore, user: String::new(), reference: src.into(),
            key: String::new(), meter: "4/4".into(), bars: 1, from_bar: 1, part: String::new(), role: String::new(),
        };
        let control = PrepareControl::default();
        let roll = tasks::roll_text(&roll::from_score_controlled(src, &control).unwrap());
        assert!(tasks::score_survives(src, &roll, &control).unwrap(), "a valid example survives its round trip");
        let bad = "use mod.score.*\n// Score{ is absent\n42";
        assert!(!tasks::score_survives(bad, &roll, &control).unwrap(), "an invalid score rejects the example");
        assert_eq!(score(&case, bad, &control).unwrap(), [0.0; 10], "an invalid prediction still scores zero");
        let cases = [case];
        let predictions = baseline(&cases, "oracle", 1);
        let report = evaluate(&cases, &predictions, 1, None, &control).unwrap();
        assert_eq!(report.per_task[&Task::RollToScore].1[0], 1.0, "the worker parses the oracle prediction");
        // Even one instruction per iteration would exhaust this budget before the score.
        let iterations = 512;
        let budgeted = control.with_instruction_budget(iterations);
        let slow = src.replace("let c =", &format!("let total = 0\nfor i in 0..{iterations} {{ total += i }}\nlet c ="));
        let slow_case = Case { id: "slow".into(), reference: slow.clone(), ..cases[0].clone() };
        // The index is exactly representable, but its ScriptValue array cannot have a valid layout.
        // This exercises the worker's physical refusal without exhausting the test machine.
        let index = isize::MAX as usize / std::mem::size_of::<u64>() + 1;
        let memory = src.replace("let c =", &format!("let xs = []\nxs[{index}] = 1\nlet c ="));
        let memory_line = memory.lines().position(|line| line.starts_with("xs[")).unwrap() as u32 + 1;
        let memory_case = Case { id: "memory".into(), reference: memory, ..cases[0].clone() };
        let batch = [slow_case, memory_case, cases[0].clone()];
        let answers = baseline(&batch, "oracle", batch.len());
        let first = evaluate(&batch, &answers, 1, None, &budgeted).unwrap();
        let second = evaluate(&batch, &answers, 1, None, &budgeted).unwrap();
        let loop_line = slow.lines().position(|line| line.starts_with("for ")).unwrap() as u32 + 1;
        let expected = [
            (loop_line, LoadError::Unfinished { line: Some(loop_line) }),
            (memory_line, LoadError::Memory { line: Some(memory_line) }),
        ];
        assert_eq!(first.unfinished.len(), expected.len(), "only the two refused predictions are recorded");
        let json = report_json(&first);
        let rows = json.get("unfinished").unwrap().as_arr().unwrap();
        let text = report_text(&first);
        for (i, (line, cause)) in expected.iter().enumerate() {
            let item = &first.unfinished[i];
            assert_eq!((&item.id, item.line, &item.reason), (&batch[i].id, Some(*line), &cause.to_string()),
                "each unfinished prediction keeps its own reason and line");
            assert_eq!(rows[i].get("reason").and_then(Value::as_str), Some(item.reason.as_str()), "JSON preserves the refusal");
            assert!(text.contains(&format!("{}: {}", item.id, item.reason)), "text names the refused prediction and its cause");
        }
        assert_eq!(first.per_task, report.per_task, "the following prediction gets its full budget and the same metrics as alone");
        assert_eq!((first.unfinished, first.per_task), (second.unfinished, second.per_task), "repeating the batch gives the same split");
        assert!(matches!(makepad_score_live::load_controlled(bad, &budgeted), Err(LoadError::Problems(_))), "an invalid document still reports problems");

        // Roll metrics need no script instructions. Only the requested WAV evaluates a score.
        let roll_case = Case { task: Task::Describe, reference: roll.clone(), ..cases[0].clone() };
        let roll_cases = [roll_case];
        let answer = baseline(&roll_cases, "oracle", 1);
        let render_budget = control.with_instruction_budget(0);
        let metrics = evaluate(&roll_cases, &answer, 1, None, &render_budget).unwrap();
        assert_eq!(metrics.per_task[&Task::Describe].0, roll_cases.len(), "scoring the roll alone finishes");
        let dir = std::env::temp_dir().join(format!("music_corpus_unfinished_render_{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let with_wav = evaluate(&roll_cases, &answer, 1, Some((1, &dir)), &render_budget).unwrap();
        assert!(matches!(with_wav.unfinished.as_slice(), [item] if item.id == roll_cases[0].id), "a budget hit during the cover records the item");
        assert!(with_wav.per_task.is_empty() && std::fs::read_dir(&dir).unwrap().next().is_none(), "an unfinished render publishes neither a completed row nor a WAV");
        std::fs::remove_dir(dir).unwrap();

        control.cancel();
        assert!(matches!(makepad_score_live::load_controlled(&slow, &budgeted), Err(LoadError::Cancelled)), "the budgeted view shares Stop and cancellation takes precedence");
        assert!(matches!(tasks::score_survives(src, &roll, &control), Err(LoadError::Cancelled)), "an interrupted example has no verdict");
        assert!(matches!(score(&cases[0], src, &control), Err(LoadError::Cancelled)), "an interrupted prediction has no metrics");
        assert_eq!(evaluate(&cases, &predictions, 1, None, &control).err().as_deref(), Some("the request was cancelled"));
    }
}
