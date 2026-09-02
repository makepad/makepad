//! Painter-cascade face baker (payload v2-faces-1).
//!
//! The last pass, and the one that decides whether a tilted city view runs
//! at frame rate: it replays the renderer's own road-union pipeline for
//! every z14 tile at several zoom buckets, and appends the resulting faces
//! as protobuf field 101 on the tile payload. At draw time the renderer
//! finds them and skips the union entirely. Tiles whose cascade was cheap
//! anyway are left alone (see `threshold_ms`) — the stream exists to rescue
//! heavy urban builds, and baking rural tiles costs archive size for builds
//! that were already fast.
//!
//! This is the one bake pass that needs the renderer itself, so the crate
//! only carries it under the `faces` feature; everything else in
//! `map_build` is free of the widget library. Both hosts enable it: the
//! `makepad-map-bake` CLI is a shell over [`bake_faces`], and the test-map
//! recipe runs it as its final stage.
//!
//! Readers that ignore field 101 see an identical archive: all other tiles
//! and every metadata row are copied verbatim, and `makepad_payload` gains
//! a `+faces-1` suffix so a later run can tell a baked archive from a raw
//! one ([`archive_has_faces`]).

use makepad_mbtile_reader::{compress_tile, MbtilesReader, MbtilesWriter, TileCompression};
use makepad_widgets::map::geometry::TileKey;
use makepad_widgets::map::style::probe_compiled_theme;
use makepad_widgets::map::tile::{
    bake_tile_paint_faces, decode_vector_tile_payload, encode_baked_faces_field,
    try_bake_tile_paint_faces,
};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Default buckets for z14 tiles. Europe policy (data-cost analysis) bakes
/// 14 and 16 — native zoom and deep overzoom, where cost concentrates —
/// while 15 is a transient gesture band. A city-sized test map is small
/// enough to bake all three, and 15 is where a pinch-zoom lands.
/// There is NO cross-bucket reuse: the signature guard requires the exact
/// bucket's styling structure.
pub const DEFAULT_BUCKETS: [u32; 3] = [14, 15, 16];

/// Cascade replay slower than this is worth baking; faster is not.
pub const DEFAULT_THRESHOLD_MS: f64 = 60.0;

#[derive(Clone, Debug)]
pub struct FaceBakeOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    /// Bridge-bake overlay: solved per-vertex road elevation, which the
    /// renderer prefers over tag heuristics inside its coverage bounds. The
    /// bake must see what the renderer will see, or the baked faces are for
    /// a different road surface than the one drawn.
    pub bridge_dz: Option<PathBuf>,
    pub brotli_quality: u32,
    pub threshold_ms: f64,
    /// Buckets to bake for z14 tiles. Tiles at other zooms bake their own
    /// native bucket only.
    pub buckets: Vec<u32>,
    /// Tile zooms to bake. When rerunning over an archive that already
    /// carries streams, the zoom sets must not overlap: a baked tile
    /// appends field 101 and a reader takes the FIRST field it finds.
    pub zooms: Vec<u32>,
    /// Re-encode every tile at the target quality (fleet mode: cells arrive
    /// sliced at a throwaway quality).
    pub recompress: bool,
    /// Stop baking after this many tiles; the rest are copied.
    pub limit: usize,
    /// Preserve any legacy archival-only bucket sections. The renderer
    /// profile writes the v4 shadow slots empty.
    pub full: bool,
}

pub fn default_face_bake_options(input: PathBuf, output: PathBuf) -> FaceBakeOptions {
    FaceBakeOptions {
        input,
        output,
        bridge_dz: None,
        brotli_quality: 10,
        threshold_ms: DEFAULT_THRESHOLD_MS,
        buckets: DEFAULT_BUCKETS.to_vec(),
        zooms: vec![14],
        recompress: false,
        limit: usize::MAX,
        full: false,
    }
}

#[derive(Clone, Debug, Default)]
pub struct FaceBakeStats {
    pub total: usize,
    pub baked: usize,
    pub copied: usize,
    pub field_bytes: usize,
    pub file_bytes: u64,
    /// Tiles copied through without a face stream because their individual
    /// decode, bake, or compression failed. The renderer falls back to its
    /// live cascade for these tiles.
    pub skipped: Vec<(TileKey, String)>,
}

