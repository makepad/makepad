//! The `region.graph` routing graph: CSR adjacency over directed edges with
//! per-mode speeds (car / bike / foot), full segment geometry for drawing, a
//! uniform grid for nearest-edge snapping, and turn restrictions handled by
//! edge-based expansion at restricted via-vertices only.

use crate::fmt::{ByteReader, ByteWriter, NavFmtError};
use crate::geo::*;
use crate::nav;
use std::collections::{BinaryHeap, HashMap};

const GRAPH_MAGIC: u32 = 0x4d50_4752; // "RGPM"
const GRAPH_VERSION: u32 = 1;

pub const EDGE_NONE: u32 = u32::MAX;
pub const NAME_NONE: u32 = u32::MAX;

// --- Travel modes ---

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub enum TravelMode {
    #[default]
    Car,
    Bike,
    Foot,
}

impl TravelMode {
    pub fn index(&self) -> usize {
        match self {
            TravelMode::Car => 0,
            TravelMode::Bike => 1,
            TravelMode::Foot => 2,
        }
    }
    /// Upper bound used by the A* heuristic; must be >= any edge speed.
    pub fn max_speed_kmh(&self) -> f64 {
        match self {
            TravelMode::Car => 130.0,
            TravelMode::Bike => 28.0,
            TravelMode::Foot => 7.0,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            TravelMode::Car => "Drive",
            TravelMode::Bike => "Bike",
            TravelMode::Foot => "Walk",
        }
    }
}

// --- Road kinds ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum RoadKind {
    Motorway = 0,
    MotorwayLink = 1,
    Trunk = 2,
    TrunkLink = 3,
    Primary = 4,
    PrimaryLink = 5,
    Secondary = 6,
    SecondaryLink = 7,
    Tertiary = 8,
    TertiaryLink = 9,
    Unclassified = 10,
    Residential = 11,
    LivingStreet = 12,
    Service = 13,
    Track = 14,
    Cycleway = 15,
    Footway = 16,
    Path = 17,
    Pedestrian = 18,
    Steps = 19,
    Bridleway = 20,
    Ferry = 21,
    Other = 22,
}

impl RoadKind {
    pub fn from_u8(v: u8) -> RoadKind {
        if v <= 22 {
            // Safety: contiguous discriminants 0..=22 checked above.
            unsafe { std::mem::transmute(v) }
        } else {
            RoadKind::Other
        }
    }

    fn from_highway(value: &str) -> Option<RoadKind> {
        Some(match value {
            "motorway" => RoadKind::Motorway,
            "motorway_link" => RoadKind::MotorwayLink,
            "trunk" => RoadKind::Trunk,
            "trunk_link" => RoadKind::TrunkLink,
            "primary" => RoadKind::Primary,
            "primary_link" => RoadKind::PrimaryLink,
            "secondary" => RoadKind::Secondary,
            "secondary_link" => RoadKind::SecondaryLink,
            "tertiary" => RoadKind::Tertiary,
            "tertiary_link" => RoadKind::TertiaryLink,
            "unclassified" => RoadKind::Unclassified,
            "residential" => RoadKind::Residential,
            "living_street" => RoadKind::LivingStreet,
            "service" => RoadKind::Service,
            "track" => RoadKind::Track,
            "cycleway" => RoadKind::Cycleway,
            "footway" => RoadKind::Footway,
            "path" => RoadKind::Path,
            "pedestrian" => RoadKind::Pedestrian,
            "steps" => RoadKind::Steps,
            "bridleway" => RoadKind::Bridleway,
            _ => return None,
        })
    }

    /// Default speeds [car, bike, foot] in km/h; 0 = mode not allowed.
    /// Netherlands-flavored defaults; maxspeed and access tags override.
    fn default_speeds(&self) -> [u8; 3] {
        match self {
            RoadKind::Motorway => [100, 0, 0],
            RoadKind::MotorwayLink => [60, 0, 0],
            RoadKind::Trunk => [80, 0, 0],
            RoadKind::TrunkLink => [55, 0, 0],
            RoadKind::Primary => [70, 16, 4],
            RoadKind::PrimaryLink => [50, 16, 4],
            RoadKind::Secondary => [60, 17, 5],
            RoadKind::SecondaryLink => [45, 17, 5],
            RoadKind::Tertiary => [50, 17, 5],
            RoadKind::TertiaryLink => [40, 17, 5],
            RoadKind::Unclassified => [40, 16, 5],
            RoadKind::Residential => [30, 15, 5],
            RoadKind::LivingStreet => [15, 10, 5],
            RoadKind::Service => [15, 12, 5],
            RoadKind::Track => [15, 12, 5],
            RoadKind::Cycleway => [0, 18, 4],
            RoadKind::Footway => [0, 0, 5],
            RoadKind::Path => [0, 12, 5],
            RoadKind::Pedestrian => [0, 5, 5],
            RoadKind::Steps => [0, 0, 2],
            RoadKind::Bridleway => [0, 10, 4],
            RoadKind::Ferry => [10, 10, 10],
            RoadKind::Other => [0, 0, 4],
        }
    }
}

// --- Edge flags ---

pub const EDGE_FLAG_ROUNDABOUT: u8 = 1;
pub const EDGE_FLAG_LINK: u8 = 2;
pub const EDGE_FLAG_BRIDGE: u8 = 4;
pub const EDGE_FLAG_TUNNEL: u8 = 8;
/// Geometry pool points run opposite to this directed edge's direction.
pub const EDGE_FLAG_GEO_REVERSED: u8 = 16;

#[derive(Clone, Debug)]
pub struct Edge {
    pub to: u32,
    /// Directed edge id of the opposite direction, or EDGE_NONE.
    pub rev: u32,
    pub len_m: f32,
    pub kind: RoadKind,
    /// km/h per mode index; 0 = not allowed in this direction.
    pub speeds: [u8; 3],
    pub flags: u8,
    pub name_idx: u32,
    pub geo_start: u32,
    pub geo_count: u16,
}

impl Edge {
    pub fn allows(&self, mode: TravelMode) -> bool {
        self.speeds[mode.index()] > 0
    }
    fn cost_ms(&self, mode: TravelMode) -> u64 {
        let speed = self.speeds[mode.index()] as f64;
        if speed <= 0.0 {
            return u64::MAX;
        }
        (self.len_m as f64 / (speed / 3.6) * 1000.0) as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TurnRestriction {
    pub from_edge: u32,
    pub to_edge: u32,
    pub only: bool,
}

struct SnapGrid {
    min_x: f64,
    min_y: f64,
    cell: f64,
    nx: u32,
    ny: u32,
    offsets: Vec<u32>,
    items: Vec<u32>, // canonical directed edge ids
}

pub struct RouteGraph {
    pub vertices: Vec<(u32, u32)>, // fixed norm mercator
    pub csr: Vec<u32>,             // vertex_count + 1
    pub edges: Vec<Edge>,
    pub edge_from: Vec<u32>,
    pub geometry: Vec<(u32, u32)>,
    pub names: Vec<String>,
    pub restrictions: HashMap<u32, Vec<TurnRestriction>>,
    grid: SnapGrid,
}

// --- Route result ---

#[derive(Clone, Debug)]
pub struct Route {
    pub mode: TravelMode,
    /// Full polyline start-snap → goal-snap.
    pub points: Vec<LonLat>,
    /// Cumulative distance in meters, same length as `points`.
    pub cum_dist_m: Vec<f64>,
    pub length_m: f64,
    pub duration_s: f64,
    pub maneuvers: Vec<nav::Maneuver>,
}

#[derive(Clone, Copy, Debug)]
pub struct Snap {
    /// Canonical directed edge id.
    pub edge: u32,
    /// Arc-length parameter 0..1 along the canonical edge geometry.
    pub t: f64,
    pub point: LonLat,
    pub dist_m: f64,
}

impl RouteGraph {
    pub fn vertex_lon_lat(&self, v: u32) -> LonLat {
        let (x, y) = self.vertices[v as usize];
        fixed_to_lon_lat(x, y)
    }

    pub fn edge_name<'a>(&'a self, edge: &Edge) -> Option<&'a str> {
        if edge.name_idx == NAME_NONE {
            None
        } else {
            Some(&self.names[edge.name_idx as usize])
        }
    }

