//! One bounded file-reader worker for explicitly opened Studio documents.
//! No recursive scanning, UI-thread filesystem access, or shared locks.
//!
//! Reads are event-driven: a watcher consumer calls `notify_changed` and the
//! worker reads the file at once (atomic-replacement aware). Polling remains
//! only as a slow fallback for changes no watcher reported. The worker is
//! the single authority for `FileSnapshot::revision`.

use makepad_code_editor::{document::PreparedDocument, session::PreparedView, CodeDocument, CodeSession};
use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender, TrySendError}, Arc},
    time::{Duration, Instant},
};

pub const MAX_DOCUMENTS: usize = 32;
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PENDING_SAVES: usize = 4;
/// Fallback polling cadence for changes no watcher reported.
pub const FALLBACK_INTERVAL: Duration = Duration::from_secs(5);
/// One retry after an ENOENT read, for a replacement observed mid-rename.
const REPLACEMENT_RETRY: Duration = Duration::from_millis(20);
const MAX_PENDING_COMMANDS: usize = 16;

#[derive(Clone, Debug)]
pub struct FileSnapshot {
    /// The spelling supplied to watch(). Useful for matching a pending open.
    pub requested_path: PathBuf,
    /// Canonical identity, resolved once off-thread. Different worktrees stay distinct.
    pub path: PathBuf,
    /// Monotonic within this worker's lifetime; unchanged reads emit no snapshot.
    pub revision: u64,
    pub text: Option<Arc<String>>,
    pub error: Option<String>,
    /// The watcher event (epoch, seq) that caused this read, when one did.
    pub observed: Option<(u64, u64)>,
}

/// The editor's tokenised document and its unwrapped layout for a
/// snapshot's text, prepared on this worker so the registry admits a cold
/// document without tokenising on the UI thread
/// (`DocumentRegistry::apply_prepared`, `CodeDocument::from_prepared`).
#[derive(Clone)]
pub struct PreparedSnapshot {
    pub document: PreparedDocument,
    pub view: PreparedView,
    /// FileSnapshot revision this preparation was built for. Zero until the
    /// worker stamps it; admission refuses a nonzero mismatch.
    revision: u64,
}

impl PreparedSnapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
}

impl std::fmt::Debug for PreparedSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSnapshot")
            .field("digest", &self.document.digest())
            .field("lines", &self.document.as_text().as_lines().len())
            .field("revision", &self.revision)
            .finish()
    }
}

/// Tokenise and lay out a text (worker-side; the UI only attaches it).
pub fn prepare_snapshot(text: &str) -> PreparedSnapshot {
    let document = CodeDocument::prepare(text.into());
    let view = CodeSession::prepare_view(&document, None);
    PreparedSnapshot { document, view, revision: 0 }
}

/// A snapshot with the editor state prepared for its text (`None` for an
/// unreadable file).
#[derive(Clone, Debug)]
pub struct Delivery {
    pub snapshot: Arc<FileSnapshot>,
    pub prepared: Option<PreparedSnapshot>,
}

enum Command {
    Desired(Arc<Vec<PathBuf>>),
    Notify { path: PathBuf, epoch: u64, seq: u64 },
    /// Re-read and re-prepare even if the last contents match: used when
    /// admission refused a stale or mismatched preparation.
    Refresh(PathBuf),
}

#[derive(Clone, Debug)]
pub struct SaveResult {
    pub request_id: u64,
    pub path: PathBuf,
    pub result: Result<(), String>,
    /// Present together on success, to advance the UI buffer's disk baseline.
    pub text: Option<Arc<String>>,
    pub revision: Option<u64>,
}

struct SaveRequest {
    request_id: u64,
    path: PathBuf,
    expected_disk: Arc<String>,
    text: Arc<String>,
}

pub struct DocumentWorker {
    commands: SyncSender<Command>,
    snapshots: Receiver<Delivery>,
    save_commands: SyncSender<SaveRequest>,
    save_results: Receiver<SaveResult>,
    pending_saves: VecDeque<SaveRequest>,
    outstanding_saves: BTreeMap<u64, PathBuf>,
    next_save_id: u64,
    desired: BTreeSet<PathBuf>,
    identities: BTreeMap<PathBuf, PathBuf>,
    retry: bool,
    /// Notifications not yet admitted to the worker, latest (epoch, seq) per path.
    pending_notify: BTreeMap<PathBuf, (u64, u64)>,
    pending_refresh: BTreeSet<PathBuf>,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
}

