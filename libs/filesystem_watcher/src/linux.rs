use crate::{FileSystemEvent, FileSystemEventKind, WatchCallback, WatchRoot};
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
use std::time::Duration;

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
    pub fn start(roots: Vec<WatchRoot>, on_event: WatchCallback) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let thread = thread::Builder::new()
            .name("fswatch-linux".to_string())
            .spawn(move || run_loop(roots, stop_thread, on_event))
            .map_err(|err| format!("failed to spawn linux watcher thread: {}", err))?;

        Ok(Self {
            stop,
            thread: Some(thread),
        })
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
    wd_to_entry: HashMap<i32, (String, PathBuf)>,
    path_to_wd: HashMap<PathBuf, i32>,
}

impl WatchTable {
    fn new(roots: Vec<WatchRoot>) -> Result<Self, String> {
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

    fn rescan_mount(&mut self, mount: &str) -> Result<(), String> {
        let Some(root) = self.roots.get(mount) else {
            return Ok(());
        };
        let mut dirs = Vec::new();
        collect_dirs(root, &mut dirs);
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

fn run_loop(roots: Vec<WatchRoot>, stop: Arc<AtomicBool>, on_event: WatchCallback) {
    let Ok(mut table) = WatchTable::new(roots) else {
        return;
    };

    let mounts: Vec<String> = table.roots.keys().cloned().collect();
    for mount in mounts {
        let _ = table.rescan_mount(&mount);
    }

    let mut buffer = vec![0u8; 64 * 1024];
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
        let mut changed_paths = Vec::<(String, PathBuf)>::new();
        let mut offset = 0usize;
        let end = read_len as usize;
        while offset + size_of::<InotifyEvent>() <= end {
            let event = unsafe { &*(buffer.as_ptr().add(offset) as *const InotifyEvent) };
            offset += size_of::<InotifyEvent>();
            let name_len = event.len as usize;
            let name_end = (offset + name_len).min(end);
            let name_bytes = &buffer[offset..name_end];
            offset = name_end;
            if let Some((mount, watched_dir)) = table.wd_to_entry.get(&event.wd) {
                let changed_path = changed_path_for_event(watched_dir, name_bytes);
                push_unique_change(&mut changed_paths, mount.clone(), changed_path);
                if event_requires_rescan(event.mask) {
                    touched_mounts.insert(mount.clone());
                }
            }
        }

        for mount in touched_mounts {
            let _ = table.rescan_mount(&mount);
        }

        for (mount, path) in changed_paths {
            on_event(FileSystemEvent {
                mount,
                path,
                kind: FileSystemEventKind::Changed,
            });
        }
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
    watched_dir.join(Path::new(std::ffi::OsStr::from_bytes(&name_bytes[..name_len])))
}

fn push_unique_change(changes: &mut Vec<(String, PathBuf)>, mount: String, path: PathBuf) {
    if changes
        .iter()
        .any(|(existing_mount, existing_path)| existing_mount == &mount && existing_path == &path)
    {
        return;
    }
    changes.push((mount, path));
}

fn collect_dirs(root: &Path, out: &mut Vec<PathBuf>) {
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
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" || name == "target" {
                continue;
            }
            stack.push(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
