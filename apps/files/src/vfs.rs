//! The filesystem seam.
//!
//! Every place the browser touches "the filesystem" goes through one [`Vfs`]:
//! listing a folder, measuring it, mapping it for the treemap, opening a file
//! in a viewer, and every operation that changes something. There are two
//! implementations — [`RealVfs`], which is `std::fs` and the app's normal life,
//! and the demo one, which is a plausible home held in memory so a screen
//! recording can show the whole app without showing anybody's real disk.
//!
//! A process has exactly one filesystem for its whole life, so the choice is
//! installed once at startup and read from anywhere, worker threads included.
//! Threading a handle through every signature would buy nothing: no part of
//! this app ever wants a *different* filesystem than the rest of it.
//!
//! Virtual files are also statted and read through this seam. `native_path`
//! is reserved for native integrations backed by [`RealVfs`]; the closed
//! demo returns typed [`VfsError::Unavailable`] instead of mapping a virtual
//! name onto the host disk.

use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, OnceLock},
};

use crate::{
    model::{self, FileEntry},
    ops::{OpKind, OpRequest, Undo},
    sizecache::Cached,
    treemap::{self, Node, ScanProgress, ScanRules, ScanStep},
};

/// A capability the active filesystem deliberately does not provide.
///
/// Virtual paths have no honest host path or native cache file. Keeping that
/// answer typed makes an accidentally reached native integration fail closed
/// instead of turning the virtual path into a host path and reaching the
/// platform's unsupported-filesystem trap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VfsError {
    Unavailable(&'static str),
    Io(String),
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VfsError::Unavailable(capability) => write!(f, "{capability} is unavailable"),
            VfsError::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for VfsError {}

/// What an operation did, once it is done: the sentence for the status bar,
/// how to reverse it, and the paths worth selecting afterwards.
pub struct OpOutcome {
    pub message: String,
    pub undo: Option<Undo>,
    pub touched: Vec<PathBuf>,
}

/// The filesystem the browser is looking at.
pub trait Vfs: Send + Sync {
    /// The folder a fresh window opens in.
    fn home(&self) -> PathBuf;

    /// The filesystem's wall clock, in seconds since the epoch. Demo
    /// filesystems pin this to the same instant as their generated dates.
    fn now_secs(&self) -> u64;

    /// One directory listing, sorted the way [`model::read_directory`] sorts.
    /// Real disks are dispatched to a worker; instant backends run inline.
    fn read_dir(&self, path: &Path, show_hidden: bool) -> Result<Vec<FileEntry>, String>;

    /// Metadata for one path, including paths below the current listing.
    fn stat(&self, path: &Path) -> Result<FileEntry, String>;

    /// At most `max` bytes of a file. Consumers must not assume the host has
    /// a corresponding path.
    fn read_bytes(&self, path: &Path, max: usize) -> Result<Vec<u8>, String>;

    fn is_dir(&self, path: &Path) -> bool;

    /// Whether the filesystem has anything at this path at all. The default
    /// asks the parent folder for its listing, which is the only question a
    /// virtual filesystem can always answer; a real one knows directly.
    fn exists(&self, path: &Path) -> bool {
        if self.is_dir(path) {
            return true;
        }
        let Some(parent) = path.parent() else {
            return false;
        };
        self.read_dir(parent, true)
            .map(|entries| entries.iter().any(|e| e.path == path))
            .unwrap_or(false)
    }

    /// The real file on disk behind a path, for native integrations that
    /// cannot consume bytes. A virtual filesystem returns typed Unavailable.
    fn native_path(&self, _path: &Path) -> Result<PathBuf, VfsError> {
        Err(VfsError::Unavailable("native filesystem path"))
    }

    /// Unix permission bits for the properties panel. Other backends have no
    /// inode mode to report.
    fn unix_mode(&self, _path: &Path) -> Result<u32, VfsError> {
        Err(VfsError::Unavailable("Unix file mode"))
    }

    /// Resolve links and `..` components for callers that enforce a path
    /// boundary. Virtual filesystems may return the already-normalized path.
    fn canonicalize(&self, _path: &Path) -> Result<PathBuf, VfsError> {
        Err(VfsError::Unavailable("path canonicalization"))
    }

    /// Whether a path is a link without following it.
    fn is_symlink(&self, _path: &Path) -> Result<bool, VfsError> {
        Err(VfsError::Unavailable("symbolic-link metadata"))
    }

    /// The native size-map cache. Virtual filesystems have no cache file;
    /// their scans are already instant.
    fn load_scan_cache(&self, _root: &Path) -> Result<Option<Cached>, VfsError> {
        Err(VfsError::Unavailable("native size-map cache"))
    }

    fn store_scan_cache(&self, _root: &Path, _bytes: &[u8]) -> Result<(), VfsError> {
        Err(VfsError::Unavailable("native size-map cache"))
    }

    fn forget_scan_cache(&self, _root: &Path) -> Result<(), VfsError> {
        Err(VfsError::Unavailable("native size-map cache"))
    }

    /// Recursive byte total, for the properties panel. Stops when cancelled.
    fn total_bytes(&self, path: &Path, cancel: &AtomicBool) -> u64;

    /// The tree the treemap draws.
    fn scan(
        &self,
        root: &Path,
        cancel: &AtomicBool,
        progress: &dyn Fn(ScanProgress),
    ) -> Option<Node>;

    /// The same tree, streamed back through `sink` as it is discovered, so a
    /// map of a full disk is drawable after one `read_dir` instead of after
    /// the whole walk. Returns false when the walk was cancelled.
    ///
    /// The default hands the finished tree over in one step, which is exactly
    /// right for a filesystem that answers instantly — there is nothing to
    /// stream when there is nothing to wait for. A real disk overrides it.
    fn scan_stream(
        &self,
        root: &Path,
        cancel: &AtomicBool,
        sink: &(dyn Fn(ScanStep) + Sync),
    ) -> bool {
        match self.scan(root, cancel, &|_| {}) {
            Some(node) => {
                sink(ScanStep::Opened {
                    at: Vec::new(),
                    children: node.children,
                    denied: false,
                });
                true
            }
            None => false,
        }
    }

    /// Perform an operation *synchronously*. Only a filesystem that can do so
    /// in no time at all implements this — see [`Vfs::is_instant`]; the real
    /// one hands its work to the operations engine's worker instead.
    fn perform(&self, request: &OpRequest) -> Result<OpOutcome, String>;

    /// Reverse a finished operation, synchronously. Same rule as `perform`.
    fn perform_undo(&self, undo: &Undo) -> Result<OpOutcome, String>;

    /// True when operations finish instantly and need no worker thread and no
    /// progress row — which is exactly what an in-memory tree is.
    fn is_instant(&self) -> bool;

    /// True when this is not the user's real disk, so the window can say so.
    fn is_demo(&self) -> bool {
        self.is_instant()
    }
}

/// `std::fs` — the app's normal life. Everything here delegates to the
/// modules that already own the behaviour, so there is exactly one
/// implementation of each rule.
pub struct RealVfs;

impl Vfs for RealVfs {
    fn home(&self) -> PathBuf {
        model::home_dir()
    }

    fn now_secs(&self) -> u64 {
        model::real_now_secs()
    }

    fn read_dir(&self, path: &Path, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
        model::read_directory(path, show_hidden)
    }

    fn stat(&self, path: &Path) -> Result<FileEntry, String> {
        model::real_entry_at(path).ok_or_else(|| format!("No such file: {}", path.display()))
    }

    fn read_bytes(&self, path: &Path, max: usize) -> Result<Vec<u8>, String> {
        use std::io::Read;

        let file = std::fs::File::open(path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        let mut data = Vec::with_capacity(max.min(64 * 1024));
        file.take(max as u64)
            .read_to_end(&mut data)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        Ok(data)
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn native_path(&self, path: &Path) -> Result<PathBuf, VfsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(path.to_path_buf())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(VfsError::Unavailable("native filesystem path"))
        }
    }

    fn unix_mode(&self, path: &Path) -> Result<u32, VfsError> {
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .map(|meta| meta.permissions().mode() & 0o7777)
                .map_err(|error| VfsError::Io(error.to_string()))
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            let _ = path;
            Err(VfsError::Unavailable("Unix file mode"))
        }
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::fs::canonicalize(path).map_err(|error| VfsError::Io(error.to_string()))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(VfsError::Unavailable("path canonicalization"))
        }
    }

    fn is_symlink(&self, path: &Path) -> Result<bool, VfsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_symlink())
                .map_err(|error| VfsError::Io(error.to_string()))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(VfsError::Unavailable("symbolic-link metadata"))
        }
    }

    fn load_scan_cache(&self, root: &Path) -> Result<Option<Cached>, VfsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(crate::sizecache::load(root))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = root;
            Err(VfsError::Unavailable("native size-map cache"))
        }
    }

    fn store_scan_cache(&self, root: &Path, bytes: &[u8]) -> Result<(), VfsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::sizecache::store(root, bytes);
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (root, bytes);
            Err(VfsError::Unavailable("native size-map cache"))
        }
    }

    fn forget_scan_cache(&self, root: &Path) -> Result<(), VfsError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::sizecache::forget(root);
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = root;
            Err(VfsError::Unavailable("native size-map cache"))
        }
    }

    fn total_bytes(&self, path: &Path, cancel: &AtomicBool) -> u64 {
        crate::ops::total_bytes(path, cancel)
    }

    fn scan(
        &self,
        root: &Path,
        cancel: &AtomicBool,
        progress: &dyn Fn(ScanProgress),
    ) -> Option<Node> {
        let classify = |p: &Path, is_dir: bool| model::kind_for(p, is_dir) as u8;
        let home = self.home();
        let skip = |path: &Path| model::skip_for_scan(path, &home);
        treemap::scan(root, &treemap::ScanRules { classify: &classify, skip: &skip }, cancel, progress)
    }

    fn scan_stream(
        &self,
        root: &Path,
        cancel: &AtomicBool,
        sink: &(dyn Fn(ScanStep) + Sync),
    ) -> bool {
        let classify = |p: &Path, is_dir: bool| model::kind_for(p, is_dir) as u8;
        let home = self.home();
        let skip = |path: &Path| model::skip_for_scan(path, &home);
        let rules = ScanRules {
            classify: &classify,
            skip: &skip,
        };
        treemap::scan_stream(root, &rules, cancel, sink)
    }

    fn perform(&self, _request: &OpRequest) -> Result<OpOutcome, String> {
        Err("the real filesystem runs its operations on the worker".to_string())
    }

    fn perform_undo(&self, _undo: &Undo) -> Result<OpOutcome, String> {
        Err("the real filesystem runs its operations on the worker".to_string())
    }

    fn is_instant(&self) -> bool {
        false
    }
}

