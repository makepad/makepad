//! Offline GPS navigator on top of MapView + makepad-map-nav.
//!
//! - Search box (places, streets, addresses, "supermarkt"-style category
//!   queries) over the `region.search` index, queried on a worker thread.
//! - Long-press: first sets your position (blue puck), then sets a
//!   destination and routes to it (drive / bike / walk).
//! - Start begins a simulated drive along the route with live turn-by-turn
//!   banner, ETA bar, follow camera and recenter button.
//!
//! Data: build `local/maps/noord-holland.{search,graph}` first:
//!   cargo run --release -p makepad-map-tiles -- nav-build \
//!     local/maps/noord-holland-latest.osm.pbf local/maps/noord-holland

pub use makepad_widgets;

pub mod dem;
pub mod elev_graph;

use makepad_map_nav::geo::{bearing_deg, bearing_delta_deg, LonLat};
use makepad_map_nav::graph::{Route, RouteGraph, TravelMode};
use makepad_map_nav::nav::{NavSession, NavState};
use makepad_map_nav::search::{SearchIndex, SearchResult};
use makepad_widgets::*;
use std::sync::mpsc;

app_main!(App);

const NAV_DATA_BASENAME: &str = "local/maps/noord-holland";
/// Continent-wide settlement index (cities/towns/villages) merged into
/// search so any European place is a fly-to target.
const EUROPE_PLACES_PATH: &str = "local/maps/europe-places.search";
const EUROPE_SEARCHDB_PATH: &str = "local/maps/europe.searchdb";
/// Simulated drive runs this much faster than real time.
const SIM_SPEED_MULT: f64 = 6.0;
const MAX_RESULTS: usize = 8;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let ResultButton = ButtonFlat{
        width: Fill
        height: Fit
        align: Align{x: 0.0 y: 0.5}
        label_walk: Walk{width: Fill, height: Fit}
        padding: Inset{left: 10, right: 10, top: 6, bottom: 6}
        visible: false
        text: ""
        draw_text +: {
            color: #x223038
            color_hover: #x000000
            color_focus: #x223038
            color_down: #x000000
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    let PanelText = Label{
        width: Fill
        draw_text +: {
            color: #x22303c
            text_style: theme.font_regular{font_size: 9}
        }
    }

    // Light panel checkbox: the desktop theme's label text is white.
    let LayerCheck = CheckBox{
        draw_text +: {
            color: #x223038
            color_hover: #x000000
            color_down: #x000000
            color_active: #x223038
            color_focus: #x223038
        }
    }

    // The app's floating panels are light; the desktop theme's button text
    // is white, so pin dark label colors.
    let AppButton = Button{
        draw_text +: {
            color: #x223038
            color_hover: #x000000
            color_focus: #x223038
            color_down: #x000000
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 840)
                pass.clear_color: vec4(0.08, 0.10, 0.12, 1.0)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay

                        map := MapView{
                            width: Fill
                            height: Fill
                            center_lon: 4.8952
                            center_lat: 52.3702
                            zoom: 13.0
                            min_zoom: 3.0
                            mbtiles_path: "local/maps/europe-shortbread.mbtiles"
                            detail_mbtiles_path: "local/maps/noord-holland-detail.mbtiles"
                            buildings_3d: true
                        }

                        // --- Search panel (top-left) ---
                        View{
                            width: Fill
                            height: Fit
                            search_panel := RoundedView{
                                flow: Down
                                width: 360
                                height: Fit
                                margin: Inset{left: 12, top: 12}
                                padding: 8
                                draw_bg +: {
                                    color: #xfffffff2
                                    border_radius: 9.0
                                    border_size: 1.0
                                    border_color: #x00000022
                                }
                                search_input := TextInput{
                                    width: Fill
                                    empty_text: "Search places, streets, addresses…"
                                }
                                results_view := View{
                                    flow: Down
                                    width: Fill
                                    height: Fit
                                    visible: false
                                    margin: Inset{top: 4}
                                    result_0 := ResultButton{}
                                    result_1 := ResultButton{}
                                    result_2 := ResultButton{}
                                    result_3 := ResultButton{}
                                    result_4 := ResultButton{}
                                    result_5 := ResultButton{}
                                    result_6 := ResultButton{}
                                    result_7 := ResultButton{}
                                }
                                status_label := PanelText{
                                    margin: Inset{top: 6, left: 2}
                                    text: "Loading navigation data…"
                                }
                            }
                        }

                        // --- Turn banner (top-center) ---
                        View{
                            width: Fill
                            height: Fit
                            align: Align{x: 0.5 y: 0.0}
                            banner := RoundedView{
                                flow: Down
                                width: Fit
                                height: Fit
                                visible: false
                                margin: Inset{top: 12}
                                padding: Inset{left: 18, right: 18, top: 10, bottom: 10}
                                draw_bg +: {
                                    color: #x1a7a3cf0
                                    border_radius: 9.0
                                }
                                banner_text := Label{
                                    draw_text +: {
                                        color: #xffffff
                                        text_style: theme.font_regular{font_size: 13}
                                    }
                                    text: ""
                                }
                                banner_dist := Label{
                                    margin: Inset{top: 2}
                                    draw_text +: {
                                        color: #xd8f2df
                                        text_style: theme.font_regular{font_size: 10}
                                    }
                                    text: ""
                                }
                            }
                        }

                        // --- Route elevation profile (self-pinned above the route bar) ---
                        elevation_graph := ElevationGraph{}

                        // --- Route / nav bar (bottom-center) ---
                        View{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 1.0}
                            route_bar := RoundedView{
                                flow: Right
                                width: Fit
                                height: Fit
                                visible: false
                                align: Align{x: 0.0 y: 0.5}
                                margin: Inset{bottom: 18}
                                padding: Inset{left: 14, right: 10, top: 8, bottom: 8}
                                draw_bg +: {
                                    color: #xfffffff2
                                    border_radius: 9.0
                                    border_size: 1.0
                                    border_color: #x00000022
                                }
                                route_label := Label{
                                    margin: Inset{right: 10, top: 3}
                                    draw_text +: {
                                        color: #x22303c
                                        text_style: theme.font_regular{font_size: 11}
                                    }
                                    text: ""
                                }
                                mode_dropdown := DropDown{
                                    margin: Inset{right: 6}
                                    labels: ["Drive" "Bike" "Walk"]
                                    draw_text +: {
                                        color: #x223038
                                        color_hover: #x000000
                                        color_focus: #x223038
                                        color_down: #x000000
                                    }
                                }
                                go_button := AppButton{
                                    margin: Inset{right: 4}
                                    text: "Start"
                                }
                                end_button := AppButton{
                                    text: "End"
                                }
                            }
                        }

                        // --- Layers panel (above the bottom-right buttons) ---
                        View{
                            width: Fill
                            height: Fill
                            align: Align{x: 1.0 y: 1.0}
                            layers_panel := RoundedView{
                                visible: false
                                flow: Down
                                width: Fit
                                height: Fit
                                margin: Inset{right: 16, bottom: 56}
                                padding: Inset{left: 12, right: 14, top: 10, bottom: 10}
                                spacing: 4
                                draw_bg +: {
                                    color: #xfffffff2
                                    border_radius: 9.0
                                    border_size: 1.0
                                    border_color: #x00000022
                                }
                                layer_chargers := LayerCheck{text: "EV fast chargers"}
                                layer_chargers_slow := LayerCheck{text: "EV slow chargers"}
                                layer_transit := LayerCheck{text: "Transit"}
                                layer_nature := LayerCheck{text: "Nature areas"}
                                layer_districts := LayerCheck{text: "Districts"}
                                layer_bag := LayerCheck{text: "Building age"}
                                layer_population := LayerCheck{text: "Population"}
                                PanelText{
                                    margin: Inset{top: 6}
                                    text: "Terrain · Noise · Flood · Rain: soon"
                                }
                            }
                        }

                        // --- Zoom + recenter (bottom-right) ---
                        View{
                            width: Fill
                            height: Fill
                            flow: Right
                            align: Align{x: 1.0 y: 1.0}
                            recenter_button := AppButton{
                                visible: false
                                margin: Inset{right: 6, bottom: 18}
                                text: "Recenter"
                            }
                            layers_button := AppButton{
                                margin: Inset{right: 4, bottom: 18}
                                text: "Layers"
                            }
                            tilt_button := AppButton{
                                margin: Inset{right: 4, bottom: 18}
                                text: "3D"
                            }
                            zoom_in_button := AppButton{
                                margin: Inset{right: 4, bottom: 18}
                                text: " + "
                            }
                            zoom_out_button := AppButton{
                                margin: Inset{right: 16, bottom: 18}
                                text: " - "
                            }
                        }

                        // --- Set position at map center (bottom-left) ---
                        View{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.0 y: 1.0}
                            position_button := AppButton{
                                margin: Inset{left: 16, bottom: 18}
                                text: "Set position here"
                            }
                        }
                    }
                }
            }
        }
    }
}

