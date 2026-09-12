use crate::{Emitter, ExcludePolicy, FileSystemEventKind, RescanReason, WatchRoot};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_void, CStr, CString, OsString};
use std::os::raw::{c_char, c_double};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

type CFAllocatorRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFRunLoopRef = *mut c_void;
type CFIndex = isize;
type Boolean = u8;

type FSEventStreamRef = *mut c_void;
type FSEventStreamEventId = u64;
type FSEventStreamEventFlags = u32;
type FSEventStreamCreateFlags = u32;
type CFTimeInterval = c_double;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_FS_EVENT_STREAM_EVENT_ID_SINCE_NOW: FSEventStreamEventId = 0xFFFF_FFFF_FFFF_FFFF;
const K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS: FSEventStreamCreateFlags = 0x0000_0010;
const K_FS_EVENT_STREAM_CREATE_FLAG_USE_CF_TYPES: FSEventStreamCreateFlags = 0x0000_0001;
const K_FS_EVENT_STREAM_CREATE_FLAG_NO_DEFER: FSEventStreamCreateFlags = 0x0000_0002;

/// The stream's coalescing latency.
const STREAM_LATENCY: CFTimeInterval = 0.1;

const FLAG_MUST_SCAN_SUB_DIRS: FSEventStreamEventFlags = 0x0000_0001;
const FLAG_USER_DROPPED: FSEventStreamEventFlags = 0x0000_0002;
const FLAG_KERNEL_DROPPED: FSEventStreamEventFlags = 0x0000_0004;
const FLAG_EVENT_IDS_WRAPPED: FSEventStreamEventFlags = 0x0000_0008;
const FLAG_HISTORY_DONE: FSEventStreamEventFlags = 0x0000_0010;
const FLAG_ROOT_CHANGED: FSEventStreamEventFlags = 0x0000_0020;
const FLAG_ITEM_RENAMED: FSEventStreamEventFlags = 0x0000_0800;
const FLAG_ITEM_IS_FILE: FSEventStreamEventFlags = 0x0001_0000;
const FLAG_ITEM_IS_DIR: FSEventStreamEventFlags = 0x0002_0000;

const OVERFLOW_FLAGS: FSEventStreamEventFlags = FLAG_MUST_SCAN_SUB_DIRS
    | FLAG_USER_DROPPED
    | FLAG_KERNEL_DROPPED
    | FLAG_EVENT_IDS_WRAPPED
    | FLAG_HISTORY_DONE;

/// Entries the inventory retains at most; whole directories are evicted
/// least-recently-touched first beyond it.
const INVENTORY_MAX_ENTRIES: usize = 262_144;
/// Directories with more entries than this are not inventoried: no rename
/// evidence there, but no unbounded listing either.
const INVENTORY_MAX_DIR_ENTRIES: usize = 8192;
/// Entries seeded breadth-first from each root at start, root-first, so
/// the top of a source tree has rename evidence from the first event.
const INVENTORY_SEED_BUDGET: usize = 50_000;

#[repr(C)]
struct FSEventStreamContext {
    version: CFIndex,
    info: *mut c_void,
    retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<extern "C" fn(*const c_void)>,
    copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
}

type FSEventStreamCallback = extern "C" fn(
    stream_ref: FSEventStreamRef,
    client_callback_info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const FSEventStreamEventFlags,
    event_ids: *const FSEventStreamEventId,
);

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        c_str: *const c_char,
        encoding: u32,
    ) -> CFStringRef;

    fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const *const c_void,
        num_values: CFIndex,
        callbacks: *const c_void,
    ) -> CFArrayRef;
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;

    fn CFRelease(cf: *const c_void);

    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: CFRunLoopRef);
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: CFAllocatorRef,
        callback: FSEventStreamCallback,
        context: *mut FSEventStreamContext,
        paths_to_watch: CFArrayRef,
        since_when: FSEventStreamEventId,
        latency: CFTimeInterval,
        flags: FSEventStreamCreateFlags,
    ) -> FSEventStreamRef;

    fn FSEventStreamScheduleWithRunLoop(
        stream_ref: FSEventStreamRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );

    fn FSEventStreamStart(stream_ref: FSEventStreamRef) -> Boolean;
    fn FSEventStreamStop(stream_ref: FSEventStreamRef);
    fn FSEventStreamInvalidate(stream_ref: FSEventStreamRef);
    fn FSEventStreamRelease(stream_ref: FSEventStreamRef);
}

