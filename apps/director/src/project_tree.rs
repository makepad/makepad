//! A lazy project browser over the shared FileTree widget. One worker owns
//! directory IO; the UI only applies bounded immutable listing snapshots.

use makepad_widgets::file_tree::{FileTree, FileTreeAction};
use makepad_widgets::makepad_platform::thread::{SignalToUI, TaskHandle, ThreadOptions, ThreadSpawner};
use makepad_widgets::widget_tree::CxWidgetExt;
use makepad_widgets::*;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
    sync::{atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender, TrySendError}, Arc},
    time::{Duration, Instant},
};

const MAX_DIRS: usize = 128;
const MAX_DEPTH: usize = 64;
const MAX_DIRECTORY_ENTRIES: usize = 2048;
const MAX_TOTAL_ENTRIES: usize = 8192;
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.StudioProjectTreeBase = #(StudioProjectTree::register_widget(vm))
    mod.widgets.StudioProjectTree = set_type_default() do mod.widgets.StudioProjectTreeBase{
        file_tree: FileTree{width: Fill height: Fill}
    }
}

#[derive(Clone, Debug, Default)]
pub enum ProjectTreeAction {
    Open(PathBuf),
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind { Directory, File, Symlink }

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    /// Relative to the requested project root; no parent components.
    path: PathBuf,
    name: String,
    kind: EntryKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Listing {
    entries: Vec<Entry>,
    note: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Snapshot {
    generation: u64,
    project: PathBuf,
    directories: BTreeMap<PathBuf, Listing>,
}

#[derive(Clone, Debug)]
struct Request {
    generation: u64,
    project: PathBuf,
    expanded: BTreeSet<PathBuf>,
}

struct TreeWorker {
    commands: SyncSender<Arc<Request>>,
    snapshots: Receiver<Arc<Snapshot>>,
    pending: Option<Arc<Request>>,
    stop: Arc<AtomicBool>,
    task: TaskHandle<()>,
}

impl TreeWorker {
    fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        { let _ = spawner; return Err("Local project browsing is unavailable in this browser".into()); }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (commands, rx) = mpsc::sync_channel(1);
            let (tx, snapshots) = mpsc::sync_channel(1);
            let stop = Arc::new(AtomicBool::new(false));
            let cancel = stop.clone();
            let task = spawner.spawn_worker(ThreadOptions {
                name: Some("studio-project-tree".into()), ..Default::default()
            }, move || run_worker(rx, tx, cancel)).map_err(|e| e.to_string())?;
            Ok(Self { commands, snapshots, pending: None, stop, task })
        }
    }

    fn request(&mut self, request: Request) {
        self.pending = Some(Arc::new(request));
        self.retry();
    }

    fn retry(&mut self) {
        if let Some(request) = self.pending.take() {
            match self.commands.try_send(request) {
                Ok(()) => {},
                Err(TrySendError::Full(request)) => self.pending = Some(request),
                Err(TrySendError::Disconnected(_)) => {},
            }
        }
    }
}

impl Drop for TreeWorker {
    fn drop(&mut self) { self.stop.store(true, Ordering::Relaxed); }
}

#[derive(Script, ScriptHook, WidgetRegister, WidgetRef, WidgetSet)]
pub struct StudioProjectTree {
    #[uid]
    uid: WidgetUid,
    #[live]
    file_tree: FileTree,
    #[rust]
    project: PathBuf,
    #[rust]
    expanded: BTreeSet<PathBuf>,
    #[rust]
    snapshot: Arc<Snapshot>,
    #[rust]
    nodes: HashMap<LiveId, Entry>,
    #[rust]
    selected: Option<PathBuf>,
    #[rust]
    reveal_pending: Option<PathBuf>,
    #[rust]
    worker: Option<TreeWorker>,
    #[rust]
    generation: u64,
    #[rust]
    message: Option<String>,
    #[rust]
    indexed_rows: (usize, u64, u64),
}

impl WidgetNode for StudioProjectTree {
    fn widget_uid(&self) -> WidgetUid { self.uid }

    fn walk(&mut self, cx: &mut Cx) -> Walk { self.file_tree.walk(cx) }

