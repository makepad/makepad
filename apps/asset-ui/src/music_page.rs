//! The Import page's "Music" card: pick a folder with the platform's native
//! directory dialog, then publish every track under it into the catalog.
//!
//! Music belongs in the asset store like every other kind of content — once
//! it is there the DJ/VJ app lists it through the ordinary `kind=audio`
//! catalog query, with no second library and no second path. The walk,
//! metadata, tagging and publication all live in
//! `makepad_asset_importer::music_import`; this file is the UI half: a picked
//! path, a worker thread, and the same [`ImportPhase`] progress protocol the
//! pack imports already speak, so the shared queue row renders it unchanged.

use crate::import::{ImportPhase, ServerSession};
use makepad_asset_client::{AssetClient, ClientConfig};
use makepad_asset_importer::music_import::{self, MusicReport};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

/// The card's whole state: which folder, what the worker is doing, and the
/// one-line result of the last run.
pub struct MusicImportPage {
    /// Folder the user picked. Empty until the dialog answers — the card
    /// cannot start without one.
    pub dir: String,
    /// True between opening the dialog and its answer, so the button can say
    /// so instead of looking dead.
    pub picking: bool,
    pub phase: ImportPhase,
    /// Human summary of the finished run, kept after the phase goes idle.
    pub summary: String,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<MusicMsg>>,
    /// The finished run, kept so the card can name skips and failures rather
    /// than only a happy count.
    pub last_report: Option<MusicReport>,
    /// "split audio layers" as it stood when the run was QUEUED. Read at
    /// queue time, not at finish time, so a toggle flipped while 200 tracks
    /// import cannot retroactively change what the run promised.
    pub split_layers: bool,
    /// "+ lyrics", only meaningful with `split_layers`.
    pub bake_lyrics: bool,
    /// Aliases the finished run landed, waiting to be handed to the
    /// analysis queue. Non-empty only when `split_layers` was on.
    pending_analysis: Vec<String>,
}

/// What the worker sends back: live progress, then exactly one verdict.
pub enum MusicMsg {
    Phase(ImportPhase),
    Done(ImportPhase, Box<MusicReport>),
}

impl Default for MusicImportPage {
    fn default() -> Self {
        Self {
            dir: String::new(),
            picking: false,
            phase: ImportPhase::Idle,
            summary: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            rx: None,
            last_report: None,
            split_layers: false,
            bake_lyrics: false,
            pending_analysis: Vec::new(),
        }
    }
}

/// Namespace imported music lands in. `music_import` collapses the repeated
/// class segment, so an alias reads `music/<artist>/<title>`.
pub const MUSIC_NAMESPACE: &str = "music";

impl MusicImportPage {
    /// The job this card would enqueue right now, or `None` while no folder
    /// has been picked.
    pub fn job(&self) -> Option<crate::import::ImportJob> {
        (!self.dir.trim().is_empty()).then(|| crate::import::ImportJob::Music {
            path: self.dir.trim().to_string(),
        })
    }

    /// What the card shows beside the buttons: the picked folder, or the
    /// invitation to pick one.
    pub fn dir_label(&self) -> String {
        if self.picking {
            return "choosing folder…".into();
        }
        if self.dir.trim().is_empty() {
            return "no folder chosen".into();
        }
        self.dir.clone()
    }

    pub fn set_dir(&mut self, dir: impl Into<String>) {
        self.dir = dir.into();
        self.picking = false;
    }

    pub fn compiling(&self) -> bool {
        matches!(
            self.phase,
            ImportPhase::Compiling { .. } | ImportPhase::Publishing { .. }
        )
    }

    pub fn request_stop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// One honest line for the queue row.
    pub fn status_line(&self, server_connected: bool) -> String {
        match &self.phase {
            ImportPhase::Idle => {
                if self.summary.is_empty() {
                    if server_connected {
                        "ready".into()
                    } else {
                        "no Asset Server — cannot publish".into()
                    }
                } else {
                    self.summary.clone()
                }
            }
            ImportPhase::Compiling {
                done,
                total,
                current,
                ..
            } => {
                if *total == 0 {
                    "walking the folder…".into()
                } else if current.is_empty() {
                    format!("reading tags · {done}/{total}")
                } else {
                    format!("reading tags · {done}/{total} · {current}")
                }
            }
            ImportPhase::Publishing {
                assets, blob_done, ..
            } => format!("publishing · {blob_done}/{assets}"),
            ImportPhase::Published { assets, .. } => format!("published {assets} tracks"),
            ImportPhase::Failed { message, .. } => message.clone(),
            ImportPhase::Cancelled { message, .. } => message.clone(),
            other => format!("{other:?}"),
        }
    }

