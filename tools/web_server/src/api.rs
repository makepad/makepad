use crate::{
    config::Config,
    http::{api_error, json_response, json_string, query_pairs, response, send_response},
    live_services::{LiveHealth, LiveServiceRegistry},
};
use makepad_geodata::query::LayerDb;
use makepad_map_nav::{
    geo::{cumulative_distances, haversine_m, sample_polyline, LonLat, MAX_ROUTE_POINTS},
    graph::{Route, RouteGraph, TravelMode},
    nav::ManeuverKind,
    search::{SearchIndex, SearchResult},
    search_service::SearchService,
    searchdb::SearchDb,
};
use makepad_network::{
    http_server::{HttpServerPendingBody, HttpServerResponse, HttpServerResponseSender},
    HttpServerHeaders,
};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, RwLock, Weak,
    },
};

const PRIVATE_NO_STORE: &str = "private, no-store";
const MAX_ALONG_BODY: usize = 2 * 1024 * 1024;
const MAX_ALONG_ALLOC: usize = 1024 * 1024;
const MAX_ROUTE_MANEUVERS: usize = 10_000;
const MAX_ROUTE_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHARGER_CANDIDATES: usize = 20_000;

#[derive(Clone, Debug)]
pub struct HealthState {
    pub ok: bool,
    pub search: &'static str,
    pub along: &'static str,
    pub chargers: &'static str,
    pub regional_graph: &'static str,
    pub major_graph: &'static str,
    pub radar: LiveHealth,
    pub wind: LiveHealth,
    pub weather: LiveHealth,
}

impl Default for HealthState {
    fn default() -> Self {
        Self {
            ok: false,
            search: "unavailable",
            along: "unavailable",
            chargers: "unavailable",
            regional_graph: "unavailable",
            major_graph: "disabled",
            radar: LiveHealth { status: "unavailable", updated_unix: None },
            wind: LiveHealth { status: "unavailable", updated_unix: None },
            weather: LiveHealth { status: "unavailable", updated_unix: None },
        }
    }
}

impl HealthState {
    fn json(&self) -> String {
        format!(
            "{{\"ok\":{},\"search\":{},\"along\":{},\"chargers\":{},\"regional_graph\":{},\"major_graph\":{},\"radar\":{},\"wind\":{},\"weather\":{}}}",
            self.ok,
            json_string(self.search),
            json_string(self.along),
            json_string(self.chargers),
            json_string(self.regional_graph),
            json_string(self.major_graph),
            self.radar.json(),
            self.wind.json(),
            self.weather.json(),
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
    fn chargers_status(&self) -> &'static str {
        "disabled"
    }
}

struct ProductionBackend {
    search: SearchService,
    regional_graph: Arc<RouteGraph>,
    regional_name: String,
    major_graph: Option<Arc<RouteGraph>>,
    chargers: Option<ReopenableLayerDb>,
}

struct ReopenableLayerDb {
    path: PathBuf,
    db: Mutex<Option<LayerDb>>,
}

impl ReopenableLayerDb {
    fn query_radius(
        &self,
        point: LonLat,
        radius_m: f64,
        limit: usize,
        budget: &mut usize,
    ) -> Result<Vec<makepad_geodata::query::FeatureHit>, ApiFailure> {
        let mut guard = match self.db.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = None;
                self.db.clear_poison();
                guard
            }
        };
        if guard.is_none() {
            *guard = Some(LayerDb::open(&self.path).map_err(ApiFailure::internal)?);
        }
        let result = guard
            .as_mut()
            .expect("charger database was reopened")
            .query_radius_with_budget(point.lon, point.lat, radius_m, limit, budget);
        if result.is_err() {
            *guard = None;
        }
        result.map_err(ApiFailure::internal)
    }
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
        let samples = sample_along(&request.polyline);
        let mut candidates = Vec::new();
        let mut charger_budget = MAX_CHARGER_CANDIDATES;
        for kind in &request.kinds {
            let lower = kind.to_ascii_lowercase();
            let charger_kind = lower.contains("charg") || lower.contains("laad") || lower == "ev";
            if charger_kind && self.chargers.is_some() {
                let chargers = self.chargers.as_ref().unwrap();
                for &(point, along_m) in &samples {
                    let hits = chargers
                        .query_radius(point, radius_m, 8, &mut charger_budget)?;
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

    fn chargers_status(&self) -> &'static str {
        if self.chargers.is_some() { "ready" } else { "disabled" }
    }
}

enum QueryJob {
    Search(SearchRequest, HttpServerResponseSender),
    Along(Vec<u8>, AlongPermit, HttpServerResponseSender),
}

struct RouteJob(RouteRequest, HttpServerResponseSender);

#[derive(Clone)]
struct WorkerSenders {
    search: SyncSender<QueryJob>,
    along: SyncSender<QueryJob>,
    along_admission: Arc<AlongAdmission>,
    route: SyncSender<RouteJob>,
}

struct AlongAdmission {
    used: AtomicUsize,
    limit: usize,
}

struct AlongPermit {
    admission: Arc<AlongAdmission>,
}

impl AlongAdmission {
    fn try_acquire(self: &Arc<Self>) -> Option<AlongPermit> {
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            if used >= self.limit {
                return None;
            }
            match self.used.compare_exchange_weak(
                used,
                used + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(AlongPermit { admission: self.clone() }),
                Err(changed) => used = changed,
            }
        }
    }
}

