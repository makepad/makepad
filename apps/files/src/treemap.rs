//! The data layer behind the folder size map: a streaming recursive scan of a
//! directory's bytes, and the squarified-treemap geometry that turns that tree
//! into non-overlapping rectangles whose areas are proportional to the bytes
//! they stand for. Nothing here knows about widgets, drawing, or the rest of
//! the app — a [`Cell`] is just numbers a view can turn into quads, which is
//! what keeps this module runnable and testable on its own.
//!
//! Two things make this usable on a real, full disk rather than a toy folder.
//!
//! The scan **streams**: every directory, at every depth, announces its
//! listing the moment it has been read ([`ScanStep`]), so a map of a 1.8 TB
//! home starts drawing in milliseconds and sharpens as the walk goes deeper —
//! the picture is never more than one `read_dir` behind the walk. A 500 GB
//! folder four levels down fills in live like everything else, instead of
//! sitting as one opaque growing block until its whole subtree is done.
//!
//! The layout is **pixel-bounded, not depth-bounded**: it recurses all the way
//! down to individual files and stops only where a rectangle gets too small to
//! see. Siblings too small to draw are collapsed into one "N smaller items"
//! rectangle rather than being laid out and thrown away, so the cost of laying
//! out a folder is set by how many pixels it covers, not by how many files are
//! inside it. A folder with 200 000 files in a 40×40 box costs the same as one
//! with 200.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Condvar, Mutex,
    },
    time::Duration,
};

use makepad_widgets::makepad_platform::thread::{Lane, TaskPool};
use makepad_widgets::Cx;

/// A rectangle in treemap space. Plain `f64` so this module stays free of
/// any UI vector type — the view converts to its own types at the boundary.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// Whether `(px, py)` lands inside this rect. The right and bottom edges
    /// are excluded, so two rects tiled edge to edge never both claim the
    /// seam between them — a point on a shared border belongs to exactly
    /// one of the two, never both and never neither.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Plain `w * h`. A degenerate slice (zero or negative on an edge) just
    /// falls out of this as zero or negative rather than needing its own
    /// case — callers that care about "does this rect draw at all" check
    /// the edges directly.
    pub fn area(&self) -> f64 {
        self.w * self.h
    }

    /// This rect pulled in by `edge` on all four sides and by an extra `top`
    /// strip along the top. Never returns negative edges.
    pub fn shrink(&self, edge: f64, top: f64) -> Rect {
        Rect {
            x: self.x + edge,
            y: self.y + edge + top,
            w: (self.w - 2.0 * edge).max(0.0),
            h: (self.h - 2.0 * edge - top).max(0.0),
        }
    }

    /// Whether any part of this rect lies inside `other`. Zero-area rects
    /// intersect nothing, which is the right answer for a degenerate slice.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.w
            && self.x + self.w > other.x
            && self.y < other.y + other.h
            && self.y + self.h > other.y
    }
}

/// One scanned entry. Folders carry their children; files are leaves.
///
/// There is deliberately **no path here**. A full home directory is millions
/// of nodes, and a `PathBuf` per node is hundreds of megabytes of the same
/// prefixes over and over; the path of any node is its ancestors' names
/// joined, which [`layout`] rebuilds for the few thousand rectangles that
/// actually get drawn.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    pub is_dir: bool,
    /// False while the scan is still filling this subtree in — the rectangle
    /// is real but its size is still growing.
    pub done: bool,
    /// The folder is there and could not be read: a permission the app does
    /// not have. Its bytes are unknown, not zero, and the map has to say so
    /// rather than quietly leaving them out of the total.
    pub denied: bool,
    /// Opaque kind tag supplied by the caller's `classify` callback — this
    /// module never decides what a file *is*, only how big it is and where
    /// it sits in the tree. A folder inherits the kind of its heaviest child,
    /// so a folder full of video reads as video rather than as "folder".
    pub kind: u8,
    /// Recursive byte total for a folder; the file's own size for a leaf.
    pub size: u64,
    /// Files in this subtree. Cached rather than recounted, because a status
    /// line that recounted a million nodes on every progress tick would cost
    /// more than the scan.
    pub files: u32,
    /// When this subtree last changed: minutes since the epoch of the newest
    /// file under it (a leaf's own mtime), 0 when unknown. Minutes because a
    /// "show me what's new" filter never needs seconds and a u32 of minutes
    /// outlives everyone. A folder counts as new when anything inside it is.
    pub modified: u32,
    pub children: Vec<Node>,
}

impl Node {
    /// A folder with nothing in it yet.
    pub fn dir(name: String, kind: u8) -> Node {
        Node {
            name,
            is_dir: true,
            done: false,
            denied: false,
            kind,
            size: 0,
            files: 0,
            modified: 0,
            children: Vec::new(),
        }
    }

    /// A file with no known age — the tests' shorthand; the walk always
    /// knows better and uses [`Node::file_at`].
    #[cfg(test)]
    pub fn file(name: String, kind: u8, size: u64) -> Node {
        Node::file_at(name, kind, size, 0)
    }

    /// A file with its modification time, in minutes since the epoch.
    pub fn file_at(name: String, kind: u8, size: u64, modified: u32) -> Node {
        Node {
            name,
            is_dir: false,
            done: true,
            denied: false,
            kind,
            size,
            files: 1,
            modified,
            children: Vec::new(),
        }
    }

    fn at_mut(&mut self, at: &[u32]) -> Option<&mut Node> {
        let mut node = self;
        for &index in at {
            node = node.children.get_mut(index as usize)?;
        }
        Some(node)
    }

    /// The descendant `names` leads to.
    pub fn at(&self, names: &[String]) -> Option<&Node> {
        let mut node = self;
        for name in names {
            node = node.children.iter().find(|c| &c.name == name)?;
        }
        Some(node)
    }

    /// The child called `name`, for resolving a zoom path.
    pub fn child_named(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Fold `at`'s ancestors' totals back up after something below them
    /// changed. Only the chain named by `at` is touched — the rest of the
    /// tree cannot have moved, so nothing else needs recomputing.
    fn roll_up(&mut self, at: &[u32]) {
        for depth in (0..at.len()).rev() {
            let Some(node) = self.at_mut(&at[..depth]) else {
                return;
            };
            node.size = node.children.iter().map(|c| c.size).sum();
            node.files = node.children.iter().map(|c| c.files).sum();
            node.modified = node.children.iter().map(|c| c.modified).max().unwrap_or(0);
            node.kind = heaviest_kind(&node.children).unwrap_or(node.kind);
        }
    }

    /// Fold one streamed step into this tree. Returns false when the step
    /// names a node that is no longer there, which only happens if a caller
    /// mixes steps from two different scans.
    pub fn apply(&mut self, step: ScanStep) -> bool {
        match step {
            ScanStep::Opened {
                at,
                children,
                denied,
            } => {
                let Some(node) = self.at_mut(&at) else {
                    return false;
                };
                node.size = children.iter().map(|c| c.size).sum();
                node.files = children.iter().map(|c| c.files).sum();
                node.modified = children.iter().map(|c| c.modified).max().unwrap_or(0);
                node.kind = heaviest_kind(&children).unwrap_or(node.kind);
                node.children = children;
                node.denied = denied;
                self.roll_up(&at);
                true
            }
            ScanStep::Closed { at, node: fresh } => {
                let Some(node) = self.at_mut(&at) else {
                    return false;
                };
                *node = fresh;
                self.roll_up(&at);
                true
            }
            ScanStep::Pace { .. } => true,
            ScanStep::Growing { at, size, files } => {
                let Some(node) = self.at_mut(&at) else {
                    return false;
                };
                // A running total, so it must never go backwards and make a
                // rectangle shrink under the pointer.
                node.size = node.size.max(size);
                node.files = node.files.max(files);
                self.roll_up(&at);
                true
            }
        }
    }

    /// Where `names` leads, as child indices — the form everything else here
    /// works in. `None` when any step of it is not in the tree.
    fn indices_of(&self, names: &[String]) -> Option<Vec<u32>> {
        let mut node = self;
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let index = node.children.iter().position(|c| &c.name == name)?;
            out.push(index as u32);
            node = &node.children[index];
        }
        Some(out)
    }

    /// Take the descendant `names` leads to out of the tree and hand it back,
    /// subtracting its bytes from every folder above it.
    ///
    /// This is what makes deleting something cost nothing: the map already
    /// knows how big the thing was, so it can be removed from the picture
    /// exactly, and nothing has to be read off the disk again.
    pub fn detach(&mut self, names: &[String]) -> Option<Node> {
        let indices = self.indices_of(names)?;
        let (parent_at, last) = indices.split_at(indices.len().checked_sub(1)?);
        let parent = self.at_mut(parent_at)?;
        let index = *last.first()? as usize;
        if index >= parent.children.len() {
            return None;
        }
        let node = parent.children.remove(index);
        self.roll_up(&indices);
        Some(node)
    }

    /// Put `node` inside the folder `names` leads to, adding its bytes back
    /// up the chain. False when that folder is not in the tree — which is the
    /// right answer for a file moved somewhere the map is not of.
    pub fn graft(&mut self, names: &[String], node: Node) -> bool {
        let Some(indices) = self.indices_of(names) else {
            return false;
        };
        let Some(parent) = self.at_mut(&indices) else {
            return false;
        };
        if !parent.is_dir {
            return false;
        }
        // Replacing rather than duplicating: an operation that lands on a
        // name already there overwrote it, and two rectangles for one file
        // would be a map of a disk that does not exist.
        parent.children.retain(|c| c.name != node.name);
        parent.children.push(node);
        let mut chain = indices;
        chain.push(0);
        self.roll_up(&chain);
        true
    }

    /// The folders the scan was not allowed to open, by path relative to this
    /// tree, at most `limit` of them. A map that silently leaves out a folder
    /// it could not read is a map that lies about the total.
    pub fn denied_paths(&self, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_denied(&mut String::new(), limit, &mut out);
        out
    }

    fn collect_denied(&self, prefix: &mut String, limit: usize, out: &mut Vec<String>) {
        for child in &self.children {
            if out.len() >= limit {
                return;
            }
            if !child.is_dir {
                continue;
            }
            let mark = prefix.len();
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(&child.name);
            if child.denied {
                out.push(prefix.clone());
            } else {
                child.collect_denied(prefix, limit, out);
            }
            prefix.truncate(mark);
        }
    }

    /// Mark every folder in this tree finished. The walk hands whole subtrees
    /// back complete but announces the ones above them a level at a time, so
    /// "is this folder still growing" is only knowable for certain once the
    /// whole scan is over — which is exactly when this runs.
    pub fn seal(&mut self) {
        self.done = true;
        for child in &mut self.children {
            child.seal();
        }
    }
}

/// The kind of the heaviest child — what a folder paints as, so a folder full
/// of video reads blue and a folder full of cache reads grey without anyone
/// having to open it up.
fn heaviest_kind(children: &[Node]) -> Option<u8> {
    children.iter().max_by_key(|c| c.size).map(|c| c.kind)
}

/// Progress a blocking [`scan`] reports as it walks, so a caller can show a
/// live "N files, N bytes" line instead of a frozen spinner.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanProgress {
    pub files: u64,
    pub bytes: u64,
}

/// One step of a streaming scan. `at` is an index path from the scan's root:
/// `[]` is the root itself, `[3]` its fourth child, `[3, 1]` that child's
/// second. Indices are stable because [`ScanStep::Opened`] sets a directory's
/// children once and nothing ever reorders them.
#[derive(Debug)]
pub enum ScanStep {
    /// The listing of the directory at `at` has been read: these are its
    /// children, files already sized, directories still empty and not `done`.
    /// `denied` says the folder is there and could not be opened at all.
    Opened {
        at: Vec<u32>,
        children: Vec<Node>,
        denied: bool,
    },
    /// The subtree at `at` is finished and replaces whatever stood there.
    /// The walk itself no longer produces these — every directory streams its
    /// own [`ScanStep::Opened`] — but installing a saved map is exactly this
    /// step with `at` empty, so it stays.
    Closed { at: Vec<u32>, node: Node },
    /// Running totals for a directory that is still being walked, so a big
    /// folder's rectangle grows while it is being counted instead of sitting
    /// at zero until it is done. Every ancestor's total follows from it.
    Growing { at: Vec<u32>, size: u64, files: u32 },
    /// How many folders the walk still has open. Not a percentage — a scan
    /// cannot know its own denominator before it has walked the tree, and a
    /// bar that sits at 95 per cent for a minute is worse than no bar. This
    /// number is real, and it goes to zero exactly when the scan ends.
    Pace { folders_left: u32 },
}