    /// Monotonic 0..1 for the queue row's bar, in two honest bands: the
    /// metadata read pass owns 0.02..0.20, the publish pass 0.20..1.00. One
    /// bar that filled twice would be a lie about how much work is left.
    pub fn progress_fraction(&self) -> f32 {
        match &self.phase {
            ImportPhase::Compiling { done, total, .. } => {
                if *total == 0 {
                    0.02
                } else {
                    0.02 + 0.18 * (*done as f32 / *total as f32)
                }
            }
            ImportPhase::Publishing {
                assets, blob_done, ..
            } => {
                if *assets == 0 {
                    0.20
                } else {
                    0.20 + 0.80 * (*blob_done as f32 / *assets as f32)
                }
            }
            ImportPhase::Published { .. } => 1.0,
            ImportPhase::Failed { .. } | ImportPhase::Cancelled { .. } => 1.0,
            _ => 0.0,
        }
    }

    /// Start the worker. `server` absent = refuse rather than pretend: this
    /// import has no local-only half, it publishes or it does not run.
    ///
    /// Every refusal is ALSO written to `summary`. The queue row for a job
    /// that never started is removed on the next tick, so a refusal that only
    /// returned `Err` would show the user nothing at all — "Load does
    /// nothing" is the one failure mode this card must never have.
    pub fn start_import(
        &mut self,
        path: String,
        server: Option<ServerSession>,
    ) -> Result<(), String> {
        let dir = PathBuf::from(path.trim());
        if !dir.is_dir() {
            return Err(self.refuse(format!("{} is not a folder", dir.display())));
        }
        let Some(server) = server else {
            return Err(self.refuse(
                "no Asset Server session yet — wait for the store to come up, then Load again"
                    .into(),
            ));
        };
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.summary.clear();
        self.last_report = None;
        self.phase = ImportPhase::Compiling {
            pack: "Music".into(),
            done: 0,
            total: 0,
            current: String::new(),
        };
        thread::Builder::new()
            .name("asset-ui-music-import".into())
            .spawn(move || {
                let msg = run_music_import(&dir, server, &tx, &cancel);
                let _ = tx.send(msg);
            })
            .map_err(|e| self.refuse(format!("failed to start music import thread: {e}")))?;
        Ok(())
    }

    /// Record a refusal where the card can show it, and hand the same text
    /// back to the caller for the log.
    fn refuse(&mut self, message: String) -> String {
        self.phase = ImportPhase::Failed {
            pack: "Music".into(),
            message: message.clone(),
        };
        self.summary = message.clone();
        self.rx = None;
        message
    }

