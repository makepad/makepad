//! The tasks a song's windows become, as chat turns: a system prompt that
//! teaches the two text forms, the request, and the answer. Every answer
//! is checked by reading it back the way Stage reads it (the roll by
//! `roll::from_text` and written again to the same text; a Score loaded by
//! the engine and played back to the same notes) and dropped if it does
//! not survive.
//!
//! - `describe`: style, tempo, key, meter, parts (and sometimes the form)
//!   -> a roll of N bars.
//! - `continue`: K bars of a song -> its next M bars.
//! - `infill`: a song without one part (bass, drums, melody or chords) ->
//!   that part.
//! - `roll_to_score`: a roll -> the Score the mechanical import writes for
//!   it, so the model learns to write Score.
//! - `score_to_roll`: a Score -> its roll.

use crate::prep::{self, Split};
use crate::source::SongRef;
use crate::tokens::{chat_text, Tokenizer};
use makepad_midi_library::arrangement::{self, Arrangement};
use makepad_midi_library::gm::Role;
use makepad_midi_library::roll::{self, Detail};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Task {
    Describe,
    Continue,
    Infill,
    RollToScore,
    ScoreToRoll,
}

impl Task {
    pub const ALL: [Task; 5] = [Task::Describe, Task::Continue, Task::Infill, Task::RollToScore, Task::ScoreToRoll];

    pub fn name(self) -> &'static str {
        match self {
            Task::Describe => "describe",
            Task::Continue => "continue",
            Task::Infill => "infill",
            Task::RollToScore => "roll_to_score",
            Task::ScoreToRoll => "score_to_roll",
        }
    }

    pub fn parse(s: &str) -> Option<Task> {
        Task::ALL.into_iter().find(|t| t.name() == s)
    }

    /// Whether the answer is a roll (else a Score).
    pub fn writes_roll(self) -> bool {
        self != Task::RollToScore
    }
}

/// The system prompt of every example: the two forms, briefly.
pub const SYSTEM: &str = "You are a composer who writes music as text in Makepad's two music forms.\n\
The roll: a header (`# title`, `tempo <bpm> | meter <n/d> | key <key> | grid <steps a bar>`, `form <section letter bars>, ...`, one `part <name> <role>[*] <voice> amp= pan=` line per part, `*` marks the tune), then the bars. `== <section> [L] bars a-b` starts a section. A bar line is `<n> | <chord a beat, . continues> |`, then one line per part that plays in it. The grid is four steps a beat. A pitched part has a token a step: a note (`A4`, `C#5`, `Bb3`) or a chord (`A3+C#4+E4`) starts there, `~` holds, `.` is silence; `!` after a note is loud, `?` soft. Drums have a row per drum (`K` kick, `S` snare, `H` closed hat, `O` open hat, `CR` crash, `RD` ride, `T1`-`T3` toms ...), a beat of four steps at a time, joined by ` | `: `x` hit, `X` accent, `o` ghost. `7-8 = 5-6` says bars 7-8 repeat bars 5-6.\n\
The Score: Splash code (`use mod.score.*`, a Clock, an instrument and a note list `[beat note dur vel]` per part played by a Loop, the mix and master chain, the Sections and `Score{...}`).\n\
Answer with the requested text only.";

/// How many examples of each task a song gives, and their sizes.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Tokens an example may take (prompt and answer).
    pub context: usize,
    pub describe: usize,
    pub cont: usize,
    /// Parts infilled per song (bass, drums, melody, chords in turn).
    pub infill: usize,
    /// Windows per song for each Score task.
    pub score: usize,
    /// Transposed and tempo-relabelled copies per train example.
    pub augment: usize,
    pub describe_bars: Vec<usize>,
    /// (K, M): bars given, bars written.
    pub cont_bars: Vec<(usize, usize)>,
    pub infill_bars: Vec<usize>,
    pub score_bars: Vec<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { context: 8192, describe: 2, cont: 2, infill: 2, score: 1, augment: 1, describe_bars: vec![32, 16, 8], cont_bars: vec![(16, 8), (8, 4), (4, 2)], infill_bars: vec![16, 8, 4], score_bars: vec![8, 4, 2] }
    }
}

