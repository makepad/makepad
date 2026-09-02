//! A closed, deterministic fake filesystem for the native and web demos.
//!
//! `--demo` (or `MAKEPAD_FILES_DEMO=1`, see [`crate::vfs::demo_requested`]) points
//! the browser at [`DemoVfs`] instead of the real disk, so a recording can
//! show `files` doing real work — thumbnails, Space preview, rename, copy,
//! the treemap, undo — without a single one of the user's own files ever
//! appearing on screen. Every operation genuinely mutates the in-memory tree;
//! thumbnails are supplied separately from embedded, repo-owned images.
//!
//! The tree is built once, deterministically — a seeded PRNG, never the
//! clock — so two runs (and two recordings) show byte-identical sizes and
//! dates. Everything after that lives behind a [`Mutex`], because the
//! [`Vfs`] trait hands out `&self`: an in-memory filesystem still needs
//! interior mutability to survive a rename.

use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, atomic::Ordering, Mutex},
};

use crate::{
    model::{self, FileEntry, SortSpec},
    ops::{OpKind, OpRequest, Undo},
    treemap::{Node, ScanProgress},
    vfs::{outcome_message, OpOutcome, Vfs},
};

/// The demo's home. Rooted somewhere that cannot be mistaken for a real
/// path and reads cleanly in the breadcrumb — `/Demo`, `/Demo/Documents`,
/// and so on.
const VIRTUAL_HOME: &str = "/Demo";

/// Where a trashed demo file goes; a plain hidden folder under the virtual
/// home, exactly the way `~/.Trash` sits under a real one.
const TRASH_NAME: &str = ".Trash";

/// The anchor "now" every seeded date is measured back from. A fixed
/// constant, not [`std::time::SystemTime::now`] — that is what keeps the
/// tree byte-identical across runs instead of drifting a little further
/// from "today" every time someone records a demo. (2026-08-27 00:00:00
/// UTC, chosen simply because it postdates every asset this module reads.)
const DEMO_NOW_SECS: u64 = 1_787_788_800;

/// Modified times are spread somewhere in this window before [`DEMO_NOW_SECS`].
const TWO_YEARS_SECS: u64 = 63_072_000;

/// A file's created time sits at most this far before its modified time.
const THIRTY_DAYS_SECS: u64 = 2_592_000;

/// The PRNG's seed. Any nonzero constant works; this one has no meaning
/// beyond "not zero, not a round number that looks like a bug".
const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

// ---------------------------------------------------------------------
// A tiny, deterministic PRNG
// ---------------------------------------------------------------------

/// xorshift64* — plenty of spread for sizes and dates, and small enough not
/// to be worth a `rand` dependency for a module whose only requirement is
/// "the same numbers every time".
struct Rng(u64);

impl Rng {
    /// `seed` is forced odd: xorshift's state never leaves zero once it
    /// gets there, so a zero (or even, which can shift down to zero) seed
    /// would make every "random" number the same number.
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A value in `[lo, hi)`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo)
    }
}

/// A modified/created pair somewhere in the last two years, never in the
/// future and never zero (zero reads as "unknown" everywhere this app
/// formats a timestamp, which a seeded file must never claim to be).
fn seeded_age(rng: &mut Rng) -> (u64, u64) {
    let modified = DEMO_NOW_SECS - rng.range(0, TWO_YEARS_SECS);
    let created = modified
        .saturating_sub(rng.range(0, THIRTY_DAYS_SECS))
        .max(DEMO_NOW_SECS - TWO_YEARS_SECS);
    (modified, created)
}

// ---------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------

/// One node in the closed tree. Paths are rebuilt while walking so moving a
/// subtree never requires rewriting thousands of descendants.
#[derive(Clone, Debug, PartialEq, Eq)]
struct VNode {
    name: String,
    is_dir: bool,
    /// A file's own size; always `0` for a folder — a folder's size is the
    /// fold of its children, computed by whoever needs it, exactly the way
    /// [`FileEntry::size`] is `0` for a directory too.
    size: u64,
    modified_secs: u64,
    created_secs: u64,
    children: Vec<VNode>,
}

fn folder_at(name: impl Into<String>, modified_secs: u64, created_secs: u64, children: Vec<VNode>) -> VNode {
    VNode {
        name: name.into(),
        is_dir: true,
        size: 0,
        modified_secs,
        created_secs,
        children,
    }
}

