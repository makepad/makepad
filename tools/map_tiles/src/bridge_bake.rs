//! bridge-bake: derive per-vertex road/rail elevation (Δz above local ground)
//! for grade-separated crossings and bake it into a small mbtiles overlay.
//!
//! This is bridge.md M1 (offline constraint solve over the road graph) with
//! the M2 upgrade folded in where AHN sheets are on disk: ground init from
//! the AHN DTM and measured deck heights from DSM−DTM along bridge
//! centerlines. OSM stays 2D forever, so z is derived here at build time and
//! the renderer just adds the baked Δz on top of its terrain drape.
//!
//! Solve model (per z14 tile, on a 3×3 neighborhood so tile edges agree):
//! - every road/rail vertex is a node; identical tile-grid coords merge, so
//!   shared OSM nodes become one variable and ramps stay continuous;
//! - lower bound z ≥ ground (AHN DTM, else 0);
//! - a geometric crossing with layer(a) > layer(b) forces
//!   z_a ≥ z_b + 5.5 m per layer step at the crossing point;
//! - bridge ways with a crossing hold a level deck; bridge ways over water
//!   only (no crossing, no AHN) get a small measured-free camber hump;
//! - grade limits per class propagate the lift into approach ways, which is
//!   what generates the ramps;
//! - a few constrained smoothing passes round the profile.
//! Output: layer "bridge_dz", one LineString per way with tag dz =
//! comma-joined decimeters per vertex and hw = corridor half-width meters.

use flate2::write::GzEncoder;
use flate2::Compression;
use makepad_fast_inflate::gzip_decompress_vec;
use makepad_geodata::tiff::Tiff;
use makepad_mbtile_reader::{MbtilesReader, MbtilesWriter};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::native::mvt::{
    encode_tile, read_protobuf_bytes, read_protobuf_key, read_varint, skip_protobuf_value,
    GeometryType, Layer, OsmType, TileFeature, TilePoint,
};

const EXTENT: f64 = 4096.0;
/// Solve-graph max segment length in tile units (~9 m at z14): decks and
/// ramps interpolate per vertex, so long straights need intermediate nodes.
const MAX_SEG_UNITS: f32 = 24.0;
/// Vertical clearance per layer step (≥4.7 m legal + deck structure).
const CLEARANCE_PER_LAYER_M: f32 = 5.5;
/// Sanity ceiling on any solved lift.
const MAX_LIFT_M: f32 = 40.0;

#[derive(Debug)]
pub struct BakeOptions {
    pub detail: PathBuf,
    pub output: PathBuf,
    pub bbox: (f64, f64, f64, f64), // west, south, east, north
    pub zoom: u8,
    pub ahn_dir: Option<PathBuf>,
    /// Base archive (europe-shortbread): when given, each covered base
    /// tile's own road/rail geometry is annotated with per-vertex dz
    /// (layer base_dz, L/F/P join keys) — the renderer then never matches
    /// geometry at runtime.
    pub base: Option<PathBuf>,
}

pub fn parse_bake_options(args: &[String]) -> Result<BakeOptions, String> {
    if args.len() < 3 {
        return Err("bridge-bake needs <detail.mbtiles> <output.mbtiles> --bbox w,s,e,n".into());
    }
    let mut options = BakeOptions {
        detail: PathBuf::from(&args[1]),
        output: PathBuf::from(&args[2]),
        bbox: (0.0, 0.0, 0.0, 0.0),
        zoom: 14,
        ahn_dir: None,
        base: None,
    };
    let mut have_bbox = false;
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--bbox" => {
                let value = args.get(index + 1).ok_or("--bbox needs w,s,e,n")?;
                let parts: Vec<f64> = value
                    .split(',')
                    .map(|p| p.trim().parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("--bbox: {e}"))?;
                if parts.len() != 4 {
                    return Err("--bbox needs 4 numbers".into());
                }
                options.bbox = (parts[0], parts[1], parts[2], parts[3]);
                have_bbox = true;
                index += 2;
            }
            "--zoom" => {
                options.zoom = args
                    .get(index + 1)
                    .ok_or("--zoom needs a number")?
                    .parse()
                    .map_err(|e| format!("--zoom: {e}"))?;
                index += 2;
            }
            "--ahn" => {
                options.ahn_dir =
                    Some(PathBuf::from(args.get(index + 1).ok_or("--ahn needs a directory")?));
                index += 2;
            }
            "--base" => {
                options.base =
                    Some(PathBuf::from(args.get(index + 1).ok_or("--base needs a file")?));
                index += 2;
            }
            other => return Err(format!("unknown bridge-bake option {other}")),
        }
    }
    if !have_bbox {
        return Err("bridge-bake requires --bbox (keep it small to iterate)".into());
    }
    Ok(options)
}

// --- Geo helpers ---

fn lon_to_tile_x(lon: f64, zoom: u8) -> f64 {
    (lon + 180.0) / 360.0 * (1u32 << zoom) as f64
}

fn lat_to_tile_y(lat: f64, zoom: u8) -> f64 {
    let rad = lat.to_radians();
    (1.0 - (rad.tan() + 1.0 / rad.cos()).ln() / std::f64::consts::PI) / 2.0
        * (1u32 << zoom) as f64
}

fn tile_y_to_lat(y: f64, zoom: u8) -> f64 {
    let n = std::f64::consts::PI * (1.0 - 2.0 * y / (1u32 << zoom) as f64);
    n.sinh().atan().to_degrees()
}

fn tile_x_to_lon(x: f64, zoom: u8) -> f64 {
    x / (1u32 << zoom) as f64 * 360.0 - 180.0
}

/// WGS84 → RD New (EPSG:28992), Schreutelkamp & Strang van Hees polynomial
/// approximation (~25 cm, far below the AHN pixel).
fn wgs84_to_rd(lon: f64, lat: f64) -> (f64, f64) {
    let dp = 0.36 * (lat - 52.155_174_40);
    let dl = 0.36 * (lon - 5.387_206_21);
    let x = 155_000.0
        + 190_094.945 * dl
        - 11_832.228 * dp * dl
        - 114.221 * dp * dp * dl
        - 32.391 * dl * dl * dl
        - 0.705 * dp
        - 2.340 * dp * dp * dp * dl
        - 0.608 * dp * dl * dl * dl
        - 0.008 * dl * dl
        + 0.148 * dp * dp * dl * dl * dl;
    let y = 463_000.0
        + 309_056.544 * dp
        + 3_638.893 * dl * dl
        + 73.077 * dp * dp
        - 157.984 * dp * dl * dl
        + 59.788 * dp * dp * dp
        + 0.433 * dl
        - 6.439 * dp * dp * dl * dl
        - 0.032 * dp * dl
        + 0.092 * dl * dl * dl * dl
        - 0.054 * dp * dl * dl * dl * dl;
    (x, y)
}

/// AHN sheet set: DTM (M_*.tif, bare ground, structures removed) and DSM
/// (R_*.tif, keeps bridge decks). Missing dir or missing sheets degrade to
/// solver-only — never an error.
struct Ahn {
    dtm: Vec<Tiff>,
    dsm: Vec<Tiff>,
}

impl Ahn {
    fn open(dir: Option<&Path>) -> Self {
        let mut ahn = Ahn { dtm: Vec::new(), dsm: Vec::new() };
        let Some(dir) = dir else { return ahn };
        let Ok(entries) = std::fs::read_dir(dir) else {
            eprintln!("bridge-bake: AHN dir {} unreadable, solver-only", dir.display());
            return ahn;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".tif") {
                continue;
            }
            match Tiff::open(&entry.path()) {
                Ok(tiff) => {
                    if name.starts_with("M_") {
                        ahn.dtm.push(tiff);
                    } else if name.starts_with("R_") {
                        ahn.dsm.push(tiff);
                    }
                }
                Err(error) => eprintln!("bridge-bake: skip {name}: {error}"),
            }
        }
        eprintln!(
            "bridge-bake: AHN sheets loaded: {} DTM, {} DSM",
            ahn.dtm.len(),
            ahn.dsm.len()
        );
        ahn
    }

    fn sample(set: &mut [Tiff], lon: f64, lat: f64) -> Option<f32> {
        let (rx, ry) = wgs84_to_rd(lon, lat);
        for tiff in set {
            let (ox, oy, sx, sy) = tiff.geo?;
            if rx < ox
                || ry > oy
                || rx >= ox + sx * f64::from(tiff.width)
                || ry <= oy - sy * f64::from(tiff.height)
            {
                continue;
            }
            return tiff.sample_geo(rx, ry);
        }
        None
    }

    fn ground(&mut self, lon: f64, lat: f64) -> Option<f32> {
        Self::sample(&mut self.dtm, lon, lat)
    }

    fn surface(&mut self, lon: f64, lat: f64) -> Option<f32> {
        Self::sample(&mut self.dsm, lon, lat)
    }

    fn any(&self) -> bool {
        !self.dtm.is_empty() || !self.dsm.is_empty()
    }
}

// --- Detail tile MVT decode (only what the solver needs) ---

struct RawWay {
    id: u64,
    tags: Vec<(String, String)>,
    paths: Vec<Vec<(f32, f32)>>,
}

fn decode_osm_line_ways(tile: &[u8]) -> Result<Vec<RawWay>, String> {
    decode_line_ways(tile, "osm_lines")
}

fn decode_line_ways(tile: &[u8], layer_name: &str) -> Result<Vec<RawWay>, String> {
    let pbf;
    let data: &[u8] = if tile.len() >= 2 && tile[0] == 0x1f && tile[1] == 0x8b {
        pbf = gzip_decompress_vec(tile).map_err(|e| format!("gunzip tile: {e:?}"))?;
        &pbf
    } else {
        tile
    };
    let mut ways = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (field, wire) = read_protobuf_key(data, &mut offset)?;
        if field == 3 && wire == 2 {
            decode_layer(read_protobuf_bytes(data, &mut offset)?, layer_name, &mut ways)?;
        } else {
            skip_protobuf_value(data, &mut offset, wire)?;
        }
    }
    Ok(ways)
}

