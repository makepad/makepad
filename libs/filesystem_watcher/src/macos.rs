use crate::{FileSystemEvent, FileSystemEventKind, WatchCallback, WatchRoot};
use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_double};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

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
    _event_paths: *mut c_void,
    _event_flags: *const FSEventStreamEventFlags,
    _event_ids: *const FSEventStreamEventId,
) {
    if client_callback_info.is_null() || num_events == 0 {
        return;
    }
    let info = unsafe { &*(client_callback_info as *const CallbackInfo) };
    (info.on_event)(FileSystemEvent {
        mount: info.mount.clone(),
        path: info.root.clone(),
        kind: FileSystemEventKind::Changed,
    });
}

pub struct PlatformWatcher {
    run_loop: Arc<Mutex<usize>>,
    thread: Option<JoinHandle<()>>,
}

impl PlatformWatcher {
    pub fn start(roots: Vec<WatchRoot>, on_event: WatchCallback) -> Result<Self, String> {
        let run_loop = Arc::new(Mutex::new(0usize));
        let run_loop_thread = Arc::clone(&run_loop);
        let thread = thread::Builder::new()
            .name("fswatch-macos".to_string())
            .spawn(move || run_loop_thread_main(roots, on_event, run_loop_thread))
            .map_err(|err| format!("failed to spawn macos watcher thread: {}", err))?;

        Ok(Self {
            run_loop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
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

fn run_loop_thread_main(roots: Vec<WatchRoot>, on_event: WatchCallback, run_loop_slot: Arc<Mutex<usize>>) {
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    if let Ok(mut slot) = run_loop_slot.lock() {
        *slot = run_loop as usize;
    }

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