/// (device, inode): the identity a path carries across a rename.
type Identity = (u64, u64);

/// One directory's listing as the inventory knows it.
struct DirEntries {
    names: HashMap<OsString, (Identity, bool)>,
    /// A listing that could not be taken (unreadable, too big) leaves no
    /// evidence: lookups answer `NoEvidence`, never `Absent`.
    complete: bool,
    touched: u64,
}

/// What the inventory says about a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Knowledge {
    Known(Identity, bool),
    /// The directory is inventoried and the path was not in it.
    Absent,
    /// The directory could not be inventoried.
    NoEvidence,
}

/// Identity per path for the directories the backend has looked at:
/// seeded per directory on first touch (and breadth-first from the roots
/// within a budget at start), maintained from the classified events.
#[derive(Default)]
struct Inventory {
    dirs: HashMap<PathBuf, DirEntries>,
    entries: usize,
    tick: u64,
}

type Listing = Vec<(OsString, Identity, bool)>;

impl Inventory {
    fn touch(&mut self, dir: &Path) {
        self.tick += 1;
        if let Some(entries) = self.dirs.get_mut(dir) {
            entries.touched = self.tick;
        }
    }

    fn seed(&mut self, dir: &Path, listing: Option<Listing>) {
        let (names, complete) = match listing {
            Some(listing) => (
                listing
                    .into_iter()
                    .map(|(name, identity, is_dir)| (name, (identity, is_dir)))
                    .collect::<HashMap<_, _>>(),
                true,
            ),
            None => (HashMap::new(), false),
        };
        self.tick += 1;
        self.entries += names.len();
        if let Some(old) = self.dirs.insert(
            dir.to_path_buf(),
            DirEntries {
                names,
                complete,
                touched: self.tick,
            },
        ) {
            self.entries -= old.names.len();
        }
        self.evict();
    }

    fn evict(&mut self) {
        while self.entries > INVENTORY_MAX_ENTRIES && self.dirs.len() > 1 {
            let Some(oldest) = self
                .dirs
                .iter()
                .min_by_key(|(_, entries)| entries.touched)
                .map(|(dir, _)| dir.clone())
            else {
                break;
            };
            if let Some(old) = self.dirs.remove(&oldest) {
                self.entries -= old.names.len();
            }
        }
    }

    fn lookup(&mut self, path: &Path, list: &dyn Fn(&Path) -> Option<Listing>) -> Knowledge {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return Knowledge::NoEvidence;
        };
        if !self.dirs.contains_key(dir) {
            self.seed(dir, list(dir));
        }
        self.touch(dir);
        let Some(entries) = self.dirs.get(dir) else {
            return Knowledge::NoEvidence;
        };
        match entries.names.get(name) {
            Some((identity, is_dir)) => Knowledge::Known(*identity, *is_dir),
            None if entries.complete => Knowledge::Absent,
            None => Knowledge::NoEvidence,
        }
    }

    fn insert(&mut self, path: &Path, identity: Identity, is_dir: bool) {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if let Some(entries) = self.dirs.get_mut(dir) {
            if entries.names.insert(name.to_os_string(), (identity, is_dir)).is_none() {
                self.entries += 1;
            }
        }
    }

    fn remove(&mut self, path: &Path) {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if let Some(entries) = self.dirs.get_mut(dir) {
            if entries.names.remove(name).is_some() {
                self.entries -= 1;
            }
        }
        // A removed directory takes its own listing with it.
        if let Some(old) = self.dirs.remove(path) {
            self.entries -= old.names.len();
        }
    }
}

