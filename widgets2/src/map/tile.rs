use super::geometry::*;
use super::label::*;
use super::style::*;
use crate::makepad_draw::vector::{
    append_tessellated_geometry, tessellate_path_fill, LineCap, LineJoin, Tessellator, VVertex,
    VectorPath, VectorRenderParams, VECTOR_ZBIAS_STEP,
};
use crate::makepad_draw::*;
use crate::makepad_platform::makepad_micro_serde::*;
use flate2::read::{GzDecoder, ZlibDecoder};
use makepad_mbtile_reader::MbtilesReader;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const OVERPASS_ENDPOINTS: &[&str] = &["https://overpass.kumi.systems/api/interpreter"];
pub const MAX_PENDING_REQUESTS: usize = 2;
pub const MAX_TILE_RETRIES: u8 = 6;
pub const RETRY_BASE_FRAMES: u64 = 30;
pub const RETRY_MAX_FRAMES: u64 = 300;
pub const TILE_CACHE_DIR: &str = "local/tilecache_v4";
pub const TILE_QUERY_PAD: f64 = 0.05;
pub const LOCAL_MBTILES_PATH: &str = "local/noord-holland-shortbread-1.0.mbtiles";
pub const LOCAL_MBTILES_MIN_ZOOM: u32 = 0;
pub const LOCAL_MBTILES_MAX_ZOOM: u32 = 14;
pub const MAX_LOCAL_TILE_BATCH: usize = 10;
pub const ROAD_CLIP_PADDING: f32 = 8.0;
pub const ROAD_SMOOTH_FACTOR: f32 = 0.0;
pub const ROAD_CENTER_OVERLAY_WIDTH_SCALE: f32 = 1.2;
pub const ROAD_CENTER_OVERLAY_CASING_SCALE: f32 = 0.80;
pub const ROAD_CENTER_OVERLAY_CASING_EPSILON: f32 = 0.02;
pub const ROAD_CENTER_OVERLAY_MIN_WIDTH: f32 = 0.45;
pub const EARCUT_MAX_RINGS: usize = 500;

const MVT_INTERNAL_FEATURE_KEY: &str = "__mp_feature";
const MVT_INTERNAL_RING_INDEX_KEY: &str = "__mp_ring";

// --- Tile state types ---

#[derive(Debug)]
pub enum TileLoadState {
    LoadingNetwork,
    LoadingLocal,
    Ready {
        fill_geometry: Option<Geometry>,
        stroke_geometry: Option<Geometry>,
        feature_count: usize,
        labels: Vec<TileLabel>,
    },
    Failed {
        retry_after: u64,
    },
}

#[derive(Debug)]
pub struct TileEntry {
    pub state: TileLoadState,
    pub last_used: u64,
    pub attempts: u8,
}

#[derive(Debug)]
pub struct PendingTileRequest {
    pub tile_key: TileKey,
    pub endpoint: &'static str,
}

#[derive(Debug)]
pub enum LocalSourceMessage {
    Generated {
        style_epoch: u64,
        requested: Vec<TileKey>,
        loaded: Vec<LoadedLocalTile>,
    },
    Failed {
        style_epoch: u64,
        requested: Vec<TileKey>,
        error: String,
    },
}

#[derive(Debug)]
pub struct LoadedLocalTile {
    pub tile_key: TileKey,
    pub buffers: TileBuffers,
}

// --- Internal data types ---

#[derive(Debug)]
struct WayData {
    nodes: Vec<i64>,
    tags: HashMap<String, String>,
    closed: bool,
}

#[derive(Debug)]
pub struct TileBuffers {
    pub fill_indices: Vec<u32>,
    pub fill_vertices: Vec<f32>,
    pub stroke_indices: Vec<u32>,
    pub stroke_vertices: Vec<f32>,
    pub feature_count: usize,
    pub labels: Vec<TileLabel>,
}

