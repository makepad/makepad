//! IMPORT MUSIC: the DJ page's importer, its worker thread, and the one
//! line of status it shows.
//!
//! The VJ page's importer (`import_ui.rs`) publishes media BY REFERENCE
//! with a filename for a title — right for a wall of clips, wrong for a
//! track list with ARTIST and TIME columns. This one drives the shared
//! music importer instead: ID3 and Vorbis tags, a measured duration, a
//! baked waveform picture, and the `music` tag the deck explorer filters
//! on. A track imported here is a track the DJ library can actually show.
//!
//! The shape is deliberately the same as the VJ importer's — worker
//! thread, mpsc channel, a `poll()` drained on the UI's existing tick, one
//! cancel flag — because the reasons for it are the same: a library is
//! gigabytes, decoding an MP3 is seconds, and the show keeps running while
//! it imports.
//!
//! A worker that dies without a verdict becomes a VISIBLE FAILURE rather
//! than a status line that waits forever: the channel disconnecting
//! mid-run is itself the error.

use makepad_asset_client::{
    ApiEndpoints, AssetClient, ClientConfig, PublishRequest, StoreCapabilities,
};
use makepad_asset_data::BlobId;
use makepad_asset_importer::music_import::{
    self, MusicProgress, MusicReport, MusicStage, TrackOutcome,
};
use makepad_widgets::makepad_platform::file_dialogs::VirtualFile;
use makepad_widgets::makepad_platform::thread::ThreadSpawner;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

/// Namespace imported tracks land in. The asset UI's music importer uses
/// the same one, so a library filled from either app is one library.
pub const MUSIC_NAMESPACE: &str = "music";

/// Most failure notes kept. A folder of ten thousand broken files must not
/// become ten thousand lines of UI state.
const MAX_NOTES: usize = 24;

/// Keep one selected song comfortably below the platform's broad virtual
/// file ceiling. It is held encoded while its thumbnail is baked and again
/// while the publish request is in flight.
pub const MAX_MUSIC_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PreparedMusicImport {
    pub name: String,
    pub request: PublishRequest,
}

/// What the importer is doing right now. The two working phases are the
/// importer's own two passes over the tree, reported separately so one bar
/// never fills twice.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MusicImportPhase {
    #[default]
    Idle,
    Reading { done: usize, total: usize, current: String },
    Publishing { done: usize, total: usize, current: String },
    Done(MusicImportSummary),
    Failed(String),
    Cancelled(MusicImportSummary),
}

impl MusicImportPhase {
    pub fn busy(&self) -> bool {
        matches!(self, MusicImportPhase::Reading { .. } | MusicImportPhase::Publishing { .. })
    }
}

/// The counts a finished (or abandoned) run produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MusicImportSummary {
    pub published: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Bounded per-file reasons, verbatim. A run where "2 skipped" is all
    /// you get is a run you cannot act on; these are the sentences that
    /// say which files and why.
    pub notes: Vec<String>,
}

impl MusicImportSummary {
    fn from_report(report: &MusicReport) -> MusicImportSummary {
        let notes = report
            .skipped
            .iter()
            .chain(report.failed.iter())
            .take(MAX_NOTES)
            .map(|(rel, why)| format!("{rel}: {why}"))
            .collect();
        MusicImportSummary {
            published: report.published.len(),
            updated: report.updated.len(),
            unchanged: report.unchanged.len(),
            failed: report.failed.len(),
            skipped: report.skipped.len(),
            notes,
        }
    }

    fn landed(&self) -> usize {
        self.published + self.updated + self.unchanged
    }
}

