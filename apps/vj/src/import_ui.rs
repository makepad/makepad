//! IMPORT CONTENT: the panel's state, its worker thread, and the progress
//! it reports.
//!
//! Nothing here draws. The DSL panel lives in `main.rs` next to the rest of
//! the chrome; this module owns the part that must not run on the UI thread:
//! walking a directory that may hold ten thousand files, decoding a frame of
//! each video for its thumbnail, and publishing them one by one.
//!
//! ## Why a thread and a channel rather than a few frames of work
//!
//! A multi-gigabyte directory is not a "spread it over some frames" problem.
//! Hashing one 2 GB clip is seconds of pure IO, and there may be hundreds.
//! So the whole job runs on a worker, the UI polls a channel at its existing
//! tick, and the only shared state is a cancel flag. The UI thread never
//! blocks on a file, and the show keeps running while a library imports.
//!
//! ## Progress that does not lie
//!
//! Two phases, reported as they really are: a fast SCAN (walk the tree, no
//! IO beyond `read_dir`) and a slow IMPORT (per file: alias probe, thumbnail,
//! reference admission, publish). The bar maps them into 0..1 with the scan
//! taking the first sliver, because that is roughly its share of the wall
//! clock, and the label always names the file currently being worked on.
//!
//! Messages are throttled — one per file would flood the 20 Hz poll on a
//! big library — but the first, the last and one per percent always get
//! through, so the bar starts immediately and finishes exactly.
//!
//! With VARIABLE-FRAMERATE VIDEO IMPORT on, one file is no longer a moment:
//! converting a clip decodes it, measures the motion of every frame pair and
//! re-encodes the whole thing all-intra, which is seconds to minutes. So that
//! file's own progress is reported INSIDE the step — the phase names itself
//! ("converting 42%") and the bar advances fractionally across the file
//! rather than sitting still and then jumping. A bar that does not move for
//! four minutes is indistinguishable from a hang, and this one is not one.
//!
//! A worker that dies without a verdict becomes a VISIBLE FAILURE, never a
//! bar that spins forever: the channel disconnecting mid-import is itself
//! the error.

use crate::media_scan::{self, FileOutcome, FileProgress, ImportCtx, MediaFile, MediaScan};
use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig};
use makepad_widgets::makepad_platform::thread::{Lane, TaskPool};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

/// What the panel is doing right now.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ImportPhase {
    #[default]
    Idle,
    /// Walking the tree. `found` grows as it goes; there is no total yet,
    /// because knowing it would mean having already walked.
    Scanning { found: usize },
    /// Publishing. `done` of `total`, the file being worked on, and — while
    /// a file's own slow phase runs — what that phase is and how far in it
    /// is (0..1 within this one file).
    Importing {
        done: usize,
        total: usize,
        current: String,
        phase: String,
        file_fraction: f64,
    },
    Done(ImportSummary),
    Failed(String),
    Cancelled(ImportSummary),
}

impl ImportPhase {
    pub fn busy(&self) -> bool {
        matches!(self, ImportPhase::Scanning { .. } | ImportPhase::Importing { .. })
    }
}

/// The counts a finished (or abandoned) run produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub published: usize,
    /// How many of the published ones were CONVERTED for flow-warp playback
    /// (a subset of `published`, not a separate bucket).
    pub converted: usize,
    pub already_present: usize,
    pub failed: usize,
    /// Recognised-but-unpublishable files, with reasons.
    pub skipped: usize,
    /// Up to a bounded number of real per-file failure notes, verbatim.
    /// A library where "12 failed" is all you get is a library you cannot
    /// fix; these are the sentences that say what to do.
    pub notes: Vec<String>,
}

/// Most failure notes retained. Bounded because a directory of ten thousand
/// broken files should not become ten thousand lines of UI state.
const MAX_NOTES: usize = 24;

impl ImportSummary {
    fn note(&mut self, text: String) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(text);
        }
    }

    pub fn touched(&self) -> usize {
        self.published + self.already_present + self.failed
    }
}

enum Msg {
    Phase(ImportPhase),
    Finished(ImportPhase),
}

/// The panel's whole state.
pub struct ImportPanel {
    /// Visible in the UI.
    pub open: bool,
    /// The path the text field holds.
    pub path: String,
    /// VARIABLE FRAMERATE VIDEO IMPORT: convert videos on the way in
    /// (optical flow + all-intra re-encode + `mkfl`) and store them as owned
    /// blobs, instead of referencing them where they lie. Persisted by the
    /// panel's owner; images and audio are unaffected either way.
    pub convert_video: bool,
    pub phase: ImportPhase,
    /// One line of human-readable status, always safe to draw.
    pub status: String,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Msg>>,
    pool: Option<TaskPool>,
}

