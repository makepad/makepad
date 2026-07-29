use super::style::StrokePassStyle;
use crate::makepad_draw::vector::{
    append_expanded_stroke_geometry, tessellate_path_fill,
    tessellate_path_stroke_ends_anchored, LineCap, LineJoin, Tessellator, VVertex, VectorPath,
    VectorRenderParams, VECTOR_ZBIAS_STEP,
};
use crate::makepad_draw::vector::Winding;
use crate::makepad_draw::*;

/// Curve flattening tolerance for map tile geometry, in tile-local units.
///
/// Tiles are tessellated once per tile/zoom and cached, so the tolerance is
/// fixed rather than derived from the device scale the way `DrawVector` does it.
pub const DEFAULT_FLATTEN_TOLERANCE: f32 = 0.25;

// --- Point/bounds types for geometry operations ---

#[derive(Clone, Copy, Debug)]
pub struct GeoPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct GeoBounds {
    pub min: GeoPoint,
    pub max: GeoPoint,
}

impl GeoPoint {
    pub fn from_tuple(point: (f32, f32)) -> Self {
        Self {
            x: point.0,
            y: point.1,
        }
    }

    pub fn to_tuple(self) -> (f32, f32) {
        (self.x, self.y)
    }
}

// --- Polyline simplification (Douglas-Peucker) ---

fn sq_dist(a: GeoPoint, b: GeoPoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * dx + dy * dy
}

fn sq_closest_point_on_segment(point: GeoPoint, a: GeoPoint, b: GeoPoint) -> f32 {
    let mut x = a.x;
    let mut y = a.y;
    let mut dx = b.x - x;
    let mut dy = b.y - y;
    let dot = dx * dx + dy * dy;

    if dot > 0.0 {
        let t = ((point.x - x) * dx + (point.y - y) * dy) / dot;
        if t > 1.0 {
            x = b.x;
            y = b.y;
        } else if t > 0.0 {
            x += dx * t;
            y += dy * t;
        }
    }

    dx = point.x - x;
    dy = point.y - y;
    dx * dx + dy * dy
}

fn simplify_dp_step(
    points: &[GeoPoint],
    markers: &mut [bool],
    sq_tolerance: f32,
    first: usize,
    last: usize,
) {
    if last <= first + 1 {
        return;
    }

    let mut max_sq_dist = 0.0_f32;
    let mut index = first;
    for i in first + 1..last {
        let sq = sq_closest_point_on_segment(points[i], points[first], points[last]);
        if sq > max_sq_dist {
            max_sq_dist = sq;
            index = i;
        }
    }

    if max_sq_dist > sq_tolerance {
        markers[index] = true;
        simplify_dp_step(points, markers, sq_tolerance, first, index);
        simplify_dp_step(points, markers, sq_tolerance, index, last);
    }
}

pub fn simplify_polyline(points: &[(f32, f32)], tolerance: f32) -> Vec<(f32, f32)> {
    if tolerance <= f32::EPSILON || points.len() < 2 {
        return points.to_vec();
    }

    let sq_tolerance = tolerance * tolerance;
    let points = points
        .iter()
        .copied()
        .map(GeoPoint::from_tuple)
        .collect::<Vec<_>>();

    let mut reduced = Vec::<GeoPoint>::with_capacity(points.len());
    reduced.push(points[0]);
    let mut prev = 0_usize;
    for i in 1..points.len() {
        if sq_dist(points[i], points[prev]) > sq_tolerance {
            reduced.push(points[i]);
            prev = i;
        }
    }
    if prev < points.len() - 1 {
        reduced.push(*points.last().unwrap_or(&points[0]));
    }

    if reduced.len() < 3 {
        return reduced.into_iter().map(GeoPoint::to_tuple).collect();
    }

    let len = reduced.len();
    let mut markers = vec![false; len];
    markers[0] = true;
    markers[len - 1] = true;
    simplify_dp_step(&reduced, &mut markers, sq_tolerance, 0, len - 1);

    let mut out = Vec::<(f32, f32)>::with_capacity(len);
    for (index, point) in reduced.into_iter().enumerate() {
        if markers[index] {
            out.push(point.to_tuple());
        }
    }
    out
}

// --- Line clipping (Cohen-Sutherland) ---

fn bit_code(point: GeoPoint, bounds: GeoBounds) -> u8 {
    let mut code = 0_u8;
    if point.x < bounds.min.x {
        code |= 1;
    } else if point.x > bounds.max.x {
        code |= 2;
    }
    if point.y < bounds.min.y {
        code |= 4;
    } else if point.y > bounds.max.y {
        code |= 8;
    }
    code
}

fn edge_intersection(
    a: GeoPoint,
    b: GeoPoint,
    out_code: u8,
    bounds: GeoBounds,
) -> Option<GeoPoint> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let (x, y) = if (out_code & 8) != 0 {
        if dy.abs() <= f32::EPSILON {
            return None;
        }
        (a.x + dx * (bounds.max.y - a.y) / dy, bounds.max.y)
    } else if (out_code & 4) != 0 {
        if dy.abs() <= f32::EPSILON {
            return None;
        }
        (a.x + dx * (bounds.min.y - a.y) / dy, bounds.min.y)
    } else if (out_code & 2) != 0 {
        if dx.abs() <= f32::EPSILON {
            return None;
        }
        (bounds.max.x, a.y + dy * (bounds.max.x - a.x) / dx)
    } else {
        if dx.abs() <= f32::EPSILON {
            return None;
        }
        (bounds.min.x, a.y + dy * (bounds.min.x - a.x) / dx)
    };
    Some(GeoPoint { x, y })
}

fn clip_segment(
    mut a: GeoPoint,
    mut b: GeoPoint,
    bounds: GeoBounds,
    use_last_code: bool,
    last_code: &mut u8,
) -> Option<(GeoPoint, GeoPoint)> {
    let mut code_a = if use_last_code {
        *last_code
    } else {
        bit_code(a, bounds)
    };
    let mut code_b = bit_code(b, bounds);
    *last_code = code_b;

    let mut guard = 0_u8;
    loop {
        if (code_a | code_b) == 0 {
            return Some((a, b));
        }
        if (code_a & code_b) != 0 {
            return None;
        }
        if guard > 8 {
            return None;
        }
        guard += 1;

        let code_out = if code_a != 0 { code_a } else { code_b };
        let point = edge_intersection(a, b, code_out, bounds)?;
        let new_code = bit_code(point, bounds);
        if code_out == code_a {
            a = point;
            code_a = new_code;
        } else {
            b = point;
            code_b = new_code;
        }
    }
}

pub fn clip_polyline_parts(
    points: &[(f32, f32)],
    bounds: GeoBounds,
    no_clip: bool,
) -> Vec<Vec<(f32, f32)>> {
    if points.len() < 2 {
        return Vec::new();
    }
    if no_clip {
        return vec![points.to_vec()];
    }

    let mut parts = Vec::<Vec<(f32, f32)>>::new();
    let mut k = 0_usize;
    let mut last_code = 0_u8;
    let len = points.len();

    for j in 0..len - 1 {
        let a = GeoPoint::from_tuple(points[j]);
        let b = GeoPoint::from_tuple(points[j + 1]);
        let Some((s0, s1)) = clip_segment(a, b, bounds, j > 0, &mut last_code) else {
            continue;
        };

        if parts.len() <= k {
            parts.push(Vec::new());
        }
        parts[k].push(s0.to_tuple());

        if s1.to_tuple() != points[j + 1] || j == len - 2 {
            parts[k].push(s1.to_tuple());
            k += 1;
        }
    }

    parts.retain(|part| part.len() >= 2);
    parts
}

pub fn build_polyline_parts(
    points: &[(f32, f32)],
    bounds: GeoBounds,
    no_clip: bool,
    smooth_factor: f32,
) -> Vec<Vec<(f32, f32)>> {
    let mut parts = clip_polyline_parts(points, bounds, no_clip);
    if smooth_factor > f32::EPSILON {
        for part in &mut parts {
            *part = simplify_polyline(part, smooth_factor);
        }
        parts.retain(|part| part.len() >= 2);
    }
    parts
}

// --- Path/polygon helpers ---

pub fn emit_path(path: &mut VectorPath, points: &[(f32, f32)], close: bool) {
    if points.len() < 2 {
        return;
    }
    path.move_to(points[0].0, points[0].1);
    for point in points.iter().skip(1) {
        path.line_to(point.0, point.1);
    }
    if close {
        path.close();
    }
}

pub fn hex_to_premul_rgba(hex: u32, alpha: f32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    [r * alpha, g * alpha, b * alpha, alpha]
}

pub const POLYGON_AREA_EPSILON: f64 = 1e-2;

pub fn polygon_signed_area(ring: &[(f32, f32)]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0_f64;
    for i in 0..ring.len() {
        let j = (i + 1) % ring.len();
        area += ring[i].0 as f64 * ring[j].1 as f64 - ring[j].0 as f64 * ring[i].1 as f64;
    }
    area * 0.5
}

pub fn normalize_polygon_ring(points: &[(f32, f32)]) -> Option<Vec<(f32, f32)>> {
    if points.len() < 3 {
        return None;
    }

    let mut ring = Vec::<(f32, f32)>::with_capacity(points.len());
    for &point in points {
        if ring.last().copied() != Some(point) {
            ring.push(point);
        }
    }

    if ring.len() >= 2 && ring.first().copied() == ring.last().copied() {
        ring.pop();
    }

    if ring.len() < 3 {
        return None;
    }

    let signed_area = polygon_signed_area(&ring);
    if signed_area.abs() <= POLYGON_AREA_EPSILON {
        return None;
    }

    Some(ring)
}

#[derive(Clone, Debug)]
pub struct FillRing {
    pub order: usize,
    pub points: Vec<(f32, f32)>,
    pub signed_area: f64,
}

pub fn classify_polygon_rings(rings: &[FillRing], max_rings: usize) -> Vec<Vec<Vec<(f32, f32)>>> {
    if rings.is_empty() {
        return Vec::new();
    }

    let mut selected = rings
        .iter()
        .filter(|ring| ring.signed_area.abs() > POLYGON_AREA_EPSILON)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Vec::new();
    }

    if max_rings > 0 && selected.len() > max_rings {
        selected.sort_unstable_by(|a, b| b.signed_area.abs().total_cmp(&a.signed_area.abs()));
        selected.truncate(max_rings);
        selected.sort_unstable_by_key(|ring| ring.order);
    }

    let mut polygons = Vec::<Vec<Vec<(f32, f32)>>>::new();
    let mut current = Vec::<Vec<(f32, f32)>>::new();

    for ring in selected {
        // MVT winding rule (absolute, not first-ring-relative): in y-down tile
        // coordinates an exterior ring has positive shoelace area, a hole
        // negative. A leading hole (clipped-away exterior or arbitrary-winding
        // source) starts its own polygon rather than being dropped.
        let is_exterior = ring.signed_area > 0.0;
        if is_exterior || current.is_empty() {
            if !current.is_empty() {
                polygons.push(current);
                current = Vec::new();
            }
            current.push(ring.points.clone());
        } else {
            current.push(ring.points.clone());
        }
    }

    if !current.is_empty() {
        polygons.push(current);
    }

    polygons
}

// --- Screen-space polyline helpers ---

pub fn build_screen_polyline_into(
    path_points: &[(f32, f32)],
    scale: f32,
    offset: Vec2d,
    rot: (f64, f64),
    tilt_cos: f64,
    pivot: Vec2d,
    out: &mut Vec<Vec2d>,
) {
    let (cos, sin) = rot;
    let transformed = sin != 0.0 || cos != 1.0 || tilt_cos != 1.0;
    for &(x, y) in path_points {
        let mut p = dvec2(
            x as f64 * scale as f64 + offset.x,
            y as f64 * scale as f64 + offset.y,
        );
        if transformed {
            let rel = p - pivot;
            let rotated = dvec2(rel.x * cos - rel.y * sin, rel.x * sin + rel.y * cos);
            p = pivot + dvec2(rotated.x, rotated.y * tilt_cos);
        }
        out.push(p);
    }
}