fn file_at(name: impl Into<String>, size: u64, modified_secs: u64, created_secs: u64) -> VNode {
    VNode {
        name: name.into(),
        is_dir: false,
        size,
        modified_secs,
        created_secs,
        children: Vec::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Profile {
    Pictures,
    Projects,
    Library,
    Mail,
    Music,
    Documents,
    Videos,
    Downloads,
    Desktop,
    Network,
    Trash,
}

impl Profile {
    fn folder_name(self, index: usize) -> String {
        match self {
            Profile::Pictures => format!("archive-{index:04}"),
            Profile::Projects => format!("package-{index:04}"),
            Profile::Library => format!("cache-shard-{index:04}"),
            Profile::Mail => format!("mailbox-{index:04}"),
            Profile::Music => format!("Artist {index:04}"),
            Profile::Documents => format!("Archive-{index:04}"),
            Profile::Videos => format!("Clips-{index:03}"),
            Profile::Downloads => format!("download-batch-{index:03}"),
            Profile::Desktop => format!("Workspace {index:03}"),
            Profile::Network => format!("shared-{index:03}"),
            Profile::Trash => format!("deleted-{index:03}"),
        }
    }

    fn file_name(self, index: usize) -> String {
        match self {
            Profile::Pictures => {
                if index % 17 == 0 { format!("DSC_{index:05}.ARW") } else { format!("IMG_{index:05}.jpg") }
            }
            Profile::Projects => match index % 8 {
                0 => format!("module_{index:05}.rs"),
                1 => format!("index_{index:05}.js"),
                2 => format!("package-{index:05}.json"),
                3 => format!("types_{index:05}.ts"),
                4 => format!("README-{index:05}.md"),
                5 => format!("Cargo-{index:05}.toml"),
                6 => format!("shader_{index:05}.wgsl"),
                _ => format!("config_{index:05}.json"),
            },
            Profile::Library => format!("blob-{index:06}.cache"),
            Profile::Mail => format!("message-{index:06}.eml"),
            Profile::Music => {
                if index % 23 == 0 { format!("{index:02} Lossless.flac") } else { format!("{index:02} Track.mp3") }
            }
            Profile::Documents => match index % 8 {
                0 => format!("report-{index:05}.pdf"),
                1 => format!("letter-{index:05}.docx"),
                2 => format!("ledger-{index:05}.xlsx"),
                3 => format!("export-{index:05}.csv"),
                4 => format!("slides-{index:05}.pptx"),
                5 => format!("receipt-{index:05}.pdf"),
                6 => format!("notes-{index:05}.md"),
                _ => format!("outline-{index:05}.txt"),
            },
            Profile::Videos => {
                if index % 2 == 0 { format!("clip-{index:04}.mp4") } else { format!("camera-{index:04}.mkv") }
            }
            Profile::Downloads => match index % 8 {
                0 => format!("installer-{index:04}.dmg"),
                1 => format!("linux-{index:04}.iso"),
                2 => format!("assets-{index:04}.zip"),
                3 => format!("setup-{index:04}.pkg"),
                4 => format!("partial-{index:04}.download"),
                5 => format!("screenshot-{index:04}.png"),
                6 => format!("manual-{index:04}.pdf"),
                _ => format!("export-{index:04}.csv"),
            },
            Profile::Desktop => format!("desktop-note-{index:04}.md"),
            Profile::Network => format!("team-file-{index:04}.pdf"),
            Profile::Trash => if index % 2 == 0 { format!("old-export-{index:03}.zip") } else { format!("recording-{index:03}.mkv") },
        }
    }

    fn size(self, index: usize, rng: &mut Rng) -> u64 {
        const MIB: u64 = 1024 * 1024;
        const GIB: u64 = 1024 * MIB;
        match self {
            Profile::Pictures if index % 17 == 0 => rng.range(25 * MIB, 40 * MIB),
            Profile::Pictures => rng.range(2 * MIB, 9 * MIB),
            Profile::Projects if index % 97 == 0 => rng.range(MIB, 3 * MIB),
            Profile::Projects => rng.range(1024, 40 * 1024),
            Profile::Library => rng.range(128 * 1024, 12 * MIB),
            Profile::Mail => rng.range(4 * 1024, 180 * 1024),
            Profile::Music if index % 23 == 0 => rng.range(25 * MIB, 60 * MIB),
            Profile::Music => rng.range(3 * MIB, 12 * MIB),
            Profile::Documents => rng.range(8 * 1024, 18 * MIB),
            Profile::Videos if index < 5 => rng.range(2 * GIB, 6 * GIB),
            Profile::Videos if index < 85 => rng.range(100 * MIB, 900 * MIB),
            Profile::Videos => rng.range(8 * MIB, 100 * MIB),
            Profile::Downloads if index < 40 => rng.range(80 * MIB, 4 * GIB),
            Profile::Downloads => rng.range(64 * 1024, 160 * MIB),
            Profile::Desktop => rng.range(1024, 20 * MIB),
            Profile::Network => rng.range(32 * 1024, 50 * MIB),
            Profile::Trash if index < 5 => rng.range(500 * MIB, 3 * GIB),
            Profile::Trash => rng.range(MIB, 200 * MIB),
        }
    }
}

struct TempFolder {
    name: String,
    depth: usize,
    modified_secs: u64,
    created_secs: u64,
    children: Vec<usize>,
    files: Vec<VNode>,
}

fn add_folder(folders: &mut Vec<TempFolder>, parent: usize, name: String, rng: &mut Rng) -> usize {
    let (modified_secs, created_secs) = seeded_age(rng);
    let index = folders.len();
    let depth = folders[parent].depth + 1;
    folders.push(TempFolder { name, depth, modified_secs, created_secs, children: Vec::new(), files: Vec::new() });
    folders[parent].children.push(index);
    index
}

fn add_path(folders: &mut Vec<TempFolder>, path: &[&str], rng: &mut Rng) -> usize {
    let mut parent = 0;
    for name in path {
        let found = folders[parent]
            .children
            .iter()
            .copied()
            .find(|&child| folders[child].name == *name);
        parent = found.unwrap_or_else(|| add_folder(folders, parent, (*name).to_string(), rng));
    }
    parent
}

fn materialize(index: usize, folders: &mut [Option<TempFolder>]) -> VNode {
    let temp = folders[index].take().expect("folder is materialized once");
    let mut children = Vec::with_capacity(temp.children.len() + temp.files.len());
    for child in temp.children {
        children.push(materialize(child, folders));
    }
    children.extend(temp.files);
    folder_at(temp.name, temp.modified_secs, temp.created_secs, children)
}

/// Build one bushy category with bounded listings. `folder_count` includes
/// the category root; the few supplied paths create the intentionally deep
/// branches before the remaining folders are spread four-wide.
fn build_category(
    name: &str,
    folder_count: usize,
    file_count: usize,
    max_depth: usize,
    profile: Profile,
    special_paths: &[&[&str]],
    rng: &mut Rng,
) -> VNode {
    let (modified_secs, created_secs) = seeded_age(rng);
    let mut folders = vec![TempFolder {
        name: name.to_string(),
        depth: 0,
        modified_secs,
        created_secs,
        children: Vec::new(),
        files: Vec::new(),
    }];
    for path in special_paths {
        add_path(&mut folders, path, rng);
    }
    if profile == Profile::Pictures {
        const TRIPS: [&str; 4] = ["Lisbon", "Kyoto", "Reykjavik", "Dolomites"];
        for year in 2023..=2026 {
            let year = year.to_string();
            for month in 1..=12 {
                for trip in 0..4 {
                    let trip = format!("{month:02}-{}-{trip}", TRIPS[(month + trip) % TRIPS.len()]);
                    add_path(&mut folders, &[year.as_str(), trip.as_str()], rng);
                }
            }
        }
    }
    if profile == Profile::Projects {
        let node_modules = add_path(&mut folders, &["web-dashboard", "node_modules"], rng);
        for package in 0..120 {
            add_folder(&mut folders, node_modules, format!("dependency-{package:03}"), rng);
        }
    }

    let mut parent = 0usize;
    while folders.len() < folder_count {
        while folders[parent].depth >= max_depth || folders[parent].children.len() >= 4 {
            parent += 1;
        }
        let index = folders.len();
        add_folder(&mut folders, parent, profile.folder_name(index), rng);
    }

    let base = file_count / folders.len();
    let extra = file_count % folders.len();
    let mut file_index = 0usize;
    for folder_index in 0..folders.len() {
        let count = base + usize::from(folder_index < extra);
        let folder_modified = folders[folder_index].modified_secs;
        folders[folder_index].files.reserve(count);
        for _ in 0..count {
            let modified_secs = (folder_modified + rng.range(0, 7 * 86_400)).min(DEMO_NOW_SECS);
            let created_secs = modified_secs
                .saturating_sub(rng.range(0, THIRTY_DAYS_SECS))
                .max(DEMO_NOW_SECS - TWO_YEARS_SECS);
            folders[folder_index].files.push(file_at(
                profile.file_name(file_index),
                profile.size(file_index, rng),
                modified_secs,
                created_secs,
            ));
            file_index += 1;
        }
    }
    let mut folders: Vec<Option<TempFolder>> = folders.into_iter().map(Some).collect();
    let mut root = materialize(0, &mut folders);
    if profile == Profile::Projects {
        let node_modules = descendant_mut(&mut root, &["web-dashboard", "node_modules"])
            .expect("the web project has node_modules");
        let target = 950 * 1024 * 1024;
        let current = sum_bytes_unchecked(node_modules);
        if current < target {
            let (modified, created) = seeded_age(rng);
            node_modules.children.push(file_at(".vite-dependency-cache.bin", target - current, modified, created));
        }
    }
    root
}

fn descendant_mut<'a>(mut node: &'a mut VNode, path: &[&str]) -> Option<&'a mut VNode> {
    for name in path {
        node = node.children.iter_mut().find(|child| child.is_dir && child.name == *name)?;
    }
    Some(node)
}

