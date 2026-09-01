//! route.* tools — trip planning over RouteGraph + TripModel.

use makepad_widgets::makepad_micro_serde::JsonValue;
use makepad_converse::agent_seam::ToolDefinition;
use makepad_map_nav::geo::LonLat;
use makepad_map_nav::graph::TravelMode;

use crate::broker::{arg_f64, arg_str, arg_str_list, arg_usize, def, MarkerSpec, ToolCtx};
use crate::tools::map::sync_trip_display;
use crate::trip::{Leg, StopKind, TripMode, TripModel};

pub fn defs() -> Vec<ToolDefinition> {
    vec![
        def(
            "route_plan",
            "Plan a new trip. Waypoints are place names (geocoded via search) or 'lon,lat' pairs; 'here' means the current map view. Replaces any existing trip, draws it on the map, and returns the trip digest with stable stop/leg ids.",
            r#"{"type":"object","properties":{
                "from":{"type":"string","description":"start; default 'here'"},
                "to":{"type":"string"},
                "via":{"type":"array","items":{"type":"string"},"description":"intermediate stops in order"},
                "mode":{"type":"string","enum":["car","bike","foot"],"description":"default car"}},
              "required":["to"]}"#,
        ),
        def(
            "route_add_stop",
            "Insert a stop into the current trip (before the destination unless position is given), re-route, and return the new digest with the ETA delta.",
            r#"{"type":"object","properties":{
                "place":{"type":"string","description":"place name or 'lon,lat'"},
                "kind":{"type":"string","enum":["via","charge"],"description":"default via"},
                "position":{"type":"integer","description":"index in the stop list to insert at (1 = right after origin)"}},
              "required":["place"]}"#,
        ),
        def(
            "route_remove_stop",
            "Remove a stop (by stable id like stop_3) from the trip and re-route. Origin and destination cannot be removed.",
            r#"{"type":"object","properties":{
                "stop_id":{"type":"string"}},
              "required":["stop_id"]}"#,
        ),
        def(
            "route_status",
            "Current trip digest plus where the map is looking. Call before answering questions about the trip.",
            r#"{"type":"object","properties":{}}"#,
        ),
        def(
            "route_along",
            "Find places along the planned route's corridor: sights, food, supermarkets, chargers... Returns 'km along | detour | name | kind' lines and drops markers for them. kinds are free-text search terms (e.g. 'museum', 'castle', 'restaurant', 'charger').",
            r#"{"type":"object","properties":{
                "kinds":{"type":"array","items":{"type":"string"}},
                "max_detour_min":{"type":"number","description":"max detour in minutes off the route, default 10"},
                "min_kw":{"type":"number","description":"chargers only: minimum charging power in kW"},
                "limit":{"type":"integer","description":"max results, default 12"}},
              "required":["kinds"]}"#,
        ),
    ]
}

fn to_travel_mode(mode: TripMode) -> TravelMode {
    match mode {
        TripMode::Car => TravelMode::Car,
        TripMode::Bike => TravelMode::Bike,
        TripMode::Foot => TravelMode::Foot,
    }
}

/// Resolve a waypoint string: 'here', 'lon,lat', or a searched place name.
fn resolve_place(
    ctx: &mut ToolCtx,
    text: &str,
    near: (f64, f64),
) -> Result<(String, f64, f64), String> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "here" || lower == "current" || lower == "current position" {
        let (lon, lat) = ctx.here();
        let name = if ctx.position.is_some() { "Current position" } else { "Here (map center)" };
        return Ok((name.to_string(), lon, lat));
    }
    if lower == "map center" {
        return Ok(("Map center".to_string(), near.0, near.1));
    }
    if let Some((a, b)) = t.split_once(',') {
        if let (Ok(lon), Ok(lat)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            if (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat) {
                return Ok((format!("{lon:.4},{lat:.4}"), lon, lat));
            }
        }
    }
    let nav = ctx.nav()?;
    let results = nav.search(t, Some(LonLat { lon: near.0, lat: near.1 }), 3);
    match results.first() {
        Some(r) => Ok((r.name.clone(), r.pos.lon, r.pos.lat)),
        None => Err(format!("no place found for '{t}'")),
    }
}

/// Re-route every leg of the current trip. On failure legs are left empty
/// and the error names the failing pair.
fn replan(ctx: &mut ToolCtx) -> Result<(), String> {
    let mode = to_travel_mode(ctx.trip.mode);
    let stops: Vec<(String, f64, f64)> = ctx
        .trip
        .stops
        .iter()
        .map(|s| (s.name.clone(), s.lon, s.lat))
        .collect();
    if stops.len() < 2 {
        return Err("trip needs at least two stops".into());
    }
    let mut legs = Vec::new();
    let mut routes = Vec::new();
    {
        let nav = ctx.nav()?;
        for w in stops.windows(2) {
            let from = LonLat { lon: w[0].1, lat: w[0].2 };
            let to = LonLat { lon: w[1].1, lat: w[1].2 };
            let route = nav.route_pair(from, to, mode).ok_or_else(|| {
                format!(
                    "no {} route from {} to {} (endpoint off the road network?)",
                    ctx_mode_label(mode),
                    w[0].0,
                    w[1].0
                )
            })?;
            legs.push(Leg {
                polyline: route.points.iter().map(|p| (p.lon, p.lat)).collect(),
                distance_m: route.length_m,
                duration_s: route.duration_s,
            });
            routes.push(route);
        }
    }
    ctx.trip.legs = legs;
    *ctx.leg_routes = routes;
    Ok(())
}

