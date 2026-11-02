//! The two unwraps this crate replaced, on triangle soup, kept for the old
//! versus new numbers of one report. Faithful in result; array-based here where
//! the originals used ordered maps, so their times here flatter them.

use crate::{Layout, Mesh};
use std::collections::HashMap;
use std::time::Instant;

fn normal(mesh: &Mesh, t: usize) -> ([f64; 3], f64) {
    let p = [0, 1, 2].map(|k| mesh.positions[mesh.indices[t * 3 + k] as usize]);
    let e = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let f = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let c = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
    let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    if l > 0.0 {
        ([c[0] / l, c[1] / l, c[2] / l], l * 0.5)
    } else {
        ([0.0; 3], 0.0)
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn welded(mesh: &Mesh) -> Vec<u32> {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in &mesh.positions {
        for d in 0..3 {
            low[d] = low[d].min(p[d]);
            high[d] = high[d].max(p[d]);
        }
    }
    let span = (0..3).map(|d| high[d] - low[d]).fold(1.0e-9f64, f64::max) * 1.0e-6;
    let mut seen = HashMap::new();
    mesh.positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let key = [0, 1, 2].map(|d| ((p[d] - low[d]) / span).round() as i64);
            *seen.entry(key).or_insert(i as u32)
        })
        .collect()
}

/// Greedy pairs over welded edges, the way both old unwraps found neighbours.
fn adjacency(mesh: &Mesh, weld: &[u32]) -> Vec<[usize; 3]> {
    let triangles = mesh.indices.len() / 3;
    let mut adjacent = vec![[usize::MAX; 3]; triangles];
    let mut open: HashMap<(u32, u32), (usize, usize)> = HashMap::with_capacity(triangles * 3);
    for t in 0..triangles {
        for e in 0..3 {
            let a = weld[mesh.indices[t * 3 + e] as usize];
            let b = weld[mesh.indices[t * 3 + (e + 1) % 3] as usize];
            let key = (a.min(b), a.max(b));
            if let Some((other, oe)) = open.remove(&key) {
                adjacent[t][e] = other;
                adjacent[other][oe] = t;
            } else {
                open.insert(key, (t, e));
            }
        }
    }
    adjacent
}

fn project(p: [f64; 3], n: [f64; 3]) -> [f64; 2] {
    let axis = (0..3).max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs())).unwrap();
    [p[(axis + 1) % 3] * if n[axis] < 0.0 { -1.0 } else { 1.0 }, p[(axis + 2) % 3]]
}