fn push_featured_file(folder: &mut VNode, name: &str, size: u64, rng: &mut Rng) {
    let (modified, created) = seeded_age(rng);
    folder.children.push(file_at(name, size, modified, created));
}

/// 38,000 files in 2,026 folders. The category ratios keep ordinary listings
/// near twenty entries while a scan of Home sees the whole varied tree.
fn build_root_with_seed(seed: u64) -> VNode {
    let mut rng = Rng::new(seed);
    let mut pictures = build_category(
        "Pictures", 350, 8_000, 6, Profile::Pictures,
        &[&["2024", "07-Lisbon"], &["2025", "11-Kyoto"], &["wallpapers"], &["screenshots", "2026", "08"]],
        &mut rng,
    );
    push_featured_file(&mut pictures, "wallpaper-sunrise.jpg", 6 * 1024 * 1024, &mut rng);

    let mut projects = build_category(
        "Projects", 650, 12_500, 10, Profile::Projects,
        &[
            &["atlas", "crates", "render", "src", "passes", "shadow", "cascade", "partition", "cache"],
            &["web-dashboard", "node_modules", "@makepad", "renderer", "node_modules", "tiny-color"],
            &[
                "orbit", "src", "platform", "web", "runtime", "renderer", "cache", "shaders",
                "compiled",
            ],
        ],
        &mut rng,
    );
    push_featured_file(&mut projects, "README.md", 6_000, &mut rng);

    let library = build_category(
        "Library", 400, 7_000, 8, Profile::Library,
        &[
            &[
                "Caches",
                "com.makepad.studio",
                "versions",
                "v12",
                "data",
                "blobs",
                "segments",
                "compiled",
                "chunks",
            ],
            &["Application Support", "Browser", "CacheStorage"],
        ],
        &mut rng,
    );
    let mail = build_category(
        "Mail",
        200,
        3_500,
        9,
        Profile::Mail,
        &[&[
            "Accounts",
            "Personal",
            "Archive",
            "2025",
            "Receipts",
            "Travel",
            "Thread Data",
            "Attachments",
            "Inline",
        ]],
        &mut rng,
    );
    let music = build_category("Music", 180, 3_200, 5, Profile::Music, &[&["Aurora Lines", "Midnight Hours"], &["Northbound", "Lossless Sessions"]], &mut rng);
    let mut documents = build_category(
        "Documents", 100, 1_600, 6, Profile::Documents,
        &[&["Archive", "2024", "Taxes"], &["Scanned Receipts", "2025", "Q4"]],
        &mut rng,
    );
    push_featured_file(&mut documents, "notes.md", 4_096, &mut rng);
    push_featured_file(&mut documents, "budget.csv", 18_432, &mut rng);
    push_featured_file(&mut documents, "contacts.csv", 12_288, &mut rng);
    let videos = build_category("Videos", 25, 500, 4, Profile::Videos, &[&["Camera Uploads", "2025"], &["Edits", "Final"]], &mut rng);
    let downloads = build_category("Downloads", 50, 1_000, 4, Profile::Downloads, &[&["Installers"], &["Unsorted"]], &mut rng);
    let desktop = build_category("Desktop", 30, 400, 4, Profile::Desktop, &[&["Current Work"]], &mut rng);
    let network = build_category("Network", 30, 250, 4, Profile::Network, &[&["shared", "Design Team"], &["shared", "Engineering"]], &mut rng);
    let trash = build_category(TRASH_NAME, 10, 50, 3, Profile::Trash, &[&["Old Downloads"]], &mut rng);
    let (modified, created) = seeded_age(&mut rng);
    folder_at(
        "Demo",
        modified,
        created,
        vec![desktop, documents, downloads, library, mail, music, network, pictures, projects, videos, trash],
    )
}

fn build_root() -> VNode {
    build_root_with_seed(SEED)
}

// ---------------------------------------------------------------------
// Tree lookups and edits
// ---------------------------------------------------------------------

/// The node at `path`, or `None` when `path` is not under [`VIRTUAL_HOME`]
/// or does not exist in the tree — the same "just doesn't resolve" outcome
/// either way, since nothing this module does treats them differently.
fn resolve<'a>(root: &'a VNode, path: &Path) -> Option<&'a VNode> {
    let home = Path::new(VIRTUAL_HOME);
    if path == home {
        return Some(root);
    }
    let rel = path.strip_prefix(home).ok()?;
    let mut node = root;
    for component in rel.components() {
        let std::path::Component::Normal(part) = component else { return None };
        let name = part.to_string_lossy();
        node = node.children.iter().find(|c| c.name == name)?;
    }
    Some(node)
}

