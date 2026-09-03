//! The Amsterdam bake report: the real archive, the real bake path, one
//! number per stream. It is the yardstick every renderer change is measured
//! against — bytes per tile per stream, milliseconds per tile, the resident
//! total for the route app's start view (centre 4.8952,52.3702, view zoom
//! 15.6, tilt 60°, a 1280×800 viewport, z14 tiles overzoomed to the z16
//! keyframe). It prefers the local world archive and otherwise uses the same
//! 25 captured decoded tiles from `seed-files`, so it is ignored by default:
//!
//! `cargo test -p makepad-widgets --features maps bake_report -- --ignored --nocapture`

use super::geometry::{lon_lat_to_normalized, tile_world_size, TileKey, TILE_SIZE};
use super::style::probe_compiled_theme;
use super::tile::{
    build_tile_buffers_from_mvt, load_local_tile_batch, TileBuffers, TypedStream,
    MAP_PROP_INSTANCE_BYTES, MAP_WALL_INSTANCE_BYTES, SHADOW_DISC_INSTANCE_FLOATS,
};
use crate::makepad_draw::vector::{
    decode_face_vertex, decode_fill_vertex, decode_road_vertex, FACE_TYPED_VERTEX_BYTES,
    FILL_TYPED_VERTEX_BYTES, ROAD_TYPED_VERTEX_BYTES, ROOF_TYPED_VERTEX_BYTES,
    VECTOR_FLOATS_PER_VERTEX, VECTOR_PACKED_FLOATS_PER_VERTEX,
};
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
    let path = manifest.join("../local/maps/world.mkmap");
    path.join("root.mkidx").is_file().then_some(path)
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

struct StreamRow {
    name: &'static str,
    vertices: usize,
    source_vertices: usize,
    indices: usize,
    index_width: usize,
    unchunked_index_width: usize,
    index_bytes: usize,
    vertex_bytes: usize,
    bytes: usize,
    unchunked_bytes: usize,
    chunks: usize,
    duplicate_vertices: usize,
}

fn typed_stream(name: &'static str, stream: &TypedStream, stride: usize) -> StreamRow {
    let vertices = stream.vertex_count(stride);
    let indices = stream.index_count();
    StreamRow {
        name,
        vertices,
        source_vertices: stream.source_vertex_count(),
        indices,
        index_width: 2,
        unchunked_index_width: if stream.source_vertex_count() < 65_536 { 2 } else { 4 },
        index_bytes: indices * 2,
        vertex_bytes: vertices * stride,
        bytes: stream.byte_size(),
        unchunked_bytes: stream.unchunked_byte_size(stride),
        chunks: stream.chunks.len(),
        duplicate_vertices: stream.duplicate_vertex_count(),
    }
}

fn legacy_stream(
    name: &'static str,
    vertices: usize,
    indices: usize,
    bytes: usize,
) -> StreamRow {
    let index_bytes = indices * 4;
    StreamRow {
        name,
        vertices,
        source_vertices: vertices,
        indices,
        index_width: 4,
        unchunked_index_width: 4,
        index_bytes,
        vertex_bytes: bytes - index_bytes,
        bytes,
        unchunked_bytes: bytes,
        chunks: usize::from(vertices != 0 || indices != 0),
        duplicate_vertices: 0,
    }
}

