//! Async client for the hosted route-demo API.
//!
//! Request ids are owned here and correlated to a bounded set of operations;
//! no response can accidentally complete a newer search or route request.

use makepad_map_nav::{
    geo::LonLat,
    graph::{Route, TravelMode},
    nav::{Maneuver, ManeuverKind},
    search::{Category, SearchResult},
};
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiOperation {
    Search,
    Route,
    Along,
    Weather,
    RadarManifest,
    RadarFrame,
    Wind,
}

#[derive(Clone, Debug)]
pub struct AlongResult {
    pub name: String,
    pub kind: String,
    pub pos: LonLat,
    pub km_along: f64,
    pub detour_min: f64,
    pub extra: String,
}

#[derive(Clone, Debug)]
pub struct WeatherSample {
    pub minute: i64,
    pub mm_h: f64,
    pub class: String,
}

#[derive(Clone, Debug)]
pub struct WeatherNow {
    pub stamp: String,
    pub at: LonLat,
    pub samples: Vec<WeatherSample>,
}

#[derive(Clone, Debug)]
pub struct RadarManifest {
    pub stamp: String,
    pub bbox: (f64, f64, f64, f64),
    pub minutes: Vec<i64>,
    pub display: (usize, usize),
    pub hires_now: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct WindField {
    pub stamp_unix: i64,
    pub bbox: (f64, f64, f64, f64),
    pub nx: usize,
    pub ny: usize,
    pub u: Vec<f32>,
    pub v: Vec<f32>,
}

#[derive(Debug)]
pub enum NavApiEvent {
    Search(Vec<SearchResult>),
    Route(Route),
    Along(Vec<AlongResult>),
    Weather(WeatherNow),
    RadarManifest(RadarManifest),
    RadarFrame {
        stamp: String,
        minute: i64,
        hires: bool,
        png: Vec<u8>,
    },
    Wind(WindField),
    Failed {
        operation: ApiOperation,
        status: Option<u16>,
        message: String,
    },
}

#[derive(Clone, Debug)]
enum PendingRequest {
    Search,
    Route,
    Along,
    Weather,
    RadarManifest,
    RadarFrame {
        stamp: String,
        minute: i64,
        hires: bool,
    },
    Wind,
}

impl PendingRequest {
    fn operation(&self) -> ApiOperation {
        match self {
            Self::Search => ApiOperation::Search,
            Self::Route => ApiOperation::Route,
            Self::Along => ApiOperation::Along,
            Self::Weather => ApiOperation::Weather,
            Self::RadarManifest => ApiOperation::RadarManifest,
            Self::RadarFrame { .. } => ApiOperation::RadarFrame,
            Self::Wind => ApiOperation::Wind,
        }
    }
}

pub struct NavApi {
    base_url: String,
    next_request_id: u64,
    pending: HashMap<LiveId, PendingRequest>,
}

impl Default for NavApi {
    fn default() -> Self {
        Self::new("https://makepad.nl/api")
    }
}

impl NavApi {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            next_request_id: 0x524f_5554_4500_0001,
            pending: HashMap::new(),
        }
    }

    fn send(&mut self, cx: &mut Cx, pending: PendingRequest, request: HttpRequest) {
        let operation = pending.operation();
        if operation != ApiOperation::RadarFrame {
            self.pending
                .retain(|_, older| older.operation() != operation);
        }
        let request_id = LiveId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending.insert(request_id, pending);
        cx.http_request(request_id, request);
    }

    pub fn search(&mut self, cx: &mut Cx, query: &str, near: Option<LonLat>, limit: usize) {
        let query = truncate_utf8(query.trim(), 64);
        let mut url = format!(
            "{}/search?q={}&limit={}",
            self.base_url,
            percent_encode(query),
            limit.clamp(1, 20)
        );
        if let Some(near) = near.filter(|point| valid_point(*point)) {
            url.push_str(&format!("&near={:.7},{:.7}", near.lon, near.lat));
        }
        self.send(cx, PendingRequest::Search, json_request(url, HttpMethod::GET));
    }

    pub fn route(&mut self, cx: &mut Cx, from: LonLat, to: LonLat, mode: TravelMode) {
        if !valid_point(from) || !valid_point(to) {
            return;
        }
        let mode = match mode {
            TravelMode::Car => "car",
            TravelMode::Bike => "bike",
            TravelMode::Foot => "foot",
        };
        let url = format!(
            "{}/route?from={:.7},{:.7}&to={:.7},{:.7}&mode={mode}",
            self.base_url, from.lon, from.lat, to.lon, to.lat
        );
        self.send(cx, PendingRequest::Route, json_request(url, HttpMethod::GET));
    }

    pub fn along(
        &mut self,
        cx: &mut Cx,
        route: &Route,
        kinds: &[&str],
        max_detour_min: f64,
        min_kw: f64,
        limit: usize,
    ) {
        let body = along_request_json(
            route,
            kinds,
            max_detour_min,
            min_kw,
            limit.clamp(1, 30),
        );
        let mut request = json_request(format!("{}/along", self.base_url), HttpMethod::POST);
        request.set_body_string(&body);
        self.send(cx, PendingRequest::Along, request);
    }

    pub fn weather_now(&mut self, cx: &mut Cx, at: LonLat) {
        let url = format!("{}/weather/now?at={:.7},{:.7}", self.base_url, at.lon, at.lat);
        self.send(cx, PendingRequest::Weather, json_request(url, HttpMethod::GET));
    }

    pub fn radar_manifest(&mut self, cx: &mut Cx) {
        self.send(
            cx,
            PendingRequest::RadarManifest,
            json_request(format!("{}/radar/manifest", self.base_url), HttpMethod::GET),
        );
    }

    pub fn radar_frame(&mut self, cx: &mut Cx, stamp: &str, minute: i64, hires: bool) {
        let quality = if hires { "hires" } else { "display" };
        let url = format!(
            "{}/radar/frame?stamp={}&minute={minute}&quality={quality}",
            self.base_url,
            percent_encode(stamp)
        );
        self.send(
            cx,
            PendingRequest::RadarFrame {
                stamp: stamp.to_string(),
                minute,
                hires,
            },
            HttpRequest::new(url, HttpMethod::GET),
        );
    }

    pub fn wind_current(&mut self, cx: &mut Cx) {
        self.send(
            cx,
            PendingRequest::Wind,
            json_request(format!("{}/wind/current", self.base_url), HttpMethod::GET),
        );
    }

    pub fn handle_event(&mut self, event: &Event) -> Vec<NavApiEvent> {
        let Event::NetworkResponses(responses) = event else {
            return Vec::new();
        };
        let mut events = Vec::new();
        for response in responses {
            match response {
                NetworkResponse::HttpResponse { request_id, response } => {
                    let Some(pending) = self.pending.remove(request_id) else {
                        continue;
                    };
                    events.push(parse_response(pending, response));
                }
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(pending) = self.pending.remove(request_id) else {
                        continue;
                    };
                    events.push(NavApiEvent::Failed {
                        operation: pending.operation(),
                        status: None,
                        message: error.message.clone(),
                    });
                }
                _ => {}
            }
        }
        events
    }
}

