use super::style::StrokePassStyle;
use crate::makepad_draw::vector::{
    append_tessellated_geometry, tessellate_path_stroke, LineCap, LineJoin, Tessellator, VVertex,
    VectorPath, VectorRenderParams, VECTOR_ZBIAS_STEP,
};
use crate::makepad_draw::*;

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
    let mut ccw: Option<bool> = None;

    for ring in selected {
        let is_ccw = ring.signed_area < 0.0;
        if ccw.is_none() {
            ccw = Some(is_ccw);
        }

        if ccw == Some(is_ccw) {
            if !current.is_empty() {
                polygons.push(current);
                current = Vec::new();
            }
            current.push(ring.points.clone());
        } else if !current.is_empty() {
            current.push(ring.points.clone());
        }
    }

    if !current.is_empty() {
        polygons.push(current);
    }

    polygons
}

// --- Screen-space polyline helpers ---

pub fn build_screen_polyline(
    path_points: &[(f32, f32)],
    scale: f32,
    map_offset: Vec2f,
) -> Vec<Vec2d> {
    let mut out = Vec::<Vec2d>::with_capacity(path_points.len());
    for &(x, y) in path_points {
        out.push(dvec2(
            x as f64 * scale as f64 + map_offset.x as f64,
            y as f64 * scale as f64 + map_offset.y as f64,
        ));
    }
    out
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

pub fn polyline_cumulative_lengths(points: &[Vec2d]) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len());
    let mut sum = 0.0_f64;
    out.push(sum);
    for pair in points.windows(2) {
        let dx = pair[1].x - pair[0].x;
        let dy = pair[1].y - pair[0].y;
        sum += (dx * dx + dy * dy).sqrt();
        out.push(sum);
    }
    out
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

    let lines = polylines
        .iter()
        .filter(|line| line.len() >= 2)
        .cloned()
        .collect::<Vec<_>>();
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

pub fn append_stroke_pass(
    path: &mut VectorPath,
    points: &[(f32, f32)],
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    stroke_vertices: &mut Vec<f32>,
    stroke_indices: &mut Vec<u32>,
    pass: StrokePassStyle,
    line_cap: LineCap,
    stroke_zbias: &mut f32,
) {
    let stroke_points = expand_polyline_endpoints(points, pass.width);
    emit_path(path, &stroke_points, false);
    let stroke_mult = tessellate_path_stroke(
        path,
        tess,
        tess_verts,
        tess_indices,
        pass.width,
        line_cap,
        LineJoin::Round,
        4.0,
        1.0,
    );
    append_tessellated_geometry(
        tess_verts,
        tess_indices,
        stroke_vertices,
        stroke_indices,
        VectorRenderParams {
            color: hex_to_premul_rgba(pass.color, 1.0),
            stroke_mult,
            shape_id: pass.shape_id,
            params: [0.0; 6],
            zbias: *stroke_zbias,
        },
    );
    *stroke_zbias += VECTOR_ZBIAS_STEP;
}

pub fn append_stroke_fill_overlay_pass(
    path: &mut VectorPath,
    points: &[(f32, f32)],
    tess: &mut Tessellator,
    tess_verts: &mut Vec<VVertex>,
    tess_indices: &mut Vec<u32>,
    stroke_vertices: &mut Vec<f32>,
    stroke_indices: &mut Vec<u32>,
    pass: StrokePassStyle,
    line_cap: LineCap,
    stroke_zbias: &mut f32,
) {
    let stroke_points = expand_polyline_endpoints(points, pass.width);
    emit_path(path, &stroke_points, false);
    let stroke_mult = tessellate_path_stroke(
        path,
        tess,
        tess_verts,
        tess_indices,
        pass.width,
        line_cap,
        LineJoin::Round,
        4.0,
        1.0,
    );
    append_tessellated_geometry(
        tess_verts,
        tess_indices,
        stroke_vertices,
        stroke_indices,
        VectorRenderParams {
            color: hex_to_premul_rgba(pass.color, 1.0),
            stroke_mult,
            shape_id: 0.0,
            params: [0.0; 6],
            zbias: *stroke_zbias,
        },
    );
    *stroke_zbias += VECTOR_ZBIAS_STEP;
}

fn expand_polyline_endpoints(points: &[(f32, f32)], _stroke_width: f32) -> Vec<(f32, f32)> {
    points.to_vec()
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

pub fn local_tile_to_lon_lat(tile_key: TileKey, extent: u32, x: i32, y: i32) -> (f64, f64) {
    let extent = extent.max(1) as f64;
    let n = 2.0_f64.powi(tile_key.z as i32);
    let tile_x = tile_key.x as f64 + x as f64 / extent;
    let tile_y = tile_key.y as f64 + y as f64 / extent;
    let lon = tile_x / n * 360.0 - 180.0;
    let lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * tile_y / n))
        .sinh()
        .atan();
    (lon, lat_rad.to_degrees())
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

pub fn tile_clip_rect(tile_key: TileKey, padding: f32) -> (f32, f32, f32, f32) {
    let tile_size = TILE_SIZE as f32;
    (
        tile_key.x as f32 * tile_size - padding,
        tile_key.y as f32 * tile_size - padding,
        (tile_key.x as f32 + 1.0) * tile_size + padding,
        (tile_key.y as f32 + 1.0) * tile_size + padding,
    )
}

pub fn tile_clip_bounds(tile_key: TileKey, padding: f32) -> GeoBounds {
    let (min_x, min_y, max_x, max_y) = tile_clip_rect(tile_key, padding);
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