enum MvtVal {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl MvtVal {
    fn to_tag_string(&self) -> String {
        match self {
            MvtVal::Str(s) => s.clone(),
            MvtVal::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            MvtVal::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        }
    }
}

fn decode_layer(
    layer: &[u8],
    target_layer: &str,
    ways: &mut Vec<RawWay>,
) -> Result<(), String> {
    let mut offset = 0;
    let mut name = String::new();
    let mut extent = 4096u64;
    let mut keys = Vec::<String>::new();
    let mut values = Vec::<MvtVal>::new();
    let mut features = Vec::<&[u8]>::new();
    while offset < layer.len() {
        let (field, wire) = read_protobuf_key(layer, &mut offset)?;
        match (field, wire) {
            (1, 2) => {
                name = String::from_utf8_lossy(read_protobuf_bytes(layer, &mut offset)?)
                    .to_string();
            }
            (2, 2) => features.push(read_protobuf_bytes(layer, &mut offset)?),
            (3, 2) => keys.push(
                String::from_utf8_lossy(read_protobuf_bytes(layer, &mut offset)?).to_string(),
            ),
            (4, 2) => values.push(decode_value(read_protobuf_bytes(layer, &mut offset)?)?),
            (5, 0) => extent = read_varint(layer, &mut offset)?,
            _ => skip_protobuf_value(layer, &mut offset, wire)?,
        }
    }
    if name != target_layer {
        return Ok(());
    }
    let scale = EXTENT / extent.max(1) as f64;
    for feature in features {
        decode_feature(feature, &keys, &values, scale as f32, ways)?;
    }
    Ok(())
}

fn decode_value(value: &[u8]) -> Result<MvtVal, String> {
    let mut offset = 0;
    let mut result = MvtVal::Str(String::new());
    while offset < value.len() {
        let (field, wire) = read_protobuf_key(value, &mut offset)?;
        match (field, wire) {
            (1, 2) => {
                result = MvtVal::Str(
                    String::from_utf8_lossy(read_protobuf_bytes(value, &mut offset)?).to_string(),
                );
            }
            (2, 5) => {
                let bytes = &value[offset..offset + 4];
                offset += 4;
                result = MvtVal::Num(f64::from(f32::from_le_bytes(
                    bytes.try_into().map_err(|_| "bad float")?,
                )));
            }
            (3, 1) => {
                let bytes = &value[offset..offset + 8];
                offset += 8;
                result = MvtVal::Num(f64::from_le_bytes(
                    bytes.try_into().map_err(|_| "bad double")?,
                ));
            }
            (4, 0) => result = MvtVal::Num(read_varint(value, &mut offset)? as i64 as f64),
            (5, 0) => result = MvtVal::Num(read_varint(value, &mut offset)? as f64),
            (6, 0) => {
                let raw = read_varint(value, &mut offset)?;
                result = MvtVal::Num((((raw >> 1) as i64) ^ (-((raw & 1) as i64))) as f64);
            }
            (7, 0) => result = MvtVal::Bool(read_varint(value, &mut offset)? != 0),
            _ => skip_protobuf_value(value, &mut offset, wire)?,
        }
    }
    Ok(result)
}

fn decode_feature(
    feature: &[u8],
    keys: &[String],
    values: &[MvtVal],
    scale: f32,
    ways: &mut Vec<RawWay>,
) -> Result<(), String> {
    let mut offset = 0;
    let mut id = 0u64;
    let mut tags = Vec::new();
    let mut geometry_type = 0u64;
    let mut geometry: &[u8] = &[];
    while offset < feature.len() {
        let (field, wire) = read_protobuf_key(feature, &mut offset)?;
        match (field, wire) {
            (1, 0) => id = read_varint(feature, &mut offset)?,
            (2, 2) => {
                let packed = read_protobuf_bytes(feature, &mut offset)?;
                let mut po = 0;
                while po < packed.len() {
                    let ki = read_varint(packed, &mut po)? as usize;
                    let vi = read_varint(packed, &mut po)? as usize;
                    if let (Some(key), Some(value)) = (keys.get(ki), values.get(vi)) {
                        tags.push((key.clone(), value.to_tag_string()));
                    }
                }
            }
            (3, 0) => geometry_type = read_varint(feature, &mut offset)?,
            (4, 2) => geometry = read_protobuf_bytes(feature, &mut offset)?,
            _ => skip_protobuf_value(feature, &mut offset, wire)?,
        }
    }
    if geometry_type != 2 {
        return Ok(());
    }
    let mut paths = Vec::new();
    let mut path = Vec::new();
    let (mut cx, mut cy) = (0i64, 0i64);
    let mut go = 0;
    while go < geometry.len() {
        let cmd = read_varint(geometry, &mut go)?;
        let (op, count) = (cmd & 7, cmd >> 3);
        match op {
            1 | 2 => {
                if op == 1 && !path.is_empty() {
                    paths.push(std::mem::take(&mut path));
                }
                for _ in 0..count {
                    let dx = read_varint(geometry, &mut go)?;
                    let dy = read_varint(geometry, &mut go)?;
                    cx += ((dx >> 1) as i64) ^ (-((dx & 1) as i64));
                    cy += ((dy >> 1) as i64) ^ (-((dy & 1) as i64));
                    path.push((cx as f32 * scale, cy as f32 * scale));
                }
            }
            7 => {}
            _ => return Err(format!("unknown MVT geometry op {op}")),
        }
    }
    if !path.is_empty() {
        paths.push(path);
    }
    if !paths.is_empty() {
        ways.push(RawWay { id, tags, paths });
    }
    Ok(())
}

// --- Solve graph ---

fn tag<'a>(tags: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn tag_truthy(tags: &[(String, String)], key: &str) -> bool {
    !matches!(tag(tags, key).unwrap_or("no"), "" | "0" | "no" | "false" | "False")
}

/// Max grade (m/m) used to spread lift into approaches; also half-width of
/// the corridor the renderer matches strokes against.
fn class_params(tags: &[(String, String)]) -> Option<(f32, f32)> {
    if let Some(highway) = tag(tags, "highway") {
        let (grade, half_width) = match highway {
            "proposed" | "construction" | "razed" | "abandoned" | "planned" | "corridor"
            | "elevator" => return None,
            "motorway" | "trunk" => (0.05, 11.0),
            "motorway_link" | "trunk_link" => (0.07, 6.5),
            "primary" => (0.07, 8.0),
            "primary_link" | "secondary_link" | "tertiary_link" => (0.09, 5.5),
            "secondary" => (0.08, 7.0),
            "tertiary" => (0.09, 6.0),
            "residential" | "unclassified" | "living_street" => (0.10, 5.0),
            "service" | "track" | "busway" => (0.12, 4.0),
            "pedestrian" => (0.12, 5.0),
            "cycleway" | "footway" | "path" | "bridleway" | "steps" => (0.16, 2.8),
            _ => (0.10, 5.0),
        };
        return Some((grade, half_width));
    }
    if let Some(railway) = tag(tags, "railway") {
        let (grade, half_width) = match railway {
            "rail" => (0.035, 3.2),
            "light_rail" | "tram" => (0.06, 2.8),
            "subway" => (0.05, 3.0),
            "abandoned" | "razed" | "proposed" | "disused" | "platform" | "construction" => {
                return None
            }
            _ => (0.06, 2.8),
        };
        return Some((grade, half_width));
    }
    None
}

struct SolveWay {
    nodes: Vec<u32>,
    /// Length in meters of the edge after each node (len = nodes-1).
    seg_m: Vec<f32>,
    grade: f32,
    bridge: bool,
    tunnel: bool,
    layer: i32,
    half_width_m: f32,
    center: bool,
    id: u64,
    /// Source tile this fragment was decoded from (feature binning).
    src: (u32, u32),
}

#[derive(Default)]
struct Graph {
    node_ids: HashMap<(i32, i32), u32>,
    pos: Vec<(f32, f32)>,
    ground: Vec<f32>,
    has_ground: Vec<bool>,
    low: Vec<f32>,
    ways: Vec<SolveWay>,
}

impl Graph {
    fn node(&mut self, x: f32, y: f32) -> u32 {
        let key = (x.round() as i32, y.round() as i32);
        if let Some(&id) = self.node_ids.get(&key) {
            return id;
        }
        let id = self.pos.len() as u32;
        self.node_ids.insert(key, id);
        self.pos.push((key.0 as f32, key.1 as f32));
        self.ground.push(0.0);
        self.has_ground.push(false);
        self.low.push(0.0);
        id
    }

    fn add_way(
        &mut self,
        way: &RawWay,
        offset_units: (f32, f32),
        m_per_unit: f32,
        center: bool,
        src: (u32, u32),
    ) {
        let Some((grade, class_half_width)) = class_params(&way.tags) else {
            return;
        };
        if tag(&way.tags, "area") == Some("yes") {
            return;
        }
        let bridge = tag_truthy(&way.tags, "bridge");
        let tunnel = tag_truthy(&way.tags, "tunnel");
        // NOTE: subway ways stay in the graph — Amsterdam's metro runs
        // ELEVATED on the Utrechtboog with bridge tags only on the actual
        // spans; excluding the plain sections cut the graph and cliffed
        // the deck at every tag boundary (underground metro is tunnel-
        // tagged and already excluded from lifting).
        // The OSM layer attr survives as osm_layer (plain `layer` is
        // shadowed by the MVT layer name).
        let osm_layer = tag(&way.tags, "osm_layer")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|v| v.round() as i32);
        let layer = match osm_layer {
            Some(l) => {
                if bridge && l < 1 {
                    1
                } else if tunnel && l > -1 {
                    -1
                } else {
                    l
                }
            }
            None => {
                if bridge {
                    1
                } else if tunnel {
                    -1
                } else {
                    0
                }
            }
        };
        let half_width_m = tag(&way.tags, "width")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|w| (w * 0.5 + 1.0).clamp(2.0, 16.0))
            .unwrap_or(class_half_width);
        for path in &way.paths {
            if path.len() < 2 {
                continue;
            }
            let mut nodes = Vec::new();
            let mut seg_m = Vec::new();
            let mut push_point = |graph: &mut Graph,
                                  nodes: &mut Vec<u32>,
                                  seg_m: &mut Vec<f32>,
                                  x: f32,
                                  y: f32| {
                let id = graph.node(x + offset_units.0, y + offset_units.1);
                if nodes.last() == Some(&id) {
                    return;
                }
                if let Some(&prev) = nodes.last() {
                    let (px, py) = graph.pos[prev as usize];
                    let (nx, ny) = graph.pos[id as usize];
                    seg_m.push(((nx - px).powi(2) + (ny - py).powi(2)).sqrt() * m_per_unit);
                }
                nodes.push(id);
            };
            push_point(self, &mut nodes, &mut seg_m, path[0].0, path[0].1);
            for window in path.windows(2) {
                let (ax, ay) = window[0];
                let (bx, by) = window[1];
                let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                let steps = (len / MAX_SEG_UNITS).ceil().max(1.0) as usize;
                for step in 1..=steps {
                    let t = step as f32 / steps as f32;
                    push_point(
                        self,
                        &mut nodes,
                        &mut seg_m,
                        ax + (bx - ax) * t,
                        ay + (by - ay) * t,
                    );
                }
            }
            if nodes.len() < 2 {
                continue;
            }
            self.ways.push(SolveWay {
                nodes,
                seg_m,
                grade,
                bridge,
                tunnel,
                layer,
                half_width_m,
                center,
                id: way.id,
                src,
            });
        }
    }
}