fn json_request(url: String, method: HttpMethod) -> HttpRequest {
    let mut request = HttpRequest::new(url, method);
    request.set_header("Accept".to_string(), "application/json".to_string());
    request.set_header(
        "Content-Type".to_string(),
        "application/json; charset=utf-8".to_string(),
    );
    request
}

fn parse_response(pending: PendingRequest, response: &HttpResponse) -> NavApiEvent {
    let operation = pending.operation();
    if !(200..300).contains(&response.status_code) {
        return NavApiEvent::Failed {
            operation,
            status: Some(response.status_code),
            message: format!("API returned HTTP {}", response.status_code),
        };
    }
    let parsed = match pending {
        PendingRequest::Search => parse_search(body_text(response)).map(NavApiEvent::Search),
        PendingRequest::Route => parse_route(body_text(response)).map(NavApiEvent::Route),
        PendingRequest::Along => parse_along(body_text(response)).map(NavApiEvent::Along),
        PendingRequest::Weather => parse_weather(body_text(response)).map(NavApiEvent::Weather),
        PendingRequest::RadarManifest => {
            parse_radar_manifest(body_text(response)).map(NavApiEvent::RadarManifest)
        }
        PendingRequest::RadarFrame {
            stamp,
            minute,
            hires,
        } => response
            .body
            .clone()
            .ok_or_else(|| "radar frame had no PNG body".to_string())
            .map(|png| NavApiEvent::RadarFrame {
                stamp,
                minute,
                hires,
                png,
            }),
        PendingRequest::Wind => parse_wind(body_text(response)).map(NavApiEvent::Wind),
    };
    parsed.unwrap_or_else(|message| NavApiEvent::Failed {
        operation,
        status: Some(response.status_code),
        message,
    })
}

