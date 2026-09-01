//! Base-layer schema derivation for `pbf-base`.
//!
//! Maps all-tag detail features (the z14 spool records) onto the
//! renderer-compatible shortbread-style base layers, and provides the v1
//! generalization primitives: per-class minimum zooms, exact tile clipping
//! for fragment dedupe, coordinate downsampling, Douglas-Peucker
//! simplification, endpoint stitching and polygon area thresholds.
//!
//! The schema below is derived from what `widgets/src/map/tile.rs` and
//! `widgets/src/map/style.rs` actually parse (see the layer-name match in
//! `normalize_mvt_tags` and the attribute reads in `stroke_style_for_tags`
//! / `fill_color_for_tags` / `extract_point_label`), not from shortbread
//! documentation:
//!
//! | layer            | geometry | attributes emitted                          |
//! |------------------|----------|---------------------------------------------|
//! | streets          | line     | kind, link, bridge, tunnel, layer, oneway,   |
//! |                  |          | oneway_reverse, junction, access, service,   |
//! |                  |          | rail, name (z12+), ref (z8+)                 |
//! | water_polygons   | polygon  | (none)                                       |
//! | water_lines      | line     | kind                                         |
//! | land             | polygon  | kind, plus explicit leisure/natural key      |
//! | buildings        | polygon  | height, building:levels, min_height,         |
//! |                  |          | building:min_level (when present)            |
//! | street_polygons  | polygon  | kind                                         |
//! | place_labels     | point    | kind, name, population (digits only)         |
//! | boundaries       | line     | admin_level, maritime                        |
//! | pois             | point    | name, shop, amenity, tourism, historic,      |
//! |                  |          | office, leisure, craft                       |

use super::geom::{
    bounds, clip_line, clip_ring, remove_consecutive_duplicates, signed_area, tile_range,
    tile_rect, to_local, GlobalPoint, Rect, MVT_EXTENT,
};
use super::mvt::{GeometryType, Layer, OsmType, TileFeature, TilePoint};
use smallvec::SmallVec;

/// 0.5 display pixel at a 256px tile with 4096-unit extent.
pub const SIMPLIFY_EPSILON_UNITS: f64 = 8.0;
/// Detail zoom the spool was built at.
pub const DETAIL_ZOOM: u8 = 14;

/// DP epsilon per zoom: 1.5px below z12 (mid-zoom tiles are size-critical
/// for tessellation), 0.5px at z12-13, exact at the detail zoom.
pub fn simplify_epsilon(zoom: u8) -> f64 {
    if zoom < 12 {
        24.0
    } else if zoom < DETAIL_ZOOM {
        SIMPLIFY_EPSILON_UNITS
    } else {
        0.0
    }
}

/// One renderer-facing base feature derived from a detail feature, still in
/// the source z14 tile's local coordinates (buffered, as spooled).
pub struct BaseSpec {
    pub layer: Layer,
    pub geometry_type: GeometryType,
    /// Feature id with the relation bit mixed in so way/relation ids from
    /// the same layer can never collide (`osm_id * 2 + is_relation`).
    pub id: i64,
    pub min_zoom: u8,
    pub closed: bool,
    pub tags: Vec<(String, String)>,
}

fn tag<'a>(tags: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tags.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn truthy(value: &str) -> bool {
    !matches!(value, "" | "0" | "no" | "false" | "False")
}

fn push(out: &mut Vec<(String, String)>, key: &str, value: &str) {
    out.push((key.to_string(), value.to_string()));
}

/// Map one detail feature onto zero or more base specs. Geometry is shared
/// with the source feature (the caller pairs specs with the source paths).
pub fn base_specs(feature: &TileFeature) -> SmallVec<[BaseSpec; 2]> {
    let mut out = SmallVec::new();
    let rel_bit = u64::from(feature.osm_type == OsmType::Relation);
    let id = ((feature.id as u64) << 1 | rel_bit) as i64;
    let tags = &feature.tags;

    match (feature.layer, feature.geometry_type) {
        // Way linework: roads, rail, waterways.
        (Layer::OsmLines, GeometryType::LineString) => {
            if let Some(spec) = street_spec(tags, feature.closed, id) {
                out.push(spec);
            } else if let Some(spec) = waterway_spec(tags, id) {
                out.push(spec);
            }
        }
        // Administrative boundaries come from boundary relations only; the
        // member ways are also tagged in OSM but emitting both would
        // duplicate every border line.
        (Layer::OsmRelationLines, GeometryType::LineString) => {
            if let Some(spec) = boundary_spec(tags, id) {
                out.push(spec);
            }
        }
        // Area features from closed ways and multipolygon relations.
        (Layer::OsmPolygons | Layer::OsmRelationPolygons, GeometryType::Polygon) => {
            if let Some(spec) = water_polygon_spec(tags, id) {
                out.push(spec);
            } else if let Some(spec) = land_spec(tags, id) {
                out.push(spec);
            }
            if let Some(spec) = building_spec(tags, id) {
                out.push(spec);
            } else if let Some(spec) = street_polygon_spec(tags, id) {
                out.push(spec);
            }
        }
        // Tagged nodes: place labels and named POIs.
        (Layer::OsmPoints, GeometryType::Point) => {
            if let Some(spec) = place_spec(tags, id) {
                out.push(spec);
            } else if let Some(spec) = poi_spec(tags, id) {
                out.push(spec);
            }
        }
        _ => {}
    }
    out
}

fn street_spec(tags: &[(String, String)], closed: bool, id: i64) -> Option<BaseSpec> {
    let mut kind;
    let mut is_rail = false;
    let mut min_zoom;
    if let Some(highway) = tag(tags, "highway") {
        kind = highway;
        min_zoom = match highway {
            "motorway" | "motorway_link" | "trunk" | "trunk_link" => 5,
            "primary" | "primary_link" => 7,
            "secondary" | "secondary_link" => 9,
            "tertiary" | "tertiary_link" => 10,
            "residential" | "unclassified" | "living_street" | "busway" | "pedestrian" => 12,
            "construction" | "proposed" | "razed" | "abandoned" | "planned" | "no"
            | "corridor" | "elevator" => return None,
            // service, track, cycleway, footway, path, steps, bridleway, ...
            _ => DETAIL_ZOOM,
        };
    } else if let Some(railway) = tag(tags, "railway") {
        match railway {
            "rail" | "tram" | "light_rail" | "subway" | "narrow_gauge" | "funicular"
            | "monorail" => {}
            _ => return None,
        }
        kind = railway;
        is_rail = true;
        let sidings = tag(tags, "service").is_some();
        min_zoom = if railway == "rail" && !sidings { 8 } else { 12 };
    } else {
        return None;
    }
    if kind == "road" {
        kind = "unclassified";
    }
    if min_zoom < 12 && tag(tags, "area").is_some_and(truthy) {
        // Area-mapped carriageways stay high-zoom only.
        min_zoom = 12;
    }

    let mut out = Vec::with_capacity(6);
    push(&mut out, "kind", kind);
    if is_rail {
        push(&mut out, "rail", "true");
    }
    if kind.ends_with("_link") {
        push(&mut out, "link", "true");
    }
    if let Some(v) = tag(tags, "bridge").filter(|v| truthy(v)) {
        push(&mut out, "bridge", v);
    }
    if let Some(v) = tag(tags, "tunnel").filter(|v| truthy(v)) {
        push(&mut out, "tunnel", v);
    }
    if let Some(v) = tag(tags, "layer") {
        if v.parse::<i32>().map(|n| n != 0).unwrap_or(false) {
            // Renamed to `osm_layer` by the renderer; drives stacking.
            push(&mut out, "layer", v);
        }
    }
    match tag(tags, "oneway") {
        Some("yes") | Some("true") | Some("1") => push(&mut out, "oneway", "true"),
        Some("-1") => push(&mut out, "oneway_reverse", "true"),
        _ => {}
    }
    if let Some(v) = tag(tags, "junction").filter(|v| matches!(*v, "roundabout" | "circular")) {
        push(&mut out, "junction", v);
    }
    if let Some(v) = tag(tags, "access").filter(|v| matches!(*v, "private" | "no" | "customers")) {
        push(&mut out, "access", v);
    }
    if tag(tags, "service").is_some() {
        push(&mut out, "service", "yes");
    }
    if let Some(v) = tag(tags, "ref") {
        push(&mut out, "ref", v);
    }
    if let Some(v) = tag(tags, "name") {
        push(&mut out, "name", v);
    }
    Some(BaseSpec {
        layer: Layer::BaseStreets,
        geometry_type: GeometryType::LineString,
        id,
        min_zoom,
        closed,
        tags: out,
    })
}

