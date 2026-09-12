use crate::{Emitter, WatchRoot};
use std::sync::Arc;

pub struct PlatformWatcher;

impl PlatformWatcher {
    pub fn start(_roots: Vec<WatchRoot>, _emitter: Arc<Emitter>) -> Result<Self, String> {
        Err("filesystem watcher is not implemented on this platform".to_string())
    }

    pub fn idle() -> Self {
        Self
    }

    pub fn stop(&mut self) {}
}
