//! A runnable map from nothing: fetch one city-sized OSM extract and bake
//! it into the archives a map app actually opens.
//!
//! The full map set this repo normally runs on is tens of gigabytes and
//! hours of conversion ([`tools/download_map.sh`]). Nobody should need that
//! to see the app work. This module is the small end of the same pipeline:
//! ~143 MB in, about a minute of baking on a laptop, out comes Amsterdam —
//! tiles to draw, a graph to route on, an index to search.
//!
//! The steps are the CLI's steps, in the CLI's order, with the same
//! commentary (see [`crate::progress`]):
//!
//! 1. **fetch** — the extract, resumable, verified against its MD5 sidecar.
//! 2. **detail** — [`crate::native::convert_detail`] passes 1-4 into a
//!    scratch store (pass 5 skipped: only the store feeds the next step).
//! 3. **base** — [`crate::native::convert_base`] writes the one archive the
//!    app draws from: styled z0..=14 plus all-tag detail at z14.
//! 4. **nav** — [`crate::nav_build`] writes `.graph` and `.search`.
//! 5. **faces** — `crate::faces` replays the painter cascade and bakes the
//!    road-union faces into the archive, which is what makes a tilted city
//!    view cheap to draw. Needs the renderer, so it is behind the `faces`
//!    feature; without it the recipe stops after nav and the renderer falls
//!    back to unioning at runtime.

use crate::native;
use crate::progress::Report;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const BAKE_COMPLETE_MARKER: &str = "bake.complete.json";
const BAKE_LOCK: &str = ".bake.lock";

/// Where the extract comes from. BBBike cuts city extracts daily and keeps
/// an MD5 sidecar next to each one; the Amsterdam box reaches from the dune
/// coast to past Amstelveen, so a test drive has somewhere to go.
pub const AMSTERDAM_PBF_URL: &str =
    "https://download.bbbike.org/osm/bbbike/Amsterdam/Amsterdam.osm.pbf";

/// Roughly what the download weighs, for a bar that has to guess before the
/// server has said (the real total replaces it as soon as it does).
pub const AMSTERDAM_PBF_APPROX_BYTES: u64 = 143_000_000;

/// Free space to insist on before starting: the extract, the ~1 GiB scratch
/// store, the archive, and the nav artifacts, with room to spare.
pub const REQUIRED_FREE_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// The files a bake produces, and where a host looks to decide whether it
/// has a test map already.
#[derive(Clone, Debug)]
pub struct TestMapPaths {
    /// The downloaded OSM extract. Kept after the bake: re-baking from it
    /// is a minute, re-downloading it is not.
    pub pbf: PathBuf,
    /// The archive the map draws: base z0..=14 + all-tag detail at z14.
    pub archive: PathBuf,
    /// Basename of the nav artifacts; `.graph` and `.search` sit beside it.
    pub nav_basename: PathBuf,
    /// Bounded-memory scratch for the detail passes. Deleted on success.
    pub store: PathBuf,
}

impl TestMapPaths {
    /// The standard layout under a map directory (the repo uses
    /// `local/maps`).
    pub fn in_dir(dir: impl AsRef<Path>, name: &str) -> TestMapPaths {
        let dir = dir.as_ref();
        TestMapPaths {
            pbf: dir.join(format!("{name}.osm.pbf")),
            archive: dir.join(format!("{name}-base.mbtiles")),
            nav_basename: dir.join(name),
            store: dir.join(format!("{name}.store")),
        }
    }

    /// Amsterdam under `local/maps`, the default this repo's apps use.
    pub fn amsterdam() -> TestMapPaths {
        TestMapPaths::in_dir("local/maps", "amsterdam")
    }

    pub fn graph(&self) -> PathBuf {
        self.nav_basename.with_extension("graph")
    }

    pub fn search(&self) -> PathBuf {
        self.nav_basename.with_extension("search")
    }

    fn bake_complete(&self) -> PathBuf {
        self.store.join(BAKE_COMPLETE_MARKER)
    }

    fn bake_lock(&self) -> PathBuf {
        self.store.join(BAKE_LOCK)
    }

    /// True when every artifact the app opens is on disk. The scratch store
    /// and the extract are deliberately not part of this: one is deleted on
    /// success, the other is only an input.
    pub fn is_complete(&self) -> bool {
        self.archive.is_file() && self.graph().is_file() && self.search().is_file()
    }