#[derive(Clone, Debug)]
struct StrokeDrawJob {
    sort_rank: i16,
    style: StrokeStyle,
    center_overlay: bool,
    points: Vec<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct StrokePassKey {
    color: u32,
    width_bits: u32,
    shape_id_bits: u32,
}

impl From<StrokePassStyle> for StrokePassKey {
    fn from(value: StrokePassStyle) -> Self {
        Self {
            color: value.color,
            width_bits: value.width.to_bits(),
            shape_id_bits: value.shape_id.to_bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct StrokeStyleKey {
    sort_rank: i16,
    casing: Option<StrokePassKey>,
    center: StrokePassKey,
}

impl From<StrokeStyle> for StrokeStyleKey {
    fn from(value: StrokeStyle) -> Self {
        Self {
            sort_rank: value.sort_rank,
            casing: value.casing.map(StrokePassKey::from),
            center: StrokePassKey::from(value.center),
        }
    }
}

#[derive(Clone, Debug)]
struct PreparedWay {
    way_index: usize,
    points: Vec<(f32, f32)>,
}

#[derive(Debug)]
struct FillFeatureGroup {
    color: u32,
    rings: Vec<FillRing>,
}

#[derive(DeJson)]
struct OverpassResponse {
    elements: Vec<OverpassElement>,
}

#[derive(DeJson)]
struct OverpassElement {
    #[rename(type)]
    kind: String,
    id: i64,
    lat: Option<f64>,
    lon: Option<f64>,
    nodes: Option<Vec<i64>>,
    tags: Option<HashMap<String, String>>,
}

// --- Public API ---

pub fn retry_delay_frames(attempts: u8) -> u64 {
    let shift = attempts.saturating_sub(1).min(6) as u32;
    let delay = RETRY_BASE_FRAMES.saturating_mul(1_u64 << shift);
    delay.min(RETRY_MAX_FRAMES)
}

pub fn overpass_endpoint(attempts: u8) -> &'static str {
    let index = attempts as usize % OVERPASS_ENDPOINTS.len();
    OVERPASS_ENDPOINTS[index]
}

pub fn overpass_query(tile: TileKey) -> String {
    let (south, west, north, east) = tile_bounds_padded(tile, TILE_QUERY_PAD);
    let mut ways = String::new();

    ways.push_str(&format!(
        "way[\"highway\"]({south:.6},{west:.6},{north:.6},{east:.6});\
         way[\"waterway\"]({south:.6},{west:.6},{north:.6},{east:.6});\
         way[\"natural\"=\"water\"]({south:.6},{west:.6},{north:.6},{east:.6});"
    ));

    if tile.z >= 15 {
        ways.push_str(&format!(
            "way[\"building\"][\"building\"!=\"no\"]({south:.6},{west:.6},{north:.6},{east:.6});"
        ));
    }

    if tile.z >= 14 {
        ways.push_str(&format!(
            "way[\"landuse\"]({south:.6},{west:.6},{north:.6},{east:.6});\
             way[\"leisure\"]({south:.6},{west:.6},{north:.6},{east:.6});"
        ));
    }

    format!(
        "[out:json][timeout:20];\
         ({ways});\
         (._;>;);\
         out body;"
    )
}

pub fn ensure_cache_dir() {
    let _ = fs::create_dir_all(TILE_CACHE_DIR);
}

pub fn tile_data_cache_path_for(tile_key: TileKey) -> PathBuf {
    Path::new(TILE_CACHE_DIR).join(format!(
        "z{}_x{}_y{}.json",
        tile_key.z, tile_key.x, tile_key.y
    ))
}

pub fn store_tile_data_cache_on_disk(tile_key: TileKey, body: &str) {
    ensure_cache_dir();
    let path = tile_data_cache_path_for(tile_key);
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, body).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    let _ = fs::rename(&tmp, &path);
}

pub fn format_tile_key_sample(keys: &[TileKey], limit: usize) -> String {
    if keys.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::from("[");
    for (index, key) in keys.iter().take(limit).enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("z{}x{}y{}", key.z, key.x, key.y));
    }
    if keys.len() > limit {
        out.push_str(", ...");
    }
    out.push(']');
    out
}

// --- Tile buffer building ---

