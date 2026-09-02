//! Native data plane: filesystem search indexes, route graphs, charger layer,
//! weather, rain radar, and wind/terrain sources.
//!
//! Heavy loads happen on background threads; results cross to the UI thread
//! via `ToUISender` (same pattern as examples/map). Once loaded, tool
//! executors query these synchronously — search and province-scale routing
//! are tens of milliseconds.

use makepad_geodata::knmi_hdf5::{self, KnmiFrame};
use makepad_geodata::query::LayerDb;
use makepad_geodata::radar::{RadarConfig, RadarSync};
use makepad_map_nav::geo::LonLat;
use makepad_map_nav::graph::{Route, RouteGraph, TravelMode};
use makepad_map_nav::search::{SearchIndex, SearchResult};
use makepad_map_nav::searchdb::SearchDb;
use makepad_map_nav::search_service::merge_search_results;
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// The production nav pair (see `examples/map`). When it is absent the app
/// falls back to whatever test map was baked on this machine — same
/// formats, one city instead of a province.
const NAV_DATA_BASENAME: &str = "noord-holland";
const EUROPE_PLACES_PATH: &str = "europe-places.search";
const EUROPE_SEARCHDB_PATH: &str = "europe.searchdb";
const EUROPE_MAJOR_GRAPH_PATH: &str = "europe-major.graph";
const CHARGERS_MBTILES_PATH: &str = "local/overlays/nl-chargers.mbtiles";
const RADAR_CACHE_DIR: &str = "local/overlays/radar";

pub struct NavData {
    pub nh_search: SearchIndex,
    pub nh_graph: RouteGraph,
    pub searchdb: Option<SearchDb>,
    /// In-RAM Europe settlements, used only when the searchdb is absent.
    pub places: Option<SearchIndex>,
    /// Europe major-roads long-haul graph (971MB) — loaded on the first
    /// route that leaves NH coverage, not at startup.
    pub major_graph: Option<RouteGraph>,
    major_graph_attempted: bool,
    pub chargers: Option<LayerDb>,
    maps_root: PathBuf,
}

pub enum NavLoad {
    Ready { data: Box<NavData>, stats: String },
    Failed { error: String },
}

/// The nav artifacts to load, production first, test map second. `None`
/// means this machine has neither and the caller should offer to bake one.
pub fn nav_basename(maps_root: &Path) -> Option<String> {
    for basename in [
        maps_root.join(NAV_DATA_BASENAME).to_string_lossy().into_owned(),
        makepad_map_build::testmap::TestMapPaths::in_dir(maps_root, "amsterdam")
            .nav_basename
            .to_string_lossy()
            .into_owned(),
    ] {
        if Path::new(&format!("{basename}.search")).is_file()
            && Path::new(&format!("{basename}.graph")).is_file()
        {
            return Some(basename);
        }
    }
    None
}

pub fn start_nav_load(sender: ToUISender<NavLoad>, basename: String) {
    std::thread::spawn(move || {
        let maps_root = Path::new(&basename)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let t0 = Cx::monotonic_now();
        let nh_search = match std::fs::read(format!("{basename}.search"))
            .map_err(|e| e.to_string())
            .and_then(|d| SearchIndex::deserialize(&d).map_err(|e| format!("{e:?}")))
        {
            Ok(s) => s,
            Err(e) => {
                let _ = sender.send(NavLoad::Failed {
                    error: format!("search index: {e} (run from repo root; see examples/map)"),
                });
                return;
            }
        };
        let nh_graph = match std::fs::read(format!("{basename}.graph"))
            .map_err(|e| e.to_string())
            .and_then(|d| RouteGraph::deserialize(&d).map_err(|e| format!("{e:?}")))
        {
            Ok(g) => g,
            Err(e) => {
                let _ = sender.send(NavLoad::Failed {
                    error: format!("route graph: {e}"),
                });
                return;
            }
        };
        let searchdb = SearchDb::open(&maps_root.join(EUROPE_SEARCHDB_PATH)).ok();
        let places = if searchdb.is_some() {
            None
        } else {
            std::fs::read(maps_root.join(EUROPE_PLACES_PATH))
                .ok()
                .and_then(|d| SearchIndex::deserialize(&d).ok())
        };
        let chargers = LayerDb::open(Path::new(CHARGERS_MBTILES_PATH)).ok();

        let stats = format!(
            "nav ready in {:.1}s: {} docs, {} edges{}{}{}",
            Cx::monotonic_now() - t0,
            nh_search.doc_count(),
            nh_graph.edges.len(),
            if searchdb.is_some() { ", europe searchdb" } else { "" },
            ", major graph on demand",
            if chargers.is_some() { ", chargers" } else { "" },
        );
        let _ = sender.send(NavLoad::Ready {
            data: Box::new(NavData {
                nh_search,
                nh_graph,
                searchdb,
                places,
                major_graph: None,
                major_graph_attempted: false,
                chargers,
                maps_root,
            }),
            stats,
        });
    });
}