/// List one directory for the inventory: regular files and real
/// directories with their identity; links and specials are skipped.
/// `None` when the directory cannot be read or is too big to inventory.
fn list_dir(dir: &Path) -> Option<Listing> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !(file_type.is_dir() || file_type.is_file()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        out.push((entry.file_name(), (meta.dev(), meta.ino()), file_type.is_dir()));
        if out.len() > INVENTORY_MAX_DIR_ENTRIES {
            return None;
        }
    }
    Some(out)
}

/// Seed the inventory breadth-first from `root`, honouring exclusions,
/// until `budget` entries are held.
fn seed_tree(root: &Path, exclude: &ExcludePolicy, inventory: &mut Inventory, budget: usize) {
    let mut queue = VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        if inventory.entries >= budget {
            break;
        }
        let listing = list_dir(&dir);
        if let Some(listing) = &listing {
            for (name, _, is_dir) in listing {
                if !*is_dir {
                    continue;
                }
                let child = dir.join(name);
                if !exclude.excludes_dir(root, &child) {
                    queue.push_back(child);
                }
            }
        }
        inventory.seed(&dir, listing);
    }
}

struct CallbackInfo {
    mount: String,
    root: PathBuf,
    emitter: Arc<Emitter>,
    inventory: RefCell<Inventory>,
}

extern "C" fn context_retain(info: *const c_void) -> *const c_void {
    info
}

extern "C" fn context_release(info: *const c_void) {
    if info.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(info as *mut CallbackInfo));
    }
}

/// One raw FSEvents record after path decoding, kept with its stream id so
/// classification runs in the stream's order.
struct RawEvent {
    id: FSEventStreamEventId,
    path: PathBuf,
    flags: FSEventStreamEventFlags,
}

/// What the filesystem says about a path at receipt time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Probe {
    Missing,
    File(Identity),
    Dir(Identity),
    /// Symlink, device, socket: reported as a change, never followed.
    Other,
}

fn probe(path: &Path) -> Probe {
    match std::fs::symlink_metadata(path) {
        Err(_) => Probe::Missing,
        Ok(meta) if meta.file_type().is_symlink() || !(meta.is_dir() || meta.is_file()) => {
            Probe::Other
        }
        Ok(meta) if meta.is_dir() => Probe::Dir((meta.dev(), meta.ino())),
        Ok(meta) => Probe::File((meta.dev(), meta.ino())),
    }
}

fn probe_identity(probe: Probe) -> Option<(Identity, bool)> {
    match probe {
        Probe::File(identity) => Some((identity, false)),
        Probe::Dir(identity) => Some((identity, true)),
        _ => None,
    }
}

/// Classify one batch of FSEvents records into (kind, is_dir) per path,
/// against the inventory. A missing renamed path is paired only with the
/// one renamed path in the batch that now carries the identity the missing
/// path had; otherwise the halves degrade to `Removed` and `Created`.
/// Creation is the inventory's verdict (path absent before), never a flag.
fn classify(
    events: &[RawEvent],
    probe: &dyn Fn(&Path) -> Probe,
    inventory: &mut Inventory,
    list: &dyn Fn(&Path) -> Option<Listing>,
) -> Vec<(PathBuf, FileSystemEventKind, Option<bool>)> {
    let probes: Vec<Probe> = events.iter().map(|e| probe(&e.path)).collect();
    let mut out = Vec::with_capacity(events.len());
    let mut consumed = vec![false; events.len()];
    // Rename pairs first, so an arriving half is never classified on its
    // own before its departing half is seen.
    let mut pairs: Vec<Option<usize>> = vec![None; events.len()];
    for i in 0..events.len() {
        let event = &events[i];
        if event.flags & FLAG_ITEM_RENAMED == 0 || probes[i] != Probe::Missing {
            continue;
        }
        let Knowledge::Known(identity, _) = inventory.lookup(&event.path, list) else {
            continue;
        };
        let candidates: Vec<usize> = (0..events.len())
            .filter(|&j| {
                j != i
                    && !consumed[j]
                    && events[j].flags & FLAG_ITEM_RENAMED != 0
                    && probe_identity(probes[j]).is_some_and(|(id, _)| id == identity)
            })
            .collect();
        if let [j] = candidates[..] {
            consumed[j] = true;
            pairs[i] = Some(j);
        }
    }
    for i in 0..events.len() {
        if consumed[i] {
            continue;
        }
        let event = &events[i];
        if let Some(j) = pairs[i] {
            let (identity, to_is_dir) = probe_identity(probes[j]).unwrap_or(((0, 0), false));
            inventory.remove(&event.path);
            inventory.insert(&events[j].path, identity, to_is_dir);
            out.push((
                events[j].path.clone(),
                FileSystemEventKind::Renamed {
                    from: event.path.clone(),
                },
                Some(to_is_dir),
            ));
            continue;
        }
        match probes[i] {
            Probe::Missing => {
                let is_dir = match inventory.lookup(&event.path, list) {
                    Knowledge::Known(_, is_dir) => Some(is_dir),
                    _ => {
                        if event.flags & FLAG_ITEM_IS_DIR != 0 {
                            Some(true)
                        } else if event.flags & FLAG_ITEM_IS_FILE != 0 {
                            Some(false)
                        } else {
                            None
                        }
                    }
                };
                inventory.remove(&event.path);
                out.push((event.path.clone(), FileSystemEventKind::Removed, is_dir));
            }
            Probe::Other => out.push((event.path.clone(), FileSystemEventKind::Changed, None)),
            Probe::File(identity) | Probe::Dir(identity) => {
                let is_dir = matches!(probes[i], Probe::Dir(_));
                let kind = match inventory.lookup(&event.path, list) {
                    Knowledge::Absent => FileSystemEventKind::Created,
                    Knowledge::Known(..) | Knowledge::NoEvidence => FileSystemEventKind::Changed,
                };
                inventory.insert(&event.path, identity, is_dir);
                out.push((event.path.clone(), kind, Some(is_dir)));
            }
        }
    }
    out
}