pub fn polyline_outside_rect(points: &[Vec2d], rect: Rect, margin: f64) -> bool {
    if points.is_empty() {
        return true;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in points {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    max_x < rect.pos.x - margin
        || max_y < rect.pos.y - margin
        || min_x > rect.pos.x + rect.size.x + margin
        || min_y > rect.pos.y + rect.size.y + margin
}

pub fn polyline_cumulative_lengths_into(points: &[Vec2d], out: &mut Vec<f64>) {
    let mut sum = 0.0_f64;
    out.push(sum);
    for pair in points.windows(2) {
        let dx = pair[1].x - pair[0].x;
        let dy = pair[1].y - pair[0].y;
        sum += (dx * dx + dy * dy).sqrt();
        out.push(sum);
    }
}

pub fn sample_polyline_point_at_distance(
    points: &[Vec2d],
    cumulative: &[f64],
    distance: f64,
) -> Option<Vec2d> {
    if points.len() < 2 || cumulative.len() != points.len() {
        return None;
    }

    let total = *cumulative.last()?;
    let clamped = distance.clamp(0.0, total);
    for i in 0..points.len() - 1 {
        let start = cumulative[i];
        let end = cumulative[i + 1];
        if clamped > end && i + 2 < points.len() {
            continue;
        }
        let seg_len = (end - start).max(1e-6);
        let t = ((clamped - start) / seg_len).clamp(0.0, 1.0);
        let a = points[i];
        let b = points[i + 1];
        return Some(dvec2(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
    }
    None
}

pub fn sample_polyline_tangent_angle_raw(
    points: &[Vec2d],
    cumulative: &[f64],
    distance: f64,
    delta: f64,
) -> Option<f32> {
    let total = *cumulative.last()?;
    if total <= 1e-6 {
        return None;
    }
    let d0 = (distance - delta).max(0.0);
    let d1 = (distance + delta).min(total);
    let p0 = sample_polyline_point_at_distance(points, cumulative, d0)?;
    let p1 = sample_polyline_point_at_distance(points, cumulative, d1)?;
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
        return None;
    }
    Some(dy.atan2(dx) as f32)
}

pub fn polyline_length_f32(points: &[(f32, f32)]) -> f32 {
    if points.len() < 2 {
        return 0.0;
    }
    let mut length = 0.0_f32;
    for pair in points.windows(2) {
        let dx = pair[1].0 - pair[0].0;
        let dy = pair[1].1 - pair[0].1;
        length += (dx * dx + dy * dy).sqrt();
    }
    length
}

pub fn simplify_label_path(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() <= 256 {
        return points.to_vec();
    }
    let step = (points.len() / 256).max(1);
    let mut out = Vec::with_capacity(258);
    for (index, point) in points.iter().enumerate() {
        if index == 0 || index + 1 == points.len() || index % step == 0 {
            out.push(*point);
        }
    }
    out
}

pub fn point_outside_rect(point: Vec2d, rect: Rect, margin: f64) -> bool {
    point.x < rect.pos.x - margin
        || point.y < rect.pos.y - margin
        || point.x > rect.pos.x + rect.size.x + margin
        || point.y > rect.pos.y + rect.size.y + margin
}

pub fn rects_overlap_with_padding(a: Rect, b: Rect, padding: f64) -> bool {
    let ax0 = a.pos.x - padding;
    let ay0 = a.pos.y - padding;
    let ax1 = a.pos.x + a.size.x + padding;
    let ay1 = a.pos.y + a.size.y + padding;
    let bx0 = b.pos.x - padding;
    let by0 = b.pos.y - padding;
    let bx1 = b.pos.x + b.size.x + padding;
    let by1 = b.pos.y + b.size.y + padding;
    ax0 < bx1 && ax1 > bx0 && ay0 < by1 && ay1 > by0
}

pub fn rect_outside_rect(a: Rect, b: Rect, margin: f64) -> bool {
    a.pos.x + a.size.x < b.pos.x - margin
        || a.pos.y + a.size.y < b.pos.y - margin
        || a.pos.x > b.pos.x + b.size.x + margin
        || a.pos.y > b.pos.y + b.size.y + margin
}

// --- Stroke tessellation helpers ---

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct StrokeEndpointKey {
    x: i32,
    y: i32,
}

fn stroke_endpoint_key(point: (f32, f32)) -> StrokeEndpointKey {
    const SCALE: f32 = 16.0;
    StrokeEndpointKey {
        x: (point.0 * SCALE).round() as i32,
        y: (point.1 * SCALE).round() as i32,
    }
}

pub fn merge_stroke_polylines(polylines: &[Vec<(f32, f32)>]) -> Vec<Vec<(f32, f32)>> {
    if polylines.is_empty() {
        return Vec::new();
    }

    // Forked ways (rail switches, dual-carriageway splits) duplicate their
    // shared segments exactly; drawing a segment twice at the same depth
    // rank z-fight-shimmers in tilt mode. Keep the FIRST occurrence of
    // every quantized segment, splitting a polyline where a duplicate is
    // dropped.
    let mut seen_segments =
        std::collections::HashSet::<(StrokeEndpointKey, StrokeEndpointKey)>::new();
    let mut lines = Vec::<Vec<(f32, f32)>>::new();
    for line in polylines.iter().filter(|line| line.len() >= 2) {
        let mut current = Vec::<(f32, f32)>::new();
        for pair in line.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (ka, kb) = (stroke_endpoint_key(a), stroke_endpoint_key(b));
            // Sub-quantum micro segments never dedup (they'd collide
            // across the whole tile).
            let keep = if ka == kb {
                true
            } else {
                let seg = if (ka.x, ka.y) <= (kb.x, kb.y) {
                    (ka, kb)
                } else {
                    (kb, ka)
                };
                seen_segments.insert(seg)
            };
            if keep {
                if current.is_empty() {
                    current.push(a);
                }
                current.push(b);
            } else if current.len() >= 2 {
                lines.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        if current.len() >= 2 {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        return Vec::new();
    }

    let mut endpoint_index = HashMap::<StrokeEndpointKey, Vec<(usize, bool)>>::new();
    for (line_index, line) in lines.iter().enumerate() {
        let start_key = stroke_endpoint_key(line[0]);
        let end_key = stroke_endpoint_key(line[line.len() - 1]);
        endpoint_index
            .entry(start_key)
            .or_default()
            .push((line_index, true));
        endpoint_index
            .entry(end_key)
            .or_default()
            .push((line_index, false));
    }

    #[allow(unused_assignments)]
    fn extend_chain_forward(
        chain: &mut Vec<(f32, f32)>,
        lines: &[Vec<(f32, f32)>],
        endpoint_index: &HashMap<StrokeEndpointKey, Vec<(usize, bool)>>,
        used: &mut [bool],
        mut current_line: usize,
        mut at_start: bool,
    ) {
        loop {
            let Some(&end_point) = chain.last() else {
                return;
            };
            let key = stroke_endpoint_key(end_point);
            let Some(connections) = endpoint_index.get(&key) else {
                return;
            };
            if connections.len() != 2 {
                return;
            }

            let mut next: Option<(usize, bool)> = None;
            for &(line_index, line_at_start) in connections {
                if line_index == current_line {
                    continue;
                }
                if used[line_index] {
                    continue;
                }
                if next.is_some() {
                    return;
                }
                next = Some((line_index, line_at_start));
            }
            let Some((next_line, next_starts_here)) = next else {
                return;
            };

            let oriented = if next_starts_here {
                lines[next_line].clone()
            } else {
                let mut reversed = lines[next_line].clone();
                reversed.reverse();
                reversed
            };
            if oriented.len() < 2 {
                used[next_line] = true;
                current_line = next_line;
                at_start = !next_starts_here;
                continue;
            }

            let skip = usize::from(chain.last().copied() == oriented.first().copied());
            chain.extend_from_slice(&oriented[skip..]);

            used[next_line] = true;
            current_line = next_line;
            at_start = !next_starts_here;

            if at_start && chain.len() > 2 && chain.first().copied() == chain.last().copied() {
                return;
            }
        }
    }

    fn emit_chain_if_needed(
        line_index: usize,
        lines: &[Vec<(f32, f32)>],
        endpoint_index: &HashMap<StrokeEndpointKey, Vec<(usize, bool)>>,
        used: &mut [bool],
        merged: &mut Vec<Vec<(f32, f32)>>,
    ) {
        if used[line_index] {
            return;
        }
        let mut chain = lines[line_index].clone();
        if chain.len() < 2 {
            used[line_index] = true;
            return;
        }
        used[line_index] = true;

        extend_chain_forward(&mut chain, lines, endpoint_index, used, line_index, false);
        chain.reverse();
        extend_chain_forward(&mut chain, lines, endpoint_index, used, line_index, true);
        chain.reverse();

        if chain.len() >= 2 {
            merged.push(chain);
        }
    }

    let mut used = vec![false; lines.len()];
    let mut merged = Vec::<Vec<(f32, f32)>>::new();

    for line_index in 0..lines.len() {
        if used[line_index] {
            continue;
        }
        let line = &lines[line_index];
        let start_degree = endpoint_index
            .get(&stroke_endpoint_key(line[0]))
            .map_or(0, Vec::len);
        let end_degree = endpoint_index
            .get(&stroke_endpoint_key(line[line.len() - 1]))
            .map_or(0, Vec::len);
        if start_degree != 2 || end_degree != 2 {
            emit_chain_if_needed(line_index, &lines, &endpoint_index, &mut used, &mut merged);
        }
    }

    for line_index in 0..lines.len() {
        emit_chain_if_needed(line_index, &lines, &endpoint_index, &mut used, &mut merged);
    }

    merged
}

/// A bridge way in tile-local coords: base-map strokes riding within
/// `half_width` of it (and roughly parallel) lift onto its deck. Exists
/// because upstream shortbread generalization drops bridge=yes from short
/// urban viaducts. Two sources: the runtime heuristic from the all-tag
/// detail archive (constant deck), and the offline bridge-bake overlay
/// (solved + AHN-measured per-point deck profile).
#[derive(Clone, Debug)]
pub struct BridgeCorridor {
    pub points: Vec<(f32, f32)>,
    /// Deck height in meters per corridor point (same length as `points`).
    pub decks: Vec<f32>,
    pub half_width: f32,
    /// Baked bridge-dz corridors carry every elevated way individually, so
    /// strokes and arrows only need to match their OWN centerline (tight
    /// reach — anything wider cross-lifts parallel carriageways by partial
    /// feather fractions). Heuristic corridors match generalized shortbread
    /// geometry a few meters off and need the wide reach.
    pub solved: bool,
}

/// Centerline reach for solved profiles (tile units, 256/tile ≈ 6 m/unit
/// at z14). base_dz profiles come from the SAME geometry the strokes draw,
/// so the only offsets are chaikin smoothing and clipping (≤ ~0.5 unit):
/// full deck within ~3.5 m, gone by ~6 m — parallel neighbors never catch.
const SOLVED_REACH_FULL: f32 = 0.6;
const SOLVED_REACH_ZERO: f32 = 1.0;

/// Corridor edge feather in tile units (tiles are 256 units across, so this
/// is ~9 m at z14): a hard half_width cutoff makes vertical walls between
/// adjacent vertices at the corridor boundary; much larger and unrelated
/// parallel geometry starts tenting up.
pub const CORRIDOR_FEATHER: f32 = 1.5;

/// Deck height at a tile-local point with a direction gate (~35°): for
/// geometry with a solid travel direction (oneway arrows), corridors
/// crossing it are the ways it passes under and must not lift it.
pub fn corridor_deck_at_point_dir(
    px: f32,
    py: f32,
    dir: (f32, f32),
    corridors: &[BridgeCorridor],
) -> f32 {
    let mut deck = 0.0f32;
    let (dx, dy) = dir;
    let dl = (dx * dx + dy * dy).sqrt();
    for corridor in corridors {
        for (index, w) in corridor.points.windows(2).enumerate() {
            let (ax, ay) = w[0];
            let (bx, by) = w[1];
            let (ex, ey) = (bx - ax, by - ay);
            let el2 = (ex * ex + ey * ey).max(1e-6);
            if dl > 1e-6 && ((dx * ex + dy * ey) / (dl * el2.sqrt())).abs() < 0.82 {
                continue;
            }
            let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
            let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
            let dist = (qx * qx + qy * qy).sqrt();
            // Arrows sit on their road's centerline: tight reach so a
            // parallel elevated neighbor never hoists them.
            let fade = ((SOLVED_REACH_ZERO - dist)
                / (SOLVED_REACH_ZERO - SOLVED_REACH_FULL))
                .clamp(0.0, 1.0);
            if fade <= 0.0 {
                continue;
            }
            let deck_at =
                corridor.decks[index] * (1.0 - t) + corridor.decks[index + 1] * t;
            let d = deck_at * fade;
            if d > deck {
                deck = d;
            }
        }
    }
    deck
}

/// Deck height at an arbitrary tile-local point (fills).
/// Deliberately NO direction gate: per-vertex directions are ambiguous at
/// polygon corners and butt-ends and a flipping gate tears triangular
/// teeth into deck edges. Instead the reach stays inside the corridor core
/// (half_width + a small feather), so an under-passing road only bulges
/// directly beneath the deck that visually covers it.
pub fn corridor_deck_at_point(px: f32, py: f32, corridors: &[BridgeCorridor]) -> f32 {
    let mut deck = 0.0f32;
    for corridor in corridors {
        for (index, w) in corridor.points.windows(2).enumerate() {
            let (ax, ay) = w[0];
            let (bx, by) = w[1];
            let (ex, ey) = (bx - ax, by - ay);
            let el2 = (ex * ex + ey * ey).max(1e-6);
            let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
            let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
            let dist = (qx * qx + qy * qy).sqrt();
            let fade = ((corridor.half_width + 0.5 - dist) / CORRIDOR_FEATHER).clamp(0.0, 1.0);
            if fade <= 0.0 {
                continue;
            }
            let deck_at =
                corridor.decks[index] * (1.0 - t) + corridor.decks[index + 1] * t;
            let d = deck_at * fade;
            if d > deck {
                deck = d;
            }
        }
    }
    deck
}

/// Per-anchor deck heights for a stroke run: the attribute path (deck on
/// the style) wins; otherwise corridor matching by distance + direction.
fn corridor_deck_overrides(
    verts: &[VVertex],
    anchors: &[[f32; 2]],
    corridors: &[BridgeCorridor],
) -> Option<Vec<f32>> {
    if corridors.is_empty() || anchors.len() < 2 || verts.len() != anchors.len() {
        return None;
    }
    let mut any = false;
    let out: Vec<f32> = verts
        .iter()
        .zip(anchors)
        .map(|(v, a)| {
            // Stroke direction per vertex: the width offset (vertex minus
            // its centerline anchor) is perpendicular to the line. Neighbor
            // anchors are useless — the stream is left/right pairs. The
            // degenerate threshold must sit below the thinnest half-width
            // in 256-unit tile space or thin strokes (rails) skip the gate
            // and tent up wherever a corridor crosses them.
            let (ox, oy) = (v.x - a[0], v.y - a[1]);
            let ol = (ox * ox + oy * oy).sqrt();
            let (dx, dy, dl) = if ol > 0.02 {
                (-oy, ox, ol)
            } else {
                (0.0, 0.0, 0.0)
            };
            let mut deck = 0.0f32;
            for c in corridors {
                for (ci, w) in c.points.windows(2).enumerate() {
                    let (ax, ay) = w[0];
                    let (bx, by) = w[1];
                    let (ex, ey) = (bx - ax, by - ay);
                    let el = (ex * ex + ey * ey).sqrt().max(1e-6);
                    // Direction gate (~35 deg): ways passing UNDER the
                    // bridge cross the corridor and must stay grounded.
                    // Offset-degenerate vertices (caps) skip the gate.
                    if dl > 0.0 && ((dx * ex + dy * ey) / (dl * el)).abs() < 0.82 {
                        continue;
                    }
                    let t = (((a[0] - ax) * ex + (a[1] - ay) * ey) / (el * el)).clamp(0.0, 1.0);
                    let (px, py) = (ax + ex * t - a[0], ay + ey * t - a[1]);
                    let dist = (px * px + py * py).sqrt();
                    let fade = if c.solved {
                        ((SOLVED_REACH_ZERO - dist) / (SOLVED_REACH_ZERO - SOLVED_REACH_FULL))
                            .clamp(0.0, 1.0)
                    } else {
                        ((c.half_width + CORRIDOR_FEATHER - dist) / CORRIDOR_FEATHER)
                            .clamp(0.0, 1.0)
                    };
                    let deck_at = c.decks[ci] * (1.0 - t) + c.decks[ci + 1] * t;
                    let d = deck_at * fade;
                    if d > deck {
                        deck = d;
                        any = true;
                    }
                }
            }
            deck
        })
        .collect();
    any.then_some(out)
}

/// One road polyline feeding a tier union: raw way points (collector
/// space) with optional per-point deck heights from the base_dz join.
pub struct RoadRibbon<'a> {
    pub points: &'a [(f32, f32)],
    pub dz: Option<&'a [f32]>,
    /// Already a closed outline (street_polygons plaza joining its color
    /// tier): used verbatim as a union contour instead of being offset.
    pub closed_ring: bool,
    /// Round cap discs at the way ends. A surface way ending at a tunnel
    /// portal gets a BUTT end instead — the road stops flat where its
    /// continuation dives underground.
    pub start_disc: bool,
    pub end_disc: bool,
}

/// The road geometry generator: every ribbon of a paint tier becomes
/// per-segment rectangle contours (square caps so joins are covered by
/// overlap) in ONE path, and the sweep tessellator's non-zero winding rule
/// merges them into a single seamless surface mesh — junctions, gores and
/// parallel carriageways unify by construction instead of by draw order,
/// so flat and tilted views are geometrically identical. Deck heights ride
/// per contour vertex (projected through clipping) into params[4].


/// Insert a vertex at every transversal crossing between (and within) the
/// rings, lerping the per-vertex deck heights. The downstream sweep
/// tessellator handles crossings only when they coincide with vertices.
fn precross_rings(rings: &mut Vec<(Vec<(f32, f32)>, Vec<f32>)>) {
    const CELL: f32 = 8.0;
    // Everything on a 1/64-unit grid: canonical intersection points can
    // only be shared if the endpoints they lerp from are themselves exact.
    for (points, heights) in rings.iter_mut() {
        let mut write = 0usize;
        for read in 0..points.len() {
            let snapped = (
                (points[read].0 * 64.0).round() / 64.0,
                (points[read].1 * 64.0).round() / 64.0,
            );
            if write > 0 && points[write - 1] == snapped {
                continue;
            }
            points[write] = snapped;
            heights[write] = heights[read];
            write += 1;
        }
        points.truncate(write);
        heights.truncate(write);
    }
    rings.retain(|(points, _)| points.len() >= 3);
    // Global segment table: (ring, seg index) with bbox grid.
    struct Seg {
        ring: usize,
        index: usize,
        a: (f32, f32),
        b: (f32, f32),
    }
    let mut segs: Vec<Seg> = Vec::new();
    let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> =
        std::collections::HashMap::new();
    for (ring_index, (points, _)) in rings.iter().enumerate() {
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            let seg_id = segs.len();
            segs.push(Seg { ring: ring_index, index, a, b });
            let (min_x, max_x) = (a.0.min(b.0), a.0.max(b.0));
            let (min_y, max_y) = (a.1.min(b.1), a.1.max(b.1));
            let mut cy = (min_y / CELL).floor() as i32;
            while cy <= (max_y / CELL).floor() as i32 {
                let mut cx = (min_x / CELL).floor() as i32;
                while cx <= (max_x / CELL).floor() as i32 {
                    grid.entry((cx, cy)).or_default().push(seg_id);
                    cx += 1;
                }
                cy += 1;
            }
        }
    }
    // Collect splits per segment. THE CANONICAL-POINT INVARIANT: one
    // crossing = ONE point, bit-identical in both rings — the sweep merges
    // events by exact equality, and a pair of lerp results that differ by
    // float noise leaves the crossing unresolved (wedges / dropouts).
    let mut splits: Vec<Vec<(f32, (f32, f32))>> = vec![Vec::new(); segs.len()];
    let snap = |v: f32| (v * 64.0).round() / 64.0;
    for bucket in grid.values() {
        for i in 0..bucket.len() {
            for j in i + 1..bucket.len() {
                let (s1, s2) = (&segs[bucket[i]], &segs[bucket[j]]);
                if s1.ring == s2.ring
                    && ((s1.index as i32 - s2.index as i32).abs() <= 1
                        || (s1.index == 0 || s2.index == 0)
                            && s1.index.max(s2.index)
                                == rings[s1.ring].0.len().saturating_sub(1))
                {
                    continue;
                }
                let d1 = (s1.b.0 - s1.a.0, s1.b.1 - s1.a.1);
                let d2 = (s2.b.0 - s2.a.0, s2.b.1 - s2.a.1);
                let denom = d1.0 * d2.1 - d1.1 * d2.0;
                if denom.abs() < 1e-9 {
                    continue;
                }
                let dx = s2.a.0 - s1.a.0;
                let dy = s2.a.1 - s1.a.1;
                let t = (dx * d2.1 - dy * d2.0) / denom;
                let u = (dx * d1.1 - dy * d1.0) / denom;
                if !(-0.0001..=1.0001).contains(&t) || !(-0.0001..=1.0001).contains(&u) {
                    continue;
                }
                // One snapped point, shared verbatim by both segments.
                let point = (
                    snap(s1.a.0 + d1.0 * t.clamp(0.0, 1.0)),
                    snap(s1.a.1 + d1.1 * t.clamp(0.0, 1.0)),
                );
                splits[bucket[i]].push((t.clamp(0.0, 1.0), point));
                splits[bucket[j]].push((u.clamp(0.0, 1.0), point));
            }
        }
    }
    // Rebuild rings with inserted vertices (per ring, per segment, sorted t).
    let mut per_ring: Vec<Vec<(usize, f32, (f32, f32))>> = vec![Vec::new(); rings.len()];
    for (seg_id, seg) in segs.iter().enumerate() {
        for &(t, point) in &splits[seg_id] {
            per_ring[seg.ring].push((seg.index, t, point));
        }
    }
    for (ring_index, inserts) in per_ring.iter_mut().enumerate() {
        if inserts.is_empty() {
            continue;
        }
        inserts.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap()));
        let (points, heights) = &rings[ring_index];
        let count = points.len();
        let mut new_points = Vec::with_capacity(count + inserts.len());
        let mut new_heights = Vec::with_capacity(count + inserts.len());
        let mut cursor = 0usize;
        for index in 0..count {
            new_points.push(points[index]);
            new_heights.push(heights[index]);
            let a = points[index];
            let b = points[(index + 1) % count];
            let ha = heights[index];
            let hb = heights[(index + 1) % count];
            while cursor < inserts.len() && inserts[cursor].0 == index {
                let (t, point) = (inserts[cursor].1, inserts[cursor].2);
                cursor += 1;
                // Skip points that snapped onto an existing vertex.
                if new_points
                    .last()
                    .is_some_and(|&last: &(f32, f32)| last == point)
                    || point == (b.0, b.1)
                {
                    continue;
                }
                new_points.push(point);
                new_heights.push(ha + (hb - ha) * t);
            }
        }
        rings[ring_index] = (new_points, new_heights);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_ribbon_contour(
    center_in: &[(f32, f32)],
    center_dz_in: &[f32],
    half_width: f32,
    clip: GeoBounds,
    rings_out: &mut Vec<(Vec<(f32, f32)>, Vec<f32>)>,
    ring: &mut Vec<(f32, f32)>,
    ring_dz: &mut Vec<f32>,
) {
    if center_in.len() < 2 {
        return;
    }
    let mut center = center_in.to_vec();
    let center_dz = center_dz_in;
    {
        let (ax, ay) = center[0];
        let (bx, by) = center[1];
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt().max(1e-3);
        center[0] = (
            ax - (bx - ax) / len * half_width,
            ay - (by - ay) / len * half_width,
        );
        let last = center.len() - 1;
        let (ax, ay) = center[last - 1];
        let (bx, by) = center[last];
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt().max(1e-3);
        center[last] = (
            bx + (bx - ax) / len * half_width,
            by + (by - ay) / len * half_width,
        );
    }
    let count = center.len();
    ring.clear();
    ring_dz.clear();
    for side in 0..2 {
        for step in 0..count {
            let index = if side == 0 { step } else { count - 1 - step };
            let previous = center[index.saturating_sub(1)];
            let next = center[(index + 1).min(count - 1)];
            let (ex, ey) = (next.0 - previous.0, next.1 - previous.1);
            let len = (ex * ex + ey * ey).sqrt().max(1e-3);
            let (mut nx, mut ny) = (-ey / len, ex / len);
            let (p0x, p0y) = center[index];
            let (ax, ay) = (p0x - previous.0, p0y - previous.1);
            let (bx, by) = (next.0 - p0x, next.1 - p0y);
            let al = (ax * ax + ay * ay).sqrt().max(1e-3);
            let bl = (bx * bx + by * by).sqrt().max(1e-3);
            let cos_half =
                (((ax / al + bx / bl).powi(2) + (ay / al + by / bl).powi(2)).sqrt() * 0.5)
                    .max(0.5);
            let scale = (1.0 / cos_half).min(2.0);
            nx *= half_width * scale;
            ny *= half_width * scale;
            let point = if side == 0 {
                (p0x + nx, p0y + ny)
            } else {
                (p0x - nx, p0y - ny)
            };
            ring.push(point);
            ring_dz.push(center_dz[index]);
        }
    }
    if ring.len() < 3 {
        return;
    }
    let mut area = 0.0f32;
    for i in 0..ring.len() {
        let j = (i + 1) % ring.len();
        area += ring[i].0 * ring[j].1 - ring[j].0 * ring[i].1;
    }
    if area < 0.0 {
        ring.reverse();
        ring_dz.reverse();
    }
    let clipped = clip_ring_to_rect(ring, clip);
    if clipped.len() < 3 {
        return;
    }
    let clip_changed = clipped.len() != ring.len();
    let mut out_dz: Vec<f32> = Vec::with_capacity(clipped.len());
    if clip_changed {
        for &(px, py) in &clipped {
            let mut best = f32::MAX;
            let mut best_dz = 0.0f32;
            for i in 0..ring.len() {
                let j = (i + 1) % ring.len();
                let (ax, ay) = ring[i];
                let (bx, by) = ring[j];
                let (ex, ey) = (bx - ax, by - ay);
                let el2 = (ex * ex + ey * ey).max(1e-6);
                let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
                let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
                let d2 = qx * qx + qy * qy;
                if d2 < best {
                    best = d2;
                    best_dz = ring_dz[i] * (1.0 - t) + ring_dz[j] * t;
                }
            }
            out_dz.push(best_dz);
        }
    } else {
        out_dz.extend_from_slice(ring_dz);
    }
    rings_out.push((clipped, out_dz));
}

/// Ribbon outline rings for a set of ways (plus verbatim closed rings),
/// with per-vertex deck heights. Crossings between rings are left as-is —
/// the boolean overlay downstream resolves them robustly.
pub fn road_ribbon_rings(
    ribbons: &[RoadRibbon],
    half_width: f32,
    aa: f32,
    clip: GeoBounds,
) -> Vec<(Vec<(f32, f32)>, Vec<f32>)> {
    // Geometry-first: hard face edge at half_width, the legacy 50%-alpha
    // point. (AA fringe returns as a later pass; `aa` reserved for it.)
    let _ = aa;
    let half_width = half_width.max(0.05);
    let mut rings_out: Vec<(Vec<(f32, f32)>, Vec<f32>)> = Vec::new();
    let mut ring: Vec<(f32, f32)> = Vec::new();
    let mut ring_dz: Vec<f32> = Vec::new();
    for ribbon in ribbons {
        if ribbon.closed_ring {
            ring.clear();
            ring_dz.clear();
            for (index, &point) in ribbon.points.iter().enumerate() {
                if ring.last().is_some_and(|&(lx, ly): &(f32, f32)| {
                    (lx - point.0).abs() < 1e-3 && (ly - point.1).abs() < 1e-3
                }) {
                    continue;
                }
                ring.push(point);
                ring_dz.push(ribbon.dz.map_or(0.0, |dz| dz[index]));
            }
            if ring.len() >= 2 && ring.first() == ring.last() {
                ring.pop();
                ring_dz.pop();
            }
            if ring.len() < 3 {
                continue;
            }
            let clipped = clip_ring_to_rect(&ring, clip);
            if clipped.len() < 3 {
                continue;
            }
            if clipped.len() == ring.len() {
                rings_out.push((clipped, ring_dz.clone()));
            } else {
                let mut out_dz = Vec::with_capacity(clipped.len());
                for &(px, py) in &clipped {
                    let mut best = f32::MAX;
                    let mut best_dz = 0.0f32;
                    for i in 0..ring.len() {
                        let j = (i + 1) % ring.len();
                        let (ax, ay) = ring[i];
                        let (bx, by) = ring[j];
                        let (ex, ey) = (bx - ax, by - ay);
                        let el2 = (ex * ex + ey * ey).max(1e-6);
                        let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
                        let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
                        let d2 = qx * qx + qy * qy;
                        if d2 < best {
                            best = d2;
                            best_dz = ring_dz[i] * (1.0 - t) + ring_dz[j] * t;
                        }
                    }
                    out_dz.push(best_dz);
                }
                rings_out.push((clipped, out_dz));
            }
            continue;
        }
        let mut center: Vec<(f32, f32)> = Vec::with_capacity(ribbon.points.len() + 2);
        let mut center_dz: Vec<f32> = Vec::with_capacity(ribbon.points.len() + 2);
        for (index, &point) in ribbon.points.iter().enumerate() {
            if center.last().is_some_and(|&(lx, ly): &(f32, f32)| {
                (lx - point.0).abs() < 1e-3 && (ly - point.1).abs() < 1e-3
            }) {
                continue;
            }
            center.push(point);
            center_dz.push(ribbon.dz.map_or(0.0, |dz| dz[index]));
        }
        if center.len() < 2 {
            continue;
        }
        // Segment rectangles + vertex discs: the union absorbs the overlap
        // into round joins and round dead-end caps (the legacy look), with
        // no miter mathematics to go wrong at sharp bends.
        for seg in 0..center.len() - 1 {
            let (ax, ay) = center[seg];
            let (bx, by) = center[seg + 1];
            let (ex, ey) = (bx - ax, by - ay);
            let len = (ex * ex + ey * ey).sqrt();
            if len < 1e-4 {
                continue;
            }
            let (nx, ny) = (-ey / len * half_width, ex / len * half_width);
            ring.clear();
            ring_dz.clear();
            ring.extend_from_slice(&[
                (ax + nx, ay + ny),
                (ax - nx, ay - ny),
                (bx - nx, by - ny),
                (bx + nx, by + ny),
            ]);
            ring_dz.extend_from_slice(&[
                center_dz[seg],
                center_dz[seg],
                center_dz[seg + 1],
                center_dz[seg + 1],
            ]);
            push_clipped_ring(&ring, &ring_dz, clip, &mut rings_out);
        }
        for (index, &(px, py)) in center.iter().enumerate() {
            // Interior joins get the cheapest cover that closes the wedge
            // between consecutive rects: nothing when collinear, two bevel
            // triangles for gentle bends, a full disc (round join) only at
            // real corners. Ends get the disc (round cap) unless flagged
            // butt (tunnel portals).
            if index == 0 && !ribbon.start_disc {
                continue;
            }
            if index + 1 == center.len() && !ribbon.end_disc {
                continue;
            }
            if index > 0 && index + 1 < center.len() {
                let (ax, ay) = center[index - 1];
                let (bx, by) = center[index + 1];
                let (ux, uy) = (px - ax, py - ay);
                let (vx, vy) = (bx - px, by - py);
                let ul = (ux * ux + uy * uy).sqrt().max(1e-6);
                let vl = (vx * vx + vy * vy).sqrt().max(1e-6);
                let cos_bend = (ux * vx + uy * vy) / (ul * vl);
                if cos_bend > 0.999 {
                    continue;
                }
                if cos_bend > 0.94 {
                    // Left normals of the incoming and outgoing segments.
                    let (n1x, n1y) = (-uy / ul * half_width, ux / ul * half_width);
                    let (n2x, n2y) = (-vy / vl * half_width, vx / vl * half_width);
                    for side in [1.0f32, -1.0] {
                        let (mut ax1, mut ay1) = (n1x * side, n1y * side);
                        let (mut ax2, mut ay2) = (n2x * side, n2y * side);
                        // Positive-area orientation, matching the rects.
                        if ax1 * ay2 - ay1 * ax2 < 0.0 {
                            std::mem::swap(&mut ax1, &mut ax2);
                            std::mem::swap(&mut ay1, &mut ay2);
                        }
                        ring.clear();
                        ring_dz.clear();
                        ring.extend_from_slice(&[
                            (px, py),
                            (px + ax1, py + ay1),
                            (px + ax2, py + ay2),
                        ]);
                        ring_dz.extend_from_slice(&[center_dz[index]; 3]);
                        push_clipped_ring(&ring, &ring_dz, clip, &mut rings_out);
                    }
                    continue;
                }
            }
            ring.clear();
            ring_dz.clear();
            const DISC_SEGMENTS: usize = 12;
            for step in 0..DISC_SEGMENTS {
                let angle =
                    step as f32 / DISC_SEGMENTS as f32 * std::f32::consts::TAU;
                ring.push((
                    px + angle.cos() * half_width,
                    py + angle.sin() * half_width,
                ));
                ring_dz.push(center_dz[index]);
            }
            push_clipped_ring(&ring, &ring_dz, clip, &mut rings_out);
        }
    }
    rings_out
}

/// Clip a ring and carry dz through (nearest original segment when the
/// clip rewrote vertices), appending to the output set.
fn push_clipped_ring(
    ring: &[(f32, f32)],
    ring_dz: &[f32],
    clip: GeoBounds,
    rings_out: &mut Vec<(Vec<(f32, f32)>, Vec<f32>)>,
) {
    let clipped = clip_ring_to_rect(ring, clip);
    if clipped.len() < 3 {
        return;
    }
    if clipped.len() == ring.len() {
        rings_out.push((clipped, ring_dz.to_vec()));
        return;
    }
    let mut out_dz = Vec::with_capacity(clipped.len());
    for &(px, py) in &clipped {
        let mut best = f32::MAX;
        let mut best_dz = 0.0f32;
        for i in 0..ring.len() {
            let j = (i + 1) % ring.len();
            let (ax, ay) = ring[i];
            let (bx, by) = ring[j];
            let (ex, ey) = (bx - ax, by - ay);
            let el2 = (ex * ex + ey * ey).max(1e-6);
            let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
            let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
            let d2 = qx * qx + qy * qy;
            if d2 < best {
                best = d2;
                best_dz = ring_dz[i] * (1.0 - t) + ring_dz[j] * t;
            }
        }
        out_dz.push(best_dz);
    }
    rings_out.push((clipped, out_dz));
}

#[allow(clippy::too_many_arguments)]
pub fn append_road_union(
    ribbons: &[RoadRibbon],
    half_width: f32,
    clip: GeoBounds,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    out_vertices: &mut Vec<f32>,
    out_indices: &mut Vec<u32>,
    color: [f32; 4],
    param5: f32,
    zbias: &mut f32,
    aa: f32,
    tolerance: f32,
) {
    if half_width <= 0.0 {
        return;
    }
    // The legacy stroke's 50%-alpha edge sits at half_width exactly; a hard
    // outline at half_width plus an outward fringe would read one aa/2
    // wider. Inset the body so body + fringe centers its ramp on the same
    // visual edge.
    let half_width = (half_width - aa * 0.5).max(0.05);
    let mut path = VectorPath::new();
    let mut contour_dz: Vec<f32> = Vec::new();
    let mut contours = 0usize;
    let mut ring: Vec<(f32, f32)> = Vec::new();
    let mut ring_dz: Vec<f32> = Vec::new();
    let mut rings_out: Vec<(Vec<(f32, f32)>, Vec<f32>)> = Vec::new();
    for ribbon in ribbons {
        if ribbon.closed_ring {
            ring.clear();
            ring_dz.clear();
            for (index, &point) in ribbon.points.iter().enumerate() {
                if ring.last().is_some_and(|&(lx, ly): &(f32, f32)| {
                    (lx - point.0).abs() < 1e-3 && (ly - point.1).abs() < 1e-3
                }) {
                    continue;
                }
                ring.push(point);
                ring_dz.push(ribbon.dz.map_or(0.0, |dz| dz[index]));
            }
            // Drop an explicit closing duplicate.
            if ring.len() >= 2 && ring.first() == ring.last() {
                ring.pop();
                ring_dz.pop();
            }
            if ring.len() < 3 {
                continue;
            }
            let mut area = 0.0f32;
            for i in 0..ring.len() {
                let j = (i + 1) % ring.len();
                area += ring[i].0 * ring[j].1 - ring[j].0 * ring[i].1;
            }
            if area < 0.0 {
                ring.reverse();
                ring_dz.reverse();
            }
            let clipped = clip_ring_to_rect(&ring, clip);
            if clipped.len() < 3 {
                continue;
            }
            if clipped.len() == ring.len() {
                rings_out.push((clipped, ring_dz.clone()));
            } else {
                let mut out_dz = Vec::with_capacity(clipped.len());
                for &(px, py) in &clipped {
                    let mut best = f32::MAX;
                    let mut best_dz = 0.0f32;
                    for i in 0..ring.len() {
                        let j = (i + 1) % ring.len();
                        let (ax, ay) = ring[i];
                        let (bx, by) = ring[j];
                        let (ex, ey) = (bx - ax, by - ay);
                        let el2 = (ex * ex + ey * ey).max(1e-6);
                        let t = (((px - ax) * ex + (py - ay) * ey) / el2).clamp(0.0, 1.0);
                        let (qx, qy) = (ax + ex * t - px, ay + ey * t - py);
                        let d2 = qx * qx + qy * qy;
                        if d2 < best {
                            best = d2;
                            best_dz = ring_dz[i] * (1.0 - t) + ring_dz[j] * t;
                        }
                    }
                    out_dz.push(best_dz);
                }
                rings_out.push((clipped, out_dz));
            }
            continue;
        }
        // Dedup + square-cap extend the centerline, carrying dz.
        let mut center: Vec<(f32, f32)> = Vec::with_capacity(ribbon.points.len() + 2);
        let mut center_dz: Vec<f32> = Vec::with_capacity(ribbon.points.len() + 2);
        for (index, &point) in ribbon.points.iter().enumerate() {
            if center.last().is_some_and(|&(lx, ly): &(f32, f32)| {
                (lx - point.0).abs() < 1e-3 && (ly - point.1).abs() < 1e-3
            }) {
                continue;
            }
            center.push(point);
            center_dz.push(ribbon.dz.map_or(0.0, |dz| dz[index]));
        }
        if center.len() < 2 {
            continue;
        }
        // A way doubling back on itself folds the ribbon outline into a
        // bowtie whose negative lobe bites the union — split into runs at
        // sharp reversals and ribbon each run separately.
        let mut split_at: Vec<usize> = Vec::new();
        for index in 1..center.len() - 1 {
            let (ax, ay) = center[index - 1];
            let (bx, by) = center[index];
            let (cx2, cy2) = center[index + 1];
            let (ux, uy) = (bx - ax, by - ay);
            let (vx, vy) = (cx2 - bx, cy2 - by);
            let ul = (ux * ux + uy * uy).sqrt().max(1e-3);
            let vl = (vx * vx + vy * vy).sqrt().max(1e-3);
            if (ux * vx + uy * vy) / (ul * vl) < -0.17 {
                split_at.push(index);
            }
        }
        if !split_at.is_empty() {
            let mut runs: Vec<(Vec<(f32, f32)>, Vec<f32>)> = Vec::new();
            let mut begin = 0usize;
            for &cut in split_at.iter().chain(std::iter::once(&(center.len() - 1))) {
                if cut > begin {
                    runs.push((
                        center[begin..=cut].to_vec(),
                        center_dz[begin..=cut].to_vec(),
                    ));
                }
                begin = cut;
            }
            // Re-enter each run through the same path by queueing them as
            // pseudo-ribbons: emit inline below via recursion-free loop.
            for (run_points, run_dz) in &runs {
                emit_ribbon_contour(
                    run_points,
                    run_dz,
                    half_width,
                    clip,
                    &mut rings_out,
                    &mut ring,
                    &mut ring_dz,
                );
            }
            continue;
        }
        emit_ribbon_contour(
            &center,
            &center_dz,
            half_width,
            clip,
            &mut rings_out,
            &mut ring,
            &mut ring_dz,
        );
    }
    // The sweep tessellator only splits edges at event VERTICES — it never
    // computes transversal crossings, so overlapping ribbon outlines (every
    // junction) misclassify unless every crossing IS a vertex. Pre-cross
    // all rings on the CPU, inserting intersection vertices with lerped dz.
    precross_rings(&mut rings_out);
    for (ring_points, ring_heights) in &rings_out {
        if ring_points.len() < 3 {
            continue;
        }
        path.move_to(ring_points[0].0, ring_points[0].1);
        path.winding(Winding::CCW);
        for point in ring_points.iter().skip(1) {
            path.line_to(point.0, point.1);
        }
        path.close();
        contour_dz.extend_from_slice(ring_heights);
        contours += 1;
    }
    if contours == 0 {
        return;
    }
    // aa = 0 through the tessellator: its fringe would also run along
    // contour edges INTERIOR to the union (both sides filled) and read as
    // seams. The outer-boundary fringe is rebuilt below from unpaired
    // triangle edges instead.
    tessellate_path_fill(
        &mut path,
        tess,
        tess_verts,
        tess_indices,
        LineJoin::Miter,
        4.0,
        0.0,
        false,
        tolerance,
    );
    if tess_verts.is_empty() || tess_indices.is_empty() {
        return;
    }
    let dz_aligned = tess_verts.len() == contour_dz.len();
    if !dz_aligned {
        contour_dz.clear();
        contour_dz.resize(tess_verts.len(), 0.0);
    }
    // Outer-only AA fringe: a boundary edge of the union is a triangle
    // edge that appears exactly once; skirt it with a 1-aa quad fading
    // out, normal oriented away from the owning triangle.
    let mut fringe_verts: Vec<VVertex> = Vec::new();
    let mut fringe_indices: Vec<u32> = Vec::new();
    let mut fringe_dz: Vec<f32> = Vec::new();
    if aa > 0.0 {
        let mut edge_owner: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
        for tri in tess_indices.chunks_exact(3) {
            for (a, b, c) in [
                (tri[0], tri[1], tri[2]),
                (tri[1], tri[2], tri[0]),
                (tri[2], tri[0], tri[1]),
            ] {
                let key = (a.min(b), a.max(b));
                edge_owner
                    .entry(key)
                    .and_modify(|entry| entry.1 += 1)
                    .or_insert((c, 1));
            }
        }
        for (&(a, b), &(c, count)) in &edge_owner {
            if count != 1 {
                continue;
            }
            let pa = tess_verts[a as usize];
            let pb = tess_verts[b as usize];
            let pc = tess_verts[c as usize];
            let (ex, ey) = (pb.x - pa.x, pb.y - pa.y);
            let len = (ex * ex + ey * ey).sqrt().max(1e-4);
            let (mut nx, mut ny) = (-ey / len * aa, ex / len * aa);
            if nx * (pc.x - pa.x) + ny * (pc.y - pa.y) > 0.0 {
                nx = -nx;
                ny = -ny;
            }
            let base = fringe_verts.len() as u32;
            let fringe_vertex = |x: f32, y: f32, u: f32| VVertex {
                x,
                y,
                u,
                v: 1.0,
                stroke_dist: 0.0,
                clip_radius: aa * 2.0,
            };
            // u encodes across-coverage: 0.5 (full) at the body edge, 0 at
            // the outer rim; drawn with stroke_mult 1 so the shader's
            // across*mult model yields a true aa ramp.
            fringe_verts.push(fringe_vertex(pa.x, pa.y, 0.5));
            fringe_verts.push(fringe_vertex(pb.x, pb.y, 0.5));
            fringe_verts.push(fringe_vertex(pa.x + nx, pa.y + ny, 0.0));
            fringe_verts.push(fringe_vertex(pb.x + nx, pb.y + ny, 0.0));
            let dz_a = if dz_aligned { contour_dz[a as usize] } else { 0.0 };
            let dz_b = if dz_aligned { contour_dz[b as usize] } else { 0.0 };
            fringe_dz.extend_from_slice(&[dz_a, dz_b, dz_a, dz_b]);
            fringe_indices
                .extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
    }
    // dz rides only if the tessellator kept our emit order 1:1.
    let deck: Option<&[f32]> =
        (dz_aligned && contour_dz.iter().any(|&v| v > 0.05)).then_some(&contour_dz[..]);
    crate::makepad_draw::vector::append_tessellated_geometry_decked(
        tess_verts,
        tess_indices,
        out_vertices,
        out_indices,
        VectorRenderParams {
            color,
            stroke_mult: 1e6,
            shape_id: 0.0,
            params: [0.0, 0.0, 0.0, 0.0, 0.0, param5],
            zbias: *zbias,
        },
        deck,
    );
    *zbias += VECTOR_ZBIAS_STEP;
    if !fringe_indices.is_empty() {
        let fringe_deck: Option<&[f32]> =
            (dz_aligned && fringe_dz.iter().any(|&v| v > 0.05)).then_some(&fringe_dz[..]);
        crate::makepad_draw::vector::append_tessellated_geometry_decked(
            &fringe_verts,
            &fringe_indices,
            out_vertices,
            out_indices,
            VectorRenderParams {
                color,
                stroke_mult: 1.0,
                shape_id: 0.0,
                params: [0.0, 0.0, 0.0, 0.0, 0.0, param5],
                zbias: *zbias,
            },
            fringe_deck,
        );
        *zbias += VECTOR_ZBIAS_STEP;
    }
}

#[allow(clippy::too_many_arguments)]
/// Micro-depth for strokes OUTSIDE the road paint ladder: tunnels under
/// everything, other strokes above the road surfaces (their 2D late paint).
pub fn stroke_pass_param5(pass: &StrokePassStyle) -> f32 {
    (if pass.deck_m < 0.0 { 0.05 } else { 0.22 }) + pass.depth_micro
}

pub fn append_stroke_pass(
    path: &mut VectorPath,
    points: &[(f32, f32)],
    closed: bool,
    corridors: Option<&[BridgeCorridor]>,
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    stroke_vertices: &mut Vec<f32>,
    stroke_indices: &mut Vec<u32>,
    pass: StrokePassStyle,
    start_cap: LineCap,
    end_cap: LineCap,
    line_join: LineJoin,
    aa: f32,
    tolerance: f32,
    stroke_zbias: &mut f32,
    // Tilt micro-depth slot. Callers place the stroke in the global paint
    // ladder; tunnels (deck sentinel < 0) pass their fixed under-value.
    param5: f32,
) {
    // Baked in GPU re-expandable form: the vertex shader re-derives the
    // stroke width the current view zoom calls for, so stale-bucket tiles
    // keep correct screen widths through the whole zoom gesture.
    thread_local! {
        static STROKE_ANCHORS: std::cell::RefCell<Vec<[f32; 2]>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    // Decked strokes need dense vertices: the deck height interpolates
    // per-vertex, and a straight span with two anchors renders its ramp as
    // one hard facet. Tiles are TILE_SIZE (256) units across, so 3 units is
    // ~18 m at z14 — smooth ramps without exploding vertex counts.
    // deck_m < 0 is the "never deck" sentinel (tunnels): no attribute deck
    // and no corridor matching.
    let deck_possible = pass.deck_m > 0.0
        || (pass.deck_m == 0.0 && corridors.is_some_and(|c| !c.is_empty()));
    let mut dense: Vec<(f32, f32)> = Vec::new();
    let points = if deck_possible && points.len() >= 2 {
        const MAX_SEG: f32 = 3.0;
        dense.push(points[0]);
        for w in points.windows(2) {
            let (ax, ay) = w[0];
            let (bx, by) = w[1];
            let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
            let steps = (len / MAX_SEG).ceil().max(1.0) as usize;
            for k in 1..=steps {
                let t = k as f32 / steps as f32;
                dense.push((ax + (bx - ax) * t, ay + (by - ay) * t));
            }
        }
        dense.as_slice()
    } else {
        points
    };
    emit_path(path, points, closed);
    STROKE_ANCHORS.with(|anchors| {
        let mut anchors = anchors.borrow_mut();
        let stroke_mult = tessellate_path_stroke_ends_anchored(
            path,
            tess,
            tess_verts,
            tess_indices,
            &mut anchors,
            pass.width,
            start_cap,
            end_cap,
            line_join,
            4.0,
            aa,
            tolerance,
        );
        let deck_override = if pass.deck_m > 0.0 {
            None
        } else {
            corridors.and_then(|c| corridor_deck_overrides(tess_verts, &anchors, c))
        };
        append_expanded_stroke_geometry(
            tess_verts,
            &anchors,
            tess_indices,
            stroke_vertices,
            stroke_indices,
            VectorRenderParams {
                color: hex_to_premul_rgba(pass.color, 1.0),
                stroke_mult,
                shape_id: pass.shape_id,
                params: [0.0, 0.0, 0.0, 0.0, 0.0, param5],
                zbias: *stroke_zbias,
            },
            pass.expand_class,
            pass.deck_m,
            deck_override.as_deref(),
        );
    });
    *stroke_zbias += VECTOR_ZBIAS_STEP;
}

// --- Coordinate projection ---

pub fn lon_lat_to_normalized(lon: f64, lat: f64) -> Vec2d {
    let x = (lon + 180.0) / 360.0;
    let clamped_lat = lat.clamp(-85.051_128_78, 85.051_128_78);
    let sin_lat = clamped_lat.to_radians().sin();
    let y = 0.5 - ((1.0 + sin_lat) / (1.0 - sin_lat)).ln() / (4.0 * std::f64::consts::PI);
    dvec2(x, y)
}

pub fn lon_lat_to_world(lon: f64, lat: f64, zoom: u32) -> Vec2d {
    lon_lat_to_normalized(lon, lat) * tile_world_size(zoom)
}

/// Inverse of `lon_lat_to_normalized`: normalized web mercator -> (lon, lat).
pub fn normalized_to_lon_lat(p: Vec2d) -> (f64, f64) {
    let lon = p.x * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * p.y))
        .sinh()
        .atan()
        .to_degrees();
    (lon, lat)
}

pub const TILE_SIZE: f64 = 256.0;

pub fn tile_world_size(zoom: u32) -> f64 {
    tile_world_size_zoom(zoom as f64)
}

pub fn tile_world_size_zoom(zoom: f64) -> f64 {
    TILE_SIZE * 2.0_f64.powf(zoom)
}

pub fn tile_corner_lon_lat_f64(x: f64, y: f64, zoom: u32) -> (f64, f64) {
    let n = 2.0_f64.powi(zoom as i32);
    let lon = x / n * 360.0 - 180.0;
    let lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan();
    (lon, lat_rad.to_degrees())
}

pub fn tile_bounds_padded(tile_key: TileKey, pad_tiles: f64) -> (f64, f64, f64, f64) {
    let (west, north) = tile_corner_lon_lat_f64(
        tile_key.x as f64 - pad_tiles,
        tile_key.y as f64 - pad_tiles,
        tile_key.z,
    );
    let (east, south) = tile_corner_lon_lat_f64(
        tile_key.x as f64 + 1.0 + pad_tiles,
        tile_key.y as f64 + 1.0 + pad_tiles,
        tile_key.z,
    );
    (south, west, north, east)
}


// --- Tile key ---

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileKey {
    pub z: u32,
    pub x: i32,
    pub y: i32,
}

pub fn is_descendant_tile(child: TileKey, parent: TileKey) -> bool {
    if child.z <= parent.z {
        return false;
    }
    let dz = child.z - parent.z;
    if dz >= 31 {
        return false;
    }
    let min_x = (parent.x as i64) << dz;
    let max_x = ((parent.x as i64 + 1) << dz) - 1;
    let min_y = (parent.y as i64) << dz;
    let max_y = ((parent.y as i64 + 1) << dz) - 1;
    let cx = child.x as i64;
    let cy = child.y as i64;
    cx >= min_x && cx <= max_x && cy >= min_y && cy <= max_y
}

/// True if the point lies on (within eps of) any edge of the bounds rect —
/// used to tell tile-clip cuts apart from true polyline endpoints.
pub fn point_on_bounds(point: (f32, f32), bounds: GeoBounds, eps: f32) -> bool {
    (point.0 - bounds.min.x).abs() <= eps
        || (point.0 - bounds.max.x).abs() <= eps
        || (point.1 - bounds.min.y).abs() <= eps
        || (point.1 - bounds.max.y).abs() <= eps
}

/// Sutherland-Hodgman clip of a (possibly concave) ring against a rect.
/// Returns an empty vec when the ring lies fully outside.
pub fn clip_ring_to_rect(ring: &[(f32, f32)], bounds: GeoBounds) -> Vec<(f32, f32)> {
    fn inside(p: (f32, f32), edge: u8, b: GeoBounds) -> bool {
        match edge {
            0 => p.0 >= b.min.x,
            1 => p.0 <= b.max.x,
            2 => p.1 >= b.min.y,
            _ => p.1 <= b.max.y,
        }
    }
    fn intersect(a: (f32, f32), b: (f32, f32), edge: u8, r: GeoBounds) -> (f32, f32) {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        match edge {
            0 | 1 => {
                let x = if edge == 0 { r.min.x } else { r.max.x };
                let t = if dx.abs() > f32::EPSILON {
                    (x - a.0) / dx
                } else {
                    0.0
                };
                (x, a.1 + dy * t)
            }
            _ => {
                let y = if edge == 2 { r.min.y } else { r.max.y };
                let t = if dy.abs() > f32::EPSILON {
                    (y - a.1) / dy
                } else {
                    0.0
                };
                (a.0 + dx * t, y)
            }
        }
    }

    let mut current = ring.to_vec();
    for edge in 0..4u8 {
        if current.len() < 3 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(current.len() + 4);
        for i in 0..current.len() {
            let a = current[i];
            let b = current[(i + 1) % current.len()];
            let a_in = inside(a, edge, bounds);
            let b_in = inside(b, edge, bounds);
            if a_in {
                out.push(a);
                if !b_in {
                    out.push(intersect(a, b, edge, bounds));
                }
            } else if b_in {
                out.push(intersect(a, b, edge, bounds));
            }
        }
        current = out;
    }
    if current.len() < 3 {
        return Vec::new();
    }
    current
}

/// Ring bbox fully inside the bounds rect (no clipping needed).
pub fn ring_inside_bounds(ring: &[(f32, f32)], bounds: GeoBounds) -> bool {
    ring.iter().all(|p| {
        p.0 >= bounds.min.x && p.0 <= bounds.max.x && p.1 >= bounds.min.y && p.1 <= bounds.max.y
    })
}

/// Clip rect in tile-local coordinates (tile origin at 0,0).
pub fn tile_clip_rect(padding: f32) -> (f32, f32, f32, f32) {
    let tile_size = TILE_SIZE as f32;
    (-padding, -padding, tile_size + padding, tile_size + padding)
}

pub fn tile_clip_bounds(padding: f32) -> GeoBounds {
    let (min_x, min_y, max_x, max_y) = tile_clip_rect(padding);
    GeoBounds {
        min: GeoPoint { x: min_x, y: min_y },
        max: GeoPoint { x: max_x, y: max_y },
    }
}

// --- Shared tag helpers ---

pub fn tag_is(tags: &HashMap<String, String>, key: &str, value: &str) -> bool {
    tags.get(key).is_some_and(|v| v == value)
}

pub fn tag_is_truthy(tags: &HashMap<String, String>, key: &str) -> bool {
    let Some(value) = tags.get(key) else {
        return false;
    };
    !matches!(value.as_str(), "" | "0" | "false" | "False" | "no")
}

pub fn is_road_polygon_layer(layer: &str) -> bool {
    matches!(layer, "street_polygons" | "streets_polygons_labels")
}

pub fn select_label_text(tags: &HashMap<String, String>) -> Option<String> {
    for key in ["name", "name:latin", "name:en", "name_int"] {
        if let Some(value) = tags.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(reference) = tags.get("ref") {
        let trimmed = reference.trim();
        if !trimmed.is_empty() && trimmed.len() <= 12 {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_segment_keeps_crossing_line() {
        let bounds = GeoBounds {
            min: GeoPoint { x: 0.0, y: 0.0 },
            max: GeoPoint { x: 10.0, y: 10.0 },
        };
        let parts = clip_polyline_parts(&[(-5.0, 5.0), (15.0, 5.0)], bounds, false);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].len(), 2);
        assert!((parts[0][0].0 - 0.0).abs() < 1e-5);
        assert!((parts[0][1].0 - 10.0).abs() < 1e-5);
    }

    #[test]
    fn simplify_reduces_dense_straight_line() {
        let points = vec![
            (0.0, 0.0),
            (1.0, 0.01),
            (2.0, 0.0),
            (3.0, -0.01),
            (4.0, 0.0),
        ];
        let simplified = simplify_polyline(&points, 0.2);
        assert!(simplified.len() <= 3);
        assert_eq!(simplified.first().copied(), Some((0.0, 0.0)));
        assert_eq!(simplified.last().copied(), Some((4.0, 0.0)));
    }
}

// --- Painter-order overlay unifier (no-overdraw road triangulation) ---

/// One paint group, bottom-to-top: everything that would have been painted
/// with this color at this position in the 2D painter sequence.
/// Sentinel for a paint group with no dz field (grounded tier).
pub const DZ_FIELD_NONE: u16 = u16::MAX;

pub struct PaintGroup {
    pub color: [f32; 4],
    pub param5: f32,
    /// Paint phase: 0 = plaza fills, 1 = casings, 2 = centers. Carried onto
    /// the output face so legacy strokes can interleave at their true rank.
    pub phase: u8,
    pub rank: i16,
    /// Index of this group's DzField (DZ_FIELD_NONE = grounded).
    pub field: u16,
    /// Flush tier-transition joints (way ends butt-joined to a same-class
    /// way in another tier — bridge/approach splits): skirt walls and caps
    /// are suppressed within `half_width` of these points so the deck
    /// reads as ONE continuous body across the style change.
    pub butt_points: Vec<(f32, f32)>,
    pub half_width: f32,
    /// (ring, min corner dz, max corner dz). Lifted rings stay visible but
    /// do NOT punch holes in the groups below: the road continues under a
    /// deck — paint order hides it flat, real depth hides it tilted. Rings
    /// whose dz range straddles LIFT_COVER_M join BOTH level parts, so the
    /// grounded and lifted meshes overlap at every transition instead of
    /// meeting at an epsilon-fragile shared edge.
    pub rings: Vec<(Vec<(f32, f32)>, f32, f32)>,
}

/// Rings lifted beyond this stop covering (subtracting from) lower groups.
pub const LIFT_COVER_M: f32 = 0.2;

/// A triangulated visible region of one paint group.
pub struct PaintFace {
    pub color: [f32; 4],
    pub param5: f32,
    pub phase: u8,
    pub rank: i16,
    pub field: u16,
    pub verts: Vec<VVertex>,
    pub indices: Vec<u32>,
    /// Antialiasing skirt along this face's outer boundary: u ramps
    /// 0.5 -> 0.0 outward over one screen px; drawn with stroke_mult 1.0
    /// right after the face, so painter-order AA falls out of the ladder.
    pub fringe_verts: Vec<VVertex>,
    pub fringe_indices: Vec<u32>,
    /// Level part: -1 sunk (tunnel), 0 grounded, 1 lifted (deck).
    pub level: i8,
    /// Deck side walls: vertical quads along the lifted boundary — top
    /// vertex (v=0) rides the deck dz, bottom vertex (v=1) stays grounded.
    /// Flat mode degenerates them to zero area; tilt reveals the wall.
    /// Closes the ramp-displacement crescents and reads as deck volume.
    pub skirt_verts: Vec<VVertex>,
    pub skirt_indices: Vec<u32>,
}

/// Vertical wall quads along one boundary ring of a lifted face.
fn append_ring_skirt(
    ring: &[[f64; 2]],
    butt_points: &[(f32, f32)],
    butt_reach: f32,
    verts: &mut Vec<VVertex>,
    indices: &mut Vec<u32>,
) {
    let n = ring.len();
    if n < 3 {
        return;
    }
    let mut area2 = 0.0f64;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        area2 += a[0] * b[1] - b[0] * a[1];
    }
    let out_sign = if area2 > 0.0 { 1.0f32 } else { -1.0 };
    let base = verts.len() as u32;
    for i in 0..n {
        let p = ring[i];
        let prev = ring[(i + n - 1) % n];
        let next = ring[(i + 1) % n];
        let (e0x, e0y) = ((p[0] - prev[0]) as f32, (p[1] - prev[1]) as f32);
        let (e1x, e1y) = ((next[0] - p[0]) as f32, (next[1] - p[1]) as f32);
        let l0 = (e0x * e0x + e0y * e0y).sqrt().max(1e-6);
        let l1 = (e1x * e1x + e1y * e1y).sqrt().max(1e-6);
        let (mut mx, mut my) = (-e0y / l0 - e1y / l1, e0x / l0 + e1x / l1);
        let ml = (mx * mx + my * my).sqrt().max(1e-6);
        mx /= ml;
        my /= ml;
        // stroke_dist = outward normal angle for the seam probe.
        let out_angle = (my * out_sign).atan2(mx * out_sign);
        let (px, py) = (p[0] as f32, p[1] as f32);
        verts.push(VVertex {
            x: px,
            y: py,
            u: 0.5,
            v: 0.0,
            stroke_dist: out_angle,
            clip_radius: 4.0,
        });
        verts.push(VVertex {
            x: px,
            y: py,
            u: 0.5,
            v: 1.0,
            stroke_dist: out_angle,
            clip_radius: 4.0,
        });
    }
    let reach_sq = butt_reach * butt_reach;
    for i in 0..n {
        let j = (i + 1) % n;
        let mid = (
            ((ring[i][0] + ring[j][0]) * 0.5) as f32,
            ((ring[i][1] + ring[j][1]) * 0.5) as f32,
        );
        // No wall across a flush joint: the deck continues there.
        let at_joint = butt_points.iter().any(|&(bx, by)| {
            let (dx, dy) = (mid.0 - bx, mid.1 - by);
            dx * dx + dy * dy < reach_sq
        });
        if at_joint {
            continue;
        }
        let (a_top, a_bottom) = (base + i as u32 * 2, base + i as u32 * 2 + 1);
        let (b_top, b_bottom) = (base + j as u32 * 2, base + j as u32 * 2 + 1);
        indices.extend_from_slice(&[a_top, a_bottom, b_bottom, a_top, b_bottom, b_top]);
    }
}

/// One ring's AA skirt: per-vertex miter offsets to the un-filled side.
/// Hole rings carry opposite orientation, so their skirts flip into the
/// courtyard automatically.
fn append_ring_fringe(
    ring: &[[f64; 2]],
    aa: f32,
    straddle: bool,
    verts: &mut Vec<VVertex>,
    indices: &mut Vec<u32>,
) {
    let n = ring.len();
    if n < 3 {
        return;
    }
    let mut area2 = 0.0f64;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        area2 += a[0] * b[1] - b[0] * a[1];
    }
    let out_sign = if area2 > 0.0 { 1.0f32 } else { -1.0 };
    let base = verts.len() as u32;
    for i in 0..n {
        let p = ring[i];
        let prev = ring[(i + n - 1) % n];
        let next = ring[(i + 1) % n];
        let (e0x, e0y) = ((p[0] - prev[0]) as f32, (p[1] - prev[1]) as f32);
        let (e1x, e1y) = ((next[0] - p[0]) as f32, (next[1] - p[1]) as f32);
        let l0 = (e0x * e0x + e0y * e0y).sqrt().max(1e-6);
        let l1 = (e1x * e1x + e1y * e1y).sqrt().max(1e-6);
        let (n0x, n0y) = (-e0y / l0, e0x / l0);
        let (n1x, n1y) = (-e1y / l1, e1x / l1);
        let (mut mx, mut my) = (n0x + n1x, n0y + n1y);
        let ml = (mx * mx + my * my).sqrt().max(1e-6);
        mx /= ml;
        my /= ml;
        // Miter clamp: sharp corners cap the rim reach at 2x aa. The skirt
        // STRADDLES the boundary — coverage crosses 50% exactly at the
        // face edge (the legacy convention); the inner half paints face
        // color over face color, a no-op.
        let cos_half = (mx * n1x + my * n1y).max(0.5);
        // Translucent faces must not overpaint themselves (premultiplied
        // double-blend darkens the band): their skirt starts AT the
        // boundary and only ramps outward. Opaque faces straddle so
        // coverage crosses 50% exactly at the edge.
        let reach = if straddle {
            aa * 0.5 / cos_half * out_sign
        } else {
            aa / cos_half * out_sign
        };
        let inner = if straddle { reach } else { 0.0 };
        let (px, py) = (p[0] as f32, p[1] as f32);
        // stroke_dist carries the OUTWARD normal angle: the emitter probes
        // the dz field outside each edge to drop internal seams (way-split
        // cap arcs mid-deck must not draw fringes or walls).
        let out_angle = (my * out_sign).atan2(mx * out_sign);
        // v = 1.0: the stroke-mode pixel shader multiplies by a cap mask
        // driven by tcoord.y (smoothstep from 0) — v of 0 renders the
        // whole skirt invisible.
        verts.push(VVertex {
            x: px - mx * inner,
            y: py - my * inner,
            u: 0.5,
            v: 1.0,
            stroke_dist: out_angle,
            clip_radius: aa * 4.0,
        });
        verts.push(VVertex {
            x: px + mx * reach,
            y: py + my * reach,
            u: 0.0,
            v: 1.0,
            stroke_dist: out_angle,
            clip_radius: aa * 4.0,
        });
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (a_in, a_out) = (base + i as u32 * 2, base + i as u32 * 2 + 1);
        let (b_in, b_out) = (base + j as u32 * 2, base + j as u32 * 2 + 1);
        indices.extend_from_slice(&[a_in, a_out, b_out, a_in, b_out, b_in]);
    }
}

/// Painter's algorithm as geometry: top-down subtraction cascade over the
/// groups (visible_k = shape_k − everything above), yielding DISJOINT
/// faces whose flat rendering is pixel-identical to painting the groups in
/// order — and whose depth order is therefore irrelevant, which is the
/// whole point for tilt/3D.
pub fn overlay_paint_groups(
    groups: &[PaintGroup],
    tess: &mut Tessellator,
    tolerance: f32,
    aa: f32,
) -> Vec<PaintFace> {
    use i_overlay::core::fill_rule::FillRule as IoFillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::simplify::SimplifyShape;
    use i_overlay::float::single::SingleFloatOverlay;
    type Shapes = Vec<Vec<Vec<[f64; 2]>>>;

    // Boolean input hygiene — the runaway fix. The extraction stage of the
    // boolean solver can spin forever chasing a contour cycle fed by
    // near-coincident / near-degenerate ring geometry (sliver bevel wedges,
    // float-noise twin edges). Snapping every coordinate to a 1/64-unit
    // grid turns "almost identical" into EXACTLY identical — which NonZero
    // winding resolves trivially — and post-snap degenerate rings are
    // dropped before they ever reach the solver.
    const SNAP: f64 = 64.0;
    let snap_ring = |ring: &[(f32, f32)]| -> Option<Vec<[f64; 2]>> {
        let mut out: Vec<[f64; 2]> = Vec::with_capacity(ring.len());
        for &(x, y) in ring {
            let p = [
                (f64::from(x) * SNAP).round() / SNAP,
                (f64::from(y) * SNAP).round() / SNAP,
            ];
            if out.last().is_some_and(|last| *last == p) {
                continue;
            }
            out.push(p);
        }
        while out.len() >= 2 && out.first() == out.last() {
            out.pop();
        }
        if out.len() < 3 {
            return None;
        }
        let mut area2 = 0.0f64;
        for i in 0..out.len() {
            let a = out[i];
            let b = out[(i + 1) % out.len()];
            area2 += a[0] * b[1] - b[0] * a[1];
        }
        if area2.abs() < 1e-3 {
            return None;
        }
        Some(out)
    };
    // Ring sets per group. Straddling rings (min < threshold <= max — a
    // ramp segment) are VISIBLE in both level parts so the meshes overlap
    // at transitions, but they must join NEITHER cover as grounded: a
    // partially lifted deck segment cutting the road underneath was the
    // under-the-overpass hole. Translucent groups never draw a ring twice
    // (premultiplied double-blend darkens), so their visible set is strict.
    #[derive(Clone, Copy, PartialEq)]
    enum RingSet {
        All,
        VisibleGrounded,
        CoverGrounded,
        Lifted,
        Sunk,
        CoverSunk,
        Surface,
    }
    let to_paths = |group: &PaintGroup, set: RingSet| -> Vec<Vec<[f64; 2]>> {
        let translucent = group.color[3] < 0.999;
        group
            .rings
            .iter()
            .filter(|(ring, min_dz, max_dz)| {
                if ring.len() < 3 {
                    return false;
                }
                match set {
                    RingSet::All => true,
                    RingSet::VisibleGrounded => {
                        if translucent {
                            *max_dz < LIFT_COVER_M && *min_dz > -LIFT_COVER_M
                        } else {
                            // Touches the grounded band (straddlers join).
                            *min_dz < LIFT_COVER_M && *max_dz > -LIFT_COVER_M
                        }
                    }
                    RingSet::CoverGrounded => {
                        *max_dz < LIFT_COVER_M && *min_dz > -LIFT_COVER_M
                    }
                    RingSet::Lifted => *max_dz >= LIFT_COVER_M,
                    RingSet::Sunk => *min_dz <= -LIFT_COVER_M,
                    RingSet::CoverSunk => *max_dz <= -LIFT_COVER_M,
                    RingSet::Surface => *max_dz > -LIFT_COVER_M,
                }
            })
            .filter_map(|(ring, _, _)| snap_ring(ring))
            .collect()
    };

    // The output must be DISJOINT faces per LEVEL — one flat continuous
    // triangulation with no overlapping geometry among content at the same
    // height: coplanar overlaps cannot be separated reliably in tilt
    // (independently-triangulated faces interpolate dz differently, so a
    // casing bleeds through its own center). Two covers realize that:
    // grounded content subtracts the grounded cover above it, lifted
    // content subtracts the lifted cover above it — a deck therefore cuts
    // its own casing (both lifted) but never cuts the road running
    // underneath (grounded), and ramp feet (grounded ends of a deck) still
    // cut the streets they meet. Each group first DISSOLVES its ring soup
    // per level with one local self-union, so the cross-group cascade only
    // ever sees clean outlines. The grounded/lifted parts of one group may
    // overlap at level-transition seams: same color, same depth slot,
    // same dz field — identical pixels, so the tie is invisible.
    struct GroupOutline {
        /// The whole drivable surface (grounded + lifted + portal ramps) as
        /// ONE outline: the face a car could drive over must be a single
        /// continuous mesh — splitting it at level seams left overlapping
        /// twins that micro-diverge at grazing tilt (the lens endcaps).
        surface_shapes: Shapes,
        surface_paths: Vec<Vec<[f64; 2]>>,
        grounded_shapes: Shapes,
        grounded_paths: Vec<Vec<[f64; 2]>>,
        /// Strictly grounded outline (no straddling rings) — the only part
        /// allowed to cut content below in the cascade.
        cover_grounded_paths: Vec<Vec<[f64; 2]>>,
        lifted_shapes: Shapes,
        lifted_paths: Vec<Vec<[f64; 2]>>,
        sunk_shapes: Shapes,
        sunk_paths: Vec<Vec<[f64; 2]>>,
        cover_sunk_paths: Vec<Vec<[f64; 2]>>,
        bbox: (f64, f64, f64, f64),
    }
    // Hang forensics: with /tmp/mp_boolean_debug present, every boolean's
    // input is written BEFORE the call — when the solver spins forever and
    // the memory watchdog shoots the process, the last file on disk IS the
    // repro (replayed by the boolean_repro test).
    let debug_dump = std::path::Path::new("/tmp/mp_boolean_debug").exists();
    let dump_paths = |tag: &str, paths: &[Vec<[f64; 2]>]| {
        if !debug_dump {
            return;
        }
        use std::io::Write as _;
        let name = format!(
            "/tmp/mp_boolean_last_{:?}.txt",
            std::thread::current().id()
        );
        if let Ok(mut file) = std::fs::File::create(&name) {
            let _ = writeln!(file, "# {tag}");
            for ring in paths {
                let line: Vec<String> = ring
                    .iter()
                    .map(|p| format!("{:.6},{:.6}", p[0], p[1]))
                    .collect();
                let _ = writeln!(file, "{}", line.join(" "));
            }
        }
    };
    // Chunked dissolve: the solver's contour extraction degenerates on
    // ring soups past a few thousand rings (dense residential tiers at
    // high overzoom reach 20k — measured spinning >25s and allocating
    // unboundedly). Dissolving bounded chunks and unioning the clean
    // outlines keeps every single call in solver-friendly territory.
    const DISSOLVE_CHUNK: usize = 3000;
    let dissolve = |paths: Vec<Vec<[f64; 2]>>| -> (Shapes, Vec<Vec<[f64; 2]>>) {
        if paths.is_empty() {
            return (Vec::new(), Vec::new());
        }
        dump_paths("simplify", &paths);
        let shapes: Shapes = if paths.len() <= DISSOLVE_CHUNK {
            paths.simplify_shape(IoFillRule::NonZero)
        } else {
            let mut acc: Shapes = Vec::new();
            for chunk in paths.chunks(DISSOLVE_CHUNK) {
                let part: Shapes = chunk.to_vec().simplify_shape(IoFillRule::NonZero);
                if acc.is_empty() {
                    acc = part;
                } else {
                    let part_paths: Vec<Vec<[f64; 2]>> = part
                        .iter()
                        .flat_map(|shape| shape.iter().cloned())
                        .collect();
                    acc = part_paths.overlay(&acc, OverlayRule::Union, IoFillRule::NonZero);
                }
            }
            acc
        };
        let flat = shapes
            .iter()
            .flat_map(|shape| shape.iter().cloned())
            .collect();
        (shapes, flat)
    };
    let outlines: Vec<GroupOutline> = groups
        .iter()
        .map(|group| {
            let has_lifted = group
                .rings
                .iter()
                .any(|(_, _, max_dz)| *max_dz >= LIFT_COVER_M);
            let has_sunk = group
                .rings
                .iter()
                .any(|(_, min_dz, _)| *min_dz <= -LIFT_COVER_M);
            let has_straddler = group
                .rings
                .iter()
                .any(|(_, min_dz, max_dz)| {
                    (*min_dz < LIFT_COVER_M && *max_dz >= LIFT_COVER_M)
                        || (*max_dz > -LIFT_COVER_M && *min_dz <= -LIFT_COVER_M)
                });
            let (grounded_shapes, grounded_paths) =
                dissolve(to_paths(group, RingSet::VisibleGrounded));
            let (surface_shapes, surface_paths) = if has_lifted || has_sunk {
                dissolve(to_paths(group, RingSet::Surface))
            } else {
                (grounded_shapes.clone(), grounded_paths.clone())
            };
            let cover_grounded_paths = if has_straddler {
                dissolve(to_paths(group, RingSet::CoverGrounded)).1
            } else {
                grounded_paths.clone()
            };
            let (lifted_shapes, lifted_paths) = if has_lifted {
                dissolve(to_paths(group, RingSet::Lifted))
            } else {
                (Vec::new(), Vec::new())
            };
            let (sunk_shapes, sunk_paths) = if has_sunk {
                dissolve(to_paths(group, RingSet::Sunk))
            } else {
                (Vec::new(), Vec::new())
            };
            let cover_sunk_paths = if has_sunk {
                if has_straddler {
                    dissolve(to_paths(group, RingSet::CoverSunk)).1
                } else {
                    sunk_paths.clone()
                }
            } else {
                Vec::new()
            };
            let mut bbox = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for path in grounded_paths
                .iter()
                .chain(lifted_paths.iter())
                .chain(sunk_paths.iter())
            {
                for p in path {
                    bbox.0 = bbox.0.min(p[0]);
                    bbox.1 = bbox.1.min(p[1]);
                    bbox.2 = bbox.2.max(p[0]);
                    bbox.3 = bbox.3.max(p[1]);
                }
            }
            GroupOutline {
                surface_shapes,
                surface_paths,
                grounded_shapes,
                grounded_paths,
                cover_grounded_paths,
                lifted_shapes,
                lifted_paths,
                sunk_shapes,
                sunk_paths,
                cover_sunk_paths,
                bbox,
            }
        })
        .collect();

    // Incremental cascade per level, all operands dissolved outlines.
    // LIFTED content never enters an accumulated cover: two decks at
    // different heights (a viaduct over a bridge) must not cut each other
    // — real depth separates them. The only lifted subtraction that is
    // always height-safe is WITHIN one tier: the center cutting its own
    // casing (same ways, same heights by construction). Pair them by the
    // tier/field id. Same for sunk (stacked tunnels are rarer still).
    let mut center_lifted_by_field: std::collections::HashMap<u16, Vec<Vec<[f64; 2]>>> =
        std::collections::HashMap::new();
    let mut center_sunk_by_field: std::collections::HashMap<u16, Vec<Vec<[f64; 2]>>> =
        std::collections::HashMap::new();
    for (group, outline) in groups.iter().zip(outlines.iter()) {
        if group.phase == 2 {
            if !outline.lifted_paths.is_empty() {
                center_lifted_by_field.insert(group.field, outline.lifted_paths.clone());
            }
            if !outline.sunk_paths.is_empty() {
                center_sunk_by_field.insert(group.field, outline.sunk_paths.clone());
            }
        }
    }
    let mut cover_grounded: Shapes = Vec::new();
    let mut cover_bbox = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut visibles: Vec<(Shapes, Shapes)> =
        vec![(Vec::new(), Vec::new()); outlines.len()];
    for k in (0..outlines.len()).rev() {
        let outline = &outlines[k];
        let bbox = outline.bbox;
        let overlaps_cover = bbox.0 <= cover_bbox.2
            && bbox.2 >= cover_bbox.0
            && bbox.1 <= cover_bbox.3
            && bbox.3 >= cover_bbox.1;
        let visible_part = |part_paths: &Vec<Vec<[f64; 2]>>,
                            part_shapes: &Shapes,
                            cover: &Shapes|
         -> Shapes {
            if part_paths.is_empty() {
                Vec::new()
            } else if overlaps_cover && !cover.is_empty() {
                if debug_dump {
                    let mut all = part_paths.clone();
                    all.extend(cover.iter().flat_map(|shape| shape.iter().cloned()));
                    dump_paths("difference", &all);
                }
                part_paths.overlay(cover, OverlayRule::Difference, IoFillRule::NonZero)
            } else {
                part_shapes.clone()
            }
        };
        // ONE unified surface face per group — a continuous mesh a car
        // could drive over. Grounded-only groups keep the plain cascade;
        // groups with decks rebuild as (surface − grounded cover) ∪ lifted
        // so under-deck holes close without splitting the mesh into
        // overlapping level twins (those micro-diverged at grazing tilt).
        // Casings then subtract their own tier's center deck (same ways,
        // same heights — the only always-height-safe lifted subtraction).
        let has_deck = !outline.lifted_paths.is_empty();
        let has_sunk_part = !outline.sunk_paths.is_empty();
        let main = if !has_deck && !has_sunk_part {
            visible_part(
                &outline.grounded_paths,
                &outline.grounded_shapes,
                &cover_grounded,
            )
        } else {
            let base: Shapes = if overlaps_cover && !cover_grounded.is_empty() {
                outline.surface_paths.overlay(
                    &cover_grounded,
                    OverlayRule::Difference,
                    IoFillRule::NonZero,
                )
            } else {
                outline.surface_shapes.clone()
            };
            let mut merged: Vec<Vec<[f64; 2]>> = base
                .iter()
                .flat_map(|shape| shape.iter().cloned())
                .collect();
            merged.extend(outline.lifted_paths.iter().cloned());
            let mut unified: Shapes = merged.simplify_shape(IoFillRule::NonZero);
            if groups[k].phase == 1 {
                if let Some(center) = center_lifted_by_field.get(&groups[k].field) {
                    let unified_paths: Vec<Vec<[f64; 2]>> = unified
                        .iter()
                        .flat_map(|shape| shape.iter().cloned())
                        .collect();
                    unified = unified_paths.overlay(
                        center,
                        OverlayRule::Difference,
                        IoFillRule::NonZero,
                    );
                }
            }
            unified
        };
        let sunk = if !has_sunk_part {
            Vec::new()
        } else if groups[k].phase == 1 {
            if let Some(center) = center_sunk_by_field.get(&groups[k].field) {
                outline.sunk_paths.overlay(
                    center,
                    OverlayRule::Difference,
                    IoFillRule::NonZero,
                )
            } else {
                outline.sunk_shapes.clone()
            }
        } else {
            outline.sunk_shapes.clone()
        };
        visibles[k] = (main, sunk);
        if !outline.cover_grounded_paths.is_empty() {
            if debug_dump {
                let mut all = outline.cover_grounded_paths.clone();
                all.extend(cover_grounded.iter().flat_map(|shape| shape.iter().cloned()));
                dump_paths("union-grounded", &all);
            }
            cover_grounded = outline.cover_grounded_paths.overlay(
                &cover_grounded,
                OverlayRule::Union,
                IoFillRule::NonZero,
            );
        }
        if !outline.grounded_paths.is_empty()
            || !outline.lifted_paths.is_empty()
            || !outline.sunk_paths.is_empty()
        {
            cover_bbox.0 = cover_bbox.0.min(bbox.0);
            cover_bbox.1 = cover_bbox.1.min(bbox.1);
            cover_bbox.2 = cover_bbox.2.max(bbox.2);
            cover_bbox.3 = cover_bbox.3.max(bbox.3);
        }
    }

    let mut faces = Vec::new();
    let mut path = VectorPath::new();
    let mut tess_verts: Vec<VVertex> = Vec::new();
    let mut tess_indices: Vec<u32> = Vec::new();
    for ((group, (visible_main, visible_sunk)), outline) in
        groups.iter().zip(visibles).zip(outlines.iter())
    {
        for (part_index, visible) in [visible_main, visible_sunk].into_iter().enumerate() {
            let level: i8 = if part_index == 1 { -1 } else { 0 };
            // Walls hang from the deck's TRUE outer boundary (the lifted
            // outline), attached to the unified main face.
            let lifted = part_index == 0 && !outline.lifted_shapes.is_empty();
            if visible.is_empty() {
                continue;
            }
            // Each shape = outer ring + holes; contours are crossing-free,
            // so even-odd (no explicit winding) fills holes correctly.
            for shape in &visible {
                for ring in shape {
                    if ring.len() < 3 {
                        continue;
                    }
                    path.move_to(ring[0][0] as f32, ring[0][1] as f32);
                    for point in ring.iter().skip(1) {
                        path.line_to(point[0] as f32, point[1] as f32);
                    }
                    path.close();
                }
            }
            tessellate_path_fill(
                &mut path,
                tess,
                &mut tess_verts,
                &mut tess_indices,
                LineJoin::Miter,
                4.0,
                0.0,
                false,
                tolerance,
            );
            if tess_verts.is_empty() || tess_indices.is_empty() {
                continue;
            }
            let mut fringe_verts: Vec<VVertex> = Vec::new();
            let mut fringe_indices: Vec<u32> = Vec::new();
            let mut skirt_verts: Vec<VVertex> = Vec::new();
            let mut skirt_indices: Vec<u32> = Vec::new();
            let straddle = group.color[3] >= 0.999;
            for shape in &visible {
                for ring in shape {
                    if aa > 0.0 {
                        append_ring_fringe(
                            ring,
                            aa,
                            straddle,
                            &mut fringe_verts,
                            &mut fringe_indices,
                        );
                    }
                }
            }
            if lifted {
                for shape in &outline.lifted_shapes {
                    for ring in shape {
                        append_ring_skirt(
                            ring,
                            &group.butt_points,
                            group.half_width + 0.75,
                            &mut skirt_verts,
                            &mut skirt_indices,
                        );
                    }
                }
            }
            faces.push(PaintFace {
                color: group.color,
                param5: group.param5,
                phase: group.phase,
                rank: group.rank,
                field: group.field,
                verts: tess_verts.clone(),
                indices: tess_indices.clone(),
                fringe_verts,
                fringe_indices,
                level,
                skirt_verts,
                skirt_indices,
            });
        }
    }
    faces
}

/// Nearest-way deck field for one paint tier: every way of the tier —
/// grounded ways included at dz 0 — indexed on a coarse grid, sampled by
/// nearest segment. Ownership by nearest centerline is what keeps a
/// grounded street flat beside an elevated deck (the parallel-carpet
/// problem), while the whole cross-section of the deck itself, edge to
/// edge, lifts uniformly. The baker's junction consensus makes way dz agree
/// at shared nodes, so nearest-wins stays continuous across junctions.
pub struct DzField {
    cell: f32,
    radius: f32,
    grid: std::collections::HashMap<(i32, i32), Vec<u32>>,
    /// Cells within reach of a lifted segment — the "needs subdivision" set.
    active: std::collections::HashSet<(i32, i32)>,
    segs: Vec<[f32; 6]>,
}

impl DzField {
    /// Returns None when the tier carries no lift at all — the zero field
    /// needs no sampling and no subdivision.
    pub fn build(ways: &[(&[(f32, f32)], Option<&[f32]>)], radius: f32) -> Option<DzField> {
        if !ways
            .iter()
            .any(|(_, dz)| dz.is_some_and(|dz| dz.iter().any(|&v| v.abs() > 0.01)))
        {
            return None;
        }
        let radius = radius.max(1.0);
        let cell = radius * 2.0;
        let mut field = DzField {
            cell,
            radius,
            grid: Default::default(),
            active: Default::default(),
            segs: Vec::new(),
        };
        for (points, dz) in ways {
            for i in 0..points.len().saturating_sub(1) {
                let (mut ax, mut ay) = points[i];
                let (mut bx, mut by) = points[i + 1];
                let (dza, dzb) = dz.map_or((0.0, 0.0), |d| (d[i], d[i + 1]));
                // A lifted way END (tile clip or data end mid-deck) renders
                // a round cap past the endpoint; extend the terminal
                // segment so the cap sits at full deck height instead of
                // drooping through the distance fade. A degenerate terminal
                // segment must NOT be extended: normalizing its near-zero
                // direction shoots the endpoint astronomically far out and
                // the grid insertion below then walks billions of cells.
                let cap_reach = (radius - 2.0).max(0.0);
                let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
                if cap_reach > 0.0 && len > 0.5 {
                    let (ux, uy) = ((bx - ax) / len, (by - ay) / len);
                    if i == 0 && dza.abs() > 0.2 {
                        ax -= ux * cap_reach;
                        ay -= uy * cap_reach;
                    }
                    if i + 2 == points.len() && dzb.abs() > 0.2 {
                        bx += ux * cap_reach;
                        by += uy * cap_reach;
                    }
                }
                let id = field.segs.len() as u32;
                field.segs.push([ax, ay, bx, by, dza, dzb]);
                // Hard clamp: tile-local coords live within a few hundred
                // units — any garbage (NaN, inf, un-guarded math upstream)
                // must not turn this insertion into a multi-billion-cell
                // walk that hangs the worker while allocating endlessly.
                let clamp_cell = |v: f32| ((v / cell).floor() as i32).clamp(-1024, 1024);
                let min_cx = clamp_cell(ax.min(bx) - radius);
                let max_cx = clamp_cell(ax.max(bx) + radius);
                let min_cy = clamp_cell(ay.min(by) - radius);
                let max_cy = clamp_cell(ay.max(by) + radius);
                for cx in min_cx..=max_cx {
                    for cy in min_cy..=max_cy {
                        field.grid.entry((cx, cy)).or_default().push(id);
                        if dza.abs() > 0.01 || dzb.abs() > 0.01 {
                            field.active.insert((cx, cy));
                        }
                    }
                }
            }
        }
        Some(field)
    }

    /// Does this bbox touch any cell within reach of a lifted segment?
    pub fn active_near(&self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> bool {
        let clamp_cell = |v: f32| ((v / self.cell).floor() as i32).clamp(-1024, 1024);
        let min_cx = clamp_cell(min_x);
        let max_cx = clamp_cell(max_x);
        let min_cy = clamp_cell(min_y);
        let max_cy = clamp_cell(max_y);
        for cx in min_cx..=max_cx {
            for cy in min_cy..=max_cy {
                if self.active.contains(&(cx, cy)) {
                    return true;
                }
            }
        }
        false
    }

    /// Deck height at a point: nearest tier segment within `radius` wins,
    /// fading out over the outer 1.5 units so faces that leave the corridor
    /// stay continuous.
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let key = (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
        );
        let Some(ids) = self.grid.get(&key) else {
            return 0.0;
        };
        let mut best_d2 = f32::MAX;
        let mut best_dz = 0.0f32;
        for &id in ids {
            let [ax, ay, bx, by, dza, dzb] = self.segs[id as usize];
            let (ex, ey) = (bx - ax, by - ay);
            let el2 = (ex * ex + ey * ey).max(1e-9);
            let t = (((x - ax) * ex + (y - ay) * ey) / el2).clamp(0.0, 1.0);
            let (qx, qy) = (ax + ex * t - x, ay + ey * t - y);
            let d2 = qx * qx + qy * qy;
            if d2 < best_d2 {
                best_d2 = d2;
                best_dz = dza + (dzb - dza) * t;
            }
        }
        if best_dz == 0.0 || best_d2 >= self.radius * self.radius {
            return 0.0;
        }
        let d = best_d2.sqrt();
        let fade_start = (self.radius - 1.5).max(0.0);
        let fade =
            1.0 - ((d - fade_start) / (self.radius - fade_start).max(1e-3)).clamp(0.0, 1.0);
        best_dz * fade
    }
}

/// Crack-free refinement of a face mesh near lifted geometry: every edge
/// longer than `max_edge` that touches the field's active area is split at
/// its midpoint — shared midpoints via the edge map, so neighboring
/// triangles always agree and no T-junctions appear. Runs to a fixpoint so
/// deck ramps interpolate as smoothly as the legacy dense strokes did.
pub fn subdivide_face_mesh(
    verts: &mut Vec<VVertex>,
    indices: &mut Vec<u32>,
    max_edge: f32,
    field: &DzField,
) {
    use std::collections::HashMap;
    let max_edge_sq = max_edge * max_edge;
    for _pass in 0..10 {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut out: Vec<u32> = Vec::with_capacity(indices.len());
        let mut split_any = false;
        let need_split = |verts: &[VVertex], i: u32, j: u32| -> bool {
            let (vi, vj) = (&verts[i as usize], &verts[j as usize]);
            let d2 = (vi.x - vj.x).powi(2) + (vi.y - vj.y).powi(2);
            d2 > max_edge_sq
                && field.active_near(
                    vi.x.min(vj.x),
                    vi.y.min(vj.y),
                    vi.x.max(vj.x),
                    vi.y.max(vj.y),
                )
        };
        for t in 0..indices.len() / 3 {
            let (mut a, mut b, mut c) = (indices[t * 3], indices[t * 3 + 1], indices[t * 3 + 2]);
            let (mut sab, mut sbc, mut sca) = (
                need_split(verts, a, b),
                need_split(verts, b, c),
                need_split(verts, c, a),
            );
            // Rotate so the split pattern is canonical: single split on
            // (a,b); double split on (a,b)+(b,c).
            for _ in 0..2 {
                let rotate = match (sab, sbc, sca) {
                    (false, true, _) | (false, false, true) => true,
                    (true, false, true) => true,
                    _ => false,
                };
                if !rotate {
                    break;
                }
                let (na, nb, nc) = (b, c, a);
                let (nab, nbc, nca) = (sbc, sca, sab);
                a = na;
                b = nb;
                c = nc;
                sab = nab;
                sbc = nbc;
                sca = nca;
            }
            let mut mid = |i: u32, j: u32, verts: &mut Vec<VVertex>| -> u32 {
                let key = (i.min(j), i.max(j));
                *midpoints.entry(key).or_insert_with(|| {
                    let (vi, vj) = (verts[i as usize], verts[j as usize]);
                    verts.push(VVertex {
                        x: (vi.x + vj.x) * 0.5,
                        y: (vi.y + vj.y) * 0.5,
                        u: (vi.u + vj.u) * 0.5,
                        v: (vi.v + vj.v) * 0.5,
                        stroke_dist: (vi.stroke_dist + vj.stroke_dist) * 0.5,
                        clip_radius: vi.clip_radius.max(vj.clip_radius),
                    });
                    (verts.len() - 1) as u32
                })
            };
            match (sab, sbc, sca) {
                (false, false, false) => out.extend_from_slice(&[a, b, c]),
                (true, false, false) => {
                    let m = mid(a, b, verts);
                    out.extend_from_slice(&[a, m, c, m, b, c]);
                    split_any = true;
                }
                (true, true, false) => {
                    let m1 = mid(a, b, verts);
                    let m2 = mid(b, c, verts);
                    out.extend_from_slice(&[a, m1, c, m1, m2, c, m1, b, m2]);
                    split_any = true;
                }
                (true, true, true) => {
                    let m1 = mid(a, b, verts);
                    let m2 = mid(b, c, verts);
                    let m3 = mid(c, a, verts);
                    out.extend_from_slice(&[a, m1, m3, m1, b, m2, m3, m2, c, m1, m2, m3]);
                    split_any = true;
                }
                // Rotation above normalized the remaining patterns away.
                _ => out.extend_from_slice(&[a, b, c]),
            }
        }
        *indices = out;
        if !split_any {
            break;
        }
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    /// Rasterize triangles (constant color per set) over a background —
    /// shared by ground truth and unified rendering.
    fn raster(
        sets: &[(&[VVertex], &[u32], [f32; 4])],
        size: usize,
        scale: f32,
    ) -> Vec<[f32; 3]> {
        let mut image = vec![[1.0f32; 3]; size * size];
        for (verts, indices, color) in sets {
            for tri in indices.chunks_exact(3) {
                let p: Vec<[f32; 2]> = tri
                    .iter()
                    .map(|&i| [verts[i as usize].x * scale, verts[i as usize].y * scale])
                    .collect();
                let area =
                    (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                if area.abs() < 1e-9 {
                    continue;
                }
                let sign = area.signum();
                let min_x = p.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                let max_x = (p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil() as usize).min(size - 1);
                let min_y = p.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                let max_y = (p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil() as usize).min(size - 1);
                for py in min_y..=max_y {
                    for px in min_x..=max_x {
                        let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                        let inside = (0..3).all(|k| {
                            let (a, b) = (p[k], p[(k + 1) % 3]);
                            ((b[0] - a[0]) * (fy - a[1]) - (b[1] - a[1]) * (fx - a[0])) * sign >= 0.0
                        });
                        if inside {
                            image[py * size + px] = [color[0], color[1], color[2]];
                        }
                    }
                }
            }
        }
        image
    }

    /// The abstract case: two crossing roads (grey casing + white center)
    /// and one yellow road crossing both. Unified faces must rasterize
    /// pixel-identically to painting each ring in order.
    #[test]
    fn overlay_matches_painter_order() {
        let clip = tile_clip_bounds(0.0);
        let ways: [(&[(f32, f32)], u32); 3] = [
            (&[(20.0, 128.0), (236.0, 128.0)], 0),
            (&[(128.0, 20.0), (128.0, 236.0)], 0),
            (&[(30.0, 30.0), (226.0, 226.0)], 1),
        ];
        let casing_color = [0.4, 0.4, 0.4, 1.0];
        let center_white = [1.0, 1.0, 1.0, 1.0];
        let center_yellow = [1.0, 0.95, 0.6, 1.0];
        // Paint sequence: casings (grey wide) bottom, then white centers,
        // then yellow center on top.
        let mut groups: Vec<PaintGroup> = Vec::new();
        for (pass, color, class_filter, width) in [
            (0, casing_color, 0u32, 14.0f32),
            (0, casing_color, 1, 18.0),
            (1, center_white, 0, 10.0),
            (1, center_yellow, 1, 14.0),
        ] {
            let mut rings = Vec::new();
            for &(points, class) in &ways {
                if class != class_filter {
                    continue;
                }
                let ribbon = [RoadRibbon { points, dz: None, closed_ring: false, start_disc: true, end_disc: true }];
                for (ring, _) in road_ribbon_rings(&ribbon, width * 0.5, 0.0, clip) {
                    rings.push((ring, 0.0, 0.0));
                }
            }
            let _ = pass;
            groups.push(PaintGroup {
                color,
                param5: 0.0,
                phase: 0,
                rank: 0,
                field: DZ_FIELD_NONE,
                butt_points: Vec::new(),
                half_width: 1.0,
                rings,
            });
        }
        let mut tess = Tessellator::default();
        // Ground truth: tessellate each ring separately, paint in order.
        let mut truth_sets: Vec<(Vec<VVertex>, Vec<u32>, [f32; 4])> = Vec::new();
        for group in &groups {
            for (ring, _, _) in &group.rings {
                let mut path = VectorPath::new();
                path.move_to(ring[0].0, ring[0].1);
                for point in ring.iter().skip(1) {
                    path.line_to(point.0, point.1);
                }
                path.close();
                let mut verts = Vec::new();
                let mut indices = Vec::new();
                tessellate_path_fill(
                    &mut path,
                    &mut tess,
                    &mut verts,
                    &mut indices,
                    LineJoin::Miter,
                    4.0,
                    0.0,
                    false,
                    0.25,
                );
                truth_sets.push((verts, indices, group.color));
            }
        }
        // Unified: overlay cascade.
        let faces = overlay_paint_groups(&groups, &mut tess, 0.25, 0.0);
        let truth_refs: Vec<(&[VVertex], &[u32], [f32; 4])> = truth_sets
            .iter()
            .map(|(v, i, c)| (v.as_slice(), i.as_slice(), *c))
            .collect();
        let face_refs: Vec<(&[VVertex], &[u32], [f32; 4])> = faces
            .iter()
            .map(|f| (f.verts.as_slice(), f.indices.as_slice(), f.color))
            .collect();
        let size = 512;
        let truth = raster(&truth_refs, size, 2.0);
        let unified = raster(&face_refs, size, 2.0);
        let mut wrong = 0usize;
        for i in 0..size * size {
            let d = (0..3)
                .map(|c| (truth[i][c] - unified[i][c]).abs())
                .fold(0.0f32, f32::max);
            if d > 0.1 {
                wrong += 1;
            }
        }
        let pct = wrong as f64 / (size * size) as f64 * 100.0;
        println!("overlay vs painter: {wrong} px wrong ({pct:.3}%)");
        // Tolerance: edge rasterization jitter only.
        assert!(pct < 0.2, "unified overlay diverges from painter order: {pct:.2}%");
    }
}

#[cfg(test)]
mod boolean_repro_tests {
    /// Replay a hang capture from /tmp/mp_boolean_last_*.txt (written when
    /// /tmp/mp_boolean_debug exists). Run manually:
    ///   MP_REPRO=/tmp/mp_boolean_last_ThreadId(7).txt cargo test -p \
    ///   makepad-widgets --features maps --release boolean_repro -- \
    ///   --ignored --nocapture
    #[test]
    #[ignore]
    fn boolean_repro() {
        use i_overlay::core::fill_rule::FillRule;
        use i_overlay::core::overlay_rule::OverlayRule;
        use i_overlay::float::simplify::SimplifyShape;
        use i_overlay::float::single::SingleFloatOverlay;
        let path = std::env::var("MP_REPRO").expect("set MP_REPRO to a capture file");
        let text = std::fs::read_to_string(&path).unwrap();
        let mut tag = String::new();
        let mut rings: Vec<Vec<[f64; 2]>> = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# ") {
                tag = rest.to_string();
                continue;
            }
            let ring: Vec<[f64; 2]> = line
                .split_whitespace()
                .filter_map(|pair| {
                    let (x, y) = pair.split_once(',')?;
                    Some([x.parse().ok()?, y.parse().ok()?])
                })
                .collect();
            if ring.len() >= 3 {
                rings.push(ring);
            }
        }
        println!("repro: tag={} rings={}", tag, rings.len());
        let clock = std::time::Instant::now();
        let result = match tag.as_str() {
            "simplify" => {
                // Mirror production chunking (DISSOLVE_CHUNK).
                const CHUNK: usize = 3000;
                if rings.len() <= CHUNK {
                    rings.simplify_shape(FillRule::NonZero)
                } else {
                    let mut acc: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
                    for chunk in rings.chunks(CHUNK) {
                        let part = chunk.to_vec().simplify_shape(FillRule::NonZero);
                        if acc.is_empty() {
                            acc = part;
                        } else {
                            let part_paths: Vec<Vec<[f64; 2]>> = part
                                .iter()
                                .flat_map(|shape| shape.iter().cloned())
                                .collect();
                            acc = part_paths.overlay(&acc, OverlayRule::Union, FillRule::NonZero);
                        }
                    }
                    acc
                }
            }
            _ => {
                let empty: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
                let _ = empty;
                rings.overlay(
                    &Vec::<Vec<Vec<[f64; 2]>>>::new(),
                    if tag.starts_with("union") { OverlayRule::Union } else { OverlayRule::Difference },
                    FillRule::NonZero,
                )
            }
        };
        println!(
            "repro: done in {:.1}ms, {} shapes",
            clock.elapsed().as_secs_f64() * 1000.0,
            result.len()
        );
    }
}