    /// What a finished bake occupies, for a host that wants to say so.
    pub fn bytes_on_disk(&self) -> u64 {
        [self.pbf.clone(), self.archive.clone(), self.graph(), self.search()]
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum()
    }
}

/// The stages, in order, for a host that wants to draw them all up front.
pub const STAGES: [&str; 5] = ["fetch", "detail", "base", "nav", "faces"];

/// How far through the whole bake each stage's completion is, by the clock
/// rather than by step count — the fetch dominates on a slow line and the
/// bake passes dominate on a fast one, and a bar that jumps a fifth per
/// stage reads as broken. Measured on an M-series laptop, Amsterdam:
/// fetch ~60s, detail ~6s, base ~51s, nav ~6s, faces ~67s.
const STAGE_SPAN: [(f32, f32); 5] = [
    (0.00, 0.31),
    (0.31, 0.34),
    (0.34, 0.61),
    (0.61, 0.64),
    (0.64, 1.00),
];

/// Maps a within-stage fraction onto the whole-bake bar.
pub fn overall_fraction(stage: &str, stage_fraction: f32) -> f32 {
    let index = STAGES.iter().position(|s| *s == stage).unwrap_or(0);
    let (start, end) = STAGE_SPAN[index];
    start + (end - start) * stage_fraction.clamp(0.0, 1.0)
}

/// Everything the bake needs to know, so a host can point it somewhere else
/// (another city, another directory) without a second recipe.
#[derive(Clone, Debug)]
pub struct BakeOptions {
    pub paths: TestMapPaths,
    /// Downloaded when [`TestMapPaths::pbf`] is absent. An empty URL means
    /// "the extract is already there", and the fetch stage is skipped.
    pub pbf_url: String,
    /// Detail/base tile zoom. 14 is what the renderer expects.
    pub zoom: u8,
    /// Brotli quality for the archive. 11 is the production setting and is
    /// what the timings above assume.
    pub brotli_quality: u32,
    /// Keep the scratch store after a successful bake (it is ~1 GiB, and
    /// only useful for re-running the base pass without re-reading the PBF).
    pub keep_store: bool,
    /// Bake the painter-cascade faces into the finished archive. Costs
    /// about as long again as the rest of the bake and grows the archive by
    /// roughly two thirds, and is what makes the tilted view draw cheaply.
    /// Ignored without the `faces` feature.
    pub faces: bool,
    /// Preserve the complete archival tag payload. The default keeps only
    /// renderer-consumed detail tags; this is for archival/world builds.
    pub full: bool,
}

impl BakeOptions {
    pub fn amsterdam() -> BakeOptions {
        BakeOptions {
            paths: TestMapPaths::amsterdam(),
            pbf_url: AMSTERDAM_PBF_URL.to_string(),
            zoom: 14,
            brotli_quality: 11,
            keep_store: false,
            faces: true,
            full: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeStats {
    pub skipped_tiles: usize,
}

/// Recover the useful text from a caught panic instead of replacing it with
/// a generic "see the log" message at a host boundary.
pub fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic with non-string payload".to_string()
    }
}

/// Fetching is the host's job: in an app it is the platform's HTTP stack
/// (which already reports progress and survives a sleeping laptop), in the
/// CLI it is a socket. Either way [`bake`] gets the finished file.
pub trait Fetch {
    /// Fetch `url` to `dest`, atomically. Report progress as
    /// `(loaded, total)`; `total` is `None` until the server says.
    fn fetch(
        &mut self,
        url: &str,
        dest: &Path,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String>;
}

/// A host that has already put the extract in place.
pub struct NoFetch;

impl Fetch for NoFetch {
    fn fetch(
        &mut self,
        _url: &str,
        dest: &Path,
        _on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<(), String> {
        Err(format!(
            "{} is missing and this host cannot download it",
            dest.display()
        ))
    }
}

/// Run the whole recipe. Blocking, CPU-hungry, and minutes long: call it on
/// a worker thread and watch [`crate::progress`] for what it is doing.
///
/// Re-entrant in the useful sense — every stage skips itself when its
/// output is already on disk, so a bake interrupted after the download (or
/// after the archive) resumes rather than repeats.
pub fn bake(options: &BakeOptions, fetch: &mut dyn Fetch) -> Result<BakeStats, String> {
    let started = Instant::now();
    let paths = &options.paths;
    if let Some(dir) = paths.archive.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }

