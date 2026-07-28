use super::geometry::*;
use super::icons::*;
use super::label::*;
use super::style::*;
use crate::makepad_draw::vector::{
    append_tessellated_geometry, tessellate_path_fill, LineCap, LineJoin, Tessellator, VVertex,
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
// One tile per worker job: a batch runs sequentially inside one closure, so
// 10-tile batches took 2-3s to restyle after a zoom while single-tile jobs
// spread across the thread pool.
pub const MAX_LOCAL_TILE_BATCH: usize = 1;
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

#[derive(Debug)]
pub struct TileBuffers {
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
        });
    }

    Ok(build_tile_buffers_from_features(
        tile_key,
        tile_ways,
        tagged_points,
        theme,
        render_zoom,
    ))
}

/// Local mbtiles path: decode the MVT protobuf STRAIGHT into tile-local
/// coordinates — no lon/lat round trip, no generated-JSON detour.
/// Render buckets from which 2.5D buildings are baked.
pub const BUILDING_3D_MIN_ZOOM: u32 = 16;

pub fn build_tile_buffers_from_mvt(
    tile_key: TileKey,
    raw_tile_data: &[u8],
    detail_tile_data: Option<&[u8]>,
    overlay_tiles: &[OverlayTileData],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
) -> Result<TileBuffers, String> {
    let pbf_data = decode_vector_tile_payload(raw_tile_data)?;
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;
    let mut collector = MvtLocalCollector::new(render_scale);
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    // Compose micro-POIs (trees, benches, bins…) and, in 2.5D mode, building
    // footprints with real heights from the all-tag detail archive over the
    // shortbread base — skip the extra decode below the zooms that use them.
    let want_buildings = buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM;
    if render_zoom >= ICON_MIN_ZOOM.min(BUILDING_3D_MIN_ZOOM) || want_buildings {
        if let Some(detail_data) = detail_tile_data {
            if let Err(err) = merge_detail_features(
                detail_data,
                tile_key,
                render_scale,
                render_zoom >= ICON_MIN_ZOOM,
                want_buildings,
                &mut collector.points,
                &mut collector.ways,
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
    ))
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
    points: &mut Vec<((f32, f32), HashMap<String, String>)>,
    ways: &mut Vec<TileWay>,
) -> Result<(), String> {
    let pbf_data = decode_vector_tile_payload(detail_data)?;
    let mut collector = MvtLocalCollector::new(render_scale);
    parse_mvt_tile(&pbf_data, tile_key, &mut collector)?;
    let render_zoom = tile_key.z as f32 + render_scale.max(1e-6).log2();
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
            // Named zoo enclosures / attractions label at their centroid
            // (and fill if they carry a surface like sand).
            let is_attraction = way.tags.contains_key("name")
                && (way.tags.contains_key("attraction")
                    || way.tags.contains_key("zoo")
                    || way.tags.get("tourism").map(|v| v.as_str()) == Some("attraction"));
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
            let is_building = way
                .tags
                .get("building")
                .is_some_and(|value| value != "no");
            if !is_building {
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

/// One flat-shaded wall quad: two ground vertices and two roof vertices
/// whose height rides in param4 for the tilt shader to lift.
fn append_wall_quad(
    a: (f32, f32),
    b: (f32, f32),
    height_m: f32,
    color: [f32; 4],
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    zbias: &mut f32,
) {
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (p, h) in [(a, 0.0), (b, 0.0), (b, height_m), (a, height_m)] {
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
}

fn build_tile_buffers_from_features(
    tile_key: TileKey,
    tile_ways: Vec<TileWay>,
    tagged_points: Vec<((f32, f32), HashMap<String, String>)>,
    theme: &CompiledMapTheme,
    render_zoom: u32,
) -> TileBuffers {
    // How much this tile gets magnified on screen at the styled view zoom.
    let render_scale = 2.0_f64
        .powi(render_zoom as i32 - tile_key.z as i32)
        .max(1e-3) as f32;
    // Converts "screen px at render_zoom" into tile-local units.
    let zoom_mult = zoom_width_mult(render_zoom);
    let px_to_units = 1.0 / render_scale;
    let aa_units = 1.0 / render_scale;
    let tolerance = DEFAULT_FLATTEN_TOLERANCE / render_scale;

    let mut labels = Vec::<TileLabel>::new();
    let mut icon_jobs = Vec::<((f32, f32), &'static IconMesh, u8, u8, f32, bool)>::new();
    for (point, tags) in &tagged_points {
        let mut label_point = *point;
        let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
        // Overlay points (chargers, transit stops) show earlier than the
        // dense base-POI iconography.
        let icon_zoom_floor = match layer {
            "chargers" => 12,
            "stops" => 13,
            _ => ICON_MIN_ZOOM,
        };
        if render_zoom >= icon_zoom_floor {
            if let Some((icon_name, color_class)) = icon_for_tags(tags) {
                if let Some(mesh) = icon_mesh(icon_name) {
                    // Doors and generic dots yield to real symbols in the
                    // collision pass (a recycling point must not lose to
                    // the building entrance next to it).
                    let priority = match icon_name {
                        "entrance" => 2,
                        "dot" => 1,
                        _ => 0,
                    };
                    // Micro street furniture packs tighter than shop/POI
                    // symbols — a bench must not knock out the tree row.
                    let dist_factor = match icon_name {
                        "tree" | "bench" | "waste_basket" | "recycling" | "dot"
                        | "bicycle" => 0.45f32,
                        _ => 1.0,
                    };
                    let is_tree = icon_name == "tree";
                    icon_jobs.push((*point, mesh, color_class, priority, dist_factor, is_tree));
                    // text sits below the symbol, carto-style
                    label_point.1 += 11.0 / render_scale;
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
    icon_jobs.retain(|(point, _, _, _, dist_factor, _)| {
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
    }
    let mut building_groups = Vec::<BuildingGroup>::new();
    let mut building_group_lookup = HashMap::<String, usize>::new();

    // Fill pass
    let mut fill_groups = Vec::<FillFeatureGroup>::new();
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
        let fill_color = fill_color_for_tags(theme, &way.tags, way.closed);
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
            fill_groups.push(FillFeatureGroup {
                color,
                layer_rank: fill_layer_rank(&way.tags),
                is_building: way.tags.contains_key("building"),
                alpha,
                pattern,
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
            append_tessellated_geometry(
                &tess_verts,
                &tess_indices,
                &mut fill_vertices,
                &mut fill_indices,
                VectorRenderParams {
                    color: hex_to_premul_rgba(group.color, group.alpha),
                    stroke_mult: 1e6,
                    shape_id: group.pattern,
                    params: [
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                        group.layer_rank as f32 * DEPTH_MICRO_PER_RANK,
                    ],
                    zbias: fill_zbias,
                },
            );
            fill_zbias += VECTOR_ZBIAS_STEP;
            feature_count += 1;

            if let (true, Some(outline)) = (group.is_building, building_outline) {
                // Outline the ring but drop segments that run along the tile
                // cut, so clipped buildings don't get a fake wall at the seam.
                let outline_bounds =
                    tile_clip_bounds((1.0 / render_scale).min(FILL_CLIP_OVERLAP) * 0.2);
                let outline_style = StrokePassStyle {
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
            min_y: f32,
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
                building_jobs.push(BuildingJob {
                    polygon,
                    height_m: group.height_m,
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
        let roof_color = hex_to_premul_rgba(base_color, 1.0);
        // Light from the north-west; walls shade by their outward normal.
        let (light_x, light_y) = (-0.55_f32, -0.835_f32);
        for job in &building_jobs {
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
                        job.height_m,
                        wall_color,
                        &mut fill_vertices,
                        &mut fill_indices,
                        &mut fill_zbias,
                    );
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

    // Stroke pass
    let mut stroke_jobs = Vec::<StrokeDrawJob>::new();
    let mut arrow_jobs = Vec::<(Vec<(f32, f32)>, bool)>::new();
    for prepared_way in &prepared {
        let way = &tile_ways[prepared_way.way_index];
        if let Some(label) = extract_way_label(&way.tags, &prepared_way.points) {
            labels.push(label);
        }
        if let Some(style) =
            stroke_style_for_tags(theme, &way.tags, tile_key.z, render_zoom, zoom_mult, px_to_units)
        {
            let implicit_oneway = matches!(
                way.tags.get("junction").map(|v| v.as_str()),
                Some("roundabout") | Some("circular")
            );
            if render_zoom >= ICON_MIN_ZOOM
                && (tag_is_truthy(&way.tags, "oneway") || implicit_oneway)
                && way.tags.contains_key("highway")
                && !tag_is_truthy(&way.tags, "rail")
            {
                arrow_jobs.push((
                    prepared_way.points.clone(),
                    tag_is_truthy(&way.tags, "oneway_reverse"),
                ));
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
                });
            }
            stroke_jobs.push(StrokeDrawJob {
                sort_rank: style.sort_rank,
                style,
                points: prepared_way.points.clone(),
            });
        }
    }

    let mut grouped_strokes = HashMap::<StrokeStyleKey, (StrokeStyle, Vec<Vec<(f32, f32)>>)>::new();
    for job in stroke_jobs {
        let key = StrokeStyleKey::from(job.style);
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

    // Pass 1: all casings into their own buffer so the view can draw every
    // tile's casings before any tile's centers (carto roads-casing layer).
    for (style, parts) in &merged_stroke_parts {
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
                false,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                &mut casing_vertices,
                &mut casing_indices,
                casing,
                LineCap::Butt,
                LineCap::Butt,
                LineJoin::Round,
                aa_units,
                tolerance,
                &mut casing_zbias,
            );
            feature_count += 1;
        }
    }

    // Pass 2: centers. Round caps blend same-color segments at junctions and
    // give dead ends the carto nub — but ends produced by the tile clip must
    // stay butt, or the cap disc overpaints the neighbor tile's content
    // (e.g. white road caps stamped over the tram tracks at seams).
    let cap_eps = 0.05_f32;
    for (style, parts) in &merged_stroke_parts {
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
            );
            feature_count += 1;
        }
    }

    // Pass 3: POI symbols — zoom-constant vector icons, drawn above strokes.
    for (anchor, mesh, color_class, _, _, is_tree) in &icon_jobs {
        append_icon_mesh(
            mesh,
            *anchor,
            hex_to_premul_rgba(poi_class_hex(*color_class), 1.0),
            &mut icon_vertices,
            &mut icon_indices,
            &mut icon_zbias,
        );
        // carto trees: light canopy disc with a dark center dot.
        if *is_tree {
            if let Some(core) = icon_mesh("tree_core") {
                append_icon_mesh(
                    core,
                    *anchor,
                    hex_to_premul_rgba(0x4c7a4c, 1.0),
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
            }
        }
        feature_count += 1;
    }

    // Oneway arrows: zoom-constant glyphs spaced along the way, offsets
    // pre-rotated into the travel direction (carto-style).
    let arrow_color = hex_to_premul_rgba(0x8a8a8a, 1.0);
    let arrow_interval = 170.0 / render_scale;
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
                append_oneway_arrow(
                    anchor,
                    dir_x,
                    dir_y,
                    arrow_color,
                    &mut icon_vertices,
                    &mut icon_indices,
                    &mut icon_zbias,
                );
                distance += arrow_interval;
            }
        }
    }

    compact_tile_labels(&mut labels);

    TileBuffers {
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
    let base = (out_vertices.len() / VECTOR_FLOATS_PER_VERTEX) as u32;
    for (x, y) in SHAPE {
        let ox = x * dir_x - y * dir_y;
        let oy = x * dir_y + y * dir_x;
        // param3 = 1.0: the offset is map-aligned (road direction) and must
        // rotate with the camera, unlike upright billboard POI symbols.
        out_vertices.extend_from_slice(&[
            anchor.0, anchor.1, 0.5, 1.0, color[0], color[1], color[2], color[3], 1e6, 0.0,
            ICON_SHAPE_ID, 0.0, ox, oy, 1.0, 0.0, 0.04, 16.0, *zbias,
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
            vertex.x, // param1/2: screen-px offset from the anchor
            vertex.y,
            0.0,
            0.0,
            0.04, // tilt micro-depth: symbols above every ground stroke
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
    overlay_paths: &[String],
    requested: &[TileKey],
    theme: &CompiledMapTheme,
    render_zoom: u32,
    buildings_3d: bool,
) -> Result<Vec<LoadedLocalTile>, String> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    let mut overlay_readers: Vec<(MbtilesReader, u32, u32)> = overlay_paths
        .iter()
        .filter(|path| !path.is_empty())
        .filter_map(|path| MbtilesReader::open(Path::new(path)).ok())
        .map(|mut reader| {
            let (min_zoom, max_zoom) = overlay_zoom_range(&mut reader);
            (reader, min_zoom, max_zoom)
        })
        .collect();

    let mut fetch_overlays = |tile_key: TileKey| -> Vec<OverlayTileData> {
        let mut out = Vec::new();
        for (reader, min_zoom, max_zoom) in overlay_readers.iter_mut() {
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
                });
            }
        }
        out
    };

    // The MBTiles archive is already the local, seekable tile cache. Do not
    // duplicate it into millions of generated JSON files.
    let mut loaded = Vec::<LoadedLocalTile>::new();
    let missing = requested;

    let mut reader = MbtilesReader::open(mbtiles_path)
        .map_err(|err| format!("open {}: {}", mbtiles_path.display(), err))?;

    // Optional all-tag detail overlay (micro-POIs + 2.5D buildings); only
    // consulted at the zooms that use it.
    let want_detail = render_zoom >= ICON_MIN_ZOOM
        || (buildings_3d && render_zoom >= BUILDING_3D_MIN_ZOOM);
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

                match build_tile_buffers_from_mvt(
                    tile_key,
                    &raw,
                    detail_raw.as_deref(),
                    &overlay_tiles,
                    theme,
                    render_zoom,
                    buildings_3d,
                ) {
                    Ok(buffers) => {
                        loaded.push(LoadedLocalTile { tile_key, buffers });
                    }
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
            match build_tile_buffers_from_mvt(
                tile_key,
                &tile.tile_data,
                detail_raw.as_deref(),
                &overlay_tiles,
                theme,
                render_zoom,
                buildings_3d,
            ) {
                Ok(buffers) => {
                    loaded.push(LoadedLocalTile { tile_key, buffers });
                }
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

/// Receives decoded MVT features (tile-local integer geometry + tags).
trait MvtSink {
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
}

impl MvtLocalCollector {
    fn new(render_scale: f32) -> Self {
        let min_dist = 0.35 / render_scale.max(0.001);
        Self {
            min_dist_sq: min_dist * min_dist,
            next_feature_id: 1,
            ways: Vec::new(),
            points: Vec::new(),
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
        tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }
        let scale = TILE_SIZE as f32 / extent.max(1) as f32;
        let mut out = Vec::<(f32, f32)>::with_capacity(points.len() + 1);
        let mut last: Option<(f32, f32)> = None;
        for &(x, y) in points {
            let point = (x as f32 * scale, y as f32 * scale);
            if let Some(prev) = last {
                let dx = point.0 - prev.0;
                let dy = point.1 - prev.1;
                if dx * dx + dy * dy < self.min_dist_sq {
                    continue;
                }
            }
            out.push(point);
            last = Some(point);
        }
        if out.len() < 2 {
            return;
        }
        if close {
            if out.first() != out.last() {
                out.push(out[0]);
            }
            if out.len() < 4 {
                return;
            }
        }
        self.ways.push(TileWay {
            points: out,
            tags,
            closed: close,
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

fn decode_vector_tile_payload(raw: &[u8]) -> Result<Vec<u8>, String> {
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

fn parse_mvt_tile(
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

#[cfg(test)]
mod bridge_probe_tests {
    use super::*;

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
        }];
        let theme = CompiledMapTheme::default();
        let buffers =
            build_tile_buffers_from_mvt(key, &raw, None, &overlay_tiles, &theme, 12, false)
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
            build_tile_buffers_from_mvt(key, &raw, det.as_deref(), &[], &theme, 17, false)
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
            merge_detail_features(&raw, key, 4.0, true, false, &mut points, &mut ways).unwrap();
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
