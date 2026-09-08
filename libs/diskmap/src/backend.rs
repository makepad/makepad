//! How the map talks to a filesystem. The default walks the real disk; the
//! files app installs a backend that goes through its VFS so the demo home
//! still maps.

use std::{
    path::Path,
    sync::{
        atomic::AtomicBool,
        Arc, OnceLock,
    },
};

use makepad_widgets::makepad_platform::thread::TaskPool;

use crate::{
    kind::{self, kind_for},
    sizecache::{self, Cached},
    treemap::{self, ScanRules, ScanStep},
};

/// The filesystem operations the map needs. Defaults walk the host disk.
pub trait ScanBackend: Send + Sync {
    fn now_secs(&self) -> u64 {
        sizecache::now()
    }

    fn is_instant(&self) -> bool {
        false
    }

    fn is_demo(&self) -> bool {
        self.is_instant()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn display_name(&self, path: &Path) -> String {
        kind::display_name(path)
    }

    fn load_scan_cache(&self, root: &Path) -> Option<Cached> {
        sizecache::load(root)
    }

    fn store_scan_cache(&self, root: &Path, bytes: &[u8]) {
        sizecache::store(root, bytes);
    }

    fn forget_scan_cache(&self, root: &Path) {
        sizecache::forget(root);
    }

    fn scan_stream(
        &self,
        root: &Path,
        cancel: &AtomicBool,
        sink: &(dyn Fn(ScanStep) + Sync),
        pool: &TaskPool,
    ) -> bool {
        let classify = |p: &Path, is_dir: bool| kind_for(p, is_dir) as u8;
        let home = kind::home_dir();
        let skip = |path: &Path| kind::skip_for_scan(path, &home);
        let rules = ScanRules {
            classify: &classify,
            skip: &skip,
        };
        treemap::scan_stream(root, &rules, cancel, sink, pool)
    }
}

/// The host disk.
pub struct NativeScan;

impl ScanBackend for NativeScan {}

static BACKEND: OnceLock<Arc<dyn ScanBackend>> = OnceLock::new();

/// Install the filesystem the map will use. Called once, before the UI reads
/// anything; a second call is ignored.
pub fn install_backend(backend: Arc<dyn ScanBackend>) {
    let _ = BACKEND.set(backend);
}

/// The filesystem the map is looking at.
pub fn backend() -> &'static Arc<dyn ScanBackend> {
    BACKEND.get_or_init(|| Arc::new(NativeScan))
}
