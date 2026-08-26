//! Getting from A to B: A\* over a storey's walkable lattice, a string pull to
//! throw away the staircase-shaped detail, and a room-order walk that visits
//! every reachable room.
//!
//! The A\* cost is not just distance. Every step is multiplied by a penalty
//! that grows as clearance falls below `prefer` metres, so given two ways
//! across a room the planner takes the open one. A camera that scrapes the
//! furniture is technically collision-free and still looks wrong.

use crate::analysis::{ClearMode, ClearanceField, SiteAnalysis};
use makepad_math::{vec3, Vec3f};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

const ORTHO: u32 = 100;
const DIAG: u32 = 141;

/// A\* between two cells of one storey. Returns cell indices, or `None` when
/// the two are not connected.
pub fn route_cells(site: &SiteAnalysis, si: usize, from: u32, to: u32) -> Option<Vec<u32>> {
    let st = site.storeys.get(si)?;
    let (nx, ny) = (st.nx, st.ny);
    let n = nx * ny;
    if from as usize >= n || to as usize >= n || !st.walkable[from as usize] || !st.walkable[to as usize] {
        return None;
    }
    let prefer = (site.config.body.radius * 2.5).max(0.6);
    let step_cost = |i: usize, base: u32| -> u32 {
        let c = st.clearance[i];
        let tight = ((prefer - c).max(0.0) / prefer).min(1.0);
        base + (base as f32 * 2.0 * tight) as u32
    };
    let (tx, ty) = ((to as usize) % nx, (to as usize) / nx);
    let h = |i: usize| -> u32 {
        let (x, y) = (i % nx, i / nx);
        let dx = (x as i32 - tx as i32).unsigned_abs();
        let dy = (y as i32 - ty as i32).unsigned_abs();
        let (lo, hi) = (dx.min(dy), dx.max(dy));
        lo * DIAG + (hi - lo) * ORTHO
    };

    let mut g = vec![u32::MAX; n];
    let mut parent = vec![u32::MAX; n];
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    g[from as usize] = 0;
    heap.push(Reverse((h(from as usize), from)));
    while let Some(Reverse((_, cur))) = heap.pop() {
        let ci = cur as usize;
        if cur == to {
            let mut path = vec![to];
            let mut c = to;
            while parent[c as usize] != u32::MAX {
                c = parent[c as usize];
                path.push(c);
            }
            path.reverse();
            return Some(path);
        }
        let (x, y) = (ci % nx, ci / nx);
        for (dx, dy) in [
            (1i32, 0i32),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let (jx, jy) = (x as i32 + dx, y as i32 + dy);
            if jx < 0 || jy < 0 || jx >= nx as i32 || jy >= ny as i32 {
                continue;
            }
            let j = jy as usize * nx + jx as usize;
            if !st.walkable[j] {
                continue;
            }
            let diag = dx != 0 && dy != 0;
            if diag {
                // No squeezing through the gap between two corners.
                let a = y * nx + (x as i32 + dx) as usize;
                let b = (y as i32 + dy) as usize * nx + x;
                if !st.walkable[a] || !st.walkable[b] {
                    continue;
                }
            }
            let cost = step_cost(j, if diag { DIAG } else { ORTHO });
            let ng = g[ci].saturating_add(cost);
            if ng < g[j] {
                g[j] = ng;
                parent[j] = cur;
                heap.push(Reverse((ng.saturating_add(h(j)), j as u32)));
            }
        }
    }
    None
}

