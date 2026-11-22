use crate::{Emitter, ExcludePolicy, FileSystemEventKind, RescanReason, WatchRoot};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::{c_void, CString, OsString};
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::fd::RawFd;
use std::os::raw::{c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

const O_NONBLOCK: c_int = 0o00004000;
const O_CLOEXEC: c_int = 0o2000000;
const ENOSPC: c_int = 28;

#[cfg(not(test))]
const POLL_MIN: Duration = Duration::from_secs(1);
#[cfg(test)]
const POLL_MIN: Duration = Duration::from_millis(50);

type Changes = Vec<(String, PathBuf, FileSystemEventKind, Option<bool>)>;

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
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn eventfd(initval: u32, flags: c_int) -> c_int;
    fn poll(fds: *mut PollFd, nfds: std::os::raw::c_ulong, timeout: c_int) -> c_int;
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 1;

pub struct PlatformWatcher {
    stop: Arc<AtomicBool>,
    /// an eventfd `stop` writes to: the watcher thread sleeps in poll() on it and the inotify fd
    /// (-1: none could be made, the thread looks at `stop` every 80 ms)
    wake: RawFd,
    thread: Option<JoinHandle<()>>,
}

impl PlatformWatcher {
    pub fn start(roots: Vec<WatchRoot>, emitter: Arc<Emitter>) -> Result<Self, String> {
        Self::start_table(WatchTable::new(roots, emitter.exclude().clone())?, emitter)
    }

    #[cfg(test)]
    pub(crate) fn start_with_watch_budget(
        roots: Vec<WatchRoot>,
        emitter: Arc<Emitter>,
        budget: usize,
    ) -> Result<Self, String> {
        let mut table = WatchTable::new(roots, emitter.exclude().clone())?;
        table.watch_budget = Some(budget);
        Self::start_table(table, emitter)
    }

    fn start_table(mut table: WatchTable, emitter: Arc<Emitter>) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        // Install each watch or its polling inventory before returning:
        // the caller is watching from here.
        let mounts: Vec<String> = table.roots.keys().cloned().collect();
        let mut changes = Vec::new();
        for mount in mounts {
            table.rescan_mount(&mount, &mut changes);
        }
        table.emit_changes(changes, &emitter);
        table.log_polling(false, POLL_MIN);

        let wake = unsafe { eventfd(0, O_NONBLOCK | O_CLOEXEC) };
        let thread = thread::Builder::new()
            .name("fswatch-linux".to_string())
            .spawn(move || run_loop(table, stop_thread, wake, emitter))
            .map_err(|err| {
                if wake >= 0 {
                    unsafe { close(wake) };
                }
                format!("failed to spawn linux watcher thread: {}", err)
            })?;

        Ok(Self {
            stop,
            wake,
            thread: Some(thread),
        })
    }

    /// A backend that watches nothing; `stop` is a no-op.
    pub fn idle() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            wake: -1,
            thread: None,
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if self.wake >= 0 {
            let one = 1u64;
            let _ = unsafe { write(self.wake, &one as *const u64 as *const c_void, 8) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if self.wake >= 0 {
            let _ = unsafe { close(self.wake) };
            self.wake = -1;
        }
    }
}

struct WatchTable {
    fd: RawFd,
    roots: HashMap<String, PathBuf>,
    exclude: ExcludePolicy,
    wd_to_entry: HashMap<i32, (String, PathBuf)>,
    path_to_wd: HashMap<PathBuf, i32>,
    unwatched: BTreeMap<PathBuf, Polled>,
    #[cfg(test)]
    watch_budget: Option<usize>,
}

enum Added {
    Watched,
    NoWatches,
    Skipped,
}

#[derive(PartialEq, Eq)]
struct Entry {
    is_dir: bool,
    len: u64,
    modified: Option<SystemTime>,
}

struct Polled {
    mount: String,
    entries: BTreeMap<OsString, Entry>,
}

impl Polled {
    /// Return whether directory coverage needs reconciling. Events wait
    /// until then so a consumer can write inside a newly reported folder.
    fn diff(&self, dir: &Path, entries: &BTreeMap<OsString, Entry>, changes: &mut Changes) -> bool {
        let mut directories_changed = false;
        for (name, entry) in entries {
            let kind = match self.entries.get(name) {
                None => {
                    directories_changed |= entry.is_dir;
                    FileSystemEventKind::Created
                }
                Some(old) if old != entry => {
                    directories_changed |= old.is_dir != entry.is_dir;
                    FileSystemEventKind::Changed
                }
                Some(_) => continue,
            };
            changes.push((self.mount.clone(), dir.join(name), kind, Some(entry.is_dir)));
        }
        for (name, entry) in &self.entries {
            if !entries.contains_key(name) {
                directories_changed |= entry.is_dir;
                changes.push((
                    self.mount.clone(),
                    dir.join(name),
                    FileSystemEventKind::Removed,
                    Some(entry.is_dir),
                ));
            }
        }
        directories_changed
    }
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
            unwatched: BTreeMap::new(),
            #[cfg(test)]
            watch_budget: None,
        })
    }

    fn close_all(&mut self) {
        let wds: Vec<i32> = self.wd_to_entry.keys().copied().collect();
        for wd in wds {
            let _ = unsafe { inotify_rm_watch(self.fd, wd) };
            self.wd_to_entry.remove(&wd);
        }
        self.path_to_wd.clear();
        self.unwatched.clear();
        if self.fd >= 0 {
            let _ = unsafe { close(self.fd) };
            self.fd = -1;
        }
    }

    /// Reconcile the watch coverage of one root with the directories that
    /// exist now: stale watches go, missing ones are added.
    fn rescan_mount(&mut self, mount: &str, changes: &mut Changes) {
        let Some(root) = self.roots.get(mount).cloned() else {
            return;
        };
        loop {
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
            self.unwatched
                .retain(|path, polled| polled.mount != mount || wanted.contains(path));

            let mut wanted_sorted: Vec<PathBuf> = wanted.into_iter().collect();
            wanted_sorted.sort();
            let mut directories_changed = false;
            for dir in wanted_sorted {
                let (_, changed) = self.cover_dir(mount, &dir, changes);
                directories_changed |= changed;
            }
            // A recovered watch may reveal a directory created after collect_dirs.
            if !directories_changed {
                break;
            }
        }
    }

    fn remove_path(&mut self, path: &Path) {
        let Some(wd) = self.path_to_wd.remove(path) else {
            return;
        };
        self.wd_to_entry.remove(&wd);
        let _ = unsafe { inotify_rm_watch(self.fd, wd) };
    }

    fn add_dir(&mut self, mount: &str, dir: &Path) -> Added {
        if self.path_to_wd.contains_key(dir) {
            return Added::Watched;
        }
        #[cfg(test)]
        if self
            .watch_budget
            .is_some_and(|budget| self.path_to_wd.len() >= budget)
        {
            return Added::NoWatches;
        }
        let c_path = match CString::new(dir.as_os_str().as_bytes()) {
            Ok(path) => path,
            Err(err) => {
                eprintln!(
                    "filesystem watcher: inotify_add_watch({}) failed: {}",
                    dir.display(), err
                );
                return Added::Skipped;
            }
        };
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
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ENOSPC) {
                return Added::NoWatches;
            }
            if !matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory | io::ErrorKind::PermissionDenied
            ) {
                eprintln!(
                    "filesystem watcher: inotify_add_watch({}) failed: {}",
                    dir.display(), err
                );
            }
            return Added::Skipped;
        }

        let dir = dir.to_path_buf();
        self.wd_to_entry
            .insert(wd, (mount.to_string(), dir.clone()));
        self.path_to_wd.insert(dir, wd);
        Added::Watched
    }

    fn cover_dir(&mut self, mount: &str, dir: &Path, changes: &mut Changes) -> (Added, bool) {
        let added = self.add_dir(mount, dir);
        let mut directories_changed = false;
        match added {
            Added::Watched => {
                directories_changed = self.refresh_polled(dir, changes);
            }
            Added::NoWatches => {
                if !self.unwatched.contains_key(dir) {
                    let entries = match read_entries(dir, &self.roots[mount], &self.exclude) {
                        Ok(entries) => entries,
                        Err(err)
                            if matches!(
                                err.kind(),
                                io::ErrorKind::NotFound
                                    | io::ErrorKind::NotADirectory
                                    | io::ErrorKind::PermissionDenied
                            ) =>
                        {
                            return (Added::Skipped, false);
                        }
                        // Keep retrying an unreadable initial inventory, rather
                        // than losing coverage after a transient I/O failure.
                        Err(_) => BTreeMap::new(),
                    };
                    self.unwatched.insert(
                        dir.to_path_buf(),
                        Polled { mount: mount.to_string(), entries },
                    );
                }
            }
            Added::Skipped => {
                self.unwatched.remove(dir);
            }
        }
        (added, directories_changed)
    }

    fn refresh_polled(&mut self, dir: &Path, changes: &mut Changes) -> bool {
        let Some(polled) = self.unwatched.get_mut(dir) else {
            return false;
        };
        match read_entries(dir, &self.roots[&polled.mount], &self.exclude) {
            Ok(entries) => {
                let directories_changed = polled.diff(dir, &entries, changes);
                polled.entries = entries;
                if self.path_to_wd.contains_key(dir) {
                    self.unwatched.remove(dir);
                }
                directories_changed
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.unwatched.remove(dir);
                false
            }
            // Preserve the last complete inventory on failure, including
            // after installing a watch, until its final diff can be read.
            Err(_) => false,
        }
    }

    fn poll_unwatched(&mut self, emitter: &Emitter) {
        let mut changes = Vec::new();
        let mut touched_mounts = HashSet::new();
        let retry: Vec<_> = self
            .unwatched
            .iter()
            .map(|(dir, polled)| (dir.clone(), polled.mount.clone()))
            .collect();
        for (dir, mount) in retry {
            let (added, changed) = self.cover_dir(&mount, &dir, &mut changes);
            if changed {
                touched_mounts.insert(mount);
            }
            if matches!(added, Added::NoWatches) {
                break;
            }
        }
        let remaining: Vec<_> = self
            .unwatched
            .iter()
            .map(|(dir, polled)| (dir.clone(), polled.mount.clone()))
            .collect();
        for (dir, mount) in remaining {
            if self.refresh_polled(&dir, &mut changes) {
                touched_mounts.insert(mount);
            }
        }
        for mount in touched_mounts {
            self.rescan_mount(&mount, &mut changes);
        }
        self.emit_changes(changes, emitter);
    }

    fn emit_changes(&self, changes: Changes, emitter: &Emitter) {
        for (mount, path, kind, is_dir) in changes {
            let excluded = self
                .roots
                .get(&mount)
                .is_some_and(|root| self.exclude.excludes(root, &path, is_dir == Some(true)));
            if !excluded {
                emitter.emit(mount, path, kind, is_dir);
            }
        }
    }

    fn log_polling(&self, was_polling: bool, interval: Duration) {
        let polling = !self.unwatched.is_empty();
        if was_polling == polling {
            return;
        }
        if let Some((_, polled)) = self.unwatched.first_key_value() {
            eprintln!(
                "filesystem watcher: inotify has no watches left (fs.inotify.max_user_watches): {} folders under {} are polled every {} ms instead",
                self.unwatched.len(), self.roots[&polled.mount].display(), interval.as_millis()
            );
        } else {
            eprintln!("filesystem watcher: every folder is watched again");
        }
    }
}