extern "C" fn fsevent_callback(
    _stream_ref: FSEventStreamRef,
    client_callback_info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const FSEventStreamEventFlags,
    event_ids: *const FSEventStreamEventId,
) {
    if client_callback_info.is_null() || num_events == 0 {
        return;
    }
    let info = unsafe { &*(client_callback_info as *const CallbackInfo) };
    let exclude = info.emitter.exclude();
    let paths_array = event_paths as CFArrayRef;
    let mut raw = Vec::with_capacity(num_events);
    let mut rescan = None;
    let mut undecodable = 0usize;
    if !paths_array.is_null() {
        let count = unsafe { CFArrayGetCount(paths_array) }.max(0) as usize;
        let total = num_events.min(count);
        for i in 0..total {
            let flags = if event_flags.is_null() {
                0
            } else {
                unsafe { *event_flags.add(i) }
            };
            let id = if event_ids.is_null() {
                i as FSEventStreamEventId
            } else {
                unsafe { *event_ids.add(i) }
            };
            if flags & FLAG_ROOT_CHANGED != 0 {
                rescan = Some(RescanReason::RootChanged);
                continue;
            }
            if flags & OVERFLOW_FLAGS != 0 {
                rescan.get_or_insert(RescanReason::Overflow);
                continue;
            }
            let cf_path =
                unsafe { CFArrayGetValueAtIndex(paths_array, i as CFIndex) } as CFStringRef;
            if cf_path.is_null() {
                undecodable += 1;
                continue;
            }
            let mut buf = vec![0 as c_char; 8192];
            let ok = unsafe {
                CFStringGetCString(
                    cf_path,
                    buf.as_mut_ptr(),
                    buf.len() as CFIndex,
                    K_CF_STRING_ENCODING_UTF8,
                )
            };
            if ok == 0 {
                undecodable += 1;
                continue;
            }
            let path = PathBuf::from(
                unsafe { CStr::from_ptr(buf.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            );
            // Exclusions apply before any probe.
            if exclude.excludes(&info.root, &path, flags & FLAG_ITEM_IS_DIR != 0) {
                continue;
            }
            raw.push(RawEvent { id, path, flags });
        }
    } else {
        undecodable = num_events;
    }
    raw.sort_by_key(|event| event.id);
    if let Some(reason) = rescan {
        info.emitter.emit(
            info.mount.clone(),
            info.root.clone(),
            FileSystemEventKind::RescanRequired {
                root: info.root.clone(),
                reason,
                missing: None,
            },
            Some(true),
        );
    }
    let classified = {
        let mut inventory = info.inventory.borrow_mut();
        classify(&raw, &probe, &mut inventory, &list_dir)
    };
    if classified.is_empty() && rescan.is_none() && undecodable > 0 {
        // Paths could not be decoded: report the root so nothing is silent.
        info.emitter.emit(
            info.mount.clone(),
            info.root.clone(),
            FileSystemEventKind::Changed,
            Some(true),
        );
        return;
    }
    for (path, kind, is_dir) in classified {
        info.emitter.emit(info.mount.clone(), path, kind, is_dir);
    }
}

pub struct PlatformWatcher {
    run_loop: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformWatcher {
    pub fn start(roots: Vec<WatchRoot>, emitter: Arc<Emitter>) -> Result<Self, String> {
        let run_loop = Arc::new(Mutex::new(0usize));
        let stop = Arc::new(AtomicBool::new(false));
        let run_loop_thread = Arc::clone(&run_loop);
        let stop_thread = Arc::clone(&stop);
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("fswatch-macos".to_string())
            .spawn(move || {
                run_loop_thread_main(roots, emitter, run_loop_thread, stop_thread, ready_tx)
            })
            .map_err(|err| format!("failed to spawn macos watcher thread: {}", err))?;

        ready_rx
            .recv()
            .map_err(|_| "macos watcher thread exited before initialization".to_string())?;

        Ok(Self {
            run_loop,
            stop,
            thread: Some(thread),
        })
    }

    /// A backend that watches nothing; `stop` is a no-op.
    pub fn idle() -> Self {
        Self {
            run_loop: Arc::new(Mutex::new(0)),
            stop: Arc::new(AtomicBool::new(true)),
            thread: None,
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let run_loop = self.run_loop.lock().ok().map(|guard| *guard).unwrap_or(0);
        if run_loop != 0 {
            unsafe {
                CFRunLoopStop(run_loop as CFRunLoopRef);
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_loop_thread_main(
    roots: Vec<WatchRoot>,
    emitter: Arc<Emitter>,
    run_loop_slot: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
    ready_tx: mpsc::Sender<()>,
) {
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    if let Ok(mut slot) = run_loop_slot.lock() {
        *slot = run_loop as usize;
    }

    let roots_for_poll = roots.clone();
    let mut streams = Vec::<FSEventStreamRef>::new();
    let mut arrays = Vec::<CFArrayRef>::new();
    let mut strings = Vec::<CFStringRef>::new();
    let mut infos = Vec::<*mut CallbackInfo>::new();

    for root in roots {
        let Ok(c_root) = CString::new(root.path.as_os_str().as_bytes()) else {
            continue;
        };
        let cf_root = unsafe {
            CFStringCreateWithCString(std::ptr::null(), c_root.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if cf_root.is_null() {
            continue;
        }

        let values = [cf_root as *const c_void];
        let cf_array =
            unsafe { CFArrayCreate(std::ptr::null(), values.as_ptr(), 1, std::ptr::null()) };
        if cf_array.is_null() {
            unsafe {
                CFRelease(cf_root);
            }
            continue;
        }

        let callback_info = Box::new(CallbackInfo {
            mount: root.mount,
            root: root.path,
            emitter: Arc::clone(&emitter),
            inventory: RefCell::new(Inventory::default()),
        });
        let callback_info_ptr = Box::into_raw(callback_info);
        let mut context = FSEventStreamContext {
            version: 0,
            info: callback_info_ptr as *mut c_void,
            retain: Some(context_retain),
            release: Some(context_release),
            copy_description: None,
        };

        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                fsevent_callback,
                &mut context,
                cf_array,
                K_FS_EVENT_STREAM_EVENT_ID_SINCE_NOW,
                STREAM_LATENCY,
                K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS
                    | K_FS_EVENT_STREAM_CREATE_FLAG_USE_CF_TYPES
                    | K_FS_EVENT_STREAM_CREATE_FLAG_NO_DEFER,
            )
        };
        if stream.is_null() {
            unsafe {
                context_release(callback_info_ptr as *const c_void);
                CFRelease(cf_array);
                CFRelease(cf_root);
            }
            continue;
        }

        unsafe {
            FSEventStreamScheduleWithRunLoop(stream, run_loop, kCFRunLoopDefaultMode);
        }
        let started = unsafe { FSEventStreamStart(stream) };
        if started == 0 {
            unsafe {
                FSEventStreamInvalidate(stream);
                FSEventStreamRelease(stream);
                CFRelease(cf_array);
                CFRelease(cf_root);
            }
            continue;
        }

        streams.push(stream);
        arrays.push(cf_array);
        strings.push(cf_root);
        infos.push(callback_info_ptr);
    }

    if !streams.is_empty() {
        // Watching from here: readiness first, then the bounded inventory
        // seed. Events during the seed queue on the stream and are
        // classified afterwards against the seeded inventory.
        let _ = ready_tx.send(());
        for info in &infos {
            // The stream retains the info until the run loop stops below;
            // no callback runs before CFRunLoopRun, so this borrow is the
            // only one.
            let info = unsafe { &**info };
            let mut inventory = info.inventory.borrow_mut();
            seed_tree(&info.root, info.emitter.exclude(), &mut inventory, INVENTORY_SEED_BUDGET);
        }
        if !stop.load(Ordering::Relaxed) {
            unsafe {
                CFRunLoopRun();
            }
        }
    } else {
        let _ = ready_tx.send(());
        poll_loop(roots_for_poll, emitter, stop);
    }

    for stream in streams {
        unsafe {
            FSEventStreamStop(stream);
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
        }
    }
    for array in arrays {
        unsafe { CFRelease(array) };
    }
    for string in strings {
        unsafe { CFRelease(string) };
    }

    if let Ok(mut slot) = run_loop_slot.lock() {
        *slot = 0;
    }
}

/// Fallback when no FSEvents stream could be created: root-level `Changed`
/// on fingerprint change plus one forced emit per second. Never per-file.
fn poll_loop(roots: Vec<WatchRoot>, emitter: Arc<Emitter>, stop: Arc<AtomicBool>) {
    const FORCE_EMIT_INTERVAL: Duration = Duration::from_secs(1);

    let exclude = emitter.exclude();
    let mut fingerprints: HashMap<String, u64> = HashMap::new();
    let mut last_emit: HashMap<String, std::time::Instant> = HashMap::new();
    for root in &roots {
        fingerprints.insert(root.mount.clone(), fingerprint_tree(&root.path, exclude));
        last_emit.insert(root.mount.clone(), std::time::Instant::now());
    }

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(220));
        for root in &roots {
            let next = fingerprint_tree(&root.path, exclude);
            let prev = fingerprints.entry(root.mount.clone()).or_insert(next);
            let changed = *prev != next;
            let now = std::time::Instant::now();
            let should_force_emit = last_emit
                .get(&root.mount)
                .is_some_and(|ts| now.saturating_duration_since(*ts) >= FORCE_EMIT_INTERVAL);
            if changed {
                *prev = next;
            }
            if changed || should_force_emit {
                last_emit.insert(root.mount.clone(), now);
                emitter.emit(
                    root.mount.clone(),
                    root.path.clone(),
                    FileSystemEventKind::Changed,
                    Some(true),
                );
            }
        }
    }
}

fn fingerprint_tree(root: &Path, exclude: &ExcludePolicy) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        // Never follow symlinks: a link out of the tree must not pull the
        // rest of the disk into the fingerprint. Only regular files and
        // real directories count.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !(meta.is_dir() || meta.is_file()) {
            continue;
        }
        if meta.is_dir() && exclude.excludes_dir(root, &path) {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        rel.to_string_lossy().hash(&mut hasher);
        meta.is_dir().hash(&mut hasher);
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            if let Ok(delta) = modified.duration_since(SystemTime::UNIX_EPOCH) {
                delta.as_nanos().hash(&mut hasher);
            }
        }
        if meta.is_dir() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut children = entries
                .filter_map(|entry| entry.ok().map(|v| v.path()))
                .collect::<Vec<_>>();
            children.sort();
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(path: &str, flags: FSEventStreamEventFlags) -> RawEvent {
        RawEvent {
            id: 0,
            path: PathBuf::from(path),
            flags,
        }
    }

    /// A scripted directory: `/w` holds the listed names.
    fn listing(names: &[(&str, Identity)]) -> impl Fn(&Path) -> Option<Listing> {
        let names: Vec<(OsString, Identity, bool)> = names
            .iter()
            .map(|(name, identity)| (OsString::from(name), *identity, false))
            .collect();
        move |dir: &Path| {
            if dir == Path::new("/w") {
                Some(names.clone())
            } else {
                None
            }
        }
    }

    #[test]
    fn rename_pairs_only_on_unique_identity_continuity() {
        let events = vec![
            raw("/w/a.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
            raw("/w/b.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
        ];
        let probe = |p: &Path| {
            if p.ends_with("a.rs") {
                Probe::Missing
            } else {
                Probe::File((1, 100))
            }
        };
        let list = listing(&[("a.rs", (1, 100))]);
        let mut inventory = Inventory::default();
        let out = classify(&events, &probe, &mut inventory, &list);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, PathBuf::from("/w/b.rs"));
        assert_eq!(
            out[0].1,
            FileSystemEventKind::Renamed {
                from: PathBuf::from("/w/a.rs")
            }
        );
        assert_eq!(out[0].2, Some(false));
        assert_eq!(inventory.lookup(Path::new("/w/b.rs"), &list), Knowledge::Known((1, 100), false));
        assert_eq!(inventory.lookup(Path::new("/w/a.rs"), &list), Knowledge::Absent);

        // The same batch without identity continuity degrades honestly.
        let other = |p: &Path| {
            if p.ends_with("a.rs") {
                Probe::Missing
            } else {
                Probe::File((1, 200))
            }
        };
        let mut inventory = Inventory::default();
        let out = classify(&events, &other, &mut inventory, &list);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (PathBuf::from("/w/a.rs"), FileSystemEventKind::Removed, Some(false)));
        assert_eq!(out[1], (PathBuf::from("/w/b.rs"), FileSystemEventKind::Created, Some(false)));

        // Interleaved renames pair by identity, not by adjacency.
        let events = vec![
            raw("/w/a.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
            raw("/w/c.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
            raw("/w/d.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
            raw("/w/b.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
        ];
        let probe = |p: &Path| match p.file_name().and_then(|n| n.to_str()) {
            Some("a.rs") | Some("c.rs") => Probe::Missing,
            Some("d.rs") => Probe::File((1, 300)),
            _ => Probe::File((1, 100)),
        };
        let list = listing(&[("a.rs", (1, 100)), ("c.rs", (1, 300))]);
        let mut inventory = Inventory::default();
        let out = classify(&events, &probe, &mut inventory, &list);
        assert_eq!(
            out,
            vec![
                (
                    PathBuf::from("/w/b.rs"),
                    FileSystemEventKind::Renamed {
                        from: PathBuf::from("/w/a.rs")
                    },
                    Some(false)
                ),
                (
                    PathBuf::from("/w/d.rs"),
                    FileSystemEventKind::Renamed {
                        from: PathBuf::from("/w/c.rs")
                    },
                    Some(false)
                ),
            ]
        );

        // Two candidates with the same identity (hard links): no pairing.
        let events = vec![
            raw("/w/a.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
            raw("/w/b.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
            raw("/w/c.rs", FLAG_ITEM_RENAMED | FLAG_ITEM_IS_FILE),
        ];
        let probe = |p: &Path| {
            if p.ends_with("a.rs") {
                Probe::Missing
            } else {
                Probe::File((1, 100))
            }
        };
        let list = listing(&[("a.rs", (1, 100))]);
        let mut inventory = Inventory::default();
        let out = classify(&events, &probe, &mut inventory, &list);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|(_, kind, _)| !matches!(kind, FileSystemEventKind::Renamed { .. })));
    }

    #[test]
    fn creation_is_the_inventorys_verdict_not_a_flag() {
        let list = listing(&[("old.rs", (1, 5))]);
        let created_flag = 0x0000_0100 | FLAG_ITEM_IS_FILE;
        // A sticky created flag on a known file is a change.
        let mut inventory = Inventory::default();
        let out = classify(
            &[raw("/w/old.rs", created_flag)],
            &|_| Probe::File((1, 5)),
            &mut inventory,
            &list,
        );
        assert_eq!(out[0].1, FileSystemEventKind::Changed);
        // A file the inventory did not have is created, whatever the flags.
        let out = classify(
            &[raw("/w/new.rs", FLAG_ITEM_IS_FILE)],
            &|_| Probe::File((1, 6)),
            &mut inventory,
            &list,
        );
        assert_eq!(out[0].1, FileSystemEventKind::Created);
        assert_eq!(inventory.lookup(Path::new("/w/new.rs"), &list), Knowledge::Known((1, 6), false));
        // Seen again: a change.
        let out = classify(
            &[raw("/w/new.rs", created_flag)],
            &|_| Probe::File((1, 6)),
            &mut inventory,
            &list,
        );
        assert_eq!(out[0].1, FileSystemEventKind::Changed);
        // A missing path is removed and leaves the inventory.
        let gone = classify(
            &[raw("/w/new.rs", 0x0000_0200 | FLAG_ITEM_IS_FILE)],
            &|_| Probe::Missing,
            &mut inventory,
            &list,
        );
        assert_eq!(gone[0].1, FileSystemEventKind::Removed);
        assert_eq!(gone[0].2, Some(false));
        assert_eq!(inventory.lookup(Path::new("/w/new.rs"), &list), Knowledge::Absent);
        // No evidence (an unlistable directory): never "created".
        let no_evidence = |_: &Path| -> Option<Listing> { None };
        let mut blind = Inventory::default();
        let out = classify(
            &[raw("/x/y.rs", created_flag)],
            &|_| Probe::File((1, 7)),
            &mut blind,
            &no_evidence,
        );
        assert_eq!(out[0].1, FileSystemEventKind::Changed);
    }

    #[test]
    fn inventory_is_bounded_and_evicts_least_recently_touched_directories() {
        let mut inventory = Inventory::default();
        let big: Listing = (0..INVENTORY_MAX_ENTRIES)
            .map(|i| (OsString::from(format!("f{i}")), (1, i as u64), false))
            .collect();
        inventory.seed(Path::new("/a"), Some(big));
        inventory.seed(Path::new("/b"), Some(vec![(OsString::from("x"), (1, 9), false)]));
        assert!(inventory.entries <= INVENTORY_MAX_ENTRIES);
        assert!(!inventory.dirs.contains_key(Path::new("/a")), "the oldest directory is evicted");
        assert!(inventory.dirs.contains_key(Path::new("/b")));
    }

    #[test]
    fn seed_tree_lists_the_top_of_a_tree_within_budget_and_honours_exclusions() {
        let dir = std::env::temp_dir().join(format!(
            "fswatch-seed-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("target/debug")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "a").unwrap();
        std::fs::write(dir.join("src/deep/b.rs"), "b").unwrap();
        std::fs::write(dir.join("target/debug/x.o"), "x").unwrap();
        let mut inventory = Inventory::default();
        seed_tree(&dir, &ExcludePolicy::default(), &mut inventory, INVENTORY_SEED_BUDGET);
        assert!(inventory.dirs.contains_key(&dir.join("src")));
        assert!(inventory.dirs.contains_key(&dir.join("src/deep")));
        assert!(!inventory.dirs.contains_key(&dir.join("target")), "excluded: never walked");
        assert!(!inventory.dirs.contains_key(&dir.join("target/debug")));
        assert!(matches!(
            inventory.lookup(&dir.join("src/deep/b.rs"), &list_dir),
            Knowledge::Known(_, false)
        ));
        let mut small = Inventory::default();
        seed_tree(&dir, &ExcludePolicy::default(), &mut small, 1);
        assert!(small.dirs.contains_key(&dir), "the root is always seeded first");
        assert!(!small.dirs.contains_key(&dir.join("src/deep")), "budget stops the walk");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
