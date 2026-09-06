//! One bounded file-reader worker for explicitly opened Studio documents.
//! No recursive scanning, UI-thread filesystem access, or shared locks.

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
const SAMPLE_INTERVAL: Duration = Duration::from_millis(750);

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
    commands: SyncSender<Arc<Vec<PathBuf>>>,
    snapshots: Receiver<Arc<FileSnapshot>>,
    save_commands: SyncSender<SaveRequest>,
    save_results: Receiver<SaveResult>,
    pending_saves: VecDeque<SaveRequest>,
    outstanding_saves: BTreeMap<u64, PathBuf>,
    next_save_id: u64,
    desired: BTreeSet<PathBuf>,
    identities: BTreeMap<PathBuf, PathBuf>,
    retry: bool,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
}

impl DocumentWorker {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        { let _ = spawner; return Err("Local file watching is unavailable in this browser".into()); }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (commands, rx) = mpsc::sync_channel(1);
            let (tx, snapshots) = mpsc::sync_channel(4);
            let (save_commands, save_rx) = mpsc::sync_channel(MAX_PENDING_SAVES);
            let (save_tx, save_results) = mpsc::sync_channel(MAX_PENDING_SAVES);
            let stop = Arc::new(AtomicBool::new(false));
            let cancel = stop.clone();
            let task = spawner.spawn_worker(
                ThreadOptions { name: Some("studio-documents".into()), ..Default::default() },
                move || run(rx, tx, save_rx, save_tx, cancel),
            ).map_err(|e| e.to_string())?;
            Ok(Self {
                commands, snapshots, save_commands, save_results,
                pending_saves: VecDeque::new(), outstanding_saves: BTreeMap::new(), next_save_id: 1,
                desired: BTreeSet::new(), identities: BTreeMap::new(), retry: false, stop, task,
            })
        }
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
            self.retry = true; self.retry_commands();
        }
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
        if !self.retry { return; }
        match self.commands.try_send(Arc::new(self.desired.iter().cloned().collect())) {
            Ok(()) => self.retry = false,
            Err(TrySendError::Full(_)) => {},
            Err(TrySendError::Disconnected(_)) => self.retry = false,
        }
    }

    pub fn poll(&mut self) -> Vec<Arc<FileSnapshot>> {
        self.retry_commands();
        self.retry_saves();
        let mut latest = BTreeMap::new();
        while let Ok(snapshot) = self.snapshots.try_recv() {
            if self.desired.contains(&snapshot.requested_path) {
                self.identities.insert(snapshot.requested_path.clone(), snapshot.path.clone());
                latest.insert(snapshot.requested_path.clone(), snapshot);
            }
        }
        latest.into_values().collect()
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
fn read_file(requested: &Path, watch: &mut Watch) -> Reading {
    use std::{fs::File, io::Read};
    let result = (|| {
        if watch.canonical.is_none() {
            watch.canonical = Some(std::fs::canonicalize(requested).map_err(|e| e.to_string())?);
        }
        let path = watch.canonical.as_ref().unwrap();
        // Reject FIFOs/devices before open(), which could otherwise block a
        // worker indefinitely even though no UI thread touches the filesystem.
        if !std::fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
            return Err("Only regular files can be opened".into());
        }
        let file = File::open(path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() { return Err("Only regular files can be opened".into()); }
        if metadata.len() > MAX_FILE_BYTES as u64 { return Err("File exceeds the 2 MiB live-editor limit".into()); }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FILE_BYTES { return Err("File grew beyond the 2 MiB live-editor limit".into()); }
        let text = String::from_utf8(bytes).map_err(|_| "File is not UTF-8 text".to_string())?;
        if text.contains('\0') { return Err("File contains binary NUL bytes".into()); }
        Ok(Arc::new(text))
    })();
    match result {
        Ok(text) => Reading { text: Some(text), error: None },
        Err(error) => Reading { text: None, error: Some(error) },
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
    rx: Receiver<Arc<Vec<PathBuf>>>, tx: SyncSender<Arc<FileSnapshot>>,
    save_rx: Receiver<SaveRequest>, save_tx: SyncSender<SaveResult>, stop: Arc<AtomicBool>,
) {
    let mut watches: BTreeMap<PathBuf, Watch> = BTreeMap::new();
    let mut pending: BTreeMap<PathBuf, Arc<FileSnapshot>> = BTreeMap::new();
    let mut revision = 0u64;
    let mut next_scan = Instant::now();
    let mut pending_results = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(paths) => {
                watches.retain(|p, _| paths.contains(p));
                pending.retain(|p, _| paths.contains(p));
                for path in paths.iter().take(MAX_DOCUMENTS) { watches.entry(path.clone()).or_default(); }
                next_scan = Instant::now();
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {},
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
                for (requested, watch) in &mut watches {
                    if watch.canonical == canonical {
                        watch.last = Some(Reading { text: Some(request.text.clone()), error: None });
                        pending.insert(requested.clone(), Arc::new(FileSnapshot {
                            requested_path: requested.clone(), path: canonical.clone().unwrap(),
                            revision, text: Some(request.text.clone()), error: None,
                        }));
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
                    pending.insert(requested.clone(), Arc::new(FileSnapshot {
                        requested_path: requested.clone(),
                        path: watch.canonical.clone().unwrap_or_else(|| requested.clone()),
                        revision, text: reading.text.clone(), error: reading.error.clone(),
                    }));
                    watch.last = Some(reading);
                }
            }
            next_scan = Instant::now() + SAMPLE_INTERVAL;
        }
        // At most one pending snapshot per watched path; a full output queue
        // never blocks the worker or drops the most recent disk state.
        while let Some((path, snapshot)) = pending.pop_first() {
            match tx.try_send(snapshot) {
                Ok(()) => SignalToUI::set_ui_signal(),
                Err(TrySendError::Full(snapshot)) => { pending.insert(path, snapshot); break; },
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use makepad_widgets::Cx;

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
        let cx = Cx::new(Box::new(|_, _| {}));
        let mut worker = DocumentWorker::start(&cx.thread_spawner()).unwrap();
        worker.watch(path.clone()).unwrap();
        let first = receive(&mut worker);
        assert_eq!(first.text.as_deref().map(String::as_str), Some("let a = 1;\n"));
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
        let cx = Cx::new(Box::new(|_, _| {}));
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
}
