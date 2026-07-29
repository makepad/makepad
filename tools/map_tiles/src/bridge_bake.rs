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

    fn is_truthy(&self) -> bool {
        match self {
            MvtVal::Str(value) => {
                !matches!(value.as_str(), "" | "0" | "no" | "false" | "False")
            }
            MvtVal::Num(value) => *value != 0.0,
            MvtVal::Bool(value) => *value,
        }
    }

    fn osm_layer(&self) -> Option<i32> {
        let value = match self {
            MvtVal::Str(value) => value.parse::<f32>().ok()?,
            MvtVal::Num(value) => *value as f32,
            MvtVal::Bool(_) => return None,
        };
        value.is_finite().then(|| value.round() as i32)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VerticalClass {
    Tunnel,
    Surface,
    Elevated,
}

impl VerticalClass {
    fn normalized_layer(bridge: bool, tunnel: bool, osm_layer: Option<i32>) -> i32 {
        match osm_layer {
            Some(layer) if bridge && layer < 1 => 1,
            Some(layer) if tunnel && layer > -1 => -1,
            Some(layer) => layer,
            None if bridge => 1,
            None if tunnel => -1,
            None => 0,
        }
    }

    fn from_solve_way(bridge: bool, tunnel: bool, layer: i32) -> Self {
        if tunnel || layer < 0 {
            Self::Tunnel
        } else if bridge || layer > 0 {
            Self::Elevated
        } else {
            Self::Surface
        }
    }

    fn from_base_feature(
        layer: &str,
        bridge: bool,
        tunnel: bool,
        osm_layer: Option<&MvtVal>,
    ) -> Self {
        let bridge = bridge || layer == "bridges";
        let layer =
            Self::normalized_layer(bridge, tunnel, osm_layer.and_then(MvtVal::osm_layer));
        Self::from_solve_way(bridge, tunnel, layer)
    }
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
        let layer = VerticalClass::normalized_layer(bridge, tunnel, osm_layer);
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

    // Lateral consensus: a dual carriageway is TWO parallel ways whose
    // ribbons overlap in 2D — if their solved profiles differ, the merged
    // surface renders a height step down its middle (persistent stepped
    // twin slabs). Each way vertex adopts the highest solved height found
    // on a parallel SAME-CLASS way (width classes match on duals) within
    // its own half-width. A nearby service/bus road is a different class
    // and must NOT be hoisted to deck height.
    {
        let mut adopted: Vec<(usize, f32)> = Vec::new();
        for way in &graph.ways {
            if way.tunnel || way.nodes.len() < 2 {
                continue;
            }
            let cap = way.half_width_m / m_per_unit.max(1e-6);
            for index in 0..way.nodes.len() {
                let node = way.nodes[index] as usize;
                let (px, py) = graph.pos[node];
                let previous =
                    graph.pos[way.nodes[index.saturating_sub(1)] as usize];
                let next =
                    graph.pos[way.nodes[(index + 1).min(way.nodes.len() - 1)] as usize];
                let (dx, dy) = (next.0 - previous.0, next.1 - previous.1);
                let dl = (dx * dx + dy * dy).sqrt().max(1e-6);
                let mut best = z[node];
                let cy0 = ((py - cap) / CELL).floor() as i32;
                let cy1 = ((py + cap) / CELL).floor() as i32;
                let cx0 = ((px - cap) / CELL).floor() as i32;
                let cx1 = ((px + cap) / CELL).floor() as i32;
                for cy in cy0..=cy1 {
                    for cx in cx0..=cx1 {
                        let Some(bucket) = grid.get(&(cx, cy)) else { continue };
                        for &(other_index, other_seg) in bucket {
                            let other = &graph.ways[other_index];
                            if other.tunnel
                                || (other.half_width_m - way.half_width_m).abs() > 1.5
                            {
                                continue;
                            }
                            let a = graph.pos[other.nodes[other_seg] as usize];
                            let b = graph.pos[other.nodes[other_seg + 1] as usize];
                            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                            let el2 = (ex * ex + ey * ey).max(1e-6);
                            if ((dx * ex + dy * ey) / (dl * el2.sqrt())).abs() < 0.82 {
                                continue;
                            }
                            let t = (((px - a.0) * ex + (py - a.1) * ey) / el2)
                                .clamp(0.0, 1.0);
                            let (qx, qy) = (a.0 + ex * t - px, a.1 + ey * t - py);
                            let dist = (qx * qx + qy * qy).sqrt();
                            if dist <= 0.7 || dist >= cap {
                                continue;
                            }
                            let za = z[other.nodes[other_seg] as usize];
                            let zb = z[other.nodes[other_seg + 1] as usize];
                            let height = za * (1.0 - t) + zb * t;
                            if height > best + 0.3 {
                                best = height;
                            }
                        }
                    }
                }
                if best > z[node] + 0.3 {
                    adopted.push((node, best));
                }
            }
        }
        for (node, height) in adopted {
            if height > z[node] {
                z[node] = height;
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
                VerticalClass::from_solve_way(way.bridge, way.tunnel, way.layer),
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
                VerticalClass::from_solve_way(way.bridge, way.tunnel, way.layer),
            );
        }
    }
    (features, stats, field, tunnel_field)
}

/// Spatial index over the solved height segments of one tile solve, for
/// annotating the base tile's own geometry.
#[derive(Clone, Copy)]
struct SolvedSegment {
    geometry: [f32; 6],
    vertical: VerticalClass,
}

#[derive(Default)]
struct SolvedField {
    grid: HashMap<(i32, i32), Vec<SolvedSegment>>,
}

const FIELD_CELL: f32 = 96.0;
const VERTICAL_CLASS_AMBIGUITY: f32 = 0.75;

impl SolvedField {
    fn push(
        &mut self,
        ax: f32,
        ay: f32,
        bx: f32,
        by: f32,
        za: f32,
        zb: f32,
        vertical: VerticalClass,
    ) {
        let (min_x, max_x) = (ax.min(bx), ax.max(bx));
        let (min_y, max_y) = (ay.min(by), ay.max(by));
        let solved = SolvedSegment {
            geometry: [ax, ay, bx, by, za, zb],
            vertical,
        };
        let mut cy = (min_y / FIELD_CELL).floor() as i32;
        while cy <= (max_y / FIELD_CELL).floor() as i32 {
            let mut cx = (min_x / FIELD_CELL).floor() as i32;
            while cx <= (max_x / FIELD_CELL).floor() as i32 {
                self.grid.entry((cx, cy)).or_default().push(solved);
                cx += 1;
            }
            cy += 1;
        }
    }

    /// Height at (px,py): nearest solved segment within `cap` units,
    /// direction-gated (~35°) when `dir` is given. Returns (z, distance,
    /// side sign relative to the matched segment) — distance and side feed
    /// the way-level consistency filters in the annotator.
    fn sample(
        &self,
        px: f32,
        py: f32,
        dir: Option<(f32, f32)>,
        cap: f32,
        vertical: Option<VerticalClass>,
    ) -> Option<(f32, f32, f32)> {
        self.sample_gated(px, py, dir, cap, 0.82, vertical)
    }

    /// Highest z among PARALLEL segments with lateral distance in
    /// (min_dist, cap) — the own centerline (dist ~0) is excluded, so a
    /// dual-carriageway twin's height is visible past one's own line.
    fn parallel_max(
        &self,
        px: f32,
        py: f32,
        dir: (f32, f32),
        min_dist: f32,
        cap: f32,
    ) -> Option<f32> {
        let mut best: Option<f32> = None;
        let cy0 = ((py - cap) / FIELD_CELL).floor() as i32;
        let cy1 = ((py + cap) / FIELD_CELL).floor() as i32;
        let cx0 = ((px - cap) / FIELD_CELL).floor() as i32;
        let cx1 = ((px + cap) / FIELD_CELL).floor() as i32;
        let dl = (dir.0 * dir.0 + dir.1 * dir.1).sqrt().max(1e-6);
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let Some(segs) = self.grid.get(&(cx, cy)) else { continue };
                for seg in segs {
                    let [ax, ay, bx, by, za, zb] = seg.geometry;
                    let (ex, ey) = (bx - ax, by - ay);
                    let el2 = (ex * ex + ey * ey).max(1e-6);
                    if ((dir.0 * ex + dir.1 * ey) / (dl * el2.sqrt())).abs() < 0.82 {
                        continue;
                    }
                    let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
                    let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
                    let dist = (qx * qx + qy * qy).sqrt();
                    if dist <= min_dist || dist >= cap {
                        continue;
                    }
                    let height = za * (1.0 - t) + zb * t;
                    if best.is_none_or(|b| height > b) {
                        best = Some(height);
                    }
                }
            }
        }
        best
    }

    /// `min_dot` relaxes the direction gate: endpoint fallbacks use ~0.5 so
    /// angled merges still join their deck while a way SPLIT sitting under
    /// a crossing viaduct no longer grabs the deck overhead (that spike
    /// rendered as a detached floating slab).
    fn sample_gated(
        &self,
        px: f32,
        py: f32,
        dir: Option<(f32, f32)>,
        cap: f32,
        min_dot: f32,
        vertical: Option<VerticalClass>,
    ) -> Option<(f32, f32, f32)> {
        fn candidate_is_better(
            candidate: (f32, f32, f32),
            best: (f32, f32, f32),
        ) -> bool {
            match candidate.1.total_cmp(&best.1) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Greater => false,
                std::cmp::Ordering::Equal => match candidate
                    .0
                    .abs()
                    .total_cmp(&best.0.abs())
                {
                    // Equal-distance duplicate profiles settle on the
                    // larger vertical separation. This is max lift for
                    // decks and max depth for tunnels, independent of
                    // segment insertion/traversal order.
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => {
                        candidate.0.total_cmp(&best.0).is_gt()
                            || (candidate.0.total_cmp(&best.0).is_eq()
                                && candidate.2.total_cmp(&best.2).is_gt())
                    }
                },
            }
        }

        let mut best_any: Option<(f32, f32, f32)> = None;
        let mut best_compatible: Option<(f32, f32, f32)> = None;
        let cy0 = ((py - cap) / FIELD_CELL).floor() as i32;
        let cy1 = ((py + cap) / FIELD_CELL).floor() as i32;
        let cx0 = ((px - cap) / FIELD_CELL).floor() as i32;
        let cx1 = ((px + cap) / FIELD_CELL).floor() as i32;
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let Some(segs) = self.grid.get(&(cx, cy)) else { continue };
                for seg in segs {
                    let [ax, ay, bx, by, za, zb] = seg.geometry;
                    let (ex, ey) = (bx - ax, by - ay);
                    let el2 = (ex * ex + ey * ey).max(1e-6);
                    if let Some((dx, dy)) = dir {
                        let dl = (dx * dx + dy * dy).sqrt();
                        if dl > 1e-6
                            && ((dx * ex + dy * ey) / (dl * el2.sqrt())).abs() < min_dot
                        {
                            continue;
                        }
                    }
                    let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
                    let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
                    let dist = (qx * qx + qy * qy).sqrt();
                    if dist >= cap {
                        continue;
                    }
                    let side = ex * (py - ay) - ey * (px - ax);
                    let candidate = (za * (1.0 - t) + zb * t, dist, side.signum());
                    if best_any.is_none_or(|best| candidate_is_better(candidate, best)) {
                        best_any = Some(candidate);
                    }
                    if vertical == Some(seg.vertical)
                        && best_compatible
                            .is_none_or(|best| candidate_is_better(candidate, best))
                    {
                        best_compatible = Some(candidate);
                    }
                }
            }
        }
        match (best_compatible, best_any) {
            (Some(compatible), Some(nearest))
                if compatible.1 <= nearest.1 + VERTICAL_CLASS_AMBIGUITY =>
            {
                Some(compatible)
            }
            (_, nearest) => nearest,
        }
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
    vertical: VerticalClass,
    points: Vec<(f32, f32)>,
}