    let disposition = scratch_disposition(options);
    if matches!(disposition, ScratchDisposition::Skip)
        || (outputs_complete(options) && !paths.store.exists())
    {
        crate::step!("nav", "test map already built: {}", paths.archive.display());
        return Ok(BakeStats::default());
    }
    let _lock = BakeLock::acquire(paths)?;
    if paths.bake_complete().exists() && !matches!(disposition, ScratchDisposition::Skip) {
        fs::remove_file(paths.bake_complete())
            .map_err(|error| format!("remove stale completion marker: {error}"))?;
    }
    match &disposition {
        ScratchDisposition::Resume(stage) => {
            crate::step!("detail", "resuming unfinished bake at stage {stage}");
        }
        ScratchDisposition::Clean => {
            crate::step!("detail", "resuming unfinished bake at stage detail pass 1");
            crate::note!("detail", "  no durable scratch stage; starting clean");
            clean_scratch_for_restart(paths)?;
        }
        ScratchDisposition::Fresh | ScratchDisposition::Skip => {}
    }

    // All artifacts may have landed before a crash just ahead of the final
    // marker. Commit that state now instead of repeating any expensive pass.
    if outputs_complete(options) {
        write_bake_complete(options)?;
        remove_scratch_after_success(options);
        crate::step!("nav", "test map already built: {}", paths.archive.display());
        return Ok(BakeStats::default());
    }

    fetch_stage(options, fetch)?;
    detail_stage(options)?;
    base_stage(options)?;
    nav_stage(options)?;
    let skipped_tiles = faces_stage(options)?;

    if !outputs_complete(options) {
        return Err("bake finished but an artifact is missing".to_string());
    }
    write_bake_complete(options)?;
    remove_scratch_after_success(options);
    crate::step!(
        "nav",
        "test map ready in {:.0}s — {:.1} GiB on disk",
        started.elapsed().as_secs_f64(),
        paths.bytes_on_disk() as f64 / (1024.0 * 1024.0 * 1024.0)
    );
    Ok(BakeStats { skipped_tiles })
}

#[derive(Debug, PartialEq, Eq)]
enum ScratchDisposition {
    Fresh,
    Resume(&'static str),
    Clean,
    Skip,
}

fn scratch_disposition(options: &BakeOptions) -> ScratchDisposition {
    let paths = &options.paths;
    if !paths.store.exists() {
        return ScratchDisposition::Fresh;
    }
    if paths.bake_complete().is_file() && outputs_complete(options) {
        return ScratchDisposition::Skip;
    }
    if paths.store.join("spool.complete.json").is_file() {
        if !paths.archive.is_file() {
            return ScratchDisposition::Resume("base");
        }
        if !paths.graph().is_file() || !paths.search().is_file() {
            return ScratchDisposition::Resume("nav");
        }
        return ScratchDisposition::Resume("faces");
    }
    if paths.store.join("spool.pass3.json").is_file() {
        return ScratchDisposition::Resume("detail pass 4");
    }
    if paths.store.join("spool.pass2.json").is_file() {
        return ScratchDisposition::Resume("detail pass 3");
    }
    ScratchDisposition::Clean
}

fn outputs_complete(options: &BakeOptions) -> bool {
    if !options.paths.is_complete() {
        return false;
    }
    #[cfg(feature = "faces")]
    if options.faces && !crate::faces::archive_has_faces(&options.paths.archive) {
        return false;
    }
    true
}

fn clean_scratch_for_restart(paths: &TestMapPaths) -> Result<(), String> {
    for entry in fs::read_dir(&paths.store)
        .map_err(|err| format!("read {}: {err}", paths.store.display()))?
    {
        let entry = entry.map_err(|err| format!("read {}: {err}", paths.store.display()))?;
        if entry.file_name() == BAKE_LOCK {
            continue;
        }
        let path = entry.path();
        if entry
            .file_type()
            .map_err(|err| format!("stat {}: {err}", path.display()))?
            .is_dir()
        {
            fs::remove_dir_all(&path)
                .map_err(|err| format!("remove {}: {err}", path.display()))?;
        } else {
            fs::remove_file(&path)
                .map_err(|err| format!("remove {}: {err}", path.display()))?;
        }
    }
    Ok(())
}

fn write_bake_complete(options: &BakeOptions) -> Result<(), String> {
    let paths = &options.paths;
    for path in [&paths.archive, &paths.graph(), &paths.search()] {
        fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|err| format!("fsync {}: {err}", path.display()))?;
    }
    if let Some(parent) = paths.archive.parent() {
        sync_dir(parent)?;
    }
    let marker = serde_json::json!({
        "format": "makepad-testmap-bake-v1",
        "source": paths.pbf.display().to_string(),
        "archive": paths.archive.display().to_string(),
        "graph": paths.graph().display().to_string(),
        "search": paths.search().display().to_string(),
    });
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|err| format!("serialize bake completion marker: {err}"))?;
    let path = paths.bake_complete();
    let partial = paths.store.join("bake.complete.partial");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&partial)
        .map_err(|err| format!("create {}: {err}", partial.display()))?;
    file.write_all(&bytes)
        .map_err(|err| format!("write {}: {err}", partial.display()))?;
    file.sync_all()
        .map_err(|err| format!("fsync {}: {err}", partial.display()))?;
    fs::rename(&partial, &path)
        .map_err(|err| format!("publish {}: {err}", path.display()))?;
    sync_dir(&paths.store)
}

