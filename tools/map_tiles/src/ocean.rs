//! ocean-tiles — bake the osmdata.openstreetmap.de water polygons
//! (EPSG:3857 shapefiles, the standard preprocessed OSM coastline product)
//! into two mbtiles overlays for the renderer's `ocean` layer:
//!
//! - LOW  (z0-9):  every ocean-covered tile, from the simplified product.
//! - HIGH (z10-14): only tiles in the coastline neighborhood (segment-touched
//!   plus a 1-tile dilation ring), from the full product.
//!
//! The renderer's overlay ancestor-shift serves open ocean above z9 from the
//! LOW archive's z9 tiles (a full-water square overzooms exactly), so the
//! ~190M interior high-zoom tiles are deliberately absent. The 1-tile
//! dilation keeps exact full-square tiles next to every coast so the
//! simplified fallback can never wobble a phantom shoreline into view.

use makepad_map_build::native::geom::{MVT_EXTENT, TILE_BUFFER};
use makepad_map_build::native::mvt::{encode_tile, GeometryType, Layer, OsmType, TileFeature, TilePoint};
use makepad_mbtile_reader::{
    compress_tile, compression_metadata_rows, MbtilesWriter, TileCompression,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

/// EPSG:3857 half-extent: mercator meters at the antimeridian.
const MERC_HALF: f64 = 20_037_508.342789244;
const BROTLI_QUALITY: u32 = 10;
const LOW_ZOOMS: (u8, u8) = (0, 9);
const HIGH_ZOOMS: (u8, u8) = (10, 14);
/// Rings smaller than this (squared MVT units) are dropped as noise.
const MIN_RING_AREA2: f64 = 8.0;

pub struct OceanOptions {
    pub simplified_shp: std::path::PathBuf,
    pub full_shp: std::path::PathBuf,
    pub out_low: std::path::PathBuf,
    pub out_high: std::path::PathBuf,
}

pub fn parse_ocean_options(args: &[String]) -> Result<OceanOptions, String> {
    if args.len() != 5 {
        return Err(
            "ocean-tiles <simplified.shp> <full.shp> <out-low.mbtiles> <out-high.mbtiles>"
                .to_string(),
        );
    }
    Ok(OceanOptions {
        simplified_shp: args[1].clone().into(),
        full_shp: args[2].clone().into(),
        out_low: args[3].clone().into(),
        out_high: args[4].clone().into(),
    })
}

pub fn build_ocean(options: OceanOptions) -> Result<(), String> {
    let start = Instant::now();
    println!("ocean-tiles: reading {}", options.simplified_shp.display());
    let simplified = read_shp_polygons(&options.simplified_shp)?;
    println!(
        "ocean-tiles: simplified product: {} records ({:.1}s)",
        simplified.len(),
        start.elapsed().as_secs_f32()
    );
    write_archive(
        &options.out_low,
        &simplified,
        LOW_ZOOMS,
        false,
        "Ocean polygons z0-9 (simplified osmdata water polygons, full coverage)",
    )?;
    drop(simplified);

    let t_full = Instant::now();
    println!("ocean-tiles: reading {}", options.full_shp.display());
    let full = read_shp_polygons(&options.full_shp)?;
    println!(
        "ocean-tiles: full product: {} records ({:.1}s)",
        full.len(),
        t_full.elapsed().as_secs_f32()
    );
    write_archive(
        &options.out_high,
        &full,
        HIGH_ZOOMS,
        true,
        "Ocean polygons z10-14 (osmdata water polygons, coastline neighborhood only)",
    )?;
    println!(
        "ocean-tiles: done in {:.1}s",
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Shapefile reading (type-5 Polygon records only; no attributes needed)
// ---------------------------------------------------------------------------

/// One record: rings in EPSG:3857. Outer rings CW, holes CCW (ESRI spec);
/// orientation is preserved through projection and clipping, and the
/// renderer's nonzero fill only needs outer/hole to stay opposite.
type ShpRecord = Vec<Vec<(f64, f64)>>;

fn read_shp_polygons(path: &Path) -> Result<Vec<ShpRecord>, String> {
    let data = std::fs::read(path).map_err(|err| format!("read {}: {err}", path.display()))?;
    if data.len() < 100 {
        return Err(format!("{}: too short for a shapefile", path.display()));
    }
    let file_code = i32::from_be_bytes(data[0..4].try_into().unwrap());
    if file_code != 9994 {
        return Err(format!("{}: not a shapefile (code {file_code})", path.display()));
    }
    let mut records = Vec::new();
    let mut offset = 100_usize;
    while offset + 8 <= data.len() {
        let content_words = i32::from_be_bytes(data[offset + 4..offset + 8].try_into().unwrap());
        let content_len = content_words as usize * 2;
        let body = offset + 8;
        if body + content_len > data.len() {
            break;
        }
        let shape_type = i32::from_le_bytes(data[body..body + 4].try_into().unwrap());
        if shape_type == 5 {
            // Polygon: box f64x4, num_parts i32, num_points i32, parts, points
            let num_parts =
                i32::from_le_bytes(data[body + 36..body + 40].try_into().unwrap()) as usize;
            let num_points =
                i32::from_le_bytes(data[body + 40..body + 44].try_into().unwrap()) as usize;
            let parts_off = body + 44;
            let points_off = parts_off + num_parts * 4;
            if points_off + num_points * 16 > body + content_len {
                return Err(format!("{}: malformed polygon record", path.display()));
            }
            let part_start = |i: usize| -> usize {
                i32::from_le_bytes(data[parts_off + i * 4..parts_off + i * 4 + 4].try_into().unwrap())
                    as usize
            };
            let mut rings = Vec::with_capacity(num_parts);
            for part in 0..num_parts {
                let begin = part_start(part);
                let end = if part + 1 < num_parts { part_start(part + 1) } else { num_points };
                let mut ring = Vec::with_capacity(end.saturating_sub(begin));
                for point in begin..end {
                    let base = points_off + point * 16;
                    let x = f64::from_le_bytes(data[base..base + 8].try_into().unwrap());
                    let y = f64::from_le_bytes(data[base + 8..base + 16].try_into().unwrap());
                    ring.push((x, y));
                }
                if ring.len() >= 4 {
                    rings.push(ring);
                }
            }
            if !rings.is_empty() {
                records.push(rings);
            }
        }
        offset = body + content_len;
    }
    Ok(records)
}

// ---------------------------------------------------------------------------
// Tiling
// ---------------------------------------------------------------------------

/// EPSG:3857 -> global tile units ((1<<zoom)*4096 across the world).
fn project(x: f64, y: f64, zoom: u8) -> (f64, f64) {
    let world = ((1_u64 << zoom) * MVT_EXTENT as u64) as f64;
    (
        (x + MERC_HALF) / (2.0 * MERC_HALF) * world,
        (MERC_HALF - y) / (2.0 * MERC_HALF) * world,
    )
}

fn ring_area2(ring: &[(f64, f64)]) -> f64 {
    let mut area2 = 0.0;
    for i in 0..ring.len() {
        let (ax, ay) = ring[i];
        let (bx, by) = ring[(i + 1) % ring.len()];
        area2 += ax * by - bx * ay;
    }
    area2
}

/// Sutherland-Hodgman clip of one ring against an axis-aligned rect.
fn clip_ring_rect(ring: &[(f64, f64)], min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<(f64, f64)> {
    let mut current = ring.to_vec();
    for edge in 0..4 {
        if current.is_empty() {
            return current;
        }
        let inside = |p: (f64, f64)| match edge {
            0 => p.0 >= min_x,
            1 => p.0 <= max_x,
            2 => p.1 >= min_y,
            _ => p.1 <= max_y,
        };
        let cross = |a: (f64, f64), b: (f64, f64)| -> (f64, f64) {
            match edge {
                0 => (min_x, a.1 + (b.1 - a.1) * (min_x - a.0) / (b.0 - a.0)),
                1 => (max_x, a.1 + (b.1 - a.1) * (max_x - a.0) / (b.0 - a.0)),
                2 => (a.0 + (b.0 - a.0) * (min_y - a.1) / (b.1 - a.1), min_y),
                _ => (a.0 + (b.0 - a.0) * (max_y - a.1) / (b.1 - a.1), max_y),
            }
        };
        let mut next = Vec::with_capacity(current.len() + 4);
        for i in 0..current.len() {
            let a = current[i];
            let b = current[(i + 1) % current.len()];
            match (inside(a), inside(b)) {
                (true, true) => next.push(b),
                (true, false) => next.push(cross(a, b)),
                (false, true) => {
                    next.push(cross(a, b));
                    next.push(b);
                }
                (false, false) => {}
            }
        }
        current = next;
    }
    current
}

/// Recursively bisect a set of rings over the tile grid, emitting per-tile
/// fragments. Interior regions collapse to 5-vertex rectangles immediately,
/// so cost is O(verts * depth + covered_tiles).
#[allow(clippy::too_many_arguments)]
fn bisect(
    rings: Vec<Vec<(f64, f64)>>,
    tx0: u32,
    ty0: u32,
    tx1: u32,
    ty1: u32,
    zoom: u8,
    keep: &dyn Fn(u32, u32) -> bool,
    sink: &mut dyn FnMut(u32, u32, Vec<Vec<(f64, f64)>>),
) {
    if rings.is_empty() {
        return;
    }
    if tx0 == tx1 && ty0 == ty1 {
        if keep(tx0, ty0) {
            sink(tx0, ty0, rings);
        }
        return;
    }
    let extent = MVT_EXTENT as f64;
    let buffer = TILE_BUFFER as f64;
    // Split the wider axis at the tile midpoint, clipping with the tile
    // buffer so leaf fragments already carry seam overlap.
    if tx1 - tx0 >= ty1 - ty0 {
        let mid = tx0 + (tx1 - tx0) / 2;
        let split = (mid + 1) as f64 * extent;
        let left: Vec<_> = rings
            .iter()
            .map(|r| clip_ring_rect(r, tx0 as f64 * extent - buffer, f64::MIN, split + buffer, f64::MAX))
            .filter(|r| r.len() >= 4)
            .collect();
        let right: Vec<_> = rings
            .into_iter()
            .map(|r| clip_ring_rect(&r, split - buffer, f64::MIN, (tx1 + 1) as f64 * extent + buffer, f64::MAX))
            .filter(|r| r.len() >= 4)
            .collect();
        bisect(left, tx0, ty0, mid, ty1, zoom, keep, sink);
        bisect(right, mid + 1, ty0, tx1, ty1, zoom, keep, sink);
    } else {
        let mid = ty0 + (ty1 - ty0) / 2;
        let split = (mid + 1) as f64 * extent;
        let top: Vec<_> = rings
            .iter()
            .map(|r| clip_ring_rect(r, f64::MIN, ty0 as f64 * extent - buffer, f64::MAX, split + buffer))
            .filter(|r| r.len() >= 4)
            .collect();
        let bottom: Vec<_> = rings
            .into_iter()
            .map(|r| clip_ring_rect(&r, f64::MIN, split - buffer, f64::MAX, (ty1 + 1) as f64 * extent + buffer))
            .filter(|r| r.len() >= 4)
            .collect();
        bisect(top, tx0, ty0, tx1, mid, zoom, keep, sink);
        bisect(bottom, tx0, mid + 1, tx1, ty1, zoom, keep, sink);
    }
}

type TileMap = HashMap<(u32, u32), Vec<Vec<TilePoint>>>;

/// Tiles whose neighborhood contains actual coastline geometry at this zoom:
/// every tile a ring segment's bbox touches, dilated by one tile ring.
fn coastal_candidates(records: &[ShpRecord], zoom: u8) -> HashSet<(u32, u32)> {
    let max_tile = (1_u32 << zoom) - 1;
    let extent = MVT_EXTENT as f64;
    let chunks: Vec<&[ShpRecord]> = records.chunks(records.len().div_ceil(16).max(1)).collect();
    let touched = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    let mut set = HashSet::<(u32, u32)>::new();
                    for record in chunk {
                        for ring in record {
                            for i in 0..ring.len() {
                                let (ax, ay) = project(ring[i].0, ring[i].1, zoom);
                                let (bx, by) =
                                    project(ring[(i + 1) % ring.len()].0, ring[(i + 1) % ring.len()].1, zoom);
                                let x0 = ((ax.min(bx) / extent).floor().max(0.0) as u32).min(max_tile);
                                let x1 = ((ax.max(bx) / extent).floor().max(0.0) as u32).min(max_tile);
                                let y0 = ((ay.min(by) / extent).floor().max(0.0) as u32).min(max_tile);
                                let y1 = ((ay.max(by) / extent).floor().max(0.0) as u32).min(max_tile);
                                for ty in y0..=y1 {
                                    for tx in x0..=x1 {
                                        set.insert((tx, ty));
                                    }
                                }
                            }
                        }
                    }
                    set
                })
            })
            .collect();
        let mut merged = HashSet::new();
        for handle in handles {
            merged.extend(handle.join().unwrap());
        }
        merged
    });
    let mut dilated = HashSet::with_capacity(touched.len() * 3);
    for &(tx, ty) in &touched {
        for dy in -1_i64..=1 {
            for dx in -1_i64..=1 {
                let nx = tx as i64 + dx;
                let ny = ty as i64 + dy;
                if nx >= 0 && ny >= 0 && nx <= max_tile as i64 && ny <= max_tile as i64 {
                    dilated.insert((nx as u32, ny as u32));
                }
            }
        }
    }
    dilated
}