/// Reconcile the two carriageways encoded as sibling paths of one base-map
/// feature. Shortbread commonly emits a dual motorway as reversed parallel
/// paths under one feature id. Sampling them independently can give one side
/// a deck and the other ground, which tears the renderer's union surface down
/// the middle.
///
/// This deliberately has a narrow admission gate: same source feature,
/// positive lift on both paths, full-run reversed endpoint pairing, reciprocal
/// parallel coverage, realistic carriageway spacing, and one already-agreeing
/// lifted anchor. Heights only propagate from a snapshot and only upward, so
/// an unrelated nearby service road (a different feature) cannot be hoisted.
fn reconcile_sibling_carriageways(
    base_paths: &[BasePath],
    path_src: &[((u32, u32), (f32, f32))],
    final_dz: &mut [Option<Vec<f32>>],
    m_per_unit: f32,
) {
    const MIN_SEPARATION_M: f32 = 4.0;
    const MAX_SEPARATION_M: f32 = 24.0;
    const MIN_PARALLEL_DOT: f32 = 0.92;

    fn direction(points: &[(f32, f32)], index: usize) -> (f32, f32) {
        let previous = points[index.saturating_sub(1)];
        let next = points[(index + 1).min(points.len() - 1)];
        let (dx, dy) = (next.0 - previous.0, next.1 - previous.1);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        (dx / len, dy / len)
    }

    fn path_length(points: &[(f32, f32)]) -> f32 {
        points.windows(2).map(|w| distance(w[0], w[1])).sum()
    }

    fn endpoint_distance(
        a: (f32, f32),
        a_offset: (f32, f32),
        b: (f32, f32),
        b_offset: (f32, f32),
    ) -> f32 {
        distance(
            (a.0 + a_offset.0, a.1 + a_offset.1),
            (b.0 + b_offset.0, b.1 + b_offset.1),
        )
    }

    /// For every target vertex, interpolate the nearest locally-parallel
    /// source segment. The returned distance is in meters.
    fn matches(
        target: &BasePath,
        target_offset: (f32, f32),
        source: &BasePath,
        source_offset: (f32, f32),
        source_dz: &[f32],
        m_per_unit: f32,
    ) -> Vec<Option<(f32, f32)>> {
        let mut out = Vec::with_capacity(target.points.len());
        for (index, &(px, py)) in target.points.iter().enumerate() {
            let p = (px + target_offset.0, py + target_offset.1);
            let target_dir = direction(&target.points, index);
            let mut best: Option<(f32, f32)> = None;
            for seg in 0..source.points.len().saturating_sub(1) {
                let a = (
                    source.points[seg].0 + source_offset.0,
                    source.points[seg].1 + source_offset.1,
                );
                let b = (
                    source.points[seg + 1].0 + source_offset.0,
                    source.points[seg + 1].1 + source_offset.1,
                );
                let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                let el2 = (ex * ex + ey * ey).max(1e-6);
                let el = el2.sqrt();
                if ((target_dir.0 * ex + target_dir.1 * ey) / el).abs()
                    < MIN_PARALLEL_DOT
                {
                    continue;
                }
                let t = (((p.0 - a.0) * ex + (p.1 - a.1) * ey) / el2).clamp(0.0, 1.0);
                let q = (a.0 + ex * t, a.1 + ey * t);
                let dist_m = distance(p, q) * m_per_unit;
                if dist_m > MAX_SEPARATION_M
                    || best.is_some_and(|(_, best_dist)| dist_m >= best_dist)
                {
                    continue;
                }
                let height = source_dz[seg] * (1.0 - t) + source_dz[seg + 1] * t;
                best = Some((height, dist_m));
            }
            out.push(best);
        }
        out
    }

    let snapshot = final_dz.to_vec();
    let mut raised = snapshot.clone();
    let mut groups: HashMap<((u32, u32), String, u32), Vec<usize>> = HashMap::new();
    for (index, path) in base_paths.iter().enumerate() {
        if path.is_polygon || path.layer != "streets" || path.points.len() < 2 {
            continue;
        }
        let Some(dz) = snapshot.get(index).and_then(|dz| dz.as_ref()) else {
            continue;
        };
        if dz.len() != path.points.len()
            || dz.iter().any(|&value| value < -0.05)
            || !dz.iter().any(|&value| value > 0.3)
        {
            continue;
        }
        groups
            .entry((path_src[index].0, path.layer.clone(), path.feature))
            .or_default()
            .push(index);
    }

    for siblings in groups.values() {
        for pair_start in 0..siblings.len() {
            for pair_end in pair_start + 1..siblings.len() {
                let (a_index, b_index) = (siblings[pair_start], siblings[pair_end]);
                let (a, b) = (&base_paths[a_index], &base_paths[b_index]);
                let (a_offset, b_offset) = (path_src[a_index].1, path_src[b_index].1);
                let (a_dz, b_dz) = (
                    snapshot[a_index].as_ref().unwrap(),
                    snapshot[b_index].as_ref().unwrap(),
                );
                let a_len = path_length(&a.points);
                let b_len = path_length(&b.points);
                let length_ratio = a_len / b_len.max(1e-6);
                if !(0.75..=1.33).contains(&length_ratio) {
                    continue;
                }
                let reverse_ends = [
                    endpoint_distance(a.points[0], a_offset, *b.points.last().unwrap(), b_offset)
                        * m_per_unit,
                    endpoint_distance(*a.points.last().unwrap(), a_offset, b.points[0], b_offset)
                        * m_per_unit,
                ];
                let direct_sum = (endpoint_distance(a.points[0], a_offset, b.points[0], b_offset)
                    + endpoint_distance(
                        *a.points.last().unwrap(),
                        a_offset,
                        *b.points.last().unwrap(),
                        b_offset,
                    ))
                    * m_per_unit;
                let reverse_sum = reverse_ends[0] + reverse_ends[1];
                if reverse_ends
                    .iter()
                    .any(|&dist| !(MIN_SEPARATION_M..=MAX_SEPARATION_M).contains(&dist))
                    || reverse_sum >= direct_sum * 0.75
                {
                    continue;
                }

                let a_from_b = matches(a, a_offset, b, b_offset, b_dz, m_per_unit);
                let b_from_a = matches(b, b_offset, a, a_offset, a_dz, m_per_unit);
                let covered_a = a_from_b.iter().filter(|sample| sample.is_some()).count();
                let covered_b = b_from_a.iter().filter(|sample| sample.is_some()).count();
                if covered_a * 10 < a.points.len() * 8
                    || covered_b * 10 < b.points.len() * 8
                {
                    continue;
                }
                let mut separations: Vec<f32> = a_from_b
                    .iter()
                    .chain(b_from_a.iter())
                    .filter_map(|sample| sample.map(|(_, dist)| dist))
                    .collect();
                separations.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let median_separation = separations[separations.len() / 2];
                if !(MIN_SEPARATION_M..=MAX_SEPARATION_M).contains(&median_separation) {
                    continue;
                }
                let agreeing_anchor = a_from_b
                    .iter()
                    .zip(a_dz)
                    .chain(b_from_a.iter().zip(b_dz))
                    .any(|(sample, own)| {
                        sample.is_some_and(|(other, _)| {
                            *own > 0.3 && other > 0.3 && (*own - other).abs() <= 1.0
                        })
                    });
                if !agreeing_anchor {
                    continue;
                }

                if let Some(target) = raised[a_index].as_mut() {
                    for (value, sample) in target.iter_mut().zip(a_from_b) {
                        if let Some((other, _)) = sample {
                            *value = value.max(other);
                        }
                    }
                }
                if let Some(target) = raised[b_index].as_mut() {
                    for (value, sample) in target.iter_mut().zip(b_from_a) {
                        if let Some((other, _)) = sample {
                            *value = value.max(other);
                        }
                    }
                }
            }
        }
    }
    final_dz.clone_from_slice(&raised);
}

/// Reconcile exact shared line vertices before the final grade propagation.
/// Near-agreeing interior vertices are ordinary duplicate copies. At a
/// nominal tile edge we deliberately mirror the later encoded-feature
/// boundary policy and take the largest absolute height unconditionally;
/// doing that here lets the subsequent grade and cross-tile plane fits consume
/// the final seam anchor instead of having serialization create a one-vertex
/// cliff afterwards.
fn reconcile_base_vertex_consensus(
    base_paths: &[BasePath],
    path_src: &[((u32, u32), (f32, f32))],
    final_dz: &mut [Option<Vec<f32>>],
) {
    const MAX_CONTINUATION_STEP_M: f32 = 3.0;
    const MAX_ENDPOINT_DOT: f32 = -0.90;
    const MIN_THROUGH_DOT: f32 = 0.90;

    #[derive(Clone, Copy)]
    struct Endpoint {
        path: usize,
        vertex: usize,
        outward: (f32, f32),
    }

    #[derive(Clone, Copy)]
    struct ThroughVertex {
        path: usize,
        vertex: usize,
        outgoing: [(f32, f32); 2],
    }

    let mut consensus = HashMap::<(i32, i32), f32>::new();
    let mut edge_consensus = HashMap::<(i32, i32), f32>::new();
    let mut endpoints = HashMap::<(i32, i32), Vec<Endpoint>>::new();
    let mut through_vertices = HashMap::<(i32, i32), Vec<ThroughVertex>>::new();
    let extent = EXTENT as f32;
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = &final_dz[path_index] else { continue };
        if base_path.is_polygon || dz.len() != base_path.points.len() {
            continue;
        }
        let offset = path_src[path_index].1;
        for (vertex, &value) in dz.iter().enumerate() {
            let (px, py) = base_path.points[vertex];
            let key = (
                ((px + offset.0) * 4.0).round() as i32,
                ((py + offset.1) * 4.0).round() as i32,
            );
            let entry = consensus.entry(key).or_insert(value);
            if value.abs() > entry.abs() {
                *entry = value;
            }
            if px <= 2.0 || px >= extent - 2.0 || py <= 2.0 || py >= extent - 2.0 {
                let entry = edge_consensus.entry(key).or_insert(value);
                if value.abs() > entry.abs() {
                    *entry = value;
                }
            }
        }
        if base_path.points.len() >= 2 {
            let last = base_path.points.len() - 1;
            for (vertex, inner_vertex) in [(0, 1), (last, last - 1)] {
                let point = base_path.points[vertex];
                let inner = base_path.points[inner_vertex];
                let (dx, dy) = (point.0 - inner.0, point.1 - inner.1);
                let len = (dx * dx + dy * dy).sqrt();
                if len <= 1e-6 {
                    continue;
                }
                let key = (
                    ((point.0 + offset.0) * 4.0).round() as i32,
                    ((point.1 + offset.1) * 4.0).round() as i32,
                );
                endpoints.entry(key).or_default().push(Endpoint {
                    path: path_index,
                    vertex,
                    outward: (dx / len, dy / len),
                });
            }
            for vertex in 1..last {
                let point = base_path.points[vertex];
                let direction = |neighbor: (f32, f32)| {
                    let (dx, dy) = (neighbor.0 - point.0, neighbor.1 - point.1);
                    let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                    (dx / len, dy / len)
                };
                let key = (
                    ((point.0 + offset.0) * 4.0).round() as i32,
                    ((point.1 + offset.1) * 4.0).round() as i32,
                );
                through_vertices.entry(key).or_default().push(ThroughVertex {
                    path: path_index,
                    vertex,
                    outgoing: [
                        direction(base_path.points[vertex - 1]),
                        direction(base_path.points[vertex + 1]),
                    ],
                });
            }
        }
    }

    // A short source way can be raised from its far endpoint by the first
    // hold+grade pass after the earlier junction consensus. Reconcile only
    // true, already-lifted continuations here: exact shared node, same
    // source layer, compatible tangents, and a modest height step. Besides
    // endpoint pairs, an endpoint may continue through an interior gore
    // vertex. This joins semantic splits such as steps/footway bridge
    // members and reversible lanes while leaving perpendicular branches
    // and chance stacked crossings alone.
    let mut endpoint_targets = HashMap::<(usize, usize), f32>::new();
    for entries in endpoints.values() {
        for (index, a) in entries.iter().enumerate() {
            for b in entries.iter().skip(index + 1) {
                if a.path == b.path
                    || base_paths[a.path].layer != base_paths[b.path].layer
                    || a.outward.0 * b.outward.0 + a.outward.1 * b.outward.1
                        > MAX_ENDPOINT_DOT
                {
                    continue;
                }
                let Some(a_dz) = final_dz[a.path].as_ref() else { continue };
                let Some(b_dz) = final_dz[b.path].as_ref() else { continue };
                let (a_value, b_value) = (a_dz[a.vertex], b_dz[b.vertex]);
                if a_value <= 0.2
                    || b_value <= 0.2
                    || (a_value - b_value).abs() > MAX_CONTINUATION_STEP_M
                {
                    continue;
                }
                let target = a_value.max(b_value);
                for endpoint in [a, b] {
                    endpoint_targets
                        .entry((endpoint.path, endpoint.vertex))
                        .and_modify(|value| *value = value.max(target))
                        .or_insert(target);
                }
            }
        }
    }
    for (key, endpoint_entries) in &endpoints {
        let Some(through_entries) = through_vertices.get(key) else {
            continue;
        };
        for endpoint in endpoint_entries {
            for through in through_entries {
                if endpoint.path == through.path
                    || base_paths[endpoint.path].layer != base_paths[through.path].layer
                    || !through.outgoing.iter().any(|direction| {
                        endpoint.outward.0 * direction.0
                            + endpoint.outward.1 * direction.1
                            >= MIN_THROUGH_DOT
                    })
                {
                    continue;
                }
                let Some(endpoint_dz) = final_dz[endpoint.path].as_ref() else {
                    continue;
                };
                let Some(through_dz) = final_dz[through.path].as_ref() else {
                    continue;
                };
                let (endpoint_value, through_value) =
                    (endpoint_dz[endpoint.vertex], through_dz[through.vertex]);
                if endpoint_value <= 0.2
                    || through_value <= 0.2
                    || (endpoint_value - through_value).abs() > MAX_CONTINUATION_STEP_M
                {
                    continue;
                }
                let target = endpoint_value.max(through_value);
                for target_vertex in [
                    (endpoint.path, endpoint.vertex),
                    (through.path, through.vertex),
                ] {
                    endpoint_targets
                        .entry(target_vertex)
                        .and_modify(|value| *value = value.max(target))
                        .or_insert(target);
                }
            }
        }
    }

    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = final_dz[path_index].as_mut() else { continue };
        if base_path.is_polygon || dz.len() != base_path.points.len() {
            continue;
        }
        let offset = path_src[path_index].1;
        for (vertex, value) in dz.iter_mut().enumerate() {
            let (px, py) = base_path.points[vertex];
            let key = (
                ((px + offset.0) * 4.0).round() as i32,
                ((py + offset.1) * 4.0).round() as i32,
            );
            let Some(&target) = consensus.get(&key) else {
                continue;
            };
            let at_nominal_edge =
                px <= 2.0 || px >= extent - 2.0 || py <= 2.0 || py >= extent - 2.0;
            if (target - *value).abs() < 1.0 {
                *value = target;
            }
            if at_nominal_edge {
                if let Some(&target) = edge_consensus.get(&key) {
                    *value = target;
                }
            }
            if let Some(&target) = endpoint_targets.get(&(path_index, vertex)) {
                *value = value.max(target);
            }
        }
    }
}