/// How often a still-running subtree reports its running total.
const GROW_EVERY: Duration = Duration::from_millis(120);
/// How many entries pass between progress reports in the blocking [`scan`].
const PROGRESS_STRIDE: u64 = 512;

/// What one directory entry looks like before it becomes a [`Node`] — the
/// path is kept only for as long as the walk needs it to recurse, and is
/// never stored in the tree.
struct Listed {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: u32,
    kind: u8,
}

/// The device a path lives on, so a walk can stay on one volume.
#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::symlink_metadata(path).ok().map(|m| m.dev())
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> Option<u64> {
    None
}

/// What a walk is allowed to look at, and what a file *is*.
///
/// The skip rule is the reason a scan of a home directory does not make macOS
/// throw a permission dialog per protected folder: those folders are never
/// entered in the first place. It is a plain predicate so the policy lives
/// with the app that has an opinion about it, and this module stays a walker.
pub struct ScanRules<'a> {
    /// The opaque kind tag stored on each node. This module never decides
    /// what a file *is*, only how big it is and where it sits.
    pub classify: &'a (dyn Fn(&Path, bool) -> u8 + Sync),
    /// True for a directory the walk must not enter and must not count.
    pub skip: &'a (dyn Fn(&Path) -> bool + Sync),
}

/// What one directory read produced: its entries, and whether it could be
/// read at all.
struct Listing {
    entries: Vec<Listed>,
    /// The folder exists and the app is not allowed to look inside it. Tried
    /// exactly once, never per file — one refusal per folder is a note in the
    /// corner of the map, one per file is a storm of dialogs.
    denied: bool,
}

/// One directory's entries.
///
/// Symlinks are never followed: a link is recorded as its own leaf, sized by
/// the link itself and never by whatever it points at, and it is never
/// recursed into. That single rule is what keeps a cyclic link — a folder
/// somewhere under the root linking back to one of its own ancestors — from
/// turning a scan into an infinite walk.
///
/// A directory on a different volume than the root is skipped entirely: a
/// mounted backup disk under the folder being measured is not that folder's
/// bytes, and counting it would make every number on the map wrong. So is
/// anything [`ScanRules::skip`] refuses.
fn read_listing(
    dir: &Path,
    rules: &ScanRules,
    device: Option<u64>,
    growth: &mut Growth,
    pool: Option<&TaskPool>,
) -> Listing {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            // Not a scan failure. It is a folder we know exists and cannot
            // see into, recorded as such rather than aborting the walk — and
            // never opened a second time.
            return Listing {
                entries: Vec::new(),
                denied: error.kind() == std::io::ErrorKind::PermissionDenied,
            };
        }
    };

    // First the names, which cost nothing. `file_type` comes out of the
    // directory record itself wherever the filesystem carries one, and it
    // never follows a symlink — exactly the leaf treatment a link needs.
    let mut found: Vec<Found> = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        // The Finder's per-folder scratch file. It is on every disk, it is
        // never what anybody is cleaning up, and it makes a map of a photo
        // library half noise.
        if name == ".DS_Store" {
            continue;
        }
        found.push(Found {
            name,
            path: entry.path(),
            is_dir: file_type.is_dir(),
            size: 0,
            modified: 0,
            keep: true,
        });
    }

    // Then the metadata, which is the whole cost of a scan: one `lstat` per
    // entry, each of them a round trip to the disk on a tree nobody has
    // touched lately. They are independent, so on a directory big enough for
    // it to matter they are made to wait in parallel rather than in turn —
    // a folder with a quarter of a million files is otherwise one thread at
    // disk latency while five others have nothing to do.
    if let (true, Some(pool)) = (found.len() >= STAT_PARALLEL_MIN, pool) {
        // Independent, so a folder big enough for it to matter waits for its
        // `lstat`s in parallel rather than in turn — the caller-helping
        // `fan_out` replacement for the `std::thread::scope` this used to be.
        let chunk = found.len().div_ceil(STAT_THREADS);
        let slices: Vec<Option<&mut [Found]>> = found.chunks_mut(chunk).map(Some).collect();
        let count = slices.len();
        let slices = Mutex::new(slices);
        pool.fan_out(Lane::Heavy, count, |index| {
            let slice = slices.lock().unwrap_or_else(|e| e.into_inner())[index].take();
            if let Some(slice) = slice {
                stat_all(slice, device);
            }
        });
    } else {
        stat_all(&mut found, device);
    }

    let mut entries = Vec::with_capacity(found.len());
    for item in found {
        if !item.keep {
            continue;
        }
        if item.is_dir && (rules.skip)(&item.path) {
            continue;
        }
        if !item.is_dir {
            // Counted here rather than after the loop, because a directory
            // holding a quarter of a million files takes many seconds to
            // stat and the numbers on screen must not sit still for all of
            // them — a scan that looks frozen is a scan nobody waits for.
            growth.add(item.size);
        }
        entries.push(Listed {
            kind: (rules.classify)(&item.path, item.is_dir),
            name: item.name,
            path: item.path,
            is_dir: item.is_dir,
            size: item.size,
            modified: item.modified,
        });
    }
    Listing {
        entries,
        denied: false,
    }
}

/// One entry between "the directory says it is there" and "we know how big it
/// is". Directories carry no size and are only checked for being another
/// volume; files carry nothing but.
struct Found {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: u32,
    keep: bool,
}

/// A file mtime as whole minutes since the epoch, saturating; 0 when the
/// filesystem would not say.
fn modified_minutes(metadata: &fs::Metadata) -> u32 {
    metadata.modified().ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() / 60).min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

/// Size every file in `slice`, and drop every directory that turns out to sit
/// on another volume — a mounted backup disk under the folder being measured
/// is not that folder's bytes, and counting it would make every number on the
/// map wrong.
///
/// `symlink_metadata` never traverses the link, so a symlink is sized by the
/// link itself and never by whatever it points at.
fn stat_all(slice: &mut [Found], device: Option<u64>) {
    for item in slice {
        if item.is_dir {
            item.keep = device.is_none() || device_of(&item.path) == device;
        } else if let Ok(meta) = fs::symlink_metadata(&item.path) {
            item.size = meta.len();
            // Free with the stat already in hand — this is what lets the map
            // answer "show me only what's new".
            item.modified = modified_minutes(&meta);
        }
    }
}

fn stubs(listing: &[Listed]) -> Vec<Node> {
    listing
        .iter()
        .map(|l| {
            if l.is_dir {
                Node::dir(l.name.clone(), l.kind)
            } else {
                Node::file_at(l.name.clone(), l.kind, l.size, l.modified)
            }
        })
        .collect()
}

/// Running totals for the directory a thread is currently chewing on,
/// reported at a bounded rate so its rectangle grows on screen without the
/// report channel becoming the bottleneck.
struct Growth<'a> {
    sink: &'a (dyn Fn(ScanStep) + Sync),
    /// How many folders the pool still owes an answer for. Read rather than
    /// computed, so a thread that has been inside one enormous directory for
    /// a minute still reports a number that moves — a counter that freezes
    /// looks exactly like a scan that has died.
    open: &'a AtomicU32,
    at: Vec<u32>,
    size: u64,
    files: u32,
    due: f64,
    pace_due: f64,
}

impl<'a> Growth<'a> {
    fn new(sink: &'a (dyn Fn(ScanStep) + Sync), open: &'a AtomicU32) -> Growth<'a> {
        Growth {
            sink,
            open,
            at: Vec::new(),
            size: 0,
            files: 0,
            due: Cx::monotonic_now() + GROW_EVERY.as_secs_f64(),
            pace_due: Cx::monotonic_now(),
        }
    }

    /// Point the running total at a different node and start it over.
    fn start(&mut self, at: &[u32]) {
        self.at.clear();
        self.at.extend_from_slice(at);
        self.size = 0;
        self.files = 0;
    }

    /// Report how much of the tree is still unopened, at the same bounded
    /// rate as everything else — a folder finishes thousands of times a
    /// second in a build tree and the number on screen does not need to.
    fn pace(&mut self) {
        let folders_left = self.open.load(Ordering::Relaxed);
        let now = Cx::monotonic_now();
        if now >= self.pace_due || folders_left == 0 {
            self.pace_due = now + GROW_EVERY.as_secs_f64();
            (self.sink)(ScanStep::Pace { folders_left });
        }
    }

    fn add(&mut self, size: u64) {
        self.size += size;
        self.files += 1;
        let now = Cx::monotonic_now();
        if now < self.due {
            return;
        }
        self.due = now + GROW_EVERY.as_secs_f64();
        // The queue depth rides along on the same clock, so it keeps moving
        // even while this thread is stuck inside one huge directory.
        self.pace();
        if self.at.is_empty() {
            return;
        }
        (self.sink)(ScanStep::Growing {
            at: self.at.clone(),
            size: self.size,
            files: self.files,
        });
    }
}

/// One directory the walk still owes an answer for.
struct Job {
    path: PathBuf,
    at: Vec<u32>,
}

/// The walk's shared state: folders waiting to be read, and how many threads
/// are inside one right now. A thread that finds the stack empty *and* nobody
/// working knows the walk is over — that is the only termination condition,
/// and it is why the two live under the same lock.
struct Queue {
    jobs: Vec<Job>,
    working: usize,
}

/// A directory with at least this many entries has its metadata read by
/// several threads at once. Below it the coordination costs more than the
/// wait it saves.
const STAT_PARALLEL_MIN: usize = 1024;
/// Threads one big directory's metadata read is split across. Small on
/// purpose: several folders can be doing this at once, and past a handful of
/// outstanding requests a disk stops going any faster.
const STAT_THREADS: usize = 4;

/// Threads the walk uses.
///
/// A single thread is not the answer: walking a tree is latency-bound on
/// every filesystem worth the name. Neither is a thread per top-level folder,
/// which is what this used to be — a home directory is one enormous `Library`
/// and twenty small things, so within a second the "parallel" scan is one
/// thread doing all of the work. Every folder is a work item and any idle
/// thread takes the next one, so the threads stay busy right down to the
/// last directory of the deepest build tree.
const SCAN_THREADS: usize = 6;

/// Walk `root`, streaming the tree back through `sink` as it is discovered.
///
/// The root's own listing goes out first, so a caller has a drawable map
/// within one `read_dir`. Every folder — at any depth — then becomes a work
/// item: it announces its own listing and hands its subfolders back to the
/// pool. There is deliberately no depth cutoff: an earlier design walked deep
/// subtrees whole and delivered them in one piece, and on a disk whose bytes
/// sit in one enormous subtree that meant the map showed a single opaque
/// growing block for minutes and then everything at once.
///
/// Returns false when the walk was cancelled, so a caller never paints a
/// half-built tree as if it were the finished picture.
pub fn scan_stream(
    root: &Path,
    rules: &ScanRules,
    cancel: &AtomicBool,
    sink: &(dyn Fn(ScanStep) + Sync),
    pool: &TaskPool,
) -> bool {
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    let device = device_of(root);
    let queue = Mutex::new(Queue {
        jobs: vec![Job {
            path: root.to_path_buf(),
            at: Vec::new(),
        }],
        working: 0,
    });
    let wake = Condvar::new();
    let open = AtomicU32::new(1);
    // The walk's own worker loop, run on the pool AND the calling thread —
    // the caller-helping `fan_out` replacement for the `std::thread::scope`
    // this used to be. `scan_stream` only ever runs as a Heavy pool job
    // itself (see `treemap_view`), never on the UI thread.
    pool.fan_out(Lane::Heavy, SCAN_THREADS, |_index| {
        let mut growth = Growth::new(sink, &open);
        while let Some(job) = take(&queue, &wake, cancel) {
            let children = run_job(job, rules, device, cancel, sink, &mut growth, pool);
            finish(&queue, &wake, children, &open);
            growth.pace();
        }
    });
    !cancel.load(Ordering::Relaxed)
}