/// Every vertex/index stream of a bake, in the order the tile struct lists.
fn streams(b: &TileBuffers) -> [StreamRow; 14] {
    let bytes = b.stream_bytes();
    [
        typed_stream("fill", &b.fill, FILL_TYPED_VERTEX_BYTES),
        legacy_stream(
            "fill_misc",
            b.fill_misc_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX,
            b.fill_misc_indices.len(),
            bytes[1],
        ),
        typed_stream("face", &b.face, FACE_TYPED_VERTEX_BYTES),
        typed_stream("casing", &b.casing, ROAD_TYPED_VERTEX_BYTES),
        typed_stream("stroke", &b.stroke, ROAD_TYPED_VERTEX_BYTES),
        typed_stream("fringe", &b.fringe, ROAD_TYPED_VERTEX_BYTES),
        legacy_stream("icon", b.icon_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, b.icon_indices.len(), bytes[6]),
        legacy_stream("icon_high", b.icon_high_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, b.icon_high_indices.len(), bytes[7]),
        legacy_stream("road_icon", b.road_icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX, b.road_icon_indices.len(), bytes[8]),
        typed_stream("fill_3d", &b.fill_3d, ROOF_TYPED_VERTEX_BYTES),
        legacy_stream("fill_3d_misc", b.fill_3d_misc_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, b.fill_3d_misc_indices.len(), bytes[10]),
        legacy_stream("wall", b.wall_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, b.wall_indices.len(), bytes[11]),
        legacy_stream("tree", b.tree_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, b.tree_indices.len(), bytes[12]),
        legacy_stream("tree_cross", b.tree_cross_vertices.len() / VECTOR_PACKED_FLOATS_PER_VERTEX, b.tree_cross_indices.len(), bytes[13]),
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
    for record in vertices.chunks_exact(VECTOR_PACKED_FLOATS_PER_VERTEX) {
        let shape_id = unpack_pair_f16(record[6]).1.round() as i32;
        let material = unpack_pair_f16(record[8]).0.round() as i32;
        *into.entry((shape_id, material)).or_default() += VECTOR_PACKED_FLOATS_PER_VERTEX * 4;
    }
}

fn classify_fill_packed(
    vertices: &[u8],
    into: &mut std::collections::BTreeMap<(i32, i32), usize>,
) {
    for record in vertices.chunks_exact(FILL_TYPED_VERTEX_BYTES) {
        let code = decode_fill_vertex(record).params.to_f32().0.round() as i32;
        let (shape_id, material) = match code {
            30 | 32 => (code, 5),
            31 => (code, 0),
            material => (0, material),
        };
        *into.entry((shape_id, material)).or_default() += FILL_TYPED_VERTEX_BYTES;
    }
}

/// Face records are shape-0 fills by construction; the split is by material.
fn classify_face_packed(
    vertices: &[u8],
    into: &mut std::collections::BTreeMap<(i32, i32), usize>,
) {
    for record in vertices.chunks_exact(FACE_TYPED_VERTEX_BYTES) {
        let meta = decode_face_vertex(record).params.to_f32().0;
        let material = ((meta % 64.0) / 8.0).floor() as i32;
        *into.entry((0, material)).or_default() += FACE_TYPED_VERTEX_BYTES;
    }
}

fn classify_road_packed(
    vertices: &[u8],
    into: &mut std::collections::BTreeMap<(i32, i32), usize>,
) {
    for record in vertices.chunks_exact(ROAD_TYPED_VERTEX_BYTES) {
        let meta = decode_road_vertex(record).params.to_f32().0 % 1024.0;
        let kind = (meta / 256.0).floor() as i32;
        let dash = ((meta % 256.0) / 64.0).floor() as i32;
        let low = meta % 64.0;
        let shape_id = if kind == 1 {
            0
        } else {
            100 + match dash {
                1 => 10,
                2 => 11,
                3 => 12,
                _ => 0,
            }
        };
        let material = if kind == 1 { (low / 8.0).floor() as i32 } else { 0 };
        *into.entry((shape_id, material)).or_default() += ROAD_TYPED_VERTEX_BYTES;
    }
}

#[test]
#[ignore = "run with --ignored --nocapture"]
fn amsterdam_start_view_bake_report() {
    let archive = archive_path();
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../seed-files/amsterdam-tiles");
    if archive.is_none() && !fixture_root.is_dir() {
        eprintln!("bake report: no local world archive or Amsterdam fixtures, skipping");
        return;
    }
    let theme = probe_compiled_theme();
    let keys = start_view_tiles();
    let mut reader = archive
        .as_ref()
        .map(|archive| TileArchiveReader::open(archive).expect("open archive"));
    println!(
        "== Amsterdam start view: {} z{} tiles, render zoom {}, 3D on ==",
        keys.len(),
        REQUEST_ZOOM,
        RENDER_ZOOM
    );
    println!(
        "source: {}",
        if archive.is_some() { "local archive" } else { "in-repo captured tiles" }
    );
    println!(
        "{:>14} {:>8} {:>8} {:>7} {:>9} {:>8} {:>7} {:>8} {:>8} | per-stream MiB (fill fill_misc face casing stroke fringe icon icon_hi road_ic fill3d fill3d_misc wall tree treeX) + instances (icons count/KiB, shadow discs count/KiB)",
        "tile", "raw KiB", "mvt KiB", "feats", "bake MiB", "verts", "labels", "icons", "ms"
    );
    let mut total_raw = 0usize;
    let mut total_mvt = 0usize;
    let mut total_bytes = 0usize;
    let mut total_ms = 0.0f64;
    let mut flat_total_bytes = 0usize;
    let mut flat_unchunked_total_bytes = 0usize;
    let mut flat_fringe_bytes = 0usize;
    let mut flat_baked = 0usize;
    let mut stream_totals = [0usize; 14];
    let mut unchunked_stream_totals = [0usize; 14];
    let mut flat_stream_totals = [0usize; 14];
    let mut flat_unchunked_stream_totals = [0usize; 14];
    let mut total_u16_index_bytes = 0usize;
    let mut total_u32_index_bytes = 0usize;
    let mut total_unchunked_u16_index_bytes = 0usize;
    let mut total_unchunked_u32_index_bytes = 0usize;
    let mut total_unchunked_vertex_bytes = 0usize;
    let mut typed_u32_index_count = 0usize;
    let mut total_vertex_bytes = 0usize;
    let mut total_duplicate_vertices = 0usize;
    let mut total_typed_source_vertices = 0usize;
    let mut max_chunk_count = 0usize;
    let mut total_icon_instance_bytes = 0usize;
    let mut total_shadow_disc_instance_bytes = 0usize;
    let mut total_shadow_disc_instances = 0usize;
    let mut total_wall_instance_bytes = 0usize;
    let mut total_tree_instance_bytes = 0usize;
    let mut total_stalk_instance_bytes = 0usize;
    let mut total_stalk_instances = 0usize;
    let mut total_stoplight_instance_bytes = 0usize;
    let mut total_stoplight_instances = 0usize;
    let mut icon_kinds = std::collections::BTreeMap::new();
    let mut fill_kinds = std::collections::BTreeMap::new();
    let mut face_kinds = std::collections::BTreeMap::new();
    let mut casing_kinds = std::collections::BTreeMap::new();
    let mut baked = 0usize;
    for key in &keys {
        let (raw, decoded, b, flat, ms) = if let (Some(archive), Some(reader)) =
            (archive.as_ref(), reader.as_mut())
        {
            let tms_row = (1_i64 << key.z) - 1 - key.y as i64;
            let Some(raw) = reader
                .get_tile(key.z as i64, key.x as i64, tms_row)
                .ok()
                .flatten()
            else {
                println!("{:>14} (no tile)", format!("{}/{}/{}", key.z, key.x, key.y));
                continue;
            };
            let decoded = reader.decode_tile(&raw).unwrap_or_default();
            let start = Instant::now();
            let (loaded, unavailable) = load_local_tile_batch(
                archive,
                Some(archive),
                None,
                &[],
                &[*key],
                &theme,
                RENDER_ZOOM,
                true,
                false,
                true,
            )
            .expect("bake");
            let ms = start.elapsed().as_secs_f64() * 1e3;
            let Some(tile) = loaded.into_iter().next() else {
                println!(
                    "{:>14} unavailable ({} keys)",
                    format!("{}/{}/{}", key.z, key.x, key.y),
                    unavailable.len()
                );
                continue;
            };
            let flat = load_local_tile_batch(
                archive,
                Some(archive),
                None,
                &[],
                &[*key],
                &theme,
                RENDER_ZOOM,
                false,
                true,
                true,
            )
            .expect("flat bake")
            .0
            .into_iter()
            .next()
            .map(|tile| tile.buffers);
            (raw, decoded, tile.buffers, flat, ms)
        } else {
            let stem = format!("z{}-x{}-y{}", key.z, key.x, key.y);
            let raw = std::fs::read(fixture_root.join(format!("{stem}.raw")))
                .unwrap_or_else(|error| panic!("read captured {stem}.raw: {error}"));
            let decoded = std::fs::read(fixture_root.join(format!("{stem}.decoded")))
                .unwrap_or_else(|error| panic!("read captured {stem}.decoded: {error}"));
            let start = Instant::now();
            let b = build_tile_buffers_from_mvt(
                *key, &decoded, Some(&decoded), None, false, &[], &theme, RENDER_ZOOM,
                true, false, true,
            )
            .expect("fixture bake");
            let ms = start.elapsed().as_secs_f64() * 1e3;
            let flat = Some(
                build_tile_buffers_from_mvt(
                    *key, &decoded, Some(&decoded), None, false, &[], &theme, RENDER_ZOOM,
                    false, true, true,
                )
                .expect("flat fixture bake"),
            );
            (raw, decoded, b, flat, ms)
        };
        let mvt = decoded.len();
        // MAP_BAKE_REPORT_DUMP=<dir>: keep the raw (archive) and decoded tile
        // bytes so a data-side survey can read them without the archive.
        if let Some(dir) = std::env::var_os("MAP_BAKE_REPORT_DUMP") {
            let dir = Path::new(&dir);
            let _ = std::fs::create_dir_all(dir);
            let stem = format!("z{}-x{}-y{}", key.z, key.x, key.y);
            let _ = std::fs::write(dir.join(format!("{stem}.raw")), &raw);
            let _ = std::fs::write(dir.join(format!("{stem}.decoded")), &decoded);
        }
        if let Some(flat) = flat.as_ref() {
            for (slot, (before, after)) in flat
                .unchunked_stream_bytes()
                .into_iter()
                .zip(flat.stream_bytes())
                .enumerate()
            {
                flat_unchunked_stream_totals[slot] += before;
                flat_stream_totals[slot] += after;
            }
            flat_total_bytes += flat.byte_size();
            flat_unchunked_total_bytes += flat.byte_size()
                + flat
                    .unchunked_stream_bytes()
                    .iter()
                    .zip(flat.stream_bytes().iter())
                    .map(|(before, after)| before.saturating_sub(*after))
                    .sum::<usize>();
            flat_fringe_bytes += flat.fringe.byte_size();
            flat_baked += 1;
            total_duplicate_vertices += flat.typed_duplicate_vertices();
            total_typed_source_vertices += flat.typed_source_vertices();
            max_chunk_count = max_chunk_count.max(flat.max_typed_chunk_count());
        }
        let per_stream = streams(&b);
        let bytes = b.byte_size();
        let verts: usize = per_stream.iter().map(|row| row.vertices).sum();
        for (slot, row) in per_stream.iter().enumerate() {
            stream_totals[slot] += row.bytes;
            unchunked_stream_totals[slot] += row.unchunked_bytes;
            if row.index_width == 2 {
                total_u16_index_bytes += row.index_bytes;
            } else {
                total_u32_index_bytes += row.index_bytes;
            }
            if row.unchunked_index_width == 2 {
                total_unchunked_u16_index_bytes += row.indices * 2;
            } else {
                total_unchunked_u32_index_bytes += row.indices * 4;
                if row.index_width == 2 {
                    typed_u32_index_count += row.indices;
                }
            }
            total_unchunked_vertex_bytes +=
                row.unchunked_bytes - row.indices * row.unchunked_index_width;
            total_vertex_bytes += row.vertex_bytes;
        }
        total_duplicate_vertices += b.typed_duplicate_vertices();
        total_typed_source_vertices += b.typed_source_vertices();
        max_chunk_count = max_chunk_count.max(b.max_typed_chunk_count());
        total_raw += raw.len();
        total_mvt += mvt;
        total_bytes += bytes;
        total_ms += ms;
        baked += 1;
        let cols: Vec<String> = per_stream
            .iter()
            .map(|row| format!("{:.1}", mib(row.bytes)))
            .collect();
        let icon_count: usize = b
            .icon_instances
            .iter()
            .chain(b.icon_high_instances.iter())
            .map(|group| group.count())
            .sum();
        total_icon_instance_bytes += b.icon_instance_floats() * 4;
        total_shadow_disc_instance_bytes += b.shadow_disc_instances.len() * 4;
        total_shadow_disc_instances +=
            b.shadow_disc_instances.len() / SHADOW_DISC_INSTANCE_FLOATS;
        total_wall_instance_bytes += b.wall_instances.len() * MAP_WALL_INSTANCE_BYTES;
        total_tree_instance_bytes += (b.tree_instances.len()
            + b.tree_template_indices.len()
            + b.tree_template_vertices.len()
            + b.tree_cross_template_indices.len()
            + b.tree_cross_template_vertices.len())
            * 4;
        total_stalk_instance_bytes += (b.stalk_template_indices.len()
            + b.stalk_template_vertices.len())
            * 4
            + b.stalk_instances.len() * MAP_PROP_INSTANCE_BYTES;
        total_stalk_instances += b.stalk_instances.len();
        total_stoplight_instance_bytes += (b.stoplight_template_indices.len()
            + b.stoplight_template_vertices.len())
            * 4
            + b.stoplight_instances.len() * MAP_PROP_INSTANCE_BYTES;
        total_stoplight_instances += b.stoplight_instances.len();
        classify_packed(&b.icon_vertices, &mut icon_kinds);
        for chunk in &b.fill.chunks {
            classify_fill_packed(&chunk.vertices, &mut fill_kinds);
        }
        for chunk in &b.face.chunks {
            classify_face_packed(&chunk.vertices, &mut face_kinds);
        }
        for chunk in &b.casing.chunks {
            classify_road_packed(&chunk.vertices, &mut casing_kinds);
        }
        println!(
            "{:>14} {:>8} {:>8} {:>7} {:>9.1} {:>8} {:>7} {:>8} {:>8.0} | {} + {} / {:.0}, {} / {:.0}",
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
            b.icon_instance_floats() as f64 * 4.0 / 1024.0,
            b.shadow_disc_instances.len() / SHADOW_DISC_INSTANCE_FLOATS,
            b.shadow_disc_instances.len() as f64 * 4.0 / 1024.0,
        );
        for row in &per_stream {
            println!(
                "{:>14} {:<12} vertices {:>8} (source {:>8})  indices {:>8}  u{} (was u{})  index bytes {:>9}  chunks {:>2}  dup {:>5}",
                "",
                row.name,
                row.vertices,
                row.source_vertices,
                row.indices,
                row.index_width * 8,
                row.unchunked_index_width * 8,
                row.index_bytes,
                row.chunks,
                row.duplicate_vertices,
            );
        }
        if !b.stage_summary.is_empty() {
            println!("{:>14} stages: {}", "", b.stage_summary);
        }
    }
    println!("== totals: {} tiles baked ==", baked);
    let unchunked_total_bytes = total_bytes
        + unchunked_stream_totals
            .iter()
            .zip(stream_totals.iter())
            .map(|(before, after)| before.saturating_sub(*after))
            .sum::<usize>();
    println!(
        "tilted raw {:.1} MiB  mvt {:.1} MiB  baked {:.1} -> {:.1} MiB ({:.0}x raw), fringe {:.1} MiB  bake {:.0} ms total, {:.0} ms/tile",
        mib(total_raw),
        mib(total_mvt),
        mib(unchunked_total_bytes),
        mib(total_bytes),
        total_bytes as f64 / total_raw.max(1) as f64,
        mib(stream_totals[5]),
        total_ms,
        total_ms / baked.max(1) as f64
    );
    println!(
        "flat baked {:.1} -> {:.1} MiB, fringe {:.1} MiB ({} tiles)",
        mib(flat_unchunked_total_bytes),
        mib(flat_total_bytes),
        mib(flat_fringe_bytes),
        flat_baked,
    );
    println!(
        "index bytes: u16 {:.1} MiB, u32 {:.1} MiB, vertex bytes {:.1} MiB; typed u32 premium over u16 {:.1} MiB",
        mib(total_unchunked_u16_index_bytes),
        mib(total_unchunked_u32_index_bytes),
        mib(total_unchunked_vertex_bytes),
        mib(typed_u32_index_count * 2),
    );
    println!(
        "after index bytes: u16 {:.1} MiB, u32 {:.1} MiB, vertex bytes {:.1} MiB",
        mib(total_u16_index_bytes),
        mib(total_u32_index_bytes),
        mib(total_vertex_bytes),
    );
    println!(
        "typed chunking: {} duplicate vertices / {} source ({:.3}%), max {} chunks/tile/stream",
        total_duplicate_vertices,
        total_typed_source_vertices,
        total_duplicate_vertices as f64 * 100.0 / total_typed_source_vertices.max(1) as f64,
        max_chunk_count,
    );
    let names = [
        "fill", "fill_misc", "face", "casing", "stroke", "fringe", "icon", "icon_high",
        "road_icon", "fill_3d", "fill_3d_misc", "wall", "tree", "tree_cross",
    ];
    for ((name, before), after) in names
        .iter()
        .zip(unchunked_stream_totals.iter())
        .zip(stream_totals.iter())
    {
        println!(
            "  {:<10} {:>8.1} -> {:>8.1} MiB {:>5.1}%",
            name,
            mib(*before),
            mib(*after),
            *after as f64 * 100.0 / total_bytes.max(1) as f64
        );
    }
    println!("== flat per-stream MiB ==");
    for ((name, before), after) in names
        .iter()
        .zip(flat_unchunked_stream_totals.iter())
        .zip(flat_stream_totals.iter())
    {
        println!(
            "  {:<10} {:>8.1} -> {:>8.1} MiB",
            name,
            mib(*before),
            mib(*after),
        );
    }
    println!(
        "  {:<10} {:>8.1} MiB {:>5.1}%",
        "icon_inst",
        mib(total_icon_instance_bytes),
        total_icon_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64
    );
    println!(
        "  {:<16} {:>8.1} MiB {:>5.1}%  ({} instances)",
        "shadow_disc_inst",
        mib(total_shadow_disc_instance_bytes),
        total_shadow_disc_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64,
        total_shadow_disc_instances,
    );
    println!(
        "  {:<10} {:>8.1} MiB {:>5.1}%",
        "wall_inst",
        mib(total_wall_instance_bytes),
        total_wall_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64
    );
    println!(
        "  {:<10} {:>8.1} MiB {:>5.1}%",
        "tree_inst",
        mib(total_tree_instance_bytes),
        total_tree_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64
    );
    println!(
        "  {:<16} {:>8.1} MiB {:>5.1}%  ({} instances)",
        "stalk_inst",
        mib(total_stalk_instance_bytes),
        total_stalk_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64,
        total_stalk_instances,
    );
    println!(
        "  {:<16} {:>8.1} MiB {:>5.1}%  ({} instances)",
        "stoplight_inst",
        mib(total_stoplight_instance_bytes),
        total_stoplight_instance_bytes as f64 * 100.0 / total_bytes.max(1) as f64,
        total_stoplight_instances,
    );
    for (name, kinds) in [
        ("icon", &icon_kinds),
        ("fill", &fill_kinds),
        ("face", &face_kinds),
        ("casing", &casing_kinds),
    ] {
        println!("== {name} stream vertex bytes by (shape, material) ==");
        let mut rows: Vec<_> = kinds.iter().collect();
        rows.sort_by_key(|(_, bytes)| std::cmp::Reverse(**bytes));
        for ((shape, material), bytes) in rows.into_iter().take(8) {
            println!("  shape {shape:>3} mat {material:>2}  {:>8.1} MiB", mib(*bytes));
        }
    }
}
