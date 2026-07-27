//! Turn instruction generation and the `NavSession` state machine that
//! map-matches live position samples onto an active route, tracks progress
//! and detects going off-route.

use crate::geo::*;
use crate::graph::{
    Edge, Route, RouteGraph, TravelMode, EDGE_FLAG_ROUNDABOUT, EDGE_NONE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManeuverKind {
    Depart,
    Arrive,
    TurnSlightLeft,
    TurnLeft,
    TurnSharpLeft,
    TurnSlightRight,
    TurnRight,
    TurnSharpRight,
    UTurn,
    RoundaboutExit(u8),
}

impl ManeuverKind {
    pub fn instruction(&self) -> String {
        match self {
            ManeuverKind::Depart => "Depart".to_string(),
            ManeuverKind::Arrive => "Arrive at destination".to_string(),
            ManeuverKind::TurnSlightLeft => "Bear left".to_string(),
            ManeuverKind::TurnLeft => "Turn left".to_string(),
            ManeuverKind::TurnSharpLeft => "Turn sharply left".to_string(),
            ManeuverKind::TurnSlightRight => "Bear right".to_string(),
            ManeuverKind::TurnRight => "Turn right".to_string(),
            ManeuverKind::TurnSharpRight => "Turn sharply right".to_string(),
            ManeuverKind::UTurn => "Make a U-turn".to_string(),
            ManeuverKind::RoundaboutExit(n) => {
                let ord = match n {
                    1 => "1st",
                    2 => "2nd",
                    3 => "3rd",
                    n => return format!("At the roundabout, take exit {}", n),
                };
                format!("At the roundabout, take the {} exit", ord)
            }
        }
    }

    /// Arrow glyph for compact banner display.
    pub fn arrow(&self) -> &'static str {
        match self {
            ManeuverKind::Depart => "▲",
            ManeuverKind::Arrive => "⚑",
            ManeuverKind::TurnSlightLeft => "↖",
            ManeuverKind::TurnLeft => "←",
            ManeuverKind::TurnSharpLeft => "↙",
            ManeuverKind::TurnSlightRight => "↗",
            ManeuverKind::TurnRight => "→",
            ManeuverKind::TurnSharpRight => "↘",
            ManeuverKind::UTurn => "↩",
            ManeuverKind::RoundaboutExit(_) => "↻",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Maneuver {
    pub kind: ManeuverKind,
    pub at: LonLat,
    /// Street name after the maneuver (empty when unnamed).
    pub name: String,
    /// Distance from route start, meters.
    pub dist_m: f64,
    /// Index into the route's point array.
    pub point_index: usize,
}

impl Maneuver {
    /// "Turn left onto Prinsengracht" style text.
    pub fn text(&self) -> String {
        let base = self.kind.instruction();
        if self.name.is_empty()
            || matches!(self.kind, ManeuverKind::Arrive | ManeuverKind::Depart)
        {
            base
        } else {
            format!("{} onto {}", base, self.name)
        }
    }
}

/// Depart + Arrive only, for trivial single-edge routes.
pub fn simple_maneuvers(points: &[LonLat], length_m: f64) -> Vec<Maneuver> {
    let mut out = Vec::new();
    if points.is_empty() {
        return out;
    }
    out.push(Maneuver {
        kind: ManeuverKind::Depart,
        at: points[0],
        name: String::new(),
        dist_m: 0.0,
        point_index: 0,
    });
    out.push(Maneuver {
        kind: ManeuverKind::Arrive,
        at: *points.last().unwrap(),
        name: String::new(),
        dist_m: length_m,
        point_index: points.len() - 1,
    });
    out
}

fn kind_for_delta(delta: f64) -> Option<ManeuverKind> {
    let mag = delta.abs();
    if mag < 25.0 {
        None
    } else if mag > 165.0 {
        Some(ManeuverKind::UTurn)
    } else if delta < 0.0 {
        Some(if mag < 60.0 {
            ManeuverKind::TurnSlightLeft
        } else if mag < 130.0 {
            ManeuverKind::TurnLeft
        } else {
            ManeuverKind::TurnSharpLeft
        })
    } else {
        Some(if mag < 60.0 {
            ManeuverKind::TurnSlightRight
        } else if mag < 130.0 {
            ManeuverKind::TurnRight
        } else {
            ManeuverKind::TurnSharpRight
        })
    }
}

/// Bearing of the route at point index `at`, looking `dir_m` meters along
/// the polyline (negative = backwards).
fn route_bearing(points: &[LonLat], cum: &[f64], at: usize, dir_m: f64) -> f64 {
    let target = cum[at] + dir_m;
    if dir_m < 0.0 {
        let mut j = at;
        while j > 0 && cum[j - 1] > target {
            j -= 1;
        }
        let j = j.saturating_sub(1).min(at.saturating_sub(1));
        bearing_deg(points[j], points[at])
    } else {
        let mut j = at;
        while j + 1 < points.len() && cum[j + 1] < target {
            j += 1;
        }
        let j = (j + 1).min(points.len() - 1).max(at + 1).min(points.len() - 1);
        bearing_deg(points[at], points[j])
    }
}

/// Number of route-choice alternatives at `vertex` when arriving via
/// `in_edge` (excluding the immediate reverse), for the given mode.
fn out_options(graph: &RouteGraph, vertex: u32, in_edge: u32, mode: TravelMode) -> usize {
    let start = graph.csr[vertex as usize] as usize;
    let end = graph.csr[vertex as usize + 1] as usize;
    let rev_of_in = if in_edge == EDGE_NONE {
        EDGE_NONE
    } else {
        graph.edges[in_edge as usize].rev
    };
    (start..end)
        .filter(|&i| {
            let edge: &Edge = &graph.edges[i];
            edge.allows(mode) && (i as u32) != rev_of_in
        })
        .count()
}

/// Walk the routed edge sequence and emit turn instructions where bearing
/// change + road topology warrant them. `edge_bounds` holds the point index
/// where each edge of `edge_seq` starts.
pub fn generate_maneuvers(
    graph: &RouteGraph,
    edge_seq: &[u32],
    points: &[LonLat],
    cum: &[f64],
    edge_bounds: &[(usize, u32)],
    mode: TravelMode,
) -> Vec<Maneuver> {
    let mut out = Vec::new();
    if points.len() < 2 || edge_seq.is_empty() {
        return simple_maneuvers(points, *cum.last().unwrap_or(&0.0));
    }
    let first_name = graph
        .edge_name(&graph.edges[edge_seq[0] as usize])
        .unwrap_or("")
        .to_string();
    out.push(Maneuver {
        kind: ManeuverKind::Depart,
        at: points[0],
        name: first_name,
        dist_m: 0.0,
        point_index: 0,
    });

    let mut i = 0usize;
    while i + 1 < edge_seq.len() {
        let in_edge_id = edge_seq[i];
        let out_edge_id = edge_seq[i + 1];
        let in_edge = &graph.edges[in_edge_id as usize];
        let out_edge = &graph.edges[out_edge_id as usize];
        let boundary_point = edge_bounds
            .get(i + 1)
            .map(|&(p, _)| p)
            .unwrap_or(0)
            .min(points.len() - 1);

        // Roundabout: collapse the in-roundabout edges into one instruction.
        if out_edge.flags & EDGE_FLAG_ROUNDABOUT != 0
            && in_edge.flags & EDGE_FLAG_ROUNDABOUT == 0
        {
            let mut exits = 0u8;
            let mut j = i + 1;
            while j < edge_seq.len() {
                let edge = &graph.edges[edge_seq[j] as usize];
                if edge.flags & EDGE_FLAG_ROUNDABOUT == 0 {
                    break;
                }
                // Count exit opportunities at the far vertex of each
                // roundabout edge (non-roundabout outgoing, allowed).
                let vertex = edge.to;
                let start = graph.csr[vertex as usize] as usize;
                let end = graph.csr[vertex as usize + 1] as usize;
                let has_exit = (start..end).any(|k| {
                    let e = &graph.edges[k];
                    e.allows(mode) && e.flags & EDGE_FLAG_ROUNDABOUT == 0
                });
                if has_exit {
                    exits = exits.saturating_add(1);
                }
                j += 1;
            }
            let after_name = if j < edge_seq.len() {
                graph
                    .edge_name(&graph.edges[edge_seq[j] as usize])
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            };
            out.push(Maneuver {
                kind: ManeuverKind::RoundaboutExit(exits.max(1)),
                at: points[boundary_point],
                name: after_name,
                dist_m: cum[boundary_point],
                point_index: boundary_point,
            });
            i = j;
            continue;
        }

        let in_bearing = route_bearing(points, cum, boundary_point, -10.0);
        let out_bearing = route_bearing(points, cum, boundary_point, 10.0);
        let delta = bearing_delta_deg(in_bearing, out_bearing);
        let vertex = in_edge.to;
        let options = out_options(graph, vertex, in_edge_id, mode);

        // A forced bend on a road without alternatives is not a turn.
        if options > 1 || delta.abs() > 100.0 {
            if let Some(kind) = kind_for_delta(delta) {
                let name = graph.edge_name(out_edge).unwrap_or("").to_string();
                // Merge maneuvers landing within ~25m of the previous one —
                // micro-edges on squares/complex junctions otherwise emit
                // "turn left" three times for one physical turn.
                let boundary_dist = cum[boundary_point];
                let dup = out.last().is_some_and(|m: &Maneuver| {
                    (m.dist_m - boundary_dist).abs() < 25.0 && m.kind != ManeuverKind::Depart
                });
                if !dup {
                    out.push(Maneuver {
                        kind,
                        at: points[boundary_point],
                        name,
                        dist_m: boundary_dist,
                        point_index: boundary_point,
                    });
                }
            }
        }
        i += 1;
    }

    let total = *cum.last().unwrap_or(&0.0);
    out.push(Maneuver {
        kind: ManeuverKind::Arrive,
        at: *points.last().unwrap(),
        name: String::new(),
        dist_m: total,
        point_index: points.len() - 1,
    });
    out
}

// --- NavSession ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavState {
    Navigating,
    OffRoute,
    Arrived,
}

#[derive(Clone, Debug)]
pub struct NavStatus {
    pub state: NavState,
    /// Position snapped onto the route polyline.
    pub matched: LonLat,
    pub progress_m: f64,
    pub remaining_m: f64,
    pub remaining_s: f64,
    /// Index into `route.maneuvers` of the upcoming maneuver.
    pub next_maneuver: Option<usize>,
    pub dist_to_next_m: f64,
    /// Distance from the raw position to the route.
    pub deviation_m: f64,
    /// Off-route persisted long enough that the app should recompute.
    pub needs_reroute: bool,
}

/// Distance from route beyond which a sample counts as off-route.
const OFF_ROUTE_DIST_M: f64 = 35.0;
/// Seconds of continuous off-route before requesting a reroute.
const OFF_ROUTE_GRACE_S: f64 = 4.0;
/// Within this distance of the end (and close to the route) = arrived.
const ARRIVE_DIST_M: f64 = 22.0;

pub struct NavSession {
    route: Route,
    progress_m: f64,
    matched_index: usize,
    off_route_since: Option<f64>,
    arrived: bool,
}

impl NavSession {
    pub fn new(route: Route) -> Self {
        Self {
            route,
            progress_m: 0.0,
            matched_index: 0,
            off_route_since: None,
            arrived: false,
        }
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    pub fn progress_m(&self) -> f64 {
        self.progress_m
    }

    /// Feed a position sample; `now_s` is any monotonic clock in seconds.
    pub fn update(&mut self, pos: LonLat, now_s: f64) -> NavStatus {
        let points = &self.route.points;
        let cum = &self.route.cum_dist_m;
        let total = self.route.length_m;
        let meters = meters_per_norm_unit(pos.lat);
        let (px, py) = lon_lat_to_norm(pos);

        // Match within a window ahead of current progress (monotonic — noise
        // never snaps the puck backwards), with a little backwards slack.
        let window_ahead_m = 300.0;
        let back_slack_m = 40.0;
        let start_idx = {
            let target = (self.progress_m - back_slack_m).max(0.0);
            let mut i = self.matched_index.min(points.len().saturating_sub(1));
            while i > 0 && cum[i] > target {
                i -= 1;
            }
            i
        };
        let mut best: Option<(f64, usize, f64)> = None; // dist_m, seg index, t
        let mut i = start_idx;
        while i + 1 < points.len() {
            if cum[i] > self.progress_m + window_ahead_m {
                break;
            }
            let a = lon_lat_to_norm(points[i]);
            let b = lon_lat_to_norm(points[i + 1]);
            let (proj, t) = project_on_segment((px, py), a, b);
            let d = ((proj.0 - px).powi(2) + (proj.1 - py).powi(2)).sqrt() * meters;
            if best.is_none() || d < best.unwrap().0 {
                best = Some((d, i, t));
            }
            i += 1;
        }

        let (deviation_m, seg, t) = best.unwrap_or((f64::MAX, self.matched_index, 0.0));
        let seg_len = if seg + 1 < cum.len() { cum[seg + 1] - cum[seg] } else { 0.0 };
        let sample_progress = cum[seg] + seg_len * t;
        let matched = if deviation_m.is_finite() && seg + 1 < points.len() {
            LonLat::new(
                points[seg].lon + (points[seg + 1].lon - points[seg].lon) * t,
                points[seg].lat + (points[seg + 1].lat - points[seg].lat) * t,
            )
        } else {
            pos
        };

        let on_route = deviation_m < OFF_ROUTE_DIST_M;
        if on_route {
            self.off_route_since = None;
            if sample_progress > self.progress_m {
                self.progress_m = sample_progress;
                self.matched_index = seg;
            }
        } else if self.off_route_since.is_none() {
            self.off_route_since = Some(now_s);
        }
        let needs_reroute = self
            .off_route_since
            .is_some_and(|since| now_s - since > OFF_ROUTE_GRACE_S);

        let remaining_m = (total - self.progress_m).max(0.0);
        if !self.arrived
            && remaining_m < ARRIVE_DIST_M
            && deviation_m < OFF_ROUTE_DIST_M * 2.0
        {
            self.arrived = true;
        }

        // ETA from the route's average speed over the remaining stretch.
        let avg_speed = if self.route.duration_s > 0.0 {
            total / self.route.duration_s
        } else {
            1.0
        };
        let remaining_s = remaining_m / avg_speed.max(0.1);

        let next_maneuver = self
            .route
            .maneuvers
            .iter()
            .position(|m| m.dist_m > self.progress_m + 3.0 || m.kind == ManeuverKind::Arrive);
        let dist_to_next_m = next_maneuver
            .map(|idx| (self.route.maneuvers[idx].dist_m - self.progress_m).max(0.0))
            .unwrap_or(remaining_m);

        let state = if self.arrived {
            NavState::Arrived
        } else if !on_route {
            NavState::OffRoute
        } else {
            NavState::Navigating
        };

        NavStatus {
            state,
            matched: if on_route { matched } else { pos },
            progress_m: self.progress_m,
            remaining_m,
            remaining_s,
            next_maneuver,
            dist_to_next_m,
            deviation_m,
            needs_reroute,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphBuilder, TravelMode};
    use std::collections::HashMap;

    fn l_route() -> Route {
        // L-shape: go east ~1km, turn right (south) ~1km. A third arm
        // continues east so the corner is a real junction with a choice.
        let mut b = GraphBuilder::new();
        b.add_node(0, 4.9, 52.37);
        b.add_node(1, 4.9146, 52.37);
        b.add_node(2, 4.9146, 52.361);
        b.add_node(3, 4.9292, 52.37);
        let mut tags_a = HashMap::new();
        tags_a.insert("highway".to_string(), "residential".to_string());
        tags_a.insert("name".to_string(), "Eaststreet".to_string());
        let mut tags_b = HashMap::new();
        tags_b.insert("highway".to_string(), "residential".to_string());
        tags_b.insert("name".to_string(), "Southlane".to_string());
        b.add_way(1, vec![0, 1], tags_a.clone());
        b.add_way(2, vec![1, 2], tags_b);
        b.add_way(3, vec![1, 3], tags_a);
        let graph = b.build();
        graph
            .route(LonLat::new(4.9, 52.37), LonLat::new(4.9146, 52.361), TravelMode::Car)
            .expect("route")
    }

    #[test]
    fn l_shape_gets_right_turn() {
        let route = l_route();
        let kinds: Vec<_> = route.maneuvers.iter().map(|m| m.kind).collect();
        assert_eq!(kinds.first(), Some(&ManeuverKind::Depart));
        assert_eq!(kinds.last(), Some(&ManeuverKind::Arrive));
        assert!(
            kinds.contains(&ManeuverKind::TurnRight),
            "kinds {:?}",
            kinds
        );
        let turn = route
            .maneuvers
            .iter()
            .find(|m| m.kind == ManeuverKind::TurnRight)
            .unwrap();
        assert_eq!(turn.name, "Southlane");
        assert!(turn.dist_m > 800.0 && turn.dist_m < 1200.0, "at {}", turn.dist_m);
    }

    #[test]
    fn session_tracks_progress_and_arrives() {
        let route = l_route();
        let total = route.length_m;
        let points = route.points.clone();
        let cum = route.cum_dist_m.clone();
        let mut session = NavSession::new(route);

        // Drive the route by sampling every ~50m with tiny noise.
        let mut now = 0.0;
        let mut last_remaining = f64::MAX;
        let mut arrived = false;
        let mut target = 0.0;
        while target <= total {
            let idx = cum.partition_point(|&c| c < target).min(points.len() - 1);
            let p = points[idx];
            let noisy = LonLat::new(p.lon + 0.00003, p.lat - 0.00002); // ~2-3m
            let status = session.update(noisy, now);
            assert!(!status.needs_reroute, "unexpected reroute at {}m", target);
            assert!(status.remaining_m <= last_remaining + 1.0);
            last_remaining = status.remaining_m;
            if status.state == NavState::Arrived {
                arrived = true;
                break;
            }
            now += 5.0;
            target += 50.0;
        }
        assert!(arrived, "never arrived; remaining {}", last_remaining);
    }

    #[test]
    fn session_detects_off_route() {
        let route = l_route();
        let start = route.points[0];
        let mut session = NavSession::new(route);
        let mut status = session.update(start, 0.0);
        assert_eq!(status.state, NavState::Navigating);
        // Teleport 200m off the route and stay there.
        let off = LonLat::new(start.lon, start.lat + 0.002);
        status = session.update(off, 1.0);
        assert_eq!(status.state, NavState::OffRoute);
        assert!(!status.needs_reroute);
        status = session.update(off, 7.0);
        assert!(status.needs_reroute);
    }
}