/// The next folder to read, or `None` when the walk is over — the stack is
/// empty and no thread is still inside a folder that could refill it.
///
/// Nothing is reported from in here. A sink call under this lock would
/// serialise every worker behind whatever the caller does with a step, and a
/// sink that panicked would leave `working` counted forever and hang the pool.
fn take(queue: &Mutex<Queue>, wake: &Condvar, cancel: &AtomicBool) -> Option<Job> {
    let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if cancel.load(Ordering::Relaxed) {
            wake.notify_all();
            return None;
        }
        if let Some(job) = queue.jobs.pop() {
            queue.working += 1;
            return Some(job);
        }
        if queue.working == 0 {
            // Nobody is left who could push more work, so there will not be
            // any. Every other waiter has to hear that too.
            wake.notify_all();
            return None;
        }
        queue = wake.wait(queue).unwrap_or_else(|e| e.into_inner());
    }
}

/// Hand a folder's subfolders back to the pool and stop counting as busy.
/// Returns how many folders are left to open.
fn finish(queue: &Mutex<Queue>, wake: &Condvar, children: Vec<Job>, open: &AtomicU32) {
    {
        let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.jobs.extend(children);
        queue.working -= 1;
        open.store((queue.jobs.len() + queue.working) as u32, Ordering::Relaxed);
    }
    // Outside the lock: every waiting thread is about to try to take it.
    wake.notify_all();
}

/// Read one folder: announce its listing, hand its subfolders back to the
/// pool as work items of their own.
fn run_job(
    job: Job,
    rules: &ScanRules,
    device: Option<u64>,
    cancel: &AtomicBool,
    sink: &(dyn Fn(ScanStep) + Sync),
    growth: &mut Growth,
    pool: &TaskPool,
) -> Vec<Job> {
    if cancel.load(Ordering::Relaxed) {
        return Vec::new();
    }
    // The running total belongs to this folder while its own listing is being
    // read, which on a folder with a quarter of a million files is most of
    // the time this job takes.
    growth.start(&job.at);
    let listing = read_listing(&job.path, rules, device, growth, Some(pool));
    growth.start(&[]);
    sink(ScanStep::Opened {
        at: job.at.clone(),
        children: stubs(&listing.entries),
        denied: listing.denied,
    });
    listing
        .entries
        .into_iter()
        .enumerate()
        .filter(|(_, entry)| entry.is_dir)
        .map(|(index, entry)| {
            let mut at = job.at.clone();
            at.push(index as u32);
            Job {
                path: entry.path,
                at,
            }
        })
        .collect()
}

/// Walk `root` recursively and hand back the whole tree at once. The simple
/// blocking form, for callers that only want a total — the map itself uses
/// [`scan_stream`].
pub fn scan(
    root: &Path,
    rules: &ScanRules,
    cancel: &AtomicBool,
    progress: &dyn Fn(ScanProgress),
) -> Option<Node> {
    let mut total = ScanProgress::default();
    let mut since = 0u64;
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let kind = (rules.classify)(root, true);
    let device = device_of(root);
    let node = scan_blocking(
        root,
        name,
        kind,
        rules,
        device,
        cancel,
        progress,
        &mut total,
        &mut since,
    )?;
    // One last report so a caller that only reads the callback's argument
    // after the walk returns still sees the true final tally, rather than
    // whatever the last stride boundary happened to leave behind.
    progress(total);
    Some(node)
}

#[allow(clippy::too_many_arguments)]
fn scan_blocking(
    dir: &Path,
    name: String,
    kind: u8,
    rules: &ScanRules,
    device: Option<u64>,
    cancel: &AtomicBool,
    progress: &dyn Fn(ScanProgress),
    total: &mut ScanProgress,
    since: &mut u64,
) -> Option<Node> {
    // Checked on every directory, so a cancelled scan stops within a fraction
    // of a second rather than at the end of the walk.
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let idle = AtomicU32::new(0);
    // The simple blocking form has no pool of its own to fan a big
    // directory's `lstat`s out on; it stats serially.
    let listing = read_listing(dir, rules, device, &mut Growth::new(&|_| {}, &idle), None);
    let denied = listing.denied;
    let mut children = Vec::with_capacity(listing.entries.len());
    for entry in listing.entries {
        if entry.is_dir {
            children.push(scan_blocking(
                &entry.path,
                entry.name,
                entry.kind,
                rules,
                device,
                cancel,
                progress,
                total,
                since,
            )?);
        } else {
            total.files += 1;
            total.bytes += entry.size;
            *since += 1;
            if *since >= PROGRESS_STRIDE {
                *since = 0;
                progress(*total);
            }
            children.push(Node::file_at(entry.name, entry.kind, entry.size, entry.modified));
        }
    }
    Some(Node {
        size: children.iter().map(|c| c.size).sum(),
        files: children.iter().map(|c| c.files).sum(),
        modified: children.iter().map(|c| c.modified).max().unwrap_or(0),
        kind: heaviest_kind(&children).unwrap_or(kind),
        name,
        is_dir: true,
        done: true,
        denied,
        children,
    })
}

// ---------------------------------------------------------------- geometry

/// The squarified treemap layout of `sizes` inside `rect`, one output rect
/// per input, in the same order as `sizes`. Bruls, Huizing & van Wijk
/// (2000): lay children out in rows along the rect's shorter side, closing a
/// row the moment adding the next child would make the row's worst aspect
/// ratio worse rather than better. That local greedy rule is what keeps
/// treemap cells close to square instead of degenerating into the thin
/// slivers a naive slice-and-dice layout produces.
#[allow(dead_code)] // production packs in canon space; the tests keep this as the oracle
pub fn squarify(sizes: &[u64], rect: Rect) -> Vec<Rect> {
    let n = sizes.len();
    let mut out = vec![
        Rect {
            x: rect.x,
            y: rect.y,
            w: 0.0,
            h: 0.0
        };
        n
    ];
    if n == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }
    let total: f64 = sizes.iter().map(|&s| s as f64).sum();
    if total <= 0.0 {
        // Every input is zero: every output stays the zero-area rect `out`
        // was already filled with, and there is nothing to lay out.
        return out;
    }
    // The layout math below divides by a row's own thickness, which is
    // zero for a zero-size item. Rather than guard every division, the
    // zero entries are filtered out up front and left as the zero rects
    // `out` already holds; only the strictly positive sizes go through the
    // real algorithm, sorted descending as it requires.
    let mut order: Vec<usize> = (0..n).filter(|&i| sizes[i] > 0).collect();
    // Size descending, input index as the tiebreak: equal sizes are
    // everywhere on a real disk (shards, dedup'd assets), and a sort that
    // may swap them between two layouts of the same data is a map that
    // shuffles its tiles every time the camera settles.
    order.sort_unstable_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    // Scale byte counts to areas that sum exactly to the container's area —
    // this is what makes every output rect's area proportional to its size.
    let scale = rect.area() / total;
    let scaled: Vec<f64> = order.iter().map(|&i| sizes[i] as f64 * scale).collect();
    let placed = squarify_rows(&scaled, rect);
    for (slot, &original) in order.iter().enumerate() {
        out[original] = placed[slot];
    }
    out
}

/// The core algorithm on pre-scaled, strictly positive, descending-sorted
/// areas. Returns one rect per input, in input order.
#[allow(dead_code)] // production packs in canon space; the tests keep this as the oracle
fn squarify_rows(areas: &[f64], mut rect: Rect) -> Vec<Rect> {
    let mut out = Vec::with_capacity(areas.len());
    let mut start = 0;
    while start < areas.len() {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            // Floating-point drift can shave the leftover rect down to
            // nothing a touch early; whatever is left just becomes
            // zero-area rects instead of a division by zero.
            for _ in start..areas.len() {
                out.push(Rect {
                    x: rect.x,
                    y: rect.y,
                    w: 0.0,
                    h: 0.0,
                });
            }
            break;
        }
        // Grow the row one item at a time for as long as doing so does not
        // make its worst aspect ratio worse — the "squarified" rule.
        let mut end = start + 1;
        let mut current = worst_ratio(&areas[start..end], rect);
        while end < areas.len() {
            let grown = worst_ratio(&areas[start..end + 1], rect);
            if grown <= current {
                current = grown;
                end += 1;
            } else {
                break;
            }
        }
        let row = &areas[start..end];
        out.extend(lay_out_row(row, rect));
        rect = leftover(row, rect);
        start = end;
    }
    out
}

/// One row's rects: a strip spanning the rect's shorter side, subdivided
/// among `areas` in proportion to their size, with the strip's thickness
/// along the longer side set so the strip's total area equals `sum(areas)`.
fn lay_out_row(areas: &[f64], rect: Rect) -> Vec<Rect> {
    if rect.w >= rect.h {
        lay_out_row_stacked(areas, rect)
    } else {
        lay_out_row_flowed(areas, rect)
    }
}

/// The rect is at least as wide as it is tall, so the row becomes a
/// vertical strip at the left edge, itself subdivided top to bottom.
fn lay_out_row_stacked(areas: &[f64], rect: Rect) -> Vec<Rect> {
    let covered: f64 = areas.iter().sum();
    let width = if rect.h > 0.0 { covered / rect.h } else { 0.0 };
    let mut y = rect.y;
    let mut out = Vec::with_capacity(areas.len());
    for &a in areas {
        let h = if width > 0.0 { a / width } else { 0.0 };
        out.push(Rect { x: rect.x, y, w: width, h });
        y += h;
    }
    out
}

/// The rect is taller than it is wide, so the row becomes a horizontal
/// strip at the top edge, itself subdivided left to right.
fn lay_out_row_flowed(areas: &[f64], rect: Rect) -> Vec<Rect> {
    let covered: f64 = areas.iter().sum();
    let height = if rect.w > 0.0 { covered / rect.w } else { 0.0 };
    let mut x = rect.x;
    let mut out = Vec::with_capacity(areas.len());
    for &a in areas {
        let w = if height > 0.0 { a / height } else { 0.0 };
        out.push(Rect { x, y: rect.y, w, h: height });
        x += w;
    }
    out
}

/// What remains of `rect` after placing a row for `areas`: the same rect
/// with the row's strip removed from whichever side it occupied. Clamped to
/// zero rather than left to go slightly negative under floating-point
/// rounding, so the next iteration's "is this rect degenerate" check is
/// exact instead of an epsilon comparison.
fn leftover(areas: &[f64], rect: Rect) -> Rect {
    let covered: f64 = areas.iter().sum();
    if rect.w >= rect.h {
        let width = if rect.h > 0.0 { covered / rect.h } else { 0.0 };
        Rect {
            x: rect.x + width,
            y: rect.y,
            w: (rect.w - width).max(0.0),
            h: rect.h,
        }
    } else {
        let height = if rect.w > 0.0 { covered / rect.w } else { 0.0 };
        Rect {
            x: rect.x,
            y: rect.y + height,
            w: rect.w,
            h: (rect.h - height).max(0.0),
        }
    }
}

/// The worst (largest) aspect ratio among the rects a row of `areas` would
/// produce in `rect` — the number [`squarify_rows`] compares before and
/// after adding one more item, to decide whether the row should keep
/// growing or close.
fn worst_ratio(areas: &[f64], rect: Rect) -> f64 {
    lay_out_row(areas, rect)
        .iter()
        .map(|r| {
            if r.w <= 0.0 || r.h <= 0.0 {
                f64::INFINITY
            } else {
                (r.w / r.h).max(r.h / r.w)
            }
        })
        .fold(0.0_f64, f64::max)
}

// ------------------------------------------------------------------ filter

/// What the filter box means. Every field is ANDed; a file matches when it
/// passes all of them, and a folder's filtered size is the sum of its
/// matching files — so under ".mov" the map is literally "where do my movie
/// bytes live", and folders holding none of them vanish.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Lowercase substrings. A term is satisfied by the file's own name or
    /// by any folder on the way down to it — "cache" means everything under
    /// a cache folder, which is what a person pointing at a treemap means.
    pub names: Vec<String>,
    /// Lowercase extensions, no dot. Any of these (ORed) when non-empty.
    pub exts: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Only files at least this new: minutes since the epoch.
    pub newer_than: Option<u32>,
    /// Only files at least this old — `>1y` finds the forgotten stuff.
    pub older_than: Option<u32>,
    /// Allowed kind tags as a bitmask over [`Node::kind`]; None = all kinds.
    pub kinds: Option<u16>,
}