static VFS: OnceLock<Arc<dyn Vfs>> = OnceLock::new();

/// Choose the filesystem for this process. Called once, before the UI reads
/// anything; a second call is ignored, because a browser that changed
/// filesystems underneath itself would be showing two different worlds.
pub fn install(vfs: Arc<dyn Vfs>) {
    let _ = VFS.set(vfs);
}

/// The filesystem this process is browsing.
pub fn vfs() -> &'static Arc<dyn Vfs> {
    VFS.get_or_init(|| {
        #[cfg(all(target_arch = "wasm32", feature = "demo"))]
        {
            Arc::new(crate::demo::DemoVfs::new())
        }
        #[cfg(not(all(target_arch = "wasm32", feature = "demo")))]
        {
            Arc::new(RealVfs)
        }
    })
}

/// True when the browser is showing the demo home rather than a real disk.
pub fn is_demo() -> bool {
    vfs().is_demo()
}

/// The active filesystem's wall clock.
pub fn now_secs() -> u64 {
    vfs().now_secs()
}

/// The demo is asked for by `--demo` on the command line or `MAKEPAD_FILES_DEMO=1`
/// in the environment, so it can be started from a launcher that has no
/// argument list of its own.
pub fn demo_requested() -> bool {
    cfg!(feature = "demo")
        || std::env::args().any(|a| a == "--demo")
        || std::env::var("MAKEPAD_FILES_DEMO").is_ok_and(|v| v != "0" && !v.is_empty())
}