    /// Geometry of a directed edge in travel direction, as lon/lat.
    pub fn edge_points(&self, edge_id: u32) -> Vec<LonLat> {
        let edge = &self.edges[edge_id as usize];
        let start = edge.geo_start as usize;
        let count = edge.geo_count as usize;
        let mut out: Vec<LonLat> = self.geometry[start..start + count]
            .iter()
            .map(|&(x, y)| fixed_to_lon_lat(x, y))
            .collect();
        if edge.flags & EDGE_FLAG_GEO_REVERSED != 0 {
            out.reverse();
        }
        out
    }

    fn out_edges(&self, v: u32) -> std::ops::Range<usize> {
        self.csr[v as usize] as usize..self.csr[v as usize + 1] as usize
    }

    /// True when the transition in_edge → out_edge at `via` is banned.
    fn turn_banned(&self, via: u32, in_edge: u32, out_edge: u32) -> bool {
        let Some(list) = self.restrictions.get(&via) else {
            return false;
        };
        let mut in_only_set = false;
        let mut only_allows = false;
        for r in list {
            if r.from_edge != in_edge {
                continue;
            }
            if r.only {
                in_only_set = true;
                if r.to_edge == out_edge {
                    only_allows = true;
                }
            } else if r.to_edge == out_edge {
                return true;
            }
        }
        in_only_set && !only_allows
    }

    // --- Snapping ---

