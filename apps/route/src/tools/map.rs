//! map.* tools — camera & display, backed by the MapViewRef API.

use makepad_widgets::makepad_micro_serde::JsonValue;
use makepad_converse::agent_seam::ToolDefinition;
use makepad_widgets::*;

use crate::broker::{arg_array, arg_f64, def, num_f64, MarkerSpec, ToolCtx};
use crate::trip::StopKind;

pub fn defs() -> Vec<ToolDefinition> {
    vec![
        def(
            "map_fly_to",
            "Fly the map camera to a position. Use after finding a place so the user sees it.",
            r#"{"type":"object","properties":{
                "lon":{"type":"number","description":"longitude (WGS84)"},
                "lat":{"type":"number","description":"latitude (WGS84)"},
                "zoom":{"type":"number","description":"map zoom level 3-17, default 13 (city). 15+ = street level"},
                "tilt":{"type":"number","description":"camera tilt in degrees 0-70 (3D view)"}},
              "required":["lon","lat"]}"#,
        ),
        def(
            "map_show_trip",
            "Fit the camera to the whole planned trip and redraw its route and stop markers.",
            r#"{"type":"object","properties":{}}"#,
        ),
        def(
            "map_set_layer",
            "Toggle a map data layer on/off: rain (radar animation), wind, terrain (3D hillshade), chargers, transit, nature, districts, buildings_age, demographics.",
            r#"{"type":"object","properties":{
                "layer":{"type":"string"},
                "on":{"type":"boolean"}},
              "required":["layer","on"]}"#,
        ),
        def(
            "map_set_theme",
            "Switch the map theme: light (default), night, or circuit.",
            r#"{"type":"object","properties":{
                "theme":{"type":"string","enum":["light","night","circuit"]}},
              "required":["theme"]}"#,
        ),
        def(
            "map_set_markers",
            "Drop ad-hoc markers on the map (search results, sights, candidates). Replaces previous ad-hoc markers; trip stop markers stay. Pass an empty list to clear.",
            r#"{"type":"object","properties":{
                "markers":{"type":"array","items":{"type":"object","properties":{
                    "lon":{"type":"number"},
                    "lat":{"type":"number"},
                    "label":{"type":"string"},
                    "kind":{"type":"string","description":"search|sight|charger|generic — sets pin color"}},
                  "required":["lon","lat","label"]}}},
              "required":["markers"]}"#,
        ),
    ]
}

pub fn fly_to(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let lon = arg_f64(args, "lon").ok_or("missing lon")?;
    let lat = arg_f64(args, "lat").ok_or("missing lat")?;
    let zoom = arg_f64(args, "zoom").unwrap_or(13.0).clamp(3.0, 17.0);
    ctx.map.fly_to(ctx.cx, lon, lat, zoom);
    if let Some(tilt) = arg_f64(args, "tilt") {
        ctx.map.set_tilt(ctx.cx, tilt.clamp(0.0, 70.0));
    }
    Ok(format!("camera flying to {lon:.5},{lat:.5} zoom {zoom:.1}"))
}

pub fn show_trip(ctx: &mut ToolCtx, _args: &JsonValue) -> Result<String, String> {
    if ctx.trip.stops.is_empty() {
        return Err("no trip planned yet".into());
    }
    sync_trip_display(ctx, true);
    Ok(format!("showing trip:\n{}", ctx.trip.digest()))
}

pub fn set_layer(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let layer = crate::broker::arg_str(args, "layer").ok_or("missing layer")?.to_string();
    let on = matches!(args.key("on"), Some(JsonValue::Bool(true)));
    let name = ctx.layers.set_layer(&layer, on)?;
    Ok(format!(
        "{name} {} — {}",
        if on { "on" } else { "off" },
        ctx.layers.summary()
    ))
}

pub fn set_theme(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let theme = crate::broker::arg_str(args, "theme").ok_or("missing theme")?.to_string();
    ctx.layers.set_theme_name(&theme)?;
    Ok(ctx.layers.summary())
}

pub fn set_markers(ctx: &mut ToolCtx, args: &JsonValue) -> Result<String, String> {
    let items = arg_array(args, "markers").ok_or("missing markers array")?;
    let mut specs = Vec::new();
    for item in items {
        let lon = item.key("lon").and_then(num_f64).ok_or("marker missing lon")?;
        let lat = item.key("lat").and_then(num_f64).ok_or("marker missing lat")?;
        let label = item
            .key("label")
            .and_then(|v| v.string())
            .cloned()
            .unwrap_or_default();
        let kind = item
            .key("kind")
            .and_then(|v| v.string())
            .cloned()
            .unwrap_or_else(|| "generic".to_string());
        specs.push(MarkerSpec { lon, lat, label, kind });
    }
    let n = specs.len();
    ctx.markers.adhoc = specs;
    sync_trip_display(ctx, false);
    Ok(format!("{n} marker(s) shown"))
}