pub fn build_tile_buffers_from_body(
    tile_key: TileKey,
    body: &str,
    theme: &CompiledMapTheme,
) -> Result<TileBuffers, String> {
    let parsed = OverpassResponse::deserialize_json_lenient(body)
        .map_err(|e| format!("json error at line {} col {}: {}", e.line, e.col, e.msg))?;

    let mut nodes = HashMap::<i64, (f64, f64)>::new();
    let mut ways = Vec::<WayData>::new();
    let mut labels = Vec::<TileLabel>::new();

    for element in parsed.elements {
        match element.kind.as_str() {
            "node" => {
                if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                    nodes.insert(element.id, (lon, lat));
                    if let Some(tags) = element.tags {
                        let world = lon_lat_to_world(lon, lat, tile_key.z);
                        if let Some(label) =
                            extract_point_label(&tags, (world.x as f32, world.y as f32))
                        {
                            labels.push(label);
                        }
                    }
                }
            }
            "way" => {
                if let Some(node_ids) = element.nodes {
                    let closed =
                        node_ids.len() > 2 && node_ids.first().copied() == node_ids.last().copied();
                    ways.push(WayData {
                        nodes: node_ids,
                        tags: element.tags.unwrap_or_default(),
                        closed,
                    });
                }
            }
            _ => {}
        }
    }

    let mut path = VectorPath::new();
    let mut tess = Tessellator::default();
    let mut tess_verts = Vec::<VVertex>::new();
    let mut tess_indices = Vec::<u32>::new();

    let mut fill_indices = Vec::<u32>::new();
    let mut fill_vertices = Vec::<f32>::new();
    let mut stroke_indices = Vec::<u32>::new();
    let mut stroke_vertices = Vec::<f32>::new();
    let mut fill_zbias = 0.0_f32;
    let mut stroke_zbias = 0.0_f32;
    let mut feature_count = 0usize;

    let mut prepared = Vec::<PreparedWay>::with_capacity(ways.len());
    for (way_index, way) in ways.iter().enumerate() {
        let projected = project_way_points_with_nodes(&way.nodes, &nodes, tile_key.z);
        if projected.len() < 2 {
            continue;
        }
        let mut points = Vec::<(f32, f32)>::with_capacity(projected.len());
        for (_node_id, point) in projected {
            points.push(point);
        }
        prepared.push(PreparedWay { way_index, points });
    }

    // Fill pass
    let mut fill_groups = Vec::<FillFeatureGroup>::new();
    let mut fill_group_lookup = HashMap::<(String, u32), usize>::new();
    for (order, prepared_way) in prepared.iter().enumerate() {
        let way = &ways[prepared_way.way_index];
        let Some(color) = fill_color_for_tags(theme, &way.tags, way.closed) else {
            continue;
        };
        let Some(ring_points) = normalize_polygon_ring(&prepared_way.points) else {
            continue;
        };

        let feature_key = way
            .tags
            .get(MVT_INTERNAL_FEATURE_KEY)
            .cloned()
            .unwrap_or_else(|| format!("way:{}", prepared_way.way_index));
        let group_key = (feature_key, color);
        let group_index = if let Some(index) = fill_group_lookup.get(&group_key).copied() {
            index
        } else {
            let index = fill_groups.len();
            fill_group_lookup.insert(group_key, index);
            fill_groups.push(FillFeatureGroup {
                color,
                rings: Vec::new(),
            });
            index
        };

        let ring_order = way
            .tags
            .get(MVT_INTERNAL_RING_INDEX_KEY)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(order);
        let signed_area = polygon_signed_area(&ring_points);
        if signed_area.abs() <= POLYGON_AREA_EPSILON {
            continue;
        }
        fill_groups[group_index].rings.push(FillRing {
            order: ring_order,
            points: ring_points,
            signed_area,
        });
    }

    for group in fill_groups {
        let polygons = classify_polygon_rings(&group.rings, EARCUT_MAX_RINGS);
        for polygon in polygons {
            if polygon.is_empty() {
                continue;
            }
            for ring in &polygon {
                emit_path(&mut path, ring, true);
            }
            tessellate_path_fill(
                &mut path,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                LineJoin::Miter,
                4.0,
                1.0,
                false,
            );
            append_tessellated_geometry(
                &tess_verts,
                &tess_indices,
                &mut fill_vertices,
                &mut fill_indices,
                VectorRenderParams {
                    color: hex_to_premul_rgba(group.color, 1.0),
                    stroke_mult: 1e6,
                    shape_id: 0.0,
                    params: [0.0; 6],
                    zbias: fill_zbias,
                },
            );
            fill_zbias += VECTOR_ZBIAS_STEP;
            feature_count += 1;
        }
    }

    // Stroke pass
    let mut stroke_jobs = Vec::<StrokeDrawJob>::new();
    for prepared_way in &prepared {
        let way = &ways[prepared_way.way_index];
        if let Some(label) = extract_way_label(&way.tags, &prepared_way.points) {
            labels.push(label);
        }
        if let Some(style) = stroke_style_for_tags(theme, &way.tags, tile_key.z) {
            let center_overlay = way.tags.contains_key("highway");
            stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                center_overlay,
                points: prepared_way.points.clone(),
            });
        }
    }

    let mut grouped_strokes =
        HashMap::<(StrokeStyleKey, bool), (StrokeStyle, bool, Vec<Vec<(f32, f32)>>)>::new();
    for job in stroke_jobs {
        let key = (StrokeStyleKey::from(job.style), job.center_overlay);
        let entry =
            grouped_strokes
                .entry(key)
                .or_insert((job.style, job.center_overlay, Vec::new()));
        entry.2.push(job.points);
    }

    let mut merged_stroke_jobs = Vec::<StrokeDrawJob>::new();
    for (_key, (style, center_overlay, polylines)) in grouped_strokes {
        for points in merge_stroke_polylines(&polylines) {
            merged_stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                center_overlay,
                points,
            });
        }
    }

    merged_stroke_jobs.sort_unstable_by_key(|job| job.sort_rank);
    let clip_bounds = tile_clip_bounds(tile_key, ROAD_CLIP_PADDING);
    let mut merged_stroke_parts = Vec::<(StrokeStyle, bool, Vec<Vec<(f32, f32)>>)>::new();
    for job in merged_stroke_jobs {
        let parts = build_polyline_parts(&job.points, clip_bounds, false, ROAD_SMOOTH_FACTOR);
        merged_stroke_parts.push((job.style, job.center_overlay, parts));
    }

    // Pass 1: casings
    for (style, _center_overlay, parts) in &merged_stroke_parts {
        let Some(casing) = style.casing else {
            continue;
        };
        for part in parts {
            if part.len() < 2 {
                continue;
            }
            append_stroke_pass(
                &mut path,
                part,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                &mut stroke_vertices,
                &mut stroke_indices,
                casing,
                LineCap::Butt,
                &mut stroke_zbias,
            );
            feature_count += 1;
        }
    }

    // Pass 2: centers/overlays
    for (style, center_overlay, parts) in &merged_stroke_parts {
        for part in parts {
            if part.len() < 2 {
                continue;
            }
            if *center_overlay {
                let overlay_width = if let Some(casing) = style.casing {
                    let casing_limit = (casing.width - ROAD_CENTER_OVERLAY_CASING_EPSILON).max(0.0);
                    (casing.width * ROAD_CENTER_OVERLAY_CASING_SCALE).min(casing_limit)
                } else {
                    style.center.width * ROAD_CENTER_OVERLAY_WIDTH_SCALE
                }
                .max(ROAD_CENTER_OVERLAY_MIN_WIDTH);
                if overlay_width > 0.0 {
                    append_stroke_fill_overlay_pass(
                        &mut path,
                        part,
                        &mut tess,
                        &mut tess_verts,
                        &mut tess_indices,
                        &mut stroke_vertices,
                        &mut stroke_indices,
                        StrokePassStyle {
                            color: style.center.color,
                            width: overlay_width,
                            ..style.center
                        },
                        LineCap::Square,
                        &mut stroke_zbias,
                    );
                    feature_count += 1;
                }
            } else {
                append_stroke_pass(
                    &mut path,
                    part,
                    &mut tess,
                    &mut tess_verts,
                    &mut tess_indices,
                    &mut stroke_vertices,
                    &mut stroke_indices,
                    style.center,
                    LineCap::Butt,
                    &mut stroke_zbias,
                );
                feature_count += 1;
            }
        }
    }

    compact_tile_labels(&mut labels);

    Ok(TileBuffers {
        fill_indices,
        fill_vertices,
        stroke_indices,
        stroke_vertices,
        feature_count,
        labels,
    })
}