    fn area(&self) -> Area { self.file_tree.area() }

    fn redraw(&mut self, cx: &mut Cx) { self.file_tree.redraw(cx); }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        // FileTree is embedded by value, so its instantiated rows need to be
        // exposed explicitly for widget inspection and descendant lookup.
        self.file_tree.children(visit);
    }
}

impl StudioProjectTree {
    pub fn set_project(&mut self, cx: &mut Cx, project: PathBuf) -> Result<(), String> {
        if !project.is_absolute() || project.as_os_str().len() > 4096
            || project.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err("Project root must be an absolute path without parent components".into());
        }
        if self.worker.as_ref().is_some_and(|w| w.stop.load(Ordering::Relaxed) || w.task.is_finished()) {
            return Err("Project tree worker is stopped".into());
        }
        if self.worker.is_none() {
            match TreeWorker::start(&cx.thread_spawner()) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => { self.message = Some(error.clone()); self.file_tree.redraw(cx); return Err(error); },
            }
        }
        if self.project != project {
            for path in &self.expanded {
                self.file_tree.set_folder_is_open(cx, path_id(&self.project, path), false, Animate::No);
            }
            self.file_tree.forget();
            self.indexed_rows = (0, 0, 0);
            cx.widget_tree_mark_dirty(self.uid);
            self.file_tree.scroll_to_y(cx, 0.0);
            self.project = project;
            self.expanded.clear();
            self.expanded.insert(PathBuf::new());
            self.snapshot = Arc::new(Snapshot::default());
            self.nodes.clear();
            self.selected = None;
            self.reveal_pending = None;
            self.file_tree.set_folder_is_open(cx, path_id(&self.project, Path::new("")), true, Animate::No);
        }
        self.refresh(cx);
        Ok(())
    }

    /// Explicit refresh preserves open folders, scroll and selected file.
    pub fn refresh(&mut self, cx: &mut Cx) {
        if self.project.as_os_str().is_empty() { return; }
        self.generation = self.generation.wrapping_add(1);
        self.message = None;
        if let Some(worker) = &mut self.worker {
            worker.request(Request {
                generation: self.generation, project: self.project.clone(), expanded: self.expanded.clone(),
            });
        }
        self.file_tree.redraw(cx);
    }

    /// Hosts may call this even when their dock tab is not being drawn.
    pub fn poll(&mut self, cx: &mut Cx) {
        let Some(worker) = &mut self.worker else { return; };
        worker.retry();
        let mut latest = None;
        while let Ok(snapshot) = worker.snapshots.try_recv() {
            if snapshot.project == self.project && snapshot.generation == self.generation { latest = Some(snapshot); }
        }
        if let Some(snapshot) = latest {
            let mut nodes = HashMap::new();
            for listing in snapshot.directories.values() {
                for entry in &listing.entries { nodes.insert(path_id(&self.project, &entry.path), entry.clone()); }
            }
            for (id, previous) in &self.nodes {
                if nodes.get(id).is_none_or(|entry| entry.kind != previous.kind) {
                    self.file_tree.forget_node(*id);
                }
            }
            // A complete parent listing proves a removed/replaced folder no
            // longer needs a watch. Collapsed or partial listings prove nothing.
            let removed: Vec<_> = self.expanded.iter().filter(|path| {
                path.parent().and_then(|parent| snapshot.directories.get(parent)).is_some_and(|listing| {
                    listing.note.is_none() && !listing.entries.iter().any(|entry| entry.path == **path && entry.kind == EntryKind::Directory)
                })
            }).cloned().collect();
            let stale: Vec<_> = self.expanded.iter().filter(|path| removed.iter().any(|removed| path.starts_with(removed))).cloned().collect();
            for path in &stale {
                self.expanded.remove(path);
                self.file_tree.set_folder_is_open(cx, path_id(&self.project, path), false, Animate::No);
            }
            self.nodes = nodes;
            self.snapshot = snapshot;
            self.file_tree.redraw(cx);
            if !stale.is_empty() { self.refresh(cx); }
        }
    }

    /// Open the path's ancestors and reveal its existing stable row. It does
    /// not open a file or change the host's selected editor by itself.
    pub fn reveal(&mut self, cx: &mut Cx, path: &Path) -> Result<(), String> {
        let relative = path.strip_prefix(&self.project).map_err(|_| "File is outside this project")?;
        if !valid_relative(relative) { return Err("Unsupported project path".into()); }
        let mut expanded = self.expanded.clone();
        let mut parent = relative.parent();
        while let Some(path) = parent {
            expanded.insert(path.to_owned());
            parent = path.parent();
        }
        if expanded.len() > MAX_DIRS { return Err(format!("Expand at most {MAX_DIRS} folders")); }
        self.expanded = expanded;
        for path in &self.expanded {
            self.file_tree.set_folder_is_open(cx, path_id(&self.project, path), true, Animate::No);
        }
        self.selected = Some(relative.to_owned());
        self.reveal_pending = Some(relative.to_owned());
        self.file_tree.select_node(cx, path_id(&self.project, relative));
        self.refresh(cx);
        Ok(())
    }

    pub fn request_stop(&self) {
        if let Some(worker) = &self.worker { worker.stop.store(true, Ordering::Relaxed); }
    }

    pub fn is_finished(&self) -> bool { self.worker.as_ref().is_none_or(|w| w.task.is_finished()) }

    pub fn status(&self) -> String {
        if let Some(message) = &self.message { return message.clone(); }
        if self.worker.as_ref().is_some_and(|w| w.pending.is_some()) { return "Project refresh queued; reader is busy".into(); }
        if self.project.as_os_str().is_empty() { return "Choose a project".into(); }
        if self.snapshot.project != self.project { return "Loading project…".into(); }
        let notes = self.snapshot.directories.values().filter(|listing| listing.note.is_some()).count();
        if notes > 0 { format!("{notes} folders have partial or unavailable listings") }
        else { format!("{} folders · {} entries", self.snapshot.directories.len(), self.nodes.len()) }
    }
}