impl NavData {
    /// Merged search over the NH detail index + Europe-wide index, same
    /// policy as examples/map: score-sorted, name+2km deduped, truncated.
    pub fn search(&self, text: &str, near: Option<LonLat>, limit: usize) -> Vec<SearchResult> {
        let mut results = self.nh_search.query(text, near, limit);
        if let Some(db) = &self.searchdb {
            if let Ok(more) = db.query(text, near, limit) {
                results.extend(more);
            }
        } else if let Some(places) = &self.places {
            results.extend(places.query(text, near, limit));
        }
        merge_search_results(results, Vec::new(), limit)
    }

    /// Detail graph first; whole-pair fallback to the Europe major-roads
    /// graph when an endpoint is outside NH coverage (no cross-graph
    /// stitching — same policy as examples/map).
    pub fn route_pair(&mut self, from: LonLat, to: LonLat, mode: TravelMode) -> Option<Route> {
        if let Some(route) = self.nh_graph.route(from, to, mode) {
            return Some(route);
        }
        if self.major_graph.is_none() && !self.major_graph_attempted {
            self.major_graph_attempted = true;
            let t0 = Cx::monotonic_now();
            self.major_graph = std::fs::read(self.maps_root.join(EUROPE_MAJOR_GRAPH_PATH))
                .ok()
                .and_then(|d| RouteGraph::deserialize(&d).ok());
            if self.major_graph.is_some() {
                makepad_widgets::log!(
                    "nav: europe-major graph loaded on demand in {:.1}s",
                    Cx::monotonic_now() - t0
                );
            }
        }
        self.major_graph.as_ref()?.route(from, to, mode)
    }
}

// --- Rain radar -------------------------------------------------------------

pub struct RadarData {
    /// Decoded forecast frames of the newest file, minutes_offset 0..=120.
    pub frames: Vec<KnmiFrame>,
    /// Product timestamp digits from the filename (YYYYMMDDHHMM, UTC).
    pub stamp: String,
    /// Mercator-reprojected BGRA animation frames for `set_rain_frames`.
    pub display_frames: Vec<Vec<u32>>,
    pub display_width: usize,
    pub display_height: usize,
    /// Hi-res dual-radar composite for the "now" frame (BGRA, width, height),
    /// built from the raw Herwijnen + Den Helder volumes at 250 m.
    pub now_hires: Option<(Vec<u32>, usize, usize)>,
}

/// Geographic bbox of the reprojected radar display frames.
pub fn radar_display_bbox() -> (f64, f64, f64, f64) {
    use makepad_geodata::radar_raster::{RASTER_EAST, RASTER_NORTH, RASTER_SOUTH, RASTER_WEST};
    (RASTER_WEST, RASTER_SOUTH, RASTER_EAST, RASTER_NORTH)
}

/// Timestamp digits from a KNMI radar filename ("..._YYYYMMDDHHMM.h5").
fn filename_stamp(filename: &str) -> String {
    filename
        .rsplit('_')
        .next()
        .unwrap_or("")
        .trim_end_matches(".h5")
        .to_string()
}

