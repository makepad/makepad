//! `pbf-base`: build ONE archive holding renderer-compatible base layers at
//! z0..=14 plus renderer-consumed detail layers at z14, straight from the
//! existing pbf-detail store (no PBF re-ingest), with per-archive tile
//! compression. `--full` preserves the former all-tag detail profile.
//!
//! Pipeline:
//! 1. Validate the store's completion marker against the source PBF.
//! 2. Sample tiles, build the shared "dict-v1" dictionary and print the
//!    gzip / brotli / brotli+dict A/B numbers.
//! 3. Phase 1 (all cores): per z14 spool block, sort + group tiles, derive
//!    base features, encode+compress the combined z14 tile into per-block
//!    temp files, and downsample base fragments into per-zoom spools.
//! 4. Phase 2: write zooms ascending 0..=14 through a [`TileSink`]
//!    (block-major within each zoom, matching the MBTiles rowid scheme).
//!
//! All tile output goes through the small [`TileSink`] seam so the MBTiles
//! container can later be swapped for a custom single-file format without
//! touching emission or generalization code.

use super::geom::TILE_BUFFER;
use super::mvt::{
    encode_tile, encode_tile_with_profile, read_protobuf_bytes, read_protobuf_key,
    skip_protobuf_value, GeometryType, Layer, OsmType, TileFeature, TilePoint,
};
use super::schema::{
    base_specs, dissolve_polygon_features, downsample_paths, exact_clip_to_tile,
    finalize_feature, merge_features_by_tags, split_giant_polygons, tags_for_zoom, DETAIL_ZOOM,
};
use super::spool::{records_to_tiles, BlockKey, BlockSpoolWriter, SortedBlock, SpoolSummary};
use crate::versatiles::{GeoBounds, TileBounds};
use makepad_mbtile_reader::{
    compress_tile, compression_metadata_rows, MbtilesWriter, TileCompression,
};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DICT_MAX_BYTES: usize = 64 * 1024;
const DICT_SAMPLE_TILES: usize = 2048;
const DICT_SAMPLE_BLOCKS: usize = 12;
const DICT_MIN_COUNT: u64 = 4;
const SAMPLE_PYRAMID_ZOOMS: [u8; 4] = [5, 8, 10, 12];

#[derive(Clone, Debug)]
pub struct BaseOptions {
    pub source: PathBuf,
    pub output: PathBuf,
    pub store: PathBuf,
    pub bbox: Option<GeoBounds>,
    pub brotli_quality: u32,
    pub use_dict: bool,
    pub threads: usize,
    pub max_zoom: u8,
    pub sort_memory_mib: usize,
    pub baseline: Option<ProgressBaseline>,
    /// Preserve all raw detail tags and __makepad_osm_* provenance. The
    /// default archive contains only fields read by the renderer.
    pub full: bool,
}

/// Reference numbers from existing gzip archives covering the same extract,
/// used purely for live progress estimates (`--baseline t,base,detail,low`).
#[derive(Clone, Copy, Debug)]
pub struct ProgressBaseline {
    /// z14 tile count of the gzip detail archive for this extract.
    pub z14_tiles: u64,
    /// z14 gzip bytes in the baseline base archive for this extract.
    pub z14_base_gzip: u64,
    /// z14 gzip bytes in the baseline detail archive for this extract.
    pub z14_detail_gzip: u64,
    /// z0-13 gzip bytes in the baseline base archive for this extract.
    pub lowzoom_gzip: u64,
}

impl ProgressBaseline {
    pub fn parse(value: &str) -> Result<Self, String> {
        let parts: Vec<u64> = value
            .split(',')
            .map(|part| part.trim().parse::<u64>())
            .collect::<Result<_, _>>()
            .map_err(|err| format!("invalid --baseline '{value}': {err}"))?;
        if parts.len() != 4 {
            return Err(format!(
                "invalid --baseline '{value}': expected z14_tiles,z14_base_gzip,z14_detail_gzip,lowzoom_gzip"
            ));
        }
        Ok(Self {
            z14_tiles: parts[0].max(1),
            z14_base_gzip: parts[1],
            z14_detail_gzip: parts[2],
            lowzoom_gzip: parts[3],
        })
    }
}

pub fn default_base_options(source: PathBuf, output: PathBuf, store: PathBuf) -> BaseOptions {
    BaseOptions {
        source,
        output,
        store,
        bbox: None,
        brotli_quality: 11,
        use_dict: false,
        threads: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        max_zoom: DETAIL_ZOOM,
        sort_memory_mib: 128,
        baseline: None,
        full: false,
    }
}

/// Uncompressed byte split of a combined tile into base vs detail layers,
/// by walking the top-level MVT layer frames (detail layers are the
/// `osm_*`/`*_dz` names). Used to apportion compressed bytes for progress.
fn split_base_detail(mvt: &[u8]) -> Result<(u64, u64), String> {
    let mut base = 0_u64;
    let mut detail = 0_u64;
    let mut offset = 0;
    while offset < mvt.len() {
        let (field, wire) = read_protobuf_key(mvt, &mut offset)?;
        if field == 3 && wire == 2 {
            let layer = read_protobuf_bytes(mvt, &mut offset)?;
            let mut name = "";
            let mut layer_offset = 0;
            while layer_offset < layer.len() {
                let (layer_field, layer_wire) = read_protobuf_key(layer, &mut layer_offset)?;
                if layer_field == 1 && layer_wire == 2 {
                    name = std::str::from_utf8(read_protobuf_bytes(layer, &mut layer_offset)?)
                        .unwrap_or("");
                    break;
                }
                skip_protobuf_value(layer, &mut layer_offset, layer_wire)?;
            }
            if name.starts_with("osm_") || name.ends_with("_dz") {
                detail += layer.len() as u64;
            } else {
                base += layer.len() as u64;
            }
        } else {
            skip_protobuf_value(mvt, &mut offset, wire)?;
        }
    }
    Ok((base, detail))
}

// ---------------------------------------------------------------------------
// Tile sink seam
// ---------------------------------------------------------------------------

pub struct SinkStats {
    pub tile_count: u64,
    pub tile_bytes: u64,
    pub file_bytes: u64,
}

/// One output archive. Tiles must arrive with zoom ascending and, within a
/// zoom, in 256x256-block-major (block row, block column, local row, local
/// column) order — the order this pipeline produces. The v1 backend is the
/// MBTiles writer; keep emission code on this trait so the container format
/// can be swapped without touching generalization logic.
pub trait TileSink: Send {
    fn write_tile(&mut self, zoom: u8, x: u32, y: u32, compressed: &[u8]) -> Result<(), String>;
    fn finish(self: Box<Self>, metadata: &[(String, String)]) -> Result<SinkStats, String>;
}

struct MbtilesSink {
    writer: MbtilesWriter,
    path: PathBuf,
}

impl MbtilesSink {
    fn create(path: &Path) -> Result<Self, String> {
        Ok(Self {
            writer: MbtilesWriter::create(path)
                .map_err(|err| format!("create {}: {err}", path.display()))?,
            path: path.to_path_buf(),
        })
    }
}

impl TileSink for MbtilesSink {
    fn write_tile(&mut self, zoom: u8, x: u32, y: u32, compressed: &[u8]) -> Result<(), String> {
        self.writer
            .write_tile_xyz(zoom, x, y, compressed)
            .map_err(|err| format!("write tile {zoom}/{x}/{y}: {err}"))
    }

    fn finish(self: Box<Self>, metadata: &[(String, String)]) -> Result<SinkStats, String> {
        let mut writer = self.writer;
        for (key, value) in metadata {
            writer.set_metadata(key.clone(), value.clone());
        }
        let stats = writer
            .finish()
            .map_err(|err| format!("finish {}: {err}", self.path.display()))?;
        Ok(SinkStats {
            tile_count: stats.tile_count,
            tile_bytes: stats.tile_bytes,
            file_bytes: stats.file_bytes,
        })
    }
}

// ---------------------------------------------------------------------------
// Store validation (reuses the pbf-detail completion marker)
// ---------------------------------------------------------------------------

/// Frontier slack required beyond the bbox's farthest corner: one z14 tile
/// of global units, covering the anchor-vs-corner rounding differences.
const FRONTIER_MARGIN: f64 = 4096.0;