/// Snap a world point to the nearest walkable cell of a storey.
///
/// Nearest **in a straight clear line**, not merely nearest by distance. A
/// point on a stair tread or inside a wall has walkable ground on both sides
/// of that wall, and the plain nearest cell is regularly the one *outside* the
/// building — which then routes the whole leg around the garden, through the
/// façade, and back in.
pub fn cell_near(site: &SiteAnalysis, si: usize, p: Vec3f) -> Option<u32> {
    let st = site.storeys.get(si)?;
    let (cx, cy, _) = site.grid.cell_of(vec3(p.x, p.y, st.eye_z))?;
    let start = st.at(cx, cy);
    if st.walkable[start] {
        return Some(start as u32);
    }
    let field = site.clearance(ClearMode::Walk(si));
    let from = vec3(p.x, p.y, st.eye_z);
    let radius = site.config.body.radius * 0.7;
    let max_r = ((3.0 / site.grid.cell).ceil() as i32).max(4);
    let mut fallback: Option<(f32, u32)> = None;
    for r in 1..=max_r {
        let mut visible: Option<(f32, u32)> = None;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x < 0 || y < 0 || x >= st.nx as i32 || y >= st.ny as i32 {
                    continue;
                }
                let i = y as usize * st.nx + x as usize;
                if !st.walkable[i] {
                    continue;
                }
                let d = (dx * dx + dy * dy) as f32;
                if fallback.map_or(true, |(bd, _)| d < bd) {
                    fallback = Some((d, i as u32));
                }
                let w = site.grid.world_of(x as usize, y as usize, 0);
                if field.segment_clear(from, vec3(w.x, w.y, st.eye_z), radius)
                    && visible.map_or(true, |(bd, _)| d < bd)
                {
                    visible = Some((d, i as u32));
                }
            }
        }
        if let Some((_, i)) = visible {
            return Some(i);
        }
    }
    fallback.map(|(_, i)| i)
}

pub fn cell_point(site: &SiteAnalysis, si: usize, cell: u32) -> Vec3f {
    let st = &site.storeys[si];
    let (x, y) = ((cell as usize) % st.nx, (cell as usize) / st.nx);
    let w = site.grid.world_of(x, y, 0);
    vec3(w.x, w.y, st.eye_z)
}

/// Full route between two world points on one storey: A\*, then a string pull
/// that keeps only the corners the geometry actually forces.
///
/// The visibility test is [`ClearanceField::segment_clear`] — the same oracle
/// the planner and the QA use, at the same radius. If this function shortens a
/// path across something, the QA would have caught it; it cannot, because they
/// are asking one function.
pub fn route_points(site: &SiteAnalysis, si: usize, from: Vec3f, to: Vec3f) -> Option<Vec<Vec3f>> {
    let a = cell_near(site, si, from)?;
    let b = cell_near(site, si, to)?;
    let cells = route_cells(site, si, a, b)?;
    let pts: Vec<Vec3f> = cells.iter().map(|c| cell_point(site, si, *c)).collect();
    let field = site.clearance(ClearMode::Walk(si));
    Some(string_pull(&field, &pts, site.config.body.radius))
}

/// Greedy furthest-visible waypoint. Keeps the first and last point.
pub fn string_pull(field: &ClearanceField, pts: &[Vec3f], radius: f32) -> Vec<Vec3f> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    let mut i = 0usize;
    while i < pts.len() - 1 {
        let mut j = pts.len() - 1;
        // Never let one straight leg swallow the whole path: long legs make
        // the spline bulge away from the geometry that justified them.
        let cap = (i + 64).min(pts.len() - 1);
        j = j.min(cap);
        while j > i + 1 {
            if field.segment_clear(pts[i], pts[j], radius) {
                break;
            }
            j -= 1;
        }
        out.push(pts[j]);
        i = j;
    }
    out
}

