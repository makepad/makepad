use crate::{
    config::Config,
    http::{api_error, json_response, json_string, query_pairs, send_response},
    live_services::LiveServiceRegistry,
};
use makepad_geodata::query::LayerDb;
use makepad_map_nav::{
    geo::{haversine_m, LonLat},
    graph::{Route, RouteGraph, TravelMode},
    nav::ManeuverKind,
    search::{SearchIndex, SearchResult},
    search_service::SearchService,
    searchdb::SearchDb,
};
use makepad_micro_serde::{DeJson, JsonValue};
use makepad_network::{
    http_server::{HttpServerResponse, HttpServerResponseSender},
    HttpServerHeaders,
};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex, RwLock,
    },
};

const PRIVATE_NO_STORE: &str = "private, no-store";
const MAX_ALONG_BODY: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct HealthState {
    pub ok: bool,
    pub search: &'static str,
    pub regional_graph: &'static str,
    pub major_graph: &'static str,
    // Reserved service slots for the later radar/weather/wind lane.
    pub radar: &'static str,
    pub wind: &'static str,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            ok: false,
            search: "unavailable",
            regional_graph: "unavailable",
            major_graph: "disabled",
            radar: "unavailable",
            wind: "unavailable",
        }
    }
}

impl HealthState {
    fn json(&self) -> String {
        format!(
            "{{\"ok\":{},\"search\":{},\"regional_graph\":{},\"major_graph\":{},\"radar\":{},\"wind\":{}}}",
            self.ok,
            json_string(self.search),
            json_string(self.regional_graph),
            json_string(self.major_graph),
            json_string(self.radar),
            json_string(self.wind),
        )
    }
}

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub query: String,
    pub near: Option<LonLat>,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct RouteRequest {
    pub from: LonLat,
    pub to: LonLat,
    pub mode: TravelMode,
    pub mode_name: &'static str,
}

#[derive(Clone, Debug)]
pub struct AlongRequest {
    pub polyline: Vec<LonLat>,
    pub cum_dist_m: Vec<f64>,
    pub kinds: Vec<String>,
    pub max_detour_min: f64,
    pub min_kw: f64,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub struct AlongResult {
    pub name: String,
    pub kind: String,
    pub lon: f64,
    pub lat: f64,
    pub km_along: f64,
    pub detour_min: f64,
    pub extra: String,
}

pub struct RouteResult {
    pub graph: String,
    pub route: Route,
}

#[derive(Clone, Debug)]
pub struct ApiFailure {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ApiFailure {
    fn response(self) -> HttpServerResponse {
        api_error(self.status, self.code, &self.message)
    }