/// One example, ready to write.
#[derive(Clone, Debug)]
pub struct Example {
    pub task: Task,
    pub id: String,
    pub user: String,
    pub assistant: String,
    /// Bars of the song, 1-based inclusive.
    pub bars: (usize, usize),
    pub shift: i32,
    pub tempo_scale: f64,
    /// What the eval checks the answer against.
    pub want: Want,
    pub tokens: usize,
    pub target_tokens: usize,
}

/// The request as the eval reads it.
#[derive(Clone, Debug, Default)]
pub struct Want {
    pub key: String,
    pub meter: String,
    /// Bars the answer should hold.
    pub bars: usize,
    /// For `continue`: the bar the answer starts at.
    pub from_bar: usize,
    /// For `infill`: the part.
    pub part: String,
    pub role: String,
}

/// What became of a window that did not make an example.
pub type Drop = (&'static str, Task);

pub struct Built {
    pub examples: Vec<Example>,
    pub dropped: Vec<Drop>,
}

/// The song without who it is: the model learns music, not names.
pub fn anonymise(a: &Arrangement, song: &SongRef) -> Arrangement {
    let mut out = a.clone();
    out.title = "Untitled".into();
    out.artist = String::new();
    out.year = None;
    out.genre = song.genres.first().cloned().unwrap_or_default();
    out.notes = Vec::new();
    out.rights = None;
    out
}

pub fn roll_text(a: &Arrangement) -> String {
    roll::to_text(a, &Detail::default())
}

/// The header lines of a roll and the rest.
pub fn split_header(text: &str) -> (String, String) {
    let mut head = String::new();
    let mut body = String::new();
    let mut in_head = true;
    for line in text.lines() {
        if in_head && (line.starts_with("# ") || line.starts_with("tempo ") || line.starts_with("form ") || line.starts_with("part ")) {
            head.push_str(line);
            head.push('\n');
        } else {
            in_head = false;
            body.push_str(line);
            body.push('\n');
        }
    }
    (head, body)
}

/// A window as its roll says it: written and read back once, so its notes
/// sit on the grid at the roll's three levels and its chord line is the one
/// the roll's own notes give. Every answer is made from this.
pub fn canon(w: &Arrangement) -> Result<Arrangement, &'static str> {
    let back = roll::from_text(&roll::to_text(w, &Detail::default())).map_err(|_| "roundtrip_roll")?;
    let mut back = back;
    back.genre = w.genre.clone();
    // The window's own key (a passage can sit in another key than the song).
    let key = prep::key_of(&back);
    if !key.is_empty() && key != back.key {
        back.key = key;
        back = roll::from_text(&roll::to_text(&back, &Detail::default())).map_err(|_| "roundtrip_roll")?;
        back.genre = w.genre.clone();
    }
    Ok(back)
}

/// A roll that reads back to itself.
pub fn roll_survives(text: &str, d: &Detail) -> bool {
    roll::from_text(text).is_ok_and(|b| roll::to_text(&b, d) == text)
}

/// Each part's notes on the grid: (part, step, key, steps held).
fn grid_notes(a: &Arrangement) -> Vec<(String, i64, i32, i64)> {
    let mut v: Vec<(String, i64, i32, i64)> = a
        .parts
        .iter()
        .flat_map(|p| {
            let drums = p.role == Role::Drums;
            p.notes.iter().map(move |n| (p.name.clone(), (n.beat * roll::STEPS).round() as i64, n.key.round() as i32, if drums { 0 } else { (n.dur * roll::STEPS).round() as i64 }))
        })
        .collect();
    v.sort();
    v
}

/// A Score that loads, and plays the notes of `roll_text` at their steps
/// and lengths (played back by the engine's composer).
pub fn score_survives(score: &str, roll_text: &str) -> bool {
    let (Ok(read), Ok(want)) = (roll::from_score(score), roll::from_text(roll_text)) else { return false };
    let (a, b) = (grid_notes(&read), grid_notes(&want));
    if a != b && std::env::var_os("MUSIC_CORPUS_DEBUG").is_some() {
        let i = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
        eprintln!("score differs at note {i} ({} vs {} notes): played {:?}, roll {:?}", a.len(), b.len(), a.get(i), b.get(i));
    }
    a == b
}

fn meter(a: &Arrangement) -> String {
    format!("{}/{}", a.sig.0, a.sig.1)
}

fn parts_text(a: &Arrangement) -> String {
    a.parts.iter().map(|p| if p.melody { format!("{} ({}, the tune)", p.name, p.role.name()) } else { format!("{} ({})", p.name, p.role.name()) }).collect::<Vec<_>>().join(", ")
}