impl Default for ImportPanel {
    fn default() -> Self {
        Self {
            open: false,
            path: String::new(),
            convert_video: false,
            phase: ImportPhase::Idle,
            status: "drop a folder here, or type a path".to_string(),
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            pool: None,
        }
    }
}

impl ImportPanel {
    pub fn set_task_pool(&mut self, pool: TaskPool) {
        self.pool = Some(pool);
    }

    pub fn set_path(&mut self, path: impl Into<String>) {
        self.path = path.into();
        if !self.phase.busy() {
            self.status = format!("ready: {}", self.path);
        }
    }

    pub fn busy(&self) -> bool {
        self.phase.busy()
    }

    /// 0..1 for the progress bar. Two honest bands: the scan is cheap and
    /// gets the first 6%, the imports get the rest.
    pub fn progress(&self) -> f64 {
        match &self.phase {
            ImportPhase::Idle => 0.0,
            ImportPhase::Scanning { .. } => 0.03,
            ImportPhase::Importing { done, total, file_fraction, .. } => {
                if *total == 0 {
                    0.06
                } else {
                    // The part of the current file that is already done
                    // counts: with conversion on, one file can be minutes.
                    let at = *done as f64 + file_fraction.clamp(0.0, 1.0);
                    0.06 + 0.94 * (at / *total as f64).min(1.0)
                }
            }
            ImportPhase::Done(_) | ImportPhase::Failed(_) | ImportPhase::Cancelled(_) => 1.0,
        }
    }

    pub fn cancel(&mut self) {
        if self.phase.busy() {
            self.cancel.store(true, Ordering::SeqCst);
            self.status = "cancelling…".to_string();
        }
    }

    /// Begin an import. `endpoints`/`token`/`server_id` come from the live
    /// session, so the worker talks to exactly the store the UI is showing —
    /// embedded or remote, it makes no difference here.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(
        &mut self,
        endpoints: ApiEndpoints,
        server_id: [u8; 16],
        token: Option<String>,
        cache_parent: PathBuf,
    ) -> Result<(), String> {
        if self.phase.busy() {
            return Err("an import is already running".to_string());
        }
        let raw = self.path.trim().to_string();
        if raw.is_empty() {
            return Err(self.refuse("name a file or folder to import"));
        }
        let root = PathBuf::from(&raw);
        if !root.exists() {
            return Err(self.refuse(format!("{} does not exist", root.display())));
        }
        let Some(token) = token else {
            return Err(self.refuse("no asset server session yet"));
        };

        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.phase = ImportPhase::Scanning { found: 0 };
        self.status = format!("scanning {}…", root.display());

        let cache = cache_parent.join("import-cache");
        let convert = self.convert_video;
        let pool = self
            .pool
            .clone()
            .ok_or_else(|| self.refuse("import worker is not started"))?;
        pool.submit(Lane::Heavy, move || {
                let verdict =
                    run(&root, endpoints, server_id, token, cache, convert, &tx, &cancel);
                let _ = tx.send(Msg::Finished(verdict));
            })
            .map(|handle| handle.detach())
            .map_err(|e| self.refuse(format!("cannot start the import job: {e}")))?;
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn start(
        &mut self,
        _endpoints: ApiEndpoints,
        _server_id: [u8; 16],
        _token: Option<String>,
        _cache_parent: PathBuf,
    ) -> Result<(), String> {
        Err(self.refuse("folder import unavailable on web"))
    }

    fn refuse(&mut self, why: impl Into<String>) -> String {
        let why = why.into();
        self.phase = ImportPhase::Failed(why.clone());
        self.status = why.clone();
        why
    }

    /// Drain the worker. Returns true when anything changed, so the caller
    /// only redraws when there is something new.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(rx) = &self.rx else { break };
            match rx.try_recv() {
                Ok(Msg::Phase(phase)) => {
                    self.status = describe(&phase);
                    self.phase = phase;
                    changed = true;
                }
                Ok(Msg::Finished(phase)) => {
                    self.status = describe(&phase);
                    self.phase = phase;
                    self.rx = None;
                    changed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    // A worker that vanished mid-run must SAY so. A bar left
                    // spinning at 40% is a lie the user cannot act on.
                    if self.phase.busy() {
                        let why = "the import thread exited without a verdict".to_string();
                        self.phase = ImportPhase::Failed(why.clone());
                        self.status = why;
                    }
                    changed = true;
                    break;
                }
            }
        }
        changed
    }
}

