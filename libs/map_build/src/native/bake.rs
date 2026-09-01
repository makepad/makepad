//! Baked fill triangulation ("v2-fills-1"): an ADDITIVE companion stream
//! appended to pyramid tiles so the renderer's flat mode can skip both the
//! runtime mesh unifier and earcut for the expensive polygon layers.
//!
//! Contract (agreed with the render side):
//! - The MVT layers keep their full dissolved-ring geometry; every consumer
//!   that ignores the baked stream (drape re-gridding, route/bridge
//!   matching, legacy readers) keeps working unchanged.
//! - The baked stream is appended to the tile as top-level protobuf field
//!   100 (bytes). Conformant MVT/protobuf readers skip unknown fields, so
//!   payload v2 degrades to v1 automatically.
//! - Buildings are never baked (runtime extrusion needs footprint rings).
//! - Only features above a vertex threshold are baked (the superlinear
//!   earcut tail); small polygons stay runtime-tessellated.
//!
//! Stream layout, all varints LEB128, columns per feature:
//! ```text
//! u8      version (=1)
//! varint  baked_feature_count
//! per feature:
//!   varint layer_id       Layer discriminant of the polygon layer
//!   varint feature_index  index into that MVT layer's feature array
//!   varint vertex_count
//!   varint index_count    triangle-strip stream length (incl. restarts)
//!   X column: vertex_count zigzag(delta) varints, tile units, first absolute
//!   Y column: same as X
//!   strip column: index_count zigzag(delta) varints, first absolute;
//!       consecutive entries continue the strip (i0 i1 i2, then each next
//!       index forms a triangle with the previous two, alternating winding);
//!       a repeated index (degenerate) restarts the strip.
//! ```
//! Emitted triangles have positive (y-down clockwise) winding after strip
//! decoding with the standard even/odd alternation.

use super::mvt::{GeometryType, Layer, TileFeature, TilePoint};
use super::schema::ring_area2;
use std::collections::HashMap;

/// Features with at least this many total ring vertices get baked.
pub const BAKE_MIN_VERTICES: usize = 96;

const BAKED_FIELD: u32 = 100;

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

// ---------------------------------------------------------------------------
// Ear clipping with hole bridging
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Vertex {
    x: f64,
    y: f64,
    /// Index into the deduplicated output vertex column.
    column: u32,
}

fn area2_f(a: Vertex, b: Vertex, c: Vertex) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
}