    fn internal(message: impl Into<String>) -> Self {
        eprintln!("navigation API internal error: {}", message.into());
        Self { status: 500, code: "internal", message: "internal service error".into() }
    }
}

pub trait NavBackend: Send + Sync + 'static {
    fn search(&self, request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure>;
    fn route(&self, request: RouteRequest) -> Result<RouteResult, ApiFailure>;
    fn along(&self, request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure>;
}

struct ProductionBackend {
    search: SearchService,
    regional_graph: Arc<RouteGraph>,
    regional_name: String,
    major_graph: Option<Arc<RouteGraph>>,
    chargers: Option<Mutex<LayerDb>>,
}

impl NavBackend for ProductionBackend {
    fn search(&self, request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure> {
        Ok(self.search.query(&request.query, request.near, request.limit))
    }

    fn route(&self, request: RouteRequest) -> Result<RouteResult, ApiFailure> {
        if let Some(route) = self.regional_graph.route(request.from, request.to, request.mode) {
            return Ok(RouteResult { graph: self.regional_name.clone(), route });
        }
        if let Some(graph) = &self.major_graph {
            if let Some(route) = graph.route(request.from, request.to, request.mode) {
                return Ok(RouteResult { graph: "europe-major".into(), route });
            }
        }
        Err(ApiFailure {
            status: 422,
            code: "not_found",
            message: "no route found".into(),
        })
    }

    fn along(&self, request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure> {
        const MIN_PER_M: f64 = 2.0 / (40_000.0 / 60.0);
        let radius_m = (request.max_detour_min / MIN_PER_M).min(5_000.0);
        let samples = sample_along(&request.polyline, &request.cum_dist_m);
        let mut candidates = Vec::new();
        for kind in &request.kinds {
            let lower = kind.to_ascii_lowercase();
            let charger_kind = lower.contains("charg") || lower.contains("laad") || lower == "ev";
            if charger_kind {
                let Some(chargers) = &self.chargers else { continue };
                let mut chargers = chargers.lock().map_err(|_| ApiFailure::internal("charger database unavailable"))?;
                for &(point, along_m) in &samples {
                    let hits = chargers
                        .query_radius(point.lon, point.lat, radius_m, 8)
                        .map_err(ApiFailure::internal)?;
                    for hit in hits {
                        let kw = hit.attrs.get("max_kw").and_then(|value| value.as_i64()).unwrap_or(0);
                        if (kw as f64) < request.min_kw {
                            continue;
                        }
                        let operator = hit.attrs.get("operator").and_then(|value| value.as_str()).unwrap_or("");
                        let city = hit.attrs.get("city").and_then(|value| value.as_str()).unwrap_or("");
                        let name = hit.name.filter(|name| !name.is_empty()).unwrap_or_else(|| {
                            format!("{operator} {city}").trim().to_string()
                        });
                        let distance = hit.distance_m.unwrap_or(radius_m);
                        candidates.push(AlongResult {
                            name,
                            kind: "charger".into(),
                            lon: hit.center.0,
                            lat: hit.center.1,
                            km_along: along_m / 1_000.0,
                            detour_min: distance * MIN_PER_M,
                            extra: format!("{kw} kW, {operator}"),
                        });
                    }
                }
            } else {
                for &(point, along_m) in &samples {
                    for result in self.search.query(kind, Some(point), 6) {
                        let Some(distance) = result.distance_m.filter(|distance| *distance <= radius_m) else {
                            continue;
                        };
                        let category = category_name(&result);
                        candidates.push(AlongResult {
                            name: result.name,
                            kind: category,
                            lon: result.pos.lon,
                            lat: result.pos.lat,
                            km_along: along_m / 1_000.0,
                            detour_min: distance * MIN_PER_M,
                            extra: result.secondary,
                        });
                    }
                }
            }
        }
        candidates.sort_by(|a, b| a.detour_min.total_cmp(&b.detour_min));
        let mut kept: Vec<AlongResult> = Vec::new();
        for candidate in candidates {
            if candidate.detour_min > request.max_detour_min
                || kept.iter().any(|existing| {
                    existing.name.eq_ignore_ascii_case(&candidate.name)
                        && haversine_m(
                            LonLat::new(existing.lon, existing.lat),
                            LonLat::new(candidate.lon, candidate.lat),
                        ) < 1_000.0
                })
            {
                continue;
            }
            kept.push(candidate);
        }
        kept.sort_by(|a, b| a.km_along.total_cmp(&b.km_along));
        kept.truncate(request.limit);
        Ok(kept)
    }
}

enum QueryJob {
    Search(SearchRequest, HttpServerResponseSender),
    Along(AlongRequest, HttpServerResponseSender),
}

struct RouteJob(RouteRequest, HttpServerResponseSender);

#[derive(Clone)]
struct WorkerSenders {
    query: SyncSender<QueryJob>,
    route: SyncSender<RouteJob>,
}

pub struct ServiceRegistry {
    health: RwLock<HealthState>,
    workers: RwLock<Option<WorkerSenders>>,
    pub live: LiveServiceRegistry,
}

impl ServiceRegistry {
    pub fn new(major_enabled: bool) -> Arc<Self> {
        let mut health = HealthState::default();
        if major_enabled {
            health.major_graph = "loading";
        }
        Arc::new(Self {
            health: RwLock::new(health),
            workers: RwLock::new(None),
            live: LiveServiceRegistry::default(),
        })
    }

    pub fn install(
        self: &Arc<Self>,
        backend: Arc<dyn NavBackend>,
        query_workers: usize,
        route_workers: usize,
        route_queue: usize,
        major_ready: bool,
    ) {
        let (query_sender, query_receiver) = mpsc::sync_channel(route_queue);
        let (route_sender, route_receiver) = mpsc::sync_channel(route_queue);
        spawn_query_workers(backend.clone(), query_receiver, query_workers);
        spawn_route_workers(backend, route_receiver, route_workers);
        *self.workers.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(WorkerSenders {
            query: query_sender,
            route: route_sender,
        });
        *self.health.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = HealthState {
            ok: true,
            search: "ready",
            regional_graph: "ready",
            major_graph: if major_ready { "ready" } else { "disabled" },
            radar: "unavailable",
            wind: "unavailable",
        };
    }

    pub fn fail(&self, major_enabled: bool) {
        *self.health.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = HealthState {
            major_graph: if major_enabled { "failed" } else { "disabled" },
            ..HealthState::default()
        };
    }

    pub fn health(&self) -> HealthState {
        let mut health = self.health.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let live = self.live.state();
        health.radar = live.radar;
        health.wind = live.wind;
        health
    }

    pub fn handle_get(&self, headers: &HttpServerHeaders, sender: &HttpServerResponseSender) -> bool {
        match headers.path.as_str() {
            "/api/healthz" => {
                if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
                    send_response(sender, api_error(405, "bad_request", "method not allowed"));
                } else {
                    send_response(sender, json_response(200, "no-store", self.health().json()));
                }
                true
            }
            "/api/search" => {
                if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
                    send_response(sender, api_error(405, "bad_request", "method not allowed"));
                    return true;
                }
                let request = match parse_search(headers.search.as_deref()) {
                    Ok(request) => request,
                    Err(error) => {
                        send_response(sender, error.response());
                        return true;
                    }
                };
                self.enqueue_query(QueryJob::Search(request, sender.clone()));
                true
            }
            "/api/route" => {
                if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
                    send_response(sender, api_error(405, "bad_request", "method not allowed"));
                    return true;
                }
                let request = match parse_route(headers.search.as_deref()) {
                    Ok(request) => request,
                    Err(error) => {
                        send_response(sender, error.response());
                        return true;
                    }
                };
                self.enqueue_route(RouteJob(request, sender.clone()));
                true
            }
            "/api/along" => {
                send_response(sender, api_error(405, "bad_request", "method not allowed"));
                true
            }
            path if path.starts_with("/api/") => {
                send_response(sender, api_error(404, "not_found", "API endpoint not found"));
                true
            }
            _ => false,
        }
    }

    pub fn handle_post(
        &self,
        headers: &HttpServerHeaders,
        body: &[u8],
        sender: &HttpServerResponseSender,
    ) -> bool {
        if headers.path == "/api/along" {
            let request = match parse_along(body) {
                Ok(request) => request,
                Err(error) => {
                    send_response(sender, error.response());
                    return true;
                }
            };
            self.enqueue_query(QueryJob::Along(request, sender.clone()));
            true
        } else if headers.path.starts_with("/api/") {
            let status = if matches!(headers.path.as_str(), "/api/search" | "/api/route" | "/api/healthz") {
                405
            } else {
                404
            };
            let code = if status == 404 { "not_found" } else { "bad_request" };
            send_response(sender, api_error(status, code, if status == 404 { "API endpoint not found" } else { "method not allowed" }));
            true
        } else {
            false
        }
    }

    fn enqueue_query(&self, job: QueryJob) {
        let workers = self.workers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let sender = match &job {
            QueryJob::Search(_, sender) | QueryJob::Along(_, sender) => sender.clone(),
        };
        let Some(workers) = workers else {
            send_response(&sender, api_error(503, "unavailable", "navigation services are not ready"));
            return;
        };
        match workers.query.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(job)) => {
                let sender = match job { QueryJob::Search(_, sender) | QueryJob::Along(_, sender) => sender };
                send_response(&sender, api_error(429, "busy", "query queue is full"));
            }
            Err(TrySendError::Disconnected(job)) => {
                let sender = match job { QueryJob::Search(_, sender) | QueryJob::Along(_, sender) => sender };
                send_response(&sender, api_error(503, "unavailable", "query workers are unavailable"));
            }
        }
    }

