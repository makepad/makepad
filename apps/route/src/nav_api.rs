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

const REQUEST_TIMEOUT_SECONDS: f64 = 20.0;
const MAX_RETRY_SECONDS: f64 = 30.0;
const MAX_SEARCH_RESULTS: usize = 20;
const MAX_ROUTE_POINTS: usize = 20_000;
const MAX_ALONG_RESULTS: usize = 30;
const MAX_WEATHER_SAMPLES: usize = 64;
const MAX_RADAR_FRAMES: usize = 64;
const MAX_RADAR_DIMENSION: usize = 8_192;
const MAX_RADAR_PIXELS: usize = 32 * 1024 * 1024;
const MAX_WIND_DIMENSION: usize = 2_048;
const MAX_WIND_CELLS: usize = 1024 * 1024;

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
pub enum RouteRequestContext {
    Initial {
        generation: u64,
        destination: SearchResult,
    },
    Reroute {
        generation: u64,
    },
}

impl RouteRequestContext {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Initial { generation, .. } | Self::Reroute { generation } => *generation,
        }
    }
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
    Route {
        context: RouteRequestContext,
        route: Route,
    },
    Along {
        route_generation: u64,
        results: Vec<AlongResult>,
    },
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
        route_context: Option<RouteRequestContext>,
        route_generation: Option<u64>,
        retrying: bool,
    },
}

#[derive(Clone, Debug)]
enum PendingRequest {
    Search,
    Route(RouteRequestContext),
    Along { route_generation: u64 },
    Weather,
    RadarManifest,
    RadarFrame {
        stamp: String,
        minute: i64,
        hires: bool,
    },
    Wind,
}

struct PendingCall {
    request: HttpRequest,
    request_kind: PendingRequest,
    attempt: u8,
    timeout: Timer,
}

struct RetryCall {
    request: HttpRequest,
    request_kind: PendingRequest,
    attempt: u8,
    timer: Timer,
}

impl PendingRequest {
    fn operation(&self) -> ApiOperation {
        match self {
            Self::Search => ApiOperation::Search,
            Self::Route(_) => ApiOperation::Route,
            Self::Along { .. } => ApiOperation::Along,
            Self::Weather => ApiOperation::Weather,
            Self::RadarManifest => ApiOperation::RadarManifest,
            Self::RadarFrame { .. } => ApiOperation::RadarFrame,
            Self::Wind => ApiOperation::Wind,
        }
    }

    fn route_context(&self) -> Option<RouteRequestContext> {
        match self {
            Self::Route(context) => Some(context.clone()),
            _ => None,
        }
    }

    fn route_generation(&self) -> Option<u64> {
        match self {
            Self::Along { route_generation } => Some(*route_generation),
            _ => None,
        }
    }
}