impl DocumentWorker {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        Self::start_with_fallback(spawner, FALLBACK_INTERVAL)
    }

    /// `fallback` is the polling cadence for changes no watcher reported.
    pub fn start_with_fallback(spawner: &ThreadSpawner, fallback: Duration) -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        { let _ = (spawner, fallback); return Err("Local file watching is unavailable in this browser".into()); }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (commands, rx) = mpsc::sync_channel(MAX_PENDING_COMMANDS);
            // One slot per watchable document: a full watch set publishes in
            // one wake instead of dripping four snapshots per 50 ms timeout.
            let (tx, snapshots) = mpsc::sync_channel::<Delivery>(MAX_DOCUMENTS);
            let (save_commands, save_rx) = mpsc::sync_channel(MAX_PENDING_SAVES);
            let (save_tx, save_results) = mpsc::sync_channel(MAX_PENDING_SAVES);
            let stop = Arc::new(AtomicBool::new(false));
            let cancel = stop.clone();
            let task = spawner.spawn_worker(
                ThreadOptions { name: Some("studio-documents".into()), ..Default::default() },
                move || run(rx, tx, save_rx, save_tx, cancel, fallback),
            ).map_err(|e| e.to_string())?;
            Ok(Self {
                commands, snapshots, save_commands, save_results,
                pending_saves: VecDeque::new(), outstanding_saves: BTreeMap::new(), next_save_id: 1,
                desired: BTreeSet::new(), identities: BTreeMap::new(), retry: false,
                pending_notify: BTreeMap::new(), pending_refresh: BTreeSet::new(), stop, task,
            })
        }
    }

    /// A watcher consumer reports that `path` changed on disk under the
    /// watcher's (epoch, seq). Only opened documents are read; admission is
    /// nonblocking and the latest notification per path is retained until
    /// the worker accepts it. Unknown paths are ignored, not errors.
    pub fn notify_changed(&mut self, path: PathBuf, epoch: u64, seq: u64) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) || self.task.is_finished() {
            return Err("Document reader is stopped".into());
        }
        let known = self.desired.contains(&path) || self.identities.values().any(|p| p == &path);
        if !known {
            return Ok(());
        }
        let slot = self.pending_notify.entry(path).or_insert((epoch, seq));
        if (epoch, seq) >= *slot {
            *slot = (epoch, seq);
        }
        self.retry_commands();
        Ok(())
    }

    /// Admission is local and nonblocking. A full command queue retains the
    /// latest desired set and retries from poll(); intermediate sets coalesce.
    pub fn watch(&mut self, path: PathBuf) -> Result<(), String> {
        if !path.is_absolute() || path.as_os_str().len() > 4096 {
            return Err("Open an absolute file path of at most 4096 bytes".into());
        }
        if self.stop.load(Ordering::Relaxed) || self.task.is_finished() {
            return Err("Document reader is stopped".into());
        }
        if !self.desired.contains(&path) && self.desired.len() >= MAX_DOCUMENTS {
            return Err(format!("At most {MAX_DOCUMENTS} files can be watched; close a document first"));
        }
        self.desired.insert(path);
        self.retry = true;
        self.retry_commands();
        Ok(())
    }

    pub fn unwatch(&mut self, path: &Path) {
        if self.desired.remove(path) {
            self.identities.remove(path);
            self.pending_notify.remove(path);
            self.pending_refresh.remove(path);
            self.retry = true; self.retry_commands();
        }
    }

    /// Force a re-read and re-prepare of an opened path. Admission uses this
    /// after refusing a mismatched preparation so the worker, not the UI
    /// thread, produces the next attempt.
    pub fn refresh(&mut self, path: &Path) {
        let known = self.desired.contains(path) || self.identities.values().any(|p| p == path)
            || self.identities.contains_key(path);
        if !known {
            return;
        }
        self.pending_refresh.insert(path.to_owned());
        self.retry_commands();
    }

    /// Queue an explicit save of an opened file. Admission, queue retry, and
    /// acknowledgement polling never wait. Full admission is reported to the
    /// caller; accepted requests remain owned here until delivered.
    pub fn save(&mut self, path: PathBuf, expected_disk: Arc<String>, text: Arc<String>) -> Result<u64, String> {
        if self.stop.load(Ordering::Relaxed) || self.task.is_finished() {
            return Err("Document reader is stopped".into());
        }
        if !self.desired.contains(&path) && !self.identities.values().any(|p| p == &path) {
            return Err("Only an explicitly opened document can be saved".into());
        }
        if text.len() > MAX_FILE_BYTES || expected_disk.len() > MAX_FILE_BYTES || text.contains('\0') {
            return Err("Save requires UTF-8 text without NUL bytes, at most 2 MiB".into());
        }
        if self.outstanding_saves.len() >= MAX_PENDING_SAVES {
            return Err("Save queue is full; retry after an outstanding save completes".into());
        }
        let request_id = self.next_save_id;
        self.next_save_id += 1;
        self.outstanding_saves.insert(request_id, path.clone());
        self.pending_saves.push_back(SaveRequest { request_id, path, expected_disk, text });
        self.retry_saves();
        Ok(request_id)
    }

    fn retry_saves(&mut self) {
        while let Some(request) = self.pending_saves.pop_front() {
            match self.save_commands.try_send(request) {
                Ok(()) => {},
                Err(TrySendError::Full(request) | TrySendError::Disconnected(request)) => {
                    self.pending_saves.push_front(request); break;
                }
            }
        }
    }

    pub fn poll_saves(&mut self) -> Vec<SaveResult> {
        self.retry_commands();
        self.retry_saves();
        let mut results = Vec::new();
        while let Ok(result) = self.save_results.try_recv() {
            self.outstanding_saves.remove(&result.request_id);
            results.push(result);
        }
        if self.task.is_finished() {
            // The worker can publish its final result between the first drain
            // and is_finished(). Completion makes this second drain stable.
            while let Ok(result) = self.save_results.try_recv() {
                self.outstanding_saves.remove(&result.request_id);
                results.push(result);
            }
            for (request_id, path) in std::mem::take(&mut self.outstanding_saves) {
                results.push(SaveResult { request_id, path, result: Err("Document reader stopped before acknowledging the save".into()), text: None, revision: None });
            }
            self.pending_saves.clear();
        }
        results
    }

    fn retry_commands(&mut self) {
        if self.retry {
            match self.commands.try_send(Command::Desired(Arc::new(self.desired.iter().cloned().collect()))) {
                Ok(()) => self.retry = false,
                Err(TrySendError::Full(_)) => return,
                Err(TrySendError::Disconnected(_)) => { self.retry = false; return; },
            }
        }
        while let Some((path, (epoch, seq))) = self.pending_notify.pop_first() {
            match self.commands.try_send(Command::Notify { path: path.clone(), epoch, seq }) {
                Ok(()) => {},
                Err(TrySendError::Full(_)) => { self.pending_notify.insert(path, (epoch, seq)); break; },
                Err(TrySendError::Disconnected(_)) => { self.pending_notify.clear(); break; },
            }
        }
        while let Some(path) = self.pending_refresh.pop_first() {
            match self.commands.try_send(Command::Refresh(path.clone())) {
                Ok(()) => {},
                Err(TrySendError::Full(_)) => { self.pending_refresh.insert(path); break; },
                Err(TrySendError::Disconnected(_)) => { self.pending_refresh.clear(); break; },
            }
        }
    }

    /// Drain the snapshot channel: retry outstanding saves, drop paths no
    /// longer desired, and keep only the latest delivery per requested path.
    fn take_deliveries(&mut self) -> Vec<Delivery> {
        self.retry_commands();
        self.retry_saves();
        let mut latest: BTreeMap<PathBuf, Delivery> = BTreeMap::new();
        while let Ok(delivery) = self.snapshots.try_recv() {
            if !self.desired.contains(&delivery.snapshot.requested_path) {
                continue;
            }
            self.identities.insert(
                delivery.snapshot.requested_path.clone(),
                delivery.snapshot.path.clone(),
            );
            latest.insert(delivery.snapshot.requested_path.clone(), delivery);
        }
        latest.into_values().collect()
    }

    /// The snapshots alone (the prepared editor state is dropped): the
    /// registry then tokenises on admission. Hosts use `poll_prepared`.
    pub fn poll(&mut self) -> Vec<Arc<FileSnapshot>> {
        self.take_deliveries().into_iter().map(|d| d.snapshot).collect()
    }
    /// Every delivered snapshot with its prepared editor state.
    pub fn poll_prepared(&mut self) -> Vec<Delivery> {
        self.take_deliveries()
    }
    pub fn request_stop(&self) { self.stop.store(true, Ordering::Relaxed); }
    pub fn is_finished(&self) -> bool { self.task.is_finished() }
    pub fn watched_paths(&self) -> impl Iterator<Item = &PathBuf> { self.desired.iter() }
}