/// The **walk** over the room graph, not the visit list: always step to the
/// best-scoring unvisited neighbour, and when there is none, walk back through
/// the rooms in between to the nearest one that still has an unvisited
/// neighbour.
///
/// Consecutive entries are therefore always adjacent — joined by a portal or a
/// stair — and rooms already seen appear again when the route re-treads them,
/// which is what actually happens when you tour a house. Returning only the
/// first visits would put non-adjacent rooms next to each other, and the
/// generator would then draw a straight line between two rooms with no door
/// between them; when they were on different storeys that line went through
/// the floor slab.
///
/// Every interior room reachable from `start` appears at least once.
///
/// The outdoors is a "room" too — the walkable ground around the building is
/// one enormous connected region — and it is excluded here. Wandering into the
/// garden halfway through a house tour is not a shot, it is a wrong turn.
pub fn room_order(site: &SiteAnalysis, start: usize) -> Vec<usize> {
    let n = site.rooms.len();
    let inside = |r: usize| site.rooms[r].interior || r == start;
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for p in &site.portals {
        if !inside(p.a) || !inside(p.b) {
            continue;
        }
        adj[p.a].push(p.b);
        adj[p.b].push(p.a);
    }
    for s in &site.stairs {
        if !inside(s.lower_room) || !inside(s.upper_room) {
            continue;
        }
        adj[s.lower_room].push(s.upper_room);
        adj[s.upper_room].push(s.lower_room);
    }
    let mut seen = vec![false; n];
    let mut order = Vec::new();
    let mut cur = start;
    seen[cur] = true;
    order.push(cur);
    loop {
        // Best unvisited neighbour.
        let next = adj[cur]
            .iter()
            .copied()
            .filter(|r| !seen[*r])
            .max_by(|a, b| {
                site.room_priority(*a)
                    .partial_cmp(&site.room_priority(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(nx) = next {
            seen[nx] = true;
            order.push(nx);
            cur = nx;
            continue;
        }
        // Nothing next door: BFS to the nearest room that still has an
        // unvisited neighbour, and walk the corridor back through it.
        let mut prev = vec![usize::MAX; n];
        let mut q = VecDeque::new();
        let mut mark = vec![false; n];
        q.push_back(cur);
        mark[cur] = true;
        let mut target = None;
        while let Some(r) = q.pop_front() {
            if !seen[r] {
                target = Some(r);
                break;
            }
            for nb in &adj[r] {
                if !mark[*nb] {
                    mark[*nb] = true;
                    prev[*nb] = r;
                    q.push_back(*nb);
                }
            }
        }
        let Some(t) = target else { break };
        // Re-tread the whole way back, listing every room passed through so
        // that consecutive entries stay adjacent.
        let mut back = vec![t];
        let mut c = t;
        while prev[c] != usize::MAX {
            c = prev[c];
            back.push(c);
        }
        back.reverse();
        for r in back.into_iter().skip(1) {
            seen[r] = true;
            order.push(r);
        }
        cur = t;
    }
    order
}

/// Shortest path over the room graph from `from` to `to`, inclusive of both.
/// Consecutive entries are adjacent, so a generator can walk it directly.
pub fn room_path(site: &SiteAnalysis, from: usize, to: usize) -> Vec<usize> {
    let n = site.rooms.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for p in &site.portals {
        adj[p.a].push(p.b);
        adj[p.b].push(p.a);
    }
    for s in &site.stairs {
        adj[s.lower_room].push(s.upper_room);
        adj[s.upper_room].push(s.lower_room);
    }
    let mut prev = vec![usize::MAX; n];
    let mut seen = vec![false; n];
    let mut q = VecDeque::new();
    seen[from] = true;
    q.push_back(from);
    while let Some(r) = q.pop_front() {
        if r == to {
            let mut path = vec![to];
            let mut c = to;
            while prev[c] != usize::MAX {
                c = prev[c];
                path.push(c);
            }
            path.reverse();
            return path;
        }
        for nb in &adj[r] {
            if !seen[*nb] {
                seen[*nb] = true;
                prev[*nb] = r;
                q.push_back(*nb);
            }
        }
    }
    Vec::new()
}

/// The portal joining two rooms, if they share one.
pub fn portal_between(site: &SiteAnalysis, a: usize, b: usize) -> Option<&crate::analysis::Portal> {
    site.portals
        .iter()
        .find(|p| (p.a == a && p.b == b) || (p.a == b && p.b == a))
}

/// The stair joining two rooms, if they are on different storeys.
pub fn stair_between(site: &SiteAnalysis, a: usize, b: usize) -> Option<&crate::analysis::StairLink> {
    site.stairs
        .iter()
        .find(|s| (s.lower_room == a && s.upper_room == b) || (s.lower_room == b && s.upper_room == a))
}