impl Widget for StudioProjectTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            if let Some(path) = &self.reveal_pending { self.file_tree.begin_reveal(path_id(&self.project, path)); }
            if self.project.as_os_str().is_empty() {
                self.file_tree.file(cx, id!(project_tree_empty), "Choose a project");
            } else {
                let name = self.project.file_name().map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.project.display().to_string());
                draw_directory(cx, &mut self.file_tree, &self.project, Path::new(""), &name, &self.snapshot, 0);
                if let Some(message) = &self.message { self.file_tree.file(cx, id!(project_tree_message), message); }
            }
        }
        // Rows are inserted/pruned while drawing the embedded FileTree. Its
        // own UID is not indexed, so invalidate this wrapper after those
        // mutations. HashMap iteration order is irrelevant to this fingerprint.
        let mut rows = (0usize, 0u64, 0u64);
        self.file_tree.children(&mut |id, node| {
            let mut hash = id.0 ^ node.widget_uid().0.rotate_left(23);
            hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d049bb133111eb);
            hash ^= hash >> 31;
            rows.0 += 1;
            rows.1 ^= hash;
            rows.2 = rows.2.wrapping_add(hash);
        });
        if rows != self.indexed_rows {
            self.indexed_rows = rows;
            cx.widget_tree_mark_dirty(self.uid);
        }
        if let Some(y) = self.file_tree.take_reveal_y() {
            let rect = self.file_tree.area().rect(cx);
            let row = self.file_tree.row_height();
            let adjustment = if y < rect.pos.y { y - rect.pos.y }
                else if y + row > rect.pos.y + rect.size.y { y + row - rect.pos.y - rect.size.y }
                else { 0.0 };
            if adjustment.abs() > 1.0 { self.file_tree.scroll_by(cx, adjustment); }
            else { self.reveal_pending = None; }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.poll(cx);
        let actions = cx.capture_actions(|cx| self.file_tree.handle_event(cx, event, scope));
        for action in actions {
            let Some(action) = action.as_widget_action() else { continue; };
            match action.cast::<FileTreeAction>() {
                FileTreeAction::FileClicked(id) => {
                    if let Some(entry) = self.nodes.get(&id) {
                        self.selected = Some(entry.path.clone());
                        cx.widget_action(self.uid, ProjectTreeAction::Open(self.project.join(&entry.path)));
                    }
                }
                FileTreeAction::FolderClicked(id) => {
                    let path = if id == path_id(&self.project, Path::new("")) { Some(PathBuf::new()) }
                        else { self.nodes.get(&id).map(|entry| entry.path.clone()) };
                    if let Some(path) = path {
                        if self.file_tree.is_folder_open(id) {
                            if self.expanded.len() >= MAX_DIRS && !self.expanded.contains(&path) {
                                self.file_tree.set_folder_is_open(cx, id, false, Animate::No);
                                self.message = Some(format!("Expand at most {MAX_DIRS} folders; close another first"));
                                self.file_tree.redraw(cx);
                                continue;
                            }
                            self.expanded.insert(path);
                        } else { self.expanded.remove(&path); }
                        self.refresh(cx);
                    }
                }
                FileTreeAction::ShouldFileStartDrag(id) => {
                    if let Some(entry) = self.nodes.get(&id) {
                        let path = self.project.join(&entry.path);
                        if let Some(path) = path.to_str() {
                            self.file_tree.start_dragging_file_node(cx, id, vec![DragItem::FilePath {
                                path: path.to_owned(), internal_id: None,
                            }]);
                        } else {
                            self.message = Some("This filename cannot be represented by native file drag".into());
                            self.file_tree.redraw(cx);
                        }
                    }
                }
                _ => {},
            }
        }
    }
}