impl Drop for DocumentWorker {
    fn drop(&mut self) { self.request_stop(); }
}

#[derive(Clone, Debug, PartialEq)]
struct Reading { text: Option<Arc<String>>, error: Option<String> }

#[derive(Default)]
struct Watch { canonical: Option<PathBuf>, last: Option<Reading> }

#[cfg(not(target_arch = "wasm32"))]
struct ReadFailure { message: String, not_found: bool }

#[cfg(not(target_arch = "wasm32"))]
fn read_once(requested: &Path, watch: &mut Watch) -> Result<Arc<String>, ReadFailure> {
    use std::{fs::File, io::{ErrorKind, Read}};
    let io = |e: std::io::Error| ReadFailure { not_found: e.kind() == ErrorKind::NotFound, message: e.to_string() };
    let plain = |message: &str| ReadFailure { message: message.into(), not_found: false };
    if watch.canonical.is_none() {
        watch.canonical = Some(std::fs::canonicalize(requested).map_err(io)?);
    }
    let path = watch.canonical.as_ref().unwrap();
    // Reject FIFOs/devices before open(), which could otherwise block a
    // worker indefinitely even though no UI thread touches the filesystem.
    if !std::fs::metadata(path).map_err(io)?.is_file() {
        return Err(plain("Only regular files can be opened"));
    }
    let file = File::open(path).map_err(io)?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file() { return Err(plain("Only regular files can be opened")); }
    if metadata.len() > MAX_FILE_BYTES as u64 { return Err(plain("File exceeds the 2 MiB live-editor limit")); }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() > MAX_FILE_BYTES { return Err(plain("File grew beyond the 2 MiB live-editor limit")); }
    let text = String::from_utf8(bytes).map_err(|_| plain("File is not UTF-8 text"))?;
    if text.contains('\0') { return Err(plain("File contains binary NUL bytes")); }
    Ok(Arc::new(text))
}