fn waterway_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let waterway = tag(tags, "waterway")?;
    let min_zoom = match waterway {
        "river" => 8,
        // The Dutch canal grid alone is a third of a z8 tile; canals wait a
        // zoom longer than rivers.
        "canal" => 9,
        "stream" | "ditch" | "drain" => 12,
        _ => return None,
    };
    if tag(tags, "tunnel").is_some_and(truthy) {
        // Culverted / underground waterways stay off the base map.
        return None;
    }
    let mut out = Vec::with_capacity(1);
    push(&mut out, "kind", waterway);
    Some(BaseSpec {
        layer: Layer::BaseWaterLines,
        geometry_type: GeometryType::LineString,
        id,
        min_zoom,
        closed: false,
        tags: out,
    })
}

fn water_polygon_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let water = tag(tags, "natural") == Some("water")
        || matches!(tag(tags, "waterway"), Some("riverbank") | Some("dock"))
        || matches!(tag(tags, "landuse"), Some("reservoir") | Some("basin"));
    if !water {
        return None;
    }
    Some(BaseSpec {
        layer: Layer::BaseWaterPolygons,
        geometry_type: GeometryType::Polygon,
        id,
        min_zoom: 0,
        closed: true,
        tags: Vec::new(),
    })
}

fn land_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let mut out = Vec::with_capacity(2);
    let min_zoom;
    if let Some(landuse) = tag(tags, "landuse") {
        match landuse {
            // Forests read at overview zooms (and drape samples them from
            // z6); agricultural texture waits until z9.
            "forest" => min_zoom = 6,
            "farmland" | "meadow" | "grass" | "orchard" | "vineyard" => min_zoom = 9,
            "allotments" | "village_green" | "recreation_ground" | "cemetery"
            | "residential" | "industrial" | "commercial" | "retail" | "railway"
            | "landfill" | "quarry" | "brownfield" | "greenfield" | "garages" => {
                min_zoom = 10;
            }
            _ => return None,
        }
        push(&mut out, "kind", landuse);
    } else if let Some(leisure) = tag(tags, "leisure") {
        match leisure {
            "park" | "garden" | "playground" | "golf_course" | "pitch" | "sports_centre"
            | "nature_reserve" | "stadium" | "common" | "dog_park" => {}
            _ => return None,
        }
        min_zoom = 10;
        push(&mut out, "kind", leisure);
        // The renderer collapses leisure-ish kinds to leisure="park" during
        // normalization; an explicit leisure attribute preserves the
        // specific value for fill rank / pattern decisions.
        push(&mut out, "leisure", leisure);
    } else if let Some(natural) = tag(tags, "natural") {
        let kind = match natural {
            "wood" => "forest",
            "grassland" => "grass",
            "scrub" | "heath" | "shrubbery" | "sand" | "beach" | "shingle" | "glacier"
            | "wetland" | "bare_rock" => natural,
            _ => return None,
        };
        min_zoom = match natural {
            "wood" | "glacier" => 6,
            "grassland" | "heath" | "scrub" | "wetland" | "bare_rock" => 9,
            _ => 10,
        };
        push(&mut out, "kind", kind);
        push(&mut out, "natural", natural);
    } else {
        return None;
    }
    Some(BaseSpec {
        layer: Layer::BaseLand,
        geometry_type: GeometryType::Polygon,
        id,
        min_zoom,
        closed: true,
        tags: out,
    })
}

fn building_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let building = tag(tags, "building")?;
    if building == "no" {
        return None;
    }
    if tag(tags, "location") == Some("underground")
        || tag(tags, "layer").is_some_and(|v| v.starts_with('-'))
    {
        return None;
    }
    let mut out = Vec::new();
    for key in ["height", "building:levels", "min_height", "building:min_level"] {
        if let Some(v) = tag(tags, key) {
            push(&mut out, key, v);
        }
    }
    Some(BaseSpec {
        layer: Layer::BaseBuildings,
        geometry_type: GeometryType::Polygon,
        id,
        // z14 only: the versatiles baseline has no z13 buildings either,
        // and they dominated z13 tile size (80%+ of the biggest tiles).
        min_zoom: DETAIL_ZOOM,
        closed: true,
        tags: out,
    })
}

fn street_polygon_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let highway = tag(tags, "highway")?;
    if !matches!(highway, "pedestrian" | "footway") {
        return None;
    }
    if !(tag(tags, "area").is_some_and(truthy) || tag(tags, "place") == Some("square")) {
        return None;
    }
    let mut out = Vec::with_capacity(1);
    push(&mut out, "kind", highway);
    Some(BaseSpec {
        layer: Layer::BaseStreetPolygons,
        geometry_type: GeometryType::Polygon,
        id,
        min_zoom: DETAIL_ZOOM,
        closed: true,
        tags: out,
    })
}

fn place_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let place = tag(tags, "place")?;
    let name = tag(tags, "name")?;
    if name.is_empty() {
        return None;
    }
    let min_zoom = match place {
        "city" => 4,
        "town" => 8,
        "village" => 10,
        "suburb" | "borough" | "quarter" | "neighbourhood" => 12,
        "hamlet" => 13,
        _ => return None,
    };
    let mut out = Vec::with_capacity(3);
    push(&mut out, "kind", place);
    push(&mut out, "name", name);
    if let Some(population) = tag(tags, "population") {
        // The renderer parses population with u64::parse; only forward
        // clean digit strings so a malformed value cannot demote the label.
        if !population.is_empty() && population.bytes().all(|b| b.is_ascii_digit()) {
            push(&mut out, "population", population);
        }
    }
    Some(BaseSpec {
        layer: Layer::BasePlaceLabels,
        geometry_type: GeometryType::Point,
        id,
        min_zoom,
        closed: false,
        tags: out,
    })
}

const POI_KEYS: [&str; 7] = [
    "shop", "amenity", "tourism", "historic", "office", "leisure", "craft",
];

fn poi_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    let name = tag(tags, "name")?;
    if name.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(2);
    for key in POI_KEYS {
        if let Some(v) = tag(tags, key) {
            push(&mut out, key, v);
        }
    }
    if out.is_empty() {
        return None;
    }
    push(&mut out, "name", name);
    Some(BaseSpec {
        layer: Layer::BasePois,
        geometry_type: GeometryType::Point,
        id,
        min_zoom: DETAIL_ZOOM,
        closed: false,
        tags: out,
    })
}