impl Query {
    /// True when this query filters nothing — the map is the whole disk.
    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }

    /// Parse the typed form: whitespace-separated terms, ANDed.
    /// `>100mb` / `<2gb` / `>=1kb` bound file sizes; `<7d` / `<24h` / `<2w` /
    /// `<3mo` / `<1y` mean "modified within"; `.mov` / `*.mov` match an
    /// extension; anything else is a name substring. `now_min` is the current
    /// time in minutes since the epoch, for the age terms.
    pub fn parse(text: &str, now_min: u32) -> Query {
        let mut query = Query::default();
        for raw in text.split_whitespace() {
            let term = raw.to_lowercase();
            if let Some(rest) = term.strip_prefix("*.") {
                if !rest.is_empty() {
                    query.exts.push(rest.to_string());
                }
            } else if let Some(rest) = term.strip_prefix('.') {
                if !rest.is_empty() && rest.chars().all(|c| c.is_alphanumeric()) {
                    query.exts.push(rest.to_string());
                }
            } else if let Some(bound) = term
                .strip_prefix(">=")
                .or_else(|| term.strip_prefix('>'))
            {
                if let Some(minutes) = parse_age(bound) {
                    // ">7d" reads as "older than a week" — untouched since.
                    query.older_than = Some(now_min.saturating_sub(minutes));
                } else if let Some(bytes) = parse_size(bound) {
                    query.min_size = Some(bytes);
                } else {
                    query.names.push(term);
                }
            } else if let Some(bound) = term
                .strip_prefix("<=")
                .or_else(|| term.strip_prefix('<'))
            {
                if let Some(minutes) = parse_age(bound) {
                    query.newer_than = Some(now_min.saturating_sub(minutes));
                } else if let Some(bytes) = parse_size(bound) {
                    query.max_size = Some(bytes);
                } else {
                    query.names.push(term);
                }
            } else {
                query.names.push(term);
            }
        }
        query
    }

    /// The bitmask of name terms `name` satisfies.
    pub fn name_hits(&self, name: &str) -> u32 {
        let mut hits = 0u32;
        if self.names.is_empty() {
            return 0;
        }
        let lower = name.to_lowercase();
        for (index, term) in self.names.iter().enumerate().take(32) {
            if lower.contains(term.as_str()) {
                hits |= 1 << index;
            }
        }
        hits
    }

    /// The mask that means "every name term satisfied".
    fn all_names(&self) -> u32 {
        if self.names.is_empty() {
            0
        } else {
            (1u32 << self.names.len().min(32)) - 1
        }
    }

    /// Whether one file passes, given the name terms its folders already
    /// satisfied on the way down.
    fn file_matches(&self, node: &Node, inherited: u32) -> bool {
        if let Some(min) = self.min_size {
            if node.size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if node.size > max {
                return false;
            }
        }
        if let Some(cutoff) = self.newer_than {
            if node.modified < cutoff {
                return false;
            }
        }
        if let Some(cutoff) = self.older_than {
            if node.modified > cutoff {
                return false;
            }
        }
        if let Some(kinds) = self.kinds {
            if kinds & (1u16 << (node.kind as u32).min(15)) == 0 {
                return false;
            }
        }
        if !self.exts.is_empty() {
            let lower = node.name.to_lowercase();
            if !self
                .exts
                .iter()
                .any(|ext| lower.len() > ext.len() && lower.ends_with(ext.as_str())
                    && lower.as_bytes()[lower.len() - ext.len() - 1] == b'.')
            {
                return false;
            }
        }
        (inherited | self.name_hits(&node.name)) == self.all_names()
    }
}

/// "100mb" -> bytes. 1000-based, like every number this app prints.
fn parse_size(text: &str) -> Option<u64> {
    let unit_at = text.find(|c: char| c.is_alphabetic())?;
    let value: f64 = text[..unit_at].parse().ok()?;
    let scale: u64 = match &text[unit_at..] {
        "b" => 1,
        "k" | "kb" => 1_000,
        "m" | "mb" => 1_000_000,
        "g" | "gb" => 1_000_000_000,
        "t" | "tb" => 1_000_000_000_000,
        _ => return None,
    };
    (value >= 0.0).then(|| (value * scale as f64) as u64)
}

/// "7d" -> minutes. Hours, days, weeks, months, years.
fn parse_age(text: &str) -> Option<u32> {
    let unit_at = text.find(|c: char| c.is_alphabetic())?;
    let value: f64 = text[..unit_at].parse().ok()?;
    let scale: u32 = match &text[unit_at..] {
        "h" => 60,
        "d" => 60 * 24,
        "w" => 60 * 24 * 7,
        "mo" => 60 * 24 * 30,
        "y" => 60 * 24 * 365,
        _ => return None,
    };
    (value >= 0.0).then(|| (value * scale as f64) as u32)
}

/// The bytes and files under `node` that pass `query`, with `inherited` name
/// terms already satisfied by the folders above. One prune-walk — this is
/// what a keystroke in the filter box costs.
#[allow(dead_code)] // production packs in canon space; the tests keep this as the oracle
pub fn filtered_size(node: &Node, query: &Query, inherited: u32) -> (u64, u32) {
    if !node.is_dir {
        return if query.file_matches(node, inherited) {
            (node.size, 1)
        } else {
            (0, 0)
        };
    }
    let inherited = inherited | query.name_hits(&node.name);
    let mut bytes = 0u64;
    let mut files = 0u32;
    for child in &node.children {
        let (b, f) = filtered_size(child, query, inherited);
        bytes += b;
        files += f;
    }
    (bytes, files)
}

/// The filtered weight of every node, mirrored in the tree's own shape:
/// `children` aligns index-for-index with the node's children. Measured once
/// per (tree, query) and read by every relayout after that — the camera
/// moves every frame, the answer to "which bytes match" does not. Without
/// this, a filtered layout re-walked whole subtrees at every nesting level
/// of every frame of an orbit, and the frame rate showed it.
pub struct Measure {
    pub bytes: u64,
    pub files: u32,
    pub children: Vec<Measure>,
}

/// One O(n) walk answering `query` for every node at once. `inherited` is
/// the name-term mask the folders above `node` already satisfied.
pub fn measure(node: &Node, query: &Query, inherited: u32) -> Measure {
    if !node.is_dir {
        return if query.file_matches(node, inherited) {
            Measure { bytes: node.size, files: 1, children: Vec::new() }
        } else {
            Measure { bytes: 0, files: 0, children: Vec::new() }
        };
    }
    let inherited = inherited | query.name_hits(&node.name);
    let children: Vec<Measure> = node
        .children
        .iter()
        .map(|child| measure(child, query, inherited))
        .collect();
    Measure {
        bytes: children.iter().map(|m| m.bytes).sum(),
        files: children.iter().map(|m| m.files).sum(),
        children,
    }
}

/// Bytes per kind tag under `node` — the legend's numbers. One walk.
pub fn kind_totals(node: &Node) -> [u64; 16] {
    let mut totals = [0u64; 16];
    fn add(node: &Node, totals: &mut [u64; 16]) {
        if node.is_dir {
            for child in &node.children {
                add(child, totals);
            }
        } else {
            totals[(node.kind as usize).min(15)] += node.size;
        }
    }
    add(node, &mut totals);
    totals
}

// ------------------------------------------------------------------ layout

/// The pixel sizes that decide how far down the map goes. Every one of them
/// is a statement about what a person can see, which is why the layout has no
/// depth limit at all: it stops where the picture stops saying anything, and
/// on a big enough screen that is at the individual file.
#[derive(Clone, Copy, Debug)]
pub struct MapStyle {
    /// A rectangle thinner than this on either edge is not drawn.
    pub min_side: f64,
    /// Where refinement stops: once a row's lead rectangle would come out
    /// smaller than this, that row and everything after it are drawn as one
    /// "N smaller items" plate over the region their rows would occupy.
    /// Deliberately NOT an input to the packing itself — the arrangement is
    /// fixed by the weights and the rect's aspect alone, so a zoom can only
    /// refine the plate in place, never re-shuffle what was already visible.
    /// This is also what bounds the cost of a folder to its pixels rather
    /// than to how many files it holds.
    pub min_area: f64,
    /// The border a folder insets its children by, as a *fraction of the
    /// packing area's short side* — never a point size. A point-sized inset
    /// made the children's share of their frame depend on the zoom, so the
    /// whole map "breathed" as it scaled; a fractional one rides the zoom
    /// exactly, which is what makes the geometry a pure function of map
    /// space. Narrowing with depth falls out on its own: each level's inset
    /// is a fraction of an area that is itself smaller.
    pub inset: f64,
    /// How tall a group's floating name is drawn, in points. Purely a draw
    /// hint — a name never reserves layout room, because a strip that
    /// appears at some zoom is a strip that shoves children at that zoom.
    pub header: f64,
    /// A folder needs to be at least this wide and tall on screen before its
    /// floating name is worth drawing; below it, the name would cost more
    /// than it tells. Gates drawing only — never geometry.
    pub header_min: (f64, f64),
    /// A folder whose inside comes out smaller than this on either edge is
    /// drawn as one plate instead of being opened up — nesting borders
    /// thinner than this are all border and no bytes.
    pub group_min: f64,
    /// A hard ceiling on rectangles, so a pathological tree cannot make one
    /// frame take a second.
    pub max_cells: usize,
}

impl Default for MapStyle {
    fn default() -> Self {
        MapStyle {
            min_side: 2.0,
            min_area: 9.0,
            // ~3pt at the top level of a default-height window (≈735pt),
            // matching the old point-sized look at zoom 1 exactly where it
            // was calibrated, then scaling with whatever it frames.
            inset: 0.004,
            header: 12.0,
            header_min: (58.0, 34.0),
            group_min: 6.0,
            max_cells: 60_000,
        }
    }
}

/// One drawable rectangle of the finished map — a folder or a file, already
/// positioned, with nothing left for a view to compute except paint it.
#[derive(Clone, Debug)]
pub struct Cell {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub files: u32,
    pub is_dir: bool,
    pub kind: u8,
    /// 0 for the mapped folder's own children, 1 for their children, and so
    /// on — how many group borders separate this cell from the root.
    pub depth: usize,
    pub rect: Rect,
    /// True when this cell is a folder drawn as a bordered group whose
    /// children are also in the output; false for a file, and false for a
    /// folder too small to open up.
    pub is_group: bool,
    /// The header strip this group earned, in points; 0 when it earned none.
    pub header: f64,
    /// True while the scan is still filling this subtree in.
    pub pending: bool,
    /// When non-zero this cell stands for that many sibling entries at once,
    /// each too small to draw on its own.
    pub extra: u32,
}

impl Cell {
    /// Whether this cell is the "N smaller items" aggregate rather than one
    /// real file or folder.
    pub fn is_bundle(&self) -> bool {
        self.extra > 0
    }
}

/// Flatten `node`'s children into drawable cells inside `area`.
///
/// `root_path` is the folder `node` stands for; every cell's path is built
/// from it and the names on the way down, which is why the tree itself does
/// not carry paths.
///
/// `viewport` is the part of `area` actually on screen. When a camera has
/// zoomed the map, `area` is the whole map at its blown-up size and the
/// viewport is the window into it: everything outside is skipped entirely —
/// no cell, no recursion — which is what keeps a deep zoom costing what the
/// pixels on screen cost rather than what the whole magnified map would.
/// With no camera the two are the same rect.
///
/// The output is in painter's order: a group's own cell always comes before
/// its children, so a caller drawing the vector front to back gets children
/// on top of their group for free, with no separate z-ordering step.
pub fn layout(
    node: &Node,
    root_path: &Path,
    area: Rect,
    viewport: Rect,
    style: &MapStyle,
    filter: Option<&Measure>,
) -> Vec<Cell> {
    let mut out = Vec::new();
    let mut path = root_path.to_path_buf();
    // A stale measure — one made of a different tree — must never index out
    // of step with the children; the caller keys its cache on the tree
    // revision, and this is the belt to that suspender.
    let filter = filter.filter(|m| m.children.len() == node.children.len());
    // At the root the canonical packing space IS the area: the body's aspect
    // is the same at every zoom, so invariance starts true and the recursion
    // keeps it true (see `layout_children` on what canon is for).
    layout_children(&node.children, &mut path, area, area, viewport, 0, style, filter, &mut out);
    out
}

