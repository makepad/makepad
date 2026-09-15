use crate::{Emitter, ExcludePolicy, FileSystemEventKind, RescanReason, WatchRoot};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CString};
use std::fs;
use std::mem::size_of;
use std::os::fd::RawFd;
use std::os::raw::{c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

const O_NONBLOCK: c_int = 0o00004000;
const O_CLOEXEC: c_int = 0o2000000;

const IN_MODIFY: u32 = 0x0000_0002;
const IN_ATTRIB: u32 = 0x0000_0004;
const IN_CLOSE_WRITE: u32 = 0x0000_0008;
const IN_MOVED_FROM: u32 = 0x0000_0040;
const IN_MOVED_TO: u32 = 0x0000_0080;
const IN_CREATE: u32 = 0x0000_0100;
const IN_DELETE: u32 = 0x0000_0200;
const IN_DELETE_SELF: u32 = 0x0000_0400;
const IN_MOVE_SELF: u32 = 0x0000_0800;
const IN_Q_OVERFLOW: u32 = 0x0000_4000;
const IN_IGNORED: u32 = 0x0000_8000;
const IN_ISDIR: u32 = 0x4000_0000;

#[repr(C)]
struct InotifyEvent {
    wd: c_int,
    mask: u32,
    cookie: u32,
    len: u32,
}

unsafe extern "C" {
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, pathname: *const c_char, mask: u32) -> c_int;
    fn inotify_rm_watch(fd: c_int, wd: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
}

pub struct PlatformWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformWatcher {
    pub fn start(roots: Vec<WatchRoot>, emitter: Arc<Emitter>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        // Fail synchronously when inotify itself is unavailable, and install
        // every watch before returning: the caller is watching from here.
        let mut table = WatchTable::new(roots, emitter.exclude().clone())?;
        let mounts: Vec<String> = table.roots.keys().cloned().collect();
        for mount in mounts {
            let _ = table.rescan_mount(&mount);
        }

        let thread = thread::Builder::new()
            .name("fswatch-linux".to_string())
            .spawn(move || run_loop(table, stop_thread, emitter))
            .map_err(|err| format!("failed to spawn linux watcher thread: {}", err))?;

        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// A backend that watches nothing; `stop` is a no-op.
    pub fn idle() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            thread: None,
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct WatchTable {
    fd: RawFd,
    roots: HashMap<String, PathBuf>,
    exclude: ExcludePolicy,
    wd_to_entry: HashMap<i32, (String, PathBuf)>,
    path_to_wd: HashMap<PathBuf, i32>,
}

impl WatchTable {
    fn new(roots: Vec<WatchRoot>, exclude: ExcludePolicy) -> Result<Self, String> {
        let fd = unsafe { inotify_init1(O_NONBLOCK | O_CLOEXEC) };
        if fd < 0 {
            return Err(format!(
                "inotify_init1 failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let mut root_map = HashMap::new();
        for root in roots {
            root_map.insert(root.mount, root.path);
        }

        Ok(Self {
            fd,
            roots: root_map,
            exclude,
            wd_to_entry: HashMap::new(),
            path_to_wd: HashMap::new(),
        })
    }

    fn close_all(&mut self) {
        let wds: Vec<i32> = self.wd_to_entry.keys().copied().collect();
        for wd in wds {
            let _ = unsafe { inotify_rm_watch(self.fd, wd) };
            self.wd_to_entry.remove(&wd);
        }
        self.path_to_wd.clear();
        if self.fd >= 0 {
            let _ = unsafe { close(self.fd) };
            self.fd = -1;
        }
    }

    /// Reconcile the watch coverage of one root with the directories that
    /// exist now: stale watches go, missing ones are added.
    fn rescan_mount(&mut self, mount: &str) -> Result<(), String> {
        let Some(root) = self.roots.get(mount).cloned() else {
            return Ok(());
        };
        let mut dirs = Vec::new();
        collect_dirs(&root, &self.exclude, &mut dirs);
        let wanted: HashSet<PathBuf> = dirs.into_iter().collect();

        let stale: Vec<PathBuf> = self
            .wd_to_entry
            .iter()
            .filter_map(|(_, (entry_mount, path))| {
                if entry_mount == mount && !wanted.contains(path) {
                    Some(path.clone())
                } else {
                    None
                }
            })
            .collect();
        for path in stale {
            self.remove_path(&path);
        }

        let mut wanted_sorted: Vec<PathBuf> = wanted.into_iter().collect();
        wanted_sorted.sort();
        for dir in wanted_sorted {
            self.add_dir(mount, &dir)?;
        }
        Ok(())
    }

    fn remove_path(&mut self, path: &Path) {
        let Some(wd) = self.path_to_wd.remove(path) else {
            return;
        };
        self.wd_to_entry.remove(&wd);
        let _ = unsafe { inotify_rm_watch(self.fd, wd) };
    }

    fn add_dir(&mut self, mount: &str, dir: &Path) -> Result<(), String> {
        if self.path_to_wd.contains_key(dir) {
            return Ok(());
        }
        let c_path = CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| format!("path contains interior NUL byte: {}", dir.display()))?;
        let mask = IN_CREATE
            | IN_DELETE
            | IN_MODIFY
            | IN_MOVED_FROM
            | IN_MOVED_TO
            | IN_ATTRIB
            | IN_CLOSE_WRITE
            | IN_DELETE_SELF
            | IN_MOVE_SELF;
        let wd = unsafe { inotify_add_watch(self.fd, c_path.as_ptr(), mask) };
        if wd < 0 {
            return Err(format!(
                "inotify_add_watch({}) failed: {}",
                dir.display(),
                std::io::Error::last_os_error()
            ));
        }

        let dir = dir.to_path_buf();
        self.wd_to_entry
            .insert(wd, (mount.to_string(), dir.clone()));
        self.path_to_wd.insert(dir, wd);
        Ok(())
    }
}

/// One decoded inotify record, before rename pairing.
struct Decoded {
    mount: String,
    path: PathBuf,
    mask: u32,
    cookie: u32,
}

/// Map one `read` worth of decoded records to events, in kernel order.
/// `IN_MOVED_FROM` / `IN_MOVED_TO` with the same cookie pair into
/// `Renamed`; identical (mount, path, kind) repeats within the read collapse
/// to one through a hash set.
fn classify(records: &[Decoded]) -> Vec<(String, PathBuf, FileSystemEventKind, Option<bool>)> {
    let mut out: Vec<(String, PathBuf, FileSystemEventKind, Option<bool>)> = Vec::new();
    let mut seen: HashSet<(String, PathBuf, u8)> = HashSet::new();
    let mut consumed = vec![false; records.len()];
    for i in 0..records.len() {
        if consumed[i] {
            continue;
        }
        let record = &records[i];
        let is_dir = Some(record.mask & IN_ISDIR != 0);
        let kind = if record.mask & IN_MOVED_FROM != 0 {
            let pair = (i + 1..records.len()).find(|&j| {
                !consumed[j]
                    && records[j].mask & IN_MOVED_TO != 0
                    && records[j].cookie == record.cookie
                    && records[j].mount == record.mount
            });
            match pair {
                Some(j) => {
                    consumed[j] = true;
                    out.push((
                        record.mount.clone(),
                        records[j].path.clone(),
                        FileSystemEventKind::Renamed {
                            from: record.path.clone(),
                        },
                        Some(records[j].mask & IN_ISDIR != 0),
                    ));
                    continue;
                }
                None => FileSystemEventKind::Removed,
            }
        } else if record.mask & IN_MOVED_TO != 0 {
            FileSystemEventKind::Created
        } else if record.mask & IN_CREATE != 0 {
            FileSystemEventKind::Created
        } else if record.mask & (IN_DELETE | IN_DELETE_SELF | IN_MOVE_SELF) != 0 {
            FileSystemEventKind::Removed
        } else if record.mask & (IN_MODIFY | IN_CLOSE_WRITE | IN_ATTRIB) != 0 {
            FileSystemEventKind::Changed
        } else {
            continue;
        };
        let tag = match kind {
            FileSystemEventKind::Changed => 0u8,
            FileSystemEventKind::Created => 1,
            FileSystemEventKind::Removed => 2,
            FileSystemEventKind::Renamed { .. } => 3,
            FileSystemEventKind::RescanRequired { .. } => 4,
        };
        if seen.insert((record.mount.clone(), record.path.clone(), tag)) {
            out.push((record.mount.clone(), record.path.clone(), kind, is_dir));
        }
    }
    out
}

fn run_loop(mut table: WatchTable, stop: Arc<AtomicBool>, emitter: Arc<Emitter>) {
    let mut buffer = vec![0u8; 64 * 1024];
    // Wall-clock of the last successful read: the lower bound of the
    // interval an overflow may have lost.
    let mut last_read_ok = SystemTime::now();
    while !stop.load(Ordering::Relaxed) {
        let read_len = unsafe { read(table.fd, buffer.as_mut_ptr() as *mut c_void, buffer.len()) };
        if read_len < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::WouldBlock {
                thread::sleep(Duration::from_millis(20));
            } else {
                thread::sleep(Duration::from_millis(80));
            }
            continue;
        }
        if read_len == 0 {
            thread::sleep(Duration::from_millis(80));
            continue;
        }

        let mut touched_mounts = HashSet::new();
        let mut overflow = false;
        let mut records = Vec::<Decoded>::new();
        let mut offset = 0usize;
        let end = read_len as usize;
        while offset + size_of::<InotifyEvent>() <= end {
            let event = unsafe { &*(buffer.as_ptr().add(offset) as *const InotifyEvent) };
            offset += size_of::<InotifyEvent>();
            let name_len = event.len as usize;
            let name_end = (offset + name_len).min(end);
            let name_bytes = &buffer[offset..name_end];
            offset = name_end;
            if event.mask & IN_Q_OVERFLOW != 0 {
                overflow = true;
                continue;
            }
            if event.mask & IN_IGNORED != 0 {
                continue;
            }
            if let Some((mount, watched_dir)) = table.wd_to_entry.get(&event.wd) {
                let changed_path = changed_path_for_event(watched_dir, name_bytes);
                records.push(Decoded {
                    mount: mount.clone(),
                    path: changed_path,
                    mask: event.mask,
                    cookie: event.cookie,
                });
                if event_requires_rescan(event.mask) {
                    touched_mounts.insert(mount.clone());
                }
            }
        }

        if overflow {
            // Events were lost, possibly directory creations: rebuild the
            // watch coverage of every root before saying so, and bound the
            // interval the consumer must reconcile.
            let now = SystemTime::now();
            let roots: Vec<(String, PathBuf)> = table
                .roots
                .iter()
                .map(|(mount, path)| (mount.clone(), path.clone()))
                .collect();
            for (mount, _) in &roots {
                let _ = table.rescan_mount(mount);
            }
            for (mount, root) in roots {
                emitter.emit(
                    mount,
                    root.clone(),
                    FileSystemEventKind::RescanRequired {
                        root,
                        reason: RescanReason::Overflow,
                        missing: Some((last_read_ok, now)),
                    },
                    Some(true),
                );
            }
            touched_mounts.clear();
        }

        for mount in touched_mounts {
            let _ = table.rescan_mount(&mount);
        }

        for (mount, path, kind, is_dir) in classify(&records) {
            let excluded = table
                .roots
                .get(&mount)
                .is_some_and(|root| table.exclude.excludes(root, &path, is_dir == Some(true)));
            if excluded {
                continue;
            }
            emitter.emit(mount, path, kind, is_dir);
        }
        last_read_ok = SystemTime::now();
    }

    table.close_all();
}

fn event_requires_rescan(mask: u32) -> bool {
    (mask & (IN_DELETE_SELF | IN_MOVE_SELF)) != 0
        || ((mask & IN_ISDIR) != 0
            && (mask & (IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO)) != 0)
}

fn changed_path_for_event(watched_dir: &Path, name_bytes: &[u8]) -> PathBuf {
    let name_len = name_bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name_bytes.len());
    if name_len == 0 {
        return watched_dir.to_path_buf();
    }
    watched_dir.join(Path::new(std::ffi::OsStr::from_bytes(
        &name_bytes[..name_len],
    )))
}

fn collect_dirs(root: &Path, exclude: &ExcludePolicy, out: &mut Vec<PathBuf>) {
    if !root.is_dir() {
        return;
    }
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        out.push(dir.clone());
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let child = entry.path();
            if exclude.excludes_dir(root, &child) {
                continue;
            }
            stack.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(path: &str, mask: u32, cookie: u32) -> Decoded {
        Decoded {
            mount: "m".into(),
            path: PathBuf::from(path),
            mask,
            cookie,
        }
    }

    #[test]
    fn file_event_path_uses_directory_and_name() {
        let path = changed_path_for_event(Path::new("/tmp/project/src"), b"main.rs\0\0");
        assert_eq!(path, PathBuf::from("/tmp/project/src/main.rs"));
    }

    #[test]
    fn self_event_path_falls_back_to_watched_directory() {
        let path = changed_path_for_event(Path::new("/tmp/project/src"), b"");
        assert_eq!(path, PathBuf::from("/tmp/project/src"));
    }

    #[test]
    fn rescans_when_directory_tree_changes() {
        assert!(event_requires_rescan(IN_CREATE | IN_ISDIR));
        assert!(event_requires_rescan(IN_MOVED_TO | IN_ISDIR));
        assert!(event_requires_rescan(IN_DELETE_SELF));
        assert!(!event_requires_rescan(IN_CLOSE_WRITE));
        assert!(!event_requires_rescan(IN_MODIFY));
    }

    #[test]
    fn masks_map_to_kinds_and_cookies_pair_renames() {
        let out = classify(&[
            decoded("/w/a.rs", IN_CREATE, 0),
            decoded("/w/a.rs", IN_MODIFY, 0),
            decoded("/w/a.rs", IN_MODIFY, 0),
            decoded("/w/a.rs", IN_CLOSE_WRITE, 0),
            decoded("/w/a.rs", IN_MOVED_FROM, 7),
            decoded("/w/b.rs", IN_MOVED_TO, 7),
            decoded("/w/c.rs", IN_MOVED_FROM, 9),
            decoded("/w/d.rs", IN_MOVED_TO, 11),
            decoded("/w/sub", IN_DELETE | IN_ISDIR, 0),
        ]);
        let kinds: Vec<_> = out.iter().map(|(_, p, k, d)| (p.clone(), k.clone(), *d)).collect();
        assert_eq!(kinds[0], (PathBuf::from("/w/a.rs"), FileSystemEventKind::Created, Some(false)));
        assert_eq!(kinds[1], (PathBuf::from("/w/a.rs"), FileSystemEventKind::Changed, Some(false)));
        assert_eq!(
            kinds[2],
            (
                PathBuf::from("/w/b.rs"),
                FileSystemEventKind::Renamed {
                    from: PathBuf::from("/w/a.rs")
                },
                Some(false)
            )
        );
        assert_eq!(kinds[3], (PathBuf::from("/w/c.rs"), FileSystemEventKind::Removed, Some(false)));
        assert_eq!(kinds[4], (PathBuf::from("/w/d.rs"), FileSystemEventKind::Created, Some(false)));
        assert_eq!(kinds[5], (PathBuf::from("/w/sub"), FileSystemEventKind::Removed, Some(true)));
        assert_eq!(kinds.len(), 6, "repeated IN_MODIFY within one read collapses");
    }

    #[test]
    fn excluded_directories_are_never_walked() {
        let dir = std::env::temp_dir().join(format!(
            "fswatch-collect-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("target-wasm/debug")).unwrap();
        std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
        let mut dirs = Vec::new();
        collect_dirs(&dir, &ExcludePolicy::default(), &mut dirs);
        assert!(dirs.contains(&dir.join("src/deep")));
        assert!(!dirs.iter().any(|d| d.starts_with(dir.join("target-wasm"))));
        assert!(!dirs.iter().any(|d| d.starts_with(dir.join(".git"))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