pub fn nav_start(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    if !ctx.trip.is_routed() || ctx.leg_routes.is_empty() {
        return Err("no routed trip — plan a route first".into());
    }
    let simulate = match args.key("simulate") {
        Some(JsonValue::Bool(b)) => *b,
        _ => ctx.position.is_none(),
    };
    *ctx.nav_action = Some(crate::nav::NavAction::Start { simulate });
    Ok(format!(
        "navigation starting ({})",
        if simulate { "simulated drive" } else { "live GPS" }
    ))
}

pub fn nav_stop(ctx: &mut ToolCtx, _args: &JsonValue) -> Result<String, String> {
    *ctx.nav_action = Some(crate::nav::NavAction::Stop);
    Ok("navigation ending".into())
}

fn ctx_mode_label(mode: TravelMode) -> &'static str {
    match mode {
        TravelMode::Car => "car",
        TravelMode::Bike => "bike",
        TravelMode::Foot => "foot",
    }
}

pub fn plan(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let to = arg_str(args, "to").ok_or("missing 'to'")?.to_string();
    let from = arg_str(args, "from").unwrap_or("here").to_string();
    let vias = arg_str_list(args, "via");
    let mode = arg_str(args, "mode")
        .and_then(TripMode::parse)
        .unwrap_or_default();

    let center = ctx.map_center();
    let mut waypoints = Vec::new();
    waypoints.push(resolve_place(ctx, &from, center)?);
    for via in &vias {
        waypoints.push(resolve_place(ctx, via, center)?);
    }
    waypoints.push(resolve_place(ctx, &to, center)?);

    let mut trip = TripModel::from_waypoints(&waypoints);
    trip.mode = mode;
    let prev = std::mem::replace(ctx.trip, trip);
    if let Err(e) = replan(ctx) {
        *ctx.trip = prev;
        return Err(e);
    }
    ctx.markers.adhoc.clear();
    sync_trip_display(ctx, true);
    Ok(format!("trip planned:\n{}", ctx.trip.digest()))
}

pub fn add_stop(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    if ctx.trip.stops.len() < 2 {
        return Err("no trip to add to — plan a route first".into());
    }
    let place = arg_str(args, "place").ok_or("missing 'place'")?.to_string();
    let kind = match arg_str(args, "kind") {
        Some("charge") => StopKind::Charge,
        _ => StopKind::Via,
    };
    let position = arg_usize(args, "position");

    let prev_duration = ctx.trip.total_duration_s();
    let near = ctx
        .trip
        .bounds()
        .map(|(min, max)| ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5))
        .unwrap_or_else(|| ctx.map_center());
    let (name, lon, lat) = resolve_place(ctx, &place, near)?;
    let ref_id = ctx.trip.insert_stop(name.clone(), lon, lat, kind, position);
    if let Err(e) = replan(ctx) {
        ctx.trip.remove_stop(&ref_id);
        let _ = replan(ctx);
        return Err(e);
    }
    sync_trip_display(ctx, true);
    let delta = ctx.trip.total_duration_s() - prev_duration;
    Ok(format!(
        "added {ref_id}: {name} ({} min ETA delta)\n{}",
        (delta / 60.0).round() as i64,
        ctx.trip.digest()
    ))
}

pub fn remove_stop(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let stop_id = arg_str(args, "stop_id").ok_or("missing 'stop_id'")?.to_string();
    let prev_duration = ctx.trip.total_duration_s();
    let removed = ctx
        .trip
        .remove_stop(&stop_id)
        .ok_or_else(|| format!("cannot remove '{stop_id}' — unknown id or an endpoint"))?;
    replan(ctx)?;
    sync_trip_display(ctx, true);
    let delta = ctx.trip.total_duration_s() - prev_duration;
    Ok(format!(
        "removed {stop_id} ({}) — {} min ETA delta\n{}",
        removed.name,
        (delta / 60.0).round() as i64,
        ctx.trip.digest()
    ))
}

pub fn status(ctx: &mut ToolCtx, _args: &JsonValue) -> Result<String, String> {
    let (lon, lat) = ctx.map_center();
    let zoom = ctx.map.map_zoom().unwrap_or(13.0);
    let gps = match ctx.position {
        Some((glon, glat)) => format!("{glon:.5},{glat:.5}"),
        None => "no fix".to_string(),
    };
    Ok(format!(
        "gps: {gps}\nmap center: {lon:.4},{lat:.4} zoom {zoom:.1}\ntrip:\n{}",
        ctx.trip.digest()
    ))
}

