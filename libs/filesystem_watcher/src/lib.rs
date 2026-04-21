use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct WatchRoot {
    pub mount: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub enum FileSystemEventKind {
    Changed,
}

#[derive(Clone, Debug)]
pub struct FileSystemEvent {
    pub mount: String,
    pub path: PathBuf,
    pub kind: FileSystemEventKind,
}

type WatchCallback = Arc<dyn Fn(FileSystemEvent) + Send + Sync + 'static>;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as imp;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use unsupported as imp;

pub struct FileSystemWatcher {
    inner: imp::PlatformWatcher,
}

impl FileSystemWatcher {
    pub fn start<F>(roots: Vec<WatchRoot>, on_event: F) -> Result<Self, String>
    where
        F: Fn(FileSystemEvent) + Send + Sync + 'static,
    {
        let callback: WatchCallback = Arc::new(on_event);
        let inner = imp::PlatformWatcher::start(roots, callback)?;
        Ok(Self { inner })
    }
}

impl Drop for FileSystemWatcher {
    fn drop(&mut self) {
        self.inner.stop();
    }
}