    fn enqueue_route(&self, job: RouteJob) {
        let workers = self.workers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let sender = job.1.clone();
        let Some(workers) = workers else {
            send_response(&sender, api_error(503, "unavailable", "navigation services are not ready"));
            return;
        };
        match workers.route.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(job)) => send_response(&job.1, api_error(429, "busy", "route queue is full")),
            Err(TrySendError::Disconnected(job)) => send_response(&job.1, api_error(503, "unavailable", "route workers are unavailable")),
        }
    }
}

fn spawn_query_workers(backend: Arc<dyn NavBackend>, receiver: Receiver<QueryJob>, count: usize) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..count {
        let backend = backend.clone();
        let receiver = receiver.clone();
        std::thread::Builder::new()
            .name(format!("web-query-{index}"))
            .spawn(move || loop {
                let job = receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
                match job {
                    Ok(QueryJob::Search(request, sender)) => {
                        let query = request.query.clone();
                        let response = backend
                            .search(request)
                            .map(|results| search_response_for(&query, results))
                            .unwrap_or_else(ApiFailure::response);
                        send_response(&sender, response);
                    }
                    Ok(QueryJob::Along(request, sender)) => {
                        let response = backend.along(request).map(along_response).unwrap_or_else(ApiFailure::response);
                        send_response(&sender, response);
                    }
                    Err(_) => break,
                }
            })
            .expect("start query worker");
    }
}

