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

const BUCKETS: [u32; 3] = [14, 15, 16];

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
    let mut limit = usize::MAX;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--bridge-dz" => {
                bridge_dz_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--brotli-quality" => {
                quality = args[i + 1].parse().expect("quality");
                i += 2;
            }
            "--limit" => {
                limit = args[i + 1].parse().expect("limit");
                i += 2;
            }
            other => panic!("unknown arg {other}"),
        }
    }

    let mut reader = MbtilesReader::open(input).expect("open input");
    let metadata = reader.get_metadata().expect("metadata");
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
        let mut probe_dz = |zoom: u8, col: u32, row: u32| -> (Option<Vec<u8>>, bool) {
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
    std::thread::scope(|scope| {
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
                if zoom != 14 || seq >= limit {
                    let _ = tx.send((seq, Baked::Verbatim(zoom, col, row, blob), 0));
                    continue;
                }
                let raw = codec.decode(&blob).expect("codec decode");
                let pbf = decode_vector_tile_payload(&raw).expect("payload");
                let y = (1u32 << zoom) - 1 - row;
                let key = TileKey { z: zoom as u32, x: col as i32, y: y as i32 };
                let mut buckets = Vec::new();
                let t_bake = std::time::Instant::now();
                for bucket in BUCKETS {
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
                            buckets.push(baked);
                        }
                    }
                }
                // Only bake tiles whose cascade is actually expensive: the
                // stream exists to rescue heavy urban builds, and baking
                // every rural tile costs archive size for builds that were
                // already fast. Threshold on the measured cascade replay.
                if t_bake.elapsed().as_secs_f64() * 1e3 < 60.0 {
                    buckets.clear();
                }
                if buckets.is_empty() {
                    let _ = tx.send((seq, Baked::Verbatim(zoom, col, row, blob), 0));
                } else {
                    let field = encode_baked_faces_field(&buckets);
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