fn validate_store(options: &BaseOptions) -> Result<bool, String> {
    if !options.store.is_dir() {
        return Err(format!(
            "{} is not a directory; run pbf-detail first to build the store",
            options.store.display()
        ));
    }
    let marker_path = options.store.join("spool.complete.json");
    if !marker_path.exists() {
        return validate_live_store(options);
    }
    let marker = read_store_marker(&marker_path, "makepad-native-detail-spool-v1")?;
    check_marker_identity(options, &marker, &marker_path)?;
    Ok(false)
}

/// The spool is still being written by pass 4. A bbox slice is valid once
/// the published streaming frontier strictly exceeds the distance from the
/// NL spiral anchor to the bbox's FARTHEST corner: every relation that
/// could touch the bbox then has a spiral key below the frontier and is
/// fully on disk (passes 2-3 finished before the frontier file existed).
fn validate_live_store(options: &BaseOptions) -> Result<bool, String> {
    let Some(bbox) = options.bbox else {
        return Err(format!(
            "{} is incomplete; only --bbox slices may read a live store via the streaming frontier",
            options.store.display()
        ));
    };
    let stamp_path = options.store.join("spool.pass3.json");
    if !stamp_path.exists() {
        return Err(format!(
            "{} is incomplete and has no pass-3 stamp; pass 4 has not started",
            options.store.display()
        ));
    }
    let stamp = read_store_marker(&stamp_path, "makepad-native-detail-pass-stamp-v1")?;
    check_marker_identity(options, &stamp, &stamp_path)?;
    let frontier_path = options.store.join("spool-frontier.txt");
    let frontier_text = fs::read_to_string(&frontier_path).map_err(|err| {
        format!(
            "{} is incomplete and has no streaming frontier yet (read {}: {err})",
            options.store.display(),
            frontier_path.display()
        )
    })?;
    let frontier: f64 = frontier_text
        .trim()
        .parse()
        .map_err(|err| format!("parse {}: {err}", frontier_path.display()))?;
    let needed = bbox_far_corner_distance(bbox) + FRONTIER_MARGIN;
    if frontier <= needed {
        return Err(format!(
            "streaming frontier {frontier:.0} does not yet cover bbox {} (needs > {needed:.0}); retry later",
            bbox.as_csv()
        ));
    }
    crate::note!("base", 
        "  live store: frontier {frontier:.0} covers bbox (far corner {:.0})",
        needed - FRONTIER_MARGIN
    );
    Ok(true)
}

/// Distance in global mercator units from the NL spiral anchor to the
/// farthest corner of a lon/lat bbox. A mercator rect's farthest point
/// from any anchor is one of its four corners.
fn bbox_far_corner_distance(bbox: GeoBounds) -> f64 {
    let (anchor_x, anchor_y) = super::geom::project_lon_lat(
        super::geom::SPIRAL_ANCHOR_LON,
        super::geom::SPIRAL_ANCHOR_LAT,
        DETAIL_ZOOM,
    );
    let mut far = 0.0_f64;
    for lon in [bbox.west, bbox.east] {
        for lat in [bbox.south, bbox.north] {
            let (x, y) = super::geom::project_lon_lat(lon, lat, DETAIL_ZOOM);
            far = far.max(((x - anchor_x).powi(2) + (y - anchor_y).powi(2)).sqrt());
        }
    }
    far
}

fn read_store_marker(path: &Path, format: &str) -> Result<serde_json::Value, String> {
    let bytes = fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    let marker: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|err| format!("parse {}: {err}", path.display()))?;
    if marker.get("format").and_then(|value| value.as_str()) != Some(format) {
        return Err(format!("{} has an unsupported marker format", path.display()));
    }
    Ok(marker)
}