/// Give the two padded copies of a centerline one common plane where they
/// cross a nominal tile seam. MVT clips extend beyond the tile (+64/-64 in
/// the usual 4096 extent), so neighboring copies overlap without sharing a
/// raw vertex. Matching only equal vertices leaves two independently baked
/// ramp planes on top of each other.
///
/// For a reciprocal pair of nearly coincident seam crossings, the vertices
/// INSIDE the two nominal tiles are immutable anchors. Their combined grade
/// defines one seam height and slope; only the two OUTSIDE padding vertices
/// are changed. The overlapping segments then agree in both height and
/// grade without introducing a kink in either tile's authoritative interior.
fn reconcile_cross_tile_copies(
    base_paths: &[BasePath],
    path_src: &[((u32, u32), (f32, f32))],
    final_dz: &mut [Option<Vec<f32>>],
    m_per_unit: f32,
) {
    const MAX_CROSSING_GAP_M: f32 = 1.25;
    const MIN_PARALLEL_DOT: f32 = 0.98;
    const MAX_GRADE: f32 = 0.20;

    #[derive(Clone, Copy, Hash, PartialEq, Eq)]
    enum Seam {
        /// x coordinate is the east tile index; y is the common row.
        Vertical(u32, u32),
        /// y coordinate is the south tile index; x is the common column.
        Horizontal(u32, u32),
    }

    #[derive(Clone, Copy)]
    struct Crossing {
        path: usize,
        /// True for west/north (the negative coordinate side).
        negative_side: bool,
        point: (f32, f32),
        dir: (f32, f32),
        inside_vertex: usize,
        outside_vertex: usize,
        inside_point: (f32, f32),
        outside_point: (f32, f32),
    }

    fn push_crossing(
        seams: &mut HashMap<Seam, Vec<Crossing>>,
        seam: Seam,
        path: usize,
        seg: usize,
        negative_side: bool,
        a: (f32, f32),
        b: (f32, f32),
        axis_a: f32,
        axis_b: f32,
        boundary: f32,
    ) {
        let da = axis_a - boundary;
        let db = axis_b - boundary;
        // We only need true padding overlaps. A vertex exactly on the seam
        // is already handled by ordinary exact-vertex consensus.
        if da * db >= 0.0 {
            return;
        }
        let t = (-da / (db - da)).clamp(0.0, 1.0);
        let point = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let a_inside = if negative_side { da < 0.0 } else { da > 0.0 };
        seams.entry(seam).or_default().push(Crossing {
            path,
            negative_side,
            point,
            dir: (dx / len, dy / len),
            inside_vertex: if a_inside { seg } else { seg + 1 },
            outside_vertex: if a_inside { seg + 1 } else { seg },
            inside_point: if a_inside { a } else { b },
            outside_point: if a_inside { b } else { a },
        });
    }

    let mut seams: HashMap<Seam, Vec<Crossing>> = HashMap::new();
    for (path_index, path) in base_paths.iter().enumerate() {
        if path.is_polygon
            || path.points.len() < 2
            || final_dz
                .get(path_index)
                .and_then(|values| values.as_ref())
                .is_none_or(|values| values.len() != path.points.len())
        {
            continue;
        }
        let (src, offset) = path_src[path_index];
        for seg in 0..path.points.len() - 1 {
            let local_a = path.points[seg];
            let local_b = path.points[seg + 1];
            let a = (local_a.0 + offset.0, local_a.1 + offset.1);
            let b = (local_b.0 + offset.0, local_b.1 + offset.1);
            push_crossing(
                &mut seams,
                Seam::Vertical(src.0, src.1),
                path_index,
                seg,
                false,
                a,
                b,
                local_a.0,
                local_b.0,
                0.0,
            );
            push_crossing(
                &mut seams,
                Seam::Vertical(src.0 + 1, src.1),
                path_index,
                seg,
                true,
                a,
                b,
                local_a.0,
                local_b.0,
                EXTENT as f32,
            );
            push_crossing(
                &mut seams,
                Seam::Horizontal(src.0, src.1),
                path_index,
                seg,
                false,
                a,
                b,
                local_a.1,
                local_b.1,
                0.0,
            );
            push_crossing(
                &mut seams,
                Seam::Horizontal(src.0, src.1 + 1),
                path_index,
                seg,
                true,
                a,
                b,
                local_a.1,
                local_b.1,
                EXTENT as f32,
            );
        }
    }

    let max_gap = MAX_CROSSING_GAP_M / m_per_unit.max(1e-6);
    let mut proposals: HashMap<(usize, usize), Vec<f32>> = HashMap::new();
    for crossings in seams.values() {
        let nearest = |index: usize, opposite_negative_side: bool| -> Option<usize> {
            let from = crossings[index];
            crossings
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    candidate.negative_side == opposite_negative_side
                        && base_paths[candidate.path].layer == base_paths[from.path].layer
                        && (from.dir.0 * candidate.dir.0 + from.dir.1 * candidate.dir.1).abs()
                            >= MIN_PARALLEL_DOT
                })
                .filter_map(|(candidate_index, candidate)| {
                    let gap = distance(from.point, candidate.point);
                    (gap <= max_gap).then_some((candidate_index, gap))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .map(|(candidate_index, _)| candidate_index)
        };
        for negative_index in 0..crossings.len() {
            let negative = crossings[negative_index];
            if !negative.negative_side {
                continue;
            }
            let Some(positive_index) = nearest(negative_index, false) else {
                continue;
            };
            if nearest(positive_index, true) != Some(negative_index) {
                continue;
            }
            let positive = crossings[positive_index];
            let negative_dz = final_dz[negative.path].as_ref().unwrap();
            let positive_dz = final_dz[positive.path].as_ref().unwrap();
            let z_negative = negative_dz[negative.inside_vertex];
            let z_positive = positive_dz[positive.inside_vertex];
            if z_negative.abs() <= 0.2 && z_positive.abs() <= 0.2 {
                continue;
            }
            let negative_to_seam = distance(negative.inside_point, negative.point);
            let seam_to_positive = distance(positive.point, positive.inside_point);
            let anchor_distance = negative_to_seam + seam_to_positive;
            if anchor_distance <= 1e-3 {
                continue;
            }
            let grade = (z_positive - z_negative) / anchor_distance;
            if grade.abs() / m_per_unit.max(1e-6) > MAX_GRADE {
                continue;
            }
            let seam_height = z_negative + grade * negative_to_seam;
            let negative_outside =
                seam_height + grade * distance(negative.point, negative.outside_point);
            let positive_outside =
                seam_height - grade * distance(positive.outside_point, positive.point);
            if negative_outside.abs() <= MAX_LIFT_M
                && positive_outside.abs() <= MAX_LIFT_M
            {
                proposals
                    .entry((negative.path, negative.outside_vertex))
                    .or_default()
                    .push(negative_outside);
                proposals
                    .entry((positive.path, positive.outside_vertex))
                    .or_default()
                    .push(positive_outside);
            }
        }
    }
    for ((path, vertex), values) in proposals {
        let min = values.iter().copied().fold(f32::MAX, f32::min);
        let max = values.iter().copied().fold(f32::MIN, f32::max);
        // A corner vertex can receive both a horizontal and vertical seam
        // proposal. Apply it only when both planes agree.
        if max - min <= 0.2 {
            if let Some(target) = final_dz[path].as_mut() {
                target[vertex] = values.iter().sum::<f32>() / values.len() as f32;
            }
        }
    }

    // Some MVT encoders quantize a seam intersection one extent unit
    // INSIDE both tiles. The authoritative tile then ends at x=4095 while
    // its neighbor carries a padding-only reverse copy ending at x=-1.
    // Neither segment crosses x=4096/0, so the crossing logic above never
    // sees them even though their terminal points and centerlines coincide.
    //
    // Pair those terminal copies conservatively and fit the padding segment
    // to the authoritative in-tile segment's height plane. The authoritative
    // profile remains untouched. Both padding vertices are fitted because
    // the later encoded-feature boundary consensus would otherwise raise
    // only the shared tip and recreate the triangular wedge.
    const SEAM_QUANTIZATION_UNITS: f32 = 2.0;
    const MAX_TERMINAL_GAP_UNITS: f32 = 2.25;

    #[derive(Clone, Copy)]
    struct TerminalSegment {
        path: usize,
        /// True when this source tile lies west/north of the seam.
        source_negative_side: bool,
        /// The segment lies inside its source tile rather than in padding.
        authoritative: bool,
        tip_vertex: usize,
        neighbor_vertex: usize,
        tip: (f32, f32),
        neighbor: (f32, f32),
        dir: (f32, f32),
    }

    fn push_terminal(
        seams: &mut HashMap<Seam, Vec<TerminalSegment>>,
        seam: Seam,
        path: usize,
        source_negative_side: bool,
        tip_vertex: usize,
        neighbor_vertex: usize,
        tip: (f32, f32),
        neighbor: (f32, f32),
        tip_axis: f32,
        neighbor_axis: f32,
        boundary: f32,
    ) {
        let tip_delta = tip_axis - boundary;
        let neighbor_delta = neighbor_axis - boundary;
        // This pass is only for the same-side quantization case. Genuine
        // crossings were handled above, and the endpoint must be the point
        // closest to the nominal seam.
        if tip_delta.abs() > SEAM_QUANTIZATION_UNITS
            || tip_delta * neighbor_delta < 0.0
            || neighbor_delta.abs() <= tip_delta.abs() + 0.5
        {
            return;
        }
        let neighbor_negative_side = neighbor_delta < 0.0;
        let authoritative = neighbor_negative_side == source_negative_side;
        let (dx, dy) = (neighbor.0 - tip.0, neighbor.1 - tip.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 1e-3 {
            return;
        }
        seams.entry(seam).or_default().push(TerminalSegment {
            path,
            source_negative_side,
            authoritative,
            tip_vertex,
            neighbor_vertex,
            tip,
            neighbor,
            dir: (dx / len, dy / len),
        });
    }

    let mut terminal_seams = HashMap::<Seam, Vec<TerminalSegment>>::new();
    for (path_index, path) in base_paths.iter().enumerate() {
        if path.is_polygon
            || path.points.len() < 2
            || final_dz
                .get(path_index)
                .and_then(|values| values.as_ref())
                .is_none_or(|values| values.len() != path.points.len())
        {
            continue;
        }
        let (src, offset) = path_src[path_index];
        let last = path.points.len() - 1;
        for (tip_vertex, neighbor_vertex) in [(0, 1), (last, last - 1)] {
            let local_tip = path.points[tip_vertex];
            let local_neighbor = path.points[neighbor_vertex];
            let tip = (local_tip.0 + offset.0, local_tip.1 + offset.1);
            let neighbor = (
                local_neighbor.0 + offset.0,
                local_neighbor.1 + offset.1,
            );
            push_terminal(
                &mut terminal_seams,
                Seam::Vertical(src.0, src.1),
                path_index,
                false,
                tip_vertex,
                neighbor_vertex,
                tip,
                neighbor,
                local_tip.0,
                local_neighbor.0,
                0.0,
            );
            push_terminal(
                &mut terminal_seams,
                Seam::Vertical(src.0 + 1, src.1),
                path_index,
                true,
                tip_vertex,
                neighbor_vertex,
                tip,
                neighbor,
                local_tip.0,
                local_neighbor.0,
                EXTENT as f32,
            );
            push_terminal(
                &mut terminal_seams,
                Seam::Horizontal(src.0, src.1),
                path_index,
                false,
                tip_vertex,
                neighbor_vertex,
                tip,
                neighbor,
                local_tip.1,
                local_neighbor.1,
                0.0,
            );
            push_terminal(
                &mut terminal_seams,
                Seam::Horizontal(src.0, src.1 + 1),
                path_index,
                true,
                tip_vertex,
                neighbor_vertex,
                tip,
                neighbor,
                local_tip.1,
                local_neighbor.1,
                EXTENT as f32,
            );
        }
    }

    let mut terminal_proposals = HashMap::<(usize, usize), Vec<f32>>::new();
    for terminals in terminal_seams.values() {
        let match_pair = |authoritative: TerminalSegment,
                          padding: TerminalSegment|
         -> Option<(f32, f32)> {
            if !authoritative.authoritative
                || padding.authoritative
                || authoritative.path == padding.path
                || authoritative.source_negative_side == padding.source_negative_side
                || base_paths[authoritative.path].layer != base_paths[padding.path].layer
            {
                return None;
            }
            let tip_gap = distance(authoritative.tip, padding.tip);
            if tip_gap > MAX_TERMINAL_GAP_UNITS
                || (authoritative.dir.0 * padding.dir.0
                    + authoritative.dir.1 * padding.dir.1)
                    .abs()
                    < MIN_PARALLEL_DOT
            {
                return None;
            }
            let authoritative_dz = final_dz[authoritative.path].as_ref().unwrap();
            let tip_z = authoritative_dz[authoritative.tip_vertex];
            let (vx, vy) = (
                authoritative.neighbor.0 - authoritative.tip.0,
                authoritative.neighbor.1 - authoritative.tip.1,
            );
            let length_sq = (vx * vx + vy * vy).max(1e-6);
            let t = ((padding.neighbor.0 - authoritative.tip.0) * vx
                + (padding.neighbor.1 - authoritative.tip.1) * vy)
                / length_sq;
            // The padding segment must actually overlap the authoritative
            // terminal segment, not merely be a nearby parallel road.
            if !(0.0..=1.02).contains(&t) {
                return None;
            }
            let projected = (
                authoritative.tip.0 + vx * t,
                authoritative.tip.1 + vy * t,
            );
            let lateral_gap = distance(projected, padding.neighbor);
            if lateral_gap > max_gap {
                return None;
            }
            let neighbor_z = authoritative_dz[authoritative.neighbor_vertex];
            let authoritative_length = length_sq.sqrt();
            let grade = (neighbor_z - tip_z) / authoritative_length;
            if grade.abs() / m_per_unit.max(1e-6) > MAX_GRADE {
                return None;
            }
            let target = tip_z + (neighbor_z - tip_z) * t;
            if target.abs() > MAX_LIFT_M {
                return None;
            }
            Some((tip_gap + lateral_gap, target))
        };

        let nearest_padding = |authoritative_index: usize| -> Option<(usize, f32, f32)> {
            terminals
                .iter()
                .enumerate()
                .filter_map(|(padding_index, &padding)| {
                    match_pair(terminals[authoritative_index], padding)
                        .map(|(score, target)| (padding_index, score, target))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        };
        let nearest_authoritative = |padding_index: usize| -> Option<(usize, f32)> {
            terminals
                .iter()
                .enumerate()
                .filter_map(|(authoritative_index, &authoritative)| {
                    match_pair(authoritative, terminals[padding_index])
                        .map(|(score, _)| (authoritative_index, score))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        };

        for authoritative_index in 0..terminals.len() {
            if !terminals[authoritative_index].authoritative {
                continue;
            }
            let Some((padding_index, _, target)) = nearest_padding(authoritative_index) else {
                continue;
            };
            if nearest_authoritative(padding_index).map(|pair| pair.0)
                != Some(authoritative_index)
            {
                continue;
            }
            let padding = terminals[padding_index];
            let authoritative = terminals[authoritative_index];
            let tip_target =
                final_dz[authoritative.path].as_ref().unwrap()[authoritative.tip_vertex];
            terminal_proposals
                .entry((padding.path, padding.tip_vertex))
                .or_default()
                .push(tip_target);
            terminal_proposals
                .entry((padding.path, padding.neighbor_vertex))
                .or_default()
                .push(target);
        }
    }
    for ((path, vertex), values) in terminal_proposals {
        let min = values.iter().copied().fold(f32::MAX, f32::min);
        let max = values.iter().copied().fold(f32::MIN, f32::max);
        if max - min <= 0.2 {
            let value = values.iter().sum::<f32>() / values.len() as f32;
            if let Some(target) = final_dz[path].as_mut() {
                target[vertex] = value;
            }
        }
    }

    reconcile_exact_continuation_endpoints(base_paths, path_src, final_dz, m_per_unit);
    // Grade propagation above can change several owner-path samples. Copy
    // that final profile into non-exact buffered duplicates last so the
    // terminal repair cannot re-diverge adjacent tiles.
    reconcile_wholly_padded_duplicates(base_paths, path_src, final_dz);
}

/// Cross-tile fitting is terminal profile surgery, so it can move an owner
/// endpoint after the earlier shared-vertex consensus has run. Reassert only
/// true exact continuations afterwards: two already-lifted line endpoints in
/// the same rendered layer, at the same global node, with opposite tangents
/// and a modest height disagreement. This also gives wholly-padding and owner
/// copies the same final bridge-transition node without lifting branches.
fn reconcile_exact_continuation_endpoints(
    base_paths: &[BasePath],
    path_src: &[((u32, u32), (f32, f32))],
    final_dz: &mut [Option<Vec<f32>>],
    m_per_unit: f32,
) {
    const MAX_CONTINUATION_STEP_M: f32 = 3.0;
    const MAX_ENDPOINT_DOT: f32 = -0.90;
    const MAX_GRADE: f32 = 0.08;
    const ENDPOINT_HOLD_M: f32 = 12.0;

    #[derive(Clone, Copy)]
    struct Endpoint {
        path: usize,
        vertex: usize,
        outward: (f32, f32),
    }

    let snapshot = final_dz.to_vec();
    let mut endpoints = HashMap::<(i32, i32), Vec<Endpoint>>::new();
    for (path_index, path) in base_paths.iter().enumerate() {
        let Some(dz) = snapshot.get(path_index).and_then(|values| values.as_ref()) else {
            continue;
        };
        if path.is_polygon || path.points.len() < 2 || dz.len() != path.points.len() {
            continue;
        }
        let offset = path_src[path_index].1;
        let last = path.points.len() - 1;
        for (vertex, inner_vertex) in [(0, 1), (last, last - 1)] {
            let point = path.points[vertex];
            let inner = path.points[inner_vertex];
            let (dx, dy) = (point.0 - inner.0, point.1 - inner.1);
            let length = (dx * dx + dy * dy).sqrt();
            if length <= 1e-6 {
                continue;
            }
            let key = (
                ((point.0 + offset.0) * 4.0).round() as i32,
                ((point.1 + offset.1) * 4.0).round() as i32,
            );
            endpoints.entry(key).or_default().push(Endpoint {
                path: path_index,
                vertex,
                outward: (dx / length, dy / length),
            });
        }
    }

    let mut targets = HashMap::<(usize, usize), f32>::new();
    for entries in endpoints.values() {
        for (index, a) in entries.iter().enumerate() {
            for b in entries.iter().skip(index + 1) {
                if a.path == b.path
                    || base_paths[a.path].layer != base_paths[b.path].layer
                    || a.outward.0 * b.outward.0 + a.outward.1 * b.outward.1
                        > MAX_ENDPOINT_DOT
                {
                    continue;
                }
                let a_value = snapshot[a.path].as_ref().unwrap()[a.vertex];
                let b_value = snapshot[b.path].as_ref().unwrap()[b.vertex];
                if a_value <= 0.2
                    || b_value <= 0.2
                    || (a_value - b_value).abs() > MAX_CONTINUATION_STEP_M
                {
                    continue;
                }
                let target = a_value.max(b_value);
                for endpoint in [a, b] {
                    targets
                        .entry((endpoint.path, endpoint.vertex))
                        .and_modify(|value| *value = value.max(target))
                        .or_insert(target);
                }
            }
        }
    }

    for ((path, vertex), target) in targets {
        if let Some(values) = final_dz[path].as_mut() {
            values[vertex] = values[vertex].max(target);
            let points = &base_paths[path].points;
            let hold_units = ENDPOINT_HOLD_M / m_per_unit.max(1e-6);
            let grade_per_unit = MAX_GRADE * m_per_unit;
            let mut arc = 0.0f32;
            if vertex == 0 {
                for index in 1..points.len() {
                    arc += distance(points[index - 1], points[index]);
                    let required =
                        target - grade_per_unit * (arc - hold_units).max(0.0);
                    if required > values[index] {
                        values[index] = required;
                    }
                }
            } else {
                for index in (0..vertex).rev() {
                    arc += distance(points[index], points[index + 1]);
                    let required =
                        target - grade_per_unit * (arc - hold_units).max(0.0);
                    if required > values[index] {
                        values[index] = required;
                    }
                }
            }
        }
    }
}

/// Replace an independently sampled path which lives wholly in one tile's
/// clipping padding with the profile of its authoritative copy in the
/// adjacent tile.
///
/// The ordinary seam pass above handles segments which cross the nominal
/// edge. Shortbread can also emit a complete, short way in both tiles: all
/// of one copy's vertices lie beyond 4096 while the adjacent copy lies
/// inside 0..4096. There is then no seam crossing or near-edge terminal to
/// pair, even though both ribbons are drawn from the buffered tiles.
///
/// This pass is intentionally stricter than ordinary geometry matching:
/// the source must be wholly outside on exactly one side, every source
/// vertex must match a locally parallel segment in the one owning neighbor,
/// the layer and vertical class must agree, and the matched point must lie
/// inside that neighbor's nominal tile. All proposals read from a snapshot,
/// and ambiguous equal-distance profiles are accepted only when they agree,
/// so input enumeration cannot choose the winning height.
fn reconcile_wholly_padded_duplicates(
    base_paths: &[BasePath],
    path_src: &[((u32, u32), (f32, f32))],
    final_dz: &mut [Option<Vec<f32>>],
) {
    const MAX_LATERAL_GAP: f32 = 0.75;
    const MAX_ENDPOINT_GAP: f32 = 4.0;
    const MIN_PARALLEL_DOT: f32 = 0.98;
    const SCORE_TIE_EPSILON: f32 = 1e-4;
    const MAX_TIED_HEIGHT_GAP_M: f32 = 0.2;

    #[derive(Clone, Copy)]
    enum PaddingSide {
        West,
        East,
        North,
        South,
    }

    fn padding_side(path: &BasePath) -> Option<PaddingSide> {
        let extent = EXTENT as f32;
        let west = path.points.iter().all(|point| point.0 < 0.0);
        let east = path.points.iter().all(|point| point.0 > extent);
        let north = path.points.iter().all(|point| point.1 < 0.0);
        let south = path.points.iter().all(|point| point.1 > extent);
        match (west, east, north, south) {
            (true, false, false, false) => Some(PaddingSide::West),
            (false, true, false, false) => Some(PaddingSide::East),
            (false, false, true, false) => Some(PaddingSide::North),
            (false, false, false, true) => Some(PaddingSide::South),
            _ => None,
        }
    }

    fn owner_tile(src: (u32, u32), side: PaddingSide) -> Option<(u32, u32)> {
        match side {
            PaddingSide::West => src.0.checked_sub(1).map(|x| (x, src.1)),
            PaddingSide::East => src.0.checked_add(1).map(|x| (x, src.1)),
            PaddingSide::North => src.1.checked_sub(1).map(|y| (src.0, y)),
            PaddingSide::South => src.1.checked_add(1).map(|y| (src.0, y)),
        }
    }

    fn vertex_direction(points: &[(f32, f32)], vertex: usize) -> Option<(f32, f32)> {
        if points.len() < 2 {
            return None;
        }
        let previous = points[vertex.saturating_sub(1)];
        let next = points[(vertex + 1).min(points.len() - 1)];
        let (dx, dy) = (next.0 - previous.0, next.1 - previous.1);
        let len = (dx * dx + dy * dy).sqrt();
        (len > 1e-5).then_some((dx / len, dy / len))
    }

    let snapshot = final_dz.to_vec();
    let extent = EXTENT as f32;
    let max_gap_sq = MAX_LATERAL_GAP * MAX_LATERAL_GAP;
    let mut replacements = Vec::<(usize, Vec<f32>)>::new();
    let mut paths_by_source = HashMap::<(u32, u32), Vec<usize>>::new();
    for (path_index, &(source, _)) in path_src.iter().enumerate() {
        paths_by_source.entry(source).or_default().push(path_index);
    }

    for (padding_index, padding) in base_paths.iter().enumerate() {
        let Some(side) = padding_side(padding) else {
            continue;
        };
        let Some(padding_dz) = snapshot
            .get(padding_index)
            .and_then(|values| values.as_ref())
            .filter(|values| values.len() == padding.points.len())
        else {
            continue;
        };
        if padding.is_polygon || padding.points.len() < 2 {
            continue;
        }
        let Some(owner) = owner_tile(path_src[padding_index].0, side) else {
            continue;
        };
        let padding_offset = path_src[padding_index].1;
        let mut candidates = Vec::<(f32, Vec<f32>)>::new();

        let Some(authority_indices) = paths_by_source.get(&owner) else {
            continue;
        };
        for &authority_index in authority_indices {
            let authority = &base_paths[authority_index];
            if authority_index == padding_index
                || authority.is_polygon
                || authority.points.len() < 2
                || path_src[authority_index].0 != owner
                || authority.layer != padding.layer
                || authority.vertical != padding.vertical
                || !authority.points.iter().all(|point| {
                    point.0 >= 0.0
                        && point.0 <= extent
                        && point.1 >= 0.0
                        && point.1 <= extent
                })
            {
                continue;
            }
            let Some(authority_dz) = snapshot
                .get(authority_index)
                .and_then(|values| values.as_ref())
                .filter(|values| values.len() == authority.points.len())
            else {
                continue;
            };
            let authority_offset = path_src[authority_index].1;
            let padding_endpoints = [padding.points[0], *padding.points.last().unwrap()]
                .map(|point| {
                    (
                        point.0 + padding_offset.0,
                        point.1 + padding_offset.1,
                    )
                });
            let authority_endpoints = [
                authority.points[0],
                *authority.points.last().unwrap(),
            ]
            .map(|point| {
                (
                    point.0 + authority_offset.0,
                    point.1 + authority_offset.1,
                )
            });
            let endpoint_gap = |reversed: bool| {
                let target = if reversed {
                    [authority_endpoints[1], authority_endpoints[0]]
                } else {
                    authority_endpoints
                };
                distance(padding_endpoints[0], target[0])
                    .max(distance(padding_endpoints[1], target[1]))
            };
            if endpoint_gap(false).min(endpoint_gap(true)) > MAX_ENDPOINT_GAP {
                continue;
            }
            let mut score = 0.0f32;
            let mut profile = Vec::with_capacity(padding.points.len());
            let mut matches_all = true;

            for (vertex, &local_point) in padding.points.iter().enumerate() {
                let Some(direction) = vertex_direction(&padding.points, vertex) else {
                    matches_all = false;
                    break;
                };
                let point = (
                    local_point.0 + padding_offset.0,
                    local_point.1 + padding_offset.1,
                );
                let mut best_vertex_match: Option<(f32, f32)> = None;
                for segment in 0..authority.points.len() - 1 {
                    let local_a = authority.points[segment];
                    let local_b = authority.points[segment + 1];
                    let a = (
                        local_a.0 + authority_offset.0,
                        local_a.1 + authority_offset.1,
                    );
                    let b = (
                        local_b.0 + authority_offset.0,
                        local_b.1 + authority_offset.1,
                    );
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let length_sq = dx * dx + dy * dy;
                    if length_sq <= 1e-8 {
                        continue;
                    }
                    let length = length_sq.sqrt();
                    let tangent_dot =
                        ((direction.0 * dx + direction.1 * dy) / length).abs();
                    if tangent_dot < MIN_PARALLEL_DOT {
                        continue;
                    }
                    let t = (((point.0 - a.0) * dx + (point.1 - a.1) * dy)
                        / length_sq)
                        .clamp(0.0, 1.0);
                    let local_match = (
                        local_a.0 + (local_b.0 - local_a.0) * t,
                        local_a.1 + (local_b.1 - local_a.1) * t,
                    );
                    if local_match.0 < 0.0
                        || local_match.0 > extent
                        || local_match.1 < 0.0
                        || local_match.1 > extent
                    {
                        continue;
                    }
                    let match_point = (a.0 + dx * t, a.1 + dy * t);
                    let gap_x = match_point.0 - point.0;
                    let gap_y = match_point.1 - point.1;
                    let gap_sq = gap_x * gap_x + gap_y * gap_y;
                    if gap_sq > max_gap_sq {
                        continue;
                    }
                    let target =
                        authority_dz[segment] * (1.0 - t) + authority_dz[segment + 1] * t;
                    let candidate_score = gap_sq + (1.0 - tangent_dot) * 0.25;
                    if best_vertex_match.is_none_or(|(best, _)| candidate_score < best) {
                        best_vertex_match = Some((candidate_score, target));
                    }
                }
                let Some((vertex_score, target)) = best_vertex_match else {
                    matches_all = false;
                    break;
                };
                score += vertex_score;
                profile.push(target);
            }
            if !matches_all || profile.len() != padding_dz.len() {
                continue;
            }
            score /= profile.len() as f32;
            candidates.push((score, profile));
        }

        let best_score = candidates
            .iter()
            .map(|candidate| candidate.0)
            .fold(f32::MAX, f32::min);
        let tied_profiles: Vec<&Vec<f32>> = candidates
            .iter()
            .filter(|candidate| candidate.0 <= best_score + SCORE_TIE_EPSILON)
            .map(|candidate| &candidate.1)
            .collect();
        if tied_profiles.is_empty() {
            continue;
        }
        let mut replacement = Vec::with_capacity(padding.points.len());
        let mut ambiguous = false;
        for vertex in 0..padding.points.len() {
            let mut values: Vec<f32> = tied_profiles
                .iter()
                .map(|profile| profile[vertex])
                .collect();
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let min = values[0];
            let max = *values.last().unwrap();
            if max - min > MAX_TIED_HEIGHT_GAP_M {
                ambiguous = true;
                break;
            }
            replacement.push(values.iter().sum::<f32>() / values.len() as f32);
        }
        if !ambiguous {
            replacements.push((padding_index, replacement));
        }
    }

    for (path, replacement) in replacements {
        if let Some(target) = final_dz[path].as_mut() {
            *target = replacement;
        }
    }
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
    if !BASE_DZ_LAYERS.contains(&name.as_str()) {
        return Ok(());
    }
    let scale = EXTENT as f32 / extent.max(1) as f32;
    for (feature_index, feature) in features.iter().enumerate() {
        let mut fo = 0;
        let mut packed_tags: &[u8] = &[];
        let mut geometry_type = 0u64;
        let mut geometry: &[u8] = &[];
        while fo < feature.len() {
            let (field, wire) = read_protobuf_key(feature, &mut fo)?;
            match (field, wire) {
                (2, 2) => packed_tags = read_protobuf_bytes(feature, &mut fo)?,
                (3, 0) => geometry_type = read_varint(feature, &mut fo)?,
                (4, 2) => geometry = read_protobuf_bytes(feature, &mut fo)?,
                _ => skip_protobuf_value(feature, &mut fo, wire)?,
            }
        }
        if geometry_type != 2 && geometry_type != 3 {
            continue;
        }
        let mut bridge = name == "bridges";
        let mut tunnel = false;
        let mut osm_layer = None;
        let mut to = 0;
        while to < packed_tags.len() {
            let key_index = read_varint(packed_tags, &mut to)? as usize;
            let value_index = read_varint(packed_tags, &mut to)? as usize;
            let (Some(key), Some(value)) = (keys.get(key_index), values.get(value_index)) else {
                continue;
            };
            match key.as_str() {
                "bridge" => bridge |= value.is_truthy(),
                "tunnel" => tunnel |= value.is_truthy(),
                "osm_layer" => osm_layer = Some(value),
                _ => {}
            }
        }
        let vertical =
            VerticalClass::from_base_feature(&name, bridge, tunnel, osm_layer);
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
                vertical,
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
                field.sample(gx, gy, None, 2.4, Some(base_path.vertical))
            } else {
                let endpoint = index == 0 || index == count - 1;
                let previous = points[index.saturating_sub(1)];
                let next = points[(index + 1).min(count - 1)];
                let dir = (next.0 - previous.0, next.1 - previous.1);
                let gated =
                    field.sample(gx, gy, Some(dir), 1.2, Some(base_path.vertical));
                if endpoint {
                    gated.or_else(|| {
                        field.sample_gated(
                            gx,
                            gy,
                            Some(dir),
                            0.8,
                            0.5,
                            Some(base_path.vertical),
                        )
                    })
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
                let gated =
                    tunnel_field.sample(gx, gy, Some(dir), 1.2, Some(base_path.vertical));
                let sampled = if endpoint {
                    gated.or_else(|| {
                        tunnel_field.sample_gated(
                            gx,
                            gy,
                            Some(dir),
                            0.8,
                            0.5,
                            Some(base_path.vertical),
                        )
                    })
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
        // Isolated 1-2 vertex spikes are grabs from a PARALLEL deck
        // directly overhead (stacked carriageways are coincident in 2D —
        // no direction or distance gate can reject them): real deck
        // membership is CONTIGUOUS. Zero short isolated runs; genuine
        // merges are re-established by the junction consensus afterwards,
        // which blends at grade instead of spiking.
        if !base_path.is_polygon {
            let mut index = 0;
            while index < count {
                if dz[index] > 0.2 {
                    let run_start = index;
                    while index < count && dz[index] > 0.2 {
                        index += 1;
                    }
                    // Whole-way profiles always keep (a short bridge way IS
                    // two lifted vertices); internal/end spikes die only
                    // when their ARC is short — vertex counts lie on
                    // sparse geometry.
                    let whole_path = run_start == 0 && index == count;
                    if !whole_path {
                        let mut arc = 0.0f32;
                        for vertex in run_start.max(1)..index {
                            arc += distance(points[vertex - 1], points[vertex]);
                        }
                        if arc * m_per_unit < 25.0 {
                            for value in dz[run_start..index].iter_mut() {
                                *value = 0.0;
                            }
                        }
                    }
                } else {
                    index += 1;
                }
            }
        }
        // Second chance: gore twins. Split ways duplicate a carriageway
        // with a slightly OFFSET centerline — the twin fails the
        // direction/median gates and would bake lower than its sibling
        // (two overlapping slabs at different heights). A twin lies within
        // a lane of the accepted profile: resample ungated with a tight
        // cap and require a consistent tight match; frontage streets sit
        // farther out and still reject.
        if !base_path.is_polygon && dz.iter().all(|&v| v == 0.0) && count >= 2 {
            let mut twin = vec![0.0f32; count];
            let mut dists: Vec<f32> = Vec::new();
            for index in 0..count {
                let (px, py) = points[index];
                let (gx, gy) = (px + global_offset.0, py + global_offset.1);
                if let Some((height, dist, _)) =
                    field.sample(gx, gy, None, 2.5, Some(base_path.vertical))
                {
                    if height > 0.2 {
                        twin[index] = height;
                        dists.push(dist);
                    }
                }
            }
            if dists.len() * 10 >= count * 6 {
                let mut sorted = dists.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                if sorted[sorted.len() / 2] <= 2.0 {
                    dz = twin;
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
    // Junction consensus entries carry the DIRECTION of the height
    // carrier at that vertex: a joining way may inherit the height
    // without its own support only when it attaches TANGENTIALLY (slip
    // roads and gores) — perpendicular side streets never inherit a
    // curb-lift.
    let mut junction: HashMap<(i32, i32), (f32, (f32, f32))> = HashMap::new();
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = &sampled_paths[path_index] else { continue };
        if base_path.is_polygon {
            continue;
        }
        let offset = path_src[path_index].1;
        let count = base_path.points.len();
        for (vertex, &height) in dz.iter().enumerate() {
            if height <= 0.2 {
                continue;
            }
            let (px, py) = base_path.points[vertex];
            let previous = base_path.points[vertex.saturating_sub(1)];
            let next = base_path.points[(vertex + 1).min(count - 1)];
            let (dx, dy) = (next.0 - previous.0, next.1 - previous.1);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let key = (
                ((px + offset.0) * 4.0).round() as i32,
                ((py + offset.1) * 4.0).round() as i32,
            );
            let entry = junction.entry(key).or_insert((0.0, (0.0, 0.0)));
            if height > entry.0 {
                *entry = (height, (dx / len, dy / len));
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
                if let Some(&(height, junction_dir)) = junction.get(&key) {
                    // Take the consensus where this way already carries
                    // height at or next to the vertex — a grounded side
                    // street touching a lifted junction node must not
                    // inherit a curb-lift.
                    let neighbor = dz[vertex.saturating_sub(1)]
                        .max(dz[(vertex + 1).min(count - 1)]);
                    let supported =
                        dz[vertex].max(neighbor) > (0.3 * height).min(0.5);
                    // Unsupported ENDPOINTS still inherit when the join is
                    // tangential: a slip road leaves the deck corridor
                    // immediately (its own samples are empty) but must
                    // attach AT deck height, not bump below it.
                    let tangential = if !supported
                        && (vertex == 0 || vertex + 1 == count)
                    {
                        let previous = points[vertex.saturating_sub(1)];
                        let next = points[(vertex + 1).min(count - 1)];
                        let (dx, dy) = (next.0 - previous.0, next.1 - previous.1);
                        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
                        let dot = (dx / len) * junction_dir.0
                            + (dy / len) * junction_dir.1;
                        // A slip road carries SOME height of its own a few
                        // vertices in (gore-twin sampling); a grounded way
                        // whose split merely sits under a parallel deck is
                        // all-zero — no inheritance for it.
                        let inward_support = if vertex == 0 {
                            dz.iter().take(4).skip(1).any(|&v| v > 0.2)
                        } else {
                            dz.iter().rev().take(4).skip(1).any(|&v| v > 0.2)
                        };
                        dot.abs() > 0.8 && inward_support
                    } else {
                        false
                    };
                    if (supported || tangential) && height > dz[vertex] {
                        dz[vertex] = height;
                    }
                }
            }
            // Grade-limited blend from both ends so a raised junction
            // endpoint ramps into the path instead of spiking. HOLD the
            // junction height for the first stretch: a slip road must
            // attach AT deck level through the gore and descend after —
            // decaying immediately left a ledge along every merge.
            let hold_units = 12.0 / m_per_unit.max(1e-6);
            let mut arc = 0.0f32;
            for index in 1..count {
                let seg = distance(points[index - 1], points[index]);
                arc += seg;
                let decay = (arc - hold_units).max(0.0) - (arc - seg - hold_units).max(0.0);
                let limit = dz[index - 1] - grade_per_unit * decay;
                if limit > dz[index] {
                    dz[index] = limit;
                }
            }
            arc = 0.0;
            for index in (0..count - 1).rev() {
                let seg = distance(points[index], points[index + 1]);
                arc += seg;
                let decay = (arc - hold_units).max(0.0) - (arc - seg - hold_units).max(0.0);
                let limit = dz[index + 1] - grade_per_unit * decay;
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
    reconcile_sibling_carriageways(&base_paths, &path_src, &mut final_dz, m_per_unit);
    reconcile_base_vertex_consensus(&base_paths, &path_src, &mut final_dz);
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = final_dz[path_index].as_mut() else { continue };
        if !base_path.is_polygon {
            let count = base_path.points.len();
            // Interior sampling holes: a zero run bounded by LIFTED values
            // (base geometry drifting past the 1.2u gate) is a hole, not a
            // descent — a real descent carries graded values. Interpolate
            // by arc length, bounded so a genuine dip between two spans
            // stays a dip.
            if count >= 3 {
                let mut index = 1;
                while index < count {
                    if dz[index] == 0.0 && dz[index - 1] > 0.3 {
                        let run_start = index;
                        let mut run_end = index;
                        while run_end < count && dz[run_end] == 0.0 {
                            run_end += 1;
                        }
                        if run_end < count && dz[run_end] > 0.3 {
                            let mut arc = 0.0f32;
                            let mut arcs = Vec::with_capacity(run_end - run_start + 1);
                            for vertex in run_start..=run_end {
                                arc += distance(
                                    base_path.points[vertex - 1],
                                    base_path.points[vertex],
                                );
                                arcs.push(arc);
                            }
                            let total = arc.max(1e-3);
                            if total * m_per_unit < 120.0 {
                                let from = dz[run_start - 1];
                                let to = dz[run_end];
                                for (slot, vertex) in (run_start..run_end).enumerate() {
                                    let t = arcs[slot] / total;
                                    dz[vertex] = from + (to - from) * t;
                                }
                            }
                        }
                        index = run_end;
                    } else {
                        index += 1;
                    }
                }
            }
            // The consensus above can raise a vertex WITHOUT a ramp behind
            // it (it runs after pass-2 blending): re-run the hold+grade
            // blend so no cliff survives.
            let hold_units = 12.0 / m_per_unit.max(1e-6);
            let mut arc = 0.0f32;
            for index in 1..count {
                let seg = distance(base_path.points[index - 1], base_path.points[index]);
                arc += seg;
                let decay =
                    (arc - hold_units).max(0.0) - (arc - seg - hold_units).max(0.0);
                let limit = dz[index - 1] - grade_per_unit * decay;
                if limit > dz[index] {
                    dz[index] = limit;
                }
            }
            arc = 0.0;
            for index in (0..count - 1).rev() {
                let seg = distance(base_path.points[index], base_path.points[index + 1]);
                arc += seg;
                let decay =
                    (arc - hold_units).max(0.0) - (arc - seg - hold_units).max(0.0);
                let limit = dz[index + 1] - grade_per_unit * decay;
                if limit > dz[index] {
                    dz[index] = limit;
                }
            }
        }
    }
    // This must be terminal profile surgery: the padding endpoints are
    // fitted to one common cross-tile plane after all consensus, dropout
    // filling, and grade propagation have settled the inside anchors.
    reconcile_cross_tile_copies(&base_paths, &path_src, &mut final_dz, m_per_unit);

    let mut per_tile: HashMap<(u32, u32), Vec<TileFeature>> = HashMap::new();
    for (path_index, base_path) in base_paths.iter().enumerate() {
        let Some(dz) = final_dz[path_index].take() else { continue };
        let src = path_src[path_index].0;
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
    fn base_vertical_class_parses_string_and_numeric_osm_layers() {
        for layer in [
            MvtVal::Str("-1".to_string()),
            MvtVal::Num(-2.0),
        ] {
            assert_eq!(
                VerticalClass::from_base_feature(
                    "streets",
                    false,
                    false,
                    Some(&layer)
                ),
                VerticalClass::Tunnel
            );
        }
        for layer in [
            MvtVal::Str("1".to_string()),
            MvtVal::Num(2.0),
        ] {
            assert_eq!(
                VerticalClass::from_base_feature(
                    "streets",
                    false,
                    false,
                    Some(&layer)
                ),
                VerticalClass::Elevated
            );
        }
        for layer in [
            MvtVal::Str("0".to_string()),
            MvtVal::Num(0.0),
        ] {
            assert_eq!(
                VerticalClass::from_base_feature(
                    "streets",
                    false,
                    false,
                    Some(&layer)
                ),
                VerticalClass::Surface
            );
        }
        assert_eq!(
            VerticalClass::from_base_feature("bridges", false, false, None),
            VerticalClass::Elevated
        );
        assert_eq!(
            VerticalClass::from_base_feature(
                "streets",
                true,
                false,
                Some(&MvtVal::Num(-1.0))
            ),
            VerticalClass::Elevated
        );
        assert_eq!(
            VerticalClass::from_base_feature(
                "streets",
                false,
                true,
                Some(&MvtVal::Num(2.0))
            ),
            VerticalClass::Tunnel
        );
    }

    #[test]
    fn semantic_field_sampling_is_insertion_order_independent() {
        for surface_first in [false, true] {
            let mut field = SolvedField::default();
            let push_surface = |field: &mut SolvedField| {
                field.push(
                    -10.0,
                    1.0,
                    10.0,
                    -1.0,
                    0.0,
                    0.0,
                    VerticalClass::Surface,
                );
            };
            let push_bridge = |field: &mut SolvedField| {
                field.push(
                    -10.0,
                    -1.0,
                    10.0,
                    1.0,
                    6.5,
                    6.5,
                    VerticalClass::Elevated,
                );
            };
            if surface_first {
                push_surface(&mut field);
                push_bridge(&mut field);
            } else {
                push_bridge(&mut field);
                push_surface(&mut field);
            }

            let surface = field
                .sample(
                    0.0,
                    0.0,
                    Some((1.0, 0.0)),
                    1.2,
                    Some(VerticalClass::Surface),
                )
                .unwrap();
            let elevated = field
                .sample(
                    0.0,
                    0.0,
                    Some((1.0, 0.0)),
                    1.2,
                    Some(VerticalClass::Elevated),
                )
                .unwrap();
            assert!(surface.0.abs() < 1e-6);
            assert!((elevated.0 - 6.5).abs() < 1e-6);
        }
    }

    #[test]
    fn semantic_field_same_class_equal_distance_is_order_independent() {
        for high_first in [false, true] {
            let mut field = SolvedField::default();
            let push_low = |field: &mut SolvedField| {
                field.push(
                    -10.0,
                    -1.0,
                    10.0,
                    -1.0,
                    4.0,
                    4.0,
                    VerticalClass::Elevated,
                );
            };
            let push_high = |field: &mut SolvedField| {
                field.push(
                    -10.0,
                    1.0,
                    10.0,
                    1.0,
                    7.0,
                    7.0,
                    VerticalClass::Elevated,
                );
            };
            if high_first {
                push_high(&mut field);
                push_low(&mut field);
            } else {
                push_low(&mut field);
                push_high(&mut field);
            }
            let sampled = field
                .sample(
                    0.0,
                    0.0,
                    Some((1.0, 0.0)),
                    1.2,
                    Some(VerticalClass::Elevated),
                )
                .unwrap();
            assert!((sampled.0 - 7.0).abs() < 1e-6);
        }
    }

    #[test]
    fn semantic_field_sampling_keeps_nearest_fallback() {
        let mut field = SolvedField::default();
        field.push(
            -10.0,
            0.0,
            10.0,
            0.0,
            6.5,
            6.5,
            VerticalClass::Elevated,
        );
        field.push(
            -10.0,
            1.0,
            10.0,
            1.0,
            0.0,
            0.0,
            VerticalClass::Surface,
        );

        let sampled = field
            .sample(
                0.0,
                0.0,
                Some((1.0, 0.0)),
                1.2,
                Some(VerticalClass::Surface),
            )
            .unwrap();
        assert!((sampled.0 - 6.5).abs() < 1e-6);
    }

    #[test]
    fn semantic_sampling_survives_gore_twin_resample() {
        let mut field = SolvedField::default();
        field.push(
            100.0,
            100.0,
            200.0,
            100.0,
            6.5,
            6.5,
            VerticalClass::Elevated,
        );
        field.push(
            100.0,
            100.5,
            200.0,
            100.5,
            0.0,
            0.0,
            VerticalClass::Surface,
        );
        let base_path = BasePath {
            layer: "streets".to_string(),
            feature: 38,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: vec![(100.0, 100.0), (150.0, 100.0), (200.0, 100.0)],
        };

        let annotated = annotate_base_tiles(
            vec![((1, 2), (0.0, 0.0), vec![base_path])],
            &field,
            &SolvedField::default(),
            0.36,
        );
        assert!(
            annotated.is_empty(),
            "ordinary path was re-lifted by a later sampling pass"
        );
    }

    #[test]
    fn reconciles_reversed_sibling_carriageways_only() {
        let path = |feature, points: &[(f32, f32)]| BasePath {
            layer: "streets".to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: points.to_vec(),
        };
        let paths = vec![
            path(7, &[(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)]),
            path(7, &[(100.0, 10.0), (50.0, 10.0), (0.0, 10.0)]),
            // A nearby higher road under another feature never participates.
            path(8, &[(100.0, 15.0), (50.0, 15.0), (0.0, 15.0)]),
            // Same feature, but a partial perpendicular branch is rejected.
            path(7, &[(50.0, -25.0), (50.0, 25.0)]),
        ];
        let sources = vec![((1, 2), (0.0, 0.0)); paths.len()];
        let mut dz = vec![
            Some(vec![0.0, 0.0, 5.5]),
            Some(vec![5.5, 0.0, 6.4]),
            Some(vec![9.0, 9.0, 9.0]),
            Some(vec![4.0, 4.0]),
        ];

        reconcile_sibling_carriageways(&paths, &sources, &mut dz, 1.0);

        assert!((dz[0].as_ref().unwrap()[0] - 6.4).abs() < 0.01);
        assert_eq!(dz[2].as_ref().unwrap(), &[9.0, 9.0, 9.0]);
        assert_eq!(dz[3].as_ref().unwrap(), &[4.0, 4.0]);
    }

    #[test]
    fn reconciles_geometric_copies_across_tile_overlap() {
        let path = |feature, points: &[(f32, f32)]| BasePath {
            layer: "streets".to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: points.to_vec(),
        };
        let paths = vec![
            path(1, &[(4012.0, 3484.0), (4160.0, 3431.0)]),
            path(9, &[(-64.0, 3477.0), (331.0, 3334.0)]),
            // A separate parallel centerline is much farther than the
            // duplicate-copy tolerance and must remain independent.
            path(10, &[(-64.0, 3517.0), (331.0, 3374.0)]),
            // A crossing line in the neighbor tile is direction-rejected.
            path(11, &[(-64.0, 3370.0), (64.0, 3498.0)]),
        ];
        let sources = vec![
            ((10, 20), (0.0, 0.0)),
            ((11, 20), (4096.0, 0.0)),
            ((11, 20), (4096.0, 0.0)),
            ((11, 20), (4096.0, 0.0)),
        ];
        let mut dz = vec![
            Some(vec![8.1, 5.2]),
            Some(vec![6.4, 1.2]),
            Some(vec![10.0, 10.0]),
            Some(vec![12.0, 12.0]),
        ];

        reconcile_cross_tile_copies(&paths, &sources, &mut dz, 0.4);

        let west = dz[0].as_ref().unwrap();
        let east = dz[1].as_ref().unwrap();
        assert!((west[0] - 8.1).abs() < 0.001);
        assert!((east[1] - 1.2).abs() < 0.001);
        let sample_at_x = |points: &[(f32, f32)], offset_x: f32, values: &[f32], x: f32| {
            let ax = points[0].0 + offset_x;
            let bx = points[1].0 + offset_x;
            let t = (x - ax) / (bx - ax);
            values[0] + (values[1] - values[0]) * t
        };
        for x in [4048.0, 4096.0, 4144.0] {
            let west_height = sample_at_x(&paths[0].points, 0.0, west, x);
            let east_height = sample_at_x(&paths[1].points, 4096.0, east, x);
            assert!(
                (west_height - east_height).abs() < 0.02,
                "overlap mismatch at {x}: {west_height} vs {east_height}"
            );
        }
        let west_grade =
            (west[1] - west[0]) / distance(paths[0].points[0], paths[0].points[1]);
        let east_grade =
            (east[1] - east[0]) / distance(paths[1].points[0], paths[1].points[1]);
        assert!((west_grade - east_grade).abs() < 0.0001);
        assert_eq!(dz[2].as_ref().unwrap(), &[10.0, 10.0]);
        assert_eq!(dz[3].as_ref().unwrap(), &[12.0, 12.0]);
    }

    #[test]
    fn reconciles_wholly_padded_duplicate_from_authoritative_neighbor() {
        let path = |layer: &str, feature, points: &[(f32, f32)]| BasePath {
            layer: layer.to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Elevated,
            points: points.to_vec(),
        };

        for padding_first in [false, true] {
            for padding_reversed in [false, true] {
                let mut padding_points =
                    vec![(3787.0, 4102.0), (3812.0, 4128.0), (3850.0, 4160.0)];
                let mut padding_heights = vec![16.6, 16.8, 15.3];
                if padding_reversed {
                    padding_points.reverse();
                    padding_heights.reverse();
                }
                let padding = || path("streets", 107, &padding_points);
                let authority = || {
                    path(
                        "streets",
                        168,
                        &[(3787.0, 6.0), (3812.0, 32.0), (3853.0, 66.0)],
                    )
                };
                let (paths, sources, mut dz) = if padding_first {
                    (
                        vec![padding(), authority()],
                        vec![
                            ((8415, 5382), (0.0, 0.0)),
                            ((8415, 5383), (0.0, 4096.0)),
                        ],
                        vec![
                            Some(padding_heights),
                            Some(vec![17.6, 16.8, 15.2]),
                        ],
                    )
                } else {
                    (
                        vec![authority(), padding()],
                        vec![
                            ((8415, 5383), (0.0, 4096.0)),
                            ((8415, 5382), (0.0, 0.0)),
                        ],
                        vec![
                            Some(vec![17.6, 16.8, 15.2]),
                            Some(padding_heights),
                        ],
                    )
                };

                reconcile_wholly_padded_duplicates(&paths, &sources, &mut dz);

                let padding_index =
                    paths.iter().position(|path| path.feature == 107).unwrap();
                let authority_index =
                    paths.iter().position(|path| path.feature == 168).unwrap();
                let reconciled = dz[padding_index].as_ref().unwrap();
                let expected = if padding_reversed {
                    vec![15.31, 16.8, 17.6]
                } else {
                    vec![17.6, 16.8, 15.31]
                };
                for (&actual, expected) in reconciled.iter().zip(expected) {
                    assert!((actual - expected).abs() < 0.01);
                }
                assert_eq!(
                    dz[authority_index].as_ref().unwrap(),
                    &[17.6, 16.8, 15.2]
                );
            }
        }
    }

    #[test]
    fn post_padding_consensus_flushes_owner_and_padding_continuations() {
        let path =
            |feature, vertical, points: &[(f32, f32)]| BasePath {
                layer: "streets".to_string(),
                feature,
                path: 0,
                is_polygon: false,
                vertical,
                points: points.to_vec(),
            };

        for reversed_order in [false, true] {
            let mut entries = vec![
                (
                    path(
                        77,
                        VerticalClass::Surface,
                        &[(3777.0, 4087.0), (3787.0, 4102.0)],
                    ),
                    ((8415, 5382), (0.0, 0.0)),
                    Some(vec![17.7, 17.7]),
                ),
                (
                    path(
                        107,
                        VerticalClass::Elevated,
                        &[(3787.0, 4102.0), (3812.0, 4128.0), (3850.0, 4160.0)],
                    ),
                    ((8415, 5382), (0.0, 0.0)),
                    Some(vec![16.6, 16.8, 15.3]),
                ),
                (
                    path(
                        121,
                        VerticalClass::Surface,
                        &[(3777.0, -9.0), (3787.0, 6.0)],
                    ),
                    ((8415, 5383), (0.0, 4096.0)),
                    Some(vec![17.7, 17.7]),
                ),
                (
                    path(
                        168,
                        VerticalClass::Elevated,
                        &[(3787.0, 6.0), (3812.0, 32.0), (3853.0, 66.0)],
                    ),
                    ((8415, 5383), (0.0, 4096.0)),
                    Some(vec![16.9, 16.8, 15.2]),
                ),
            ];
            if reversed_order {
                entries.reverse();
            }
            let mut paths = Vec::new();
            let mut sources = Vec::new();
            let mut dz = Vec::new();
            for (path, source, heights) in entries {
                paths.push(path);
                sources.push(source);
                dz.push(heights);
            }

            reconcile_cross_tile_copies(&paths, &sources, &mut dz, 0.36472446);

            let endpoint = |feature, at_start| {
                let index = paths
                    .iter()
                    .position(|path| path.feature == feature)
                    .unwrap();
                let values = dz[index].as_ref().unwrap();
                values[if at_start { 0 } else { values.len() - 1 }]
            };
            for value in [
                endpoint(77, false),
                endpoint(107, true),
                endpoint(121, false),
                endpoint(168, true),
            ] {
                assert!((value - 17.7).abs() < 0.001, "{value}");
            }

            // The exact-node repair raises and grade-blends the owner path.
            // Its slightly drifted padding duplicate must receive that final
            // profile, rather than retaining the pre-blend samples.
            let profile = |feature| {
                let index = paths
                    .iter()
                    .position(|path| path.feature == feature)
                    .unwrap();
                dz[index].as_ref().unwrap()
            };
            let padding = profile(107);
            let authority = profile(168);
            assert!((padding[0] - authority[0]).abs() < 0.001);
            assert!((padding[1] - authority[1]).abs() < 0.001);
            let t = (38.0 * 41.0 + 32.0 * 34.0) / (41.0 * 41.0 + 34.0 * 34.0);
            let projected = authority[1] * (1.0 - t) + authority[2] * t;
            assert!(
                (padding[2] - projected).abs() < 0.01,
                "final padding profile diverged: padding={padding:?} authority={authority:?}"
            );
        }
    }

    #[test]
    fn exact_cross_tile_endpoint_consensus_blends_into_neighbors() {
        let path = |feature, points: &[(f32, f32)]| BasePath {
            layer: "streets".to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Elevated,
            points: points.to_vec(),
        };

        for reversed_order in [false, true] {
            let mut entries = vec![
                (
                    path(1, &[(0.0, 0.0), (20.0, 0.0)]),
                    ((11, 20), (4096.0, 0.0)),
                    Some(vec![6.0, 5.0]),
                ),
                (
                    path(2, &[(4076.0, 0.0), (4096.0, 0.0)]),
                    ((10, 20), (0.0, 0.0)),
                    Some(vec![8.0, 8.0]),
                ),
            ];
            if reversed_order {
                entries.reverse();
            }
            let mut paths = Vec::new();
            let mut sources = Vec::new();
            let mut dz = Vec::new();
            for (path, source, heights) in entries {
                paths.push(path);
                sources.push(source);
                dz.push(heights);
            }

            reconcile_exact_continuation_endpoints(
                &paths,
                &sources,
                &mut dz,
                1.0,
            );

            let low_index =
                paths.iter().position(|path| path.feature == 1).unwrap();
            let high_index =
                paths.iter().position(|path| path.feature == 2).unwrap();
            let low = dz[low_index].as_ref().unwrap();
            let high = dz[high_index].as_ref().unwrap();
            assert!((low[0] - 8.0).abs() < 1e-6);
            assert!((high[1] - 8.0).abs() < 1e-6);
            assert!(
                (low[1] - 7.36).abs() < 1e-5,
                "endpoint adjustment did not blend inward: {low:?}"
            );
            assert!(
                low[0] - low[1] <= 0.64 + 1e-5,
                "terminal cliff survived: {low:?}"
            );
        }
    }

    #[test]
    fn wholly_padded_reconciliation_rejects_parallel_and_other_layer_paths() {
        let path =
            |layer: &str, feature, vertical, points: &[(f32, f32)]| BasePath {
                layer: layer.to_string(),
                feature,
                path: 0,
                is_polygon: false,
                vertical,
                points: points.to_vec(),
            };
        let paths = vec![
            path(
                "streets",
                107,
                VerticalClass::Elevated,
                &[(3787.0, 4102.0), (3812.0, 4128.0), (3850.0, 4160.0)],
            ),
            // A nearby parallel street in the owning tile is outside the
            // sub-unit duplicate tolerance.
            path(
                "streets",
                169,
                VerticalClass::Elevated,
                &[(3787.0, 8.0), (3812.0, 34.0), (3853.0, 68.0)],
            ),
            // Coincident label geometry is not the same physical layer.
            path(
                "street_labels",
                170,
                VerticalClass::Elevated,
                &[(3787.0, 6.0), (3812.0, 32.0), (3853.0, 66.0)],
            ),
            // Coincident surface geometry is not the elevated duplicate.
            path(
                "streets",
                171,
                VerticalClass::Surface,
                &[(3787.0, 6.0), (3812.0, 32.0), (3853.0, 66.0)],
            ),
            // A path crossing the nominal edge is handled by the ordinary
            // seam pass and is never rewritten as wholly-padding geometry.
            path(
                "streets",
                172,
                VerticalClass::Elevated,
                &[(3787.0, 4090.0), (3812.0, 4128.0)],
            ),
        ];
        let sources = vec![
            ((8415, 5382), (0.0, 0.0)),
            ((8415, 5383), (0.0, 4096.0)),
            ((8415, 5383), (0.0, 4096.0)),
            ((8415, 5383), (0.0, 4096.0)),
            ((8415, 5382), (0.0, 0.0)),
        ];
        let mut dz = vec![
            Some(vec![16.6, 16.8, 15.3]),
            Some(vec![30.0, 30.0, 30.0]),
            Some(vec![25.0, 25.0, 25.0]),
            Some(vec![20.0, 20.0, 20.0]),
            Some(vec![7.0, 8.0]),
        ];

        reconcile_wholly_padded_duplicates(&paths, &sources, &mut dz);

        assert_eq!(dz[0].as_ref().unwrap(), &[16.6, 16.8, 15.3]);
        assert_eq!(dz[1].as_ref().unwrap(), &[30.0, 30.0, 30.0]);
        assert_eq!(dz[2].as_ref().unwrap(), &[25.0, 25.0, 25.0]);
        assert_eq!(dz[3].as_ref().unwrap(), &[20.0, 20.0, 20.0]);
        assert_eq!(dz[4].as_ref().unwrap(), &[7.0, 8.0]);
    }

    #[test]
    fn reconciles_edge_consensus_before_cross_tile_plane() {
        let path = |feature, points: &[(f32, f32)]| BasePath {
            layer: "streets".to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: points.to_vec(),
        };
        let paths = vec![
            // West-tile padding copy. Its seam anchor was initially lower
            // than the coincident carrier, so a cross-tile plane fitted
            // before consensus produced a low outside endpoint.
            path(113, &[(4095.0, 1989.0), (4160.0, 1804.0)]),
            // East tile's authoritative, longer copy of the same centerline.
            path(95, &[(-1.0, 1989.0), (523.0, 486.0), (710.0, -64.0)]),
            // Another physical centerline supplies the final shared seam
            // height, as the encoded-feature boundary consensus also does.
            path(90, &[(4021.0, 2216.0), (4095.0, 1989.0)]),
        ];
        let sources = vec![
            ((8417, 5385), (0.0, 0.0)),
            ((8418, 5385), (4096.0, 0.0)),
            ((8417, 5385), (0.0, 0.0)),
        ];
        let mut dz = vec![
            Some(vec![5.5, 6.3]),
            Some(vec![5.5, 11.758, 10.9]),
            Some(vec![9.4, 10.9]),
        ];

        reconcile_base_vertex_consensus(&paths, &sources, &mut dz);
        assert!((dz[0].as_ref().unwrap()[0] - 10.9).abs() < 1e-6);
        assert!((dz[1].as_ref().unwrap()[0] - 10.9).abs() < 1e-6);

        reconcile_cross_tile_copies(&paths, &sources, &mut dz, 0.36472446);

        let west = dz[0].as_ref().unwrap();
        let east = dz[1].as_ref().unwrap();
        assert!(
            west[1] > 10.8,
            "stale pre-consensus anchor left a low padding wedge: {west:?}"
        );
        assert!((west[1] - 11.01).abs() < 0.03);
        assert!((east[0] - 10.9).abs() < 0.01);
        assert!((east[1] - 11.758).abs() < 1e-6);
        assert!((east[2] - 10.9).abs() < 1e-6);
        assert_eq!(dz[2].as_ref().unwrap(), &[9.4, 10.9]);
    }

    #[test]
    fn reconciles_lifted_collinear_endpoint_continuations() {
        let path = |feature, points: &[(f32, f32)]| BasePath {
            layer: "streets".to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: points.to_vec(),
        };
        let paths = vec![
            path(1, &[(90.0, 100.0), (100.0, 100.0)]),
            path(2, &[(100.0, 100.0), (110.0, 101.0)]),
            path(3, &[(110.0, 101.0), (120.0, 102.0)]),
            // A lifted perpendicular branch shares the first node but is
            // not the physical continuation and must remain independent.
            path(4, &[(100.0, 100.0), (100.0, 110.0)]),
        ];
        let sources = vec![((10, 20), (0.0, 0.0)); paths.len()];
        let mut dz = vec![
            Some(vec![3.0, 3.0]),
            Some(vec![5.2, 5.5]),
            Some(vec![7.0, 7.2]),
            Some(vec![8.0, 8.0]),
        ];

        reconcile_base_vertex_consensus(&paths, &sources, &mut dz);

        assert_eq!(dz[0].as_ref().unwrap(), &[3.0, 5.2]);
        assert_eq!(dz[1].as_ref().unwrap(), &[5.2, 7.0]);
        assert_eq!(dz[2].as_ref().unwrap(), &[7.0, 7.2]);
        assert_eq!(dz[3].as_ref().unwrap(), &[8.0, 8.0]);
    }

    #[test]
    fn reconciles_lifted_endpoint_into_interior_gore() {
        let path = |feature, points: &[(f32, f32)]| BasePath {
            layer: "streets".to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: points.to_vec(),
        };
        let paths = vec![
            // A high road endpoint arrives from the north.
            path(1, &[(100.0, 90.0), (100.0, 100.0)]),
            // The lower continuation is an interior vertex of a narrow V.
            path(2, &[(98.0, 110.0), (100.0, 100.0), (102.0, 110.0)]),
            // An unrelated through road at the same node is perpendicular.
            path(3, &[(90.0, 100.0), (100.0, 100.0), (110.0, 100.0)]),
        ];
        let sources = vec![((10, 20), (0.0, 0.0)); paths.len()];
        let mut dz = vec![
            Some(vec![8.5, 8.5]),
            Some(vec![5.5, 5.5, 5.5]),
            Some(vec![4.0, 4.0, 4.0]),
        ];

        reconcile_base_vertex_consensus(&paths, &sources, &mut dz);

        assert_eq!(dz[0].as_ref().unwrap(), &[8.5, 8.5]);
        assert_eq!(dz[1].as_ref().unwrap(), &[5.5, 8.5, 5.5]);
        assert_eq!(dz[2].as_ref().unwrap(), &[4.0, 4.0, 4.0]);
    }

    #[test]
    fn reconciles_same_side_quantized_terminal_overlap() {
        let path = |layer: &str, feature, points: &[(f32, f32)]| BasePath {
            layer: layer.to_string(),
            feature,
            path: 0,
            is_polygon: false,
            vertical: VerticalClass::Surface,
            points: points.to_vec(),
        };
        let paths = vec![
            // Authoritative west-tile segment: its quantized endpoint is
            // one extent unit shy of the nominal x=4096 seam.
            path("streets", 90, &[(4021.0, 2216.0), (4095.0, 1989.0)]),
            // Reverse copy in the east tile's west padding. It also ends at
            // x=-1, so neither copy crosses the nominal seam.
            path("streets", 76, &[(-64.0, 2181.0), (-1.0, 1989.0)]),
            // A separate parallel road is too far from the shared endpoint.
            path("streets", 77, &[(-64.0, 2193.0), (-1.0, 2001.0)]),
            // A line sharing the endpoint but crossing the carrier's
            // direction is rejected by the tangent gate.
            path("streets", 78, &[(-64.0, 1989.0), (-1.0, 1989.0)]),
            // Coincident label geometry is not the same physical layer.
            path("street_labels", 41, &[(-64.0, 2181.0), (-1.0, 1989.0)]),
        ];
        let sources = vec![
            ((8417, 5385), (0.0, 0.0)),
            ((8418, 5385), (4096.0, 0.0)),
            ((8418, 5385), (4096.0, 0.0)),
            ((8418, 5385), (4096.0, 0.0)),
            ((8418, 5385), (4096.0, 0.0)),
        ];
        let mut dz = vec![
            Some(vec![9.4, 10.9]),
            // Before the later encoded-feature boundary consensus, even
            // the coincident seam tip can disagree substantially.
            Some(vec![1.8, 5.5]),
            Some(vec![4.0, 4.0]),
            Some(vec![7.0, 10.9]),
            Some(vec![2.0, 10.9]),
        ];

        reconcile_cross_tile_copies(&paths, &sources, &mut dz, 0.36472446);

        let authoritative = dz[0].as_ref().unwrap();
        let padding = dz[1].as_ref().unwrap();
        assert_eq!(authoritative, &[9.4, 10.9]);
        assert!((padding[1] - 10.9).abs() < 1e-6);
        let tip = paths[0].points[1];
        let inner = paths[0].points[0];
        let padding_outer = (
            paths[1].points[0].0 + sources[1].1.0,
            paths[1].points[0].1,
        );
        let (vx, vy) = (inner.0 - tip.0, inner.1 - tip.1);
        let t = ((padding_outer.0 - tip.0) * vx + (padding_outer.1 - tip.1) * vy)
            / (vx * vx + vy * vy);
        let expected = authoritative[1] + (authoritative[0] - authoritative[1]) * t;
        assert!(
            (padding[0] - expected).abs() < 1e-5,
            "{} != {}",
            padding[0],
            expected
        );
        assert!((padding[0] - 9.63048).abs() < 1e-4);
        assert!(padding[0] > 9.0, "low padding wedge survived: {padding:?}");
        assert_eq!(dz[2].as_ref().unwrap(), &[4.0, 4.0]);
        assert_eq!(dz[3].as_ref().unwrap(), &[7.0, 10.9]);
        assert_eq!(dz[4].as_ref().unwrap(), &[2.0, 10.9]);
    }

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
            let ungated = field.sample(probe.0, probe.1, None, 3.0, None);
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
                sampled.push(
                    field
                        .sample(px, py, Some(dir), 1.2, None)
                        .map(|(z, d, _)| (z, d)),
                );
            }
            println!(
                "L={} F={} P={} pts {} first ({:.0},{:.0}) samples {:?}",
                path.layer, path.feature, path.path, count,
                path.points[0].0, path.points[0].1, sampled
            );
        }
    }

}
