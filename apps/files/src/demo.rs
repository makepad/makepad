//! The demo filesystem: a whole fake home, in memory, for screen recordings.
//!
//! `--demo` (or `MAKEPAD_FILES_DEMO=1`, see [`crate::vfs::demo_requested`]) points
//! the browser at [`DemoVfs`] instead of the real disk, so a recording can
//! show `files` doing real work — thumbnails, Space preview, rename, copy,
//! the treemap, undo — without a single one of the user's own files ever
//! appearing on screen. It is not a mock of those features: every operation
//! genuinely mutates a real tree, and every thumbnailable file has a real,
//! repo-safe asset behind it (see [`Vfs::real_path`]) so the same decoders
//! and viewers the real filesystem uses render something real.
//!
//! The tree is built once, deterministically — a seeded PRNG, never the
//! clock — so two runs (and two recordings) show byte-identical sizes and
//! dates. Everything after that lives behind a [`Mutex`], because the
//! [`Vfs`] trait hands out `&self`: an in-memory filesystem still needs
//! interior mutability to survive a rename.

use std::{
    fs,
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

/// The repo root this process almost always already has as its current
/// directory. [`repo_asset`] tries the current directory first and this
/// second, so the demo still finds its assets when launched some other way.
const REPO_ROOT_FALLBACK: &str = "/Users/admin/makepad/makepad";

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
    let created = modified.saturating_sub(rng.range(0, THIRTY_DAYS_SECS));
    (modified, created)
}

/// A file's size: the real asset's own byte count when it has one (so a
/// thumbnail and its properties panel never disagree), else a plausible
/// number for its kind from the seeded RNG.
fn seeded_size(real: Option<&Path>, rng: &mut Rng, range: (u64, u64)) -> u64 {
    if let Some(path) = real {
        if let Ok(meta) = fs::metadata(path) {
            return meta.len();
        }
    }
    rng.range(range.0, range.1)
}

// ---------------------------------------------------------------------
// Finding real, repo-safe assets to back the virtual files
// ---------------------------------------------------------------------

/// Resolve `relative` against the repo root: the current directory first
/// (the normal case — this process starts in the repo root), then
/// [`REPO_ROOT_FALLBACK`]. `None` when neither has it, which a caller
/// treats the same as "no real asset" rather than an error — a demo file
/// with a missing backing asset just falls back to its type icon.
fn repo_asset(relative: &str) -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(relative);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let fallback = Path::new(REPO_ROOT_FALLBACK).join(relative);
    fallback.exists().then_some(fallback)
}

/// Every file directly inside a repo-relative directory whose extension is
/// one of `exts` (case-insensitive), sorted by path. The sort is what makes
/// this deterministic: `read_dir` order is whatever the OS feels like
/// handing back, and two demo trees built in the same checkout must pick
/// the same assets in the same order every time. A missing directory is
/// simply an empty pool, never an error.
fn discover_repo_files(relative_dir: &str, exts: &[&str]) -> Vec<PathBuf> {
    let Some(dir) = repo_asset(relative_dir) else {
        return Vec::new();
    };
    let Ok(read_dir) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read_dir
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .map(|ext| exts.iter().any(|e| ext.eq_ignore_ascii_case(e)))
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    out
}

/// The window manager's own desktop backgrounds, when this machine has any
/// — the one place outside the repo this module is allowed to look (every
/// other asset is repo-safe), and entirely optional: an absent directory
/// just means the wallpaper pool falls back to the repo's own photos.
/// Never looks anywhere else under the user's home.
fn discover_wallpapers() -> Vec<PathBuf> {
    let themes_dir = makepad_ai_hub::home::makepad_home().join("wm/themes");
    let Ok(theme_entries) = fs::read_dir(&themes_dir) else {
        return Vec::new();
    };
    let mut theme_dirs: Vec<PathBuf> = theme_entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    theme_dirs.sort();

    let mut out = Vec::new();
    for theme_dir in theme_dirs {
        let Ok(bg_entries) = fs::read_dir(theme_dir.join("backgrounds")) else {
            continue;
        };
        let mut files: Vec<PathBuf> = bg_entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        out.extend(files);
    }
    out
}