/// The mutable twin of [`resolve`].
fn resolve_mut<'a>(root: &'a mut VNode, path: &Path) -> Option<&'a mut VNode> {
    let home = Path::new(VIRTUAL_HOME);
    if path == home {
        return Some(root);
    }
    let rel = path.strip_prefix(home).ok()?;
    let mut node = root;
    for component in rel.components() {
        let std::path::Component::Normal(part) = component else { return None };
        let name = part.to_string_lossy().into_owned();
        node = node.children.iter_mut().find(|c| c.name == name)?;
    }
    Some(node)
}

/// `path` split into its parent folder and its own name — `None` for a
/// path with neither (the root, or something not path-shaped at all).
fn split_path(path: &Path) -> Option<(PathBuf, String)> {
    let parent = path.parent()?.to_path_buf();
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((parent, name))
}

/// Remove and return the child named `name`, or `None` when there is no
/// such child.
fn take_child(parent: &mut VNode, name: &str) -> Option<VNode> {
    let index = parent.children.iter().position(|c| c.name == name)?;
    Some(parent.children.remove(index))
}

/// Byte total of a subtree: a file's own size, or the recursive fold of a
/// folder's children — never the folder's own (always-zero) `size` field.
/// Bails out with whatever it has already added up once `cancel` is
/// raised, matching [`crate::ops::total_bytes`]'s contract.
fn sum_bytes(node: &VNode, cancel: &AtomicBool) -> u64 {
    if !node.is_dir {
        return node.size;
    }
    let mut total = 0u64;
    for child in &node.children {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        total += sum_bytes(child, cancel);
    }
    total
}

fn sum_bytes_unchecked(node: &VNode) -> u64 {
    if node.is_dir {
        node.children.iter().map(sum_bytes_unchecked).sum()
    } else {
        node.size
    }
}

fn file_entry(path: PathBuf, node: &VNode) -> FileEntry {
    FileEntry {
        kind: model::kind_for(&path, node.is_dir),
        name: node.name.clone(),
        is_dir: node.is_dir,
        size: sum_bytes_unchecked(node),
        modified_secs: node.modified_secs,
        created_secs: node.created_secs,
        permissions: if node.is_dir { "rwxr-xr-x".to_string() } else { "rw-r--r--".to_string() },
        child_count: node.is_dir.then(|| node.children.len() as u32),
        path,
    }
}

/// `name` split the way [`unique_name`] needs it: a dotfile or an
/// extensionless name reports no extension, which is the signal to put the
/// disambiguating suffix at the very end instead of splicing it into the
/// name's only dot. Mirrors `ops::split_stem_ext` exactly (that one works
/// against the disk, this one against the tree — see the module doc
/// comment on why `ops.rs` isn't reused here).
fn split_stem_ext(name: &str) -> (String, String) {
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) => (stem.to_string_lossy().into_owned(), ext.to_string_lossy().into_owned()),
        _ => (name.to_string(), String::new()),
    }
}