fn segments_intersect(
    a0: (f32, f32),
    a1: (f32, f32),
    b0: (f32, f32),
    b1: (f32, f32),
) -> Option<(f32, f32)> {
    let d1 = (a1.0 - a0.0, a1.1 - a0.1);
    let d2 = (b1.0 - b0.0, b1.1 - b0.1);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-6 {
        return None;
    }
    let dx = b0.0 - a0.0;
    let dy = b0.1 - a0.1;
    let t = (dx * d2.1 - dy * d2.0) / denom;
    let u = (dx * d1.1 - dy * d1.0) / denom;
    // Endpoint touches are junctions (or should be), not grade separation.
    if t <= 0.02 || t >= 0.98 || u <= 0.02 || u >= 0.98 {
        return None;
    }
    Some((t, u))
}

struct TileJob {
    z: u8,
    x: u32,
    y: u32,
    features: Vec<TileFeature>,
    stats: SolveStats,
}

#[derive(Default, Clone, Copy)]
struct SolveStats {
    ways: usize,
    crossings: usize,
    measured: usize,
    baked: usize,
}

#[allow(clippy::too_many_arguments)]
/// ONE global solve over every decoded tile: per-tile solves (even with a
/// 1-ring context) reached slightly different heights for the same way on
/// each side of a tile seam — visible steps and doubled slabs mid-deck.
/// A single graph + single field makes overlap copies coincide exactly.
/// `range` = tiles whose bridge_dz features should be emitted.
fn solve_bbox(
    z: u8,
    origin: (u32, u32),
    range: (u32, u32, u32, u32),
    decoded: &HashMap<(u32, u32), Vec<RawWay>>,
    ahn: &mut Ahn,
) -> (
    HashMap<(u32, u32), Vec<TileFeature>>,
    SolveStats,
    SolvedField,
    SolvedField,
) {
    let mut stats = SolveStats::default();
    let axis = 1u32 << z;
    let (x0, y0) = origin;
    let lat_min = tile_y_to_lat(f64::from(range.3) + 1.0, z);
    let lat_max = tile_y_to_lat(f64::from(range.1), z);
    let center_lat = (lat_min + lat_max) * 0.5;
    let m_per_unit =
        (center_lat.to_radians().cos() * 40_075_016.686 / f64::from(axis) / EXTENT) as f32;

    let mut graph = Graph::default();
    for (&(tx, ty), ways) in decoded {
        let offset = (
            (tx as i64 - x0 as i64) as f32 * EXTENT as f32,
            (ty as i64 - y0 as i64) as f32 * EXTENT as f32,
        );
        let center =
            tx >= range.0 && tx <= range.2 && ty >= range.1 && ty <= range.3;
        for way in ways {
            graph.add_way(way, offset, m_per_unit, center, (tx, ty));
        }
    }
    stats.ways = graph.ways.len();
    if graph.ways.is_empty() {
        return (
            HashMap::new(),
            stats,
            SolvedField::default(),
            SolvedField::default(),
        );
    }

    // Ground init from the AHN DTM (bare earth, NAP). Water and structures
    // are nodata; fill from way-neighbors below so canals don't read as 0
    // while their quays sit at 2 m.
    let lonlat = |graph: &Graph, node: u32| -> (f64, f64) {
        let (ux, uy) = graph.pos[node as usize];
        (
            tile_x_to_lon(f64::from(x0) + f64::from(ux) / EXTENT, z),
            tile_y_to_lat(f64::from(y0) + f64::from(uy) / EXTENT, z),
        )
    };
    if ahn.any() {
        for node in 0..graph.pos.len() as u32 {
            let (lon, lat) = lonlat(&graph, node);
            if let Some(ground) = ahn.ground(lon, lat) {
                if (-10.0..=400.0).contains(&ground) {
                    graph.ground[node as usize] = ground;
                    graph.has_ground[node as usize] = true;
                }
            }
        }
        for _ in 0..8 {
            let mut changed = false;
            for way in &graph.ways {
                for window in way.nodes.windows(2) {
                    let (a, b) = (window[0] as usize, window[1] as usize);
                    if graph.has_ground[a] && !graph.has_ground[b] {
                        graph.ground[b] = graph.ground[a];
                        graph.has_ground[b] = true;
                        changed = true;
                    } else if graph.has_ground[b] && !graph.has_ground[a] {
                        graph.ground[a] = graph.ground[b];
                        graph.has_ground[a] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // Erode the DTM into a street-level base: dikes and embankments are
        // real terrain the renderer's flat city ground cannot show, so a
        // deck's VISIBLE height is z above the surrounding low ground — not
        // above the dike top its abutments happen to sit on (which zeroed
        // out whole viaducts, DTM-pixel roulette deciding per twin span).
        // Slope-capped min propagation (2%/m) removes narrow earthworks
        // while keeping genuinely sustained relief.
        let mut eroding = true;
        let mut erosion_passes = 0;
        while eroding && erosion_passes < 96 {
            eroding = false;
            erosion_passes += 1;
            for way in &graph.ways {
                for (seg, window) in way.nodes.windows(2).enumerate() {
                    let (a, b) = (window[0] as usize, window[1] as usize);
                    let slack = 0.02 * way.seg_m[seg].max(0.1);
                    if graph.ground[a] + slack < graph.ground[b] {
                        graph.ground[b] = graph.ground[a] + slack;
                        eroding = true;
                    }
                    if graph.ground[b] + slack < graph.ground[a] {
                        graph.ground[a] = graph.ground[b] + slack;
                        eroding = true;
                    }
                }
            }
        }
    }
    // The solve runs in VISIBLE space: 0 is the renderer's flat city
    // ground, crossings force relative separation above it, and the baked
    // dz is directly the on-screen lift.
    for node in 0..graph.pos.len() {
        graph.low[node] = 0.0;
    }

    // Segment grid for crossing detection: only elevated ways ever test, so
    // index everything once and query per elevated segment. Tunnels are in
    // the grid as the UNDER side — an elevated rail crossing a tunnel-tagged
    // underpass still needs its clearance (the layer floor in the constraint
    // keeps the clearance measured from ground, not from tunnel depth) —
    // they just never lift themselves.
    const CELL: f32 = 64.0;
    let mut grid: HashMap<(i32, i32), Vec<(usize, usize)>> = HashMap::new();
    for (way_index, way) in graph.ways.iter().enumerate() {
        for seg in 0..way.nodes.len() - 1 {
            let (ax, ay) = graph.pos[way.nodes[seg] as usize];
            let (bx, by) = graph.pos[way.nodes[seg + 1] as usize];
            let (min_x, max_x) = (ax.min(bx), ax.max(bx));
            let (min_y, max_y) = (ay.min(by), ay.max(by));
            let mut cell_y = (min_y / CELL).floor() as i32;
            while cell_y <= (max_y / CELL).floor() as i32 {
                let mut cell_x = (min_x / CELL).floor() as i32;
                while cell_x <= (max_x / CELL).floor() as i32 {
                    grid.entry((cell_x, cell_y)).or_default().push((way_index, seg));
                    cell_x += 1;
                }
                cell_y += 1;
            }
        }
    }

    // AHN measured deck heights (M2): DSM keeps the deck surface, DTM at the
    // abutments gives the ground reference. Applied once as lower bounds.
    let mut way_has_measure = vec![false; graph.ways.len()];
    if !ahn.dsm.is_empty() {
        for way_index in 0..graph.ways.len() {
            let way = &graph.ways[way_index];
            if !way.bridge || way.nodes.len() < 2 {
                continue;
            }
            let nodes = way.nodes.clone();
            let total_m: f32 = way.seg_m.iter().sum();
            let g0 = graph.ground[nodes[0] as usize];
            let g1 = graph.ground[*nodes.last().unwrap() as usize];
            let mut arc = 0.0f32;
            let mut lifts: Vec<Option<f32>> = Vec::with_capacity(nodes.len());
            let mut arcs: Vec<f32> = Vec::with_capacity(nodes.len());
            for (index, &node) in nodes.iter().enumerate() {
                if index > 0 {
                    arc += way.seg_m[index - 1];
                }
                arcs.push(arc);
                let t = if total_m > 0.0 { arc / total_m } else { 0.0 };
                let base = g0 + (g1 - g0) * t;
                let (lon, lat) = lonlat(&graph, node);
                lifts.push(
                    ahn.surface(lon, lat)
                        .map(|s| s - base)
                        .filter(|m| (-2.0..=MAX_LIFT_M).contains(m)),
                );
            }
            let mut valid: Vec<f32> = lifts.iter().flatten().copied().collect();
            if valid.len() < 3 {
                continue;
            }
            valid.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = valid[valid.len() / 2];
            if !(0.3..=MAX_LIFT_M).contains(&median) {
                continue;
            }
            // Fill gaps by interpolation, clamp outliers (cars, railings),
            // then a couple of smoothing taps.
            let mut profile = vec![0.0f32; nodes.len()];
            let cap = (median * 1.5 + 2.0).min(MAX_LIFT_M);
            for (index, lift) in lifts.iter().enumerate() {
                profile[index] = lift.unwrap_or(median).clamp(0.0, cap);
            }
            // The DSM keeps overhead gantries, signs and catenary: sharp
            // spikes a real deck can't have. A deck is grade-limited, so
            // clip anything rising faster than ~2× the class grade
            // (forward + backward min pass keeps genuine gradual rises).
            let slack = way.grade * 2.0;
            for index in 1..profile.len() {
                let limit = profile[index - 1] + slack * way.seg_m[index - 1].max(0.1);
                if profile[index] > limit {
                    profile[index] = limit;
                }
            }
            for index in (0..profile.len() - 1).rev() {
                let limit = profile[index + 1] + slack * way.seg_m[index].max(0.1);
                if profile[index] > limit {
                    profile[index] = limit;
                }
            }
            for _ in 0..2 {
                let snapshot = profile.clone();
                for index in 1..profile.len() - 1 {
                    profile[index] =
                        (snapshot[index - 1] + snapshot[index] + snapshot[index + 1]) / 3.0;
                }
            }
            way_has_measure[way_index] = true;
            stats.measured += 1;
            let total = total_m.max(1e-3);
            for (index, &node) in nodes.iter().enumerate() {
                let _ = total;
                let bound = profile[index];
                if bound > graph.low[node as usize] {
                    graph.low[node as usize] = bound.min(MAX_LIFT_M);
                }
            }
        }
    }

    // Constraint rounds: crossings raise decks, deck rules keep spans level,
    // grade propagation grows the approach ramps. Iterate so stacked
    // structures (deck over deck) settle.
    let mut way_has_crossing = vec![false; graph.ways.len()];
    for _ in 0..4 {
        // Crossing clearance.
        for (way_index, way) in graph.ways.iter().enumerate() {
            let elevated = !way.tunnel && (way.bridge || way.layer >= 1);
            if !elevated {
                continue;
            }
            for seg in 0..way.nodes.len() - 1 {
                let a0 = graph.pos[way.nodes[seg] as usize];
                let a1 = graph.pos[way.nodes[seg + 1] as usize];
                let (min_x, max_x) = (a0.0.min(a1.0), a0.0.max(a1.0));
                let (min_y, max_y) = (a0.1.min(a1.1), a0.1.max(a1.1));
                let mut hits: Vec<(usize, usize, f32)> = Vec::new();
                let mut cell_y = (min_y / CELL).floor() as i32;
                while cell_y <= (max_y / CELL).floor() as i32 {
                    let mut cell_x = (min_x / CELL).floor() as i32;
                    while cell_x <= (max_x / CELL).floor() as i32 {
                        if let Some(bucket) = grid.get(&(cell_x, cell_y)) {
                            for &(other_index, other_seg) in bucket {
                                if other_index == way_index {
                                    continue;
                                }
                                let other = &graph.ways[other_index];
                                if other.layer >= way.layer {
                                    continue;
                                }
                                let b0 = graph.pos[other.nodes[other_seg] as usize];
                                let b1 = graph.pos[other.nodes[other_seg + 1] as usize];
                                let shared = way.nodes[seg] == other.nodes[other_seg]
                                    || way.nodes[seg] == other.nodes[other_seg + 1]
                                    || way.nodes[seg + 1] == other.nodes[other_seg]
                                    || way.nodes[seg + 1] == other.nodes[other_seg + 1];
                                if shared {
                                    continue;
                                }
                                if let Some((_t, u)) = segments_intersect(a0, a1, b0, b1) {
                                    let z_under = graph.low
                                        [other.nodes[other_seg] as usize]
                                        * (1.0 - u)
                                        + graph.low[other.nodes[other_seg + 1] as usize] * u;
                                    let steps =
                                        (way.layer - other.layer.max(0)).clamp(1, 3) as f32;
                                    hits.push((
                                        other_index,
                                        other_seg,
                                        z_under + CLEARANCE_PER_LAYER_M * steps,
                                    ));
                                }
                            }
                        }
                        cell_x += 1;
                    }
                    cell_y += 1;
                }
                if std::env::var("BB_DEBUG").is_ok()
                    && matches!(way.id, 104447474 | 7381773 | 515946648 | 7381715)
                {
                    eprintln!(
                        "DBG way {} seg {} hits {} low_a {:.1}",
                        way.id,
                        seg,
                        hits.len(),
                        graph.low[way.nodes[seg] as usize]
                    );
                }
                for (_, _, needed) in hits {
                    let needed = needed.min(MAX_LIFT_M);
                    // Hold the deck level for ±30 m around the crossing,
                    // not just the crossing segment: pinning two vertices
                    // makes a 40 m speed-bump tent out of what is a level
                    // deck in reality.
                    const DECK_HOLD_M: f32 = 30.0;
                    let mut arc = 0.0f32;
                    let mut index = seg;
                    loop {
                        let node = way.nodes[index] as usize;
                        if needed > graph.low[node] {
                            graph.low[node] = needed;
                        }
                        if index == 0 {
                            break;
                        }
                        arc += way.seg_m[index - 1];
                        if arc > DECK_HOLD_M {
                            break;
                        }
                        index -= 1;
                    }
                    arc = 0.0;
                    index = seg + 1;
                    loop {
                        let node = way.nodes[index] as usize;
                        if needed > graph.low[node] {
                            graph.low[node] = needed;
                        }
                        if index + 1 >= way.nodes.len() {
                            break;
                        }
                        arc += way.seg_m[index];
                        if arc > DECK_HOLD_M {
                            break;
                        }
                        index += 1;
                    }
                    way_has_crossing[way_index] = true;
                    stats.crossings += 1;
                }
            }
        }

        // Deck rules per bridge way.
        for (way_index, way) in graph.ways.iter().enumerate() {
            if !way.bridge || way.nodes.len() < 2 || way_has_measure[way_index] {
                continue;
            }
            let total_m: f32 = way.seg_m.iter().sum();
            if way_has_crossing[way_index] {
                // Hold the span level at the tallest requirement; grade
                // propagation shapes everything outside the deck.
                let deck = way
                    .nodes
                    .iter()
                    .map(|&n| graph.low[n as usize])
                    .fold(f32::MIN, f32::max);
                let margin = (total_m * 0.15).min(30.0);
                let mut arc = 0.0f32;
                for (index, &node) in way.nodes.iter().enumerate() {
                    if index > 0 {
                        arc += way.seg_m[index - 1];
                    }
                    if arc >= margin && (total_m - arc) >= margin
                        && deck > graph.low[node as usize]
                    {
                        graph.low[node as usize] = deck;
                    }
                }
            } else {
                // Canal bridge with nothing crossing underneath: a gentle
                // camber hump above the visible base so it still reads as
                // a bridge.
                let hump = (total_m * 0.03).clamp(0.4, 1.5);
                let total = total_m.max(1e-3);
                let mut arc = 0.0f32;
                for (index, &node) in way.nodes.iter().enumerate() {
                    if index > 0 {
                        arc += way.seg_m[index - 1];
                    }
                    let t = arc / total;
                    let bound = hump * (std::f32::consts::PI * t).sin();
                    if bound > graph.low[node as usize] {
                        graph.low[node as usize] = bound;
                    }
                }
            }
        }

        // Grade propagation: a lifted node pulls its way-neighbors up to
        // within grade·distance — this is what builds the approach ramps.
        let mut changed = true;
        let mut iterations = 0;
        while changed && iterations < 64 {
            changed = false;
            iterations += 1;
            for way in &graph.ways {
                if way.tunnel {
                    continue;
                }
                for seg in 0..way.nodes.len() - 1 {
                    let a = way.nodes[seg] as usize;
                    let b = way.nodes[seg + 1] as usize;
                    let slack = way.grade * way.seg_m[seg].max(0.01);
                    if graph.low[a] - slack > graph.low[b] + 1e-3 {
                        graph.low[b] = graph.low[a] - slack;
                        changed = true;
                    }
                    if graph.low[b] - slack > graph.low[a] + 1e-3 {
                        graph.low[a] = graph.low[b] - slack;
                        changed = true;
                    }
                }
            }
        }
    }

    // Sag suppression: viaduct chains are split into many short OSM ways
    // and only the spans with crossings get pinned — the connector stubs
    // between them would dip toward ground and climb back within tens of
    // meters (a W-profile no real road has). Morphological closing along
    // way chains fills short dips between higher flanks; genuinely
    // separate overpasses keep their ground stretch (ramps need ~200 m).
    for _ in 0..6 {
        let mut filled = false;
        for way in &graph.ways {
            if way.tunnel {
                continue;
            }
            for index in 1..way.nodes.len() - 1 {
                let left = graph.low[way.nodes[index - 1] as usize];
                let right = graph.low[way.nodes[index + 1] as usize];
                let fill = left.min(right);
                let node = way.nodes[index] as usize;
                if fill > graph.low[node] + 0.05 {
                    graph.low[node] = fill;
                    filled = true;
                }
            }
        }
        if !filled {
            break;
        }
    }

    // Constrained smoothing: round ramp kinks without dipping below the
    // solved bounds.
    let mut z: Vec<f32> = graph.low.clone();
    for _ in 0..3 {
        let snapshot = z.clone();
        for way in &graph.ways {
            if way.tunnel {
                continue;
            }
            for index in 1..way.nodes.len() - 1 {
                let previous = snapshot[way.nodes[index - 1] as usize];
                let current = snapshot[way.nodes[index] as usize];
                let next = snapshot[way.nodes[index + 1] as usize];
                let average = (previous + current + next) / 3.0;
                let node = way.nodes[index] as usize;
                let bounded = average.max(graph.low[node]);
                if bounded > z[node] {
                    z[node] = bounded;
                }
            }
        }
    }

    // Bake: in-range ways whose lift is visible (bridge_dz — the
    // heuristic corridor consumers and debugging), binned per source tile
    // in that tile's local coordinates.
    let mut features: HashMap<(u32, u32), Vec<TileFeature>> = HashMap::new();
    for way in &graph.ways {
        if !way.center || way.tunnel {
            continue;
        }
        let src_offset = (
            (way.src.0 as i64 - x0 as i64) as f32 * EXTENT as f32,
            (way.src.1 as i64 - y0 as i64) as f32 * EXTENT as f32,
        );
        let mut dz_dm = Vec::with_capacity(way.nodes.len());
        let mut max_dz = 0.0f32;
        for &node in &way.nodes {
            let dz = z[node as usize].max(0.0);
            max_dz = max_dz.max(dz);
            dz_dm.push((dz * 10.0).round() as i64);
        }
        if max_dz < 0.15 {
            continue;
        }
        let path: Vec<TilePoint> = way
            .nodes
            .iter()
            .map(|&node| {
                let (ux, uy) = graph.pos[node as usize];
                TilePoint {
                    x: (ux - src_offset.0).round() as i32,
                    y: (uy - src_offset.1).round() as i32,
                }
            })
            .collect();
        if path.len() < 2 {
            continue;
        }
        let dz_tag = dz_dm
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",");
        features.entry(way.src).or_default().push(TileFeature {
            layer: Layer::BridgeDz,
            geometry_type: GeometryType::LineString,
            osm_type: OsmType::Way,
            id: way.id as i64,
            closed: false,
            tags: vec![
                ("dz".to_string(), dz_tag),
                ("hw".to_string(), format!("{:.1}", way.half_width_m)),
            ],
            paths: vec![path],
        });
        stats.baked += 1;
    }

    // Tunnel sink solve — the downward mirror of the lift solve. A tunnel
    // clears the surface by TUNNEL_CLEAR_M at every 2D crossing with a
    // non-tunnel way, holds that depth for TUNNEL_HOLD_M around the
    // crossing, ramps at road grade, and surfaces exactly at its portals
    // (nodes shared with surface ways).
    const TUNNEL_CLEAR_M: f32 = 5.5;
    const TUNNEL_HOLD_M: f32 = 30.0;
    let mut sink = vec![0.0f32; z.len()];
    let mut is_surface_node = vec![false; z.len()];
    for way in &graph.ways {
        if !way.tunnel {
            for &node in &way.nodes {
                is_surface_node[node as usize] = true;
            }
        }
    }
    for (way_index, way) in graph.ways.iter().enumerate() {
        if !way.tunnel || way.nodes.len() < 2 {
            continue;
        }
        for seg in 0..way.nodes.len() - 1 {
            let a0 = graph.pos[way.nodes[seg] as usize];
            let a1 = graph.pos[way.nodes[seg + 1] as usize];
            let (min_x, max_x) = (a0.0.min(a1.0), a0.0.max(a1.0));
            let (min_y, max_y) = (a0.1.min(a1.1), a0.1.max(a1.1));
            let mut hit = false;
            let mut cell_y = (min_y / CELL).floor() as i32;
            'outer: while cell_y <= (max_y / CELL).floor() as i32 {
                let mut cell_x = (min_x / CELL).floor() as i32;
                while cell_x <= (max_x / CELL).floor() as i32 {
                    if let Some(bucket) = grid.get(&(cell_x, cell_y)) {
                        for &(other_index, other_seg) in bucket {
                            if other_index == way_index {
                                continue;
                            }
                            let other = &graph.ways[other_index];
                            if other.tunnel {
                                continue;
                            }
                            let shared = way.nodes[seg] == other.nodes[other_seg]
                                || way.nodes[seg] == other.nodes[other_seg + 1]
                                || way.nodes[seg + 1] == other.nodes[other_seg]
                                || way.nodes[seg + 1] == other.nodes[other_seg + 1];
                            if shared {
                                continue;
                            }
                            let b0 = graph.pos[other.nodes[other_seg] as usize];
                            let b1 = graph.pos[other.nodes[other_seg + 1] as usize];
                            if segments_intersect(a0, a1, b0, b1).is_some() {
                                hit = true;
                                break 'outer;
                            }
                        }
                    }
                    cell_x += 1;
                }
                cell_y += 1;
            }
            if !hit {
                continue;
            }
            // Hold depth around the crossing segment, both directions.
            let mut arc = 0.0f32;
            let mut index = seg;
            loop {
                let node = way.nodes[index] as usize;
                sink[node] = sink[node].max(TUNNEL_CLEAR_M);
                if index == 0 {
                    break;
                }
                arc += way.seg_m[index - 1];
                if arc > TUNNEL_HOLD_M {
                    break;
                }
                index -= 1;
            }
            arc = 0.0;
            index = seg + 1;
            loop {
                let node = way.nodes[index] as usize;
                sink[node] = sink[node].max(TUNNEL_CLEAR_M);
                if index + 1 >= way.nodes.len() {
                    break;
                }
                arc += way.seg_m[index];
                if arc > TUNNEL_HOLD_M {
                    break;
                }
                index += 1;
            }
        }
    }
    // Grade ramps along tunnel ways; portals pinned to the surface.
    for _ in 0..3 {
        for way in &graph.ways {
            if !way.tunnel || way.nodes.len() < 2 {
                continue;
            }
            for seg in 0..way.nodes.len() - 1 {
                let a = way.nodes[seg] as usize;
                let b = way.nodes[seg + 1] as usize;
                let reach = way.grade * way.seg_m[seg].max(0.1);
                sink[b] = sink[b].max(sink[a] - reach);
                sink[a] = sink[a].max(sink[b] - reach);
            }
        }
        for way in &graph.ways {
            if !way.tunnel {
                continue;
            }
            for &node in &way.nodes {
                if is_surface_node[node as usize] {
                    sink[node as usize] = 0.0;
                }
            }
        }
    }
    for way in &graph.ways {
        if !way.tunnel {
            continue;
        }
        for &node in &way.nodes {
            let node = node as usize;
            if sink[node] > 0.05 {
                z[node] = z[node].min(-sink[node]);
                stats.crossings += 1;
            }
        }
    }

    // The solved field itself, for sampling base-tile geometry.
    let mut field = SolvedField::default();
    for way in &graph.ways {
        if way.tunnel {
            continue;
        }
        for seg in 0..way.nodes.len() - 1 {
            let a = way.nodes[seg] as usize;
            let b = way.nodes[seg + 1] as usize;
            // GROUNDED segments belong in the field too: nearest-wins is
            // what keeps a siding 4 m from an elevated track grounded (its
            // own z=0 centerline out-competes the neighbor's deck).
            field.push(
                graph.pos[a].0,
                graph.pos[a].1,
                graph.pos[b].0,
                graph.pos[b].1,
                z[a],
                z[b],
            );
        }
    }
    // Tunnel ways get their own field (never mixed into the surface one:
    // a street directly above a tunnel line must not inherit the sink).
    let mut tunnel_field = SolvedField::default();
    for way in &graph.ways {
        if !way.tunnel {
            continue;
        }
        for seg in 0..way.nodes.len() - 1 {
            let a = way.nodes[seg] as usize;
            let b = way.nodes[seg + 1] as usize;
            tunnel_field.push(
                graph.pos[a].0,
                graph.pos[a].1,
                graph.pos[b].0,
                graph.pos[b].1,
                z[a].min(0.0),
                z[b].min(0.0),
            );
        }
    }
    (features, stats, field, tunnel_field)
}

/// Spatial index over the solved height segments of one tile solve, for
/// annotating the base tile's own geometry.
#[derive(Default)]
struct SolvedField {
    grid: HashMap<(i32, i32), Vec<[f32; 6]>>,
}

const FIELD_CELL: f32 = 96.0;

impl SolvedField {
    fn push(&mut self, ax: f32, ay: f32, bx: f32, by: f32, za: f32, zb: f32) {
        let (min_x, max_x) = (ax.min(bx), ax.max(bx));
        let (min_y, max_y) = (ay.min(by), ay.max(by));
        let mut cy = (min_y / FIELD_CELL).floor() as i32;
        while cy <= (max_y / FIELD_CELL).floor() as i32 {
            let mut cx = (min_x / FIELD_CELL).floor() as i32;
            while cx <= (max_x / FIELD_CELL).floor() as i32 {
                self.grid.entry((cx, cy)).or_default().push([ax, ay, bx, by, za, zb]);
                cx += 1;
            }
            cy += 1;
        }
    }

    /// Height at (px,py): nearest solved segment within `cap` units,
    /// direction-gated (~35°) when `dir` is given. Returns (z, distance,
    /// side sign relative to the matched segment) — distance and side feed
    /// the way-level consistency filters in the annotator.
    fn sample(&self, px: f32, py: f32, dir: Option<(f32, f32)>, cap: f32) -> Option<(f32, f32, f32)> {
        let mut best_dist = cap;
        let mut best_z = None;
        let cy0 = ((py - cap) / FIELD_CELL).floor() as i32;
        let cy1 = ((py + cap) / FIELD_CELL).floor() as i32;
        let cx0 = ((px - cap) / FIELD_CELL).floor() as i32;
        let cx1 = ((px + cap) / FIELD_CELL).floor() as i32;
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let Some(segs) = self.grid.get(&(cx, cy)) else { continue };
                for seg in segs {
                    let [ax, ay, bx, by, za, zb] = *seg;
                    let (ex, ey) = (bx - ax, by - ay);
                    let el2 = (ex * ex + ey * ey).max(1e-6);
                    if let Some((dx, dy)) = dir {
                        let dl = (dx * dx + dy * dy).sqrt();
                        if dl > 1e-6
                            && ((dx * ex + dy * ey) / (dl * el2.sqrt())).abs() < 0.82
                        {
                            continue;
                        }
                    }
                    let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
                    let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
                    let dist = (qx * qx + qy * qy).sqrt();
                    if dist < best_dist {
                        best_dist = dist;
                        let side = ex * (py - ay) - ey * (px - ax);
                        best_z = Some((za * (1.0 - t) + zb * t, dist, side.signum()));
                    }
                }
            }
        }
        best_z
    }
}

