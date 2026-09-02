//! The Amsterdam bake report: the real archive, the real bake path, one
//! number per stream. It is the yardstick every renderer change is measured
//! against — bytes per tile per stream, milliseconds per tile, the resident
//! total for the route app's start view (centre 4.8952,52.3702, view zoom
//! 15.6, tilt 60°, a 1280×800 viewport, z14 tiles overzoomed to the z16
//! keyframe). Needs the local world archive, so it is ignored by default:
//!
//! `cargo test -p makepad-widgets --features maps bake_report -- --ignored --nocapture`

use super::geometry::{lon_lat_to_normalized, tile_world_size, TileKey, TILE_SIZE};
use super::style::probe_compiled_theme;
use super::tile::{load_local_tile_batch, TileBuffers};
use makepad_mbtile_reader::TileArchiveReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

const CENTER_LON_LAT: (f64, f64) = (4.8952, 52.3702);
const VIEW_ZOOM: f64 = 15.6;
const TILT_DEG: f64 = 60.0;
const VIEW_SIZE: (f64, f64) = (1280.0, 800.0);
/// The archive's top zoom: the view clamps its requests here.
const REQUEST_ZOOM: u32 = 14;
/// `MapView::render_bucket` for the start zoom: `round(15.6)`.
const RENDER_ZOOM: u32 = 16;

fn archive_path() -> Option<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    ["../local/maps/world.mkmap", "../../local/maps/world.mkmap"]
        .iter()
        .map(|rel| manifest.join(rel))
        .find(|path| path.join("root.mkidx").is_file())
}

/// The tile set `MapView::visible_tile_keys` requests for the start view:
/// the tilt-stretched viewport in request-zoom world pixels, one prefetch
/// tile outward, nearest-first.
fn start_view_tiles() -> Vec<TileKey> {
    let world_size = tile_world_size(REQUEST_ZOOM);
    let center = lon_lat_to_normalized(CENTER_LON_LAT.0, CENTER_LON_LAT.1);
    let center_world = (center.x * world_size, center.y * world_size);
    let overzoom = 2.0_f64.powf(VIEW_ZOOM - REQUEST_ZOOM as f64).max(1.0);
    let half_w = VIEW_SIZE.0 * 0.5;
    let half_h = VIEW_SIZE.1 * 0.5 / TILT_DEG.to_radians().cos().max(1e-3);
    let half = (half_w / overzoom, half_h / overzoom);
    let span = |min: f64, max: f64| {
        ((min / TILE_SIZE).floor() as i32 - 1, (max / TILE_SIZE).ceil() as i32)
    };
    let (min_x, max_x) = span(center_world.0 - half.0, center_world.0 + half.0);
    let (min_y, max_y) = span(center_world.1 - half.1, center_world.1 + half.1);
    let center_tx = (center_world.0 / TILE_SIZE).floor() as i32;
    let center_ty = (center_world.1 / TILE_SIZE).floor() as i32;
    let mut keys: Vec<TileKey> = (min_y..=max_y)
        .flat_map(|y| (min_x..=max_x).map(move |x| TileKey { z: REQUEST_ZOOM, x, y }))
        .collect();
    keys.sort_unstable_by_key(|key| {
        ((key.x - center_tx).abs() + (key.y - center_ty).abs(), key.y, key.x)
    });
    keys
}

