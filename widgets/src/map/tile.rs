use super::geometry::*;
use super::icons::*;
use super::label::*;
use super::style::*;
use crate::makepad_draw::vector::{
    append_tessellated_geometry, append_tessellated_geometry_decked, tessellate_path_fill,
    LineCap, LineJoin, Tessellator, VVertex,
    VectorPath, VectorRenderParams, VECTOR_FLOATS_PER_VERTEX, VECTOR_ZBIAS_STEP,
};
use crate::makepad_draw::*;
use crate::makepad_platform::makepad_micro_serde::*;
use makepad_fast_inflate::{gzip_decompress_vec, zlib_decompress_vec};
use makepad_mbtile_reader::MbtilesReader;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const OVERPASS_ENDPOINTS: &[&str] = &["https://overpass.kumi.systems/api/interpreter"];
pub const MAX_PENDING_REQUESTS: usize = 2;
pub const MAX_TILE_RETRIES: u8 = 6;
pub const RETRY_BASE_FRAMES: u64 = 30;
pub const RETRY_MAX_FRAMES: u64 = 300;
pub const TILE_CACHE_DIR: &str = "local/tilecache_v4";
pub const TILE_QUERY_PAD: f64 = 0.05;
// Default archive: the curated Europe Shortbread base produced by
// `./download_map.sh convert`. Apps can override per-widget via the
// MapView `mbtiles_path` property (examples/map pins Noord-Holland).
pub const LOCAL_MBTILES_PATH: &str = "local/maps/europe-shortbread.mbtiles";
pub const LOCAL_MBTILES_MIN_ZOOM: u32 = 0;
pub const LOCAL_MBTILES_MAX_ZOOM: u32 = 14;
// Stroke clip padding must stay below the MVT generator's tile buffer (~4
// world px) so cross-boundary ways are cut by OUR clip (detectable, butt-
// capped) rather than ending mid-buffer with a rogue round cap.
pub const ROAD_CLIP_PADDING: f32 = 3.0;
// Fills are clipped to their own tile square (+ tiny overlap against AA
// hairlines) so a tile's buffer fragments never overpaint the neighbor.
pub const FILL_CLIP_OVERLAP: f32 = 0.25;
pub const ROAD_SMOOTH_FACTOR: f32 = 0.0;
pub const BUILDING_OUTLINE_MIN_ZOOM: u32 = 15;
pub const BUILDING_OUTLINE_WIDTH_PX: f32 = 0.9;
pub const EARCUT_MAX_RINGS: usize = 500;

const MVT_INTERNAL_FEATURE_KEY: &str = "__mp_feature";
const MVT_INTERNAL_RING_INDEX_KEY: &str = "__mp_ring";
/// Tilt-mode road ladder: all union casings under all union centers under
/// rails/dashes (legacy strokes get +ROAD_LEGACY_OVER in append) under
/// arrows (0.85). Deck bump (0.30 per 2 m in triangulate) exceeds the
/// casing/center split so an elevated deck occludes grounded centers.
pub const ROAD_UNION_CASING_DEPTH: f32 = 0.10;
pub const ROAD_UNION_CENTER_DEPTH: f32 = 0.20;
const MVT_INTERNAL_FIDX_KEY: &str = "__mp_fidx";
const MVT_INTERNAL_PIDX_KEY: &str = "__mp_pidx";

// --- Tile state types ---

