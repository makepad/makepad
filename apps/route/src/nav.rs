//! Turn-by-turn navigation (route.md M0/M4 of gps.md): NavSession over the
//! planned trip, leg by leg. Position comes from real GPS fixes when they
//! flow, or the simulated drive (examples/map pattern) for desk testing.

use makepad_map_nav::geo::{bearing_deg, bearing_delta_deg, LonLat};
use makepad_map_nav::graph::Route;
use makepad_map_nav::nav::{NavSession, NavState};

pub const SIM_SPEED_MULT: f64 = 6.0;

/// Requested by tools (executed by the app after the tool run).
pub enum NavAction {
    Start { simulate: bool },
    Stop,
}

pub struct ActiveNav {
    pub session: NavSession,
    /// One routed leg per consecutive stop pair (with maneuvers).
    pub routes: Vec<Route>,
    pub leg_index: usize,
    /// Point-count offset of this leg inside the full trip polyline.
    pub leg_point_offset: usize,
    pub simulate: bool,
    pub sim_progress_m: f64,
    pub sim_last_tick: Option<std::time::Instant>,
    pub started: std::time::Instant,
    pub map_rotation: f64,
    /// Latest position/heading fed to the session.
    pub position: Option<LonLat>,
    pub heading: Option<f64>,
}

pub struct NavTick {
    pub banner: String,
    pub banner_dist: String,
    pub position: LonLat,
    pub heading: Option<f64>,
    pub rotation: f64,
    pub progress_index: usize,
    pub finished_leg: bool,
    pub arrived: bool,
    pub needs_reroute: bool,
}

impl ActiveNav {
    pub fn new(routes: Vec<Route>, simulate: bool) -> Option<Self> {
        let first = routes.first()?.clone();
        Some(Self {
            session: NavSession::new(first),
            routes,
            leg_index: 0,
            leg_point_offset: 0,
            simulate,
            sim_progress_m: 0.0,
            sim_last_tick: None,
            started: std::time::Instant::now(),
            map_rotation: 0.0,
            position: None,
            heading: None,
        })
    }

    pub fn current_route(&self) -> &Route {
        &self.routes[self.leg_index]
    }

    pub fn start_point(&self) -> Option<LonLat> {
        self.current_route().points.first().copied()
    }

    /// Advance the simulated position by wall-clock dt and process.
    pub fn tick_sim(&mut self) -> Option<NavTick> {
        if !self.simulate {
            return None;
        }
        let now = std::time::Instant::now();
        let dt = self
            .sim_last_tick
            .map(|last| now.duration_since(last).as_secs_f64())
            .unwrap_or(0.05)
            .min(0.5);
        self.sim_last_tick = Some(now);
        let route = self.current_route().clone();
        let avg_speed = if route.duration_s > 0.0 {
            route.length_m / route.duration_s
        } else {
            10.0
        };
        self.sim_progress_m =
            (self.sim_progress_m + avg_speed * SIM_SPEED_MULT * dt).min(route.length_m);
        let pos = point_at(&route, self.sim_progress_m);
        let ahead = point_at(&route, (self.sim_progress_m + 12.0).min(route.length_m));
        let heading = if self.sim_progress_m + 1.0 < route.length_m {
            Some(bearing_deg(pos, ahead))
        } else {
            self.heading
        };
        Some(self.feed(pos, heading, dt))
    }

    /// Feed a (real or simulated) position into the session.
    pub fn feed(&mut self, pos: LonLat, heading: Option<f64>, dt: f64) -> NavTick {
        self.position = Some(pos);
        self.heading = heading;
        let now_s = self.started.elapsed().as_secs_f64();
        let status = self.session.update(pos, now_s);

        // Heading-up camera: ease onto the travel bearing, shortest arc.
        if let Some(target) = heading {
            let delta = bearing_delta_deg(self.map_rotation, target);
            let blend = 1.0 - (-dt * 3.0).exp();
            self.map_rotation = (self.map_rotation + delta * blend).rem_euclid(360.0);
        }

        let route = self.current_route();
        let leg_progress_index = route.cum_dist_m.partition_point(|&c| c < status.progress_m);
        let last_leg = self.leg_index + 1 >= self.routes.len();

        let mut tick = NavTick {
            banner: String::new(),
            banner_dist: String::new(),
            position: pos,
            heading,
            rotation: self.map_rotation,
            progress_index: self.leg_point_offset + leg_progress_index,
            finished_leg: false,
            arrived: false,
            needs_reroute: status.needs_reroute,
        };
        match status.state {
            NavState::Arrived => {
                if last_leg {
                    tick.arrived = true;
                    tick.banner = "You have arrived".to_string();
                } else {
                    tick.finished_leg = true;
                }
            }
            _ => {
                if let Some(idx) = status.next_maneuver {
                    let maneuver = &route.maneuvers[idx];
                    tick.banner = maneuver.text();
                    let time_scale = if self.simulate { SIM_SPEED_MULT } else { 1.0 };
                    tick.banner_dist = format!(
                        "in {}   ·   {} · {} left",
                        fmt_dist(status.dist_to_next_m),
                        fmt_dist(remaining_trip_m(self, status.remaining_m)),
                        crate::trip::fmt_duration(remaining_trip_s(self, status.remaining_s) / time_scale)
                    );
                }
            }
        }
        tick
    }

    /// Move to the next leg; returns false when there is none.
    pub fn advance_leg(&mut self) -> bool {
        if self.leg_index + 1 >= self.routes.len() {
            return false;
        }
        self.leg_point_offset += self.current_route().points.len();
        self.leg_index += 1;
        self.session = NavSession::new(self.current_route().clone());
        self.sim_progress_m = 0.0;
        true
    }
}

/// Remaining meters over the current leg plus all later legs.
fn remaining_trip_m(nav: &ActiveNav, leg_remaining_m: f64) -> f64 {
    let later: f64 = nav.routes[nav.leg_index + 1..]
        .iter()
        .map(|r| r.length_m)
        .sum();
    leg_remaining_m + later
}

fn remaining_trip_s(nav: &ActiveNav, leg_remaining_s: f64) -> f64 {
    let later: f64 = nav.routes[nav.leg_index + 1..]
        .iter()
        .map(|r| r.duration_s)
        .sum();
    leg_remaining_s + later
}

/// Route point at a given distance from the start (linear interpolation).
pub fn point_at(route: &Route, dist_m: f64) -> LonLat {
    let cum = &route.cum_dist_m;
    let points = &route.points;
    if points.is_empty() {
        return LonLat { lon: 0.0, lat: 0.0 };
    }
    let idx = cum.partition_point(|&c| c < dist_m);
    if idx == 0 {
        return points[0];
    }
    if idx >= points.len() {
        return *points.last().unwrap();
    }
    let seg = (cum[idx] - cum[idx - 1]).max(1e-9);
    let t = (dist_m - cum[idx - 1]) / seg;
    let a = points[idx - 1];
    let b = points[idx];
    LonLat {
        lon: a.lon + (b.lon - a.lon) * t,
        lat: a.lat + (b.lat - a.lat) * t,
    }
}

pub fn fmt_dist(meters: f64) -> String {
    if meters >= 1000.0 {
        format!("{:.1} km", meters / 1000.0)
    } else {
        format!("{:.0} m", meters)
    }
}