impl Drop for AlongPermit {
    fn drop(&mut self) {
        self.admission.used.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct ServiceRegistry {
    health: RwLock<HealthState>,
    workers: RwLock<Option<WorkerSenders>>,
    pub live: Arc<LiveServiceRegistry>,
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
            live: Arc::new(LiveServiceRegistry::default()),
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
        let chargers = backend.chargers_status();
        let (search_sender, search_receiver) = mpsc::sync_channel(route_queue);
        let (along_sender, along_receiver) = mpsc::sync_channel(route_queue);
        let (route_sender, route_receiver) = mpsc::sync_channel(route_queue);
        let along_workers = query_workers.saturating_sub(1).max(1);
        let along_admission = Arc::new(AlongAdmission {
            used: AtomicUsize::new(0),
            limit: route_queue.saturating_add(along_workers),
        });
        let registry = Arc::downgrade(self);
        spawn_query_workers(registry.clone(), backend.clone(), search_receiver, 1, "search");
        spawn_query_workers(registry.clone(), backend.clone(), along_receiver, along_workers, "along");
        spawn_route_workers(registry, backend, route_receiver, route_workers);
        *self.workers.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(WorkerSenders {
            search: search_sender,
            along: along_sender,
            along_admission,
            route: route_sender,
        });
        *self.health.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = HealthState {
            ok: true,
            search: "ready",
            along: "ready",
            chargers,
            regional_graph: "ready",
            major_graph: if major_ready { "ready" } else { "disabled" },
            radar: LiveHealth { status: "unavailable", updated_unix: None },
            wind: LiveHealth { status: "unavailable", updated_unix: None },
            weather: LiveHealth { status: "unavailable", updated_unix: None },
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
        health.weather = live.weather;
        health
    }

    fn mark_degraded(&self, service: &'static str) {
        let mut health = self.health.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        health.ok = false;
        match service {
            "route" => health.regional_graph = "degraded",
            "along" => health.along = "degraded",
            _ => health.search = "degraded",
        }
    }

    pub fn handle_get(&self, headers: &HttpServerHeaders, sender: &HttpServerResponseSender) -> bool {
        if self.live.handle_get(headers, sender) {
            return true;
        }
        match headers.path.as_str() {
            "/api/healthz" => {
                if headers.verb == "OPTIONS" {
                    send_response(sender, api_options("GET, HEAD, OPTIONS"));
                } else if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
                    send_response(sender, api_method_not_allowed("GET, HEAD, OPTIONS"));
                } else {
                    let health = self.health();
                    send_response(sender, json_response(if health.ok { 200 } else { 503 }, "no-store", health.json()));
                }
                true
            }
            "/api/search" => {
                if headers.verb == "OPTIONS" {
                    send_response(sender, api_options("GET, HEAD, OPTIONS"));
                    return true;
                }
                if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
                    send_response(sender, api_method_not_allowed("GET, HEAD, OPTIONS"));
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
                if headers.verb == "OPTIONS" {
                    send_response(sender, api_options("GET, HEAD, OPTIONS"));
                    return true;
                }
                if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
                    send_response(sender, api_method_not_allowed("GET, HEAD, OPTIONS"));
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
                if headers.verb == "OPTIONS" {
                    send_response(sender, api_options("POST, OPTIONS"));
                } else {
                    send_response(sender, api_method_not_allowed("POST, OPTIONS"));
                }
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
        body: Vec<u8>,
        sender: &HttpServerResponseSender,
    ) -> bool {
        if headers.path == "/api/along" {
            if !has_json_content_type(headers) {
                send_response(sender, api_error(415, "unsupported_media_type", "Content-Type must be application/json"));
                return true;
            }
            if body.len() > MAX_ALONG_BODY {
                send_response(sender, api_error(413, "too_large", "request body exceeds 2 MiB"));
                return true;
            }
            self.enqueue_along(body, sender.clone());
            true
        } else if headers.path.starts_with("/api/") {
            let status = if matches!(headers.path.as_str(), "/api/search" | "/api/route" | "/api/healthz" | "/api/radar/manifest" | "/api/radar/frame" | "/api/wind/current" | "/api/weather/now") {
                405
            } else {
                404
            };
            let code = if status == 404 { "not_found" } else { "bad_request" };
            if status == 405 {
                send_response(sender, api_method_not_allowed("GET, HEAD, OPTIONS"));
            } else {
                send_response(sender, api_error(status, code, "API endpoint not found"));
            }
            true
        } else {
            false
        }
    }

    pub fn handle_post_pending(
        &self,
        headers: &HttpServerHeaders,
        body: HttpServerPendingBody,
        sender: &HttpServerResponseSender,
    ) -> bool {
        if headers.path == "/api/along" {
            if !has_json_content_type(headers) {
                body.reject(api_error(415, "unsupported_media_type", "Content-Type must be application/json"));
                return true;
            }
            let workers = self.workers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            let Some(workers) = workers else {
                body.reject(api_error(503, "unavailable", "navigation services are not ready"));
                return true;
            };
            let Some(permit) = workers.along_admission.try_acquire() else {
                body.reject(api_error(429, "busy", "query queue is full"));
                return true;
            };
            let along = workers.along;
            let response = sender.clone();
            let _ = std::thread::Builder::new()
                .name("web-along-upload".into())
                .spawn(move || {
                    let Ok(bytes) = body.receive() else { return };
                    let job = QueryJob::Along(bytes, permit, response);
                    match along.send(job) {
                        Ok(()) => {}
                        Err(error) => {
                            reject_query_job(
                                error.0,
                                api_error(503, "unavailable", "query workers are unavailable"),
                            );
                        }
                    }
                });
            true
        } else if headers.path.starts_with("/api/") {
            let response = if matches!(headers.path.as_str(), "/api/search" | "/api/route" | "/api/healthz" | "/api/radar/manifest" | "/api/radar/frame" | "/api/wind/current" | "/api/weather/now") {
                api_method_not_allowed("GET, HEAD, OPTIONS")
            } else {
                api_error(404, "not_found", "API endpoint not found")
            };
            body.reject(response);
            true
        } else {
            false
        }
    }

    fn enqueue_query(&self, job: QueryJob) {
        let workers = self.workers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let Some(workers) = workers else {
            reject_query_job(job, api_error(503, "unavailable", "navigation services are not ready"));
            return;
        };
        match workers.search.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(job)) => {
                reject_query_job(job, api_error(429, "busy", "query queue is full"));
            }
            Err(TrySendError::Disconnected(job)) => {
                reject_query_job(job, api_error(503, "unavailable", "query workers are unavailable"));
            }
        }
    }

    fn enqueue_along(&self, body: Vec<u8>, sender: HttpServerResponseSender) {
        let workers = self.workers.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let Some(workers) = workers else {
            send_response(&sender, api_error(503, "unavailable", "navigation services are not ready"));
            return;
        };
        let Some(permit) = workers.along_admission.try_acquire() else {
            send_response(&sender, api_error(429, "busy", "query queue is full"));
            return;
        };
        let job = QueryJob::Along(body, permit, sender);
        match workers.along.send(job) {
            Ok(()) => {}
            Err(error) => {
                reject_query_job(
                    error.0,
                    api_error(503, "unavailable", "query workers are unavailable"),
                );
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

fn spawn_query_workers(
    registry: Weak<ServiceRegistry>,
    backend: Arc<dyn NavBackend>,
    receiver: Receiver<QueryJob>,
    count: usize,
    class: &'static str,
) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..count {
        let registry = registry.clone();
        let backend = backend.clone();
        let receiver = receiver.clone();
        std::thread::Builder::new()
            .name(format!("web-{class}-{index}"))
            .spawn(move || loop {
                let job = receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
                let Ok(job) = job else { break };
                let sender = match &job {
                    QueryJob::Search(_, sender) | QueryJob::Along(_, _, sender) => sender.clone(),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                    QueryJob::Search(request, _) => {
                        let query = request.query.clone();
                        backend
                            .search(request)
                            .map(|results| search_response_for(&query, results))
                            .unwrap_or_else(ApiFailure::response)
                    }
                    QueryJob::Along(body, _permit, _) => parse_along(&body)
                        .and_then(|request| backend.along(request))
                        .map(along_response)
                        .unwrap_or_else(ApiFailure::response),
                }));
                match result {
                    Ok(response) => send_response(&sender, response),
                    Err(_) => {
                        if let Some(registry) = registry.upgrade() {
                            registry.mark_degraded(class);
                        }
                        send_response(&sender, ApiFailure::internal("query worker panic").response());
                    }
                }
            })
            .expect("start query worker");
    }
}

fn reject_query_job(job: QueryJob, response: HttpServerResponse) {
    match job {
        QueryJob::Search(_, sender) | QueryJob::Along(_, _, sender) => {
            send_response(&sender, response)
        }
    }
}

fn has_json_content_type(headers: &HttpServerHeaders) -> bool {
    let mut values = headers
        .lines
        .iter()
        .skip(1)
        .filter_map(|line| makepad_network::utils::split_header_line(line, "Content-Type"));
    let valid = values.next().is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
    });
    valid && values.next().is_none()
}

