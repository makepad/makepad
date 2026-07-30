//! TripModel — the single source of truth for the planned trip (route.md §2.4).
//!
//! The LLM never carries route geometry in its context: it references stops
//! and legs by stable ids (`stop_2`, `leg_1`) and receives compact digests.
//! Replans mutate this struct; the map mirrors it after every mutation.

/// Stable-id trip stop. Ids are never reused after removal.
#[derive(Clone, Debug)]
pub struct Stop {
    pub id: u64,
    pub name: String,
    pub lon: f64,
    pub lat: f64,
    pub kind: StopKind,
}

impl Stop {
    pub fn ref_id(&self) -> String {
        format!("stop_{}", self.id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopKind {
    Origin,
    Via,
    Charge,
    Destination,
}

impl StopKind {
    pub fn label(&self) -> &'static str {
        match self {
            StopKind::Origin => "origin",
            StopKind::Via => "via",
            StopKind::Charge => "charge",
            StopKind::Destination => "destination",
        }
    }
}

/// Routed leg between consecutive stops. `legs[i]` connects `stops[i]` to
/// `stops[i+1]`.
#[derive(Clone, Debug, Default)]
pub struct Leg {
    /// lon/lat polyline as produced by the router.
    pub polyline: Vec<(f64, f64)>,
    pub distance_m: f64,
    pub duration_s: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TripMode {
    #[default]
    Car,
    Bike,
    Foot,
}

impl TripMode {
    pub fn label(&self) -> &'static str {
        match self {
            TripMode::Car => "car",
            TripMode::Bike => "bike",
            TripMode::Foot => "foot",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "car" | "drive" | "driving" => Some(TripMode::Car),
            "bike" | "bicycle" | "cycling" => Some(TripMode::Bike),
            "foot" | "walk" | "walking" => Some(TripMode::Foot),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TripModel {
    pub stops: Vec<Stop>,
    pub legs: Vec<Leg>,
    pub mode: TripMode,
    next_stop_id: u64,
}

impl TripModel {
    /// Start a new trip from an ordered list of (name, lon, lat) waypoints.
    /// First is the origin, last the destination, the rest vias. Legs are
    /// filled in separately by the router.
    pub fn from_waypoints(waypoints: &[(String, f64, f64)]) -> Self {
        let mut trip = TripModel::default();
        let n = waypoints.len();
        for (i, (name, lon, lat)) in waypoints.iter().enumerate() {
            let kind = if i == 0 {
                StopKind::Origin
            } else if i + 1 == n {
                StopKind::Destination
            } else {
                StopKind::Via
            };
            trip.push_stop(name.clone(), *lon, *lat, kind);
        }
        trip
    }

    fn alloc_id(&mut self) -> u64 {
        self.next_stop_id += 1;
        self.next_stop_id
    }

    fn push_stop(&mut self, name: String, lon: f64, lat: f64, kind: StopKind) {
        let id = self.alloc_id();
        self.stops.push(Stop { id, name, lon, lat, kind });
    }

    pub fn stop_by_ref(&self, ref_id: &str) -> Option<&Stop> {
        let id: u64 = ref_id.strip_prefix("stop_")?.parse().ok()?;
        self.stops.iter().find(|s| s.id == id)
    }

    /// Insert a stop before the destination (or at `position` if given as an
    /// index into the stop list). Returns the new stop's ref id. Legs are
    /// invalidated and must be re-routed.
    pub fn insert_stop(
        &mut self,
        name: String,
        lon: f64,
        lat: f64,
        kind: StopKind,
        position: Option<usize>,
    ) -> String {
        let id = self.alloc_id();
        let at = position
            .unwrap_or_else(|| self.stops.len().saturating_sub(1))
            .clamp(
                if self.stops.is_empty() { 0 } else { 1 },
                self.stops.len().saturating_sub(1).max(0),
            );
        self.stops.insert(at, Stop { id, name, lon, lat, kind });
        self.legs.clear();
        format!("stop_{id}")
    }

    /// Remove a stop by ref id. Origin/destination cannot be removed.
    /// Returns the removed stop. Legs are invalidated.
    pub fn remove_stop(&mut self, ref_id: &str) -> Option<Stop> {
        let id: u64 = ref_id.strip_prefix("stop_")?.parse().ok()?;
        let idx = self.stops.iter().position(|s| s.id == id)?;
        if idx == 0 || idx + 1 == self.stops.len() {
            return None;
        }
        self.legs.clear();
        Some(self.stops.remove(idx))
    }

    pub fn is_routed(&self) -> bool {
        !self.stops.is_empty() && self.legs.len() + 1 == self.stops.len()
    }

    pub fn total_distance_m(&self) -> f64 {
        self.legs.iter().map(|l| l.distance_m).sum()
    }

    pub fn total_duration_s(&self) -> f64 {
        self.legs.iter().map(|l| l.duration_s).sum()
    }

    /// Concatenated polyline over all legs (for map display / corridor
    /// sampling). Consecutive duplicate joint points are collapsed.
    pub fn full_polyline(&self) -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = Vec::new();
        for leg in &self.legs {
            for &p in &leg.polyline {
                if out.last() != Some(&p) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Geographic bounds of the whole trip (stops + polylines):
    /// ((min_lon, min_lat), (max_lon, max_lat)).
    pub fn bounds(&self) -> Option<((f64, f64), (f64, f64))> {
        let mut min = (f64::MAX, f64::MAX);
        let mut max = (f64::MIN, f64::MIN);
        let mut any = false;
        let mut take = |lon: f64, lat: f64| {
            min.0 = min.0.min(lon);
            min.1 = min.1.min(lat);
            max.0 = max.0.max(lon);
            max.1 = max.1.max(lat);
            any = true;
        };
        for s in &self.stops {
            take(s.lon, s.lat);
        }
        for l in &self.legs {
            for &(lon, lat) in &l.polyline {
                take(lon, lat);
            }
        }
        if any { Some((min, max)) } else { None }
    }

    /// Sample points spaced `spacing_m` along the full polyline. Returns
    /// (lon, lat, meters_from_start). Always includes the start point;
    /// includes the end point if it is more than half a spacing beyond the
    /// last sample. This is the substrate for corridor queries (route.along).
    pub fn sample_along(&self, spacing_m: f64) -> Vec<(f64, f64, f64)> {
        let line = self.full_polyline();
        sample_polyline(&line, spacing_m)
    }

    /// Compact, line-oriented digest for the LLM. Small by design: the
    /// model reasons over ids and numbers, not geometry.
    pub fn digest(&self) -> String {
        if self.stops.is_empty() {
            return "no trip planned".into();
        }
        let mut out = String::new();
        let mut cum_s = 0.0;
        for (i, stop) in self.stops.iter().enumerate() {
            if i > 0 {
                if let Some(leg) = self.legs.get(i - 1) {
                    cum_s += leg.duration_s;
                    out.push_str(&format!(
                        "leg_{}: {:.1} km, {}\n",
                        i,
                        leg.distance_m / 1000.0,
                        fmt_duration(leg.duration_s)
                    ));
                }
            }
            out.push_str(&format!(
                "{}: {} ({}) at {:.5},{:.5}{}\n",
                stop.ref_id(),
                stop.name,
                stop.kind.label(),
                stop.lon,
                stop.lat,
                if i > 0 && self.is_routed() {
                    format!(" — +{} from start", fmt_duration(cum_s))
                } else {
                    String::new()
                }
            ));
        }
        if self.is_routed() {
            out.push_str(&format!(
                "total: {:.1} km, {} ({})\n",
                self.total_distance_m() / 1000.0,
                fmt_duration(self.total_duration_s()),
                self.mode.label()
            ));
        } else if self.stops.len() > 1 {
            out.push_str("(not routed yet)\n");
        }
        out
    }
}

pub fn fmt_duration(secs: f64) -> String {
    let mins = (secs / 60.0).round() as i64;
    if mins >= 60 {
        format!("{}h{:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}min")
    }
}

/// Haversine distance in meters.
pub fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lon1, lat1) = (a.0.to_radians(), a.1.to_radians());
    let (lon2, lat2) = (b.0.to_radians(), b.1.to_radians());
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let h = (dlat / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * h.sqrt().asin()
}

/// Sample a lon/lat polyline every `spacing_m` meters.
/// Returns (lon, lat, meters_from_start).
pub fn sample_polyline(line: &[(f64, f64)], spacing_m: f64) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    if line.is_empty() || spacing_m <= 0.0 {
        return out;
    }
    out.push((line[0].0, line[0].1, 0.0));
    let mut cum = 0.0;
    let mut next_at = spacing_m;
    for w in line.windows(2) {
        let seg = haversine_m(w[0], w[1]);
        if seg <= 0.0 {
            continue;
        }
        while next_at <= cum + seg {
            let t = (next_at - cum) / seg;
            let lon = w[0].0 + (w[1].0 - w[0].0) * t;
            let lat = w[0].1 + (w[1].1 - w[0].1) * t;
            out.push((lon, lat, next_at));
            next_at += spacing_m;
        }
        cum += seg;
    }
    let end = *line.last().unwrap();
    if cum - out.last().unwrap().2 > spacing_m * 0.5 {
        out.push((end.0, end.1, cum));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waypoints() -> Vec<(String, f64, f64)> {
        vec![
            ("Amsterdam".into(), 4.8952, 52.3702),
            ("Groningen".into(), 6.5665, 53.2194),
        ]
    }

    #[test]
    fn stable_ids_survive_removal() {
        let mut trip = TripModel::from_waypoints(&waypoints());
        let r = trip.insert_stop("Fastned Lelystad".into(), 5.475, 52.518, StopKind::Charge, None);
        assert_eq!(r, "stop_3");
        assert!(trip.remove_stop(&r).is_some());
        let r2 = trip.insert_stop("Ionity Harderwijk".into(), 5.62, 52.35, StopKind::Charge, None);
        assert_eq!(r2, "stop_4"); // id 3 never reused
        assert!(trip.stop_by_ref("stop_3").is_none());
        assert!(trip.stop_by_ref("stop_4").is_some());
    }

    #[test]
    fn cannot_remove_endpoints() {
        let mut trip = TripModel::from_waypoints(&waypoints());
        assert!(trip.remove_stop("stop_1").is_none());
        assert!(trip.remove_stop("stop_2").is_none());
    }

    #[test]
    fn sampling_spacing() {
        // ~111km of longitude at the equator
        let line = vec![(0.0, 0.0), (1.0, 0.0)];
        let samples = sample_polyline(&line, 10_000.0);
        assert!(samples.len() >= 11);
        for w in samples.windows(2) {
            let d = w[1].2 - w[0].2;
            assert!((d - 10_000.0).abs() < 1.0 || d < 10_000.0);
        }
    }

    #[test]
    fn digest_unrouted() {
        let trip = TripModel::from_waypoints(&waypoints());
        let d = trip.digest();
        assert!(d.contains("stop_1: Amsterdam (origin)"));
        assert!(d.contains("not routed"));
    }
}