/// True when this archive already carries a baked face stream. Re-baking
/// one is wasted minutes, and a second field would shadow the first.
pub fn archive_has_faces(path: &Path) -> bool {
    MbtilesReader::open(path)
        .and_then(|mut reader| reader.get_metadata())
        .map(|metadata| {
            metadata
                .get("makepad_payload")
                .is_some_and(|value| value.contains("faces-1"))
        })
        .unwrap_or(false)
}

/// Remove any existing baked-faces field (field 101, LEN) from a decoded
/// tile payload: rebake runs feed previously-baked cells, and the stale
/// field must not shadow the fresh one (first-field-wins on parse).
fn strip_baked_field(pbf: Vec<u8>) -> Vec<u8> {
    const BAKED_FACES_FIELD: u64 = 101;
    let read_varint = |bytes: &[u8], i: &mut usize| -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *bytes.get(*i)?;
            *i += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    };
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0usize;
    while i < pbf.len() {
        let start = i;
        let Some(key) = read_varint(&pbf, &mut i) else { break };
        if key & 0x7 != 2 {
            break; // unexpected wire type at top level: keep rest as-is
        }
        let Some(len) = read_varint(&pbf, &mut i) else { break };
        let end = i + len as usize;
        if end > pbf.len() {
            break;
        }
        if key >> 3 == BAKED_FACES_FIELD {
            if out.is_none() {
                let mut fresh = Vec::with_capacity(pbf.len());
                fresh.extend_from_slice(&pbf[..start]);
                out = Some(fresh);
            }
        } else if let Some(out) = out.as_mut() {
            out.extend_from_slice(&pbf[start..end]);
        }
        i = end;
    }
    out.unwrap_or(pbf)
}

/// Version handshake: bake the input's first z14 tile across the fleet
/// buckets and return the xor of the two renderer-consumed signatures. Any
/// semantic drift in the road regions or dissolved building groups changes
/// this, so a dispatcher can refuse a stale worker BEFORE it bakes junk.
pub fn fingerprint(input: &Path) -> Result<u64, String> {
    let mut reader =
        MbtilesReader::open(input).map_err(|err| format!("open {}: {err}", input.display()))?;
    let theme = probe_compiled_theme();
    let mut fingerprint = 0u64;
    let codec = reader.tile_codec().clone();
    reader
        .for_each_tile(|tile| {
            if tile.zoom_level != 14 || fingerprint != 0 {
                return;
            }
            let Ok(raw) = codec.decode(&tile.tile_data) else { return };
            let Ok(pbf) = decode_vector_tile_payload(&raw) else { return };
            let zoom = tile.zoom_level as u8;
            let y = (1u32 << zoom) - 1 - tile.tile_row as u32;
            let key = TileKey { z: zoom as u32, x: tile.tile_column as i32, y: y as i32 };
            for bucket in [15u32, 16, 17, 18] {
                if let Some(baked) =
                    bake_tile_paint_faces(key, &pbf, Some(&pbf), None, false, &theme, bucket)
                {
                    fingerprint ^= baked.signature.rotate_left(bucket);
                    fingerprint ^= baked.building_signature.rotate_right(bucket);
                    // Content strength: signature-neutral capture bugs must
                    // still change the fingerprint. Legacy shadow sections
                    // are deliberately absent because no renderer reads them.
                    let region_points: u64 = baked
                        .regions
                        .iter()
                        .flat_map(|region| {
                            region
                                .main
                                .iter()
                                .chain(&region.sunk)
                                .chain(&region.lifted_outlines)
                        })
                        .flat_map(|shape| shape.iter())
                        .map(|ring| ring.len() as u64)
                        .sum();
                    let building_points: u64 = baked
                        .buildings
                        .iter()
                        .flat_map(|building| building.rings.iter())
                        .map(|ring| ring.len() as u64)
                        .sum();
                    fingerprint ^= (baked.regions.len() as u64)
                        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        .rotate_left(bucket * 2);
                    fingerprint ^= region_points
                        .wrapping_mul(0xbf58_476d_1ce4_e5b9)
                        .rotate_right(bucket * 2);
                    fingerprint ^= (baked.buildings.len() as u64)
                        .wrapping_mul(0x94d0_49bb_1331_11eb)
                        .rotate_left(bucket * 3);
                    fingerprint ^= building_points
                        .wrapping_mul(0xd6e8_feb8_6659_fd93)
                        .rotate_right(bucket * 3);
                }
            }
        })
        .map_err(|err| format!("iterate {}: {err}", input.display()))?;
    Ok(fingerprint)
}

