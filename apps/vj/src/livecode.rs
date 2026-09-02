//! LIVECODING: observed effect-document directories, and the compile
//! answer a coding agent reads back.
//!
//! Two halves, both small.
//!
//! **The origins.** Two directories are watched by the store this process
//! hosts (see [`makepad_asset_store::observe`]): `local/vjfx/`, the
//! scratch origin anybody can drop a file into, and
//! `apps/vj/resources/effects/`, where the bundled presets themselves
//! live. Editing a bundled preset's FILE therefore republishes that
//! preset's alias as a new revision — the same gesture as writing a new
//! one, because the alias rule is the same rule (`vjfx/<file-stem>`).
//!
//! The self-healing seed pass ([`crate::effects::seed`]) and the observer
//! do not fight: a seeded head carries the `builtin` tag and an observed
//! one does not, and seeding never rewrites a head that is not its own.
//! Whichever runs second, the file on disk is what the alias ends up
//! naming. The observer is started AFTER the seed pass on the same worker
//! thread so the common case is not even a race.
//!
//! **The answer.** A document that does not parse, or whose shader does not
//! compile, must not fail silently at whoever just saved it. The app
//! already reports both — the script evaluator through
//! `VjFxView::set_effect_source`'s `Err`, the draw-shader compiler through
//! `error!` — so this module TAPS that reporting rather than inventing a
//! second validator, and writes it where a polling agent can read it:
//!
//! - `local/vjfx/compile.log` — one line per outcome, appended, capped.
//! - `local/vjfx/status/<stem>.status` — the latest outcome for one
//!   document, overwritten, with the full error text.
//!
//! Both are written only for documents that live in an observed origin:
//! the bundled library rendering its own thumbnails is not news.
//!
//! The status file's first line is `compile ok` or `compile error: …`, so
//! the poll is one `grep`. "ok" is only ever claimed by a path that
//! actually DREW the document (a thumbnail lane, or a slot load plus a
//! settle window), because a draw shader compiles at draw time and a
//! parse-clean document with a broken shader is exactly the case that
//! must not read as fine.

use makepad_widgets::*;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use crate::clock::Instant;
use std::time::Duration;

/// How long after a load the settle pass harvests compile errors. A draw
/// shader compiles on the next draw of the host that loaded it; a couple of
/// frames is plenty, and the window costs nothing but the answer's latency.
const SETTLE_MS: u64 = 400;

/// Lines the tap keeps. Bounded: the tap runs inside logging.
const TAP_CAP: usize = 96;

/// Lines `compile.log` keeps.
const LOG_CAP: usize = 200;

fn checkout_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The scratch origin: `local/vjfx/`, overridable with `VJ_FX_ORIGIN`.
/// This is also where the compile answers are written, so an agent has one
/// directory to know about.
pub fn scratch_origin() -> PathBuf {
    if let Ok(dir) = std::env::var("VJ_FX_ORIGIN") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    checkout_root().join("local/vjfx")
}

/// The SEED origin: the bundled preset directory itself, in a checkout run.
/// Absent from an installed binary's filesystem, and then simply not
/// watched.
pub fn seed_origin() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/effects");
    dir.is_dir().then_some(dir)
}

/// Every observed origin, scratch first.
pub fn origins() -> Vec<PathBuf> {
    let mut out = vec![scratch_origin()];
    if let Some(seed) = seed_origin() {
        out.push(seed);
    }
    out
}

pub fn compile_log_path() -> PathBuf {
    scratch_origin().join("compile.log")
}

pub fn status_path(stem: &str) -> PathBuf {
    scratch_origin().join("status").join(format!("{stem}.status"))
}

/// Is `stem` a document one of the origins holds? Only those get an answer
/// written; everything else in the library is somebody else's business.
fn is_observed(stem: &str) -> bool {
    origins()
        .iter()
        .any(|dir| dir.join(format!("{stem}.splash")).is_file())
}

/// The origin file behind a document stem, if one of the origins holds it.
fn origin_file(stem: &str) -> Option<PathBuf> {
    origins()
        .into_iter()
        .map(|dir| dir.join(format!("{stem}.splash")))
        .find(|p| p.is_file())
}

/// Is this LIVECODED document transition-suited?
///
/// The catalog knows (the observer tags it) but a search hit does not carry
/// tags, and the client needs the answer per tile — for the slot type gate
/// and for the thumbnail lane, which must premix inputs for a document that
/// shapes the program picture instead of drawing its own. So it is read
/// from the file, once, and remembered against that file's mtime.
#[cfg(not(target_arch = "wasm32"))]
pub fn observed_is_transition(stem: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, (u128, bool)>>> = OnceLock::new();
    let Some(path) = origin_file(stem) else { return false };
    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some((at, is)) = map.get(stem) {
            if *at == mtime {
                return *is;
            }
        }
    }
    let Ok(source) = std::fs::read_to_string(&path) else { return false };
    let is = makepad_asset_store::observe::declares_transition(&source, stem, &[]);
    if let Ok(mut map) = cache.lock() {
        if map.len() > 512 {
            map.clear();
        }
        map.insert(stem.to_string(), (mtime, is));
    }
    is
}