fn boundary_spec(tags: &[(String, String)], id: i64) -> Option<BaseSpec> {
    if tag(tags, "boundary") != Some("administrative") {
        return None;
    }
    let admin_level = tag(tags, "admin_level")?.parse::<u8>().ok()?;
    let min_zoom = match admin_level {
        0..=4 => 4,
        5..=8 => 10,
        _ => return None,
    };
    let mut out = Vec::with_capacity(2);
    push(&mut out, "admin_level", &admin_level.to_string());
    if tag(tags, "maritime").is_some_and(truthy) {
        push(&mut out, "maritime", "true");
    }
    Some(BaseSpec {
        layer: Layer::BaseBoundaries,
        geometry_type: GeometryType::LineString,
        id,
        min_zoom,
        closed: false,
        tags: out,
    })
}

/// Per-zoom attribute thinning: identical within a zoom (so spool fragments
/// of one feature still merge), and aggressive at low zooms — fewer
/// distinct tag-sets means far better same-tags feature merging.
pub fn tags_for_zoom(layer: Layer, tags: &[(String, String)], zoom: u8) -> Vec<(String, String)> {
    if layer != Layer::BaseStreets {
        return tags.to_vec();
    }
    tags.iter()
        .filter(|(k, _)| match k.as_str() {
            "name" => zoom >= 12,
            "ref" => zoom >= 8,
            "bridge" | "tunnel" | "layer" => zoom >= 10,
            "oneway" | "oneway_reverse" | "junction" | "access" | "service" => zoom >= 12,
            // kind / rail / link always
            _ => true,
        })
        .cloned()
        .collect()
}

/// Minimum kept polygon area per layer and zoom, in doubled square units
/// (`signed_area` returns 2A; 2048 = 4 display px²). Below the detail zoom
/// the floor scales up as zoom drops: a sub-4px² polygon has no business in
/// a mid-zoom tile, and at overview zooms only large areas matter.
pub fn min_ring_area2(layer: Layer, zoom: u8) -> i64 {
    let base = match layer {
        Layer::BaseBuildings => 128,
        Layer::BaseStreetPolygons => 128,
        _ => 2048,
    };
    let multiplier = match zoom {
        0..=8 => 8,
        9..=10 => 4,
        11 => 2,
        _ => 1,
    };
    base * multiplier
}

// ---------------------------------------------------------------------------
// Geometry: exact clip, downsampling, DP simplification, stitching
// ---------------------------------------------------------------------------

/// The source feature's paths lifted to z14 global units with the closing
/// point restored on closed lines, then clipped to the *unbuffered* z14
/// tile box so overlapping tile buffers dedupe exactly and neighbouring
/// fragments meet at identical boundary points.
pub struct GlobalPaths {
    pub paths: Vec<Vec<GlobalPoint>>,
}

pub fn exact_clip_to_tile(
    tile_x: u32,
    tile_y: u32,
    geometry_type: GeometryType,
    closed: bool,
    paths: &[Vec<TilePoint>],
) -> GlobalPaths {
    let origin_x = i64::from(tile_x) * MVT_EXTENT;
    let origin_y = i64::from(tile_y) * MVT_EXTENT;
    let rect = Rect {
        min_x: origin_x as f64,
        min_y: origin_y as f64,
        max_x: (origin_x + MVT_EXTENT) as f64,
        max_y: (origin_y + MVT_EXTENT) as f64,
    };
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let mut global: Vec<GlobalPoint> = path
            .iter()
            .map(|p| GlobalPoint {
                x: origin_x + i64::from(p.x),
                y: origin_y + i64::from(p.y),
            })
            .collect();
        match geometry_type {
            GeometryType::Point => {
                out.push(global);
            }
            GeometryType::LineString => {
                if closed && global.len() >= 3 && global.first() != global.last() {
                    global.push(global[0]);
                }
                for piece in clip_line(&global, rect) {
                    if piece.len() >= 2 {
                        out.push(piece);
                    }
                }
            }
            GeometryType::Polygon => {
                // Rings keep their stored orientation (outer positive,
                // holes negative); Sutherland-Hodgman preserves order.
                let mut clipped = clip_ring(&global, rect);
                remove_consecutive_duplicates(&mut clipped);
                if clipped.len() >= 3 && signed_area(&clipped) != 0 {
                    out.push(clipped);
                }
            }
        }
    }
    GlobalPaths { paths: out }
}

/// Scale z14-global coordinates down to `zoom`-global coordinates.
pub fn downsample_paths(
    global: &GlobalPaths,
    geometry_type: GeometryType,
    zoom: u8,
) -> Vec<Vec<GlobalPoint>> {
    let shift = DETAIL_ZOOM - zoom;
    let divisor = (1_i64 << shift) as f64;
    let mut out = Vec::with_capacity(global.paths.len());
    for path in &global.paths {
        let mut scaled: Vec<GlobalPoint> = path
            .iter()
            .map(|p| GlobalPoint {
                x: (p.x as f64 / divisor).round() as i64,
                y: (p.y as f64 / divisor).round() as i64,
            })
            .collect();
        scaled.dedup();
        match geometry_type {
            GeometryType::Point => out.push(scaled),
            GeometryType::LineString => {
                if scaled.len() >= 2 {
                    out.push(scaled);
                }
            }
            GeometryType::Polygon => {
                if scaled.first() == scaled.last() && scaled.len() > 1 {
                    scaled.pop();
                }
                if scaled.len() >= 3 && signed_area(&scaled) != 0 {
                    out.push(scaled);
                }
            }
        }
    }
    out
}

/// A downsampled fragment localized to one target-zoom tile.
pub struct TargetFragment {
    pub tile_x: u32,
    pub tile_y: u32,
    pub paths: Vec<Vec<TilePoint>>,
}

/// Distribute target-zoom global paths onto buffered target tiles.
/// Lines/points reuse the buffered clipping the z14 spool uses; polygon
/// rings are clipped independently, preserving orientation, so multipolygon
/// outer/hole structure survives without regrouping.
pub fn to_target_tiles(
    zoom: u8,
    geometry_type: GeometryType,
    paths: &[Vec<GlobalPoint>],
    buffer: i64,
) -> Result<Vec<TargetFragment>, String> {
    let mut fragments: Vec<TargetFragment> = Vec::new();
    let mut push_path =
        |tile_x: u32, tile_y: u32, local: Vec<TilePoint>| {
            if let Some(fragment) = fragments
                .iter_mut()
                .find(|f| f.tile_x == tile_x && f.tile_y == tile_y)
            {
                fragment.paths.push(local);
            } else {
                fragments.push(TargetFragment {
                    tile_x,
                    tile_y,
                    paths: vec![local],
                });
            }
        };
    for path in paths {
        if path.is_empty() {
            continue;
        }
        let Some((min_x, min_y, max_x, max_y)) = bounds(path) else {
            continue;
        };
        match geometry_type {
            GeometryType::Point => {
                let axis = 1_i64 << zoom;
                for &point in path {
                    let tile_x = point.x.div_euclid(MVT_EXTENT).clamp(0, axis - 1) as u32;
                    let tile_y = point.y.div_euclid(MVT_EXTENT).clamp(0, axis - 1) as u32;
                    let local = to_local(point, tile_x, tile_y)?;
                    push_path(tile_x, tile_y, vec![local]);
                }
            }
            GeometryType::LineString => {
                let range = tile_range(zoom, min_x, min_y, max_x, max_y, buffer)?;
                for tile_y in range.y_min..=range.y_max {
                    for tile_x in range.x_min..=range.x_max {
                        let rect = tile_rect(tile_x, tile_y, buffer);
                        for piece in clip_line(path, rect) {
                            let mut local = Vec::with_capacity(piece.len());
                            for point in piece {
                                local.push(to_local(point, tile_x, tile_y)?);
                            }
                            local.dedup();
                            if local.len() >= 2 {
                                push_path(tile_x, tile_y, local);
                            }
                        }
                    }
                }
            }
            GeometryType::Polygon => {
                let range = tile_range(zoom, min_x, min_y, max_x, max_y, buffer)?;
                for tile_y in range.y_min..=range.y_max {
                    for tile_x in range.x_min..=range.x_max {
                        let rect = tile_rect(tile_x, tile_y, buffer);
                        let mut clipped = clip_ring(path, rect);
                        remove_consecutive_duplicates(&mut clipped);
                        if clipped.len() >= 3 && signed_area(&clipped) != 0 {
                            let mut local = Vec::with_capacity(clipped.len());
                            for point in clipped {
                                local.push(to_local(point, tile_x, tile_y)?);
                            }
                            push_path(tile_x, tile_y, local);
                        }
                    }
                }
            }
        }
    }
    Ok(fragments)
}