fn check_marker_identity(
    options: &BaseOptions,
    marker: &serde_json::Value,
    path: &Path,
) -> Result<(), String> {
    let marker_zoom = marker
        .get("zoom")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("{} has no zoom", path.display()))?;
    if marker_zoom != u64::from(DETAIL_ZOOM) {
        return Err(format!(
            "store detail zoom {marker_zoom} is not {DETAIL_ZOOM}; pbf-base requires a z{DETAIL_ZOOM} store"
        ));
    }
    let source_bytes = options
        .source
        .metadata()
        .map_err(|err| format!("stat {}: {err}", options.source.display()))?
        .len();
    let marker_source_bytes = marker
        .get("source_bytes")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("{} has no source_bytes", path.display()))?;
    if marker_source_bytes != source_bytes {
        return Err(format!(
            "store was built from a {marker_source_bytes}-byte PBF but {} is {source_bytes} bytes",
            options.source.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-tile build: combined z14 MVT + downsampled base fragments
// ---------------------------------------------------------------------------

struct FragMsg {
    zoom: u8,
    tile_x: u32,
    tile_y: u32,
    layer: Layer,
    geometry_type: GeometryType,
    osm_type: OsmType,
    id: i64,
    closed: bool,
    tags: Arc<Vec<(String, String)>>,
    paths: Vec<Vec<TilePoint>>,
}

struct TileBuild {
    mvt14: Vec<u8>,
    frags: Vec<FragMsg>,
}

fn build_tile(
    x: u32,
    y: u32,
    features: Vec<TileFeature>,
    pyramid_top: u8,
    emit_detail_zoom: bool,
    full: bool,
) -> Result<TileBuild, String> {
    let mut combined: Vec<TileFeature> = Vec::with_capacity(features.len() + 8);
    let mut frags = Vec::new();
    for feature in &features {
        for spec in base_specs(feature) {
            if emit_detail_zoom {
                let mut paths = feature.paths.clone();
                if spec.geometry_type == GeometryType::LineString && spec.closed {
                    for path in &mut paths {
                        if path.len() >= 3 && path.first() != path.last() {
                            let first = path[0];
                            path.push(first);
                        }
                    }
                }
                let base_feature = TileFeature {
                    layer: spec.layer,
                    geometry_type: spec.geometry_type,
                    osm_type: feature.osm_type,
                    id: spec.id,
                    closed: spec.closed,
                    tags: tags_for_zoom(spec.layer, &spec.tags, DETAIL_ZOOM),
                    paths,
                };
                if let Some(base_feature) = finalize_feature(base_feature, DETAIL_ZOOM) {
                    combined.push(base_feature);
                }
            }
            if spec.min_zoom > pyramid_top {
                continue;
            }
            let global =
                exact_clip_to_tile(x, y, spec.geometry_type, spec.closed, &feature.paths);
            if global.paths.is_empty() {
                continue;
            }
            for zoom in spec.min_zoom..=pyramid_top {
                let scaled = downsample_paths(&global, spec.geometry_type, zoom);
                if scaled.is_empty() {
                    continue;
                }
                let tags = Arc::new(tags_for_zoom(spec.layer, &spec.tags, zoom));
                for fragment in
                    super::schema::to_target_tiles(zoom, spec.geometry_type, &scaled, TILE_BUFFER)?
                {
                    frags.push(FragMsg {
                        zoom,
                        tile_x: fragment.tile_x,
                        tile_y: fragment.tile_y,
                        layer: spec.layer,
                        geometry_type: spec.geometry_type,
                        osm_type: feature.osm_type,
                        id: spec.id,
                        closed: spec.closed,
                        tags: Arc::clone(&tags),
                        paths: fragment.paths,
                    });
                }
            }
        }
    }
    let mvt14 = if emit_detail_zoom {
        combined.extend(features);
        // Same-tag street merging AT z14 (schema.rs gates this to
        // Layer::BaseStreets, stitch-only): one-feature-per-way put ~65%
        // more street features in urban tiles than the shortbread
        // reference and inflated the renderer's road-union input by the
        // same factor. NOTE: changes per-layer feature indices — bridge-dz
        // overlays must be rebaked against archives built with this
        // (stale joins fail closed via the endpoint identity check).
        let combined = merge_features_by_tags(combined, DETAIL_ZOOM);
        encode_tile_with_profile(combined, full)?
    } else {
        Vec::new()
    };
    Ok(TileBuild { mvt14, frags })
}

// ---------------------------------------------------------------------------
// Shared dictionary ("dict-v1") + A/B measurement
// ---------------------------------------------------------------------------

fn harvest_strings(mvt: &[u8], counts: &mut BTreeMap<Vec<u8>, u64>) -> Result<(), String> {
    let mut offset = 0;
    while offset < mvt.len() {
        let (field, wire) = read_protobuf_key(mvt, &mut offset)?;
        if field == 3 && wire == 2 {
            let layer = read_protobuf_bytes(mvt, &mut offset)?;
            let mut layer_offset = 0;
            while layer_offset < layer.len() {
                let (layer_field, layer_wire) = read_protobuf_key(layer, &mut layer_offset)?;
                match (layer_field, layer_wire) {
                    (1, 2) | (3, 2) => {
                        let bytes = read_protobuf_bytes(layer, &mut layer_offset)?;
                        *counts.entry(bytes.to_vec()).or_default() += 1;
                    }
                    (4, 2) => {
                        let value = read_protobuf_bytes(layer, &mut layer_offset)?;
                        let mut value_offset = 0;
                        while value_offset < value.len() {
                            let (value_field, value_wire) =
                                read_protobuf_key(value, &mut value_offset)?;
                            if value_field == 1 && value_wire == 2 {
                                let bytes = read_protobuf_bytes(value, &mut value_offset)?;
                                *counts.entry(bytes.to_vec()).or_default() += 1;
                            } else {
                                skip_protobuf_value(value, &mut value_offset, value_wire)?;
                            }
                        }
                    }
                    _ => skip_protobuf_value(layer, &mut layer_offset, layer_wire)?,
                }
            }
        } else {
            skip_protobuf_value(mvt, &mut offset, wire)?;
        }
    }
    Ok(())
}

/// Deterministic dict-v1: the deduped layer/key/value string corpus of the
/// sample, sorted by (tile-frequency, bytes) ascending so the most frequent
/// strings sit nearest the window end (cheapest back-references), truncated
/// to 64KB by dropping the least frequent strings first.
fn build_dictionary(samples: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let mut counts = BTreeMap::<Vec<u8>, u64>::new();
    for sample in samples {
        harvest_strings(sample, &mut counts)?;
    }
    let mut entries: Vec<(u64, Vec<u8>)> = counts
        .into_iter()
        .filter(|(bytes, count)| {
            *count >= DICT_MIN_COUNT && bytes.len() >= 2 && bytes.len() <= 512
        })
        .map(|(bytes, count)| (count, bytes))
        .collect();
    entries.sort();
    let mut total: usize = entries.iter().map(|(_, b)| b.len()).sum();
    let mut skip = 0;
    while total > DICT_MAX_BYTES && skip < entries.len() {
        total -= entries[skip].1.len();
        skip += 1;
    }
    let mut dict = Vec::with_capacity(total);
    for (_, bytes) in &entries[skip..] {
        dict.extend_from_slice(bytes);
    }
    Ok(dict)
}

struct AbNumbers {
    tiles: usize,
    raw: u64,
    gzip: u64,
    brotli: u64,
    brotli_dict: u64,
}

fn print_ab_numbers(ab: &AbNumbers, quality: u32, dict_len: usize) {
    let percent = |part: u64| part as f64 * 100.0 / ab.raw.max(1) as f64;
    crate::note!("base", "  sample: {} tiles, {} raw MVT bytes", ab.tiles, ab.raw);
    crate::note!("base", 
        "  gzip-fast:        {:>12} bytes ({:.1}% of raw)",
        ab.gzip,
        percent(ab.gzip)
    );
    crate::note!("base", 
        "  brotli q{}:       {:>12} bytes ({:.1}% of raw, {:.1}% smaller than gzip)",
        quality,
        ab.brotli,
        percent(ab.brotli),
        (1.0 - ab.brotli as f64 / ab.gzip.max(1) as f64) * 100.0
    );
    crate::note!("base", 
        "  brotli q{}+dict:  {:>12} bytes ({:.1}% of raw, {:.2}% smaller than plain brotli, dict {} bytes)",
        quality,
        ab.brotli_dict,
        percent(ab.brotli_dict),
        (1.0 - ab.brotli_dict as f64 / ab.brotli.max(1) as f64) * 100.0,
        dict_len
    );
}

fn measure_compression(
    samples: &[Vec<u8>],
    quality: u32,
    dict: &[u8],
    threads: usize,
) -> Result<AbNumbers, String> {
    let chunk_size = samples.len().div_ceil(threads.max(1)).max(1);
    let totals = std::thread::scope(|scope| -> Result<(u64, u64, u64, u64), String> {
        let mut handles = Vec::new();
        for chunk in samples.chunks(chunk_size) {
            handles.push(scope.spawn(move || -> Result<(u64, u64, u64, u64), String> {
                let mut raw = 0_u64;
                let mut gzip = 0_u64;
                let mut brotli = 0_u64;
                let mut brotli_dict = 0_u64;
                for sample in chunk {
                    raw += sample.len() as u64;
                    gzip += compress_tile(&TileCompression::Gzip, None, sample)
                        .map_err(|err| err.to_string())?
                        .len() as u64;
                    brotli +=
                        compress_tile(&TileCompression::Brotli { quality }, None, sample)
                            .map_err(|err| err.to_string())?
                            .len() as u64;
                    brotli_dict +=
                        compress_tile(&TileCompression::Brotli { quality }, Some(dict), sample)
                            .map_err(|err| err.to_string())?
                            .len() as u64;
                }
                Ok((raw, gzip, brotli, brotli_dict))
            }));
        }
        let mut totals = (0, 0, 0, 0);
        for handle in handles {
            let (raw, gzip, brotli, brotli_dict) =
                handle.join().map_err(|_| "A/B worker panicked".to_string())??;
            totals.0 += raw;
            totals.1 += gzip;
            totals.2 += brotli;
            totals.3 += brotli_dict;
        }
        Ok(totals)
    })?;
    Ok(AbNumbers {
        tiles: samples.len(),
        raw: totals.0,
        gzip: totals.1,
        brotli: totals.2,
        brotli_dict: totals.3,
    })
}

/// Sample encoded MVT tiles spread across the selected blocks: full z14
/// combined tiles plus partial low-zoom base tiles synthesized from the
/// sampled tiles' own fragments (representative string tables at every
/// zoom, even though their geometry covers one z14 tile each).
#[allow(clippy::too_many_arguments)]
fn sample_tiles(
    spool_dir: &Path,
    blocks: &[BlockKey],
    sort_memory: usize,
    pyramid_top: u8,
    emit_detail_zoom: bool,
    tile_bounds: Option<TileBounds>,
    live_spool: bool,
    full: bool,
) -> Result<Vec<Vec<u8>>, String> {
    let block_count = blocks.len().min(DICT_SAMPLE_BLOCKS).max(1);
    let per_block = DICT_SAMPLE_TILES / block_count;
    // One thread per sampled block; each block sorts into its own chunk
    // files, so the per-block work is independent.
    let mut samples = Vec::new();
    let block_results = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..block_count {
            let block = blocks[index * blocks.len() / block_count];
            handles.push(scope.spawn(move || {
                sample_block(
                    spool_dir,
                    block,
                    sort_memory,
                    per_block,
                    pyramid_top,
                    emit_detail_zoom,
                    tile_bounds,
                    live_spool,
                    full,
                )
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "sampling worker panicked".to_string())
            })
            .collect::<Vec<_>>()
    });
    for result in block_results {
        samples.extend(result??);
    }
    if samples.is_empty() {
        return Err("dictionary sampling produced no tiles".to_string());
    }
    Ok(samples)
}

#[allow(clippy::too_many_arguments)]
fn sample_block(
    spool_dir: &Path,
    block: BlockKey,
    sort_memory: usize,
    per_block: usize,
    pyramid_top: u8,
    emit_detail_zoom: bool,
    tile_bounds: Option<TileBounds>,
    live_spool: bool,
    full: bool,
) -> Result<Vec<Vec<u8>>, String> {
    let mut samples = Vec::new();
    {
        clean_stale_chunks(spool_dir, block)?;
        let sorted = SortedBlock::prepare(spool_dir, block, Some(sort_memory), live_spool)?;
        let mut tile_index = 0_usize;
        let mut taken = 0_usize;
        let mut sorted = records_to_tiles(sorted, block, |x, y, features| {
            if tile_bounds.is_some_and(|bounds| !bounds.contains(x, y)) {
                return Ok(());
            }
            tile_index += 1;
            if taken >= per_block || (tile_index - 1) % 17 != 0 {
                return Ok(());
            }
            taken += 1;
            let build = build_tile(x, y, features, pyramid_top, emit_detail_zoom, full)?;
            // Partial pyramid tiles from every 4th sample.
            if taken % 4 == 0 {
                let mut per_zoom_tile =
                    BTreeMap::<(u8, u32, u32), Vec<TileFeature>>::new();
                for frag in &build.frags {
                    if !SAMPLE_PYRAMID_ZOOMS.contains(&frag.zoom) {
                        continue;
                    }
                    per_zoom_tile
                        .entry((frag.zoom, frag.tile_x, frag.tile_y))
                        .or_default()
                        .push(TileFeature {
                            layer: frag.layer,
                            geometry_type: frag.geometry_type,
                            osm_type: frag.osm_type,
                            id: frag.id,
                            closed: frag.closed,
                            tags: (*frag.tags).clone(),
                            paths: frag.paths.clone(),
                        });
                }
                for ((zoom, _, _), features) in per_zoom_tile {
                    let kept: Vec<TileFeature> = features
                        .into_iter()
                        .filter_map(|feature| finalize_feature(feature, zoom))
                        .collect();
                    if kept.is_empty() {
                        continue;
                    }
                    let mvt = encode_tile(kept)?;
                    if !mvt.is_empty() {
                        samples.push(mvt);
                    }
                }
            }
            if !build.mvt14.is_empty() {
                samples.push(build.mvt14);
            }
            Ok(())
        })?;
        sorted.cleanup_chunks()?;
    }
    Ok(samples)
}

// ---------------------------------------------------------------------------
// Phase 1: extract + z14 emission + per-zoom fragment spools
// ---------------------------------------------------------------------------

fn clean_stale_chunks(dir: &Path, key: BlockKey) -> Result<(), String> {
    // Pid-scoped: a concurrent bbox slice may be sorting the same edge
    // block, and its live chunks must survive. Foreign stale chunks are
    // swept by the dispatcher between runs.
    let prefix = format!("block-{}-{}.sort-{}-", key.y, key.x, std::process::id());
    for entry in fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))? {
        let entry = entry.map_err(|err| format!("read {} entry: {err}", dir.display()))?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix))
        {
            fs::remove_file(entry.path())
                .map_err(|err| format!("remove {}: {err}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn z14_block_path(dir: &Path, key: BlockKey) -> PathBuf {
    dir.join(format!("block-{}-{}.tiles", key.y, key.x))
}

struct Phase1Stats {
    blocks_done: AtomicUsize,
    tiles_done: AtomicU64,
    z14_bytes: AtomicU64,
    /// Compressed bytes apportioned to base layers (uncompressed-share est).
    z14_base_bytes: AtomicU64,
    /// Compressed bytes apportioned to detail layers (uncompressed-share est).
    z14_detail_bytes: AtomicU64,
    frag_records: AtomicU64,
}

/// Everything the live progress line needs for projections.
#[derive(Clone, Copy)]
struct ProgressContext {
    baseline: Option<ProgressBaseline>,
    /// Brotli/gzip size ratio used to project the (gzip) low-zoom baseline
    /// onto our codec: measured when a phase-0 sample exists, else a fixed
    /// estimate refined post-hoc by the in-run sample.
    br_over_gzip: f64,
}

/// Raw z14 MVT tiles collected during extraction for the post-hoc A/B
/// measurement (used when --dict is off, replacing the separate phase 0).
struct SampleState {
    counter: AtomicU64,
    tiles: std::sync::Mutex<Vec<Vec<u8>>>,
    bytes: AtomicU64,
}

const INRUN_SAMPLE_STRIDE: u64 = 29;
const INRUN_SAMPLE_MAX_TILES: usize = 2048;
const INRUN_SAMPLE_MAX_BYTES: u64 = 192 * 1024 * 1024;

/// One tile's worth of decoded spool features, headed for the encode pool.
struct TileJob {
    block_index: usize,
    seq: u64,
    x: u32,
    y: u32,
    features: Vec<TileFeature>,
}

enum WriterMsg {
    /// A finished (compressed) z14 tile; empty `bytes` advances the block's
    /// sequence without writing a record.
    Tile {
        block_index: usize,
        seq: u64,
        tile_key: u16,
        bytes: Vec<u8>,
    },
    /// The reader finished streaming a block; `tiles` is its job count.
    BlockEnd { block_index: usize, tiles: u64 },
}

/// Per-block reorder buffer for the z14 writer: tiles arrive from the
/// encode pool in any order and are written strictly by sequence number.
/// The buffer stays small because out-of-order distance is bounded by the
/// encode pool size plus channel capacity.
#[derive(Default)]
struct BlockAssembly {
    next_seq: u64,
    total: Option<u64>,
    pending: BTreeMap<u64, (u16, Vec<u8>)>,
    out: Option<BufWriter<File>>,
}

impl BlockAssembly {
    fn partial_path(dir: &Path, block: BlockKey) -> PathBuf {
        z14_block_path(dir, block).with_extension("tiles.partial")
    }

    fn drain(&mut self, dir: &Path, block: BlockKey) -> Result<(), String> {
        while let Some((tile_key, bytes)) = self.pending.remove(&self.next_seq) {
            if !bytes.is_empty() {
                if self.out.is_none() {
                    let path = Self::partial_path(dir, block);
                    self.out = Some(BufWriter::with_capacity(
                        1024 * 1024,
                        File::create(&path)
                            .map_err(|err| format!("create {}: {err}", path.display()))?,
                    ));
                }
                let out = self.out.as_mut().unwrap();
                out.write_all(&tile_key.to_le_bytes())
                    .and_then(|_| out.write_all(&(bytes.len() as u32).to_le_bytes()))
                    .and_then(|_| out.write_all(&bytes))
                    .map_err(|err| format!("write z14 block temp: {err}"))?;
            }
            self.next_seq += 1;
        }
        Ok(())
    }

    fn try_finalize(&mut self, dir: &Path, block: BlockKey) -> Result<bool, String> {
        let Some(total) = self.total else {
            return Ok(false);
        };
        self.drain(dir, block)?;
        if self.next_seq < total {
            return Ok(false);
        }
        let partial = Self::partial_path(dir, block);
        if let Some(mut out) = self.out.take() {
            out.flush()
                .map_err(|err| format!("flush z14 block temp: {err}"))?;
        } else {
            File::create(&partial)
                .map_err(|err| format!("create {}: {err}", partial.display()))?;
        }
        let final_path = z14_block_path(dir, block);
        fs::rename(&partial, &final_path)
            .map_err(|err| format!("rename {}: {err}", partial.display()))?;
        Ok(true)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_phase1(
    spool_dir: &Path,
    blocks: &[BlockKey],
    work: &Path,
    options: &BaseOptions,
    compression: TileCompression,
    dict: Option<&[u8]>,
    pyramid_top: u8,
    emit_detail_zoom: bool,
    tile_bounds: Option<TileBounds>,
    progress: ProgressContext,
    sample_out: Option<&SampleState>,
    live_spool: bool,
) -> Result<Vec<Option<SpoolSummary>>, String> {
    let z14_dir = work.join("z14-tiles");
    if emit_detail_zoom {
        fs::create_dir_all(&z14_dir)
            .map_err(|err| format!("create {}: {err}", z14_dir.display()))?;
    }
    let sort_memory = options.sort_memory_mib * 1024 * 1024;
    let stats = Phase1Stats {
        blocks_done: AtomicUsize::new(0),
        tiles_done: AtomicU64::new(0),
        z14_bytes: AtomicU64::new(0),
        z14_base_bytes: AtomicU64::new(0),
        z14_detail_bytes: AtomicU64::new(0),
        frag_records: AtomicU64::new(0),
    };
    let next_block = AtomicUsize::new(0);
    let stop_logging = AtomicBool::new(false);
    let (frag_tx, frag_rx) = sync_channel::<Vec<FragMsg>>(1024);
    let started = Instant::now();

    let summaries = std::thread::scope(|scope| -> Result<Vec<Option<SpoolSummary>>, String> {
        // Single spool-writer thread: owns the per-zoom fragment spools.
        let spool_work = work.to_path_buf();
        let spool_thread = scope.spawn(move || -> Result<Vec<Option<SpoolSummary>>, String> {
            let mut writers: Vec<Option<BlockSpoolWriter>> =
                (0..usize::from(DETAIL_ZOOM)).map(|_| None).collect();
            for batch in frag_rx.iter() {
                for msg in batch {
                    let slot = &mut writers[usize::from(msg.zoom)];
                    if slot.is_none() {
                        let dir = spool_work.join(format!("z{}", msg.zoom));
                        *slot = Some(BlockSpoolWriter::create(&dir)?);
                    }
                    slot.as_mut().unwrap().push_parts(
                        msg.tile_x,
                        msg.tile_y,
                        msg.layer,
                        msg.geometry_type,
                        msg.osm_type,
                        msg.id,
                        msg.closed,
                        &msg.tags,
                        msg.paths.iter().map(Vec::as_slice),
                    )?;
                }
            }
            let mut summaries = Vec::with_capacity(writers.len());
            for writer in writers {
                summaries.push(match writer {
                    Some(writer) => Some(writer.finish()?),
                    None => None,
                });
            }
            Ok(summaries)
        });

        // Progress logger (250ms flag granularity so scope exit is prompt).
        scope.spawn(|| {
            let mut ticks = 0_u32;
            loop {
                std::thread::sleep(Duration::from_millis(250));
                if stop_logging.load(Ordering::Relaxed) {
                    break;
                }
                ticks += 1;
                if ticks % 8 != 0 {
                    continue;
                }
                let done = stats.blocks_done.load(Ordering::Relaxed);
                let tiles = stats.tiles_done.load(Ordering::Relaxed);
                if done == blocks.len() || tiles == 0 {
                    continue;
                }
                let elapsed = started.elapsed().as_secs_f64();
                let out_bytes = stats.z14_bytes.load(Ordering::Relaxed);
                let rate = tiles as f64 / elapsed;
                let gib = |bytes: u64| bytes as f64 / 1_073_741_824.0;
                match progress.baseline {
                    Some(baseline) if emit_detail_zoom => {
                        let base_out = stats.z14_base_bytes.load(Ordering::Relaxed);
                        let detail_out = stats.z14_detail_bytes.load(Ordering::Relaxed);
                        let done_frac = tiles as f64 / baseline.z14_tiles as f64;
                        // Same-coverage gzip baseline: average baseline
                        // bytes/tile times tiles done (tile mix estimate).
                        let baseline_bytes = (baseline.z14_base_gzip
                            + baseline.z14_detail_gzip)
                            as f64
                            / baseline.z14_tiles as f64
                            * tiles as f64;
                        let ratio = out_bytes as f64 * 100.0 / baseline_bytes.max(1.0);
                        let proj_z14 =
                            out_bytes as f64 / done_frac.max(f64::EPSILON);
                        let proj_total = proj_z14
                            + baseline.lowzoom_gzip as f64 * progress.br_over_gzip;
                        let remaining =
                            baseline.z14_tiles.saturating_sub(tiles) as f64;
                        crate::tick!("base", done_frac as f32 * EXTRACT_SPAN,
                            "  extract: {tiles}/{} z14 tiles ({:.1}%) | {:.2} GiB out (base {:.2} + detail {:.2} est) | {:.1}% of same-tiles gzip | {:.0} tiles/s | proj z14 {:.2} GiB, archive ~{:.2} GiB | ETA {:.1} min | cpu {:.0}%",
                            baseline.z14_tiles,
                            done_frac * 100.0,
                            gib(out_bytes),
                            gib(base_out),
                            gib(detail_out),
                            ratio,
                            rate,
                            proj_z14 / 1_073_741_824.0,
                            proj_total / 1_073_741_824.0,
                            remaining / rate.max(1.0) / 60.0,
                            process_cpu_percent().unwrap_or(0.0)
                        );
                    }
                    _ => crate::tick!("base",
                        done as f32 / blocks.len().max(1) as f32 * EXTRACT_SPAN,
                        "  extract: block {done}/{} | {tiles} z14 tiles | {:.2} GiB z14 out | {} fragments | {:.0} tiles/s | cpu {:.0}%",
                        blocks.len(),
                        gib(out_bytes),
                        stats.frag_records.load(Ordering::Relaxed),
                        rate,
                        process_cpu_percent().unwrap_or(0.0)
                    ),
                }
            }
        });

        // Tile-level pipeline: a few reader threads sort blocks and stream
        // per-tile jobs; an encode pool does the expensive geometry + brotli
        // work; one writer thread reassembles each block's tiles in order
        // through a bounded reorder buffer. This saturates all cores even
        // when a small bbox selects only a handful of blocks.
        let threads = options.threads.max(1);
        let readers = blocks.len().max(1).min((threads / 4).max(1)).min(4);
        let encoders = threads.saturating_sub(readers).max(1);
        let (job_tx, job_rx) = sync_channel::<TileJob>(threads * 2);
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let (writer_tx, writer_rx) = sync_channel::<WriterMsg>(threads * 4);

        // Ordered z14 writer with per-block reorder buffers.
        let writer_thread = emit_detail_zoom.then(|| {
            let z14_dir = z14_dir.clone();
            let stats = &stats;
            scope.spawn(move || -> Result<(), String> {
                let mut assemblies: std::collections::HashMap<usize, BlockAssembly> =
                    std::collections::HashMap::new();
                for msg in writer_rx.iter() {
                    match msg {
                        WriterMsg::Tile {
                            block_index,
                            seq,
                            tile_key,
                            bytes,
                        } => {
                            let assembly = assemblies.entry(block_index).or_default();
                            assembly.pending.insert(seq, (tile_key, bytes));
                            assembly.drain(&z14_dir, blocks[block_index])?;
                            if assembly.try_finalize(&z14_dir, blocks[block_index])? {
                                assemblies.remove(&block_index);
                                stats.blocks_done.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        WriterMsg::BlockEnd { block_index, tiles } => {
                            let assembly = assemblies.entry(block_index).or_default();
                            assembly.total = Some(tiles);
                            if assembly.try_finalize(&z14_dir, blocks[block_index])? {
                                assemblies.remove(&block_index);
                                stats.blocks_done.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
                if !assemblies.is_empty() {
                    return Err(format!(
                        "z14 writer finished with {} unfinalized blocks",
                        assemblies.len()
                    ));
                }
                Ok(())
            })
        });

        // Encode pool: geometry derivation, MVT encode, brotli.
        let mut encoder_handles = Vec::new();
        for _ in 0..encoders {
            let job_rx = std::sync::Arc::clone(&job_rx);
            let frag_tx = frag_tx.clone();
            let writer_tx = writer_tx.clone();
            let stats = &stats;
            encoder_handles.push(scope.spawn(move || -> Result<(), String> {
                loop {
                    let job = { job_rx.lock().unwrap().recv() };
                    let Ok(job) = job else {
                        return Ok(());
                    };
                    let build = build_tile(
                        job.x,
                        job.y,
                        job.features,
                        pyramid_top,
                        emit_detail_zoom,
                        options.full,
                    )?;
                    stats.tiles_done.fetch_add(1, Ordering::Relaxed);
                    if let Some(sample) = sample_out {
                        if !build.mvt14.is_empty()
                            && sample.counter.fetch_add(1, Ordering::Relaxed)
                                % INRUN_SAMPLE_STRIDE
                                == 0
                            && sample.bytes.load(Ordering::Relaxed) < INRUN_SAMPLE_MAX_BYTES
                        {
                            let mut tiles = sample.tiles.lock().unwrap();
                            if tiles.len() < INRUN_SAMPLE_MAX_TILES {
                                sample
                                    .bytes
                                    .fetch_add(build.mvt14.len() as u64, Ordering::Relaxed);
                                tiles.push(build.mvt14.clone());
                            }
                        }
                    }
                    if !build.frags.is_empty() {
                        stats
                            .frag_records
                            .fetch_add(build.frags.len() as u64, Ordering::Relaxed);
                        frag_tx
                            .send(build.frags)
                            .map_err(|_| "fragment spool thread exited early".to_string())?;
                    }
                    if emit_detail_zoom {
                        let bytes = if build.mvt14.is_empty() {
                            Vec::new()
                        } else {
                            compress_tile(&compression, dict, &build.mvt14).map_err(|err| {
                                format!(
                                    "compress z{DETAIL_ZOOM}/{}/{}: {err}",
                                    job.x, job.y
                                )
                            })?
                        };
                        stats
                            .z14_bytes
                            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                        if !bytes.is_empty() {
                            // Apportion the compressed size by uncompressed
                            // layer share for the base/detail progress split.
                            let (base_raw, detail_raw) = split_base_detail(&build.mvt14)?;
                            let total_raw = (base_raw + detail_raw).max(1);
                            let base_share = bytes.len() as u64 * base_raw / total_raw;
                            stats
                                .z14_base_bytes
                                .fetch_add(base_share, Ordering::Relaxed);
                            stats.z14_detail_bytes.fetch_add(
                                bytes.len() as u64 - base_share,
                                Ordering::Relaxed,
                            );
                        }
                        let tile_key = ((job.y & 255) << 8 | (job.x & 255)) as u16;
                        writer_tx
                            .send(WriterMsg::Tile {
                                block_index: job.block_index,
                                seq: job.seq,
                                tile_key,
                                bytes,
                            })
                            .map_err(|_| "z14 writer thread exited early".to_string())?;
                    }
                }
            }));
        }

        // Readers: sort blocks and stream tile jobs.
        let mut reader_handles = Vec::new();
        for _ in 0..readers {
            let job_tx = job_tx.clone();
            let writer_tx = writer_tx.clone();
            let stats = &stats;
            let next_block = &next_block;
            reader_handles.push(scope.spawn(move || -> Result<(), String> {
                loop {
                    let block_index = next_block.fetch_add(1, Ordering::Relaxed);
                    if block_index >= blocks.len() {
                        return Ok(());
                    }
                    let block = blocks[block_index];
                    clean_stale_chunks(spool_dir, block)?;
                    let sorted = SortedBlock::prepare(spool_dir, block, Some(sort_memory), live_spool)?;
                    let mut seq = 0_u64;
                    let mut sorted = records_to_tiles(sorted, block, |x, y, features| {
                        if tile_bounds.is_some_and(|bounds| !bounds.contains(x, y)) {
                            return Ok(());
                        }
                        job_tx
                            .send(TileJob {
                                block_index,
                                seq,
                                x,
                                y,
                                features,
                            })
                            .map_err(|_| "encode pool exited early".to_string())?;
                        seq += 1;
                        Ok(())
                    })?;
                    sorted.cleanup_chunks()?;
                    if emit_detail_zoom {
                        writer_tx
                            .send(WriterMsg::BlockEnd {
                                block_index,
                                tiles: seq,
                            })
                            .map_err(|_| "z14 writer thread exited early".to_string())?;
                    } else {
                        stats.blocks_done.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }));
        }
        drop(job_tx);
        drop(writer_tx);
        drop(frag_tx);

        let mut first_error: Option<String> = None;
        let mut record_error = |result: Result<Result<(), String>, _>, what: &str| {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    // Prefer root causes over "exited early" symptoms.
                    match &first_error {
                        None => first_error = Some(err),
                        Some(held)
                            if held.contains("exited early")
                                && !err.contains("exited early") =>
                        {
                            first_error = Some(err)
                        }
                        _ => {}
                    }
                }
                Err(_) => {
                    first_error.get_or_insert(format!("{what} panicked"));
                }
            }
        };
        for handle in reader_handles {
            record_error(handle.join(), "phase-1 reader");
        }
        for handle in encoder_handles {
            record_error(handle.join(), "phase-1 encoder");
        }
        if let Some(handle) = writer_thread {
            record_error(handle.join(), "z14 writer");
        }
        let spool_result = match spool_thread.join() {
            Ok(result) => result,
            Err(_) => Err("fragment spool thread panicked".to_string()),
        };
        stop_logging.store(true, Ordering::Relaxed);
        match (spool_result, first_error) {
            (Err(spool_err), Some(worker_err)) => {
                if worker_err.contains("exited early") {
                    Err(spool_err)
                } else {
                    Err(worker_err)
                }
            }
            (Err(spool_err), None) => Err(spool_err),
            (Ok(_), Some(worker_err)) => Err(worker_err),
            (Ok(summaries), None) => Ok(summaries),
        }
    })?;
    crate::note!("base", 
        "  extract done: {} blocks, {} z14 tiles, {:.2} GiB z14 payload, {} fragment records in {:.1}s",
        blocks.len(),
        stats.tiles_done.load(Ordering::Relaxed),
        stats.z14_bytes.load(Ordering::Relaxed) as f64 / 1_073_741_824.0,
        stats.frag_records.load(Ordering::Relaxed),
        started.elapsed().as_secs_f64()
    );
    Ok(summaries)
}

// ---------------------------------------------------------------------------
// Phase 2: ordered write through the sink
// ---------------------------------------------------------------------------

/// This process's current CPU usage in percent (up to cores x 100),
/// sampled via ps so under-saturation is visible in the log itself.
fn process_cpu_percent() -> Option<f64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "%cpu=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    std::str::from_utf8(&output.stdout).ok()?.trim().parse().ok()
}

struct ZoomProgress {
    zoom: u8,
    tiles: u64,
    bytes: u64,
    started: Instant,
    last_log: Instant,
}

/// How much of the base stage's bar phase 1 (extract + z14 compression)
/// owns. Phase 2 writes the pyramid into the archive and phase 3 tidies;
/// on an Amsterdam-sized extract that split is roughly 39s to 12s.
const EXTRACT_SPAN: f32 = 0.75;

impl ZoomProgress {
    fn new(zoom: u8) -> Self {
        Self {
            zoom,
            tiles: 0,
            bytes: 0,
            started: Instant::now(),
            last_log: Instant::now(),
        }
    }

    fn add(&mut self, tiles: u64, bytes: u64) {
        self.tiles += tiles;
        self.bytes += bytes;
        if self.last_log.elapsed() >= Duration::from_secs(2) {
            crate::note!("base", 
                "  z{}: {} tiles | {:.1} MiB | {:.0} tiles/s | cpu {:.0}%",
                self.zoom,
                self.tiles,
                self.bytes as f64 / 1_048_576.0,
                self.tiles as f64 / self.started.elapsed().as_secs_f64(),
                process_cpu_percent().unwrap_or(0.0)
            );
            self.last_log = Instant::now();
        }
    }

    fn finish(&self, max_zoom: u8) {
        // Phase 2 walks z0 upwards, so a finished zoom is a real position
        // in the run — the only fraction available here.
        let done = (self.zoom as f32 + 1.0) / (max_zoom as f32 + 1.0);
        crate::tick!("base",
            EXTRACT_SPAN + (1.0 - EXTRACT_SPAN) * done,
            "  z{}: {} tiles, {:.1} MiB in {:.1}s",
            self.zoom,
            self.tiles,
            self.bytes as f64 / 1_048_576.0,
            self.started.elapsed().as_secs_f64()
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn write_archive(
    work: &Path,
    options: &BaseOptions,
    compression: TileCompression,
    dict: Option<&[u8]>,
    pyramid_top: u8,
    emit_detail_zoom: bool,
    metadata: &[(String, String)],
) -> Result<SinkStats, String> {
    let mut sink: Box<dyn TileSink> = Box::new(MbtilesSink::create(&options.output)?);
    let sort_memory = options.sort_memory_mib * 1024 * 1024;

    // Each pyramid zoom runs the same tile-level pipeline as phase 1: a
    // sorter prefetch thread keeps block sorting off the critical path, the
    // encode pool does finalize+MVT+brotli on all cores, and one writer
    // reassembles the strict global order through a seq-keyed reorder
    // buffer (a single job producer assigns seq, so block order holds).
    for zoom in 0..=pyramid_top {
        let dir = work.join(format!("z{zoom}"));
        if !dir.is_dir() {
            continue;
        }
        let summary = SpoolSummary::from_dir(&dir)?;
        let encode_threads = options.threads.saturating_sub(2).max(1);
        let (sorted_tx, sorted_rx) = sync_channel::<(BlockKey, SortedBlock)>(1);
        let (job_tx, job_rx) = sync_channel::<(u64, u32, u32, Vec<TileFeature>)>(
            options.threads * 2,
        );
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let (out_tx, out_rx) = sync_channel::<(u64, u32, u32, Vec<u8>)>(options.threads * 4);

        let progress = std::thread::scope(|scope| -> Result<ZoomProgress, String> {
            let sorter_dir = dir.clone();
            let sorter_blocks = summary.blocks.clone();
            let sorter = scope.spawn(move || -> Result<(), String> {
                for block in sorter_blocks {
                    let sorted = SortedBlock::prepare(&sorter_dir, block, Some(sort_memory), false)?;
                    if sorted_tx.send((block, sorted)).is_err() {
                        return Ok(());
                    }
                }
                Ok(())
            });

            let mut encoder_handles = Vec::new();
            for _ in 0..encode_threads {
                let job_rx = std::sync::Arc::clone(&job_rx);
                let out_tx = out_tx.clone();
                encoder_handles.push(scope.spawn(move || -> Result<(), String> {
                    loop {
                        let job = { job_rx.lock().unwrap().recv() };
                        let Ok((seq, x, y, features)) = job else {
                            return Ok(());
                        };
                        // Dissolve BEFORE simplification: the union pass
                        // depends on exact shared fragment edges.
                        let features = dissolve_polygon_features(features, zoom);
                        let kept: Vec<TileFeature> = features
                            .into_iter()
                            .filter_map(|feature| finalize_feature(feature, zoom))
                            .collect();
                        let kept =
                            split_giant_polygons(merge_features_by_tags(kept, zoom), zoom);
                        let bytes = if kept.is_empty() {
                            Vec::new()
                        } else {
                            // Additive baked-fill stream (payload v2): built
                            // from the same finalized features, appended as
                            // an ignorable protobuf field after the MVT.
                            let baked = super::bake::baked_fills_field(&kept)?;
                            let mut mvt = encode_tile(kept)?;
                            if let Some(field) = baked {
                                mvt.extend_from_slice(&field);
                            }
                            if mvt.is_empty() {
                                Vec::new()
                            } else {
                                compress_tile(&compression, dict, &mvt).map_err(|err| {
                                    format!("compress z{zoom}/{x}/{y}: {err}")
                                })?
                            }
                        };
                        if out_tx.send((seq, x, y, bytes)).is_err() {
                            return Ok(());
                        }
                    }
                }));
            }
            drop(out_tx);

            let sink = &mut sink;
            let writer = scope.spawn(move || -> Result<ZoomProgress, String> {
                let mut progress = ZoomProgress::new(zoom);
                let mut next_seq = 0_u64;
                let mut pending = BTreeMap::<u64, (u32, u32, Vec<u8>)>::new();
                for (seq, x, y, bytes) in out_rx.iter() {
                    pending.insert(seq, (x, y, bytes));
                    while let Some((x, y, bytes)) = pending.remove(&next_seq) {
                        if !bytes.is_empty() {
                            sink.write_tile(zoom, x, y, &bytes)?;
                            progress.add(1, bytes.len() as u64);
                        }
                        next_seq += 1;
                    }
                }
                if !pending.is_empty() {
                    return Err(format!(
                        "z{zoom} writer finished with {} out-of-order tiles",
                        pending.len()
                    ));
                }
                Ok(progress)
            });

            // Main thread: stream tile jobs in strict block-major order.
            let mut stream_error: Option<String> = None;
            let mut seq = 0_u64;
            for (block, sorted) in sorted_rx.iter() {
                let result = records_to_tiles(sorted, block, |x, y, features| {
                    job_tx
                        .send((seq, x, y, features))
                        .map_err(|_| "pyramid encode pool exited early".to_string())?;
                    seq += 1;
                    Ok(())
                });
                match result {
                    Ok(mut sorted) => sorted.cleanup_chunks()?,
                    Err(err) => {
                        stream_error = Some(err);
                        break;
                    }
                }
            }
            drop(job_tx);

            let mut first_error = stream_error;
            for handle in encoder_handles {
                match handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        first_error.get_or_insert(err);
                    }
                    Err(_) => {
                        first_error.get_or_insert("pyramid encoder panicked".to_string());
                    }
                }
            }
            let writer_result = match writer.join() {
                Ok(result) => result,
                Err(_) => Err("pyramid writer panicked".to_string()),
            };
            match sorter.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    first_error.get_or_insert(err);
                }
                Err(_) => {
                    first_error.get_or_insert("pyramid sorter panicked".to_string());
                }
            }
            match (writer_result, first_error) {
                (Err(writer_err), Some(err)) => {
                    if err.contains("exited early") {
                        Err(writer_err)
                    } else {
                        Err(err)
                    }
                }
                (Err(writer_err), None) => Err(writer_err),
                (Ok(_), Some(err)) => Err(err),
                (Ok(progress), None) => Ok(progress),
            }
        })?;
        progress.finish(options.max_zoom);
    }

    if emit_detail_zoom {
        let z14_dir = work.join("z14-tiles");
        let mut block_files = Vec::new();
        for entry in
            fs::read_dir(&z14_dir).map_err(|err| format!("read {}: {err}", z14_dir.display()))?
        {
            let entry =
                entry.map_err(|err| format!("read {} entry: {err}", z14_dir.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(body) = name.strip_prefix("block-").and_then(|n| n.strip_suffix(".tiles"))
            else {
                continue;
            };
            let Some((y, x)) = body.split_once('-') else {
                continue;
            };
            let (Ok(y), Ok(x)) = (y.parse::<u32>(), x.parse::<u32>()) else {
                continue;
            };
            block_files.push((BlockKey { y, x }, entry.path()));
        }
        block_files.sort_by_key(|(key, _)| *key);
        // Structurally IO-bound: the z14 tiles were compressed during
        // extraction; this pass only streams the temp blocks into the
        // archive in rowid order, so CPU sits near one core by design.
        crate::note!("base", "  z{DETAIL_ZOOM}: ordered copy of pre-compressed tiles (io-bound pass)");
        let mut progress = ZoomProgress::new(DETAIL_ZOOM);
        for (key, path) in block_files {
            let mut reader = BufReader::with_capacity(
                1024 * 1024,
                File::open(&path).map_err(|err| format!("open {}: {err}", path.display()))?,
            );
            let mut header = [0_u8; 6];
            loop {
                match reader.read_exact(&mut header) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(err) => return Err(format!("read {}: {err}", path.display())),
                }
                let tile_key = u16::from_le_bytes([header[0], header[1]]);
                let length =
                    u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
                let mut bytes = vec![0_u8; length];
                reader
                    .read_exact(&mut bytes)
                    .map_err(|err| format!("read {}: {err}", path.display()))?;
                let x = (key.x << 8) | u32::from(tile_key & 255);
                let y = (key.y << 8) | u32::from(tile_key >> 8);
                sink.write_tile(DETAIL_ZOOM, x, y, &bytes)?;
                progress.add(1, length as u64);
            }
        }
        progress.finish(options.max_zoom);
    }

    sink.finish(metadata)
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

pub fn convert_base(options: BaseOptions) -> Result<(), String> {
    if !options.source.is_file() {
        return Err(format!("{} is not a file", options.source.display()));
    }
    if options.output.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite it",
            options.output.display()
        ));
    }
    if options.max_zoom > DETAIL_ZOOM {
        return Err(format!("--max-zoom {} exceeds {DETAIL_ZOOM}", options.max_zoom));
    }
    if options.brotli_quality > 11 {
        return Err(format!(
            "--brotli-quality {} exceeds 11",
            options.brotli_quality
        ));
    }
    if options.use_dict && options.brotli_quality < 2 {
        // The brotli encoder silently ignores the custom dictionary at
        // quality 0/1, which would make the br:dict-v1 metadata a lie.
        return Err("--dict requires --brotli-quality 2 or higher".to_string());
    }
    if let Some(parent) = options.output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }
    let live_spool = validate_store(&options)?;
    let header = super::read_pbf_header(&options.source)?;

    let started = Instant::now();
    let spool_dir = options.store.join("spool");
    let summary = SpoolSummary::from_dir(&spool_dir)?;
    let tile_bounds = options.bbox.map(|bbox| bbox.tile_bounds(DETAIL_ZOOM));
    let selected = match tile_bounds {
        Some(tiles) => {
            summary
                .blocks
                .iter()
                .copied()
                .filter(|block| {
                    let x_min = block.x << 8;
                    let y_min = block.y << 8;
                    tiles.x_min <= x_min + 255
                        && x_min <= tiles.x_max
                        && tiles.y_min <= y_min + 255
                        && y_min <= tiles.y_max
                })
                .collect::<Vec<_>>()
        }
        None => summary.blocks.clone(),
    };
    if selected.is_empty() {
        return Err("the requested bbox selects no spool blocks".to_string());
    }
    let pyramid_top = options.max_zoom.min(DETAIL_ZOOM - 1);
    let emit_detail_zoom = options.max_zoom == DETAIL_ZOOM;
    let compression = TileCompression::Brotli {
        quality: options.brotli_quality,
    };

    crate::step!("base", "pbf-base: single-origin base+detail archive");
    crate::note!("base", "  source:  {}", options.source.display());
    crate::note!("base", "  output:  {}", options.output.display());
    crate::note!("base", "  store:   {}", options.store.display());
    crate::note!("base", 
        "  blocks:  {} of {} (bbox {})",
        selected.len(),
        summary.blocks.len(),
        options
            .bbox
            .map(|b| b.as_csv())
            .unwrap_or_else(|| "none".to_string())
    );
    crate::note!("base", 
        "  codec:   brotli q{} dict={} threads={} max_zoom={}",
        options.brotli_quality, options.use_dict, options.threads, options.max_zoom
    );

    let sort_memory = options.sort_memory_mib * 1024 * 1024;
    // Brotli/gzip ratio for progress projections: measured up front when a
    // dictionary must be built first, otherwise a fixed estimate refined by
    // the post-extraction A/B on the in-run sample.
    let mut br_over_gzip = 0.56;
    let mut dict = None;
    if options.use_dict {
        // The dictionary must exist before any tile is compressed, so this
        // path keeps the separate sampling pass (parallel per block, but
        // bounded by block count — the price of --dict).
        crate::step!("base", "Phase 0: sampling tiles for dict-v1 + A/B measurement");
        let samples = sample_tiles(
            &spool_dir,
            &selected,
            sort_memory,
            pyramid_top,
            emit_detail_zoom,
            tile_bounds,
            live_spool,
            options.full,
        )?;
        let dictionary = build_dictionary(&samples)?;
        let ab =
            measure_compression(&samples, options.brotli_quality, &dictionary, options.threads)?;
        print_ab_numbers(&ab, options.brotli_quality, dictionary.len());
        br_over_gzip = ab.brotli as f64 / ab.gzip.max(1) as f64;
        dict = Some(dictionary);
    } else {
        crate::note!("base", 
            "Phase 0 skipped (no --dict): A/B sample is collected during extraction"
        );
    }

    // Work directory for per-zoom spools + z14 temp tiles.
    let work = options.output.with_extension("work");
    if work.exists() {
        fs::remove_dir_all(&work).map_err(|err| format!("remove {}: {err}", work.display()))?;
    }
    fs::create_dir_all(&work).map_err(|err| format!("create {}: {err}", work.display()))?;

    crate::step!("base", "Phase 1/3: extracting base layers + compressing z{DETAIL_ZOOM} tiles");
    let sample_state = SampleState {
        counter: AtomicU64::new(0),
        tiles: std::sync::Mutex::new(Vec::new()),
        bytes: AtomicU64::new(0),
    };
    run_phase1(
        &spool_dir,
        &selected,
        &work,
        &options,
        compression,
        dict.as_deref(),
        pyramid_top,
        emit_detail_zoom,
        tile_bounds,
        ProgressContext {
            baseline: options.baseline,
            br_over_gzip,
        },
        (!options.use_dict).then_some(&sample_state),
        live_spool,
    )?;
    if !options.use_dict {
        let samples = std::mem::take(&mut *sample_state.tiles.lock().unwrap());
        if samples.is_empty() {
            crate::note!("base", "  A/B: no z14 tiles sampled (nothing to measure)");
        } else {
            let dictionary = build_dictionary(&samples)?;
            let ab = measure_compression(
                &samples,
                options.brotli_quality,
                &dictionary,
                options.threads,
            )?;
            crate::note!("base", "  A/B (sampled during extraction):");
            print_ab_numbers(&ab, options.brotli_quality, dictionary.len());
        }
    }

    crate::step!("base", "Phase 2/3: writing archive zooms 0..={}", options.max_zoom);
    let metadata = archive_metadata(&options, &header.bounds, &compression, dict.as_deref());
    let stats = match write_archive(
        &work,
        &options,
        compression,
        dict.as_deref(),
        pyramid_top,
        emit_detail_zoom,
        &metadata,
    ) {
        Ok(stats) => stats,
        Err(err) => {
            // We verified above that no file existed at the output path, so
            // anything there now is our partial archive; remove it so a
            // retry is not blocked by the exists-check. The work directory
            // is kept for inspection.
            let _ = fs::remove_file(&options.output);
            return Err(err);
        }
    };

    crate::step!("base", "Phase 3/3: cleaning work directory");
    fs::remove_dir_all(&work).map_err(|err| format!("remove {}: {err}", work.display()))?;
    crate::note!("base", 
        "Done: {} tiles, {:.2} GiB payload, {:.2} GiB file in {:.1}s",
        stats.tile_count,
        stats.tile_bytes as f64 / 1_073_741_824.0,
        stats.file_bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn archive_metadata(
    options: &BaseOptions,
    pbf_bounds: &Option<[f64; 4]>,
    compression: &TileCompression,
    dict: Option<&[u8]>,
) -> Vec<(String, String)> {
    let bounds = options
        .bbox
        .map(|b| [b.west, b.south, b.east, b.north])
        .or(*pbf_bounds)
        .unwrap_or([-180.0, -85.051_128_8, 180.0, 85.051_128_8]);
    let mut metadata = vec![
        ("name".to_string(), "Makepad OSM base+detail".to_string()),
        (
            "description".to_string(),
            if options.full {
                "Single-origin OpenStreetMap archive: shortbread-style base layers z0-14 plus all-tag detail layers at z14"
            } else {
                "Single-origin OpenStreetMap archive: shortbread-style base layers z0-14 plus renderer-consumed detail layers at z14"
            }
            .to_string(),
        ),
        ("type".to_string(), "baselayer".to_string()),
        ("version".to_string(), "1".to_string()),
        ("format".to_string(), "pbf".to_string()),
        ("scheme".to_string(), "tms".to_string()),
        ("minzoom".to_string(), "0".to_string()),
        ("maxzoom".to_string(), options.max_zoom.to_string()),
        (
            "bounds".to_string(),
            format!(
                "{:.7},{:.7},{:.7},{:.7}",
                bounds[0], bounds[1], bounds[2], bounds[3]
            ),
        ),
        (
            "center".to_string(),
            format!(
                "{:.7},{:.7},{}",
                (bounds[0] + bounds[2]) * 0.5,
                (bounds[1] + bounds[3]) * 0.5,
                options.max_zoom.min(7)
            ),
        ),
        (
            "attribution".to_string(),
            "OpenStreetMap contributors".to_string(),
        ),
        (
            "license".to_string(),
            "Open Database License 1.0".to_string(),
        ),
        (
            "makepad_source_kind".to_string(),
            "osm-base-detail-v1".to_string(),
        ),
        // Payload v2: dissolved pyramid polygons + additive baked-fill
        // triangle streams (protobuf field 100; ignorable by v1 readers).
        ("makepad_payload".to_string(), "v2-fills-1".to_string()),
        (
            "makepad_source_file".to_string(),
            options.source.display().to_string(),
        ),
    ];
    if options.max_zoom == DETAIL_ZOOM {
        metadata.push((
            "makepad_all_osm_tags".to_string(),
            options.full.to_string(),
        ));
        metadata.push((
            "makepad_detail_zoom".to_string(),
            DETAIL_ZOOM.to_string(),
        ));
        metadata.push((
            "makepad_2_5d_tags".to_string(),
            if options.full {
                "building,building:part,height,min_height,building:levels,building:min_level,roof:shape,roof:height,roof:levels,roof:direction,roof:orientation,roof:angle,building:material,building:colour,roof:material,roof:colour"
            } else {
                "building,building:part,height,min_height,building:levels,building:min_level"
            }
            .to_string(),
        ));
    }
    let mut vector_layers = vec![
        Layer::BaseStreets,
        Layer::BaseWaterPolygons,
        Layer::BaseWaterLines,
        Layer::BaseLand,
        Layer::BaseBuildings,
        Layer::BaseStreetPolygons,
        Layer::BasePlaceLabels,
        Layer::BaseBoundaries,
        Layer::BasePois,
    ];
    if options.max_zoom == DETAIL_ZOOM {
        vector_layers.extend([
            Layer::OsmPoints,
            Layer::OsmLines,
            Layer::OsmPolygons,
            Layer::OsmRelationPoints,
            Layer::OsmRelationLines,
            Layer::OsmRelationPolygons,
        ]);
    }
    let layer_json: Vec<String> = vector_layers
        .iter()
        .map(|layer| format!(r#"{{"id":"{}","fields":{{}}}}"#, layer.name()))
        .collect();
    metadata.push((
        "json".to_string(),
        format!(r#"{{"vector_layers":[{}]}}"#, layer_json.join(",")),
    ));
    metadata.extend(compression_metadata_rows(compression, dict));
    metadata
}