fn path_id(project: &Path, relative: &Path) -> LiveId {
    let full = project.join(relative);
    let bytes = full.as_os_str().as_encoded_bytes();
    LiveId::from_bytes(LiveId::from_str("studio-project-node").0, bytes, 0, bytes.len(), 0)
}

fn note_id(project: &Path, relative: &Path) -> LiveId {
    LiveId::from_num(path_id(project, relative).0, 0x5f6e6f7465)
}

fn draw_directory(cx: &mut Cx2d, tree: &mut FileTree, project: &Path, relative: &Path,
    name: &str, snapshot: &Snapshot, depth: usize) {
    if tree.begin_folder(cx, path_id(project, relative), name).is_err() { return; }
    if depth >= MAX_DEPTH {
        tree.file(cx, note_id(project, relative), "Folder depth limit reached");
    } else if let Some(listing) = snapshot.directories.get(relative) {
        for entry in &listing.entries {
            if entry.kind == EntryKind::Directory {
                draw_directory(cx, tree, project, &entry.path, &entry.name, snapshot, depth + 1);
            } else {
                let name = if entry.kind == EntryKind::Symlink { format!("{} ↗", entry.name) } else { entry.name.clone() };
                tree.file(cx, path_id(project, &entry.path), &name);
            }
        }
        if let Some(note) = &listing.note { tree.file(cx, note_id(project, relative), note); }
        else if listing.entries.is_empty() { tree.file(cx, note_id(project, relative), "Empty folder"); }
    } else { tree.file(cx, note_id(project, relative), "Loading…"); }
    tree.end_folder();
}

fn valid_relative(path: &Path) -> bool {
    path.as_os_str().len() <= 4096 && path.components().count() <= MAX_DEPTH
        && path.components().all(|c| matches!(c, Component::Normal(name) if name != ".git"))
}

fn visible_directory(path: &Path, expanded: &BTreeSet<PathBuf>) -> bool {
    if path.as_os_str().is_empty() { return true; }
    let mut parent = path.parent();
    while let Some(path) = parent {
        if !expanded.contains(path) { return false; }
        parent = path.parent();
    }
    true
}

/// Check each component before reading a requested expanded directory. Symlink
/// entries are leaves, including links back to a parent or outside the root.
fn directory_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if !valid_relative(relative) { return Err("Unsupported folder path".into()); }
    let mut path = root.to_owned();
    for component in relative.components() {
        path.push(component.as_os_str());
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() { return Err("Symbolic-link folders are not expanded".into()); }
        if !metadata.is_dir() { return Err("Folder was removed or replaced by a file".into()); }
    }
    Ok(path)
}