fn form_text(a: &Arrangement) -> String {
    a.sections.iter().map(|s| format!("{} {}{}", s.name, s.letter, s.to - s.from)).collect::<Vec<_>>().join(", ")
}

/// Tags worth a prompt: the strongest few, not the artist's name.
pub fn style_tags(song: &SongRef) -> Vec<String> {
    let artist = prep::norm_name(&song.artist).replace(' ', "");
    song.tags.iter().filter(|(t, _)| artist.is_empty() || !(t.contains(&artist) || artist.contains(t.as_str()))).filter(|(t, _)| !matches!(t.as_str(), "favorites" | "favourite" | "favorite" | "seenlive" | "awesome" | "love" | "beautiful" | "lovedtracks")).take(6).map(|(t, _)| t.clone()).collect()
}

fn style_text(song: &SongRef, a: &Arrangement) -> String {
    let mut bits = Vec::new();
    if !a.genre.is_empty() {
        bits.push(a.genre.clone());
    }
    if let Some(d) = song.decade() {
        bits.push(format!("{d}s"));
    }
    let tags = style_tags(song);
    if !tags.is_empty() {
        bits.push(format!("tags: {}", tags.join(", ")));
    }
    if bits.is_empty() {
        "any".into()
    } else {
        bits.join("; ")
    }
}

/// The tempo, written one of three ways (the label augmentation).
fn tempo_text(bpm: f64, salt: u64) -> String {
    let n = arrangement::num(bpm, 1);
    match salt % 3 {
        0 => format!("{n} bpm"),
        1 => format!("a {} tempo", prep::tempo_label(bpm)),
        _ => format!("{n} bpm ({})", prep::tempo_label(bpm)),
    }
}

/// A window moved and relabelled for a train copy.
fn augmented(w: &Arrangement, salt: u64, copy: usize) -> Option<(Arrangement, i32, f64)> {
    let shifts = prep::shifts(w);
    if shifts.is_empty() {
        return None;
    }
    let h = prep::hash_str(&["aug", &salt.to_string(), &copy.to_string()]);
    let shift = shifts[(h % shifts.len() as u64) as usize];
    let scale = [0.92, 1.0, 1.08][((h >> 16) % 3) as usize];
    let mut a = prep::transpose(w, shift);
    if scale != 1.0 {
        a.bpm = (a.bpm * scale * 100.0).round() / 100.0;
        for t in &mut a.tempo {
            t.1 = (t.1 * scale * 100.0).round() / 100.0;
        }
    }
    Some((a, shift, scale))
}

struct Ctx<'a> {
    song: &'a SongRef,
    tok: &'a Tokenizer,
    lim: &'a Limits,
    /// Tokens of the system prompt and the chat layout.
    overhead: usize,
}

impl Ctx<'_> {
    fn fit(&self, user: &str, assistant: &str) -> Option<(usize, usize)> {
        let target = self.tok.count(assistant) + 1;
        if target + self.overhead > self.lim.context {
            return None;
        }
        let total = self.overhead + self.tok.count(user) + target;
        (total <= self.lim.context).then_some((total, target))
    }
}

/// The overhead of the chat layout around the user and assistant text.
pub fn overhead(tok: &Tokenizer) -> usize {
    tok.count(&chat_text(SYSTEM, "", ""))
}

/// A part to infill for a role, if the window has one.
fn infill_part(w: &Arrangement, role: &str) -> Option<usize> {
    w.parts.iter().position(|p| match role {
        "bass" => p.role == Role::Bass && !p.melody,
        "drums" => p.role == Role::Drums,
        "melody" => p.melody,
        _ => !p.melody && matches!(p.role, Role::Keys | Role::Guitar | Role::Pads | Role::Strings),
    })
}

const INFILL_ROLES: [&str; 4] = ["bass", "drums", "melody", "chords"];

