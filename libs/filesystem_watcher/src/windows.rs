use crate::{Emitter, FileSystemEventKind, RescanReason, WatchRoot};
use std::ffi::{c_void, OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use makepad_windows_sys::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_NOTIFY_ENUM_DIR, HANDLE, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::*,
    },
};

pub struct PlatformWatcher {
    stop: Arc<AtomicBool>,
    handles: Arc<Mutex<Vec<usize>>>,
    threads: Vec<JoinHandle<()>>,
}

impl PlatformWatcher {
    /// Returns once every root thread has opened its root and is about to
    /// issue its first read; a root that cannot be opened (or is a reparse
    /// point) fails the whole start.
    pub fn start(roots: Vec<WatchRoot>, emitter: Arc<Emitter>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let handles = Arc::new(Mutex::new(Vec::<usize>::new()));
        let mut threads = Vec::new();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        for root in roots {
            let stop_thread = Arc::clone(&stop);
            let emitter = Arc::clone(&emitter);
            let handles_thread = Arc::clone(&handles);
            let ready = ready_tx.clone();
            let thread = thread::Builder::new()
                .name(format!("fswatch-win-{}", root.mount))
                .spawn(move || watch_root_loop(root, stop_thread, emitter, handles_thread, ready))
                .map_err(|err| format!("failed to spawn windows watcher thread: {}", err))?;
            threads.push(thread);
        }
        drop(ready_tx);

        let mut watcher = Self {
            stop,
            handles,
            threads,
        };
        for _ in 0..watcher.threads.len() {
            match ready_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    watcher.stop();
                    return Err(error);
                }
                Err(_) => {
                    watcher.stop();
                    return Err("windows watcher thread exited before initialization".to_string());
                }
            }
        }
        Ok(watcher)
    }

    /// A backend that watches nothing; `stop` is a no-op.
    pub fn idle() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            handles: Arc::new(Mutex::new(Vec::new())),
            threads: Vec::new(),
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut handles) = self.handles.lock() {
            for handle in handles.drain(..) {
                let handle = HANDLE(handle as *mut c_void);
                if !handle.0.is_null() && handle != INVALID_HANDLE_VALUE {
                    unsafe {
                        let _ = CloseHandle(handle);
                    }
                }
            }
        }
        while let Some(thread) = self.threads.pop() {
            let _ = thread.join();
        }
    }
}

/// One `FILE_NOTIFY_INFORMATION` record.
struct Record {
    action: u32,
    relative: PathBuf,
}

/// Decode the records of one `ReadDirectoryChangesW` buffer.
fn decode(buffer: &[u8]) -> Vec<Record> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    loop {
        if offset + 12 > buffer.len() {
            break;
        }
        let read_u32 = |at: usize| {
            u32::from_le_bytes([
                buffer[at],
                buffer[at + 1],
                buffer[at + 2],
                buffer[at + 3],
            ])
        };
        let next = read_u32(offset) as usize;
        let action = read_u32(offset + 4);
        let name_bytes = read_u32(offset + 8) as usize;
        let name_start = offset + 12;
        let name_end = (name_start + name_bytes).min(buffer.len());
        let wide: Vec<u16> = buffer[name_start..name_end]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        records.push(Record {
            action,
            relative: PathBuf::from(OsString::from_wide(&wide)),
        });
        if next == 0 {
            break;
        }
        offset += next;
    }
    records
}

/// Map records to events: an old-name record directly followed by a
/// new-name record becomes one `Renamed`; lone halves degrade.
fn classify(root: &std::path::Path, records: &[Record]) -> Vec<(PathBuf, FileSystemEventKind)> {
    let mut out = Vec::with_capacity(records.len());
    let mut i = 0;
    while i < records.len() {
        let record = &records[i];
        let path = root.join(&record.relative);
        match record.action {
            action if action == FILE_ACTION_ADDED.0 => {
                out.push((path, FileSystemEventKind::Created))
            }
            action if action == FILE_ACTION_REMOVED.0 => {
                out.push((path, FileSystemEventKind::Removed))
            }
            action if action == FILE_ACTION_MODIFIED.0 => {
                out.push((path, FileSystemEventKind::Changed))
            }
            action if action == FILE_ACTION_RENAMED_OLD_NAME.0 => {
                if let Some(next) = records.get(i + 1) {
                    if next.action == FILE_ACTION_RENAMED_NEW_NAME.0 {
                        out.push((
                            root.join(&next.relative),
                            FileSystemEventKind::Renamed { from: path },
                        ));
                        i += 2;
                        continue;
                    }
                }
                out.push((path, FileSystemEventKind::Removed));
            }
            action if action == FILE_ACTION_RENAMED_NEW_NAME.0 => {
                out.push((path, FileSystemEventKind::Created))
            }
            _ => out.push((path, FileSystemEventKind::Changed)),
        }
        i += 1;
    }
    out
}