#[derive(Debug)]
pub enum TileLoadState {
    LoadingNetwork,
    LoadingLocal,
    Ready {
        fill_geometry: Option<Geometry>,
        casing_geometry: Option<Geometry>,
        stroke_geometry: Option<Geometry>,
        icon_geometry: Option<Geometry>,
        feature_count: usize,
        labels: Vec<TileLabel>,
        pin_hits: Vec<PinHit>,
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
    /// View-zoom bucket the geometry was styled for; stale buckets stay
    /// drawable while a rebuild is in flight.
    pub bucket: u32,
    /// This bake carries 3D extrusions (buildings/trees/signals).
    pub baked_3d: bool,
    /// Cross-fade state: the replaced generation's geometry stays drawable
    /// underneath while the new one fades in.
    pub fade: Option<TileFade>,
}

#[derive(Debug)]
pub struct TileFade {
    pub started: std::time::Instant,
    /// Render bucket the outgoing geometry was styled for, so its stroke
    /// widths can be corrected while it fades out.
    pub bucket: u32,
    /// This fade is the flat->3D transition: the incoming bake grows its
    /// heights with the fade. 3D->3D rebakes keep full height (alpha-only
    /// crossfade) so zoom regens never replay the animation.
    pub grow_heights: bool,
    pub fill_geometry: Option<Geometry>,
    pub casing_geometry: Option<Geometry>,
    pub stroke_geometry: Option<Geometry>,
    pub icon_geometry: Option<Geometry>,
}

#[derive(Debug)]
pub struct PendingTileRequest {
    pub tile_key: TileKey,
    pub endpoint: &'static str,
}

#[derive(Debug)]
pub enum TileWorkerMessage {
    LocalBatchLoaded {
        style_epoch: u64,
        requested: Vec<TileKey>,
        loaded: Vec<LoadedLocalTile>,
        /// Keys whose tile data exists but failed to decode — retryable,
        /// unlike keys absent from the archive.
        failed: Vec<TileKey>,
    },
    LocalBatchFailed {
        style_epoch: u64,
        requested: Vec<TileKey>,
        error: String,
    },
    NetworkTileParsed {
        style_epoch: u64,
        tile_key: TileKey,
        buffers: TileBuffers,
    },
    NetworkTileParseFailed {
        style_epoch: u64,
        tile_key: TileKey,
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

/// A tappable pin baked into a tile: normalized world position + the
/// attributes the info bubble shows.
#[derive(Debug, Clone)]
pub struct PinHit {
    pub norm: (f64, f64),
    pub info: Vec<(String, String)>,
    /// 3D stalk height of this pin's marker (0 = grounded).
    pub lift_m: f32,
}

#[derive(Debug)]
pub struct TileBuffers {
    pub pin_hits: Vec<PinHit>,
    pub fill_indices: Vec<u32>,
    pub fill_vertices: Vec<f32>,
    pub casing_indices: Vec<u32>,
    pub casing_vertices: Vec<f32>,
    pub stroke_indices: Vec<u32>,
    pub stroke_vertices: Vec<f32>,
    pub icon_indices: Vec<u32>,
    pub icon_vertices: Vec<f32>,
    pub feature_count: usize,
    pub labels: Vec<TileLabel>,
    /// View-zoom bucket this tile's styling was built for.
    pub render_zoom: u32,
}

#[derive(Clone, Debug)]
struct StrokeDrawJob {
    sort_rank: i16,
    style: StrokeStyle,
    points: Vec<(f32, f32)>,
    /// Road-tier union candidate: solid road geometry joins the tier's
    /// union mesh instead of the legacy per-way stroke path.
    union_road: bool,
    /// Per-point deck heights (base_dz join), aligned with `points`.
    dz: Option<Vec<f32>>,
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
    alpha: f32,
    layer_rank: u8,
    is_building: bool,
    pattern: f32,
    /// Bake into the ICON buffer (pass 3, after road strokes): district
    /// tints must colorize the roads too, and fills draw before strokes.
    late: bool,
    /// 3D bridge deck height (m): road polygons and bridge-area slabs at
    /// close zoom lift with the stroke decks instead of lying flat under
    /// the crossing.
    deck_m: f32,
    /// Road-surface polygon: eligible for per-vertex corridor decks when a
    /// baked bridge-dz overlay covers the tile.
    deckable: bool,
    /// This feature's own baked outline profiles (base_dz join): fill
    /// vertices lift by projecting onto these only.
    profiles: Vec<BridgeCorridor>,
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

/// Network/Overpass path: parse the JSON body, project lon/lat to tile-local
/// coordinates, then hand off to the shared feature builder.
pub fn build_tile_buffers_from_body(
    tile_key: TileKey,
    body: &str,
    theme: &CompiledMapTheme,
    render_zoom: u32,
) -> Result<TileBuffers, String> {
    let parsed = OverpassResponse::deserialize_json_lenient(body)
        .map_err(|e| format!("json error at line {} col {}: {}", e.line, e.col, e.msg))?;

    let tile_origin = dvec2(
        tile_key.x as f64 * TILE_SIZE,
        tile_key.y as f64 * TILE_SIZE,
    );
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;

    let mut nodes = HashMap::<i64, (f64, f64)>::new();
    let mut ways = Vec::<WayData>::new();
    let mut tagged_points = Vec::<((f32, f32), HashMap<String, String>)>::new();

    for element in parsed.elements {
        match element.kind.as_str() {
            "node" => {
                if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                    nodes.insert(element.id, (lon, lat));
                    if let Some(tags) = element.tags {
                        let world = lon_lat_to_world(lon, lat, tile_key.z) - tile_origin;
                        tagged_points.push(((world.x as f32, world.y as f32), tags));
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

    let mut tile_ways = Vec::<TileWay>::with_capacity(ways.len());
    for way in ways {
        let projected =
            project_way_points_with_nodes(&way.nodes, &nodes, tile_key, tile_origin, render_scale);
        if projected.len() < 2 {
            continue;
        }
        let points = projected.into_iter().map(|(_, point)| point).collect();
        tile_ways.push(TileWay {
            points,
            tags: way.tags,
            closed: way.closed,
            dz: None,
        });
    }

    Ok(build_tile_buffers_from_features(
        tile_key,
        tile_ways,
        tagged_points,
        theme,
        render_zoom,
        false,
        Vec::new(),
        false,
        true,
        false,
    ))
}

/// Local mbtiles path: decode the MVT protobuf STRAIGHT into tile-local
/// coordinates — no lon/lat round trip, no generated-JSON detour.
/// Render buckets from which 2.5D buildings are baked.
pub const BUILDING_3D_MIN_ZOOM: u32 = 15;

pub fn build_tile_buffers_from_mvt(
    tile_key: TileKey,
    raw_tile_data: &[u8],
    detail_tile_data: Option<&[u8]>,
    bridge_dz_tile_data: Option<&[u8]>,
    bridge_dz_covered: bool,
    overlay_tiles: &[OverlayTileData],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    union_roads: bool,
) -> Result<TileBuffers, String> {
    let have_charger_overlay = overlay_tiles.iter().any(|overlay| overlay.has_chargers);
    let pbf_data = decode_vector_tile_payload(raw_tile_data)?;
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;
    let mut collector = MvtLocalCollector::new(render_scale);
    // Baked base_dz overlay: per-vertex deck heights for this exact base
    // tile's features, joined during collection (no geometry matching).
    if let Some(dz_data) = bridge_dz_tile_data {
        match parse_base_dz_map(dz_data, tile_key) {
            Ok(map) => collector.base_dz = map,
            Err(err) => log!(
                "MapView: base dz tile z{} x{} y{} decode failed: {}",
                tile_key.z,
                tile_key.x,
                tile_key.y,
                err
            ),
        }
    }
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    // Compose micro-POIs (trees, benches, bins…) and, in 2.5D mode, building
    // footprints with real heights from the all-tag detail archive over the
    // shortbread base — skip the extra decode below the zooms that use them.
    let want_buildings = buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM;
    let mut bridge_corridors = Vec::<BridgeCorridor>::new();
    // Bridge corridors want the detail archive from bucket 14 in 3D.
    if render_zoom >= ICON_MIN_ZOOM.min(BUILDING_3D_MIN_ZOOM)
        || want_buildings
        || (buildings_3d && render_zoom >= 14)
    {
        if let Some(detail_data) = detail_tile_data {
            if let Err(err) = merge_detail_features(
                detail_data,
                tile_key,
                render_scale,
                render_zoom >= ICON_MIN_ZOOM,
                want_buildings,
                // Inside baked bridge-dz coverage the solved corridors
                // replace the tag heuristic entirely.
                !bridge_dz_covered,
                &mut collector.points,
                &mut collector.ways,
                &mut bridge_corridors,
            ) {
                log!(
                    "MapView: detail tile z{} x{} y{} decode failed: {}",
                    tile_key.z,
                    tile_key.x,
                    tile_key.y,
                    err
                );
            }
        }
    }

    for overlay in overlay_tiles {
        if let Err(err) = merge_overlay_features(
            overlay,
            tile_key,
            render_scale,
            &mut collector.points,
            &mut collector.ways,
        ) {
            log!(
                "MapView: overlay tile z{} x{} y{} decode failed: {}",
                tile_key.z,
                tile_key.x,
                tile_key.y,
                err
            );
        }
    }
    Ok(build_tile_buffers_from_features(
        tile_key,
        collector.ways,
        collector.points,
        theme,
        render_zoom,
        buildings_3d,
        bridge_corridors,
        bridge_dz_covered,
        union_roads,
        have_charger_overlay,
    ))
}

/// Verbatim-point sink for the bridge-bake overlay: the dz array is
/// per-vertex, so the min-distance simplification of MvtLocalCollector
/// would break the alignment.
struct BridgeDzCollector {
    next_feature_id: u64,
    ways: Vec<TileWay>,
}

impl MvtSink for BridgeDzCollector {
    fn alloc_feature_id(&mut self) -> u64 {
        let id = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.wrapping_add(1).max(1);
        id
    }

    fn add_path(
        &mut self,
        _tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        self.ways.push(TileWay {
            points: points
                .iter()
                .map(|&(x, y)| (x as f32 * scale, y as f32 * scale))
                .collect(),
            tags,
            closed: close,
            dz: None,
        });
    }

    fn add_point(
        &mut self,
        _tile_key: TileKey,
        _extent: u32,
        _point: (i32, i32),
        _tags: HashMap<String, String>,
    ) {
    }
}

/// Decode the base_dz layer of a bake overlay tile into the join map:
/// (source layer, feature index, path index) -> per-raw-vertex deck meters.
fn parse_base_dz_map(
    dz_tile_data: &[u8],
    tile_key: TileKey,
) -> Result<HashMap<(String, u32, u32), Vec<f32>>, String> {
    let pbf_data = decode_vector_tile_payload(dz_tile_data)?;
    let mut collector = BridgeDzCollector { next_feature_id: 1, ways: Vec::new() };
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    let mut map = HashMap::new();
    for way in collector.ways {
        if way.tags.get("layer").map(|v| v.as_str()) != Some("base_dz") {
            continue;
        }
        let (Some(layer), Some(fidx), Some(pidx), Some(dz)) = (
            way.tags.get("L"),
            way.tags.get("F"),
            way.tags.get("P"),
            way.tags.get("dz"),
        ) else {
            continue;
        };
        let (Ok(fidx), Ok(pidx)) = (fidx.parse::<u32>(), pidx.parse::<u32>()) else {
            continue;
        };
        let decks: Vec<f32> = dz
            .split(',')
            .filter_map(|v| v.parse::<f32>().ok())
            .map(|dm| (dm * 0.1).max(0.0))
            .collect();
        map.insert((layer.clone(), fidx, pidx), decks);
    }
    Ok(map)
}

/// Decode a bridge-bake overlay tile into per-point-deck corridors. Tags:
/// dz = comma-joined decimeters per vertex, hw = corridor half-width meters.
fn parse_bridge_dz_corridors(
    dz_tile_data: &[u8],
    tile_key: TileKey,
) -> Result<Vec<BridgeCorridor>, String> {
    let pbf_data = decode_vector_tile_payload(dz_tile_data)?;
    let mut collector = BridgeDzCollector { next_feature_id: 1, ways: Vec::new() };
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    // Tile-local units per meter at this tile's latitude.
    let tile_span_m = {
        let n = (1u64 << tile_key.z.min(30)) as f64;
        let merc_y = 1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n;
        let lat = (std::f64::consts::PI * merc_y).sinh().atan();
        40_075_016.686 * lat.cos() / n
    };
    let units_per_m = (TILE_SIZE / tile_span_m.max(1.0)) as f32;
    let mut corridors = Vec::new();
    for way in collector.ways {
        if way.tags.get("layer").map(|v| v.as_str()) != Some("bridge_dz") {
            continue;
        }
        let Some(dz_tag) = way.tags.get("dz") else {
            continue;
        };
        let decks: Vec<f32> = dz_tag
            .split(',')
            .filter_map(|v| v.parse::<f32>().ok())
            .map(|dm| (dm * 0.1).max(0.0))
            .collect();
        if decks.len() != way.points.len() {
            log!(
                "MapView: bridge dz way point/deck mismatch ({} vs {})",
                way.points.len(),
                decks.len()
            );
            continue;
        }
        let half_width_m = way
            .tags
            .get("hw")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(5.0);
        corridors.push(BridgeCorridor {
            points: way.points,
            decks,
            half_width: (half_width_m * units_per_m).max(2.0),
            solved: true,
        });
    }
    Ok(corridors)
}

/// Merge features from a geodata overlay tile (layers.md track: chargers,
/// transit, nature, districts…). The MVT layer name arrives as the "layer"
/// tag and drives styling. Ancestor tiles (overlay maxzoom below the
/// requested zoom) are scaled into this tile's local space and rely on the
/// existing fill/stroke clipping; points get a bounds filter here.
fn merge_overlay_features(
    overlay: &OverlayTileData,
    tile_key: TileKey,
    render_scale: f32,
    points: &mut Vec<((f32, f32), HashMap<String, String>)>,
    ways: &mut Vec<TileWay>,
) -> Result<(), String> {
    let pbf_data = decode_vector_tile_payload(&overlay.raw)?;
    let mut collector = MvtLocalCollector::new(render_scale);
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    let scale = (1u32 << overlay.shift) as f32;
    let offset_x = overlay.quadrant_x as f32 * TILE_SIZE as f32;
    let offset_y = overlay.quadrant_y as f32 * TILE_SIZE as f32;
    let transform = |p: (f32, f32)| (p.0 * scale - offset_x, p.1 * scale - offset_y);
    for (point, tags) in collector.points {
        let point = transform(point);
        if point.0 < -32.0
            || point.1 < -32.0
            || point.0 > TILE_SIZE as f32 + 32.0
            || point.1 > TILE_SIZE as f32 + 32.0
        {
            continue;
        }
        if overlay.filter != 0
            && tags.get("layer").map(|v| v.as_str()) == Some("chargers")
        {
            let kw = tags
                .get("max_kw")
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0);
            let is_fast = kw >= 50.0;
            if (overlay.filter == 1) != is_fast {
                continue;
            }
        }
        points.push((point, tags));
    }
    for mut way in collector.ways {
        for point in way.points.iter_mut() {
            *point = transform(*point);
        }
        ways.push(way);
    }
    Ok(())
}

/// Merge whitelisted features from a detail-archive tile: micro-POI points
/// retagged into the synthetic `micro_pois` layer (icons only — the label
/// extractor ignores that layer, so base-poi labels are never duplicated),
/// and in 2.5D mode building polygons retagged `detail_buildings`.
#[allow(clippy::too_many_arguments)]
fn merge_detail_features(
    detail_data: &[u8],
    tile_key: TileKey,
    render_scale: f32,
    want_points: bool,
    want_buildings: bool,
    collect_corridors: bool,
    points: &mut Vec<((f32, f32), HashMap<String, String>)>,
    ways: &mut Vec<TileWay>,
    corridors: &mut Vec<BridgeCorridor>,
) -> Result<(), String> {
    let pbf_data = decode_vector_tile_payload(detail_data)?;
    let mut collector = MvtLocalCollector::new(render_scale);
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    let render_zoom = tile_key.z as f32 + render_scale.max(1e-6).log2();
    for way in &collector.ways {
        if !collect_corridors {
            break;
        }
        if way.closed || way.points.len() < 2 {
            continue;
        }
        let tags = &way.tags;
        if tags.get("layer").map(|v| v.as_str()) != Some("osm_lines") {
            continue;
        }
        let bridge = tags.get("bridge").map(|v| v.as_str()).unwrap_or("");
        if !(bridge == "yes" || bridge == "viaduct") {
            continue;
        }
        if !(tags.contains_key("highway") || tags.contains_key("railway")) {
            continue;
        }
        // Real OSM layer survives as osm_layer (plain `layer` is shadowed
        // by the MVT layer name). No layer = low crossing (canal bridge).
        let osm_layer = tags
            .get("osm_layer")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let deck_m = if osm_layer >= 1.0 {
            5.5 * osm_layer.min(3.0)
        } else {
            2.5
        };
        // Corridor width from the way's own width tag when present. Tiles
        // are TILE_SIZE (256) units across, whatever the source extent.
        let tile_span_m = {
            let n = (1u64 << tile_key.z.min(30)) as f64;
            let merc_y = 1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n;
            let lat = (std::f64::consts::PI * merc_y).sinh().atan();
            40_075_016.686 * lat.cos() / n
        };
        let units_per_m = (TILE_SIZE / tile_span_m.max(1.0)) as f32;
        let half_width = tags
            .get("width")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|w| (w * 0.75).clamp(4.0, 14.0))
            .unwrap_or(7.0)
            * units_per_m;
        corridors.push(BridgeCorridor {
            decks: vec![deck_m; way.points.len()],
            points: way.points.clone(),
            half_width: half_width.max(0.5),
            solved: false,
        });
    }
    if want_points {
        for (point, mut tags) in collector.points {
            if tags.get("layer").map(|value| value.as_str()) != Some("osm_points") {
                continue;
            }
            // Attraction nodes (zoo animals) carry a label with no icon.
            let is_attraction_node = tags.contains_key("name")
                && (tags.contains_key("attraction") || tags.contains_key("zoo"));
            match micro_icon_for_tags(&tags) {
                Some((icon, _)) => {
                    if render_zoom < micro_icon_min_zoom(icon) {
                        continue;
                    }
                }
                None => {
                    if !is_attraction_node || render_zoom < 15.5 {
                        continue;
                    }
                }
            }
            tags.insert("layer".to_string(), "micro_pois".to_string());
            points.push((point, tags));
        }
    }
    // Station/stop platforms render as gray polygons from z15.5 in both
    // 2D and 3D modes; buildings only when the 3D pass wants them.
    let want_platforms = render_zoom >= 15.5;
    if want_buildings || want_platforms || want_points {
        for mut way in collector.ways {
            // Polygon-anchored POIs (parking lots and garages, shops and
            // offices mapped on their building) icon at the centroid like
            // carto; the icon-collision pass dedups against any base node.
            if want_points && way.closed {
                // Underground garages span whole blocks; carto shows their
                // entrance node, not a centroid P in the middle of nowhere.
                let underground =
                    way.tags.get("parking").map(|v| v.as_str()) == Some("underground");
                if let Some((icon, _)) = micro_icon_for_tags(&way.tags).filter(|_| !underground) {
                    if render_zoom >= micro_icon_min_zoom(icon) && way.points.len() >= 3 {
                        let mut tags = way.tags.clone();
                        tags.insert("layer".to_string(), "micro_pois".to_string());
                        points.push((ring_centroid(&way.points), tags));
                    }
                }
            }
            // Plain building ways AND assembled multipolygon relations
            // (palaces, courtyarded blocks) both carry building geometry.
            let from_polygons = matches!(
                way.tags.get("layer").map(|value| value.as_str()),
                Some("osm_polygons") | Some("osm_relation_polygons")
            );
            // Pedestrian squares mapped as highway=pedestrian + area=yes
            // stay in osm_lines (highway ways don't classify as polygons
            // at conversion). area=yes MEANS polygon, so close the ring
            // unconditionally — tile clipping can leave it open.
            if !from_polygons {
                // Walls, fences and hedges draw as thin barrier lines
                // (the dark perimeter around Artis is its wall).
                if let Some(barrier) = way.tags.get("barrier") {
                    if want_platforms
                        && matches!(
                            barrier.as_str(),
                            "wall" | "fence" | "retaining_wall" | "city_wall" | "hedge"
                        )
                    {
                        way.tags
                            .insert("layer".to_string(), "barrier_line".to_string());
                        ways.push(way);
                    }
                    continue;
                }
                let is_ped_area = tag_is_truthy(&way.tags, "area")
                    && matches!(
                        way.tags.get("highway").map(|v| v.as_str()),
                        Some("pedestrian" | "footway")
                    );
                // Attractions are areas by convention; clipping may have
                // opened the ring, so no first==last requirement.
                let is_attraction_ring = way.tags.contains_key("name")
                    && (way.tags.contains_key("attraction")
                        || way.tags.contains_key("zoo")
                        || way.tags.get("tourism").map(|v| v.as_str()) == Some("attraction"));
                let target_layer = if is_ped_area {
                    Some("street_polygons")
                } else if is_attraction_ring {
                    Some("attraction_area")
                } else {
                    None
                };
                if let Some(layer) = target_layer {
                    if want_platforms && way.points.len() >= 3 {
                        if way.points.first() != way.points.last() {
                            let first = way.points[0];
                            way.points.push(first);
                        }
                        way.closed = true;
                        way.tags.insert("layer".to_string(), layer.to_string());
                        ways.push(way);
                    }
                }
                continue;
            }
            // osm_lines rings arrive as LineStrings, so `closed` is only
            // set for real Polygon geometry — detect implicit closure.
            let ring_closed = way.closed
                || (way.points.len() >= 4 && way.points.first() == way.points.last());
            if !ring_closed {
                continue;
            }
            let is_platform = way.tags.get("railway").map(|v| v.as_str()) == Some("platform")
                || way.tags.get("public_transport").map(|v| v.as_str()) == Some("platform");
            if is_platform {
                if want_platforms {
                    way.tags.insert("layer".to_string(), "platforms".to_string());
                    ways.push(way);
                }
                continue;
            }
            // Small green patches (verges, lawns) are generalized away in
            // the z14 base tiles; at street zoom the detail archive fills
            // them back in. Bigger landuse stays with the base tile.
            let is_green_patch = matches!(
                way.tags.get("landuse").map(|v| v.as_str()),
                Some("grass" | "village_green" | "flowerbed" | "meadow")
            ) || matches!(
                way.tags.get("leisure").map(|v| v.as_str()),
                Some("garden")
            ) || matches!(
                way.tags.get("natural").map(|v| v.as_str()),
                Some("scrub" | "heath" | "shrubbery" | "sand" | "beach" | "shingle")
            );
            // Zoo perimeter draws carto's purple boundary line.
            if matches!(
                way.tags.get("tourism").map(|v| v.as_str()),
                Some("zoo" | "theme_park")
            ) {
                if want_platforms {
                    way.tags
                        .insert("layer".to_string(), "tourism_boundary".to_string());
                    ways.push(way);
                }
                continue;
            }
            let is_building = way
                .tags
                .get("building")
                .is_some_and(|value| value != "no");
            let is_building_part = way
                .tags
                .get("building:part")
                .is_some_and(|value| value != "no");
            // Named zoo enclosures / attractions label at their centroid
            // (and fill if they carry a surface like sand). Famous BUILDINGS
            // also carry tourism=attraction (Westerkerk, Munttoren…) — in 3D
            // mode they must fall through to the extrusion path, not get
            // swallowed as a flat attraction fill.
            let is_attraction = way.tags.contains_key("name")
                && (way.tags.contains_key("attraction")
                    || way.tags.contains_key("zoo")
                    || way.tags.get("tourism").map(|v| v.as_str()) == Some("attraction"))
                && !(want_buildings && (is_building || is_building_part));
            if is_attraction {
                if want_platforms {
                    way.tags
                        .insert("layer".to_string(), "attraction_area".to_string());
                    ways.push(way);
                }
                continue;
            }
            if is_green_patch {
                if want_platforms {
                    way.tags.insert("layer".to_string(), "detail_land".to_string());
                    ways.push(way);
                }
                continue;
            }
            // Pedestrian squares (Hella Haasseplein) are polygons the z14
            // base generalizes away; route them into the existing street-
            // area pipeline so fill, rank and labels all apply.
            let is_pedestrian_area = matches!(
                way.tags.get("highway").map(|v| v.as_str()),
                Some("pedestrian" | "footway")
            ) || way.tags.get("place").map(|v| v.as_str()) == Some("square");
            if is_pedestrian_area {
                if want_platforms {
                    way.tags
                        .insert("layer".to_string(), "street_polygons".to_string());
                    ways.push(way);
                }
                continue;
            }
            if !want_buildings {
                continue;
            }
            if !is_building && !is_building_part {
                continue;
            }
            // Underground volumes (metro halls mapped as building:part,
            // parking cellars) must never extrude above ground.
            if way.tags.get("location").map(|v| v.as_str()) == Some("underground")
                || way
                    .tags
                    .get("osm_layer")
                    .is_some_and(|value| value.starts_with('-'))
            {
                continue;
            }
            way.tags
                .insert("layer".to_string(), "detail_buildings".to_string());
            ways.push(way);
        }
    }
    Ok(())
}

/// Per-icon zoom gates, carto-style: doors only when you could walk
/// through one, signals/chargers at street level.
fn micro_icon_min_zoom(icon: &str) -> f32 {
    match icon {
        "entrance" => 18.0,
        "traffic_signals" | "charger" | "dot" => 16.5,
        "parking" => 15.5,
        _ => 0.0,
    }
}

/// ADAPTIVE Chaikin corner-cutting: only vertices whose adjacent segments
/// are both short (dense curve sampling from tile quantization) get cut;
/// sparse vertices are real corners — street grids must stay sharp or
/// roads round through buildings and detach from their bridges.
fn chaikin_smooth(points: &[(f32, f32)], rounds: usize, cut_below: f32) -> Vec<(f32, f32)> {
    if rounds == 0 || points.len() < 3 || points.len() > 2000 {
        return points.to_vec();
    }
    let closed = points.first() == points.last();
    let mut pts = if closed {
        points[..points.len() - 1].to_vec()
    } else {
        points.to_vec()
    };
    let cut_below_sq = cut_below * cut_below;
    let seg_sq = |a: (f32, f32), b: (f32, f32)| {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        dx * dx + dy * dy
    };
    let lerp =
        |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
    for _ in 0..rounds {
        if pts.len() < 3 {
            break;
        }
        let n = pts.len();
        let mut out = Vec::with_capacity(n * 2 + 2);
        let range = if closed { 0..n } else { 1..n - 1 };
        if !closed {
            out.push(pts[0]);
        }
        for i in range {
            let prev = pts[(i + n - 1) % n];
            let v = pts[i];
            let next = pts[(i + 1) % n];
            // Only gentle bends get cut (turn < ~30 degrees): densely
            // sampled quay curves still carry SHARP corners at bridge
            // junctions between short segments — rounding those pulls
            // the road through the corner buildings.
            let a = (v.0 - prev.0, v.1 - prev.1);
            let b = (next.0 - v.0, next.1 - v.1);
            let dot = (a.0 * b.0 + a.1 * b.1) as f64;
            let len = ((a.0 as f64 * a.0 as f64 + a.1 as f64 * a.1 as f64)
                * (b.0 as f64 * b.0 as f64 + b.1 as f64 * b.1 as f64))
                .sqrt();
            let gentle = len > 1e-12 && dot / len > 0.866;
            if gentle && seg_sq(prev, v) < cut_below_sq && seg_sq(v, next) < cut_below_sq {
                out.push(lerp(v, prev, 0.25));
                out.push(lerp(v, next, 0.25));
            } else {
                out.push(v);
            }
        }
        if !closed {
            out.push(*pts.last().unwrap());
        }
        pts = out;
    }
    if closed {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    pts
}

/// `chaikin_smooth` with the bridge-dz channel riding along: dz lerps with
/// the same 0.25 corner cuts so lifted geometry keeps its ramp profile
/// through the smoothing.
fn chaikin_smooth_dz(
    points: &[(f32, f32)],
    dz: Option<&[f32]>,
    rounds: usize,
    cut_below: f32,
) -> (Vec<(f32, f32)>, Option<Vec<f32>>) {
    let Some(dz) = dz else {
        return (chaikin_smooth(points, rounds, cut_below), None);
    };
    if rounds == 0 || points.len() < 3 || points.len() > 2000 || dz.len() != points.len() {
        return (points.to_vec(), Some(dz.to_vec()));
    }
    let closed = points.first() == points.last();
    let mut pts: Vec<(f32, f32, f32)> = points
        .iter()
        .zip(dz)
        .map(|(&(x, y), &d)| (x, y, d))
        .collect();
    if closed {
        pts.pop();
    }
    let cut_below_sq = cut_below * cut_below;
    let seg_sq = |a: (f32, f32, f32), b: (f32, f32, f32)| {
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        dx * dx + dy * dy
    };
    let lerp = |a: (f32, f32, f32), b: (f32, f32, f32), t: f32| {
        (
            a.0 + (b.0 - a.0) * t,
            a.1 + (b.1 - a.1) * t,
            a.2 + (b.2 - a.2) * t,
        )
    };
    for _ in 0..rounds {
        if pts.len() < 3 {
            break;
        }
        let n = pts.len();
        let mut out = Vec::with_capacity(n * 2 + 2);
        let range = if closed { 0..n } else { 1..n - 1 };
        if !closed {
            out.push(pts[0]);
        }
        for i in range {
            let prev = pts[(i + n - 1) % n];
            let v = pts[i];
            let next = pts[(i + 1) % n];
            let a = (v.0 - prev.0, v.1 - prev.1);
            let b = (next.0 - v.0, next.1 - v.1);
            let dot = (a.0 * b.0 + a.1 * b.1) as f64;
            let len = ((a.0 as f64 * a.0 as f64 + a.1 as f64 * a.1 as f64)
                * (b.0 as f64 * b.0 as f64 + b.1 as f64 * b.1 as f64))
                .sqrt();
            let gentle = len > 1e-12 && dot / len > 0.866;
            if gentle && seg_sq(prev, v) < cut_below_sq && seg_sq(v, next) < cut_below_sq {
                out.push(lerp(v, prev, 0.25));
                out.push(lerp(v, next, 0.25));
            } else {
                out.push(v);
            }
        }
        if !closed {
            out.push(*pts.last().unwrap());
        }
        pts = out;
    }
    if closed {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    (
        pts.iter().map(|&(x, y, _)| (x, y)).collect(),
        Some(pts.iter().map(|&(_, _, d)| d).collect()),
    )
}

/// Building height in meters from OSM tags: explicit `height`, else
/// `building:levels` × 3m + roof allowance, else a modest default.
fn building_height_m(tags: &HashMap<String, String>) -> f32 {
    if let Some(height) = tags.get("height") {
        let digits: String = height
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(h) = digits.parse::<f32>() {
            return h.clamp(2.0, 220.0);
        }
    }
    if let Some(levels) = tags.get("building:levels") {
        if let Ok(n) = levels.trim().parse::<f32>() {
            return (n * 3.0 + 2.0).clamp(3.0, 220.0);
        }
    }
    8.0
}

/// Base height (bottom of the volume) for building:part features:
/// `min_height` meters, else `building:min_level` x 3m.
fn building_min_height_m(tags: &HashMap<String, String>) -> f32 {
    if let Some(min_height) = tags.get("min_height") {
        let digits: String = min_height
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(h) = digits.parse::<f32>() {
            return h.clamp(0.0, 220.0);
        }
    }
    if let Some(levels) = tags.get("building:min_level") {
        if let Ok(n) = levels.trim().parse::<f32>() {
            return (n * 3.0).clamp(0.0, 220.0);
        }
    }
    0.0
}

/// Ray-cast point-in-polygon on a tile-local ring.
fn point_in_ring(point: (f32, f32), ring: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        if (yi > point.1) != (yj > point.1) {
            let x_cross = xi + (point.1 - yi) / (yj - yi) * (xj - xi);
            if point.0 < x_cross {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// A low-poly SPHERE: horizontal rings in map units, per-vertex height in
/// param4 — the tilt shader's per-meter lift renders a true ball silhouette
/// (stacked flat discs read as separate pancakes).
#[allow(clippy::too_many_arguments)]
fn append_ball(
    center: (f32, f32),
    radius_units: f32,
    radius_m: f32,
    center_h_m: f32,
    color: [f32; 4],
    segs: u32,
    rings: u32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let (segs, rings) = (segs.max(3), rings.max(2));
    // Phong-ish per-vertex lighting (Gouraud across the triangles): the
    // same NW sun as the building walls plus a tight glossy highlight, so
    // canopies and lights read as lit volumes instead of flat blobs.
    // Map coords: x east, y SOUTH (screen down), z up.
    let light = {
        let (lx, ly, lz) = (-0.55f32, -0.835, 1.05);
        let len = (lx * lx + ly * ly + lz * lz).sqrt();
        (lx / len, ly / len, lz / len)
    };
    let view = {
        let (vx, vy, vz) = (0.0f32, 0.62, 0.79);
        let len = (vx * vx + vy * vy + vz * vz).sqrt();
        (vx / len, vy / len, vz / len)
    };
    let half = {
        let (hx, hy, hz) = (light.0 + view.0, light.1 + view.1, light.2 + view.2);
        let len = (hx * hx + hy * hy + hz * hz).sqrt();
        (hx / len, hy / len, hz / len)
    };
    let lit = |nx: f32, ny: f32, nz: f32| -> [f32; 4] {
        let ndl = (nx * light.0 + ny * light.1 + nz * light.2).max(0.0);
        let ndh = (nx * half.0 + ny * half.1 + nz * half.2).max(0.0);
        let diffuse = 0.45 + 0.55 * ndl;
        let spec = ndh.powi(32) * 0.85;
        [
            (color[0] * diffuse + spec).min(1.0),
            (color[1] * diffuse + spec).min(1.0),
            (color[2] * diffuse + spec).min(1.0),
            color[3],
        ]
    };
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    let mut push_vertex = |x: f32, y: f32, h: f32, shade: [f32; 4]| {
        out_vertices.extend_from_slice(&[
            x, y, 0.5, 1.0, shade[0], shade[1], shade[2], shade[3], 1e6, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, h, 0.05, 24.0, *zbias,
        ]);
    };
    // rings from south pole (phi -90) to north pole (phi +90)
    for ring in 0..=rings {
        let phi = (ring as f32 / rings as f32 - 0.5) * std::f32::consts::PI;
        let ring_r = radius_units * phi.cos();
        let h = center_h_m + radius_m * phi.sin();
        for seg in 0..segs {
            let a = seg as f32 / segs as f32 * std::f32::consts::TAU;
            let shade = lit(phi.cos() * a.cos(), phi.cos() * a.sin(), phi.sin());
            push_vertex(
                center.0 + a.cos() * ring_r,
                center.1 + a.sin() * ring_r,
                h,
                shade,
            );
        }
    }
    for ring in 0..rings {
        for seg in 0..segs {
            let next = (seg + 1) % segs;
            let a = base + ring * segs + seg;
            let b = base + ring * segs + next;
            let c = base + (ring + 1) * segs + seg;
            let d = base + (ring + 1) * segs + next;
            out_indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

/// One flat-shaded wall quad: two ground vertices and two roof vertices
/// whose height rides in param4 for the tilt shader to lift.
fn append_wall_quad(
    a: (f32, f32),
    b: (f32, f32),
    base_m: f32,
    height_m: f32,
    color: [f32; 4],
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (p, h) in [(a, base_m), (b, base_m), (b, height_m), (a, height_m)] {
        out_vertices.extend_from_slice(&[
            p.0, p.1, 0.5, 1.0, color[0], color[1], color[2], color[3], 1e6, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, h, 0.05, 90.0, *zbias,
        ]);
    }
    out_indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    *zbias += VECTOR_ZBIAS_STEP;
}

/// A way in tile-local coordinates ready for styling/tessellation.
pub struct TileWay {
    pub points: Vec<(f32, f32)>,
    pub tags: HashMap<String, String>,
    pub closed: bool,
    /// Baked per-vertex deck height (m), aligned with `points` — from the
    /// base_dz overlay join. The way lifts off its own profile.
    pub dz: Option<Vec<f32>>,
}

fn build_tile_buffers_from_features(
    tile_key: TileKey,
    tile_ways: Vec<TileWay>,
    tagged_points: Vec<((f32, f32), HashMap<String, String>)>,
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    bridge_corridors: Vec<BridgeCorridor>,
    bridge_dz_covered: bool,
    union_roads: bool,
    have_charger_overlay: bool,
) -> TileBuffers {
    // How much this tile gets magnified on screen at the styled view zoom.
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;
    // Every open way with baked dz becomes its own lift profile: strokes
    // and arrows match against these (exact same geometry, tight reach) —
    // never against other ways.
    let own_profiles: Vec<BridgeCorridor> = tile_ways
        .iter()
        .filter(|way| !way.closed)
        .filter_map(|way| {
            way.dz.as_ref().map(|dz| BridgeCorridor {
                points: way.points.clone(),
                decks: dz.clone(),
                half_width: 2.0,
                solved: true,
            })
        })
        .collect();
    // Inside baked coverage: only own profiles lift strokes. Outside:
    // the tag-heuristic corridor soup.
    let stroke_corridors_available = if bridge_dz_covered {
        !own_profiles.is_empty()
    } else {
        !bridge_corridors.is_empty()
    };
    // Converts "screen px at render_zoom" into tile-local units.
    let zoom_mult = zoom_width_mult(render_zoom);
    let px_to_units = 1.0 / render_scale;
    let aa_units = 1.0 / render_scale;
    let tolerance = DEFAULT_FLATTEN_TOLERANCE / render_scale;

    let mut labels = Vec::<TileLabel>::new();
    let mut pin_hits = Vec::<PinHit>::new();
    let mut icon_jobs =
        Vec::<((f32, f32), &'static IconMesh, u8, u8, f32, u8, f32, f32, f32, f32)>::new();
    let mut tree_points_3d = Vec::<(f32, f32)>::new();
    let mut signal_points_3d = Vec::<(f32, f32)>::new();
    for (point, tags) in &tagged_points {
        let mut label_point = *point;
        let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
        // Overlay points (chargers, transit stops) show earlier than the
        // dense base-POI iconography. Chargers tier by power: an ultra-fast
        // site matters at road-trip zoom, a street post doesn't.
        let icon_zoom_floor = match layer {
            "chargers" => {
                let kw = tags
                    .get("max_kw")
                    .and_then(|value| value.parse::<f64>().ok())
                    .unwrap_or(0.0);
                if kw >= 150.0 {
                    8
                } else if kw >= 50.0 {
                    10
                } else {
                    12
                }
            }
            "stops" => 13,
            _ => ICON_MIN_ZOOM,
        };
        if render_zoom >= icon_zoom_floor {
            if let Some((icon_name, color_class)) = icon_for_tags(tags) {
                if let Some(mesh) = icon_mesh(icon_name) {
                    // Doors and generic dots yield to real symbols in the
                    // collision pass (a recycling point must not lose to
                    // the building entrance next to it).
                    // Chargers place before everything (EV navigator) and
                    // are never collided away by shop/POI symbols.
                    let priority = match icon_name {
                        // Overlay charger pins are never collided away —
                        // base-map charging_station icons yield to them.
                        "charger" if layer == "chargers" => 0,
                        "charger" => 2,
                        "entrance" => 3,
                        "dot" => 2,
                        _ => 1,
                    };
                    // Micro street furniture packs tighter than shop/POI
                    // symbols — a bench must not knock out the tree row.
                    let dist_factor = match icon_name {
                        "tree" | "bench" | "waste_basket" | "recycling" | "dot"
                        | "bicycle" => 0.45f32,
                        _ => 1.0,
                    };
                    let charger_kw = (layer == "chargers")
                        .then(|| {
                            tags.get("max_kw")
                                .and_then(|value| value.parse::<f64>().ok())
                                .unwrap_or(0.0)
                        })
                        .unwrap_or(0.0);
                    // Stall count (OCPI EVSEs) rides along for the in-pin
                    // "kW/stalls" text at close zooms.
                    let charger_stalls = (layer == "chargers")
                        .then(|| {
                            tags.get("evses")
                                .and_then(|value| value.parse::<f64>().ok())
                                .unwrap_or(0.0)
                        })
                        .unwrap_or(0.0);
                    // Chargers render as Tesla-style pin badges: wide badge
                    // (bolt + kW text inside) for fast sites, small badge
                    // for street AC.
                    let two_tone = match icon_name {
                        "tree" => 1u8,
                        "charger" if charger_kw >= 50.0 => 2,
                        "charger" => 3,
                        _ => 0,
                    };
                    let mesh = match two_tone {
                        2 => icon_mesh("charger_pin_fast").unwrap_or(mesh),
                        3 => icon_mesh("charger_pin_ac").unwrap_or(mesh),
                        _ => mesh,
                    };
                    // The icon's own zoom floor rides into the vertex data
                    // (param4): the shader hides the icon the instant the
                    // LIVE view zoom drops below it, so stale deeper-bucket
                    // tiles never flash markers while zooming out.
                    // Overlay layers (chargers, stops) use their TIER floor;
                    // micro_icon_min_zoom("charger") is the 16.5 street-level
                    // gate for BASE-map charging posts and must not apply to
                    // overlay pins (it hid every pin below z16 — the
                    // "chargers disappeared" bug).
                    let zoom_floor = if layer == "chargers" || layer == "stops" {
                        icon_zoom_floor as f32
                    } else {
                        micro_icon_min_zoom(icon_name).max(icon_zoom_floor as f32)
                    };
                    // 3D mode: markers fly on stalks above the skyline —
                    // chargers highest, then shops/cafés (base pois), then
                    // transit stops. Street furniture (benches, entrances,
                    // micro POIs) stays on the ground where it belongs.
                    let pin_lift_m = if buildings_3d {
                        if layer == "chargers" {
                            if charger_kw >= 50.0 { 26.0f32 } else { 20.0 }
                        } else if layer == "stops" {
                            12.0
                        } else if tags.get("layer").map(|v| v.as_str()) == Some("pois") {
                            18.0
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    // In 3D mode trees become little REAL 3D trees (trunk +
                    // canopy blob lifted by the building height mechanism)
                    // instead of flat billboard discs.
                    // With the charger overlay active the base-map
                    // charging_station icons are duplicates of overlay
                    // pins — drop them instead of letting them collide.
                    if icon_name == "charger" && layer != "chargers" && have_charger_overlay {
                        continue;
                    }
                    if buildings_3d && icon_name == "tree" {
                        tree_points_3d.push(*point);
                        continue;
                    }
                    if buildings_3d && icon_name == "traffic_signals" {
                        signal_points_3d.push(*point);
                        continue;
                    }
                    icon_jobs.push((
                        *point,
                        mesh,
                        color_class,
                        priority,
                        dist_factor,
                        two_tone,
                        charger_kw as f32,
                        charger_stalls as f32,
                        zoom_floor,
                        pin_lift_m,
                    ));
                    if two_tone == 2 || two_tone == 3 {
                        // Tappable: record position + info for the bubble.
                        let world = (1u32 << tile_key.z) as f64;
                        let norm = (
                            (tile_key.x as f64 + point.0 as f64 / TILE_SIZE) / world,
                            (tile_key.y as f64 + point.1 as f64 / TILE_SIZE) / world,
                        );
                        let mut info: Vec<(String, String)> = Vec::new();
                        for key in ["name", "operator", "city", "max_kw", "evses", "connectors"] {
                            if let Some(value) = tags.get(key) {
                                if !value.trim().is_empty() {
                                    info.push((key.to_string(), value.clone()));
                                }
                            }
                        }
                        pin_hits.push(PinHit { norm, info, lift_m: 0.0 });
                    }
                    if two_tone == 2 {
                        // In-pin text via the NORMAL text renderer (drawn in
                        // the post-icon pin phase, billboard-anchored):
                        // Tesla pins show the stall count (the kW is implied
                        // by the brand, like the Tesla app), other brands
                        // show the peak kW.
                        let is_tesla = tags
                            .get("operator")
                            .or_else(|| tags.get("brand"))
                            .is_some_and(|v| v.to_lowercase().contains("tesla"));
                        let pin_text = if is_tesla && charger_stalls >= 1.0 {
                            format!("{:.0}", charger_stalls.min(99.0))
                        } else if charger_kw >= 1.0 {
                            format!("{:.0}", charger_kw.min(999.0))
                        } else {
                            String::new()
                        };
                        if !pin_text.is_empty() {
                            labels.push(TileLabel {
                                text: pin_text,
                                priority: 1,
                                source_layer: "chargers".to_string(),
                                road_kind: format!(
                                    "chp{}_{:.0}x{:.0}",
                                    icon_zoom_floor,
                                    point.0 * 4.0,
                                    point.1 * 4.0
                                ),
                                color_class: crate::map::label::LABEL_CLASS_PIN,
                                path_points: crate::map::label::point_label_path_pub((
                                    point.0, point.1,
                                )),
                                name_key: String::new(),
                                bbox: (0.0, 0.0, 0.0, 0.0),
                                        lift_m: 0.0,
                            });
                        }
                        // brand reads below the pin from z13; the kW digits
                        // are part of the icon composite itself.
                        if render_zoom >= 13 {
                            if let Some(operator) = tags.get("operator") {
                                let brand = operator
                                    .split([' ', '/'])
                                    .next()
                                    .unwrap_or("")
                                    .to_string();
                                if brand.len() >= 2 {
                                    labels.push(TileLabel {
                                        text: brand,
                                        priority: 3,
                                        source_layer: "charger_brand".to_string(),
                                        road_kind: format!(
                                            "chb{:.0}x{:.0}",
                                            point.0 * 4.0,
                                            point.1 * 4.0
                                        ),
                                        color_class: if operator.to_lowercase().contains("tesla")
                                        {
                                            crate::map::label::LABEL_CLASS_HEALTH
                                        } else {
                                            crate::map::label::LABEL_CLASS_AMENITY
                                        },
                                        // Anchor AT the charger point; the
                                        // below-the-pin offset is applied in
                                        // SCREEN space at candidate time so it
                                        // doesn't tilt-compress or orbit the
                                        // billboard pin when the camera moves.
                                        path_points: crate::map::label::point_label_path_pub((
                                            point.0, point.1,
                                        )),
                                        name_key: String::new(),
                                        bbox: (0.0, 0.0, 0.0, 0.0),
                                        lift_m: 0.0,
                                    });
                                }
                            }
                        }
                    } else {
                        // text sits below the symbol, carto-style
                        label_point.1 += 11.0 / render_scale;
                    }
                }
            }
        }
        if let Some(label) = extract_point_label(tags, label_point) {
            labels.push(label);
        }
    }
    icon_jobs.sort_by_key(|job| job.3);

    // Icon-vs-icon collision: keep the first symbol in any ~icon-sized
    // neighborhood (dense shopping streets otherwise stack into a carpet).
    let icon_min_dist = (ICON_SIZE_PX + 3.0) / render_scale;
    let icon_min_dist_sq = icon_min_dist * icon_min_dist;
    let mut accepted_icons = Vec::<(f32, f32)>::new();
    icon_jobs.retain(|(point, _, _, _, dist_factor, _, _, _, _, _)| {
        let collides = accepted_icons.iter().any(|other| {
            let dx = other.0 - point.0;
            let dy = other.1 - point.1;
            dx * dx + dy * dy < icon_min_dist_sq * dist_factor * dist_factor
        });
        if collides {
            false
        } else {
            accepted_icons.push(*point);
            true
        }
    });

    let mut path = VectorPath::new();
    let mut tess = Tessellator::default();
    let mut tess_verts = Vec::<VVertex>::new();
    let mut tess_indices = Vec::<u32>::new();

    let mut fill_indices = Vec::<u32>::new();
    let mut fill_vertices = Vec::<f32>::new();
    let mut casing_indices = Vec::<u32>::new();
    let mut casing_vertices = Vec::<f32>::new();
    let mut stroke_indices = Vec::<u32>::new();
    let mut stroke_vertices = Vec::<f32>::new();
    let mut icon_indices = Vec::<u32>::new();
    let mut icon_vertices = Vec::<f32>::new();
    let mut fill_zbias = 0.0_f32;
    let mut casing_zbias = 0.0_f32;
    let mut stroke_zbias = 0.0_f32;
    let mut icon_zbias = 0.0_f32;
    let mut feature_count = 0usize;

    let mut prepared = Vec::<PreparedWay>::with_capacity(tile_ways.len());
    for (way_index, way) in tile_ways.iter().enumerate() {
        if way.points.len() < 2 {
            continue;
        }
        prepared.push(PreparedWay {
            way_index,
            points: way.points.clone(),
        });
    }

    // 2.5D: when the detail archive supplied building footprints with real
    // heights, they replace the base building fills entirely. Rings group
    // per source feature so multipolygon buildings (palaces, courtyarded
    // blocks) keep their holes.
    let has_detail_buildings = tile_ways
        .iter()
        .any(|way| way.tags.get("layer").map(|v| v.as_str()) == Some("detail_buildings"));
    struct BuildingGroup {
        rings: Vec<FillRing>,
        height_m: f32,
        min_height_m: f32,
        is_part: bool,
    }
    let mut building_groups = Vec::<BuildingGroup>::new();
    let mut building_group_lookup = HashMap::<String, usize>::new();
    // Building-age layer active: index BAG polygons by quantized centroid
    // so extruded buildings can pick up their bouwjaar tint (BAG footprints
    // match OSM buildings nearly 1:1).
    let mut bag_centroid_colors = HashMap::<(i32, i32), u32>::new();
    for way in tile_ways.iter() {
        if way.tags.get("layer").map(|v| v.as_str()) == Some("bag")
            && way.closed
            && way.points.len() >= 3
        {
            if let Some(color) = crate::map::style::bag_year_color(&way.tags) {
                let c = ring_centroid(&way.points);
                bag_centroid_colors
                    .insert(((c.0 / 6.0).round() as i32, (c.1 / 6.0).round() as i32), color);
            }
        }
    }

    // Fill pass
    let mut fill_groups = Vec::<FillFeatureGroup>::new();
    let mut plaza_rings: Vec<(u32, f32, Vec<(f32, f32)>, Option<Vec<f32>>)> = Vec::new();
    let mut fill_group_lookup = HashMap::<(String, u32, u32), usize>::new();
    for (order, prepared_way) in prepared.iter().enumerate() {
        let way = &tile_ways[prepared_way.way_index];
        if way.tags.get("layer").map(|v| v.as_str()) == Some("detail_buildings") {
            let Some(mut ring_points) = normalize_polygon_ring(&prepared_way.points) else {
                continue;
            };
            let clip = tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP));
            if !ring_inside_bounds(&ring_points, clip) {
                ring_points = clip_ring_to_rect(&ring_points, clip);
                if ring_points.len() < 3 {
                    continue;
                }
            }
            let signed_area = polygon_signed_area(&ring_points);
            if signed_area.abs() <= POLYGON_AREA_EPSILON {
                continue;
            }
            let feature_key = way
                .tags
                .get(MVT_INTERNAL_FEATURE_KEY)
                .cloned()
                .unwrap_or_else(|| format!("bldg:{}", prepared_way.way_index));
            let group_index =
                if let Some(index) = building_group_lookup.get(&feature_key).copied() {
                    index
                } else {
                    let index = building_groups.len();
                    building_group_lookup.insert(feature_key, index);
                    building_groups.push(BuildingGroup {
                        rings: Vec::new(),
                        height_m: building_height_m(&way.tags),
                        min_height_m: building_min_height_m(&way.tags),
                        is_part: way
                            .tags
                            .get("building:part")
                            .is_some_and(|value| value != "no"),
                    });
                    index
                };
            let ring_order = way
                .tags
                .get(MVT_INTERNAL_RING_INDEX_KEY)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(order);
            building_groups[group_index].rings.push(FillRing {
                order: ring_order,
                points: ring_points,
                signed_area,
            });
            continue;
        }
        if has_detail_buildings && way.tags.contains_key("building") {
            // Base buildings are replaced by the extruded detail set.
            continue;
        }
        // Labels are independent of fills: a named zoo enclosure with no
        // distinctive surface still gets its name at the centroid.
        let fill_color = fill_color_for_tags(theme, &way.tags, way.closed, render_zoom);
        let Some(mut ring_points) = normalize_polygon_ring(&prepared_way.points) else {
            continue;
        };
        // Overlap only needs to cover the AA fringe (~1 screen px). Any wider
        // and the double-drawn strip shows the later tile's LAND painting over
        // the earlier tile's BUILDINGS (per-tile rank order doesn't hold
        // across tiles), visible as a pale band at high zoom.
        let fill_clip_bounds = tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP));
        if !ring_inside_bounds(&ring_points, fill_clip_bounds) {
            ring_points = clip_ring_to_rect(&ring_points, fill_clip_bounds);
            if ring_points.len() < 3 {
                continue;
            }
        }

        let area_label_ok = render_zoom >= 15
            || matches!(
                way.tags.get("layer").map(|value| value.as_str()),
                Some("natura2000" | "wetlands")
            );
        if area_label_ok {
            if let Some(label) = extract_area_label(&way.tags, ring_centroid(&ring_points)) {
                labels.push(label);
            }
        }
        let Some(color) = fill_color else {
            continue;
        };
        let feature_key = way
            .tags
            .get(MVT_INTERNAL_FEATURE_KEY)
            .cloned()
            .unwrap_or_else(|| format!("way:{}", prepared_way.way_index));
        let pattern = fill_pattern_shape(&way.tags);
        let alpha = fill_alpha_for_tags(&way.tags);
        let group_key = (feature_key, color, pattern.to_bits() ^ alpha.to_bits());
        let group_index = if let Some(index) = fill_group_lookup.get(&group_key).copied() {
            index
        } else {
            let index = fill_groups.len();
            fill_group_lookup.insert(group_key, index);
            let mvt_layer = way.tags.get("layer").map(|v| v.as_str()).unwrap_or("");
            let deckable =
                matches!(mvt_layer, "street_polygons" | "streets_med" | "streets_low")
                    && !tag_is_truthy(&way.tags, "tunnel");
            // Attribute decks are the shortbread-tag fallback; solved
            // bridge-dz coverage replaces them with corridor matching.
            let deck_m = if deckable
                && !bridge_dz_covered
                && tag_is_truthy(&way.tags, "bridge")
            {
                9.0
            } else {
                0.0
            };
            fill_groups.push(FillFeatureGroup {
                color,
                layer_rank: fill_layer_rank(&way.tags),
                is_building: way.tags.contains_key("building"),
                alpha,
                pattern,
                late: matches!(
                    way.tags.get("layer").map(|v| v.as_str()),
                    Some("gemeenten" | "wijken" | "buurten")
                ),
                deck_m,
                deckable,
                profiles: Vec::new(),
                rings: Vec::new(),
            });
            index
        };

        // Road-surface polygons join the road tier unions instead of the
        // fill pipeline: the junction plaza and its road class must be ONE
        // surface (2D reference: plazas paint over minor-road centers).
        let plaza_layer = way.tags.get("layer").map(|v| v.as_str()).unwrap_or("");
        if union_roads
            && matches!(plaza_layer, "street_polygons" | "streets_med" | "streets_low")
            && !tag_is_truthy(&way.tags, "tunnel")
            && way.closed
        {
            plaza_rings.push((
                color,
                fill_alpha_for_tags(&way.tags),
                way.points.clone(),
                way.dz.clone(),
            ));
            continue;
        }
        let ring_order = way
            .tags
            .get(MVT_INTERNAL_RING_INDEX_KEY)
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(order);
        let signed_area = polygon_signed_area(&ring_points);
        if signed_area.abs() <= POLYGON_AREA_EPSILON {
            continue;
        }
        if let Some(dz) = &way.dz {
            fill_groups[group_index].profiles.push(BridgeCorridor {
                points: way.points.clone(),
                decks: dz.clone(),
                half_width: 3.0,
                solved: true,
            });
        }
        fill_groups[group_index].rings.push(FillRing {
            order: ring_order,
            points: ring_points,
            signed_area,
        });
    }

    // Road-surface plaza rings diverted to the road tier unions.
    let plaza_rings_ref = &plaza_rings;
    let _ = plaza_rings_ref;
    // Paint fills in semantic order (land -> sites -> water -> buildings ->
    // street areas), not raw MVT layer order which puts land/sites on top of
    // the buildings. Stable within each rank to preserve source order.
    let mut fill_order = (0..fill_groups.len()).collect::<Vec<_>>();
    fill_order.sort_by_key(|&index| fill_groups[index].layer_rank);

    let building_outline = if render_zoom >= BUILDING_OUTLINE_MIN_ZOOM {
        theme.building_outline
    } else {
        None
    };

    for group_index in fill_order {
        let group = &fill_groups[group_index];
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
                aa_units,
                false,
                tolerance,
            );
            // Road-surface polygons join the STROKE pass: in tilt mode
            // passes 1-3 carry the relief depth boost, and a junction
            // plaza left in the unboosted fill domain gets sliced by every
            // casing rim crossing it — flat and tilted views must layer
            // roads identically.
            let (target_verts, target_indices, target_zbias) = if group.late {
                (&mut icon_vertices, &mut icon_indices, &mut icon_zbias)
            } else if group.deckable {
                (&mut stroke_vertices, &mut stroke_indices, &mut stroke_zbias)
            } else {
                (&mut fill_vertices, &mut fill_indices, &mut fill_zbias)
            };
            let road_surface_micro = if group.deckable {
                // Under the union surfaces: the tier meshes ARE the road
                // now; plazas are backdrop.
                0.05
            } else {
                0.0
            };
            // Baked coverage: the polygon rides its OWN annotated outline
            // profile (base_dz join). Outside coverage there is no fill
            // profile source, so fills stay on the constant attribute deck.
            let fill_decks: Option<Vec<f32>> =
                if group.deckable && !group.profiles.is_empty() {
                    Some(
                        tess_verts
                            .iter()
                            .map(|v| corridor_deck_at_point(v.x, v.y, &group.profiles))
                            .collect(),
                    )
                } else {
                    None
                };
            append_tessellated_geometry_decked(
                &tess_verts,
                &tess_indices,
                target_verts,
                target_indices,
                VectorRenderParams {
                    color: hex_to_premul_rgba(group.color, group.alpha),
                    stroke_mult: 1e6,
                    shape_id: group.pattern,
                    params: [
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        group.deck_m,
                        road_surface_micro
                            + group.layer_rank as f32 * DEPTH_MICRO_PER_RANK
                            + (feature_count % 16) as f32 * DEPTH_MICRO_PER_FEATURE,
                    ],
                    zbias: *target_zbias,
                },
                fill_decks.as_deref(),
            );
            *target_zbias += VECTOR_ZBIAS_STEP;
            feature_count += 1;

            if let (true, Some(outline)) = (group.is_building, building_outline) {
                // Outline the ring but drop segments that run along the tile
                // cut, so clipped buildings don't get a fake wall at the seam.
                let outline_bounds =
                    tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP) * 0.2);
                let outline_style = StrokePassStyle { deck_m: 0.0,
                    color: outline,
                    width: BUILDING_OUTLINE_WIDTH_PX / render_scale,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 46.0 * DEPTH_MICRO_PER_RANK,
                };
                for ring in &polygon {
                    let mut closed_points = ring.clone();
                    if closed_points.first() != closed_points.last() {
                        if let Some(first) = closed_points.first().copied() {
                            closed_points.push(first);
                        }
                    }
                    for part in clip_polyline_parts(&closed_points, outline_bounds, false) {
                        if part.len() < 2 {
                            continue;
                        }
                        let full_loop = part.len() == closed_points.len()
                            && part.first() == part.last();
                        let points = if full_loop { &part[..part.len() - 1] } else { &part[..] };
                        append_stroke_pass(
                            &mut path,
                            points,
                            full_loop,
                            None,
                            &mut tess,
                            &mut tess_verts,
                            &mut tess_indices,
                            &mut fill_vertices,
                            &mut fill_indices,
                            outline_style,
                            LineCap::Butt,
                            LineCap::Butt,
                            LineJoin::Miter,
                            aa_units,
                            tolerance,
                            &mut fill_zbias,
                            stroke_pass_param5(&outline_style),
                        );
                    }
                }
            }
        }
    }

    // 2.5D building extrusion: per-edge flat-shaded walls (exterior rings
    // AND courtyard holes), then the roof with holes preserved, lifted by
    // height (the tilt shader does the lifting per frame, so tilt animates
    // without rebuilding tiles). North-first paint order is the painter's
    // approximation of occlusion under the screen-top extrusion.
    if !building_groups.is_empty() {
        struct BuildingJob {
            polygon: Vec<Vec<(f32, f32)>>,
            height_m: f32,
            base_m: f32,
            tint: Option<u32>,
            min_y: f32,
        }
        // Simple 3D Buildings: an outline whose interior holds
        // building:parts must NOT extrude — the parts carry the true
        // volumes (Westerkerk's nave + 85m Westertoren); the outline
        // keeps only a flat footprint fill beneath them.
        let part_centroids: Vec<(f32, f32)> = building_groups
            .iter()
            .filter(|group| group.is_part)
            .filter_map(|group| {
                group
                    .rings
                    .iter()
                    .max_by(|a, b| {
                        a.signed_area
                            .abs()
                            .partial_cmp(&b.signed_area.abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|ring| ring_centroid(&ring.points))
            })
            .collect();
        if !part_centroids.is_empty() {
            for group in building_groups.iter_mut() {
                if group.is_part {
                    continue;
                }
                let covers = group.rings.iter().any(|ring| {
                    part_centroids
                        .iter()
                        .any(|c| point_in_ring(*c, &ring.points))
                });
                if covers {
                    group.height_m = 0.0;
                    group.min_height_m = 0.0;
                }
            }
        }
        let mut building_jobs = Vec::<BuildingJob>::new();
        for group in &building_groups {
            for polygon in classify_polygon_rings(&group.rings, EARCUT_MAX_RINGS) {
                if polygon.is_empty() {
                    continue;
                }
                let min_y = polygon
                    .iter()
                    .flat_map(|ring| ring.iter())
                    .fold(f32::MAX, |acc, p| acc.min(p.1));
                let tint = if bag_centroid_colors.is_empty() {
                    None
                } else {
                    polygon.first().and_then(|ring| {
                        let c = ring_centroid(ring);
                        let (qx, qy) = ((c.0 / 6.0).round() as i32, (c.1 / 6.0).round() as i32);
                        let mut found = None;
                        'search: for dy in -1..=1 {
                            for dx in -1..=1 {
                                if let Some(color) =
                                    bag_centroid_colors.get(&(qx + dx, qy + dy))
                                {
                                    found = Some(*color);
                                    break 'search;
                                }
                            }
                        }
                        found
                    })
                };
                building_jobs.push(BuildingJob {
                    polygon,
                    height_m: group.height_m,
                    base_m: group.min_height_m.clamp(0.0, group.height_m),
                    tint,
                    min_y,
                });
            }
        }
        building_jobs.sort_by(|a, b| {
            a.min_y
                .partial_cmp(&b.min_y)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let base_color = theme.building_fill_color().unwrap_or(0xd9d0c9);
        // Light from the north-west; walls shade by their outward normal.
        let (light_x, light_y) = (-0.55_f32, -0.835_f32);
        for job in &building_jobs {
            // Building-age layer tints the 3D model itself (walls shade
            // from the same hue via the normal lighting math).
            let roof_color = hex_to_premul_rgba(job.tint.unwrap_or(base_color), 1.0);
            if job.height_m <= 0.05 {
                // Flattened outline: footprint fill only, no walls.
            } else {
            for ring in &job.polygon {
                // Outward normal needs ring orientation; positive shoelace
                // in y-down tile space = exterior winding, holes come
                // opposite so their normals flip into the courtyard.
                let clockwise = polygon_signed_area(ring) > 0.0;
                let n = ring.len();
                // South-most edges last so they paint over northern walls.
                let mut edge_order: Vec<usize> = (0..n).collect();
                edge_order.sort_by(|&i, &j| {
                    let yi = ring[i].1 + ring[(i + 1) % n].1;
                    let yj = ring[j].1 + ring[(j + 1) % n].1;
                    yi.partial_cmp(&yj).unwrap_or(std::cmp::Ordering::Equal)
                });
                for &i in &edge_order {
                    let a = ring[i];
                    let b = ring[(i + 1) % n];
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len = (dx * dx + dy * dy).sqrt();
                    if len < 1e-4 {
                        continue;
                    }
                    let (mut nx, mut ny) = (dy / len, -dx / len);
                    if !clockwise {
                        nx = -nx;
                        ny = -ny;
                    }
                    let facing = (nx * light_x + ny * light_y).clamp(-1.0, 1.0);
                    let shade = 0.62 + 0.20 * (facing + 1.0);
                    let wall_color = [
                        roof_color[0] * shade,
                        roof_color[1] * shade,
                        roof_color[2] * shade,
                        1.0,
                    ];
                    append_wall_quad(
                        a,
                        b,
                        job.base_m,
                        job.height_m,
                        wall_color,
                        &mut fill_vertices,
                        &mut fill_indices,
                        &mut fill_zbias,
                    );
                }
            }
            }
            for ring in &job.polygon {
                emit_path(&mut path, ring, true);
            }
            tessellate_path_fill(
                &mut path,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                LineJoin::Miter,
                4.0,
                aa_units,
                false,
                tolerance,
            );
            append_tessellated_geometry(
                &tess_verts,
                &tess_indices,
                &mut fill_vertices,
                &mut fill_indices,
                VectorRenderParams {
                    color: roof_color,
                    stroke_mult: 1e6,
                    shape_id: 0.0,
                    params: [0.0, 0.0, 0.0, 0.0, job.height_m, 0.05],
                    zbias: fill_zbias,
                },
            );
            fill_zbias += VECTOR_ZBIAS_STEP;
            feature_count += 1;
        }
    }

    // Little 3D trees (tilt mode): two crossed trunk quads (visible from
    // any camera heading) + two stacked canopy discs lifted by the same
    // per-meter height mechanism as building roofs — the tilt compression
    // turns them into oval blobs.
    if !tree_points_3d.is_empty() {
        let n = (1u32 << tile_key.z) as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
            .sinh()
            .atan();
        // tile-local units per meter at this latitude
        let units_per_m =
            (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32;
        let trunk_color = hex_to_premul_rgba(0x8a6b4a, 1.0);
        let canopy_color = hex_to_premul_rgba(0x4a7d44, 1.0);
        let arm = 0.7 * units_per_m;

        for (x, y) in &tree_points_3d {
            append_wall_quad(
                (*x - arm, *y),
                (*x + arm, *y),
                0.0,
                7.5,
                trunk_color,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            append_wall_quad(
                (*x, *y - arm),
                (*x, *y + arm),
                0.0,
                7.5,
                trunk_color,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            // Street-tree proportions vs buildings: ~11.5m total. The
            // canopy is a PROLATE ellipsoid (taller than wide) on a tall
            // trunk — scaling the ball uniformly reads as a bush.
            append_ball(
                (*x, *y),
                2.9 * units_per_m,
                4.0,
                7.5,
                canopy_color,
                16,
                8,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            feature_count += 1;
        }
    }

    // Dynamic stalk heights: every flying marker clears the building under
    // it by ~8 m (a 100 m tower gets a 108 m pin), plus a small
    // deterministic stagger so clustered pins don't form one flat plane.
    let job_lifts: Vec<f32> = icon_jobs
        .iter()
        .map(|job| {
            let base = job.9;
            if base <= 0.0 {
                return 0.0;
            }
            let (px, py) = job.0;
            let mut clearance = 0.0f32;
            for group in &building_groups {
                if group.height_m <= clearance {
                    continue;
                }
                for ring in &group.rings {
                    if ring.signed_area <= 0.0 {
                        continue;
                    }
                    if point_in_ring((px, py), &ring.points) {
                        clearance = clearance.max(group.height_m);
                        break;
                    }
                }
            }
            base.max(clearance + 8.0)
        })
        .collect();
    // Propagate the FINAL lifts into the labels and tap zones that belong
    // to these markers, so text and hit-testing ride the same stalk.
    for (job, lift) in icon_jobs.iter().zip(job_lifts.iter()) {
        if *lift <= 0.0 {
            continue;
        }
        let (jx, jy) = job.0;
        for label in labels.iter_mut() {
            let eligible = label.color_class == crate::map::label::LABEL_CLASS_PIN
                || label.road_kind.starts_with("chb")
                || label.road_kind.starts_with("poi")
                || label.road_kind.starts_with("stS")
                || label.road_kind.starts_with("stp");
            if !eligible || label.path_points.is_empty() {
                continue;
            }
            let (lx, ly) = label.path_points[0];
            let (mx, my) = label
                .path_points
                .last()
                .map(|p| ((lx + p.0) * 0.5, (ly + p.1) * 0.5))
                .unwrap_or((lx, ly));
            if (mx - jx).abs() < 2.5 && (my - jy).abs() < 2.5 {
                label.lift_m = *lift;
            }
        }
        let world = (1u32 << tile_key.z) as f64;
        let jnorm = (
            (tile_key.x as f64 + jx as f64 / crate::map::geometry::TILE_SIZE) / world,
            (tile_key.y as f64 + jy as f64 / crate::map::geometry::TILE_SIZE) / world,
        );
        for hit in pin_hits.iter_mut() {
            if (hit.norm.0 - jnorm.0).abs() < 1e-7 && (hit.norm.1 - jnorm.1).abs() < 1e-7 {
                hit.lift_m = *lift;
            }
        }
    }
    // Marker stalks (3D mode): thin dark lines from the ground point up to
    // every floating marker.
    if buildings_3d {
        let has_pins = icon_jobs.iter().any(|job| job.9 > 0.0);
        if has_pins {
            let n = (1u32 << tile_key.z) as f64;
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
                .sinh()
                .atan();
            let units_per_m =
                (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32;
            let stalk_color = hex_to_premul_rgba(0x4a5058, 1.0);
            for (job_index, job) in icon_jobs.iter().enumerate() {
                let lift = job_lifts[job_index];
                if lift <= 0.0 {
                    continue;
                }
                // Chargers get a slightly heavier stalk than POI markers.
                let arm = if job.5 == 2 || job.5 == 3 { 0.22 } else { 0.14 } * units_per_m;
                let (x, y) = job.0;
                append_wall_quad(
                    (x - arm, y),
                    (x + arm, y),
                    0.0,
                    lift,
                    stalk_color,
                    &mut fill_vertices,
                    &mut fill_indices,
                    &mut fill_zbias,
                );
                append_wall_quad(
                    (x, y - arm),
                    (x, y + arm),
                    0.0,
                    lift,
                    stalk_color,
                    &mut fill_vertices,
                    &mut fill_indices,
                    &mut fill_zbias,
                );
            }
        }
    }

    // Little 3D stoplights (tilt mode): a slim dark pole with the classic
    // three lights stacked on top — red above amber above green.
    if !signal_points_3d.is_empty() {
        let n = (1u32 << tile_key.z) as f64;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * (tile_key.y as f64 + 0.5) / n))
            .sinh()
            .atan();
        let units_per_m =
            (crate::map::geometry::TILE_SIZE * n / (40_075_016.686 * lat.cos())) as f32;
        let pole_color = hex_to_premul_rgba(0x3c4046, 1.0);
        let lights = [
            (hex_to_premul_rgba(0x2ecc40, 1.0), 3.5f32),
            (hex_to_premul_rgba(0xf5a623, 1.0), 4.35),
            (hex_to_premul_rgba(0xd7263d, 1.0), 5.2),
        ];
        let arm = 0.32 * units_per_m;
        for (x, y) in &signal_points_3d {
            append_wall_quad(
                (*x - arm, *y),
                (*x + arm, *y),
                0.0,
                3.2,
                pole_color,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            append_wall_quad(
                (*x, *y - arm),
                (*x, *y + arm),
                0.0,
                3.2,
                pole_color,
                &mut fill_vertices,
                &mut fill_indices,
                &mut fill_zbias,
            );
            for (color, height_m) in lights {
                append_ball(
                    (*x, *y),
                    0.5 * units_per_m,
                    0.5,
                    height_m,
                    color,
                    8,
                    4,
                    &mut fill_vertices,
                    &mut fill_indices,
                    &mut fill_zbias,
                );
            }
            feature_count += 1;
        }
    }

    // Stroke pass
    let mut stroke_jobs = Vec::<StrokeDrawJob>::new();
    let mut arrow_jobs = Vec::<(Vec<(f32, f32)>, bool)>::new();
    for prepared_way in &prepared {
        let way = &tile_ways[prepared_way.way_index];
        if let Some(label) = extract_way_label(&way.tags, &prepared_way.points) {
            labels.push(label);
        }
        // Oneway arrows read from mid zoom, not just street level —
        // direction matters while route-planning zoomed out. NOT tied to
        // stroke style presence: at close zooms wide roads render as
        // street polygons and their centerline stroke style is None, but
        // the direction arrows must survive.
        let implicit_oneway = matches!(
            way.tags.get("junction").map(|v| v.as_str()),
            Some("roundabout") | Some("circular")
        );
        if render_zoom >= 15
            && (tag_is_truthy(&way.tags, "oneway") || implicit_oneway)
            && way.tags.contains_key("highway")
            && !tag_is_truthy(&way.tags, "rail")
        {
            arrow_jobs.push((
                prepared_way.points.clone(),
                tag_is_truthy(&way.tags, "oneway_reverse"),
            ));
        }
        if let Some(mut style) =
            stroke_style_for_tags(theme, &way.tags, tile_key.z, render_zoom, zoom_mult, px_to_units)
        {
            // Inside baked bridge-dz coverage the solved corridor profile
            // is the only lift source — shortbread bridge tags are the
            // coarse signal that lifted whole merged runs.
            if bridge_dz_covered {
                style.center.deck_m = 0.0;
                if let Some(casing) = style.casing.as_mut() {
                    casing.deck_m = 0.0;
                }
            }
            // Tunnels must never ride a corridor deck: a tunnel tube
            // running parallel to a bridge (IJtunnel next to Zouthavenbrug)
            // passes the direction gate and would hoist above ground.
            // deck_m < 0 is the "never deck" sentinel for the stroke pass.
            if tag_is_truthy(&way.tags, "tunnel") {
                style.center.deck_m = -1.0;
                if let Some(casing) = style.casing.as_mut() {
                    casing.deck_m = -1.0;
                }
            }
            if let Some(dots) = thin_bridge_dots_for_tags(
                theme,
                &way.tags,
                render_zoom,
                zoom_mult,
                px_to_units,
            ) {
                stroke_jobs.push(StrokeDrawJob {
                    sort_rank: dots.sort_rank,
                    style: dots,
                    points: prepared_way.points.clone(),
                    union_road: false,
                    dz: None,
                });
            }
            // Solid road geometry joins the per-tier union mesh: one
            // seamless surface per class, identical flat and tilted. Dashed
            // shapes (rails, tunnels' dash patterns) keep the stroke path.
            let union_road = union_roads
                && way.tags.contains_key("highway")
                && !tag_is_truthy(&way.tags, "rail")
                && !tag_is_truthy(&way.tags, "tunnel")
                && style.center.shape_id == 0.0
                && style.casing.map_or(true, |casing| casing.shape_id == 0.0);
            stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                points: prepared_way.points.clone(),
                union_road,
                dz: if union_road { way.dz.clone() } else { None },
            });
        }
    }

    let mut union_tiers =
        HashMap::<StrokeStyleKey, (StrokeStyle, Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>)>::new();
    let mut grouped_strokes = HashMap::<StrokeStyleKey, (StrokeStyle, Vec<Vec<(f32, f32)>>)>::new();
    for job in stroke_jobs {
        let key = StrokeStyleKey::from(job.style);
        if job.union_road {
            let entry = union_tiers.entry(key).or_insert((job.style, Vec::new()));
            entry.1.push((job.points, job.dz));
            continue;
        }
        let entry = grouped_strokes.entry(key).or_insert((job.style, Vec::new()));
        entry.1.push(job.points);
    }

    let mut merged_stroke_jobs = Vec::<StrokeDrawJob>::new();
    for (_key, (style, polylines)) in grouped_strokes {
        for points in merge_stroke_polylines(&polylines) {
            merged_stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                points,
                union_road: false,
                dz: None,
            });
        }
    }

    // Deterministic paint order: rank, then style bits (HashMap iteration
    // order must not leak into the render).
    merged_stroke_jobs.sort_unstable_by_key(|job| {
        (
            job.sort_rank,
            job.style.center.color,
            job.style.center.width.to_bits(),
        )
    });
    let clip_bounds = tile_clip_bounds(ROAD_CLIP_PADDING);
    // Overzoomed tiles magnify the source tile's coordinate quantization
    // into visibly angular curves (ovals read as polygons at 8-16x). A
    // round or two of Chaikin corner-cutting restores the curvature.
    let chaikin_rounds = if render_scale >= 8.0 {
        2
    } else if render_scale >= 3.0 {
        1
    } else {
        0
    };
    // Only cut where segments are shorter than ~10 screen px — dense
    // quantized curves qualify, real street corners never do.
    let chaikin_cut_below = 10.0 / render_scale;
    let mut merged_stroke_parts = Vec::<(StrokeStyle, Vec<Vec<(f32, f32)>>)>::new();
    for job in merged_stroke_jobs {
        let smooth = chaikin_smooth(&job.points, chaikin_rounds, chaikin_cut_below);
        let parts = build_polyline_parts(&smooth, clip_bounds, false, ROAD_SMOOTH_FACTOR);
        merged_stroke_parts.push((job.style, parts));
    }

    // Painter interleave vs the road faces: legacy strokes whose rank is
    // below the topmost road tier paint UNDER the faces (cycleway dashes,
    // park paths — covered by roads at crossings in the reference); only
    // higher ranks (trams, rails) stay above.
    let max_tier_rank = union_tiers
        .values()
        .map(|(style, _)| style.sort_rank)
        .max()
        .unwrap_or(i16::MIN);

    // Road paint ladder: union faces and legacy strokes merge into ONE
    // ordered sequence — plazas, then all casings by rank, then all centers
    // by rank — exactly the reference painter. Legacy strokes no longer sit
    // wholesale under the faces: a park path draws above the plaza it
    // crosses, a cycle lane above the road it rides, and higher road faces
    // still cover both. Flat mode is buffer order; tilt follows the same
    // order through the param5 ladder. Only centers ranked above the top
    // road tier (trams, rails) stay in the stroke buffer above everything.
    let union_clip = clip_bounds;
    let mut tier_list: Vec<&(StrokeStyle, Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>)> =
        union_tiers.values().collect();
    tier_list.sort_by_key(|entry| {
        (
            entry.0.sort_rank,
            entry.0.center.color,
            entry.0.center.width.to_bits(),
        )
    });
    let mut groups: Vec<PaintGroup> = Vec::new();
    {
        let mut plaza_keys: Vec<(u32, u32)> = plaza_rings
            .iter()
            .map(|(color, alpha, _, _)| (*color, alpha.to_bits()))
            .collect();
        plaza_keys.sort_unstable();
        plaza_keys.dedup();
        for (color, alpha_bits) in plaza_keys {
            let ribbons: Vec<RoadRibbon> = plaza_rings
                .iter()
                .filter(|(c, a, _, _)| *c == color && a.to_bits() == alpha_bits)
                .map(|(_, _, points, dz)| RoadRibbon {
                    points,
                    dz: dz.as_deref(),
                    closed_ring: true,
                })
                .collect();
            let rings = road_ribbon_rings(&ribbons, 1.0, 0.0, union_clip);
            groups.push(PaintGroup {
                color: hex_to_premul_rgba(color, f32::from_bits(alpha_bits)),
                param5: 0.0,
                phase: 0,
                rank: i16::MIN,
                field: 0,
                rings: rings
                    .into_iter()
                    .map(|(ring, dz)| {
                        let max_dz = dz.iter().copied().fold(0.0f32, f32::max);
                        (ring, max_dz)
                    })
                    .collect(),
            });
        }
    }
    // Union ways get the same corner-cut smoothing as legacy strokes so
    // curve shapes stay identical between the two pipelines; dz rides
    // along through the cuts.
    let smoothed_tiers: Vec<(StrokeStyle, Vec<(Vec<(f32, f32)>, Option<Vec<f32>>)>)> = tier_list
        .iter()
        .map(|entry| {
            (
                entry.0,
                entry
                    .1
                    .iter()
                    .map(|(points, dz)| {
                        chaikin_smooth_dz(
                            points,
                            dz.as_deref(),
                            chaikin_rounds,
                            chaikin_cut_below,
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    // Per-tier deck fields: index 0 is the plaza field, tier i lives at
    // 1 + i. Casing and center faces of one tier share one field, so both
    // displace identically in tilt — no more detached outlines on ramps.
    let mut dz_fields: Vec<Option<DzField>> = Vec::new();
    {
        // Plazas ride their own ring dz AND any road lifting through them
        // (a bridge deck crossing a quay), so the road ways join the field.
        let mut plaza_ways: Vec<(&[(f32, f32)], Option<&[f32]>)> = plaza_rings
            .iter()
            .map(|(_, _, points, dz)| (points.as_slice(), dz.as_deref()))
            .collect();
        for (_, ways) in &smoothed_tiers {
            plaza_ways.extend(
                ways.iter()
                    .map(|(points, dz)| (points.as_slice(), dz.as_deref())),
            );
        }
        dz_fields.push(DzField::build(&plaza_ways, 6.0));
    }
    for (style, ways) in &smoothed_tiers {
        let half_width = style
            .casing
            .map_or(style.center.width, |casing| casing.width.max(style.center.width))
            * 0.5;
        let ways_ref: Vec<(&[(f32, f32)], Option<&[f32]>)> = ways
            .iter()
            .map(|(points, dz)| (points.as_slice(), dz.as_deref()))
            .collect();
        dz_fields.push(DzField::build(&ways_ref, half_width + 2.0));
    }
    for pass in 0..2u8 {
        for (tier_index, (style, ways)) in smoothed_tiers.iter().enumerate() {
            let (color, width) = if pass == 0 {
                let Some(casing) = style.casing else { continue };
                (casing.color, casing.width)
            } else {
                (style.center.color, style.center.width)
            };
            let ribbons: Vec<RoadRibbon> = ways
                .iter()
                .map(|(points, dz)| RoadRibbon {
                    points,
                    dz: dz.as_deref(),
                    closed_ring: false,
                })
                .collect();
            let rings =
                road_ribbon_rings(&ribbons, (width * 0.5).max(0.05), aa_units, union_clip);
            groups.push(PaintGroup {
                color: hex_to_premul_rgba(color, 1.0),
                param5: 0.0,
                phase: 1 + pass,
                rank: style.sort_rank,
                field: (1 + tier_index) as u16,
                rings: rings
                    .into_iter()
                    .map(|(ring, dz)| {
                        let max_dz = dz.iter().copied().fold(0.0f32, f32::max);
                        (ring, max_dz)
                    })
                    .collect(),
            });
        }
    }
    let faces = if groups.is_empty() {
        Vec::new()
    } else {
        overlay_paint_groups(&groups, &mut tess, tolerance)
    };

    enum RoadPaintEvent<'a> {
        Face(usize),
        Stroke {
            pass: StrokePassStyle,
            part: &'a [(f32, f32)],
            start_cap: LineCap,
            end_cap: LineCap,
        },
    }
    // Round caps blend same-color segments at junctions and give dead ends
    // the carto nub — but ends produced by the tile clip must stay butt, or
    // the cap disc overpaints the neighbor tile's content.
    let cap_eps = 0.05_f32;
    let mut events: Vec<((u8, i16, u8, u32), RoadPaintEvent<'_>)> = Vec::new();
    for (face_index, face) in faces.iter().enumerate() {
        events.push((
            (face.phase, face.rank, 1, face_index as u32),
            RoadPaintEvent::Face(face_index),
        ));
    }
    let mut stroke_seq = 0u32;
    for (style, parts) in &merged_stroke_parts {
        for part in parts {
            if part.len() < 2 {
                continue;
            }
            if let Some(casing) = style.casing {
                events.push((
                    (1, style.sort_rank, 0, stroke_seq),
                    RoadPaintEvent::Stroke {
                        pass: casing,
                        part,
                        start_cap: LineCap::Butt,
                        end_cap: LineCap::Butt,
                    },
                ));
                stroke_seq += 1;
            }
            if style.sort_rank <= max_tier_rank {
                let start_cap = if point_on_bounds(part[0], clip_bounds, cap_eps) {
                    LineCap::Butt
                } else {
                    LineCap::Round
                };
                let end_cap = if point_on_bounds(part[part.len() - 1], clip_bounds, cap_eps) {
                    LineCap::Butt
                } else {
                    LineCap::Round
                };
                events.push((
                    (2, style.sort_rank, 0, stroke_seq),
                    RoadPaintEvent::Stroke {
                        pass: style.center,
                        part,
                        start_cap,
                        end_cap,
                    },
                ));
                stroke_seq += 1;
            }
        }
    }
    events.sort_by_key(|(key, _)| *key);

    let ladder_step =
        (4.0 * DEPTH_MICRO_PER_RANK).min(0.44 / events.len().max(1) as f32);
    for (event_index, (_, event)) in events.iter().enumerate() {
        let ladder_param5 = 0.06 + event_index as f32 * ladder_step;
        match event {
            RoadPaintEvent::Face(face_index) => {
                let face = &faces[*face_index];
                // 3D: faces re-acquire deck height from their tier's dz
                // field — height never needs to survive the boolean. The
                // mesh is refined near lifted geometry first, so ramps
                // interpolate as smoothly as the legacy dense strokes.
                let field = dz_fields
                    .get(face.field as usize)
                    .and_then(|field| field.as_ref());
                let mut sub_verts;
                let mut sub_indices;
                let (verts, indices, deck): (&[VVertex], &[u32], Option<Vec<f32>>) =
                    match field {
                        Some(field) => {
                            sub_verts = face.verts.clone();
                            sub_indices = face.indices.clone();
                            subdivide_face_mesh(&mut sub_verts, &mut sub_indices, 3.0, field);
                            let deck: Vec<f32> = sub_verts
                                .iter()
                                .map(|v| field.sample(v.x, v.y))
                                .collect();
                            let lifted = deck.iter().any(|&d| d > 0.05);
                            (&sub_verts, &sub_indices, lifted.then_some(deck))
                        }
                        None => (&face.verts, &face.indices, None),
                    };
                append_tessellated_geometry_decked(
                    verts,
                    indices,
                    &mut casing_vertices,
                    &mut casing_indices,
                    VectorRenderParams {
                        color: face.color,
                        stroke_mult: 1e6,
                        shape_id: 0.0,
                        params: [0.0, 0.0, 0.0, 0.0, 0.0, ladder_param5],
                        zbias: casing_zbias,
                    },
                    deck.as_deref(),
                );
                casing_zbias += VECTOR_ZBIAS_STEP;
                feature_count += 1;
            }
            RoadPaintEvent::Stroke {
                pass,
                part,
                start_cap,
                end_cap,
            } => {
                // Tunnels keep their fixed under-everything depth slot.
                let param5 = if pass.deck_m < 0.0 { 0.05 } else { ladder_param5 };
                append_stroke_pass(
                    &mut path,
                    part,
                    false,
                    stroke_corridors_available.then(|| {
                        if bridge_dz_covered { &own_profiles[..] } else { &bridge_corridors[..] }
                    }),
                    &mut tess,
                    &mut tess_verts,
                    &mut tess_indices,
                    &mut casing_vertices,
                    &mut casing_indices,
                    *pass,
                    *start_cap,
                    *end_cap,
                    LineJoin::Round,
                    aa_units,
                    tolerance,
                    &mut casing_zbias,
                    param5,
                );
                feature_count += 1;
            }
        }
    }

    // Centers ranked above the topmost road tier (trams, rails): stroke
    // buffer, above every face and interleaved stroke.
    for (style, parts) in &merged_stroke_parts {
        if style.sort_rank <= max_tier_rank {
            continue;
        }
        for part in parts {
            if part.len() < 2 {
                continue;
            }
            let start_cap = if point_on_bounds(part[0], clip_bounds, cap_eps) {
                LineCap::Butt
            } else {
                LineCap::Round
            };
            let end_cap = if point_on_bounds(part[part.len() - 1], clip_bounds, cap_eps) {
                LineCap::Butt
            } else {
                LineCap::Round
            };
            append_stroke_pass(
                &mut path,
                part,
                false,
                stroke_corridors_available.then(|| {
                    if bridge_dz_covered { &own_profiles[..] } else { &bridge_corridors[..] }
                }),
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                &mut stroke_vertices,
                &mut stroke_indices,
                style.center,
                start_cap,
                end_cap,
                LineJoin::Round,
                aa_units,
                tolerance,
                &mut stroke_zbias,
                stroke_pass_param5(&style.center),
            );
            feature_count += 1;
        }
    }

    // Pass 3: POI symbols — zoom-constant vector icons, drawn above strokes.
    for (job_index, (anchor, mesh, color_class, _, _, two_tone, kw, stalls, zoom_floor, _)) in
        icon_jobs.iter().enumerate()
    {
        let pin_lift_m = job_lifts[job_index];
        // The lift rides in param4's hundreds (0.25 m quanta) so the zoom
        // floor keeps its low digits.
        let param4_encoded = zoom_floor + (pin_lift_m * 4.0).round() * 100.0;
        append_icon_mesh(
            mesh,
            *anchor,
            hex_to_premul_rgba(poi_class_hex(*color_class), 1.0),
            param4_encoded,
            &mut icon_vertices,
            &mut icon_indices,
            &mut icon_zbias,
        );
        // carto trees: light canopy disc with a dark center dot.
        if *two_tone == 1 {
            if let Some(core) = icon_mesh("tree_core") {
                append_icon_mesh(
                    core,
                    *anchor,
                    hex_to_premul_rgba(0x4c7a4c, 1.0),
                    param4_encoded,
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
            }
        }
        // Charger pins are one COMPOSITE at a single anchor: badge, white
        // bolt (offset baked in the mesh) and, for fast sites, the kW
        // digits as vector glyphs — all billboard together, so nothing
        // detaches, doubles or re-lays-out while zooming or rotating.
        if *two_tone == 2 || *two_tone == 3 {
            let bolt_name = if *two_tone == 2 { "charger_bolt_fast" } else { "charger_bolt_ac" };
            if let Some(bolt) = icon_mesh(bolt_name) {
                append_icon_mesh(
                    bolt,
                    *anchor,
                    hex_to_premul_rgba(0xffffff, 1.0),
                    param4_encoded,
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
            }
        }
        let _ = (kw, stalls);
        feature_count += 1;
    }

    // Oneway arrows: zoom-constant glyphs spaced along the way, offsets
    // pre-rotated into the travel direction (carto-style).
    let arrow_color = hex_to_premul_rgba(0x8a8a8a, 1.0);
    let arrow_interval = 170.0 / render_scale;
    let mut arrow_debug_appended = 0usize;
    for (points, reverse) in &arrow_jobs {
        for part in build_polyline_parts(points, clip_bounds, false, 0.0) {
            let mut cumulative = Vec::<f32>::with_capacity(part.len());
            let mut total = 0.0_f32;
            cumulative.push(0.0);
            for pair in part.windows(2) {
                let dx = pair[1].0 - pair[0].0;
                let dy = pair[1].1 - pair[0].1;
                total += (dx * dx + dy * dy).sqrt();
                cumulative.push(total);
            }
            if total < arrow_interval * 0.6 {
                continue;
            }
            let mut distance = arrow_interval * 0.5;
            while distance < total {
                // find segment containing this distance
                let mut segment = 0;
                while segment + 2 < cumulative.len() && cumulative[segment + 1] < distance {
                    segment += 1;
                }
                let seg_len = (cumulative[segment + 1] - cumulative[segment]).max(1e-6);
                let t = ((distance - cumulative[segment]) / seg_len).clamp(0.0, 1.0);
                let a = part[segment];
                let b = part[segment + 1];
                let anchor = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                let mut dir_x = (b.0 - a.0) / seg_len;
                let mut dir_y = (b.1 - a.1) / seg_len;
                if *reverse {
                    dir_x = -dir_x;
                    dir_y = -dir_y;
                }
                // Arrows on a lifted deck ride it (direction-gated, so a
                // road under a viaduct keeps its arrows on the ground).
                let arrow_corridors: &[BridgeCorridor] =
                    if bridge_dz_covered { &own_profiles } else { &bridge_corridors };
                let lift_m = if arrow_corridors.is_empty() {
                    0.0
                } else {
                    corridor_deck_at_point_dir(
                        anchor.0,
                        anchor.1,
                        (dir_x, dir_y),
                        arrow_corridors,
                    )
                };
                append_oneway_arrow(
                    anchor,
                    dir_x,
                    dir_y,
                    lift_m,
                    arrow_color,
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
                arrow_debug_appended += 1;
                distance += arrow_interval;
            }
        }
    }

    // Arrow debug: enabled by the presence of /tmp/mp_arrow_debug (worker
    // log! doesn't surface through the studio bridge, and env vars don't
    // reach a studio-launched app).
    if std::path::Path::new("/tmp/mp_arrow_debug").exists() {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/mp_arrow_debug/log.txt")
        {
            let _ = writeln!(
                file,
                "z{} x{} y{} rz{} rs{:.1}: jobs {} appended {} interval {:.1}",
                tile_key.z, tile_key.x, tile_key.y, render_zoom, render_scale,
                arrow_jobs.len(), arrow_debug_appended, arrow_interval
            );
        }
    }

    compact_tile_labels(&mut labels);

    TileBuffers {
        pin_hits,
        fill_indices,
        fill_vertices,
        casing_indices,
        casing_vertices,
        stroke_indices,
        stroke_vertices,
        icon_indices,
        icon_vertices,
        feature_count,
        labels,
        render_zoom,
    }
}

/// Shape id telling the map vertex shader to treat (param1, param2) as a
/// screen-px offset added AFTER the map transform (zoom-constant symbols).
pub const ICON_SHAPE_ID: f32 = 20.0;

fn ring_centroid(ring: &[(f32, f32)]) -> (f32, f32) {
    if ring.is_empty() {
        return (0.0, 0.0);
    }
    let mut sum = (0.0_f32, 0.0_f32);
    for point in ring {
        sum.0 += point.0;
        sum.1 += point.1;
    }
    (sum.0 / ring.len() as f32, sum.1 / ring.len() as f32)
}

/// Screen-px arrow glyph (shaft + head, +x = travel direction) as
/// zoom-constant anchor+offset vertices like the POI symbols.
fn append_oneway_arrow(
    anchor: (f32, f32),
    dir_x: f32,
    dir_y: f32,
    lift_m: f32,
    color: [f32; 4],
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    const SHAPE: [(f32, f32); 7] = [
        (-6.0, -0.9),
        (0.5, -0.9),
        (0.5, 0.9),
        (-6.0, 0.9),
        (0.5, -3.0),
        (6.0, 0.0),
        (0.5, 3.0),
    ];
    const INDICES: [u32; 9] = [0, 1, 2, 0, 2, 3, 4, 5, 6];
    // Icon param4 encodes zoom_floor + lift-quanta*100 (0.25 m each). Ceil
    // to the next quantum: arrows hug the deck from just above instead of
    // floating up to a meter over it.
    let lift_encoded = (lift_m.max(0.0) * 4.0).ceil() * 100.0;
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (x, y) in SHAPE {
        let ox = x * dir_x - y * dir_y;
        let oy = x * dir_y + y * dir_x;
        // param3 = 1.0: the offset is map-aligned (road direction) and must
        // rotate with the camera, unlike upright billboard POI symbols.
        // Above every road surface tier and deck bump — arrows behave
        // like 2D icons (always over the roads), which is the flat-mode
        // semantics we mirror.
        out_vertices.extend_from_slice(&[
            anchor.0, anchor.1, 0.5, 1.0, color[0], color[1], color[2], color[3], 1e6, 0.0,
            ICON_SHAPE_ID, 0.0, ox, oy, 1.0, lift_encoded, 0.85, 16.0, *zbias,
        ]);
    }
    for index in INDICES {
        out_indices.push(base + index);
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

fn append_icon_mesh(
    mesh: &IconMesh,
    anchor: (f32, f32),
    color: [f32; 4],
    min_zoom: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    append_icon_mesh_offset(
        mesh,
        anchor,
        (0.0, 0.0),
        color,
        min_zoom,
        out_vertices,
        out_indices,
        zbias,
    )
}

/// Like append_icon_mesh with an extra SCREEN-px offset added to every
/// vertex — lets shared meshes (digits) compose inside a pin badge while
/// staying zoom-constant with it.
#[allow(clippy::too_many_arguments)]
fn append_icon_mesh_offset(
    mesh: &IconMesh,
    anchor: (f32, f32),
    screen_offset: (f32, f32),
    color: [f32; 4],
    min_zoom: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    append_icon_mesh_offset_scaled(
        mesh,
        anchor,
        screen_offset,
        1.0,
        color,
        min_zoom,
        out_vertices,
        out_indices,
        zbias,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_icon_mesh_offset_scaled(
    mesh: &IconMesh,
    anchor: (f32, f32),
    screen_offset: (f32, f32),
    scale: f32,
    color: [f32; 4],
    min_zoom: f32,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for vertex in &mesh.verts {
        out_vertices.extend_from_slice(&[
            anchor.0,
            anchor.1,
            vertex.u,
            vertex.v,
            color[0],
            color[1],
            color[2],
            color[3],
            1e6, // stroke_mult: fill
            vertex.stroke_dist,
            ICON_SHAPE_ID,
            0.0,      // param0: solid color
            // param1/2: screen-px offset from the anchor
            vertex.x * scale + screen_offset.0,
            vertex.y * scale + screen_offset.1,
            0.0,
            // param4: this icon's view-zoom floor; the shader collapses the
            // vertex when the live view zoom is below it (no stale flash).
            min_zoom,
            // Tilt depth: a SMALL camera-ward bias -- enough to clear the
            // marker's own ground pixel (fill/stroke micro-ranks are tiny),
            // small enough that buildings meaningfully in FRONT occlude
            // the marker, keeping the 3D illusion honest.
            0.35,
            24.0, // clip_radius: generous, avoids pop-in at view edges
            *zbias,
        ]);
    }
    for index in &mesh.indices {
        out_indices.push(base + index);
    }
    *zbias += VECTOR_ZBIAS_STEP;
}

fn project_way_points_with_nodes(
    node_ids: &[i64],
    nodes: &HashMap<i64, (f64, f64)>,
    tile_key: TileKey,
    tile_origin: Vec2d,
    render_scale: f32,
) -> Vec<(i64, (f32, f32))> {
    let mut out = Vec::with_capacity(node_ids.len());
    let mut last: Option<(f32, f32)> = None;
    // Drop detail below ~a third of a screen pixel AT THE STYLED ZOOM —
    // invisible, but it dominates vertex volume at low buckets (a z14 tile
    // holds ~60K building ring points). Scale-aware, unlike the old fixed
    // source-zoom filter that ate visible corners when overzoomed.
    let min_dist = 0.35 / render_scale.max(0.001);
    let min_dist_sq = min_dist * min_dist;

    for node_id in node_ids {
        let Some((lon, lat)) = nodes.get(node_id).copied() else {
            continue;
        };
        let world = lon_lat_to_world(lon, lat, tile_key.z) - tile_origin;
        let point = (world.x as f32, world.y as f32);

        if let Some(prev) = last {
            let dx = point.0 - prev.0;
            let dy = point.1 - prev.1;
            if dx * dx + dy * dy < min_dist_sq {
                continue;
            }
        }

        out.push((*node_id, point));
        last = Some(point);
    }

    out
}

// --- Local mbtiles loading ---

/// One decoded overlay tile handed to the tile builder: raw MVT bytes plus
/// the ancestor shift (0 = exact zoom) and the quadrant offsets that map the
/// ancestor's local space into this tile's.
pub struct OverlayTileData {
    pub raw: Vec<u8>,
    pub shift: u32,
    pub quadrant_x: u32,
    pub quadrant_y: u32,
    /// 0 = all features, 1 = fast chargers (>=50 kW), 2 = slow chargers.
    pub filter: u8,
    /// Source is a charger overlay: base-map charging_station icons are
    /// suppressed as duplicates while one is active.
    pub has_chargers: bool,
}

fn overlay_zoom_range(reader: &mut MbtilesReader) -> (u32, u32) {
    let metadata = reader.get_metadata().unwrap_or_default();
    let parse = |key: &str, fallback: u32| {
        metadata
            .get(key)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(fallback)
    };
    (parse("minzoom", 0), parse("maxzoom", 30))
}

pub fn load_local_tile_batch(
    mbtiles_path: &Path,
    detail_mbtiles_path: Option<&Path>,
    bridge_dz_mbtiles_path: Option<&Path>,
    overlay_paths: &[String],
    requested: &[TileKey],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
    union_roads: bool,
) -> Result<(Vec<LoadedLocalTile>, Vec<TileKey>), String> {
    if requested.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    // Baked bridge-dz overlay (bridge.md M1/M2): solved per-vertex road
    // elevation. Coverage is the archive's metadata bounds — inside them the
    // solved profile replaces every tag-based deck heuristic, including for
    // tiles that simply have no elevated roads.
    let mut bridge_dz = bridge_dz_mbtiles_path
        .filter(|path| path.is_file())
        .and_then(|path| MbtilesReader::open(path).ok())
        .and_then(|mut reader| {
            let meta = reader.get_metadata().unwrap_or_default();
            let zoom = meta.get("minzoom").and_then(|z| z.parse::<u32>().ok())?;
            let bounds: Vec<f64> = meta
                .get("bounds")?
                .split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect();
            if bounds.len() != 4 {
                return None;
            }
            Some((reader, zoom, [bounds[0], bounds[1], bounds[2], bounds[3]]))
        });
    let mut fetch_bridge_dz = |tile_key: TileKey| -> (Option<Vec<u8>>, bool) {
        let Some((reader, zoom, bounds)) = bridge_dz.as_mut() else {
            return (None, false);
        };
        if tile_key.z != *zoom {
            return (None, false);
        }
        let n = (1u64 << tile_key.z) as f64;
        let west = tile_key.x as f64 / n * 360.0 - 180.0;
        let east = (tile_key.x as f64 + 1.0) / n * 360.0 - 180.0;
        let lat = |y: f64| {
            (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees()
        };
        let north = lat(tile_key.y as f64);
        let south = lat(tile_key.y as f64 + 1.0);
        let covered =
            west >= bounds[0] && east <= bounds[2] && south >= bounds[1] && north <= bounds[3];
        if !covered {
            return (None, false);
        }
        let tms_row = (1_i64 << tile_key.z) - 1 - tile_key.y as i64;
        let raw = reader
            .get_tile(tile_key.z as i64, tile_key.x as i64, tms_row)
            .ok()
            .flatten();
        (raw, true)
    };

    // Path entries may carry a "?fast" / "?slow" charger-power filter.
    let mut overlay_readers: Vec<(MbtilesReader, u32, u32, u8, bool)> = overlay_paths
        .iter()
        .filter(|path| !path.is_empty())
        .filter_map(|path| {
            let (file, filter) = match path.split_once('?') {
                Some((file, "fast")) => (file, 1u8),
                Some((file, "slow")) => (file, 2),
                Some((file, _)) => (file, 0),
                None => (path.as_str(), 0),
            };
            let has_chargers = file.contains("chargers");
            MbtilesReader::open(Path::new(file))
                .ok()
                .map(|reader| (reader, filter, has_chargers))
        })
        .map(|(mut reader, filter, has_chargers)| {
            let (min_zoom, max_zoom) = overlay_zoom_range(&mut reader);
            (reader, min_zoom, max_zoom, filter, has_chargers)
        })
        .collect();

    let mut fetch_overlays = |tile_key: TileKey| -> Vec<OverlayTileData> {
        let mut out = Vec::new();
        for (reader, min_zoom, max_zoom, filter, has_chargers) in overlay_readers.iter_mut() {
            if tile_key.z < *min_zoom {
                continue;
            }
            let shift = tile_key.z.saturating_sub(*max_zoom);
            let fetch_z = tile_key.z - shift;
            let fetch_x = (tile_key.x as u32 >> shift) as i64;
            let fetch_y = (tile_key.y as u32 >> shift) as i64;
            let tms_row = (1_i64 << fetch_z) - 1 - fetch_y;
            if let Ok(Some(raw)) = reader.get_tile(fetch_z as i64, fetch_x, tms_row) {
                out.push(OverlayTileData {
                    raw,
                    shift,
                    quadrant_x: (tile_key.x as u32) - ((fetch_x as u32) << shift),
                    quadrant_y: (tile_key.y as u32) - ((fetch_y as u32) << shift),
                    filter: *filter,
                    has_chargers: *has_chargers,
                });
            }
        }
        out
    };

    // The MBTiles archive is already the local, seekable tile cache. Do not
    // duplicate it into millions of generated JSON files.
    let mut loaded = Vec::<LoadedLocalTile>::new();
    let mut decode_failed = Vec::<TileKey>::new();
    let missing = requested;

    let mut reader = MbtilesReader::open(mbtiles_path)
        .map_err(|err| format!("open {}: {}", mbtiles_path.display(), err))?;

    // Optional all-tag detail overlay (micro-POIs + 2.5D buildings); only
    // consulted at the zooms that use it.
    let want_detail = render_zoom >= ICON_MIN_ZOOM
        || (buildings_3d && render_zoom >= 14);
    let mut detail_reader = if want_detail {
        detail_mbtiles_path
            .filter(|path| path.is_file())
            .and_then(|path| MbtilesReader::open(path).ok())
    } else {
        None
    };

    let mut by_zoom = HashMap::<u32, Vec<TileKey>>::new();
    for key in missing {
        by_zoom.entry(key.z).or_default().push(*key);
    }

    let mut logged_xyz_row_scheme = false;

    for (zoom, mut keys) in by_zoom {
        let tile_count = 1_i64 << zoom;

        if reader.supports_direct_tile_lookup() {
            // Match the writer's block-major rowid order to keep visible-tile
            // reads close together on disk.
            keys.sort_unstable_by_key(|key| {
                (key.y >> 8, key.x >> 8, key.y & 255, key.x & 255)
            });
            let mut unavailable = Vec::new();
            for tile_key in keys {
                let tms_row = tile_count - 1 - tile_key.y as i64;
                let raw = reader
                    .get_tile(zoom as i64, tile_key.x as i64, tms_row)
                    .map_err(|err| {
                        format!(
                            "read tile z{} x{} y{} from {}: {}",
                            tile_key.z,
                            tile_key.x,
                            tile_key.y,
                            mbtiles_path.display(),
                            err
                        )
                    })?;
                let Some(raw) = raw else {
                    unavailable.push(tile_key);
                    continue;
                };
                let detail_raw = detail_reader
                    .as_mut()
                    .and_then(|reader| reader.get_tile(zoom as i64, tile_key.x as i64, tms_row).ok())
                    .flatten();
                let overlay_tiles = fetch_overlays(tile_key);
                let (bridge_dz_raw, bridge_dz_covered) = fetch_bridge_dz(tile_key);

                match build_tile_buffers_from_mvt(
                    tile_key,
                    &raw,
                    detail_raw.as_deref(),
                    bridge_dz_raw.as_deref(),
                    bridge_dz_covered,
                    &overlay_tiles,
                    theme,
                    render_zoom,
                    buildings_3d,
                    union_roads,
                ) {
                    Ok(buffers) => {
                        loaded.push(LoadedLocalTile { tile_key, buffers });
                    }
                    Err(err) => {
                        decode_failed.push(tile_key);
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
            if !unavailable.is_empty() {
                unavailable.sort_unstable();
                log!(
                    "MapView: local mbtiles missing {} tile(s) at z{} sample:{}",
                    unavailable.len(),
                    zoom,
                    format_tile_key_sample(&unavailable, 8)
                );
            }
            continue;
        }

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

            let detail_raw = detail_reader
                .as_mut()
                .and_then(|reader| {
                    let tms_row = tile_count - 1 - tile_key.y as i64;
                    reader
                        .get_tile(zoom as i64, tile_key.x as i64, tms_row)
                        .ok()
                })
                .flatten();
            let overlay_tiles = fetch_overlays(tile_key);
            let (bridge_dz_raw, bridge_dz_covered) = fetch_bridge_dz(tile_key);
            match build_tile_buffers_from_mvt(
                tile_key,
                &tile.tile_data,
                detail_raw.as_deref(),
                bridge_dz_raw.as_deref(),
                bridge_dz_covered,
                &overlay_tiles,
                theme,
                render_zoom,
                buildings_3d,
                union_roads,
            ) {
                Ok(buffers) => {
                    loaded.push(LoadedLocalTile { tile_key, buffers });
                }
                Err(err) => {
                    decode_failed.push(tile_key);
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

    Ok((loaded, decode_failed))
}

// --- MVT (Mapbox Vector Tile) parsing ---

/// Receives decoded MVT features (tile-local integer geometry + tags).
pub trait MvtSink {
    fn alloc_feature_id(&mut self) -> u64;
    fn add_path(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        tags: HashMap<String, String>,
        close: bool,
    );
    fn add_point(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        point: (i32, i32),
        tags: HashMap<String, String>,
    );
}

/// Collects MVT features directly in tile-local f32 coordinates with
/// scale-aware vertex thinning — the typed replacement for the old
/// MVT -> Overpass-JSON -> parse round trip.
struct MvtLocalCollector {
    min_dist_sq: f32,
    next_feature_id: u64,
    ways: Vec<TileWay>,
    points: Vec<((f32, f32), HashMap<String, String>)>,
    /// Baked per-vertex deck heights keyed (source layer, feature index,
    /// path index) — joined to paths during collection.
    base_dz: HashMap<(String, u32, u32), Vec<f32>>,
}

impl MvtLocalCollector {
    fn new(render_scale: f32) -> Self {
        let min_dist = 0.35 / render_scale.max(0.001);
        Self {
            min_dist_sq: min_dist * min_dist,
            next_feature_id: 1,
            ways: Vec::new(),
            points: Vec::new(),
            base_dz: HashMap::new(),
        }
    }
}

impl MvtSink for MvtLocalCollector {
    fn alloc_feature_id(&mut self) -> u64 {
        let id = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.wrapping_add(1).max(1);
        id
    }

    fn add_path(
        &mut self,
        _tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        mut tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        // Baked per-vertex dz joins on (source layer, feature idx, path
        // idx) — the exact geometry this path decodes to, no matching.
        let feature_index = tags.remove(MVT_INTERNAL_FIDX_KEY);
        let path_index = tags.remove(MVT_INTERNAL_PIDX_KEY);
        let dz_raw: Option<&Vec<f32>> = if self.base_dz.is_empty() {
            None
        } else {
            match (tags.get("layer"), feature_index, path_index) {
                (Some(layer), Some(fidx), Some(pidx)) => {
                    match (fidx.parse::<u32>(), pidx.parse::<u32>()) {
                        (Ok(fidx), Ok(pidx)) => {
                            let dz = self.base_dz.get(&(layer.clone(), fidx, pidx));
                            // Length must match the raw path or the bake
                            // enumeration diverged — refuse silently.
                            dz.filter(|dz| dz.len() == points.len())
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        let mut out = Vec::<(f32, f32)>::with_capacity(points.len() + 1);
        let mut out_dz = Vec::<f32>::new();
        let mut last: Option<(f32, f32)> = None;
        for (index, &(x, y)) in points.iter().enumerate() {
            let point = (x as f32 * scale, y as f32 * scale);
            if let Some(prev) = last {
                let dx = point.0 - prev.0;
                let dy = point.1 - prev.1;
                if dx * dx + dy * dy < self.min_dist_sq {
                    continue;
                }
            }
            out.push(point);
            if let Some(dz) = dz_raw {
                out_dz.push(dz[index]);
            }
            last = Some(point);
        }
        if out.len() < 2 {
            return;
        }
        if close {
            if out.first() != out.last() {
                out.push(out[0]);
                if !out_dz.is_empty() {
                    out_dz.push(out_dz[0]);
                }
            }
            if out.len() < 4 {
                return;
            }
        }
        let dz = (dz_raw.is_some() && out_dz.iter().any(|&v| v > 0.05)).then_some(out_dz);
        self.ways.push(TileWay {
            points: out,
            tags,
            closed: close,
            dz,
        });
    }

    fn add_point(
        &mut self,
        _tile_key: TileKey,
        extent: u32,
        point: (i32, i32),
        tags: HashMap<String, String>,
    ) {
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        self.points
            .push(((point.0 as f32 * scale, point.1 as f32 * scale), tags));
    }
}

pub fn decode_vector_tile_payload(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        return gzip_decompress_vec(raw).map_err(|e| format!("gzip decode failed: {}", e));
    }
    if raw.len() >= 2 && raw[0] == 0x78 {
        if let Ok(out) = zlib_decompress_vec(raw) {
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

pub fn parse_mvt_tile(
    tile_data: &[u8],
    tile_key: TileKey,
    builder: &mut impl MvtSink,
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
    builder: &mut impl MvtSink,
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
    for (feature_index, feature_data) in features.into_iter().enumerate() {
        parse_mvt_feature(
            feature_index as u32,
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
    feature_index: u32,
    feature_data: &[u8],
    layer_name: &str,
    keys: &[String],
    values: &[MvtValue],
    extent: u32,
    tile_key: TileKey,
    builder: &mut impl MvtSink,
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
        // Join keys for the baked base_dz overlay: feature index within
        // the source layer + path index within the feature, in decode
        // order (the bake tool enumerates identically).
        path_tags.insert(MVT_INTERNAL_FIDX_KEY.to_string(), feature_index.to_string());
        path_tags.insert(MVT_INTERNAL_PIDX_KEY.to_string(), ring_index.to_string());
        builder.add_path(tile_key, extent, &path, path_tags, close);
    }

    Ok(())
}

fn normalize_mvt_tags(
    layer_name: &str,
    geom_type: MvtGeomType,
    tags: &mut HashMap<String, String>,
) {
    // The source-layer name OWNS the "layer" key. OSM's own layer=-1/1
    // stacking tag collides with it and silently broke recognition of any
    // layer-tagged feature (the Artis zoo way, bridges, tunnels) — keep
    // the OSM value under "osm_layer" instead.
    if let Some(previous) = tags.insert("layer".to_string(), layer_name.to_string()) {
        if previous != layer_name {
            tags.insert("osm_layer".to_string(), previous);
        }
    }

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
    match layer.as_str() {
        "addresses" => tags
            .get("housenumber")
            .or_else(|| tags.get("housename"))
            .is_some_and(|value| !value.trim().is_empty()),
        "pois" => select_label_text(tags).is_some(),
        // All-tag detail archive points pass through; the micro-POI
        // whitelist decides downstream what actually draws.
        "osm_points" => true,
        "water_polygons_labels" => select_label_text(tags).is_some(),
        // Geodata overlay point layers (layers.md).
        "chargers" | "stops" => true,
        // Settlement names (city/town/suburb…).
        "place_labels" => select_label_text(tags).is_some(),
        _ => {
            is_road_point_label_layer(layer)
                && tags.contains_key("highway")
                && select_label_text(tags).is_some()
        }
    }
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

/// CPU rasterizer for tile buffers: flat top view = paint in buffer
/// order (fill, casing, stroke, icon) and index order. Lets the union
/// generator iterate against the legacy reference headlessly.
pub fn raster_buffers(buffers: &TileBuffers, px_per_unit: f32) -> (usize, Vec<u8>) {
    let size = (256.0 * px_per_unit) as usize;
    let mut image = vec![255u8; size * size * 3];
    let f = 19usize;
    for (verts, indices) in [
        (&buffers.fill_vertices, &buffers.fill_indices),
        (&buffers.casing_vertices, &buffers.casing_indices),
        (&buffers.stroke_vertices, &buffers.stroke_indices),
        (&buffers.icon_vertices, &buffers.icon_indices),
    ] {
        for tri in indices.chunks_exact(3) {
            let mut ps = [[0.0f32; 2]; 3];
            let mut rgba = [0.0f32; 4];
            let mut us = [0.0f32; 3];
            let mut stroke_mult = 1.0f32;
            for (k, &vi) in tri.iter().enumerate() {
                let v = &verts[vi as usize * f..vi as usize * f + f];
                let (mut x, mut y) = (v[0], v[1]);
                if v[10] >= 100.0 {
                    x += v[12];
                    y += v[13];
                }
                // Icons (shape 20) offset in screen px — px_per_unit
                // maps them approximately for the diff.
                if (v[10] - 20.0).abs() < 0.5 {
                    x += v[12] / px_per_unit;
                    y += v[13] / px_per_unit;
                }
                ps[k] = [x * px_per_unit, y * px_per_unit];
                rgba = [v[4], v[5], v[6], v[7]];
                us[k] = v[2];
                stroke_mult = v[8].min(1e6);
            }
            if rgba[3] <= 0.004 {
                continue;
            }
            // Un-premultiply for blending over the framebuffer.
            let (cr, cg, cb) = if rgba[3] > 1e-4 {
                (rgba[0] / rgba[3], rgba[1] / rgba[3], rgba[2] / rgba[3])
            } else {
                (0.0, 0.0, 0.0)
            };
            let min_x = ps.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
            let max_x = (ps.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as usize).min(size - 1);
            let min_y = ps.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
            let max_y = (ps.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as usize).min(size - 1);
            if min_x > max_x || min_y > max_y {
                continue;
            }
            let area = (ps[1][0] - ps[0][0]) * (ps[2][1] - ps[0][1])
                - (ps[1][1] - ps[0][1]) * (ps[2][0] - ps[0][0]);
            if area.abs() < 1e-6 {
                continue;
            }
            for py in min_y..=max_y {
                for px in min_x..=max_x {
                    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let sign = area.signum();
                    // Edge functions double as barycentric weights: w[k]
                    // belongs to the vertex OPPOSITE edge (k, k+1) = k+2.
                    let mut ws = [0.0f32; 3];
                    let mut inside = true;
                    for k in 0..3 {
                        let (a, b) = (ps[k], ps[(k + 1) % 3]);
                        let e = ((b[0] - a[0]) * (fy - a[1])
                            - (b[1] - a[1]) * (fx - a[0]))
                            * sign;
                        if e < 0.0 {
                            inside = false;
                            break;
                        }
                        ws[(k + 2) % 3] = e;
                    }
                    if !inside {
                        continue;
                    }
                    // Coverage from the interpolated across-stroke u, the
                    // fragment shader's model (0/1 at AA edges, peak mid).
                    let wsum = (ws[0] + ws[1] + ws[2]).max(1e-9);
                    let u_pix =
                        (ws[0] * us[0] + ws[1] * us[1] + ws[2] * us[2]) / wsum;
                    let across = 1.0 - (u_pix * 2.0 - 1.0).abs();
                    let alpha = rgba[3] * (across * stroke_mult).clamp(0.0, 1.0);
                    if alpha <= 0.004 {
                        continue;
                    }
                    let o = (py * size + px) * 3;
                    for (c, channel) in [cr, cg, cb].into_iter().enumerate() {
                        image[o + c] = ((channel * 255.0) * alpha
                            + image[o + c] as f32 * (1.0 - alpha))
                            .clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
    (size, image)
}

pub fn write_ppm(path: &str, size: usize, image: &[u8]) {
    use std::io::Write as _;
    let mut file = std::fs::File::create(path).unwrap();
    write!(file, "P6\n{size} {size}\n255\n").unwrap();
    file.write_all(image).unwrap();
}


#[cfg(test)]
mod bridge_probe_tests {
    #[test]
    #[ignore]
    fn probe_rai_detail_tags() {
        use super::*;
        let (lon, lat) = (4.8895f64, 52.3405f64);
        let z = 14u32;
        let n = (1u64 << z) as f64;
        let nx = (lon + 180.0) / 360.0;
        let r = lat.to_radians();
        let ny = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0;
        let (tx, ty) = ((nx * n) as i64, (ny * n) as i64);
        let mut reader =
            MbtilesReader::open(Path::new("../local/maps/europe-osm-detail.mbtiles")).unwrap();
        let tms = (1i64 << z) - 1 - ty;
        let raw = reader.get_tile(z as i64, tx, tms).unwrap().unwrap();
        let data = decode_vector_tile_payload(&raw).unwrap();
        struct Dump;
        impl MvtSink for Dump {
            fn alloc_feature_id(&mut self) -> u64 {
                0
            }
            fn add_point(
                &mut self,
                _k: TileKey,
                _e: u32,
                _p: (i32, i32),
                _t: HashMap<String, String>,
            ) {
            }
            fn add_path(
                &mut self,
                _k: TileKey,
                _e: u32,
                _pts: &[(i32, i32)],
                tags: HashMap<String, String>,
                _close: bool,
            ) {
                let bridge = tags.get("bridge").map(|v| v.as_str()).unwrap_or("");
                if bridge == "yes" || bridge == "viaduct" {
                    let mut kv: Vec<String> = tags
                        .iter()
                        .filter(|(k, _)| !k.starts_with("__"))
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    kv.sort();
                    println!("DET {}", kv.join(" "));
                }
            }
        }
        let key = TileKey { z, x: tx as i32, y: ty as i32 };
        parse_mvt_tile(&data, key, &mut Dump).unwrap();
    }

    #[test]
    #[ignore] // needs local bake output
    fn probe_bridge_dz_load() {
        use super::*;
        let path = std::path::Path::new("../local/maps/ams-bridge-dz.mbtiles");
        assert!(path.is_file(), "no bake output at {}", path.display());
        let mut reader = MbtilesReader::open(path).unwrap();
        let meta = reader.get_metadata().unwrap_or_default();
        println!("meta minzoom={:?} bounds={:?}", meta.get("minzoom"), meta.get("bounds"));
        let (x, y) = (8414i64, 5387i64);
        let tms = (1i64 << 14) - 1 - y;
        let raw = reader.get_tile(14, x, tms).unwrap().expect("dz tile missing");
        println!("raw {} bytes", raw.len());
        let key = TileKey { z: 14, x: x as i32, y: y as i32 };
        let corridors = parse_bridge_dz_corridors(&raw, key).unwrap();
        println!("corridors: {}", corridors.len());
        for corridor in corridors.iter().take(5) {
            let max = corridor.decks.iter().fold(0.0f32, |a, &b| a.max(b));
            println!(
                "  pts {} decks max {:.1} hw {:.2} first ({:.1},{:.1})",
                corridor.points.len(),
                max,
                corridor.half_width,
                corridor.points[0].0,
                corridor.points[0].1
            );
        }
        assert!(!corridors.is_empty());

        // Full pipeline: fetch + parse + corridor match into GPU buffers.
        let theme = CompiledMapTheme::default();
        let keys = vec![TileKey { z: 14, x: 8414, y: 5387 }];
        let (loaded, failed) = load_local_tile_batch(
            std::path::Path::new("../local/maps/europe-shortbread.mbtiles"),
            Some(std::path::Path::new("../local/maps/europe-osm-detail.mbtiles")),
            Some(path),
            &[],
            &keys,
            &theme,
            17,
            true,
            true,
        )
        .unwrap();
        println!("loaded {} failed {}", loaded.len(), failed.len());
        let buffers = &loaded[0].buffers;
        let floats_per_vertex = 19;
        let mut decked = 0usize;
        let mut max_deck = 0.0f32;
        for chunk in buffers.stroke_vertices.chunks_exact(floats_per_vertex) {
            let deck = chunk[15];
            if deck > 0.3 {
                decked += 1;
                max_deck = max_deck.max(deck);
            }
        }
        println!(
            "stroke verts {} decked {} max {:.1}",
            buffers.stroke_vertices.len() / floats_per_vertex,
            decked,
            max_deck
        );
        assert!(decked > 0, "no decked stroke vertices — dz not reaching geometry");

        // Join diagnostics: how many base paths find their dz, and how many
        // fail the length check.
        let raw2 = reader.get_tile(14, x, tms).unwrap().unwrap();
        let map = parse_base_dz_map(&raw2, key).unwrap();
        println!("base_dz map entries: {}", map.len());
        struct JoinProbe {
            map: HashMap<(String, u32, u32), Vec<f32>>,
            hit: usize,
            len_mismatch: usize,
            miss: usize,
            oneway: usize,
            oneway_values: Vec<String>,
        }
        impl MvtSink for JoinProbe {
            fn alloc_feature_id(&mut self) -> u64 {
                1
            }
            fn add_path(
                &mut self,
                _tile_key: TileKey,
                _extent: u32,
                points: &[(i32, i32)],
                tags: HashMap<String, String>,
                _close: bool,
            ) {
                let (Some(layer), Some(fidx), Some(pidx)) = (
                    tags.get("layer"),
                    tags.get(MVT_INTERNAL_FIDX_KEY),
                    tags.get(MVT_INTERNAL_PIDX_KEY),
                ) else {
                    return;
                };
                let key = (
                    layer.clone(),
                    fidx.parse::<u32>().unwrap_or(9999),
                    pidx.parse::<u32>().unwrap_or(9999),
                );
                if tags.get("layer").map(|v| v.as_str()) == Some("streets") {
                    if let Some(value) = tags.get("oneway") {
                        self.oneway += 1;
                        if !self.oneway_values.contains(value) {
                            self.oneway_values.push(value.clone());
                        }
                    }
                }
                match self.map.get(&key) {
                    Some(dz) if dz.len() == points.len() => self.hit += 1,
                    Some(dz) => {
                        self.len_mismatch += 1;
                        if self.len_mismatch <= 5 {
                            println!(
                                "  len mismatch {:?}: dz {} vs points {}",
                                key,
                                dz.len(),
                                points.len()
                            );
                        }
                    }
                    None => self.miss += 1,
                }
            }
            fn add_point(
                &mut self,
                _tile_key: TileKey,
                _extent: u32,
                _point: (i32, i32),
                _tags: HashMap<String, String>,
            ) {
            }
        }
        let mut probe = JoinProbe {
            map,
            hit: 0,
            len_mismatch: 0,
            miss: 0,
            oneway: 0,
            oneway_values: Vec::new(),
        };
        let base_raw = MbtilesReader::open(std::path::Path::new(
            "../local/maps/europe-shortbread.mbtiles",
        ))
        .unwrap()
        .get_tile(14, x, tms)
        .unwrap()
        .unwrap();
        let base_pbf = decode_vector_tile_payload(&base_raw).unwrap();
        parse_mvt_tile(&base_pbf, key, &mut probe).unwrap();
        println!(
            "join: hit {} len_mismatch {} miss {} oneway {} values {:?}",
            probe.hit, probe.len_mismatch, probe.miss, probe.oneway, probe.oneway_values
        );

        // Oneway arrows: count map-aligned icon glyphs and their lifts.
        let mut arrows = 0;
        let mut lifted_arrows = 0;
        for chunk in buffers.icon_vertices.chunks_exact(floats_per_vertex) {
            let shape = chunk[10];
            let param3 = chunk[14];
            let param4 = chunk[15];
            if (shape - 20.0).abs() < 0.1 && (param3 - 1.0).abs() < 0.1 {
                arrows += 1;
                if param4 >= 100.0 {
                    lifted_arrows += 1;
                }
            }
        }
        println!("arrow verts {} lifted {}", arrows, lifted_arrows);
    }

    #[test]
    #[ignore] // needs local archives; headless union-vs-reference A/B
    fn union_ab_raster() {
        let theme = CompiledMapTheme::default();
        // Raampoort tile.
        let keys = vec![TileKey { z: 14, x: 8413, y: 5385 }];
        let out_dir = std::env::var("AB_OUT").unwrap_or_else(|_| "/tmp".into());
        let mut images: Vec<(usize, Vec<u8>)> = Vec::new();
        for union in [false, true] {
            let (loaded, _) = load_local_tile_batch(
                std::path::Path::new("../local/maps/europe-shortbread.mbtiles"),
                Some(std::path::Path::new("../local/maps/europe-osm-detail.mbtiles")),
                Some(std::path::Path::new("../local/maps/ams-bridge-dz.mbtiles")),
                &[],
                &keys,
                &theme,
                17,
                false,
                union,
            )
            .unwrap();
            let (size, image) = super::raster_buffers(&loaded[0].buffers, 8.0);
            super::write_ppm(
                &format!("{out_dir}/ab_{}.ppm", if union { "union" } else { "ref" }),
                size,
                &image,
            );
            images.push((size, image));
        }
        let (size, reference) = &images[0];
        let (_, union_img) = &images[1];
        let mut diff = vec![255u8; size * size * 3];
        let mut differing = 0usize;
        for i in 0..size * size {
            let o = i * 3;
            let delta = (0..3)
                .map(|c| (reference[o + c] as i32 - union_img[o + c] as i32).abs())
                .max()
                .unwrap();
            if delta > 24 {
                differing += 1;
                diff[o] = 255;
                diff[o + 1] = 0;
                diff[o + 2] = 0;
            } else if delta > 8 {
                diff[o] = 255;
                diff[o + 1] = 200;
                diff[o + 2] = 0;
            }
        }
        super::write_ppm(&format!("{out_dir}/ab_diff.ppm"), *size, &diff);
        println!(
            "diff pixels >24: {differing} of {} ({:.2}%)",
            size * size,
            differing as f64 / (size * size) as f64 * 100.0
        );
    }

    #[test]
    #[ignore]
    fn probe_rai_bridge_tags() {
        use super::*;
        let (lon, lat) = (4.8895f64, 52.3405f64);
        let z = 14u32;
        let n = (1u64 << z) as f64;
        let nx = (lon + 180.0) / 360.0;
        let r = lat.to_radians();
        let ny = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0;
        let (tx, ty) = ((nx * n) as i64, (ny * n) as i64);
        let mut reader = MbtilesReader::open(Path::new("../local/maps/europe-shortbread.mbtiles"))
            .or_else(|_| {
                MbtilesReader::open(Path::new("local/maps/europe-shortbread.mbtiles"))
            })
            .unwrap();
        let tms = (1i64 << z) - 1 - ty;
        let raw = reader.get_tile(z as i64, tx, tms).unwrap().unwrap();
        let data = decode_vector_tile_payload(&raw).unwrap();
        struct Dump;
        impl MvtSink for Dump {
            fn alloc_feature_id(&mut self) -> u64 {
                0
            }
            fn add_point(
                &mut self,
                _k: TileKey,
                _e: u32,
                _p: (i32, i32),
                _t: HashMap<String, String>,
            ) {
            }
            fn add_path(
                &mut self,
                _k: TileKey,
                _e: u32,
                _pts: &[(i32, i32)],
                tags: HashMap<String, String>,
                _close: bool,
            ) {
                let layer = tags.get("layer").cloned().unwrap_or_default();
                let name = tags.get("name").cloned().unwrap_or_default();
                let interesting = name.contains("brug")
                    || name.contains("Europaboulevard")
                    || tags.contains_key("bridge");
                if interesting && matches!(layer.as_str(), "streets" | "street_polygons" | "bridges") {
                    let mut kv: Vec<String> = tags
                        .iter()
                        .filter(|(k, _)| !k.starts_with("__"))
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    kv.sort();
                    println!("[{layer}] {}", kv.join(" "));
                }
            }
        }
        let key = TileKey { z, x: tx as i32, y: ty as i32 };
        parse_mvt_tile(&data, key, &mut Dump).unwrap();
    }

    use super::*;

    #[test]
    #[ignore]
    fn westerkerk_probe() {
        let detail = std::path::Path::new("../local/maps/europe-osm-detail.mbtiles");
        if !detail.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::MbtilesReader::open(detail).unwrap();
        let (z, x, y) = (14i64, 8414i64, 5384i64);
        let raw = reader.get_tile(z, x, (1 << z) - 1 - y).unwrap().unwrap();
        let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
        let pbf = decode_vector_tile_payload(&raw).unwrap();
        let mut collector = MvtLocalCollector::new(4.0);
        parse_mvt_tile(&pbf, key, &mut collector).unwrap();
        let mut by_layer = std::collections::HashMap::<String, usize>::new();
        for way in &collector.ways {
            let layer = way.tags.get("layer").cloned().unwrap_or_default();
            *by_layer.entry(layer).or_default() += 1;
            if way.tags.contains_key("building:part") {
                println!(
                    "PART layer={} closed={} pts={} id={:?} h={:?} min={:?}",
                    way.tags.get("layer").cloned().unwrap_or_default(),
                    way.closed,
                    way.points.len(),
                    way.tags.get("__makepad_osm_id"),
                    way.tags.get("height"),
                    way.tags.get("min_height"),
                );
            }
        }
        let mut stats: Vec<_> = by_layer.into_iter().collect();
        stats.sort();
        println!("LAYER STATS {:?}", stats);
    }

    #[test]
    #[ignore]
    fn place_labels_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        if !base.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        // Amsterdam's own tile: dump raw place kinds.
        {
            let (z, x, y) = (10i64, 525i64, 336i64);
            if let Some(raw) = reader.get_tile(z, x, (1 << z) - 1 - y).unwrap() {
                let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
                let pbf = decode_vector_tile_payload(&raw).unwrap();
                let mut collector = MvtLocalCollector::new(1.0);
                parse_mvt_tile(&pbf, key, &mut collector).unwrap();
                for (_, tags) in &collector.points {
                    if tags.get("layer").map(|v| v.as_str()) == Some("place_labels") {
                        let name = tags.get("name").cloned().unwrap_or_default();
                        if name.contains("Amsterdam") || name.contains("Haarlem") {
                            let mut t: Vec<_> = tags.iter().collect();
                            t.sort();
                            println!("PLACE {:?}", t);
                        }
                    }
                }
            }
        }
        for (z, x, y) in [(11i64, 1052i64, 674i64), (10, 526, 337), (8, 131, 84)] {
            let raw = reader.get_tile(z, x, (1 << z) - 1 - y).unwrap().unwrap();
            let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
            let theme = CompiledMapTheme::default();
            let buffers =
                build_tile_buffers_from_mvt(key, &raw, None, None, false, &[], &theme, z as u32, false, true)
                    .unwrap();
            let places: Vec<&TileLabel> = buffers
                .labels
                .iter()
                .filter(|l| l.source_layer == "place_labels")
                .collect();
            let streets = buffers
                .labels
                .iter()
                .filter(|l| l.source_layer.starts_with("street"))
                .count();
            println!(
                "z{} labels total {} places {} streets {}",
                z,
                buffers.labels.len(),
                places.len(),
                streets
            );
            for label in places.iter().take(5) {
                println!("  {:?} kind={} prio={}", label.text, label.road_kind, label.priority);
            }
        }
    }

    #[test]
    #[ignore]
    fn overlay_batch_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let overlay = "../local/overlays/nl-chargers.mbtiles".to_string();
        let transit = "../local/overlays/nl-transit.mbtiles".to_string();
        if !base.exists() {
            return;
        }
        let theme = CompiledMapTheme::default();
        let keys = vec![TileKey { z: 12, x: 2103, y: 1346 }];
        let loaded = load_local_tile_batch(
            base,
            None,
            None,
            &[overlay, transit],
            &keys,
            &theme,
            12,
            false,
            true,
        )
        .unwrap();
        for tile in &loaded.0 {
            println!(
                "tile z{} icons {} strokes {} labels {}",
                tile.tile_key.z,
                tile.buffers.icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
                tile.buffers.stroke_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
                tile.buffers.labels.len()
            );
        }
    }

    #[test]
    #[ignore]
    fn overlay_chargers_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let overlay = std::path::Path::new("../local/overlays/nl-chargers.mbtiles");
        if !base.exists() || !overlay.exists() {
            return;
        }
        let mut base_reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        let mut overlay_reader = makepad_mbtile_reader::MbtilesReader::open(overlay).unwrap();
        let (z, x, y) = (12i64, 2103i64, 1346i64);
        let raw = base_reader.get_tile(z, x, (1 << z) - 1 - y).unwrap().unwrap();
        let ov = overlay_reader
            .get_tile(z, x, (1 << z) - 1 - y)
            .unwrap()
            .unwrap();
        let key = TileKey { z: z as u32, x: x as i32, y: y as i32 };
        let overlay_tiles = vec![OverlayTileData {
            raw: ov,
            shift: 0,
            quadrant_x: 0,
            quadrant_y: 0,
            filter: 0,
            has_chargers: true,
        }];
        let theme = CompiledMapTheme::default();
        let buffers =
            build_tile_buffers_from_mvt(key, &raw, None, None, false, &overlay_tiles, &theme, 12, false, true)
                .unwrap();
        println!(
            "icon verts {} labels {} features {}",
            buffers.icon_vertices.len() / VECTOR_FLOATS_PER_VERTEX,
            buffers.labels.len(),
            buffers.feature_count
        );
    }

    #[test]
    #[ignore]
    fn artis_full_build_probe() {
        let base = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        let detail = std::path::Path::new("../local/maps/noord-holland-detail.mbtiles");
        if !base.exists() || !detail.exists() {
            return;
        }
        let mut base_reader = makepad_mbtile_reader::MbtilesReader::open(base).unwrap();
        let mut detail_reader = makepad_mbtile_reader::MbtilesReader::open(detail).unwrap();
        let y = 5384i64;
        let key = TileKey { z: 14, x: 8415, y: y as i32 };
        let raw = base_reader.get_tile(14, 8415, 16383 - y).unwrap().unwrap();
        let det = detail_reader.get_tile(14, 8415, 16383 - y).unwrap();
        let theme = CompiledMapTheme::default();
        let buffers =
            build_tile_buffers_from_mvt(key, &raw, det.as_deref(), None, false, &[], &theme, 17, false, true)
                .unwrap();
        let attraction: Vec<&TileLabel> = buffers
            .labels
            .iter()
            .filter(|label| label.source_layer == "green_area")
            .collect();
        println!("green_area labels: {}", attraction.len());
        for label in attraction.iter() {
            if label.color_class == 3 {
                println!("  ATTRACTION {:?}", label.text);
            }
        }
        println!("total labels: {}", buffers.labels.len());
    }

    #[test]
    #[ignore]
    fn artis_attraction_probe() {
        let path = std::path::Path::new("../local/maps/noord-holland-detail.mbtiles");
        if !path.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::MbtilesReader::open(path).unwrap();
        for y in 5378..=5392 {
            let Some(raw) = reader.get_tile(14, 8415, 16383 - y).unwrap() else {
                continue;
            };
            let key = TileKey { z: 14, x: 8415, y: y as i32 };
            let mut points = Vec::new();
            let mut ways = Vec::new();
            {
                let pbf_data = decode_vector_tile_payload(&raw).unwrap();
                let mut collector = MvtLocalCollector::new(4.0);
                parse_mvt_tile(&pbf_data, key, &mut collector).unwrap();
                let mut max_id = 0i64;
                for way in &collector.ways {
                    if let Some(id) = way
                        .tags
                        .get("__makepad_osm_id")
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        if way.tags.get("__makepad_osm_type").map(|v| v.as_str()) == Some("way") {
                            max_id = max_id.max(id);
                        }
                        if id == 1391036659 {
                            println!("tile y={} FOUND flamingo way!", y);
                        }
                    }
                }
                println!("tile y={} max way id {}", y, max_id);
            }
            merge_detail_features(
                &raw,
                key,
                4.0,
                true,
                false,
                true,
                &mut points,
                &mut ways,
                &mut Vec::new(),
            )
            .unwrap();
            let mut admitted = 0;
            let mut labeled = 0;
            for way in &ways {
                if way.tags.get("layer").map(|v| v.as_str()) == Some("attraction_area") {
                    admitted += 1;
                    let ring = normalize_polygon_ring(&way.points);
                    let label = ring.as_ref().and_then(|ring| {
                        crate::map::label::extract_area_label(&way.tags, ring_centroid(ring))
                    });
                    if label.is_some() {
                        labeled += 1;
                    }
                    println!(
                        "tile y={} ADMIT {:?} attraction={:?} ring={:?}",
                        y,
                        way.tags.get("name"),
                        way.tags.get("attraction"),
                        ring.as_ref().map(|r| r.len())
                    );
                }
            }
            println!(
                "tile y={} attraction_area ways: {} labeled: {}",
                y, admitted, labeled
            );
        }
    }

    // Diagnostic: print line features near the Reguliersgracht x
    // Keizersgracht bridge to identify the "black dashed fragments".
    // Run: cargo test -p makepad-widgets --features maps bridge_probe -- --nocapture --ignored
    #[test]
    #[ignore]
    fn bridge_probe() {
        let path = std::path::Path::new("../local/maps/europe-shortbread.mbtiles");
        if !path.exists() {
            return;
        }
        let mut reader = makepad_mbtile_reader::MbtilesReader::open(path).unwrap();
        let raw = reader.get_tile(14, 8414, 16383 - 5386).unwrap().unwrap();
        let data = decode_vector_tile_payload(&raw).unwrap();
        let mut collector = MvtLocalCollector::new(1.0);
        parse_mvt_tile(&data, TileKey { z: 14, x: 8414, y: 5386 }, &mut collector).unwrap();
        let target = (3219.0f32 / 16.0, 3973.0f32 / 16.0);
        for way in &collector.ways {
            let near = way.points.iter().any(|p| {
                let dx = p.0 - target.0;
                let dy = p.1 - target.1;
                dx * dx + dy * dy < (12.0f32) * 12.0
            });
            if near && !way.closed {
                let mut tags: Vec<_> = way.tags.iter().collect();
                tags.sort();
                println!("LINE pts={} {:?}", way.points.len(), tags);
            }
        }
    }
}
