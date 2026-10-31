//! A batch: the songs one Generate wrote for a request, their ratings and
//! comments, and the rules learned from them. The songs are written and
//! rendered on their own threads at once; [`Run::poll`] hands the host each
//! song as it moves on, so the first can be heard while the rest arrive.
//!
//! On disk (`~/makepad/local/music-iter/batches/bNNN/`, `MUSIC_ITER_DIR`
//! moves the root): `request.txt`; per song `sK.splash`, `sK.wav`,
//! `sK.card.tsv`, `sK.meta` (title, idea); `ratings.tsv`; `rules.md` (the
//! rules before and after learning, and what changed). `log.md` at the
//! root keeps every batch's songs, ratings and rule changes in order.

use crate::card::{self, Card};
use crate::gen::{self, Progress, Request, SharedWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// What the person rates, each optionally up or down.
pub const ASPECTS: [&str; 7] = ["structure", "instruments", "melody", "drums", "bass", "harmony", "mix"];

pub fn root() -> PathBuf {
    match std::env::var_os("MUSIC_ITER_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("makepad/local/music-iter"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// The director is planning the batch's concepts.
    Waiting,
    Writing { turn: usize },
    Rendering,
    /// Revising after a producer's notes (the first version plays already
    /// on disk).
    Revising,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub state: State,
    pub title: String,
    pub idea: String,
    pub source: String,
    pub card: Option<Card>,
    pub seconds: f64,
    /// Per aspect of [`ASPECTS`]: None (not rated), up, down.
    pub ratings: [Option<bool>; 7],
    pub feedback: String,
    /// The director's concept card (empty when it wrote without one).
    pub concept: String,
}

impl Default for Entry {
    fn default() -> Self {
        Entry { state: State::Waiting, title: String::new(), idea: String::new(), source: String::new(), card: None, seconds: 0.0, ratings: [None; 7], feedback: String::new(), concept: String::new() }
    }
}

impl Entry {
    pub fn rated(&self) -> bool {
        self.ratings.iter().any(Option::is_some) || !self.feedback.trim().is_empty()
    }
}

pub struct Batch {
    pub id: u32,
    pub dir: PathBuf,
    pub request: Request,
    pub songs: Vec<Entry>,
}

/// A song moved on.
pub enum Update {
    State(usize, State),
    /// The director's card for a song.
    Concept(usize, gen::Concept),
    Ready { index: usize, title: String, idea: String, source: String, card: Card, seconds: f64 },
}

pub struct Run {
    rx: Receiver<Update>,
    pub cancel: Arc<AtomicBool>,
    pending: usize,
}

impl Run {
    /// Apply what arrived to `batch`; true when anything changed.
    pub fn poll(&mut self, batch: &mut Batch) -> bool {
        let mut changed = false;
        while let Ok(update) = self.rx.try_recv() {
            changed = true;
            match update {
                Update::Concept(i, c) => {
                    let e = &mut batch.songs[i];
                    (e.title, e.concept) = (c.title, c.card);
                }
                Update::State(i, state) => {
                    if matches!(state, State::Failed(_)) {
                        self.pending -= 1;
                    }
                    batch.songs[i].state = state;
                }
                Update::Ready { index, title, idea, source, card, seconds } => {
                    self.pending -= 1;
                    let e = &mut batch.songs[index];
                    (e.title, e.idea, e.source, e.card, e.seconds, e.state) = (title, idea, source, Some(card), seconds, State::Ready);
                }
            }
        }
        changed
    }

    pub fn done(&self) -> bool {
        self.pending == 0
    }

    pub fn stop(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn next_id(root: &Path) -> u32 {
    std::fs::read_dir(root.join("batches")).map_or(0, |dir| dir.filter_map(|e| e.ok()?.file_name().to_str()?.strip_prefix('b')?.parse::<u32>().ok()).max().unwrap_or(0)) + 1
}

/// One composer: write song `k` from its card, render, score and keep it.
#[allow(clippy::too_many_arguments)]
fn compose(k: usize, n: usize, concept: Option<gen::Concept>, writer: &dyn gen::Writer, system: &str, request: &Request, recent: &str, cancel: &AtomicBool, dir: &Path, tx: &std::sync::mpsc::Sender<Update>) {
    let progress = |p: Progress| {
        let _ = tx.send(Update::State(k, match p {
            Progress::Writing { turn } => State::Writing { turn },
            Progress::Revising => State::Revising,
            Progress::Rendering => State::Rendering,
        }));
    };
    match gen::write_song(writer, system, request, k, n, recent, concept.as_ref(), cancel, &progress) {
        Ok(w) => {
            let card = card::score(&w.song, &card::Opts::default());
            // One producer's pass: the weak lines and the form, as notes for
            // a revision, kept only when it scores at least as well.
            let (w, card, revised) = match gen::critique(&w.song, &card) {
                Some(notes) if !cancel.load(std::sync::atomic::Ordering::Relaxed) => {
                    progress(Progress::Revising);
                    let mut turns = w.history.clone();
                    turns.push(notes.clone());
                    let _ = std::fs::write(dir.join(format!("s{}.notes", k + 1)), &notes);
                    match gen::write_from(writer, system, request, turns, cancel, &|_| {}) {
                        Ok(r) => {
                            let rc = card::score(&r.song, &card::Opts::default());
                            if rc.total() >= card.total() {
                                {
                                    let note = format!("revised: card {:.0} -> {:.0}", card.total(), rc.total());
                                    (r, rc, note)
                                }
                            } else {
                                (w, card.clone(), format!("revision kept out: card {:.0} -> {:.0}", card.total(), rc.total()))
                            }
                        }
                        Err(e) => (w, card, format!("revision failed: {e}")),
                    }
                }
                _ => (w, card, "no revision needed".to_string()),
            };
            let name = format!("s{}", k + 1);
            let _ = std::fs::write(dir.join(format!("{name}.splash")), &w.source);
            let _ = std::fs::write(dir.join(format!("{name}.wav")), crate::render::wav(w.song.mix.rate, &w.song.mix.left, &w.song.mix.right));
            let _ = std::fs::write(dir.join(format!("{name}.card.tsv")), card.to_tsv());
            let _ = std::fs::write(dir.join(format!("{name}.meta")), format!("{}\n{}\nturns {}\n{revised}\n", w.title, w.idea, w.turns));
            let _ = tx.send(Update::Ready { index: k, title: w.title, idea: w.idea, source: w.source, card, seconds: w.song.mix.seconds() });
        }
        Err(e) => {
            let _ = std::fs::write(dir.join(format!("s{}.failed", k + 1)), &e);
            let _ = tx.send(Update::State(k, State::Failed(e)));
        }
    }
}

impl Batch {
    /// A new batch of `n` songs, written and rendered at once.
    pub fn start(writer: SharedWriter, request: Request, n: usize) -> Result<(Batch, Run), String> {
        Batch::start_with(writer, request, n, None)
    }

    /// A new batch written from `cards` (no director): the same concepts
    /// again, e.g. to compare composers or patches before and after.
    pub fn start_with(writer: SharedWriter, request: Request, n: usize, cards: Option<Vec<gen::Concept>>) -> Result<(Batch, Run), String> {
        let n = cards.as_ref().map_or(n, Vec::len);
        let root = root();
        let id = next_id(&root);
        let dir = root.join("batches").join(format!("b{id:03}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(dir.join("request.txt"), format!("{}model: {}\n", request.text(), writer.label())).map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let system = Arc::new(gen::system());
        let recent = Arc::new(recent_verdicts(&root));
        let history = history(&root, 3);
        // The director plans five different songs, then five composers
        // write them at once, each from its own card.
        let director = (tx.clone(), writer.clone(), cancel.clone(), request.clone(), dir.clone());
        std::thread::spawn(move || {
            let (tx, writer, cancel, request, dir) = director;
            let planned = match cards {
                Some(cards) => Ok(cards),
                None => gen::direct(&*writer, &request, n, &history, &cancel),
            };
            let concepts = match planned {
                Ok(cards) => {
                    let mut md = format!("# b{id:03} concepts\n");
                    for (k, c) in cards.iter().enumerate() {
                        md.push_str(&format!("\n## s{}\n{}\n", k + 1, c.card));
                        let _ = std::fs::write(dir.join(format!("s{}.concept", k + 1)), &c.card);
                        let _ = tx.send(Update::Concept(k, c.clone()));
                    }
                    let _ = std::fs::write(dir.join("concepts.md"), md);
                    cards.into_iter().map(Some).collect()
                }
                // Without a plan the composers still write, told to differ.
                Err(e) => {
                    let _ = std::fs::write(dir.join("concepts.failed"), &e);
                    vec![None; n]
                }
            };
            for (k, concept) in concepts.into_iter().enumerate() {
                let (tx, writer, cancel, system, recent, request, dir) = (tx.clone(), writer.clone(), cancel.clone(), system.clone(), recent.clone(), request.clone(), dir.clone());
                std::thread::spawn(move || compose(k, n, concept, &*writer, &system, &request, &recent, &cancel, &dir, &tx));
            }
        });
        let songs = vec![Entry::default(); n];
        Ok((Batch { id, dir, request, songs }, Run { rx, cancel, pending: n }))
    }

    /// Reopen a batch from disk (its finished songs and ratings).
    pub fn open(dir: &Path) -> Option<Batch> {
        let id = dir.file_name()?.to_str()?.strip_prefix('b')?.parse().ok()?;
        let request_text = std::fs::read_to_string(dir.join("request.txt")).ok()?;
        let field = |key: &str| request_text.lines().find_map(|l| l.strip_prefix(key).map(|v| v.trim().to_string()));
        let request = Request {
            prompt: field("Brief:").unwrap_or_default(),
            markers: field("Style:").map(|s| s.split(", ").map(String::from).collect()).unwrap_or_default(),
            seconds: field("Length: about").and_then(|v| v.split(' ').next()?.parse().ok()).unwrap_or(30),
        };
        let mut songs = Vec::new();
        for k in 1.. {
            let name = format!("s{k}");
            let source = std::fs::read_to_string(dir.join(format!("{name}.splash")));
            let failed = std::fs::read_to_string(dir.join(format!("{name}.failed")));
            // A song with only its card was still being written (or the
            // app closed first).
            let planned = dir.join(format!("{name}.concept")).exists();
            if source.is_err() && failed.is_err() && !planned {
                break;
            }
            let meta = std::fs::read_to_string(dir.join(format!("{name}.meta"))).unwrap_or_default();
            let mut lines = meta.lines();
            let mut e = Entry { title: lines.next().unwrap_or("").into(), idea: lines.next().unwrap_or("").into(), ..Entry::default() };
            e.concept = std::fs::read_to_string(dir.join(format!("{name}.concept"))).unwrap_or_default();
            match source {
                Ok(source) => {
                    e.source = source;
                    e.card = std::fs::read_to_string(dir.join(format!("{name}.card.tsv"))).ok().map(|t| Card::from_tsv(&t));
                    e.state = State::Ready;
                    // 16-bit stereo at 48 kHz after a 44-byte header.
                    e.seconds = std::fs::metadata(dir.join(format!("{name}.wav"))).map_or(0.0, |m| m.len().saturating_sub(44) as f64 / (48_000.0 * 4.0));
                }
                Err(_) => e.state = State::Failed(failed.unwrap_or_else(|_| "the batch was still writing it when it was opened".into())),
            }
            songs.push(e);
        }
        let mut batch = Batch { id, dir: dir.to_path_buf(), request, songs };
        batch.read_ratings();
        Some(batch)
    }

    /// The newest batch on disk.
    pub fn latest() -> Option<Batch> {
        let id = next_id(&root()) - 1;
        (id > 0).then(|| Batch::open(&root().join("batches").join(format!("b{id:03}")))).flatten()
    }

    pub fn save_ratings(&self) {
        let mut out = String::from("song\t");
        out.push_str(&ASPECTS.join("\t"));
        out.push_str("\tfeedback\n");
        for (i, e) in self.songs.iter().enumerate() {
            out.push_str(&format!("s{}", i + 1));
            for r in e.ratings {
                out.push_str(match r {
                    Some(true) => "\tup",
                    Some(false) => "\tdown",
                    None => "\t",
                });
            }
            out.push_str(&format!("\t{}\n", e.feedback.replace(['\t', '\n'], " ")));
        }
        let _ = std::fs::write(self.dir.join("ratings.tsv"), out);
    }

    fn read_ratings(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.dir.join("ratings.tsv")) else { return };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split('\t').collect();
            let Some(i) = f.first().and_then(|s| s.strip_prefix('s')?.parse::<usize>().ok()) else { continue };
            let Some(e) = self.songs.get_mut(i.wrapping_sub(1)) else { continue };
            for (a, slot) in e.ratings.iter_mut().enumerate() {
                *slot = match f.get(a + 1) {
                    Some(&"up") => Some(true),
                    Some(&"down") => Some(false),
                    _ => None,
                };
            }
            e.feedback = f.get(ASPECTS.len() + 1).unwrap_or(&"").to_string();
        }
    }

    /// The rated songs as the learner reads them.
    pub fn rated(&self) -> Vec<gen::Rated<'_>> {
        self.songs
            .iter()
            .enumerate()
            .filter(|(_, e)| e.state == State::Ready && e.rated())
            .map(|(i, e)| gen::Rated {
                label: format!("b{:03}/s{}", self.id, i + 1),
                title: &e.title,
                idea: &e.idea,
                source: &e.source,
                ratings: ASPECTS.iter().zip(e.ratings).filter_map(|(a, r)| r.map(|r| (*a, r))).collect(),
                feedback: &e.feedback,
                card: e.card.as_ref().map(Card::report),
                concept: &e.concept,
            })
            .collect()
    }
}

/// What learning did.
#[derive(Clone, Debug, PartialEq)]
pub struct Learned {
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub changes: String,
}

impl Learned {
    pub fn added(&self) -> Vec<&String> {
        self.after.iter().filter(|r| !self.before.contains(r)).collect()
    }

    pub fn removed(&self) -> Vec<&String> {
        self.before.iter().filter(|r| !self.after.contains(r)).collect()
    }
}

/// Learn from the batch's ratings: the model rewrites the rules, they go
/// into MUSIC-AGENTS.md, and the batch's `rules.md` and the root's
/// `log.md` record it. Blocking (run it on a thread).
pub fn learn(writer: &dyn gen::Writer, batch: &Batch, cancel: &AtomicBool) -> Result<Learned, String> {
    batch.save_ratings();
    let rated = batch.rated();
    if rated.is_empty() {
        return Err("rate an aspect of a song (or write a comment) first".into());
    }
    let before = crate::context::learned(&crate::context::read());
    let (after, changes) = gen::learn(writer, &rated, &before, &crate::context::today(), cancel)?;
    crate::context::write_learned(&after)?;
    let learned = Learned { before, after, changes };
    let mut md = format!("# b{:03} rules\n\n## Changes\n{}\n\n## Added\n", batch.id, learned.changes);
    for r in learned.added() {
        md.push_str(&format!("+ {r}\n"));
    }
    md.push_str("\n## Removed or merged\n");
    for r in learned.removed() {
        md.push_str(&format!("- {r}\n"));
    }
    md.push_str("\n## Rules now\n");
    for r in &learned.after {
        md.push_str(&format!("- {r}\n"));
    }
    let _ = std::fs::write(batch.dir.join("rules.md"), &md);
    append_log(batch, Some(&learned));
    Ok(learned)
}

/// A batch's entry in `log.md`: its request, songs, ratings and the rule
/// changes learned from them.
pub fn append_log(batch: &Batch, learned: Option<&Learned>) {
    let mut out = format!("\n## b{:03} ({})\n{}", batch.id, crate::context::today(), batch.request.text());
    for (i, e) in batch.songs.iter().enumerate() {
        let ratings: Vec<String> = ASPECTS.iter().zip(e.ratings).filter_map(|(a, r)| r.map(|r| format!("{a} {}", if r { "up" } else { "down" }))).collect();
        let field = |key: &str| e.concept.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_string());
        if let (Some(genre), Some(tempo), Some(key)) = (field("SUBGENRE:"), field("TEMPO:"), field("KEY:")) {
            out.push_str(&format!("- s{} concept: {genre} · {tempo} · {key}\n", i + 1));
        }
        out.push_str(&format!(
            "- s{} \"{}\" card {}: {}{}\n",
            i + 1,
            e.title,
            e.card.as_ref().map_or("-".into(), |c| format!("{:.0}", c.total())),
            if ratings.is_empty() { "unrated".into() } else { ratings.join(", ") },
            if e.feedback.trim().is_empty() { String::new() } else { format!(" -- \"{}\"", e.feedback.trim()) }
        ));
    }
    if let Some(l) = learned {
        out.push_str(&format!("Rules: {}\n", l.changes.lines().next().unwrap_or("")));
        for r in l.added() {
            out.push_str(&format!("  + {r}\n"));
        }
        for r in l.removed() {
            out.push_str(&format!("  - {r}\n"));
        }
    }
    let path = root().join("log.md");
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| "# Music iteration log\n".into());
    text.push_str(&out);
    let _ = std::fs::write(path, text);
}

/// The last two batches' verdicts, short, for the next batch's prompt.
pub fn recent_verdicts(root: &Path) -> String {
    let last = next_id(root) - 1;
    let mut out = String::new();
    for id in last.saturating_sub(1).max(1)..=last {
        let Some(b) = Batch::open(&root.join("batches").join(format!("b{id:03}"))) else { continue };
        for (i, e) in b.songs.iter().enumerate().filter(|(_, e)| e.rated()) {
            let ratings: Vec<String> = ASPECTS.iter().zip(e.ratings).filter_map(|(a, r)| r.map(|r| format!("{a} {}", if r { "liked" } else { "disliked" }))).collect();
            out.push_str(&format!("- b{id:03}/s{} \"{}\" ({}): {}{}\n", i + 1, e.title, e.idea, ratings.join(", "), if e.feedback.trim().is_empty() { String::new() } else { format!("; \"{}\"", e.feedback.trim()) }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen::Scripted;

    /// A batch with a scripted model: songs arrive (one after a repair),
    /// ratings round trip, learning rewrites the rules in a copy of the
    /// guide and the log records it.
    #[test]
    fn a_batch_writes_renders_rates_and_learns() {
        let dir = std::env::temp_dir().join(format!("music_iter_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let guide = dir.join("MUSIC-AGENTS.md");
        std::fs::write(&guide, makepad_score_live::guide::MUSIC_AGENTS).unwrap();
        std::env::set_var("MUSIC_ITER_DIR", &dir);
        std::env::set_var("MUSIC_AGENTS_MD", &guide);
        let song = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet kit = Drums{kit: @analog_hybrid}\nlet bass = Synth{preset: @acid}\nlet kick = Loop{play: kit hits: \"x...x...x...x...\" amp: 0.9}\nlet line = Loop{play: bass beats: 4 code: fn(i) { for s in 0..8 { play(choose([@a1 @c2 @e2]), {dur: 0.2}) sleep(0.5) } }}\nlet a = Section{bars: 4 loops: [kick line] next: @stop}\nScore{clock kit bass kick line a arrange: [a]}\n";
        let writer: SharedWriter = Arc::new(Scripted(move |system: &str, turns: &[String]| {
            if system.contains("You are the director") {
                let cards: String = (1..=2).map(|k| format!("=== CONCEPT {k}\nTITLE: Song {k}\nSUBGENRE: techno\nTEMPO: 12{k}\nKEY: A minor\nPROGRESSION: i\nPALETTE: 909\nFORM: a 4\nFEEL: straight\nMOOD: dark\nLEAD: acid\nHOOK: three notes\nDIFFERS: number {k}\n")).collect();
                return Ok(cards);
            }
            if system.contains("Learned from listening\" section") {
                return Ok("```md\n- 2026-10-01 do: a four-on-the-floor 909 kick; the drums were liked.\n```\nCHANGES: added one rule from the liked drums.".into());
            }
            // Song 2's first reply has no score: it is asked again.
            if turns[0].contains("song 2 of") && turns.len() == 1 {
                return Ok("TITLE: Nothing\nIDEA: none".into());
            }
            Ok(format!("TITLE: Test\nIDEA: a kick and an acid line\n```splash\n{song}```\n"))
        }));
        let req = Request { prompt: "a test".into(), markers: vec!["techno".into()], seconds: 8 };
        let (mut batch, mut run) = Batch::start(writer.clone(), req, 2).unwrap();
        let t0 = std::time::Instant::now();
        while !run.done() && t0.elapsed().as_secs() < 120 {
            run.poll(&mut batch);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(batch.songs.iter().all(|e| e.state == State::Ready), "{:?}", batch.songs.iter().map(|e| &e.state).collect::<Vec<_>>());
        assert!(batch.songs[0].card.as_ref().unwrap().total() > 0.0);
        assert!(batch.songs[1].concept.contains("TITLE: Song 2"), "each song carries its card");
        assert!(std::fs::read_to_string(batch.dir.join("concepts.md")).unwrap().contains("## s2"));
        batch.songs[0].ratings[3] = Some(true);
        batch.songs[1].ratings[2] = Some(false);
        batch.songs[1].feedback = "too busy".into();
        batch.save_ratings();
        let reopened = Batch::open(&batch.dir).unwrap();
        assert_eq!(reopened.songs[0].ratings[3], Some(true));
        assert_eq!(reopened.songs[1].feedback, "too busy");
        assert_eq!(reopened.rated().len(), 2);
        let learned = learn(&*writer, &reopened, &AtomicBool::new(false)).unwrap();
        assert_eq!(learned.added().len(), 1);
        assert_eq!(crate::context::learned(&std::fs::read_to_string(&guide).unwrap()), learned.after);
        assert!(std::fs::read_to_string(dir.join("log.md")).unwrap().contains("drums up"));
        assert!(recent_verdicts(&dir).contains("too busy"));
        let h = history(&dir, 3);
        assert!(h.contains("(techno)") && h.contains("120 bpm") && h.contains("drums liked"), "{h}");
        assert!(std::fs::read_to_string(dir.join("log.md")).unwrap().contains("concept: techno"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What the last `batches` batches made, for the director: per song its
/// title, sub-genre, key (the scorecard's reading), tempo, palette (the
/// instruments it played) and the person's ratings.
pub fn history(root: &Path, batches: u32) -> String {
    let last = next_id(root) - 1;
    let mut out = String::new();
    for id in last.saturating_sub(batches - 1).max(1)..=last {
        let Some(b) = Batch::open(&root.join("batches").join(format!("b{id:03}"))) else { continue };
        out.push_str(&format!("b{id:03} ({}):\n", b.request.text().lines().collect::<Vec<_>>().join("; ")));
        for e in b.songs.iter().filter(|e| e.state == State::Ready) {
            let key = e.card.as_ref().and_then(|c| c.get("harmony")).and_then(|m| m.value.split(':').next()).unwrap_or("?").to_string();
            let genre = e.concept.lines().find_map(|l| l.strip_prefix("SUBGENRE:")).unwrap_or("").trim().to_string();
            let (bpm, palette) = makepad_score_live::model::load(&e.source).map_or((0.0, String::new()), |l| {
                let names: Vec<String> = l.doc.insts.iter().map(|i| match &i.kind {
                    makepad_score_live::model::InstKind::Synth { .. } => format!("{} (synth)", i.name),
                    makepad_score_live::model::InstKind::Drums { kit, .. } => format!("drums {kit}"),
                    makepad_score_live::model::InstKind::Native { native, program, .. } if native == "soundfont" => format!("{} (GM {program})", i.name),
                    makepad_score_live::model::InstKind::Native { native, .. } => format!("{} ({native})", i.name),
                    makepad_score_live::model::InstKind::Shader { shader, .. } => format!("{} (shader {})", i.name, shader.lib.as_deref().unwrap_or("own")),
                    makepad_score_live::model::InstKind::Ironfish { .. } => format!("{} (ironfish)", i.name),
                    makepad_score_live::model::InstKind::Voice { .. } => format!("{} (voice)", i.name),
                    _ => i.name.clone(),
                }).collect();
                (l.doc.clock.bpm, names.join(", "))
            });
            let ratings: Vec<String> = ASPECTS.iter().zip(e.ratings).filter_map(|(a, r)| r.map(|r| format!("{a} {}", if r { "liked" } else { "disliked" }))).collect();
            out.push_str(&format!(
                "- \"{}\"{} · {key} · {bpm:.0} bpm · {palette} · {}{}\n",
                e.title,
                if genre.is_empty() { String::new() } else { format!(" ({genre})") },
                if ratings.is_empty() { "unrated".into() } else { ratings.join(", ") },
                if e.feedback.trim().is_empty() { String::new() } else { format!("; \"{}\"", e.feedback.trim()) }
            ));
        }
    }
    out
}