fn project_way_points_with_nodes(
    node_ids: &[i64],
    nodes: &HashMap<i64, (f64, f64)>,
    zoom: u32,
) -> Vec<(i64, (f32, f32))> {
    let mut out = Vec::with_capacity(node_ids.len());
    let mut last: Option<(f32, f32)> = None;

    for node_id in node_ids {
        let Some((lon, lat)) = nodes.get(node_id).copied() else {
            continue;
        };
        let world = lon_lat_to_world(lon, lat, zoom);
        let point = (world.x as f32, world.y as f32);

        if let Some(prev) = last {
            let dx = point.0 - prev.0;
            let dy = point.1 - prev.1;
            if dx * dx + dy * dy < 0.25 {
                continue;
            }
        }

        out.push((*node_id, point));
        last = Some(point);
    }

    out
}

// --- Local mbtiles loading ---

pub fn load_local_tile_batch(
    mbtiles_path: &Path,
    cache_dir: &Path,
    requested: &[TileKey],
    theme: &CompiledMapTheme,
) -> Result<Vec<LoadedLocalTile>, String> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    let mut loaded = Vec::<LoadedLocalTile>::new();
    let mut missing = Vec::<TileKey>::new();
    for key in requested {
        let cache_path = cache_dir.join(format!("z{}_x{}_y{}.json", key.z, key.x, key.y));
        match fs::read_to_string(&cache_path) {
            Ok(body) => match build_tile_buffers_from_body(*key, &body, theme) {
                Ok(buffers) => loaded.push(LoadedLocalTile {
                    tile_key: *key,
                    buffers,
                }),
                Err(err) => {
                    log!(
                        "MapView: cache parse failed for tile z{} x{} y{}: {}",
                        key.z,
                        key.x,
                        key.y,
                        err
                    );
                    let _ = fs::remove_file(cache_path);
                    missing.push(*key);
                }
            },
            Err(_) => missing.push(*key),
        }
    }

    if missing.is_empty() {
        return Ok(loaded);
    }

    let mut reader = MbtilesReader::open(mbtiles_path)
        .map_err(|err| format!("open {}: {}", mbtiles_path.display(), err))?;

    let mut by_zoom = HashMap::<u32, Vec<TileKey>>::new();
    for key in &missing {
        by_zoom.entry(key.z).or_default().push(*key);
    }

    let mut logged_xyz_row_scheme = false;

    for (zoom, keys) in by_zoom {
        let tile_count = 1_i64 << zoom;
        let mut needed_tms = HashMap::<(i64, i64), TileKey>::new();
        let mut needed_xyz = HashMap::<(i64, i64), TileKey>::new();
        for key in keys {
            let x = key.x as i64;
            let xyz_row = key.y as i64;
            let tms_row = tile_count - 1 - key.y as i64;
            needed_tms.insert((x, tms_row), key);
            needed_xyz.insert((x, xyz_row), key);
        }

        let tiles = reader.get_tiles_at_zoom(zoom as i64).map_err(|err| {
            format!(
                "read zoom {} from {}: {}",
                zoom,
                mbtiles_path.display(),
                err
            )
        })?;

        for tile in tiles {
            let lookup = (tile.tile_column, tile.tile_row);

            let matched = if let Some(tile_key) = needed_tms.remove(&lookup) {
                let xyz_lookup = (tile_key.x as i64, tile_key.y as i64);
                needed_xyz.remove(&xyz_lookup);
                Some((tile_key, false))
            } else if let Some(tile_key) = needed_xyz.remove(&lookup) {
                let tms_lookup = (tile_key.x as i64, tile_count - 1 - tile_key.y as i64);
                needed_tms.remove(&tms_lookup);
                Some((tile_key, true))
            } else {
                None
            };

            let Some((tile_key, used_xyz_row)) = matched else {
                continue;
            };

            if used_xyz_row && !logged_xyz_row_scheme {
                log!("MapView: local mbtiles rows appear XYZ-oriented (matched without TMS row flip)");
                logged_xyz_row_scheme = true;
            }

            match mbtiles_tile_to_overpass_json(tile_key, &tile.tile_data) {
                Ok(body) => match build_tile_buffers_from_body(tile_key, &body, theme) {
                    Ok(buffers) => {
                        store_tile_data_cache_on_disk(tile_key, &body);
                        loaded.push(LoadedLocalTile { tile_key, buffers });
                    }
                    Err(err) => {
                        log!(
                            "MapView: failed to triangulate local mbtile z{} x{} y{}: {}",
                            tile_key.z,
                            tile_key.x,
                            tile_key.y,
                            err
                        );
                    }
                },
                Err(err) => {
                    log!(
                        "MapView: failed to decode local mbtile z{} x{} y{}: {}",
                        tile_key.z,
                        tile_key.x,
                        tile_key.y,
                        err
                    );
                }
            }
        }

        if !needed_tms.is_empty() {
            let mut missing = needed_tms.values().copied().collect::<Vec<_>>();
            missing.sort_unstable();
            log!(
                "MapView: local mbtiles missing {} tile(s) at z{} sample:{}",
                missing.len(),
                zoom,
                format_tile_key_sample(&missing, 8)
            );
        }
    }

    Ok(loaded)
}