/// Newest timestamp for which BOTH radar volumes are on disk, with paths.
fn newest_volume_pair(
    herwijnen: &RadarSync,
    den_helder: &RadarSync,
) -> Option<(String, std::path::PathBuf, std::path::PathBuf)> {
    let h_state = herwijnen.state();
    let d_state = den_helder.state();
    for h_frame in h_state.frames.iter().rev() {
        let stamp = filename_stamp(&h_frame.filename);
        if let Some(d_frame) = d_state
            .frames
            .iter()
            .rev()
            .find(|d| filename_stamp(&d.filename) == stamp)
        {
            return Some((stamp, h_frame.path.clone(), d_frame.path.clone()));
        }
    }
    None
}

/// Decode both volume files and composite them at 250 m, reprojected to a
/// mercator BGRA image for `set_rain_now_hires`.
fn build_hires_now(
    projection: &makepad_geodata::radar_raster::RadarProjection,
    herwijnen_path: &std::path::Path,
    den_helder_path: &std::path::Path,
) -> Option<(Vec<u32>, usize, usize)> {
    use makepad_geodata::radar_volume::{composite_volumes, RadarVolume};
    let mut volumes = Vec::new();
    for path in [herwijnen_path, den_helder_path] {
        let data = std::fs::read(path).ok()?;
        match RadarVolume::decode(&data) {
            Ok(volume) => volumes.push(volume),
            Err(error) => {
                log!("radar volume decode {}: {error}", path.display());
                return None;
            }
        }
    }
    let frame = composite_volumes(&volumes, 4);
    let rgba = projection.composite_to_rgba(&frame);
    Some((
        makepad_geodata::radar_raster::rgba_to_bgra_texels(&rgba),
        projection.width,
        projection.height,
    ))
}