fn emit_rescan(emitter: &Emitter, root: &WatchRoot) {
    emitter.emit(
        root.mount.clone(),
        root.path.clone(),
        FileSystemEventKind::RescanRequired {
            root: root.path.clone(),
            reason: RescanReason::Overflow,
            missing: None,
        },
        Some(true),
    );
}

fn watch_root_loop(
    root: WatchRoot,
    stop: Arc<AtomicBool>,
    emitter: Arc<Emitter>,
    handles: Arc<Mutex<Vec<usize>>>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let wide = wide_null(root.path.as_os_str());
    // Open the root itself, never what a reparse point behind it targets.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    };

    let handle = match handle {
        Ok(handle) => handle,
        Err(error) => {
            let _ = ready.send(Err(format!(
                "cannot open watch root {}: error {}",
                root.path.display(),
                error
            )));
            return;
        }
    };

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let known = unsafe { GetFileInformationByHandle(handle, &mut info) }.is_ok();
    if known && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        unsafe {
            let _ = CloseHandle(handle);
        }
        let _ = ready.send(Err(format!(
            "watch root {} is a reparse point (symlink or junction) and is refused",
            root.path.display()
        )));
        return;
    }

    if let Ok(mut list) = handles.lock() {
        list.push(handle.0 as usize);
    }

    let exclude = emitter.exclude();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        if first {
            // Watching from the first read on: readiness right before it.
            let _ = ready.send(Ok(()));
            first = false;
        }
        let mut bytes_returned = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                true,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_CREATION,
                Some(&mut bytes_returned),
                None,
                None,
            )
        };

        if ok.is_err() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if unsafe { GetLastError() } == ERROR_NOTIFY_ENUM_DIR {
                emit_rescan(&emitter, &root);
            }
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        if bytes_returned == 0 {
            // A successful read with no bytes means the buffer overflowed.
            emit_rescan(&emitter, &root);
            continue;
        }

        let records = decode(&buffer[..bytes_returned as usize]);
        for (path, kind) in classify(&root.path, &records) {
            // Exclusions by the directories above the path come before any
            // probe; a directory event is checked again once its kind is
            // known.
            if exclude.excludes_file(&root.path, &path) {
                continue;
            }
            // symlink_metadata: a reparse point is reported as itself, never
            // followed.
            let is_dir = match &kind {
                FileSystemEventKind::Removed => None,
                _ => std::fs::symlink_metadata(&path).ok().map(|meta| meta.is_dir()),
            };
            if is_dir == Some(true) && exclude.excludes_dir(&root.path, &path) {
                continue;
            }
            emitter.emit(root.mount.clone(), path, kind, is_dir);
        }
    }

    if let Ok(mut list) = handles.lock() {
        list.retain(|h| *h != handle.0 as usize);
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(next: u32, action: FILE_ACTION, name: &str) -> Vec<u8> {
        let wide: Vec<u16> = name.encode_utf16().collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&next.to_le_bytes());
        bytes.extend_from_slice(&action.0.to_le_bytes());
        bytes.extend_from_slice(&((wide.len() * 2) as u32).to_le_bytes());
        for unit in wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn records_decode_and_rename_pairs_join() {
        let first = record(0, FILE_ACTION_RENAMED_OLD_NAME, "src\\a.rs");
        let mut buffer = Vec::new();
        let mut head = first.clone();
        head[..4].copy_from_slice(&(first.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&head);
        let second = record(0, FILE_ACTION_RENAMED_NEW_NAME, "src\\b.rs");
        let mut second_head = second.clone();
        second_head[..4].copy_from_slice(&(second.len() as u32).to_le_bytes());
        buffer.extend_from_slice(&second_head);
        buffer.extend_from_slice(&record(0, FILE_ACTION_MODIFIED, "src\\c.rs"));
        let records = decode(&buffer);
        assert_eq!(records.len(), 3);
        let out = classify(std::path::Path::new("C:\\w"), &records);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].1,
            FileSystemEventKind::Renamed {
                from: PathBuf::from("C:\\w\\src\\a.rs")
            }
        );
        assert_eq!(out[0].0, PathBuf::from("C:\\w\\src\\b.rs"));
        assert_eq!(out[1].1, FileSystemEventKind::Changed);
    }
}