fn remove_scratch_after_success(options: &BakeOptions) {
    if !options.keep_store && options.paths.store.exists() {
        crate::note!(
            "nav",
            "  removing scratch store {}",
            options.paths.store.display()
        );
        let _ = fs::remove_dir_all(&options.paths.store);
    }
}

struct BakeLock {
    path: PathBuf,
    contents: String,
}

impl BakeLock {
    fn acquire(paths: &TestMapPaths) -> Result<Self, String> {
        fs::create_dir_all(&paths.store)
            .map_err(|err| format!("create {}: {err}", paths.store.display()))?;
        let path = paths.bake_lock();
        for _ in 0..4 {
            let pid = std::process::id();
            let started = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let contents = format!("pid={pid}\nstarted={started}\n");
            match fs::OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(contents.as_bytes())
                        .map_err(|err| format!("write {}: {err}", path.display()))?;
                    file.sync_all()
                        .map_err(|err| format!("fsync {}: {err}", path.display()))?;
                    sync_dir(&paths.store)?;
                    return Ok(Self { path, contents });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let existing = fs::read_to_string(&path).unwrap_or_default();
                    let existing_pid = lock_value(&existing, "pid").and_then(|v| v.parse().ok());
                    if existing_pid.is_some_and(pid_is_alive) {
                        let since = lock_value(&existing, "started").unwrap_or("unknown");
                        return Err(format!(
                            "scratch {} is locked by pid {} since {}; refusing concurrent bake",
                            paths.store.display(),
                            existing_pid.unwrap(),
                            since
                        ));
                    }
                    crate::note!("detail", "  clearing stale scratch lock {}", path.display());
                    match fs::remove_file(&path) {
                        Ok(()) => continue,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(err) => return Err(format!("remove stale {}: {err}", path.display())),
                    }
                }
                Err(error) => return Err(format!("create {}: {error}", path.display())),
            }
        }
        Err(format!("scratch lock {} changed too often", path.display()))
    }
}