// --- MVT (Mapbox Vector Tile) parsing ---

fn mbtiles_tile_to_overpass_json(
    tile_key: TileKey,
    raw_tile_data: &[u8],
) -> Result<String, String> {
    let pbf_data = decode_vector_tile_payload(raw_tile_data)?;
    let mut builder = MvtTileJsonBuilder::default();
    parse_mvt_tile(&pbf_data, tile_key, &mut builder)?;
    Ok(builder.to_json())
}

fn decode_vector_tile_payload(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut decoder = GzDecoder::new(raw);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|err| format!("gzip decode failed: {}", err))?;
        return Ok(out);
    }
    if raw.len() >= 2 && raw[0] == 0x78 {
        let mut decoder = ZlibDecoder::new(raw);
        let mut out = Vec::new();
        if decoder.read_to_end(&mut out).is_ok() {
            return Ok(out);
        }
    }
    Ok(raw.to_vec())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MvtGeomType {
    Unknown,
    Point,
    LineString,
    Polygon,
}

impl MvtGeomType {
    fn from_u64(value: u64) -> Self {
        match value {
            1 => Self::Point,
            2 => Self::LineString,
            3 => Self::Polygon,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
enum MvtValue {
    String(String),
    Float(f32),
    Double(f64),
    Int(i64),
    UInt(u64),
    SInt(i64),
    Bool(bool),
}

impl MvtValue {
    fn to_tag_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Float(value) => format!("{}", value),
            Self::Double(value) => format!("{}", value),
            Self::Int(value) => format!("{}", value),
            Self::UInt(value) => format!("{}", value),
            Self::SInt(value) => format!("{}", value),
            Self::Bool(value) => {
                if *value {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
        }
    }
}

#[derive(Debug)]
struct MvtTileJsonBuilder {
    nodes: Vec<(i64, f64, f64)>,
    tagged_nodes: Vec<(i64, f64, f64, HashMap<String, String>)>,
    ways: Vec<(i64, Vec<i64>, HashMap<String, String>)>,
    next_node_id: i64,
    next_way_id: i64,
    next_generated_feature_id: u64,
}

impl Default for MvtTileJsonBuilder {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            tagged_nodes: Vec::new(),
            ways: Vec::new(),
            next_node_id: 1,
            next_way_id: 1,
            next_generated_feature_id: 1,
        }
    }
}

impl MvtTileJsonBuilder {
    fn alloc_feature_id(&mut self) -> u64 {
        let id = self.next_generated_feature_id;
        self.next_generated_feature_id = self.next_generated_feature_id.wrapping_add(1);
        if self.next_generated_feature_id == 0 {
            self.next_generated_feature_id = 1;
        }
        id
    }

    fn add_path(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        let mut node_ids = Vec::with_capacity(points.len() + 1);
        for &(x, y) in points {
            let node_id = self.next_node_id;
            self.next_node_id += 1;
            let (lon, lat) = local_tile_to_lon_lat(tile_key, extent, x, y);
            self.nodes.push((node_id, lon, lat));
            if node_ids.last().copied() != Some(node_id) {
                node_ids.push(node_id);
            }
        }
        if node_ids.len() < 2 {
            return;
        }
        if close && node_ids.first().copied() != node_ids.last().copied() {
            if let Some(first) = node_ids.first().copied() {
                node_ids.push(first);
            }
        }
        if node_ids.len() < 2 {
            return;
        }
        let way_id = self.next_way_id;
        self.next_way_id += 1;
        self.ways.push((way_id, node_ids, tags));
    }

    fn add_point(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        point: (i32, i32),
        tags: HashMap<String, String>,
    ) {
        let node_id = self.next_node_id;
        self.next_node_id += 1;
        let (lon, lat) = local_tile_to_lon_lat(tile_key, extent, point.0, point.1);
        self.tagged_nodes.push((node_id, lon, lat, tags));
    }

