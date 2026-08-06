//! Painter-cascade face baker (payload v2-faces-1).
//!
//! Reads a base archive, replays the renderer's road-union pipeline for
//! every z14 tile at buckets 14/15/16 (bake_tile_paint_faces — the exact
//! code path the app runs), appends the resulting field-101 stream to the
//! tile payload, and writes a new archive. All other tiles and metadata are
//! copied verbatim; readers that ignore field 101 see an identical archive.
//!
//! Usage:
//!   makepad-map-bake <in.mbtiles> <out.mbtiles> [--bridge-dz <dz.mbtiles>]
//!       [--brotli-quality N] [--limit N]

use makepad_mbtile_reader::{compress_tile, MbtilesReader, MbtilesWriter, TileCompression};
use makepad_widgets::map::style::probe_compiled_theme;
use makepad_widgets::map::geometry::TileKey;
use makepad_widgets::map::tile::{
    bake_tile_paint_faces, decode_vector_tile_payload, encode_baked_faces_field,
};
use std::path::Path;

/// Default buckets. Europe policy (data-cost analysis): bake 14 and 16
/// only — native zoom and deep overzoom, where cost concentrates. There is
/// NO cross-bucket reuse: the signature guard requires the exact bucket's
/// styling structure, so rz15 (a transient gesture band) intentionally
/// falls back to the runtime cascade. rz17+ was always runtime (outside
/// the 14..=16 consumption window, reusing nothing).
const DEFAULT_BUCKETS: [u32; 3] = [14, 15, 16];

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <in.mbtiles> <out.mbtiles> [--bridge-dz dz.mbtiles] [--brotli-quality N] [--limit N]", args[0]);
        std::process::exit(1);
    }
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);
    let mut bridge_dz_path: Option<String> = None;
    let mut quality = 10u32;
    let mut recompress = false;
    let mut limit = usize::MAX;
    let mut threshold_ms = 60.0f64;
    let mut fingerprint_only = false;
    let mut buckets: Vec<u32> = DEFAULT_BUCKETS.to_vec();
    // Tile zooms to bake. z14 tiles use the --buckets list; mid-zoom tiles
    // bake their native bucket only (the runtime cascade at z11-13 city
    // tiles measured 117-340ms — worse than z14). When rerunning over an
    // archive that ALREADY carries streams (e.g. adding mid-zooms to a
    // finished z14 bake), the zoom sets must not overlap: a baked tile
    // appends field 101, and a reader takes the FIRST field it finds.
    let mut zooms: Vec<u32> = vec![14];
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--bridge-dz" => {
                bridge_dz_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--recompress" => {
                recompress = true;
                i += 1;
                continue;
            }
            "--brotli-quality" => {
                quality = args[i + 1].parse().expect("quality");
                i += 2;
            }
            "--limit" => {
                limit = args[i + 1].parse().expect("limit");
                i += 2;
            }
            "--fingerprint" => {
                fingerprint_only = true;
                i += 1;
                continue;
            }
            "--threshold-ms" => {
                threshold_ms = args[i + 1].parse().expect("threshold");
                i += 2;
            }
            "--buckets" => {
                buckets = args[i + 1]
                    .split(',')
                    .map(|b| b.trim().parse().expect("bucket"))
                    .collect();
                i += 2;
            }
            "--zooms" => {
                zooms = args[i + 1]
                    .split(',')
                    .map(|z| z.trim().parse().expect("zoom"))
                    .collect();
                i += 2;
            }
            other => panic!("unknown arg {other}"),
        }
    }
    eprintln!(
        "map-bake: zooms {zooms:?}, buckets {buckets:?} (z14) / native (below), threshold {threshold_ms}ms, brotli q{quality}"
    );

    let mut reader = MbtilesReader::open(input).expect("open input");
    let metadata = reader.get_metadata().expect("metadata");
    if fingerprint_only {
        // Version handshake: bake the input's first z14 tile across the
        // fleet buckets and print the xor of the cascade signatures. Any
        // semantic drift anywhere in the paint pipeline changes this, so
        // the dispatcher can refuse a stale worker BEFORE it bakes junk.
        let theme = probe_compiled_theme();
        let mut fingerprint = 0u64;
        let codec = reader.tile_codec().clone();
        reader
            .for_each_tile(|tile| {
                if tile.zoom_level != 14 || fingerprint != 0 {
                    return;
                }
                let raw = codec.decode(&tile.tile_data).expect("decode");
                let pbf = decode_vector_tile_payload(&raw).expect("payload");
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
            .expect("iterate");
        println!("MAPBAKE-FINGERPRINT {fingerprint:016x}");
        return;
    }
    let mut dz_reader = bridge_dz_path
        .as_deref()
        .map(|p| MbtilesReader::open(Path::new(p)).expect("open bridge-dz"));
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

    let mut writer = MbtilesWriter::create(output).expect("create output");
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
    let t0 = std::time::Instant::now();

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
            .expect("iterate");
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
    let queue = std::sync::Mutex::new(
        work.into_iter().enumerate().rev().collect::<Vec<_>>(),
    );
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Baked, usize)>();
    // Reorder window: one slow monster tile must not let the other
    // workers run the whole cell ahead of the writer — the out-of-order
    // results buffer unboundedly (observed: 185GB on a Chongqing cell).
    let written_next = std::sync::atomic::AtomicUsize::new(0);
    const REORDER_WINDOW: usize = 768;
    std::thread::scope(|scope| {
        let buckets = &buckets;
        let zooms = &zooms;
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
                        let compressed = compress_tile(
                            &TileCompression::Brotli { quality },
                            None,
                            &raw,
                        )
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
                let tile_buckets: &[u32] =
                    if zoom == 14 { buckets } else { &native_bucket };
                let mut baked_buckets = Vec::new();
                let t_bake = std::time::Instant::now();
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
                // Only bake tiles whose cascade is actually expensive: the
                // stream exists to rescue heavy urban builds, and baking
                // every rural tile costs archive size for builds that were
                // already fast. Threshold on the measured cascade replay.
                if t_bake.elapsed().as_secs_f64() * 1e3 < threshold_ms {
                    baked_buckets.clear();
                }
                if baked_buckets.is_empty() {
                    if recompress || had_stale_field {
                        // Re-encode when fleet-recompressing, or when a
                        // stale baked field was stripped (verbatim would
                        // resurrect it).
                        let compressed = compress_tile(
                            &TileCompression::Brotli { quality },
                            None,
                            &pbf,
                        )
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
                    let compressed = compress_tile(
                        &TileCompression::Brotli { quality },
                        None,
                        &appended,
                    )
                    .expect("compress");
                    let _ = tx.send((seq, Baked::Raw(zoom, col, row, compressed), field_len));
                }
            });
        }
        drop(tx);

        let mut pending: std::collections::BTreeMap<usize, (Baked, usize)> = Default::default();
        let mut next = 0usize;
        let mut baked_tiles = 0usize;
        let mut baked_bytes = 0usize;
        let mut copied = 0usize;
        for (seq, baked, field_len) in rx {
            pending.insert(seq, (baked, field_len));
            while let Some((baked, field_len)) = pending.remove(&next) {
                match baked {
                    Baked::Verbatim(zoom, col, row, blob) => {
                        let y = (1u32 << zoom) - 1 - row;
                        writer.write_tile_xyz(zoom, col, y, &blob).expect("copy");
                        copied += 1;
                    }
                    Baked::Raw(zoom, col, row, payload) => {
                        let y = (1u32 << zoom) - 1 - row;
                        writer.write_tile_xyz(zoom, col, y, &payload).expect("write baked");
                        baked_tiles += 1;
                        baked_bytes += field_len;
                    }
                }
                next += 1;
                written_next.store(next, std::sync::atomic::Ordering::Relaxed);
                if next % 2000 == 0 {
                    eprintln!(
                        "bake: {next}/{total} tiles, {baked_tiles} baked, {:.1} MiB field data, {:.0}s",
                        baked_bytes as f64 / 1e6,
                        t0.elapsed().as_secs_f64()
                    );
                }
            }
        }
        eprintln!(
            "bake finished: {total} tiles, {baked_tiles} baked, {copied} copied, {:.1} MiB field data",
            baked_bytes as f64 / 1e6
        );
    });
    let stats = writer.finish().expect("finish");
    println!(
        "done: {total} tiles, output {:?}, {:.0}s",
        stats,
        t0.elapsed().as_secs_f64()
    );
}