/// A chart in its own space, as the old unwrap left it.
fn disk_map(mesh: &Mesh, weld: &[u32], tris: &[usize], out: &mut [[f64; 2]]) -> bool {
    let mut local = HashMap::new();
    let mut boundary_edges: HashMap<(u32, u32), i32> = HashMap::new();
    for &t in tris {
        for k in 0..3 {
            let (a, b) = (weld[mesh.indices[t * 3 + k] as usize], weld[mesh.indices[t * 3 + (k + 1) % 3] as usize]);
            let next = local.len() as u32;
            local.entry(a).or_insert(next);
            *boundary_edges.entry((a.min(b), a.max(b))).or_default() += if a < b { 1 } else { -1 };
            if boundary_edges[&(a.min(b), a.max(b))] == 0 {
                boundary_edges.remove(&(a.min(b), a.max(b)));
            }
        }
    }
    let edge_total: usize = {
        let mut edges = std::collections::HashSet::new();
        for &t in tris {
            for k in 0..3 {
                let (a, b) = (weld[mesh.indices[t * 3 + k] as usize], weld[mesh.indices[t * 3 + (k + 1) % 3] as usize]);
                edges.insert((a.min(b), a.max(b)));
            }
        }
        edges.len()
    };
    if local.len() + tris.len() != edge_total + 1 || boundary_edges.is_empty() {
        return false;
    }
    let mut outgoing: HashMap<u32, u32> = HashMap::new();
    for (&(a, b), &sign) in &boundary_edges {
        if sign.abs() != 1 {
            return false;
        }
        let (from, to) = if sign > 0 { (a, b) } else { (b, a) };
        if outgoing.insert(from, to).is_some() {
            return false;
        }
    }
    let start = *outgoing.keys().min().unwrap();
    let mut boundary = vec![start];
    let mut at = outgoing[&start];
    while at != start {
        boundary.push(at);
        let Some(&n) = outgoing.get(&at) else { return false };
        at = n;
        if boundary.len() > outgoing.len() {
            return false;
        }
    }
    if boundary.len() != outgoing.len() {
        return false;
    }
    let pos = |v: u32| mesh.positions[v as usize];
    let lengths: Vec<f64> = (0..boundary.len())
        .map(|i| {
            let (a, b) = (pos(boundary[i]), pos(boundary[(i + 1) % boundary.len()]));
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        })
        .collect();
    let total: f64 = lengths.iter().sum();
    if !(total > 0.0) {
        return false;
    }
    let n = local.len();
    let mut uv = vec![[0.5, 0.5]; n];
    let mut fixed = vec![false; n];
    let mut arc = 0.0;
    for (i, &v) in boundary.iter().enumerate() {
        let angle = std::f64::consts::TAU * arc / total;
        let l = local[&v] as usize;
        uv[l] = [0.5 + 0.5 * angle.cos(), 0.5 + 0.5 * angle.sin()];
        fixed[l] = true;
        arc += lengths[i];
    }
    let mut neighbours = vec![Vec::new(); n];
    for &t in tris {
        for k in 0..3 {
            let a = local[&weld[mesh.indices[t * 3 + k] as usize]] as usize;
            let b = local[&weld[mesh.indices[t * 3 + (k + 1) % 3] as usize]] as usize;
            if !neighbours[a].contains(&(b as u32)) {
                neighbours[a].push(b as u32);
                neighbours[b].push(a as u32);
            }
        }
    }
    let mut next = uv.clone();
    for _ in 0..128 {
        for v in 0..n {
            if fixed[v] || neighbours[v].is_empty() {
                continue;
            }
            let mut p = [0.0; 2];
            for &o in &neighbours[v] {
                p[0] += uv[o as usize][0];
                p[1] += uv[o as usize][1];
            }
            next[v] = [p[0] / neighbours[v].len() as f64, p[1] / neighbours[v].len() as f64];
        }
        std::mem::swap(&mut uv, &mut next);
    }
    for &t in tris {
        let q = [0, 1, 2].map(|k| uv[local[&weld[mesh.indices[t * 3 + k] as usize]] as usize]);
        let area = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0]);
        if !area.is_finite() || area.abs() <= 1.0e-14 {
            return false;
        }
    }
    for &t in tris {
        for k in 0..3 {
            out[t * 3 + k] = uv[local[&weld[mesh.indices[t * 3 + k] as usize]] as usize];
        }
    }
    true
}

