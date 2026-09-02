use crate::{
    clock,
    nav_api::{ApiOperation, NavApi, NavApiEvent, RadarManifest},
    provisioner::MapProvisioner,
    side_panel::{AlongKind, PanelAction, PanelController},
    trip::{Leg, TripModel},
    AMSTERDAM_CENTER,
};
use makepad_map_nav::{geo::LonLat, graph::Route, nav::NavSession};
use makepad_widgets::*;
use std::collections::BTreeMap;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 800)
                pass.clear_color: #x11171d
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        map := MapView{
                            width: Fill
                            height: Fill
                            center_lon: 4.8952
                            center_lat: 52.3702
                            zoom: 13.0
                            tilt: 35.0
                            min_zoom: 3.0
                            buildings_3d: true
                            use_network: false
                        }
                        RouteSidePanel{}
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
    #[rust]
    provisioner: MapProvisioner,
    #[rust]
    panel: PanelController,
    #[rust]
    api: NavApi,
    #[rust]
    position: Option<LocationUpdateEvent>,
    #[rust]
    pending_destination: Option<makepad_map_nav::search::SearchResult>,
    #[rust]
    trip: TripModel,
    #[rust]
    route: Option<Route>,
    #[rust]
    nav_session: Option<NavSession>,
    #[rust]
    nav_started: Option<f64>,
    #[rust]
    reroute_in_flight: bool,
    #[rust]
    radar_manifest: Option<RadarManifest>,
    #[rust]
    radar_frames: BTreeMap<i64, Vec<u32>>,
    #[rust]
    rain_enabled: bool,
    #[rust]
    wind_enabled: bool,
    #[rust]
    radar_timer: Timer,
    #[rust]
    wind_timer: Timer,
}