    fn to_json(&self) -> String {
        let mut out = String::with_capacity(
            32 + self.nodes.len() * 64 + self.tagged_nodes.len() * 192 + self.ways.len() * 192,
        );
        out.push_str("{\"elements\":[");
        let mut first = true;

        for &(id, lon, lat) in &self.nodes {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"type\":\"node\",\"id\":");
            out.push_str(&id.to_string());
            out.push_str(",\"lat\":");
            out.push_str(&format!("{:.8}", lat));
            out.push_str(",\"lon\":");
            out.push_str(&format!("{:.8}", lon));
            out.push('}');
        }

        for (id, lon, lat, tags) in &self.tagged_nodes {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"type\":\"node\",\"id\":");
            out.push_str(&id.to_string());
            out.push_str(",\"lat\":");
            out.push_str(&format!("{:.8}", lat));
            out.push_str(",\"lon\":");
            out.push_str(&format!("{:.8}", lon));
            out.push_str(",\"tags\":{");
            let mut tag_first = true;
            for (key, value) in tags {
                if !tag_first {
                    out.push(',');
                }
                tag_first = false;
                append_json_string(&mut out, key);
                out.push(':');
                append_json_string(&mut out, value);
            }
            out.push_str("}}");
        }

        for (id, node_ids, tags) in &self.ways {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"type\":\"way\",\"id\":");
            out.push_str(&id.to_string());
            out.push_str(",\"nodes\":[");
            for (index, node_id) in node_ids.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&node_id.to_string());
            }
            out.push_str("],\"tags\":{");
            let mut tag_first = true;
            for (key, value) in tags {
                if !tag_first {
                    out.push(',');
                }
                tag_first = false;
                append_json_string(&mut out, key);
                out.push(':');
                append_json_string(&mut out, value);
            }
            out.push_str("}}");
        }

        out.push_str("]}");
        out
    }
}

fn parse_mvt_tile(
    tile_data: &[u8],
    tile_key: TileKey,
    builder: &mut MvtTileJsonBuilder,
) -> Result<(), String> {
    let mut pos = 0_usize;
    while pos < tile_data.len() {
        let key = read_pb_varint(tile_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (3, 2) => {
                let layer = read_pb_len_slice(tile_data, &mut pos)?;
                parse_mvt_layer(layer, tile_key, builder)?;
            }
            _ => skip_pb_field(tile_data, &mut pos, wire)?,
        }
    }
    Ok(())
}

fn parse_mvt_layer(
    layer_data: &[u8],
    tile_key: TileKey,
    builder: &mut MvtTileJsonBuilder,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut layer_name = String::new();
    let mut extent = 4096_u32;
    let mut features = Vec::<&[u8]>::new();
    let mut keys = Vec::<String>::new();
    let mut values = Vec::<MvtValue>::new();

    while pos < layer_data.len() {
        let key = read_pb_varint(layer_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 2) => {
                let slice = read_pb_len_slice(layer_data, &mut pos)?;
                layer_name = String::from_utf8_lossy(slice).into_owned();
            }
            (2, 2) => features.push(read_pb_len_slice(layer_data, &mut pos)?),
            (3, 2) => {
                let slice = read_pb_len_slice(layer_data, &mut pos)?;
                keys.push(String::from_utf8_lossy(slice).into_owned());
            }
            (4, 2) => {
                let value = parse_mvt_value(read_pb_len_slice(layer_data, &mut pos)?)?;
                values.push(value);
            }
            (5, 0) => extent = read_pb_varint(layer_data, &mut pos)? as u32,
            _ => skip_pb_field(layer_data, &mut pos, wire)?,
        }
    }

    let extent = extent.max(1);
    for feature_data in features {
        parse_mvt_feature(
            feature_data,
            &layer_name,
            &keys,
            &values,
            extent,
            tile_key,
            builder,
        )?;
    }
    Ok(())
}