/// Join line paths whose endpoints coincide exactly (fragments produced by
/// the exact tile clip meet at identical boundary points). Greedy; paths
/// that cannot be joined remain separate.
pub fn stitch_paths(paths: Vec<Vec<TilePoint>>) -> Vec<Vec<TilePoint>> {
    let mut paths: Vec<Vec<TilePoint>> = paths
        .into_iter()
        .filter(|path| path.len() >= 2)
        .collect();
    if paths.len() < 2 {
        return paths;
    }
    let mut out: Vec<Vec<TilePoint>> = Vec::with_capacity(paths.len());
    while let Some(mut current) = paths.pop() {
        loop {
            // A ring that already closed must not absorb further paths.
            if current.len() > 2 && current.first() == current.last() {
                break;
            }
            let start = *current.first().unwrap();
            let end = *current.last().unwrap();
            let mut joined = false;
            let mut index = 0;
            while index < paths.len() {
                let candidate = &paths[index];
                let c_start = *candidate.first().unwrap();
                let c_end = *candidate.last().unwrap();
                // Closed rings are complete; never splice them into others.
                if candidate.len() > 2 && c_start == c_end {
                    index += 1;
                    continue;
                }
                if c_start == end {
                    let mut candidate = paths.swap_remove(index);
                    current.extend(candidate.drain(1..));
                    joined = true;
                    break;
                }
                if c_end == end {
                    let mut candidate = paths.swap_remove(index);
                    candidate.reverse();
                    current.extend(candidate.drain(1..));
                    joined = true;
                    break;
                }
                if c_end == start {
                    let mut candidate = paths.swap_remove(index);
                    candidate.pop();
                    candidate.extend(current);
                    current = candidate;
                    joined = true;
                    break;
                }
                if c_start == start {
                    let mut candidate = paths.swap_remove(index);
                    candidate.reverse();
                    candidate.pop();
                    candidate.extend(current);
                    current = candidate;
                    joined = true;
                    break;
                }
                index += 1;
            }
            if !joined {
                break;
            }
        }
        out.push(current);
    }
    out
}

/// Iterative Douglas-Peucker on tile-local coordinates. `epsilon` is the
/// maximum perpendicular deviation in tile units.
pub fn simplify_path(points: &[TilePoint], epsilon: f64) -> Vec<TilePoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let eps2 = epsilon * epsilon;
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0_usize, points.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let a = points[first];
        let b = points[last];
        let abx = f64::from(b.x - a.x);
        let aby = f64::from(b.y - a.y);
        let len2 = abx * abx + aby * aby;
        let mut max_dist2 = -1.0;
        let mut max_index = first;
        for (index, p) in points.iter().enumerate().take(last).skip(first + 1) {
            let apx = f64::from(p.x - a.x);
            let apy = f64::from(p.y - a.y);
            let dist2 = if len2 <= f64::EPSILON {
                apx * apx + apy * apy
            } else {
                let cross = apx * aby - apy * abx;
                cross * cross / len2
            };
            if dist2 > max_dist2 {
                max_dist2 = dist2;
                max_index = index;
            }
        }
        if max_dist2 > eps2 {
            keep[max_index] = true;
            stack.push((first, max_index));
            stack.push((max_index, last));
        }
    }
    points
        .iter()
        .zip(&keep)
        .filter(|(_, &k)| k)
        .map(|(p, _)| *p)
        .collect()
}

/// Simplify a ring (stored open) while preserving orientation; returns None
/// when the result degenerates.
///
/// Epsilon is capped by the ring's bounding box: large polygons arrive as
/// per-z14-tile quilt fragments whose cells shrink to a few units at low
/// zooms, and a fixed epsilon would simplify every cell to a line and erase
/// the polygon entirely.
pub fn simplify_ring(ring: &[TilePoint], epsilon: f64) -> Option<Vec<TilePoint>> {
    if ring.len() < 3 {
        return None;
    }
    let (mut min_x, mut max_x, mut min_y, mut max_y) =
        (ring[0].x, ring[0].x, ring[0].y, ring[0].y);
    for p in ring {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    let span = f64::from((max_x - min_x).min(max_y - min_y));
    let epsilon = epsilon.min(span / 3.0);
    if epsilon <= 0.0 {
        return if ring_area2(ring) != 0 {
            Some(ring.to_vec())
        } else {
            None
        };
    }
    let mut closed: Vec<TilePoint> = ring.to_vec();
    closed.push(ring[0]);
    let mut simplified = simplify_path(&closed, epsilon);
    simplified.pop();
    if simplified.len() < 3 || ring_area2(&simplified) == 0 {
        return None;
    }
    Some(simplified)
}

/// Doubled signed shoelace area of a tile-local ring.
pub fn ring_area2(ring: &[TilePoint]) -> i64 {
    if ring.len() < 3 {
        return 0;
    }
    let mut area = 0_i64;
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        area += i64::from(a.x) * i64::from(b.y) - i64::from(b.x) * i64::from(a.y);
    }
    area
}

/// Finalize one merged base feature for encoding at `zoom`: stitch line
/// fragments, simplify below the detail zoom, and drop degenerate or
/// sub-threshold geometry. Returns None when nothing remains.
pub fn finalize_feature(mut feature: TileFeature, zoom: u8) -> Option<TileFeature> {
    let epsilon = simplify_epsilon(zoom);
    match feature.geometry_type {
        GeometryType::Point => Some(feature),
        GeometryType::LineString => {
            let stitched = stitch_paths(std::mem::take(&mut feature.paths));
            let mut paths = Vec::with_capacity(stitched.len());
            for path in stitched {
                let path = if epsilon > 0.0 {
                    simplify_path(&path, epsilon)
                } else {
                    path
                };
                if path.len() >= 2 && path.windows(2).any(|w| w[0] != w[1]) {
                    paths.push(path);
                }
            }
            if paths.is_empty() {
                return None;
            }
            feature.paths = paths;
            Some(feature)
        }
        GeometryType::Polygon => {
            // Dissolved layers filter per RING: after the union pass each
            // ring is an independent blob boundary (or hole), and a giant
            // group total must not carry isolated slivers through.
            let per_ring_floor = zoom < DETAIL_ZOOM
                && matches!(
                    feature.layer,
                    Layer::BaseWaterPolygons | Layer::BaseLand | Layer::BaseStreetPolygons
                );
            let floor = min_ring_area2(feature.layer, zoom);
            let mut paths = Vec::with_capacity(feature.paths.len());
            let mut outer_area2 = 0_i64;
            for ring in std::mem::take(&mut feature.paths) {
                let ring = if epsilon > 0.0 {
                    match simplify_ring(&ring, epsilon) {
                        Some(ring) => ring,
                        None => continue,
                    }
                } else {
                    ring
                };
                let area2 = ring_area2(&ring);
                if area2 == 0 {
                    continue;
                }
                if per_ring_floor && area2.abs() < floor {
                    continue;
                }
                if area2 > 0 {
                    outer_area2 += area2;
                }
                paths.push(ring);
            }
            if paths.is_empty() || outer_area2 == 0 {
                return None;
            }
            if !per_ring_floor && zoom < DETAIL_ZOOM && outer_area2 < floor {
                return None;
            }
            // Holes without a surviving outer ring cannot render.
            if !paths.iter().any(|ring| ring_area2(ring) > 0) {
                return None;
            }
            if per_ring_floor {
                // Dissolved layers: split pinched rings and emit strict
                // nesting order (outer, its holes, next outer, ...) so the
                // ring-cap split can never strand a hole away from its
                // outer in another chunk.
                paths = order_rings_nested(paths);
                if paths.is_empty() {
                    return None;
                }
            } else if paths.first().is_some_and(|ring| ring_area2(ring) < 0) {
                // A hole whose outer ring was simplified away must not lead
                // the ring list; keep outers first.
                paths.sort_by_key(|ring| ring_area2(ring) < 0);
            }
            feature.paths = paths;
            Some(feature)
        }
    }
}

