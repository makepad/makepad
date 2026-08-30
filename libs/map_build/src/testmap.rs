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
//!
//! The faces bake that follows in production is a rendering optimisation on
//! top of a finished archive, needs the renderer itself, and so lives with
//! the renderer (`makepad_widgets::map::face_bake`); a host that wants it
//! runs it after [`bake`] returns.

use crate::native;
use crate::progress::Report;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
pub const STAGES: [&str; 4] = ["fetch", "detail", "base", "nav"];

/// How far through the whole bake each stage's completion is, by the clock
/// rather than by step count — the fetch dominates on a slow line and the
/// base pass dominates on a fast one, and a bar that jumps 25% per stage
/// reads as broken. Measured on an M-series laptop: fetch ~60s, detail ~6s,
/// base ~51s, nav ~6s.
const STAGE_SPAN: [(f32, f32); 4] =
    [(0.00, 0.48), (0.48, 0.53), (0.53, 0.94), (0.94, 1.00)];

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
}

impl BakeOptions {
    pub fn amsterdam() -> BakeOptions {
        BakeOptions {
            paths: TestMapPaths::amsterdam(),
            pbf_url: AMSTERDAM_PBF_URL.to_string(),
            zoom: 14,
            brotli_quality: 11,
            keep_store: false,
        }
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
pub fn bake(options: &BakeOptions, fetch: &mut dyn Fetch) -> Result<(), String> {
    let started = Instant::now();
    let paths = &options.paths;
    if let Some(dir) = paths.archive.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }

    fetch_stage(options, fetch)?;
    detail_stage(options)?;
    base_stage(options)?;
    nav_stage(options)?;

    if !options.keep_store && paths.store.exists() {
        // Only now, with every artifact written and verified present: the
        // store is the one big thing worth reclaiming.
        crate::note!("nav", "  removing scratch store {}", paths.store.display());
        let _ = fs::remove_dir_all(&paths.store);
    }
    if !paths.is_complete() {
        return Err("bake finished but an artifact is missing".to_string());
    }
    crate::step!(
        "nav",
        "test map ready in {:.0}s — {:.1} GiB on disk",
        started.elapsed().as_secs_f64(),
        paths.bytes_on_disk() as f64 / (1024.0 * 1024.0 * 1024.0)
    );
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