fn parse_mvt_feature(
    feature_data: &[u8],
    layer_name: &str,
    keys: &[String],
    values: &[MvtValue],
    extent: u32,
    tile_key: TileKey,
    builder: &mut MvtTileJsonBuilder,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut feature_id: Option<u64> = None;
    let mut tag_indexes = Vec::<u32>::new();
    let mut geom_type = MvtGeomType::Unknown;
    let mut geometry_cmds = Vec::<u32>::new();

    while pos < feature_data.len() {
        let key = read_pb_varint(feature_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 0) => feature_id = Some(read_pb_varint(feature_data, &mut pos)?),
            (2, 2) => {
                let packed = read_pb_len_slice(feature_data, &mut pos)?;
                tag_indexes = read_packed_u32(packed)?;
            }
            (3, 0) => geom_type = MvtGeomType::from_u64(read_pb_varint(feature_data, &mut pos)?),
            (4, 2) => {
                let packed = read_pb_len_slice(feature_data, &mut pos)?;
                geometry_cmds = read_packed_u32(packed)?;
            }
            _ => skip_pb_field(feature_data, &mut pos, wire)?,
        }
    }

    if geom_type == MvtGeomType::Unknown {
        return Ok(());
    }

    let mut tags = HashMap::<String, String>::new();
    for pair in tag_indexes.chunks_exact(2) {
        let key_index = pair[0] as usize;
        let value_index = pair[1] as usize;
        let Some(key) = keys.get(key_index) else {
            continue;
        };
        let Some(value) = values.get(value_index) else {
            continue;
        };
        tags.insert(key.clone(), value.to_tag_string());
    }
    normalize_mvt_tags(layer_name, geom_type, &mut tags);

    let paths = decode_mvt_geometry(&geometry_cmds, geom_type)?;
    if geom_type == MvtGeomType::Point {
        if !should_emit_mvt_point_label_feature(&tags) {
            return Ok(());
        }
        for path in paths {
            let Some(point) = path.first().copied() else {
                continue;
            };
            builder.add_point(tile_key, extent, point, tags.clone());
        }
        return Ok(());
    }

    let polygon_feature_key = if geom_type == MvtGeomType::Polygon {
        let raw_id = feature_id.unwrap_or_else(|| builder.alloc_feature_id());
        Some(format!("{}:{}", layer_name, raw_id))
    } else {
        None
    };

    for (ring_index, mut path) in paths.into_iter().enumerate() {
        if path.len() < 2 {
            continue;
        }
        let close = geom_type == MvtGeomType::Polygon;
        if close && path.first().copied() != path.last().copied() {
            if let Some(first) = path.first().copied() {
                path.push(first);
            }
        }
        if close && path.len() < 4 {
            continue;
        }
        let mut path_tags = tags.clone();
        if let Some(feature_key) = &polygon_feature_key {
            path_tags.insert(MVT_INTERNAL_FEATURE_KEY.to_string(), feature_key.clone());
            path_tags.insert(
                MVT_INTERNAL_RING_INDEX_KEY.to_string(),
                ring_index.to_string(),
            );
        }
        builder.add_path(tile_key, extent, &path, path_tags, close);
    }

    Ok(())
}

fn normalize_mvt_tags(
    layer_name: &str,
    geom_type: MvtGeomType,
    tags: &mut HashMap<String, String>,
) {
    tags.entry("layer".to_string())
        .or_insert_with(|| layer_name.to_string());

    match layer_name {
        "building" | "buildings" => {
            tags.entry("building".to_string())
                .or_insert_with(|| "yes".to_string());
        }
        "water" | "water_polygons" | "water_polygons_labels" | "ocean" => {
            if geom_type == MvtGeomType::Polygon {
                tags.entry("natural".to_string())
                    .or_insert_with(|| "water".to_string());
            } else {
                tags.entry("waterway".to_string())
                    .or_insert_with(|| "river".to_string());
            }
        }
        "waterway" | "water_lines" | "water_lines_labels" | "dam_lines" | "pier_lines" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("subclass").cloned())
                .or_else(|| tags.get("class").cloned())
                .unwrap_or_else(|| "river".to_string());
            tags.entry("waterway".to_string()).or_insert(value);
        }
        "transportation"
        | "transportation_name"
        | "road"
        | "streets"
        | "street_polygons"
        | "street_labels"
        | "street_labels_points"
        | "streets_polygons_labels"
        | "bridges"
        | "aerialways"
        | "ferries"
        | "public_transport" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("subclass").cloned())
                .or_else(|| tags.get("class").cloned())
                .unwrap_or_else(|| "residential".to_string());
            tags.entry("highway".to_string())
                .or_insert_with(|| normalize_highway_kind(&value));
        }
        "railway" => {
            tags.entry("railway".to_string())
                .or_insert_with(|| "rail".to_string());
        }
        "park" => {
            tags.entry("leisure".to_string())
                .or_insert_with(|| "park".to_string());
        }
        "landuse" | "landcover" | "land" | "sites" | "pois" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("class").cloned())
                .or_else(|| tags.get("subclass").cloned())
                .unwrap_or_else(|| "residential".to_string());
            if is_leisure_kind(&value) {
                tags.entry("leisure".to_string())
                    .or_insert_with(|| "park".to_string());
            } else {
                tags.entry("landuse".to_string()).or_insert(value);
            }
        }
        _ => {}
    }
}

fn should_emit_mvt_point_label_feature(tags: &HashMap<String, String>) -> bool {
    let Some(layer) = tags.get("layer") else {
        return false;
    };
    if !is_road_point_label_layer(layer) {
        return false;
    }
    if !tags.contains_key("highway") {
        return false;
    }
    select_label_text(tags).is_some()
}

fn normalize_highway_kind(kind: &str) -> String {
    match kind {
        "motorway_link" => "motorway".to_string(),
        "trunk_link" => "trunk".to_string(),
        "primary_link" => "primary".to_string(),
        "secondary_link" => "secondary".to_string(),
        "tertiary_link" => "tertiary".to_string(),
        "major_road" => "primary".to_string(),
        "minor_road" => "residential".to_string(),
        "path" => "path".to_string(),
        other => other.to_string(),
    }
}

fn is_leisure_kind(kind: &str) -> bool {
    matches!(
        kind,
        "park" | "garden" | "playground" | "golf_course" | "pitch" | "sports_centre"
    )
}

