use crate::{FileSystemEvent, FileSystemEventKind, WatchCallback, WatchRoot};
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type Handle = *mut c_void;

const INVALID_HANDLE_VALUE: Handle = (-1isize) as Handle;

const FILE_LIST_DIRECTORY: u32 = 0x0001;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

const FILE_NOTIFY_CHANGE_FILE_NAME: u32 = 0x0000_0001;
const FILE_NOTIFY_CHANGE_DIR_NAME: u32 = 0x0000_0002;
const FILE_NOTIFY_CHANGE_ATTRIBUTES: u32 = 0x0000_0004;
const FILE_NOTIFY_CHANGE_SIZE: u32 = 0x0000_0008;
const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x0000_0010;
const FILE_NOTIFY_CHANGE_CREATION: u32 = 0x0000_0040;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: Handle,
    ) -> Handle;

    fn ReadDirectoryChangesW(
        hDirectory: Handle,
        lpBuffer: *mut c_void,
        nBufferLength: u32,
        bWatchSubtree: i32,
        dwNotifyFilter: u32,
        lpBytesReturned: *mut u32,
        lpOverlapped: *mut c_void,
        lpCompletionRoutine: *mut c_void,
    ) -> i32;

    fn CloseHandle(hObject: Handle) -> i32;
}

pub struct PlatformWatcher {
    stop: Arc<AtomicBool>,
    handles: Arc<Mutex<Vec<usize>>>,
    threads: Vec<JoinHandle<()>>,
}

impl PlatformWatcher {
    pub fn start(roots: Vec<WatchRoot>, on_event: WatchCallback) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let handles = Arc::new(Mutex::new(Vec::<usize>::new()));
        let mut threads = Vec::new();

        for root in roots {
            let stop_thread = Arc::clone(&stop);
            let callback = Arc::clone(&on_event);
            let handles_thread = Arc::clone(&handles);
            let thread = thread::Builder::new()
                .name(format!("fswatch-win-{}", root.mount))
                .spawn(move || watch_root_loop(root, stop_thread, callback, handles_thread))
                .map_err(|err| format!("failed to spawn windows watcher thread: {}", err))?;
            threads.push(thread);
        }

        Ok(Self {
            stop,
            handles,
            threads,
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut handles) = self.handles.lock() {
            for handle in handles.drain(..) {
                let handle = handle as Handle;
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
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

fn watch_root_loop(
    root: WatchRoot,
    stop: Arc<AtomicBool>,
    on_event: WatchCallback,
    handles: Arc<Mutex<Vec<usize>>>,
) {
    let wide = wide_null(root.path.as_os_str());
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };

    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return;
    }

    if let Ok(mut list) = handles.lock() {
        list.push(handle as usize);
    }

    let mut buffer = vec![0u8; 64 * 1024];
    while !stop.load(Ordering::Relaxed) {
        let mut bytes_returned = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                handle,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                1,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_ATTRIBUTES
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_CREATION,
                &mut bytes_returned,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };

        if ok == 0 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        if bytes_returned > 0 {
            on_event(FileSystemEvent {
                mount: root.mount.clone(),
                path: PathBuf::from(&root.path),
                kind: FileSystemEventKind::Changed,
            });
        }
    }

    if let Ok(mut list) = handles.lock() {
        list.retain(|h| *h != handle as usize);
    }
    unsafe {
        let _ = CloseHandle(handle);
    }
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}
