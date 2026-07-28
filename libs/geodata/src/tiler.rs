//! Feature -> tile pyramid -> .mbtiles.
//!
//! `geometry_to_tiles` turns one WGS84 geometry into per-(zoom, x, y) MVT
//! features (clip, simplify, winding). Two consumers: the in-memory `Tileset`
//! (fine up to a few million features) and `spool::SpoolTiler` (streams
//! country-scale layers through per-block disk files). Both flush through
//! `MbtilesWriter` in its required deterministic order.

use crate::geo::{self, NormBBox};
use crate::mvt::{command, zigzag, AttrVal, GeomType, PreFeature, TileEnc, EXTENT};
use crate::wkb::Geometry;
use flate2::write::GzEncoder;
use flate2::Compression;
use makepad_mbtile_reader::MbtilesWriter;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;

/// Clip buffer around each tile, in MVT units (64 = 1.5% of the extent).
const TILE_BUFFER: f64 = 64.0;
/// Drop polygon rings smaller than this many square MVT units after scaling.
const MIN_RING_AREA: f64 = 8.0;

pub struct TilesetConfig {
    pub name: String,
    pub description: String,
    pub attribution: String,
    pub license: String,
    pub minzoom: u8,
    pub maxzoom: u8,
}

#[derive(Default)]
pub struct TilesetStats {
    pub features_in: u64,
    pub tile_features: u64,
    pub tiles: u64,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// Shared geometry -> tile-feature machinery
// ---------------------------------------------------------------------------

/// Track WGS84 bounds as (min_lon, min_lat, max_lon, max_lat).
pub fn grow_lonlat_bounds(bounds: &mut (f64, f64, f64, f64), lon: f64, lat: f64) {
    bounds.0 = bounds.0.min(lon);
    bounds.1 = bounds.1.min(lat);
    bounds.2 = bounds.2.max(lon);
    bounds.3 = bounds.3.max(lat);
}

pub fn empty_lonlat_bounds() -> (f64, f64, f64, f64) {
    (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    )
}

/// Clip/scale one WGS84 geometry into MVT features for every covered tile in
/// the zoom range, delivering them to `sink(zoom, x, y, feature)`.
pub fn geometry_to_tiles(
    geometry: &Geometry,
    zmin: u8,
    zmax: u8,
    attrs: &[(String, AttrVal)],
    lonlat_bounds: &mut (f64, f64, f64, f64),
    sink: &mut dyn FnMut(u8, u32, u32, PreFeature),
) {
    match geometry {
        Geometry::Point(lon, lat) => {
            tile_points(&[(*lon, *lat)], zmin, zmax, attrs, lonlat_bounds, sink)
        }
        Geometry::MultiPoint(pts) => tile_points(pts, zmin, zmax, attrs, lonlat_bounds, sink),
        Geometry::LineString(pts) => tile_lines(
            std::slice::from_ref(pts),
            zmin,
            zmax,
            attrs,
            lonlat_bounds,
            sink,
        ),
        Geometry::MultiLineString(lines) => {
            tile_lines(lines, zmin, zmax, attrs, lonlat_bounds, sink)
        }
        Geometry::Polygon(rings) => tile_polygons(
            std::slice::from_ref(rings),
            zmin,
            zmax,
            attrs,
            lonlat_bounds,
            sink,
        ),
        Geometry::MultiPolygon(polys) => {
            tile_polygons(polys, zmin, zmax, attrs, lonlat_bounds, sink)
        }
    }
}

fn tile_points(
    pts: &[(f64, f64)],
    zmin: u8,
    zmax: u8,
    attrs: &[(String, AttrVal)],
    lonlat_bounds: &mut (f64, f64, f64, f64),
    sink: &mut dyn FnMut(u8, u32, u32, PreFeature),
) {
    let norm: Vec<(f64, f64)> = pts
        .iter()
        .map(|&(lon, lat)| {
            grow_lonlat_bounds(lonlat_bounds, lon, lat);
            geo::wgs84_to_norm(lon, lat)
        })
        .collect();
    for zoom in zmin..=zmax {
        let scale = f64::from(1u32 << zoom);
        let mut by_tile: HashMap<(u32, u32), Vec<(i64, i64)>> = HashMap::new();
        for &(nx, ny) in &norm {
            let wx = nx * scale;
            let wy = ny * scale;
            let tx = (wx.floor() as i64).clamp(0, i64::from(1u32 << zoom) - 1) as u32;
            let ty = (wy.floor() as i64).clamp(0, i64::from(1u32 << zoom) - 1) as u32;
            let lx = ((wx - f64::from(tx)) * f64::from(EXTENT)).round() as i64;
            let ly = ((wy - f64::from(ty)) * f64::from(EXTENT)).round() as i64;
            by_tile.entry((tx, ty)).or_default().push((lx, ly));
        }
        for ((tx, ty), pts) in by_tile {
            let mut commands = Vec::with_capacity(1 + pts.len() * 2);
            commands.push(command(1, pts.len() as u32));
            let (mut cx, mut cy) = (0i64, 0i64);
            for (px, py) in pts {
                commands.push(zigzag(px - cx));
                commands.push(zigzag(py - cy));
                cx = px;
                cy = py;
            }
            sink(
                zoom,
                tx,
                ty,
                PreFeature {
                    geom_type: GeomType::Point,
                    commands,
                    attrs: attrs.to_vec(),
                },
            );
        }
    }
}

fn tile_lines(
    lines: &[Vec<(f64, f64)>],
    zmin: u8,
    zmax: u8,
    attrs: &[(String, AttrVal)],
    lonlat_bounds: &mut (f64, f64, f64, f64),
    sink: &mut dyn FnMut(u8, u32, u32, PreFeature),
) {
    let mut bbox = NormBBox::empty();
    let norm: Vec<Vec<(f64, f64)>> = lines
        .iter()
        .map(|pts| {
            pts.iter()
                .map(|&(lon, lat)| {
                    grow_lonlat_bounds(lonlat_bounds, lon, lat);
                    let p = geo::wgs84_to_norm(lon, lat);
                    bbox.add(p.0, p.1);
                    p
                })
                .collect()
        })
        .collect();
    if bbox.is_empty() {
        return;
    }
    for zoom in zmin..=zmax {
        let scale = f64::from(1u32 << zoom);
        let buffer = TILE_BUFFER / f64::from(EXTENT);
        for (tx, ty) in tiles_covering(&bbox, zoom, buffer) {
            let mut parts: Vec<Vec<(i64, i64)>> = Vec::new();
            for line in &norm {
                let local: Vec<(f64, f64)> = line
                    .iter()
                    .map(|&(nx, ny)| (nx * scale - f64::from(tx), ny * scale - f64::from(ty)))
                    .collect();
                clip_line_parts(&local, buffer, &mut parts);
            }
            if parts.is_empty() {
                continue;
            }
            let mut commands = Vec::new();
            let (mut cx, mut cy) = (0i64, 0i64);
            let mut any = false;
            for part in &parts {
                let part = dedup(part);
                if part.len() < 2 {
                    continue;
                }
                any = true;
                commands.push(command(1, 1));
                commands.push(zigzag(part[0].0 - cx));
                commands.push(zigzag(part[0].1 - cy));
                cx = part[0].0;
                cy = part[0].1;
                commands.push(command(2, (part.len() - 1) as u32));
                for &(px, py) in &part[1..] {
                    commands.push(zigzag(px - cx));
                    commands.push(zigzag(py - cy));
                    cx = px;
                    cy = py;
                }
            }
            if any {
                sink(
                    zoom,
                    tx,
                    ty,
                    PreFeature {
                        geom_type: GeomType::Line,
                        commands,
                        attrs: attrs.to_vec(),
                    },
                );
            }
        }
    }
}

fn tile_polygons(
    polys: &[Vec<Vec<(f64, f64)>>],
    zmin: u8,
    zmax: u8,
    attrs: &[(String, AttrVal)],
    lonlat_bounds: &mut (f64, f64, f64, f64),
    sink: &mut dyn FnMut(u8, u32, u32, PreFeature),
) {
    let mut bbox = NormBBox::empty();
    let norm: Vec<Vec<Vec<(f64, f64)>>> = polys
        .iter()
        .map(|rings| {
            rings
                .iter()
                .map(|ring| {
                    ring.iter()
                        .map(|&(lon, lat)| {
                            grow_lonlat_bounds(lonlat_bounds, lon, lat);
                            let p = geo::wgs84_to_norm(lon, lat);
                            bbox.add(p.0, p.1);
                            p
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    if bbox.is_empty() {
        return;
    }
    for zoom in zmin..=zmax {
        let scale = f64::from(1u32 << zoom);
        let buffer = TILE_BUFFER / f64::from(EXTENT);
        for (tx, ty) in tiles_covering(&bbox, zoom, buffer) {
            let mut commands = Vec::new();
            let (mut cx, mut cy) = (0i64, 0i64);
            let mut any = false;
            for rings in &norm {
                for (ring_index, ring) in rings.iter().enumerate() {
                    let local: Vec<(f64, f64)> = ring
                        .iter()
                        .map(|&(nx, ny)| {
                            (nx * scale - f64::from(tx), ny * scale - f64::from(ty))
                        })
                        .collect();
                    let clipped = clip_ring(&local, buffer);
                    if clipped.len() < 3 {
                        continue;
                    }
                    let mut pts: Vec<(i64, i64)> = clipped
                        .iter()
                        .map(|&(x, y)| {
                            (
                                (x * f64::from(EXTENT)).round() as i64,
                                (y * f64::from(EXTENT)).round() as i64,
                            )
                        })
                        .collect();
                    if pts.len() >= 2 && pts.first() == pts.last() {
                        pts.pop();
                    }
                    let pts = dedup(&pts);
                    if pts.len() < 3 {
                        continue;
                    }
                    let area2 = signed_area2(&pts);
                    if (area2.abs() as f64) < MIN_RING_AREA * 2.0 {
                        continue;
                    }
                    // MVT spec: in tile coords (y down) the exterior ring has
                    // positive shoelace area, interior rings negative.
                    let want_positive = ring_index == 0;
                    let ordered: Vec<(i64, i64)> = if (area2 > 0) == want_positive {
                        pts
                    } else {
                        pts.into_iter().rev().collect()
                    };

                    any = true;
                    commands.push(command(1, 1));
                    commands.push(zigzag(ordered[0].0 - cx));
                    commands.push(zigzag(ordered[0].1 - cy));
                    cx = ordered[0].0;
                    cy = ordered[0].1;
                    commands.push(command(2, (ordered.len() - 1) as u32));
                    for &(px, py) in &ordered[1..] {
                        commands.push(zigzag(px - cx));
                        commands.push(zigzag(py - cy));
                        cx = px;
                        cy = py;
                    }
                    commands.push(command(7, 1));
                }
            }
            if any {
                sink(
                    zoom,
                    tx,
                    ty,
                    PreFeature {
                        geom_type: GeomType::Polygon,
                        commands,
                        attrs: attrs.to_vec(),
                    },
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory tileset
// ---------------------------------------------------------------------------

type TileKey = (u8, u32, u32);

pub struct Tileset {
    tiles: BTreeMap<u128, (TileKey, Vec<(String, PreFeature)>)>,
    fields: HashMap<String, HashMap<String, &'static str>>,
    bounds: (f64, f64, f64, f64),
    stats: TilesetStats,
    sidecar: crate::sidecar::SidecarBuilder,
    ring_layers: Vec<String>,
}

impl Tileset {
    pub fn new() -> Self {
        Tileset {
            tiles: BTreeMap::new(),
            fields: HashMap::new(),
            bounds: empty_lonlat_bounds(),
            stats: TilesetStats::default(),
            sidecar: crate::sidecar::SidecarBuilder::new(),
            ring_layers: Vec::new(),
        }
    }

    /// Layers whose features store a simplified exterior ring in the sidecar,
    /// enabling exact point-in-polygon queries.
    pub fn query_rings(&mut self, layers: &[&str]) {
        self.ring_layers = layers.iter().map(|s| s.to_string()).collect();
    }

    /// Add any WGS84 geometry across a zoom range.
    pub fn add(
        &mut self,
        layer: &str,
        zmin: u8,
        zmax: u8,
        geometry: &Geometry,
        attrs: &[(String, AttrVal)],
    ) {
        self.stats.features_in += 1;
        note_fields(&mut self.fields, layer, attrs);
        let want_ring = self.ring_layers.iter().any(|l| l == layer);
        self.sidecar.add(layer, geometry, attrs, want_ring);
        let tiles = &mut self.tiles;
        let stats_tile_features = &mut self.stats.tile_features;
        geometry_to_tiles(
            geometry,
            zmin,
            zmax,
            attrs,
            &mut self.bounds,
            &mut |zoom, x, y, feature| {
                let key = geo::tile_order_key(zoom, x, y);
                tiles
                    .entry(key)
                    .or_insert_with(|| ((zoom, x, y), Vec::new()))
                    .1
                    .push((layer.to_string(), feature));
                *stats_tile_features += 1;
            },
        );
    }

    /// Encode, gzip and write everything to a fresh .mbtiles file.
    pub fn finish(mut self, path: &Path, config: &TilesetConfig) -> Result<TilesetStats, String> {
        let mut writer = create_writer(path, config, &self.fields, self.bounds)?;
        let sidecar = std::mem::take(&mut self.sidecar);
        let feature_rows = sidecar.write(&mut writer)?;
        eprintln!("  sidecar: {feature_rows} queryable features");
        for (_, ((zoom, x, y), features)) in std::mem::take(&mut self.tiles) {
            let mut enc = TileEnc::new();
            for (layer_name, feature) in &features {
                enc.add_feature(layer_name, feature);
            }
            let data = gzip_tile(&enc.encode())?;
            writer
                .write_tile_xyz(zoom, x, y, &data)
                .map_err(|e| format!("write tile z{zoom}/{x}/{y}: {e:?}"))?;
            self.stats.tiles += 1;
            self.stats.bytes += data.len() as u64;
        }
        writer.finish().map_err(|e| format!("finish mbtiles: {e:?}"))?;
        Ok(self.stats)
    }
}

// ---------------------------------------------------------------------------
// Shared writer helpers (also used by spool.rs)
// ---------------------------------------------------------------------------

pub(crate) fn note_fields(
    fields: &mut HashMap<String, HashMap<String, &'static str>>,
    layer: &str,
    attrs: &[(String, AttrVal)],
) {
    let layer_fields = fields.entry(layer.to_string()).or_default();
    for (key, value) in attrs {
        let type_name = match value {
            AttrVal::Str(_) => "String",
            AttrVal::Int(_) => "Number",
            AttrVal::Float(_) => "Number",
            AttrVal::Bool(_) => "Boolean",
        };
        layer_fields.insert(key.clone(), type_name);
    }
}

pub(crate) fn gzip_tile(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::new(6));
    gz.write_all(raw).map_err(|e| format!("gzip: {e}"))?;
    gz.finish().map_err(|e| format!("gzip: {e}"))
}

pub(crate) fn create_writer(
    path: &Path,
    config: &TilesetConfig,
    fields: &HashMap<String, HashMap<String, &'static str>>,
    bounds: (f64, f64, f64, f64),
) -> Result<MbtilesWriter, String> {
    let mut writer = MbtilesWriter::create(path).map_err(|e| format!("create mbtiles: {e:?}"))?;
    writer.set_metadata("name", &config.name);
    writer.set_metadata("description", &config.description);
    writer.set_metadata("format", "pbf");
    writer.set_metadata("type", "overlay");
    writer.set_metadata("minzoom", config.minzoom.to_string());
    writer.set_metadata("maxzoom", config.maxzoom.to_string());
    writer.set_metadata("attribution", &config.attribution);
    writer.set_metadata("license", &config.license);
    if bounds.0.is_finite() {
        writer.set_metadata(
            "bounds",
            format!(
                "{:.6},{:.6},{:.6},{:.6}",
                bounds.0, bounds.1, bounds.2, bounds.3
            ),
        );
    }
    let vector_layers: Vec<serde_json::Value> = fields
        .iter()
        .map(|(layer_name, layer_fields)| {
            serde_json::json!({
                "id": layer_name,
                "fields": layer_fields,
                "minzoom": config.minzoom,
                "maxzoom": config.maxzoom,
            })
        })
        .collect();
    writer.set_metadata(
        "json",
        serde_json::json!({ "vector_layers": vector_layers }).to_string(),
    );
    writer.set_metadata(
        "geodata_built_unix",
        format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ),
    );
    Ok(writer)
}

// ---------------------------------------------------------------------------
// Clip helpers
// ---------------------------------------------------------------------------

/// Tiles whose (buffered) extent intersects the bbox at this zoom.
fn tiles_covering(bbox: &NormBBox, zoom: u8, buffer: f64) -> Vec<(u32, u32)> {
    let scale = f64::from(1u32 << zoom);
    let axis = i64::from(1u32 << zoom);
    let min_tx = ((bbox.min_x * scale - buffer).floor() as i64).clamp(0, axis - 1);
    let max_tx = ((bbox.max_x * scale + buffer).floor() as i64).clamp(0, axis - 1);
    let min_ty = ((bbox.min_y * scale - buffer).floor() as i64).clamp(0, axis - 1);
    let max_ty = ((bbox.max_y * scale + buffer).floor() as i64).clamp(0, axis - 1);
    let mut tiles = Vec::new();
    for ty in min_ty..=max_ty {
        for tx in min_tx..=max_tx {
            tiles.push((tx as u32, ty as u32));
        }
    }
    tiles
}

/// Sutherland-Hodgman clip of a ring against the buffered unit square.
fn clip_ring(ring: &[(f64, f64)], buffer: f64) -> Vec<(f64, f64)> {
    let lo = -buffer;
    let hi = 1.0 + buffer;
    let mut pts = ring.to_vec();
    if pts.len() >= 2 && pts.first() == pts.last() {
        pts.pop();
    }
    for &(axis_x, bound, keep_greater) in &[
        (true, lo, true),
        (true, hi, false),
        (false, lo, true),
        (false, hi, false),
    ] {
        if pts.is_empty() {
            return pts;
        }
        let inside = |p: &(f64, f64)| {
            let v = if axis_x { p.0 } else { p.1 };
            if keep_greater {
                v >= bound
            } else {
                v <= bound
            }
        };
        let intersect = |a: &(f64, f64), b: &(f64, f64)| -> (f64, f64) {
            let (av, bv) = if axis_x { (a.0, b.0) } else { (a.1, b.1) };
            let t = if (bv - av).abs() < f64::EPSILON {
                0.0
            } else {
                (bound - av) / (bv - av)
            };
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
        };
        let mut out = Vec::with_capacity(pts.len() + 4);
        for i in 0..pts.len() {
            let current = pts[i];
            let previous = pts[(i + pts.len() - 1) % pts.len()];
            let current_in = inside(&current);
            let previous_in = inside(&previous);
            if current_in {
                if !previous_in {
                    out.push(intersect(&previous, &current));
                }
                out.push(current);
            } else if previous_in {
                out.push(intersect(&previous, &current));
            }
        }
        pts = out;
    }
    pts
}

/// Clip a polyline to the buffered unit square, appending surviving sub-parts
/// (scaled to MVT integer units) to `parts`.
fn clip_line_parts(line: &[(f64, f64)], buffer: f64, parts: &mut Vec<Vec<(i64, i64)>>) {
    let lo = -buffer;
    let hi = 1.0 + buffer;
    let inside = |p: &(f64, f64)| p.0 >= lo && p.0 <= hi && p.1 >= lo && p.1 <= hi;
    let to_units = |p: (f64, f64)| -> (i64, i64) {
        (
            (p.0 * f64::from(EXTENT)).round() as i64,
            (p.1 * f64::from(EXTENT)).round() as i64,
        )
    };
    let mut current: Vec<(i64, i64)> = Vec::new();
    for window in line.windows(2) {
        let (a, b) = (window[0], window[1]);
        match (inside(&a), inside(&b)) {
            (true, true) => {
                if current.is_empty() {
                    current.push(to_units(a));
                }
                current.push(to_units(b));
            }
            (true, false) => {
                if current.is_empty() {
                    current.push(to_units(a));
                }
                if let Some(exit) = clip_segment(a, b, lo, hi) {
                    current.push(to_units(exit.1));
                }
                if current.len() >= 2 {
                    parts.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
            (false, true) => {
                if let Some(entry) = clip_segment(a, b, lo, hi) {
                    current.push(to_units(entry.0));
                }
                current.push(to_units(b));
            }
            (false, false) => {
                if let Some((entry, exit)) = clip_segment(a, b, lo, hi) {
                    let part = vec![to_units(entry), to_units(exit)];
                    if part[0] != part[1] {
                        parts.push(part);
                    }
                }
            }
        }
    }
    if current.len() >= 2 {
        parts.push(current);
    }
}

/// Liang-Barsky segment clip against the buffered square.
fn clip_segment(
    a: (f64, f64),
    b: (f64, f64),
    lo: f64,
    hi: f64,
) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
    for &(p, q) in &[
        (-dx, a.0 - lo),
        (dx, hi - a.0),
        (-dy, a.1 - lo),
        (dy, hi - a.1),
    ] {
        if p.abs() < f64::EPSILON {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((
        (a.0 + dx * t0, a.1 + dy * t0),
        (a.0 + dx * t1, a.1 + dy * t1),
    ))
}

fn dedup(pts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(pts.len());
    for &p in pts {
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// Twice the shoelace signed area on tile coordinates (y down).
/// Positive = clockwise on screen = MVT exterior ring.
fn signed_area2(pts: &[(i64, i64)]) -> i64 {
    let mut sum = 0i64;
    for i in 0..pts.len() {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % pts.len()];
        sum += x1 * y2 - x2 * y1;
    }
    sum
}