fn spawn_route_workers(backend: Arc<dyn NavBackend>, receiver: Receiver<RouteJob>, count: usize) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..count {
        let backend = backend.clone();
        let receiver = receiver.clone();
        std::thread::Builder::new()
            .name(format!("web-route-{index}"))
            .spawn(move || loop {
                let job = receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
                match job {
                    Ok(RouteJob(request, sender)) => {
                        let mode_name = request.mode_name;
                        let response = backend.route(request).map(|result| route_response(result, mode_name)).unwrap_or_else(ApiFailure::response);
                        send_response(&sender, response);
                    }
                    Err(_) => break,
                }
            })
            .expect("start route worker");
    }
}

pub fn start_production_load(config: Config, data_root: PathBuf, registry: Arc<ServiceRegistry>) {
    std::thread::Builder::new()
        .name("web-nav-loader".into())
        .spawn(move || {
            let major_enabled = config.major_graph.is_some();
            match load_backend(&config, &data_root) {
                Ok((backend, total_bytes, major_ready)) => {
                    eprintln!("nav ready: {total_bytes} bytes validated");
                    registry.install(
                        Arc::new(backend),
                        config.query_workers,
                        config.route_workers,
                        config.route_queue,
                        major_ready,
                    );
                }
                Err(error) => {
                    eprintln!("nav load failed: {error}");
                    registry.fail(major_enabled);
                }
            }
        })
        .expect("start nav loader");
}

fn load_backend(config: &Config, data_root: &Path) -> Result<(ProductionBackend, u64, bool), String> {
    let regional_search_path = resolve_basename(data_root, &config.nav_basename, ".search")?;
    let regional_graph_path = resolve_basename(data_root, &config.nav_basename, ".graph")?;
    let mut total_bytes = 0;
    let regional_search_bytes = read_counted(&regional_search_path, &mut total_bytes)?;
    let regional = Arc::new(SearchIndex::deserialize(&regional_search_bytes).map_err(|error| format!("regional search: {error:?}"))?);
    let regional_graph_bytes = read_counted(&regional_graph_path, &mut total_bytes)?;
    let regional_graph = Arc::new(RouteGraph::deserialize(&regional_graph_bytes).map_err(|error| format!("regional graph: {error:?}"))?);

    let searchdb_path = optional_resolve(data_root, config.searchdb.as_deref())?;
    let searchdb = match searchdb_path {
        Some(path) => {
            let len = path.metadata().map_err(|error| error.to_string())?.len();
            total_bytes += len;
            eprintln!("nav data: {} {len} bytes", path.display());
            Some(Arc::new(SearchDb::open(&path).map_err(|error| format!("searchdb: {error:?}"))?))
        }
        None => None,
    };
    let places = if searchdb.is_none() {
        match optional_resolve(data_root, config.places.as_deref())? {
            Some(path) => {
                let bytes = read_counted(&path, &mut total_bytes)?;
                Some(Arc::new(SearchIndex::deserialize(&bytes).map_err(|error| format!("places search: {error:?}"))?))
            }
            None => None,
        }
    } else {
        None
    };
    let major_graph = match config.major_graph.as_deref() {
        Some(relative) => {
            let path = resolve_data_path(data_root, relative)?;
            let bytes = read_counted(&path, &mut total_bytes)?;
            Some(Arc::new(RouteGraph::deserialize(&bytes).map_err(|error| format!("major graph: {error:?}"))?))
        }
        None => None,
    };
    let chargers = match config.chargers.as_deref() {
        Some(relative) => {
            let path = resolve_data_path(data_root, relative)?;
            let len = path.metadata().map_err(|error| error.to_string())?.len();
            total_bytes += len;
            eprintln!("nav data: {} {len} bytes", path.display());
            Some(Mutex::new(LayerDb::open(&path)?))
        }
        None => None,
    };
    let regional_name = config
        .nav_basename
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("regional")
        .to_string();
    let major_ready = major_graph.is_some();
    Ok((ProductionBackend {
        search: SearchService::new(regional, searchdb, places),
        regional_graph,
        regional_name,
        major_graph,
        chargers,
    }, total_bytes, major_ready))
}

