use crate::{FileSystemEvent, FileSystemEventKind, WatchCallback, WatchRoot};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CString};
use std::fs;
use std::os::fd::RawFd;
use std::os::raw::{c_char, c_int, c_long};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const O_EVTONLY: c_int = 0x0000_8000;

const EV_ADD: u16 = 0x0001;
const EV_ENABLE: u16 = 0x0004;
const EV_DELETE: u16 = 0x0002;
const EV_CLEAR: u16 = 0x0020;
const EVFILT_VNODE: i16 = -4;

const NOTE_DELETE: u32 = 0x0000_0001;
const NOTE_WRITE: u32 = 0x0000_0002;
const NOTE_EXTEND: u32 = 0x0000_0004;
const NOTE_ATTRIB: u32 = 0x0000_0008;
const NOTE_LINK: u32 = 0x0000_0010;
const NOTE_RENAME: u32 = 0x0000_0020;
const NOTE_REVOKE: u32 = 0x0000_0040;

#[repr(C)]
#[derive(Clone, Copy)]
struct KEvent {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: isize,
    udata: *mut c_void,
}

impl Default for KEvent {
    fn default() -> Self {
        Self {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        }
    }
}

#[repr(C)]
struct Timespec {
    tv_sec: c_long,
    tv_nsec: c_long,
}

unsafe extern "C" {
    fn kqueue() -> c_int;
    fn kevent(
        kq: c_int,
        changelist: *const KEvent,
        nchanges: c_int,
        eventlist: *mut KEvent,
        nevents: c_int,
        timeout: *const Timespec,
    ) -> c_int;
    fn open(pathname: *const c_char, flags: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
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
            .name("fswatch-macos".to_string())
            .spawn(move || run_loop(roots, stop_thread, on_event))
            .map_err(|err| format!("failed to spawn macos watcher thread: {}", err))?;

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
    kq: RawFd,
    roots: HashMap<String, PathBuf>,
    fd_to_entry: HashMap<RawFd, (String, PathBuf)>,
    path_to_fd: HashMap<PathBuf, RawFd>,
}

impl WatchTable {
    fn new(roots: Vec<WatchRoot>) -> Result<Self, String> {
        let kq = unsafe { kqueue() };
        if kq < 0 {
            return Err(format!(
                "kqueue failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let mut root_map = HashMap::new();
        for root in roots {
            root_map.insert(root.mount, root.path);
        }

        Ok(Self {
            kq,
            roots: root_map,
            fd_to_entry: HashMap::new(),
            path_to_fd: HashMap::new(),
        })
    }

    fn close_all(&mut self) {
        let paths: Vec<PathBuf> = self.path_to_fd.keys().cloned().collect();
        for path in paths {
            self.remove_path(&path);
        }
        if self.kq >= 0 {
            let _ = unsafe { close(self.kq) };
            self.kq = -1;
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
            .fd_to_entry
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
        let Some(fd) = self.path_to_fd.remove(path) else {
            return;
        };
        self.fd_to_entry.remove(&fd);

        let change = KEvent {
            ident: fd as usize,
            filter: EVFILT_VNODE,
            flags: EV_DELETE,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let _ = unsafe {
            kevent(
                self.kq,
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        let _ = unsafe { close(fd) };
    }

    fn add_dir(&mut self, mount: &str, dir: &Path) -> Result<(), String> {
        if self.path_to_fd.contains_key(dir) {
            return Ok(());
        }

        let c_path = CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| format!("path contains interior NUL byte: {}", dir.display()))?;
        let fd = unsafe { open(c_path.as_ptr(), O_EVTONLY) };
        if fd < 0 {
            return Err(format!(
                "open({}) failed: {}",
                dir.display(),
                std::io::Error::last_os_error()
            ));
        }

        let change = KEvent {
            ident: fd as usize,
            filter: EVFILT_VNODE,
            flags: EV_ADD | EV_ENABLE | EV_CLEAR,
            fflags: NOTE_DELETE
                | NOTE_WRITE
                | NOTE_EXTEND
                | NOTE_ATTRIB
                | NOTE_LINK
                | NOTE_RENAME
                | NOTE_REVOKE,
            data: 0,
            udata: std::ptr::null_mut(),
        };

        let res = unsafe {
            kevent(
                self.kq,
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if res < 0 {
            let err = std::io::Error::last_os_error();
            let _ = unsafe { close(fd) };
            return Err(format!(
                "kevent register failed for {}: {}",
                dir.display(),
                err
            ));
        }

        let dir = dir.to_path_buf();
        self.fd_to_entry
            .insert(fd, (mount.to_string(), dir.clone()));
        self.path_to_fd.insert(dir, fd);
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

    let mut events = vec![KEvent::default(); 512];
    while !stop.load(Ordering::Relaxed) {
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 120_000_000,
        };

        let count = unsafe {
            kevent(
                table.kq,
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as c_int,
                &timeout,
            )
        };

        if count < 0 {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        if count == 0 {
            continue;
        }

        let mut touched_mounts = HashSet::new();
        for event in events.iter().take(count as usize) {
            let fd = event.ident as RawFd;
            if let Some((mount, _)) = table.fd_to_entry.get(&fd) {
                touched_mounts.insert(mount.clone());
            }
        }

        for mount in touched_mounts {
            let root_path = table.roots.get(&mount).cloned();
            let _ = table.rescan_mount(&mount);
            if let Some(path) = root_path {
                on_event(FileSystemEvent {
                    mount: mount.clone(),
                    path,
                    kind: FileSystemEventKind::Changed,
                });
            }
        }
    }

    table.close_all();
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
