//! The JIT compile cache of `makepad run` as the phone's settings see it: its size, a request to
//! clear it (honoured by the host at the next process start: nothing is deleted under the
//! running compiler) and a restart into a new process. Only where the WM runs under
//! `makepad run` (MAKEPAD_RUN_HOME is set by the host); elsewhere there is no cache to show.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const UNKNOWN: u64 = u64::MAX;
static SIZE: AtomicU64 = AtomicU64::new(UNKNOWN);
static MEASURING: AtomicBool = AtomicBool::new(false);

fn home() -> Option<PathBuf> {
    std::env::var_os("MAKEPAD_RUN_HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Whether there is a JIT cache here at all.
pub fn available() -> bool {
    home().is_some()
}

fn dir_bytes(dir: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() { stack.push(e.path()); } else { total += meta.len(); }
        }
    }
    total
}

/// Measures the cache on a thread of its own (the page shows "measuring…" meanwhile).
pub fn refresh() {
    let Some(home) = home() else { return };
    if MEASURING.swap(true, Ordering::AcqRel) { return; }
    let _ = std::thread::Builder::new().name("jit-cache-size".into()).spawn(move || {
        SIZE.store(dir_bytes(&home.join("target")), Ordering::Release);
        MEASURING.store(false, Ordering::Release);
    });
}

/// "123.4 MB", or None while it is being measured.
pub fn size_text() -> Option<String> {
    let bytes = SIZE.load(Ordering::Acquire);
    (bytes != UNKNOWN).then(|| format!("{:.1} MB", bytes as f64 / 1048576.0))
}

/// A clear was asked for and the process has not restarted since.
pub fn clear_pending() -> bool {
    home().is_some_and(|h| h.join("clear-cache").exists())
}

/// The next process starts with an empty cache (cold).
pub fn request_clear() {
    if let Some(home) = home() {
        let _ = std::fs::write(home.join("clear-cache"), b"settings\n");
    }
}

/// A new process boots the WM again (Android: the host restarts the Activity's task).
pub fn restart() {
    #[cfg(target_os = "android")]
    {
        extern "C" {
            fn makepad_run_restart_process();
        }
        unsafe { makepad_run_restart_process() };
    }
}