fn scan_directory(path: &Path, relative: &Path, budget: usize, stop: &AtomicBool) -> Listing {
    let mut listing = Listing::default();
    let iterator = match std::fs::read_dir(path) {
        Ok(iterator) => iterator,
        Err(error) => { listing.note = Some(format!("Cannot read folder: {error}")); return listing; },
    };
    let limit = budget.min(MAX_DIRECTORY_ENTRIES);
    for (examined, entry) in iterator.enumerate() {
        if stop.load(Ordering::Relaxed) { break; }
        // Bound iteration too, including entries that cannot be inspected.
        if examined >= MAX_DIRECTORY_ENTRIES + 1 || listing.entries.len() >= limit {
            listing.note = Some(if budget < MAX_DIRECTORY_ENTRIES {
                format!("Project listing limit reached ({MAX_TOTAL_ENTRIES} entries); close another folder")
            } else { format!("Folder listing limited to {MAX_DIRECTORY_ENTRIES} entries") });
            break;
        }
        let entry = match entry { Ok(entry) => entry, Err(error) => { listing.note = Some(format!("Partial listing: {error}")); continue; } };
        if entry.file_name() == ".git" { continue; }
        let file_type = match entry.file_type() { Ok(kind) => kind, Err(error) => { listing.note = Some(format!("Partial listing: {error}")); continue; } };
        let kind = if file_type.is_symlink() { EntryKind::Symlink }
            else if file_type.is_dir() { EntryKind::Directory }
            else if file_type.is_file() { EntryKind::File } else { continue; };
        listing.entries.push(Entry {
            path: relative.join(entry.file_name()), name: entry.file_name().to_string_lossy().into_owned(), kind,
        });
    }
    listing.entries.sort_by(|a, b| {
        (a.kind != EntryKind::Directory).cmp(&(b.kind != EntryKind::Directory))
            .then_with(|| a.name.cmp(&b.name)).then_with(|| a.path.cmp(&b.path))
    });
    listing
}

fn scan(request: &Request, stop: &AtomicBool) -> Snapshot {
    let mut snapshot = Snapshot { generation: request.generation, project: request.project.clone(), ..Default::default() };
    let root = match std::fs::canonicalize(&request.project) {
        Ok(root) => root,
        Err(error) => {
            snapshot.directories.insert(PathBuf::new(), Listing { note: Some(format!("Cannot open project: {error}")), ..Default::default() });
            return snapshot;
        },
    };
    let mut paths: Vec<_> = request.expanded.iter().filter(|p| visible_directory(p, &request.expanded)).cloned().collect();
    if !paths.iter().any(|p| p.as_os_str().is_empty()) { paths.push(PathBuf::new()); }
    paths.sort_by_key(|p| (p.components().count(), p.clone()));
    let mut budget = MAX_TOTAL_ENTRIES;
    for path in paths.into_iter().take(MAX_DIRS) {
        if stop.load(Ordering::Relaxed) { break; }
        let listing = match directory_path(&root, &path) {
            Ok(absolute) => scan_directory(&absolute, &path, budget, stop),
            Err(error) => Listing { note: Some(error), ..Default::default() },
        };
        budget = budget.saturating_sub(listing.entries.len());
        snapshot.directories.insert(path, listing);
    }
    snapshot
}