/// A name for `name` that does not collide with any of `siblings`: "report
/// (2).txt", then "report (3).txt", exactly the way [`crate::ops::unique_path`]
/// disambiguates a real copy on disk — just checked against a folder's
/// children instead of `Path::exists`.
fn unique_name(siblings: &[VNode], name: &str) -> String {
    if !siblings.iter().any(|c| c.name == name) {
        return name.to_string();
    }
    let (stem, ext) = split_stem_ext(name);
    let mut n: u64 = 2;
    loop {
        let candidate = if ext.is_empty() { format!("{name} ({n})") } else { format!("{stem} ({n}).{ext}") };
        if !siblings.iter().any(|c| c.name == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Refuses a copy/move whose destination is one of the sources or sits
/// inside one of them — mirrors `ops::refuse_into_self`'s rule, just
/// without needing `canonicalize` (there are no symlinks, and no two
/// virtual paths ever alias the same node).
fn refuse_into_self(sources: &[PathBuf], dest_dir: &Path) -> Option<String> {
    for source in sources {
        if dest_dir == source.as_path() || dest_dir.starts_with(source) {
            return Some(format!("Can't copy or move \"{}\" into itself", model::display_name(source)));
        }
    }
    None
}

// ---------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------

fn perform_rename(tree: &mut VNode, request: &OpRequest) -> Result<OpOutcome, String> {
    let old_path = request.sources.first().ok_or_else(|| "Rename needs a source".to_string())?;
    let new_name = request.new_name.as_deref().ok_or_else(|| "Rename needs a new name".to_string())?;
    let old_name = old_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("Can't rename {}", old_path.display()))?;
    let new_path = request.dest_dir.join(new_name);

    let parent = resolve_mut(tree, &request.dest_dir)
        .ok_or_else(|| format!("No such folder: {}", request.dest_dir.display()))?;
    if &new_path != old_path && parent.children.iter().any(|c| c.name == new_name) {
        return Err(format!("\"{new_name}\" already exists"));
    }
    let node = parent
        .children
        .iter_mut()
        .find(|c| c.name == old_name)
        .ok_or_else(|| format!("No such file: {}", old_path.display()))?;
    node.name = new_name.to_string();

    Ok(OpOutcome {
        message: format!("Renamed to \"{new_name}\""),
        undo: Some(Undo::Moved { pairs: vec![(old_path.clone(), new_path.clone())] }),
        touched: vec![new_path],
    })
}

fn perform_new_folder(tree: &mut VNode, request: &OpRequest) -> Result<OpOutcome, String> {
    let dest = resolve_mut(tree, &request.dest_dir)
        .ok_or_else(|| format!("No such folder: {}", request.dest_dir.display()))?;
    if !dest.is_dir {
        return Err(format!("{} is not a folder", request.dest_dir.display()));
    }
    let requested = request.new_name.as_deref().unwrap_or("New Folder");
    let name = unique_name(&dest.children, requested);
    dest.children.push(folder_at(name.clone(), DEMO_NOW_SECS, DEMO_NOW_SECS, Vec::new()));
    let path = request.dest_dir.join(&name);

    Ok(OpOutcome {
        message: outcome_message(OpKind::NewFolder, 1, &path),
        undo: Some(Undo::Created { paths: vec![path.clone()] }),
        touched: vec![path],
    })
}

fn perform_copy(tree: &mut VNode, request: &OpRequest) -> Result<OpOutcome, String> {
    if let Some(message) = refuse_into_self(&request.sources, &request.dest_dir) {
        return Err(message);
    }
    let mut touched = Vec::new();
    for source in &request.sources {
        let (parent_path, name) = split_path(source).ok_or_else(|| format!("Can't copy {}", source.display()))?;
        let cloned: VNode = {
            let parent = resolve(tree, &parent_path).ok_or_else(|| format!("No such folder: {}", parent_path.display()))?;
            parent
                .children
                .iter()
                .find(|c| c.name == name)
                .ok_or_else(|| format!("No such file: {}", source.display()))?
                .clone()
        };
        let dest = resolve_mut(tree, &request.dest_dir)
            .ok_or_else(|| format!("No such folder: {}", request.dest_dir.display()))?;
        let unique = unique_name(&dest.children, &name);
        let mut item = cloned;
        item.name = unique.clone();
        dest.children.push(item);
        touched.push(request.dest_dir.join(&unique));
    }

    Ok(OpOutcome {
        message: outcome_message(OpKind::Copy, touched.len(), &request.dest_dir),
        undo: Some(Undo::Created { paths: touched.clone() }),
        touched,
    })
}

fn perform_move(tree: &mut VNode, request: &OpRequest) -> Result<OpOutcome, String> {
    if let Some(message) = refuse_into_self(&request.sources, &request.dest_dir) {
        return Err(message);
    }
    {
        let dest = resolve(tree, &request.dest_dir)
            .ok_or_else(|| format!("No such folder: {}", request.dest_dir.display()))?;
        if !dest.is_dir {
            return Err(format!("{} is not a folder", request.dest_dir.display()));
        }
    }

    let mut moved_pairs = Vec::new();
    let mut touched = Vec::new();
    let mut skipped = 0usize;
    for source in &request.sources {
        // A cut-and-paste back onto the folder it came from is a no-op,
        // not a move that happens to land where it started — same rule as
        // `ops::already_there`.
        if source.parent() == Some(request.dest_dir.as_path()) {
            skipped += 1;
            touched.push(source.clone());
            continue;
        }
        let (parent_path, name) = split_path(source).ok_or_else(|| format!("Can't move {}", source.display()))?;
        let node = {
            let parent = resolve_mut(tree, &parent_path)
                .ok_or_else(|| format!("No such folder: {}", parent_path.display()))?;
            take_child(parent, &name).ok_or_else(|| format!("No such file: {}", source.display()))?
        };
        let dest = resolve_mut(tree, &request.dest_dir)
            .ok_or_else(|| format!("No such folder: {}", request.dest_dir.display()))?;
        let unique = unique_name(&dest.children, &name);
        let mut item = node;
        item.name = unique.clone();
        dest.children.push(item);
        let target = request.dest_dir.join(&unique);
        moved_pairs.push((source.clone(), target.clone()));
        touched.push(target);
    }

    if moved_pairs.is_empty() && skipped > 0 {
        return Ok(OpOutcome { message: "Nothing to move — already there".to_string(), undo: None, touched });
    }
    let message = if skipped > 0 {
        format!("Moved {} item(s) ({} already there)", moved_pairs.len(), skipped)
    } else {
        outcome_message(OpKind::Move, moved_pairs.len(), &request.dest_dir)
    };
    Ok(OpOutcome { message, undo: Some(Undo::Moved { pairs: moved_pairs }), touched })
}

fn perform_trash(tree: &mut VNode, request: &OpRequest) -> Result<OpOutcome, String> {
    let trash_path = Path::new(VIRTUAL_HOME).join(TRASH_NAME);
    let mut pairs = Vec::new();
    let mut touched = Vec::new();
    for source in &request.sources {
        let (parent_path, name) = split_path(source).ok_or_else(|| format!("Can't trash {}", source.display()))?;
        let node = {
            let parent = resolve_mut(tree, &parent_path)
                .ok_or_else(|| format!("No such folder: {}", parent_path.display()))?;
            take_child(parent, &name).ok_or_else(|| format!("No such file: {}", source.display()))?
        };
        // Seeded at construction and never removed by any operation this
        // module supports, so the trash folder always exists here.
        let dest = resolve_mut(tree, &trash_path).expect("the demo trash always exists");
        let unique = unique_name(&dest.children, &name);
        let mut item = node;
        item.name = unique.clone();
        dest.children.push(item);
        let target = trash_path.join(&unique);
        pairs.push((source.clone(), target.clone()));
        touched.push(target);
    }

    Ok(OpOutcome {
        message: outcome_message(OpKind::Trash, pairs.len(), &trash_path),
        undo: Some(Undo::Moved { pairs }),
        touched,
    })
}

/// Erases every source outright — no undo, no trash behind it, per
/// `OpKind::Delete`'s contract.
fn perform_delete(tree: &mut VNode, request: &OpRequest) -> Result<OpOutcome, String> {
    let mut removed = 0usize;
    for source in &request.sources {
        let (parent_path, name) = split_path(source).ok_or_else(|| format!("Can't delete {}", source.display()))?;
        let parent =
            resolve_mut(tree, &parent_path).ok_or_else(|| format!("No such folder: {}", parent_path.display()))?;
        take_child(parent, &name).ok_or_else(|| format!("No such file: {}", source.display()))?;
        removed += 1;
    }
    Ok(OpOutcome {
        message: format!("Deleted {removed} item{} permanently", if removed == 1 { "" } else { "s" }),
        undo: None,
        touched: Vec::new(),
    })
}

fn undo_moved(tree: &mut VNode, pairs: &[(PathBuf, PathBuf)]) -> Result<OpOutcome, String> {
    let mut restored = Vec::new();
    for (from, to) in pairs {
        let (to_parent, to_name) = split_path(to).ok_or_else(|| format!("Can't undo move of {}", to.display()))?;
        let node = {
            let parent =
                resolve_mut(tree, &to_parent).ok_or_else(|| format!("No such folder: {}", to_parent.display()))?;
            take_child(parent, &to_name).ok_or_else(|| format!("Nothing to undo at {}", to.display()))?
        };
        let (from_parent, from_name) = split_path(from).ok_or_else(|| format!("Can't undo move to {}", from.display()))?;
        let dest = resolve_mut(tree, &from_parent)
            .ok_or_else(|| format!("No such folder: {}", from_parent.display()))?;
        let mut item = node;
        item.name = from_name;
        dest.children.push(item);
        restored.push(from.clone());
    }
    Ok(OpOutcome { message: format!("Undid move of {} item(s)", restored.len()), undo: None, touched: restored })
}

fn undo_created(tree: &mut VNode, paths: &[PathBuf]) -> Result<OpOutcome, String> {
    let mut removed = Vec::new();
    for path in paths {
        let (parent_path, name) = split_path(path).ok_or_else(|| format!("Can't undo creation of {}", path.display()))?;
        let parent =
            resolve_mut(tree, &parent_path).ok_or_else(|| format!("No such folder: {}", parent_path.display()))?;
        take_child(parent, &name).ok_or_else(|| format!("Nothing to undo at {}", path.display()))?;
        removed.push(path.clone());
    }
    Ok(OpOutcome { message: format!("Undid creation of {} item(s)", removed.len()), undo: None, touched: removed })
}

// ---------------------------------------------------------------------
// Scanning, for the treemap
// ---------------------------------------------------------------------

/// Entries visited between [`ScanProgress`] reports — the in-memory
/// equivalent of `treemap::PROGRESS_STRIDE`. The tree is tiny compared to a
/// real disk, so this mostly just guarantees the final report; it exists
/// so the demo's `scan` still honours the "bounded rate" half of the
/// contract rather than assuming a small tree makes it moot.
const SCAN_PROGRESS_STRIDE: u64 = 64;

fn scan_vnode(
    node: &VNode,
    path: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(ScanProgress),
    total: &mut ScanProgress,
    since_report: &mut u64,
) -> Option<Node> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let kind = model::kind_for(path, node.is_dir) as u8;
    let result = if node.is_dir {
        let mut children = Vec::with_capacity(node.children.len());
        let mut size = 0u64;
        for child in &node.children {
            let child_path = path.join(&child.name);
            let child_node = scan_vnode(child, &child_path, cancel, progress, total, since_report)?;
            size += child_node.size;
            children.push(child_node);
        }
        Node {
            files: children.iter().map(|c| c.files).sum(),
            modified: children.iter().map(|c| c.modified).max().unwrap_or(0),
            name: node.name.clone(),
            is_dir: true,
            done: true,
            denied: false,
            size,
            kind,
            children,
        }
    } else {
        total.files += 1;
        total.bytes += node.size;
        Node::file_at(node.name.clone(), kind, node.size, (node.modified_secs / 60) as u32)
    };
    // Reported at most once every `SCAN_PROGRESS_STRIDE` nodes (folders and
    // files both count), the same bounded-rate rule `treemap::scan` keeps —
    // a demo tree is small enough that this rarely fires before the final
    // report `Vfs::scan` sends once the whole walk is done.
    *since_report += 1;
    if *since_report >= SCAN_PROGRESS_STRIDE {
        *since_report = 0;
        progress(*total);
    }
    Some(result)
}

// ---------------------------------------------------------------------
// The Vfs
// ---------------------------------------------------------------------

/// The demo filesystem is entirely memory-backed and never resolves a
/// virtual path against the host.
pub struct DemoVfs {
    root: Mutex<VNode>,
}

impl DemoVfs {
    /// Build the full seeded tree immediately so every later operation is an
    /// in-memory lookup.
    pub fn new() -> Self {
        DemoVfs { root: Mutex::new(build_root()) }
    }
}

impl Default for DemoVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs for DemoVfs {
    fn home(&self) -> PathBuf {
        PathBuf::from(VIRTUAL_HOME)
    }

    fn now_secs(&self) -> u64 {
        DEMO_NOW_SECS
    }

    fn read_dir(&self, path: &Path, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
        let tree = self.root.lock().unwrap();
        let node = resolve(&tree, path).ok_or_else(|| format!("No such folder: {}", path.display()))?;
        if !node.is_dir {
            return Err(format!("{} is not a folder", path.display()));
        }
        let mut entries = Vec::new();
        for child in &node.children {
            // Same rule as `model::read_directory`: a name starting with a
            // dot (here, exactly `.Trash`) is hidden unless asked for.
            if !show_hidden && child.name.starts_with('.') {
                continue;
            }
            let child_path = path.join(&child.name);
            entries.push(file_entry(child_path, child));
        }
        let mut order: Vec<usize> = (0..entries.len()).collect();
        model::sort_indices(&entries, &mut order, SortSpec::default());
        Ok(order.into_iter().map(|i| entries[i].clone()).collect())
    }

    fn is_dir(&self, path: &Path) -> bool {
        let tree = self.root.lock().unwrap();
        resolve(&tree, path).is_some_and(|n| n.is_dir)
    }

    fn stat(&self, path: &Path) -> Result<FileEntry, String> {
        let tree = self.root.lock().unwrap();
        let node = resolve(&tree, path).ok_or_else(|| format!("No such file: {}", path.display()))?;
        Ok(file_entry(path.to_path_buf(), node))
    }

    fn read_bytes(&self, path: &Path, max: usize) -> Result<Vec<u8>, String> {
        let tree = self.root.lock().unwrap();
        let node = resolve(&tree, path).ok_or_else(|| format!("No such file: {}", path.display()))?;
        if node.is_dir {
            return Err(format!("{} is a folder", path.display()));
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "txt" | "md" | "rs" | "toml" | "json" | "csv" | "ts" | "js" | "wgsl" | "eml") {
            return Err(format!("{} has no text content in the demo", path.display()));
        }
        let name = node.name.as_str();
        let text = match ext.as_str() {
            "json" => format!("{{\n  \"file\": \"{name}\",\n  \"source\": \"files demo\",\n  \"generated\": true\n}}\n"),
            "csv" => format!("file,kind,status\n{name},synthetic,ready\nsummary.csv,demo,closed filesystem\n"),
            "rs" => format!("// Synthetic preview for {name}\npub fn demo_file() -> &'static str {{\n    \"files demo\"\n}}\n"),
            "md" => format!("# {name}\n\nThis is synthetic content from the closed files demo filesystem.\n\nNo host files were read.\n"),
            _ => format!("Synthetic preview for {name}\nGenerated by the closed files demo filesystem.\nNo host files were read.\n"),
        };
        let mut bytes = text.into_bytes();
        bytes.truncate(max);
        Ok(bytes)
    }

    fn total_bytes(&self, path: &Path, cancel: &AtomicBool) -> u64 {
        let tree = self.root.lock().unwrap();
        resolve(&tree, path).map(|node| sum_bytes(node, cancel)).unwrap_or(0)
    }

    fn scan(&self, root: &Path, cancel: &AtomicBool, progress: &dyn Fn(ScanProgress)) -> Option<Node> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let tree = self.root.lock().unwrap();
        let node = resolve(&tree, root)?;
        let mut total = ScanProgress::default();
        let mut since_report = 0u64;
        let result = scan_vnode(node, root, cancel, progress, &mut total, &mut since_report)?;
        // One last report so a caller that only reads the callback's
        // argument after the walk returns still sees the true final tally
        // — same guarantee `treemap::scan` makes.
        progress(total);
        Some(result)
    }

    fn perform(&self, request: &OpRequest) -> Result<OpOutcome, String> {
        let mut tree = self.root.lock().unwrap();
        match request.kind {
            OpKind::Rename => perform_rename(&mut tree, request),
            OpKind::NewFolder => perform_new_folder(&mut tree, request),
            OpKind::Copy => perform_copy(&mut tree, request),
            OpKind::Move => perform_move(&mut tree, request),
            OpKind::Trash => perform_trash(&mut tree, request),
            OpKind::Delete => perform_delete(&mut tree, request),
        }
    }

    fn perform_undo(&self, undo: &Undo) -> Result<OpOutcome, String> {
        let mut tree = self.root.lock().unwrap();
        match undo {
            Undo::Moved { pairs } => undo_moved(&mut tree, pairs),
            Undo::Created { paths } => undo_created(&mut tree, paths),
        }
    }

    fn is_instant(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(vfs: &DemoVfs, path: &str) -> Vec<FileEntry> {
        vfs.read_dir(Path::new(path), true).unwrap()
    }

    fn top_level_folders() -> [&'static str; 10] {
        ["Desktop", "Documents", "Downloads", "Library", "Mail", "Music", "Network", "Pictures", "Projects", "Videos"]
    }

    #[test]
    fn the_tree_is_deterministic() {
        assert_eq!(build_root_with_seed(SEED), build_root_with_seed(SEED));
    }

    #[test]
    fn no_timestamp_is_zero_or_in_the_future() {
        let vfs = DemoVfs::new();
        let mut stack = vec![PathBuf::from(VIRTUAL_HOME)];
        let mut files_checked = 0;
        while let Some(dir) = stack.pop() {
            for entry in listing(&vfs, dir.to_str().unwrap()) {
                if entry.is_dir {
                    stack.push(entry.path.clone());
                    continue;
                }
                files_checked += 1;
                assert_ne!(entry.modified_secs, 0, "{} has no modified time", entry.path.display());
                assert_ne!(entry.created_secs, 0, "{} has no created time", entry.path.display());
                assert!(entry.modified_secs <= DEMO_NOW_SECS, "{} is modified in the future", entry.path.display());
                assert!(entry.created_secs <= DEMO_NOW_SECS, "{} is created in the future", entry.path.display());
                assert!(entry.modified_secs >= DEMO_NOW_SECS - TWO_YEARS_SECS, "{} is older than the demo window", entry.path.display());
                assert!(entry.created_secs >= DEMO_NOW_SECS - TWO_YEARS_SECS, "{} was created before the demo window", entry.path.display());
            }
        }
        assert!((30_000..=45_000).contains(&files_checked), "file count out of range: {files_checked}");
    }

    #[test]
    fn read_dir_sorts_folders_first_then_by_name() {
        let vfs = DemoVfs::new();
        let entries = listing(&vfs, VIRTUAL_HOME);
        let first_file = entries.iter().position(|e| !e.is_dir);
        let last_folder = entries.iter().rposition(|e| e.is_dir);
        if let (Some(first_file), Some(last_folder)) = (first_file, last_folder) {
            assert!(last_folder < first_file, "a folder sorted after a file");
        }
        let names: Vec<&str> = entries.iter().filter(|e| e.is_dir).map(|e| e.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(names, sorted);
    }

    #[test]
    fn rename_works_collides_and_undoes() {
        let vfs = DemoVfs::new();
        let dest_dir = PathBuf::from(VIRTUAL_HOME).join("Documents");
        let old_path = dest_dir.join("notes.md");

        let outcome = vfs
            .perform(&OpRequest {
                id: 1,
                kind: OpKind::Rename,
                sources: vec![old_path.clone()],
                dest_dir: dest_dir.clone(),
                new_name: Some("journal.md".to_string()),
                home: vfs.home(),
            })
            .unwrap();
        let new_path = dest_dir.join("journal.md");
        assert_eq!(outcome.touched, vec![new_path.clone()]);
        assert!(vfs.read_dir(&dest_dir, true).unwrap().iter().any(|e| e.name == "journal.md"));
        assert!(!vfs.read_dir(&dest_dir, true).unwrap().iter().any(|e| e.name == "notes.md"));

        // Renaming onto an existing sibling is refused.
        let collide = vfs.perform(&OpRequest {
            id: 2,
            kind: OpKind::Rename,
            sources: vec![new_path.clone()],
            dest_dir: dest_dir.clone(),
            new_name: Some("budget.csv".to_string()),
            home: vfs.home(),
        });
        assert!(collide.is_err());

        let Some(Undo::Moved { pairs }) = outcome.undo else { panic!("expected a Moved undo") };
        let undo_outcome = vfs.perform_undo(&Undo::Moved { pairs }).unwrap();
        assert_eq!(undo_outcome.touched, vec![old_path.clone()]);
        assert!(vfs.read_dir(&dest_dir, true).unwrap().iter().any(|e| e.name == "notes.md"));
    }

    #[test]
    fn copy_into_the_same_folder_gets_a_suffix_and_undoes() {
        let vfs = DemoVfs::new();
        let dir = PathBuf::from(VIRTUAL_HOME).join("Documents");
        let source = dir.join("notes.md");

        let outcome = vfs
            .perform(&OpRequest {
                id: 1,
                kind: OpKind::Copy,
                sources: vec![source.clone()],
                dest_dir: dir.clone(),
                new_name: None,
                home: vfs.home(),
            })
            .unwrap();
        let copy_path = dir.join("notes (2).md");
        assert_eq!(outcome.touched, vec![copy_path.clone()]);
        assert!(vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "notes.md"), "the original must survive its own copy");
        assert!(vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "notes (2).md"));

        let Some(Undo::Created { paths }) = outcome.undo else { panic!("expected a Created undo") };
        vfs.perform_undo(&Undo::Created { paths }).unwrap();
        assert!(!vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "notes (2).md"));
        assert!(vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "notes.md"));
    }

    #[test]
    fn trash_moves_out_and_undo_restores_it() {
        let vfs = DemoVfs::new();
        let dir = PathBuf::from(VIRTUAL_HOME).join("Documents");
        let source = dir.join("budget.csv");

        let outcome = vfs
            .perform(&OpRequest {
                id: 1,
                kind: OpKind::Trash,
                sources: vec![source.clone()],
                dest_dir: dir.clone(),
                new_name: None,
                home: vfs.home(),
            })
            .unwrap();
        assert!(!vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "budget.csv"));
        let trash_path = PathBuf::from(VIRTUAL_HOME).join(".Trash").join("budget.csv");
        assert_eq!(outcome.touched, vec![trash_path.clone()]);
        assert!(vfs.read_dir(&PathBuf::from(VIRTUAL_HOME).join(".Trash"), true).unwrap().iter().any(|e| e.name == "budget.csv"));

        let Some(Undo::Moved { pairs }) = outcome.undo else { panic!("expected a Moved undo") };
        vfs.perform_undo(&Undo::Moved { pairs }).unwrap();
        assert!(vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "budget.csv"), "undo must put it back in the same folder");
    }

    #[test]
    fn delete_removes_permanently_with_no_undo() {
        let vfs = DemoVfs::new();
        let dir = PathBuf::from(VIRTUAL_HOME).join("Documents");
        let source = dir.join("contacts.csv");

        let outcome = vfs
            .perform(&OpRequest {
                id: 1,
                kind: OpKind::Delete,
                sources: vec![source.clone()],
                dest_dir: dir.clone(),
                new_name: None,
                home: vfs.home(),
            })
            .unwrap();
        assert!(outcome.undo.is_none());
        assert!(outcome.touched.is_empty());
        assert!(!vfs.read_dir(&dir, true).unwrap().iter().any(|e| e.name == "contacts.csv"));
    }

    #[test]
    fn total_bytes_matches_the_scans_own_size() {
        let vfs = DemoVfs::new();
        let home = PathBuf::from(VIRTUAL_HOME);
        let cancel = AtomicBool::new(false);
        let total = vfs.total_bytes(&home, &cancel);
        assert!(total > 0);

        let scanned = vfs.scan(&home, &cancel, &|_| {}).expect("scan should complete");
        assert_eq!(scanned.size, total);

        // And the scan's own count should agree with a manual walk.
        let mut stack = vec![home.clone()];
        let mut file_total = 0u64;
        while let Some(dir) = stack.pop() {
            for entry in vfs.read_dir(&dir, true).unwrap() {
                if entry.is_dir {
                    stack.push(entry.path.clone());
                } else {
                    file_total += entry.size;
                }
            }
        }
        assert_eq!(file_total, total);
    }

    fn scan_stats(node: &Node, depth: usize, stats: &mut (usize, usize, usize, usize)) -> u64 {
        stats.2 = stats.2.max(depth);
        if node.is_dir {
            stats.1 += 1;
            let sum: u64 = node.children.iter().map(|child| scan_stats(child, depth + 1, stats)).sum();
            assert_eq!(node.size, sum, "folder {} has an inconsistent recursive size", node.name);
            sum
        } else {
            stats.0 += 1;
            if node.size >= 2 * 1024 * 1024 * 1024 {
                stats.3 += 1;
            }
            node.size
        }
    }

    #[test]
    fn generator_shape_sizes_and_timings_meet_the_demo_contract() {
        let generated_at = makepad_widgets::Cx::time_now();
        let vfs = DemoVfs::new();
        let generation = makepad_widgets::Cx::time_now() - generated_at;
        let scan_at = makepad_widgets::Cx::time_now();
        let node = vfs.scan(Path::new(VIRTUAL_HOME), &AtomicBool::new(false), &|_| {}).unwrap();
        let scan = makepad_widgets::Cx::time_now() - scan_at;
        let mut stats = (0usize, 0usize, 0usize, 0usize);
        let total = scan_stats(&node, 0, &mut stats);
        eprintln!("files demo generator: {generation:.3}s; full inline scan: {scan:.3}s");
        assert!((30_000..=45_000).contains(&stats.0), "file count: {}", stats.0);
        assert!((2_000..=3_500).contains(&stats.1), "folder count: {}", stats.1);
        assert!(stats.2 >= 9, "max depth: {}", stats.2);
        assert!(stats.3 >= 3, "only {} files are at least 2 GiB", stats.3);
        assert!(total >= 150 * 1024 * 1024 * 1024, "tree is only {total} bytes");
        assert!(generation < 0.150, "generation took {generation:.3}s");
        assert!(scan < 0.100, "inline scan took {scan:.3}s");
    }

    #[test]
    fn stat_and_synthetic_markdown_reads_work() {
        let vfs = DemoVfs::new();
        let path = Path::new("/Demo/Documents/notes.md");
        let entry = vfs.stat(path).unwrap();
        assert_eq!(entry.path, path);
        assert_eq!(entry.name, "notes.md");
        assert!(!entry.is_dir);
        let bytes = vfs.read_bytes(path, 4096).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("notes.md"));
        assert!(text.contains("closed files demo filesystem"));
    }

    #[test]
    fn home_is_demo_and_operations_are_instant() {
        let vfs = DemoVfs::new();
        assert_eq!(vfs.home(), PathBuf::from("/Demo"));
        assert!(vfs.is_instant());
        assert!(vfs.is_demo());
    }

    #[test]
    fn every_expected_top_level_folder_is_present_and_non_empty() {
        let vfs = DemoVfs::new();
        let root = listing(&vfs, VIRTUAL_HOME);
        for name in top_level_folders() {
            let entry = root.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("missing top-level folder {name}"));
            assert!(entry.is_dir);
            let children = listing(&vfs, &format!("{VIRTUAL_HOME}/{name}"));
            assert!(!children.is_empty(), "{name} has no contents");
        }
        // The trash exists (it showed up in `root`, which asked to see
        // hidden entries too) but is hidden from a normal listing.
        assert!(root.iter().any(|e| e.name == ".Trash"), "the trash folder should still exist when hidden entries are shown");
        assert!(vfs.read_dir(Path::new(VIRTUAL_HOME), false).unwrap().iter().all(|e| !e.name.starts_with('.')));
    }
}