impl Drop for BakeLock {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).ok().as_deref() == Some(&self.contents) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn lock_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    let result = unsafe { kill(pid as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetExitCodeProcess(process: *mut std::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    let mut code = 0;
    let alive = unsafe { GetExitCodeProcess(process, &mut code) != 0 && code == STILL_ACTIVE };
    unsafe { CloseHandle(process) };
    alive
}

#[cfg(not(any(unix, windows)))]
fn pid_is_alive(pid: u32) -> bool {
    pid == std::process::id()
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| format!("fsync {}: {err}", path.display()))
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn fetch_stage(options: &BakeOptions, fetch: &mut dyn Fetch) -> Result<(), String> {
    let path = &options.paths.pbf;
    if path.is_file() {
        crate::step!("fetch", "extract already here: {}", path.display());
        return Ok(());
    }
    if options.pbf_url.is_empty() {
        return Err(format!("{} is missing and no URL was given", path.display()));
    }
    crate::step!("fetch", "downloading {}", options.pbf_url);
    let started = Instant::now();
    let mut last_line = Instant::now();
    fetch.fetch(&options.pbf_url, path, &mut |loaded, total| {
        let total = total.unwrap_or(AMSTERDAM_PBF_APPROX_BYTES).max(1);
        let fraction = (loaded as f32 / total as f32).min(1.0);
        // The bar moves every chunk; the log line does not.
        if last_line.elapsed().as_secs_f64() >= 2.0 {
            last_line = Instant::now();
            crate::tick!(
                "fetch",
                fraction,
                "  {:.0} of {:.0} MB",
                loaded as f64 / 1.0e6,
                total as f64 / 1.0e6
            );
        } else {
            crate::progress::report(Report {
                stage: "fetch",
                line: String::new(),
                fraction: Some(fraction),
                headline: false,
            });
        }
    })?;
    let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    crate::tick!(
        "fetch",
        1.0,
        "  {:.0} MB in {:.0}s",
        bytes as f64 / 1.0e6,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn detail_stage(options: &BakeOptions) -> Result<(), String> {
    let paths = &options.paths;
    if paths.archive.is_file() {
        crate::step!("detail", "detail scratch no longer needed; archive already here");
        return Ok(());
    }
    // The store carries its own completion marker; convert_detail resumes
    // from whichever pass it reached, so there is nothing to skip by hand.
    let mut detail = native::default_detail_options(
        paths.pbf.clone(),
        // Pass 5 is skipped, so this output is never written. It still has
        // to name a file the store can be described against.
        paths.archive.with_extension("detail.mbtiles"),
        paths.store.clone(),
    );
    detail.zoom = options.zoom;
    detail.no_tiles = true;
    detail.full = options.full;
    native::convert_detail(detail)
}

fn base_stage(options: &BakeOptions) -> Result<(), String> {
    let paths = &options.paths;
    if paths.archive.is_file() {
        crate::step!("base", "archive already here: {}", paths.archive.display());
        return Ok(());
    }
    // Written under a partial name and renamed on success: a half-written
    // archive that looks finished would be served to the renderer on the
    // next run and read as a broken map.
    let partial = paths.archive.with_extension("partial.mbtiles");
    let _ = fs::remove_file(&partial);
    let mut base = native::default_base_options(
        paths.pbf.clone(),
        partial.clone(),
        paths.store.clone(),
    );
    base.brotli_quality = options.brotli_quality;
    base.max_zoom = options.zoom;
    base.full = options.full;
    native::convert_base(base)?;
    fs::rename(&partial, &paths.archive)
        .map_err(|e| format!("rename {}: {e}", partial.display()))
}

fn nav_stage(options: &BakeOptions) -> Result<(), String> {
    let paths = &options.paths;
    if paths.graph().is_file() && paths.search().is_file() {
        crate::step!("nav", "nav artifacts already here: {}", paths.graph().display());
        return Ok(());
    }
    crate::nav_build::nav_build(crate::nav_build::NavBuildOptions {
        source: paths.pbf.clone(),
        output_basename: paths.nav_basename.clone(),
        bbox: None,
        skip_addresses: false,
        places_only: false,
        searchdb: false,
        major_roads_only: false,
    })
}

/// Bake the face stream into the finished archive, in place: the app opens
/// one archive path, so the baked copy replaces the plain one rather than
/// sitting beside it as a second 200 MB file. Written under a partial name
/// and renamed, so a machine that loses power mid-bake still has the
/// working archive it had before.
#[cfg(feature = "faces")]
fn faces_stage(options: &BakeOptions) -> Result<usize, String> {
    let archive = &options.paths.archive;
    if !options.faces {
        return Ok(0);
    }
    if crate::faces::archive_has_faces(archive) {
        crate::step!("faces", "archive already carries baked faces");
        return Ok(0);
    }
    crate::step!(
        "faces",
        "Baking road faces into the tiles (what makes the tilted view cheap)"
    );
    let partial = archive.with_extension("faces.partial.mbtiles");
    let _ = fs::remove_file(&partial);
    let mut face_options =
        crate::faces::default_face_bake_options(archive.clone(), partial.clone());
    face_options.full = options.full;
    let stats = crate::faces::bake_faces(&face_options)?;
    if stats.baked == 0 {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "face bake produced zero baked tiles ({} skipped)",
            stats.skipped.len()
        ));
    }
    if !stats.skipped.is_empty() {
        crate::note!(
            "faces",
            "  faces: {} tiles skipped; renderer fallback will handle them",
            stats.skipped.len()
        );
        for (key, reason) in stats.skipped.iter().take(5) {
            crate::note!(
                "faces",
                "  skipped {}/{}/{}: {}",
                key.z,
                key.x,
                key.y,
                reason
            );
        }
    }
    fs::rename(&partial, archive)
        .map_err(|e| format!("rename {}: {e}", partial.display()))?;
    Ok(stats.skipped.len())
}

#[cfg(not(feature = "faces"))]
fn faces_stage(options: &BakeOptions) -> Result<usize, String> {
    if options.faces {
        crate::note!(
            "faces",
            "  faces not baked: this build has no renderer (feature `faces` off)"
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fixture(name: &str) -> (PathBuf, BakeOptions) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from("target/map-build-test-fixtures").join(format!(
            "{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut options = BakeOptions::amsterdam();
        options.paths = TestMapPaths::in_dir(&root, "fixture");
        options.faces = false;
        (root, options)
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"fixture").unwrap();
    }

    #[test]
    fn paths_describe_the_artifact_set() {
        let paths = TestMapPaths::in_dir("/tmp/maps", "amsterdam");
        assert_eq!(paths.pbf, PathBuf::from("/tmp/maps/amsterdam.osm.pbf"));
        assert_eq!(paths.archive, PathBuf::from("/tmp/maps/amsterdam-base.mbtiles"));
        assert_eq!(paths.graph(), PathBuf::from("/tmp/maps/amsterdam.graph"));
        assert_eq!(paths.search(), PathBuf::from("/tmp/maps/amsterdam.search"));
        assert!(!paths.is_complete());
    }

    #[test]
    fn caught_panic_keeps_its_message() {
        let panic = std::panic::catch_unwind(|| panic!("bad tile ring"));
        assert_eq!(panic_message(panic.unwrap_err()), "bad tile ring");
    }

    #[test]
    fn the_overall_bar_only_moves_forwards() {
        let mut last = 0.0;
        for stage in STAGES {
            for step in 0..=10 {
                let value = overall_fraction(stage, step as f32 / 10.0);
                assert!(value >= last, "{stage} at {step} went backwards");
                last = value;
            }
        }
        assert_eq!(last, 1.0);
        // An unknown stage must not panic or wind the bar back.
        assert_eq!(overall_fraction("nonsense", 0.0), 0.0);
    }

    #[test]
    fn unfinished_scratch_resumes_the_last_durable_stage() {
        let (root, options) = fixture("resume");
        touch(&options.paths.store.join("spool.pass2.json"));
        touch(&options.paths.store.join("ways.dat"));
        assert_eq!(
            scratch_disposition(&options),
            ScratchDisposition::Resume("detail pass 3")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scratch_without_a_durable_stage_is_cleaned() {
        let (root, options) = fixture("clean");
        touch(&options.paths.store.join("ways.dat"));
        touch(&options.paths.bake_lock());
        assert_eq!(scratch_disposition(&options), ScratchDisposition::Clean);
        clean_scratch_for_restart(&options.paths).unwrap();
        assert!(!options.paths.store.join("ways.dat").exists());
        assert!(options.paths.bake_lock().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_scratch_and_outputs_skip_the_bake() {
        let (root, options) = fixture("skip");
        for path in [
            options.paths.archive.clone(),
            options.paths.graph(),
            options.paths.search(),
            options.paths.bake_complete(),
        ] {
            touch(&path);
        }
        assert_eq!(scratch_disposition(&options), ScratchDisposition::Skip);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_lock_refuses_and_dead_lock_is_cleared() {
        let (root, options) = fixture("locked");
        let guard = BakeLock::acquire(&options.paths).unwrap();
        let error = BakeLock::acquire(&options.paths).err().unwrap();
        assert!(error.contains("refusing concurrent bake"));
        drop(guard);

        fs::write(
            options.paths.bake_lock(),
            format!("pid={}\nstarted=1\n", u32::MAX),
        )
        .unwrap();
        let stale_replaced = BakeLock::acquire(&options.paths).unwrap();
        drop(stale_replaced);
        fs::remove_dir_all(root).unwrap();
    }
}