/// `canon` is the packing space: a rect with the same area as `area` but a
/// *canonical* aspect, derived purely from the map's own zoom-invariant
/// proportions — never from the point-sized insets and header strips that
/// make the realized `area`'s aspect wobble a few percent as the zoom
/// changes. All row-membership decisions run in canon space and the finished
/// geometry is mapped affinely onto `area`, so the arrangement literally
/// cannot drift with zoom: the packer never sees a zoom-dependent number.
/// The price is rows optimized for an aspect a few percent off the realized
/// one — a squareness error far below what the eye notices, where a row
/// re-break is exactly what the eye is drawn to.
#[allow(clippy::too_many_arguments)]
fn layout_children(
    children: &[Node],
    path: &mut PathBuf,
    area: Rect,
    canon: Rect,
    viewport: Rect,
    depth: usize,
    style: &MapStyle,
    filter: Option<&Measure>,
    out: &mut Vec<Cell>,
) {
    if children.is_empty() || area.w <= 0.0 || area.h <= 0.0 || out.len() >= style.max_cells {
        return;
    }
    if canon.w <= 0.0 || canon.h <= 0.0 {
        return;
    }
    if !area.intersects(&viewport) {
        return;
    }
    // Under a filter every child weighs only its matching bytes — the whole
    // map re-proportions to the question being asked. The weights were all
    // measured in one walk up front (see [`measure`]); reading them here is
    // an index, not a subtree walk. The unfiltered path costs nothing extra.
    let measured = filter.map(|m| &m.children);
    let weight = |index: usize| match measured {
        Some(list) => list[index].bytes,
        None => children[index].size,
    };
    let files_of = |index: usize| match measured {
        Some(list) => list[index].files,
        None => children[index].files,
    };
    let total: f64 = (0..children.len()).map(|i| weight(i) as f64).sum();
    if total <= 0.0 {
        return;
    }
    // The packing is of ALL the children, always. An earlier design fed only
    // the children big enough to see into the packer and swept the rest into
    // a synthetic "smaller items" entry — which made the packer's INPUT
    // depend on the zoom, so crossing any zoom step re-packed the whole
    // group and the map visibly reshuffled. Now the arrangement is fixed by
    // the weights and the rect's aspect alone — both zoom-invariant — and
    // the zoom only decides how far down the row list refinement runs.
    //
    // Cost stays bounded by the pixels, not the child count, because the
    // rows come out biggest-first: everything too small to see is a suffix
    // of the sorted order, so only the items that could reach a visible row
    // need sorting at all. `sort_floor` keeps a 64× margin below the
    // visibility cutoff so the last visible row closes on exactly the
    // neighbours the full sort would have offered it — an item 64× smaller
    // than a row-mate makes the aspect test slam the row shut long before,
    // so nothing below the margin can ever influence visible geometry.
    // The canon packing space, re-anchored at the origin and normalized to
    // the realized rect's exact area: row math happens here, so membership
    // depends only on the canonical aspect and the weights; the stop
    // thresholds stay honest because canon areas equal screen areas.
    let canon = {
        let aspect = canon.w / canon.h;
        Rect {
            x: 0.0,
            y: 0.0,
            w: (area.area() * aspect).sqrt(),
            h: (area.area() / aspect).sqrt(),
        }
    };
    let realize = |r: &Rect| Rect {
        x: area.x + r.x / canon.w * area.w,
        y: area.y + r.y / canon.h * area.h,
        w: r.w / canon.w * area.w,
        h: r.h / canon.h * area.h,
    };
    let scale = area.area() / total; // square points per byte
    let tail_floor = (style.min_area / scale).max(1.0);
    let sort_floor = (tail_floor / 64.0).max(1.0) as u64;
    let mut order: Vec<usize> = Vec::new();
    let mut rest_size: u64 = 0;
    let mut rest_count: u32 = 0;
    let mut rest_files: u32 = 0;
    let mut max_weight: u64 = 0;
    for i in 0..children.len() {
        let w = weight(i);
        max_weight = max_weight.max(w);
        if w >= sort_floor {
            order.push(i);
        } else if w > 0 {
            rest_size += w;
            rest_count += 1;
            rest_files += files_of(i);
        }
    }
    // When even the biggest child is below the visibility floor the whole
    // group is one tail plate — no order, no sort, no rows.
    if (max_weight as f64) * scale < style.min_area {
        let count = rest_count as usize + order.len();
        if count > 0
            && area.w >= style.min_side
            && area.h >= style.min_side
            && out.len() < style.max_cells
        {
            out.push(Cell {
                path: path.clone(),
                name: format!("{count} smaller item{}", if count == 1 { "" } else { "s" }),
                size: rest_size + order.iter().map(|&i| weight(i)).sum::<u64>(),
                files: rest_files + order.iter().map(|&i| files_of(i)).sum::<u32>(),
                is_dir: false,
                kind: u8::MAX,
                depth,
                rect: area,
                is_group: false,
                header: 0.0,
                pending: false,
                extra: count as u32,
            });
        }
        return;
    }
    // The sort is cut at what the pixels can hold before sorting: an item
    // ranked past `area / min_area` has, by descending order, less than
    // `min_area` to its name, so it lives in the tail plate and only its
    // sum matters — its exact position in the order buys nothing. This is
    // what keeps a quarter-million-file folder costing a selection pass, not
    // a quarter-million-element sort, at every distance.
    let cap = ((area.area() / style.min_area) as usize + 64).min(style.max_cells + 64);
    if order.len() > cap {
        order.select_nth_unstable_by(cap, |&a, &b| {
            weight(b).cmp(&weight(a)).then(a.cmp(&b))
        });
        for &i in &order[cap..] {
            rest_size += weight(i);
            rest_count += 1;
            rest_files += files_of(i);
        }
        order.truncate(cap);
    }
    // Descending, deterministic under ties (child index breaks them), so the
    // same children lay out the same way every single time.
    order.sort_unstable_by(|&a, &b| weight(b).cmp(&weight(a)).then(a.cmp(&b)));
    let scaled: Vec<f64> = order.iter().map(|&i| weight(i) as f64 * scale).collect();

    // Stream the squarified rows biggest-first, exactly as the full packing
    // would place them, and stop refining at the first row whose lead item
    // is too small to see. Everything from there on — plus whatever never
    // made the sort — is drawn as one aggregate plate over the leftover
    // rect, which is precisely the region those rows would occupy: zooming
    // in only ever subdivides that plate in place, and nothing that was
    // already on screen can move, because nothing about its inputs changed.
    let mut leftover_canon = canon;
    let mut start = 0usize;
    while start < scaled.len() {
        if leftover_canon.w <= 0.0 || leftover_canon.h <= 0.0 {
            break;
        }
        if scaled[start] < style.min_area {
            break;
        }
        if out.len() >= style.max_cells {
            return;
        }
        if !realize(&leftover_canon).intersects(&viewport) {
            // Everything still unplaced lives inside the leftover, and the
            // leftover only ever shrinks toward one corner: once it has left
            // the window, so has every remaining row and the tail plate.
            return;
        }
        // Grow the row while doing so does not worsen its worst aspect —
        // the squarified rule, unchanged, in canon space.
        let mut end = start + 1;
        let mut current = worst_ratio(&scaled[start..end], leftover_canon);
        while end < scaled.len() {
            let grown = worst_ratio(&scaled[start..end + 1], leftover_canon);
            if grown <= current {
                current = grown;
                end += 1;
            } else {
                break;
            }
        }
        // The strip this row occupies. A row whose strip misses the window
        // still consumes its area — the leftover chain is the geometry — but
        // its items need no rects, no cells and no recursion.
        let next_leftover = leftover(&scaled[start..end], leftover_canon);
        let strip = if leftover_canon.w >= leftover_canon.h {
            Rect {
                x: leftover_canon.x,
                y: leftover_canon.y,
                w: leftover_canon.w - next_leftover.w,
                h: leftover_canon.h,
            }
        } else {
            Rect {
                x: leftover_canon.x,
                y: leftover_canon.y,
                w: leftover_canon.w,
                h: leftover_canon.h - next_leftover.h,
            }
        };
        if !realize(&strip).intersects(&viewport) {
            leftover_canon = next_leftover;
            start = end;
            continue;
        }
        let row_rects = lay_out_row(&scaled[start..end], leftover_canon);
        for (slot, canon_rect) in row_rects.iter().enumerate() {
            let rect = &realize(canon_rect);
            if out.len() >= style.max_cells {
                return;
            }
            if rect.w < style.min_side || rect.h < style.min_side {
                // Invisible at this scale: drawing it would just be a
                // sliver, and if it is a folder its children are smaller
                // still. Skipping it moves nothing — the geometry of every
                // neighbour was fixed before this test ran.
                continue;
            }
            if !rect.intersects(&viewport) {
                // Off the edge of the window the camera is looking through,
                // and so is everything inside it.
                continue;
            }
            let index = order[start + slot];
            let child = &children[index];
            // The frame around a group's children is a fraction of THIS
            // level's packing area, so every sibling wears the same border
            // and the border scales exactly with the zoom: the children's
            // share of their frame is the same at every magnification, which
            // is the last thing that used to make the map breathe. Names
            // reserve nothing — a group's name floats over its children at
            // draw time (`header` below is only the hint that it earned one).
            let inner = rect.shrink(style.inset * area.w.min(area.h), 0.0);
            let header = if child.is_dir
                && rect.w >= style.header_min.0
                && rect.h >= style.header_min.1
            {
                style.header
            } else {
                0.0
            };
            let is_group = child.is_dir
                && !child.children.is_empty()
                && inner.w >= style.group_min
                && inner.h >= style.group_min;
            path.push(&child.name);
            out.push(Cell {
                path: path.clone(),
                name: child.name.clone(),
                size: weight(index),
                files: files_of(index),
                is_dir: child.is_dir,
                kind: child.kind,
                depth,
                rect: *rect,
                is_group,
                header: if is_group { header } else { 0.0 },
                pending: child.is_dir && !child.done,
                extra: 0,
            });
            if is_group {
                let filter = measured.map(|list| &list[index]);
                // The child packs against its own raw canon rect — its
                // zoom-invariant share of this packing — never against the
                // inset-shrunk realized rect whose aspect wobbles with zoom.
                layout_children(
                    &child.children,
                    path,
                    inner,
                    *canon_rect,
                    viewport,
                    depth + 1,
                    style,
                    filter,
                    out,
                );
            }
            path.pop();
        }
        leftover_canon = next_leftover;
        start = end;
    }

    // The tail: every remaining sorted item plus everything below the sort
    // margin, presented as the one "N smaller items" plate over the region
    // their rows will occupy when a deeper zoom refines them into being.
    let mut tail_size = rest_size;
    let mut tail_count = rest_count;
    let mut tail_files = rest_files;
    for &i in &order[start..] {
        tail_size += weight(i);
        tail_count += 1;
        tail_files += files_of(i);
    }
    let tail_rect = realize(&leftover_canon);
    if tail_count > 0
        && tail_size > 0
        && tail_rect.w >= style.min_side
        && tail_rect.h >= style.min_side
        && tail_rect.intersects(&viewport)
        && out.len() < style.max_cells
    {
        out.push(Cell {
            path: path.clone(),
            name: format!(
                "{} smaller item{}",
                tail_count,
                if tail_count == 1 { "" } else { "s" }
            ),
            size: tail_size,
            files: tail_files,
            is_dir: false,
            kind: u8::MAX,
            depth,
            rect: tail_rect,
            is_group: false,
            header: 0.0,
            pending: false,
            extra: tail_count,
        });
    }
}

/// The deepest (i.e. last in painter's order) cell containing the point —
/// the one a mouse at that point is actually pointing at, not whichever
/// group happens to sit behind it.
pub fn hit(cells: &[Cell], x: f64, y: f64) -> Option<usize> {
    cells.iter().rposition(|c| c.rect.contains(x, y))
}