fn spawn_route_workers(
    registry: Weak<ServiceRegistry>,
    backend: Arc<dyn NavBackend>,
    receiver: Receiver<RouteJob>,
    count: usize,
) {
    let receiver = Arc::new(Mutex::new(receiver));
    for index in 0..count {
        let registry = registry.clone();
        let backend = backend.clone();
        let receiver = receiver.clone();
        std::thread::Builder::new()
            .name(format!("web-route-{index}"))
            .spawn(move || loop {
                let job = receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
                let Ok(RouteJob(request, sender)) = job else { break };
                let response_sender = sender.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mode_name = request.mode_name;
                    backend
                        .route(request)
                        .and_then(|result| route_response(result, mode_name))
                        .unwrap_or_else(ApiFailure::response)
                }));
                match result {
                    Ok(response) => send_response(&response_sender, response),
                    Err(_) => {
                        if let Some(registry) = registry.upgrade() {
                            registry.mark_degraded("route");
                        }
                        send_response(
                            &response_sender,
                            ApiFailure::internal("route worker panic").response(),
                        );
                    }
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
            Some(ReopenableLayerDb {
                db: Mutex::new(Some(LayerDb::open(&path)?)),
                path,
            })
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
        return Err(ApiFailure { status: 413, code: "too_large", message: "request body exceeds 2 MiB".into() });
    }
    let mut request = AlongJsonParser::new(body).parse()?;
    if request.polyline.len() != request.cum_dist_m.len() {
        return Err(bad_request("cum_dist_m must match polyline length"));
    }
    request.cum_dist_m = cumulative_distances(&request.polyline)
        .filter(|distances| distances.last().is_some_and(|total| *total > 0.0 && *total <= 20_000_000.0))
        .ok_or_else(|| bad_request("polyline geometry is outside the supported range"))?;
    let max_detour_min = request.max_detour_min;
    if !max_detour_min.is_finite() || !(1.0..=60.0).contains(&max_detour_min) {
        return Err(bad_request("max_detour_min must be in 1..=60"));
    }
    let min_kw = request.min_kw;
    if !min_kw.is_finite() || !(0.0..=5_000.0).contains(&min_kw) {
        return Err(bad_request("min_kw must be in 0..=5000"));
    }
    let limit = request.limit as f64;
    if !limit.is_finite() || limit.fract() != 0.0 || !(1.0..=30.0).contains(&limit) {
        return Err(bad_request("limit must be an integer in 1..=30"));
    }
    request.limit = limit as usize;
    Ok(request)
}