fn run_worker(rx: Receiver<Arc<Request>>, tx: SyncSender<Arc<Snapshot>>, stop: Arc<AtomicBool>) {
    let mut request: Option<Arc<Request>> = None;
    let mut last: Option<Arc<Snapshot>> = None;
    let mut pending = None;
    let mut next_scan = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(mut value) => {
                while let Ok(newer) = rx.try_recv() { value = newer; }
                request = Some(value); next_scan = Instant::now();
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {},
        }
        if Instant::now() >= next_scan {
            if let Some(request) = &request {
                let snapshot = Arc::new(scan(request, &stop));
                if last.as_ref().is_none_or(|previous| **previous != *snapshot) {
                    last = Some(snapshot.clone()); pending = Some(snapshot);
                }
            }
            next_scan = Instant::now() + SCAN_INTERVAL;
        }
        if let Some(snapshot) = pending.take() {
            match tx.try_send(snapshot) {
                Ok(()) => SignalToUI::set_ui_signal(),
                Err(TrySendError::Full(snapshot)) => pending = Some(snapshot),
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("studio-tree-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            std::fs::create_dir_all(&path).unwrap(); Self(path)
        }
        fn request(&self, expanded: &[&str]) -> Request {
            Request { generation: 1, project: self.0.clone(), expanded: expanded.iter().map(PathBuf::from).collect() }
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn nested_project_is_lazy_sorted_and_keeps_dotfiles() {
        let temp = Temp::new();
        for path in ["src/nested", "assets", ".git/objects"] { std::fs::create_dir_all(temp.0.join(path)).unwrap(); }
        for path in ["src/main.rs", "src/nested/deep.rs", "z.rs", ".gitignore", "a.rs"] { std::fs::write(temp.0.join(path), "code").unwrap(); }
        let stop = AtomicBool::new(false);
        let first = scan(&temp.request(&[""]), &stop);
        assert_eq!(first.directories.len(), 1);
        let names: Vec<_> = first.directories[Path::new("")].entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["assets", "src", ".gitignore", "a.rs", "z.rs"]);
        let nested = scan(&temp.request(&["", "src", "src/nested"]), &stop);
        assert!(nested.directories[Path::new("src/nested")].entries.iter().any(|e| e.path == Path::new("src/nested/deep.rs")));
        let collapsed = scan(&temp.request(&["", "src/nested"]), &stop);
        assert_eq!(collapsed.directories.len(), 1);
    }

    #[test]
    fn refresh_reports_added_deleted_and_unreadable_directories() {
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join("src")).unwrap();
        std::fs::write(temp.0.join("src/old.rs"), "old").unwrap();
        let request = temp.request(&["", "src"]);
        let stop = AtomicBool::new(false);
        let before = scan(&request, &stop);
        let stable = path_id(&temp.0, Path::new("src"));
        std::fs::remove_file(temp.0.join("src/old.rs")).unwrap();
        std::fs::write(temp.0.join("src/new.rs"), "new").unwrap();
        let after = scan(&request, &stop);
        assert_ne!(before, after);
        assert_eq!(after.directories[Path::new("src")].entries[0].name, "new.rs");
        assert_eq!(path_id(&temp.0, Path::new("src")), stable);
        std::fs::remove_dir_all(temp.0.join("src")).unwrap();
        assert!(scan(&request, &stop).directories[Path::new("src")].note.is_some());
    }

    #[test]
    fn listing_and_total_budget_are_bounded_and_invalid_paths_rejected() {
        let temp = Temp::new();
        for index in 0..MAX_DIRECTORY_ENTRIES + 2 { std::fs::write(temp.0.join(format!("file-{index:04}")), "").unwrap(); }
        let listing = scan_directory(&temp.0, Path::new(""), MAX_TOTAL_ENTRIES, &AtomicBool::new(false));
        assert_eq!(listing.entries.len(), MAX_DIRECTORY_ENTRIES);
        assert!(listing.note.unwrap().contains("limited"));
        let listing = scan_directory(&temp.0, Path::new(""), 3, &AtomicBool::new(false));
        assert_eq!(listing.entries.len(), 3);
        assert!(listing.note.unwrap().contains("Project listing limit"));
        for path in ["../outside", "/absolute", ".git/objects"] { assert!(directory_path(&temp.0, Path::new(path)).is_err()); }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_folders_remain_leaves_and_cannot_escape_or_loop() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join("src")).unwrap();
        symlink(&temp.0, temp.0.join("src/back")).unwrap();
        symlink("/", temp.0.join("outside")).unwrap();
        let result = scan(&temp.request(&["", "src", "src/back", "outside"]), &AtomicBool::new(false));
        let outside = result.directories[Path::new("")].entries.iter().find(|e| e.name == "outside").unwrap();
        assert_eq!(outside.kind, EntryKind::Symlink);
        assert!(result.directories[Path::new("outside")].entries.is_empty());
        assert!(result.directories[Path::new("outside")].note.as_ref().unwrap().contains("Symbolic-link"));
        assert!(result.directories[Path::new("src/back")].entries.is_empty());
    }
}