/// Merge finalized features that share (layer, geometry type, tags) into
/// one multi-part feature, then re-stitch and re-simplify lines. This is
/// the difference between "one feature per OSM way" (thousands of 2-unit
/// motorway fragments per overview tile, each with its own id + tag row)
/// and the shortbread-style merged linework a renderer can tessellate
/// cheaply. Lines merge below the detail zoom; polygons (winding rule
/// keeps multi-outer features correct) merge below z13. Points never merge.
pub fn merge_features_by_tags(features: Vec<TileFeature>, zoom: u8) -> Vec<TileFeature> {
    if zoom > DETAIL_ZOOM {
        return features;
    }
    // AT the detail zoom (z14) only the streets layer merges: one feature
    // per OSM way put ~65% more street features per Amsterdam tile than the
    // shortbread reference and inflated the renderer's road-union input by
    // the same factor. simplify_epsilon(14) is 0, so this is stitch-only —
    // same-tag chains collapse (fewer features, identical geometry).
    // Other z14 layers keep way granularity (osm_* detail semantics).
    let detail = zoom >= DETAIL_ZOOM;
    let mut merged: Vec<TileFeature> = Vec::with_capacity(features.len());
    let mut index: std::collections::HashMap<
        (u8, u8, Vec<(String, String)>),
        usize,
    > = std::collections::HashMap::new();
    for feature in features {
        // Lines only: polygon merging would grow per-feature ring counts,
        // and the renderer's per-feature tessellation cost is superlinear
        // in vertices — giant polygon features are the enemy, not feature
        // count (see split_giant_polygons).
        let mergeable = feature.geometry_type == GeometryType::LineString
            && (!detail || feature.layer == Layer::BaseStreets);
        if !mergeable {
            merged.push(feature);
            continue;
        }
        let key = (
            feature.layer as u8,
            feature.geometry_type as u8,
            feature.tags.clone(),
        );
        match index.get(&key) {
            Some(&at) => merged[at].paths.extend(feature.paths),
            None => {
                index.insert(key, merged.len());
                merged.push(feature);
            }
        }
    }
    let epsilon = simplify_epsilon(zoom);
    for feature in &mut merged {
        if feature.geometry_type == GeometryType::LineString && feature.paths.len() > 1 {
            // Consecutive ways of one road share endpoint nodes exactly, so
            // stitching across merged ways collapses chains into long
            // polylines; a second DP pass then removes the collinear joints.
            let stitched = stitch_paths(std::mem::take(&mut feature.paths));
            feature.paths = stitched
                .into_iter()
                .map(|path| {
                    if epsilon > 0.0 {
                        simplify_path(&path, epsilon)
                    } else {
                        path
                    }
                })
                .filter(|path| path.len() >= 2)
                .collect();
        }
    }
    merged.retain(|feature| !feature.paths.is_empty());
    merged
}

/// Dissolve exact-abutting polygon fragments into clean boundary rings by
/// directed-edge cancellation: an edge shared by two abutting fragments
/// appears once in each direction and cancels; surviving edges form the
/// union boundary, reassembled by walking successors (in-degree equals
/// out-degree everywhere, so walks always close). Exact for the z14-quilt
/// fragments this pipeline produces — their shared edges have identical
/// endpoints by construction. MUST run before simplification (DP would
/// destroy the exact edge matching). Non-matching borders are left as-is.
pub fn dissolve_rings(rings: Vec<Vec<TilePoint>>) -> Vec<Vec<TilePoint>> {
    use std::collections::HashMap;
    type EdgeKey = (i32, i32, i32, i32);
    let key = |a: TilePoint, b: TilePoint| -> EdgeKey { (a.x, a.y, b.x, b.y) };
    let mut edge_count: HashMap<EdgeKey, i32> = HashMap::new();
    for ring in &rings {
        if ring.len() < 3 {
            continue;
        }
        for index in 0..ring.len() {
            let a = ring[index];
            let b = ring[(index + 1) % ring.len()];
            if a == b {
                continue;
            }
            *edge_count.entry(key(a, b)).or_insert(0) += 1;
            // Cancel against reverse occurrences eagerly.
            let reverse = key(b, a);
            if let Some(reverse_count) = edge_count.get_mut(&reverse) {
                if *reverse_count > 0 {
                    *reverse_count -= 1;
                    *edge_count.get_mut(&key(a, b)).unwrap() -= 1;
                }
            }
        }
    }
    // Successor map over surviving edges (with multiplicity).
    let mut outgoing: HashMap<(i32, i32), Vec<TilePoint>> = HashMap::new();
    let mut survivors = 0_usize;
    for (&(ax, ay, bx, by), &count) in &edge_count {
        for _ in 0..count.max(0) {
            outgoing
                .entry((ax, ay))
                .or_default()
                .push(TilePoint { x: bx, y: by });
            survivors += 1;
        }
    }
    if survivors == 0 {
        return Vec::new();
    }
    // Planar face walking: at a junction vertex the successor is NOT
    // arbitrary — picking the wrong edge stitches two faces into one
    // figure-8 ring (or mis-partitions an Euler circuit into rings with
    // inverted winding). The standard rule: continue with the edge that
    // makes the sharpest turn on the traced-face side, i.e. minimize the
    // clockwise angle from the reversed incoming direction.
    let pick_successor = |candidates: &mut Vec<TilePoint>,
                          at: TilePoint,
                          incoming_from: TilePoint|
     -> Option<TilePoint> {
        if candidates.is_empty() {
            return None;
        }
        if candidates.len() == 1 {
            return candidates.pop();
        }
        let base = f64::atan2(
            f64::from(incoming_from.y - at.y),
            f64::from(incoming_from.x - at.x),
        );
        let mut best_index = 0;
        let mut best_turn = f64::MAX;
        for (index, candidate) in candidates.iter().enumerate() {
            let angle = f64::atan2(
                f64::from(candidate.y - at.y),
                f64::from(candidate.x - at.x),
            );
            // Clockwise rotation from the reversed incoming edge, in
            // (0, tau]; going straight back scores tau (last resort).
            let mut turn = base - angle;
            while turn <= 1e-12 {
                turn += std::f64::consts::TAU;
            }
            if turn < best_turn {
                best_turn = turn;
                best_index = index;
            }
        }
        Some(candidates.swap_remove(best_index))
    };
    let mut out = Vec::new();
    let mut starts: Vec<(i32, i32)> = outgoing.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        loop {
            let Some(first) = outgoing.get_mut(&start).and_then(Vec::pop) else {
                break;
            };
            let start_point = TilePoint {
                x: start.0,
                y: start.1,
            };
            let mut ring = vec![start_point, first];
            loop {
                let current = *ring.last().unwrap();
                if current == start_point {
                    ring.pop(); // rings are stored open
                    break;
                }
                let previous = ring[ring.len() - 2];
                let next = outgoing
                    .get_mut(&(current.x, current.y))
                    .and_then(|candidates| pick_successor(candidates, current, previous));
                let Some(next) = next else {
                    // Dangling walk (non-matching input edges): drop it.
                    ring.clear();
                    break;
                };
                ring.push(next);
            }
            if ring.len() >= 3 && ring_area2(&ring) != 0 {
                out.push(ring);
            }
        }
    }
    out
}