fn resolve_basename(root: &Path, basename: &Path, suffix: &str) -> Result<PathBuf, String> {
    let mut path: OsString = basename.as_os_str().to_owned();
    path.push(suffix);
    resolve_data_path(root, Path::new(&path))
}

fn optional_resolve(root: &Path, path: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let Some(path) = path else { return Ok(None) };
    validate_relative_data_path(path)?;
    let unresolved = root.join(path);
    if !unresolved.exists() {
        return Ok(None);
    }
    resolve_data_path(root, path).map(Some)
}

fn resolve_data_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    validate_relative_data_path(relative)?;
    let path = root.join(relative).canonicalize().map_err(|error| format!("data file {}: {error}", relative.display()))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(format!("data path is not a regular file under --data-dir: {}", relative.display()));
    }
    Ok(path)
}

fn validate_relative_data_path(relative: &Path) -> Result<(), String> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(format!(
            "data path must be relative and remain under --data-dir: {}",
            relative.display()
        ));
    }
    Ok(())
}

fn read_counted(path: &Path, total: &mut u64) -> Result<Vec<u8>, String> {
    let metadata = path.metadata().map_err(|error| error.to_string())?;
    *total += metadata.len();
    eprintln!("nav data: {} {} bytes", path.display(), metadata.len());
    fs::read(path).map_err(|error| error.to_string())
}

fn parse_search(query: Option<&str>) -> Result<SearchRequest, ApiFailure> {
    let pairs = query_pairs(query).map_err(|_| bad_request("invalid query encoding"))?;
    let query = value(&pairs, "q").ok_or_else(|| bad_request("missing q"))?.trim().to_string();
    if query.is_empty() || query.len() > 64 || query.split_whitespace().count() > 8 {
        return Err(bad_request("q must be 1..64 UTF-8 bytes and at most eight words"));
    }
    let near = value(&pairs, "near").map(parse_coordinate).transpose()?;
    let limit = parse_limit(value(&pairs, "limit"), 8, 20)?;
    Ok(SearchRequest { query, near, limit })
}

fn parse_route(query: Option<&str>) -> Result<RouteRequest, ApiFailure> {
    let pairs = query_pairs(query).map_err(|_| bad_request("invalid query encoding"))?;
    let from = parse_coordinate(value(&pairs, "from").ok_or_else(|| bad_request("missing from"))?)?;
    let to = parse_coordinate(value(&pairs, "to").ok_or_else(|| bad_request("missing to"))?)?;
    let (mode, mode_name) = match value(&pairs, "mode").unwrap_or("car") {
        "car" => (TravelMode::Car, "car"),
        "bike" => (TravelMode::Bike, "bike"),
        "foot" => (TravelMode::Foot, "foot"),
        _ => return Err(bad_request("mode must be car, bike, or foot")),
    };
    Ok(RouteRequest { from, to, mode, mode_name })
}