fn tile_fragments(records: &[ShpRecord], zoom: u8, coastal_only: bool) -> TileMap {
    let candidates = if coastal_only {
        Some(coastal_candidates(records, zoom))
    } else {
        None
    };
    let max_tile = (1_u32 << zoom) - 1;
    let extent = MVT_EXTENT as f64;
    let chunks: Vec<&[ShpRecord]> = records.chunks(records.len().div_ceil(16).max(1)).collect();
    std::thread::scope(|scope| {
        let candidates = &candidates;
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    let keep = |tx: u32, ty: u32| -> bool {
                        candidates.as_ref().is_none_or(|set| set.contains(&(tx, ty)))
                    };
                    let mut tiles = TileMap::new();
                    for record in chunk {
                        let rings: Vec<Vec<(f64, f64)>> = record
                            .iter()
                            .map(|ring| ring.iter().map(|&(x, y)| project(x, y, zoom)).collect())
                            .collect();
                        let mut min_x = f64::MAX;
                        let mut min_y = f64::MAX;
                        let mut max_x = f64::MIN;
                        let mut max_y = f64::MIN;
                        for &(x, y) in rings.iter().flatten() {
                            min_x = min_x.min(x);
                            min_y = min_y.min(y);
                            max_x = max_x.max(x);
                            max_y = max_y.max(y);
                        }
                        let tx0 = ((min_x / extent).floor().max(0.0) as u32).min(max_tile);
                        let tx1 = ((max_x / extent).floor().max(0.0) as u32).min(max_tile);
                        let ty0 = ((min_y / extent).floor().max(0.0) as u32).min(max_tile);
                        let ty1 = ((max_y / extent).floor().max(0.0) as u32).min(max_tile);
                        bisect(
                            rings,
                            tx0,
                            ty0,
                            tx1,
                            ty1,
                            zoom,
                            &keep,
                            &mut |tx, ty, fragment_rings| {
                                let origin_x = tx as f64 * extent;
                                let origin_y = ty as f64 * extent;
                                let mut kept = Vec::new();
                                for ring in fragment_rings {
                                    if ring_area2(&ring).abs() < MIN_RING_AREA2 {
                                        continue;
                                    }
                                    let mut path: Vec<TilePoint> = ring
                                        .iter()
                                        .map(|&(x, y)| TilePoint {
                                            x: (x - origin_x).round() as i32,
                                            y: (y - origin_y).round() as i32,
                                        })
                                        .collect();
                                    path.dedup();
                                    if path.len() > 1 && path.first() == path.last() {
                                        path.pop(); // encoder closes rings implicitly
                                    }
                                    if path.len() >= 4 {
                                        kept.push(path);
                                    }
                                }
                                if !kept.is_empty() {
                                    tiles.entry((tx, ty)).or_default().append(&mut kept);
                                }
                            },
                        );
                    }
                    tiles
                })
            })
            .collect();
        let mut merged = TileMap::new();
        for handle in handles {
            for (key, mut paths) in handle.join().unwrap() {
                merged.entry(key).or_default().append(&mut paths);
            }
        }
        merged
    })
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// MbtilesWriter rowid order within a zoom: 256x256 block-major.
fn block_major_key(x: u32, y: u32) -> u64 {
    (((y >> 8) as u64) << 48) | (((x >> 8) as u64) << 32) | (((y & 255) as u64) << 16) | (x & 255) as u64
}