impl App {
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        let map = self.ui.map_view(cx, ids!(map));
        self.provisioner.ensure_source(cx, &map);
        self.api = NavApi::new(self.provisioner.api_url());
        cx.start_location_updates();
        self.api
            .weather_now(cx, LonLat::new(AMSTERDAM_CENTER.0, AMSTERDAM_CENTER.1));
    }

    fn current_position(&self, cx: &mut Cx) -> LonLat {
        self.position
            .as_ref()
            .map(|fix| LonLat::new(fix.lon, fix.lat))
            .or_else(|| self.ui.map_view(cx, ids!(map)).center().map(|(lon, lat)| LonLat::new(lon, lat)))
            .unwrap_or_else(|| LonLat::new(AMSTERDAM_CENTER.0, AMSTERDAM_CENTER.1))
    }

    fn map_center(&self, cx: &mut Cx) -> LonLat {
        self.ui
            .map_view(cx, ids!(map))
            .center()
            .map(|(lon, lat)| LonLat::new(lon, lat))
            .unwrap_or_else(|| LonLat::new(AMSTERDAM_CENTER.0, AMSTERDAM_CENTER.1))
    }

    fn handle_panel_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in self.panel.actions(cx, &self.ui, actions) {
            match action {
                PanelAction::Search(query) => {
                    self.panel
                        .set_search_status(cx, &self.ui, "Searching hosted places…");
                    let near = self.map_center(cx);
                    self.api.search(cx, &query, Some(near), 8);
                }
                PanelAction::SelectResult(index) => {
                    if let Some(result) = self.panel.result(index).cloned() {
                        self.ui.map_view(cx, ids!(map)).fly_to(
                            cx,
                            result.pos.lon,
                            result.pos.lat,
                            13.5,
                        );
                    }
                }
                PanelAction::RouteHere(index) => {
                    let Some(destination) = self.panel.result(index).cloned() else {
                        continue;
                    };
                    self.pending_destination = Some(destination.clone());
                    self.reroute_in_flight = true;
                    self.panel
                        .set_search_status(cx, &self.ui, "Routing on makepad.nl…");
                    let from = self.current_position(cx);
                    self.api.route(
                        cx,
                        from,
                        destination.pos,
                        makepad_map_nav::graph::TravelMode::Car,
                    );
                    self.api.weather_now(cx, destination.pos);
                }
                PanelAction::Along(kind) => {
                    let Some(route) = self.route.as_ref() else {
                        self.panel.set_along_status(
                            cx,
                            &self.ui,
                            "Plan a route before searching along it.",
                        );
                        continue;
                    };
                    let kinds: &[&str] = match kind {
                        AlongKind::Chargers => &["charger"],
                        AlongKind::Museums => &["museum"],
                    };
                    self.panel
                        .set_along_status(cx, &self.ui, "Searching along the route…");
                    self.api.along(cx, route, kinds, 10.0, 50.0, 12);
                }
                PanelAction::Rain(on) => self.set_rain(cx, on),
                PanelAction::Wind(on) => self.set_wind(cx, on),
            }
        }
    }

    fn set_rain(&mut self, cx: &mut Cx, on: bool) {
        if self.rain_enabled == on {
            return;
        }
        self.rain_enabled = on;
        if on {
            self.api.radar_manifest(cx);
            self.radar_timer = cx.start_interval(30.0);
            self.panel
                .set_weather(cx, &self.ui, "Rain radar: loading manifest…");
        } else {
            cx.stop_timer(self.radar_timer);
            self.radar_timer = Timer::empty();
            self.radar_frames.clear();
            let bbox = self
                .radar_manifest
                .as_ref()
                .map(|manifest| manifest.bbox)
                .unwrap_or((0.0, 0.0, 0.0, 0.0));
            self.ui
                .map_view(cx, ids!(map))
                .set_rain_frames(cx, Vec::new(), 0, 0, bbox);
            self.ui
                .map_view(cx, ids!(map))
                .set_rain_now_hires(cx, None);
        }
    }

    fn set_wind(&mut self, cx: &mut Cx, on: bool) {
        if self.wind_enabled == on {
            return;
        }
        self.wind_enabled = on;
        if on {
            self.api.wind_current(cx);
            self.wind_timer = cx.start_interval(300.0);
        } else {
            cx.stop_timer(self.wind_timer);
            self.wind_timer = Timer::empty();
            self.ui.map_view(cx, ids!(map)).set_wind_field(
                cx,
                0,
                0,
                Vec::new(),
                Vec::new(),
                (0.0, 0.0, 0.0, 0.0),
            );
        }
    }

    fn handle_api_event(&mut self, cx: &mut Cx, event: NavApiEvent) {
        match event {
            NavApiEvent::Search(results) => self.panel.set_results(cx, &self.ui, results),
            NavApiEvent::Route(route) => {
                self.reroute_in_flight = false;
                let destination = self.pending_destination.take();
                let from = route.points.first().copied().unwrap_or_else(|| self.current_position(cx));
                let to = route.points.last().copied().unwrap_or(from);
                let destination_name = destination
                    .as_ref()
                    .map(|result| result.name.clone())
                    .unwrap_or_else(|| "Destination".to_string());
                self.trip = TripModel::from_waypoints(&[
                    ("Current position".to_string(), from.lon, from.lat),
                    (destination_name.clone(), to.lon, to.lat),
                ]);
                self.trip.legs = vec![Leg {
                    polyline: route.points.iter().map(|point| (point.lon, point.lat)).collect(),
                    distance_m: route.length_m,
                    duration_s: route.duration_s,
                }];
                let points = self.trip.full_polyline();
                let map = self.ui.map_view(cx, ids!(map));
                map.set_route(cx, &points);
                map.set_markers(
                    cx,
                    vec![
                        MapMarker::new(1, from.lon, from.lat, vec4(0.10, 0.55, 0.95, 1.0)),
                        MapMarker::new(2, to.lon, to.lat, vec4(0.95, 0.30, 0.20, 1.0)),
                    ],
                );
                self.nav_session = Some(NavSession::new(route.clone()));
                self.nav_started = Some(clock::monotonic_now(cx));
                self.route = Some(route);
                self.panel.set_search_status(
                    cx,
                    &self.ui,
                    &format!(
                        "Route to {destination_name}: {:.1} km · {:.0} min",
                        self.trip.total_distance_m() / 1000.0,
                        self.trip.total_duration_s() / 60.0
                    ),
                );
                self.panel
                    .set_along_status(cx, &self.ui, "Choose chargers or museums.");
            }
            NavApiEvent::Along(results) => {
                let markers = results
                    .iter()
                    .enumerate()
                    .map(|(index, result)| {
                        MapMarker::new(
                            100 + index as u64,
                            result.pos.lon,
                            result.pos.lat,
                            vec4(0.15, 0.70, 0.35, 1.0),
                        )
                    })
                    .collect();
                self.ui.map_view(cx, ids!(map)).set_markers(cx, markers);
                let text = if results.is_empty() {
                    "No matching places along this route".to_string()
                } else {
                    results
                        .iter()
                        .take(3)
                        .map(|result| {
                            format!(
                                "{} ({:.1} km, +{:.0} min)",
                                result.name, result.km_along, result.detour_min
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" · ")
                };
                self.panel.set_along_status(cx, &self.ui, &text);
            }
            NavApiEvent::Weather(weather) => {
                let text = weather
                    .samples
                    .iter()
                    .map(|sample| format!("+{}m {} ({:.1} mm/h)", sample.minute, sample.class, sample.mm_h))
                    .collect::<Vec<_>>()
                    .join(" · ");
                self.panel
                    .set_weather(cx, &self.ui, &format!("Weather now: {text}"));
            }
            NavApiEvent::RadarManifest(manifest) => {
                if !self.rain_enabled {
                    return;
                }
                self.radar_frames.clear();
                for minute in manifest.minutes.iter().copied() {
                    self.api.radar_frame(cx, &manifest.stamp, minute, false);
                }
                if manifest.minutes.contains(&0) {
                    self.api.radar_frame(cx, &manifest.stamp, 0, true);
                }
                self.radar_manifest = Some(manifest);
            }
            NavApiEvent::RadarFrame {
                stamp,
                minute,
                hires,
                png,
            } => {
                if !self.rain_enabled {
                    return;
                }
                let Some(manifest) = self
                    .radar_manifest
                    .as_ref()
                    .filter(|manifest| manifest.stamp == stamp)
                else {
                    return;
                };
                let Ok(image) = ImageBuffer::from_png(&png) else {
                    return;
                };
                let map = self.ui.map_view(cx, ids!(map));
                if hires {
                    map.set_rain_now_hires(cx, Some((image.data, image.width, image.height)));
                } else {
                    self.radar_frames.insert(minute, image.data);
                    map.set_rain_frames(
                        cx,
                        self.radar_frames.values().cloned().collect(),
                        manifest.display.0,
                        manifest.display.1,
                        manifest.bbox,
                    );
                }
            }
            NavApiEvent::Wind(wind) => {
                if self.wind_enabled {
                    self.ui.map_view(cx, ids!(map)).set_wind_field(
                        cx, wind.nx, wind.ny, wind.u, wind.v, wind.bbox,
                    );
                }
            }
            NavApiEvent::Failed {
                operation,
                status,
                message,
            } => {
                if operation == ApiOperation::Route && status != Some(503) {
                    self.reroute_in_flight = false;
                }
                if (matches!(operation, ApiOperation::RadarManifest | ApiOperation::RadarFrame)
                    && !self.rain_enabled)
                    || (operation == ApiOperation::Wind && !self.wind_enabled)
                {
                    return;
                }
                let unavailable_live_layer = matches!(
                    operation,
                    ApiOperation::Weather
                        | ApiOperation::RadarManifest
                        | ApiOperation::RadarFrame
                        | ApiOperation::Wind
                ) && status == Some(404);
                if unavailable_live_layer {
                    match operation {
                        ApiOperation::Weather => self.panel.hide_weather(cx, &self.ui),
                        ApiOperation::RadarManifest | ApiOperation::RadarFrame => {
                            self.set_rain(cx, false);
                            self.ui
                                .check_box(cx, ids!(rain_toggle))
                                .set_active(cx, false, Animate::No);
                        }
                        ApiOperation::Wind => {
                            self.set_wind(cx, false);
                            self.ui
                                .check_box(cx, ids!(wind_toggle))
                                .set_active(cx, false, Animate::No);
                        }
                        _ => {}
                    }
                    return;
                }
                let text = if matches!(operation, ApiOperation::Search | ApiOperation::Route | ApiOperation::Along)
                    && status == Some(503)
                {
                    format!("{operation:?} service is starting; retrying automatically…")
                } else {
                    format!("{operation:?}: {message}")
                };
                match operation {
                    ApiOperation::Search | ApiOperation::Route => {
                        self.panel.set_search_status(cx, &self.ui, &text)
                    }
                    ApiOperation::Along => self.panel.set_along_status(cx, &self.ui, &text),
                    _ => self.panel.set_weather(cx, &self.ui, &text),
                }
            }
        }
    }

    fn update_location(&mut self, cx: &mut Cx, fix: &LocationUpdateEvent) {
        let map = self.ui.map_view(cx, ids!(map));
        map.set_puck(
            cx,
            Some(MapPuck::new(fix.lon, fix.lat, fix.heading_deg, fix.accuracy_m)),
        );
        if let (Some(session), Some(started)) = (&mut self.nav_session, self.nav_started) {
            let status = session.update(
                LonLat::new(fix.lon, fix.lat),
                (clock::monotonic_now(cx) - started).max(0.0),
            );
            let route = session.route();
            let progress = route
                .cum_dist_m
                .partition_point(|distance| *distance < status.progress_m);
            map.set_route_progress(cx, progress);
            if status.needs_reroute && !self.reroute_in_flight {
                if let Some(destination) = route.points.last().copied() {
                    self.reroute_in_flight = true;
                    self.api.route(
                        cx,
                        LonLat::new(fix.lon, fix.lat),
                        destination,
                        route.mode,
                    );
                }
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        self.handle_panel_actions(cx, actions);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        crate::side_panel::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ensure_started(cx);
        self.provisioner.handle_event();
        if self.rain_enabled && self.radar_timer.is_event(event).is_some() {
            self.api.radar_manifest(cx);
        }
        if self.wind_enabled && self.wind_timer.is_event(event).is_some() {
            self.api.wind_current(cx);
        }
        let api_events = self.api.handle_event(cx, event);
        for api_event in api_events {
            self.handle_api_event(cx, api_event);
        }
        if let Event::LocationUpdate(fix) = event {
            self.position = Some(fix.clone());
            self.update_location(cx, fix);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