fn body_text(response: &HttpResponse) -> &str {
    response
        .body()
        .and_then(|body| std::str::from_utf8(body).ok())
        .unwrap_or("")
}

fn parse_search(json: &str) -> Result<Vec<SearchResult>, String> {
    let wire = SearchResponseWire::deserialize_json(json).map_err(json_error)?;
    Ok(wire
        .results
        .into_iter()
        .enumerate()
        .map(|(doc_id, result)| SearchResult {
            doc_id: doc_id as u32,
            name: result.name,
            secondary: result.secondary,
            category: category_from_api(&result.category),
            pos: LonLat::new(result.lon, result.lat),
            distance_m: result.distance_m,
            score: result.score,
        })
        .collect())
}

fn parse_route(json: &str) -> Result<Route, String> {
    let wire = RouteResponseWire::deserialize_json(json).map_err(json_error)?;
    let mode = match wire.mode.as_str() {
        "car" => TravelMode::Car,
        "bike" => TravelMode::Bike,
        "foot" => TravelMode::Foot,
        other => return Err(format!("unknown route mode {other:?}")),
    };
    if wire.points.len() != wire.cum_dist_m.len() {
        return Err("route points and cumulative distances differ in length".to_string());
    }
    let points: Vec<LonLat> = wire
        .points
        .into_iter()
        .map(|point| LonLat::new(point[0], point[1]))
        .collect();
    let maneuvers = wire
        .maneuvers
        .into_iter()
        .map(|maneuver| {
            Ok(Maneuver {
                kind: maneuver_kind(&maneuver.kind, maneuver.roundabout_exit)?,
                at: LonLat::new(maneuver.lon, maneuver.lat),
                name: maneuver.name,
                dist_m: maneuver.dist_m,
                point_index: maneuver.point_index,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Route {
        mode,
        points,
        cum_dist_m: wire.cum_dist_m,
        length_m: wire.length_m,
        duration_s: wire.duration_s,
        maneuvers,
    })
}

fn parse_along(json: &str) -> Result<Vec<AlongResult>, String> {
    let wire = AlongResponseWire::deserialize_json(json).map_err(json_error)?;
    Ok(wire
        .results
        .into_iter()
        .map(|result| AlongResult {
            name: result.name,
            kind: result.kind,
            pos: LonLat::new(result.lon, result.lat),
            km_along: result.km_along,
            detour_min: result.detour_min,
            extra: result.extra,
        })
        .collect())
}

fn parse_weather(json: &str) -> Result<WeatherNow, String> {
    let wire = WeatherResponseWire::deserialize_json(json).map_err(json_error)?;
    Ok(WeatherNow {
        stamp: wire.stamp,
        at: LonLat::new(wire.at[0], wire.at[1]),
        samples: wire
            .samples
            .into_iter()
            .map(|sample| WeatherSample {
                minute: sample.minute,
                mm_h: sample.mm_h,
                class: sample.class,
            })
            .collect(),
    })
}

fn parse_radar_manifest(json: &str) -> Result<RadarManifest, String> {
    let wire = RadarManifestWire::deserialize_json(json).map_err(json_error)?;
    Ok(RadarManifest {
        stamp: wire.stamp,
        bbox: (wire.bbox[0], wire.bbox[1], wire.bbox[2], wire.bbox[3]),
        minutes: wire.minutes,
        display: (wire.display.width, wire.display.height),
        hires_now: (wire.hires_now.width, wire.hires_now.height),
    })
}

fn parse_wind(json: &str) -> Result<WindField, String> {
    let wire = WindResponseWire::deserialize_json(json).map_err(json_error)?;
    if wire.u.len() != wire.v.len() || wire.u.len() != wire.nx.saturating_mul(wire.ny) {
        return Err("wind grid dimensions do not match vector arrays".to_string());
    }
    Ok(WindField {
        stamp_unix: wire.stamp_unix,
        bbox: (wire.bbox[0], wire.bbox[1], wire.bbox[2], wire.bbox[3]),
        nx: wire.nx,
        ny: wire.ny,
        u: wire.u,
        v: wire.v,
    })
}

fn maneuver_kind(kind: &str, exit: Option<u8>) -> Result<ManeuverKind, String> {
    Ok(match kind {
        "depart" => ManeuverKind::Depart,
        "arrive" => ManeuverKind::Arrive,
        "turn_slight_left" => ManeuverKind::TurnSlightLeft,
        "turn_left" => ManeuverKind::TurnLeft,
        "turn_sharp_left" => ManeuverKind::TurnSharpLeft,
        "turn_slight_right" => ManeuverKind::TurnSlightRight,
        "turn_right" => ManeuverKind::TurnRight,
        "turn_sharp_right" => ManeuverKind::TurnSharpRight,
        "u_turn" => ManeuverKind::UTurn,
        "roundabout_exit" => ManeuverKind::RoundaboutExit(
            exit.ok_or_else(|| "roundabout maneuver is missing its exit".to_string())?,
        ),
        other => return Err(format!("unknown maneuver kind {other:?}")),
    })
}

fn category_from_api(category: &str) -> Category {
    match category {
        "city" => Category::City,
        "town" => Category::Town,
        "village" => Category::Village,
        "suburb" => Category::Suburb,
        "neighbourhood" => Category::Neighbourhood,
        "hamlet" => Category::Hamlet,
        "street" => Category::Street,
        "address" => Category::Address,
        "station" => Category::Station,
        "tram_stop" => Category::TramStop,
        "bus_stop" => Category::BusStop,
        "ferry_terminal" => Category::FerryTerminal,
        "airport" => Category::Airport,
        "supermarket" => Category::Supermarket,
        "convenience" => Category::Convenience,
        "bakery" => Category::Bakery,
        "restaurant" => Category::Restaurant,
        "fast_food" => Category::FastFood,
        "cafe" => Category::Cafe,
        "bar" => Category::Bar,
        "pub" => Category::Pub,
        "pharmacy" => Category::Pharmacy,
        "hospital" => Category::Hospital,
        "school" => Category::School,
        "university" => Category::University,
        "library" => Category::Library,
        "museum" => Category::Museum,
        "attraction" => Category::Attraction,
        "gallery" => Category::Gallery,
        "hotel" => Category::Hotel,
        "bank" => Category::Bank,
        "cinema" => Category::Cinema,
        "theatre" => Category::Theatre,
        "park" => Category::Park,
        "fuel" => Category::Fuel,
        "parking" => Category::Parking,
        "charging_station" | "charger" => Category::ChargingStation,
        _ => Category::Other,
    }
}

fn valid_point(point: LonLat) -> bool {
    point.lon.is_finite()
        && point.lat.is_finite()
        && (-180.0..=180.0).contains(&point.lon)
        && (-90.0..=90.0).contains(&point.lat)
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

fn along_request_json(
    route: &Route,
    kinds: &[&str],
    max_detour_min: f64,
    min_kw: f64,
    limit: usize,
) -> String {
    let polyline = route
        .points
        .iter()
        .map(|point| format!("[{:.7},{:.7}]", point.lon, point.lat))
        .collect::<Vec<_>>()
        .join(",");
    let cumulative = route
        .cum_dist_m
        .iter()
        .map(|distance| distance.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let kinds = kinds
        .iter()
        .take(4)
        .map(|kind| format!("\"{}\"", json_escape(kind)))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{{\"polyline\":[{polyline}],\"cum_dist_m\":[{cumulative}],\"kinds\":[{kinds}],\"max_detour_min\":{max_detour_min},\"min_kw\":{min_kw},\"limit\":{limit}}}"
    )
}

fn json_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_error(error: makepad_micro_serde::DeJsonErr) -> String {
    format!("invalid API JSON: {}", error.msg)
}

#[derive(DeJson)]
struct SearchResponseWire {
    query: String,
    results: Vec<SearchResultWire>,
}

#[derive(DeJson)]
struct SearchResultWire {
    name: String,
    secondary: String,
    category: String,
    lon: f64,
    lat: f64,
    distance_m: Option<f64>,
    score: f64,
}

#[derive(DeJson)]
struct RouteResponseWire {
    graph: String,
    mode: String,
    length_m: f64,
    duration_s: f64,
    points: Vec<[f64; 2]>,
    cum_dist_m: Vec<f64>,
    maneuvers: Vec<ManeuverWire>,
}

#[derive(DeJson)]
struct ManeuverWire {
    kind: String,
    roundabout_exit: Option<u8>,
    lon: f64,
    lat: f64,
    name: String,
    dist_m: f64,
    point_index: usize,
    text: String,
}

#[derive(DeJson)]
struct AlongResponseWire {
    results: Vec<AlongResultWire>,
}

#[derive(DeJson)]
struct AlongResultWire {
    name: String,
    kind: String,
    lon: f64,
    lat: f64,
    km_along: f64,
    detour_min: f64,
    extra: String,
}

#[derive(DeJson)]
struct WeatherResponseWire {
    stamp: String,
    at: [f64; 2],
    samples: Vec<WeatherSampleWire>,
}

#[derive(DeJson)]
struct WeatherSampleWire {
    minute: i64,
    mm_h: f64,
    class: String,
}

#[derive(DeJson)]
struct RadarManifestWire {
    stamp: String,
    bbox: [f64; 4],
    minutes: Vec<i64>,
    display: ImageSizeWire,
    hires_now: ImageSizeWire,
}

#[derive(DeJson)]
struct ImageSizeWire {
    width: usize,
    height: usize,
}

#[derive(DeJson)]
struct WindResponseWire {
    stamp_unix: i64,
    bbox: [f64; 4],
    nx: usize,
    ny: usize,
    u: Vec<f32>,
    v: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_audit_search_response() {
        let results = parse_search(
            r#"{"query":"Oudegracht 399","results":[{"name":"Oudegracht 399","secondary":"Utrecht","category":"address","lon":5.1214,"lat":52.0872,"distance_m":34210.0,"score":1.23}]}"#,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "Oudegracht 399");
        assert_eq!(results[0].category, Category::Address);
        assert_eq!(results[0].pos, LonLat::new(5.1214, 52.0872));
    }

    #[test]
    fn parses_audit_route_response_into_nav_route() {
        let route = parse_route(
            r#"{"graph":"noord-holland","mode":"car","length_m":51234.0,"duration_s":3260.0,"points":[[4.8952,52.3702],[4.8960,52.3698]],"cum_dist_m":[0.0,72.4],"maneuvers":[{"kind":"turn_left","roundabout_exit":null,"lon":4.8960,"lat":52.3698,"name":"Example street","dist_m":72.4,"point_index":1,"text":"Turn left onto Example street"}]}"#,
        )
        .unwrap();
        assert_eq!(route.mode, TravelMode::Car);
        assert_eq!(route.points.len(), 2);
        assert_eq!(route.maneuvers[0].kind, ManeuverKind::TurnLeft);
        assert_eq!(route.maneuvers[0].text(), "Turn left onto Example street");
    }

    #[test]
    fn parses_audit_along_weather_radar_and_wind_responses() {
        let along = parse_along(r#"{"results":[{"name":"Charging site","kind":"charger","lon":5.01,"lat":52.22,"km_along":24.1,"detour_min":3.0,"extra":"150 kW, operator"}]}"#).unwrap();
        assert_eq!(along[0].kind, "charger");
        let weather = parse_weather(r#"{"stamp":"202609021230","at":[4.8952,52.3702],"samples":[{"minute":0,"mm_h":0.0,"class":"dry"},{"minute":30,"mm_h":0.4,"class":"light"}]}"#).unwrap();
        assert_eq!(weather.samples[1].class, "light");
        let radar = parse_radar_manifest(r#"{"stamp":"202609021230","bbox":[0.0,48.89,10.86,55.98],"minutes":[0,5,10,30,60,120],"display":{"width":1024,"height":1280},"hires_now":{"width":2048,"height":2560}}"#).unwrap();
        assert_eq!(radar.display, (1024, 1280));
        let wind = parse_wind(r#"{"stamp_unix":1788350400,"bbox":[2.0,48.0,9.0,56.0],"nx":2,"ny":1,"u":[-2.15,-2.05],"v":[0.01,0.03]}"#).unwrap();
        assert_eq!(wind.nx, 2);
        assert_eq!(wind.u.len(), 2);
    }
}
