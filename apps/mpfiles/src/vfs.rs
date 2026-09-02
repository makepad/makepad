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
//! Virtual files are also statted and read through this seam. `real_path` is
//! reserved for native integrations backed by [`RealVfs`]; the closed demo
//! never maps a virtual name onto the host disk.

use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, OnceLock},
};

use crate::{
    model::{self, FileEntry},
    ops::{OpKind, OpRequest, Undo},
    treemap::{self, Node, ScanProgress, ScanRules, ScanStep},
};

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

    /// The real file on disk behind a path: what a decoder opens and what a
    /// viewer process is handed. The identity function for a real filesystem.
    fn real_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
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

    fn real_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
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
        treemap::scan(root, &treemap::ScanRules { classify: &classify, skip: &model::skip_for_scan }, cancel, progress)
    }

    fn scan_stream(
        &self,
        root: &Path,
        cancel: &AtomicBool,
        sink: &(dyn Fn(ScanStep) + Sync),
    ) -> bool {
        // Closures with nothing captured, so the walk's threads can all share
        // them without any synchronisation of their own.
        let classify = |p: &Path, is_dir: bool| model::kind_for(p, is_dir) as u8;
        let rules = ScanRules {
            classify: &classify,
            skip: &model::skip_for_scan,
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
    VFS.get_or_init(|| Arc::new(RealVfs))
}

/// True when the browser is showing the demo home rather than a real disk.
pub fn is_demo() -> bool {
    vfs().is_demo()
}

/// The demo is asked for by `--demo` on the command line or `MPFILES_DEMO=1`
/// in the environment, so it can be started from a launcher that has no
/// argument list of its own.
pub fn demo_requested() -> bool {
    cfg!(feature = "demo")
        || std::env::args().any(|a| a == "--demo")
        || std::env::var("MPFILES_DEMO").is_ok_and(|v| v != "0" && !v.is_empty())
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
        assert_eq!(real.real_path(path), path);
        assert!(!real.is_instant());
        assert!(!real.is_demo());
    }

    #[test]
    fn real_stat_and_bounded_reads_use_the_same_entry_shape() {
        let real = RealVfs;
        let path = std::env::temp_dir().join(format!("mpfiles-vfs-stat-{}", std::process::id()));
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