/// One line for the status label beside the IMPORT button. Idle is empty:
/// an importer that has never run has nothing to say, and the row it sits
/// in is already full of controls.
pub fn describe(phase: &MusicImportPhase) -> String {
    match phase {
        MusicImportPhase::Idle => String::new(),
        MusicImportPhase::Reading { done, total, .. } => {
            if *total == 0 {
                "import: reading…".to_string()
            } else {
                format!("import: reading {done}/{total}")
            }
        }
        MusicImportPhase::Publishing { done, total, .. } => {
            format!("import: publishing {done}/{total}")
        }
        MusicImportPhase::Done(summary) => summary_line("imported", summary),
        MusicImportPhase::Cancelled(summary) => summary_line("stopped", summary),
        MusicImportPhase::Failed(why) => format!("import failed: {why}"),
    }
}

fn summary_line(verb: &str, summary: &MusicImportSummary) -> String {
    let mut line = format!("{verb} {}", summary.landed());
    if summary.failed > 0 {
        line.push_str(&format!(" · {} failed", summary.failed));
    }
    if summary.skipped > 0 {
        line.push_str(&format!(" · {} skipped", summary.skipped));
    }
    if summary.landed() == 0 {
        if let Some(note) = summary.notes.first() {
            line.push_str(" · ");
            line.push_str(note);
        }
    }
    line
}

enum Msg {
    Phase(MusicImportPhase),
    Prepared {
        imports: VecDeque<PreparedMusicImport>,
        summary: MusicImportSummary,
        cancelled: bool,
    },
    Finished(MusicImportPhase),
}

/// The single gate used before any stem fetch or local separation is
/// submitted. Store capability wins over the deck's persisted mode.
pub fn stems_may_run(capabilities: StoreCapabilities, mode: crate::decks::ProcessMode) -> bool {
    capabilities.ai && mode.computes()
}

/// The DJ importer's whole state.
#[derive(Default)]
pub struct MusicImporter {
    pub phase: MusicImportPhase,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Msg>>,
    prepared: VecDeque<PreparedMusicImport>,
    prepared_inflight: bool,
    prepared_done: usize,
    prepared_total: usize,
    prepared_summary: Option<MusicImportSummary>,
}

impl MusicImporter {
    pub fn busy(&self) -> bool {
        self.phase.busy()
    }

    pub fn status(&self) -> String {
        describe(&self.phase)
    }

    /// The per-file reasons a finished run collected, for the log.
    pub fn notes(&self) -> &[String] {
        match &self.phase {
            MusicImportPhase::Done(summary) | MusicImportPhase::Cancelled(summary) => {
                &summary.notes
            }
            _ => &[],
        }
    }

