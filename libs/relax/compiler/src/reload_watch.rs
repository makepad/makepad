//! Directory notifications retain watches across editor atomic-save renames. Startup is
//! proportional to the watch set; normal drains visit only delivered source-file events.
//! Linux: inotify, macOS: FSEvents file events, Windows: ReadDirectoryChangesW. Other
//! platforms and lost events (queue overflow, a watched directory moved) keep the stat fallback.
//! `wait(None)` sleeps until a file event or a `kick`: a watcher at rest makes no system call.
/// A watched file whose folder is missing (removed by a checkout or a tool, maybe made again
/// later): the nearest folder above it that exists, and the name there of the next folder down
/// toward it. The watch listens for that name in that folder: its creation loses events (the
/// caller rescans and rebuilds the watch, which then watches the new folder). One missing folder
/// left every other file without native events: the watcher polled all of them, ~100 rounds a
/// second, for the rest of the run.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn nearest_existing(dir: &std::path::Path) -> Option<(&std::path::Path, &std::ffi::OsStr)> {
    let mut below = dir;
    for above in dir.ancestors().skip(1) {
        if above.is_dir() {
            return Some((above, below.file_name()?));
        }
        below = above;
    }
    None
}

#[cfg(target_os = "linux")]
mod native {
    use std::{collections::HashMap, ffi::CString, os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::ffi::OsStrExt}, path::Path};
    unsafe extern "C" {
        fn inotify_init1(flags: i32) -> i32;
        fn inotify_add_watch(fd: i32, path: *const std::ffi::c_char, mask: u32) -> i32;
        fn read(fd: i32, buf: *mut core::ffi::c_void, size: usize) -> isize;
        fn write(fd: i32, buf: *const core::ffi::c_void, size: usize) -> isize;
        fn eventfd(initval: u32, flags: i32) -> i32;
        fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: i32) -> i32;
    }
    #[repr(C)]
    struct PollFd { fd: i32, events: i16, revents: i16 }
    /// The eventfd `kick` writes to (-1: none could be made; `wait` then wakes on file events and
    /// its timeout only).
    fn kick_fd() -> i32 {
        static FD: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
        // EFD_NONBLOCK | EFD_CLOEXEC
        *FD.get_or_init(|| unsafe { eventfd(0, 0x800 | 0x80000) })
    }
    pub fn kick() {
        let fd = kick_fd();
        if fd >= 0 {
            let one = 1u64;
            unsafe { write(fd, (&one as *const u64).cast(), 8) };
        }
    }
    pub struct Watch {
        fd: OwnedFd,
        names: HashMap<(i32, Vec<u8>), usize>,
        directories: HashMap<i32, usize>,
        /// (folder, name): a missing folder on the way to watched files (`nearest_existing`)
        appears: std::collections::HashSet<(i32, Vec<u8>)>,
    }
    impl Watch {
        pub fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Option<Self> {
            let paths: Vec<_> = paths.collect();
            // Linux O_NONBLOCK | O_CLOEXEC; no descriptor escapes to app children.
            let fd = unsafe { inotify_init1(0x800 | 0x80000) };
            if fd < 0 { return None; }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut dirs = HashMap::new();
            let mut names = HashMap::new();
            let mut appears = std::collections::HashSet::new();
            // MODIFY, ATTRIB, CLOSE_WRITE, MOVED_FROM/TO, CREATE, DELETE,
            // DELETE_SELF and MOVE_SELF. Watching directories handles new inodes.
            let add = |dir: &Path| -> Option<i32> {
                let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
                let wd = unsafe { inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), 0xfce) };
                (wd >= 0).then_some(wd)
            };
            for (i, path) in paths.iter().enumerate() {
                let path = Path::new(path);
                let parent = path.parent()?;
                for dir in std::iter::once(parent).chain(path.is_dir().then_some(path)) {
                    if dirs.contains_key(dir) { continue; }
                    match add(dir) {
                        Some(wd) => { dirs.insert(dir.to_owned(), Some(wd)); }
                        None if dir.exists() => return None,
                        None => {
                            let (above, next) = super::nearest_existing(dir)?;
                            let wd = match dirs.get(above) { Some(&Some(wd)) => wd, _ => add(above)? };
                            dirs.insert(above.to_owned(), Some(wd));
                            appears.insert((wd, next.as_bytes().to_vec()));
                            dirs.insert(dir.to_owned(), None);
                        }
                    }
                }
                if let Some(wd) = dirs[parent] {
                    names.insert((wd, path.file_name()?.as_bytes().to_vec()), i);
                }
            }
            let directories = paths.iter().enumerate().filter_map(|(i, p)| {
                let p = Path::new(p);
                p.is_dir().then(|| dirs.get(p).copied().flatten().map(|wd| (wd, i))).flatten()
            }).collect();
            Some(Self { fd, names, directories, appears })
        }
        /// Sleeps until the inotify fd has events, `kick` is called or `timeout` passes (None: no
        /// timeout).
        pub fn wait(&mut self, timeout: Option<std::time::Duration>) {
            let k = kick_fd();
            let mut fds = [PollFd { fd: self.fd.as_raw_fd(), events: 1, revents: 0 }, PollFd { fd: k, events: 1, revents: 0 }];
            let ms = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
            unsafe { poll(fds.as_mut_ptr(), if k >= 0 { 2 } else { 1 }, ms) };
            if k >= 0 && fds[1].revents != 0 {
                let mut n = 0u64;
                unsafe { read(k, (&mut n as *mut u64).cast(), 8) };
            }
        }
        /// None means events may have been lost: stat every source this round.
        pub fn changed(&mut self) -> Option<Vec<usize>> {
            let mut changed = Vec::new();
            let mut lost = false;
            let mut buf = [0u8; 65536];
            loop {
                let n = unsafe { read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::WouldBlock { break; }
                    if e.kind() == std::io::ErrorKind::Interrupted { continue; }
                    return None;
                }
                if n == 0 { break; }
                let mut pos = 0;
                while pos + 16 <= n as usize {
                    let word = |off| u32::from_ne_bytes(buf[pos + off..pos + off + 4].try_into().unwrap());
                    let wd = word(0) as i32;
                    let mask = word(4);
                    let len = word(12) as usize;
                    if pos + 16 + len > n as usize { return None; }
                    // Queue overflow or a removed/moved watched directory needs rescan
                    // and watch reconstruction by the caller.
                    lost |= mask & (0x4000 | 0x8000 | 0x400 | 0x800) != 0;
                    let bytes = &buf[pos + 16..pos + 16 + len];
                    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
                    let name = (wd, bytes[..end].to_vec());
                    if let Some(&i) = self.names.get(&name) { changed.push(i); }
                    lost |= self.appears.contains(&name);
                    if let Some(&i) = self.directories.get(&wd) { changed.push(i); }
                    pos += 16 + len;
                }
            }
            if lost { return None; }
            changed.sort_unstable();
            changed.dedup();
            Some(changed)
        }
    }
}
#[cfg(target_os = "macos")]
mod native {
    use std::{collections::HashMap, ffi::{c_char, c_void, CStr, CString}, os::unix::ffi::OsStrExt, path::Path};
    use relax_session::sync::Mutex;
    type Stream = *mut c_void;
    type Callback = extern "C" fn(Stream, *mut c_void, usize, *const *const c_char, *const u32, *const u64);
    #[repr(C)]
    struct Context { version: isize, info: *mut c_void, retain: *const c_void, release: *const c_void, describe: *const c_void }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeArrayCallBacks: [usize; 5];
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, encoding: u32) -> *const c_void;
        fn CFArrayCreate(alloc: *const c_void, values: *const *const c_void, n: isize, callbacks: *const c_void) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn FSEventStreamCreate(alloc: *const c_void, callback: Callback, context: *mut Context, paths: *const c_void, since: u64, latency: f64, flags: u32) -> Stream;
        fn FSEventStreamSetDispatchQueue(stream: Stream, queue: *mut c_void);
        fn FSEventStreamStart(stream: Stream) -> u8;
        fn FSEventStreamStop(stream: Stream);
        fn FSEventStreamInvalidate(stream: Stream);
        fn FSEventStreamRelease(stream: Stream);
        #[cfg(test)]
        fn FSEventStreamFlushSync(stream: Stream);
    }
    // libdispatch is part of libSystem
    unsafe extern "C" {
        fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
        fn dispatch_sync_f(queue: *mut c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
        fn dispatch_release(object: *mut c_void);
    }
    struct Shared {
        /// canonical full path -> watched source index
        names: HashMap<Vec<u8>, usize>,
        /// the watched directories (canonical)
        roots: std::collections::HashSet<Vec<u8>>,
        /// a missing folder on the way to watched files (canonical; `nearest_existing`)
        appears: std::collections::HashSet<Vec<u8>>,
        /// delivered indices; true once events may have been lost
        pending: Mutex<(Vec<usize>, bool)>,
        /// `flush`'s marker file (canonical) and whether its event was delivered
        #[cfg(test)]
        marker: Mutex<Option<(Vec<u8>, bool)>>,
    }
    pub struct Watch {
        stream: Stream,
        queue: *mut c_void,
        shared: *mut Shared,
    }
    // the stream is stopped and released from whichever thread owns the watch
    unsafe impl Send for Watch {}
    /// Set by a delivery or `kick`, taken by `wait`.
    static KICK: (std::sync::Mutex<bool>, std::sync::Condvar) = (std::sync::Mutex::new(false), std::sync::Condvar::new());
    pub fn kick() {
        *KICK.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        KICK.1.notify_all();
    }
    extern "C" fn delivered(_: Stream, info: *mut c_void, n: usize, paths: *const *const c_char, flags: *const u32, _: *const u64) {
        deliver(info, n, paths, flags);
        kick();
    }
    fn deliver(info: *mut c_void, n: usize, paths: *const *const c_char, flags: *const u32) {
        let shared = unsafe { &*(info as *const Shared) };
        let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        for k in 0..n {
            let (path, flags) = unsafe { (CStr::from_ptr(*paths.add(k)), *flags.add(k)) };
            // MustScanSubDirs, UserDropped, KernelDropped, or an event on a watched directory
            // itself (moved or removed): the caller stats everything and rebuilds the watch.
            pending.1 |= flags & (0x1 | 0x2 | 0x4) != 0 || shared.roots.contains(path.to_bytes()) || shared.appears.contains(path.to_bytes());
            #[cfg(test)]
            if let Some(m) = shared.marker.lock().unwrap_or_else(|e| e.into_inner()).as_mut().filter(|m| m.0 == path.to_bytes()) {
                m.1 = true;
            }
            if let Some(&i) = shared.names.get(path.to_bytes()) { pending.0.push(i); }
            if let Some(slash) = path.to_bytes().iter().rposition(|&b| b == b'/') {
                if let Some(&i) = shared.names.get(&path.to_bytes()[..slash]) { pending.0.push(i); }
            }
        }
    }
    extern "C" fn barrier(_: *mut c_void) {}
    impl Watch {
        pub fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Option<Self> {
            // FSEvents reports resolved paths (/private/var/.. for /var/..): key on those.
            let mut dirs: HashMap<&Path, Option<Vec<u8>>> = HashMap::new();
            let mut names = HashMap::new();
            let mut appears = std::collections::HashSet::new();
            let canonical = |dir: &Path| std::fs::canonicalize(dir).ok().map(|c| c.as_os_str().as_bytes().to_vec());
            for (i, path) in paths.enumerate() {
                let path = Path::new(path);
                let dir = path.parent()?;
                for watched in std::iter::once(dir).chain(path.is_dir().then_some(path)) {
                    if dirs.contains_key(watched) { continue; }
                    match canonical(watched) {
                        Some(c) => { dirs.insert(watched, Some(c)); }
                        None if watched.exists() => return None,
                        None => {
                            let (above, next) = super::nearest_existing(watched)?;
                            let mut at = match dirs.get(above) { Some(Some(c)) => c.clone(), _ => canonical(above)? };
                            dirs.insert(above, Some(at.clone()));
                            at.push(b'/');
                            at.extend_from_slice(next.as_bytes());
                            appears.insert(at);
                            dirs.insert(watched, None);
                        }
                    }
                }
                if let Some(Some(d)) = dirs.get(dir) {
                    let mut full = d.clone();
                    full.push(b'/');
                    full.extend_from_slice(path.file_name()?.as_bytes());
                    names.insert(full, i);
                }
            }
            let mut roots: Vec<Vec<u8>> = dirs.into_values().flatten().collect();
            roots.sort_unstable();
            roots.dedup();
            let root_set = roots.iter().cloned().collect();
            unsafe {
                let strings: Vec<*const c_void> = roots.iter().filter_map(|r| CString::new(r.clone()).ok()).map(|c| CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), 0x0800_0100)).collect();
                if strings.iter().any(|s| s.is_null()) {
                    for s in strings.into_iter().filter(|s| !s.is_null()) { CFRelease(s); }
                    return None;
                }
                let array = CFArrayCreate(std::ptr::null(), strings.as_ptr(), strings.len() as isize, (&raw const kCFTypeArrayCallBacks).cast());
                for s in strings { CFRelease(s); }
                if array.is_null() { return None; }
                let shared = Box::into_raw(Box::new(Shared { names, roots: root_set, appears, pending: Mutex::new((Vec::new(), false)), #[cfg(test)] marker: Mutex::new(None) }));
                let mut context = Context { version: 0, info: shared.cast(), retain: std::ptr::null(), release: std::ptr::null(), describe: std::ptr::null() };
                // UseCFTypes off (paths arrive as C strings), NoDefer 0x2, FileEvents 0x10;
                // since now, latency 0.
                let stream = FSEventStreamCreate(std::ptr::null(), delivered, &mut context, array, u64::MAX, 0.0, 0x2 | 0x10);
                CFRelease(array);
                if stream.is_null() {
                    drop(Box::from_raw(shared));
                    return None;
                }
                let queue = dispatch_queue_create(c"run-watch".as_ptr(), std::ptr::null());
                let watch = Self { stream, queue, shared };
                FSEventStreamSetDispatchQueue(stream, queue);
                if FSEventStreamStart(stream) == 0 { return None; }
                Some(watch)
            }
        }
        /// Sleeps until FSEvents delivers, `kick` is called or `timeout` passes (None: no timeout).
        pub fn wait(&mut self, timeout: Option<std::time::Duration>) {
            let g = KICK.0.lock().unwrap_or_else(|e| e.into_inner());
            let mut g = match timeout {
                Some(t) => KICK.1.wait_timeout_while(g, t, |k| !*k).unwrap_or_else(|e| e.into_inner()).0,
                None => KICK.1.wait_while(g, |k| !*k).unwrap_or_else(|e| e.into_inner()),
            };
            *g = false;
        }
        /// Every event of the file operations made so far delivered: what a test asserts on is
        /// the state after them. FSEventStreamFlushSync hands over only what fseventsd holds, and
        /// the kernel passes it a write a moment later (a write just made was not in the flush).
        /// A marker file made now in a watched folder is reported after every earlier operation:
        /// this returns once its event is delivered. With every watched folder gone, a folder's
        /// own event (a loss) is the one still coming.
        #[cfg(test)]
        pub fn flush(&mut self) {
            static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let shared = unsafe { &*self.shared };
            let root = shared.roots.iter().find(|r| Path::new(std::ffi::OsStr::from_bytes(r)).is_dir()).cloned();
            let Some(mut marker) = root else {
                while !shared.pending.lock().unwrap_or_else(|e| e.into_inner()).1 {
                    self.wait(None);
                }
                return;
            };
            marker.extend_from_slice(format!("/.relax-flush-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)).as_bytes());
            *shared.marker.lock().unwrap_or_else(|e| e.into_inner()) = Some((marker.clone(), false));
            let path = Path::new(std::ffi::OsStr::from_bytes(&marker)).to_path_buf();
            std::fs::write(&path, b"").unwrap();
            loop {
                unsafe { FSEventStreamFlushSync(self.stream) }
                if shared.marker.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.1) { break; }
                self.wait(None);
            }
            *shared.marker.lock().unwrap_or_else(|e| e.into_inner()) = None;
            std::fs::remove_file(&path).unwrap();
        }
        /// None means events may have been lost: stat every source this round.
        pub fn changed(&mut self) -> Option<Vec<usize>> {
            let shared = unsafe { &*self.shared };
            let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.1 { return None; }
            let mut changed = std::mem::take(&mut pending.0);
            drop(pending);
            changed.sort_unstable();
            changed.dedup();
            Some(changed)
        }
    }
    impl Drop for Watch {
        fn drop(&mut self) {
            unsafe {
                // Stop on a never-started stream is a no-op.
                FSEventStreamStop(self.stream);
                FSEventStreamInvalidate(self.stream);
                // the serial queue finishes any callback in flight before the barrier runs
                dispatch_sync_f(self.queue, std::ptr::null_mut(), barrier);
                FSEventStreamRelease(self.stream);
                dispatch_release(self.queue);
                drop(Box::from_raw(self.shared));
            }
        }
    }
}
#[cfg(windows)]
mod native {
    //! ReadDirectoryChangesW per directory, overlapped: `changed` only reads the completion
    //! status of each pending read and re-arms the ones that completed. Every directory handle
    //! completes to one I/O completion port, which `wait` sleeps on and `kick` posts to.
    use crate::os::win32::{CancelIoEx, CloseHandle, CreateFileW, CreateIoCompletionPort, GetOverlappedResult, GetQueuedCompletionStatusEx, PostQueuedCompletionStatus, ReadDirectoryChangesW, Overlapped, OverlappedEntry, HANDLE};
    use std::{collections::HashMap, os::windows::ffi::OsStrExt, path::Path};
    type Handle = HANDLE;
    const STATUS_PENDING: usize = 0x103;
    // FILE_NAME, DIR_NAME, SIZE, LAST_WRITE, CREATION: renames, deletes, creates and writes (a
    // folder's too: a missing one on the way to watched files may be made again)
    const FILTER: u32 = 0x1 | 0x2 | 0x8 | 0x10 | 0x40;
    /// The kernel writes into `buf` and `overlapped` while a read is pending: boxed, never moved.
    struct Dir {
        handle: Handle,
        overlapped: Overlapped,
        buf: [u32; 16384],
        /// lowercase file name -> watched source index (names compare case-insensitively)
        names: HashMap<String, usize>,
        own_index: Option<usize>,
        /// lowercase names of missing folders on the way to watched files (`nearest_existing`)
        appears: std::collections::HashSet<String>,
        /// a read was issued (one that failed to issue has no packet to come)
        armed: bool,
    }
    impl Dir {
        fn arm(&mut self) -> bool {
            self.overlapped = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: 0 };
            let len = std::mem::size_of_val(&self.buf) as u32;
            self.armed = unsafe { ReadDirectoryChangesW(self.handle, self.buf.as_mut_ptr().cast(), len, 0, FILTER, std::ptr::null_mut(), &mut self.overlapped, std::ptr::null_mut()) != 0 };
            self.armed
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            unsafe {
                // A read on a handle of the port completes -- its status and its notifications
                // written into this box -- only when a thread takes its packet from the port, so
                // nothing signals the handle meanwhile (waiting on it hung a watch's drop for
                // good). An outstanding read is cancelled and its packet taken here before the
                // box goes. One watcher thread waits on the port, so no other thread takes it;
                // the other packets taken meanwhile only wake: one kick passes them on.
                if self.armed && std::ptr::read_volatile(&self.overlapped.internal) == STATUS_PENDING {
                    CancelIoEx(self.handle, &mut self.overlapped);
                    let own: *const Overlapped = &self.overlapped;
                    let mut entries: [OverlappedEntry; 64] = std::mem::zeroed();
                    let mut removed = 0u32;
                    let mut others = false;
                    while std::ptr::read_volatile(&self.overlapped.internal) == STATUS_PENDING
                        && GetQueuedCompletionStatusEx(port(), entries.as_mut_ptr(), entries.len() as u32, &mut removed, u32::MAX, 0) != 0
                    {
                        others |= entries[..removed as usize].iter().any(|e| e.overlapped.cast_const() != own);
                    }
                    if others { kick(); }
                }
                CloseHandle(self.handle);
            }
        }
    }
    pub struct Watch { dirs: Vec<Box<Dir>> }
    /// The process's completion port (0: none could be made; `wait` then polls every 5 ms). Its
    /// packets only wake `wait`: what completed is read from each directory's OVERLAPPED once its
    /// packet was taken. One thread waits on it (the live watcher's), which a dropped directory
    /// relies on to take its own cancelled read's packet.
    fn port() -> HANDLE {
        static PORT: std::sync::OnceLock<HANDLE> = std::sync::OnceLock::new();
        // INVALID_HANDLE_VALUE: a new port, one concurrent thread
        *PORT.get_or_init(|| unsafe { CreateIoCompletionPort(-1, 0, 0, 1) })
    }
    pub fn kick() {
        let port = port();
        if port != 0 { unsafe { PostQueuedCompletionStatus(port, 0, 0, std::ptr::null_mut()) }; }
    }
    // directory handles may be used and closed from any thread
    unsafe impl Send for Watch {}
    impl Watch {
        pub fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Option<Self> {
            let mut index: HashMap<String, usize> = HashMap::new();
            let mut dirs: Vec<Box<Dir>> = Vec::new();
            let paths: Vec<_> = paths.collect();
            // None: the directory cannot be watched (Err: it is missing)
            let open = |dir: &Path| -> Result<Option<Box<Dir>>, ()> {
                if !dir.is_dir() { return Err(()); }
                let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
                // FILE_LIST_DIRECTORY; share read/write/delete so saves and renames proceed;
                // OPEN_EXISTING; BACKUP_SEMANTICS (a directory) | OVERLAPPED
                let handle = unsafe { CreateFileW(wide.as_ptr(), 0x1, 0x7, std::ptr::null_mut(), 3, 0x0200_0000 | 0x4000_0000, 0) };
                if handle == -1 { return Ok(None); }
                // its completions wake `wait` (a handle that cannot join the port is a watch
                // that cannot sleep on it: none)
                if port() == 0 || unsafe { CreateIoCompletionPort(handle, port(), 0, 0) } == 0 {
                    unsafe { CloseHandle(handle) };
                    return Ok(None);
                }
                let mut entry = Box::new(Dir { handle, overlapped: Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: 0 }, buf: [0; 16384], names: HashMap::new(), own_index: None, appears: Default::default(), armed: false });
                Ok(entry.arm().then_some(entry))
            };
            // a missing directory's key maps to usize::MAX
            for (i, path) in paths.iter().enumerate() {
                let path = Path::new(path);
                let parent = path.parent()?;
                for dir in std::iter::once(parent).chain(path.is_dir().then_some(path)) {
                    let key = dir.to_string_lossy().to_lowercase();
                    if index.contains_key(&key) { continue; }
                    match open(dir) {
                        Ok(entry) => {
                            dirs.push(entry?);
                            index.insert(key, dirs.len() - 1);
                        }
                        Err(()) => {
                            let (above, next) = super::nearest_existing(dir)?;
                            let above_key = above.to_string_lossy().to_lowercase();
                            let d = match index.get(&above_key) {
                                Some(&d) => d,
                                None => {
                                    dirs.push(open(above).ok()??);
                                    index.insert(above_key, dirs.len() - 1);
                                    dirs.len() - 1
                                }
                            };
                            dirs[d].appears.insert(next.to_string_lossy().to_lowercase());
                            index.insert(key, usize::MAX);
                        }
                    }
                }
                let d = index[&parent.to_string_lossy().to_lowercase()];
                if d != usize::MAX {
                    dirs[d].names.insert(path.file_name()?.to_string_lossy().to_lowercase(), i);
                }
            }
            for (i, path) in paths.iter().enumerate() {
                if Path::new(path).is_dir() {
                    if let Some(&d) = index.get(&path.to_lowercase()).filter(|&&d| d != usize::MAX) { dirs[d].own_index = Some(i); }
                }
            }
            Some(Self { dirs })
        }
        /// Sleeps until a directory read completes, `kick` is called or `timeout` passes (None: no
        /// timeout). Every queued packet is taken: they only wake.
        pub fn wait(&mut self, timeout: Option<std::time::Duration>) {
            let mut entries: [OverlappedEntry; 64] = unsafe { std::mem::zeroed() };
            let mut removed = 0u32;
            // INFINITE = u32::MAX
            let mut ms = timeout.map_or(u32::MAX, |t| t.as_millis().min(u32::MAX as u128 - 1) as u32);
            while unsafe { GetQueuedCompletionStatusEx(port(), entries.as_mut_ptr(), entries.len() as u32, &mut removed, ms, 0) } != 0 && removed as usize == entries.len() {
                ms = 0;
            }
        }
        /// What the file operations made so far reported, every read they completed taken: a
        /// read completes during the operation, a re-armed one at once with what came meanwhile,
        /// so the port then holds no packet. For tests: the state after the operations, no wait.
        #[cfg(test)]
        pub fn settled(&mut self) -> Option<Vec<usize>> {
            let mut seen = Vec::new();
            loop {
                let mut entries: [OverlappedEntry; 64] = unsafe { std::mem::zeroed() };
                let mut removed = 0u32;
                let took = unsafe { GetQueuedCompletionStatusEx(port(), entries.as_mut_ptr(), entries.len() as u32, &mut removed, 0, 0) } != 0;
                seen.extend(self.changed()?);
                if !took { break; }
            }
            seen.sort_unstable();
            seen.dedup();
            Some(seen)
        }
        /// None means events may have been lost: stat every source this round.
        pub fn changed(&mut self) -> Option<Vec<usize>> {
            let mut changed = Vec::new();
            for dir in self.dirs.iter_mut() {
                // HasOverlappedIoCompleted: no system call while nothing happened
                if unsafe { std::ptr::read_volatile(&dir.overlapped.internal) } == STATUS_PENDING { continue; }
                let mut bytes = 0u32;
                // a failed read (directory removed, notify-enum overflow) or an empty one
                // (buffer overflow) loses events
                if unsafe { GetOverlappedResult(dir.handle, &mut dir.overlapped, &mut bytes, 0) } == 0 || bytes == 0 { return None; }
                if let Some(i) = dir.own_index { changed.push(i); }
                let buf: &[u8] = unsafe { std::slice::from_raw_parts(dir.buf.as_ptr().cast(), bytes as usize) };
                let mut pos = 0usize;
                loop {
                    // FILE_NOTIFY_INFORMATION: next offset, action, name length in bytes, UTF-16 name
                    if pos + 12 > buf.len() { return None; }
                    let word = |off: usize| u32::from_ne_bytes(buf[pos + off..pos + off + 4].try_into().unwrap()) as usize;
                    let (next, len) = (word(0), word(8));
                    if pos + 12 + len > buf.len() { return None; }
                    let name: Vec<u16> = buf[pos + 12..pos + 12 + len].chunks_exact(2).map(|c| u16::from_ne_bytes([c[0], c[1]])).collect();
                    let name = String::from_utf16_lossy(&name).to_lowercase();
                    if let Some(&i) = dir.names.get(&name) { changed.push(i); }
                    if dir.appears.contains(&name) { return None; }
                    if next == 0 { break; }
                    pos += next;
                }
                if !dir.arm() { return None; }
            }
            changed.sort_unstable();
            changed.dedup();
            Some(changed)
        }
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod native {
    pub struct Watch;
    pub fn kick() {}
    impl Watch {
        pub fn new<'a>(_: impl Iterator<Item = &'a str>) -> Option<Self> { None }
        pub fn wait(&mut self, timeout: Option<std::time::Duration>) { std::thread::sleep(timeout.map_or(std::time::Duration::from_millis(5), |t| t.min(std::time::Duration::from_millis(5)))) }
        pub fn changed(&mut self) -> Option<Vec<usize>> { None }
    }
}
/// `Watch::wait` returns now: an input of the live watcher that is not a file event arrived (a
/// streamed edit, a domain's new inputs, files the post-boot loader added, the startup's end).
pub use native::kick;
pub use native::Watch;

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    /// On Windows the tests share the process's completion port, on macOS the wake flag: one
    /// watcher at a time, as in the live loop (a dropped Windows watch takes its own packets).
    fn one_watcher() -> std::sync::MutexGuard<'static, ()> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// What the watch reports for the file operations made so far (None: events lost). inotify
    /// and ReadDirectoryChangesW report an operation before it returns (on Windows a read's
    /// completion reaches it once its packet is taken: `settled`); FSEvents a moment later, all
    /// of it once a marker made after it is delivered (`flush`). An empty answer is the state
    /// "no event after the write", not a wait.
    fn next(watch: &mut super::Watch) -> Option<Vec<usize>> {
        #[cfg(windows)]
        return watch.settled();
        #[cfg(not(windows))]
        {
            #[cfg(target_os = "macos")]
            watch.flush();
            let mut seen = Vec::new();
            loop {
                let got = watch.changed()?;
                if got.is_empty() { break; }
                seen.extend(got);
            }
            seen.sort_unstable();
            seen.dedup();
            Some(seen)
        }
    }
    #[test]
    fn directory_inputs_observe_new_sources() {
        let _one = one_watcher();
        let dir = std::env::temp_dir().join(format!("relax-notify-add-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("existing.rs");
        let added = dir.join("added.rs");
        std::fs::write(&source, "old").unwrap();
        let mut watch = super::Watch::new([dir.to_str().unwrap(), source.to_str().unwrap()].into_iter()).unwrap();
        if cfg!(target_os = "macos") {
            std::thread::sleep(std::time::Duration::from_millis(300));
            next(&mut watch);
        }
        std::fs::write(&added, "new").unwrap();
        // A lost-event rescan is also sufficient: the directory's stamp changed.
        let seen = next(&mut watch);
        assert!(seen.as_ref().is_none_or(|ids| ids.contains(&0)), "no event for the directory after a file was added: {seen:?}");
        drop(watch);
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(added).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn notifications_survive_atomic_save_and_filter_unrelated_names() {
        let _one = one_watcher();
        let dir = std::env::temp_dir().join(format!("relax-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.rs");
        let unrelated = dir.join("unrelated.rs");
        let temporary = dir.join("save.tmp");
        std::fs::write(&source, "old").unwrap();
        let new_watch = || super::Watch::new(std::iter::once(source.to_str().unwrap())).unwrap();
        let mut watch = new_watch();
        if cfg!(target_os = "macos") {
            // FSEvents may still deliver the writes made just before the stream started (fseventsd
            // had not processed them yet): the file's (a stray stat) or the fresh directory's own
            // (None: events may be lost, and the caller rebuilds the watch -- as here)
            for _ in 0..5 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if next(&mut watch).is_some() { break; }
                watch = new_watch();
            }
        }
        std::fs::write(&unrelated, "unrelated").unwrap();
        assert_eq!(next(&mut watch), Some(vec![]), "an unwatched name in the folder reported");
        for text in ["first", "second"] {
            std::fs::write(&temporary, text).unwrap();
            std::fs::rename(&temporary, &source).unwrap();
            assert_eq!(next(&mut watch), Some(vec![0]), "no event after the atomic save of {text:?}");
        }
        std::fs::write(&source, "direct").unwrap();
        assert_eq!(next(&mut watch), Some(vec![0]), "no event after the write");
        std::fs::remove_file(&source).unwrap();
        assert_eq!(next(&mut watch), Some(vec![0]), "no event after the delete");
        drop(watch);
        std::fs::remove_file(unrelated).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    /// A watched directory moved away (a checkout or tool replacing a tree) means the watch no
    /// longer covers the source paths: the caller rescans and rebuilds it. (Windows does not
    /// report a rename of the watched directory itself.)
    #[cfg(not(windows))]
    #[test]
    fn moving_the_watched_directory_loses_events() {
        let _one = one_watcher();
        let base = std::env::temp_dir().join(format!("relax-notify-move-{}", std::process::id()));
        let dir = base.join("src");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.rs");
        std::fs::write(&source, "old").unwrap();
        let mut watch = super::Watch::new(std::iter::once(source.to_str().unwrap())).unwrap();
        if cfg!(target_os = "macos") { std::thread::sleep(std::time::Duration::from_millis(300)); next(&mut watch); }
        let moved = base.join("moved");
        std::fs::rename(&dir, &moved).unwrap();
        let seen = next(&mut watch);
        assert!(seen.is_none(), "no lost-events report after the watched folder moved: {seen:?}");
        drop(watch);
        std::fs::remove_file(moved.join("source.rs")).unwrap();
        std::fs::remove_dir(&moved).unwrap();
        std::fs::remove_dir(&base).unwrap();
    }
    /// A watched file whose folder is missing (removed by a checkout or a tool) leaves every
    /// other file on native events, and the folder's return loses events: the caller rescans and
    /// rebuilds the watch, which then covers the new folder.
    #[test]
    fn a_missing_folder_keeps_the_watch_and_its_return_loses_events() {
        let _one = one_watcher();
        let dir = std::env::temp_dir().join(format!("relax-notify-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.rs");
        let gone = dir.join("gone");
        std::fs::write(&source, "old").unwrap();
        let inside = gone.join("deeper").join("inside.rs");
        let new_watch = || super::Watch::new([source.to_str().unwrap(), inside.to_str().unwrap()].into_iter()).unwrap();
        let mut watch = new_watch();
        if cfg!(target_os = "macos") {
            // FSEvents may still deliver the fresh directory's own creation (see above)
            for _ in 0..5 {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if next(&mut watch).is_some() { break; }
                watch = new_watch();
            }
        }
        std::fs::write(&source, "new").unwrap();
        assert_eq!(next(&mut watch), Some(vec![0]), "no event after the write beside a missing folder");
        std::fs::create_dir(&gone).unwrap();
        let seen = next(&mut watch);
        assert!(seen.is_none(), "no lost-events report after the missing folder came back: {seen:?}");
        drop(watch);
        std::fs::remove_dir(&gone).unwrap();
        std::fs::remove_file(&source).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