#[cfg(target_arch = "wasm32")]
pub fn observed_is_transition(_stem: &str) -> bool {
    false
}

/// Run the observer for this process's hosted store until `stop` flips.
/// Blocking — call it on the worker thread that has just finished seeding.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_observer(client: &mut makepad_asset_client::AssetClient, stop: &AtomicBool) {
    let mut config = makepad_asset_store::observe::ObserveConfig::vjfx(origins());
    // The one thing an engine name cannot tell the store: a scene engine
    // somebody shipped as a transition. The bundled list is that knowledge.
    config.transition_stems = crate::effects::seed::TRANSITION_PRESETS
        .iter()
        .map(|s| s.to_string())
        .collect();
    makepad_asset_store::observe::run(client, &config, stop);
}

// ---------------------------------------------------------------------------
// the log tap
// ---------------------------------------------------------------------------

struct Tap {
    seq: AtomicU64,
    lines: Mutex<VecDeque<(u64, String)>>,
}

fn tap() -> &'static Tap {
    static TAP: OnceLock<Tap> = OnceLock::new();
    TAP.get_or_init(|| Tap { seq: AtomicU64::new(0), lines: Mutex::new(VecDeque::new()) })
}

/// The tap itself. NEVER logs — it runs inside the logging call.
fn collect(message: &str, level: LogLevel) {
    if !matches!(level, LogLevel::Error | LogLevel::Warning | LogLevel::Panic) {
        return;
    }
    let t = tap();
    let seq = t.seq.fetch_add(1, Ordering::Relaxed);
    let Ok(mut lines) = t.lines.lock() else { return };
    lines.push_back((seq, message.chars().take(4000).collect()));
    while lines.len() > TAP_CAP {
        lines.pop_front();
    }
}

/// Install the tap and make sure the answer directories exist. Idempotent.
pub fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // The log callback may run on any worker. Construct its OnceLock on
        // the UI thread before publishing the callback so no worker can race
        // the UI through OnceLock's blocking initialization path on wasm.
        let _ = tap();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::fs::create_dir_all(scratch_origin().join("status"));
        makepad_widgets::makepad_platform::log::set_log_tap(Some(collect));
    });
}

/// The current tap position: everything logged AFTER this belongs to what
/// happens next.
pub fn mark() -> u64 {
    tap().seq.load(Ordering::Relaxed)
}

/// Error/warning lines logged since `mark`, most useful first.
fn lines_since(mark: u64) -> Vec<String> {
    let t = tap();
    let Ok(lines) = t.lines.lock() else { return Vec::new() };
    lines
        .iter()
        .filter(|(seq, _)| *seq >= mark)
        .map(|(_, text)| text.clone())
        .collect()
}

/// Does this logged line describe a compile/evaluation failure of a
/// DOCUMENT, as opposed to any other thing the app grumbles about?
fn is_compile_line(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("draw shader")
        || t.contains("shader")
        || t.contains("script")
        || t.contains("failed to compile")
}

// ---------------------------------------------------------------------------
// revision → document
// ---------------------------------------------------------------------------

fn aliases() -> &'static Mutex<HashMap<String, String>> {
    static ALIASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ALIASES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember which document a revision is, so an outcome reported against a
/// revision can be written under the file stem an agent knows.
#[cfg(not(target_arch = "wasm32"))]
pub fn remember(revision: &str, alias: Option<&str>) {
    let Some(alias) = alias else { return };
    let stem = alias.rsplit('/').next().unwrap_or(alias);
    if stem.is_empty() || !is_observed(stem) {
        return;
    }
    let Ok(mut map) = aliases().lock() else { return };
    if map.len() > 512 {
        map.clear();
    }
    map.insert(revision.to_string(), stem.to_string());
}

#[cfg(target_arch = "wasm32")]
pub fn remember(_revision: &str, _alias: Option<&str>) {}

/// The file stem behind a revision, when it is an observed document.
pub fn stem_of(revision: &str) -> Option<String> {
    let map = aliases().lock().ok()?;
    map.get(revision).cloned()
}

// ---------------------------------------------------------------------------
// reporting
// ---------------------------------------------------------------------------

enum Job {
    /// Harvest the tap at `at` and write the verdict for this document.
    Settle { stem: String, revision: String, mark: u64, at: Instant },
    /// A definite failure: write it now, no settle.
    Failed { stem: String, revision: String, error: String },
}