fn write_archive(
    out_path: &Path,
    records: &[ShpRecord],
    zooms: (u8, u8),
    coastal_only: bool,
    description: &str,
) -> Result<(), String> {
    let compression = TileCompression::Brotli { quality: BROTLI_QUALITY };
    let mut writer = MbtilesWriter::create(out_path)
        .map_err(|err| format!("create {}: {err}", out_path.display()))?;
    let mut tile_count = 0_u64;
    let mut tile_bytes = 0_u64;
    for zoom in zooms.0..=zooms.1 {
        let t_zoom = Instant::now();
        let tiles = tile_fragments(records, zoom, coastal_only);
        let mut keys: Vec<(u32, u32)> = tiles.keys().copied().collect();
        keys.sort_by_key(|&(x, y)| block_major_key(x, y));
        // Encode + compress in parallel, preserving write order.
        let encoded = std::thread::scope(|scope| {
            let tiles = &tiles;
            let chunk_len = keys.len().div_ceil(16).max(1);
            let handles: Vec<_> = keys
                .chunks(chunk_len)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|&(x, y)| {
                                let feature = TileFeature {
                                    layer: Layer::Ocean,
                                    geometry_type: GeometryType::Polygon,
                                    osm_type: OsmType::Way,
                                    id: 0,
                                    closed: true,
                                    tags: Vec::new(),
                                    paths: tiles[&(x, y)].clone(),
                                };
                                let raw = encode_tile(vec![feature])?;
                                let compressed = compress_tile(&compression, None, &raw)
                                    .map_err(|err| format!("compress: {err}"))?;
                                Ok::<_, String>((x, y, compressed))
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Result<Vec<_>, _>>()
        })?;
        let zoom_tiles: usize = encoded.iter().map(Vec::len).sum();
        for batch in encoded {
            for (x, y, compressed) in batch {
                tile_bytes += compressed.len() as u64;
                tile_count += 1;
                writer
                    .write_tile_xyz(zoom, x, y, &compressed)
                    .map_err(|err| format!("write {zoom}/{x}/{y}: {err}"))?;
            }
        }
        println!(
            "ocean-tiles: {} z{zoom}: {zoom_tiles} tiles in {:.1}s",
            out_path.file_name().unwrap_or_default().to_string_lossy(),
            t_zoom.elapsed().as_secs_f32()
        );
    }
    let mut metadata: Vec<(String, String)> = vec![
        ("name".into(), "makepad-ocean".into()),
        ("format".into(), "pbf".into()),
        ("description".into(), description.into()),
        ("minzoom".into(), zooms.0.to_string()),
        ("maxzoom".into(), zooms.1.to_string()),
        ("bounds".into(), "-180.0,-85.0511,180.0,85.0511".into()),
        ("attribution".into(), "OpenStreetMap contributors".into()),
        (
            "json".into(),
            r#"{"vector_layers":[{"id":"ocean","fields":{}}]}"#.into(),
        ),
    ];
    metadata.extend(compression_metadata_rows(&compression, None));
    for (key, value) in metadata {
        writer.set_metadata(key, value);
    }
    let stats = writer
        .finish()
        .map_err(|err| format!("finish {}: {err}", out_path.display()))?;
    println!(
        "ocean-tiles: {} complete: {tile_count} tiles, {:.1} MiB payload, {:.1} MiB file",
        out_path.display(),
        tile_bytes as f64 / (1024.0 * 1024.0),
        stats.file_bytes as f64 / (1024.0 * 1024.0)
    );
    Ok(())
}