/// Read a watched file. An ENOENT is retried once after a short delay: an
/// atomic replacement (write temporary, rename over) is observed by a
/// watcher before the rename lands, and the replacement must not be
/// reported as a vanished file.
#[cfg(not(target_arch = "wasm32"))]
fn read_file(requested: &Path, watch: &mut Watch) -> Reading {
    let result = match read_once(requested, watch) {
        Err(failure) if failure.not_found => {
            std::thread::sleep(REPLACEMENT_RETRY);
            read_once(requested, watch)
        }
        other => other,
    };
    match result {
        Ok(text) => Reading { text: Some(text), error: None },
        Err(failure) => Reading { text: None, error: Some(failure.message) },
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: std::time::SystemTime,
    readonly: bool,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64, i64, i64, u32),
}

#[cfg(not(target_arch = "wasm32"))]
impl FileStamp {
    fn of(metadata: &std::fs::Metadata) -> Result<Self, String> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            len: metadata.len(), modified: metadata.modified().map_err(|e| e.to_string())?,
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            identity: (metadata.dev(), metadata.ino(), metadata.mtime(), metadata.mtime_nsec(), metadata.ctime(), metadata.ctime_nsec(), metadata.mode()),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn guarded_read(path: &Path) -> Result<(String, FileStamp, std::fs::Permissions), String> {
    use std::{fs::{self, File}, io::Read};
    if fs::canonicalize(path).map_err(|e| e.to_string())? != path {
        return Err("Save refused: the file path now resolves to a different target".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Save requires an existing regular file, without a replaced symlink target".into());
    }
    if metadata.len() > MAX_FILE_BYTES as u64 { return Err("Disk file exceeds the 2 MiB limit".into()); }
    let before = FileStamp::of(&metadata)?;
    let file = File::open(path).map_err(|e| e.to_string())?;
    if FileStamp::of(&file.metadata().map_err(|e| e.to_string())?)? != before {
        return Err("Save conflict: file changed while being opened".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&file).take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE_BYTES { return Err("Disk file grew beyond the 2 MiB limit".into()); }
    if FileStamp::of(&file.metadata().map_err(|e| e.to_string())?)? != before
        || FileStamp::of(&fs::symlink_metadata(path).map_err(|e| e.to_string())?)? != before {
        return Err("Save conflict: file changed while being read".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "Disk file is not UTF-8".to_owned())?;
    Ok((text, before, metadata.permissions()))
}

/// Check exact contents and file identity, write a bounded sibling temporary,
/// preserve permissions, then recheck and atomically replace. This rejects
/// every observed concurrent edit. POSIX offers no conditional rename: another
/// writer can still race the final check and rename. This is not a filesystem
/// compare-and-swap guarantee, and no advisory lock is claimed to provide one.
#[cfg(not(target_arch = "wasm32"))]
fn save_file(path: &Path, expected: &str, text: &str, request_id: u64, stop: &AtomicBool) -> Result<(), String> {
    use std::{fs::{self, OpenOptions}, io::Write, sync::atomic::AtomicU64};
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
    if text.len() > MAX_FILE_BYTES || text.contains('\0') { return Err("Invalid save payload".into()); }
    if stop.load(Ordering::Relaxed) { return Err("Save cancelled during shutdown".into()); }
    let (disk, before, permissions) = guarded_read(path)?;
    if disk != expected { return Err("Save conflict: disk contents changed; local buffer retained".into()); }
    let parent = path.parent().ok_or("File has no parent directory")?;
    let name = path.file_name().ok_or("File has no name")?.to_string_lossy();
    let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".{name}.studio-save-{}-{request_id}-{nonce}.tmp", std::process::id()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| format!("Cannot create save temporary: {e}"))?;
    struct RemoveTemporary(PathBuf);
    impl Drop for RemoveTemporary { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
    let cleanup = RemoveTemporary(temporary.clone());
    file.write_all(text.as_bytes()).map_err(|e| format!("Cannot write save temporary: {e}"))?;
    file.set_permissions(permissions).map_err(|e| format!("Cannot preserve file permissions: {e}"))?;
    file.sync_all().map_err(|e| format!("Cannot sync save temporary: {e}"))?;
    drop(file);
    let (disk, after, _) = guarded_read(path)?;
    if before != after || disk != expected { return Err("Save conflict: file changed before replacement; local buffer retained".into()); }
    if stop.load(Ordering::Relaxed) { return Err("Save cancelled during shutdown".into()); }
    fs::rename(&temporary, path).map_err(|e| format!("Cannot replace saved file: {e}"))?;
    drop(cleanup);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn run(
    rx: Receiver<Command>, tx: SyncSender<Delivery>,
    save_rx: Receiver<SaveRequest>, save_tx: SyncSender<SaveResult>, stop: Arc<AtomicBool>,
    fallback: Duration,
) {
    let mut watches: BTreeMap<PathBuf, Watch> = BTreeMap::new();
    let mut pending: BTreeMap<PathBuf, Delivery> = BTreeMap::new();
    let mut revision = 0u64;
    let mut next_scan = Instant::now();
    let mut pending_results = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        let mut commands = Vec::new();
        // While snapshots are still queued for the UI, wake almost at once
        // so they leave as soon as the channel has room; the 50 ms idle wait
        // is only for a worker with nothing pending.
        let wait = if pending.is_empty() && pending_results.is_empty() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(1)
        };
        match rx.recv_timeout(wait) {
            Ok(command) => commands.push(command),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {},
        }
        while let Ok(command) = rx.try_recv() { commands.push(command); }
        for command in commands {
            match command {
                Command::Desired(paths) => {
                    watches.retain(|p, _| paths.contains(p));
                    pending.retain(|p, _| paths.contains(p));
                    for path in paths.iter().take(MAX_DOCUMENTS) { watches.entry(path.clone()).or_default(); }
                    next_scan = Instant::now();
                }
                Command::Refresh(path) => {
                    for (requested, watch) in &mut watches {
                        if requested == &path || watch.canonical.as_ref() == Some(&path) {
                            watch.last = None;
                        }
                    }
                    next_scan = Instant::now();
                }
                Command::Notify { path, epoch, seq } => {
                    if stop.load(Ordering::Relaxed) { return; }
                    // The notified path may be the requested spelling or the
                    // canonical identity; resolve once more only on a miss.
                    let canonical_of_notified = watches.values().any(|w| w.canonical.as_ref() == Some(&path))
                        .then(|| path.clone())
                        .or_else(|| std::fs::canonicalize(&path).ok());
                    for (requested, watch) in &mut watches {
                        let matches = requested == &path
                            || watch.canonical.as_ref() == Some(&path)
                            || (watch.canonical.is_some() && watch.canonical == canonical_of_notified);
                        if !matches { continue; }
                        let reading = read_file(requested, watch);
                        if watch.last.as_ref() != Some(&reading) {
                            revision += 1;
                            let prepared = reading.text.as_deref().map(|t| prepare_snapshot(t).with_revision(revision));
                            pending.insert(requested.clone(), Delivery { snapshot: Arc::new(FileSnapshot {
                                requested_path: requested.clone(),
                                path: watch.canonical.clone().unwrap_or_else(|| requested.clone()),
                                revision, text: reading.text.clone(), error: reading.error.clone(),
                                observed: Some((epoch, seq)),
                            }), prepared });
                            watch.last = Some(reading);
                        }
                    }
                }
            }
        }
        while let Ok(request) = save_rx.try_recv() {
            if stop.load(Ordering::Relaxed) { return; }
            let canonical = watches.iter().find_map(|(path, watch)| {
                if path == &request.path || watch.canonical.as_ref() == Some(&request.path) {
                    watch.canonical.clone()
                } else { None }
            });
            let outcome = canonical.as_ref().ok_or_else(|| "Document must be opened successfully before saving".to_owned())
                .and_then(|path| save_file(path, &request.expected_disk, &request.text, request.request_id, &stop));
            let (saved_text, saved_revision) = if outcome.is_ok() {
                revision += 1;
                let prepared = prepare_snapshot(&request.text).with_revision(revision);
                for (requested, watch) in &mut watches {
                    if watch.canonical == canonical {
                        watch.last = Some(Reading { text: Some(request.text.clone()), error: None });
                        pending.insert(requested.clone(), Delivery { snapshot: Arc::new(FileSnapshot {
                            requested_path: requested.clone(), path: canonical.clone().unwrap(),
                            revision, text: Some(request.text.clone()), error: None,
                            observed: None,
                        }), prepared: Some(prepared.clone()) });
                    }
                }
                (Some(request.text), Some(revision))
            } else { (None, None) };
            pending_results.push_back(SaveResult { request_id: request.request_id, path: canonical.unwrap_or(request.path), result: outcome, text: saved_text, revision: saved_revision });
        }
        // Admission includes unacknowledged results, so this queue and channel
        // together can never hold more than MAX_PENDING_SAVES payloads.
        while let Some(result) = pending_results.pop_front() {
            match save_tx.try_send(result) {
                Ok(()) => SignalToUI::set_ui_signal(),
                Err(TrySendError::Full(result)) => { pending_results.push_front(result); break; },
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        if Instant::now() >= next_scan {
            for (requested, watch) in &mut watches {
                if stop.load(Ordering::Relaxed) { return; }
                let reading = read_file(requested, watch);
                if watch.last.as_ref() != Some(&reading) {
                    revision += 1;
                    let prepared = reading.text.as_deref().map(|t| prepare_snapshot(t).with_revision(revision));
                    pending.insert(requested.clone(), Delivery { snapshot: Arc::new(FileSnapshot {
                        requested_path: requested.clone(),
                        path: watch.canonical.clone().unwrap_or_else(|| requested.clone()),
                        revision, text: reading.text.clone(), error: reading.error.clone(),
                        observed: None,
                    }), prepared });
                    watch.last = Some(reading);
                }
            }
            next_scan = Instant::now() + fallback;
        }
        // At most one pending snapshot per watched path; a full output queue
        // never blocks the worker or drops the most recent disk state.
        while let Some((path, delivery)) = pending.pop_first() {
            match tx.try_send(delivery) {
                Ok(()) => SignalToUI::set_ui_signal(),
                Err(TrySendError::Full(delivery)) => { pending.insert(path, delivery); break; },
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use makepad_widgets::Cx;

    /// A headless test has no event loop to wake; the platform waker is a
    /// no-op until the main-thread event loop has started, so tests need no
    /// special setup.
    fn headless_cx() -> Cx {
        Cx::new(Box::new(|_, _| {}))
    }

    fn scratch() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("studio-doc-{}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap(); std::fs::canonicalize(path).unwrap()
    }
    fn receive(worker: &mut DocumentWorker) -> Arc<FileSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(snapshot) = worker.poll().pop() { return snapshot; }
            assert!(Instant::now() < deadline, "reader produced no snapshot");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn watches_only_open_files_detects_replacement_and_stops() {
        let dir = scratch();
        let path = dir.join("source.rs");
        std::fs::write(&path, "let a = 1;\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_millis(100)).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        assert_eq!(first.text.as_deref().map(String::as_str), Some("let a = 1;\n"));
        assert_eq!(first.observed, None);
        let replacement = dir.join("replacement");
        std::fs::write(&replacement, "let a = 2;\n").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let second = receive(&mut worker);
        assert!(second.revision > first.revision);
        assert_eq!(second.path, first.path);
        assert_eq!(second.text.as_deref().map(String::as_str), Some("let a = 2;\n"));
        worker.request_stop();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline, "reader did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(worker.watch(path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rejects_binary_and_large_files_without_truncating() {
        let dir = scratch(); let path = dir.join("data");
        std::fs::write(&path, [0, 1, 2]).unwrap();
        assert!(read_file(&path, &mut Watch::default()).error.unwrap().contains("binary"));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
        assert!(read_file(&path, &mut Watch::default()).error.unwrap().contains("limit"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_save_preserves_permissions_and_refuses_conflicting_disk() {
        let dir = scratch(); let path = dir.join("source.rs");
        std::fs::write(&path, "original\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
        }
        let stop = AtomicBool::new(false);
        save_file(&path, "original\n", "saved\n", 1, &stop).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o751);
        }
        let error = save_file(&path, "original\n", "must not overwrite\n", 2, &stop).unwrap_err();
        assert!(error.contains("conflict"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved\n");
        stop.store(true, Ordering::Relaxed);
        assert!(save_file(&path, "saved\n", "cancelled\n", 3, &stop).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "temporary file leaked");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_refuses_a_replaced_symlink_target() {
        let dir = scratch(); let path = dir.join("source.rs"); let other = dir.join("other.rs");
        std::fs::write(&other, "same text").unwrap();
        std::os::unix::fs::symlink(&other, &path).unwrap();
        assert!(save_file(&path, "same text", "do not write", 1, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "same text");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saves_are_bounded_acknowledged_and_worker_stops_without_draining_output() {
        let dir = scratch(); let path = dir.join("source.rs");
        std::fs::write(&path, "original\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start(&cx.thread_spawner()).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        let expected = first.text.clone().unwrap(); let saved = Arc::new("saved\n".to_owned());
        let id = worker.save(path.clone(), expected.clone(), saved.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let acknowledgement = loop {
            if let Some(result) = worker.poll_saves().pop() { break result; }
            assert!(Instant::now() < deadline, "save produced no acknowledgement");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(acknowledgement.request_id, id);
        assert_eq!(acknowledgement.path, path);
        assert!(acknowledgement.result.is_ok(), "{:?}", acknowledgement.result);
        assert_eq!(acknowledgement.text, Some(saved.clone()));
        assert!(acknowledgement.revision.unwrap() > first.revision);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), *saved);
        for _ in 0..MAX_PENDING_SAVES {
            worker.save(path.clone(), saved.clone(), saved.clone()).unwrap();
        }
        assert!(worker.save(path.clone(), saved.clone(), saved.clone()).unwrap_err().contains("full"));
        // Leave all subsequent save acknowledgements and snapshots unread.
        std::thread::sleep(Duration::from_millis(350));
        worker.request_stop();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline, "full output queues blocked shutdown");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(worker.poll_saves().len(), MAX_PENDING_SAVES);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Collect snapshots for `window`, polling every 2 ms.
    fn collect(worker: &mut DocumentWorker, window: Duration) -> Vec<Arc<FileSnapshot>> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            out.extend(worker.poll());
            std::thread::sleep(Duration::from_millis(2));
        }
        out
    }

    #[test]
    fn a_notification_reads_at_once_without_polling() {
        let dir = scratch(); let path = dir.join("live.rs");
        std::fs::write(&path, "v1\n").unwrap();
        let cx = headless_cx();
        // A fallback far beyond the test: only the notification can produce the read.
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120)).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        assert_eq!(first.text.as_deref().map(String::as_str), Some("v1\n"));
        std::fs::write(&path, "v2\n").unwrap();
        // An unknown path is ignored, never an error.
        worker.notify_changed(dir.join("other.rs"), 1, 1).unwrap();
        let started = Instant::now();
        worker.notify_changed(path.clone(), 4, 77).unwrap();
        let snapshot = receive(&mut worker);
        let latency = started.elapsed();
        assert_eq!(snapshot.text.as_deref().map(String::as_str), Some("v2\n"));
        assert_eq!(snapshot.observed, Some((4, 77)));
        assert!(snapshot.revision > first.revision);
        // The read is immediate; the bound is generous for a loaded machine.
        assert!(latency < Duration::from_millis(500), "notify → snapshot took {latency:?}");
        eprintln!("record=document_notify_latency ms={:.2}", latency.as_secs_f64() * 1000.0);
        // Same content again: no snapshot.
        worker.notify_changed(path.clone(), 4, 78).unwrap();
        assert!(collect(&mut worker, Duration::from_millis(150)).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_atomic_replacement_is_one_snapshot() {
        let dir = scratch(); let path = dir.join("atomic.rs");
        std::fs::write(&path, "before\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120)).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        let temporary = dir.join(".atomic.rs.tmp");
        std::fs::write(&temporary, "after\n").unwrap();
        std::fs::rename(&temporary, &path).unwrap();
        worker.notify_changed(path.clone(), 2, 10).unwrap();
        worker.notify_changed(path.clone(), 2, 11).unwrap();
        let snapshots = collect(&mut worker, Duration::from_millis(400));
        assert_eq!(snapshots.len(), 1, "{snapshots:?}");
        assert_eq!(snapshots[0].text.as_deref().map(String::as_str), Some("after\n"));
        assert_eq!(snapshots[0].path, first.path);
        assert!(snapshots[0].revision > first.revision);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn revisions_stay_monotonic_across_notifications_and_polling() {
        let dir = scratch(); let path = dir.join("mono.rs");
        std::fs::write(&path, "r1\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_millis(150)).unwrap();
        worker.watch(path.clone()).unwrap();
        let r1 = receive(&mut worker);
        std::fs::write(&path, "r2\n").unwrap();
        worker.notify_changed(path.clone(), 1, 5).unwrap();
        let r2 = receive(&mut worker);
        assert_eq!(r2.observed, Some((1, 5)));
        assert!(r2.revision > r1.revision);
        // Sleep past the mtime granularity, then change without notifying:
        // the fallback poll must still observe it, with a higher revision.
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&path, "r3\n").unwrap();
        let r3 = receive(&mut worker);
        assert_eq!(r3.text.as_deref().map(String::as_str), Some("r3\n"));
        assert_eq!(r3.observed, None);
        assert!(r3.revision > r2.revision);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_dirty_buffer_conflicts_on_a_notified_disk_change() {
        use crate::document::DocumentRegistry;
        let dir = scratch(); let path = dir.join("conflict.rs");
        std::fs::write(&path, "base\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120)).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        let mut registry = DocumentRegistry::default();
        let doc = registry.apply_snapshot(&first).unwrap();
        let view = doc.view_session();
        view.borrow().paste("human ".into());
        doc.local_text_changed();
        assert!(doc.is_dirty());
        let human = doc.current_text();
        std::fs::write(&path, "agent\n").unwrap();
        worker.notify_changed(path.clone(), 9, 1).unwrap();
        let disk = receive(&mut worker);
        assert_eq!(disk.observed, Some((9, 1)));
        registry.apply_snapshot(&disk).unwrap();
        assert_eq!(doc.current_text(), human, "local edits must never be overwritten");
        assert!(doc.has_conflict());
        assert_eq!(doc.disk_text().as_str(), "agent\n");
        // A clean buffer takes the disk update in place.
        doc.discard_local_and_reload().unwrap();
        assert!(!doc.is_dirty() && !doc.has_conflict());
        std::fs::write(&path, "agent 2\n").unwrap();
        worker.notify_changed(path.clone(), 9, 2).unwrap();
        let disk = receive(&mut worker);
        registry.apply_snapshot(&disk).unwrap();
        assert_eq!(doc.current_text(), "agent 2\n");
        assert!(!doc.has_conflict());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn poll_retries_pending_saves() {
        let dir = scratch();
        let path = dir.join("retry.rs");
        std::fs::write(&path, "original\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120)).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        let expected = first.text.clone().unwrap();
        let saved = Arc::new("saved-via-poll\n".to_owned());
        let request_id = worker.next_save_id;
        worker.next_save_id += 1;
        worker.outstanding_saves.insert(request_id, path.clone());
        worker.pending_saves.push_back(SaveRequest {
            request_id,
            path: path.clone(),
            expected_disk: expected,
            text: saved.clone(),
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            if let Some(snapshot) = worker
                .poll()
                .into_iter()
                .find(|s| s.text.as_deref().map(String::as_str) == Some(saved.as_str()))
            {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "poll did not retry the pending save");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(snapshot.text.as_deref().map(String::as_str), Some(saved.as_str()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), *saved);
        let ack = loop {
            if let Some(result) = worker.poll_saves().pop() {
                break result;
            }
            assert!(Instant::now() < deadline, "save produced no acknowledgement");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(ack.request_id, request_id);
        assert!(ack.result.is_ok(), "{:?}", ack.result);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn poll_filters_snapshots_to_desired_paths() {
        let dir = scratch();
        let keep = dir.join("keep.rs");
        let drop = dir.join("drop.rs");
        std::fs::write(&keep, "keep-1\n").unwrap();
        std::fs::write(&drop, "drop-1\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120)).unwrap();
        worker.watch(keep.clone()).unwrap();
        worker.watch(drop.clone()).unwrap();
        let mut seen_keep = false;
        let mut seen_drop = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(seen_keep && seen_drop) {
            for snapshot in worker.poll() {
                seen_keep |= snapshot.requested_path == keep;
                seen_drop |= snapshot.requested_path == drop;
            }
            assert!(Instant::now() < deadline, "initial snapshots missing");
            std::thread::sleep(Duration::from_millis(20));
        }
        worker.unwatch(&drop);
        std::fs::write(&keep, "keep-2\n").unwrap();
        std::fs::write(&drop, "drop-2\n").unwrap();
        worker.notify_changed(keep.clone(), 1, 1).unwrap();
        worker.notify_changed(drop.clone(), 1, 2).unwrap();
        let snapshots = collect(&mut worker, Duration::from_millis(400));
        assert!(
            snapshots.iter().any(|s| s.requested_path == keep && s.text.as_deref().map(String::as_str) == Some("keep-2\n")),
            "{snapshots:?}"
        );
        assert!(
            snapshots.iter().all(|s| s.requested_path != drop),
            "unwatched path leaked: {snapshots:?}"
        );
        let prepared = worker.poll_prepared();
        assert!(prepared.iter().all(|d| d.snapshot.requested_path != drop));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn poll_coalesces_to_the_latest_snapshot_per_path() {
        let dir = scratch();
        let path = dir.join("coalesce.rs");
        std::fs::write(&path, "v1\n").unwrap();
        let cx = headless_cx();
        let mut worker = DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120)).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        assert_eq!(first.text.as_deref().map(String::as_str), Some("v1\n"));
        std::fs::write(&path, "v2\n").unwrap();
        worker.notify_changed(path.clone(), 2, 1).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        std::fs::write(&path, "v3\n").unwrap();
        worker.notify_changed(path.clone(), 2, 2).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        let snapshots = worker.poll();
        assert_eq!(snapshots.len(), 1, "{snapshots:?}");
        assert_eq!(snapshots[0].text.as_deref().map(String::as_str), Some("v3\n"));
        std::fs::write(&path, "v4\n").unwrap();
        worker.notify_changed(path.clone(), 2, 3).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        std::fs::write(&path, "v5\n").unwrap();
        worker.notify_changed(path.clone(), 2, 4).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        let prepared = worker.poll_prepared();
        assert_eq!(prepared.len(), 1, "{prepared:?}");
        assert_eq!(prepared[0].snapshot.text.as_deref().map(String::as_str), Some("v5\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