fn parse_along(body: &[u8]) -> Result<AlongRequest, ApiFailure> {
    if body.len() > MAX_ALONG_BODY {
        return Err(bad_request("request body exceeds 2 MiB"));
    }
    let text = std::str::from_utf8(body).map_err(|_| bad_request("body must be UTF-8 JSON"))?;
    let json = JsonValue::deserialize_json(text).map_err(|_| bad_request("invalid JSON body"))?;
    if json.object().is_none() {
        return Err(bad_request("JSON body must be an object"));
    }
    let polyline_values = json_array(&json, "polyline")?;
    if !(2..=20_000).contains(&polyline_values.len()) {
        return Err(bad_request("polyline must contain 2..=20000 points"));
    }
    let mut polyline = Vec::with_capacity(polyline_values.len());
    for value in polyline_values {
        let pair = match value { JsonValue::Array(pair) if pair.len() == 2 => pair, _ => return Err(bad_request("polyline points must be [lon,lat]")) };
        polyline.push(validate_coordinate(number(&pair[0])?, number(&pair[1])?)?);
    }
    let cum_values = json_array(&json, "cum_dist_m")?;
    if cum_values.len() != polyline.len() {
        return Err(bad_request("cum_dist_m must match polyline length"));
    }
    let mut cum_dist_m = Vec::with_capacity(cum_values.len());
    for value in cum_values {
        let distance = number(value)?;
        if !distance.is_finite() || distance < 0.0 || cum_dist_m.last().is_some_and(|previous| distance < *previous) {
            return Err(bad_request("cum_dist_m must be finite, non-negative, and sorted"));
        }
        cum_dist_m.push(distance);
    }
    if cum_dist_m.last().copied().unwrap_or(0.0) <= 0.0 {
        return Err(bad_request("route distance must be positive"));
    }
    let kind_values = json_array(&json, "kinds")?;
    if !(1..=4).contains(&kind_values.len()) {
        return Err(bad_request("kinds must contain 1..=4 values"));
    }
    let mut kinds = Vec::with_capacity(kind_values.len());
    for value in kind_values {
        let kind = value.string().ok_or_else(|| bad_request("kinds must be strings"))?.trim();
        if kind.is_empty() || kind.len() > 32 {
            return Err(bad_request("each kind must be 1..32 bytes"));
        }
        kinds.push(kind.to_string());
    }
    let max_detour_min = optional_number(&json, "max_detour_min")?.unwrap_or(10.0);
    if !max_detour_min.is_finite() || !(1.0..=60.0).contains(&max_detour_min) {
        return Err(bad_request("max_detour_min must be in 1..=60"));
    }
    let min_kw = optional_number(&json, "min_kw")?.unwrap_or(0.0);
    if !min_kw.is_finite() || !(0.0..=5_000.0).contains(&min_kw) {
        return Err(bad_request("min_kw must be in 0..=5000"));
    }
    let limit = optional_number(&json, "limit")?.unwrap_or(12.0);
    if !limit.is_finite() || limit.fract() != 0.0 || !(1.0..=30.0).contains(&limit) {
        return Err(bad_request("limit must be an integer in 1..=30"));
    }
    Ok(AlongRequest { polyline, cum_dist_m, kinds, max_detour_min, min_kw, limit: limit as usize })
}

fn json_array<'a>(json: &'a JsonValue, key: &str) -> Result<&'a Vec<JsonValue>, ApiFailure> {
    match json.key(key) {
        Some(JsonValue::Array(values)) => Ok(values),
        _ => Err(bad_request(&format!("missing or invalid {key}"))),
    }
}

fn optional_number(json: &JsonValue, key: &str) -> Result<Option<f64>, ApiFailure> {
    json.key(key).map(number).transpose()
}

fn number(value: &JsonValue) -> Result<f64, ApiFailure> {
    match value {
        JsonValue::F64(value) => Ok(*value),
        JsonValue::I64(value) => Ok(*value as f64),
        JsonValue::U64(value) => Ok(*value as f64),
        JsonValue::I128(value) => Ok(*value as f64),
        JsonValue::U128(value) => Ok(*value as f64),
        _ => Err(bad_request("expected a number")),
    }
}

fn value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
}