// --- Worker protocol ---

enum NavRequest {
    Search {
        id: u64,
        query: String,
        near: Option<LonLat>,
    },
    Route {
        id: u64,
        from: LonLat,
        to: LonLat,
        mode: TravelMode,
    },
    Elevation {
        id: u64,
        points: Vec<LonLat>,
    },
}

enum NavResponse {
    Ready {
        docs: usize,
        edges: usize,
    },
    LoadFailed {
        error: String,
    },
    SearchDone {
        id: u64,
        results: Vec<SearchResult>,
    },
    RouteDone {
        id: u64,
        route: Box<Option<Route>>,
    },
    ElevationDone {
        id: u64,
        profile: Box<Option<dem::ElevationProfile>>,
    },
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    nav_rx: ToUIReceiver<NavResponse>,
    #[rust]
    nav_tx: Option<mpsc::Sender<NavRequest>>,
    #[rust]
    next_request_id: u64,
    #[rust]
    active_search_id: u64,
    #[rust]
    active_route_id: u64,
    #[rust]
    data_ready: bool,
    #[rust]
    search_results: Vec<SearchResult>,
    #[rust]
    search_debounce: Timer,
    #[rust]
    pending_query: String,
    #[rust]
    position: Option<LonLat>,
    #[rust]
    heading: Option<f64>,
    #[rust]
    dest: Option<(LonLat, String)>,
    #[rust]
    route: Option<Route>,
    #[rust]
    session: Option<NavSession>,
    #[rust]
    navigating: bool,
    #[rust]
    follow: bool,
    #[rust]
    program_moves: u32,
    #[rust]
    sim_progress_m: f64,
    /// Per-frame tick: the sim advances on NextFrame, not a timer, so the
    /// follow camera moves once per rendered frame instead of at 20 Hz.
    #[rust]
    sim_next_frame: NextFrame,
    #[rust]
    sim_last_tick: Option<std::time::Instant>,
    /// Smoothed heading-up camera rotation during nav.
    #[rust]
    map_rotation: f64,
    /// Eased 2.5D tilt animation: (current, target).
    #[rust]
    tilt_current: f64,
    #[rust]
    tilt_target: f64,
    #[rust]
    tilt_next_frame: NextFrame,
    #[rust]
    sim_started: Option<std::time::Instant>,
    #[rust]
    mode: TravelMode,
    #[rust]
    active_elevation_id: u64,
    #[rust]
    layers_open: bool,
    #[rust]
    layer_states: [bool; 7],
}