struct Candidate {
    name: String,
    lon: f64,
    lat: f64,
    kind: String,
    km_along: f64,
    detour_min: f64,
    extra: String,
}

pub fn along(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    if !ctx.trip.is_routed() {
        return Err("no routed trip — plan a route first".into());
    }
    let kinds = arg_str_list(args, "kinds");
    if kinds.is_empty() {
        return Err("kinds must not be empty".into());
    }
    let max_detour_min = arg_f64(args, "max_detour_min").unwrap_or(10.0).clamp(1.0, 60.0);
    let min_kw = arg_f64(args, "min_kw").unwrap_or(0.0);
    let limit = arg_usize(args, "limit").unwrap_or(12).clamp(1, 30);

    // Detour estimate: there-and-back off the corridor at ~40 km/h.
    const MIN_PER_M: f64 = 2.0 / (40_000.0 / 60.0);
    let radius_m = (max_detour_min / MIN_PER_M).min(5000.0);
    let total_m = ctx.trip.total_distance_m();
    let spacing_m = (total_m / 48.0).max(3000.0);
    let samples = ctx.trip.sample_along(spacing_m);

    let mut cands: Vec<Candidate> = Vec::new();
    {
        let nav = ctx.nav()?;
        for kind in &kinds {
            let lk = kind.to_ascii_lowercase();
            let is_charger = lk.contains("charg") || lk.contains("laad") || lk == "ev";
            if is_charger && nav.chargers.is_some() {
                let chargers = nav.chargers.as_mut().unwrap();
                for &(lon, lat, along_m) in &samples {
                    let hits = chargers
                        .query_radius(lon, lat, radius_m, 8)
                        .unwrap_or_default();
                    for h in hits {
                        let kw = h
                            .attrs
                            .get("max_kw")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        if (kw as f64) < min_kw {
                            continue;
                        }
                        let operator = h
                            .attrs
                            .get("operator")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let city = h.attrs.get("city").and_then(|v| v.as_str()).unwrap_or("");
                        let name = h
                            .name
                            .clone()
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| format!("{operator} {city}").trim().to_string());
                        let d = h.distance_m.unwrap_or(radius_m);
                        cands.push(Candidate {
                            name,
                            lon: h.center.0,
                            lat: h.center.1,
                            kind: "charger".into(),
                            km_along: along_m / 1000.0,
                            detour_min: d * MIN_PER_M,
                            extra: format!("{kw} kW, {operator}"),
                        });
                    }
                }
            } else {
                for &(lon, lat, along_m) in &samples {
                    let results = nav.search(kind, Some(LonLat { lon, lat }), 6);
                    for r in results {
                        let d = match r.distance_m {
                            Some(d) if d <= radius_m => d,
                            _ => continue,
                        };
                        cands.push(Candidate {
                            name: r.name.clone(),
                            lon: r.pos.lon,
                            lat: r.pos.lat,
                            kind: r.category.label().to_string(),
                            km_along: along_m / 1000.0,
                            detour_min: d * MIN_PER_M,
                        extra: r.secondary.clone(),
                        });
                    }
                }
            }
        }
    }

    // Dedupe (same name within ~1 km, keep smallest detour), order by km.
    cands.sort_by(|a, b| {
        a.detour_min
            .partial_cmp(&b.detour_min)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<Candidate> = Vec::new();
    for c in cands {
        if c.detour_min > max_detour_min {
            continue;
        }
        if kept.iter().any(|k| {
            k.name.eq_ignore_ascii_case(&c.name)
                && crate::trip::haversine_m((k.lon, k.lat), (c.lon, c.lat)) < 1000.0
        }) {
            continue;
        }
        kept.push(c);
    }
    kept.sort_by(|a, b| {
        a.km_along
            .partial_cmp(&b.km_along)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let dropped = kept.len().saturating_sub(limit);
    kept.truncate(limit);

    if kept.is_empty() {
        return Ok(format!(
            "nothing matching {kinds:?} within {max_detour_min:.0} min of the route"
        ));
    }

    // Mirror on the map as ad-hoc markers.
    ctx.markers.adhoc = kept
        .iter()
        .map(|c| MarkerSpec {
            lon: c.lon,
            lat: c.lat,
            label: c.name.clone(),
            kind: if c.kind == "charger" { "charger".into() } else { "sight".into() },
        })
        .collect();
    sync_trip_display(ctx, false);

    let mut out = String::new();
    for c in &kept {
        out.push_str(&format!(
            "km {:>3.0} | +{:>2.0} min | {} | {}",
            c.km_along, c.detour_min.max(1.0), c.name, c.kind
        ));
        if !c.extra.is_empty() {
            out.push_str(&format!(" | {}", c.extra));
        }
        out.push('\n');
    }
    if dropped > 0 {
        out.push_str(&format!("({dropped} more beyond limit)\n"));
    }
    Ok(out)
}