/// One real file per kind of virtual file, cycled through in order. A
/// deterministic sort (see [`discover_repo_files`]) plus deterministic
/// cycling is what makes two [`DemoVfs`] instances identical: nothing here
/// ever consults `read_dir` order or the clock.
struct Pools {
    videos: Vec<PathBuf>,
    photos: Vec<PathBuf>,
    wallpapers: Vec<PathBuf>,
    pdfs: Vec<PathBuf>,
    screenshots: Vec<PathBuf>,
    csvs: Vec<PathBuf>,
    txts: Vec<PathBuf>,
    mds: Vec<PathBuf>,
    rss: Vec<PathBuf>,
    tomls: Vec<PathBuf>,
}

impl Pools {
    fn discover() -> Self {
        let photos = discover_repo_files("local/mb3d", &["jpg", "jpeg"]);

        let mut wallpapers = discover_wallpapers();
        if wallpapers.is_empty() {
            // No wm theme on this machine: the repo's own photos are
            // still real images, just not desktop backgrounds.
            wallpapers = photos.clone();
        }

        // The AI-generated clips lead the pool: they are the richest thing in
        // the repo to look at, which is what a demo of a file browser wants
        // behind its video thumbnails and previews.
        let mut videos = discover_repo_files("local/ai_content_app", &["mp4"]);
        videos.extend(discover_video_cache());
        videos.extend(discover_repo_files("local/flowtest/real", &["mp4"]));
        videos.extend(discover_repo_files("local/flowtest", &["mp4"]));

        let mut pdfs = discover_repo_files("local/rotorquant/paper", &["pdf"]);
        pdfs.extend(repo_asset("local/retourformulier-techpunt-ned.pdf"));

        let screenshots: Vec<PathBuf> = [
            "examples/splash/window_0_frame_000000.png",
            "examples/map/window_0_frame_000000.png",
        ]
        .into_iter()
        .filter_map(repo_asset)
        .collect();

        let csvs = discover_repo_files("box3d", &["csv"]);
        let txts = discover_repo_files("local/mb3d", &["txt"]);
        let mds: Vec<PathBuf> = ["AGENTS.md", "README.md"].into_iter().filter_map(repo_asset).collect();
        let rss = discover_repo_files("apps/files/src", &["rs"]);
        let tomls: Vec<PathBuf> = ["Cargo.toml"].into_iter().filter_map(repo_asset).collect();

        Pools { videos, photos, wallpapers, pdfs, screenshots, csvs, txts, mds, rss, tomls }
    }
}

/// The VJ's decoder cache, when this machine has one. It is read-only extra
/// volume for the demo's video pool and entirely optional — a machine that has
/// never run the VJ gets the repo's own clips and nothing is missing.
fn discover_video_cache() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for side in ["media-video-a", "media-video-b"] {
        let dir = home.join(".makepad-vj").join(side).join("decoder-input");
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4")))
            .collect();
        // `read_dir` order is not stable across machines and the demo tree
        // must be, so the names are sorted before anything uses them.
        found.sort();
        out.extend(found);
    }
    out
}

/// Take the next item of `pool`, wrapping around once it runs out. `None`
/// when the pool is empty — the caller's file simply gets no real asset.
fn cycle(pool: &[PathBuf], index: &mut usize) -> Option<PathBuf> {
    if pool.is_empty() {
        return None;
    }
    let item = pool[*index % pool.len()].clone();
    *index += 1;
    Some(item)
}

// ---------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------

/// One node of the demo's tree: a folder with children, or a file with a
/// size, two timestamps and — maybe — a real asset behind it. Unlike
/// [`FileEntry`] this carries no full path: a node only knows its own
/// name, and the path is rebuilt by whoever is walking the tree, the same
/// way a real directory entry does not know its own parent either.
#[derive(Clone, Debug)]
struct VNode {
    name: String,
    is_dir: bool,
    /// A file's own size; always `0` for a folder — a folder's size is the
    /// fold of its children, computed by whoever needs it, exactly the way
    /// [`FileEntry::size`] is `0` for a directory too.
    size: u64,
    modified_secs: u64,
    created_secs: u64,
    /// The real file [`Vfs::real_path`] hands back for this node; always
    /// `None` for a folder.
    real_asset: Option<PathBuf>,
    children: Vec<VNode>,
}

fn folder(name: &str, children: Vec<VNode>) -> VNode {
    VNode {
        name: name.to_string(),
        is_dir: true,
        size: 0,
        modified_secs: DEMO_NOW_SECS,
        created_secs: DEMO_NOW_SECS,
        real_asset: None,
        children,
    }
}