struct AlongJsonParser<'a> {
    input: &'a [u8],
    pos: usize,
    depth: usize,
    allocated: usize,
}

impl<'a> AlongJsonParser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0, depth: 0, allocated: 0 }
    }

    fn parse(&mut self) -> Result<AlongRequest, ApiFailure> {
        std::str::from_utf8(self.input).map_err(|_| bad_request("body must be UTF-8 JSON"))?;
        self.open(b'{')?;
        let mut polyline = None;
        let mut cum_dist_m = None;
        let mut kinds = None;
        let mut max_detour_min = None;
        let mut min_kw = None;
        let mut limit = None;
        if !self.consume(b'}') {
            loop {
                let key = self.string(32)?;
                self.expect(b':')?;
                match key.as_str() {
                    "polyline" if polyline.is_none() => polyline = Some(self.polyline()?),
                    "cum_dist_m" if cum_dist_m.is_none() => cum_dist_m = Some(self.numbers()?),
                    "kinds" if kinds.is_none() => kinds = Some(self.kinds()?),
                    "max_detour_min" if max_detour_min.is_none() => max_detour_min = Some(self.number()?),
                    "min_kw" if min_kw.is_none() => min_kw = Some(self.number()?),
                    "limit" if limit.is_none() => {
                        let value = self.number()?;
                        if !value.is_finite() || value.fract() != 0.0 || value < 0.0 || value > usize::MAX as f64 {
                            return Err(bad_request("limit must be an integer"));
                        }
                        limit = Some(value as usize);
                    }
                    "polyline" | "cum_dist_m" | "kinds" | "max_detour_min" | "min_kw" | "limit" => {
                        return Err(bad_request("duplicate JSON field"));
                    }
                    _ => return Err(bad_request("unknown JSON field")),
                }
                if self.consume(b'}') {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.close();
        self.ws();
        if self.pos != self.input.len() {
            return Err(bad_request("trailing data after JSON object"));
        }
        Ok(AlongRequest {
            polyline: polyline.ok_or_else(|| bad_request("missing or invalid polyline"))?,
            cum_dist_m: cum_dist_m.ok_or_else(|| bad_request("missing or invalid cum_dist_m"))?,
            kinds: kinds.ok_or_else(|| bad_request("missing or invalid kinds"))?,
            max_detour_min: max_detour_min.unwrap_or(10.0),
            min_kw: min_kw.unwrap_or(0.0),
            limit: limit.unwrap_or(12),
        })
    }

    fn polyline(&mut self) -> Result<Vec<LonLat>, ApiFailure> {
        self.open(b'[')?;
        let mut points = Vec::new();
        if !self.consume(b']') {
            loop {
                if points.len() >= MAX_ROUTE_POINTS {
                    return Err(bad_request("polyline must contain 2..=20000 points"));
                }
                self.open(b'[')?;
                let lon = self.number()?;
                self.expect(b',')?;
                let lat = self.number()?;
                self.expect(b']')?;
                self.close();
                self.charge(std::mem::size_of::<LonLat>() * 2)?;
                points.push(validate_coordinate(lon, lat)?);
                if self.consume(b']') {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.close();
        if !(2..=MAX_ROUTE_POINTS).contains(&points.len()) {
            return Err(bad_request("polyline must contain 2..=20000 points"));
        }
        Ok(points)
    }

    fn numbers(&mut self) -> Result<Vec<f64>, ApiFailure> {
        self.open(b'[')?;
        let mut numbers = Vec::new();
        if !self.consume(b']') {
            loop {
                if numbers.len() >= MAX_ROUTE_POINTS {
                    return Err(bad_request("cum_dist_m has too many values"));
                }
                let value = self.number()?;
                self.charge(std::mem::size_of::<f64>() * 2)?;
                numbers.push(value);
                if self.consume(b']') {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.close();
        Ok(numbers)
    }

    fn kinds(&mut self) -> Result<Vec<String>, ApiFailure> {
        self.open(b'[')?;
        let mut kinds = Vec::new();
        if !self.consume(b']') {
            loop {
                if kinds.len() >= 4 {
                    return Err(bad_request("kinds must contain 1..=4 values"));
                }
                let value = self.string(32)?;
                let kind = value.trim();
                if kind.is_empty() || kind.len() > 32 {
                    return Err(bad_request("each kind must be 1..32 bytes"));
                }
                self.charge(std::mem::size_of::<String>() * 2 + kind.len())?;
                kinds.push(kind.to_string());
                if self.consume(b']') {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.close();
        if kinds.is_empty() {
            return Err(bad_request("kinds must contain 1..=4 values"));
        }
        Ok(kinds)
    }

    fn number(&mut self) -> Result<f64, ApiFailure> {
        self.ws();
        let start = self.pos;
        self.take_if(b'-');
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    return Err(bad_request("invalid JSON number"));
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) { self.pos += 1; }
            }
            _ => return Err(bad_request("expected a JSON number")),
        }
        if self.take_if(b'.') {
            let before = self.pos;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) { self.pos += 1; }
            if self.pos == before { return Err(bad_request("invalid JSON number")); }
        }
        if self.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
            self.pos += 1;
            if self.peek().is_some_and(|byte| matches!(byte, b'+' | b'-')) { self.pos += 1; }
            let before = self.pos;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) { self.pos += 1; }
            if self.pos == before { return Err(bad_request("invalid JSON number")); }
        }
        std::str::from_utf8(&self.input[start..self.pos])
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .ok_or_else(|| bad_request("invalid or non-finite JSON number"))
    }

    fn string(&mut self, max_bytes: usize) -> Result<String, ApiFailure> {
        self.ws();
        if self.peek() != Some(b'\"') { return Err(bad_request("expected a JSON string")); }
        self.pos += 1;
        let mut output = String::new();
        loop {
            let byte = self.peek().ok_or_else(|| bad_request("unterminated JSON string"))?;
            self.pos += 1;
            match byte {
                b'\"' => break,
                b'\\' => {
                    let escape = self.peek().ok_or_else(|| bad_request("invalid JSON escape"))?;
                    self.pos += 1;
                    match escape {
                        b'\"' => output.push('\"'), b'\\' => output.push('\\'), b'/' => output.push('/'),
                        b'b' => output.push('\u{8}'), b'f' => output.push('\u{c}'), b'n' => output.push('\n'),
                        b'r' => output.push('\r'), b't' => output.push('\t'),
                        b'u' => output.push(self.unicode_escape()?),
                        _ => return Err(bad_request("invalid JSON escape")),
                    }
                }
                0..=0x1f => return Err(bad_request("control byte in JSON string")),
                0x20..=0x7f => output.push(byte as char),
                _ => {
                    let start = self.pos - 1;
                    let text = std::str::from_utf8(&self.input[start..])
                        .map_err(|_| bad_request("invalid UTF-8 JSON string"))?;
                    let ch = text.chars().next().ok_or_else(|| bad_request("invalid UTF-8 JSON string"))?;
                    self.pos = start + ch.len_utf8();
                    output.push(ch);
                }
            }
            if output.len() > max_bytes { return Err(bad_request("JSON string is too long")); }
        }
        self.charge(output.len())?;
        Ok(output)
    }

    fn unicode_escape(&mut self) -> Result<char, ApiFailure> {
        let unit = self.hex4()?;
        let scalar = if (0xd800..=0xdbff).contains(&unit) {
            if self.input.get(self.pos..self.pos + 2) != Some(&b"\\u"[..]) {
                return Err(bad_request("invalid Unicode surrogate"));
            }
            self.pos += 2;
            let low = self.hex4()?;
            if !(0xdc00..=0xdfff).contains(&low) { return Err(bad_request("invalid Unicode surrogate")); }
            0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&unit) {
            return Err(bad_request("invalid Unicode surrogate"));
        } else {
            u32::from(unit)
        };
        char::from_u32(scalar).ok_or_else(|| bad_request("invalid Unicode escape"))
    }

    fn hex4(&mut self) -> Result<u16, ApiFailure> {
        let mut value = 0u16;
        for _ in 0..4 {
            let byte = self.peek().ok_or_else(|| bad_request("short Unicode escape"))?;
            self.pos += 1;
            let digit = match byte {
                b'0'..=b'9' => byte - b'0', b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10, _ => return Err(bad_request("invalid Unicode escape")),
            };
            value = value * 16 + u16::from(digit);
        }
        Ok(value)
    }

    fn open(&mut self, byte: u8) -> Result<(), ApiFailure> {
        self.expect(byte)?;
        self.depth += 1;
        if self.depth > 4 { return Err(bad_request("JSON nesting is too deep")); }
        Ok(())
    }

    fn close(&mut self) { self.depth = self.depth.saturating_sub(1); }

    fn charge(&mut self, bytes: usize) -> Result<(), ApiFailure> {
        self.allocated = self.allocated.checked_add(bytes).ok_or_else(|| bad_request("JSON allocation limit exceeded"))?;
        if self.allocated > MAX_ALONG_ALLOC { return Err(bad_request("JSON allocation limit exceeded")); }
        Ok(())
    }

    fn expect(&mut self, byte: u8) -> Result<(), ApiFailure> {
        self.ws();
        if self.peek() != Some(byte) { return Err(bad_request("invalid JSON body")); }
        self.pos += 1;
        Ok(())
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.ws();
        if self.peek() == Some(byte) { self.pos += 1; true } else { false }
    }

    fn take_if(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) { self.pos += 1; true } else { false }
    }

    fn peek(&self) -> Option<u8> { self.input.get(self.pos).copied() }
    fn ws(&mut self) {
        while self.peek().is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t')) { self.pos += 1; }
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

fn api_options(allow: &str) -> HttpServerResponse {
    response(204, None, "private, no-store", &format!("Allow: {allow}\r\n"), Vec::new())
}

fn api_method_not_allowed(allow: &str) -> HttpServerResponse {
    let body = b"{\"error\":{\"code\":\"bad_request\",\"message\":\"method not allowed\"}}".to_vec();
    response(
        405,
        Some("application/json; charset=utf-8"),
        PRIVATE_NO_STORE,
        &format!("Allow: {allow}\r\n"),
        body,
    )
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

fn route_response(result: RouteResult, mode_name: &str) -> Result<HttpServerResponse, ApiFailure> {
    let route = result.route;
    if route.points.len() > MAX_ROUTE_POINTS
        || route.cum_dist_m.len() != route.points.len()
        || route.maneuvers.len() > MAX_ROUTE_MANEUVERS
        || route
            .maneuvers
            .iter()
            .any(|maneuver| maneuver.name.len() > 512 || maneuver.text().len() > 1_024)
    {
        return Err(ApiFailure::internal("route output exceeds resource limits"));
    }
    let mut json = format!(
        "{{\"graph\":{},\"mode\":{},\"length_m\":{},\"duration_s\":{},\"points\":[",
        json_string(&result.graph), json_string(mode_name), json_number(route.length_m), json_number(route.duration_s)
    );
    for (index, point) in route.points.iter().enumerate() {
        if index > 0 { json.push(','); }
        json.push_str(&format!("[{},{}]", json_number(point.lon), json_number(point.lat)));
        ensure_route_size(&json)?;
    }
    json.push_str("],\"cum_dist_m\":[");
    for (index, distance) in route.cum_dist_m.iter().enumerate() {
        if index > 0 { json.push(','); }
        json.push_str(&json_number(*distance));
        ensure_route_size(&json)?;
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
        ensure_route_size(&json)?;
    }
    json.push_str("]}");
    ensure_route_size(&json)?;
    Ok(json_response(200, PRIVATE_NO_STORE, json))
}

fn ensure_route_size(json: &str) -> Result<(), ApiFailure> {
    if json.len() > MAX_ROUTE_RESPONSE_BYTES {
        Err(ApiFailure::internal("route response exceeds encoded byte limit"))
    } else {
        Ok(())
    }
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

pub fn sample_along(polyline: &[LonLat]) -> Vec<(LonLat, f64)> {
    let total = cumulative_distances(polyline)
        .and_then(|distances| distances.last().copied())
        .unwrap_or(0.0);
    let spacing = (total / 48.0).max(3_000.0);
    sample_polyline(polyline, spacing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_geodata::{mvt::AttrVal, sidecar::SidecarBuilder, wkb::Geometry};
    use makepad_map_nav::{
        graph::GraphBuilder,
        search::{Category, SearchIndexBuilder},
    };
    use makepad_mbtile_reader::MbtilesWriter;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        assert_eq!(request.cum_dist_m[0], 0.0);
        assert!(parse_along(br#"{"polyline":[[4.9,52.3]],"cum_dist_m":[0],"kinds":["museum"]}"#).is_err());
        assert!(parse_along(&vec![b' '; MAX_ALONG_BODY + 1]).is_err());
        let untrusted = parse_along(br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[1,19000000],"kinds":["museum"]}"#).unwrap();
        assert_eq!(untrusted.cum_dist_m[0], 0.0);
        assert!(untrusted.cum_dist_m[1] < 20_000.0);
        assert!(parse_along(br#"{"polyline":[[[[[0]]]]],"cum_dist_m":[0,1],"kinds":["museum"]}"#).is_err());
        assert!(parse_along(br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[0,10000],"kinds":["museum"]} trailing"#).is_err());
    }

    #[test]
    fn corridor_sampling_is_bounded() {
        let points = [LonLat::new(0.0, 0.0), LonLat::new(1.0, 1.0)];
        let samples = sample_along(&points);
        assert!(samples.len() <= 49);
        assert_eq!(samples.first().unwrap().1, 0.0);
        let total = haversine_m(points[0], points[1]);
        assert!((samples[1].1 - total / 48.0).abs() < 1e-6);
    }

    #[test]
    fn ten_km_sampling_matches_trip_policy() {
        let points = [LonLat::new(0.0, 0.0), LonLat::new(0.09, 0.0)];
        let samples = sample_along(&points);
        assert_eq!(samples.iter().map(|sample| sample.1).collect::<Vec<_>>(), vec![0.0, 3_000.0, 6_000.0, 9_000.0]);
    }

    #[test]
    fn caller_cumulative_values_cannot_move_samples_between_segments() {
        let request = parse_along(
            br#"{"polyline":[[0,0],[0.045,0],[0.09,0]],"cum_dist_m":[0,100,10000],"kinds":["museum"]}"#,
        )
        .unwrap();
        let samples = sample_along(&request.polyline);
        assert_eq!(samples[1].1, 3_000.0);
        assert!(samples[1].0.lon < request.polyline[1].lon);
    }

    #[test]
    fn route_response_enforces_shared_point_limit() {
        let points = vec![LonLat::new(4.0, 52.0); MAX_ROUTE_POINTS + 1];
        let route = Route {
            mode: TravelMode::Car,
            cum_dist_m: vec![0.0; points.len()],
            points,
            length_m: 0.0,
            duration_s: 0.0,
            maneuvers: Vec::new(),
        };
        assert!(route_response(RouteResult { graph: "fixture".into(), route }, "car").is_err());
    }

    #[test]
    fn charger_kind_falls_back_to_normal_search_without_database() {
        let mut search = SearchIndexBuilder::new();
        search.add(
            "Fast Charger",
            "Fixture",
            LonLat::new(4.9, 52.3),
            Category::Other,
            255,
        );
        let mut graph = GraphBuilder::new();
        graph.add_node(1, 4.9, 52.3);
        graph.add_node(2, 5.0, 52.2);
        let mut tags = std::collections::HashMap::new();
        tags.insert("highway".into(), "residential".into());
        graph.add_way(1, vec![1, 2], tags);
        let backend = ProductionBackend {
            search: SearchService::new(Arc::new(search.build()), None, None),
            regional_graph: Arc::new(graph.build()),
            regional_name: "fixture".into(),
            major_graph: None,
            chargers: None,
        };
        let results = backend.along(AlongRequest {
            polyline: vec![LonLat::new(4.9, 52.3), LonLat::new(5.0, 52.2)],
            cum_dist_m: vec![0.0, 13_000.0],
            kinds: vec!["charger".into()],
            max_detour_min: 10.0,
            min_kw: 0.0,
            limit: 12,
        }).unwrap();
        assert!(results.iter().any(|result| result.name == "Fast Charger"));
    }

    #[test]
    fn poisoned_charger_database_is_reopened() {
        let path = std::env::temp_dir().join(format!(
            "makepad-poisoned-chargers-{}-{}.mbtiles",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut writer = MbtilesWriter::create(&path).unwrap();
        let mut sidecar = SidecarBuilder::new();
        sidecar.add(
            "chargers",
            &Geometry::Point(4.9, 52.3),
            &[("max_kw".into(), AttrVal::Int(150))],
            false,
        );
        sidecar.write(&mut writer).unwrap();
        writer.finish().unwrap();

        let database = Arc::new(ReopenableLayerDb {
            path: path.clone(),
            db: Mutex::new(Some(LayerDb::open(&path).unwrap())),
        });
        let poison = database.clone();
        assert!(std::thread::spawn(move || {
            let _guard = poison.db.lock().unwrap();
            panic!("poison charger mutex");
        })
        .join()
        .is_err());
        let mut budget = 8;
        let hits = database
            .query_radius(LonLat::new(4.9, 52.3), 1_000.0, 8, &mut budget)
            .unwrap();
        assert_eq!(hits.len(), 1);
        fs::remove_file(path).unwrap();
    }

    struct DropBackend {
        dropped: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }

    impl Drop for DropBackend {
        fn drop(&mut self) {
            let (lock, changed) = &*self.dropped;
            *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            changed.notify_all();
        }
    }

    impl NavBackend for DropBackend {
        fn search(&self, _request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure> {
            unreachable!()
        }

        fn route(&self, _request: RouteRequest) -> Result<RouteResult, ApiFailure> {
            unreachable!()
        }

        fn along(&self, _request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure> {
            unreachable!()
        }
    }

    #[test]
    fn workers_exit_when_registry_closes_input_channels() {
        let dropped = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        {
            let registry = ServiceRegistry::new(false);
            registry.install(
                Arc::new(DropBackend { dropped: dropped.clone() }),
                1,
                1,
                1,
                false,
            );
        }
        let (lock, changed) = &*dropped;
        let guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (guard, _) = changed
            .wait_timeout_while(guard, std::time::Duration::from_secs(2), |done| !*done)
            .unwrap();
        assert!(*guard, "worker threads retained their backend after input closure");
    }

    #[test]
    fn production_route_falls_back_whole_pair_to_major_graph() {
        let mut regional = GraphBuilder::new();
        for (id, lon) in [(1, 4.0), (2, 4.01), (3, 5.0), (4, 5.01)] {
            regional.add_node(id, lon, 52.0);
        }
        let mut tags = std::collections::HashMap::new();
        tags.insert("highway".into(), "residential".into());
        regional.add_way(1, vec![1, 2], tags.clone());
        regional.add_way(2, vec![3, 4], tags.clone());

        let mut major = GraphBuilder::new();
        major.add_node(1, 4.0, 52.0);
        major.add_node(2, 4.5, 52.0);
        major.add_node(3, 5.0, 52.0);
        major.add_way(1, vec![1, 2, 3], tags);

        let backend = ProductionBackend {
            search: SearchService::new(Arc::new(SearchIndexBuilder::new().build()), None, None),
            regional_graph: Arc::new(regional.build()),
            regional_name: "regional".into(),
            major_graph: Some(Arc::new(major.build())),
            chargers: None,
        };
        let result = backend
            .route(RouteRequest {
                from: LonLat::new(4.0, 52.0),
                to: LonLat::new(5.0, 52.0),
                mode: TravelMode::Car,
                mode_name: "car",
            })
            .unwrap();
        assert_eq!(result.graph, "europe-major");
    }
}