/// A path from the BASE tile, in the exact enumeration order the renderer
/// uses: (layer name, feature index within layer, path index within
/// feature). Geometry scaled to 4096 units, polygons closed the same way
/// the renderer closes them.
struct BasePath {
    layer: String,
    feature: u32,
    path: u32,
    is_polygon: bool,
    points: Vec<(f32, f32)>,
}

const BASE_DZ_LAYERS: &[&str] =
    &["streets", "streets_med", "streets_low", "street_polygons", "bridges"];

fn decode_base_paths(tile: &[u8]) -> Result<Vec<BasePath>, String> {
    let pbf;
    let data: &[u8] = if tile.len() >= 2 && tile[0] == 0x1f && tile[1] == 0x8b {
        pbf = gzip_decompress_vec(tile).map_err(|e| format!("gunzip base tile: {e:?}"))?;
        &pbf
    } else {
        tile
    };
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (field, wire) = read_protobuf_key(data, &mut offset)?;
        if field == 3 && wire == 2 {
            decode_base_layer(read_protobuf_bytes(data, &mut offset)?, &mut out)?;
        } else {
            skip_protobuf_value(data, &mut offset, wire)?;
        }
    }
    Ok(out)
}

fn decode_base_layer(layer: &[u8], out: &mut Vec<BasePath>) -> Result<(), String> {
    let mut offset = 0;
    let mut name = String::new();
    let mut extent = 4096u64;
    let mut features = Vec::<&[u8]>::new();
    while offset < layer.len() {
        let (field, wire) = read_protobuf_key(layer, &mut offset)?;
        match (field, wire) {
            (1, 2) => {
                name = String::from_utf8_lossy(read_protobuf_bytes(layer, &mut offset)?)
                    .to_string();
            }
            (2, 2) => features.push(read_protobuf_bytes(layer, &mut offset)?),
            (5, 0) => extent = read_varint(layer, &mut offset)?,
            _ => skip_protobuf_value(layer, &mut offset, wire)?,
        }
    }
    if !BASE_DZ_LAYERS.contains(&name.as_str()) {
        return Ok(());
    }
    let scale = EXTENT as f32 / extent.max(1) as f32;
    for (feature_index, feature) in features.iter().enumerate() {
        let mut fo = 0;
        let mut geometry_type = 0u64;
        let mut geometry: &[u8] = &[];
        while fo < feature.len() {
            let (field, wire) = read_protobuf_key(feature, &mut fo)?;
            match (field, wire) {
                (3, 0) => geometry_type = read_varint(feature, &mut fo)?,
                (4, 2) => geometry = read_protobuf_bytes(feature, &mut fo)?,
                _ => skip_protobuf_value(feature, &mut fo, wire)?,
            }
        }
        if geometry_type != 2 && geometry_type != 3 {
            continue;
        }
        let mut paths: Vec<Vec<(f32, f32)>> = Vec::new();
        let mut path: Vec<(f32, f32)> = Vec::new();
        let (mut cx, mut cy) = (0i64, 0i64);
        let mut go = 0;
        while go < geometry.len() {
            let cmd = read_varint(geometry, &mut go)?;
            let (op, count) = (cmd & 7, cmd >> 3);
            match op {
                1 | 2 => {
                    if op == 1 && !path.is_empty() {
                        paths.push(std::mem::take(&mut path));
                    }
                    for _ in 0..count {
                        let dx = read_varint(geometry, &mut go)?;
                        let dy = read_varint(geometry, &mut go)?;
                        cx += ((dx >> 1) as i64) ^ (-((dx & 1) as i64));
                        cy += ((dy >> 1) as i64) ^ (-((dy & 1) as i64));
                        path.push((cx as f32 * scale, cy as f32 * scale));
                    }
                }
                7 => {}
                _ => return Err(format!("unknown MVT geometry op {op} in base tile")),
            }
        }
        if !path.is_empty() {
            paths.push(path);
        }
        for (path_index, mut points) in paths.into_iter().enumerate() {
            let is_polygon = geometry_type == 3;
            // Renderer closes polygon rings by appending the first point.
            if is_polygon {
                if points.first() != points.last() {
                    if let Some(first) = points.first().copied() {
                        points.push(first);
                    }
                }
            }
            out.push(BasePath {
                layer: name.clone(),
                feature: feature_index as u32,
                path: path_index as u32,
                is_polygon,
                points,
            });
        }
    }
    Ok(())
}