/// Group polygon features of the dissolvable layers by (layer, tags) and
/// replace each group's fragment soup with dissolved union rings. Buildings
/// keep per-feature footprints (extrusion needs them); everything here runs
/// below the detail zoom only.
pub fn dissolve_polygon_features(features: Vec<TileFeature>, zoom: u8) -> Vec<TileFeature> {
    if zoom >= DETAIL_ZOOM {
        return features;
    }
    let mut out = Vec::with_capacity(features.len());
    let mut groups: std::collections::HashMap<(u8, Vec<(String, String)>), TileFeature> =
        std::collections::HashMap::new();
    for feature in features {
        let dissolvable = feature.geometry_type == GeometryType::Polygon
            && matches!(
                feature.layer,
                Layer::BaseWaterPolygons | Layer::BaseLand | Layer::BaseStreetPolygons
            );
        if !dissolvable {
            out.push(feature);
            continue;
        }
        let key = (feature.layer as u8, feature.tags.clone());
        match groups.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.get_mut().paths.extend(feature.paths);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(feature);
            }
        }
    }
    let mut dissolved: Vec<TileFeature> = groups
        .into_values()
        .filter_map(|mut feature| {
            feature.paths = dissolve_rings(std::mem::take(&mut feature.paths));
            if feature.paths.is_empty() {
                None
            } else {
                Some(feature)
            }
        })
        .collect();
    // Deterministic output order regardless of hash iteration.
    dissolved.sort_by(|a, b| (a.layer as u8, a.id).cmp(&(b.layer as u8, b.id)));
    out.extend(dissolved);
    out
}

/// Split a ring that revisits a vertex (pinch / vertex-bowtie, typically
/// created by simplification collapsing a narrow neck) into simple
/// sub-rings. A pinched ring is ambiguous to tessellate — the renderer's
/// sweep fills both lobes while a net-area consumer sees their difference —
/// so every downstream consumer gets simple rings instead.
pub fn split_pinched_ring(ring: Vec<TilePoint>) -> Vec<Vec<TilePoint>> {
    let mut seen: std::collections::HashMap<(i32, i32), usize> = std::collections::HashMap::new();
    for (index, point) in ring.iter().enumerate() {
        if let Some(&first) = seen.get(&(point.x, point.y)) {
            // Split into [first..index] and the remainder; recurse on both.
            let lobe: Vec<TilePoint> = ring[first..index].to_vec();
            let mut rest: Vec<TilePoint> = ring[..first].to_vec();
            rest.extend_from_slice(&ring[index..]);
            let mut out = Vec::new();
            if lobe.len() >= 3 {
                out.extend(split_pinched_ring(lobe));
            }
            if rest.len() >= 3 {
                out.extend(split_pinched_ring(rest));
            }
            return out;
        }
        seen.insert((point.x, point.y), index);
    }
    vec![ring]
}

pub(crate) fn point_in_ring_tile(point: TilePoint, ring: &[TilePoint]) -> bool {
    let mut inside = false;
    let mut previous = *ring.last().unwrap();
    for &current in ring {
        let crosses = (current.y > point.y) != (previous.y > point.y)
            && (f64::from(point.x))
                < f64::from(previous.x - current.x) * f64::from(point.y - current.y)
                    / f64::from(previous.y - current.y)
                    + f64::from(current.x);
        inside ^= crosses;
        previous = current;
    }
    inside
}

/// Order rings into MVT nesting order — every outer immediately followed by
/// its holes — so `split_giant_polygons` can never strand a hole in a chunk
/// away from its outer (the renderer groups rings per feature; an orphaned
/// hole renders as a phantom filled lobe). Holes are assigned to the
/// smallest containing outer, probing several vertices because dissolved
/// holes share boundary vertices with their outers; unassignable holes are
/// dropped (they cannot render meaningfully).
pub fn order_rings_nested(rings: Vec<Vec<TilePoint>>) -> Vec<Vec<TilePoint>> {
    let mut outers: Vec<(Vec<TilePoint>, i64)> = Vec::new();
    let mut holes: Vec<Vec<TilePoint>> = Vec::new();
    for ring in rings.into_iter().flat_map(split_pinched_ring) {
        let area2 = ring_area2(&ring);
        if area2 > 0 {
            outers.push((ring, area2));
        } else if area2 < 0 {
            holes.push(ring);
        }
    }
    let mut holes_of: Vec<Vec<Vec<TilePoint>>> = (0..outers.len()).map(|_| Vec::new()).collect();
    'hole: for hole in holes {
        for probe_step in 0..hole.len().min(8) {
            let probe = hole[probe_step * hole.len() / hole.len().min(8)];
            let owner = outers
                .iter()
                .enumerate()
                .filter(|(_, (outer, _))| point_in_ring_tile(probe, outer))
                .min_by_key(|(_, (_, area2))| *area2)
                .map(|(index, _)| index);
            if let Some(owner) = owner {
                holes_of[owner].push(hole);
                continue 'hole;
            }
        }
        // Orphan hole: drop.
    }
    let mut out = Vec::new();
    for (index, (outer, _)) in outers.into_iter().enumerate() {
        out.push(outer);
        out.append(&mut holes_of[index]);
    }
    out
}

/// Maximum rings per polygon feature below the detail zoom. Large lakes and
/// forests arrive as one feature holding every z14 quilt cell as a ring;
/// per-feature tessellation cost grows superlinearly with vertex count, so
/// monsters are split into bounded chunks. Chunks only break in front of an
/// outer (positive) ring — holes always stay with their outer.
pub const MAX_RINGS_PER_FEATURE: usize = 32;