/// A byte count the way a tooltip reads it — "1.5 KB", "15 KB", "2.5 GB" —
/// using exactly the app's own 1000-based rounding (see `format_size` in
/// `model.rs`), because the same number should never look different just
/// for being shown in a different view of the same file.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else if value >= 10.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `scan_stream` fans out on its pool and helps with the work itself, so
    /// it must never run on the thread that owns the pool (`fan_out` asserts
    /// against that). Give it a pool built here and a dedicated thread to
    /// call from, same as production code does with a Heavy pool job.
    fn with_pool<R: Send>(f: impl FnOnce(&TaskPool) -> R + Send) -> R {
        let cx = Cx::new(Box::new(|_, _| {}));
        let pool = cx.task_pool();
        std::thread::scope(|scope| scope.spawn(|| f(&pool)).join().unwrap())
    }

    fn leaf(name: &str, size: u64) -> Node {
        Node::file(name.to_string(), 0, size)
    }

    fn dir(name: &str, children: Vec<Node>) -> Node {
        Node {
            name: name.to_string(),
            is_dir: true,
            done: true,
            denied: false,
            kind: 0,
            size: children.iter().map(|c| c.size).sum(),
            files: children.iter().map(|c| c.files).sum(),
            modified: children.iter().map(|c| c.modified).max().unwrap_or(0),
            children,
        }
    }

    // The zoom-invariance contract, mechanically: pack a rich tree at a
    // ladder of zooms over the same anchored viewport and demand that every
    // cell present at two zooms sits in EXACTLY the same place in map space,
    // at every nesting level, with no exemptions. Insets are fractions of
    // map space and names reserve no room, so there is nothing left that may
    // lawfully move — the only tolerance is floating-point noise. This is
    // the test that fails when any zoom-dependent quantity leaks back into
    // the geometry.
    #[test]
    fn the_arrangement_is_zoom_invariant_by_construction() {
        // Mixed sizes, an equal-size run (the tie-swap trap), nesting, and
        // one folder with ten thousand files (the bundle-floor trap).
        let mut crowd = Vec::new();
        for i in 0..10_000u64 {
            let size = ((i.wrapping_mul(2_654_435_761)) % 997 + 1) * 4096;
            crowd.push(leaf(&format!("c{i}.bin"), size));
        }
        let tree = dir(
            "root",
            vec![
                leaf("huge.mov", 6_000_000_000),
                dir(
                    "nest",
                    vec![
                        dir(
                            "deep",
                            vec![leaf("a.bin", 900_000_000), leaf("b.bin", 400_000_000)],
                        ),
                        leaf("c.bin", 700_000_000),
                    ],
                ),
                dir("crowd", crowd),
                dir(
                    "equal",
                    (0..8).map(|i| leaf(&format!("e{i}.dat"), 50_000_000)).collect(),
                ),
                leaf("mid.tar", 350_000_000),
            ],
        );
        let style = MapStyle::default();
        let viewport = Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 };
        // The camera anchor: map point (0.3, 0.4) pinned to screen (400, 300)
        // at every zoom, the way the view's zoom-at-cursor works.
        let layout_at = |z: f64| {
            let area = Rect {
                x: 400.0 - 0.3 * 1200.0 * z,
                y: 300.0 - 0.4 * 800.0 * z,
                w: 1200.0 * z,
                h: 800.0 * z,
            };
            (layout(&tree, Path::new("/root"), area, viewport, &style, None), area)
        };
        let zooms = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
        let laid: Vec<_> = zooms.iter().map(|&z| layout_at(z)).collect();

        for pair in laid.windows(2) {
            let (cells_a, area_a) = &pair[0];
            let (cells_b, area_b) = &pair[1];
            check_invariant(cells_a, *area_a, cells_b, *area_b);
        }
        // And the far ends against each other: the full 32× throw.
        let (first, area_first) = &laid[0];
        let (last, area_last) = &laid[laid.len() - 1];
        check_invariant(first, *area_first, last, *area_last);
    }

    /// Every real cell present in both layouts must occupy exactly the same
    /// map-space rect — width and height as well as centre — at every depth.
    /// No drift budget, no exemptions: only floating-point noise is forgiven.
    fn check_invariant(cells_a: &[Cell], area_a: Rect, cells_b: &[Cell], area_b: Rect) {
        use std::collections::HashMap;
        let by_path_a: HashMap<&Path, &Cell> = cells_a
            .iter()
            .filter(|c| !c.is_bundle())
            .map(|c| (c.path.as_path(), c))
            .collect();
        let mut compared = 0usize;
        for cell in cells_b.iter().filter(|c| !c.is_bundle()) {
            let Some(a) = by_path_a.get(cell.path.as_path()) else {
                continue;
            };
            // Map A's rect into B's screen space and compare all four numbers.
            let scale_w = area_b.w / area_a.w;
            let scale_h = area_b.h / area_a.h;
            let expected = Rect {
                x: area_b.x + (a.rect.x - area_a.x) * scale_w,
                y: area_b.y + (a.rect.y - area_a.y) * scale_h,
                w: a.rect.w * scale_w,
                h: a.rect.h * scale_h,
            };
            let ratio = (area_b.w / area_a.w).max(1.0);
            // Millipoints at zoom 1, ~0.03pt across the full 32× throw —
            // orders of magnitude under a pixel, and a genuine geometry
            // change moves a tile by whole points at least.
            let noise = 1e-3 * ratio;
            for (got, want) in [
                (cell.rect.x, expected.x),
                (cell.rect.y, expected.y),
                (cell.rect.w, expected.w),
                (cell.rect.h, expected.h),
            ] {
                assert!(
                    (got - want).abs() <= noise,
                    "{} moved between zooms (depth {}, {:?} vs expected {:?})",
                    cell.path.display(),
                    cell.depth,
                    cell.rect,
                    expected
                );
            }
            compared += 1;
        }
        assert!(compared > 20, "only {compared} cells survived both layouts — the test is not biting");
    }

    // The cost canary the bundle floor used to be for: a quarter-million
    // files in one folder must cost the layout what its pixels cost, not
    // what its listing costs. Run by hand with
    // `cargo test -p files --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn packing_cost_canary_200k() {
        let mut crowd = Vec::new();
        for i in 0..200_000u64 {
            let size = ((i.wrapping_mul(2_654_435_761)) % 9973 + 1) * 4096;
            crowd.push(leaf(&format!("f{i}.bin"), size));
        }
        let tree = dir(
            "root",
            vec![leaf("huge.mov", 900_000_000_000), dir("crowd", crowd)],
        );
        let style = MapStyle::default();
        let viewport = Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 };
        // Far: the crowd is a small tile, its files all in the tail plate.
        let far = Rect { x: 0.0, y: 0.0, w: 1200.0, h: 800.0 };
        // Near: 64x in, anchored inside the crowd so its files fill the panel.
        let near = Rect { x: -20_000.0, y: -20_000.0, w: 1200.0 * 64.0, h: 800.0 * 64.0 };
        for (name, area) in [("far", far), ("near", near)] {
            let t = Cx::monotonic_now();
            let mut cells = 0usize;
            const RUNS: u32 = 20;
            for _ in 0..RUNS {
                cells = layout(&tree, Path::new("/root"), area, viewport, &style, None).len();
            }
            println!(
                "200k-folder {name}: {:.2}ms per layout, {cells} cells",
                (Cx::monotonic_now() - t) * 1000.0 / RUNS as f64
            );
        }
    }

    /// The rules a test walks under: everything is generic, nothing is
    /// skipped. The app's own policy is tested where the app defines it.
    fn open_rules<'a>() -> ScanRules<'a> {
        ScanRules {
            classify: &|_: &Path, _: bool| 0u8,
            skip: &|_: &Path| false,
        }
    }

    fn style() -> MapStyle {
        MapStyle {
            min_side: 1.0,
            min_area: 1.0,
            // A fraction of the packing area's short side, like the default:
            // 1% keeps a visible margin at the test geometries (a 100pt rect
            // frames its children by a full point) without eating them.
            inset: 0.01,
            header: 6.0,
            header_min: (30.0, 20.0),
            group_min: 3.0,
            max_cells: 10_000,
        }
    }

    fn overlap_area(a: Rect, b: Rect) -> f64 {
        let x_overlap = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let y_overlap = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        if x_overlap > 1e-6 && y_overlap > 1e-6 {
            x_overlap * y_overlap
        } else {
            0.0
        }
    }

    fn assert_no_overlaps(rects: &[Rect]) {
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let overlap = overlap_area(rects[i], rects[j]);
                assert!(
                    overlap < 1e-6,
                    "rects {} and {} overlap by {} ({:?} vs {:?})",
                    i,
                    j,
                    overlap,
                    rects[i],
                    rects[j]
                );
            }
        }
    }

    fn assert_inside(rect: Rect, container: Rect) {
        assert!(rect.x >= container.x - 1e-6);
        assert!(rect.y >= container.y - 1e-6);
        assert!(rect.x + rect.w <= container.x + container.w + 1e-6);
        assert!(rect.y + rect.h <= container.y + container.h + 1e-6);
    }

    // The exact example from Bruls, Huizing & van Wijk (2000): sizes that
    // sum to the container's area, so proportionality is easy to check by
    // hand as well as by assertion.
    #[test]
    fn squarify_known_case_is_valid() {
        let sizes = [6u64, 6, 4, 3, 2, 2, 1];
        let rect = Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 };
        let rects = squarify(&sizes, rect);
        assert_eq!(rects.len(), sizes.len());
        let total: f64 = sizes.iter().map(|&s| s as f64).sum();
        for (i, r) in rects.iter().enumerate() {
            assert_inside(*r, rect);
            let expected = sizes[i] as f64 / total * rect.area();
            assert!(
                (r.area() - expected).abs() < 1e-6,
                "size {} -> area {} but expected {}",
                sizes[i],
                r.area(),
                expected
            );
        }
        assert_no_overlaps(&rects);
        let sum_areas: f64 = rects.iter().map(Rect::area).sum();
        assert!((sum_areas - rect.area()).abs() < 1e-6);
    }

    // The property that makes this a *squarified* treemap rather than a
    // slice-and-dice strip: equal-weight items come out close to square,
    // not as sixteen slivers running the length of the rect.
    #[test]
    fn squarify_keeps_cells_reasonably_square() {
        let sizes = [100u64; 16];
        let rect = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let rects = squarify(&sizes, rect);
        for r in &rects {
            let aspect = (r.w / r.h).max(r.h / r.w);
            assert!(aspect < 2.0, "aspect {} too extreme for {:?}", aspect, r);
        }
    }

    // A long descending run is the shape a real folder has, and the shape a
    // buggy row-closing rule turns into hairlines.
    #[test]
    fn squarify_stays_square_on_a_long_descending_run() {
        let sizes: Vec<u64> = (1..=200).rev().map(|i| i as u64 * i as u64).collect();
        let rect = Rect { x: 0.0, y: 0.0, w: 900.0, h: 600.0 };
        let rects = squarify(&sizes, rect);
        let worst = rects
            .iter()
            .filter(|r| r.area() > 4.0)
            .map(|r| (r.w / r.h).max(r.h / r.w))
            .fold(0.0_f64, f64::max);
        assert!(worst < 6.0, "worst aspect ratio {worst} is a hairline");
        assert_no_overlaps(&rects);
    }

    #[test]
    fn squarify_edge_cases() {
        let rect = Rect { x: 1.0, y: 2.0, w: 10.0, h: 5.0 };

        // Empty input gives empty output.
        assert!(squarify(&[], rect).is_empty());

        // A single item fills the whole rect exactly.
        let single = squarify(&[42], rect);
        assert_eq!(single.len(), 1);
        assert!((single[0].x - rect.x).abs() < 1e-9);
        assert!((single[0].y - rect.y).abs() < 1e-9);
        assert!((single[0].w - rect.w).abs() < 1e-9);
        assert!((single[0].h - rect.h).abs() < 1e-9);

        // All zero: every rect is zero-area, nothing panics or produces NaN.
        let zeros = squarify(&[0, 0, 0], rect);
        assert_eq!(zeros.len(), 3);
        for r in &zeros {
            assert_eq!(r.area(), 0.0);
            assert!(!r.w.is_nan() && !r.h.is_nan());
        }

        // Mixed zero and non-zero: the zero entries get zero area, the
        // rest still accounts for the whole rect between them.
        let mixed = squarify(&[10, 0, 5, 0], rect);
        assert_eq!(mixed.len(), 4);
        assert_eq!(mixed[1].area(), 0.0);
        assert_eq!(mixed[3].area(), 0.0);
        assert!(mixed[0].area() > 0.0 && mixed[2].area() > 0.0);
        let sum: f64 = mixed.iter().map(Rect::area).sum();
        assert!((sum - rect.area()).abs() < 1e-6);
        for r in &mixed {
            assert!(!r.w.is_nan() && !r.h.is_nan());
        }
    }

    #[test]
    fn layout_paints_groups_before_their_children() {
        let tree = dir(
            "root",
            vec![
                dir("sub", vec![leaf("a.txt", 100), leaf("b.txt", 200)]),
                leaf("c.txt", 50),
            ],
        );
        let area = Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let cells = layout(&tree, Path::new("/root"), area, area, &style(), None);

        let sub_index = cells.iter().position(|c| c.name == "sub").unwrap();
        let a_index = cells.iter().position(|c| c.name == "a.txt").unwrap();
        let b_index = cells.iter().position(|c| c.name == "b.txt").unwrap();
        assert!(sub_index < a_index);
        assert!(sub_index < b_index);
        assert_eq!(cells[sub_index].depth, 0);
        assert_eq!(cells[a_index].depth, 1);
        assert_eq!(cells[b_index].depth, 1);
        assert!(cells[sub_index].is_group);
        assert!(!cells[a_index].is_group);
        // Paths are rebuilt from the names on the way down.
        assert_eq!(cells[a_index].path, Path::new("/root/sub/a.txt"));
    }

    // The whole point of the rewrite: the map goes all the way down to the
    // file, not two folders and then a flat plate.
    #[test]
    fn layout_reaches_individual_files_at_any_depth() {
        let mut tree = leaf("buried.bin", 1_000_000);
        for name in ["j", "i", "h", "g", "f", "e", "d", "c", "b", "a"] {
            tree = dir(name, vec![tree]);
        }
        let tree = dir("root", vec![tree]);
        let area = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let cells = layout(&tree, Path::new("/root"), area, area, &style(), None);
        let buried = cells.iter().find(|c| c.name == "buried.bin").unwrap();
        assert_eq!(buried.depth, 10);
        assert_eq!(
            buried.path,
            Path::new("/root/a/b/c/d/e/f/g/h/i/j/buried.bin")
        );
        assert!(buried.rect.area() > 100.0);
    }

    #[test]
    fn layout_drops_slivers_below_min_side() {
        let tree = dir("root", vec![leaf("big.bin", 1_000_000), leaf("tiny.bin", 1)]);
        let area = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 1000.0 };
        let mut style = style();
        style.min_side = 4.0;
        style.min_area = 16.0;
        let cells = layout(&tree, Path::new("/root"), area, area, &style, None);
        assert!(cells.iter().any(|c| c.name == "big.bin"));
        assert!(!cells.iter().any(|c| c.name == "tiny.bin"));
    }

    // A folder with a quarter of a million tiny files must cost the map what
    // its rectangle is worth, not what its listing is worth.
    #[test]
    fn layout_bundles_the_invisible_tail_instead_of_laying_it_out() {
        let mut children = vec![leaf("big.bin", 500_000_000)];
        children.extend((0..50_000).map(|i| leaf(&format!("t{i}.tmp"), 100)));
        let tree = dir("root", children);
        let area = Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 };
        let cells = layout(&tree, Path::new("/root"), area, area, &MapStyle::default(), None);
        // Two rectangles: the big file, and one that says how many were left.
        assert!(cells.len() < 8, "{} cells is a laid-out tail", cells.len());
        let bundle = cells.iter().find(|c| c.is_bundle()).unwrap();
        assert_eq!(bundle.extra, 50_000);
        assert_eq!(bundle.size, 5_000_000);
        // Every byte is still on the map: the bundle is a sum, not a cull.
        let mapped: u64 = cells.iter().filter(|c| c.depth == 0).map(|c| c.size).sum();
        assert_eq!(mapped, tree.size);
    }

    // The whole reason the map is worth keeping between runs: a delete is
    // arithmetic on a tree we already have, not another walk of the disk.
    #[test]
    fn deleting_something_costs_no_scan_and_leaves_the_totals_right() {
        let mut tree = dir(
            "root",
            vec![
                dir("movies", vec![leaf("big.mov", 900), leaf("small.mov", 100)]),
                leaf("notes.txt", 25),
            ],
        );
        assert_eq!(tree.size, 1025);
        assert_eq!(tree.files, 3);

        let gone = tree
            .detach(&["movies".into(), "big.mov".into()])
            .expect("the file was on the map");
        assert_eq!(gone.size, 900);
        // Every folder above it shrank by exactly what left.
        assert_eq!(tree.size, 125);
        assert_eq!(tree.files, 2);
        assert_eq!(tree.at(&["movies".into()]).unwrap().size, 100);

        // Nothing is there to take twice.
        assert!(tree.detach(&["movies".into(), "big.mov".into()]).is_none());
    }

    // Trash is a move, not a disappearance: if the Trash is inside the map,
    // the bytes are still on it and the total must not change.
    #[test]
    fn a_move_inside_the_map_keeps_the_total() {
        let mut tree = dir(
            "root",
            vec![
                dir("movies", vec![leaf("big.mov", 900)]),
                dir("trash", vec![]),
            ],
        );
        let before = tree.size;
        let node = tree.detach(&["movies".into(), "big.mov".into()]).unwrap();
        assert_eq!(tree.size, 0);
        assert!(tree.graft(&["trash".into()], node));
        assert_eq!(tree.size, before);
        assert_eq!(tree.at(&["trash".into()]).unwrap().size, 900);
        // Somewhere the map is not of: the bytes really did leave.
        let node = tree.detach(&["trash".into(), "big.mov".into()]).unwrap();
        assert!(!tree.graft(&["nowhere".into()], node));
        assert_eq!(tree.size, 0);
    }

    // A folder we were refused is not a folder of zero bytes, and the map has
    // to be able to say which ones they were.
    #[test]
    fn refused_folders_are_named_not_silently_dropped() {
        let mut locked = dir("Documents", vec![]);
        locked.denied = true;
        let mut inner = dir("deep", vec![]);
        inner.denied = true;
        let tree = dir("root", vec![locked, dir("ok", vec![inner, leaf("a", 1)])]);
        let named = tree.denied_paths(8);
        assert_eq!(named, vec!["Documents".to_string(), "ok/deep".to_string()]);
        // Bounded, because a list nobody can read is not a warning.
        assert_eq!(tree.denied_paths(1).len(), 1);
    }

    // The camera contract: blowing the map up N× and looking at it through a
    // window must cost what the window costs, and must actually show more —
    // the detail floor follows the magnified area, not the screen.
    fn layout_of(tree: &Node, area: Rect) -> Vec<String> {
        layout(tree, Path::new("/root"), area, area, &MapStyle::default(), None)
            .into_iter()
            .filter(|c| !c.is_bundle())
            .map(|c| c.name)
            .collect()
    }

    // Equal sizes are everywhere on a real disk — shards, dedup'd assets —
    // and they must lay out in the same order every single time, and keep
    // that order when the camera's area drifts. Anything else is a map that
    // shuffles its tiles whenever the camera settles.
    #[test]
    fn equal_sized_siblings_never_swap_between_layouts() {
        let children: Vec<Node> = (0..120)
            .map(|i| leaf(&format!("shard-{i:03}"), 510_000_000))
            .chain((0..300).map(|i| leaf(&format!("t{i}.tmp"), 1_000 + i as u64)))
            .collect();
        let tree = dir("root", children);
        let area = Rect { x: 0.0, y: 0.0, w: 900.0, h: 600.0 };

        // Twice at the same area: byte-identical order.
        let a = layout_of(&tree, area);
        let b = layout_of(&tree, area);
        assert_eq!(a, b);
        // The equal-size run keeps child order, deterministically.
        let shards: Vec<&String> = a.iter().filter(|n| n.starts_with("shard")).collect();
        assert!(shards.windows(2).all(|w| w[0] < w[1]), "{shards:?}");

        // At a slightly different area (a small zoom's settle), tiles may be
        // added or dropped — but the ones present in both keep their
        // relative order exactly.
        let grown = Rect { x: 0.0, y: 0.0, w: 940.0, h: 627.0 };
        let c = layout_of(&tree, grown);
        let shared: std::collections::HashSet<&String> = a
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .intersection(&c.iter().collect())
            .copied()
            .collect();
        let a_shared: Vec<&String> = a.iter().filter(|n| shared.contains(n)).collect();
        let c_shared: Vec<&String> = c.iter().filter(|n| shared.contains(n)).collect();
        assert_eq!(a_shared, c_shared, "surviving tiles reordered across a small zoom");
    }

    #[test]
    fn a_zoomed_layout_culls_to_the_viewport_and_gains_detail() {
        // 200 equal folders of 40 files each: at screen size a folder is a
        // ~17pt tile, so its files land far below the visibility floor.
        let children: Vec<Node> = (0..200)
            .map(|i| {
                dir(
                    &format!("d{i}"),
                    (0..40).map(|j| leaf(&format!("f{j}.bin"), 25_000)).collect(),
                )
            })
            .collect();
        let tree = dir("root", children);
        let screen = Rect { x: 0.0, y: 0.0, w: 300.0, h: 200.0 };
        let style = MapStyle::default();

        // Unzoomed: the folders show, their files are bundled away.
        let flat = layout(&tree, Path::new("/root"), screen, screen, &style, None);
        assert!(flat.iter().any(|c| c.name == "d0"));
        assert!(flat.iter().all(|c| !c.name.starts_with('f')));

        // 8× camera, looking at the top-left corner of the blown-up map.
        let area = Rect { x: 0.0, y: 0.0, w: 2400.0, h: 1600.0 };
        let zoomed = layout(&tree, Path::new("/root"), area, screen, &style, None);

        // Everything delivered is at least partly on screen…
        for cell in &zoomed {
            assert!(
                cell.rect.intersects(&screen),
                "{} at {:?} is entirely off screen",
                cell.name,
                cell.rect
            );
        }
        // …the off-screen majority was skipped, not delivered…
        let dirs = zoomed.iter().filter(|c| c.is_dir).count();
        assert!(dirs < 60, "{dirs} of 200 folders for a 1/64 window");
        // …and the zoom bought real detail: the files inside are visible now.
        assert!(zoomed.iter().any(|c| c.name.starts_with('f')));
    }

    #[test]
    fn hit_finds_the_deepest_cell() {
        let tree = dir("root", vec![dir("sub", vec![leaf("a.txt", 100)])]);
        let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let cells = layout(&tree, Path::new("/root"), area, area, &style(), None);

        let group = cells.iter().position(|c| c.name == "sub").unwrap();
        let child = cells.iter().position(|c| c.name == "a.txt").unwrap();
        let group_rect = cells[group].rect;
        let child_rect = cells[child].rect;

        // A point in the group's margin (inside the border/header inset,
        // before the child's own rect begins) should hit the group.
        assert_eq!(hit(&cells, group_rect.x + 0.5, group_rect.y + 0.5), Some(group));

        // A point solidly inside the child should hit the child, not the
        // group sitting behind it, even though both rects contain it.
        let cx = child_rect.x + child_rect.w / 2.0;
        let cy = child_rect.y + child_rect.h / 2.0;
        assert!(group_rect.contains(cx, cy), "test setup: child not nested in group");
        assert_eq!(hit(&cells, cx, cy), Some(child));
    }

    #[test]
    fn a_folder_paints_as_its_heaviest_content() {
        let mut tree = dir(
            "root",
            vec![Node::file("clip.mov".into(), 5, 900), Node::file("note.txt".into(), 2, 10)],
        );
        // Rebuilt the way `apply` would, so the rule is the one the scan uses.
        tree.kind = heaviest_kind(&tree.children).unwrap();
        assert_eq!(tree.kind, 5);
    }

    // ------------------------------------------------------------- scanning

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "files-treemap-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn sample_tree(root: &Path) -> u64 {
        fs::create_dir_all(root.join("sub/subsub")).unwrap();
        fs::write(root.join("a.txt"), b"aaaaa").unwrap(); // 5 bytes
        fs::write(root.join("sub/b.txt"), b"bbbbbbb").unwrap(); // 7 bytes
        fs::write(root.join("sub/subsub/c.txt"), b"ccc").unwrap(); // 3 bytes

        #[cfg(unix)]
        {
            // A link back up to an ancestor of the folder it sits in: if the
            // scan ever followed it, this test would hang rather than
            // finish, which is exactly the bug this case exists to catch.
            let link = root.join("sub/subsub/loop");
            std::os::unix::fs::symlink(root, &link).unwrap();
            15 + fs::symlink_metadata(&link).unwrap().len()
        }
        #[cfg(not(unix))]
        {
            15
        }
    }

    #[test]
    fn scan_rolls_up_recursive_sizes_and_stops_symlink_cycles() {
        let root = temp_root("scan");
        let expected = sample_tree(&root);

        let cancel = AtomicBool::new(false);
        let node = scan(&root, &open_rules(), &cancel, &|_| {}).expect("scan should complete");

        assert_eq!(node.size, expected);
        assert_eq!(node.files, if cfg!(unix) { 4 } else { 3 });

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_returns_none_when_already_cancelled() {
        let root = temp_root("cancel");
        fs::create_dir_all(&root).unwrap();

        let cancel = AtomicBool::new(true);
        assert!(scan(&root, &open_rules(), &cancel, &|_| {}).is_none());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_reports_progress_at_a_bounded_rate_not_per_entry() {
        let root = temp_root("progress");
        fs::create_dir_all(&root).unwrap();
        for i in 0..600 {
            fs::write(root.join(format!("f{i}.bin")), b"x").unwrap();
        }

        let cancel = AtomicBool::new(false);
        let calls = std::sync::atomic::AtomicU32::new(0);
        let node = scan(&root, &open_rules(), &cancel, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();

        assert_eq!(node.files, 600);
        // 600 entries at a stride of 512 is at most two mid-walk reports
        // plus the guaranteed final one — nowhere near one call per file.
        let calls = calls.load(Ordering::Relaxed);
        assert!(calls <= 4, "too many progress calls: {calls}");

        fs::remove_dir_all(&root).ok();
    }

    // The streamed walk and the blocking one have to agree, or the map is a
    // different disk than the properties panel.
    #[test]
    fn the_streamed_scan_builds_the_same_tree_as_the_blocking_one() {
        let root = temp_root("stream");
        let expected = sample_tree(&root);
        // Deep enough that an old depth-cutoff walker would have switched to
        // handing back whole subtrees — this tree must stream all the way.
        fs::create_dir_all(root.join("deep/a/b/c/d")).unwrap();
        fs::write(root.join("deep/a/b/c/d/e.bin"), vec![0u8; 400]).unwrap();

        let cancel = AtomicBool::new(false);
        let steps = Mutex::new(Vec::new());
        let ok = with_pool(|pool| {
            scan_stream(&root, &open_rules(), &cancel, &|step| {
                steps.lock().unwrap().push(step);
            }, pool)
        });
        assert!(ok);

        let mut tree = Node::dir("root".into(), 0);
        for step in steps.into_inner().unwrap() {
            assert!(tree.apply(step), "a step named a node that is not there");
        }
        assert_eq!(tree.size, expected + 400);
        assert_eq!(tree.files, if cfg!(unix) { 5 } else { 4 });
        assert!(
            tree.children.iter().any(|c| c.name == "deep"),
            "the root's own listing never arrived"
        );

        // Folders are announced a level at a time and are only known to be
        // finished when the walk is, which is what `seal` says.
        fn all_done(node: &Node) -> bool {
            node.children.iter().all(|c| (!c.is_dir || c.done) && all_done(c))
        }
        assert!(!all_done(&tree), "nothing should be sealed before the walk ends");
        tree.seal();
        assert!(all_done(&tree));

        fs::remove_dir_all(&root).ok();
    }

    // The reason the walk has no depth cutoff: a 500 GB folder four levels
    // down must fill in live on the map, not sit as one opaque block until
    // its whole subtree has been walked. Every directory at every depth
    // announces its own listing; nothing is delivered as a finished subtree.
    #[test]
    fn every_directory_streams_its_own_listing_at_any_depth() {
        let root = temp_root("stream-depth");
        sample_tree(&root); // root, sub, sub/subsub
        fs::create_dir_all(root.join("deep/a/b/c/d")).unwrap();
        fs::write(root.join("deep/a/b/c/d/e.bin"), vec![0u8; 400]).unwrap();

        let cancel = AtomicBool::new(false);
        let steps = Mutex::new(Vec::new());
        assert!(with_pool(|pool| {
            scan_stream(&root, &open_rules(), &cancel, &|step| {
                steps.lock().unwrap().push(step);
            }, pool)
        }));

        let mut opened_ats: Vec<Vec<u32>> = Vec::new();
        for step in steps.into_inner().unwrap() {
            match step {
                ScanStep::Opened { at, .. } => opened_ats.push(at),
                ScanStep::Closed { .. } => {
                    panic!("the walk handed back a whole subtree instead of streaming it")
                }
                _ => {}
            }
        }
        // One listing per directory: root, sub, subsub, deep, a, b, c, d.
        assert_eq!(opened_ats.len(), 8, "opened: {opened_ats:?}");
        let deepest = opened_ats.iter().map(|at| at.len()).max().unwrap();
        assert_eq!(deepest, 5, "deep/a/b/c/d never announced its own listing");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_streamed_scan_announces_the_root_before_it_walks_anything() {
        let root = temp_root("first-step");
        sample_tree(&root);

        let cancel = AtomicBool::new(false);
        let first = Mutex::new(None);
        with_pool(|pool| {
            scan_stream(&root, &open_rules(), &cancel, &|step| {
                let mut slot = first.lock().unwrap();
                if slot.is_none() {
                    *slot = Some(match step {
                        ScanStep::Opened { at, children, .. } => (at, children.len()),
                        other => panic!("first step was {other:?}, not the root listing"),
                    });
                }
            }, pool)
        });
        let (at, count) = first.into_inner().unwrap().expect("no steps at all");
        assert!(at.is_empty());
        assert_eq!(count, 2); // a.txt and sub/

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_cancelled_stream_reports_that_it_did_not_finish() {
        let root = temp_root("stream-cancel");
        sample_tree(&root);
        let cancel = AtomicBool::new(true);
        assert!(!with_pool(|pool| scan_stream(&root, &open_rules(), &cancel, &|_| {}, pool)));
        fs::remove_dir_all(&root).ok();
    }

    // ------------------------------------------------------------- filter

    #[test]
    fn the_query_parser_reads_every_term_form() {
        let now = 1_000_000u32;
        let q = Query::parse("cache .mov *.mkv >100mb <2gb <7d qwen", now);
        assert_eq!(q.names, vec!["cache".to_string(), "qwen".to_string()]);
        assert_eq!(q.exts, vec!["mov".to_string(), "mkv".to_string()]);
        assert_eq!(q.min_size, Some(100_000_000));
        assert_eq!(q.max_size, Some(2_000_000_000));
        assert_eq!(q.newer_than, Some(now - 7 * 24 * 60));
        assert!(q.older_than.is_none());
        // ">1y" is the other direction: untouched for a year.
        let old = Query::parse(">1y", now);
        assert_eq!(old.older_than, Some(now - 525_600));
        // Sizes are 1000-based like every number the app prints; bounds
        // that fail to parse fall back to being name terms, never dropped.
        let odd = Query::parse(">wat", now);
        assert_eq!(odd.names, vec![">wat".to_string()]);
        assert!(Query::parse("", now).is_empty());
    }

    #[test]
    fn a_filtered_folder_weighs_only_its_matching_bytes() {
        let tree = dir(
            "root",
            vec![
                dir(
                    "movies",
                    vec![leaf("a.mov", 900), leaf("b.txt", 50), leaf("c.mov", 100)],
                ),
                dir("docs", vec![leaf("d.txt", 500)]),
            ],
        );
        let q = Query::parse(".mov", 0);
        let (bytes, files) = filtered_size(&tree, &q, 0);
        assert_eq!((bytes, files), (1000, 2));
        // The one-walk measure agrees with the recursive sum, at the root
        // and per child — it is the layout's only source of weights now.
        let m = measure(&tree, &q, 0);
        assert_eq!((m.bytes, m.files), (1000, 2));
        assert_eq!(m.children.len(), 2);
        assert_eq!(m.children[0].bytes, 1000);
        assert_eq!(m.children[1].bytes, 0);
        assert_eq!(m.children[0].children.iter().map(|c| c.bytes).collect::<Vec<_>>(), vec![900, 0, 100]);
        // A folder with no matching bytes vanishes from the layout entirely.
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let cells = layout(&tree, Path::new("/root"), area, area, &style(), Some(&m));
        assert!(cells.iter().any(|c| c.name == "movies" && c.size == 1000));
        assert!(!cells.iter().any(|c| c.name == "docs"));
        assert!(!cells.iter().any(|c| c.name == "b.txt"));
        // And no filter costs nothing different from before.
        let plain = layout(&tree, Path::new("/root"), area, area, &style(), None);
        assert!(plain.iter().any(|c| c.name == "docs"));
    }

    // The measure is a cache, and caches go stale: one made of a different
    // tree shape must be refused wholesale, never indexed out of step.
    #[test]
    fn a_stale_measure_is_refused_not_misapplied() {
        let tree = dir("root", vec![leaf("a.mov", 900), leaf("b.txt", 50)]);
        let q = Query::parse(".mov", 0);
        let mut m = measure(&tree, &q, 0);
        m.children.pop(); // now shaped like some other tree
        let area = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let cells = layout(&tree, Path::new("/root"), area, area, &style(), Some(&m));
        // Fell back to the unfiltered weights: everything is on the map.
        assert!(cells.iter().any(|c| c.name == "b.txt"));
    }

    #[test]
    fn a_name_term_matches_everything_under_a_matching_folder() {
        let tree = dir(
            "root",
            vec![
                dir("cache", vec![leaf("blob.bin", 700)]),
                leaf("cache.log", 40),
                leaf("other.bin", 25),
            ],
        );
        let q = Query::parse("cache", 0);
        let (bytes, files) = filtered_size(&tree, &q, 0);
        assert_eq!((bytes, files), (740, 2));
    }

    #[test]
    fn age_terms_ride_on_the_rolled_up_mtime() {
        let now = 2_000_000u32;
        let mut tree = dir(
            "root",
            vec![dir(
                "sub",
                vec![
                    Node::file_at("new.txt".into(), 0, 10, now - 60),
                    Node::file_at("old.txt".into(), 0, 20, now - 1_000_000),
                ],
            )],
        );
        tree.children[0].modified = now - 60;
        tree.modified = now - 60;
        let q = Query::parse("<7d", now);
        assert_eq!(filtered_size(&tree, &q, 0), (10, 1));
        // The folder counts as new because something new is inside it —
        // that is what the max roll-up means.
        assert_eq!(tree.modified, now - 60);
    }

    #[test]
    fn the_walk_rolls_the_newest_mtime_up_to_the_root() {
        let root = temp_root("mtime");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/a.txt"), b"aa").unwrap();
        let cancel = AtomicBool::new(false);
        let steps = Mutex::new(Vec::new());
        assert!(with_pool(|pool| {
            scan_stream(&root, &open_rules(), &cancel, &|step| {
                steps.lock().unwrap().push(step);
            }, pool)
        }));
        let mut tree = Node::dir("root".into(), 0);
        for step in steps.into_inner().unwrap() {
            tree.apply(step);
        }
        // Written moments ago: the minutes-since-epoch must be recent and
        // must have reached the root through the roll-up.
        assert!(tree.modified > 0);
        let now = (Cx::time_now().max(0.0) as u64 / 60).min(u32::MAX as u64) as u32;
        assert!(now - tree.modified < 5, "root mtime {} vs now {}", tree.modified, now);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn kind_totals_sum_files_by_tag() {
        let tree = dir(
            "root",
            vec![
                Node::file("a.mov".into(), 5, 900),
                Node::file("b.mov".into(), 5, 100),
                Node::file("c.txt".into(), 2, 30),
            ],
        );
        let totals = kind_totals(&tree);
        assert_eq!(totals[5], 1000);
        assert_eq!(totals[2], 30);
        assert_eq!(totals[0], 0);
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1500), "1.5 KB");
        assert_eq!(format_bytes(15_000), "15 KB");
        assert_eq!(format_bytes(2_500_000_000), "2.5 GB");
    }
}
