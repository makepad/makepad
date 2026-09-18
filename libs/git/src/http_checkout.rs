//! Persistent, bounded file writers for blocking HTTP checkout workers.
//! Hooks stay on the caller so terminal/UI notifications never run on writers.
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc, Mutex, OnceLock},
};
use crate::{GitError, IndexEntry, Object, ObjectId, Repository};
use super::HttpSyncHooks;

pub(super) struct File { pub path: String, pub oid: ObjectId, pub mode: u32 }
type ResultRow = Result<(IndexEntry, u64), GitError>;
struct Job {
    root: Arc<PathBuf>, file: File, blob: Arc<Object>,
    result: mpsc::SyncSender<ResultRow>, cancelled: Arc<AtomicBool>,
}
struct Pool { tx: mpsc::SyncSender<Job>, lanes: usize }
static POOL: OnceLock<Result<Pool, String>> = OnceLock::new();

fn pool() -> Result<&'static Pool, GitError> {
    POOL.get_or_init(|| {
        let lanes = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
        let (tx, rx) = mpsc::sync_channel::<Job>(lanes * 2);
        let rx = Arc::new(Mutex::new(rx));
        for lane in 0..lanes {
            let rx = rx.clone();
            std::thread::Builder::new().name(format!("git-checkout-{lane}")).spawn(move || loop {
                let job = match rx.lock() { Ok(rx) => rx.recv(), Err(_) => break };
                let Ok(job) = job else { break };
                let result = if job.cancelled.load(Ordering::Relaxed) {
                    Err(GitError::InvalidObject("checkout cancelled".into()))
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write_one(&job)))
                        .unwrap_or_else(|_| Err(GitError::InvalidObject("checkout writer panicked".into())))
                };
                let _ = job.result.send(result);
            }).map_err(|e| e.to_string())?;
        }
        Ok(Pool { tx, lanes })
    }).as_ref().map_err(|e| GitError::InvalidObject(format!("checkout worker pool: {e}")))
}

fn write_one(job: &Job) -> ResultRow {
    let path = job.root.join(&job.file.path);
    if job.file.mode == 0o120000 {
        #[cfg(unix)] {
            let target = std::str::from_utf8(&job.blob.data).map_err(|_| GitError::InvalidObject("non-UTF8 symlink".into()))?;
            if fs::symlink_metadata(&path).is_ok() { fs::remove_file(&path)?; }
            std::os::unix::fs::symlink(target, &path)?;
        }
        #[cfg(not(unix))] fs::write(&path, &job.blob.data)?;
    } else {
        // Do not follow an old symlink when a tracked path becomes a file.
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) { fs::remove_file(&path)?; }
        fs::write(&path, &job.blob.data)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(if job.file.mode == 0o100755 { 0o755 } else { 0o644 }))?;
        }
    }
    let metadata = fs::symlink_metadata(&path)?;
    Ok((crate::worktree::index_entry_from_metadata(job.file.path.clone(), job.file.oid, job.file.mode, &metadata), job.blob.data.len() as u64))
}

pub(super) fn write(
    repo: &mut Repository, files: Vec<File>, imported: &HashMap<ObjectId, Arc<Object>>,
    hooks: &mut dyn HttpSyncHooks,
) -> Result<(Vec<IndexEntry>, u64), GitError> {
    let pool = pool()?;
    let root = Arc::new(repo.workdir.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::sync_channel(pool.lanes);
    let mut entries = Vec::with_capacity(files.len());
    let mut bytes = 0;
    let mut active = 0;
    let mut error = None;
    let mut pending = files.into_iter();
    let mut exhausted = false;
    loop {
        while error.is_none() && !exhausted && active < pool.lanes {
            let Some(file) = pending.next() else { exhausted = true; break };
            let blob = imported.get(&file.oid).cloned().map(Ok).unwrap_or_else(|| {
                repo.read_blob(&file.oid).map(|data| Arc::new(Object { kind: crate::ObjectKind::Blob, data }))
            });
            let blob = match blob { Ok(blob) => blob, Err(e) => { error = Some(e); break } };
            if blob.kind != crate::ObjectKind::Blob { error = Some(GitError::InvalidObject("checkout expected a blob".into())); break; }
            if pool.tx.send(Job { root: root.clone(), file, blob, result: tx.clone(), cancelled: cancelled.clone() }).is_err() {
                error = Some(GitError::InvalidObject("checkout workers stopped".into())); break;
            }
            active += 1;
        }
        if error.is_some() { cancelled.store(true, Ordering::Relaxed); }
        if active == 0 { break; }
        // This is a blocking setup API, never called on a UI or audio thread.
        match rx.recv() {
            Ok(Ok((entry, size))) => { hooks.on_checkout_file(&entry.path); bytes += size; entries.push(entry); }
            Ok(Err(e)) => { if error.is_none() { error = Some(e); } }
            Err(_) => { error = Some(GitError::InvalidObject("checkout results disconnected".into())); break; }
        }
        active -= 1;
    }
    // All admitted writes finish before an error reaches staging cleanup.
    if let Some(error) = error { return Err(error); }
    Ok((entries, bytes))
}