/// The mesh editor's unwrap and pack: dihedral charts, disk maps or one chart
/// per polygon, then a square grid at one common scale.
pub fn mesh_edit(mesh: &Mesh, size: u32, padding: u32) -> (Layout, Vec<u32>) {
    let start = Instant::now();
    let triangles = mesh.indices.len() / 3;
    let weld = welded(mesh);
    let adjacent = adjacency(mesh, &weld);
    let normals: Vec<_> = (0..triangles).map(|t| normal(mesh, t)).collect();
    let mut visited = vec![false; triangles];
    let mut uv = vec![[0.0; 2]; triangles * 3];
    let mut chart_of = vec![0u32; triangles];
    let mut charts: Vec<Vec<usize>> = Vec::new();
    for seed in 0..triangles {
        if visited[seed] {
            continue;
        }
        visited[seed] = true;
        let mut stack = vec![seed];
        let mut tris = Vec::new();
        while let Some(t) = stack.pop() {
            tris.push(t);
            for e in 0..3 {
                let o = adjacent[t][e];
                if o != usize::MAX && !visited[o] && dot(normals[t].0, normals[o].0) >= 0.5 {
                    visited[o] = true;
                    stack.push(o);
                }
            }
        }
        let flat = tris.iter().all(|&t| dot(normals[t].0, normals[tris[0]].0) > 1.0 - 1.0e-10);
        let point = |t: usize, k: usize| mesh.positions[mesh.indices[t * 3 + k] as usize];
        if flat {
            for &t in &tris {
                for k in 0..3 {
                    uv[t * 3 + k] = project(point(t, k), normals[tris[0]].0);
                }
                chart_of[t] = charts.len() as u32;
            }
            charts.push(tris);
        } else if disk_map(mesh, &weld, &tris, &mut uv) {
            for &t in &tris {
                chart_of[t] = charts.len() as u32;
            }
            charts.push(tris);
        } else {
            for &t in &tris {
                for k in 0..3 {
                    uv[t * 3 + k] = project(point(t, k), normals[t].0);
                }
                chart_of[t] = charts.len() as u32;
                charts.push(vec![t]);
            }
        }
    }
    // The square grid.
    let columns = (charts.len() as f64).sqrt().ceil().max(1.0);
    let rows = (charts.len() as f64 / columns).ceil().max(1.0);
    let pad = padding as f64 / size as f64;
    let cell = [1.0 / columns, 1.0 / rows];
    let mut maximum = [0.0f64; 2];
    let mut bounds = Vec::with_capacity(charts.len());
    for chart in &charts {
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for &t in chart {
            for k in 0..3 {
                for d in 0..2 {
                    low[d] = low[d].min(uv[t * 3 + k][d]);
                    high[d] = high[d].max(uv[t * 3 + k][d]);
                }
            }
        }
        for d in 0..2 {
            maximum[d] = maximum[d].max(high[d] - low[d]);
        }
        bounds.push((low, high));
    }
    let scale = ((cell[0] - 2.0 * pad).max(0.0) / maximum[0].max(1.0e-12)).min((cell[1] - 2.0 * pad).max(0.0) / maximum[1].max(1.0e-12));
    let mut area = 0.0;
    for (i, chart) in charts.iter().enumerate() {
        let origin = [(i as f64 % columns) * cell[0], (i as f64 / columns).floor() * cell[1]];
        let (low, _) = bounds[i];
        for &t in chart {
            for k in 0..3 {
                let c = t * 3 + k;
                uv[c] = [
                    (origin[0] + pad + (uv[c][0] - low[0]) * scale) * size as f64,
                    (origin[1] + pad + (uv[c][1] - low[1]) * scale) * size as f64,
                ];
            }
            let q = [uv[t * 3], uv[t * 3 + 1], uv[t * 3 + 2]];
            area += ((q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0])).abs() * 0.5;
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let fill = area / (size as f64 * size as f64);
    (Layout { charts: charts.len(), uv, fill, seconds, detail: String::new() }, chart_of)
}

/// The AO atlas charts: planar patches under 15 degrees pairwise and 30 from
/// their mean, each sized apart and clamped, on a shelf.
pub fn ao_atlas(mesh: &Mesh, size: u32) -> (Layout, Vec<u32>) {
    let start = Instant::now();
    let triangles = mesh.indices.len() / 3;
    let weld = welded(mesh);
    let adjacent = adjacency(mesh, &weld);
    let normals: Vec<_> = (0..triangles).map(|t| normal(mesh, t).0).collect();
    let areas: Vec<f64> = (0..triangles).map(|t| normal(mesh, t).1).collect();
    let mut chart_of = vec![usize::MAX; triangles];
    let mut charts: Vec<Vec<usize>> = Vec::new();
    for seed in 0..triangles {
        if chart_of[seed] != usize::MAX {
            continue;
        }
        let index = charts.len();
        chart_of[seed] = index;
        let mut tris = vec![seed];
        let mut sum = normals[seed];
        let mut stack = vec![seed];
        while let Some(t) = stack.pop() {
            let l = dot(sum, sum).sqrt().max(1.0e-12);
            let mean = [sum[0] / l, sum[1] / l, sum[2] / l];
            for e in 0..3 {
                let o = adjacent[t][e];
                if o == usize::MAX || chart_of[o] != usize::MAX {
                    continue;
                }
                if dot(normals[o], normals[t]) < 0.966 || dot(normals[o], mean) < 0.866 {
                    continue;
                }
                chart_of[o] = index;
                tris.push(o);
                for d in 0..3 {
                    sum[d] += normals[o][d];
                }
                stack.push(o);
            }
        }
        charts.push(tris);
    }
    struct Box {
        axis: usize,
        low: [f64; 2],
        high: [f64; 2],
        w: usize,
        h: usize,
        x: usize,
        y: usize,
    }
    let plane = |p: [f64; 3], axis: usize| match axis {
        0 => [p[2], p[1]],
        1 => [p[0], p[2]],
        _ => [p[0], p[1]],
    };
    let (gutter, base) = (1usize, 64.0);
    let mut boxes: Vec<Box> = charts
        .iter()
        .map(|tris| {
            let mut sum = [0.0; 3];
            for &t in tris {
                for d in 0..3 {
                    sum[d] += normals[t][d] * areas[t];
                }
            }
            let axis = (0..3).max_by(|&a, &b| sum[a].abs().total_cmp(&sum[b].abs())).unwrap();
            let mut low = [f64::INFINITY; 2];
            let mut high = [f64::NEG_INFINITY; 2];
            for &t in tris {
                for k in 0..3 {
                    let q = plane(mesh.positions[mesh.indices[t * 3 + k] as usize], axis);
                    for d in 0..2 {
                        low[d] = low[d].min(q[d]);
                        high[d] = high[d].max(q[d]);
                    }
                }
            }
            Box { axis, low, high, w: 0, h: 0, x: 0, y: 0 }
        })
        .collect();
    let fit = |boxes: &mut Vec<Box>, k: f64| -> bool {
        let mut order: Vec<usize> = (0..boxes.len()).collect();
        for b in boxes.iter_mut() {
            b.w = (((b.high[0] - b.low[0]) * base * k).ceil() as usize + 2 * gutter).clamp(3, 512);
            b.h = (((b.high[1] - b.low[1]) * base * k).ceil() as usize + 2 * gutter).clamp(3, 512);
        }
        order.sort_by(|&a, &b| boxes[b].h.cmp(&boxes[a].h).then(a.cmp(&b)));
        let (mut x, mut y, mut h) = (0usize, 0usize, 0usize);
        for i in order {
            if x + boxes[i].w > size as usize {
                x = 0;
                y += h;
                h = 0;
            }
            if y + boxes[i].h > size as usize {
                return false;
            }
            boxes[i].x = x;
            boxes[i].y = y;
            x += boxes[i].w;
            h = h.max(boxes[i].h);
        }
        true
    };
    let total: f64 = areas.iter().sum();
    let mut k = ((size as f64 * size as f64 * 0.85 / total.max(1.0e-12)).sqrt() / base).max(1.0e-6);
    let mut floor_hit = false;
    while !fit(&mut boxes, k) {
        k *= 0.93;
        if k < 1.0e-5 {
            floor_hit = true;
            break;
        }
    }
    let mut uv = vec![[0.0; 2]; triangles * 3];
    let mut area = 0.0;
    for (index, tris) in charts.iter().enumerate() {
        let b = &boxes[index];
        let su = (b.w - 2 * gutter) as f64 / (b.high[0] - b.low[0]).max(1.0e-6);
        let sv = (b.h - 2 * gutter) as f64 / (b.high[1] - b.low[1]).max(1.0e-6);
        let s = su.min(sv);
        for &t in tris {
            for k in 0..3 {
                let q = plane(mesh.positions[mesh.indices[t * 3 + k] as usize], b.axis);
                uv[t * 3 + k] = [
                    b.x as f64 + gutter as f64 + (q[0] - b.low[0]) * s,
                    b.y as f64 + gutter as f64 + (q[1] - b.low[1]) * s,
                ];
            }
            let q = [uv[t * 3], uv[t * 3 + 1], uv[t * 3 + 2]];
            area += ((q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0])).abs() * 0.5;
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    let chart_of = chart_of.iter().map(|&c| c as u32).collect();
    let fill = area / (size as f64 * size as f64);
    let detail = if floor_hit { "overflows the atlas".to_string() } else { String::new() };
    (Layout { charts: charts.len(), uv, fill, seconds, detail }, chart_of)
}
