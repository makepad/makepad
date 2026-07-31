use super::mvt::{GeometryType, Layer, OsmType, TagPair, TilePoint};
use super::spool::BlockSpoolWriter;
use super::store::NodeCoord;
use super::FastHashMap;

pub const MVT_EXTENT: i64 = 4096;
pub const TILE_BUFFER: i64 = 64;
const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalPoint {
    pub x: i64,
    pub y: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourcePath {
    pub nodes: Vec<NodeCoord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolygonPart {
    pub outer: Vec<GlobalPoint>,
    pub holes: Vec<Vec<GlobalPoint>>,
}

pub fn project_decimicro(id: i64, lon: i32, lat: i32, zoom: u8) -> NodeCoord {
    let lon = f64::from(lon) * 1e-7;
    let lat = (f64::from(lat) * 1e-7).clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
    let world = ((1_u64 << zoom) as f64) * MVT_EXTENT as f64;
    let normalized_x = (lon + 180.0) / 360.0;
    let sin_lat = lat.to_radians().sin();
    let normalized_y =
        0.5 - ((1.0 + sin_lat) / (1.0 - sin_lat)).ln() / (4.0 * std::f64::consts::PI);
    NodeCoord {
        id,
        x: (normalized_x * world).round() as i64,
        y: (normalized_y * world).round() as i64,
    }
}

pub fn project_node(node: NodeCoord) -> GlobalPoint {
    GlobalPoint {
        x: node.x,
        y: node.y,
    }
}

pub fn project_path(path: &[NodeCoord]) -> Vec<GlobalPoint> {
    let mut result = Vec::with_capacity(path.len());
    for &node in path {
        let point = project_node(node);
        if result.last() != Some(&point) {
            result.push(point);
        }
    }
    result
}

pub fn emit_point<T: TagPair>(
    spool: &mut BlockSpoolWriter,
    zoom: u8,
    layer: Layer,
    osm_type: OsmType,
    id: i64,
    tags: &[T],
    point: GlobalPoint,
) -> Result<u64, String> {
    let axis = 1_i64 << zoom;
    let tile_x = point.x.div_euclid(MVT_EXTENT).clamp(0, axis - 1) as u32;
    let tile_y = point.y.div_euclid(MVT_EXTENT).clamp(0, axis - 1) as u32;
    let local = to_local(point, tile_x, tile_y)?;
    let points = [local];
    spool.push_parts(
        tile_x,
        tile_y,
        layer,
        GeometryType::Point,
        osm_type,
        id,
        false,
        tags,
        std::iter::once(points.as_slice()),
    )?;
    Ok(1)
}

pub fn emit_lines<T: TagPair>(
    spool: &mut BlockSpoolWriter,
    zoom: u8,
    layer: Layer,
    osm_type: OsmType,
    id: i64,
    closed: bool,
    tags: &[T],
    paths: &[Vec<GlobalPoint>],
) -> Result<u64, String> {
    let mut emitted = 0_u64;
    for path in paths {
        if path.len() < 2 {
            continue;
        }
        let Some((min_x, min_y, max_x, max_y)) = bounds(path) else {
            continue;
        };
        let range = tile_range(zoom, min_x, min_y, max_x, max_y, TILE_BUFFER)?;
        for tile_y in range.y_min..=range.y_max {
            for tile_x in range.x_min..=range.x_max {
                let rect = tile_rect(tile_x, tile_y, TILE_BUFFER);
                let clipped = clip_line(path, rect);
                if clipped.is_empty() {
                    continue;
                }
                let mut local_paths = Vec::with_capacity(clipped.len());
                for clipped_path in clipped {
                    let mut local = Vec::with_capacity(clipped_path.len());
                    for point in clipped_path {
                        local.push(to_local(point, tile_x, tile_y)?);
                    }
                    remove_consecutive_duplicates(&mut local);
                    if local.len() >= 2 {
                        local_paths.push(local);
                    }
                }
                if local_paths.is_empty() {
                    continue;
                }
                spool.push_parts(
                    tile_x,
                    tile_y,
                    layer,
                    GeometryType::LineString,
                    osm_type,
                    id,
                    closed,
                    tags,
                    local_paths.iter().map(Vec::as_slice),
                )?;
                emitted += 1;
            }
        }
    }
    Ok(emitted)
}

pub fn emit_polygons<T: TagPair>(
    spool: &mut BlockSpoolWriter,
    zoom: u8,
    layer: Layer,
    osm_type: OsmType,
    id: i64,
    tags: &[T],
    polygons: &[PolygonPart],
) -> Result<u64, String> {
    let mut emitted = 0_u64;
    for polygon in polygons {
        if polygon.outer.len() < 3 {
            continue;
        }
        let Some((min_x, min_y, max_x, max_y)) = bounds(&polygon.outer) else {
            continue;
        };
        let range = tile_range(zoom, min_x, min_y, max_x, max_y, TILE_BUFFER)?;
        for tile_y in range.y_min..=range.y_max {
            for tile_x in range.x_min..=range.x_max {
                let rect = tile_rect(tile_x, tile_y, TILE_BUFFER);
                let mut outer = clip_ring(&polygon.outer, rect);
                if !normalize_ring(&mut outer, true) {
                    continue;
                }
                let mut paths = vec![to_local_ring(&outer, tile_x, tile_y)?];
                for hole in &polygon.holes {
                    let mut clipped = clip_ring(hole, rect);
                    if normalize_ring(&mut clipped, false) {
                        paths.push(to_local_ring(&clipped, tile_x, tile_y)?);
                    }
                }
                spool.push_parts(
                    tile_x,
                    tile_y,
                    layer,
                    GeometryType::Polygon,
                    osm_type,
                    id,
                    true,
                    tags,
                    paths.iter().map(Vec::as_slice),
                )?;
                emitted += 1;
            }
        }
    }
    Ok(emitted)
}

/// A feature fully localized to one tile, ready for the spool writer —
/// produced on resolver worker threads so the single writer only appends.
pub struct PreparedFeature {
    pub tile_x: u32,
    pub tile_y: u32,
    pub layer: Layer,
    pub geometry_type: GeometryType,
    pub osm_type: OsmType,
    pub id: i64,
    pub closed: bool,
    pub paths: Vec<Vec<TilePoint>>,
}

/// emit_point minus the spool.
pub fn prepare_point(
    zoom: u8,
    layer: Layer,
    osm_type: OsmType,
    id: i64,
    point: GlobalPoint,
    out: &mut Vec<PreparedFeature>,
) -> Result<(), String> {
    let axis = 1_i64 << zoom;
    let tile_x = point.x.div_euclid(MVT_EXTENT).clamp(0, axis - 1) as u32;
    let tile_y = point.y.div_euclid(MVT_EXTENT).clamp(0, axis - 1) as u32;
    let local = to_local(point, tile_x, tile_y)?;
    out.push(PreparedFeature {
        tile_x,
        tile_y,
        layer,
        geometry_type: GeometryType::Point,
        osm_type,
        id,
        closed: false,
        paths: vec![vec![local]],
    });
    Ok(())
}

/// emit_lines minus the spool: collect per-tile localized features.
#[allow(clippy::too_many_arguments)]
pub fn prepare_lines(
    zoom: u8,
    layer: Layer,
    osm_type: OsmType,
    id: i64,
    closed: bool,
    paths: &[Vec<GlobalPoint>],
    out: &mut Vec<PreparedFeature>,
) -> Result<(), String> {
    for path in paths {
        if path.len() < 2 {
            continue;
        }
        let Some((min_x, min_y, max_x, max_y)) = bounds(path) else {
            continue;
        };
        let range = tile_range(zoom, min_x, min_y, max_x, max_y, TILE_BUFFER)?;
        for tile_y in range.y_min..=range.y_max {
            for tile_x in range.x_min..=range.x_max {
                let rect = tile_rect(tile_x, tile_y, TILE_BUFFER);
                let clipped = clip_line(path, rect);
                if clipped.is_empty() {
                    continue;
                }
                let mut local_paths = Vec::with_capacity(clipped.len());
                for clipped_path in clipped {
                    let mut local = Vec::with_capacity(clipped_path.len());
                    for point in clipped_path {
                        local.push(to_local(point, tile_x, tile_y)?);
                    }
                    remove_consecutive_duplicates(&mut local);
                    if local.len() >= 2 {
                        local_paths.push(local);
                    }
                }
                if local_paths.is_empty() {
                    continue;
                }
                out.push(PreparedFeature {
                    tile_x,
                    tile_y,
                    layer,
                    geometry_type: GeometryType::LineString,
                    osm_type,
                    id,
                    closed,
                    paths: local_paths,
                });
            }
        }
    }
    Ok(())
}

/// emit_polygons minus the spool: collect per-tile localized features.
pub fn prepare_polygons(
    zoom: u8,
    layer: Layer,
    osm_type: OsmType,
    id: i64,
    polygons: &[PolygonPart],
    out: &mut Vec<PreparedFeature>,
) -> Result<(), String> {
    for polygon in polygons {
        if polygon.outer.len() < 3 {
            continue;
        }
        let Some((min_x, min_y, max_x, max_y)) = bounds(&polygon.outer) else {
            continue;
        };
        let range = tile_range(zoom, min_x, min_y, max_x, max_y, TILE_BUFFER)?;
        for tile_y in range.y_min..=range.y_max {
            for tile_x in range.x_min..=range.x_max {
                let rect = tile_rect(tile_x, tile_y, TILE_BUFFER);
                let mut outer = clip_ring(&polygon.outer, rect);
                if !normalize_ring(&mut outer, true) {
                    continue;
                }
                let mut paths = vec![to_local_ring(&outer, tile_x, tile_y)?];
                for hole in &polygon.holes {
                    let mut clipped = clip_ring(hole, rect);
                    if normalize_ring(&mut clipped, false) {
                        paths.push(to_local_ring(&clipped, tile_x, tile_y)?);
                    }
                }
                out.push(PreparedFeature {
                    tile_x,
                    tile_y,
                    layer,
                    geometry_type: GeometryType::Polygon,
                    osm_type,
                    id,
                    closed: true,
                    paths,
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn to_local(point: GlobalPoint, tile_x: u32, tile_y: u32) -> Result<TilePoint, String> {
    let x = point.x - i64::from(tile_x) * MVT_EXTENT;
    let y = point.y - i64::from(tile_y) * MVT_EXTENT;
    Ok(TilePoint {
        x: i32::try_from(x).map_err(|_| "tile-local x exceeds i32".to_string())?,
        y: i32::try_from(y).map_err(|_| "tile-local y exceeds i32".to_string())?,
    })
}

fn to_local_ring(
    ring: &[GlobalPoint],
    tile_x: u32,
    tile_y: u32,
) -> Result<Vec<TilePoint>, String> {
    ring.iter()
        .copied()
        .map(|point| to_local(point, tile_x, tile_y))
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) struct TileRange {
    pub(crate) x_min: u32,
    pub(crate) y_min: u32,
    pub(crate) x_max: u32,
    pub(crate) y_max: u32,
}

pub(crate) fn tile_range(
    zoom: u8,
    min_x: i64,
    min_y: i64,
    max_x: i64,
    max_y: i64,
    buffer: i64,
) -> Result<TileRange, String> {
    let axis = 1_i64
        .checked_shl(u32::from(zoom))
        .ok_or_else(|| format!("zoom {zoom} is too large"))?;
    let x_min = (min_x - buffer).div_euclid(MVT_EXTENT).clamp(0, axis - 1);
    let y_min = (min_y - buffer).div_euclid(MVT_EXTENT).clamp(0, axis - 1);
    let x_max = (max_x + buffer).div_euclid(MVT_EXTENT).clamp(0, axis - 1);
    let y_max = (max_y + buffer).div_euclid(MVT_EXTENT).clamp(0, axis - 1);
    Ok(TileRange {
        x_min: x_min as u32,
        y_min: y_min as u32,
        x_max: x_max as u32,
        y_max: y_max as u32,
    })
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Rect {
    pub(crate) min_x: f64,
    pub(crate) min_y: f64,
    pub(crate) max_x: f64,
    pub(crate) max_y: f64,
}

pub(crate) fn tile_rect(tile_x: u32, tile_y: u32, buffer: i64) -> Rect {
    let x = i64::from(tile_x) * MVT_EXTENT;
    let y = i64::from(tile_y) * MVT_EXTENT;
    Rect {
        min_x: (x - buffer) as f64,
        min_y: (y - buffer) as f64,
        max_x: (x + MVT_EXTENT + buffer) as f64,
        max_y: (y + MVT_EXTENT + buffer) as f64,
    }
}

pub(crate) fn bounds(points: &[GlobalPoint]) -> Option<(i64, i64, i64, i64)> {
    let first = *points.first()?;
    let mut result = (first.x, first.y, first.x, first.y);
    for point in &points[1..] {
        result.0 = result.0.min(point.x);
        result.1 = result.1.min(point.y);
        result.2 = result.2.max(point.x);
        result.3 = result.3.max(point.y);
    }
    Some(result)
}

pub(crate) fn clip_line(points: &[GlobalPoint], rect: Rect) -> Vec<Vec<GlobalPoint>> {
    let mut output = Vec::new();
    let mut current = Vec::new();
    for segment in points.windows(2) {
        if let Some((start, end)) = clip_segment(segment[0], segment[1], rect) {
            if current.last() != Some(&start) {
                if current.len() >= 2 {
                    output.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                current.push(start);
            }
            if current.last() != Some(&end) {
                current.push(end);
            }
        } else if current.len() >= 2 {
            output.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() >= 2 {
        output.push(current);
    }
    output
}

fn clip_segment(
    start: GlobalPoint,
    end: GlobalPoint,
    rect: Rect,
) -> Option<(GlobalPoint, GlobalPoint)> {
    let x0 = start.x as f64;
    let y0 = start.y as f64;
    let dx = (end.x - start.x) as f64;
    let dy = (end.y - start.y) as f64;
    let mut t0: f64 = 0.0;
    let mut t1: f64 = 1.0;
    for (p, q) in [
        (-dx, x0 - rect.min_x),
        (dx, rect.max_x - x0),
        (-dy, y0 - rect.min_y),
        (dy, rect.max_y - y0),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return None;
                }
                t0 = t0.max(r);
            } else {
                if r < t0 {
                    return None;
                }
                t1 = t1.min(r);
            }
        }
    }
    let point = |t: f64| GlobalPoint {
        x: (x0 + t * dx).round() as i64,
        y: (y0 + t * dy).round() as i64,
    };
    Some((point(t0), point(t1)))
}

pub(crate) fn clip_ring(points: &[GlobalPoint], rect: Rect) -> Vec<GlobalPoint> {
    let mut output = points.to_vec();
    if output.first() == output.last() {
        output.pop();
    }
    for edge in 0..4 {
        if output.is_empty() {
            break;
        }
        let input = std::mem::take(&mut output);
        let mut previous = *input.last().unwrap();
        let mut previous_inside = inside(previous, rect, edge);
        for current in input {
            let current_inside = inside(current, rect, edge);
            if current_inside {
                if !previous_inside {
                    output.push(edge_intersection(previous, current, rect, edge));
                }
                output.push(current);
            } else if previous_inside {
                output.push(edge_intersection(previous, current, rect, edge));
            }
            previous = current;
            previous_inside = current_inside;
        }
        remove_consecutive_duplicates(&mut output);
    }
    output
}

fn inside(point: GlobalPoint, rect: Rect, edge: usize) -> bool {
    match edge {
        0 => point.x as f64 >= rect.min_x,
        1 => point.x as f64 <= rect.max_x,
        2 => point.y as f64 >= rect.min_y,
        _ => point.y as f64 <= rect.max_y,
    }
}

fn edge_intersection(start: GlobalPoint, end: GlobalPoint, rect: Rect, edge: usize) -> GlobalPoint {
    let x0 = start.x as f64;
    let y0 = start.y as f64;
    let dx = (end.x - start.x) as f64;
    let dy = (end.y - start.y) as f64;
    let t = match edge {
        0 => (rect.min_x - x0) / dx,
        1 => (rect.max_x - x0) / dx,
        2 => (rect.min_y - y0) / dy,
        _ => (rect.max_y - y0) / dy,
    };
    GlobalPoint {
        x: (x0 + t * dx).round() as i64,
        y: (y0 + t * dy).round() as i64,
    }
}

pub(crate) fn remove_consecutive_duplicates<T: PartialEq>(points: &mut Vec<T>) {
    points.dedup();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
}

pub(crate) fn signed_area(points: &[GlobalPoint]) -> i128 {
    if points.len() < 3 {
        return 0;
    }
    let mut area = 0_i128;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        area += i128::from(a.x) * i128::from(b.y) - i128::from(b.x) * i128::from(a.y);
    }
    area
}

fn normalize_ring(points: &mut Vec<GlobalPoint>, outer: bool) -> bool {
    remove_consecutive_duplicates(points);
    if points.len() < 3 {
        return false;
    }
    let area = signed_area(points);
    if area == 0 {
        return false;
    }
    if (outer && area < 0) || (!outer && area > 0) {
        points.reverse();
    }
    true
}

pub fn assemble_rings(paths: Vec<SourcePath>) -> (Vec<SourcePath>, Vec<SourcePath>) {
    let mut endpoints = FastHashMap::<i64, Vec<usize>>::default();
    for (index, path) in paths.iter().enumerate() {
        if let (Some(first), Some(last)) = (path.nodes.first(), path.nodes.last()) {
            endpoints.entry(first.id).or_default().push(index);
            if first.id != last.id {
                endpoints.entry(last.id).or_default().push(index);
            }
        }
    }
    let mut used = vec![false; paths.len()];
    let mut closed = Vec::new();
    let mut open = Vec::new();
    for start in 0..paths.len() {
        if used[start] || paths[start].nodes.is_empty() {
            continue;
        }
        used[start] = true;
        let mut nodes = paths[start].nodes.clone();
        loop {
            let Some(end_id) = nodes.last().map(|node| node.id) else {
                break;
            };
            if nodes.len() > 2 && nodes.first().unwrap().id == end_id {
                break;
            }
            let Some(candidates) = endpoints.get_mut(&end_id) else {
                break;
            };
            let Some(next) = candidates.iter().copied().find(|index| !used[*index]) else {
                break;
            };
            used[next] = true;
            let mut extension = paths[next].nodes.clone();
            if extension.first().is_some_and(|node| node.id != end_id) {
                extension.reverse();
            }
            if extension.first().is_none_or(|node| node.id != end_id) {
                break;
            }
            nodes.extend(extension.into_iter().skip(1));
        }
        let result = SourcePath { nodes };
        if result.nodes.len() > 3
            && result.nodes.first().map(|node| node.id)
                == result.nodes.last().map(|node| node.id)
        {
            closed.push(result);
        } else {
            open.push(result);
        }
    }
    (closed, open)
}

pub fn group_polygon_rings(
    outer_paths: Vec<SourcePath>,
    inner_paths: Vec<SourcePath>,
) -> (Vec<PolygonPart>, Vec<Vec<GlobalPoint>>) {
    let (closed_outer, mut open) = assemble_rings(outer_paths);
    let (closed_inner, open_inner) = assemble_rings(inner_paths);
    open.extend(open_inner);

    let mut polygons = closed_outer
        .into_iter()
        .map(|path| {
            let mut outer = project_path(&path.nodes);
            normalize_ring(&mut outer, true);
            PolygonPart {
                outer,
                holes: Vec::new(),
            }
        })
        .filter(|part| part.outer.len() >= 3)
        .collect::<Vec<_>>();

    for path in closed_inner {
        let mut hole = project_path(&path.nodes);
        if !normalize_ring(&mut hole, false) {
            continue;
        }
        let probe = hole[0];
        if let Some(polygon) = polygons
            .iter_mut()
            .filter(|polygon| point_in_ring(probe, &polygon.outer))
            .min_by_key(|polygon| signed_area(&polygon.outer).abs())
        {
            polygon.holes.push(hole);
        } else {
            open.push(SourcePath {
                nodes: path.nodes,
            });
        }
    }
    let open = open
        .into_iter()
        .map(|path| project_path(&path.nodes))
        .filter(|path| path.len() >= 2)
        .collect();
    (polygons, open)
}

fn point_in_ring(point: GlobalPoint, ring: &[GlobalPoint]) -> bool {
    let mut inside = false;
    let mut previous = *ring.last().unwrap_or(&point);
    for &current in ring {
        let crosses = (current.y > point.y) != (previous.y > point.y)
            && (point.x as f64)
                < (previous.x - current.x) as f64 * (point.y - current.y) as f64
                    / (previous.y - current.y) as f64
                    + current.x as f64;
        inside ^= crosses;
        previous = current;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, x: i32, y: i32) -> NodeCoord {
        NodeCoord {
            id,
            x: i64::from(x),
            y: i64::from(y),
        }
    }

    #[test]
    fn segment_and_ring_clipping_keep_exact_edges() {
        let rect = Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 100.0,
        };
        assert_eq!(
            clip_segment(
                GlobalPoint { x: -10, y: 50 },
                GlobalPoint { x: 110, y: 50 },
                rect
            ),
            Some((
                GlobalPoint { x: 0, y: 50 },
                GlobalPoint { x: 100, y: 50 }
            ))
        );
        let ring = vec![
            GlobalPoint { x: -10, y: -10 },
            GlobalPoint { x: 110, y: -10 },
            GlobalPoint { x: 110, y: 110 },
            GlobalPoint { x: -10, y: 110 },
        ];
        let mut clipped = clip_ring(&ring, rect);
        assert!(normalize_ring(&mut clipped, true));
        assert_eq!(clipped.len(), 4);
        assert_eq!(signed_area(&clipped), 20_000);
    }

    #[test]
    fn assembles_reversed_way_members_into_ring() {
        let paths = vec![
            SourcePath {
                nodes: vec![node(1, 0, 0), node(2, 1, 0)],
            },
            SourcePath {
                nodes: vec![node(3, 1, 1), node(2, 1, 0)],
            },
            SourcePath {
                nodes: vec![node(3, 1, 1), node(4, 0, 1), node(1, 0, 0)],
            },
        ];
        let (closed, open) = assemble_rings(paths);
        assert!(open.is_empty());
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].nodes.first().unwrap().id, 1);
        assert_eq!(closed[0].nodes.last().unwrap().id, 1);
        assert_eq!(closed[0].nodes.len(), 5);
    }
}