/// One line describing a phase, for the panel's status label.
pub fn describe(phase: &ImportPhase) -> String {
    match phase {
        ImportPhase::Idle => "drop a folder here, or type a path".to_string(),
        ImportPhase::Scanning { found } => format!("scanning… {found} media files so far"),
        ImportPhase::Importing { done, total, current, phase, file_fraction } => {
            if phase.is_empty() {
                format!("importing {}/{} · {current}", done + 1, total)
            } else {
                format!(
                    "{phase} {}/{} · {current} {:.0}%",
                    done + 1,
                    total,
                    file_fraction * 100.0
                )
            }
        }
        ImportPhase::Done(s) => summary_line("imported", s),
        ImportPhase::Cancelled(s) => summary_line("cancelled", s),
        ImportPhase::Failed(why) => format!("failed: {why}"),
    }
}

fn summary_line(verb: &str, s: &ImportSummary) -> String {
    let mut line = format!("{verb}: {} new", s.published);
    if s.converted > 0 {
        line.push_str(&format!(" · {} flow-converted", s.converted));
    }
    if s.already_present > 0 {
        line.push_str(&format!(" · {} already in the library", s.already_present));
    }
    if s.failed > 0 {
        line.push_str(&format!(" · {} failed", s.failed));
    }
    if s.skipped > 0 {
        line.push_str(&format!(" · {} unsupported", s.skipped));
    }
    line
}

/// The worker body. Everything expensive lives here.
#[allow(clippy::too_many_arguments)]
fn run(
    root: &Path,
    endpoints: ApiEndpoints,
    server_id: [u8; 16],
    token: String,
    cache: PathBuf,
    convert_video: bool,
    tx: &mpsc::Sender<Msg>,
    cancel: &Arc<AtomicBool>,
) -> ImportPhase {
    let convert_dir = cache.join("convert");
    let mut config = ClientConfig::new(cache);
    config.token = Some(token);
    let mut client = match AssetClient::connect(config, endpoints, Some(server_id)) {
        Ok(c) => c,
        Err(error) => return ImportPhase::Failed(format!("asset server: {error}")),
    };

    let scan: MediaScan = media_scan::scan(root);
    let _ = tx.send(Msg::Phase(ImportPhase::Scanning { found: scan.files.len() }));
    let mut summary = ImportSummary { skipped: scan.skipped.len(), ..Default::default() };
    for skip in scan.skipped.iter().take(MAX_NOTES) {
        summary.note(format!(
            "skipped {}: {}",
            skip.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            skip.why
        ));
    }
    if scan.files.is_empty() {
        return ImportPhase::Done(summary);
    }
    if scan.truncated {
        summary.note("the folder held more files than one import may take".to_string());
    }

    let total = scan.files.len();
    let rights = media_scan::personal_rights(root);
    let convert_options = makepad_video_flow::ConvertOptions::default();
    // The UI polls at 20 Hz; a ten-thousand-file library must not send ten
    // thousand messages. First, last, and about one per percent.
    let step = (total / 100).max(1);
    for (index, file) in scan.files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return ImportPhase::Cancelled(summary);
        }
        let label = label_of(file);
        if index == 0 || index + 1 >= total || index % step == 0 {
            let _ = tx.send(Msg::Phase(ImportPhase::Importing {
                done: index,
                total,
                current: label.clone(),
                phase: String::new(),
                file_fraction: 0.0,
            }));
        }
        // Inside one file: the conversion reports its own progress here, and
        // it is forwarded as it arrives (the converter already throttles to
        // a few messages a second).
        let mut progress = |p: FileProgress| {
            let _ = tx.send(Msg::Phase(ImportPhase::Importing {
                done: index,
                total,
                current: label.clone(),
                phase: p.phase.to_string(),
                file_fraction: p.fraction,
            }));
        };
        let cancelled = || cancel.load(Ordering::SeqCst);
        let mut ctx = ImportCtx {
            rights: &rights,
            convert_dir: convert_video.then_some(convert_dir.as_path()),
            convert_options,
            progress: &mut progress,
            cancel: &cancelled,
        };
        match media_scan::import_file(&mut client, root, file, &mut ctx) {
            FileOutcome::Published { converted, note } => {
                summary.published += 1;
                if converted {
                    summary.converted += 1;
                }
                if let Some(note) = note {
                    summary.note(note);
                }
            }
            FileOutcome::AlreadyPresent => summary.already_present += 1,
            FileOutcome::Failed(why) => {
                summary.failed += 1;
                summary.note(format!("{label}: {why}"));
            }
            // Stopped in the middle of this file: the converter has already
            // removed its half-written temp, and this run is over.
            FileOutcome::Cancelled => return ImportPhase::Cancelled(summary),
        }
    }
    ImportPhase::Done(summary)
}