fn parse_limit(value: Option<&str>, default: usize, max: usize) -> Result<usize, ApiFailure> {
    let Some(value) = value else { return Ok(default) };
    let limit = value.parse::<usize>().map_err(|_| bad_request("limit must be an integer"))?;
    if !(1..=max).contains(&limit) {
        return Err(bad_request(&format!("limit must be in 1..={max}")));
    }
    Ok(limit)
}

fn parse_coordinate(value: &str) -> Result<LonLat, ApiFailure> {
    let (lon, lat) = value.split_once(',').ok_or_else(|| bad_request("coordinates must be lon,lat"))?;
    if lat.contains(',') {
        return Err(bad_request("coordinates must be lon,lat"));
    }
    validate_coordinate(
        lon.parse().map_err(|_| bad_request("invalid longitude"))?,
        lat.parse().map_err(|_| bad_request("invalid latitude"))?,
    )
}

fn validate_coordinate(lon: f64, lat: f64) -> Result<LonLat, ApiFailure> {
    if !lon.is_finite() || !lat.is_finite() || !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err(bad_request("coordinates are outside WGS84 bounds"));
    }
    Ok(LonLat::new(lon, lat))
}

fn bad_request(message: impl Into<String>) -> ApiFailure {
    ApiFailure { status: 400, code: "bad_request", message: message.into() }
}

fn search_response_for(query: &str, results: Vec<SearchResult>) -> HttpServerResponse {
    let mut json = format!("{{\"query\":{},\"results\":[", json_string(query));
    for (index, result) in results.iter().enumerate() {
        if index > 0 { json.push(','); }
        let distance = result.distance_m.map(json_number).unwrap_or_else(|| "null".into());
        json.push_str(&format!(
            "{{\"name\":{},\"secondary\":{},\"category\":{},\"lon\":{},\"lat\":{},\"distance_m\":{},\"score\":{}}}",
            json_string(&result.name), json_string(&result.secondary), json_string(&category_name(result)),
            json_number(result.pos.lon), json_number(result.pos.lat), distance, json_number(result.score)
        ));
    }
    json.push_str("]}");
    json_response(200, PRIVATE_NO_STORE, json)
}

fn route_response(result: RouteResult, mode_name: &str) -> HttpServerResponse {
    let route = result.route;
    let mut json = format!(
        "{{\"graph\":{},\"mode\":{},\"length_m\":{},\"duration_s\":{},\"points\":[",
        json_string(&result.graph), json_string(mode_name), json_number(route.length_m), json_number(route.duration_s)
    );
    for (index, point) in route.points.iter().enumerate() {
        if index > 0 { json.push(','); }
        json.push_str(&format!("[{},{}]", json_number(point.lon), json_number(point.lat)));
    }
    json.push_str("],\"cum_dist_m\":[");
    for (index, distance) in route.cum_dist_m.iter().enumerate() {
        if index > 0 { json.push(','); }
        json.push_str(&json_number(*distance));
    }
    json.push_str("],\"maneuvers\":[");
    for (index, maneuver) in route.maneuvers.iter().enumerate() {
        if index > 0 { json.push(','); }
        let (kind, exit) = maneuver_kind(maneuver.kind);
        json.push_str(&format!(
            "{{\"kind\":{},\"roundabout_exit\":{},\"lon\":{},\"lat\":{},\"name\":{},\"dist_m\":{},\"point_index\":{},\"text\":{}}}",
            json_string(kind), exit.map(|value| value.to_string()).unwrap_or_else(|| "null".into()),
            json_number(maneuver.at.lon), json_number(maneuver.at.lat), json_string(&maneuver.name),
            json_number(maneuver.dist_m), maneuver.point_index, json_string(&maneuver.text())
        ));
    }
    json.push_str("]}");
    json_response(200, PRIVATE_NO_STORE, json)
}

fn along_response(results: Vec<AlongResult>) -> HttpServerResponse {
    let mut json = "{\"results\":[".to_string();
    for (index, result) in results.iter().enumerate() {
        if index > 0 { json.push(','); }
        json.push_str(&format!(
            "{{\"name\":{},\"kind\":{},\"lon\":{},\"lat\":{},\"km_along\":{},\"detour_min\":{},\"extra\":{}}}",
            json_string(&result.name), json_string(&result.kind), json_number(result.lon), json_number(result.lat),
            json_number(result.km_along), json_number(result.detour_min), json_string(&result.extra)
        ));
    }
    json.push_str("]}");
    json_response(200, PRIVATE_NO_STORE, json)
}