/// Annotate one base tile against the solved field: base_dz features keyed
/// (L, F, P) with per-raw-vertex dz. The renderer joins these to the exact
/// features it draws — no geometry matching at render time.
fn annotate_base_tiles(
    tile_paths: Vec<((u32, u32), (f32, f32), Vec<BasePath>)>,
    field: &SolvedField,
    tunnel_field: &SolvedField,
    m_per_unit: f32,
) -> HashMap<(u32, u32), Vec<TileFeature>> {
    // Flatten with per-path source tile + global offset: sampling AND the
    // junction consensus run in GLOBAL coordinates, so the overlapping
    // clip copies of one road in adjacent tiles anneal to identical
    // heights instead of per-tile-blended almost-identical ones.
    let mut base_paths: Vec<BasePath> = Vec::new();
    let mut path_src: Vec<((u32, u32), (f32, f32))> = Vec::new();
    for (src, offset, paths) in tile_paths {
        for base_path in paths {
            base_paths.push(base_path);
            path_src.push((src, offset));
        }
    }
    // Pass 1: sample every path. Interior line vertices gate by direction
    // (a road under a viaduct must not sample the deck above); ENDPOINTS
    // sample ungated with a small cap — a way meeting a deck at a junction
    // joins it at any angle, and a gated miss there tears the ramp chain
    // apart at every OSM way split.
    let mut sampled_paths: Vec<Option<Vec<f32>>> = Vec::with_capacity(base_paths.len());
    for (flat_index, base_path) in base_paths.iter().enumerate() {
        let global_offset = path_src[flat_index].1;
        let points = &base_path.points;
        if points.len() < 2 {
            sampled_paths.push(None);
            continue;
        }
        let count = points.len();
        let mut dz = vec![0.0f32; count];
        let mut match_dists: Vec<f32> = Vec::new();
        let mut side_pos = false;
        let mut side_neg = false;
        for index in 0..count {
            let (px, py) = points[index];
            let (gx, gy) = (px + global_offset.0, py + global_offset.1);
            let sampled = if base_path.is_polygon {
                field.sample(gx, gy, None, 2.4)
            } else {
                let endpoint = index == 0 || index == count - 1;
                let previous = points[index.saturating_sub(1)];
                let next = points[(index + 1).min(count - 1)];
                let dir = (next.0 - previous.0, next.1 - previous.1);
                let gated = field.sample(gx, gy, Some(dir), 1.2);
                if endpoint {
                    gated.or_else(|| field.sample(gx, gy, None, 0.8))
                } else {
                    gated
                }
            };
            if let Some((height, dist, side)) = sampled {
                if height > 0.2 {
                    dz[index] = height;
                    match_dists.push(dist);
                    if side >= 0.0 {
                        side_pos = true;
                    } else {
                        side_neg = true;
                    }
                }
            }
        }
        // Way-level consistency: a road's OWN solved centerline matches at
        // ~0.2-0.6 units; a PARALLEL neighbor (frontage street beside a
        // viaduct — passes the direction gate!) matches at 1.2+. Median
        // match distance separates them. Deck POLYGONS legitimately match
        // at half-width from the centerline, but they STRADDLE it — a
        // plaza lying beside a viaduct is entirely on one side.
        if !match_dists.is_empty() {
            let mut sorted = match_dists.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = sorted[sorted.len() / 2];
            let reject = if base_path.is_polygon {
                !(side_pos && side_neg)
            } else {
                median > 1.0
            };
            if reject {
                for value in dz.iter_mut() {
                    *value = 0.0;
                }
            }
        }
        // Tunnel pass: paths with no deck lift try the (negative) tunnel
        // field with the same gating — its own centerline matches tight,
        // parallel surface streets are median-rejected exactly like decks.
        if !base_path.is_polygon && dz.iter().all(|&v| v == 0.0) {
            let mut tunnel_dists: Vec<f32> = Vec::new();
            for index in 0..count {
                let (px, py) = points[index];
                let endpoint = index == 0 || index == count - 1;
                let previous = points[index.saturating_sub(1)];
                let next = points[(index + 1).min(count - 1)];
                let dir = (next.0 - previous.0, next.1 - previous.1);
                let (gx, gy) = (px + global_offset.0, py + global_offset.1);
                let gated = tunnel_field.sample(gx, gy, Some(dir), 1.2);
                let sampled = if endpoint {
                    gated.or_else(|| tunnel_field.sample(gx, gy, None, 0.8))
                } else {
                    gated
                };
                if let Some((depth, dist, _)) = sampled {
                    if depth < -0.2 {
                        dz[index] = depth;
                        tunnel_dists.push(dist);
                    }
                }
            }
            if !tunnel_dists.is_empty() {
                let mut sorted = tunnel_dists.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                if sorted[sorted.len() / 2] > 1.0 {
                    for value in dz.iter_mut() {
                        *value = 0.0;
                    }
                }
            }
        }
        sampled_paths.push(Some(dz));
    }
    // Pass 2: junction continuity. Split ways share endpoint coordinates;
    // the deck height at a shared endpoint must agree or consecutive
    // segments disconnect mid-ramp. Take the max over all path endpoints
    // at the same (rounded) coordinate, then blend the raised endpoint
    // into each path at road grade so it doesn't spike.
    // Consensus over ALL coincident line vertices (0.25-unit grid): split
    // ways duplicate the centerline through a gore, and independently
    // sampled dz disagrees by decimeters — in 2D overpainting hides it, in
    // 3D the overlap fractures into slivers. Coincident vertices must
    // agree exactly.
    let mut junction: HashMap<(i32, i32), f32> = HashMap::new();
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = &sampled_paths[path_index] else { continue };
        if base_path.is_polygon {
            continue;
        }
        let offset = path_src[path_index].1;
        for (vertex, &height) in dz.iter().enumerate() {
            if height <= 0.2 {
                continue;
            }
            let (px, py) = base_path.points[vertex];
            let key = (
                ((px + offset.0) * 4.0).round() as i32,
                ((py + offset.1) * 4.0).round() as i32,
            );
            let entry = junction.entry(key).or_insert(0.0);
            if height > *entry {
                *entry = height;
            }
        }
    }
    let grade_per_unit = 0.08 * m_per_unit;
    let mut final_dz: Vec<Option<Vec<f32>>> = vec![None; base_paths.len()];
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(mut dz) = sampled_paths[path_index].clone() else { continue };
        let offset = path_src[path_index].1;
        let points = &base_path.points;
        let count = points.len();
        if !base_path.is_polygon {
            for vertex in 0..count {
                let (px, py) = points[vertex];
                let key = (
                    ((px + offset.0) * 4.0).round() as i32,
                    ((py + offset.1) * 4.0).round() as i32,
                );
                if let Some(&height) = junction.get(&key) {
                    // Take the consensus only where this way already
                    // carries height at or next to the vertex — a grounded
                    // side street touching a lifted junction node must not
                    // inherit a curb-lift.
                    let neighbor = dz[vertex.saturating_sub(1)]
                        .max(dz[(vertex + 1).min(count - 1)]);
                    let supported =
                        dz[vertex].max(neighbor) > (0.3 * height).min(0.5);
                    if supported && height > dz[vertex] {
                        dz[vertex] = height;
                    }
                }
            }
            // Grade-limited blend from both ends so a raised junction
            // endpoint ramps into the path instead of spiking.
            for index in 1..count {
                let seg = distance(points[index - 1], points[index]);
                let limit = dz[index - 1] - grade_per_unit * seg;
                if limit > dz[index] {
                    dz[index] = limit;
                }
            }
            for index in (0..count - 1).rev() {
                let seg = distance(points[index], points[index + 1]);
                let limit = dz[index + 1] - grade_per_unit * seg;
                if limit > dz[index] {
                    dz[index] = limit;
                }
            }
        }
        // Close single-vertex dropouts (cap-boundary flutter along a deck).
        if count >= 3 {
            for index in 1..count - 1 {
                let fill = dz[index - 1].min(dz[index + 1]);
                if fill > dz[index] + 0.05 {
                    dz[index] = fill;
                }
            }
        }
        final_dz[path_index] = Some(dz);
    }
    // Final pass: EXACT consensus over coincident line vertices across all
    // tiles — overlap clip copies of one road share their inner vertices,
    // and after per-path blending they can differ by centimeters, which a
    // tilted camera renders as doubled slabs. Largest |dz| wins.
    let mut vertex_consensus: HashMap<(i32, i32), f32> = HashMap::new();
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = &final_dz[path_index] else { continue };
        if base_path.is_polygon {
            continue;
        }
        let offset = path_src[path_index].1;
        for (vertex, &value) in dz.iter().enumerate() {
            let (px, py) = base_path.points[vertex];
            let key = (
                ((px + offset.0) * 4.0).round() as i32,
                ((py + offset.1) * 4.0).round() as i32,
            );
            let entry = vertex_consensus.entry(key).or_insert(value);
            if value.abs() > entry.abs() {
                *entry = value;
            }
        }
    }
    let mut per_tile: HashMap<(u32, u32), Vec<TileFeature>> = HashMap::new();
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(mut dz) = final_dz[path_index].take() else { continue };
        let (src, offset) = path_src[path_index];
        if !base_path.is_polygon {
            for (vertex, value) in dz.iter_mut().enumerate() {
                let (px, py) = base_path.points[vertex];
                let key = (
                    ((px + offset.0) * 4.0).round() as i32,
                    ((py + offset.1) * 4.0).round() as i32,
                );
                if let Some(&consensus) = vertex_consensus.get(&key) {
                    *value = consensus;
                }
            }
        }
        let any = dz.iter().any(|&v| v.abs() > 0.2);
        if !any {
            continue;
        }
        let dz_tag = dz
            .iter()
            .map(|v| ((v * 10.0).round() as i64).to_string())
            .collect::<Vec<_>>()
            .join(",");
        per_tile.entry(src).or_default().push(TileFeature {
            layer: Layer::BaseDz,
            geometry_type: GeometryType::LineString,
            osm_type: OsmType::Way,
            id: (base_path.feature as i64) << 8 | base_path.path as i64,
            closed: false,
            tags: vec![
                ("L".to_string(), base_path.layer.clone()),
                ("F".to_string(), base_path.feature.to_string()),
                ("P".to_string(), base_path.path.to_string()),
                ("dz".to_string(), dz_tag),
            ],
            paths: vec![base_path
                .points
                .iter()
                .map(|&(px, py)| TilePoint { x: px.round() as i32, y: py.round() as i32 })
                .collect()],
        });
    }
    per_tile
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