/// Builds the seeded tree. One `Builder` lives exactly as long as
/// [`build_root`]'s call to it: the RNG state and the per-kind cycle
/// counters are what make repeated calls to `b.photo(...)` etc. hand out a
/// different (but, across two whole trees, identical) size/date/asset every
/// time.
struct Builder {
    rng: Rng,
    pools: Pools,
    video_i: usize,
    photo_i: usize,
    wallpaper_i: usize,
    pdf_i: usize,
    png_i: usize,
    csv_i: usize,
    txt_i: usize,
    md_i: usize,
    rs_i: usize,
    toml_i: usize,
}

impl Builder {
    fn new() -> Self {
        Builder {
            rng: Rng::new(SEED),
            pools: Pools::discover(),
            video_i: 0,
            photo_i: 0,
            wallpaper_i: 0,
            pdf_i: 0,
            png_i: 0,
            csv_i: 0,
            txt_i: 0,
            md_i: 0,
            rs_i: 0,
            toml_i: 0,
        }
    }

    fn file(&mut self, name: &str, real: Option<PathBuf>, size_range: (u64, u64)) -> VNode {
        let size = seeded_size(real.as_deref(), &mut self.rng, size_range);
        let (modified_secs, created_secs) = seeded_age(&mut self.rng);
        VNode { name: name.to_string(), is_dir: false, size, modified_secs, created_secs, real_asset: real, children: Vec::new() }
    }

    fn video(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.videos, &mut self.video_i);
        self.file(name, real, (8_000_000, 120_000_000))
    }

    fn photo(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.photos, &mut self.photo_i);
        self.file(name, real, (1_000_000, 6_000_000))
    }

    fn wallpaper(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.wallpapers, &mut self.wallpaper_i);
        self.file(name, real, (1_000_000, 6_000_000))
    }

    fn pdf(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.pdfs, &mut self.pdf_i);
        self.file(name, real, (100_000, 4_000_000))
    }

    /// A pdf pinned to one specific repo-relative asset rather than the
    /// cycling pool — for the one file (`retourformulier.pdf`) whose real
    /// name and content should actually agree.
    fn pdf_exact(&mut self, name: &str, relative: &str) -> VNode {
        let real = repo_asset(relative);
        self.file(name, real, (100_000, 4_000_000))
    }

    fn screenshot(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.screenshots, &mut self.png_i);
        self.file(name, real, (200_000, 3_000_000))
    }

    fn csv(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.csvs, &mut self.csv_i);
        self.file(name, real, (1_000, 40_000))
    }

    fn code(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.rss, &mut self.rs_i);
        self.file(name, real, (1_000, 40_000))
    }

    fn markdown(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.mds, &mut self.md_i);
        self.file(name, real, (500, 20_000))
    }

    fn toml(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.tomls, &mut self.toml_i);
        self.file(name, real, (200, 5_000))
    }

    fn text(&mut self, name: &str) -> VNode {
        let real = cycle(&self.pools.txts, &mut self.txt_i);
        self.file(name, real, (200, 20_000))
    }

    /// No repo-safe audio asset exists (see the module doc comment's list
    /// of sources), so every track stays unmapped: it still gets the audio
    /// icon and a plausible size, just no waveform or playback preview.
    fn audio(&mut self, name: &str) -> VNode {
        self.file(name, None, (3_000_000, 9_000_000))
    }

    /// Junk with no kind-appropriate repo-safe asset to point at (an
    /// archive, an installer): unmapped by design, per the module's rule
    /// that a wrong-kind mapping is worse than no mapping at all.
    fn junk(&mut self, name: &str, size_range: (u64, u64)) -> VNode {
        self.file(name, None, size_range)
    }
}