    /// Nearest edge point for `mode`, searched outward on the grid.
    pub fn snap(&self, pos: LonLat, mode: TravelMode) -> Option<Snap> {
        let (px, py) = lon_lat_to_norm(pos);
        let meters = meters_per_norm_unit(pos.lat);
        let cell_of = |v: f64, min: f64| ((v - min) / self.grid.cell).floor() as i64;
        let cx = cell_of(px, self.grid.min_x);
        let cy = cell_of(py, self.grid.min_y);

        let mut best: Option<(f64, u32, usize, f64, (f64, f64))> = None;
        // Expand rings until we have a hit and one extra ring for safety.
        let max_ring = 24i64;
        let mut found_ring: Option<i64> = None;
        for ring in 0..=max_ring {
            if let Some(fr) = found_ring {
                if ring > fr + 1 {
                    break;
                }
            }
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs().max(dy.abs()) != ring {
                        continue;
                    }
                    let gx = cx + dx;
                    let gy = cy + dy;
                    if gx < 0 || gy < 0 || gx >= self.grid.nx as i64 || gy >= self.grid.ny as i64 {
                        continue;
                    }
                    let cell_idx = (gy as usize) * self.grid.nx as usize + gx as usize;
                    let start = self.grid.offsets[cell_idx] as usize;
                    let end = self.grid.offsets[cell_idx + 1] as usize;
                    for &edge_id in &self.grid.items[start..end] {
                        let edge = &self.edges[edge_id as usize];
                        let rev_allows = edge.rev != EDGE_NONE
                            && self.edges[edge.rev as usize].allows(mode);
                        if !edge.allows(mode) && !rev_allows {
                            continue;
                        }
                        let geo = &self.geometry[edge.geo_start as usize
                            ..edge.geo_start as usize + edge.geo_count as usize];
                        for i in 0..geo.len().saturating_sub(1) {
                            let a = (fixed_to_norm(geo[i].0), fixed_to_norm(geo[i].1));
                            let b = (fixed_to_norm(geo[i + 1].0), fixed_to_norm(geo[i + 1].1));
                            let (proj, t) = project_on_segment((px, py), a, b);
                            let d = ((proj.0 - px).powi(2) + (proj.1 - py).powi(2)).sqrt() * meters;
                            if best.is_none() || d < best.unwrap().0 {
                                best = Some((d, edge_id, i, t, proj));
                            }
                        }
                    }
                }
            }
            if best.is_some() && found_ring.is_none() {
                found_ring = Some(ring);
            }
        }

        let (dist_m, edge_id, seg, seg_t, proj) = best?;
        // Convert segment index + t to arc-length parameter along the edge.
        let edge = &self.edges[edge_id as usize];
        let geo = &self.geometry
            [edge.geo_start as usize..edge.geo_start as usize + edge.geo_count as usize];
        let mut total = 0.0;
        let mut upto = 0.0;
        for i in 0..geo.len() - 1 {
            let a = fixed_to_lon_lat(geo[i].0, geo[i].1);
            let b = fixed_to_lon_lat(geo[i + 1].0, geo[i + 1].1);
            let d = haversine_m(a, b);
            if i < seg {
                upto += d;
            } else if i == seg {
                upto += d * seg_t;
            }
            total += d;
        }
        let t = if total > 0.0 { (upto / total).clamp(0.0, 1.0) } else { 0.0 };
        Some(Snap {
            edge: edge_id,
            t,
            point: norm_to_lon_lat(proj.0, proj.1),
            dist_m,
        })
    }

    // --- Routing ---

    pub fn route(&self, from: LonLat, to: LonLat, mode: TravelMode) -> Option<Route> {
        let start = self.snap(from, mode)?;
        let goal = self.snap(to, mode)?;
        self.route_snapped(start, goal, mode)
    }

    pub fn route_snapped(&self, start: Snap, goal: Snap, mode: TravelMode) -> Option<Route> {
        // Same-edge special case: travel within one edge if direction works.
        if start.edge == goal.edge {
            if let Some(route) = self.same_edge_route(&start, &goal, mode) {
                return Some(route);
            }
        }

        let goal_pos = goal.point;
        let vmax_mps = mode.max_speed_kmh() / 3.6;
        let heuristic = |v: u32| -> u64 {
            let p = self.vertex_lon_lat(v);
            (haversine_m(p, goal_pos) / vmax_mps * 1000.0) as u64
        };

        // State: (vertex, incoming edge). The incoming edge only matters at
        // restricted vias and for U-turn penalties; fold it to EDGE_NONE-ish
        // granularity would break restriction correctness, so keep it exact —
        // the state space stays ~edge count, fine at province scale.
        type State = (u32, u32); // (vertex, in_edge)
        let mut dist: HashMap<State, u64> = HashMap::new();
        let mut parent: HashMap<State, (State, u32)> = HashMap::new();
        let mut heap: BinaryHeap<std::cmp::Reverse<(u64, u32, u32)>> = BinaryHeap::new();

        // Seed: enter via the start edge (and/or its reverse).
        let seed = |heap: &mut BinaryHeap<std::cmp::Reverse<(u64, u32, u32)>>,
                    dist: &mut HashMap<State, u64>,
                    edge_id: u32,
                    frac: f64| {
            if edge_id == EDGE_NONE {
                return;
            }
            let edge = &self.edges[edge_id as usize];
            if !edge.allows(mode) {
                return;
            }
            let cost = (edge.cost_ms(mode) as f64 * frac) as u64;
            let state: State = (edge.to, edge_id);
            if dist.get(&state).map_or(true, |&d| cost < d) {
                dist.insert(state, cost);
                heap.push(std::cmp::Reverse((cost + heuristic(edge.to), edge.to, edge_id)));
            }
        };
        let start_edge = &self.edges[start.edge as usize];
        seed(&mut heap, &mut dist, start.edge, 1.0 - start.t);
        seed(&mut heap, &mut dist, start_edge.rev, start.t);

        // Goal entries: arriving at the from-vertex of an edge that carries
        // the goal snap, plus the partial cost along it.
        let goal_edge = &self.edges[goal.edge as usize];
        let mut goal_states: HashMap<u32, (u64, u32, f64)> = HashMap::new();
        if goal_edge.allows(mode) {
            let v = self.edge_from[goal.edge as usize];
            let cost = (goal_edge.cost_ms(mode) as f64 * goal.t) as u64;
            goal_states.insert(v, (cost, goal.edge, goal.t));
        }
        if goal_edge.rev != EDGE_NONE {
            let rev = &self.edges[goal_edge.rev as usize];
            if rev.allows(mode) {
                let v = self.edge_from[goal_edge.rev as usize];
                let cost = (rev.cost_ms(mode) as f64 * (1.0 - goal.t)) as u64;
                let insert = goal_states
                    .get(&v)
                    .map_or(true, |&(existing, _, _)| cost < existing);
                if insert {
                    goal_states.insert(v, (cost, goal_edge.rev, 1.0 - goal.t));
                }
            }
        }
        if goal_states.is_empty() {
            return None;
        }

        let mut best_total: Option<(u64, State, u32, f64)> = None; // cost, state, goal edge, frac
        let mut popped = 0usize;
        while let Some(std::cmp::Reverse((f_cost, v, in_edge))) = heap.pop() {
            let state: State = (v, in_edge);
            let g_cost = match dist.get(&state) {
                Some(&d) => d,
                None => continue,
            };
            if f_cost.saturating_sub(heuristic(v)) > g_cost {
                continue; // stale heap entry
            }
            popped += 1;
            if popped > 4_000_000 {
                break; // runaway guard
            }
            if let Some(&(tail_cost, g_edge, frac)) = goal_states.get(&v) {
                // Entering the goal edge is a turn too; respect bans.
                if !self.turn_banned(v, in_edge, g_edge) {
                    let total = g_cost + tail_cost;
                    if best_total.map_or(true, |(b, ..)| total < b) {
                        best_total = Some((total, state, g_edge, frac));
                    }
                }
            }
            if let Some((best, ..)) = best_total {
                if g_cost.saturating_add(heuristic(v)) >= best && heap.is_empty() {
                    break;
                }
                if g_cost > best {
                    break; // everything reachable now is worse
                }
            }
            for out_idx in self.out_edges(v) {
                let out_id = out_idx as u32;
                let edge = &self.edges[out_idx];
                if !edge.allows(mode) {
                    continue;
                }
                if self.turn_banned(v, in_edge, out_id) {
                    continue;
                }
                let mut step = edge.cost_ms(mode);
                // Discourage immediate U-turns for vehicles.
                if in_edge != EDGE_NONE
                    && self.edges[in_edge as usize].rev == out_id
                    && mode != TravelMode::Foot
                {
                    step = step.saturating_add(30_000);
                }
                let next_cost = g_cost.saturating_add(step);
                let next_state: State = (edge.to, out_id);
                if dist.get(&next_state).map_or(true, |&d| next_cost < d) {
                    dist.insert(next_state, next_cost);
                    parent.insert(next_state, (state, out_id));
                    heap.push(std::cmp::Reverse((
                        next_cost + heuristic(edge.to),
                        edge.to,
                        out_id,
                    )));
                }
            }
        }

        let (total_ms, mut state, tail_edge, tail_frac) = best_total?;

        // Reconstruct the edge chain (in reverse), then build geometry.
        let mut chain: Vec<u32> = Vec::new();
        loop {
            let (_, in_edge) = state;
            chain.push(in_edge);
            match parent.get(&state) {
                Some(&(prev, _)) => state = prev,
                None => break,
            }
        }
        chain.reverse();

        self.assemble_route(mode, &start, &goal, &chain, tail_edge, tail_frac, total_ms)
    }

    fn same_edge_route(&self, start: &Snap, goal: &Snap, mode: TravelMode) -> Option<Route> {
        let edge = &self.edges[start.edge as usize];
        let (t0, t1) = (start.t, goal.t);
        if (t1 - t0).abs() < 1e-9 {
            return None;
        }
        let forward_ok = t1 > t0 && edge.allows(mode);
        let backward_ok = t1 < t0
            && edge.rev != EDGE_NONE
            && self.edges[edge.rev as usize].allows(mode);
        if !forward_ok && !backward_ok {
            return None;
        }
        let pts = self.edge_points(start.edge);
        let sliced = slice_polyline(&pts, t0.min(t1), t0.max(t1));
        let mut points = sliced;
        if t1 < t0 {
            points.reverse();
        }
        let speed = if forward_ok {
            edge.speeds[mode.index()]
        } else {
            self.edges[edge.rev as usize].speeds[mode.index()]
        } as f64;
        let (points, cum) = with_cumulative(points);
        let length_m = *cum.last().unwrap_or(&0.0);
        let duration_s = length_m / (speed / 3.6);
        let maneuvers = nav::simple_maneuvers(&points, length_m);
        Some(Route {
            mode,
            points,
            cum_dist_m: cum,
            length_m,
            duration_s,
            maneuvers,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble_route(
        &self,
        mode: TravelMode,
        start: &Snap,
        goal: &Snap,
        chain: &[u32],
        tail_edge: u32,
        tail_frac: f64,
        total_ms: u64,
    ) -> Option<Route> {
        let mut points: Vec<LonLat> = Vec::new();
        let mut edge_seq: Vec<u32> = Vec::new();
        let mut point_edge_bounds: Vec<(usize, u32)> = Vec::new(); // start point idx per edge

        // Head partial: from the snap point to the end of the first chain edge.
        let first = *chain.first()?;
        {
            let pts = self.edge_points(first);
            // start.t is along the canonical edge; if `first` is the reverse
            // directed edge, the parameter flips.
            let t = if first == start.edge { start.t } else { 1.0 - start.t };
            let sliced = slice_polyline(&pts, t, 1.0);
            point_edge_bounds.push((points.len(), first));
            append_points(&mut points, &sliced);
            edge_seq.push(first);
        }
        for &edge_id in &chain[1..] {
            let pts = self.edge_points(edge_id);
            point_edge_bounds.push((points.len().saturating_sub(1), edge_id));
            append_points(&mut points, &pts);
            edge_seq.push(edge_id);
        }
        // Tail partial along the goal edge.
        if tail_frac > 1e-9 {
            let pts = self.edge_points(tail_edge);
            let sliced = slice_polyline(&pts, 0.0, tail_frac);
            point_edge_bounds.push((points.len().saturating_sub(1), tail_edge));
            append_points(&mut points, &sliced);
            edge_seq.push(tail_edge);
        }

        if points.len() < 2 {
            return None;
        }
        // Snap exact endpoints.
        if let Some(p) = points.first_mut() {
            *p = start.point;
        }
        if let Some(p) = points.last_mut() {
            *p = goal.point;
        }

        let (points, cum) = with_cumulative(points);
        let length_m = *cum.last().unwrap_or(&0.0);
        let duration_s = total_ms as f64 / 1000.0;
        let maneuvers = nav::generate_maneuvers(self, &edge_seq, &points, &cum, &point_edge_bounds, mode);
        Some(Route {
            mode,
            points,
            cum_dist_m: cum,
            length_m,
            duration_s,
            maneuvers,
        })
    }

    // --- Serialization ---

    pub fn serialize(&self) -> Vec<u8> {
        let mut w = ByteWriter::new();
        w.u32(GRAPH_MAGIC);
        w.u32(GRAPH_VERSION);
        w.u32(self.vertices.len() as u32);
        for (x, y) in &self.vertices {
            w.u32(*x);
            w.u32(*y);
        }
        w.u32(self.csr.len() as u32);
        for v in &self.csr {
            w.u32(*v);
        }
        w.u32(self.edges.len() as u32);
        for (edge, from) in self.edges.iter().zip(&self.edge_from) {
            w.u32(*from);
            w.u32(edge.to);
            w.u32(edge.rev);
            w.f32(edge.len_m);
            w.u8(edge.kind as u8);
            w.u8(edge.speeds[0]);
            w.u8(edge.speeds[1]);
            w.u8(edge.speeds[2]);
            w.u8(edge.flags);
            w.u32(edge.name_idx);
            w.u32(edge.geo_start);
            w.u16(edge.geo_count);
        }
        w.u32(self.geometry.len() as u32);
        for (x, y) in &self.geometry {
            w.u32(*x);
            w.u32(*y);
        }
        w.u32(self.names.len() as u32);
        for name in &self.names {
            w.str32(name);
        }
        let total_restrictions: u32 = self.restrictions.values().map(|v| v.len() as u32).sum();
        w.u32(total_restrictions);
        let mut vias: Vec<_> = self.restrictions.keys().copied().collect();
        vias.sort_unstable();
        for via in vias {
            for r in &self.restrictions[&via] {
                w.u32(via);
                w.u32(r.from_edge);
                w.u32(r.to_edge);
                w.u8(r.only as u8);
            }
        }
        // Grid
        w.f64(self.grid.min_x);
        w.f64(self.grid.min_y);
        w.f64(self.grid.cell);
        w.u32(self.grid.nx);
        w.u32(self.grid.ny);
        w.u32(self.grid.offsets.len() as u32);
        for v in &self.grid.offsets {
            w.u32(*v);
        }
        w.u32(self.grid.items.len() as u32);
        for v in &self.grid.items {
            w.u32(*v);
        }
        w.buf
    }

    pub fn deserialize(data: &[u8]) -> Result<RouteGraph, NavFmtError> {
        let mut r = ByteReader::new(data);
        if r.u32()? != GRAPH_MAGIC {
            return Err(NavFmtError::BadMagic);
        }
        let version = r.u32()?;
        if version != GRAPH_VERSION {
            return Err(NavFmtError::BadVersion(version));
        }
        let nv = r.u32()? as usize;
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            vertices.push((r.u32()?, r.u32()?));
        }
        let ncsr = r.u32()? as usize;
        if ncsr != nv + 1 {
            return Err(NavFmtError::Corrupt("csr length"));
        }
        let mut csr = Vec::with_capacity(ncsr);
        for _ in 0..ncsr {
            csr.push(r.u32()?);
        }
        let ne = r.u32()? as usize;
        let mut edges = Vec::with_capacity(ne);
        let mut edge_from = Vec::with_capacity(ne);
        for _ in 0..ne {
            edge_from.push(r.u32()?);
            edges.push(Edge {
                to: r.u32()?,
                rev: r.u32()?,
                len_m: r.f32()?,
                kind: RoadKind::from_u8(r.u8()?),
                speeds: [r.u8()?, r.u8()?, r.u8()?],
                flags: r.u8()?,
                name_idx: r.u32()?,
                geo_start: r.u32()?,
                geo_count: r.u16()?,
            });
        }
        let ng = r.u32()? as usize;
        let mut geometry = Vec::with_capacity(ng);
        for _ in 0..ng {
            geometry.push((r.u32()?, r.u32()?));
        }
        for edge in &edges {
            if edge.geo_start as usize + edge.geo_count as usize > geometry.len() {
                return Err(NavFmtError::Corrupt("edge geometry range"));
            }
        }
        let nn = r.u32()? as usize;
        let mut names = Vec::with_capacity(nn);
        for _ in 0..nn {
            names.push(r.str32()?);
        }
        let nr = r.u32()? as usize;
        let mut restrictions: HashMap<u32, Vec<TurnRestriction>> = HashMap::new();
        for _ in 0..nr {
            let via = r.u32()?;
            let from_edge = r.u32()?;
            let to_edge = r.u32()?;
            let only = r.u8()? != 0;
            restrictions.entry(via).or_default().push(TurnRestriction {
                from_edge,
                to_edge,
                only,
            });
        }
        let min_x = r.f64()?;
        let min_y = r.f64()?;
        let cell = r.f64()?;
        let nx = r.u32()?;
        let ny = r.u32()?;
        let noff = r.u32()? as usize;
        if noff != (nx as usize * ny as usize) + 1 {
            return Err(NavFmtError::Corrupt("grid offsets length"));
        }
        let mut offsets = Vec::with_capacity(noff);
        for _ in 0..noff {
            offsets.push(r.u32()?);
        }
        let nitems = r.u32()? as usize;
        let mut items = Vec::with_capacity(nitems);
        for _ in 0..nitems {
            items.push(r.u32()?);
        }
        Ok(RouteGraph {
            vertices,
            csr,
            edges,
            edge_from,
            geometry,
            names,
            restrictions,
            grid: SnapGrid {
                min_x,
                min_y,
                cell,
                nx,
                ny,
                offsets,
                items,
            },
        })
    }
}

fn append_points(points: &mut Vec<LonLat>, add: &[LonLat]) {
    for &p in add {
        if points.last().map_or(true, |&last| {
            (last.lon - p.lon).abs() > 1e-9 || (last.lat - p.lat).abs() > 1e-9
        }) {
            points.push(p);
        }
    }
}

fn with_cumulative(points: Vec<LonLat>) -> (Vec<LonLat>, Vec<f64>) {
    let mut cum = Vec::with_capacity(points.len());
    let mut total = 0.0;
    for i in 0..points.len() {
        if i > 0 {
            total += haversine_m(points[i - 1], points[i]);
        }
        cum.push(total);
    }
    (points, cum)
}

/// Slice a polyline by arc-length parameters t0..t1 (0..1), inclusive of
/// interpolated endpoints.
fn slice_polyline(points: &[LonLat], t0: f64, t1: f64) -> Vec<LonLat> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut seg_len = Vec::with_capacity(points.len() - 1);
    let mut total = 0.0;
    for i in 0..points.len() - 1 {
        let d = haversine_m(points[i], points[i + 1]);
        seg_len.push(d);
        total += d;
    }
    if total <= 0.0 {
        return vec![points[0], points[points.len() - 1]];
    }
    let d0 = t0.clamp(0.0, 1.0) * total;
    let d1 = t1.clamp(0.0, 1.0) * total;
    let interp = |target: f64| -> LonLat {
        let mut acc = 0.0;
        for i in 0..seg_len.len() {
            if acc + seg_len[i] >= target || i == seg_len.len() - 1 {
                let f = if seg_len[i] > 0.0 {
                    ((target - acc) / seg_len[i]).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return LonLat::new(
                    points[i].lon + (points[i + 1].lon - points[i].lon) * f,
                    points[i].lat + (points[i + 1].lat - points[i].lat) * f,
                );
            }
            acc += seg_len[i];
        }
        points[points.len() - 1]
    };
    let mut out = vec![interp(d0)];
    let mut acc = 0.0;
    for i in 0..seg_len.len() {
        acc += seg_len[i];
        if acc > d0 && acc < d1 {
            out.push(points[i + 1]);
        }
    }
    out.push(interp(d1));
    out
}

// --- Builder ---

#[derive(Clone, Debug)]
pub struct BuildRestriction {
    pub from_way: i64,
    pub via_node: i64,
    pub to_way: i64,
    pub only: bool,
}

struct BuildWay {
    osm_id: i64,
    node_ids: Vec<i64>,
    tags: HashMap<String, String>,
}

#[derive(Default)]
pub struct GraphBuilder {
    nodes: HashMap<i64, LonLat>,
    ways: Vec<BuildWay>,
    restrictions: Vec<BuildRestriction>,
}

struct WayAttrs {
    kind: RoadKind,
    /// Speeds in the forward node order.
    fwd: [u8; 3],
    /// Speeds against the node order (0 for oneway modes).
    back: [u8; 3],
    name: Option<String>,
    flags: u8,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, id: i64, lon: f64, lat: f64) {
        self.nodes.insert(id, LonLat::new(lon, lat));
    }

    pub fn add_way(&mut self, osm_id: i64, node_ids: Vec<i64>, tags: HashMap<String, String>) {
        if node_ids.len() >= 2 {
            self.ways.push(BuildWay {
                osm_id,
                node_ids,
                tags,
            });
        }
    }

    pub fn add_restriction(&mut self, r: BuildRestriction) {
        self.restrictions.push(r);
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn way_count(&self) -> usize {
        self.ways.len()
    }

    fn classify_way(tags: &HashMap<String, String>) -> Option<WayAttrs> {
        let get = |k: &str| tags.get(k).map(|s| s.as_str());
        let kind = if get("route") == Some("ferry") {
            RoadKind::Ferry
        } else {
            RoadKind::from_highway(get("highway")?)?
        };
        let mut speeds = kind.default_speeds();

        // Blanket access, then per-mode overrides.
        match get("access") {
            Some("no") | Some("private") => speeds = [0, 0, 0],
            _ => {}
        }
        match get("vehicle") {
            Some("no") | Some("private") => {
                speeds[0] = 0;
                speeds[1] = 0;
            }
            _ => {}
        }
        match get("motor_vehicle") {
            Some("no") | Some("private") => speeds[0] = 0,
            Some("yes") if speeds[0] == 0 && kind != RoadKind::Cycleway => {
                speeds[0] = kind.default_speeds()[0].max(15)
            }
            _ => {}
        }
        match get("bicycle") {
            Some("no") | Some("use_sidepath") => speeds[1] = 0,
            Some("yes") | Some("designated") | Some("permissive") if speeds[1] == 0 => {
                speeds[1] = 14;
            }
            _ => {}
        }
        match get("foot") {
            Some("no") => speeds[2] = 0,
            Some("yes") | Some("designated") | Some("permissive") if speeds[2] == 0 => {
                speeds[2] = 5;
            }
            _ => {}
        }
        if speeds == [0, 0, 0] {
            return None;
        }

        // maxspeed caps the car speed (never raises the routing default
        // above 105: NL motorway variable limits).
        if let Some(ms) = get("maxspeed").and_then(parse_maxspeed_kmh) {
            if speeds[0] > 0 {
                speeds[0] = ms.clamp(5.0, 105.0) as u8;
            }
        }

        let mut flags = 0u8;
        if get("junction") == Some("roundabout") || get("junction") == Some("circular") {
            flags |= EDGE_FLAG_ROUNDABOUT;
        }
        if matches!(
            kind,
            RoadKind::MotorwayLink
                | RoadKind::TrunkLink
                | RoadKind::PrimaryLink
                | RoadKind::SecondaryLink
                | RoadKind::TertiaryLink
        ) {
            flags |= EDGE_FLAG_LINK;
        }
        if get("bridge").is_some_and(|v| v != "no") {
            flags |= EDGE_FLAG_BRIDGE;
        }
        if get("tunnel").is_some_and(|v| v != "no") {
            flags |= EDGE_FLAG_TUNNEL;
        }

        // Oneway applies to car + bike (unless oneway:bicycle=no); never foot.
        let mut oneway_fwd = false;
        let mut oneway_rev = false;
        match get("oneway") {
            Some("yes") | Some("1") | Some("true") => oneway_fwd = true,
            Some("-1") | Some("reverse") => oneway_rev = true,
            _ => {
                if flags & EDGE_FLAG_ROUNDABOUT != 0 {
                    oneway_fwd = true;
                }
            }
        }
        let bike_oneway_exempt = get("oneway:bicycle") == Some("no");

        let mut fwd = speeds;
        let mut back = speeds;
        if oneway_fwd {
            back[0] = 0;
            if !bike_oneway_exempt {
                back[1] = 0;
            }
        }
        if oneway_rev {
            fwd[0] = 0;
            if !bike_oneway_exempt {
                fwd[1] = 0;
            }
        }

        let name = tags
            .get("name")
            .or_else(|| tags.get("ref"))
            .cloned()
            .filter(|s| !s.is_empty());

        Some(WayAttrs {
            kind,
            fwd,
            back,
            name,
            flags,
        })
    }

    pub fn build(self) -> RouteGraph {
        // 1. Classify ways; count node usage among kept ways.
        let mut kept: Vec<(usize, WayAttrs)> = Vec::new();
        let mut usage: HashMap<i64, u32> = HashMap::new();
        for (idx, way) in self.ways.iter().enumerate() {
            let Some(attrs) = Self::classify_way(&way.tags) else {
                continue;
            };
            let mut have_any = false;
            for node in &way.node_ids {
                if self.nodes.contains_key(node) {
                    *usage.entry(*node).or_insert(0) += 1;
                    have_any = true;
                }
            }
            if have_any {
                kept.push((idx, attrs));
            }
        }

        // 2. Vertices: nodes used >= 2x + way endpoints (with coords).
        let mut vertex_of: HashMap<i64, u32> = HashMap::new();
        let mut vertices: Vec<(u32, u32)> = Vec::new();
        let ensure_vertex = |node: i64, nodes: &HashMap<i64, LonLat>,
                                 vertex_of: &mut HashMap<i64, u32>,
                                 vertices: &mut Vec<(u32, u32)>|
         -> Option<u32> {
            let pos = nodes.get(&node)?;
            Some(*vertex_of.entry(node).or_insert_with(|| {
                let id = vertices.len() as u32;
                vertices.push(lon_lat_to_fixed(*pos));
                id
            }))
        };
        // A way with nodes missing from the input (outside the bbox) must
        // split into runs of consecutive present nodes, never jump the gap.
        let runs_of = |way: &BuildWay, nodes: &HashMap<i64, LonLat>| -> Vec<Vec<i64>> {
            let mut runs = Vec::new();
            let mut current: Vec<i64> = Vec::new();
            for node in &way.node_ids {
                if nodes.contains_key(node) {
                    current.push(*node);
                } else if !current.is_empty() {
                    if current.len() >= 2 {
                        runs.push(std::mem::take(&mut current));
                    } else {
                        current.clear();
                    }
                }
            }
            if current.len() >= 2 {
                runs.push(current);
            }
            runs
        };
        for (idx, _) in &kept {
            let way = &self.ways[*idx];
            for run in runs_of(way, &self.nodes) {
                for (i, node) in run.iter().enumerate() {
                    let is_junction = usage.get(node).copied().unwrap_or(0) >= 2;
                    if i == 0 || i == run.len() - 1 || is_junction {
                        ensure_vertex(*node, &self.nodes, &mut vertex_of, &mut vertices);
                    }
                }
            }
        }

        // 3. Split ways into segments between vertices; emit directed edges.
        struct RawEdge {
            from: u32,
            to: u32,
            rev_tmp: u32,
            len_m: f32,
            kind: RoadKind,
            speeds: [u8; 3],
            flags: u8,
            name_idx: u32,
            geo_start: u32,
            geo_count: u16,
        }
        let mut names: Vec<String> = Vec::new();
        let mut name_of: HashMap<String, u32> = HashMap::new();
        let mut geometry: Vec<(u32, u32)> = Vec::new();
        let mut raw_edges: Vec<RawEdge> = Vec::new();
        // Directed edge ids per way (for restriction resolution).
        let mut way_edges: HashMap<i64, Vec<u32>> = HashMap::new();

        for (idx, attrs) in &kept {
            let way = &self.ways[*idx];
            let name_idx = match &attrs.name {
                Some(name) => *name_of.entry(name.clone()).or_insert_with(|| {
                    names.push(name.clone());
                    (names.len() - 1) as u32
                }),
                None => NAME_NONE,
            };
            for run in runs_of(way, &self.nodes) {
            let mut seg_nodes: Vec<i64> = Vec::new();
            for (i, node) in run.iter().enumerate() {
                seg_nodes.push(*node);
                let at_vertex = vertex_of.contains_key(node) && i > 0;
                let at_end = i == run.len() - 1;
                if (at_vertex || at_end) && seg_nodes.len() >= 2 {
                    let from_v = vertex_of[&seg_nodes[0]];
                    let to_v = vertex_of[seg_nodes.last().unwrap()];
                    // Geometry pool: canonical forward order.
                    let geo_start = geometry.len() as u32;
                    let mut len_m = 0.0f64;
                    for (j, n) in seg_nodes.iter().enumerate() {
                        let p = self.nodes[n];
                        geometry.push(lon_lat_to_fixed(p));
                        if j > 0 {
                            len_m += haversine_m(self.nodes[&seg_nodes[j - 1]], p);
                        }
                    }
                    let geo_count = seg_nodes.len().min(u16::MAX as usize) as u16;
                    if geo_count as usize != seg_nodes.len() {
                        // Absurdly long segment; truncate geometry reference
                        // (routing still correct through len_m).
                        geometry.truncate(geo_start as usize + geo_count as usize);
                    }
                    let has_fwd = attrs.fwd.iter().any(|&s| s > 0);
                    let has_back = attrs.back.iter().any(|&s| s > 0);
                    let fwd_id = raw_edges.len() as u32;
                    if has_fwd {
                        raw_edges.push(RawEdge {
                            from: from_v,
                            to: to_v,
                            rev_tmp: if has_back { fwd_id + 1 } else { EDGE_NONE },
                            len_m: len_m as f32,
                            kind: attrs.kind,
                            speeds: attrs.fwd,
                            flags: attrs.flags,
                            name_idx,
                            geo_start,
                            geo_count,
                        });
                        way_edges.entry(way.osm_id).or_default().push(fwd_id);
                    }
                    if has_back {
                        let back_id = raw_edges.len() as u32;
                        raw_edges.push(RawEdge {
                            from: to_v,
                            to: from_v,
                            rev_tmp: if has_fwd { back_id - 1 } else { EDGE_NONE },
                            len_m: len_m as f32,
                            kind: attrs.kind,
                            speeds: attrs.back,
                            flags: attrs.flags | EDGE_FLAG_GEO_REVERSED,
                            name_idx,
                            geo_start,
                            geo_count,
                        });
                        way_edges.entry(way.osm_id).or_default().push(back_id);
                    }
                    seg_nodes.clear();
                    seg_nodes.push(*node);
                }
            }
            }
        }

        // 4. CSR sort by from-vertex; remap rev + way_edges ids.
        let mut order: Vec<u32> = (0..raw_edges.len() as u32).collect();
        order.sort_by_key(|&i| raw_edges[i as usize].from);
        let mut remap = vec![0u32; raw_edges.len()];
        for (new_id, &old_id) in order.iter().enumerate() {
            remap[old_id as usize] = new_id as u32;
        }
        let mut edges: Vec<Edge> = Vec::with_capacity(raw_edges.len());
        let mut edge_from: Vec<u32> = Vec::with_capacity(raw_edges.len());
        for &old_id in &order {
            let raw = &raw_edges[old_id as usize];
            edges.push(Edge {
                to: raw.to,
                rev: if raw.rev_tmp == EDGE_NONE {
                    EDGE_NONE
                } else {
                    remap[raw.rev_tmp as usize]
                },
                len_m: raw.len_m,
                kind: raw.kind,
                speeds: raw.speeds,
                flags: raw.flags,
                name_idx: raw.name_idx,
                geo_start: raw.geo_start,
                geo_count: raw.geo_count,
            });
            edge_from.push(raw.from);
        }
        let mut csr = vec![0u32; vertices.len() + 1];
        for from in &edge_from {
            csr[*from as usize + 1] += 1;
        }
        for i in 1..csr.len() {
            csr[i] += csr[i - 1];
        }
        for ids in way_edges.values_mut() {
            for id in ids.iter_mut() {
                *id = remap[*id as usize];
            }
        }

        // 5. Turn restrictions: resolve (from_way, via_node, to_way).
        let mut restrictions: HashMap<u32, Vec<TurnRestriction>> = HashMap::new();
        for r in &self.restrictions {
            let Some(&via_vertex) = vertex_of.get(&r.via_node) else {
                continue;
            };
            let Some(from_ids) = way_edges.get(&r.from_way) else {
                continue;
            };
            let Some(to_ids) = way_edges.get(&r.to_way) else {
                continue;
            };
            let incoming: Vec<u32> = from_ids
                .iter()
                .copied()
                .filter(|&e| edges[e as usize].to == via_vertex)
                .collect();
            let outgoing: Vec<u32> = to_ids
                .iter()
                .copied()
                .filter(|&e| edge_from[e as usize] == via_vertex)
                .collect();
            for &fe in &incoming {
                for &te in &outgoing {
                    restrictions.entry(via_vertex).or_default().push(TurnRestriction {
                        from_edge: fe,
                        to_edge: te,
                        only: r.only,
                    });
                }
            }
        }

        // 6. Snap grid over canonical edges (~250m cells).
        let grid = build_grid(&vertices, &edges, &geometry);

        RouteGraph {
            vertices,
            csr,
            edges,
            edge_from,
            geometry,
            names,
            restrictions,
            grid,
        }
    }
}

fn parse_maxspeed_kmh(v: &str) -> Option<f64> {
    let v = v.trim();
    if v == "walk" {
        return Some(8.0);
    }
    if v == "none" {
        return Some(105.0);
    }
    if let Some(mph) = v.strip_suffix("mph").map(|s| s.trim()) {
        return mph.parse::<f64>().ok().map(|n| n * 1.609_344);
    }
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<f64>().ok()
}

fn build_grid(vertices: &[(u32, u32)], edges: &[Edge], geometry: &[(u32, u32)]) -> SnapGrid {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for &(x, y) in vertices {
        let (fx, fy) = (fixed_to_norm(x), fixed_to_norm(y));
        min_x = min_x.min(fx);
        min_y = min_y.min(fy);
        max_x = max_x.max(fx);
        max_y = max_y.max(fy);
    }
    if vertices.is_empty() {
        min_x = 0.0;
        min_y = 0.0;
        max_x = 1.0;
        max_y = 1.0;
    }
    let mid_lat = norm_to_lon_lat((min_x + max_x) * 0.5, (min_y + max_y) * 0.5).lat;
    let meters = meters_per_norm_unit(mid_lat).max(1.0);
    let cell = (250.0 / meters).max(1e-9);
    let span_x = (max_x - min_x).max(cell);
    let span_y = (max_y - min_y).max(cell);
    let nx = ((span_x / cell).ceil() as u32 + 1).clamp(1, 4096);
    let ny = ((span_y / cell).ceil() as u32 + 1).clamp(1, 4096);
    // Recompute cell so the clamped grid still covers the span.
    let cell = (span_x / nx as f64).max(span_y / ny as f64).max(cell);

    let mut cell_items: Vec<Vec<u32>> = vec![Vec::new(); nx as usize * ny as usize];
    for (edge_id, edge) in edges.iter().enumerate() {
        // Canonical direction only (the one whose geometry runs forward).
        if edge.flags & EDGE_FLAG_GEO_REVERSED != 0 && edge.rev != EDGE_NONE {
            continue;
        }
        let geo = &geometry[edge.geo_start as usize..edge.geo_start as usize + edge.geo_count as usize];
        let mut e_min_x = f64::MAX;
        let mut e_min_y = f64::MAX;
        let mut e_max_x = f64::MIN;
        let mut e_max_y = f64::MIN;
        for &(x, y) in geo {
            let (fx, fy) = (fixed_to_norm(x), fixed_to_norm(y));
            e_min_x = e_min_x.min(fx);
            e_min_y = e_min_y.min(fy);
            e_max_x = e_max_x.max(fx);
            e_max_y = e_max_y.max(fy);
        }
        if e_min_x > e_max_x {
            continue;
        }
        let cx0 = (((e_min_x - min_x) / cell).floor().max(0.0) as u32).min(nx - 1);
        let cx1 = (((e_max_x - min_x) / cell).floor().max(0.0) as u32).min(nx - 1);
        let cy0 = (((e_min_y - min_y) / cell).floor().max(0.0) as u32).min(ny - 1);
        let cy1 = (((e_max_y - min_y) / cell).floor().max(0.0) as u32).min(ny - 1);
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                cell_items[(cy * nx + cx) as usize].push(edge_id as u32);
            }
        }
    }

    let mut offsets = Vec::with_capacity(cell_items.len() + 1);
    let mut items = Vec::new();
    offsets.push(0u32);
    for cell_list in &cell_items {
        items.extend_from_slice(cell_list);
        offsets.push(items.len() as u32);
    }
    SnapGrid {
        min_x,
        min_y,
        cell,
        nx,
        ny,
        offsets,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x3 grid of streets, ~500m apart, node ids row*10+col:
    ///   00--01--02
    ///   |   |   |
    ///   10--11--12
    ///   |   |   |
    ///   20--21--22
    fn grid_builder() -> GraphBuilder {
        let mut b = GraphBuilder::new();
        let base = LonLat::new(4.9, 52.37);
        let dlon = 0.0073; // ~500m at 52.37N
        let dlat = 0.0045; // ~500m
        for row in 0..3i64 {
            for col in 0..3i64 {
                b.add_node(
                    row * 10 + col,
                    base.lon + col as f64 * dlon,
                    base.lat - row as f64 * dlat,
                );
            }
        }
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "residential".to_string());
        let mut way_id = 100;
        for row in 0..3i64 {
            b.add_way(way_id, vec![row * 10, row * 10 + 1, row * 10 + 2], tags.clone());
            way_id += 1;
        }
        for col in 0..3i64 {
            b.add_way(way_id, vec![col, 10 + col, 20 + col], tags.clone());
            way_id += 1;
        }
        b
    }

    fn node_pos(row: i64, col: i64) -> LonLat {
        LonLat::new(4.9 + col as f64 * 0.0073, 52.37 - row as f64 * 0.0045)
    }

    #[test]
    fn builds_grid_graph() {
        let graph = grid_builder().build();
        assert_eq!(graph.vertices.len(), 9);
        // Every street is two-way: 2 directed edges per segment, 12 segments.
        assert_eq!(graph.edges.len(), 24);
        assert_eq!(*graph.csr.last().unwrap() as usize, graph.edges.len());
    }

    #[test]
    fn routes_straight_line() {
        let graph = grid_builder().build();
        let route = graph
            .route(node_pos(0, 0), node_pos(0, 2), TravelMode::Car)
            .expect("route");
        // ~1km straight along the top row.
        assert!(route.length_m > 800.0 && route.length_m < 1300.0, "len {}", route.length_m);
        assert!(route.duration_s > 0.0);
        assert!(route.points.len() >= 2);
        // Ends near the requested points.
        assert!(haversine_m(route.points[0], node_pos(0, 0)) < 50.0);
        assert!(haversine_m(*route.points.last().unwrap(), node_pos(0, 2)) < 50.0);
    }

    #[test]
    fn snap_finds_nearest_street() {
        let graph = grid_builder().build();
        // Slightly north of the middle of the top-left horizontal segment.
        let p = LonLat::new(4.9036, 52.3705);
        let snap = graph.snap(p, TravelMode::Car).expect("snap");
        assert!(snap.dist_m < 100.0, "dist {}", snap.dist_m);
        assert!(snap.t > 0.1 && snap.t < 0.9, "t {}", snap.t);
    }

    #[test]
    fn oneway_forces_detour() {
        let mut b = GraphBuilder::new();
        // Two parallel horizontal streets connected at both ends.
        //  0--1  (oneway east-to-west: travel 0->1 must detour via 10/11)
        // 10--11
        b.add_node(0, 4.9, 52.37);
        b.add_node(1, 4.9073, 52.37);
        b.add_node(10, 4.9, 52.3655);
        b.add_node(11, 4.9073, 52.3655);
        let mut oneway = HashMap::new();
        oneway.insert("highway".to_string(), "residential".to_string());
        oneway.insert("oneway".to_string(), "-1".to_string()); // 1 -> 0 only
        let mut twoway = HashMap::new();
        twoway.insert("highway".to_string(), "residential".to_string());
        b.add_way(1, vec![0, 1], oneway);
        b.add_way(2, vec![10, 11], twoway.clone());
        b.add_way(3, vec![0, 10], twoway.clone());
        b.add_way(4, vec![1, 11], twoway);
        let graph = b.build();

        let car = graph
            .route(LonLat::new(4.9, 52.37), LonLat::new(4.9073, 52.37), TravelMode::Car)
            .expect("car route");
        // Must detour: down, across, up => ~3x the direct distance.
        assert!(car.length_m > 1200.0, "car len {}", car.length_m);

        let foot = graph
            .route(LonLat::new(4.9, 52.37), LonLat::new(4.9073, 52.37), TravelMode::Foot)
            .expect("foot route");
        // Foot ignores oneway: straight across ~500m.
        assert!(foot.length_m < 700.0, "foot len {}", foot.length_m);
    }

    #[test]
    fn no_left_turn_forces_detour() {
        //      2
        //      |
        //  0---1
        //      |
        //      3
        // Ban the turn 0->1 -> 1->2 (left); route 0 -> 2 must go via 3? No —
        // with only these streets the router must U-turn or take 1->3 then
        // back. Add a loop: 3--4--2 so a legal alternative exists.
        let mut b = GraphBuilder::new();
        b.add_node(0, 4.9, 52.37);
        b.add_node(1, 4.9073, 52.37);
        b.add_node(2, 4.9073, 52.3745);
        b.add_node(3, 4.9073, 52.3655);
        b.add_node(4, 4.9146, 52.3655);
        b.add_node(5, 4.9146, 52.3745);
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "residential".to_string());
        b.add_way(1, vec![0, 1], tags.clone());
        b.add_way(2, vec![1, 2], tags.clone());
        b.add_way(3, vec![1, 3], tags.clone());
        b.add_way(4, vec![3, 4], tags.clone());
        b.add_way(5, vec![4, 5], tags.clone());
        b.add_way(6, vec![5, 2], tags.clone());

        let start = LonLat::new(4.9, 52.37);
        let end = LonLat::new(4.9073, 52.3745);

        let unrestricted = b.build();
        let direct = unrestricted.route(start, end, TravelMode::Car).expect("direct");

        let mut b2 = GraphBuilder::new();
        b2.add_node(0, 4.9, 52.37);
        b2.add_node(1, 4.9073, 52.37);
        b2.add_node(2, 4.9073, 52.3745);
        b2.add_node(3, 4.9073, 52.3655);
        b2.add_node(4, 4.9146, 52.3655);
        b2.add_node(5, 4.9146, 52.3745);
        b2.add_way(1, vec![0, 1], tags.clone());
        b2.add_way(2, vec![1, 2], tags.clone());
        b2.add_way(3, vec![1, 3], tags.clone());
        b2.add_way(4, vec![3, 4], tags.clone());
        b2.add_way(5, vec![4, 5], tags.clone());
        b2.add_way(6, vec![5, 2], tags);
        b2.add_restriction(BuildRestriction {
            from_way: 1,
            via_node: 1,
            to_way: 2,
            only: false,
        });
        let restricted = b2.build();
        let detour = restricted.route(start, end, TravelMode::Car).expect("detour");

        assert!(
            detour.length_m > direct.length_m * 1.5,
            "direct {} detour {}",
            direct.length_m,
            detour.length_m
        );
    }

    #[test]
    fn car_banned_from_cycleway() {
        let mut b = GraphBuilder::new();
        b.add_node(0, 4.9, 52.37);
        b.add_node(1, 4.9073, 52.37);
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "cycleway".to_string());
        b.add_way(1, vec![0, 1], tags);
        let graph = b.build();
        assert!(graph
            .route(LonLat::new(4.9, 52.37), LonLat::new(4.9073, 52.37), TravelMode::Car)
            .is_none());
        assert!(graph
            .route(LonLat::new(4.9, 52.37), LonLat::new(4.9073, 52.37), TravelMode::Bike)
            .is_some());
    }

    #[test]
    fn same_edge_route() {
        let mut b = GraphBuilder::new();
        b.add_node(0, 4.9, 52.37);
        b.add_node(1, 4.9146, 52.37);
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "residential".to_string());
        b.add_way(1, vec![0, 1], tags);
        let graph = b.build();
        let route = graph
            .route(LonLat::new(4.9030, 52.3702), LonLat::new(4.9110, 52.3702), TravelMode::Foot)
            .expect("route");
        assert!(route.length_m > 400.0 && route.length_m < 700.0, "len {}", route.length_m);
    }

    #[test]
    fn way_with_missing_nodes_splits_into_runs() {
        // Way 0-1-2-3 where node 2 has no coords (outside bbox): must yield
        // 0-1 and (nothing) — never a phantom edge jumping 1 -> 3.
        let mut b = GraphBuilder::new();
        b.add_node(0, 4.9, 52.37);
        b.add_node(1, 4.9073, 52.37);
        b.add_node(3, 4.9219, 52.37);
        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "residential".to_string());
        b.add_way(1, vec![0, 1, 2, 3], tags);
        let graph = b.build();
        // Node 3 forms a length-1 run: dropped. Only 0-1 remains (2 directed).
        assert_eq!(graph.edges.len(), 2);
        for edge in &graph.edges {
            assert!(edge.len_m < 600.0, "phantom gap edge len {}", edge.len_m);
        }
    }

    #[test]
    fn serialization_roundtrip() {
        let graph = grid_builder().build();
        let bytes = graph.serialize();
        let loaded = RouteGraph::deserialize(&bytes).unwrap();
        assert_eq!(loaded.vertices.len(), graph.vertices.len());
        assert_eq!(loaded.edges.len(), graph.edges.len());
        let a = graph
            .route(node_pos(0, 0), node_pos(2, 2), TravelMode::Bike)
            .expect("route a");
        let b = loaded
            .route(node_pos(0, 0), node_pos(2, 2), TravelMode::Bike)
            .expect("route b");
        assert!((a.length_m - b.length_m).abs() < 1.0);
        assert_eq!(a.points.len(), b.points.len());
    }
}