    /// Begin an import of exactly these files and folders. The session
    /// details come from the live connection, so the worker publishes into
    /// the same store the explorer is listing — embedded or remote.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(
        &mut self,
        paths: Vec<PathBuf>,
        endpoints: ApiEndpoints,
        server_id: [u8; 16],
        token: Option<String>,
        cache_parent: PathBuf,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("an import is already running".to_string());
        }
        if paths.is_empty() {
            return Err(self.refuse("nothing to import"));
        }
        let Some(token) = token else {
            return Err(self.refuse("no asset server session yet"));
        };
        self.cancel = Arc::new(AtomicBool::new(false));
        self.clear_prepared();
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.phase = MusicImportPhase::Reading { done: 0, total: 0, current: String::new() };
        let cache = cache_parent.join("music-import-cache");
        thread::Builder::new()
            .name("vj-music-import".into())
            .spawn(move || {
                let verdict = run(&paths, endpoints, server_id, token, cache, &tx, &cancel);
                let _ = tx.send(Msg::Finished(verdict));
            })
            .map_err(|e| self.refuse(format!("cannot start the import thread: {e}")))?;
        Ok(())
    }

    /// Prepare browser/file-dialog bytes for publication through the
    /// app's already connected client runtime. This is the same importer
    /// bake and request builder as the path worker; only the filesystem read
    /// at the front is absent.
    pub fn start_files(
        &mut self,
        files: Vec<VirtualFile>,
        spawner: ThreadSpawner,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("an import is already running".to_string());
        }
        if files.is_empty() {
            return Err(self.refuse("nothing to import"));
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        self.clear_prepared();
        self.phase = MusicImportPhase::Reading {
            done: 0,
            total: files.len(),
            current: String::new(),
        };
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        launch_file_preparation(files, tx, cancel, spawner)
            .map_err(|error| self.refuse(format!("cannot start the import: {error}")))?;
        Ok(())
    }

    pub fn take_prepared(&mut self) -> Option<PreparedMusicImport> {
        if self.prepared_inflight {
            return None;
        }
        let prepared = self.prepared.pop_front()?;
        self.prepared_inflight = true;
        self.phase = MusicImportPhase::Publishing {
            done: self.prepared_done,
            total: self.prepared_total,
            current: prepared.name.clone(),
        };
        Some(prepared)
    }

    pub fn prepared_settled(&mut self, outcome: TrackOutcome) {
        let Some(summary) = self.prepared_summary.as_mut() else { return };
        match outcome {
            TrackOutcome::Published => summary.published += 1,
            TrackOutcome::Updated => summary.updated += 1,
            TrackOutcome::Unchanged => summary.unchanged += 1,
        }
        self.prepared_done += 1;
        self.prepared_inflight = false;
        if self.prepared_done >= self.prepared_total {
            self.finish_prepared();
        }
    }

    pub fn prepared_failed(&mut self, name: &str, error: impl AsRef<str>) {
        let Some(summary) = self.prepared_summary.as_mut() else { return };
        summary.failed += 1;
        push_note(summary, name, error.as_ref());
        self.prepared_done += 1;
        self.prepared_inflight = false;
        if self.prepared_done >= self.prepared_total {
            self.finish_prepared();
        }
    }

    pub fn cancel(&mut self) {
        if self.busy() {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Drain the worker. True when anything changed, so the caller only
    /// repaints when there is something new.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(rx) = &self.rx else { break };
            match rx.try_recv() {
                Ok(Msg::Phase(phase)) => {
                    self.phase = phase;
                    changed = true;
                }
                Ok(Msg::Prepared { imports, summary, cancelled }) => {
                    self.rx = None;
                    self.prepared = imports;
                    self.prepared_total = self.prepared.len();
                    self.prepared_summary = Some(summary);
                    if cancelled {
                        let summary = self.prepared_summary.take().unwrap_or_default();
                        self.phase = MusicImportPhase::Cancelled(summary);
                        self.prepared.clear();
                    } else if self.prepared_total == 0 {
                        self.finish_prepared();
                    } else {
                        self.phase = MusicImportPhase::Publishing {
                            done: 0,
                            total: self.prepared_total,
                            current: String::new(),
                        };
                    }
                    changed = true;
                    break;
                }
                Ok(Msg::Finished(phase)) => {
                    self.phase = phase;
                    self.rx = None;
                    changed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    if self.phase.busy() {
                        self.phase = MusicImportPhase::Failed(
                            "the import thread exited without a verdict".to_string(),
                        );
                    }
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    fn refuse(&mut self, why: impl Into<String>) -> String {
        let why = why.into();
        self.phase = MusicImportPhase::Failed(why.clone());
        why
    }

    fn clear_prepared(&mut self) {
        self.prepared.clear();
        self.prepared_inflight = false;
        self.prepared_done = 0;
        self.prepared_total = 0;
        self.prepared_summary = None;
    }

    fn finish_prepared(&mut self) {
        let summary = self.prepared_summary.take().unwrap_or_default();
        self.phase = MusicImportPhase::Done(summary);
        self.prepared.clear();
        self.prepared_inflight = false;
    }
}

fn push_note(summary: &mut MusicImportSummary, name: &str, error: &str) {
    if summary.notes.len() < MAX_NOTES {
        summary.notes.push(format!("{name}: {error}"));
    }
}

fn launch_file_preparation(
    files: Vec<VirtualFile>,
    tx: mpsc::Sender<Msg>,
    cancel: Arc<AtomicBool>,
    spawner: ThreadSpawner,
) -> Result<(), String> {
    spawner
        .spawn(move || {
            let (imports, summary, cancelled) = prepare_files(files, &tx, &cancel);
            let _ = tx.send(Msg::Prepared {
                imports,
                summary,
                cancelled,
            });
        })
        .map(|handle| handle.detach())
        .map_err(|error| error.to_string())
}

fn prepare_files(
    files: Vec<VirtualFile>,
    tx: &mpsc::Sender<Msg>,
    cancel: &AtomicBool,
) -> (VecDeque<PreparedMusicImport>, MusicImportSummary, bool) {
    let total = files.len();
    let rights = music_import::personal_library_rights(std::path::Path::new("selected-files"));
    let mut imports = VecDeque::new();
    let mut summary = MusicImportSummary::default();
    let mut digests = HashSet::new();
    for (index, file) in files.into_iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return (imports, summary, true);
        }
        let _ = tx.send(Msg::Phase(MusicImportPhase::Reading {
            done: index,
            total,
            current: file.name.clone(),
        }));
        let byte_len = u64::try_from(file.bytes.len()).unwrap_or(u64::MAX);
        let error = if file.size != byte_len {
            Some(format!("size says {} bytes, received {byte_len}", file.size))
        } else if byte_len > MAX_MUSIC_FILE_BYTES {
            Some(format!(
                "too big: {byte_len} bytes (limit {MAX_MUSIC_FILE_BYTES})"
            ))
        } else {
            None
        };
        if let Some(error) = error {
            summary.failed += 1;
            push_note(&mut summary, &file.name, &error);
            continue;
        }
        if music_import::Container::from_name_mime(&file.name, &file.mime).is_none() {
            summary.skipped += 1;
            push_note(&mut summary, &file.name, "unsupported audio format");
            continue;
        }
        let digest = BlobId::hash_of(&file.bytes);
        if !digests.insert(digest) {
            summary.skipped += 1;
            push_note(&mut summary, &file.name, "duplicate file in this import");
            continue;
        }
        match music_import::publish_request_from_bytes(
            &file.name,
            &file.mime,
            file.bytes.to_vec(),
            MUSIC_NAMESPACE,
            &rights,
        ) {
            Ok(request) => imports.push_back(PreparedMusicImport {
                name: file.name,
                request,
            }),
            Err(error) => {
                summary.failed += 1;
                push_note(&mut summary, &file.name, &error);
            }
        }
    }
    (imports, summary, false)
}

/// The worker body. Everything expensive lives here.
#[cfg(not(target_arch = "wasm32"))]
fn run(
    paths: &[PathBuf],
    endpoints: ApiEndpoints,
    server_id: [u8; 16],
    token: String,
    cache: PathBuf,
    tx: &mpsc::Sender<Msg>,
    cancel: &Arc<AtomicBool>,
) -> MusicImportPhase {
    let mut config = ClientConfig::new(cache);
    config.token = Some(token);
    let mut client = match AssetClient::connect(config, endpoints, Some(server_id)) {
        Ok(client) => client,
        Err(error) => return MusicImportPhase::Failed(format!("asset server: {error}")),
    };
    let scan = music_import::scan_music_paths(paths);
    if scan.files.is_empty() && scan.skipped.is_empty() {
        return MusicImportPhase::Failed("no audio files in what you dropped".to_string());
    }
    let rights = music_import::personal_library_rights(&common_root(paths));
    // The UI polls at 20 Hz; a five-thousand-track library must not send
    // five thousand messages. First, last, and about one per percent.
    let mut last_sent = usize::MAX;
    let mut progress = |p: MusicProgress| {
        let step = (p.total / 100).max(1);
        let boundary = p.done == 0 || p.done + 1 >= p.total || p.done % step == 0;
        if !boundary && last_sent != usize::MAX {
            return;
        }
        last_sent = p.done;
        let phase = match p.stage {
            MusicStage::Reading => MusicImportPhase::Reading {
                done: p.done,
                total: p.total,
                current: p.current.to_string(),
            },
            MusicStage::Publishing => MusicImportPhase::Publishing {
                done: p.done,
                total: p.total,
                current: p.current.to_string(),
            },
        };
        let _ = tx.send(Msg::Phase(phase));
    };
    let stop = || cancel.load(Ordering::SeqCst);
    match music_import::import_music_scan(
        &mut client,
        &scan,
        MUSIC_NAMESPACE,
        &rights,
        false,
        &mut progress,
        &stop,
    ) {
        Ok(report) if report.cancelled => {
            MusicImportPhase::Cancelled(MusicImportSummary::from_report(&report))
        }
        Ok(report) => MusicImportPhase::Done(MusicImportSummary::from_report(&report)),
        Err(error) => MusicImportPhase::Failed(error),
    }
}

/// The folder a set of dropped paths shares. It names the library in the
/// published rights and nothing else, so "close enough to be honest" is
/// the whole requirement: a file's own folder, or the deepest folder every
/// path sits under.
#[cfg(not(target_arch = "wasm32"))]
fn common_root(paths: &[PathBuf]) -> PathBuf {
    let mut folders = paths.iter().map(|path| {
        if path.is_dir() {
            path.clone()
        } else {
            path.parent().map(|dir| dir.to_path_buf()).unwrap_or_else(|| path.clone())
        }
    });
    let Some(mut root) = folders.next() else {
        return PathBuf::new();
    };
    for folder in folders {
        while !folder.starts_with(&root) {
            let Some(parent) = root.parent() else {
                return root;
            };
            root = parent.to_path_buf();
        }
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_data::MediaType;
    use makepad_audio_encode::{encode_vorbis, EncodeOptions};

    fn virtual_file(name: &str, mime: &str, bytes: Vec<u8>) -> VirtualFile {
        VirtualFile {
            name: name.into(),
            mime: mime.into(),
            size: bytes.len() as u64,
            bytes: Arc::from(bytes),
        }
    }

    fn mp3_file() -> Vec<u8> {
        let mut frame = vec![0xFF, 0xFB, 0x90, 0x04];
        frame.resize(417, 0);
        frame
    }

    fn wav_file() -> Vec<u8> {
        let rate = 8_000u32;
        let mut data = Vec::new();
        for sample in 0..800i16 {
            data.extend_from_slice(&sample.wrapping_mul(31).to_le_bytes());
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn ogg_file() -> Vec<u8> {
        let pcm: Vec<f32> = (0..2_048)
            .map(|sample| ((sample as f32) * 0.04).sin() * 0.25)
            .collect();
        encode_vorbis(
            8_000,
            1,
            &pcm,
            &EncodeOptions { threads: 1, ..Default::default() },
        )
        .unwrap()
    }

    fn prepared(files: Vec<VirtualFile>) -> (VecDeque<PreparedMusicImport>, MusicImportSummary) {
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let (imports, summary, cancelled) = prepare_files(files, &tx, &cancel);
        assert!(!cancelled);
        (imports, summary)
    }

    #[test]
    fn a_reading_phase_names_its_half_of_the_run() {
        assert_eq!(
            describe(&MusicImportPhase::Reading { done: 40, total: 200, current: "a.mp3".into() }),
            "import: reading 40/200"
        );
        // Before the walk has finished there is no total to divide by.
        assert_eq!(
            describe(&MusicImportPhase::Reading { done: 0, total: 0, current: String::new() }),
            "import: reading…"
        );
        assert_eq!(
            describe(&MusicImportPhase::Publishing { done: 12, total: 200, current: "Strange".into() }),
            "import: publishing 12/200"
        );
    }

    #[test]
    fn a_finished_run_counts_every_outcome() {
        let summary = MusicImportSummary {
            published: 190,
            updated: 5,
            unchanged: 3,
            failed: 1,
            skipped: 2,
            notes: Vec::new(),
        };
        assert_eq!(describe(&MusicImportPhase::Done(summary)), "imported 198 · 1 failed · 2 skipped");
    }

    #[test]
    fn a_clean_run_says_only_what_landed() {
        let summary = MusicImportSummary { published: 4, ..Default::default() };
        assert_eq!(describe(&MusicImportPhase::Done(summary)), "imported 4");
    }

    #[test]
    fn an_idle_importer_says_nothing_at_all() {
        assert_eq!(describe(&MusicImportPhase::Idle), "");
        assert!(!MusicImporter::default().busy());
    }

    #[test]
    fn the_provenance_root_is_the_folder_the_paths_share() {
        let base = std::env::temp_dir().join("vj-common-root");
        let deep = base.join("Lib/Artist");
        // Two files under one artist folder: that folder is the root.
        assert_eq!(
            common_root(&[deep.join("a.mp3"), deep.join("b.mp3")]),
            deep
        );
        // Files under different folders fall back to what they share.
        assert_eq!(
            common_root(&[deep.join("a.mp3"), base.join("Lib/loose.mp3")]),
            base.join("Lib")
        );
        assert_eq!(common_root(&[]), std::path::PathBuf::new());
    }

    #[test]
    fn byte_first_import_prepares_mp3_ogg_and_wav_publish_requests() {
        let (imports, summary) = prepared(vec![
            virtual_file("first.mp3", "audio/mpeg", mp3_file()),
            virtual_file("second.ogg", "audio/ogg", ogg_file()),
            virtual_file("third.wav", "audio/wav", wav_file()),
        ]);
        assert_eq!(summary, MusicImportSummary::default());
        assert_eq!(imports.len(), 3);
        let media: Vec<_> = imports
            .iter()
            .map(|prepared| prepared.request.artifact.media)
            .collect();
        assert_eq!(media, [MediaType::Mp3, MediaType::Ogg, MediaType::Wav]);
        assert!(imports.iter().all(|prepared| {
            prepared.request.alias.is_some()
                && prepared.request.tags.iter().any(|tag| tag == "music")
                && prepared.request.artifact.media_millis > 0
        }));
    }

    #[test]
    fn digest_dedup_prepares_the_same_file_only_once() {
        let bytes = wav_file();
        let (imports, summary) = prepared(vec![
            virtual_file("same.wav", "audio/wav", bytes.clone()),
            virtual_file("copy.wav", "audio/wav", bytes),
        ]);
        assert_eq!(imports.len(), 1);
        assert_eq!(summary.skipped, 1);
        assert!(summary.notes[0].contains("duplicate file"));
    }

    #[test]
    fn bad_selected_bytes_become_bounded_status_notes() {
        let (imports, summary) = prepared(vec![
            virtual_file("broken.wav", "audio/wav", b"RIFFnope".to_vec()),
            virtual_file("future.flac", "audio/flac", vec![1, 2, 3]),
        ]);
        assert!(imports.is_empty());
        assert_eq!((summary.failed, summary.skipped), (1, 1));
        assert_eq!(summary.notes.len(), 2);
        assert!(describe(&MusicImportPhase::Done(summary)).contains("broken.wav"));
    }

    #[test]
    fn a_store_without_ai_short_circuits_live_stems() {
        assert!(!stems_may_run(
            StoreCapabilities::browser(),
            crate::decks::ProcessMode::Live,
        ));
        assert!(!stems_may_run(
            StoreCapabilities::native(),
            crate::decks::ProcessMode::Off,
        ));
        assert!(stems_may_run(
            StoreCapabilities::native(),
            crate::decks::ProcessMode::Live,
        ));
    }
}
