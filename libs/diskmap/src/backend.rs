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
        let mut cached = sizecache::load(root)?;
        // DiskMap calls this on its worker. Cached sizes remain valid, but
        // source membership can change without any file changing size.
        let mut kinds = SourceKinds::new(root);
        reclassify(&mut cached.tree, root, &mut kinds);
        Some(cached)
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
        // This mutex belongs solely to scan workers, never to the UI.
        let kinds = std::sync::Mutex::new(SourceKinds::new(root));
        let classify = |p: &Path, is_dir: bool| {
            kinds.lock().unwrap_or_else(|e| e.into_inner()).classify(p, is_dir)
        };
        let home = kind::home_dir();
        let skip = |path: &Path| kind::skip_for_scan(path, &home);
        let rules = ScanRules {
            classify: &classify,
            skip: &skip,
        };
        treemap::scan_stream(root, &rules, cancel, sink, pool)
    }
}

/// One source filter per native scan; no source bytes are read or hashed.
struct SourceKinds {
    root: std::path::PathBuf,
    sources: makepad_code_graph::FsSourceSet,
    reported_error: bool,
}

impl SourceKinds {
    fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            sources: makepad_code_graph::FsSourceSet::new(root.into(), Default::default()),
            reported_error: false,
        }
    }

    fn classify(&mut self, path: &Path, is_dir: bool) -> u8 {
        let kind = kind_for(path, is_dir);
        if kind != kind::FileKind::Code { return kind as u8; }
        let allowed = path.strip_prefix(&self.root).ok().is_some_and(|path| {
            match self.sources.allows_path(&path.to_string_lossy().replace('\\', "/")) {
                Ok(allowed) => allowed,
                Err(error) => {
                    if !self.reported_error {
                        eprintln!("disk source membership unavailable: {error}");
                        self.reported_error = true;
                    }
                    false
                }
            }
        });
        if allowed { kind as u8 } else { kind::FileKind::Text as u8 }
    }
}

fn reclassify(node: &mut treemap::Node, path: &Path, kinds: &mut SourceKinds) {
    for child in &mut node.children {
        reclassify(child, &path.join(&child.name), kinds);
    }
    node.kind = if node.is_dir {
        node.children.iter().max_by_key(|child| child.size)
            .map_or(kind::FileKind::Folder as u8, |child| child.kind)
    } else {
        kinds.classify(path, false)
    };
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