    /// Drain the worker channel. Returns true when anything changed, so the
    /// caller only redraws on real news.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(rx) = &self.rx else { break };
            match rx.try_recv() {
                Ok(msg) => {
                    self.ingest(msg);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    if self.compiling() {
                        // The thread died without a verdict. Say so; never
                        // leave a bar spinning at a lie.
                        self.phase = ImportPhase::Failed {
                            pack: "Music".into(),
                            message: "music import thread exited without a result".into(),
                        };
                        self.summary = "music import thread exited without a result".into();
                    }
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    /// Tracks this run landed that the caller asked to have analysed, once.
    /// Aliases, not ids — the importer reports aliases and the bake lane
    /// resolves them; the UI thread never blocks on a lookup.
    pub fn take_pending_analysis(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_analysis)
    }

    /// Every track the run left in the catalog under this folder: newly
    /// published, re-published, and the ones already there byte-for-byte.
    /// The bake is idempotent (a track that already carries stems is
    /// skipped without touching the GPU), so including the unchanged ones is
    /// what makes "import again with the switch on" analyse the library.
    fn analysis_batch(report: &MusicReport) -> Vec<String> {
        let mut aliases = Vec::with_capacity(report.landed());
        aliases.extend(report.published.iter().cloned());
        aliases.extend(report.updated.iter().cloned());
        aliases.extend(report.unchanged.iter().cloned());
        aliases
    }

    fn ingest(&mut self, msg: MusicMsg) {
        let phase = match msg {
            MusicMsg::Phase(phase) => phase,
            MusicMsg::Done(phase, report) => {
                // Only a run that was QUEUED with the switch on hands its
                // tracks to the bake, and only when it actually landed some.
                if self.split_layers {
                    self.pending_analysis = Self::analysis_batch(&report);
                }
                // The verdict line names every outcome, so a run that
                // published nothing cannot read as a clean one.
                self.summary = match &phase {
                    ImportPhase::Failed { message, .. }
                    | ImportPhase::Cancelled { message, .. } => message.clone(),
                    _ => summarise(&report),
                };
                self.last_report = Some(*report);
                self.rx = None;
                phase
            }
        };
        if let ImportPhase::Failed { message, .. } = &phase {
            self.summary = message.clone();
            self.rx = None;
        }
        self.phase = phase;
    }
}

/// Worker body: connect as an ordinary client and run the shared importer.
fn run_music_import(
    dir: &std::path::Path,
    server: ServerSession,
    tx: &mpsc::Sender<MusicMsg>,
    cancel: &Arc<AtomicBool>,
) -> MusicMsg {
    let cache = crate::asset_store_state::asset_ui_home().join("music-import-cache");
    let mut config = ClientConfig::new(cache);
    config.token = Some(server.token.clone());
    let mut client =
        match AssetClient::connect(config, server.endpoints, Some(server.server_id)) {
            Ok(client) => client,
            Err(error) => {
                return MusicMsg::Done(
                    ImportPhase::Failed {
                        pack: "Music".into(),
                        message: format!("asset client: {error}"),
                    },
                    Box::default(),
                )
            }
        };
    let rights = music_import::personal_library_rights(dir);
    // One channel message per file would flood a 5000-track library; the UI
    // polls at 5 Hz, so send the first, the last, and roughly one per
    // percent in between.
    let mut last_sent = usize::MAX;
    let mut progress = |p: music_import::MusicProgress| {
        let step = (p.total / 100).max(1);
        let boundary = p.done == 0 || p.done + 1 >= p.total || p.done % step == 0;
        if !boundary && last_sent != usize::MAX {
            return;
        }
        last_sent = p.done;
        let phase = match p.stage {
            music_import::MusicStage::Reading => ImportPhase::Compiling {
                pack: "Music".into(),
                done: p.done,
                total: p.total,
                current: p.current.to_string(),
            },
            music_import::MusicStage::Publishing => ImportPhase::Publishing {
                pack: "Music".into(),
                assets: p.total,
                blobs: p.total,
                blob_done: p.done,
            },
        };
        let _ = tx.send(MusicMsg::Phase(phase));
    };
    let stop = || cancel.load(Ordering::SeqCst);
    match music_import::import_music(
        &mut client,
        dir,
        MUSIC_NAMESPACE,
        &rights,
        false,
        &mut progress,
        &stop,
    ) {
        Ok(report) if report.cancelled => MusicMsg::Done(
            ImportPhase::Cancelled {
                pack: "Music".into(),
                message: format!("cancelled · {}", summarise(&report)),
            },
            Box::new(report),
        ),
        Ok(report) => {
            let phase = ImportPhase::Published {
                pack: "Music".into(),
                assets: report.landed(),
                blobs: report.landed(),
                created: !report.published.is_empty(),
                annotated: report.landed(),
                out: dir.to_path_buf(),
                library: Vec::new(),
                bake: Default::default(),
            };
            MusicMsg::Done(phase, Box::new(report))
        }
        Err(error) => MusicMsg::Done(
            ImportPhase::Failed {
                pack: "Music".into(),
                message: error,
            },
            Box::default(),
        ),
    }
}

/// One line naming every outcome that is not "published fine", so a run with
/// skips or failures cannot look like a clean one.
pub fn summarise(report: &MusicReport) -> String {
    let mut parts = vec![format!("{} new", report.published.len())];
    if !report.updated.is_empty() {
        parts.push(format!("{} updated", report.updated.len()));
    }
    if !report.unchanged.is_empty() {
        parts.push(format!("{} unchanged", report.unchanged.len()));
    }
    if !report.skipped.is_empty() {
        parts.push(format!("{} skipped", report.skipped.len()));
    }
    if !report.failed.is_empty() {
        parts.push(format!("{} failed", report.failed.len()));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::ImportJob;

    #[test]
    fn no_folder_means_no_job() {
        let mut page = MusicImportPage::default();
        assert!(page.job().is_none());
        assert_eq!(page.dir_label(), "no folder chosen");
        page.picking = true;
        assert_eq!(page.dir_label(), "choosing folder…");
        page.set_dir("/Users/me/Music");
        assert!(!page.picking);
        assert_eq!(page.dir_label(), "/Users/me/Music");
        assert_eq!(
            page.job(),
            Some(ImportJob::Music {
                path: "/Users/me/Music".into()
            })
        );
    }

    #[test]
    fn two_folders_are_two_jobs_but_one_folder_queues_once() {
        let a = ImportJob::Music { path: "/a".into() };
        let b = ImportJob::Music { path: "/b".into() };
        assert!(a.conflicts(&a));
        assert!(!a.conflicts(&b));
        assert_eq!(a.title(), "Music · a");
    }

    #[test]
    fn start_refuses_a_missing_folder_and_a_missing_server() {
        let mut page = MusicImportPage::default();
        let error = page
            .start_import("/definitely/not/here".into(), None)
            .expect_err("missing folder");
        assert!(error.contains("not a folder"), "{error}");
        let error = page
            .start_import(std::env::temp_dir().to_string_lossy().into_owned(), None)
            .expect_err("missing server");
        assert!(error.contains("Asset Server"), "{error}");
        // Refusing must not leave the card looking busy.
        assert!(!page.compiling());
    }

    #[test]
    fn progress_is_monotonic_and_ends_at_one() {
        let mut page = MusicImportPage::default();
        assert_eq!(page.progress_fraction(), 0.0);
        page.phase = ImportPhase::Compiling {
            pack: "Music".into(),
            done: 0,
            total: 0,
            current: String::new(),
        };
        let scanning = page.progress_fraction();
        page.phase = ImportPhase::Compiling {
            pack: "Music".into(),
            done: 5,
            total: 10,
            current: "Strange".into(),
        };
        let half = page.progress_fraction();
        assert!(scanning < half && half < 1.0, "{scanning} {half}");
        assert!(page.status_line(true).contains("5/10"));
        page.phase = ImportPhase::Failed {
            pack: "Music".into(),
            message: "boom".into(),
        };
        assert_eq!(page.progress_fraction(), 1.0);
        assert_eq!(page.status_line(true), "boom");
    }

    #[test]
    fn a_dead_worker_becomes_a_visible_failure() {
        let mut page = MusicImportPage::default();
        let (tx, rx) = mpsc::channel();
        page.rx = Some(rx);
        page.phase = ImportPhase::Compiling {
            pack: "Music".into(),
            done: 0,
            total: 3,
            current: String::new(),
        };
        drop(tx);
        assert!(page.poll());
        assert!(matches!(page.phase, ImportPhase::Failed { .. }));
        assert!(page.summary.contains("without a result"));
    }

    /// "split audio layers" hands the finished run's tracks to the bake —
    /// and a run queued WITHOUT it hands over nothing, ever.
    #[test]
    fn only_a_run_queued_with_the_switch_on_feeds_the_bake() {
        let report = MusicReport {
            published: vec!["music/a/one".into()],
            updated: vec!["music/a/two".into()],
            unchanged: vec!["music/a/three".into()],
            failed: vec![("music/a/four".into(), "boom".into())],
            skipped: vec![("music/a/five".into(), "flac".into())],
            cancelled: false,
        };

        let mut off = MusicImportPage::default();
        off.ingest(MusicMsg::Done(
            ImportPhase::Idle,
            Box::new(report.clone()),
        ));
        assert!(
            off.take_pending_analysis().is_empty(),
            "the switch is off: the import publishes and stops there"
        );

        let mut on = MusicImportPage::default();
        on.split_layers = true;
        on.bake_lyrics = true;
        on.ingest(MusicMsg::Done(ImportPhase::Idle, Box::new(report)));
        let batch = on.take_pending_analysis();
        // Everything that LANDED, including the byte-identical re-imports:
        // the bake skips a track that already carries stems without
        // touching the GPU, so re-importing is how a library gets analysed.
        assert_eq!(
            batch,
            vec![
                "music/a/one".to_string(),
                "music/a/two".to_string(),
                "music/a/three".to_string()
            ]
        );
        // Failed and skipped files are not in the catalog; they cannot be.
        assert!(!batch.iter().any(|alias| alias.ends_with("four")));
        assert!(!batch.iter().any(|alias| alias.ends_with("five")));
        // Handed over exactly once.
        assert!(on.take_pending_analysis().is_empty());
    }

    #[test]
    fn summary_names_every_non_clean_outcome() {
        let report = MusicReport {
            published: vec!["a".into()],
            updated: vec!["b".into()],
            unchanged: vec!["c".into()],
            failed: vec![("d".into(), "boom".into())],
            skipped: vec![("e".into(), "unsupported".into())],
            cancelled: false,
        };
        let line = summarise(&report);
        assert!(line.contains("1 new"), "{line}");
        assert!(line.contains("1 updated"), "{line}");
        assert!(line.contains("1 unchanged"), "{line}");
        assert!(line.contains("1 skipped"), "{line}");
        assert!(line.contains("1 failed"), "{line}");
        assert_eq!(report.landed(), 3);
    }
}
