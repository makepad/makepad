use crate::{FileSystemEvent, FileSystemEventKind, WatchCallback, WatchRoot};
use std::collections::HashMap;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_double};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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

struct CallbackInfo {
    mount: String,
    root: PathBuf,
    on_event: WatchCallback,
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

extern "C" fn fsevent_callback(
    _stream_ref: FSEventStreamRef,
    client_callback_info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    _event_flags: *const FSEventStreamEventFlags,
    _event_ids: *const FSEventStreamEventId,
) {
    if client_callback_info.is_null() || num_events == 0 {
        return;
    }
    let info = unsafe { &*(client_callback_info as *const CallbackInfo) };
    let paths_array = event_paths as CFArrayRef;
    let mut emitted = 0usize;
    if !paths_array.is_null() {
        let count = unsafe { CFArrayGetCount(paths_array) }.max(0) as usize;
        let total = num_events.min(count);
        for i in 0..total {
            let cf_path = unsafe { CFArrayGetValueAtIndex(paths_array, i as CFIndex) } as CFStringRef;
            if cf_path.is_null() {
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
                continue;
            }
            let path = unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            (info.on_event)(FileSystemEvent {
                mount: info.mount.clone(),
                path: PathBuf::from(path),
                kind: FileSystemEventKind::Changed,
            });
            emitted += 1;
        }
    }
    if emitted == 0 {
        (info.on_event)(FileSystemEvent {
            mount: info.mount.clone(),
            path: info.root.clone(),
            kind: FileSystemEventKind::Changed,
        });
    }
}

pub struct PlatformWatcher {
    run_loop: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformWatcher {
    pub fn start(roots: Vec<WatchRoot>, on_event: WatchCallback) -> Result<Self, String> {
        let run_loop = Arc::new(Mutex::new(0usize));
        let stop = Arc::new(AtomicBool::new(false));
        let run_loop_thread = Arc::clone(&run_loop);
        let stop_thread = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("fswatch-macos".to_string())
            .spawn(move || run_loop_thread_main(roots, on_event, run_loop_thread, stop_thread))
            .map_err(|err| format!("failed to spawn macos watcher thread: {}", err))?;

        Ok(Self {
            run_loop,
            stop,
            thread: Some(thread),
        })
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
    on_event: WatchCallback,
    run_loop_slot: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
) {
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    if let Ok(mut slot) = run_loop_slot.lock() {
        *slot = run_loop as usize;
    }

    let roots_for_poll = roots.clone();
    let mut streams = Vec::<FSEventStreamRef>::new();
    let mut arrays = Vec::<CFArrayRef>::new();
    let mut strings = Vec::<CFStringRef>::new();

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
        let cf_array = unsafe {
            CFArrayCreate(
                std::ptr::null(),
                values.as_ptr(),
                1,
                std::ptr::null(),
            )
        };
        if cf_array.is_null() {
            unsafe {
                CFRelease(cf_root);
            }
            continue;
        }

        let callback_info = Box::new(CallbackInfo {
            mount: root.mount,
            root: root.path,
            on_event: Arc::clone(&on_event),
        });
        let callback_info_ptr = Box::into_raw(callback_info) as *mut c_void;
        let mut context = FSEventStreamContext {
            version: 0,
            info: callback_info_ptr,
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
                0.1,
                K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS
                    | K_FS_EVENT_STREAM_CREATE_FLAG_USE_CF_TYPES
                    | K_FS_EVENT_STREAM_CREATE_FLAG_NO_DEFER,
            )
        };
        if stream.is_null() {
            unsafe {
                context_release(callback_info_ptr);
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
    }

    if !streams.is_empty() {
        unsafe {
            CFRunLoopRun();
        }
    } else {
        poll_loop(roots_for_poll, on_event, stop);
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

fn poll_loop(roots: Vec<WatchRoot>, on_event: WatchCallback, stop: Arc<AtomicBool>) {
    const FORCE_EMIT_INTERVAL: Duration = Duration::from_secs(1);

    let mut fingerprints: HashMap<String, u64> = HashMap::new();
    let mut last_emit: HashMap<String, std::time::Instant> = HashMap::new();
    for root in &roots {
        fingerprints.insert(root.mount.clone(), fingerprint_tree(&root.path));
        last_emit.insert(root.mount.clone(), std::time::Instant::now());
    }

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(220));
        for root in &roots {
            let next = fingerprint_tree(&root.path);
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
                (on_event)(FileSystemEvent {
                    mount: root.mount.clone(),
                    path: root.path.clone(),
                    kind: FileSystemEventKind::Changed,
                });
            }
        }
    }
}

fn fingerprint_tree(root: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
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
