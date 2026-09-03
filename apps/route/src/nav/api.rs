//! Hosted navigation service and demo-profile controller.
//!
//! The controller intentionally registers `crate::script_mod`: the demo has
//! no UI DSL of its own. Only data/service behavior differs from native.

use crate::{
    assistant::{AssistantController, AssistantService},
    clock,
    nav_api::{ApiOperation, NavApi, NavApiEvent, RadarManifest, RouteRequestContext},
    overlays::{self, OverlaySelection},
    provisioner::MapProvisioner,
    side_panel::{PanelAction, PanelController},
    ChatEntry, ChatState, EntryKind, AMSTERDAM_CENTER,
};
use makepad_map_nav::{
    geo::LonLat,
    graph::{Route, TravelMode},
    nav::NavSession,
    search::SearchResult,
};
use makepad_widgets::*;
use std::collections::BTreeMap;

/// Common navigation-service surface. The native implementation is backed by
/// the filesystem data plane; this implementation forwards to the site API.
pub trait NavService {
    fn search(&mut self, cx: &mut Cx, query: &str, near: Option<LonLat>, limit: usize);
    fn route(
        &mut self,
        cx: &mut Cx,
        from: LonLat,
        to: LonLat,
        mode: TravelMode,
        context: RouteRequestContext,
    );
    fn along(
        &mut self,
        cx: &mut Cx,
        route: &Route,
        kinds: &[&str],
        max_detour_min: f64,
        min_kw: f64,
        limit: usize,
        route_generation: u64,
    );
    fn weather(&mut self, cx: &mut Cx, at: LonLat);
    fn radar_manifest(&mut self, cx: &mut Cx);
    fn radar_frame(&mut self, cx: &mut Cx, stamp: &str, minute: i64, hires: bool);
    fn wind(&mut self, cx: &mut Cx);
}

impl NavService for NavApi {
    fn search(&mut self, cx: &mut Cx, query: &str, near: Option<LonLat>, limit: usize) {
        NavApi::search(self, cx, query, near, limit);
    }

    fn route(
        &mut self,
        cx: &mut Cx,
        from: LonLat,
        to: LonLat,
        mode: TravelMode,
        context: RouteRequestContext,
    ) {
        NavApi::route(self, cx, from, to, mode, context);
    }

    fn along(
        &mut self,
        cx: &mut Cx,
        route: &Route,
        kinds: &[&str],
        max_detour_min: f64,
        min_kw: f64,
        limit: usize,
        route_generation: u64,
    ) {
        NavApi::along(
            self,
            cx,
            route,
            kinds,
            max_detour_min,
            min_kw,
            limit,
            route_generation,
        );
    }

    fn weather(&mut self, cx: &mut Cx, at: LonLat) {
        self.weather_now(cx, at);
    }

    fn radar_manifest(&mut self, cx: &mut Cx) {
        NavApi::radar_manifest(self, cx);
    }

    fn radar_frame(&mut self, cx: &mut Cx, stamp: &str, minute: i64, hires: bool) {
        NavApi::radar_frame(self, cx, stamp, minute, hires);
    }

    fn wind(&mut self, cx: &mut Cx) {
        self.wind_current(cx);
    }
}

#[derive(Default)]
struct DemoLayers {
    overlays: OverlaySelection,
    rain: bool,
    wind: bool,
    terrain: bool,
    tilt_shift: bool,
    theme: u32,
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
    assistant: AssistantService,
    #[rust]
    panel: PanelController,
    #[rust]
    api: NavApi,
    #[rust]
    chat: ChatState,
    #[rust]
    layers: DemoLayers,
    #[rust]
    layers_panel_open: bool,
    #[rust]
    assistant_panel_open: bool,
    #[rust]
    warp_check_disabled: Option<bool>,
    #[rust]
    position: Option<LocationUpdateEvent>,
    #[rust]
    route: Option<Route>,
    #[rust]
    nav_session: Option<NavSession>,
    #[rust]
    nav_started: Option<f64>,
    #[rust]
    route_generation: u64,
    #[rust]
    radar_manifest: Option<RadarManifest>,
    #[rust]
    radar_frames: BTreeMap<i64, Vec<u32>>,
    #[rust]
    radar_hires_stamp: Option<String>,
    #[rust]
    radar_timer: Timer,
    #[rust]
    wind_timer: Timer,
}