/// The description of an operation, for the message an instant filesystem
/// hands back. Shared so the demo's sentences read like the real ones.
pub fn outcome_message(kind: OpKind, count: usize, where_to: &Path) -> String {
    let items = format!("{} item{}", count, if count == 1 { "" } else { "s" });
    match kind {
        OpKind::Copy => format!("Copied {items} to {}", model::display_name(where_to)),
        OpKind::Move => format!("Moved {items} to {}", model::display_name(where_to)),
        OpKind::Trash => format!("Moved {items} to the Trash"),
        OpKind::Rename => format!("Renamed {items}"),
        OpKind::NewFolder => format!("Created {}", model::display_name(where_to)),
        OpKind::Delete => format!("Deleted {items} permanently"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_filesystem_is_the_identity_on_paths() {
        let real = RealVfs;
        let path = Path::new("/a/b/c.png");
        assert_eq!(real.native_path(path).unwrap(), path);
        assert!(!real.is_instant());
        assert!(!real.is_demo());
    }

    #[test]
    fn virtual_native_entry_points_are_typed_unavailable() {
        let virtual_fs = crate::demo::DemoVfs::new();

        assert_eq!(
            virtual_fs.native_path(Path::new("/Demo/file")),
            Err(VfsError::Unavailable("native filesystem path"))
        );
        assert!(matches!(
            virtual_fs.forget_scan_cache(Path::new("/Demo")),
            Err(VfsError::Unavailable("native size-map cache"))
        ));
    }

    #[test]
    fn real_stat_and_bounded_reads_use_the_same_entry_shape() {
        let real = RealVfs;
        let path = std::env::temp_dir().join(format!("files-vfs-stat-{}", std::process::id()));
        std::fs::write(&path, b"abcdef").unwrap();
        let entry = real.stat(&path).unwrap();
        assert_eq!(entry.path, path);
        assert_eq!(entry.size, 6);
        assert!(!entry.is_dir);
        assert_eq!(real.read_bytes(&path, 3).unwrap(), b"abc");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn the_default_filesystem_is_the_real_one() {
        // Nothing installed anything in this test binary, so asking for the
        // filesystem must still answer — with the disk.
        assert!(!vfs().is_demo());
    }

    #[test]
    fn operation_sentences_read_the_same_either_way() {
        let dir = Path::new("/x/Documents");
        assert_eq!(outcome_message(OpKind::Copy, 1, dir), "Copied 1 item to Documents");
        assert_eq!(outcome_message(OpKind::Move, 3, dir), "Moved 3 items to Documents");
        assert_eq!(outcome_message(OpKind::Trash, 2, dir), "Moved 2 items to the Trash");
        assert_eq!(
            outcome_message(OpKind::Delete, 1, dir),
            "Deleted 1 item permanently"
        );
    }
}
