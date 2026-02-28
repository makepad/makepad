use crate::{WatchCallback, WatchRoot};

pub struct PlatformWatcher;

impl PlatformWatcher {
    pub fn start(_roots: Vec<WatchRoot>, _on_event: WatchCallback) -> Result<Self, String> {
        Err("filesystem watcher is not implemented on this platform".to_string())
    }

    pub fn stop(&mut self) {}
}