/// Rewrite `input` to `output` with baked faces appended.
///
/// Blocking and CPU-hungry (it uses every core but two) — minutes on a
/// city, hours on a continent. Decode, bake, compression errors and panics
/// are isolated per tile: the source tile is copied through, recorded in
/// [`FaceBakeStats::skipped`], and left to the renderer's live fallback.
pub fn bake_faces(options: &FaceBakeOptions) -> Result<FaceBakeStats, String> {
    let input = options.input.as_path();
    let output = options.output.as_path();
    let quality = options.brotli_quality;
    // Detail, not a headline: the host has already said what this stage
    // is for, and a parameter dump makes a poor title.
    crate::note!(
        "faces",
        "  zooms {:?}, buckets {:?} (z14) / native (below), threshold {}ms, brotli q{}",
        options.zooms,
        options.buckets,
        options.threshold_ms,
        quality
    );

    let mut reader =
        MbtilesReader::open(input).map_err(|err| format!("open {}: {err}", input.display()))?;
    let metadata = reader
        .get_metadata()
        .map_err(|err| format!("metadata {}: {err}", input.display()))?;

    let mut dz_reader = match options.bridge_dz.as_deref() {
        Some(path) => Some(
            MbtilesReader::open(path)
                .map_err(|err| format!("open {}: {err}", path.display()))?,
        ),
        None => None,
    };
    // Solved-dz coverage bounds (mirrors the renderer's load path): inside
    // them the dz archive is authoritative even when a tile has no row.
    let dz_bounds: Option<(f64, f64, f64, f64)> = dz_reader.as_mut().and_then(|r| {
        let meta = r.get_metadata().ok()?;
        let bounds: Vec<f64> = meta
            .get("bounds")?
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        (bounds.len() == 4).then(|| (bounds[0], bounds[1], bounds[2], bounds[3]))
    });

    let mut writer = MbtilesWriter::create(output)
        .map_err(|err| format!("create {}: {err}", output.display()))?;
    for (key, value) in &metadata {
        if key == "makepad_payload" {
            continue;
        }
        writer.set_metadata(key.clone(), value.clone());
    }
    let payload = metadata
        .get("makepad_payload")
        .map(|v| format!("{v}+faces-1"))
        .unwrap_or_else(|| "v2-faces-1".to_string());
    writer.set_metadata("makepad_payload", payload);
    writer.set_tile_compression(TileCompression::Brotli { quality }, None);

    let theme = probe_compiled_theme();
    let t0 = Instant::now();

    // Source iteration is ascending-rowid (same rowid scheme as the writer
    // requires), so the parallel bake must restore order with a sequence-
    // keyed reorder buffer before writing.
    let mut work: Vec<(u8, u32, u32, Vec<u8>, Option<Vec<u8>>, bool)> = Vec::new();
    {
        let probe_dz = |zoom: u8, col: u32, row: u32| -> (Option<Vec<u8>>, bool) {
            if zoom != 14 {
                return (None, false);
            }
            let y = (1u32 << zoom) - 1 - row;
            let lon = (col as f64 + 0.5) / (1u64 << zoom) as f64 * 360.0 - 180.0;
            let lat = {
                let n = std::f64::consts::PI
                    * (1.0 - 2.0 * (y as f64 + 0.5) / (1u64 << zoom) as f64);
                n.sinh().atan().to_degrees()
            };
            let covered = dz_bounds
                .is_some_and(|b| lon >= b.0 && lon <= b.2 && lat >= b.1 && lat <= b.3);
            (None, covered)
        };
        reader
            .for_each_tile(|tile| {
                let (dz, covered) =
                    probe_dz(tile.zoom_level as u8, tile.tile_column as u32, tile.tile_row as u32);
                work.push((
                    tile.zoom_level as u8,
                    tile.tile_column as u32,
                    tile.tile_row as u32,
                    tile.tile_data,
                    dz,
                    covered,
                ));
            })
            .map_err(|err| format!("iterate {}: {err}", input.display()))?;
    }
    // dz rows need the reader mutably; fetch after iteration.
    if dz_reader.is_some() {
        for item in work.iter_mut() {
            if item.0 == 14 {
                item.4 = dz_reader.as_mut().and_then(|r| {
                    r.get_tile_decoded(i64::from(item.0), item.1 as i64, item.2 as i64)
                        .ok()
                        .flatten()
                });
            }
        }
    }
    let codec = reader.tile_codec().clone();
    let total = work.len();

    enum Baked {
        Verbatim(u8, u32, u32, Vec<u8>),
        Raw(u8, u32, u32, Vec<u8>),
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).max(1))
        .unwrap_or(4);
    // pop() takes from the back: reverse so workers drain in ascending
    // sequence and the writer's reorder buffer stays small.
    let queue = std::sync::Mutex::new(work.into_iter().enumerate().rev().collect::<Vec<_>>());
    let (tx, rx) =
        std::sync::mpsc::channel::<(usize, Baked, usize, Option<(TileKey, String)>)>();
    // Reorder window: one slow monster tile must not let the other
    // workers run the whole cell ahead of the writer — the out-of-order
    // results buffer unboundedly (observed: 185GB on a Chongqing cell).
    let written_next = std::sync::atomic::AtomicUsize::new(0);
    const REORDER_WINDOW: usize = 768;
    let limit = options.limit;
    let recompress = options.recompress;
    let full = options.full;
    let threshold_ms = options.threshold_ms;
    let mut stats = FaceBakeStats { total, ..FaceBakeStats::default() };
    std::thread::scope(|scope| {
        let buckets = &options.buckets;
        let zooms = &options.zooms;
        let written_next = &written_next;
        for _ in 0..threads {
            let queue = &queue;
            let codec = codec.clone();
            let theme = &theme;
            let tx = tx.clone();
            scope.spawn(move || loop {
                let item = queue.lock().unwrap().pop();
                let Some((seq, (zoom, col, row, blob, dz_raw, dz_covered))) = item else {
                    break;
                };
                while seq
                    >= written_next
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .saturating_add(REORDER_WINDOW)
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                let y = (1u32 << zoom) - 1 - row;
                let key = TileKey { z: zoom as u32, x: col as i32, y: y as i32 };
                let should_bake = zooms.contains(&(zoom as u32)) && seq < limit;
                if !should_bake && !recompress {
                    let _ = tx.send((
                        seq,
                        Baked::Verbatim(zoom, col, row, blob),
                        0,
                        None,
                    ));
                    continue;
                }

                // Keep ownership of the source blob outside the unwind
                // boundary. On any tile-local failure it is still available
                // for a byte-verbatim fallback write.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<(Option<Vec<u8>>, usize), String> {
                        let raw = codec
                            .decode(&blob)
                            .map_err(|err| format!("codec decode: {err}"))?;
                        if !should_bake {
                            let compressed = compress_tile(
                                &TileCompression::Brotli { quality },
                                None,
                                &raw,
                            )
                            .map_err(|err| format!("compress: {err}"))?;
                            return Ok((Some(compressed), 0));
                        }

                        let pbf = decode_vector_tile_payload(&raw)
                            .map_err(|err| format!("payload decode: {err}"))?;
                        let pre_strip = pbf.len();
                        let mut pbf = strip_baked_field(pbf);
                        let had_stale_field = pbf.len() != pre_strip;
                        if std::env::var("MAPBAKE_TRACE").is_ok() {
                            eprintln!(
                                "trace: bake z{}/{}/{} ({} bytes)",
                                zoom,
                                col,
                                y,
                                raw.len()
                            );
                        }
                        let native_bucket = [zoom as u32];
                        let tile_buckets: &[u32] =
                            if zoom == 14 { buckets } else { &native_bucket };
                        let mut baked_buckets = Vec::new();
                        let t_bake = Instant::now();
                        for &bucket in tile_buckets {
                            if let Some(mut baked) = try_bake_tile_paint_faces(
                                key,
                                &pbf,
                                Some(&pbf),
                                dz_raw.as_deref(),
                                dz_covered,
                                theme,
                                bucket,
                            )? {
                                if !full {
                                    // v4 retains these slots so existing
                                    // readers remain parse-compatible, but
                                    // the draw-time mask made their payload
                                    // obsolete.
                                    baked.shadow_signature = 0;
                                    baked.shadow_shapes.clear();
                                    baked.shadow_footprints.clear();
                                }
                                if !baked.regions.is_empty() || !baked.buildings.is_empty() {
                                    baked_buckets.push(baked);
                                }
                            }
                        }
                        // Only bake tiles whose cascade is actually expensive.
                        if t_bake.elapsed().as_secs_f64() * 1e3 < threshold_ms {
                            baked_buckets.clear();
                        }
                        if baked_buckets.is_empty() {
                            if recompress || had_stale_field {
                                // Re-encode when fleet-recompressing, or
                                // when a stale field was stripped.
                                let compressed = compress_tile(
                                    &TileCompression::Brotli { quality },
                                    None,
                                    &pbf,
                                )
                                .map_err(|err| format!("compress: {err}"))?;
                                Ok((Some(compressed), 0))
                            } else {
                                Ok((None, 0))
                            }
                        } else {
                            let field = encode_baked_faces_field(&baked_buckets);
                            let field_len = field.len();
                            pbf.extend_from_slice(&field);
                            // Compress on the WORKER: heavy urban payloads
                            // at brotli q10 otherwise serialize the pass.
                            let compressed = compress_tile(
                                &TileCompression::Brotli { quality },
                                None,
                                &pbf,
                            )
                            .map_err(|err| format!("compress: {err}"))?;
                            Ok((Some(compressed), field_len))
                        }
                    },
                ));
                match result {
                    Ok(Ok((Some(payload), field_len))) => {
                        let _ = tx.send((
                            seq,
                            Baked::Raw(zoom, col, row, payload),
                            field_len,
                            None,
                        ));
                    }
                    Ok(Ok((None, field_len))) => {
                        let _ = tx.send((
                            seq,
                            Baked::Verbatim(zoom, col, row, blob),
                            field_len,
                            None,
                        ));
                    }
                    failed => {
                        let reason = match failed {
                            Ok(Err(reason)) => reason,
                            Err(payload) => crate::testmap::panic_message(payload),
                            Ok(Ok(_)) => unreachable!(),
                        };
                        crate::note!(
                            "faces",
                            "tile {}/{}/{}: {}",
                            key.z,
                            key.x,
                            key.y,
                            reason
                        );
                        let _ = tx.send((
                            seq,
                            Baked::Verbatim(zoom, col, row, blob),
                            0,
                            Some((key, reason)),
                        ));
                    }
                }
            });
        }
        drop(tx);

        let mut pending: std::collections::BTreeMap<
            usize,
            (Baked, usize, Option<(TileKey, String)>),
        > = Default::default();
        let mut next = 0usize;
        for (seq, baked, field_len, skipped) in rx {
            pending.insert(seq, (baked, field_len, skipped));
            while let Some((baked, field_len, skipped)) = pending.remove(&next) {
                match baked {
                    Baked::Verbatim(zoom, col, row, blob) => {
                        let y = (1u32 << zoom) - 1 - row;
                        writer.write_tile_xyz(zoom, col, y, &blob).expect("copy");
                        stats.copied += 1;
                    }
                    Baked::Raw(zoom, col, row, payload) => {
                        let y = (1u32 << zoom) - 1 - row;
                        writer
                            .write_tile_xyz(zoom, col, y, &payload)
                            .expect("write baked");
                        stats.baked += 1;
                        stats.field_bytes += field_len;
                    }
                }
                if let Some(skipped) = skipped {
                    stats.skipped.push(skipped);
                }
                next += 1;
                written_next.store(next, std::sync::atomic::Ordering::Relaxed);
                if next % 500 == 0 {
                    crate::tick!(
                        "faces",
                        next as f32 / total.max(1) as f32,
                        "  faces: {next}/{total} tiles, {} baked, {:.1} MiB field data, {:.0}s",
                        stats.baked,
                        stats.field_bytes as f64 / 1e6,
                        t0.elapsed().as_secs_f64()
                    );
                }
            }
        }
    });
    let written = writer
        .finish()
        .map_err(|err| format!("finish {}: {err}", output.display()))?;
    stats.file_bytes = written.file_bytes;
    crate::tick!(
        "faces",
        1.0,
        "  faces: {} tiles, {} baked, {} copied, {} skipped, {:.1} MiB field data in {:.0}s",
        stats.total,
        stats.baked,
        stats.copied,
        stats.skipped.len(),
        stats.field_bytes as f64 / 1e6,
        t0.elapsed().as_secs_f64()
    );
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::mvt::{
        encode_tile_with_profile, GeometryType, Layer, OsmType, TileFeature, TilePoint,
    };
    use makepad_mbtile_reader::MbtilesWriter;
    use makepad_widgets::map::tile::build_tile_buffers_from_mvt;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "{name}-{}-{}.mbtiles",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn malformed_tile_is_copied_and_reported_instead_of_aborting_pass() {
        let input = temp_path("makepad-face-bake-bad-input");
        let output = temp_path("makepad-face-bake-bad-output");
        let mut writer = MbtilesWriter::create(&input).unwrap();
        writer.set_metadata("format", "pbf");
        writer.write_tile_xyz(14, 8414, 5386, &[0]).unwrap();
        writer.finish().unwrap();

        let mut options = default_face_bake_options(input.clone(), output.clone());
        options.threshold_ms = 0.0;
        let stats = bake_faces(&options).unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.baked, 0);
        assert_eq!(stats.copied, 1);
        assert_eq!(stats.skipped.len(), 1);
        assert_eq!(stats.skipped[0].0, TileKey { z: 14, x: 8414, y: 5386 });
        assert!(!stats.skipped[0].1.is_empty());

        let mut reader = MbtilesReader::open(&output).unwrap();
        assert_eq!(
            reader.get_tile(14, 8414, (1_i64 << 14) - 1 - 5386).unwrap(),
            Some(vec![0])
        );
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn trimmed_and_full_tiles_bake_to_identical_renderer_buffers() {
        let features = vec![
            TileFeature {
                layer: Layer::BaseStreets,
                geometry_type: GeometryType::LineString,
                osm_type: OsmType::Way,
                id: 1,
                closed: false,
                tags: vec![
                    ("kind".to_string(), "residential".to_string()),
                    ("name".to_string(), "Parity Street".to_string()),
                ],
                paths: vec![vec![
                    TilePoint { x: 0, y: 2048 },
                    TilePoint { x: 4096, y: 2048 },
                ]],
            },
            TileFeature {
                layer: Layer::OsmPoints,
                geometry_type: GeometryType::Point,
                osm_type: OsmType::Node,
                id: 2,
                closed: false,
                tags: vec![
                    ("amenity".to_string(), "cafe".to_string()),
                    ("name".to_string(), "Parity Cafe".to_string()),
                    ("addr:housenumber".to_string(), "42".to_string()),
                ],
                paths: vec![vec![TilePoint { x: 2100, y: 1900 }]],
            },
            TileFeature {
                layer: Layer::OsmPolygons,
                geometry_type: GeometryType::Polygon,
                osm_type: OsmType::Way,
                id: 3,
                closed: true,
                tags: vec![
                    ("building".to_string(), "yes".to_string()),
                    ("height".to_string(), "12".to_string()),
                    ("roof:shape".to_string(), "gabled".to_string()),
                ],
                paths: vec![vec![
                    TilePoint { x: 1600, y: 1600 },
                    TilePoint { x: 1900, y: 1600 },
                    TilePoint { x: 1900, y: 1850 },
                    TilePoint { x: 1600, y: 1850 },
                ]],
            },
        ];
        let key = TileKey { z: 14, x: 8414, y: 5386 };
        let theme = probe_compiled_theme();
        let bake = |full: bool| {
            let mut tile = encode_tile_with_profile(features.clone(), full).unwrap();
            let bucket = try_bake_tile_paint_faces(
                key,
                &tile,
                Some(&tile),
                None,
                false,
                &theme,
                16,
            )
            .unwrap()
            .unwrap();
            tile.extend_from_slice(&encode_baked_faces_field(&[bucket]));
            build_tile_buffers_from_mvt(
                key,
                &tile,
                Some(&tile),
                None,
                false,
                &[],
                &theme,
                16,
                true,
                true,
            )
            .unwrap()
        };
        let trimmed = bake(false);
        let full = bake(true);
        assert_eq!(trimmed.labels, full.labels);
        let float_bits = |values: &[f32]| values.iter().map(|value| value.to_bits()).collect::<Vec<_>>();
        macro_rules! same_indices {
            ($($field:ident),+ $(,)?) => {$({
                assert_eq!(trimmed.$field, full.$field, stringify!($field));
            })+};
        }
        macro_rules! same_floats {
            ($($field:ident),+ $(,)?) => {$({
                assert_eq!(
                    float_bits(&trimmed.$field),
                    float_bits(&full.$field),
                    stringify!($field)
                );
            })+};
        }
        same_indices!(
            fill_indices,
            fill_misc_indices,
            casing_indices,
            stroke_indices,
            icon_indices,
            icon_high_indices,
            fringe_indices,
            fill_3d_indices,
            fill_3d_misc_indices,
            wall_indices,
            tree_indices,
            tree_cross_indices,
            tree_template_indices,
            tree_cross_template_indices,
            road_icon_indices,
        );
        same_floats!(
            fill_vertices,
            fill_misc_vertices,
            casing_vertices,
            stroke_vertices,
            icon_vertices,
            icon_high_vertices,
            shadow_disc_instances,
            fringe_vertices,
            fill_3d_vertices,
            fill_3d_misc_vertices,
            wall_vertices,
            wall_instances,
            tree_vertices,
            tree_cross_vertices,
            tree_template_vertices,
            tree_cross_template_vertices,
            tree_instances,
            road_icon_vertices,
        );
        let same_instances = |left: &[makepad_widgets::map::tile::IconInstances],
                              right: &[makepad_widgets::map::tile::IconInstances]| {
            assert_eq!(left.len(), right.len());
            for (left, right) in left.iter().zip(right) {
                assert_eq!(left.mesh_slot, right.mesh_slot);
                assert_eq!(float_bits(&left.data), float_bits(&right.data));
            }
        };
        same_instances(&trimmed.icon_instances, &full.icon_instances);
        same_instances(&trimmed.icon_high_instances, &full.icon_high_instances);
        assert_eq!(trimmed.feature_count, full.feature_count);
        assert_eq!(trimmed.mode_overlay_only, full.mode_overlay_only);
        assert_eq!(trimmed.render_zoom, full.render_zoom);
    }

    #[test]
    #[ignore = "25 real Amsterdam tiles across three buckets; explicit bake audit"]
    fn amsterdam_seed_tiles_do_not_panic_in_any_testmap_bucket() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../seed-files/amsterdam-tiles");
        let mut files = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("decoded"))
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.len(), 25);
        let theme = probe_compiled_theme();
        for path in files {
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let mut parts = stem.split('-');
            let z = parts.next().unwrap().trim_start_matches('z').parse().unwrap();
            let x = parts.next().unwrap().trim_start_matches('x').parse().unwrap();
            let y = parts.next().unwrap().trim_start_matches('y').parse().unwrap();
            let key = TileKey { z, x, y };
            let pbf = std::fs::read(&path).unwrap();
            for bucket in DEFAULT_BUCKETS {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    try_bake_tile_paint_faces(
                        key,
                        &pbf,
                        Some(&pbf),
                        None,
                        false,
                        &theme,
                        bucket,
                    )
                }));
                assert!(result.is_ok(), "{} bucket {bucket} panicked", path.display());
                assert!(
                    result.unwrap().is_ok(),
                    "{} bucket {bucket} returned an error",
                    path.display()
                );
            }
        }
    }
}