pub fn bake(options: BakeOptions) -> Result<(), String> {
    let (west, south, east, north) = options.bbox;
    if east <= west || north <= south {
        return Err("bridge-bake: empty bbox".into());
    }
    let zoom = options.zoom;
    let x0 = lon_to_tile_x(west, zoom).floor().max(0.0) as u32;
    let x1 = lon_to_tile_x(east, zoom).floor() as u32;
    let y0 = lat_to_tile_y(north, zoom).floor().max(0.0) as u32;
    let y1 = lat_to_tile_y(south, zoom).floor() as u32;
    eprintln!(
        "bridge-bake: z{zoom} x{x0}..{x1} y{y0}..{y1} ({} tiles)",
        u64::from(x1 - x0 + 1) * u64::from(y1 - y0 + 1)
    );

    let mut reader = MbtilesReader::open(&options.detail)
        .map_err(|e| format!("open {}: {e}", options.detail.display()))?;
    let mut base_reader = match options.base.as_deref() {
        Some(path) => {
            Some(MbtilesReader::open(path).map_err(|e| format!("open {}: {e}", path.display()))?)
        }
        None => None,
    };
    let mut ahn = Ahn::open(options.ahn_dir.as_deref());

    // Decode every needed tile (solve range + 1 ring) exactly once.
    let axis = 1i64 << zoom;
    let mut decoded: HashMap<(u32, u32), Vec<RawWay>> = HashMap::new();
    for y in y0.saturating_sub(1)..=(y1 + 1).min(axis as u32 - 1) {
        for x in x0.saturating_sub(1)..=(x1 + 1).min(axis as u32 - 1) {
            let tms_row = axis - 1 - i64::from(y);
            match reader.get_tile(i64::from(zoom), i64::from(x), tms_row) {
                Ok(Some(raw)) => match decode_osm_line_ways(&raw) {
                    Ok(ways) => {
                        decoded.insert((x, y), ways);
                    }
                    Err(error) => {
                        eprintln!("bridge-bake: decode z{zoom}/{x}/{y}: {error}");
                    }
                },
                Ok(None) => {}
                Err(error) => eprintln!("bridge-bake: read z{zoom}/{x}/{y}: {error}"),
            }
        }
    }
    eprintln!("bridge-bake: {} detail tiles decoded", decoded.len());

    let mut jobs: Vec<TileJob> = Vec::new();
    let mut base_annotated = 0usize;
    let origin = (
        x0.saturating_sub(1),
        y0.saturating_sub(1),
    );
    let (mut per_tile_features, totals, field, tunnel_field) =
        solve_bbox(zoom, origin, (x0, y0, x1, y1), &decoded, &mut ahn);
    // Decode every base tile up front: annotation runs as ONE global pass
    // so overlap clip copies anneal to identical heights.
    let mut tile_paths: Vec<((u32, u32), (f32, f32), Vec<BasePath>)> = Vec::new();
    if let Some(base) = base_reader.as_mut() {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let tms_row = axis - 1 - i64::from(y);
                if let Ok(Some(base_raw)) = base.get_tile(i64::from(zoom), i64::from(x), tms_row)
                {
                    match decode_base_paths(&base_raw) {
                        Ok(paths) => {
                            let offset = (
                                (x as i64 - origin.0 as i64) as f32 * EXTENT as f32,
                                (y as i64 - origin.1 as i64) as f32 * EXTENT as f32,
                            );
                            tile_paths.push(((x, y), offset, paths));
                        }
                        Err(error) => {
                            eprintln!("bridge-bake: decode base z{zoom}/{x}/{y}: {error}")
                        }
                    }
                }
            }
        }
    }
    let bbox_center_lat = tile_y_to_lat((f64::from(y0) + f64::from(y1)) * 0.5 + 0.5, zoom);
    let m_per_unit_global = (bbox_center_lat.to_radians().cos() * 40_075_016.686
        / f64::from(1u32 << zoom)
        / EXTENT) as f32;
    let mut annotated_per_tile =
        annotate_base_tiles(tile_paths, &field, &tunnel_field, m_per_unit_global);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let mut features = per_tile_features.remove(&(x, y)).unwrap_or_default();
            if let Some(mut annotated) = annotated_per_tile.remove(&(x, y)) {
                base_annotated += annotated.len();
                features.append(&mut annotated);
            }
            if !features.is_empty() {
                let stats = SolveStats::default();
                jobs.push(TileJob { z: zoom, x, y, features, stats });
            }
        }
    }
    eprintln!(
        "bridge-bake: solved {} ways, {} crossing constraints, {} AHN-measured decks, {} baked ways, {} base paths annotated in {} tiles",
        totals.ways, totals.crossings, totals.measured, totals.baked, base_annotated, jobs.len()
    );

    // Cross-tile boundary consensus: every tile solves with only 1-ring
    // context, so the same way can bake slightly different heights on the
    // two sides of a tile seam — a visible step mid-deck. Unify base_dz
    // vertices lying ON tile edges by the largest-|dz| value seen across
    // tiles (bridge continuity favors the lift, tunnels the sink).
    {
        let near_edge = |v: f64| -> bool { v <= 2.0 || v >= EXTENT - 2.0 };
        let global_key = |job_x: u32, job_y: u32, px: f64, py: f64| -> (i64, i64) {
            (
                i64::from(job_x) * EXTENT as i64 * 4 + (px * 4.0).round() as i64,
                i64::from(job_y) * EXTENT as i64 * 4 + (py * 4.0).round() as i64,
            )
        };
        let mut consensus: HashMap<(i64, i64), f32> = HashMap::new();
        for job in &jobs {
            for feature in &job.features {
                if !matches!(feature.layer, Layer::BaseDz) {
                    continue;
                }
                let Some(dz_tag) = feature.tags.iter().find(|(k, _)| k == "dz") else {
                    continue;
                };
                let values: Vec<f32> = dz_tag
                    .1
                    .split(',')
                    .filter_map(|v| v.parse::<f32>().ok())
                    .collect();
                for (point, &value) in feature.paths[0].iter().zip(values.iter()) {
                    let (px, py) = (f64::from(point.x), f64::from(point.y));
                    if near_edge(px) || near_edge(py) {
                        let key = global_key(job.x, job.y, px, py);
                        let entry = consensus.entry(key).or_insert(value);
                        if value.abs() > entry.abs() {
                            *entry = value;
                        }
                    }
                }
            }
        }
        let mut patched = 0usize;
        for job in &mut jobs {
            let (job_x, job_y) = (job.x, job.y);
            for feature in &mut job.features {
                if !matches!(feature.layer, Layer::BaseDz) {
                    continue;
                }
                let points: Vec<(f64, f64)> = feature.paths[0]
                    .iter()
                    .map(|p| (f64::from(p.x), f64::from(p.y)))
                    .collect();
                let Some(dz_tag) = feature.tags.iter_mut().find(|(k, _)| k == "dz") else {
                    continue;
                };
                let mut values: Vec<i64> = dz_tag
                    .1
                    .split(',')
                    .filter_map(|v| v.parse::<i64>().ok())
                    .collect();
                let mut changed = false;
                for (index, &(px, py)) in points.iter().enumerate() {
                    if index >= values.len() || !(near_edge(px) || near_edge(py)) {
                        continue;
                    }
                    if let Some(&value) = consensus.get(&global_key(job_x, job_y, px, py)) {
                        // Tag values are decimeters already.
                        let dm = value.round() as i64;
                        if dm != values[index] {
                            values[index] = dm;
                            changed = true;
                        }
                    }
                }
                if changed {
                    patched += 1;
                    dz_tag.1 = values
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                }
            }
        }
        eprintln!("bridge-bake: boundary consensus patched {patched} paths");
    }

    if options.output.exists() {
        std::fs::remove_file(&options.output)
            .map_err(|e| format!("remove {}: {e}", options.output.display()))?;
    }
    let mut writer = MbtilesWriter::create(&options.output)
        .map_err(|e| format!("create {}: {e}", options.output.display()))?;
    writer.set_metadata("name", "Makepad bridge dz (solved + AHN measured)");
    writer.set_metadata(
        "description",
        "Per-vertex road/rail elevation above ground, constraint-solved from OSM layers with AHN DSM-DTM measured deck heights",
    );
    writer.set_metadata("type", "overlay");
    writer.set_metadata("version", "1");
    writer.set_metadata("format", "pbf");
    writer.set_metadata("scheme", "tms");
    writer.set_metadata("minzoom", zoom.to_string());
    writer.set_metadata("maxzoom", zoom.to_string());
    // Bounds are the FULL baked tile rectangle, not the input bbox: the
    // renderer treats containment as coverage, and edge tiles are solved
    // completely (the bake iterates whole tiles).
    let bounds_west = tile_x_to_lon(f64::from(x0), zoom);
    let bounds_east = tile_x_to_lon(f64::from(x1 + 1), zoom);
    let bounds_north = tile_y_to_lat(f64::from(y0), zoom);
    let bounds_south = tile_y_to_lat(f64::from(y1 + 1), zoom);
    writer.set_metadata(
        "bounds",
        format!("{bounds_west:.7},{bounds_south:.7},{bounds_east:.7},{bounds_north:.7}"),
    );
    writer.set_metadata("attribution", "OpenStreetMap contributors; AHN (CC0)");
    writer.set_metadata(
        "json",
        r#"{"vector_layers":[{"id":"bridge_dz","fields":{"dz":"String","hw":"String"}}]}"#,
    );

    // MbtilesWriter requires block-major rowid order.
    jobs.sort_by_key(|job| (job.y >> 8, job.x >> 8, job.y & 255, job.x & 255));
    for job in jobs {
        let pbf = encode_tile(job.features)?;
        let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
        gzip.write_all(&pbf)
            .map_err(|e| format!("gzip tile {}/{}/{}: {e}", job.z, job.x, job.y))?;
        let tile = gzip
            .finish()
            .map_err(|e| format!("finish gzip tile {}/{}/{}: {e}", job.z, job.x, job.y))?;
        writer
            .write_tile_xyz(job.z, job.x, job.y, &tile)
            .map_err(|e| format!("write tile {}/{}/{}: {e}", job.z, job.x, job.y))?;
        eprintln!(
            "  z{}/{}/{}: {} ways ({} crossings{})",
            job.z,
            job.x,
            job.y,
            job.stats.baked,
            job.stats.crossings,
            if job.stats.measured > 0 {
                format!(", {} measured", job.stats.measured)
            } else {
                String::new()
            }
        );
    }
    let stats = writer
        .finish()
        .map_err(|e| format!("finish {}: {e}", options.output.display()))?;
    eprintln!(
        "bridge-bake: wrote {} tiles, {} bytes -> {}",
        stats.tile_count,
        stats.tile_bytes,
        options.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // needs a local bake output
    fn probe_baked_dz() {
        let path = Path::new("../../examples/map/local/maps/ams-bridge-dz.mbtiles");
        let mut reader = MbtilesReader::open(path).unwrap();
        // RAI / Europaboulevard rail-yard crossing tile.
        let (z, x, y) = (14i64, 8414i64, 5387u32);
        let tms = (1i64 << z) - 1 - y as i64;
        let raw = reader.get_tile(z, x, tms).unwrap().unwrap();
        let ways = decode_line_ways(&raw, "bridge_dz").unwrap();
        eprintln!("tile z{z}/{x}/{y}: {} dz ways", ways.len());
        let mut best: Vec<(f32, usize, String)> = ways
            .iter()
            .enumerate()
            .map(|(index, way)| {
                let dz = tag(&way.tags, "dz").unwrap_or("");
                let max = dz
                    .split(',')
                    .filter_map(|v| v.parse::<f32>().ok())
                    .fold(0.0f32, f32::max)
                    / 10.0;
                (max, index, dz.to_string())
            })
            .collect();
        best.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (max, index, dz) in best.iter().take(8) {
            let way = &ways[*index];
            let n = way.paths[0].len();
            let (sx, sy) = way.paths[0][0];
            eprintln!(
                "  id {} max {max:.1} m, {n} pts, start ({sx:.0},{sy:.0}), hw {:?}, dz {}",
                way.id,
                tag(&way.tags, "hw"),
                &dz[..dz.len().min(120)]
            );
        }
        let lifted = best.iter().filter(|(m, _, _)| *m > 1.0).count();
        eprintln!("ways with lift > 1 m: {lifted}");
    }
}

