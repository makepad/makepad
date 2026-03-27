use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Result of attempting to enable single-instance mode.
pub enum SingleInstanceResult {
    /// This is the primary instance. Listener started.
    Primary,
    /// Another instance was running. Items forwarded. Process should exit.
    Secondary,
    /// Another instance is active for a different build identity.
    DifferentBuild,
}

/// Global item queue, drained by the event loop on signal/timer.
static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INITIAL_APP_OPEN_ENABLED: Mutex<bool> = Mutex::new(false);
static INITIAL_APP_OPEN_ITEMS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static APP_SOCKET_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn push_app_open_item(item: String) {
    PENDING.lock().unwrap().push(item);
}

pub fn push_app_open_items(items: Vec<String>) {
    PENDING.lock().unwrap().extend(items);
}

pub fn set_initial_app_open_enabled(enabled: bool) {
    *INITIAL_APP_OPEN_ENABLED.lock().unwrap() = enabled;
}

pub fn set_initial_app_open_items(items: Vec<String>) {
    *INITIAL_APP_OPEN_ITEMS.lock().unwrap() = items;
}

pub fn enqueue_initial_app_open_if_enabled() {
    if !*INITIAL_APP_OPEN_ENABLED.lock().unwrap() {
        return;
    }
    let mut initial = INITIAL_APP_OPEN_ITEMS.lock().unwrap();
    if initial.is_empty() {
        return;
    }
    PENDING.lock().unwrap().extend(std::mem::take(&mut *initial));
}

pub fn drain_app_open_items() -> Vec<String> {
    let mut q = PENDING.lock().unwrap();
    std::mem::take(&mut *q)
}

pub fn has_pending_items() -> bool {
    !PENDING.lock().unwrap().is_empty()
}

pub fn set_app_socket_path(path: PathBuf) {
    let _ = APP_SOCKET_PATH.set(path);
}

/// Returns the socket/pipe path if this process is the primary instance.
/// None if single-instance was not enabled or this is the secondary.
pub fn app_socket_path() -> Option<PathBuf> {
    APP_SOCKET_PATH.get().cloned()
}