/// Poll KNMI (blocking curl inside RadarSync — never on the UI thread),
/// decode the newest forecast file when it changes, ship both the raw grid
/// (numeric weather_now sampling) and reprojected display frames to the UI.
/// Also syncs the raw volumes of both radars and composites them into the
/// hi-res "now" image.
pub fn start_radar_worker(sender: ToUISender<RadarData>) {
    std::thread::spawn(move || {
        let sync = RadarSync::new(RadarConfig::new(RADAR_CACHE_DIR));
        let (herwijnen_config, den_helder_config) = RadarConfig::volume_pair(RADAR_CACHE_DIR);
        let volume_syncs = (
            RadarSync::new(herwijnen_config),
            RadarSync::new(den_helder_config),
        );
        let projection = makepad_geodata::radar_raster::RadarProjection::new(1024, 1280);
        // Hi-res projection built lazily: the LUT is big and only needed
        // once volume data actually exists.
        let mut hires_projection: Option<makepad_geodata::radar_raster::RadarProjection> = None;
        let mut last_decoded = String::new();
        let mut last_volume_stamp = String::new();
        // Latest decoded forecast package, re-sent when the hi-res image
        // refreshes on its own 5-min cadence.
        let mut current: Option<RadarData> = None;
        loop {
            let state = match sync.sync() {
                Ok(state) => state,
                Err(_) => sync.state(),
            };
            let mut changed = false;
            if let Some(frame) = state.frames.last() {
                if frame.filename != last_decoded {
                    if let Ok(data) = std::fs::read(&frame.path) {
                        if let Ok(frames) = knmi_hdf5::decode_frames(&data) {
                            last_decoded = frame.filename.clone();
                            let stamp = filename_stamp(&frame.filename);
                            // Reproject all frames in parallel (same as
                            // examples/map — serial is a multi-second stall).
                            let display_frames: Vec<Vec<u32>> = std::thread::scope(|scope| {
                                let handles: Vec<_> = frames
                                    .iter()
                                    .map(|frame| {
                                        let projection = &projection;
                                        scope.spawn(move || {
                                            makepad_geodata::radar_raster::rgba_to_bgra_texels(
                                                &projection.frame_to_rgba(frame),
                                            )
                                        })
                                    })
                                    .collect();
                                handles.into_iter().map(|h| h.join().unwrap()).collect()
                            });
                            let now_hires = current.as_ref().and_then(|c| c.now_hires.clone());
                            current = Some(RadarData {
                                frames,
                                stamp,
                                display_frames,
                                display_width: 1024,
                                display_height: 1280,
                                now_hires,
                            });
                            changed = true;
                        }
                    }
                }
            }
            // Volumes: sync both radars, composite when a new common
            // timestamp appears.
            let _ = volume_syncs.0.sync();
            let _ = volume_syncs.1.sync();
            if let Some((stamp, herwijnen_path, den_helder_path)) =
                newest_volume_pair(&volume_syncs.0, &volume_syncs.1)
            {
                if stamp != last_volume_stamp {
                    let projection = hires_projection.get_or_insert_with(|| {
                        makepad_geodata::radar_raster::RadarProjection::new(2048, 2560)
                    });
                    if let Some(hires) =
                        build_hires_now(projection, &herwijnen_path, &den_helder_path)
                    {
                        last_volume_stamp = stamp;
                        if let Some(current) = &mut current {
                            current.now_hires = Some(hires);
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                if let Some(current) = &current {
                    let _ = sender.send(RadarData {
                        frames: current.frames.clone(),
                        stamp: current.stamp.clone(),
                        display_frames: current.display_frames.clone(),
                        display_width: current.display_width,
                        display_height: current.display_height,
                        now_hires: current.now_hires.clone(),
                    });
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    });
}

// --- RAD_NL25 grid sampling -------------------------------------------------
//
// Polar-stereographic forward projection for the 700x765 1km radar grid.
// Constants match libs/geodata/src/radar_raster.rs (private there, verified
// against the file's geo_product_corners to ~20m).

const A_KM: f64 = 6378.14;
const B_KM: f64 = 6356.75;
const ROW_OFFSET: f64 = 3649.9795;
const GRID_COLS: usize = 700;
const GRID_ROWS: usize = 765;

pub struct RadarGrid {
    e: f64,
    k0m: f64,
}

impl RadarGrid {
    pub fn new() -> Self {
        let e2 = 1.0 - (B_KM * B_KM) / (A_KM * A_KM);
        let e = e2.sqrt();
        let lat_ts = 60.0_f64.to_radians();
        let t_ts = (std::f64::consts::FRAC_PI_4 - lat_ts / 2.0).tan()
            / ((1.0 - e * lat_ts.sin()) / (1.0 + e * lat_ts.sin())).powf(e / 2.0);
        let m_ts = lat_ts.cos() / (1.0 - e2 * lat_ts.sin() * lat_ts.sin()).sqrt();
        RadarGrid { e, k0m: A_KM * m_ts / t_ts }
    }

    /// lon/lat (deg) → radar grid col/row (f64).
    fn forward(&self, lon_deg: f64, lat_deg: f64) -> (f64, f64) {
        let lam = lon_deg.to_radians();
        let phi = lat_deg.to_radians();
        let t = (std::f64::consts::FRAC_PI_4 - phi / 2.0).tan()
            / ((1.0 - self.e * phi.sin()) / (1.0 + self.e * phi.sin())).powf(self.e / 2.0);
        let rho = self.k0m * t;
        let x = rho * lam.sin();
        let y = -rho * lam.cos();
        (x, -y - ROW_OFFSET)
    }

    /// Rain rate in mm/h at lon/lat for one frame. `None` = outside the
    /// radar image. Raw pixel is reflectivity: dBZ = 0.5*PV - 32; rate via
    /// Marshall-Palmer Z = 200 R^1.6.
    pub fn sample_mm_h(&self, frame: &KnmiFrame, lon: f64, lat: f64) -> Option<f64> {
        let (col, row) = self.forward(lon, lat);
        let (c, r) = (col.round() as i64, row.round() as i64);
        if c < 0 || r < 0 || c >= GRID_COLS as i64 || r >= GRID_ROWS as i64 {
            return None;
        }
        if frame.cols != GRID_COLS || frame.rows != GRID_ROWS {
            return None;
        }
        let v = frame.values[r as usize * frame.cols + c as usize];
        if v == 255 {
            return None;
        }
        if v < 1 {
            return Some(0.0);
        }
        let dbz = 0.5 * v as f64 - 32.0;
        Some((10.0_f64.powf(dbz / 10.0) / 200.0).powf(1.0 / 1.6))
    }
}