pub struct NavApi {
    base_url: String,
    next_request_id: u64,
    pending: HashMap<LiveId, PendingCall>,
    retries: Vec<RetryCall>,
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
            retries: Vec::new(),
        }
    }

    fn send(&mut self, cx: &mut Cx, pending: PendingRequest, request: HttpRequest) {
        let operation = pending.operation();
        if operation != ApiOperation::RadarFrame {
            self.cancel_operation(cx, operation);
        }
        self.dispatch(cx, pending, request, 0);
    }

    fn dispatch(
        &mut self,
        cx: &mut Cx,
        request_kind: PendingRequest,
        request: HttpRequest,
        attempt: u8,
    ) {
        let request_id = LiveId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let timeout = cx.start_timeout(REQUEST_TIMEOUT_SECONDS);
        self.pending.insert(
            request_id,
            PendingCall {
                request: request.clone(),
                request_kind,
                attempt,
                timeout,
            },
        );
        cx.http_request(request_id, request);
    }

    fn cancel_operation(&mut self, cx: &mut Cx, operation: ApiOperation) {
        let stale = self
            .pending
            .iter()
            .filter(|(_, call)| call.request_kind.operation() == operation)
            .map(|(request_id, _)| *request_id)
            .collect::<Vec<_>>();
        for request_id in stale {
            if let Some(call) = self.pending.remove(&request_id) {
                cx.stop_timer(call.timeout);
            }
        }
        let mut index = 0;
        while index < self.retries.len() {
            if self.retries[index].request_kind.operation() == operation {
                let retry = self.retries.swap_remove(index);
                cx.stop_timer(retry.timer);
            } else {
                index += 1;
            }
        }
    }

    fn schedule_retry(&mut self, cx: &mut Cx, call: &PendingCall) {
        let attempt = call.attempt.saturating_add(1);
        let delay = retry_delay_seconds(attempt);
        self.retries.push(RetryCall {
            request: call.request.clone(),
            request_kind: call.request_kind.clone(),
            attempt,
            timer: cx.start_timeout(delay),
        });
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

    pub fn route(
        &mut self,
        cx: &mut Cx,
        from: LonLat,
        to: LonLat,
        mode: TravelMode,
        context: RouteRequestContext,
    ) {
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
        self.send(
            cx,
            PendingRequest::Route(context),
            json_request(url, HttpMethod::GET),
        );
    }

    pub fn along(
        &mut self,
        cx: &mut Cx,
        route: &Route,
        kinds: &[&str],
        max_detour_min: f64,
        min_kw: f64,
        limit: usize,
        route_generation: u64,
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
        self.send(cx, PendingRequest::Along { route_generation }, request);
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
        if self.has_radar_frame_request(stamp, minute, hires) {
            return;
        }
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

    pub fn cancel_radar_frames(&mut self, cx: &mut Cx) {
        self.cancel_operation(cx, ApiOperation::RadarFrame);
    }

    pub fn cancel_along(&mut self, cx: &mut Cx) {
        self.cancel_operation(cx, ApiOperation::Along);
    }

    pub fn cancel_rain(&mut self, cx: &mut Cx) {
        self.cancel_operation(cx, ApiOperation::RadarManifest);
        self.cancel_radar_frames(cx);
    }

    fn has_radar_frame_request(&self, stamp: &str, minute: i64, hires: bool) -> bool {
        let matches = |request: &PendingRequest| {
            matches!(
                request,
                PendingRequest::RadarFrame {
                    stamp: pending_stamp,
                    minute: pending_minute,
                    hires: pending_hires,
                } if pending_stamp == stamp && *pending_minute == minute && *pending_hires == hires
            )
        };
        self.pending.values().any(|call| matches(&call.request_kind))
            || self.retries.iter().any(|call| matches(&call.request_kind))
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<NavApiEvent> {
        let timed_out = self
            .pending
            .iter()
            .filter(|(_, call)| call.timeout.is_event(event).is_some())
            .map(|(request_id, _)| *request_id)
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        for request_id in timed_out {
            if let Some(call) = self.pending.remove(&request_id) {
                let route_context = call.request_kind.route_context();
                let route_generation = call.request_kind.route_generation();
                events.push(NavApiEvent::Failed {
                    operation: call.request_kind.operation(),
                    status: None,
                    message: "request timed out".to_string(),
                    route_context,
                    route_generation,
                    retrying: false,
                });
            }
        }

        let ready_retries = self
            .retries
            .iter()
            .enumerate()
            .filter(|(_, retry)| retry.timer.is_event(event).is_some())
            .map(|(index, _)| index)
            .rev()
            .collect::<Vec<_>>();
        for index in ready_retries {
            let retry = self.retries.swap_remove(index);
            self.dispatch(cx, retry.request_kind, retry.request, retry.attempt);
        }

        let Event::NetworkResponses(responses) = event else {
            return events;
        };
        for response in responses {
            match response {
                NetworkResponse::HttpResponse { request_id, response } => {
                    let Some(call) = self.pending.remove(request_id) else {
                        continue;
                    };
                    cx.stop_timer(call.timeout);
                    let retrying = retryable_status(response.status_code)
                        && retryable_operation(call.request_kind.operation());
                    if retrying {
                        self.schedule_retry(cx, &call);
                    }
                    events.push(parse_response(call.request_kind, response, retrying));
                }
                NetworkResponse::HttpError { request_id, error } => {
                    let Some(call) = self.pending.remove(request_id) else {
                        continue;
                    };
                    cx.stop_timer(call.timeout);
                    let route_context = call.request_kind.route_context();
                    let route_generation = call.request_kind.route_generation();
                    events.push(NavApiEvent::Failed {
                        operation: call.request_kind.operation(),
                        status: None,
                        message: error.message.clone(),
                        route_context,
                        route_generation,
                        retrying: false,
                    });
                }
                _ => {}
            }
        }
        events
    }
}

fn retryable_operation(operation: ApiOperation) -> bool {
    matches!(operation, ApiOperation::Search | ApiOperation::Route | ApiOperation::Along)
}

fn retryable_status(status: u16) -> bool {
    matches!(status, 429 | 503)
}

fn retry_delay_seconds(attempt: u8) -> f64 {
    2.0_f64
        .powi(attempt.saturating_sub(1).min(5) as i32)
        .min(MAX_RETRY_SECONDS)
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

fn parse_response(
    pending: PendingRequest,
    response: &HttpResponse,
    retrying: bool,
) -> NavApiEvent {
    let operation = pending.operation();
    let route_context = pending.route_context();
    let route_generation = pending.route_generation();
    if !(200..300).contains(&response.status_code) {
        return NavApiEvent::Failed {
            operation,
            status: Some(response.status_code),
            message: parse_error_message(response),
            route_context,
            route_generation,
            retrying,
        };
    }
    let parsed = match pending {
        PendingRequest::Search => parse_search(body_text(response)).map(NavApiEvent::Search),
        PendingRequest::Route(context) => parse_route(body_text(response)).map(|route| {
            NavApiEvent::Route { context, route }
        }),
        PendingRequest::Along { route_generation } => {
            parse_along(body_text(response)).map(|results| NavApiEvent::Along {
                route_generation,
                results,
            })
        }
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
                png: png.to_vec(),
            }),
        PendingRequest::Wind => parse_wind(body_text(response)).map(NavApiEvent::Wind),
    };
    parsed.unwrap_or_else(|message| NavApiEvent::Failed {
        operation,
        status: Some(response.status_code),
        message,
        route_context,
        route_generation,
        retrying: false,
    })
}

fn parse_error_message(response: &HttpResponse) -> String {
    if let Ok(wire) = ErrorResponseWire::deserialize_json(body_text(response)) {
        if !wire.error.code.is_empty() || !wire.error.message.is_empty() {
            return format!(
                "{}: {}",
                truncate_utf8(&wire.error.code, 64),
                truncate_utf8(&wire.error.message, 256)
            )
            .trim_matches(|ch: char| ch == ':' || ch.is_whitespace())
            .to_string();
        }
    }
    format!("API returned HTTP {}", response.status_code)
}

fn body_text(response: &HttpResponse) -> &str {
    response
        .body()
        .and_then(|body| std::str::from_utf8(body).ok())
        .unwrap_or("")
}

fn parse_search(json: &str) -> Result<Vec<SearchResult>, String> {
    let wire = SearchResponseWire::deserialize_json(json).map_err(json_error)?;
    if wire.results.len() > MAX_SEARCH_RESULTS {
        return Err("search response has too many results".to_string());
    }
    for result in &wire.results {
        let point = LonLat::new(result.lon, result.lat);
        if !valid_point(point)
            || !result.score.is_finite()
            || result
                .distance_m
                .is_some_and(|distance| !distance.is_finite() || distance < 0.0)
        {
            return Err("search response contains an invalid result".to_string());
        }
    }
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
    if !(2..=MAX_ROUTE_POINTS).contains(&wire.points.len()) {
        return Err("route point count is outside 2..=20000".to_string());
    }
    if wire.points.len() != wire.cum_dist_m.len() {
        return Err("route points and cumulative distances differ in length".to_string());
    }
    if wire.maneuvers.len() > wire.points.len() {
        return Err("route has too many maneuvers".to_string());
    }
    if !wire.length_m.is_finite()
        || wire.length_m < 0.0
        || !wire.duration_s.is_finite()
        || wire.duration_s < 0.0
        || wire
            .points
            .iter()
            .any(|point| !valid_point(LonLat::new(point[0], point[1])))
        || wire
            .cum_dist_m
            .iter()
            .any(|distance| !distance.is_finite() || *distance < 0.0)
        || wire
            .cum_dist_m
            .windows(2)
            .any(|pair| pair[0] > pair[1])
    {
        return Err("route contains invalid coordinates or distances".to_string());
    }
    for maneuver in &wire.maneuvers {
        if maneuver.point_index >= wire.points.len()
            || !valid_point(LonLat::new(maneuver.lon, maneuver.lat))
            || !maneuver.dist_m.is_finite()
            || maneuver.dist_m < 0.0
        {
            return Err("route contains an invalid maneuver".to_string());
        }
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
    if wire.results.len() > MAX_ALONG_RESULTS {
        return Err("along response has too many results".to_string());
    }
    if wire.results.iter().any(|result| {
        !valid_point(LonLat::new(result.lon, result.lat))
            || !result.km_along.is_finite()
            || result.km_along < 0.0
            || !result.detour_min.is_finite()
            || result.detour_min < 0.0
    }) {
        return Err("along response contains an invalid result".to_string());
    }
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
    if wire.samples.len() > MAX_WEATHER_SAMPLES
        || !valid_point(LonLat::new(wire.at[0], wire.at[1]))
        || wire
            .samples
            .iter()
            .any(|sample| sample.minute < 0 || !sample.mm_h.is_finite() || sample.mm_h < 0.0)
    {
        return Err("weather response contains invalid samples".to_string());
    }
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
    let bbox = (wire.bbox[0], wire.bbox[1], wire.bbox[2], wire.bbox[3]);
    if wire.stamp.is_empty()
        || wire.stamp.len() > 64
        || !valid_bbox(bbox)
        || wire.minutes.len() > MAX_RADAR_FRAMES
        || wire.minutes.iter().any(|minute| !(0..=1440).contains(minute))
        || !valid_radar_size(wire.display.width, wire.display.height)
        || !valid_radar_size(wire.hires_now.width, wire.hires_now.height)
    {
        return Err("radar manifest contains invalid bounds".to_string());
    }
    Ok(RadarManifest {
        stamp: wire.stamp,
        bbox,
        minutes: wire.minutes,
        display: (wire.display.width, wire.display.height),
        hires_now: (wire.hires_now.width, wire.hires_now.height),
    })
}

fn parse_wind(json: &str) -> Result<WindField, String> {
    let wire = WindResponseWire::deserialize_json(json).map_err(json_error)?;
    let bbox = (wire.bbox[0], wire.bbox[1], wire.bbox[2], wire.bbox[3]);
    let cells = wire.nx.checked_mul(wire.ny);
    if !valid_bbox(bbox)
        || !(1..=MAX_WIND_DIMENSION).contains(&wire.nx)
        || !(1..=MAX_WIND_DIMENSION).contains(&wire.ny)
        || cells.is_none_or(|cells| cells > MAX_WIND_CELLS)
        || wire.u.len() != wire.v.len()
        || Some(wire.u.len()) != cells
        || wire.u.iter().chain(&wire.v).any(|value| !value.is_finite())
    {
        return Err("wind grid dimensions do not match vector arrays".to_string());
    }
    Ok(WindField {
        stamp_unix: wire.stamp_unix,
        bbox,
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

fn valid_bbox(bbox: (f64, f64, f64, f64)) -> bool {
    valid_point(LonLat::new(bbox.0, bbox.1))
        && valid_point(LonLat::new(bbox.2, bbox.3))
        && bbox.0 < bbox.2
        && bbox.1 < bbox.3
}

fn valid_radar_size(width: usize, height: usize) -> bool {
    (1..=MAX_RADAR_DIMENSION).contains(&width)
        && (1..=MAX_RADAR_DIMENSION).contains(&height)
        && width
            .checked_mul(height)
            .is_some_and(|pixels| pixels <= MAX_RADAR_PIXELS)
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

#[derive(DeJson)]
struct ErrorResponseWire {
    error: ErrorWire,
}

#[derive(DeJson)]
struct ErrorWire {
    code: String,
    message: String,
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

    #[test]
    fn hosted_service_retry_backoff_is_bounded() {
        assert!(retryable_status(429));
        assert!(retryable_status(503));
        assert!(!retryable_status(500));
        assert_eq!(retry_delay_seconds(1), 1.0);
        assert_eq!(retry_delay_seconds(2), 2.0);
        assert_eq!(retry_delay_seconds(6), 30.0);
        assert_eq!(retry_delay_seconds(u8::MAX), 30.0);
    }

    #[test]
    fn route_validation_rejects_unsafe_geometry_and_maneuvers() {
        assert!(parse_route(
            r#"{"graph":"x","mode":"car","length_m":0.0,"duration_s":0.0,"points":[],"cum_dist_m":[],"maneuvers":[]}"#,
        )
        .is_err());
        assert!(parse_route(
            r#"{"graph":"x","mode":"car","length_m":2.0,"duration_s":1.0,"points":[[4.0,52.0],[4.1,52.1],[4.2,52.2]],"cum_dist_m":[0.0,2.0,1.0],"maneuvers":[]}"#,
        )
        .is_err());
        assert!(parse_route(
            r#"{"graph":"x","mode":"car","length_m":1.0,"duration_s":1.0,"points":[[4.0,52.0],[4.1,52.1]],"cum_dist_m":[0.0,1.0],"maneuvers":[{"kind":"arrive","roundabout_exit":null,"lon":4.1,"lat":52.1,"name":"","dist_m":1.0,"point_index":2,"text":"Arrive"}]}"#,
        )
        .is_err());
        assert!(parse_route(
            r#"{"graph":"x","mode":"car","length_m":1.0,"duration_s":1.0,"points":[[181.0,52.0],[4.1,52.1]],"cum_dist_m":[0.0,1.0],"maneuvers":[]}"#,
        )
        .is_err());
    }

    #[test]
    fn route_validation_caps_polyline_size() {
        let points = std::iter::repeat_n("[4.0,52.0]", MAX_ROUTE_POINTS + 1)
            .collect::<Vec<_>>()
            .join(",");
        let cumulative = std::iter::repeat_n("0.0", MAX_ROUTE_POINTS + 1)
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"graph":"x","mode":"car","length_m":0.0,"duration_s":0.0,"points":[{points}],"cum_dist_m":[{cumulative}],"maneuvers":[]}}"#
        );
        assert!(parse_route(&json).is_err());
    }

    #[test]
    fn hosted_array_and_layer_bounds_are_enforced() {
        let search_item = r#"{"name":"x","secondary":"","category":"city","lon":4.0,"lat":52.0,"distance_m":null,"score":1.0}"#;
        let search = format!(
            r#"{{"query":"x","results":[{}]}}"#,
            std::iter::repeat_n(search_item, MAX_SEARCH_RESULTS + 1)
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_search(&search).is_err());

        let along_item = r#"{"name":"x","kind":"charger","lon":4.0,"lat":52.0,"km_along":1.0,"detour_min":1.0,"extra":""}"#;
        let along = format!(
            r#"{{"results":[{}]}}"#,
            std::iter::repeat_n(along_item, MAX_ALONG_RESULTS + 1)
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_along(&along).is_err());

        let samples = (0..=MAX_WEATHER_SAMPLES)
            .map(|minute| format!(r#"{{"minute":{minute},"mm_h":0.0,"class":"dry"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_weather(&format!(
            r#"{{"stamp":"x","at":[4.0,52.0],"samples":[{samples}]}}"#
        ))
        .is_err());

        let minutes = (0..=MAX_RADAR_FRAMES)
            .map(|minute| minute.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_radar_manifest(&format!(
            r#"{{"stamp":"x","bbox":[0.0,48.0,10.0,56.0],"minutes":[{minutes}],"display":{{"width":1,"height":1}},"hires_now":{{"width":1,"height":1}}}}"#
        ))
        .is_err());
        assert!(parse_radar_manifest(
            r#"{"stamp":"x","bbox":[0.0,48.0,10.0,56.0],"minutes":[0],"display":{"width":0,"height":1},"hires_now":{"width":1,"height":1}}"#,
        )
        .is_err());
        assert!(parse_wind(
            r#"{"stamp_unix":1,"bbox":[2.0,48.0,9.0,56.0],"nx":2049,"ny":1,"u":[],"v":[]}"#,
        )
        .is_err());
        assert!(parse_wind(
            r#"{"stamp_unix":1,"bbox":[9.0,48.0,2.0,56.0],"nx":1,"ny":1,"u":[1.0],"v":[1.0]}"#,
        )
        .is_err());
    }

    #[test]
    fn route_and_along_events_preserve_their_generation_context() {
        let destination = SearchResult {
            doc_id: 7,
            name: "Utrecht".to_string(),
            secondary: String::new(),
            category: Category::City,
            pos: LonLat::new(5.1214, 52.0872),
            distance_m: None,
            score: 1.0,
        };
        let route_response = HttpResponse::new(
            LiveId(0),
            200,
            Default::default(),
            Some(br#"{"graph":"x","mode":"car","length_m":1.0,"duration_s":1.0,"points":[[4.0,52.0],[5.0,52.1]],"cum_dist_m":[0.0,1.0],"maneuvers":[]}"#.to_vec()),
        );
        let event = parse_response(
            PendingRequest::Route(RouteRequestContext::Initial {
                generation: 11,
                destination,
            }),
            &route_response,
            false,
        );
        assert!(matches!(
            event,
            NavApiEvent::Route {
                context: RouteRequestContext::Initial { generation: 11, .. },
                ..
            }
        ));

        let along_response = HttpResponse::new(
            LiveId(0),
            200,
            Default::default(),
            Some(br#"{"results":[]}"#.to_vec()),
        );
        assert!(matches!(
            parse_response(
                PendingRequest::Along {
                    route_generation: 11,
                },
                &along_response,
                false,
            ),
            NavApiEvent::Along {
                route_generation: 11,
                ..
            }
        ));
    }

    #[test]
    fn http_failure_parses_error_body_and_keeps_retry_context() {
        let response = HttpResponse::new(
            LiveId(0),
            429,
            Default::default(),
            Some(br#"{"error":{"code":"busy","message":"route queue is full"}}"#.to_vec()),
        );
        let event = parse_response(
            PendingRequest::Route(RouteRequestContext::Reroute { generation: 12 }),
            &response,
            true,
        );
        assert!(matches!(
            event,
            NavApiEvent::Failed {
                status: Some(429),
                message,
                route_context: Some(RouteRequestContext::Reroute { generation: 12 }),
                retrying: true,
                ..
            } if message == "busy: route queue is full"
        ));
    }

    #[test]
    fn duplicate_in_flight_radar_frame_is_detected() {
        let mut api = NavApi::new("https://example.invalid");
        api.pending.insert(
            LiveId(1),
            PendingCall {
                request: HttpRequest::new(String::new(), HttpMethod::GET),
                request_kind: PendingRequest::RadarFrame {
                    stamp: "stamp".to_string(),
                    minute: 5,
                    hires: false,
                },
                attempt: 0,
                timeout: Timer::empty(),
            },
        );
        assert!(api.has_radar_frame_request("stamp", 5, false));
        assert!(!api.has_radar_frame_request("stamp", 5, true));
    }
}