impl App {
    fn map(&self, cx: &Cx) -> MapViewRef {
        self.ui.map_view(cx, ids!(map))
    }

    fn request_id(&mut self) -> u64 {
        self.next_request_id += 1;
        self.next_request_id
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn start_worker(&mut self) {
        let (tx, rx) = mpsc::channel::<NavRequest>();
        self.nav_tx = Some(tx);
        let sender = self.nav_rx.sender();
        std::thread::spawn(move || {
            let load = || -> Result<(SearchIndex, RouteGraph), String> {
                let search_path = format!("{}.search", NAV_DATA_BASENAME);
                let graph_path = format!("{}.graph", NAV_DATA_BASENAME);
                let data = std::fs::read(&search_path)
                    .map_err(|err| format!("{}: {}", search_path, err))?;
                let index = SearchIndex::deserialize(&data).map_err(|err| err.to_string())?;
                let data = std::fs::read(&graph_path)
                    .map_err(|err| format!("{}: {}", graph_path, err))?;
                let graph = RouteGraph::deserialize(&data).map_err(|err| err.to_string())?;
                Ok((index, graph))
            };
            let (index, graph) = match load() {
                Ok(loaded) => {
                    let _ = sender.send(NavResponse::Ready {
                        docs: loaded.0.doc_count(),
                        edges: loaded.1.edges.len(),
                    });
                    loaded
                }
                Err(error) => {
                    let _ = sender.send(NavResponse::LoadFailed { error });
                    return;
                }
            };
            // The disk-backed all-of-Europe database supersedes the
            // settlements-only places index when it exists on disk.
            let searchdb = makepad_map_nav::searchdb::SearchDb::open(
                std::path::Path::new(EUROPE_SEARCHDB_PATH),
            )
            .ok();
            let places_index = if searchdb.is_some() {
                None
            } else {
                std::fs::read(EUROPE_PLACES_PATH)
                    .ok()
                    .and_then(|data| SearchIndex::deserialize(&data).ok())
            };
            let dem_cache = std::sync::Arc::new(std::sync::Mutex::new(dem::DemCache::new(
                "local/maps/dem",
            )));
            while let Ok(request) = rx.recv() {
                match request {
                    NavRequest::Search { id, query, near } => {
                        let mut results = index.query(&query, near, MAX_RESULTS);
                        if let Some(db) = &searchdb {
                            if let Ok(more) = db.query(&query, near, MAX_RESULTS) {
                                results.extend(more);
                            }
                        } else if let Some(places) = &places_index {
                            results.extend(places.query(&query, near, MAX_RESULTS));
                        }
                        // Merge the regional and continental hits: best score
                        // first, drop same-name near-duplicates (cities live
                        // in both indexes).
                        results.sort_by(|a, b| {
                            b.score
                                .partial_cmp(&a.score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        let mut merged: Vec<SearchResult> = Vec::new();
                        for result in results {
                            let duplicate = merged.iter().any(|kept| {
                                kept.name.eq_ignore_ascii_case(&result.name)
                                    && makepad_map_nav::geo::haversine_m(kept.pos, result.pos)
                                        < 2_000.0
                            });
                            if !duplicate {
                                merged.push(result);
                            }
                            if merged.len() >= MAX_RESULTS {
                                break;
                            }
                        }
                        let _ = sender.send(NavResponse::SearchDone {
                            id,
                            results: merged,
                        });
                    }
                    NavRequest::Route { id, from, to, mode } => {
                        let route = graph.route(from, to, mode);
                        let _ = sender.send(NavResponse::RouteDone {
                            id,
                            route: Box::new(route),
                        });
                    }
                    NavRequest::Elevation { id, points } => {
                        // Own thread: the first request may download DEM
                        // tiles and must not stall search/route requests.
                        let dem_cache = dem_cache.clone();
                        let sender = sender.clone();
                        std::thread::spawn(move || {
                            let mut cache = dem_cache.lock().unwrap();
                            let profile = dem::route_profile(&mut cache, &points, 200);
                            let _ = sender.send(NavResponse::ElevationDone {
                                id,
                                profile: Box::new(profile),
                            });
                        });
                    }
                }
            }
        });
    }

    // --- Search ---

    fn send_search(&mut self, cx: &mut Cx) {
        // A script hot-reload wipes #[rust] state including the worker
        // channel; bring it back on demand.
        if self.nav_tx.is_none() {
            self.start_worker();
        }
        if !self.data_ready || self.pending_query.trim().is_empty() {
            self.hide_results(cx);
            return;
        }
        let near = self
            .position
            .or_else(|| self.map(cx).center().map(|(lon, lat)| LonLat::new(lon, lat)));
        let id = self.request_id();
        self.active_search_id = id;
        if let Some(tx) = &self.nav_tx {
            let _ = tx.send(NavRequest::Search {
                id,
                query: self.pending_query.clone(),
                near,
            });
        }
    }

    fn result_button(&self, cx: &Cx, index: usize) -> ButtonRef {
        match index {
            0 => self.ui.button(cx, ids!(result_0)),
            1 => self.ui.button(cx, ids!(result_1)),
            2 => self.ui.button(cx, ids!(result_2)),
            3 => self.ui.button(cx, ids!(result_3)),
            4 => self.ui.button(cx, ids!(result_4)),
            5 => self.ui.button(cx, ids!(result_5)),
            6 => self.ui.button(cx, ids!(result_6)),
            _ => self.ui.button(cx, ids!(result_7)),
        }
    }

    fn show_results(&mut self, cx: &mut Cx) {
        for i in 0..MAX_RESULTS {
            let button = self.result_button(cx, i);
            if let Some(result) = self.search_results.get(i) {
                // Two lines: prominent name, then category · address · distance.
                let mut sub = result.category.label().to_string();
                if !result.secondary.is_empty() {
                    sub.push_str(" · ");
                    sub.push_str(&result.secondary);
                }
                if let Some(d) = result.distance_m {
                    sub.push_str(" · ");
                    sub.push_str(&fmt_dist(d));
                }
                button.set_text(cx, &format!("{}\n{}", result.name, sub));
                button.set_visible(cx, true);
            } else {
                button.set_visible(cx, false);
            }
        }
        self.ui
            .view(cx, ids!(results_view))
            .set_visible(cx, !self.search_results.is_empty());
    }

    fn hide_results(&mut self, cx: &mut Cx) {
        self.search_results.clear();
        self.ui.view(cx, ids!(results_view)).set_visible(cx, false);
    }

    fn pick_result(&mut self, cx: &mut Cx, index: usize) {
        let Some(result) = self.search_results.get(index).cloned() else {
            return;
        };
        self.hide_results(cx);
        // A fresh search must start from an empty box, not append.
        self.ui.text_input(cx, ids!(search_input)).set_text(cx, "");
        self.pending_query.clear();
        self.set_destination(cx, result.pos, &result.name, true);
    }

    // --- Routing ---

    fn set_destination(&mut self, cx: &mut Cx, pos: LonLat, name: &str, fly: bool) {
        self.dest = Some((pos, name.to_string()));
        let map = self.map(cx);
        map.set_markers(
            cx,
            vec![MapMarker::new(1, pos.lon, pos.lat, vec4(0.86, 0.24, 0.24, 1.0))],
        );
        if fly {
            self.program_moves += 1;
            map.fly_to(cx, pos.lon, pos.lat, 16.0);
        }
        if self.position.is_some() {
            self.request_route(cx);
        } else {
            self.set_status(
                cx,
                &format!("{} — double-click the map to set your position first", name),
            );
        }
    }

    fn request_route(&mut self, cx: &mut Cx) {
        let (Some(from), Some((to, name))) = (self.position, self.dest.clone()) else {
            return;
        };
        if self.nav_tx.is_none() {
            self.start_worker();
        }
        if !self.data_ready {
            return;
        }
        let id = self.request_id();
        self.active_route_id = id;
        if let Some(tx) = &self.nav_tx {
            let _ = tx.send(NavRequest::Route {
                id,
                from,
                to,
                mode: self.mode,
            });
        }
        self.set_status(cx, &format!("Routing to {}…", name));
    }

    /// Rebuild the overlay path list from the app-tracked layer states.
    fn apply_overlay_selection(&mut self, cx: &mut Cx) {
        const LAYER_PATHS: [&str; 7] = [
            "local/overlays/nl-chargers.mbtiles?fast",
            "local/overlays/nl-transit.mbtiles",
            "local/overlays/nl-nature.mbtiles",
            "local/overlays/nl-wijkbuurt.mbtiles",
            "local/overlays/nl-buildings-age.mbtiles",
            "local/overlays/nl-demographics.mbtiles",
            "local/overlays/nl-chargers.mbtiles?slow",
        ];
        let paths: Vec<&str> = LAYER_PATHS
            .iter()
            .zip(self.layer_states.iter())
            .filter(|(_, on)| **on)
            .map(|(path, _)| *path)
            .collect();
        log!("overlays: {:?}", paths);
        self.map(cx).set_overlay_paths(cx, &paths.join(";"));
    }

    fn apply_route(&mut self, cx: &mut Cx, route: Route) {
        let points: Vec<(f64, f64)> = route.points.iter().map(|p| (p.lon, p.lat)).collect();
        let map = self.map(cx);
        map.set_route(cx, &points);
        let name = self.dest.as_ref().map(|d| d.1.clone()).unwrap_or_default();
        self.ui.label(cx, ids!(route_label)).set_text(
            cx,
            &format!(
                "{} — {} · {}",
                name,
                fmt_dist(route.length_m),
                fmt_dur(route.duration_s)
            ),
        );
        self.ui.view(cx, ids!(route_bar)).set_visible(cx, true);
        self.set_status(
            cx,
            &format!(
                "{} route: {} · {} — press Start to navigate",
                self.mode.label(),
                fmt_dist(route.length_m),
                fmt_dur(route.duration_s)
            ),
        );
        if self.navigating {
            // Reroute mid-drive: restart the session on the new route.
            self.session = Some(NavSession::new(route.clone()));
            self.sim_progress_m = 0.0;
        }
        let id = self.request_id();
        self.active_elevation_id = id;
        if let Some(tx) = &self.nav_tx {
            let _ = tx.send(NavRequest::Elevation {
                id,
                points: route.points.clone(),
            });
        }
        self.route = Some(route);
    }

    // --- Navigation (simulated drive) ---

    fn start_nav(&mut self, cx: &mut Cx) {
        let Some(route) = self.route.clone() else {
            return;
        };
        self.session = Some(NavSession::new(route.clone()));
        self.navigating = true;
        self.follow = true;
        self.sim_progress_m = 0.0;
        self.sim_last_tick = Some(std::time::Instant::now());
        self.sim_started = Some(std::time::Instant::now());
        self.ui.view(cx, ids!(banner)).set_visible(cx, true);
        self.ui.button(cx, ids!(go_button)).set_visible(cx, false);
        if let Some(start) = route.points.first() {
            self.position = Some(*start);
            self.program_moves += 1;
            self.map(cx).fly_to(cx, start.lon, start.lat, 17.0);
        }
        self.sim_next_frame = cx.new_next_frame();
    }

    fn end_nav(&mut self, cx: &mut Cx) {
        self.navigating = false;
        self.session = None;
        self.route = None;
        self.dest = None;
        self.map_rotation = 0.0;
        let map = self.map(cx);
        map.set_rotation(cx, 0.0);
        map.clear_route(cx);
        map.set_markers(cx, Vec::new());
        self.ui.view(cx, ids!(banner)).set_visible(cx, false);
        self.ui.view(cx, ids!(route_bar)).set_visible(cx, false);
        if let Some(mut graph) = self
            .ui
            .widget(cx, ids!(elevation_graph))
            .borrow_mut::<crate::elev_graph::ElevationGraph>()
        {
            graph.set_profile(cx, None);
        }
        self.ui.button(cx, ids!(go_button)).set_visible(cx, true);
        self.ui
            .button(cx, ids!(recenter_button))
            .set_visible(cx, false);
        self.set_status(cx, "Search a place or double-click the map to route");
    }

    fn tick_sim(&mut self, cx: &mut Cx) {
        if !self.navigating {
            return;
        }
        let Some(route) = self.route.clone() else {
            return;
        };
        let now = std::time::Instant::now();
        let dt = self
            .sim_last_tick
            .map(|last| now.duration_since(last).as_secs_f64())
            .unwrap_or(0.05)
            .min(0.5);
        self.sim_last_tick = Some(now);

        // Advance along the route at the route's average speed, sped up.
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
        self.heading = heading;
        self.position = Some(pos);

        let now_s = self
            .sim_started
            .map(|s| now.duration_since(s).as_secs_f64())
            .unwrap_or(0.0);
        let status = self.session.as_mut().map(|s| s.update(pos, now_s));

        let map = self.map(cx);
        map.set_puck(cx, Some(MapPuck::new(pos.lon, pos.lat, heading, 12.0)));
        if self.follow {
            map.set_center(cx, pos.lon, pos.lat);
            // Heading-up: ease the camera onto the travel bearing with a
            // shortest-arc exponential so curves sweep instead of snapping.
            if let Some(target) = heading {
                let delta = bearing_delta_deg(self.map_rotation, target);
                let blend = 1.0 - (-dt * 3.0).exp();
                self.map_rotation = (self.map_rotation + delta * blend).rem_euclid(360.0);
                map.set_rotation(cx, self.map_rotation);
            }
        }

        if let Some(status) = status {
            let index = route.cum_dist_m.partition_point(|&c| c < status.progress_m);
            map.set_route_progress(cx, index);

            match status.state {
                NavState::Arrived => {
                    self.ui
                        .label(cx, ids!(banner_text))
                        .set_text(cx, "You have arrived");
                    self.ui.label(cx, ids!(banner_dist)).set_text(cx, "");
                    self.navigating = false;
                    return;
                }
                _ => {
                    if let Some(idx) = status.next_maneuver {
                        let maneuver = &route.maneuvers[idx];
                        self.ui
                            .label(cx, ids!(banner_text))
                            .set_text(cx, &maneuver.text());
                        self.ui.label(cx, ids!(banner_dist)).set_text(
                            cx,
                            &format!(
                                "in {}   ·   {} · {} left",
                                fmt_dist(status.dist_to_next_m),
                                fmt_dist(status.remaining_m),
                                fmt_dur(status.remaining_s / SIM_SPEED_MULT)
                            ),
                        );
                    }
                    if status.needs_reroute {
                        self.request_route(cx);
                    }
                }
            }
        }
        self.sim_next_frame = cx.new_next_frame();
    }
}

/// Route point at a given distance from the start (linear interpolation).
fn point_at(route: &Route, dist_m: f64) -> LonLat {
    let cum = &route.cum_dist_m;
    let points = &route.points;
    if points.is_empty() {
        return LonLat::default();
    }
    let idx = cum.partition_point(|&c| c < dist_m);
    if idx == 0 {
        return points[0];
    }
    if idx >= points.len() {
        return points[points.len() - 1];
    }
    let seg = cum[idx] - cum[idx - 1];
    let f = if seg > 0.0 {
        (dist_m - cum[idx - 1]) / seg
    } else {
        0.0
    };
    LonLat::new(
        points[idx - 1].lon + (points[idx].lon - points[idx - 1].lon) * f,
        points[idx - 1].lat + (points[idx].lat - points[idx - 1].lat) * f,
    )
}

fn fmt_dist(m: f64) -> String {
    if m < 950.0 {
        format!("{:.0} m", (m / 10.0).round() * 10.0)
    } else {
        format!("{:.1} km", m / 1000.0)
    }
}

fn fmt_dur(s: f64) -> String {
    let minutes = (s / 60.0).round() as i64;
    if minutes < 60 {
        format!("{} min", minutes.max(1))
    } else {
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.start_worker();
        self.set_status(cx, "Loading navigation data…");
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Search box
        if let Some(text) = self.ui.text_input(cx, ids!(search_input)).changed(actions) {
            self.pending_query = text;
            cx.stop_timer(self.search_debounce);
            self.search_debounce = cx.start_timeout(0.18);
        }
        if let Some((text, _)) = self.ui.text_input(cx, ids!(search_input)).returned(actions) {
            self.pending_query = text;
            self.send_search(cx);
        }
        for i in 0..MAX_RESULTS {
            if self.result_button(cx, i).clicked(actions) {
                self.pick_result(cx, i);
            }
        }

        // Map actions
        let map = self.map(cx);
        if let Some((lon, lat)) = map.long_pressed(actions) {
            if self.position.is_none() {
                self.position = Some(LonLat::new(lon, lat));
                map.set_puck(cx, Some(MapPuck::new(lon, lat, None, 15.0)));
                if self.dest.is_some() {
                    self.request_route(cx);
                } else {
                    self.set_status(
                        cx,
                        "Position set — search a place or double-click to route there",
                    );
                }
            } else if !self.navigating {
                self.set_destination(cx, LonLat::new(lon, lat), "Dropped pin", false);
            }
        }
        if let Some((_lon, _lat, info)) = map.pin_tapped(actions) {
            let get = |key: &str| {
                info.iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("")
            };
            let title = if !get("operator").is_empty() {
                get("operator").to_string()
            } else if !get("name").is_empty() {
                get("name").to_string()
            } else {
                "Charger".to_string()
            };
            let mut lines: Vec<String> = Vec::new();
            if !get("max_kw").is_empty() {
                lines.push(format!("{} kW max", get("max_kw")));
            }
            if !get("evses").is_empty() || !get("connectors").is_empty() {
                lines.push(format!(
                    "{} bays · {} connectors",
                    get("evses"),
                    get("connectors")
                ));
            }
            if !get("name").is_empty() && get("name") != title {
                lines.push(get("name").to_string());
            }
            if !get("city").is_empty() {
                lines.push(get("city").to_string());
            }
            self.ui.label(cx, ids!(pin_info_title)).set_text(cx, &title);
            self.ui
                .label(cx, ids!(pin_info_body))
                .set_text(cx, &lines.join("\n"));
            self.ui.view(cx, ids!(pin_info)).set_visible(cx, true);
        }
        if map.tapped(actions).is_some() {
            self.hide_results(cx);
            self.ui.view(cx, ids!(pin_info)).set_visible(cx, false);
        }
        if map.viewport_changed(actions).is_some() {
            if self.program_moves > 0 {
                self.program_moves -= 1;
            } else if self.navigating && self.follow {
                self.follow = false;
                self.ui
                    .button(cx, ids!(recenter_button))
                    .set_visible(cx, true);
            }
        }

        // Route bar
        if let Some(index) = self.ui.drop_down(cx, ids!(mode_dropdown)).selected(actions) {
            self.mode = match index {
                1 => TravelMode::Bike,
                2 => TravelMode::Foot,
                _ => TravelMode::Car,
            };
            if self.dest.is_some() && self.position.is_some() {
                self.request_route(cx);
            }
        }
        if self.ui.button(cx, ids!(position_button)).clicked(actions) {
            if let Some((lon, lat)) = self.map(cx).center() {
                self.position = Some(LonLat::new(lon, lat));
                self.heading = None;
                self.map(cx)
                    .set_puck(cx, Some(MapPuck::new(lon, lat, None, 15.0)));
                if self.dest.is_some() {
                    self.request_route(cx);
                } else {
                    self.set_status(
                        cx,
                        "Position set — search a place or double-click to route there",
                    );
                }
            }
        }
        if self.ui.button(cx, ids!(go_button)).clicked(actions) {
            self.start_nav(cx);
        }
        if self.ui.button(cx, ids!(end_button)).clicked(actions) {
            self.end_nav(cx);
        }
        if self.ui.button(cx, ids!(layers_button)).clicked(actions) {
            self.layers_open = !self.layers_open;
            self.ui
                .view(cx, ids!(layers_panel))
                .set_visible(cx, self.layers_open);
        }
        // Track layer state app-side: reading .active(cx) in the same
        // event pass as changed() races the widget state (same class of
        // bug as the layers panel visibility).
        let mut layers_changed = false;
        if let Some(value) = self.ui.check_box(cx, ids!(layer_chargers)).changed(actions) {
            self.layer_states[0] = value;
            layers_changed = true;
        }
        if let Some(value) = self
            .ui
            .check_box(cx, ids!(layer_chargers_slow))
            .changed(actions)
        {
            self.layer_states[6] = value;
            layers_changed = true;
        }
        if let Some(value) = self.ui.check_box(cx, ids!(layer_transit)).changed(actions) {
            self.layer_states[1] = value;
            layers_changed = true;
        }
        if let Some(value) = self.ui.check_box(cx, ids!(layer_nature)).changed(actions) {
            self.layer_states[2] = value;
            layers_changed = true;
        }
        if let Some(value) = self.ui.check_box(cx, ids!(layer_districts)).changed(actions) {
            self.layer_states[3] = value;
            layers_changed = true;
        }
        if let Some(value) = self.ui.check_box(cx, ids!(layer_bag)).changed(actions) {
            self.layer_states[4] = value;
            layers_changed = true;
        }
        if let Some(value) = self
            .ui
            .check_box(cx, ids!(layer_population))
            .changed(actions)
        {
            self.layer_states[5] = value;
            layers_changed = true;
        }
        if layers_changed {
            self.apply_overlay_selection(cx);
        }
        if self.ui.button(cx, ids!(tilt_button)).clicked(actions) {
            self.tilt_target = if self.tilt_target > 0.0 { 0.0 } else { 42.0 };
            self.tilt_next_frame = cx.new_next_frame();
        }
        if self.ui.button(cx, ids!(zoom_in_button)).clicked(actions) {
            if let Some(zoom) = self.map(cx).map_zoom() {
                self.map(cx).set_map_zoom(cx, zoom + 1.0);
            }
        }
        if self.ui.button(cx, ids!(zoom_out_button)).clicked(actions) {
            if let Some(zoom) = self.map(cx).map_zoom() {
                self.map(cx).set_map_zoom(cx, zoom - 1.0);
            }
        }
        if self.ui.button(cx, ids!(recenter_button)).clicked(actions) {
            self.follow = true;
            self.ui
                .button(cx, ids!(recenter_button))
                .set_visible(cx, false);
            if let Some(pos) = self.position {
                self.map(cx).set_center(cx, pos.lon, pos.lat);
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::elev_graph::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // Worker responses
        while let Ok(response) = self.nav_rx.try_recv() {
            match response {
                NavResponse::Ready { docs, edges } => {
                    self.data_ready = true;
                    self.set_status(
                        cx,
                        &format!(
                            "{} places · {} road edges — search, or double-click to set position",
                            group_thousands(docs),
                            group_thousands(edges)
                        ),
                    );
                }
                NavResponse::LoadFailed { error } => {
                    self.set_status(
                        cx,
                        &format!(
                            "Nav data missing ({}). Run: makepad-map-tiles nav-build \
                             local/maps/noord-holland-latest.osm.pbf {}",
                            error, NAV_DATA_BASENAME
                        ),
                    );
                }
                NavResponse::SearchDone { id, results } => {
                    if id == self.active_search_id {
                        self.search_results = results;
                        self.show_results(cx);
                    }
                }
                NavResponse::RouteDone { id, route } => {
                    if id == self.active_route_id {
                        match *route {
                            Some(route) => self.apply_route(cx, route),
                            None => self.set_status(cx, "No route found for this mode"),
                        }
                    }
                }
                NavResponse::ElevationDone { id, profile } => {
                    if id == self.active_elevation_id && self.route.is_some() {
                        if let Some(mut graph) = self
                            .ui
                            .widget(cx, ids!(elevation_graph))
                            .borrow_mut::<crate::elev_graph::ElevationGraph>()
                        {
                            graph.set_profile(cx, *profile);
                        }
                    }
                }
            }
        }

        if self.search_debounce.is_event(event).is_some() {
            self.send_search(cx);
        }
        if self.sim_next_frame.is_event(event).is_some() {
            self.tick_sim(cx);
        }
        if self.tilt_next_frame.is_event(event).is_some() {
            let delta = self.tilt_target - self.tilt_current;
            if delta.abs() < 0.1 {
                self.tilt_current = self.tilt_target;
            } else {
                self.tilt_current += delta * 0.12;
                self.tilt_next_frame = cx.new_next_frame();
            }
            self.map(cx).set_tilt(cx, self.tilt_current);
        }

        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