#[cfg(test)]
mod probe_amstelveenseweg {
    use super::*;

    #[test]
    #[ignore] // needs local archives
    fn dump_crossing() {
        let mut detail = MbtilesReader::open(Path::new(
            "../../examples/map/local/maps/europe-osm-detail.mbtiles",
        ))
        .unwrap();
        let mut baked = MbtilesReader::open(Path::new(
            "../../examples/map/local/maps/ams-bridge-dz.mbtiles",
        ))
        .unwrap();
        let spots: &[(&str, i64, i64, f32, f32, f32, f32)] = &[
            ("flatX-138", 8412, 5386, 200.0, 1500.0, 900.0, 2300.0),
            ("amstelveen-140", 8413, 5386, 300.0, 1100.0, 2700.0, 3500.0),
        ];
        for &(tag_name, x, y, x0, x1, y0, y1) in spots {
            println!("=== {tag_name} tile {x}/{y} window x{x0}-{x1} y{y0}-{y1}");
            let raw = detail.get_tile(14, x, (1 << 14) - 1 - y).unwrap().unwrap();
            let ways = decode_osm_line_ways(&raw).unwrap();
            for way in &ways {
                let hw = tag(&way.tags, "highway").unwrap_or("");
                let rw = tag(&way.tags, "railway").unwrap_or("");
                if !(hw.starts_with("motorway") || hw == "secondary" || hw == "primary"
                    || hw == "trunk" || rw == "rail" || rw == "subway" || rw == "tram")
                {
                    continue;
                }
                let hit = way.paths.iter().flatten().any(|&(px, py)| {
                    (x0..=x1).contains(&px) && (y0..=y1).contains(&py)
                });
                if !hit {
                    continue;
                }
                println!(
                    "  id {} hw={hw} rw={rw} bridge={:?} layer={:?} pts {} first {:?}",
                    way.id,
                    tag(&way.tags, "bridge"),
                    tag(&way.tags, "osm_layer"),
                    way.paths.iter().map(|p| p.len()).sum::<usize>(),
                    way.paths[0][0],
                );
            }
            if let Ok(Some(raw)) = baked.get_tile(14, x, (1 << 14) - 1 - y) {
                let dz_ways = decode_line_ways(&raw, "bridge_dz").unwrap();
                for way in &dz_ways {
                    let hit = way.paths.iter().flatten().any(|&(px, py)| {
                        (x0..=x1).contains(&px) && (y0..=y1).contains(&py)
                    });
                    if !hit {
                        continue;
                    }
                    let dz = tag(&way.tags, "dz").unwrap_or("");
                    let max = dz
                        .split(',')
                        .filter_map(|v| v.parse::<f32>().ok())
                        .fold(0.0f32, f32::max)
                        / 10.0;
                    if max > 0.5 {
                        println!("  BAKED id {} max {max:.1}", way.id);
                    }
                }
            } else {
                println!("  (no baked tile)");
            }
        }
    }
    #[test]
    #[ignore] // needs local archives
    fn solve_amstelveenseweg() {
        let mut detail = MbtilesReader::open(Path::new(
            "../../examples/map/local/maps/europe-osm-detail.mbtiles",
        ))
        .unwrap();
        let mut decoded: HashMap<(u32, u32), Vec<RawWay>> = HashMap::new();
        for y in 5385u32..=5387 {
            for x in 8412u32..=8414 {
                if let Ok(Some(raw)) = detail.get_tile(14, x as i64, (1 << 14) - 1 - y as i64) {
                    decoded.insert((x, y), decode_osm_line_ways(&raw).unwrap());
                }
            }
        }
        let mut ahn = Ahn::open(Some(Path::new("../../examples/map/local/ahn")));
        let (per_tile, stats, _field, _tunnel_field) =
            solve_bbox(14, (8412, 5385), (8413, 5386, 8413, 5386), &decoded, &mut ahn);
        let features = per_tile.get(&(8413, 5386)).cloned().unwrap_or_default();
        eprintln!(
            "solved: {} ways {} crossings {} measured {} baked",
            stats.ways, stats.crossings, stats.measured, stats.baked
        );
        for feature in &features {
            let id = feature.id;
            if matches!(id, 104447474 | 7381773 | 515946648 | 7381715 | 788302985) {
                let dz = feature
                    .tags
                    .iter()
                    .find(|(k, _)| k == "dz")
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");
                eprintln!("OUT id {id} dz {dz}");
            }
        }
    }
    #[test]
    #[ignore] // needs local bake output
    fn probe_rozenoord_seam() {
        let mut baked = MbtilesReader::open(Path::new(
            "../../examples/map/local/maps/ams-bridge-dz.mbtiles",
        ))
        .unwrap();
        for x in [8414i64, 8415] {
            let y = 5387i64;
            let Ok(Some(raw)) = baked.get_tile(14, x, (1 << 14) - 1 - y) else {
                println!("tile {x}/{y}: MISSING");
                continue;
            };
            let ways = decode_line_ways(&raw, "base_dz").unwrap();
            println!("tile {x}/{y}: {} base_dz entries", ways.len());
            for way in &ways {
                let dz = tag(&way.tags, "dz").unwrap_or("");
                let max = dz
                    .split(',')
                    .filter_map(|v| v.parse::<f32>().ok())
                    .fold(0.0f32, f32::max)
                    / 10.0;
                // Metro bridge is ~10-19 m and near the seam (local x ~4050+
                // in 8414 / ~0-400 in 8415, y ~900-1100).
                let near_seam = way.paths.iter().flatten().any(|&(px, py)| {
                    (850.0..=1250.0).contains(&py)
                        && if x == 8414 { px > 3700.0 } else { px < 500.0 }
                });
                if near_seam && max > 6.0 {
                    let first = way.paths[0][0];
                    let last = *way.paths[0].last().unwrap();
                    println!(
                        "  {}={:?} F={:?} P={:?} max {max:.1} first ({:.0},{:.0}) last ({:.0},{:.0}) dz {}",
                        "L",
                        tag(&way.tags, "L"),
                        tag(&way.tags, "F"),
                        tag(&way.tags, "P"),
                        first.0, first.1, last.0, last.1,
                        &dz[..dz.len().min(80)]
                    );
                }
            }
        }
    }
    #[test]
    #[ignore] // needs local archives
    fn probe_rozenoord_east_annotate() {
        let mut detail = MbtilesReader::open(Path::new(
            "../../examples/map/local/maps/europe-osm-detail.mbtiles",
        ))
        .unwrap();
        let mut decoded: HashMap<(u32, u32), Vec<RawWay>> = HashMap::new();
        for y in 5386u32..=5388 {
            for x in 8414u32..=8416 {
                if let Ok(Some(raw)) = detail.get_tile(14, x as i64, (1 << 14) - 1 - y as i64) {
                    decoded.insert((x, y), decode_osm_line_ways(&raw).unwrap());
                }
            }
        }
        let mut ahn = Ahn::open(Some(Path::new("../../examples/map/local/ahn")));
        let (_per_tile, _stats, field, _tunnel_field) =
            solve_bbox(14, (8414, 5386), (8415, 5387, 8415, 5387), &decoded, &mut ahn);
        // Field height right at the seam where the metro crosses:
        for probe in [(64.0f32, 960.0f32), (10.0, 990.0), (200.0, 900.0), (400.0, 850.0)] {
            let ungated = field.sample(probe.0, probe.1, None, 3.0);
            println!("field at {:?}: {:?}", probe, ungated);
        }
        // Base paths near the seam:
        let mut base = MbtilesReader::open(Path::new(
            "../../examples/map/local/maps/europe-shortbread.mbtiles",
        ))
        .unwrap();
        let base_raw = base.get_tile(14, 8415, (1 << 14) - 1 - 5387).unwrap().unwrap();
        for path in decode_base_paths(&base_raw).unwrap() {
            if path.is_polygon {
                continue;
            }
            let near = path.points.iter().any(|&(px, py)| {
                px < 500.0 && (700.0..=1150.0).contains(&py)
            });
            if !near {
                continue;
            }
            let count = path.points.len();
            let mut sampled = Vec::new();
            for index in 0..count.min(8) {
                let (px, py) = path.points[index];
                let previous = path.points[index.saturating_sub(1)];
                let next = path.points[(index + 1).min(count - 1)];
                let dir = (next.0 - previous.0, next.1 - previous.1);
                sampled.push(field.sample(px, py, Some(dir), 1.2).map(|(z, d, _)| (z, d)));
            }
            println!(
                "L={} F={} P={} pts {} first ({:.0},{:.0}) samples {:?}",
                path.layer, path.feature, path.path, count,
                path.points[0].0, path.points[0].1, sampled
            );
        }
    }
}