/// The whole seeded tree, rooted at [`VIRTUAL_HOME`]. See the module doc
/// comment for why this is deterministic, and the struct-level docs on
/// [`Builder`] for how the cycling works.
fn build_root() -> VNode {
    let mut b = Builder::new();

    let invoices = folder(
        "invoices",
        vec![
            b.pdf("invoice-2024-014.pdf"),
            b.pdf("invoice-2024-021.pdf"),
            b.csv("invoice-2024-033.csv"),
            b.pdf("invoice-2024-045.pdf"),
            b.csv("invoice-2024-058.csv"),
            b.pdf("invoice-2024-067.pdf"),
            b.pdf("invoice-2024-079.pdf"),
            b.csv("invoice-2024-090.csv"),
        ],
    );
    let documents = folder(
        "Documents",
        vec![
            invoices,
            b.markdown("notes.md"),
            b.csv("budget.csv"),
            b.csv("contacts.csv"),
            b.pdf_exact("retourformulier.pdf", "local/retourformulier-techpunt-ned.pdf"),
        ],
    );

    let vacation = folder(
        "vacation-2026",
        (42..54).map(|n| b.photo(&format!("IMG_{n:04}.jpg"))).collect(),
    );
    let wallpapers = folder(
        "wallpapers",
        ["sunrise-ridge.jpg", "neon-drift.jpg", "atlas-peaks.jpg", "coral-fade.jpg", "midnight-grid.jpg", "velvet-dune.jpg"]
            .iter()
            .map(|n| b.wallpaper(n))
            .collect(),
    );
    let pictures = folder("Pictures", vec![vacation, wallpapers]);

    let videos = folder(
        "Videos",
        [
            "neon-city-loop.mp4",
            "ocean-drone.mp4",
            "dancing-crowd.mp4",
            "sunset-timelapse.mp4",
            "tunnel-drive.mp4",
            "plasma-bloom.mp4",
            "paper-lanterns.mp4",
            "rooftop-rain.mp4",
            "glass-forest.mp4",
            "harbour-lights.mp4",
        ]
        .iter()
        .map(|n| b.video(n))
        .collect(),
    );

    let midnight_hours = folder(
        "Midnight Hours",
        vec![
            b.audio("01 Intro.mp3"),
            b.audio("02 Wavelength.mp3"),
            b.audio("03 Undertow.mp3"),
            b.audio("04 Skyline.mp3"),
            b.audio("05 Afterglow.mp3"),
        ],
    );
    let analog_drift = folder(
        "Analog Drift",
        vec![
            b.audio("01 Static Bloom.mp3"),
            b.audio("02 Vector Sun.mp3"),
            b.audio("03 Coastline.mp3"),
            b.audio("04 Nightbus.mp3"),
            b.audio("05 Drift Home.mp3"),
        ],
    );
    let music = folder("Music", vec![midnight_hours, analog_drift]);

    let downloads = folder(
        "Downloads",
        vec![
            b.junk("project-assets.zip", (5_000_000, 80_000_000)),
            b.junk("App-Installer.pkg", (20_000_000, 300_000_000)),
            b.screenshot("screenshot-2026-03-14.png"),
            b.pdf("report-draft.pdf"),
            b.csv("export-data.csv"),
            b.code("scratch.rs"),
            // Downloads is where a video lands before anyone files it.
            b.video("trailer-cut-v3.mp4"),
            b.video("clip_from_chat.mp4"),
        ],
    );

    let atlas_src = folder("src", vec![b.code("main.rs"), b.code("lib.rs"), b.code("render.rs")]);
    let atlas = folder("atlas", vec![b.toml("Cargo.toml"), b.markdown("README.md"), atlas_src]);
    let proj_notes = folder("notes", vec![b.markdown("TODO.md"), b.text("ideas.txt")]);
    let projects = folder("Projects", vec![atlas, proj_notes]);

    let trash = folder(TRASH_NAME, Vec::new());

    folder("Demo", vec![documents, pictures, videos, music, downloads, projects, trash])
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
    dest.children.push(VNode {
        name: name.clone(),
        is_dir: true,
        size: 0,
        modified_secs: DEMO_NOW_SECS,
        created_secs: DEMO_NOW_SECS,
        real_asset: None,
        children: Vec::new(),
    });
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

/// The demo filesystem: a fake home, seeded once and mutated in place by
/// whatever the user does during a recording. Nothing here ever touches
/// `std::fs` except to read the real assets [`Vfs::real_path`] hands out
/// and to `stat` them for a byte-accurate size — the tree itself lives and
/// dies with the process.
pub struct DemoVfs {
    root: Mutex<VNode>,
}

impl DemoVfs {
    /// Builds the seeded tree immediately (it is cheap — a few dozen nodes
    /// and a handful of `stat` calls) rather than lazily on first use, so a
    /// window that opens straight into the demo home never has to wait for
    /// its first listing.
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
            entries.push(FileEntry {
                kind: model::kind_for(&child_path, child.is_dir),
                name: child.name.clone(),
                is_dir: child.is_dir,
                size: child.size,
                modified_secs: child.modified_secs,
                created_secs: child.created_secs,
                permissions: if child.is_dir { "rwxr-xr-x".to_string() } else { "rw-r--r--".to_string() },
                child_count: child.is_dir.then(|| child.children.len() as u32),
                path: child_path,
            });
        }
        let mut order: Vec<usize> = (0..entries.len()).collect();
        model::sort_indices(&entries, &mut order, SortSpec::default());
        Ok(order.into_iter().map(|i| entries[i].clone()).collect())
    }

    fn is_dir(&self, path: &Path) -> bool {
        let tree = self.root.lock().unwrap();
        resolve(&tree, path).is_some_and(|n| n.is_dir)
    }

    fn real_path(&self, path: &Path) -> PathBuf {
        // A folder never has a real asset (there is nothing to decode), and
        // neither does a path that resolves to nothing at all — both fall
        // back to the identity, exactly like `RealVfs::real_path`, so the
        // caller never has to special-case "no mapping" against "no node".
        let tree = self.root.lock().unwrap();
        match resolve(&tree, path) {
            Some(node) if !node.is_dir => node.real_asset.clone().unwrap_or_else(|| path.to_path_buf()),
            _ => path.to_path_buf(),
        }
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

    fn top_level_folders() -> [&'static str; 6] {
        ["Documents", "Pictures", "Videos", "Music", "Downloads", "Projects"]
    }

    /// A listing carries enough to prove two trees are identical without
    /// pulling in the whole `FileEntry` (whose `path` also embeds the
    /// comparison, redundantly, once name is included).
    fn fingerprint(entries: &[FileEntry]) -> Vec<(String, bool, u64, u64, u64)> {
        entries.iter().map(|e| (e.name.clone(), e.is_dir, e.size, e.modified_secs, e.created_secs)).collect()
    }

    #[test]
    fn the_tree_is_deterministic() {
        let a = DemoVfs::new();
        let b = DemoVfs::new();
        // Depth-first over every folder in the tree, comparing each one's
        // listing between the two instances.
        let mut stack = vec![PathBuf::from(VIRTUAL_HOME)];
        let mut folders_checked = 0;
        while let Some(dir) = stack.pop() {
            let la = listing(&a, dir.to_str().unwrap());
            let lb = listing(&b, dir.to_str().unwrap());
            assert_eq!(fingerprint(&la), fingerprint(&lb), "listing of {} differs between two demo trees", dir.display());
            folders_checked += 1;
            for entry in &la {
                if entry.is_dir {
                    stack.push(entry.path.clone());
                }
            }
        }
        // Home itself, its six visible children and .Trash, plus every
        // folder nested under them.
        assert!(folders_checked > 10, "suspiciously few folders walked: {folders_checked}");
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
            }
        }
        assert!(files_checked > 20, "suspiciously few files walked: {files_checked}");
    }

    #[test]
    fn mapped_files_point_at_a_real_asset_of_the_matching_kind() {
        let vfs = DemoVfs::new();
        let mut stack = vec![PathBuf::from(VIRTUAL_HOME)];
        let mut mapped = 0;
        let mut unmapped = 0;
        while let Some(dir) = stack.pop() {
            for entry in listing(&vfs, dir.to_str().unwrap()) {
                if entry.is_dir {
                    stack.push(entry.path.clone());
                    continue;
                }
                let real = vfs.real_path(&entry.path);
                if real == entry.path {
                    unmapped += 1;
                    continue;
                }
                mapped += 1;
                assert!(real.exists(), "{} claims to map to {} which does not exist", entry.path.display(), real.display());
                let virtual_kind = model::kind_for(&entry.path, false);
                let real_kind = model::kind_for(&real, false);
                assert_eq!(
                    virtual_kind, real_kind,
                    "{} ({:?}) maps to {} ({:?}) — kinds disagree",
                    entry.path.display(),
                    virtual_kind,
                    real.display(),
                    real_kind
                );
            }
        }
        assert!(mapped > 0, "nothing mapped to a real asset at all");
        // Documented in the module's report to the integrator: audio and
        // some junk are expected to stay unmapped.
        assert!(unmapped > 0, "expected at least the audio tracks to stay unmapped");
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