pub fn split_giant_polygons(features: Vec<TileFeature>, zoom: u8) -> Vec<TileFeature> {
    if zoom >= DETAIL_ZOOM {
        return features;
    }
    let mut out = Vec::with_capacity(features.len());
    for mut feature in features {
        if feature.geometry_type != GeometryType::Polygon
            || feature.paths.len() <= MAX_RINGS_PER_FEATURE
        {
            out.push(feature);
            continue;
        }
        let rings = std::mem::take(&mut feature.paths);
        let mut chunk: Vec<Vec<TilePoint>> = Vec::new();
        for ring in rings {
            let is_outer = ring_area2(&ring) > 0;
            if is_outer && chunk.len() >= MAX_RINGS_PER_FEATURE {
                out.push(TileFeature {
                    paths: std::mem::take(&mut chunk),
                    ..feature.clone()
                });
            }
            chunk.push(ring);
        }
        if !chunk.is_empty() {
            feature.paths = chunk;
            out.push(feature);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_feature(tags: &[(&str, &str)]) -> TileFeature {
        TileFeature {
            layer: Layer::OsmLines,
            geometry_type: GeometryType::LineString,
            osm_type: OsmType::Way,
            id: 100,
            closed: false,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            paths: vec![vec![
                TilePoint { x: 0, y: 0 },
                TilePoint { x: 4096, y: 4096 },
            ]],
        }
    }

    #[test]
    fn motorway_maps_to_streets_at_z5() {
        let feature = line_feature(&[
            ("highway", "motorway"),
            ("ref", "A10"),
            ("oneway", "yes"),
            ("bridge", "yes"),
        ]);
        let specs = base_specs(&feature);
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        assert_eq!(spec.layer, Layer::BaseStreets);
        assert_eq!(spec.min_zoom, 5);
        assert_eq!(tag(&spec.tags, "kind"), Some("motorway"));
        assert_eq!(tag(&spec.tags, "oneway"), Some("true"));
        assert_eq!(tag(&spec.tags, "bridge"), Some("yes"));
        assert_eq!(tag(&spec.tags, "ref"), Some("A10"));
        // id carries the relation bit (way => even).
        assert_eq!(spec.id, 200);
    }

    #[test]
    fn construction_roads_and_platform_rail_are_skipped() {
        assert!(base_specs(&line_feature(&[("highway", "construction")])).is_empty());
        assert!(base_specs(&line_feature(&[("railway", "platform")])).is_empty());
        let rail = base_specs(&line_feature(&[("railway", "rail")]));
        assert_eq!(rail[0].min_zoom, 8);
        assert_eq!(tag(&rail[0].tags, "rail"), Some("true"));
        let siding = base_specs(&line_feature(&[("railway", "rail"), ("service", "siding")]));
        assert_eq!(siding[0].min_zoom, 12);
    }

    #[test]
    fn street_tags_thin_with_zoom() {
        let feature = line_feature(&[
            ("highway", "motorway"),
            ("name", "Autobahn"),
            ("ref", "A1"),
        ]);
        let spec = &base_specs(&feature)[0];
        let z6 = tags_for_zoom(spec.layer, &spec.tags, 6);
        assert!(tag(&z6, "name").is_none());
        assert!(tag(&z6, "ref").is_none());
        let z9 = tags_for_zoom(spec.layer, &spec.tags, 9);
        assert!(tag(&z9, "name").is_none());
        assert_eq!(tag(&z9, "ref"), Some("A1"));
        let z13 = tags_for_zoom(spec.layer, &spec.tags, 13);
        assert_eq!(tag(&z13, "name"), Some("Autobahn"));
    }

    #[test]
    fn water_and_building_polygons_map() {
        let mut feature = line_feature(&[("natural", "water"), ("name", "Lake")]);
        feature.layer = Layer::OsmPolygons;
        feature.geometry_type = GeometryType::Polygon;
        feature.closed = true;
        let specs = base_specs(&feature);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].layer, Layer::BaseWaterPolygons);
        assert_eq!(specs[0].min_zoom, 0);

        let mut building = line_feature(&[("building", "yes"), ("height", "12.5")]);
        building.layer = Layer::OsmRelationPolygons;
        building.osm_type = OsmType::Relation;
        building.geometry_type = GeometryType::Polygon;
        building.closed = true;
        let specs = base_specs(&building);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].layer, Layer::BaseBuildings);
        assert_eq!(tag(&specs[0].tags, "height"), Some("12.5"));
        assert_eq!(specs[0].id, 201); // relation bit set
    }

    #[test]
    fn place_labels_gate_population_digits() {
        let mut node = line_feature(&[
            ("place", "city"),
            ("name", "Amsterdam"),
            ("population", "821752"),
        ]);
        node.layer = Layer::OsmPoints;
        node.geometry_type = GeometryType::Point;
        let specs = base_specs(&node);
        assert_eq!(specs[0].layer, Layer::BasePlaceLabels);
        assert_eq!(specs[0].min_zoom, 4);
        assert_eq!(tag(&specs[0].tags, "population"), Some("821752"));

        let mut bad = line_feature(&[("place", "town"), ("name", "X"), ("population", "12k")]);
        bad.layer = Layer::OsmPoints;
        bad.geometry_type = GeometryType::Point;
        assert!(tag(&base_specs(&bad)[0].tags, "population").is_none());
    }

    #[test]
    fn exact_clip_dedupes_buffered_overlap() {
        // A horizontal line crossing the boundary between tiles (0,0) and
        // (1,0), spooled into both with a 64-unit buffer.
        let path_a = vec![
            TilePoint { x: 4000, y: 100 },
            TilePoint { x: 4160, y: 100 }, // buffer cut in tile 0
        ];
        let path_b = vec![
            TilePoint { x: -96, y: 100 }, // same geometry local to tile 1
            TilePoint { x: 64, y: 100 },
        ];
        let a = exact_clip_to_tile(0, 0, GeometryType::LineString, false, &[path_a]);
        let b = exact_clip_to_tile(1, 0, GeometryType::LineString, false, &[path_b]);
        assert_eq!(a.paths[0].last(), Some(&GlobalPoint { x: 4096, y: 100 }));
        assert_eq!(b.paths[0].first(), Some(&GlobalPoint { x: 4096, y: 100 }));

        // Downsample both fragments to z13 and stitch: one continuous path.
        let da = downsample_paths(&a, GeometryType::LineString, 13);
        let db = downsample_paths(&b, GeometryType::LineString, 13);
        let fragments = to_target_tiles(
            13,
            GeometryType::LineString,
            &[da[0].clone(), db[0].clone()],
            64,
        )
        .unwrap();
        assert_eq!(fragments.len(), 1);
        let stitched = stitch_paths(fragments.into_iter().next().unwrap().paths);
        assert_eq!(stitched.len(), 1);
    }

    #[test]
    fn simplify_keeps_endpoints_and_meaningful_corners() {
        let path = vec![
            TilePoint { x: 0, y: 0 },
            TilePoint { x: 100, y: 2 },
            TilePoint { x: 200, y: 0 },
            TilePoint { x: 200, y: 200 },
        ];
        let simplified = simplify_path(&path, 8.0);
        assert_eq!(simplified.first(), Some(&TilePoint { x: 0, y: 0 }));
        assert_eq!(simplified.last(), Some(&TilePoint { x: 200, y: 200 }));
        assert!(simplified.len() == 3, "wiggle removed, corner kept: {simplified:?}");
    }

    #[test]
    fn quilt_cell_rings_survive_low_zoom_simplification() {
        // At z5 one z14 tile of a big lake quilt is an 8x8-unit square; the
        // bbox-capped epsilon must keep its corners instead of erasing it.
        for size in [1, 2, 8, 64] {
            let ring = vec![
                TilePoint { x: 0, y: 0 },
                TilePoint { x: size, y: 0 },
                TilePoint { x: size, y: size },
                TilePoint { x: 0, y: size },
            ];
            let simplified = simplify_ring(&ring, SIMPLIFY_EPSILON_UNITS)
                .unwrap_or_else(|| panic!("{size}x{size} ring was erased"));
            assert_eq!(simplified.len(), 4, "{size}x{size} ring lost corners");
        }
        // A stitched closed ring must not absorb a touching extra path.
        let closed = vec![
            TilePoint { x: 0, y: 0 },
            TilePoint { x: 10, y: 0 },
            TilePoint { x: 10, y: 10 },
            TilePoint { x: 0, y: 0 },
        ];
        let touching = vec![TilePoint { x: 0, y: 0 }, TilePoint { x: -5, y: -5 }];
        let stitched = stitch_paths(vec![closed.clone(), touching.clone()]);
        assert_eq!(stitched.len(), 2);
        assert!(stitched.contains(&closed));
    }

    #[test]
    fn dissolve_merges_exact_abutting_quilt_cells() {
        // Two 10x10 cells sharing the x=10 edge dissolve into one 20x10
        // rectangle (outer positive winding in y-down space).
        let cell = |x0: i32| {
            vec![
                TilePoint { x: x0, y: 0 },
                TilePoint { x: x0 + 10, y: 0 },
                TilePoint { x: x0 + 10, y: 10 },
                TilePoint { x: x0, y: 10 },
            ]
        };
        let dissolved = dissolve_rings(vec![cell(0), cell(10)]);
        assert_eq!(dissolved.len(), 1);
        assert_eq!(ring_area2(&dissolved[0]), 2 * 200);
        // A 2x2 block of cells with the center cell missing yields an outer
        // ring plus a negative hole ring.
        let sq = |x0: i32, y0: i32, s: i32| {
            vec![
                TilePoint { x: x0, y: y0 },
                TilePoint { x: x0 + s, y: y0 },
                TilePoint { x: x0 + s, y: y0 + s },
                TilePoint { x: x0, y: y0 + s },
            ]
        };
        // Ring of 8 cells around a missing center (3x3 grid of 10-unit cells).
        let mut cells = Vec::new();
        for gy in 0..3 {
            for gx in 0..3 {
                if (gx, gy) != (1, 1) {
                    cells.push(sq(gx * 10, gy * 10, 10));
                }
            }
        }
        let dissolved = dissolve_rings(cells);
        let outer: i64 = dissolved.iter().map(|r| ring_area2(r)).filter(|a| *a > 0).sum();
        let holes: i64 = dissolved.iter().map(|r| ring_area2(r)).filter(|a| *a < 0).sum();
        assert_eq!(outer, 2 * 900);
        assert_eq!(holes, -2 * 100);
        // Non-abutting cells stay separate.
        let apart = dissolve_rings(vec![sq(0, 0, 10), sq(30, 0, 10)]);
        assert_eq!(apart.len(), 2);
    }

    #[test]
    fn nested_order_keeps_holes_behind_their_outers_and_splits_pinches() {
        let sq = |x0: i32, y0: i32, s: i32, cw: bool| {
            let mut r = vec![
                TilePoint { x: x0, y: y0 },
                TilePoint { x: x0 + s, y: y0 },
                TilePoint { x: x0 + s, y: y0 + s },
                TilePoint { x: x0, y: y0 + s },
            ];
            if !cw {
                r.reverse();
            }
            r
        };
        // Input scrambled: hole of the SECOND outer listed first.
        let rings = vec![
            sq(105, 105, 10, false), // hole of outer B
            sq(0, 0, 50, true),      // outer A
            sq(100, 100, 50, true),  // outer B
            sq(5, 5, 10, false),     // hole of outer A
        ];
        let ordered = order_rings_nested(rings);
        assert_eq!(ordered.len(), 4);
        assert!(ring_area2(&ordered[0]) > 0);
        assert!(ring_area2(&ordered[1]) < 0); // A's hole right after A
        assert!(ordered[1].iter().all(|p| p.x <= 50));
        assert!(ring_area2(&ordered[2]) > 0);
        assert!(ring_area2(&ordered[3]) < 0); // B's hole right after B
        assert!(ordered[3].iter().all(|p| p.x >= 100));
        // Orphan hole (no containing outer) is dropped.
        let orphan = order_rings_nested(vec![sq(0, 0, 10, false)]);
        assert!(orphan.is_empty());
        // Pinched figure-8 splits into two simple rings.
        let pinched = vec![
            TilePoint { x: 0, y: 0 },
            TilePoint { x: 10, y: 0 },
            TilePoint { x: 10, y: 10 },
            TilePoint { x: 0, y: 0 }, // revisits start
            TilePoint { x: -10, y: 0 },
            TilePoint { x: -10, y: -10 },
        ];
        let split = split_pinched_ring(pinched);
        assert_eq!(split.len(), 2);
        assert!(split.iter().all(|r| {
            let mut seen = std::collections::HashSet::new();
            r.iter().all(|p| seen.insert((p.x, p.y)))
        }));
    }

    #[test]
    fn dissolve_splits_corner_junctions_into_simple_rings() {
        let sq = |x0: i32, y0: i32, s: i32| {
            vec![
                TilePoint { x: x0, y: y0 },
                TilePoint { x: x0 + s, y: y0 },
                TilePoint { x: x0 + s, y: y0 + s },
                TilePoint { x: x0, y: y0 + s },
            ]
        };
        // Checkerboard diagonal: two filled cells touching only at one
        // corner. The face walk must produce TWO simple positive rings,
        // never one figure-8 through the junction.
        let dissolved = dissolve_rings(vec![sq(0, 0, 10), sq(10, 10, 10)]);
        assert_eq!(dissolved.len(), 2, "{dissolved:?}");
        for ring in &dissolved {
            assert_eq!(ring_area2(ring), 2 * 100, "{ring:?}");
        }
        // Plus-shape: 4 cells around a center cell, all filled — one outer
        // ring, no hole, and every corner junction resolved simply.
        let plus = dissolve_rings(vec![
            sq(10, 0, 10),
            sq(0, 10, 10),
            sq(10, 10, 10),
            sq(20, 10, 10),
            sq(10, 20, 10),
        ]);
        let total: i64 = plus.iter().map(|r| ring_area2(r)).sum();
        assert_eq!(total, 2 * 500);
        assert!(
            plus.iter().all(|r| ring_area2(r) > 0),
            "no inverted rings: {plus:?}"
        );
    }

    #[test]
    fn same_tag_way_fragments_merge_and_chain() {
        // Three consecutive motorway ways with identical thinned tags must
        // become ONE feature with ONE stitched path at overview zooms.
        let tags = vec![("kind".to_string(), "motorway".to_string())];
        let make = |id: i64, from: TilePoint, to: TilePoint| TileFeature {
            layer: Layer::BaseStreets,
            geometry_type: GeometryType::LineString,
            osm_type: OsmType::Way,
            id,
            closed: false,
            tags: tags.clone(),
            paths: vec![vec![from, to]],
        };
        let merged = merge_features_by_tags(
            vec![
                make(2, TilePoint { x: 0, y: 0 }, TilePoint { x: 100, y: 0 }),
                make(4, TilePoint { x: 100, y: 0 }, TilePoint { x: 200, y: 1 }),
                make(6, TilePoint { x: 200, y: 1 }, TilePoint { x: 300, y: 200 }),
            ],
            5,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].paths.len(), 1);
        // The collinear joint at (100,0) is removed by the second DP pass.
        assert!(merged[0].paths[0].len() <= 3, "{:?}", merged[0].paths);
        // Different tags never merge.
        let other_tags = vec![("kind".to_string(), "trunk".to_string())];
        let mut trunk = make(8, TilePoint { x: 0, y: 5 }, TilePoint { x: 9, y: 5 });
        trunk.tags = other_tags;
        let kept = merge_features_by_tags(
            vec![
                make(2, TilePoint { x: 0, y: 0 }, TilePoint { x: 100, y: 0 }),
                trunk,
            ],
            5,
        );
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn finalize_drops_small_polygons_below_detail_zoom() {
        // Positive (outer) orientation in y-down space.
        let small_ring = vec![
            TilePoint { x: 0, y: 0 },
            TilePoint { x: 20, y: 0 },
            TilePoint { x: 20, y: 20 },
            TilePoint { x: 0, y: 20 },
        ];
        let feature = TileFeature {
            layer: Layer::BaseWaterPolygons,
            geometry_type: GeometryType::Polygon,
            osm_type: OsmType::Way,
            id: 2,
            closed: true,
            tags: Vec::new(),
            paths: vec![small_ring.clone()],
        };
        // 20x20 units = 400 units^2 => 2A = 800 < 2048: dropped below z14.
        assert!(finalize_feature(feature.clone(), 10).is_none());
        // Kept at the detail zoom.
        assert!(finalize_feature(feature, DETAIL_ZOOM).is_some());
    }
}
