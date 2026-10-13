//! Directory notifications retain watches across editor atomic-save renames. Startup is
//! proportional to the watch set; normal drains visit only delivered source-file events.
//! Linux: inotify, macOS: FSEvents file events, Windows: ReadDirectoryChangesW. Other
//! platforms and lost events (queue overflow, a watched directory moved) keep the stat fallback.
#[cfg(target_os = "linux")]
mod native {
    use std::{collections::HashMap, ffi::CString, os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::ffi::OsStrExt}, path::Path};
    unsafe extern "C" {
        fn inotify_init1(flags: i32) -> i32;
        fn inotify_add_watch(fd: i32, path: *const std::ffi::c_char, mask: u32) -> i32;
        fn read(fd: i32, buf: *mut u8, size: usize) -> isize;
    }
    pub struct Watch {
        fd: OwnedFd,
        names: HashMap<(i32, Vec<u8>), usize>,
        directories: HashMap<i32, usize>,
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
            for (i, path) in paths.iter().enumerate() {
                let path = Path::new(path);
                let dir = path.parent()?;
                let wd = if let Some(&wd) = dirs.get(dir) { wd } else {
                    let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
                    // MODIFY, ATTRIB, CLOSE_WRITE, MOVED_FROM/TO, CREATE, DELETE,
                    // DELETE_SELF and MOVE_SELF. Watching directories handles new inodes.
                    let wd = unsafe { inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), 0xfce) };
                    if wd < 0 { return None; }
                    dirs.insert(dir.to_owned(), wd);
                    wd
                };
                names.insert((wd, path.file_name()?.as_bytes().to_vec()), i);
            }
            let directories = paths.iter().enumerate().filter_map(|(i, p)| {
                let p = Path::new(p);
                p.is_dir().then(|| dirs.get(p).map(|&wd| (wd, i))).flatten()
            }).collect();
            Some(Self { fd, names, directories })
        }
        /// None means events may have been lost: stat every source this round.
        pub fn changed(&mut self) -> Option<Vec<usize>> {
            let mut changed = Vec::new();
            let mut lost = false;
            let mut buf = [0u8; 65536];
            loop {
                let n = unsafe { read(self.fd.as_raw_fd(), buf.as_mut_ptr(), buf.len()) };
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
                    if let Some(&i) = self.names.get(&(wd, bytes[..end].to_vec())) { changed.push(i); }
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
        /// delivered indices; true once events may have been lost
        pending: Mutex<(Vec<usize>, bool)>,
    }
    pub struct Watch {
        stream: Stream,
        queue: *mut c_void,
        shared: *mut Shared,
    }
    // the stream is stopped and released from whichever thread owns the watch
    unsafe impl Send for Watch {}
    extern "C" fn delivered(_: Stream, info: *mut c_void, n: usize, paths: *const *const c_char, flags: *const u32, _: *const u64) {
        let shared = unsafe { &*(info as *const Shared) };
        let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        for k in 0..n {
            let (path, flags) = unsafe { (CStr::from_ptr(*paths.add(k)), *flags.add(k)) };
            // MustScanSubDirs, UserDropped, KernelDropped, or an event on a watched directory
            // itself (moved or removed): the caller stats everything and rebuilds the watch.
            pending.1 |= flags & (0x1 | 0x2 | 0x4) != 0 || shared.roots.contains(path.to_bytes());
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
            let mut dirs: HashMap<&Path, Vec<u8>> = HashMap::new();
            let mut names = HashMap::new();
            for (i, path) in paths.enumerate() {
                let path = Path::new(path);
                let dir = path.parent()?;
                if !dirs.contains_key(dir) { dirs.insert(dir, std::fs::canonicalize(dir).ok()?.as_os_str().as_bytes().to_vec()); }
                let mut full = dirs[dir].clone();
                full.push(b'/');
                full.extend_from_slice(path.file_name()?.as_bytes());
                names.insert(full, i);
            }
            let mut roots: Vec<Vec<u8>> = dirs.into_values().collect();
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
                let shared = Box::into_raw(Box::new(Shared { names, roots: root_set, pending: Mutex::new((Vec::new(), false)) }));
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
    //! status of each pending read and re-arms the ones that completed.
    use crate::os::win32::{CancelIoEx, CloseHandle, CreateFileW, GetOverlappedResult, ReadDirectoryChangesW, Overlapped, HANDLE};
    use std::{collections::HashMap, os::windows::ffi::OsStrExt, path::Path};
    type Handle = HANDLE;
    const STATUS_PENDING: usize = 0x103;
    // FILE_NAME, SIZE, LAST_WRITE, CREATION: renames, deletes, creates and writes
    const FILTER: u32 = 0x1 | 0x8 | 0x10 | 0x40;
    /// The kernel writes into `buf` and `overlapped` while a read is pending: boxed, never moved.
    struct Dir {
        handle: Handle,
        overlapped: Overlapped,
        buf: [u32; 16384],
        /// lowercase file name -> watched source index (names compare case-insensitively)
        names: HashMap<String, usize>,
        own_index: Option<usize>,
    }
    impl Dir {
        fn arm(&mut self) -> bool {
            self.overlapped = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: 0 };
            let len = std::mem::size_of_val(&self.buf) as u32;
            unsafe { ReadDirectoryChangesW(self.handle, self.buf.as_mut_ptr().cast(), len, 0, FILTER, std::ptr::null_mut(), &mut self.overlapped, std::ptr::null_mut()) != 0 }
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            unsafe {
                let mut bytes = 0;
                // wait for the cancelled read so the kernel no longer writes into the box
                if CancelIoEx(self.handle, &mut self.overlapped) != 0 { GetOverlappedResult(self.handle, &mut self.overlapped, &mut bytes, 1); }
                CloseHandle(self.handle);
            }
        }
    }
    pub struct Watch { dirs: Vec<Box<Dir>> }
    // directory handles may be used and closed from any thread
    unsafe impl Send for Watch {}
    impl Watch {
        pub fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Option<Self> {
            let mut index: HashMap<String, usize> = HashMap::new();
            let mut dirs: Vec<Box<Dir>> = Vec::new();
            let paths: Vec<_> = paths.collect();
            for (i, path) in paths.iter().enumerate() {
                let path = Path::new(path);
                let dir = path.parent()?;
                let key = dir.to_string_lossy().to_lowercase();
                let d = if let Some(&d) = index.get(&key) { d } else {
                    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
                    // FILE_LIST_DIRECTORY; share read/write/delete so saves and renames proceed;
                    // OPEN_EXISTING; BACKUP_SEMANTICS (a directory) | OVERLAPPED
                    let handle = unsafe { CreateFileW(wide.as_ptr(), 0x1, 0x7, std::ptr::null_mut(), 3, 0x0200_0000 | 0x4000_0000, 0) };
                    if handle == -1 { return None; }
                    let mut entry = Box::new(Dir { handle, overlapped: Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: 0 }, buf: [0; 16384], names: HashMap::new(), own_index: None });
                    if !entry.arm() { return None; }
                    dirs.push(entry);
                    index.insert(key, dirs.len() - 1);
                    dirs.len() - 1
                };
                dirs[d].names.insert(path.file_name()?.to_string_lossy().to_lowercase(), i);
            }
            for (i, path) in paths.iter().enumerate() {
                if Path::new(path).is_dir() {
                    if let Some(&d) = index.get(&path.to_lowercase()) { dirs[d].own_index = Some(i); }
                }
            }
            Some(Self { dirs })
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
                    if let Some(&i) = dir.names.get(&String::from_utf16_lossy(&name).to_lowercase()) { changed.push(i); }
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
    impl Watch {
        pub fn new<'a>(_: impl Iterator<Item = &'a str>) -> Option<Self> { None }
        pub fn changed(&mut self) -> Option<Vec<usize>> { None }
    }
}
pub use native::Watch;

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    /// inotify is read synchronously; FSEvents and ReadDirectoryChangesW deliver a moment later.
    fn next(watch: &mut super::Watch) -> Option<Vec<usize>> {
        let start = std::time::Instant::now();
        let mut seen = Vec::new();
        loop {
            seen.extend(watch.changed()?);
            if !seen.is_empty() || cfg!(target_os = "linux") || start.elapsed().as_secs() >= 2 {
                // events of one save may span drains
                std::thread::sleep(std::time::Duration::from_millis(if cfg!(target_os = "linux") { 0 } else { 50 }));
                seen.extend(watch.changed()?);
                seen.sort_unstable();
                seen.dedup();
                return Some(seen);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn directory_inputs_observe_new_sources() {
        let dir = std::env::temp_dir().join(format!("relax-notify-add-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("existing.rs");
        let added = dir.join("added.rs");
        std::fs::write(&source, "old").unwrap();
        let mut watch = super::Watch::new([dir.to_str().unwrap(), source.to_str().unwrap()].into_iter()).unwrap();
        if !cfg!(target_os = "linux") {
            std::thread::sleep(std::time::Duration::from_millis(300));
            watch.changed();
        }
        std::fs::write(&added, "new").unwrap();
        // A lost-event rescan is also sufficient: the directory's stamp changed.
        assert!(next(&mut watch).is_none_or(|ids| ids.contains(&0)));
        drop(watch);
        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(added).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn notifications_survive_atomic_save_and_filter_unrelated_names() {
        let dir = std::env::temp_dir().join(format!("relax-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.rs");
        let unrelated = dir.join("unrelated.rs");
        let temporary = dir.join("save.tmp");
        std::fs::write(&source, "old").unwrap();
        let mut watch = super::Watch::new(std::iter::once(source.to_str().unwrap())).unwrap();
        if !cfg!(target_os = "linux") {
            // FSEvents may still report the write just before the stream started (fseventsd
            // had not processed it yet); a stray event costs the caller one stat.
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(watch.changed().is_some());
        }
        std::fs::write(&unrelated, "unrelated").unwrap();
        if !cfg!(target_os = "linux") { std::thread::sleep(std::time::Duration::from_millis(300)); }
        assert_eq!(watch.changed(), Some(vec![]));
        for text in ["first", "second"] {
            std::fs::write(&temporary, text).unwrap();
            std::fs::rename(&temporary, &source).unwrap();
            assert_eq!(next(&mut watch), Some(vec![0]));
        }
        std::fs::write(&source, "direct").unwrap();
        assert_eq!(next(&mut watch), Some(vec![0]));
        std::fs::remove_file(&source).unwrap();
        assert_eq!(next(&mut watch), Some(vec![0]));
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
        let base = std::env::temp_dir().join(format!("relax-notify-move-{}", std::process::id()));
        let dir = base.join("src");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.rs");
        std::fs::write(&source, "old").unwrap();
        let mut watch = super::Watch::new(std::iter::once(source.to_str().unwrap())).unwrap();
        if !cfg!(target_os = "linux") { std::thread::sleep(std::time::Duration::from_millis(300)); watch.changed(); }
        let moved = base.join("moved");
        std::fs::rename(&dir, &moved).unwrap();
        let start = std::time::Instant::now();
        let mut lost = false;
        while start.elapsed().as_secs() < 2 {
            if watch.changed().is_none() { lost = true; break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(lost);
        drop(watch);
        std::fs::remove_file(moved.join("source.rs")).unwrap();
        std::fs::remove_dir(&moved).unwrap();
        std::fs::remove_dir(&base).unwrap();
    }
}
