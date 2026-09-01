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
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FaceBakeStats {
    pub total: usize,
    pub baked: usize,
    pub copied: usize,
    pub field_bytes: usize,
    pub file_bytes: u64,
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
/// buckets and return the xor of the cascade signatures. Any semantic drift
/// anywhere in the paint pipeline changes this, so a dispatcher can refuse
/// a stale worker BEFORE it bakes junk.
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
                    fingerprint ^= baked.shadow_signature.rotate_right(bucket);
                    // Content strength: signature-neutral capture bugs
                    // (e.g. a gated sweep baking empty shadow sets) must
                    // still change the fingerprint.
                    let shadow_points: u64 = baked
                        .shadow_shapes
                        .iter()
                        .flat_map(|shape| shape.iter())
                        .map(|ring| ring.len() as u64)
                        .sum();
                    fingerprint ^= (baked.shadow_shapes.len() as u64)
                        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                        .rotate_left(bucket * 2);
                    fingerprint ^= shadow_points
                        .wrapping_mul(0xbf58_476d_1ce4_e5b9)
                        .rotate_right(bucket * 2);
                }
            }
        })
        .map_err(|err| format!("iterate {}: {err}", input.display()))?;
    Ok(fingerprint)
}

/// Rewrite `input` to `output` with baked faces appended.
///
/// Blocking and CPU-hungry (it uses every core but two) — minutes on a
/// city, hours on a continent. The per-tile work inside the worker pool
/// panics rather than unwinding a half-written tile through the reorder
/// buffer; a host that cannot afford a panic should call this on a thread
/// it can lose, or catch it at its own boundary.
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
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Baked, usize)>();
    // Reorder window: one slow monster tile must not let the other
    // workers run the whole cell ahead of the writer — the out-of-order
    // results buffer unboundedly (observed: 185GB on a Chongqing cell).
    let written_next = std::sync::atomic::AtomicUsize::new(0);
    const REORDER_WINDOW: usize = 768;
    let limit = options.limit;
    let recompress = options.recompress;
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
                if !zooms.contains(&(zoom as u32)) || seq >= limit {
                    if recompress {
                        // Fleet mode: cells arrive sliced at a throwaway
                        // quality; the worker re-encodes EVERY tile at the
                        // target quality so compression parallelizes over
                        // the whole fleet and the final archive is uniform.
                        let raw = codec.decode(&blob).expect("codec decode");
                        let compressed =
                            compress_tile(&TileCompression::Brotli { quality }, None, &raw)
                                .expect("compress");
                        let _ = tx.send((seq, Baked::Raw(zoom, col, row, compressed), 0));
                    } else {
                        let _ = tx.send((seq, Baked::Verbatim(zoom, col, row, blob), 0));
                    }
                    continue;
                }
                let raw = codec.decode(&blob).expect("codec decode");
                let pbf = decode_vector_tile_payload(&raw).expect("payload");
                let pre_strip = pbf.len();
                let pbf = strip_baked_field(pbf);
                let had_stale_field = pbf.len() != pre_strip;
                let y = (1u32 << zoom) - 1 - row;
                let key = TileKey { z: zoom as u32, x: col as i32, y: y as i32 };
                if std::env::var("MAPBAKE_TRACE").is_ok() {
                    eprintln!("trace: bake z{}/{}/{} ({} bytes)", zoom, col, y, raw.len());
                }
                let native_bucket = [zoom as u32];
                let tile_buckets: &[u32] = if zoom == 14 { buckets } else { &native_bucket };
                let mut baked_buckets = Vec::new();
                let t_bake = Instant::now();
                for &bucket in tile_buckets.iter() {
                    if let Some(baked) = bake_tile_paint_faces(
                        key,
                        &pbf,
                        Some(&pbf),
                        dz_raw.as_deref(),
                        dz_covered,
                        theme,
                        bucket,
                    ) {
                        if !baked.regions.is_empty() {
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
                        // Re-encode when fleet-recompressing, or when a
                        // stale baked field was stripped (verbatim would
                        // resurrect it).
                        let compressed =
                            compress_tile(&TileCompression::Brotli { quality }, None, &pbf)
                                .expect("compress");
                        let _ = tx.send((seq, Baked::Raw(zoom, col, row, compressed), 0));
                    } else {
                        let _ = tx.send((seq, Baked::Verbatim(zoom, col, row, blob), 0));
                    }
                } else {
                    let field = encode_baked_faces_field(&baked_buckets);
                    let field_len = field.len();
                    let mut appended = pbf;
                    appended.extend_from_slice(&field);
                    // Compress on the WORKER: heavy urban payloads at
                    // brotli q10 took 100-300ms each and serialized the
                    // whole bake through the single writer thread.
                    let compressed =
                        compress_tile(&TileCompression::Brotli { quality }, None, &appended)
                            .expect("compress");
                    let _ = tx.send((seq, Baked::Raw(zoom, col, row, compressed), field_len));
                }
            });
        }
        drop(tx);

        let mut pending: std::collections::BTreeMap<usize, (Baked, usize)> = Default::default();
        let mut next = 0usize;
        for (seq, baked, field_len) in rx {
            pending.insert(seq, (baked, field_len));
            while let Some((baked, field_len)) = pending.remove(&next) {
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
        "  faces: {} tiles, {} baked, {} copied, {:.1} MiB field data in {:.0}s",
        stats.total,
        stats.baked,
        stats.copied,
        stats.field_bytes as f64 / 1e6,
        t0.elapsed().as_secs_f64()
    );
    Ok(stats)
}