fn label_of(file: &MediaFile) -> String {
    file.path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.stem.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn importing(done: usize, total: usize, phase: &str, fraction: f64) -> ImportPhase {
        ImportPhase::Importing {
            done,
            total,
            current: "clip.mp4".into(),
            phase: phase.to_string(),
            file_fraction: fraction,
        }
    }

    #[test]
    fn progress_is_monotonic_and_bounded() {
        let mut p = ImportPanel::default();
        assert_eq!(p.progress(), 0.0);
        p.phase = ImportPhase::Scanning { found: 3 };
        let scanning = p.progress();
        p.phase = importing(0, 10, "", 0.0);
        let start = p.progress();
        p.phase = importing(10, 10, "", 0.0);
        let end = p.progress();
        p.phase = ImportPhase::Done(ImportSummary::default());
        assert!(scanning > 0.0 && scanning <= start);
        assert!(start < end && end <= 1.0);
        assert_eq!(p.progress(), 1.0);
    }

    #[test]
    fn a_slow_file_moves_the_bar_inside_its_own_step() {
        // The whole point of the conversion phase: four minutes on one clip
        // must LOOK like four minutes of work, not a frozen bar.
        let mut p = ImportPanel::default();
        p.phase = importing(3, 10, "converting", 0.0);
        let start = p.progress();
        p.phase = importing(3, 10, "converting", 0.5);
        let half = p.progress();
        p.phase = importing(3, 10, "converting", 1.0);
        let full = p.progress();
        assert!(start < half && half < full, "{start} {half} {full}");
        // And it never runs past where the NEXT file's step begins.
        p.phase = importing(4, 10, "", 0.0);
        assert!((full - p.progress()).abs() < 1e-9);
        // The label says which phase, and how far in.
        assert!(describe(&importing(3, 10, "converting", 0.42)).contains("converting 4/10"));
        assert!(describe(&importing(3, 10, "converting", 0.42)).contains("42%"));
        // With no named phase it stays the plain line it always was.
        assert_eq!(describe(&importing(3, 10, "", 0.0)), "importing 4/10 · clip.mp4");
    }

    #[test]
    fn a_vanished_worker_becomes_a_visible_failure() {
        let mut p = ImportPanel::default();
        let (tx, rx) = mpsc::channel::<Msg>();
        p.rx = Some(rx);
        p.phase = importing(2, 9, "", 0.0);
        drop(tx); // the worker died without a verdict
        assert!(p.poll());
        match &p.phase {
            ImportPhase::Failed(why) => assert!(why.contains("without a verdict")),
            other => panic!("expected a visible failure, got {other:?}"),
        }
    }

    #[test]
    fn starting_without_a_path_refuses_and_says_why() {
        let mut p = ImportPanel::default();
        let err = p
            .start(
                ApiEndpoints {
                    control: "127.0.0.1:1".parse().unwrap(),
                    data: "127.0.0.1:2".parse().unwrap(),
                },
                [0u8; 16],
                Some("mpat_x".into()),
                std::env::temp_dir(),
            )
            .unwrap_err();
        assert!(err.contains("name a file or folder"));
        assert!(matches!(p.phase, ImportPhase::Failed(_)));
    }

    #[test]
    fn notes_are_bounded() {
        let mut s = ImportSummary::default();
        for i in 0..500 {
            s.note(format!("note {i}"));
        }
        assert_eq!(s.notes.len(), MAX_NOTES);
    }

    #[test]
    fn the_summary_line_reports_every_bucket() {
        let s = ImportSummary {
            published: 3,
            converted: 2,
            already_present: 2,
            failed: 1,
            skipped: 4,
            notes: vec![],
        };
        let line = summary_line("imported", &s);
        assert!(line.contains("3 new"));
        assert!(line.contains("2 flow-converted"));
        assert!(line.contains("2 already"));
        assert!(line.contains("1 failed"));
        assert!(line.contains("4 unsupported"));
        // Nothing converted, nothing said about conversion.
        let plain = ImportSummary { published: 1, ..Default::default() };
        assert!(!summary_line("imported", &plain).contains("flow-converted"));
    }
}