fn category_name(result: &SearchResult) -> String {
    result.category.label().to_ascii_lowercase().replace(' ', "_")
}

fn maneuver_kind(kind: ManeuverKind) -> (&'static str, Option<u8>) {
    match kind {
        ManeuverKind::Depart => ("depart", None),
        ManeuverKind::Arrive => ("arrive", None),
        ManeuverKind::TurnSlightLeft => ("turn_slight_left", None),
        ManeuverKind::TurnLeft => ("turn_left", None),
        ManeuverKind::TurnSharpLeft => ("turn_sharp_left", None),
        ManeuverKind::TurnSlightRight => ("turn_slight_right", None),
        ManeuverKind::TurnRight => ("turn_right", None),
        ManeuverKind::TurnSharpRight => ("turn_sharp_right", None),
        ManeuverKind::UTurn => ("u_turn", None),
        ManeuverKind::RoundaboutExit(exit) => ("roundabout_exit", Some(exit)),
    }
}

fn json_number(value: f64) -> String {
    if value.is_finite() { value.to_string() } else { "null".into() }
}

fn sample_along(polyline: &[LonLat], cumulative: &[f64]) -> Vec<(LonLat, f64)> {
    let total = cumulative.last().copied().unwrap_or(0.0);
    let spacing = (total / 48.0).max(3_000.0);
    let intervals = ((total / spacing).ceil() as usize).clamp(1, 47);
    (0..=intervals)
        .map(|index| {
            let distance = total * index as f64 / intervals as f64;
            (interpolate(polyline, cumulative, distance), distance)
        })
        .collect()
}

fn interpolate(polyline: &[LonLat], cumulative: &[f64], distance: f64) -> LonLat {
    let index = cumulative.partition_point(|value| *value < distance).min(cumulative.len() - 1);
    if index == 0 { return polyline[0] }
    let start = cumulative[index - 1];
    let end = cumulative[index];
    let fraction = if end > start { (distance - start) / (end - start) } else { 0.0 };
    LonLat::new(
        polyline[index - 1].lon + (polyline[index].lon - polyline[index - 1].lon) * fraction,
        polyline[index - 1].lat + (polyline[index].lat - polyline[index - 1].lat) * fraction,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_bounds_are_strict() {
        assert!(parse_search(Some("q=Amsterdam&limit=20")).is_ok());
        assert!(parse_search(Some("q=one+two+three+four+five+six+seven+eight+nine")).is_err());
        assert!(parse_search(Some("q=Amsterdam&limit=0")).is_err());
        assert!(parse_search(Some("q=Amsterdam&near=181,52")).is_err());
    }

    #[test]
    fn route_rejects_bad_coordinates_and_modes() {
        assert!(parse_route(Some("from=4.9,52.3&to=5.1,52.1&mode=bike")).is_ok());
        assert!(parse_route(Some("from=4.9,nan&to=5.1,52.1&mode=car")).is_err());
        assert!(parse_route(Some("from=4.9,52.3&to=5.1,52.1&mode=plane")).is_err());
    }

    #[test]
    fn along_bounds_body_and_arrays() {
        let valid = br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[0,10000],"kinds":["museum"],"limit":12}"#;
        let request = parse_along(valid).unwrap();
        assert_eq!(request.limit, 12);
        assert!(parse_along(br#"{"polyline":[[4.9,52.3]],"cum_dist_m":[0],"kinds":["museum"]}"#).is_err());
        assert!(parse_along(&vec![b' '; MAX_ALONG_BODY + 1]).is_err());
    }

    #[test]
    fn corridor_sampling_is_bounded() {
        let points = [LonLat::new(0.0, 0.0), LonLat::new(1.0, 1.0)];
        let samples = sample_along(&points, &[0.0, 1_000_000.0]);
        assert!(samples.len() <= 48);
        assert_eq!(samples.first().unwrap().1, 0.0);
        assert_eq!(samples.last().unwrap().1, 1_000_000.0);
    }
}