/// Every example of a song (`a` as imported).
pub fn build(a: &Arrangement, song: &SongRef, split: Split, lim: &Limits, tok: &Tokenizer, overhead: usize) -> Built {
    let a = anonymise(a, song);
    let cx = Ctx { song, tok, lim, overhead };
    let mut out = Built { examples: Vec::new(), dropped: Vec::new() };
    let copies = if split == Split::Train { lim.augment } else { 0 };
    // Each base window (task, from, len, extra) is written plain and as its
    // augmented copies.
    let mut emit = |task: Task, from: usize, make: &dyn Fn(&Arrangement, u64) -> Result<Option<(String, String, Want, usize)>, &'static str>| {
        let salt = prep::hash_str(&[&song.id, task.name(), &from.to_string()]);
        for copy in 0..=copies {
            let (w, shift, scale) = if copy == 0 {
                (a.clone(), 0, 1.0)
            } else {
                match augmented(&a, salt, copy) {
                    Some(x) => x,
                    None => continue,
                }
            };
            match make(&w, salt.wrapping_add(copy as u64)) {
                Err(reason) => {
                    out.dropped.push((reason, task));
                    break;
                }
                Ok(None) => {
                    out.dropped.push(("over_context", task));
                    break;
                }
                Ok(Some((user, assistant, want, len))) => {
                    let Some((tokens, target_tokens)) = cx.fit(&user, &assistant) else {
                        out.dropped.push(("over_context", task));
                        break;
                    };
                    let aug = if copy == 0 { String::new() } else { format!(":t{shift:+}x{scale}") };
                    let part = if want.part.is_empty() { String::new() } else { format!(":{}", want.part) };
                    out.examples.push(Example { task, id: format!("{}:{}:{}-{}{part}{aug}", song.id, task.name(), from + 1, from + len), user, assistant, bars: (from + 1, from + len), shift, tempo_scale: scale, want, tokens, target_tokens });
                }
            }
        }
    };

    // describe -> write.
    for &len in &lim.describe_bars {
        let starts = prep::windows(&a, len, lim.describe, "describe");
        if starts.is_empty() {
            continue;
        }
        for from in starts {
            let make = |full: &Arrangement, salt: u64| -> Result<Option<(String, String, Want, usize)>, &'static str> {
                // The largest size that fits, from this start.
                for &n in lim.describe_bars.iter().filter(|n| **n <= len) {
                    let w = canon(&prep::slice(full, from, from + n))?;
                    let text = roll_text(&w);
                    if !roll_survives(&text, &Detail::default()) {
                        return Err("roundtrip_roll");
                    }
                    let form = if salt % 2 == 0 { format!(" Form: {}.", form_text(&w)) } else { String::new() };
                    let user = format!(
                        "Write {n} bars in {} at {}, in {}. Style: {}. Parts: {}.{form} Answer with the roll only.",
                        meter(&w),
                        tempo_text(w.bpm, salt >> 8),
                        if w.key.is_empty() { "no fixed key".to_string() } else { w.key.clone() },
                        style_text(cx.song, &w),
                        parts_text(&w)
                    );
                    if cx.fit(&user, &text).is_some() {
                        return Ok(Some((user, text, Want { key: w.key.clone(), meter: meter(&w), bars: n, ..Want::default() }, n)));
                    }
                }
                Ok(None)
            };
            emit(Task::Describe, from, &make);
        }
        break;
    }

    // continue: K bars -> the next M.
    for &(k, m) in &lim.cont_bars {
        let starts = prep::windows(&a, k + m, lim.cont, "continue");
        if starts.is_empty() {
            continue;
        }
        for from in starts {
            let make = |full: &Arrangement, _salt: u64| -> Result<Option<(String, String, Want, usize)>, &'static str> {
                for &(k, m) in lim.cont_bars.iter().filter(|(kk, mm)| kk + mm <= k + m) {
                    let w = canon(&prep::slice(full, from, from + k + m))?;
                    let given = roll::to_text(&w, &Detail { bars: Some((1, k)), ..Detail::default() });
                    let (_, next) = split_header(&roll::to_text(&w, &Detail { bars: Some((k + 1, k + m)), ..Detail::default() }));
                    if next.trim().is_empty() {
                        return Err("empty_target");
                    }
                    // The given bars and the answer read back as the window.
                    let back = roll::from_text(&format!("{given}{next}")).map_err(|_| "roundtrip_roll")?;
                    let (_, again) = split_header(&roll::to_text(&back, &Detail { bars: Some((k + 1, k + m)), ..Detail::default() }));
                    if again != next {
                        return Err("roundtrip_roll");
                    }
                    let user = format!("Continue this song with bars {}-{} ({m} bars) in the same style. Answer with those bars only, numbered on.\n\n{given}", k + 1, k + m);
                    if cx.fit(&user, &next).is_some() {
                        return Ok(Some((user, next, Want { key: w.key.clone(), meter: meter(&w), bars: m, from_bar: k + 1, ..Want::default() }, k + m)));
                    }
                }
                Ok(None)
            };
            emit(Task::Continue, from, &make);
        }
        break;
    }

    // infill: a part from the others.
    let mut infilled = 0;
    for (ri, role) in INFILL_ROLES.iter().enumerate() {
        if infilled >= lim.infill {
            break;
        }
        let len = lim.infill_bars.iter().copied().find(|n| *n <= a.bar_count());
        let Some(len) = len else { break };
        let salt = format!("infill{ri}");
        // A window where the part plays in at least half the bars.
        let from = prep::windows(&a, len, 8, &salt).into_iter().find(|&f| {
            let w = prep::slice(&a, f, f + len);
            infill_part(&w, role).is_some_and(|pi| {
                let p = &w.parts[pi];
                let mut bars: Vec<usize> = p.notes.iter().map(|n| w.bar_of(n.beat)).collect();
                bars.dedup();
                bars.len() * 2 >= len && w.parts.len() >= 2
            })
        });
        let Some(from) = from else { continue };
        infilled += 1;
        let make = |full: &Arrangement, _salt: u64| -> Result<Option<(String, String, Want, usize)>, &'static str> {
            for &n in lim.infill_bars.iter().filter(|n| **n <= len) {
                let w = canon(&prep::slice(full, from, from + n))?;
                let Some(pi) = infill_part(&w, role) else { return Err("empty_target") };
                let p = &w.parts[pi];
                let mut only = w.clone();
                only.parts = vec![p.clone()];
                let mut rest = w.clone();
                rest.parts.remove(pi);
                if rest.parts.is_empty() {
                    return Err("empty_target");
                }
                let d = Detail { chords: false, ..Detail::default() };
                let target = roll::to_text(&only, &d);
                if !roll_survives(&target, &d) {
                    return Err("roundtrip_roll");
                }
                let given = roll_text(&rest);
                let voice = target.lines().find(|l| l.starts_with("part ")).and_then(|l| l.split_whitespace().nth(3)).unwrap_or("").to_string();
                let user = format!("This song has no {role} part. Write the part `{}` ({}{}, {voice}) for its {n} bars. Answer with the roll of that part only.\n\n{given}", p.name, p.role.name(), if p.melody { ", the tune" } else { "" });
                if cx.fit(&user, &target).is_some() {
                    return Ok(Some((user, target, Want { key: w.key.clone(), meter: meter(&w), bars: n, part: p.name.clone(), role: role.to_string(), ..Want::default() }, n)));
                }
            }
            Ok(None)
        };
        emit(Task::Infill, from, &make);
    }

    // The Score tasks: two different windows.
    let len = lim.score_bars.iter().copied().find(|n| *n <= a.bar_count());
    if let Some(len) = len {
        let starts = prep::windows(&a, len, lim.score * 2, "score");
        for (i, from) in starts.into_iter().enumerate() {
            let task = if i % 2 == 0 { Task::RollToScore } else { Task::ScoreToRoll };
            let make = |full: &Arrangement, _salt: u64| -> Result<Option<(String, String, Want, usize)>, &'static str> {
                for &n in lim.score_bars.iter().filter(|n| **n <= len) {
                    let w = canon(&prep::slice(full, from, from + n))?;
                    let text = roll_text(&w);
                    let back = roll::from_text(&text).map_err(|_| "roundtrip_roll")?;
                    if roll::to_text(&back, &Detail::default()) != text {
                        return Err("roundtrip_roll");
                    }
                    let score = arrangement::to_score(&back);
                    if !score_survives(&score, &text) {
                        return Err("roundtrip_score");
                    }
                    let want = Want { key: w.key.clone(), meter: meter(&w), bars: n, ..Want::default() };
                    let (user, answer) = match task {
                        Task::RollToScore => (format!("Write this roll as a Score.\n\n{text}"), score),
                        _ => (format!("Transcribe this Score as a roll.\n\n{score}"), text),
                    };
                    if cx.fit(&user, &answer).is_some() {
                        return Ok(Some((user, answer, want, n)));
                    }
                }
                Ok(None)
            };
            emit(task, from, &make);
        }
    }
    out
}