fn parse_mvt_value(bytes: &[u8]) -> Result<MvtValue, String> {
    let mut pos = 0_usize;
    let mut value = MvtValue::String(String::new());
    while pos < bytes.len() {
        let key = read_pb_varint(bytes, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 2) => {
                let slice = read_pb_len_slice(bytes, &mut pos)?;
                value = MvtValue::String(String::from_utf8_lossy(slice).into_owned());
            }
            (2, 5) => value = MvtValue::Float(f32::from_bits(read_pb_fixed32(bytes, &mut pos)?)),
            (3, 1) => value = MvtValue::Double(f64::from_bits(read_pb_fixed64(bytes, &mut pos)?)),
            (4, 0) => value = MvtValue::Int(read_pb_varint(bytes, &mut pos)? as i64),
            (5, 0) => value = MvtValue::UInt(read_pb_varint(bytes, &mut pos)?),
            (6, 0) => value = MvtValue::SInt(zigzag_decode_u64(read_pb_varint(bytes, &mut pos)?)),
            (7, 0) => value = MvtValue::Bool(read_pb_varint(bytes, &mut pos)? != 0),
            _ => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(value)
}

fn decode_mvt_geometry(
    commands: &[u32],
    geom_type: MvtGeomType,
) -> Result<Vec<Vec<(i32, i32)>>, String> {
    let mut parts = Vec::<Vec<(i32, i32)>>::new();
    let mut current = Vec::<(i32, i32)>::new();
    let mut x = 0_i32;
    let mut y = 0_i32;
    let mut index = 0_usize;

    while index < commands.len() {
        let header = commands[index];
        index += 1;
        let command_id = header & 0x7;
        let count = header >> 3;

        match command_id {
            1 => {
                for _ in 0..count {
                    if index + 1 >= commands.len() {
                        return Err("mvt geometry move_to missing arguments".to_string());
                    }
                    x = x.wrapping_add(zigzag_decode_u32(commands[index]));
                    y = y.wrapping_add(zigzag_decode_u32(commands[index + 1]));
                    index += 2;
                    if !current.is_empty() {
                        parts.push(current);
                        current = Vec::new();
                    }
                    current.push((x, y));
                }
            }
            2 => {
                for _ in 0..count {
                    if index + 1 >= commands.len() {
                        return Err("mvt geometry line_to missing arguments".to_string());
                    }
                    x = x.wrapping_add(zigzag_decode_u32(commands[index]));
                    y = y.wrapping_add(zigzag_decode_u32(commands[index + 1]));
                    index += 2;
                    current.push((x, y));
                }
            }
            7 => {
                if geom_type == MvtGeomType::Polygon && !current.is_empty() {
                    let first = current[0];
                    if current.last().copied() != Some(first) {
                        current.push(first);
                    }
                }
            }
            _ => return Err(format!("mvt geometry unknown command {}", command_id)),
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

// --- Protobuf primitives ---

fn zigzag_decode_u32(value: u32) -> i32 {
    ((value >> 1) as i32) ^ (-((value & 1) as i32))
}

fn zigzag_decode_u64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

fn read_packed_u32(bytes: &[u8]) -> Result<Vec<u32>, String> {
    let mut pos = 0_usize;
    let mut out = Vec::new();
    while pos < bytes.len() {
        out.push(read_pb_varint(bytes, &mut pos)? as u32);
    }
    Ok(out)
}

fn read_pb_fixed32(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos + 4 > bytes.len() {
        return Err("unexpected eof reading fixed32".to_string());
    }
    let value = u32::from_le_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
    ]);
    *pos += 4;
    Ok(value)
}

fn read_pb_fixed64(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    if *pos + 8 > bytes.len() {
        return Err("unexpected eof reading fixed64".to_string());
    }
    let value = u64::from_le_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
        bytes[*pos + 4],
        bytes[*pos + 5],
        bytes[*pos + 6],
        bytes[*pos + 7],
    ]);
    *pos += 8;
    Ok(value)
}

fn read_pb_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *pos < bytes.len() {
        let byte = bytes[*pos];
        *pos += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if (byte & 0x80) == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err("varint too long".to_string());
        }
    }
    Err("unexpected eof reading varint".to_string())
}

fn read_pb_len_slice<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a [u8], String> {
    let len = read_pb_varint(bytes, pos)? as usize;
    if *pos + len > bytes.len() {
        return Err("unexpected eof reading length-delimited field".to_string());
    }
    let slice = &bytes[*pos..*pos + len];
    *pos += len;
    Ok(slice)
}

fn skip_pb_field(bytes: &[u8], pos: &mut usize, wire: u8) -> Result<(), String> {
    match wire {
        0 => {
            let _ = read_pb_varint(bytes, pos)?;
            Ok(())
        }
        1 => {
            if *pos + 8 > bytes.len() {
                return Err("unexpected eof skipping 64-bit field".to_string());
            }
            *pos += 8;
            Ok(())
        }
        2 => {
            let len = read_pb_varint(bytes, pos)? as usize;
            if *pos + len > bytes.len() {
                return Err("unexpected eof skipping length-delimited field".to_string());
            }
            *pos += len;
            Ok(())
        }
        5 => {
            if *pos + 4 > bytes.len() {
                return Err("unexpected eof skipping 32-bit field".to_string());
            }
            *pos += 4;
            Ok(())
        }
        _ => Err(format!("unsupported protobuf wire type {}", wire)),
    }
}

fn append_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => {
                out.push_str("\\u");
                out.push_str(&format!("{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
