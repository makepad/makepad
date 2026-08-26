//! Coplanar triangle overlap analysis and render-only conflict resolution.
//!
//! This module deliberately reads the canonical [`RenderBatch`] triangle
//! soup but never rewrites it. It finds near-coplanar pairs through adjacent
//! plane bins and a projected-AABB grid, then measures their actual projected
//! triangle intersection area. The resulting part priorities are metadata for
//! renderers; picking, snapping, measurement, contours and sheets continue to
//! use the source geometry byte-for-byte.

use crate::model::batch::RenderBatch;
use crate::model::ids::{ElementId, MaterialId};
use crate::model::model::{ElementClass, PropertyValue};
use crate::model::scene::{Element, Material, ScenePart};
use makepad_math::Vec3f;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const DEFAULT_COPLANAR_TOL_M: f32 = 0.002;
const NORMAL_DOT_TOL: f64 = 1.0e-5;
// sqrt(2 * NORMAL_DOT_TOL), rounded upward: any normals that pass the dot
// test differ by at most one bin in each component.
const NORMAL_BIN: f64 = 0.0045;
// A near-tie can swap the dominant projection axis at both ends of the
// allowed normal delta. Inserting both axes keeps candidate generation whole.
const PROJECTION_TIE_TOL: f64 = NORMAL_BIN * 2.0;
const PROJECTED_CELL_M: f64 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct OverlapRecord {
    pub element_a: ElementId,
    pub element_a_name: String,
    pub part_a: u32,
    pub element_part_a: u32,
    pub material_a: MaterialId,
    pub material_a_name: String,
    pub element_b: ElementId,
    pub element_b_name: String,
    pub part_b: u32,
    pub element_part_b: u32,
    pub material_b: MaterialId,
    pub material_b_name: String,
    pub triangle_pairs: u32,
    pub area_m2: f64,
    /// An interior point and normal from the largest contributing triangle
    /// intersection. Useful for deterministic diagnostics and render tests.
    pub sample_point_m: [f64; 3],
    pub sample_normal: [f64; 3],
    pub draw_priority_a: u16,
    pub draw_priority_b: u16,
    pub coplanar_group: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlapReport {
    pub tolerance_m: f32,
    pub triangle_pairs: u64,
    pub area_m2: f64,
    pub pairs: Vec<OverlapRecord>,
}

impl OverlapReport {
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// A deterministic, human-readable element/part overlap table.
    pub fn table(&self) -> String {
        let mut out = String::from(
            "element A / part / material | element B / part / material | tri pairs | area m2 | priority A/B | group\n",
        );
        for r in &self.pairs {
            out.push_str(&format!(
                "{} [{}] / {} / {} | {} [{}] / {} / {} | {} | {:.6} | {}/{} | {}\n",
                r.element_a_name,
                r.element_a.0,
                r.element_part_a,
                r.material_a_name,
                r.element_b_name,
                r.element_b.0,
                r.element_part_b,
                r.material_b_name,
                r.triangle_pairs,
                r.area_m2,
                r.draw_priority_a,
                r.draw_priority_b,
                r.coplanar_group,
            ));
        }
        out
    }
}

#[derive(Clone, Copy, Debug)]
struct TriRec {
    p: [[f64; 3]; 3],
    n: [f64; 3],
    d: f64,
    part: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PlaneKey {
    d: i64,
    drop: u8,
}

type Cell = (i64, i64);

fn v3(p: Vec3f) -> [f64; 3] {
    [p.x as f64, p.y as f64, p.z as f64]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn canonical_normal(p: [[f64; 3]; 3]) -> Option<[f64; 3]> {
    let mut n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    let len = dot(n, n).sqrt();
    if len <= 1.0e-12 {
        return None;
    }
    for x in &mut n {
        *x /= len;
    }
    // A lexicographic hemisphere rule is stable when two components tie for
    // dominance (roof diagonals are common). Using the dominant component's
    // sign can flip otherwise identical normals across that tie boundary.
    let sign_component = n.iter().copied().find(|v| v.abs() > 1.0e-12).unwrap_or(1.0);
    if sign_component < 0.0 {
        n = [-n[0], -n[1], -n[2]];
    }
    Some(n)
}

fn projection_axes(drop: usize) -> (usize, usize) {
    match drop {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

fn eligible_projections(n: [f64; 3]) -> impl Iterator<Item = usize> {
    let m = n[0].abs().max(n[1].abs()).max(n[2].abs());
    (0..3).filter(move |&i| m - n[i].abs() <= PROJECTION_TIE_TOL)
}

fn projected_aabb(t: &TriRec, drop: usize, expand: f64) -> ([f64; 2], [f64; 2]) {
    let (u, v) = projection_axes(drop);
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for p in t.p {
        lo[0] = lo[0].min(p[u]);
        lo[1] = lo[1].min(p[v]);
        hi[0] = hi[0].max(p[u]);
        hi[1] = hi[1].max(p[v]);
    }
    ([lo[0] - expand, lo[1] - expand], [hi[0] + expand, hi[1] + expand])
}

fn cells(aabb: ([f64; 2], [f64; 2])) -> impl Iterator<Item = Cell> {
    let x0 = (aabb.0[0] / PROJECTED_CELL_M).floor() as i64;
    let y0 = (aabb.0[1] / PROJECTED_CELL_M).floor() as i64;
    let x1 = (aabb.1[0] / PROJECTED_CELL_M).floor() as i64;
    let y1 = (aabb.1[1] / PROJECTED_CELL_M).floor() as i64;
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

fn quantize(x: f64, cell: f64) -> i64 {
    (x / cell).floor() as i64
}

fn plane_height_bin(tol: f64) -> f64 {
    // Both triangles touch the same expanded 2D cell. Re-evaluating the plane
    // at that cell's lower corner bounds the normal-delta term by the cell
    // size instead of the radius of the entire (possibly georeferenced)
    // building. Eligible projection axes have |n| >= ~0.568; 0.55 is a
    // conservative divisor including the projection-tie allowance.
    (tol + 2.0 * NORMAL_BIN * (PROJECTED_CELL_M + tol)) / 0.55
}

fn plane_key(
    t: &TriRec,
    drop: usize,
    cell: Cell,
    origin: [f64; 3],
    height_bin: f64,
) -> PlaneKey {
    let (u, v) = projection_axes(drop);
    let q_u = cell.0 as f64 * PROJECTED_CELL_M;
    let q_v = cell.1 as f64 * PROJECTED_CELL_M;
    let height = -(t.d
        + t.n[u] * (q_u - origin[u])
        + t.n[v] * (q_v - origin[v]))
        / t.n[drop];
    PlaneKey {
        d: quantize(height, height_bin),
        drop: drop as u8,
    }
}

fn neighbouring_keys(k: PlaneKey) -> impl Iterator<Item = PlaneKey> {
    (-1..=1).map(move |d| PlaneKey {
        d: k.d + d,
        drop: k.drop,
    })
}

fn max_plane_separation(a: &TriRec, b: &TriRec) -> (f64, f64) {
    let ab = b
        .p
        .iter()
        .map(|&p| dot(a.n, sub(p, a.p[0])).abs())
        .fold(0.0, f64::max);
    let ba = a
        .p
        .iter()
        .map(|&p| dot(b.n, sub(p, b.p[0])).abs())
        .fold(0.0, f64::max);
    (ab, ba)
}

fn signed_area(poly: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        a += p[0] * q[1] - p[1] * q[0];
    }
    a * 0.5
}

fn cross2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Sutherland-Hodgman clipping of one projected triangle by the other.
fn intersection_polygon_2d(a: [[f64; 2]; 3], b: [[f64; 2]; 3]) -> Vec<[f64; 2]> {
    let winding = if signed_area(&b) >= 0.0 { 1.0 } else { -1.0 };
    let mut poly = a.to_vec();
    for i in 0..3 {
        let c0 = b[i];
        let c1 = b[(i + 1) % 3];
        let edge = [c1[0] - c0[0], c1[1] - c0[1]];
        let side = |p: [f64; 2]| {
            winding * cross2(edge, [p[0] - c0[0], p[1] - c0[1]])
        };
        let input = std::mem::take(&mut poly);
        if input.is_empty() {
            break;
        }
        let mut prev = *input.last().unwrap();
        let mut prev_side = side(prev);
        for cur in input {
            let cur_side = side(cur);
            let prev_in = prev_side >= -1.0e-12;
            let cur_in = cur_side >= -1.0e-12;
            if prev_in != cur_in {
                let denom = prev_side - cur_side;
                if denom.abs() > 1.0e-20 {
                    let t = prev_side / denom;
                    poly.push([
                        prev[0] + (cur[0] - prev[0]) * t,
                        prev[1] + (cur[1] - prev[1]) * t,
                    ]);
                }
            }
            if cur_in {
                poly.push(cur);
            }
            prev = cur;
            prev_side = cur_side;
        }
    }
    poly
}

fn polygon_centroid(poly: &[[f64; 2]]) -> [f64; 2] {
    let mut twice_area = 0.0;
    let mut c = [0.0; 2];
    for i in 0..poly.len() {
        let p = poly[i];
        let q = poly[(i + 1) % poly.len()];
        let cross = p[0] * q[1] - q[0] * p[1];
        twice_area += cross;
        c[0] += (p[0] + q[0]) * cross;
        c[1] += (p[1] + q[1]) * cross;
    }
    if twice_area.abs() <= 1.0e-20 {
        return poly.iter().fold([0.0; 2], |a, p| [a[0] + p[0], a[1] + p[1]])
            .map(|v| v / poly.len().max(1) as f64);
    }
    [c[0] / (3.0 * twice_area), c[1] / (3.0 * twice_area)]
}

fn projected_intersection(a: &TriRec, b: &TriRec, drop: usize) -> (f64, [f64; 3]) {
    let (u, v) = projection_axes(drop);
    let pa = a.p.map(|p| [p[u], p[v]]);
    let pb = b.p.map(|p| [p[u], p[v]]);
    let poly = intersection_polygon_2d(pa, pb);
    if poly.len() < 3 {
        return (0.0, [0.0; 3]);
    }
    let projected = signed_area(&poly).abs();
    let c = polygon_centroid(&poly);
    let mut point = [0.0; 3];
    point[u] = c[0];
    point[v] = c[1];
    point[drop] = a.p[0][drop]
        - (a.n[u] * (point[u] - a.p[0][u]) + a.n[v] * (point[v] - a.p[0][v]))
            / a.n[drop];
    (projected / a.n[drop].abs().max(1.0e-12), point)
}

#[cfg(test)]
fn projected_intersection_area(a: &TriRec, b: &TriRec, drop: usize) -> f64 {
    projected_intersection(a, b, drop).0
}

fn triangles_from_batches(batches: &[RenderBatch], origin: [f64; 3]) -> Vec<TriRec> {
    let mut out = Vec::new();
    for batch in batches {
        for range in &batch.element_ranges {
            let first = range.first_index / 3;
            let end = (range.first_index + range.index_count) / 3;
            for triangle in first..end {
                let (a, b, c) = batch.triangle(triangle);
                let p = [v3(a), v3(b), v3(c)];
                let Some(n) = canonical_normal(p) else {
                    continue;
                };
                let d = -dot(n, sub(p[0], origin));
                out.push(TriRec {
                    p,
                    n,
                    d,
                    part: range.part,
                });
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
struct Aggregate {
    triangle_pairs: u32,
    area_m2: f64,
    sample_area_m2: f64,
    sample_point_m: [f64; 3],
    sample_normal: [f64; 3],
}

/// Analyze all canonical scene triangles. Candidate generation uses adjacent
/// plane bins plus every cell touched by each tolerance-expanded projected
/// AABB; exact duplicates and edge-sharing triangles therefore reach the area
/// test, where only positive-area intersections survive.
pub fn analyze(
    batches: &[RenderBatch],
    parts: &[ScenePart],
    elements: &[Element],
    materials: &[Material],
    tolerance_m: f32,
) -> OverlapReport {
    let tol = tolerance_m.max(1.0e-7) as f64;
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for batch in batches {
        for vi in 0..batch.vertex_count() as u32 {
            let p = v3(batch.position(vi));
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let origin = if lo[0].is_finite() {
        [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5]
    } else {
        [0.0; 3]
    };
    let height_bin = plane_height_bin(tol);
    let tris = triangles_from_batches(batches, origin);
    let mut grid: HashMap<PlaneKey, HashMap<Cell, Vec<usize>>> = HashMap::new();
    let mut aggregate: BTreeMap<(u32, u32), Aggregate> = BTreeMap::new();

    for (i, tri) in tris.iter().enumerate() {
        let mut seen = HashSet::new();
        for drop in eligible_projections(tri.n) {
            let tri_cells: Vec<Cell> = cells(projected_aabb(tri, drop, tol)).collect();
            for &cell in &tri_cells {
                let key = plane_key(tri, drop, cell, origin, height_bin);
                for nk in neighbouring_keys(key) {
                    let Some(plane) = grid.get(&nk) else { continue };
                    let Some(candidates) = plane.get(&cell) else { continue };
                    for &j in candidates {
                        if !seen.insert(j) {
                            continue;
                        }
                        let other = &tris[j];
                        if 1.0 - dot(tri.n, other.n).abs() > NORMAL_DOT_TOL {
                            continue;
                        }
                        let (ab, ba) = max_plane_separation(tri, other);
                        if ab > tol || ba > tol {
                            continue;
                        }
                        let (area, sample_point_m) = projected_intersection(tri, other, drop);
                        // Contacts smaller than the square of the positional
                        // tolerance are indistinguishable from a shared edge
                        // after projection.  Keeping them would connect large
                        // buildings into one conflict group through numerical
                        // slivers, even though they cannot produce a visible
                        // coplanar patch.
                        if area <= (tol * tol).max(1.0e-12) {
                            continue;
                        }
                        let key = if tri.part <= other.part {
                            (tri.part, other.part)
                        } else {
                            (other.part, tri.part)
                        };
                        let a = aggregate.entry(key).or_default();
                        a.triangle_pairs += 1;
                        a.area_m2 += area;
                        if area > a.sample_area_m2 {
                            a.sample_area_m2 = area;
                            a.sample_point_m = sample_point_m;
                            a.sample_normal = tri.n;
                        }
                    }
                }
                grid.entry(key).or_default().entry(cell).or_default().push(i);
            }
        }
    }

    let mut report = OverlapReport {
        tolerance_m,
        ..Default::default()
    };
    for ((part_a, part_b), a) in aggregate {
        let Some(pa) = parts.get(part_a as usize) else { continue };
        let Some(pb) = parts.get(part_b as usize) else { continue };
        let ea = elements.get(pa.element.index());
        let eb = elements.get(pb.element.index());
        let ma = materials.get(pa.material.index());
        let mb = materials.get(pb.material.index());
        report.triangle_pairs += a.triangle_pairs as u64;
        report.area_m2 += a.area_m2;
        report.pairs.push(OverlapRecord {
            element_a: pa.element,
            element_a_name: ea.map(|e| e.name.clone()).unwrap_or_default(),
            part_a,
            element_part_a: pa.element_part,
            material_a: pa.material,
            material_a_name: ma.map(|m| m.name.clone()).unwrap_or_default(),
            element_b: pb.element,
            element_b_name: eb.map(|e| e.name.clone()).unwrap_or_default(),
            part_b,
            element_part_b: pb.element_part,
            material_b: pb.material,
            material_b_name: mb.map(|m| m.name.clone()).unwrap_or_default(),
            triangle_pairs: a.triangle_pairs,
            area_m2: a.area_m2,
            sample_point_m: a.sample_point_m,
            sample_normal: a.sample_normal,
            draw_priority_a: 0,
            draw_priority_b: 0,
            coplanar_group: 0,
        });
    }
    report
}

struct UnionFind {
    parent: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n as u32).collect() }
    }
    fn root(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let p = self.parent[x as usize];
            self.parent[x as usize] = self.parent[p as usize];
            x = self.parent[x as usize];
        }
        x
    }
    fn join(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra != rb {
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[hi as usize] = lo;
        }
    }
}

fn semantic_tier(element: Option<&Element>) -> u16 {
    let Some(e) = element else { return 2 };
    if let Some(priority) = e.properties.iter().find_map(|property| {
        if property.name != "arch.priority" {
            return None;
        }
        match property.value {
            PropertyValue::Number(value) => Some(value),
            PropertyValue::Integer(value) => Some(value as f64),
            _ => None,
        }
    }) {
        return priority.clamp(0.0, 4095.0).round() as u16;
    }
    let name = e.name.to_ascii_lowercase();
    if matches!(e.class, ElementClass::Roof | ElementClass::Shell) || name.contains("roof") {
        4
    } else if matches!(e.class, ElementClass::Beam)
        || ["framing", "joist", "rafter", "batten", "purlin"]
            .iter()
            .any(|s| name.contains(s))
    {
        1
    } else {
        2
    }
}

/// Assign a unique priority within each connected coplanar part group.
/// Semantic order is roof skin > ordinary opaque > framing, with opaque over
/// glass inside a tier; source element/part ids provide the stable final key.
pub(crate) fn resolve(
    report: &mut OverlapReport,
    parts: &mut [ScenePart],
    elements: &[Element],
    materials: &[Material],
) {
    let mut uf = UnionFind::new(parts.len());
    let mut active = HashSet::new();
    let mut neighbours: HashMap<u32, Vec<u32>> = HashMap::new();
    for r in &report.pairs {
        if (r.part_a as usize) < parts.len() && (r.part_b as usize) < parts.len() {
            uf.join(r.part_a, r.part_b);
            active.insert(r.part_a);
            active.insert(r.part_b);
            if r.part_a != r.part_b {
                neighbours.entry(r.part_a).or_default().push(r.part_b);
                neighbours.entry(r.part_b).or_default().push(r.part_a);
            }
        }
    }
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for p in active {
        groups.entry(uf.root(p)).or_default().push(p);
    }
    for (group_index, members) in groups.values_mut().enumerate() {
        members.sort_by_key(|&p| {
            let part = &parts[p as usize];
            let tier = semantic_tier(elements.get(part.element.index()));
            let opaque = materials
                .get(part.material.index())
                .map(|m| !m.transparent)
                .unwrap_or(true);
            (tier, opaque as u8, part.element.0, part.element_part, p)
        });
        let group = group_index as u32 + 1;
        for &p in members.iter() {
            // The total semantic order orients every conflict edge. A part
            // only needs to outrank its directly overlapping predecessors,
            // not every unrelated member of a large connected component.
            // Longest-path ranks keep the GPU value compact while preserving
            // a strict, deterministic winner for every reported pair.
            let rank = neighbours
                .get(&p)
                .into_iter()
                .flatten()
                .filter_map(|&q| parts.get(q as usize).map(|part| part.draw_priority))
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            parts[p as usize].coplanar_group = group;
            parts[p as usize].draw_priority = rank;
        }
    }
    for r in &mut report.pairs {
        let pa = parts.get(r.part_a as usize);
        let pb = parts.get(r.part_b as usize);
        r.draw_priority_a = pa.map(|p| p.draw_priority).unwrap_or(0);
        r.draw_priority_b = pb.map(|p| p.draw_priority).unwrap_or(0);
        r.coplanar_group = pa.map(|p| p.coplanar_group).unwrap_or(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::batch::{ElementRange, Vertex};
    use crate::model::ids::MeshId;

    fn tri(p: [[f64; 3]; 3], part: u32) -> TriRec {
        let n = canonical_normal(p).unwrap();
        TriRec {
            p,
            n,
            d: -dot(n, p[0]),
            part,
        }
    }

    fn overlap(a: &TriRec, b: &TriRec, tol: f64) -> f64 {
        let (ab, ba) = max_plane_separation(a, b);
        if 1.0 - dot(a.n, b.n).abs() > NORMAL_DOT_TOL || ab > tol || ba > tol {
            return 0.0;
        }
        let drop = eligible_projections(a.n).next().unwrap();
        projected_intersection_area(a, b, drop)
    }

    fn analyze_pair(a: [[f64; 3]; 3], b: [[f64; 3]; 3], shared_vertices: bool) -> OverlapReport {
        let mut batch = RenderBatch { material: MaterialId(0), ..Default::default() };
        for p in a {
            batch.push_vertex(&Vertex {
                position: p.map(|v| v as f32),
                normal: [0.0, 0.0, 1.0],
                ..Default::default()
            });
        }
        batch.indices.extend([0, 1, 2]);
        if shared_vertices {
            batch.indices.extend([2, 1, 0]);
        } else {
            for p in b {
                batch.push_vertex(&Vertex {
                    position: p.map(|v| v as f32),
                    normal: [0.0, 0.0, 1.0],
                    ..Default::default()
                });
            }
            batch.indices.extend([3, 4, 5]);
        }
        batch.element_ranges = vec![
            ElementRange {
                element: ElementId(0),
                part: 0,
                first_index: 0,
                index_count: 3,
                draw_priority: 0,
                coplanar_group: 0,
            },
            ElementRange {
                element: ElementId(1),
                part: 1,
                first_index: 3,
                index_count: 3,
                draw_priority: 0,
                coplanar_group: 0,
            },
        ];
        let parts = vec![
            ScenePart {
                id: 0,
                element: ElementId(0),
                element_part: 0,
                mesh: MeshId(0),
                material: MaterialId(0),
                batch: 0,
                first_index: 0,
                index_count: 3,
                draw_priority: 0,
                coplanar_group: 0,
            },
            ScenePart {
                id: 1,
                element: ElementId(1),
                element_part: 0,
                mesh: MeshId(1),
                material: MaterialId(0),
                batch: 0,
                first_index: 3,
                index_count: 3,
                draw_priority: 0,
                coplanar_group: 0,
            },
        ];
        analyze(&[batch], &parts, &[], &[], DEFAULT_COPLANAR_TOL_M)
    }

    #[test]
    fn exact_duplicates_are_not_discarded_for_shared_vertices() {
        let a = tri([[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]], 0);
        let b = tri([[0.0, 2.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 0.0]], 1);
        assert!((overlap(&a, &b, 0.002) - 2.0).abs() < 1.0e-10);
        let report = analyze_pair(a.p, b.p, true);
        assert_eq!(report.pairs.len(), 1);
        assert!((report.area_m2 - 2.0).abs() < 1.0e-9);
    }

    #[test]
    fn partial_overlap_measures_intersection_area() {
        let a = tri([[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]], 0);
        let b = tri([[0.5, 0.5, 0.0], [1.5, 0.5, 0.0], [0.5, 1.5, 0.0]], 1);
        assert!((overlap(&a, &b, 0.002) - 0.5).abs() < 1.0e-10);
        assert!((analyze_pair(a.p, b.p, false).area_m2 - 0.5).abs() < 1.0e-9);
    }

    #[test]
    fn adjacent_shared_edge_has_zero_overlap_area() {
        let a = tri([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], 0);
        let b = tri([[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], 1);
        assert!(overlap(&a, &b, 0.002) < 1.0e-12);
        assert!(analyze_pair(a.p, b.p, false).is_empty());
    }

    #[test]
    fn both_directional_plane_separations_must_pass() {
        let a = tri([[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]], 0);
        let b = tri([[0.0, 0.0, 0.001], [100.0, 0.0, 0.003], [0.0, 100.0, 0.001]], 1);
        let (ab, ba) = max_plane_separation(&a, &b);
        assert!(ab > 0.002 && ba > 0.002);
        assert_eq!(overlap(&a, &b, 0.002), 0.0);
    }

    #[test]
    fn neighbouring_plane_bins_cover_a_quantization_boundary() {
        let height_bin = plane_height_bin(0.002);
        let za = height_bin - 1.0e-5;
        let zb = height_bin + 1.0e-5;
        let a = tri([[0.0, 0.0, za], [1.0, 0.0, za], [0.0, 1.0, za]], 0);
        let b = tri([[0.0, 0.0, zb], [1.0, 0.0, zb], [0.0, 1.0, zb]], 1);
        let cell = (0, 0);
        let ka = plane_key(&a, 2, cell, [0.0; 3], height_bin);
        let kb = plane_key(&b, 2, cell, [0.0; 3], height_bin);
        assert_ne!(ka.d, kb.d, "fixture must straddle a plane bucket");
        assert!(neighbouring_keys(ka).any(|k| k == kb));
        assert!((overlap(&a, &b, 0.002) - 0.5).abs() < 1.0e-10);
        assert_eq!(analyze_pair(a.p, b.p, false).pairs.len(), 1);
    }

    #[test]
    fn projected_aabb_index_finds_pairs_with_distant_centroids() {
        let a = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]];
        let b = [[98.0, 0.5, 0.0], [99.0, 0.5, 0.0], [98.0, 1.5, 0.0]];
        let report = analyze_pair(a, b, false);
        assert_eq!(report.pairs.len(), 1);
        assert!((report.area_m2 - 0.5).abs() < 1.0e-9);
    }
}