fn read_entries(
    dir: &Path,
    root: &Path,
    exclude: &ExcludePolicy,
) -> io::Result<BTreeMap<OsString, Entry>> {
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.file_type().and_then(|file_type| {
            if exclude.excludes(root, &path, file_type.is_dir()) {
                Ok(None)
            } else {
                fs::symlink_metadata(&path).map(Some)
            }
        }) {
            Ok(Some(metadata)) => metadata,
            Ok(None) => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        entries.insert(
            entry.file_name(),
            Entry {
                is_dir: metadata.is_dir(),
                len: metadata.len(),
                modified: metadata.modified().ok(),
            },
        );
    }
    Ok(entries)
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

fn run_loop(mut table: WatchTable, stop: Arc<AtomicBool>, wake: RawFd, emitter: Arc<Emitter>) {
    let mut buffer = vec![0u8; 64 * 1024];
    let mut poll_interval = POLL_MIN;
    let mut next_poll = Instant::now() + poll_interval;
    let mut polling = !table.unwatched.is_empty();
    // Wall-clock of the last successful read: the lower bound of the
    // interval an overflow may have lost.
    let mut last_read_ok = SystemTime::now();
    while !stop.load(Ordering::Relaxed) {
        if !polling && !table.unwatched.is_empty() {
            next_poll = Instant::now() + poll_interval;
        }
        if !table.unwatched.is_empty() && Instant::now() >= next_poll {
            let started = Instant::now();
            table.poll_unwatched(&emitter);
            poll_interval = POLL_MIN.max(started.elapsed().saturating_mul(20));
            next_poll = Instant::now() + poll_interval;
        }
        table.log_polling(polling, poll_interval);
        polling = !table.unwatched.is_empty();
        let read_len = unsafe { read(table.fd, buffer.as_mut_ptr() as *mut c_void, buffer.len()) };
        if read_len < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::WouldBlock {
                thread::sleep(Duration::from_millis(20));
            } else {
                // Sleep until an event, stop, or the next inventory deadline.
                let mut fds = [
                    PollFd { fd: table.fd, events: POLLIN, revents: 0 },
                    PollFd { fd: wake, events: POLLIN, revents: 0 },
                ];
                let n = if wake >= 0 { 2 } else { 1 };
                let mut timeout = if polling {
                    let remaining = next_poll.saturating_duration_since(Instant::now());
                    // poll takes signed milliseconds; longer intervals simply
                    // sleep again until the same deadline, without truncating it.
                    c_int::try_from(remaining.as_millis().max(1)).unwrap_or(c_int::MAX)
                } else {
                    -1
                };
                if wake < 0 {
                    timeout = if timeout < 0 { 80 } else { timeout.min(80) };
                }
                let _ = unsafe { poll(fds.as_mut_ptr(), n, timeout) };
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

        let mut changes = Vec::new();
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
                table.rescan_mount(mount, &mut changes);
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
            table.rescan_mount(&mount, &mut changes);
        }

        changes.extend(classify(&records));
        table.emit_changes(changes, &emitter);
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
    use crate::{FileSystemEvent, Journal, JournalConfig};
    use std::sync::mpsc;

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

    #[test]
    fn exhausted_watches_poll_changes_and_extend_directory_coverage() {
        struct Scratch {
            dir: PathBuf,
            watcher: Option<PlatformWatcher>,
        }
        impl Drop for Scratch {
            fn drop(&mut self) {
                if let Some(watcher) = self.watcher.as_mut() {
                    watcher.stop();
                }
                let _ = fs::remove_dir_all(&self.dir);
            }
        }

        let mut scratch = Scratch {
            dir: std::env::temp_dir().join(format!(
                "fswatch-budget-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
            watcher: None,
        };
        fs::create_dir_all(scratch.dir.join("a/deep")).unwrap();
        fs::create_dir_all(scratch.dir.join("b")).unwrap();
        scratch.dir = fs::canonicalize(&scratch.dir).unwrap();
        for dir in ["", "a", "a/deep", "b"] {
            fs::write(scratch.dir.join(dir).join("existing.txt"), b"initial").unwrap();
        }

        let (tx, rx) = mpsc::channel();
        let journal = Journal::new(
            JournalConfig::default(),
            Arc::new(move |event: &FileSystemEvent| tx.send(event.clone()).is_ok()),
        )
        .unwrap();
        let emitter = Arc::new(Emitter::new(ExcludePolicy::default(), journal));
        let roots = vec![WatchRoot {
            mount: "budget".into(),
            path: scratch.dir.clone(),
        }];
        emitter.set_roots(roots.clone());
        scratch.watcher = Some(PlatformWatcher::start_with_watch_budget(roots, emitter, 2).unwrap());

        // Each change waits for its event, however long the box takes between poll passes (the
        // interval grows with a pass's time, 20x, and one 1.5 s deadline for the whole sequence
        // ran out on a busy box). The watcher's thread holds the only sender: if it ends, the
        // wait ends with it.
        let mut seen = Vec::new();
        let mut expect = |path: &Path, kind: FileSystemEventKind, is_dir: bool| loop {
            let event = rx.recv().unwrap_or_else(|err| {
                panic!("polling must report {kind:?} for {}: the watcher ended first ({err})", path.display())
            });
            let found = event.path == path && event.kind == kind;
            seen.push(event.clone());
            if found {
                assert_eq!(
                    event.is_dir, Some(is_dir),
                    "polled event must carry its entry's directory flag"
                );
                break;
            }
        };

        // The root and a use both watches; a/deep and b must both be polled.
        let file = scratch.dir.join("b/new.txt");
        fs::write(&file, b"created").unwrap();
        expect(&file, FileSystemEventKind::Created, false);
        fs::write(&file, b"rewritten with different bytes and length").unwrap();
        expect(&file, FileSystemEventKind::Changed, false);
        fs::remove_file(&file).unwrap();
        expect(&file, FileSystemEventKind::Removed, false);

        let deep_file = scratch.dir.join("a/deep/new.txt");
        fs::write(&deep_file, b"also polled").unwrap();
        expect(&deep_file, FileSystemEventKind::Created, false);

        let subdir = scratch.dir.join("b/new");
        fs::create_dir(&subdir).unwrap();
        expect(&subdir, FileSystemEventKind::Created, true);
        let nested_file = subdir.join("inside.txt");
        fs::write(&nested_file, b"covered before the directory event arrives").unwrap();
        expect(&nested_file, FileSystemEventKind::Created, false);

        let excluded = scratch.dir.join("b/target");
        fs::create_dir_all(excluded.join("deep")).unwrap();
        fs::write(excluded.join("deep/hidden.txt"), b"excluded").unwrap();
        // Two observed polls prove exclusion without a fixed sleep.
        let visible = scratch.dir.join("b/visible.txt");
        fs::write(&visible, b"first").unwrap();
        expect(&visible, FileSystemEventKind::Created, false);
        fs::write(&visible, b"second poll").unwrap();
        expect(&visible, FileSystemEventKind::Changed, false);

        scratch.watcher.as_mut().unwrap().stop();
        seen.extend(rx.try_iter());
        assert!(
            !seen.iter().any(|event| event.path.starts_with(&excluded)),
            "excluded directories and their descendants must not emit polled events"
        );
        assert!(
            !seen.iter().any(|event| event.path.file_name()
                .is_some_and(|name| name == "existing.txt")),
            "the initial polling inventories must not emit existing entries"
        );
    }
}