/// Every vertex/index stream of a bake, in the order the tile struct lists
/// them: (name, vertex floats, index count).
fn streams(b: &TileBuffers) -> [(&'static str, usize, usize); 11] {
    [
        ("fill", b.fill_vertices.len(), b.fill_indices.len()),
        ("casing", b.casing_vertices.len(), b.casing_indices.len()),
        ("stroke", b.stroke_vertices.len(), b.stroke_indices.len()),
        ("fringe", b.fringe_vertices.len(), b.fringe_indices.len()),
        ("icon", b.icon_vertices.len(), b.icon_indices.len()),
        ("icon_high", b.icon_high_vertices.len(), b.icon_high_indices.len()),
        ("road_icon", b.road_icon_vertices.len(), b.road_icon_indices.len()),
        ("fill_3d", b.fill_3d_vertices.len(), b.fill_3d_indices.len()),
        ("wall", b.wall_vertices.len(), b.wall_indices.len()),
        ("tree", b.tree_vertices.len(), b.tree_indices.len()),
        ("tree_cross", b.tree_cross_vertices.len(), b.tree_cross_indices.len()),
    ]
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Vertex bytes of a packed stream by (shape id, material): the icon pass
/// and the fill stream carry several kinds of geometry each, and the split
/// says which kind pays.
fn classify_packed(vertices: &[f32], into: &mut std::collections::BTreeMap<(i32, i32), usize>) {
    use crate::makepad_draw::vector::unpack_pair_f16;
    for record in vertices.chunks_exact(12) {
        let shape_id = unpack_pair_f16(record[6]).1.round() as i32;
        let material = unpack_pair_f16(record[8]).0.round() as i32;
        *into.entry((shape_id, material)).or_default() += 12 * 4;
    }
}

#[test]
#[ignore = "needs local/maps/world.mkmap; run with --ignored --nocapture"]
fn amsterdam_start_view_bake_report() {
    let Some(archive) = archive_path() else {
        eprintln!("bake report: no local world archive, skipping");
        return;
    };
    let theme = probe_compiled_theme();
    let keys = start_view_tiles();
    let mut reader = TileArchiveReader::open(&archive).expect("open archive");
    println!(
        "== Amsterdam start view: {} z{} tiles, render zoom {}, 3D on ==",
        keys.len(),
        REQUEST_ZOOM,
        RENDER_ZOOM
    );
    println!(
        "{:>14} {:>8} {:>8} {:>7} {:>9} {:>8} {:>7} {:>8} {:>8} | per-stream MiB (fill casing stroke fringe icon icon_hi road_ic fill3d wall tree treeX) + icon instances (count / KiB)",
        "tile", "raw KiB", "mvt KiB", "feats", "bake MiB", "verts", "labels", "icons", "ms"
    );
    let mut total_raw = 0usize;
    let mut total_mvt = 0usize;
    let mut total_bytes = 0usize;
    let mut total_ms = 0.0f64;
    let mut stream_totals = [0usize; 11];
    let mut total_icon_instance_bytes = 0usize;
    let mut total_wall_instance_bytes = 0usize;
    let mut icon_kinds = std::collections::BTreeMap::new();
    let mut fill_kinds = std::collections::BTreeMap::new();
    let mut casing_kinds = std::collections::BTreeMap::new();
    let mut baked = 0usize;
    for key in &keys {
        let tms_row = (1_i64 << key.z) - 1 - key.y as i64;
        let raw = reader
            .get_tile(key.z as i64, key.x as i64, tms_row)
            .ok()
            .flatten();
        let Some(raw) = raw else {
            println!("{:>14} (no tile)", format!("{}/{}/{}", key.z, key.x, key.y));
            continue;
        };
        let mvt = reader.decode_tile(&raw).map(|d| d.len()).unwrap_or(0);
        let start = Instant::now();
        let (loaded, unavailable) = load_local_tile_batch(
            &archive,
            Some(&archive),
            None,
            &[],
            &[*key],
            &theme,
            RENDER_ZOOM,
            true,
            true,
        )
        .expect("bake");
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let Some(tile) = loaded.first() else {
            println!(
                "{:>14} unavailable ({} keys)",
                format!("{}/{}/{}", key.z, key.x, key.y),
                unavailable.len()
            );
            continue;
        };
        let b = &tile.buffers;
        let per_stream = streams(b);
        let bytes = b.gpu_byte_size();
        let verts: usize = per_stream.iter().map(|(_, v, _)| v / 12).sum();
        for (slot, (_, v, i)) in per_stream.iter().enumerate() {
            stream_totals[slot] += (v + i) * 4;
        }
        total_raw += raw.len();
        total_mvt += mvt;
        total_bytes += bytes;
        total_ms += ms;
        baked += 1;
        let cols: Vec<String> = per_stream
            .iter()
            .map(|(_, v, i)| format!("{:.1}", mib((v + i) * 4)))
            .collect();
        let icon_count: usize = b
            .icon_instances
            .iter()
            .chain(b.icon_high_instances.iter())
            .map(|group| group.count())
            .sum();
        total_icon_instance_bytes += b.icon_instance_floats() * 4;
        total_wall_instance_bytes += b.wall_instances.len() * 4;
        classify_packed(&b.icon_vertices, &mut icon_kinds);
        classify_packed(&b.fill_vertices, &mut fill_kinds);
        classify_packed(&b.casing_vertices, &mut casing_kinds);
        println!(
            "{:>14} {:>8} {:>8} {:>7} {:>9.1} {:>8} {:>7} {:>8} {:>8.0} | {} + {} / {:.0}",
            format!("{}/{}/{}", key.z, key.x, key.y),
            raw.len() / 1024,
            mvt / 1024,
            b.feature_count,
            mib(bytes),
            verts,
            b.labels.len(),
            icon_count,
            ms,
            cols.join(" "),
            icon_count,
            b.icon_instance_floats() as f64 * 4.0 / 1024.0
        );
        if !b.stage_summary.is_empty() {
            println!("{:>14} stages: {}", "", b.stage_summary);
        }
    }
    println!("== totals: {} tiles baked ==", baked);
    println!(
        "raw {:.1} MiB  mvt {:.1} MiB  baked {:.1} MiB ({:.0}x the raw bytes)  bake {:.0} ms total, {:.0} ms/tile",
        mib(total_raw),
        mib(total_mvt),
        mib(total_bytes),
        total_bytes as f64 / total_raw.max(1) as f64,
        total_ms,
        total_ms / baked.max(1) as f64
    );
    let names = [
        "fill", "casing", "stroke", "fringe", "icon", "icon_high", "road_icon", "fill_3d",
        "wall", "tree", "tree_cross",
    ];
    for (name, bytes) in names.iter().zip(stream_totals.iter()) {
        println!(
            "  {:<10} {:>8.1} MiB {:>5.1}%",
            name,
            mib(*bytes),
            *bytes as f64 * 100.0 / total_bytes.max(1) as f64
        );
    }
    println!(
        "  {:<10} {:>8.1} MiB {:>5.1}%",
        "icon_inst",
        mib(total_icon_instance_bytes),
        total_icon_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64
    );
    println!(
        "  {:<10} {:>8.1} MiB {:>5.1}%",
        "wall_inst",
        mib(total_wall_instance_bytes),
        total_wall_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64
    );
    for (name, kinds) in [("icon", &icon_kinds), ("fill", &fill_kinds), ("casing", &casing_kinds)] {
        println!("== {name} stream vertex bytes by (shape, material) ==");
        let mut rows: Vec<_> = kinds.iter().collect();
        rows.sort_by_key(|(_, bytes)| std::cmp::Reverse(**bytes));
        for ((shape, material), bytes) in rows.into_iter().take(8) {
            println!("  shape {shape:>3} mat {material:>2}  {:>8.1} MiB", mib(*bytes));
        }
    }
}