fn worker() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        // One thread, only ever sleeping and writing two small files. It
        // outlives the app by design: there is nothing to shut down and
        // nothing it holds that matters at exit. The web build has no
        // observed origin on disk, so the status files are not written.
        #[cfg(target_arch = "wasm32")]
        drop(rx);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::thread::Builder::new()
            .name("vj-livecode-status".to_string())
            .spawn(move || {
                let mut queue: VecDeque<Job> = VecDeque::new();
                loop {
                    // Wait for work, then drain what is already queued.
                    match rx.recv() {
                        Ok(job) => queue.push_back(job),
                        Err(_) => return,
                    }
                    while let Ok(job) = rx.try_recv() {
                        queue.push_back(job);
                    }
                    while let Some(job) = queue.pop_front() {
                        match job {
                            Job::Failed { stem, revision, error } => {
                                write_status(&stem, &revision, Err(&error));
                            }
                            Job::Settle { stem, revision, mark, at } => {
                                let now = Instant::now();
                                if now < at {
                                    std::thread::sleep(at - now);
                                }
                                let errors: Vec<String> = lines_since(mark)
                                    .into_iter()
                                    .filter(|l| is_compile_line(l))
                                    .collect();
                                if errors.is_empty() {
                                    write_status(&stem, &revision, Ok(()));
                                } else {
                                    write_status(&stem, &revision, Err(&errors.join("\n")));
                                }
                            }
                        }
                    }
                }
            });
        tx
    })
}

/// Report what happened to a document that was just loaded and drawn.
///
/// `outcome` is the definite half — a parse/evaluation error, or "it
/// loaded". A load that succeeded still waits out [`SETTLE_MS`] before
/// claiming `compile ok`, because the draw shader compiles after the load
/// returns and its failure arrives through the log.
#[cfg(not(target_arch = "wasm32"))]
pub fn report(revision: &str, mark: u64, outcome: Result<(), String>) {
    let Some(stem) = stem_of(revision) else { return };
    let job = match outcome {
        Err(error) => Job::Failed { stem, revision: revision.to_string(), error },
        Ok(()) => Job::Settle {
            stem,
            revision: revision.to_string(),
            mark,
            at: Instant::now() + Duration::from_millis(SETTLE_MS),
        },
    };
    let _ = worker().send(job);
}

#[cfg(target_arch = "wasm32")]
pub fn report(_revision: &str, _mark: u64, _outcome: Result<(), String>) {}

fn now_ms() -> u64 {
    (makepad_widgets::Cx::time_now().max(0.0) * 1000.0) as u64
}

/// Write one document's verdict: the per-document status file (whole text)
/// and one line into the shared log (first line only, so the log stays
/// line-per-event).
fn write_status(stem: &str, revision: &str, outcome: Result<(), &str>) {
    let verdict = match outcome {
        Ok(()) => "compile ok".to_string(),
        Err(error) => format!("compile error: {}", error.trim()),
    };
    let short: String = revision.chars().take(20).collect();
    let status = format!("{verdict}\ndoc: {stem}\nrevision: {revision}\nt: {}\n", now_ms());
    let path = status_path(stem);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = write_atomic(&path, status.as_bytes());

    let head = verdict.lines().next().unwrap_or(&verdict);
    let line = format!("{} {stem} rev={short} {head}\n", now_ms());
    append_capped(&compile_log_path(), &line);
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("status.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Append one line, keeping the tail bounded. Rewrites only when over cap,
/// so the steady state is one small append.
fn append_capped(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let over = std::fs::metadata(path).map(|m| m.len() > 128 * 1024).unwrap_or(false);
    if over {
        if let Ok(text) = std::fs::read_to_string(path) {
            let kept: Vec<&str> = text.lines().rev().take(LOG_CAP).collect();
            let mut out = String::new();
            for l in kept.into_iter().rev() {
                out.push_str(l);
                out.push('\n');
            }
            let _ = std::fs::write(path, out);
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compile_line_is_recognised_and_ordinary_noise_is_not() {
        assert!(is_compile_line(
            "draw shader 'DrawVjFxParticles' failed to compile and will NOT be drawn:\nline 3"
        ));
        assert!(is_compile_line("script error: expected `}`"));
        assert!(!is_compile_line("midi device disconnected"));
    }

    #[test]
    fn the_status_file_says_compile_ok_or_carries_the_error_text() {
        let dir = std::env::temp_dir().join(format!(
            "vj-livecode-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(dir.join("status")).unwrap();
        // Point the module's own path helpers at the scratch dir.
        std::env::set_var("VJ_FX_ORIGIN", &dir);

        write_status("42_probe", "arev_deadbeef", Ok(()));
        let text = std::fs::read_to_string(status_path("42_probe")).unwrap();
        assert!(text.starts_with("compile ok"), "{text}");
        assert!(text.contains("doc: 42_probe"));

        write_status("42_probe", "arev_deadbeef", Err("line 7: unknown fn `wobble`"));
        let text = std::fs::read_to_string(status_path("42_probe")).unwrap();
        assert!(text.starts_with("compile error: line 7: unknown fn `wobble`"), "{text}");

        let log = std::fs::read_to_string(compile_log_path()).unwrap();
        assert_eq!(log.lines().count(), 2, "one line per outcome");
        assert!(log.lines().next().unwrap().contains("compile ok"));
        assert!(log.lines().nth(1).unwrap().contains("compile error"));

        std::env::remove_var("VJ_FX_ORIGIN");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_seed_directory_is_an_origin_in_a_checkout() {
        let origins = origins();
        assert!(origins.len() >= 2, "the bundled preset directory must be observed too");
        assert!(origins[1].join("01_fireworks.splash").is_file());
    }
}