fn point_in_triangle(p: Vertex, a: Vertex, b: Vertex, c: Vertex) -> bool {
    let d1 = area2_f(p, a, b);
    let d2 = area2_f(p, b, c);
    let d3 = area2_f(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// Triangulate one polygon (outer ring + holes, rings stored open, outer
/// positive winding in y-down space). Returns triangles as column indices.
/// None when the polygon defeats the ear clipper (self-intersections from
/// aggressive simplification); the caller simply skips baking it.
fn earcut(
    outer: &[TilePoint],
    holes: &[&[TilePoint]],
    column_of: &mut dyn FnMut(TilePoint) -> u32,
) -> Option<Vec<[u32; 3]>> {
    // Build the working polygon: outer ring plus holes connected via
    // bridges (classic approach: link each hole's rightmost vertex to a
    // visible vertex on the ring built so far).
    let mut polygon: Vec<Vertex> = outer
        .iter()
        .map(|&p| Vertex {
            x: f64::from(p.x),
            y: f64::from(p.y),
            column: column_of(p),
        })
        .collect();
    if polygon.len() < 3 {
        return None;
    }

    let mut holes_sorted: Vec<&[TilePoint]> = holes.to_vec();
    // Bridge right-most holes first.
    holes_sorted.sort_by_key(|hole| {
        std::cmp::Reverse(hole.iter().map(|p| p.x).max().unwrap_or(i32::MIN))
    });
    for hole in holes_sorted {
        if hole.len() < 3 {
            continue;
        }
        let hole_vertices: Vec<Vertex> = hole
            .iter()
            .map(|&p| Vertex {
                x: f64::from(p.x),
                y: f64::from(p.y),
                column: column_of(p),
            })
            .collect();
        // Rightmost hole vertex.
        let hole_start = hole_vertices
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.x.partial_cmp(&b.1.x).unwrap())
            .map(|(index, _)| index)
            .unwrap();
        let hv = hole_vertices[hole_start];
        // Find the polygon vertex to bridge to: the closest vertex with
        // x >= hv.x whose connection segment crosses no polygon edge —
        // approximated by choosing the visible candidate minimizing
        // distance (sufficient for our axis-aligned quilt output; failures
        // are caught by the area check downstream).
        let mut best: Option<(f64, usize)> = None;
        for (index, &pv) in polygon.iter().enumerate() {
            if pv.x < hv.x {
                continue;
            }
            let dx = pv.x - hv.x;
            let dy = pv.y - hv.y;
            let distance = dx * dx + dy * dy;
            let mut visible = true;
            for edge in 0..polygon.len() {
                let a = polygon[edge];
                let b = polygon[(edge + 1) % polygon.len()];
                if edge == index || (edge + 1) % polygon.len() == index {
                    continue;
                }
                if segments_cross(hv, pv, a, b) {
                    visible = false;
                    break;
                }
            }
            if visible && best.map(|(d, _)| distance < d).unwrap_or(true) {
                best = Some((distance, index));
            }
        }
        let bridge = best.map(|(_, index)| index)?;
        // Splice: polygon[..=bridge], hole[start..], hole[..=start], polygon[bridge..]
        let mut next: Vec<Vertex> = Vec::with_capacity(polygon.len() + hole_vertices.len() + 2);
        next.extend_from_slice(&polygon[..=bridge]);
        next.extend(hole_vertices[hole_start..].iter().copied());
        next.extend(hole_vertices[..=hole_start].iter().copied());
        next.extend_from_slice(&polygon[bridge..]);
        polygon = next;
    }

    // Ear clipping over index list.
    let mut indices: Vec<usize> = (0..polygon.len()).collect();
    let mut triangles = Vec::with_capacity(polygon.len().saturating_sub(2));
    let mut guard = 0_usize;
    while indices.len() > 3 {
        let n = indices.len();
        let mut clipped = false;
        let mut cursor = 0;
        while cursor < n {
            let prev = polygon[indices[(cursor + n - 1) % n]];
            let here = polygon[indices[cursor]];
            let next = polygon[indices[(cursor + 1) % n]];
            // Convex corner in positive (y-down clockwise) winding.
            if area2_f(prev, here, next) > 0.0 {
                let mut ear = true;
                for &other in &indices {
                    let candidate = polygon[other];
                    if (candidate.x == prev.x && candidate.y == prev.y)
                        || (candidate.x == here.x && candidate.y == here.y)
                        || (candidate.x == next.x && candidate.y == next.y)
                    {
                        continue;
                    }
                    if point_in_triangle(candidate, prev, here, next) {
                        ear = false;
                        break;
                    }
                }
                if ear {
                    triangles.push([prev.column, here.column, next.column]);
                    indices.remove(cursor);
                    clipped = true;
                    break;
                }
            }
            cursor += 1;
        }
        if !clipped {
            guard += 1;
            if guard > 2 {
                return None; // degenerate leftover; caller skips baking
            }
            // Drop the flattest corner and retry.
            let mut flattest = 0;
            let mut flattest_area = f64::MAX;
            for cursor in 0..indices.len() {
                let n = indices.len();
                let a = polygon[indices[(cursor + n - 1) % n]];
                let b = polygon[indices[cursor]];
                let c = polygon[indices[(cursor + 1) % n]];
                let area = area2_f(a, b, c).abs();
                if area < flattest_area {
                    flattest_area = area;
                    flattest = cursor;
                }
            }
            indices.remove(flattest);
        }
    }
    if indices.len() == 3 {
        let a = polygon[indices[0]];
        let b = polygon[indices[1]];
        let c = polygon[indices[2]];
        if area2_f(a, b, c) > 0.0 {
            triangles.push([a.column, b.column, c.column]);
        }
    }
    Some(triangles)
}

fn segments_cross(a: Vertex, b: Vertex, c: Vertex, d: Vertex) -> bool {
    let d1 = area2_f(c, d, a);
    let d2 = area2_f(c, d, b);
    let d3 = area2_f(a, b, c);
    let d4 = area2_f(a, b, d);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn point_in_ring_tp(point: TilePoint, ring: &[TilePoint]) -> bool {
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

// ---------------------------------------------------------------------------
// Strips
// ---------------------------------------------------------------------------

/// Greedy triangle-strip builder with degenerate restarts. Keeps decoding
/// trivial: i0 i1 i2 start a strip, every following index adds a triangle
/// with the previous two (alternating winding); a repeated index restarts.
fn build_strips(triangles: &[[u32; 3]]) -> Vec<u32> {
    let mut edge_to_triangle: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for edge in 0..3 {
            let a = triangle[edge];
            let b = triangle[(edge + 1) % 3];
            let key = if a < b { (a, b) } else { (b, a) };
            edge_to_triangle.entry(key).or_default().push(index);
        }
    }
    let mut used = vec![false; triangles.len()];
    let mut out: Vec<u32> = Vec::with_capacity(triangles.len() * 2);
    for start in 0..triangles.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let [a, b, c] = triangles[start];
        // The decoder derives winding from the ABSOLUTE window index, so
        // the encoder simulates it exactly. First pick the start triple
        // orientation for the window position it will land on.
        let start_is_odd = |len: usize| len % 2 == 1;
        let triple_at = |odd: bool| if odd { [b, a, c] } else { [a, b, c] };
        if !out.is_empty() {
            // Degenerate restart: repeat the previous index, then the new
            // strip's first index (chosen after parity is known).
            let last = *out.last().unwrap();
            out.push(last);
            let odd = start_is_odd(out.len() + 1);
            out.push(triple_at(odd)[0]);
        }
        let triple = triple_at(start_is_odd(out.len()));
        out.extend_from_slice(&triple);
        loop {
            let n = out.len();
            let p = out[n - 2];
            let q = out[n - 1];
            let key = if p < q { (p, q) } else { (q, p) };
            let Some(candidates) = edge_to_triangle.get(&key) else {
                break;
            };
            let Some(&next) = candidates.iter().find(|&&t| !used[t]) else {
                break;
            };
            let triangle = triangles[next];
            let Some(apex) = triangle
                .iter()
                .copied()
                .find(|&vertex| vertex != p && vertex != q)
            else {
                break;
            };
            // The next window starts at n-2; check the decoded winding.
            let decoded = if (n - 2) % 2 == 1 {
                [q, p, apex]
            } else {
                [p, q, apex]
            };
            if !same_triangle(decoded, triangle) {
                break;
            }
            used[next] = true;
            out.push(apex);
        }
    }
    out
}

fn same_triangle(a: [u32; 3], b: [u32; 3]) -> bool {
    // Same cyclic order (winding-preserving equality).
    (0..3).any(|shift| {
        a[0] == b[shift] && a[1] == b[(shift + 1) % 3] && a[2] == b[(shift + 2) % 3]
    })
}

/// Decode a strip stream back to triangles (used by tests and verification).
pub fn strip_to_triangles(strip: &[u32]) -> Vec<[u32; 3]> {
    let mut out = Vec::new();
    let mut odd = false;
    for window in strip.windows(3) {
        let [a, b, c] = [window[0], window[1], window[2]];
        if a != b && b != c && a != c {
            out.push(if odd { [b, a, c] } else { [a, b, c] });
        }
        odd = !odd;
    }
    out
}

// ---------------------------------------------------------------------------
// Tile-level baking
// ---------------------------------------------------------------------------

struct BakedFeature {
    layer: Layer,
    feature_index: u64,
    vertices: Vec<TilePoint>,
    strip: Vec<u32>,
}

/// Bake fills for the qualifying polygon features of a finalized pyramid
/// tile, returning the complete protobuf field-100 bytes to append after
/// `encode_tile` output (None when nothing qualifies). `features` must be
/// the exact list passed to `encode_tile` (feature indices join on
/// per-layer order; encode_tile groups by layer but preserves per-layer
/// input order, replicated here with a stable sort).
pub fn baked_fills_field(features: &[TileFeature]) -> Result<Option<Vec<u8>>, String> {
    let mut per_layer_index: HashMap<u8, u64> = HashMap::new();
    let mut baked: Vec<BakedFeature> = Vec::new();
    // encode_tile groups features into layers by Layer enum order but keeps
    // per-layer feature order; replicate that ordering contract here.
    let mut ordered: Vec<&TileFeature> = features.iter().collect();
    ordered.sort_by_key(|feature| feature.layer as u8);
    for feature in ordered {
        let layer_slot = per_layer_index.entry(feature.layer as u8).or_insert(0);
        let feature_index = *layer_slot;
        *layer_slot += 1;
        if feature.geometry_type != GeometryType::Polygon {
            continue;
        }
        if !matches!(
            feature.layer,
            Layer::BaseWaterPolygons | Layer::BaseLand | Layer::BaseStreetPolygons
        ) {
            continue;
        }
        let total_vertices: usize = feature.paths.iter().map(Vec::len).sum();
        if total_vertices < BAKE_MIN_VERTICES {
            continue;
        }
        if let Some(bake) = bake_feature(feature, feature_index) {
            baked.push(bake);
        }
    }
    if baked.is_empty() {
        return Ok(None);
    }
    let mut blob = Vec::new();
    blob.push(1_u8);
    write_varint(baked.len() as u64, &mut blob);
    for bake in &baked {
        write_varint(u64::from(bake.layer as u8), &mut blob);
        write_varint(bake.feature_index, &mut blob);
        write_varint(bake.vertices.len() as u64, &mut blob);
        write_varint(bake.strip.len() as u64, &mut blob);
        let mut previous = 0_i64;
        for vertex in &bake.vertices {
            write_varint(zigzag(i64::from(vertex.x) - previous), &mut blob);
            previous = i64::from(vertex.x);
        }
        previous = 0;
        for vertex in &bake.vertices {
            write_varint(zigzag(i64::from(vertex.y) - previous), &mut blob);
            previous = i64::from(vertex.y);
        }
        previous = 0;
        for &index in &bake.strip {
            write_varint(zigzag(i64::from(index) - previous), &mut blob);
            previous = i64::from(index);
        }
    }
    // Top-level field 100, wire type 2: skipped by conformant readers.
    let mut field = Vec::with_capacity(blob.len() + 8);
    write_varint(u64::from(BAKED_FIELD) << 3 | 2, &mut field);
    write_varint(blob.len() as u64, &mut field);
    field.extend_from_slice(&blob);
    Ok(Some(field))
}

/// Strict segment self-intersection test (bowtie detector). O(n²) but rings
/// are short after simplification and this runs at conversion time only.
fn ring_self_intersects(ring: &[TilePoint]) -> bool {
    let n = ring.len();
    if n < 4 {
        return false;
    }
    // Repeated vertices (pinch) count as self-intersection: upstream splits
    // them, so reaching here means something slipped through — don't bake.
    let mut seen = std::collections::HashSet::with_capacity(n);
    for point in ring {
        if !seen.insert((point.x, point.y)) {
            return true;
        }
    }
    let vertex = |index: usize| {
        let p = ring[index % n];
        Vertex {
            x: f64::from(p.x),
            y: f64::from(p.y),
            column: 0,
        }
    };
    for first in 0..n {
        for second in first + 2..n {
            // Skip adjacent segments (they share a vertex by construction).
            if first == 0 && second == n - 1 {
                continue;
            }
            if segments_cross(
                vertex(first),
                vertex(first + 1),
                vertex(second),
                vertex(second + 1),
            ) {
                return true;
            }
        }
    }
    false
}

fn bake_feature(feature: &TileFeature, feature_index: u64) -> Option<BakedFeature> {
    // Self-intersecting (bowtie) rings — possible from aggressive
    // simplification — tessellate ambiguously (runtime fills both lobes,
    // a bake covers the net); leave such features to the runtime path.
    if feature.paths.iter().any(|ring| ring_self_intersects(ring)) {
        return None;
    }
    // Group rings: each positive ring is an outer; each negative ring is
    // assigned to the smallest containing outer.
    let mut outers: Vec<(usize, i64)> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    for (index, ring) in feature.paths.iter().enumerate() {
        let area2 = ring_area2(ring);
        if area2 > 0 {
            outers.push((index, area2));
        } else if area2 < 0 {
            holes.push(index);
        }
    }
    if outers.is_empty() {
        return None;
    }
    let mut holes_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for &hole in &holes {
        let probe = feature.paths[hole][0];
        let owner = outers
            .iter()
            .filter(|&&(outer, _)| point_in_ring_tp(probe, &feature.paths[outer]))
            .min_by_key(|&&(_, area2)| area2)
            .map(|&(outer, _)| outer);
        if let Some(owner) = owner {
            holes_of.entry(owner).or_default().push(hole);
        }
    }

    let mut vertices: Vec<TilePoint> = Vec::new();
    let mut column_index: HashMap<(i32, i32), u32> = HashMap::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut expected_area2 = 0_i64;
    for &(outer, outer_area2) in &outers {
        let hole_indices = holes_of.get(&outer).cloned().unwrap_or_default();
        let hole_slices: Vec<&[TilePoint]> = hole_indices
            .iter()
            .map(|&hole| feature.paths[hole].as_slice())
            .collect();
        expected_area2 += outer_area2;
        for &hole in &hole_indices {
            expected_area2 += ring_area2(&feature.paths[hole]); // negative
        }
        let mut column_of = |point: TilePoint| -> u32 {
            *column_index.entry((point.x, point.y)).or_insert_with(|| {
                vertices.push(point);
                (vertices.len() - 1) as u32
            })
        };
        let piece = earcut(&feature.paths[outer], &hole_slices, &mut column_of)?;
        triangles.extend(piece);
    }
    // Area audit: the triangles must cover the polygon's net area; if the
    // clipper mangled anything, skip baking (renderer falls back).
    let mut triangle_area2 = 0_i64;
    for &[a, b, c] in &triangles {
        let (a, b, c) = (
            vertices[a as usize],
            vertices[b as usize],
            vertices[c as usize],
        );
        triangle_area2 += (i64::from(b.x) - i64::from(a.x)) * (i64::from(c.y) - i64::from(a.y))
            - (i64::from(c.x) - i64::from(a.x)) * (i64::from(b.y) - i64::from(a.y));
    }
    let tolerance = (expected_area2.abs() / 128).max(64);
    if (triangle_area2 - expected_area2).abs() > tolerance {
        return None;
    }
    let strip = build_strips(&triangles);
    // Round-trip audit of the strip encoding.
    let decoded = strip_to_triangles(&strip);
    if decoded.len() != triangles.len() {
        return None;
    }
    Some(BakedFeature {
        layer: feature.layer,
        feature_index,
        vertices,
        strip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::mvt::OsmType;

    fn polygon(layer: Layer, rings: Vec<Vec<TilePoint>>) -> TileFeature {
        TileFeature {
            layer,
            geometry_type: GeometryType::Polygon,
            osm_type: OsmType::Way,
            id: 2,
            closed: true,
            tags: Vec::new(),
            paths: rings,
        }
    }

    fn rect(x0: i32, y0: i32, w: i32, h: i32) -> Vec<TilePoint> {
        vec![
            TilePoint { x: x0, y: y0 },
            TilePoint { x: x0 + w, y: y0 },
            TilePoint { x: x0 + w, y: y0 + h },
            TilePoint { x: x0, y: y0 + h },
        ]
    }

    #[test]
    fn bakes_rect_with_hole_and_area_matches() {
        let outer = rect(0, 0, 100, 100);
        let mut hole = rect(40, 40, 20, 20);
        hole.reverse(); // negative winding
        let feature = polygon(Layer::BaseWaterPolygons, vec![outer, hole]);
        let baked = bake_feature(&feature, 0).expect("bake");
        let decoded = strip_to_triangles(&baked.strip);
        let mut area2 = 0_i64;
        for [a, b, c] in decoded {
            let (a, b, c) = (
                baked.vertices[a as usize],
                baked.vertices[b as usize],
                baked.vertices[c as usize],
            );
            let t = (i64::from(b.x) - i64::from(a.x)) * (i64::from(c.y) - i64::from(a.y))
                - (i64::from(c.x) - i64::from(a.x)) * (i64::from(b.y) - i64::from(a.y));
            assert!(t > 0, "triangle winding must be positive");
            area2 += t;
        }
        assert_eq!(area2, 2 * (100 * 100 - 20 * 20));
    }

    #[test]
    fn baked_blob_appends_as_ignorable_field() {
        let big: Vec<TilePoint> = (0..40)
            .map(|i| {
                let angle = i as f64 / 40.0 * std::f64::consts::TAU;
                TilePoint {
                    x: (2000.0 + 900.0 * angle.cos()) as i32,
                    // y-down clockwise for positive winding
                    y: (2000.0 + 900.0 * angle.sin()) as i32,
                }
            })
            .collect();
        // 3 rings x 40 vertices exceeds BAKE_MIN_VERTICES.
        let shifted: Vec<Vec<TilePoint>> = (0..3)
            .map(|k| {
                big.iter()
                    .map(|p| TilePoint { x: p.x / 4 + k * 1200, y: p.y / 4 })
                    .collect()
            })
            .collect();
        let features = vec![polygon(Layer::BaseLand, shifted)];
        let mut mvt = super::super::mvt::encode_tile(features.clone()).unwrap();
        let before = mvt.len();
        let field = baked_fills_field(&features).unwrap().expect("baked");
        mvt.extend_from_slice(&field);
        assert!(mvt.len() > before);
        // The existing inspection walker must still parse the tile.
        let inspected = super::super::mvt::inspect_tile(&mvt).unwrap();
        assert_eq!(inspected.layers.len(), 1);
        assert_eq!(inspected.layers[0].name, "land");
    }
}