impl App {
    #[cfg(test)]
    pub(crate) fn ui_ref(&self) -> &WidgetRef {
        &self.ui
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.layers.tilt_shift = true;
        self.layers.theme = theme_for_hour(local_hour_now());
        let map = self.ui.map_view(cx, ids!(map));
        self.provisioner.ensure_source(cx, &map);
        self.api = NavApi::new(self.provisioner.api_url());
        self.assistant.configure_ui(cx, &self.ui);
        self.set_status(cx, crate::assistant::UNAVAILABLE_REPLY);
        self.apply_layers(cx);
        cx.start_location_updates();
    }

    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, text);
    }

    fn push_entry(&mut self, cx: &mut Cx, kind: EntryKind, text: &str) {
        self.chat.entries.push(ChatEntry {
            kind,
            text: text.to_string(),
            trip: None,
        });
        self.ui
            .portal_list(cx, ids!(list))
            .set_first_id_and_scroll(self.chat.entries.len().saturating_sub(1), 10.0);
        self.ui.redraw(cx);
    }

    fn handle_prompt(&mut self, cx: &mut Cx, text: &str) {
        self.ui.text_input(cx, ids!(prompt_input)).set_text(cx, "");
        self.push_entry(cx, EntryKind::User, text);
        if let Some(reply) = self.assistant.unavailable_reply(text) {
            self.push_entry(cx, EntryKind::Assistant, reply);
        }
    }

    fn sync_layer_checkboxes(&self, cx: &mut Cx) {
        overlays::sync_checkboxes(cx, &self.ui, &self.layers.overlays);
        for (id, on) in [
            (ids!(layer_rain), self.layers.rain),
            (ids!(layer_wind), self.layers.wind),
            (ids!(layer_terrain), self.layers.terrain),
            (ids!(tilt_check), self.layers.tilt_shift),
        ] {
            self.ui.check_box(cx, id).set_active(cx, on, Animate::No);
        }
        self.ui
            .check_box(cx, ids!(theme_night))
            .set_active(cx, self.layers.theme == 1, Animate::No);
        self.ui
            .check_box(cx, ids!(theme_circuit))
            .set_active(cx, self.layers.theme == 2, Animate::No);
    }

    fn apply_ui_theme(&mut self, cx: &mut Cx) {
        let dark = self.layers.theme != 0;
        self.chat.dark = dark;
        let text_main = if dark {
            vec4(0.87, 0.90, 0.93, 1.0)
        } else {
            vec4(0.13, 0.19, 0.22, 1.0)
        };
        let text_dim = if dark {
            vec4(0.52, 0.56, 0.61, 1.0)
        } else {
            vec4(0.13, 0.19, 0.24, 1.0)
        };
        let (tint, tint_alpha) = if dark {
            (vec4(0.04, 0.06, 0.09, 1.0), 0.42f32)
        } else {
            (vec4(0.97, 0.98, 1.0, 1.0), 0.30f32)
        };
        for id in [ids!(assistant_panel), ids!(layers_panel)] {
            let mut panel = self.ui.widget(cx, id);
            script_apply_eval!(cx, panel, {
                draw_bg +: { tint_color: #(tint) tint_alpha: #(tint_alpha) }
            });
        }
        for id in [ids!(header_label), ids!(intro_label), ids!(status_label)] {
            let mut label = self.ui.label(cx, id);
            let color = if id == ids!(header_label) { text_main } else { text_dim };
            script_apply_eval!(cx, label, { draw_text +: { color: #(color) } });
        }
        let hot = if dark { vec4(1.0, 1.0, 1.0, 1.0) } else { vec4(0.0, 0.0, 0.0, 1.0) };
        for id in [
            ids!(layer_rain), ids!(layer_wind), ids!(layer_terrain), ids!(tilt_check),
            ids!(layer_chargers), ids!(layer_transit), ids!(layer_nature),
            ids!(layer_districts), ids!(layer_buildings), ids!(layer_demographics),
            ids!(theme_night), ids!(theme_circuit),
        ] {
            let mut check = self.ui.check_box(cx, id);
            script_apply_eval!(cx, check, {
                draw_text +: {
                    color: #(text_main) color_active: #(text_main)
                    color_hover: #(hot) color_down: #(hot) color_focus: #(text_main)
                }
            });
        }
        let input_text = if dark { vec4(0.88, 0.91, 0.94, 1.0) } else { vec4(0.09, 0.13, 0.16, 1.0) };
        let placeholder = if dark { vec4(0.50, 0.54, 0.58, 1.0) } else { vec4(0.42, 0.47, 0.52, 1.0) };
        let mut input = self.ui.text_input(cx, ids!(prompt_input));
        script_apply_eval!(cx, input, {
            draw_text +: {
                color: #(input_text) color_hover: #(input_text) color_focus: #(input_text)
                color_empty: #(placeholder) color_empty_hover: #(placeholder)
                color_empty_focus: #(placeholder)
            }
        });
    }

    fn apply_layers(&mut self, cx: &mut Cx) {
        self.sync_layer_checkboxes(cx);
        self.apply_ui_theme(cx);
        let map = self.ui.map_view(cx, ids!(map));
        map.set_overlays(cx, self.provisioner.overlay_sources(&self.layers.overlays));
        map.set_theme(cx, self.layers.theme);
    }

    fn set_rain(&mut self, cx: &mut Cx, on: bool) {
        self.layers.rain = on;
        if on {
            self.api.radar_manifest(cx);
            let center = self.map_center(cx);
            self.api.weather_now(cx, center);
            self.radar_timer = cx.start_interval(30.0);
        } else {
            self.api.cancel_rain(cx);
            cx.stop_timer(self.radar_timer);
            self.radar_timer = Timer::empty();
            self.radar_frames.clear();
            self.radar_hires_stamp = None;
            let bbox = self.radar_manifest.as_ref().map(|m| m.bbox).unwrap_or_default();
            let map = self.ui.map_view(cx, ids!(map));
            map.set_rain_frames(cx, Vec::new(), 0, 0, bbox);
            map.set_rain_now_hires(cx, None);
        }
    }

    fn set_wind(&mut self, cx: &mut Cx, on: bool) {
        self.layers.wind = on;
        if on {
            self.api.wind_current(cx);
            self.wind_timer = cx.start_interval(300.0);
        } else {
            cx.stop_timer(self.wind_timer);
            self.wind_timer = Timer::empty();
            self.ui.map_view(cx, ids!(map)).set_wind_field(
                cx, 0, 0, Vec::new(), Vec::new(), (0.0, 0.0, 0.0, 0.0),
            );
        }
    }

    fn map_center(&self, cx: &mut Cx) -> LonLat {
        self.ui
            .map_view(cx, ids!(map))
            .center()
            .map(|(lon, lat)| LonLat::new(lon, lat))
            .unwrap_or_else(|| LonLat::new(AMSTERDAM_CENTER.0, AMSTERDAM_CENTER.1))
    }

    fn handle_api_event(&mut self, cx: &mut Cx, event: NavApiEvent) {
        match event {
            NavApiEvent::Search(results) => self.apply_search_results(cx, results),
            NavApiEvent::Route { context, route } => {
                let generation = context.generation();
                if generation < self.route_generation {
                    return;
                }
                self.route_generation = generation;
                let points = route.points.iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>();
                self.ui.map_view(cx, ids!(map)).set_route(cx, &points);
                self.nav_session = Some(NavSession::new(route.clone()));
                self.nav_started = Some(clock::monotonic_now(cx));
                self.route = Some(route);
            }
            NavApiEvent::Along { route_generation, results } => {
                if route_generation == self.route_generation {
                    self.push_entry(cx, EntryKind::Info, &format!("{} places along route", results.len()));
                }
            }
            NavApiEvent::Weather(weather) => {
                let text = weather.samples.first().map(|sample| {
                    format!("weather: {} ({:.1} mm/h)", sample.class, sample.mm_h)
                });
                if let Some(text) = text {
                    self.set_status(cx, &text);
                }
            }
            NavApiEvent::RadarManifest(manifest) => self.accept_radar_manifest(cx, manifest),
            NavApiEvent::RadarFrame { stamp, minute, hires, png } => {
                self.accept_radar_frame(cx, stamp, minute, hires, png)
            }
            NavApiEvent::Wind(wind) => {
                if self.layers.wind {
                    self.ui.map_view(cx, ids!(map)).set_wind_field(
                        cx, wind.nx, wind.ny, wind.u, wind.v, wind.bbox,
                    );
                }
            }
            NavApiEvent::Failed { operation, status, message, retrying, .. } => {
                if !retrying {
                    self.set_status(cx, &format!("{operation:?}: {message}"));
                }
                if status == Some(404) {
                    if matches!(operation, ApiOperation::RadarManifest | ApiOperation::RadarFrame) {
                        self.set_rain(cx, false);
                    } else if operation == ApiOperation::Wind {
                        self.set_wind(cx, false);
                    }
                    self.sync_layer_checkboxes(cx);
                }
            }
        }
    }

    fn apply_search_results(&mut self, cx: &mut Cx, results: Vec<SearchResult>) {
        if let Some(first) = results.first() {
            self.ui.map_view(cx, ids!(map)).fly_to(cx, first.pos.lon, first.pos.lat, 13.5);
        }
    }

    fn accept_radar_manifest(&mut self, cx: &mut Cx, manifest: RadarManifest) {
        if !self.layers.rain {
            return;
        }
        let new_stamp = self.radar_manifest.as_ref().is_none_or(|old| old.stamp != manifest.stamp);
        if new_stamp {
            self.api.cancel_radar_frames(cx);
            self.radar_frames.clear();
            self.radar_hires_stamp = None;
        }
        for minute in manifest.minutes.iter().copied() {
            if !self.radar_frames.contains_key(&minute) {
                self.api.radar_frame(cx, &manifest.stamp, minute, false);
            }
        }
        if manifest.minutes.contains(&0) && self.radar_hires_stamp.as_deref() != Some(&manifest.stamp) {
            self.api.radar_frame(cx, &manifest.stamp, 0, true);
        }
        self.radar_manifest = Some(manifest);
    }

    fn accept_radar_frame(&mut self, cx: &mut Cx, stamp: String, minute: i64, hires: bool, png: Vec<u8>) {
        let Some(manifest) = self.radar_manifest.as_ref().filter(|m| {
            self.layers.rain && m.stamp == stamp && m.minutes.contains(&minute) && (!hires || minute == 0)
        }) else {
            return;
        };
        let Ok(image) = ImageBuffer::from_png(&png) else {
            return;
        };
        let expected = if hires { manifest.hires_now } else { manifest.display };
        if (image.width, image.height) != expected {
            return;
        }
        let map = self.ui.map_view(cx, ids!(map));
        if hires {
            map.set_rain_now_hires(cx, Some((image.data, image.width, image.height)));
            self.radar_hires_stamp = Some(stamp);
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
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in self.panel.actions(cx, &self.ui, actions) {
            let PanelAction::Search(text) = action;
            self.handle_prompt(cx, &text);
        }
        if self.ui.button(cx, ids!(layers_button)).clicked(actions) {
            self.layers_panel_open = !self.layers_panel_open;
            self.ui.widget(cx, ids!(layers_panel)).set_visible(cx, self.layers_panel_open);
        }
        if self.ui.button(cx, ids!(assistant_button)).clicked(actions) {
            self.assistant_panel_open = !self.assistant_panel_open;
            self.ui.widget(cx, ids!(assistant_panel)).set_visible(cx, self.assistant_panel_open);
        }
        if overlays::handle_checkboxes(cx, &self.ui, actions, &mut self.layers.overlays) {
            self.apply_layers(cx);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(layer_rain)).changed(actions) {
            self.set_rain(cx, on);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(layer_wind)).changed(actions) {
            self.set_wind(cx, on);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(layer_terrain)).changed(actions) {
            self.layers.terrain = on;
            if !on {
                self.ui.map_view(cx, ids!(map)).set_terrain_overlay(cx, TerrainOverlayData::default());
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(tilt_check)).changed(actions) {
            self.layers.tilt_shift = on;
            self.ui.redraw(cx);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(warp_check)).changed(actions) {
            let map = self.ui.map_view(cx, ids!(map));
            if map.space_warp_available() {
                map.set_space_warp(cx, on);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(theme_night)).changed(actions) {
            self.layers.theme = if on { 1 } else { 0 };
            self.apply_layers(cx);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(theme_circuit)).changed(actions) {
            self.layers.theme = if on { 2 } else { 0 };
            self.apply_layers(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        crate::side_panel::script_mod(vm);
        crate::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ensure_started(cx);
        self.provisioner.handle_event();
        if self.layers.rain && self.radar_timer.is_event(event).is_some() {
            self.api.radar_manifest(cx);
        }
        if self.layers.wind && self.wind_timer.is_event(event).is_some() {
            self.api.wind_current(cx);
        }
        for api_event in self.api.handle_event(cx, event) {
            self.handle_api_event(cx, api_event);
        }
        if let Event::LocationUpdate(fix) = event {
            self.position = Some(fix.clone());
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
                let progress = session
                    .route()
                    .cum_dist_m
                    .partition_point(|distance| *distance < status.progress_m);
                map.set_route_progress(cx, progress);
            }
        }
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::Escape && self.assistant_panel_open {
                self.assistant_panel_open = false;
                self.ui.widget(cx, ids!(assistant_panel)).set_visible(cx, false);
            }
        }
        let warp_available = self.ui.map_view(cx, ids!(map)).space_warp_available();
        if self.warp_check_disabled != Some(!warp_available) {
            self.warp_check_disabled = Some(!warp_available);
            self.ui.check_box(cx, ids!(warp_check)).set_disabled(cx, !warp_available);
        }
        self.match_event(cx, event);
        self.chat.tilt_shift_on = self.layers.tilt_shift;
        let tilt = self.ui.map_view(cx, ids!(map)).tilt() as f32;
        self.chat.tilt_strength = ((tilt - 5.0) / 50.0).clamp(0.0, 1.0);
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.chat));
    }
}

fn local_hour_now() -> u32 {
    ((Cx::time_now() as i64).rem_euclid(86_400) / 3_600) as u32
}

fn theme_for_hour(hour: u32) -> u32 {
    if hour >= 19 || hour < 7 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_twilight_theme_matches_native_boundaries() {
        assert_eq!(theme_for_hour(6), 1);
        assert_eq!(theme_for_hour(7), 0);
        assert_eq!(theme_for_hour(18), 0);
        assert_eq!(theme_for_hour(19), 1);
    }
}
