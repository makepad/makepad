use crate::http::{api_error, json_response, json_string, query_pairs, response, send_response};
use makepad_geodata::{
    knmi_hdf5::{self, KnmiFrame},
    png::{self, PngFormat},
    radar::{RadarConfig, RadarDataset, RadarSync, KNMI_ANONYMOUS_KEY},
    radar_raster::{self, RadarProjection, RASTER_EAST, RASTER_NORTH, RASTER_SOUTH, RASTER_WEST},
    wind::{WindField, WindSync},
};
use makepad_network::{http_server::HttpServerResponseSender, HttpServerHeaders};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, RecvTimeoutError, SyncSender},
        Arc, Mutex, RwLock, Weak,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const POLL_SECS: u64 = 60 * 60;
const DISPLAY_WIDTH: usize = 1024;
const DISPLAY_HEIGHT: usize = 1280;
const HIRES_WIDTH: usize = 2048;
const HIRES_HEIGHT: usize = 2560;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveHealth {
    pub status: &'static str,
    pub updated_unix: Option<u64>,
}

impl LiveHealth {
    pub fn json(self) -> String {
        format!(
            "{{\"status\":{},\"updated_unix\":{}}}",
            json_string(self.status),
            self.updated_unix.map(|value| value.to_string()).unwrap_or_else(|| "null".into())
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LiveServiceState {
    pub radar: LiveHealth,
    pub wind: LiveHealth,
    pub weather: LiveHealth,
}

impl Default for LiveServiceState {
    fn default() -> Self {
        let unavailable = LiveHealth { status: "unavailable", updated_unix: None };
        Self { radar: unavailable, wind: unavailable, weather: unavailable }
    }
}

struct RadarPackage {
    stamp: String,
    updated_unix: u64,
    manifest_json: String,
    frames: BTreeMap<i64, Arc<Vec<u8>>>,
    hires_now: Arc<Vec<u8>>,
    weather_frames: Arc<Vec<KnmiFrame>>,
}

struct WindPackage {
    json: String,
}

pub struct LiveServiceRegistry {
    state: RwLock<LiveServiceState>,
    radar: RwLock<Option<Arc<RadarPackage>>>,
    wind: RwLock<Option<Arc<WindPackage>>>,
    worker_stops: Mutex<Vec<SyncSender<()>>>,
}

struct EphemeralCache(PathBuf);

impl Drop for EphemeralCache {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Default for LiveServiceRegistry {
    fn default() -> Self {
        Self {
            state: RwLock::new(LiveServiceState::default()),
            radar: RwLock::new(None),
            wind: RwLock::new(None),
            worker_stops: Mutex::new(Vec::new()),
        }
    }
}

impl LiveServiceRegistry {
    pub fn state(&self) -> LiveServiceState {
        *self.state.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn start(
        self: &Arc<Self>,
        cache_dir: Option<&Path>,
        knmi_key_file: Option<&Path>,
    ) -> Result<(), String> {
        let key = select_knmi_key(knmi_key_file)?;
        if let Some(key_file) = knmi_key_file {
            println!("knmi: key file {}", key_file.display());
        } else {
            println!("knmi: anonymous public key");
        }

        let (cache_dir, ephemeral_cache) = if let Some(cache_dir) = cache_dir {
            (cache_dir.to_path_buf(), None)
        } else {
            println!("live cache: in-memory; restarts will re-poll");
            let cache_dir = std::env::temp_dir().join(format!(
                "makepad-web-server-live-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()
            ));
            (cache_dir.clone(), Some(Arc::new(EphemeralCache(cache_dir))))
        };
        fs::create_dir_all(&cache_dir)
            .map_err(|error| format!("create live cache {}: {error}", cache_dir.display()))?;

        let wind_dir = cache_dir.join("wind");
        self.mark_warming(false);
        let wind_sync = wind_sync(&wind_dir);
        if let Some((stamp, field)) = wind_sync.cached_with_stamp() {
            self.publish_wind(stamp, field);
        }
        if let Err(error) = self.spawn_wind_worker(wind_dir, ephemeral_cache.clone()) {
            self.mark_unavailable(false);
            return Err(error);
        }

        let radar_dir = cache_dir.join("radar");
        self.mark_warming(true);
        let (forecast, composite) = radar_syncs(&radar_dir, &key);
        if let Ok(package) = load_or_build_radar(&radar_dir, &forecast, &composite, false) {
            self.publish_radar(package);
        }
        if let Err(error) = self.spawn_radar_worker(radar_dir, key, ephemeral_cache) {
            self.mark_unavailable(true);
            return Err(error);
        }
        Ok(())
    }

    fn mark_warming(&self, radar: bool) {
        let mut state = self.state.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let warming = LiveHealth { status: "warming", updated_unix: None };
        if radar {
            state.radar = warming;
            state.weather = warming;
        } else {
            state.wind = warming;
        }
    }

    fn mark_unavailable(&self, radar: bool) {
        let mut state = self.state.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let unavailable = LiveHealth { status: "unavailable", updated_unix: None };
        if radar {
            state.radar = unavailable;
            state.weather = unavailable;
        } else {
            state.wind = unavailable;
        }
    }

    fn publish_radar(&self, package: RadarPackage) {
        let updated_unix = package.updated_unix;
        *self.radar.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(package));
        let mut state = self.state.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let ok = LiveHealth { status: "ok", updated_unix: Some(updated_unix) };
        state.radar = ok;
        state.weather = ok;
    }

    fn publish_wind(&self, updated_unix: u64, field: WindField) {
        let package = WindPackage { json: wind_json(updated_unix, &field) };
        *self.wind.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(package));
        self.state.write().unwrap_or_else(|poisoned| poisoned.into_inner()).wind =
            LiveHealth { status: "ok", updated_unix: Some(updated_unix) };
    }

    fn spawn_radar_worker(
        self: &Arc<Self>,
        cache_dir: PathBuf,
        key: String,
        ephemeral_cache: Option<Arc<EphemeralCache>>,
    ) -> Result<(), String> {
        let (stop_sender, stop_receiver) = mpsc::sync_channel(1);
        self.worker_stops.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(stop_sender);
        let registry = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("web-live-radar".into())
            .spawn(move || {
                let _ephemeral_cache = ephemeral_cache;
                let (forecast, composite) = radar_syncs(&cache_dir, &key);
                loop {
                    let result = forecast.sync().and_then(|_| composite.sync()).and_then(|_| {
                        load_or_build_radar(&cache_dir, &forecast, &composite, true)
                    });
                    if let Some(registry) = registry.upgrade() {
                        match result {
                            Ok(package) => registry.publish_radar(package),
                            Err(error) => eprintln!("live radar poll failed: {error}"),
                        }
                    } else {
                        break;
                    }
                    match stop_receiver.recv_timeout(Duration::from_secs(POLL_SECS)) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|error| format!("start live radar worker: {error}"))?;
        Ok(())
    }

    fn spawn_wind_worker(
        self: &Arc<Self>,
        cache_dir: PathBuf,
        ephemeral_cache: Option<Arc<EphemeralCache>>,
    ) -> Result<(), String> {
        let (stop_sender, stop_receiver) = mpsc::sync_channel(1);
        self.worker_stops.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(stop_sender);
        let registry: Weak<Self> = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("web-live-wind".into())
            .spawn(move || {
                let _ephemeral_cache = ephemeral_cache;
                let sync = wind_sync(&cache_dir);
                loop {
                    match sync.sync() {
                        Ok(Some(field)) => {
                            if let Some(registry) = registry.upgrade() {
                                let stamp = sync.cached_with_stamp().map(|value| value.0).unwrap_or_else(now_unix);
                                registry.publish_wind(stamp, field);
                            } else {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => eprintln!("live wind poll failed: {error}"),
                    }
                    match stop_receiver.recv_timeout(Duration::from_secs(POLL_SECS)) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|error| format!("start live wind worker: {error}"))?;
        Ok(())
    }

    pub fn handle_get(&self, headers: &HttpServerHeaders, sender: &HttpServerResponseSender) -> bool {
        if !matches!(
            headers.path.as_str(),
            "/api/radar/manifest" | "/api/radar/frame" | "/api/wind/current" | "/api/weather/now"
        ) {
            return false;
        }
        if headers.verb == "OPTIONS" {
            send_response(sender, live_options());
            return true;
        }
        if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
            send_response(sender, live_method_not_allowed());
            return true;
        }
        let result = match headers.path.as_str() {
            "/api/radar/manifest" => self.radar_manifest_response(),
            "/api/radar/frame" => self.radar_frame_response(headers.search.as_deref()),
            "/api/wind/current" => self.wind_response(),
            "/api/weather/now" => self.weather_response(headers.search.as_deref()),
            _ => unreachable!(),
        };
        send_response(sender, result);
        true
    }

    fn radar_manifest_response(&self) -> makepad_network::http_server::HttpServerResponse {
        let state = self.state();
        if state.radar.status == "unavailable" {
            return api_error(404, "unavailable", "radar service is not configured");
        }
        let Some(package) = self.radar.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() else {
            return warming_response();
        };
        json_response(200, "public, max-age=60", package.manifest_json.clone())
    }

    fn radar_frame_response(&self, query: Option<&str>) -> makepad_network::http_server::HttpServerResponse {
        let state = self.state();
        if state.radar.status == "unavailable" {
            return api_error(404, "unavailable", "radar service is not configured");
        }
        let Some(package) = self.radar.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() else {
            return warming_response();
        };
        let (stamp, minute, quality) = match parse_frame_query(query) {
            Ok(value) => value,
            Err(message) => return api_error(400, "bad_request", message),
        };
        if stamp != package.stamp {
            return warming_response();
        }
        let png = if quality == "hires" {
            if minute != 0 {
                return api_error(400, "bad_request", "hires quality is only available for minute 0");
            }
            package.hires_now.clone()
        } else {
            let Some(frame) = package.frames.get(&minute) else {
                return warming_response();
            };
            frame.clone()
        };
        response(200, Some("image/png"), "public, max-age=3600", "", (*png).clone())
    }

    fn wind_response(&self) -> makepad_network::http_server::HttpServerResponse {
        let state = self.state();
        if state.wind.status == "unavailable" {
            return api_error(404, "unavailable", "wind service is not configured");
        }
        let Some(package) = self.wind.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() else {
            return warming_response();
        };
        json_response(200, "public, max-age=300", package.json.clone())
    }

    fn weather_response(&self, query: Option<&str>) -> makepad_network::http_server::HttpServerResponse {
        let state = self.state();
        if state.weather.status == "unavailable" {
            return api_error(404, "unavailable", "weather service is not configured");
        }
        let Some(package) = self.radar.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone() else {
            return warming_response();
        };
        let (lon, lat) = match parse_weather_query(query) {
            Ok(value) => value,
            Err(message) => return api_error(400, "bad_request", message),
        };
        json_response(200, "public, max-age=60", weather_json(&package, lon, lat))
    }
}

fn select_knmi_key(knmi_key_file: Option<&Path>) -> Result<String, String> {
    let Some(key_file) = knmi_key_file else {
        return Ok(KNMI_ANONYMOUS_KEY.to_string());
    };
    let key = fs::read_to_string(key_file)
        .map_err(|error| format!("read KNMI key file {}: {error}", key_file.display()))?;
    let key = key.trim();
    if key.is_empty() || key.len() > 4096 || key.contains(['\r', '\n']) {
        return Err("KNMI key file must contain one non-empty key".into());
    }
    Ok(key.to_string())
}

fn radar_syncs(cache_dir: &Path, key: &str) -> (RadarSync, RadarSync) {
    let mut forecast = RadarConfig::for_dataset(cache_dir, RadarDataset::Forecast);
    forecast.api_key = Some(key.to_string());
    forecast.min_poll_secs = POLL_SECS;
    forecast.max_frames = 2;
    let mut composite = RadarConfig::for_dataset(cache_dir, RadarDataset::ReflectivityComposite);
    composite.api_key = Some(key.to_string());
    composite.min_poll_secs = POLL_SECS;
    composite.max_frames = 2;
    (RadarSync::new(forecast), RadarSync::new(composite))
}

fn wind_sync(cache_dir: &Path) -> WindSync {
    let mut sync = WindSync::new(cache_dir);
    sync.min_poll_secs = POLL_SECS;
    sync
}

fn load_or_build_radar(
    cache_dir: &Path,
    forecast: &RadarSync,
    composite: &RadarSync,
    allow_render: bool,
) -> Result<RadarPackage, String> {
    let forecast_state = forecast.state();
    let source = forecast_state.frames.last().ok_or("no cached radar forecast")?;
    let stamp = filename_stamp(&source.filename).ok_or("forecast filename has no timestamp")?;
    let bytes = fs::read(&source.path).map_err(|error| format!("read cached forecast: {error}"))?;
    let frames = knmi_hdf5::decode_frames(&bytes)?;
    let served_dir = cache_dir.join("served").join(&stamp);
    let display = frames
        .iter()
        .map(|frame| (i64::from(frame.minutes_offset), served_dir.join(format!("display-{}.png", frame.minutes_offset))))
        .collect::<Vec<_>>();
    let hires_path = served_dir.join("hires-now.png");
    let all_cached = display.iter().all(|(_, path)| valid_png_file(path)) && valid_png_file(&hires_path);
    if !all_cached {
        if !allow_render {
            return Err("rendered radar cache is incomplete".into());
        }
        fs::create_dir_all(&served_dir)
            .map_err(|error| format!("create rendered radar cache: {error}"))?;
        let projection = RadarProjection::new(DISPLAY_WIDTH, DISPLAY_HEIGHT);
        for (frame, (_, path)) in frames.iter().zip(&display) {
            let rgba = projection.frame_to_rgba(frame);
            write_atomic(path, &png::encode(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32, PngFormat::Rgba8, &rgba))?;
        }
        let composite_frame = composite
            .state()
            .frames
            .last()
            .and_then(|source| fs::read(&source.path).ok())
            .and_then(|bytes| knmi_hdf5::decode_frames(&bytes).ok())
            .and_then(|frames| frames.into_iter().next())
            .unwrap_or_else(|| frames[0].clone());
        let projection = RadarProjection::new(HIRES_WIDTH, HIRES_HEIGHT);
        let rgba = projection.frame_to_rgba(&composite_frame);
        write_atomic(&hires_path, &png::encode(HIRES_WIDTH as u32, HIRES_HEIGHT as u32, PngFormat::Rgba8, &rgba))?;
        prune_old_served(cache_dir, &stamp);
    }
    let png_frames = display
        .into_iter()
        .map(|(minute, path)| {
            let bytes = fs::read(path).map_err(|error| format!("read rendered radar frame: {error}"))?;
            Ok((minute, Arc::new(bytes)))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let hires_now = Arc::new(fs::read(hires_path).map_err(|error| format!("read hires radar frame: {error}"))?);
    let minutes = frames.iter().map(|frame| frame.minutes_offset.to_string()).collect::<Vec<_>>().join(",");
    let manifest_json = format!(
        "{{\"stamp\":{},\"bbox\":[{RASTER_WEST},{RASTER_SOUTH},{RASTER_EAST},{RASTER_NORTH}],\"minutes\":[{minutes}],\"display\":{{\"width\":{DISPLAY_WIDTH},\"height\":{DISPLAY_HEIGHT}}},\"hires_now\":{{\"width\":{HIRES_WIDTH},\"height\":{HIRES_HEIGHT}}}}}",
        json_string(&stamp)
    );
    let updated_unix = if source.created_unix != 0 { source.created_unix } else { file_modified_unix(&source.path) };
    Ok(RadarPackage {
        stamp,
        updated_unix,
        manifest_json,
        frames: png_frames,
        hires_now,
        weather_frames: Arc::new(frames),
    })
}

fn filename_stamp(filename: &str) -> Option<String> {
    let stamp = filename.rsplit('_').next()?.strip_suffix(".h5")?;
    (stamp.len() == 12 && stamp.bytes().all(|byte| byte.is_ascii_digit())).then(|| stamp.to_string())
}

fn valid_png_file(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else { return false };
    let mut signature = [0; 8];
    file.read_exact(&mut signature).is_ok() && signature == *b"\x89PNG\r\n\x1a\n"
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let part = path.with_extension("png.part");
    fs::write(&part, bytes).map_err(|error| format!("write cache file: {error}"))?;
    fs::rename(&part, path).map_err(|error| format!("publish cache file: {error}"))
}

fn prune_old_served(cache_dir: &Path, keep: &str) {
    let root = cache_dir.join("served");
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != keep && name.len() == 12 && name.bytes().all(|byte| byte.is_ascii_digit()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn file_modified_unix(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_else(now_unix)
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn wind_json(stamp: u64, field: &WindField) -> String {
    let floats = |values: &[f32]| values.iter().map(|value| value.to_string()).collect::<Vec<_>>().join(",");
    format!(
        "{{\"stamp_unix\":{stamp},\"bbox\":[{},{},{},{}],\"nx\":{},\"ny\":{},\"u\":[{}],\"v\":[{}]}}",
        field.west, field.south, field.east, field.north, field.nx, field.ny,
        floats(&field.u), floats(&field.v)
    )
}

fn weather_json(package: &RadarPackage, lon: f64, lat: f64) -> String {
    let mut samples = String::new();
    for frame in package.weather_frames.iter() {
        let Some(mm_h) = radar_raster::sample_mm_h(frame, lon, lat) else { continue };
        if !samples.is_empty() {
            samples.push(',');
        }
        samples.push_str(&format!(
            "{{\"minute\":{},\"mm_h\":{},\"class\":{}}}",
            frame.minutes_offset,
            mm_h,
            json_string(classify_rain(mm_h))
        ));
    }
    format!(
        "{{\"stamp\":{},\"at\":[{lon},{lat}],\"samples\":[{samples}]}}",
        json_string(&package.stamp)
    )
}

fn classify_rain(mm_h: f64) -> &'static str {
    if mm_h < 0.1 { "dry" }
    else if mm_h < 1.0 { "light" }
    else if mm_h < 4.0 { "moderate" }
    else if mm_h < 10.0 { "heavy" }
    else { "intense" }
}

fn parse_frame_query(query: Option<&str>) -> Result<(String, i64, String), &'static str> {
    let pairs = query_pairs(query).map_err(|_| "invalid query encoding")?;
    let mut stamp = None;
    let mut minute = None;
    let mut quality = None;
    for (key, value) in pairs {
        match key.as_str() {
            "stamp" if stamp.is_none() && value.len() <= 64 => stamp = Some(value),
            "minute" if minute.is_none() => minute = value.parse().ok(),
            "quality" if quality.is_none() => quality = Some(value),
            _ => return Err("expected stamp, minute, and quality exactly once"),
        }
    }
    let (Some(stamp), Some(minute), Some(quality)) = (stamp, minute, quality) else {
        return Err("missing stamp, minute, or quality");
    };
    if !matches!(quality.as_str(), "display" | "hires") || !(0..=1440).contains(&minute) {
        return Err("invalid radar minute or quality");
    }
    Ok((stamp, minute, quality))
}

fn parse_weather_query(query: Option<&str>) -> Result<(f64, f64), &'static str> {
    let pairs = query_pairs(query).map_err(|_| "invalid query encoding")?;
    if pairs.len() != 1 || pairs[0].0 != "at" {
        return Err("expected one at=lon,lat parameter");
    }
    let (lon, lat) = pairs[0].1.split_once(',').ok_or("at must be lon,lat")?;
    let lon: f64 = lon.parse().map_err(|_| "invalid longitude")?;
    let lat: f64 = lat.parse().map_err(|_| "invalid latitude")?;
    if !lon.is_finite() || !lat.is_finite() || !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
        return Err("coordinates are outside valid lon/lat bounds");
    }
    Ok((lon, lat))
}

fn warming_response() -> makepad_network::http_server::HttpServerResponse {
    json_response(503, "private, no-store", "{\"status\":\"warming\"}".into())
}

fn live_options() -> makepad_network::http_server::HttpServerResponse {
    response(204, None, "private, no-store", "Allow: GET, HEAD, OPTIONS\r\n", Vec::new())
}

fn live_method_not_allowed() -> makepad_network::http_server::HttpServerResponse {
    response(
        405,
        Some("application/json; charset=utf-8"),
        "private, no-store",
        "Allow: GET, HEAD, OPTIONS\r\n",
        b"{\"error\":{\"code\":\"bad_request\",\"message\":\"method not allowed\"}}".to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radar_package() -> RadarPackage {
        let frame = KnmiFrame {
            minutes_offset: 0,
            rows: 765,
            cols: 700,
            values: vec![0; 765 * 700],
        };
        RadarPackage {
            stamp: "202609021230".into(),
            updated_unix: 1_788_352_200,
            manifest_json: "{\"stamp\":\"202609021230\",\"bbox\":[0,48.89,10.86,55.98],\"minutes\":[0],\"display\":{\"width\":1024,\"height\":1280},\"hires_now\":{\"width\":2048,\"height\":2560}}".into(),
            frames: BTreeMap::from([(0, Arc::new(b"png".to_vec()))]),
            hires_now: Arc::new(b"hires".to_vec()),
            weather_frames: Arc::new(vec![frame]),
        }
    }

    #[test]
    fn live_wire_shapes_match_client_contracts() {
        let registry = LiveServiceRegistry::default();
        registry.publish_radar(radar_package());
        registry.publish_wind(123, WindField {
            nx: 2,
            ny: 1,
            u: vec![1.0, 2.0],
            v: vec![3.0, 4.0],
            west: 2.0,
            east: 9.0,
            south: 48.0,
            north: 56.0,
        });

        let manifest: serde_json::Value = serde_json::from_slice(&registry.radar_manifest_response().body).unwrap();
        assert_eq!(manifest["display"]["width"], 1024);
        assert_eq!(manifest["hires_now"]["height"], 2560);
        let wind: serde_json::Value = serde_json::from_slice(&registry.wind_response().body).unwrap();
        assert_eq!(wind["nx"], 2);
        assert_eq!(wind["u"].as_array().unwrap().len(), 2);
        let weather: serde_json::Value = serde_json::from_slice(&registry.weather_response(Some("at=4.9,52.3")).body).unwrap();
        assert_eq!(weather["at"], serde_json::json!([4.9, 52.3]));
        assert_eq!(weather["samples"][0]["class"], "dry");
    }

    #[test]
    fn warming_and_frame_queries_are_strict() {
        let registry = LiveServiceRegistry::default();
        registry.mark_warming(true);
        assert_eq!(registry.radar_manifest_response().body, br#"{"status":"warming"}"#);
        registry.publish_radar(radar_package());
        assert_eq!(
            registry.radar_frame_response(Some("stamp=202609021230&minute=0&quality=display")).body,
            b"png"
        );
        assert!(String::from_utf8(registry.radar_frame_response(Some("minute=0")).body).unwrap().contains("bad_request"));
    }

    #[test]
    fn knmi_key_selection_prefers_file_and_falls_back_to_anonymous() {
        assert_eq!(select_knmi_key(None).unwrap(), KNMI_ANONYMOUS_KEY);

        let key_file = PathBuf::from("target").join(format!("knmi-key-test-{}", std::process::id()));
        fs::create_dir_all(key_file.parent().unwrap()).unwrap();
        fs::write(&key_file, "personal-key\n").unwrap();
        assert_eq!(select_knmi_key(Some(&key_file)).unwrap(), "personal-key");
        fs::remove_file(key_file).unwrap();
    }
}