/// Rebuild the full marker set (trip stops + ad-hoc) and the route line;
/// optionally fit the camera to everything shown.
pub fn sync_trip_display(ctx: &mut ToolCtx, fit: bool) {
    ctx.markers.clear_names();
    let mut markers = Vec::new();
    for stop in &ctx.trip.stops {
        let id = ctx.markers.alloc(&stop.name);
        markers.push(MapMarker::new(id, stop.lon, stop.lat, stop_color(stop.kind)));
    }
    let adhoc = ctx.markers.adhoc.clone();
    for spec in &adhoc {
        let id = ctx.markers.alloc(&spec.label);
        markers.push(MapMarker::new(id, spec.lon, spec.lat, kind_color(&spec.kind)));
    }

    if ctx.trip.is_routed() {
        let points = ctx.trip.full_polyline();
        ctx.map.set_route(ctx.cx, &points);
    } else {
        ctx.map.clear_route(ctx.cx);
    }

    if fit {
        let mut bounds = ctx.trip.bounds();
        for spec in &adhoc {
            bounds = grow_bounds(bounds, spec.lon, spec.lat);
        }
        if let Some(bounds) = bounds {
            let (lon, lat, zoom) = fit_camera(ctx, bounds);
            ctx.map.fly_to(ctx.cx, lon, lat, zoom);
        }
    }
    ctx.map.set_markers(ctx.cx, markers);
}

fn grow_bounds(
    bounds: Option<((f64, f64), (f64, f64))>,
    lon: f64,
    lat: f64,
) -> Option<((f64, f64), (f64, f64))> {
    match bounds {
        None => Some(((lon, lat), (lon, lat))),
        Some((min, max)) => Some((
            (min.0.min(lon), min.1.min(lat)),
            (max.0.max(lon), max.1.max(lat)),
        )),
    }
}

// --- fit-bounds math (MapView has no fit API; web-mercator by hand) --------

fn mercator_norm(lon: f64, lat: f64) -> (f64, f64) {
    let x = (lon + 180.0) / 360.0;
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    let y = (1.0 - ((lat.tan() + 1.0 / lat.cos()).ln()) / std::f64::consts::PI) / 2.0;
    (x, y)
}

fn mercator_norm_inv(x: f64, y: f64) -> (f64, f64) {
    let lon = x * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    (lon, lat)
}

/// Center + zoom that fits the lon/lat bbox in the map viewport with margin.
fn fit_camera(ctx: &ToolCtx, ((min_lon, min_lat), (max_lon, max_lat)): ((f64, f64), (f64, f64))) -> (f64, f64, f64) {
    let (x0, y1) = mercator_norm(min_lon, min_lat);
    let (x1, y0) = mercator_norm(max_lon, max_lat);
    let (cx_n, cy_n) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let (center_lon, center_lat) = mercator_norm_inv(cx_n, cy_n);

    let rect = ctx.map.area().rect(ctx.cx);
    let (vw, vh) = if rect.size.x > 1.0 {
        (rect.size.x, rect.size.y)
    } else {
        (1280.0, 840.0)
    };
    let dx = (x1 - x0).abs().max(1e-9);
    let dy = (y1 - y0).abs().max(1e-9);
    // world pixel size at zoom z is 256 * 2^z; leave 25% margin
    let zx = (vw * 0.75 / (dx * 256.0)).log2();
    let zy = (vh * 0.75 / (dy * 256.0)).log2();
    let zoom = zx.min(zy).clamp(3.0, 16.0);
    (center_lon, center_lat, zoom)
}

fn stop_color(kind: StopKind) -> Vec4 {
    match kind {
        StopKind::Origin => vec4(0.16, 0.62, 0.32, 1.0),
        StopKind::Via => vec4(0.95, 0.60, 0.15, 1.0),
        StopKind::Charge => vec4(0.10, 0.65, 0.65, 1.0),
        StopKind::Destination => vec4(0.86, 0.24, 0.24, 1.0),
    }
}

pub fn kind_color(kind: &str) -> Vec4 {
    match kind {
        "search" => vec4(0.20, 0.45, 0.95, 1.0),
        "sight" => vec4(0.60, 0.35, 0.85, 1.0),
        "charger" => vec4(0.10, 0.65, 0.65, 1.0),
        _ => vec4(0.45, 0.48, 0.52, 1.0),
    }
}
