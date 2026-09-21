//! `RouteView`: the route app as one widget — the map with its layers,
//! the trip and turn-by-turn navigation, the side panel with its
//! transcript, the assistant's tools — everything the standalone window
//! used to keep on the app, without the window. The standalone binary
//! (main.rs) puts a `Window` around it; the module (module.rs) seats it in
//! a host's isolate. Neither reaches inside: the app's tools are answered
//! through [`RouteView::execute_tool`], the assistant through the seam
//! (assistant/mod.rs).
//!
//! The view resolves its map root once, on first use, and keeps every
//! path it derives (maps_root.rs); nothing here reads the process cwd.

use crate::assistant::{AssistantController, AssistantEvent, AssistantService, BeginTool, PromptOutcome};
use crate::broker::{self, MarkerLegend, ToolCtx};
use crate::chrome::{
    location_error_status, show_location_status, ChatEntry, ChatState, EntryKind, LocationClick,
    LocationFix, LocationState, ThemePreference, AMSTERDAM_CENTER, LOCATION_FIX_TIMEOUT_SECONDS,
    THEME_STORAGE,
};
use crate::history::DriveLog;
use crate::layers::{self, LayerState, WindUpdate};
use crate::maps_root::{self, RoutePaths};
use crate::nav::native::{self as nav_data, NavData, NavLoad, RadarData};
use crate::nav::{ActiveNav, NavAction, NavTick};
use crate::overlays::{self, TerrainLayer};
use crate::provisioner::MapProvisioner;
use crate::side_panel::{PanelAction, PanelController};
use crate::tools;
use crate::trip::{self, TripModel};
use makepad_widgets::makepad_platform::storage::StorageHandle;
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.RouteViewBase = #(RouteView::register_widget(vm))

    mod.widgets.RouteView = set_type_default() do mod.widgets.RouteViewBase {
        width: Fill
        height: Fill
        chrome := RouteChrome{}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RouteView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    started: bool,
    /// Where the map data is, when a host decided; otherwise resolved on
    /// first use (maps_root.rs).
    #[rust]
    map_root: Option<PathBuf>,
    /// Every path derived from the map root, once started.
    #[rust]
    paths: Option<RoutePaths>,
    /// The instance's storage: the app's own namespace standalone, the
    /// host's jail for a module.
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    theme_preference: ThemePreference,
    #[rust]
    panel: PanelController,
    /// The brain, or the note that there is none (assistant/mod.rs).
    #[rust]
    assistant: AssistantService,
    /// Last pushed disabled-state of the Space-warp row (None = never
    /// pushed): the row grays out whenever the camera leaves the
    /// near-first-person regime and re-enables when it returns.
    #[rust]
    warp_check_disabled: Option<bool>,
    #[rust]
    chat: ChatState,
    #[rust]
    drive_log: DriveLog,
    #[rust]
    nav_rx: ToUIReceiver<NavLoad>,
    #[rust]
    radar_rx: ToUIReceiver<RadarData>,
    #[rust]
    nav: Option<NavData>,
    #[rust]
    radar: Option<RadarData>,
    #[rust]
    trip: TripModel,
    #[rust]
    markers: MarkerLegend,
    /// Latest GPS fix from the platform geo service.
    #[rust]
    position: Option<LocationUpdateEvent>,
    #[rust]
    had_first_fix: bool,
    #[rust]
    location_state: LocationState,
    #[rust]
    location_timeout: Timer,
    #[rust]
    layers: LayerState,
    #[rust]
    layers_panel_open: bool,
    /// Map source selection, and the first-run acquisition where the
    /// build carries it.
    #[rust]
    provisioner: MapProvisioner,
    /// Route assistant popover (bottom-right button). Closed on every
    /// launch — nothing persisted.
    #[rust]
    assistant_panel_open: bool,
    /// Routed legs with maneuvers (for turn-by-turn).
    #[rust]
    leg_routes: Vec<makepad_map_nav::graph::Route>,
    #[rust]
    active_nav: Option<ActiveNav>,
    #[rust]
    nav_frame: NextFrame,
    #[rust]
    wind_rx: ToUIReceiver<WindUpdate>,
    #[rust]
    terrain_layer: TerrainLayer,
    /// Last nav banner instruction spoken, so each maneuver is announced once.
    #[rust]
    last_spoken_banner: String,
}

impl RouteView {
    /// A host's choice of map root, before the first event.
    pub fn set_map_root(&mut self, root: PathBuf) {
        self.map_root = Some(root);
    }

    /// The instance's storage (the module contract's jail), before the
    /// first event. Standalone the view opens its own namespace.
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    /// The map data layout this instance resolved, once started.
    pub fn paths(&self) -> Option<&RoutePaths> {
        self.paths.as_ref()
    }

    /// The chrome as a widget ref, for the helpers that take one.
    fn ui(&self, cx: &Cx) -> WidgetRef {
        self.view.widget(cx, ids!(chrome))
    }

    fn map(&self, cx: &Cx) -> MapViewRef {
        self.view.map_view(cx, ids!(map))
    }

    /// Idempotent init; also re-runs after a script hot-reload wipes
    /// `#[rust]` state (same guard pattern as examples/map).
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        let storage = self
            .storage
            .get_or_insert_with(|| cx.storage(THEME_STORAGE))
            .clone();
        self.theme_preference.start(cx, &storage);
        let ui = self.ui(cx);
        self.assistant.configure_ui(cx, &ui);
        self.layers.dirty = false;
        let maps_root = self
            .map_root
            .clone()
            .unwrap_or_else(maps_root::resolve_maps_root);
        let paths = RoutePaths::from_maps_root(maps_root);
        log!("maps root: {}", paths.maps.display());
        self.layers.set_maps_root(paths.maps.clone());
        self.drive_log.set_dir(paths.history.clone());
        let radar_cache = paths.radar_cache();
        self.paths = Some(paths);
        // Applies the theme above (chrome + map + checkboxes) and reflects
        // the rest of the LayerState defaults (e.g. tilt-shift on) in the
        // layers popover.
        self.apply_layers(cx);
        self.adopt_map_source(cx);
        nav_data::start_radar_worker(
            cx.thread_spawner(),
            cx.task_pool(),
            self.radar_rx.sender(),
            radar_cache,
        );
        self.assistant.start(cx);
        let status = self.assistant.status_text();
        self.set_status(cx, &status);
    }

    /// What the host's assistant is told about the map and the trip.
    pub fn ai_context_line(&self, cx: &mut Cx) -> String {
        let map = self.map(cx);
        let (lon, lat) = map.center().unwrap_or(AMSTERDAM_CENTER);
        let zoom = map.map_zoom().unwrap_or(13.0);
        let trip = match (self.trip.stops.first(), self.trip.stops.last()) {
            (Some(from), Some(to)) if self.trip.is_routed() => format!(
                "Current trip: {} → {}, {:.1} km, ETA {}.",
                from.name,
                to.name,
                self.trip.total_distance_m() / 1000.0,
                trip::fmt_duration(self.trip.total_duration_s()),
            ),
            (Some(from), Some(to)) => {
                format!("Current trip: {} → {} (not routed yet).", from.name, to.name)
            }
            _ => "No trip is planned.".to_string(),
        };
        format!("Map centre: {lon:.5}, {lat:.5}; zoom {zoom:.1}. {trip}")
    }

    /// The host's chat surface came up: the in-window popover steps aside.
    pub fn close_assistant_panel(&mut self, cx: &mut Cx) {
        if self.assistant_panel_open {
            self.assistant_panel_open = false;
            self.view
                .widget(cx, ids!(assistant_panel))
                .set_visible(cx, false);
        }
    }

    /// The end of the instance: the location watch, the drive record.
    /// Workers end with their receivers when the view is dropped.
    pub fn shutdown(&mut self, cx: &mut Cx) {
        self.cancel_location_timeout(cx);
        if self.location_state.stop() {
            cx.stop_location_updates();
        }
        self.drive_log.close();
    }

    /// Point the map and the nav plane at whatever this machine has:
    /// the production archives, else a baked test map, else the hosted
    /// archive — in which case the first-run card offers to build one.
    fn adopt_map_source(&mut self, cx: &mut Cx) {
        let Some(paths) = self.paths.clone() else {
            return;
        };
        let nav_basename = nav_data::nav_basename(&paths.maps);
        if let Some(basename) = nav_basename.clone() {
            nav_data::start_nav_load(cx.task_pool(), self.nav_rx.sender(), basename, paths.chargers());
        }
        let map = self.map(cx);
        if let Some(basename) = self.provisioner.ensure_source(cx, &map, &paths.maps) {
            if nav_basename.is_none() {
                nav_data::start_nav_load(cx.task_pool(), self.nav_rx.sender(), basename, paths.chargers());
            }
        }
        self.refresh_testmap_ui(cx);
    }

    /// Mirror the build state into the card.
    fn refresh_testmap_ui(&mut self, cx: &mut Cx) {
        let card = self.provisioner.card();
        self.view
            .widget(cx, ids!(testmap_panel))
            .set_visible(cx, card.is_some());
        let Some(card) = card else {
            return;
        };
        self.view
            .label(cx, ids!(testmap_headline))
            .set_text(cx, &card.headline);
        self.view
            .label(cx, ids!(testmap_status))
            .set_text(cx, &card.status);
        self.view.label(cx, ids!(testmap_log)).set_text(cx, &card.log);
        // The bar is a plain fill inside a fixed 470px track.
        let width = (470.0_f64 * f64::from(card.fraction.clamp(0.0, 1.0))).round();
        let mut bar = self.view.widget(cx, ids!(testmap_bar));
        script_apply_eval!(cx, bar, {
            width: #(width)
        });
        self.view
            .button(cx, ids!(testmap_start))
            .set_visible(cx, card.can_start);
        self.view
            .button(cx, ids!(testmap_start))
            .set_text(cx, card.start_label);
        self.view
            .button(cx, ids!(testmap_dismiss))
            .set_text(cx, card.dismiss_label);
        self.view
            .button(cx, ids!(testmap_dismiss))
            .set_visible(cx, card.dismissable);
        self.view.redraw(cx);
    }

    fn set_status(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(status_label)).set_text(cx, text);
    }

    // --- location ----------------------------------------------------------

    fn cancel_location_timeout(&mut self, cx: &mut Cx) {
        if !self.location_timeout.is_empty() {
            cx.stop_timer(self.location_timeout);
            self.location_timeout = Timer::empty();
        }
    }

    fn handle_location_click(&mut self, cx: &mut Cx) {
        match self.location_state.clicked() {
            LocationClick::Start => {
                show_location_status(cx, &self.ui(cx), "Locating…");
                cx.start_location_updates();
                self.location_timeout = cx.start_timeout(LOCATION_FIX_TIMEOUT_SECONDS);
            }
            LocationClick::Recenter => {
                if let Some(fix) = &self.position {
                    self.map(cx).fly_to(cx, fix.lon, fix.lat, 14.0);
                    show_location_status(cx, &self.ui(cx), "Location found");
                }
            }
            LocationClick::Ignore => {}
        }
    }

    fn fail_location(&mut self, cx: &mut Cx, status: &str) {
        if !self.location_state.failed() {
            return;
        }
        self.cancel_location_timeout(cx);
        cx.stop_location_updates();
        show_location_status(cx, &self.ui(cx), status);
    }

    fn on_location_update(&mut self, cx: &mut Cx, fix: &LocationUpdateEvent) {
        let location_fix = self.location_state.received_fix();
        if location_fix == LocationFix::Ignore {
            return;
        }
        if location_fix == LocationFix::First {
            self.cancel_location_timeout(cx);
            show_location_status(cx, &self.ui(cx), "Location found");
        }
        self.drive_log.log_fix(fix);
        let map = self.map(cx);
        map.set_puck(
            cx,
            Some(MapPuck::new(fix.lon, fix.lat, fix.heading_deg, fix.accuracy_m)),
        );
        if location_fix == LocationFix::First || !self.had_first_fix {
            self.had_first_fix = true;
            map.fly_to(cx, fix.lon, fix.lat, 14.0);
            self.push_line(cx, &format!("gps: fix acquired (±{:.0}m)", fix.accuracy_m));
        }
        self.position = Some(fix.clone());
        // Live turn-by-turn: feed the fix into the session.
        let tick = self.active_nav.as_mut().and_then(|nav| {
            if nav.simulate {
                return None;
            }
            let now = Cx::monotonic_now();
            let dt = nav
                .sim_last_tick
                .map(|last| now - last)
                .unwrap_or(1.0)
                .clamp(0.05, 5.0);
            nav.sim_last_tick = Some(now);
            let pos = makepad_map_nav::geo::LonLat {
                lon: fix.lon,
                lat: fix.lat,
            };
            Some(nav.feed(pos, fix.heading_deg, dt))
        });
        if let Some(tick) = tick {
            self.apply_nav_tick(cx, tick);
        }
    }

    // --- transcript --------------------------------------------------------

    fn render_transcript(&mut self, cx: &mut Cx) {
        let list = self.view.portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.redraw(cx);
    }

    fn push_entry(&mut self, cx: &mut Cx, kind: EntryKind, text: &str) {
        self.chat.entries.push(ChatEntry {
            kind,
            text: text.to_string(),
            trip: None,
        });
        self.render_transcript(cx);
    }

    fn push_line(&mut self, cx: &mut Cx, line: &str) {
        self.push_entry(cx, EntryKind::Info, line);
    }

    /// Push a snapshot of the current trip as a re-applyable chat row.
    fn push_trip_entry(&mut self, cx: &mut Cx) {
        if !self.trip.is_routed() {
            return;
        }
        let first = self.trip.stops.first().map(|s| s.name.clone()).unwrap_or_default();
        let last = self.trip.stops.last().map(|s| s.name.clone()).unwrap_or_default();
        let vias = self.trip.stops.len().saturating_sub(2);
        let text = format!(
            "{first} → {last}{} — {:.1} km, {}",
            if vias > 0 { format!(" (+{vias} stop{})", if vias > 1 { "s" } else { "" }) } else { String::new() },
            self.trip.total_distance_m() / 1000.0,
            trip::fmt_duration(self.trip.total_duration_s()),
        );
        let index = self.chat.trips.len();
        self.chat.trips.push(self.trip.clone());
        self.chat.entries.push(ChatEntry {
            kind: EntryKind::Assistant,
            text,
            trip: Some(index),
        });
        self.drive_log.log_trip(&self.trip.digest());
        self.render_transcript(cx);
    }

    fn commit_pending(&mut self, cx: &mut Cx) {
        let text = std::mem::take(&mut self.chat.pending);
        if text.trim().is_empty() {
            return;
        }
        let text = text.trim().to_string();
        self.push_entry(cx, EntryKind::Assistant, &text);
    }

    /// Last N user/assistant lines for the gate's in-context judgement.
    fn recent_dialog(&self) -> Vec<String> {
        const RECENT_DIALOG_LINES: usize = 12;
        self.chat
            .entries
            .iter()
            .filter_map(|e| match e.kind {
                EntryKind::User => Some(format!("user: {}", e.text)),
                EntryKind::Assistant if e.trip.is_none() => Some(format!("assistant: {}", e.text)),
                _ => None,
            })
            .rev()
            .take(RECENT_DIALOG_LINES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    // --- the assistant -----------------------------------------------------

    /// `/tool_name {json args}` — direct tool console, no LLM involved.
    /// Works without an API key; also the manual test path for the broker.
    fn run_local_command(&mut self, cx: &mut Cx, text: &str) {
        let body = &text[1..];
        let (name, args) = match body.split_once(char::is_whitespace) {
            Some((name, rest)) => (name.trim(), rest.trim()),
            None => (body.trim(), ""),
        };
        if name.is_empty() || name == "help" {
            let names: Vec<String> = broker::tool_definitions()
                .iter()
                .map(|d| d.name.clone())
                .collect();
            self.push_line(cx, &format!("tools: {}", names.join(", ")));
            return;
        }
        self.push_entry(cx, EntryKind::Tool, &format!("⚙ {name} {args}"));
        if name == "images_search" {
            let query = broker::parse_field(args, "query").unwrap_or_default();
            match self.assistant.start_image_search(cx, &query, None) {
                Ok(()) => self.push_entry(cx, EntryKind::Info, &format!("🔎 images: {query}")),
                Err(error) => self.push_entry(cx, EntryKind::Tool, &format!("⚠ {error}")),
            }
            return;
        }
        match self.execute_tool(cx, name, args) {
            Ok(text) => {
                self.push_entry(cx, EntryKind::Assistant, &text);
                if matches!(name, "route_plan" | "route_add_stop" | "route_remove_stop") {
                    self.push_trip_entry(cx);
                }
            }
            Err(error) => self.push_entry(cx, EntryKind::Tool, &format!("⚠ {error}")),
        }
    }

    fn send_user_prompt(&mut self, cx: &mut Cx, text: &str) {
        // The console works in every build; a prompt needs a brain.
        if text.starts_with('/') {
            self.run_local_command(cx, text);
            return;
        }
        if let Some(reply) = self.assistant.unavailable_reply(text) {
            self.push_entry(cx, EntryKind::User, text);
            self.push_entry(cx, EntryKind::Assistant, reply);
            return;
        }
        let map = self.map(cx);
        let (lon, lat) = map.center().unwrap_or(AMSTERDAM_CENTER);
        let zoom = map.map_zoom().unwrap_or(13.0);
        let gps = match &self.position {
            Some(p) => format!("{:.5},{:.5} (±{:.0}m)", p.lon, p.lat, p.accuracy_m),
            None => "no fix — 'here' falls back to map center".to_string(),
        };
        let prompt = format!(
            "{text}\n\n[app state]\ngps: {gps}\nmap_center: {lon:.5},{lat:.5} zoom {zoom:.1}\nnav_data: {}\ntrip:\n{}",
            if self.nav.is_some() { "ready" } else { "loading" },
            self.trip.digest()
        );
        // A busy turn is interrupted by the brain itself; its "⏹" line
        // lands through the events, ahead of this prompt's echo.
        let interrupted = self.assistant.is_busy();
        match self.assistant.send_prompt(cx, &prompt) {
            PromptOutcome::NoAgent => {
                self.push_line(cx, "⚠ no agent available");
            }
            PromptOutcome::Sent => {
                if interrupted {
                    self.push_entry(cx, EntryKind::Info, "⏹ interrupted");
                }
                self.push_entry(cx, EntryKind::User, text);
                self.set_status(cx, "thinking…");
            }
        }
    }

    fn on_assistant_event(&mut self, cx: &mut Cx, event: AssistantEvent) {
        match event {
            AssistantEvent::Info(line) => self.push_line(cx, &line),
            AssistantEvent::Text(text) => {
                self.chat.pending.push_str(&text);
                self.render_transcript(cx);
            }
            AssistantEvent::ToolRequest {
                tool_use_id,
                name,
                input,
            } => {
                self.commit_pending(cx);
                self.run_tool(cx, &tool_use_id, &name, &input);
            }
            AssistantEvent::TurnComplete { status } => {
                self.commit_pending(cx);
                self.set_status(cx, &status);
            }
            AssistantEvent::Transcript(text) => {
                self.push_entry(cx, EntryKind::Info, &format!("🎤 “{text}”"));
                let recent = self.recent_dialog();
                self.assistant.submit_utterance(text, recent);
            }
            AssistantEvent::Directed(instruction) => self.send_user_prompt(cx, &instruction),
            AssistantEvent::Thumb { slot, data } => {
                let image_ids: [&[LiveId]; 4] = [ids!(img_0), ids!(img_1), ids!(img_2), ids!(img_3)];
                if let Some(id) = image_ids.get(slot) {
                    let image = self.view.image(cx, id);
                    if image.load_image_from_data(cx, &data).is_ok() {
                        self.view.view(cx, ids!(images_row)).set_visible(cx, true);
                        self.view.view(cx, ids!(images_row)).redraw(cx);
                    }
                }
            }
            AssistantEvent::SearchDone { digest, is_error } => {
                self.push_entry(
                    cx,
                    if is_error { EntryKind::Tool } else { EntryKind::Assistant },
                    &if is_error { format!("⚠ {digest}") } else { digest.clone() },
                );
            }
            AssistantEvent::Status(status) => self.set_status(cx, &status),
        }
    }

    fn run_tool(&mut self, cx: &mut Cx, tool_use_id: &str, name: &str, input: &str) {
        let compact: String = input.chars().take(120).collect();
        self.push_entry(cx, EntryKind::Tool, &format!("⚙ {name} {compact}"));
        if self.assistant.begin_tool(cx, tool_use_id, name, input) == BeginTool::Handled {
            return;
        }
        let result = self.execute_tool(cx, name, input);
        let (text, is_error) = match result {
            Ok(t) => (t, false),
            Err(e) => (e, true),
        };
        if is_error {
            self.push_entry(cx, EntryKind::Tool, &format!("⚠ {text}"));
        } else if matches!(name, "route_plan" | "route_add_stop" | "route_remove_stop") {
            self.push_trip_entry(cx);
        }
        self.assistant.send_tool_result(cx, tool_use_id, &text, is_error);
    }

    /// Execute one broker tool and apply any layer/nav changes it made.
    /// The desktop bus (ai.rs) and the module's executor come in here too.
    pub(crate) fn execute_tool(&mut self, cx: &mut Cx, name: &str, input: &str) -> Result<String, String> {
        let map = self.map(cx);
        let history_dir = self.paths.as_ref().map(|p| p.history.clone()).unwrap_or_default();
        let mut nav_action = None;
        let result = {
            let mut tool_ctx = ToolCtx {
                cx,
                map: &map,
                trip: &mut self.trip,
                nav: self.nav.as_mut(),
                radar: self.radar.as_ref(),
                markers: &mut self.markers,
                position: self.position.as_ref().map(|p| (p.lon, p.lat)),
                layers: &mut self.layers,
                leg_routes: &mut self.leg_routes,
                nav_action: &mut nav_action,
                history_dir: &history_dir,
            };
            broker::execute(&mut tool_ctx, name, input)
        };
        if self.layers.dirty {
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        match nav_action {
            Some(NavAction::Start { simulate }) => self.start_nav(cx, simulate),
            Some(NavAction::Stop) => self.stop_nav(cx),
            None => {}
        }
        result
    }

    // --- Turn-by-turn navigation -------------------------------------------

    fn start_nav(&mut self, cx: &mut Cx, simulate: bool) {
        let Some(mut nav) = ActiveNav::new(self.leg_routes.clone(), simulate) else {
            self.push_line(cx, "⚠ nav: no routed legs");
            return;
        };
        let map = self.map(cx);
        if let Some(start) = nav.start_point() {
            map.fly_to(cx, start.lon, start.lat, 17.0);
        }
        // Driving view: tilted follow camera (also arms tilt-shift).
        map.set_tilt(cx, 42.0);
        self.view.view(cx, ids!(banner)).set_visible(cx, true);
        self.view
            .label(cx, ids!(banner_text))
            .set_text(cx, "Starting navigation…");
        self.view.label(cx, ids!(banner_dist)).set_text(cx, "");
        if simulate {
            nav.sim_last_tick = Some(Cx::monotonic_now());
            self.nav_frame = cx.new_next_frame();
        }
        self.drive_log.log_trip(&format!(
            "nav_start ({})\n{}",
            if simulate { "sim" } else { "gps" },
            self.trip.digest()
        ));
        self.active_nav = Some(nav);
        self.push_line(
            cx,
            if simulate { "▶ navigating (simulated drive)" } else { "▶ navigating (live GPS)" },
        );
    }

    fn stop_nav(&mut self, cx: &mut Cx) {
        if self.active_nav.take().is_none() {
            return;
        }
        let map = self.map(cx);
        map.set_rotation(cx, 0.0);
        map.set_tilt(cx, 0.0);
        self.view.view(cx, ids!(banner)).set_visible(cx, false);
        self.push_line(cx, "■ navigation ended");
    }

    fn apply_nav_tick(&mut self, cx: &mut Cx, tick: NavTick) {
        let map = self.map(cx);
        map.set_puck(
            cx,
            Some(MapPuck::new(
                tick.position.lon,
                tick.position.lat,
                tick.heading,
                12.0,
            )),
        );
        map.set_center(cx, tick.position.lon, tick.position.lat);
        map.set_rotation(cx, tick.rotation);
        map.set_route_progress(cx, tick.progress_index);
        if !tick.banner.is_empty() {
            self.view.label(cx, ids!(banner_text)).set_text(cx, &tick.banner);
            self.view
                .label(cx, ids!(banner_dist))
                .set_text(cx, &tick.banner_dist);
            // Announce each maneuver once, when it becomes the current banner.
            if tick.banner != self.last_spoken_banner {
                self.last_spoken_banner = tick.banner.clone();
                self.assistant.speak(&tick.banner);
            }
        }

        if tick.arrived {
            let dest = self
                .trip
                .stops
                .last()
                .map(|s| s.name.clone())
                .unwrap_or_default();
            self.push_line(cx, &format!("🏁 arrived at {dest}"));
            self.assistant.speak(&format!("You have arrived at {dest}."));
            self.active_nav = None;
            self.map(cx).set_rotation(cx, 0.0);
            return;
        }
        if tick.finished_leg {
            if let Some(nav) = &mut self.active_nav {
                let reached = self
                    .trip
                    .stops
                    .get(nav.leg_index + 1)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                nav.advance_leg();
                self.push_line(cx, &format!("● reached {reached} — continuing"));
            }
        }
        if tick.needs_reroute {
            self.reroute_nav(cx);
        }
    }

    /// Off-route with real GPS: recompute the current leg from here.
    fn reroute_nav(&mut self, cx: &mut Cx) {
        let (Some(nav_data), Some(nav)) = (self.nav.as_mut(), self.active_nav.as_mut()) else {
            return;
        };
        if nav.simulate {
            return;
        }
        let (Some(pos), Some(next_stop)) = (nav.position, self.trip.stops.get(nav.leg_index + 1))
        else {
            return;
        };
        let to = makepad_map_nav::geo::LonLat {
            lon: next_stop.lon,
            lat: next_stop.lat,
        };
        let mode = match self.trip.mode {
            trip::TripMode::Car => makepad_map_nav::graph::TravelMode::Car,
            trip::TripMode::Bike => makepad_map_nav::graph::TravelMode::Bike,
            trip::TripMode::Foot => makepad_map_nav::graph::TravelMode::Foot,
        };
        if let Some(route) = nav_data.route_pair(pos, to, mode) {
            nav.routes[nav.leg_index] = route.clone();
            nav.session = makepad_map_nav::nav::NavSession::new(route);
            // Redraw the route line: completed legs + fresh current + later.
            let points: Vec<(f64, f64)> = nav
                .routes
                .iter()
                .flat_map(|r| r.points.iter().map(|p| (p.lon, p.lat)))
                .collect();
            self.map(cx).set_route(cx, &points);
            self.push_line(cx, "↻ rerouting");
        }
    }

    fn tick_nav_sim(&mut self, cx: &mut Cx) {
        let Some(nav) = &mut self.active_nav else {
            return;
        };
        let Some(tick) = nav.tick_sim() else {
            return;
        };
        self.apply_nav_tick(cx, tick);
        if self.active_nav.is_some() {
            self.nav_frame = cx.new_next_frame();
        }
    }

    /// Redraw route/markers/camera from the current TripModel (used by the
    /// `>` re-apply button; tools do this themselves).
    fn resync_trip_display(&mut self, cx: &mut Cx) {
        let map = self.map(cx);
        let history_dir = self.paths.as_ref().map(|p| p.history.clone()).unwrap_or_default();
        let mut nav_action = None;
        let mut tool_ctx = ToolCtx {
            cx,
            map: &map,
            trip: &mut self.trip,
            nav: self.nav.as_mut(),
            radar: self.radar.as_ref(),
            markers: &mut self.markers,
            position: self.position.as_ref().map(|p| (p.lon, p.lat)),
            layers: &mut self.layers,
            leg_routes: &mut self.leg_routes,
            nav_action: &mut nav_action,
            history_dir: &history_dir,
        };
        tools::map::sync_trip_display(&mut tool_ctx, true);
    }

    // --- layers and theme --------------------------------------------------

    /// Mirror LayerState into the popover checkboxes (agent tools and the
    /// UI share one state).
    fn sync_layer_checkboxes(&mut self, cx: &mut Cx) {
        overlays::sync_checkboxes(cx, &self.ui(cx), &self.layers.overlays);
        self.view
            .check_box(cx, ids!(layer_rain))
            .set_active(cx, self.layers.rain, Animate::No);
        self.view
            .check_box(cx, ids!(layer_wind))
            .set_active(cx, self.layers.wind, Animate::No);
        self.view
            .check_box(cx, ids!(layer_terrain))
            .set_active(cx, self.layers.terrain, Animate::No);
        self.view
            .check_box(cx, ids!(tilt_check))
            .set_active(cx, self.layers.tilt_shift, Animate::No);
        self.view
            .check_box(cx, ids!(cam_check))
            .set_active(cx, self.layers.debug_cam, Animate::No);
        let warp_on = self.map(cx).space_warp();
        self.view
            .check_box(cx, ids!(warp_check))
            .set_active(cx, warp_on, Animate::No);
        self.view
            .check_box(cx, ids!(theme_night))
            .set_active(cx, self.layers.theme == 1, Animate::No);
        self.view
            .check_box(cx, ids!(theme_circuit))
            .set_active(cx, self.layers.theme == 2, Animate::No);
    }

    /// Restyle the app chrome to match the map theme (light vs night/circuit).
    fn apply_ui_theme(&mut self, cx: &mut Cx) {
        let dark = self.layers.theme != 0;
        self.chat.dark = dark;
        let text_main = if dark { vec4(0.87, 0.90, 0.93, 1.0) } else { vec4(0.13, 0.19, 0.22, 1.0) };
        let text_dim = if dark { vec4(0.52, 0.56, 0.61, 1.0) } else { vec4(0.13, 0.19, 0.24, 1.0) };

        // Glass panels: theme via tint (they sample the gauss backdrop).
        let (tint, tint_alpha) = if dark {
            (vec4(0.04, 0.06, 0.09, 1.0), 0.42f32)
        } else {
            (vec4(0.97, 0.98, 1.0, 1.0), 0.30f32)
        };
        for id in [ids!(assistant_panel), ids!(layers_panel)] {
            let mut panel = self.view.widget(cx, id);
            script_apply_eval!(cx, panel, {
                draw_bg +: {
                    tint_color: #(tint)
                    tint_alpha: #(tint_alpha)
                }
            });
        }
        for id in [ids!(header_label), ids!(intro_label), ids!(status_label)] {
            let mut label = self.view.label(cx, id);
            let color = if id == ids!(header_label) { text_main } else { text_dim };
            script_apply_eval!(cx, label, {
                draw_text +: {
                    color: #(color)
                }
            });
        }
        let check_ids = [
            ids!(layer_rain),
            ids!(layer_wind),
            ids!(layer_terrain),
            ids!(tilt_check),
            ids!(cam_check),
            ids!(layer_chargers),
            ids!(layer_transit),
            ids!(layer_nature),
            ids!(layer_districts),
            ids!(layer_buildings),
            ids!(layer_demographics),
            ids!(theme_night),
            ids!(theme_circuit),
        ];
        // Every interaction state: hover/focus/down otherwise keep their
        // DSL (light-panel) colors and go unreadable on dark.
        let text_hot = if dark { vec4(1.0, 1.0, 1.0, 1.0) } else { vec4(0.0, 0.0, 0.0, 1.0) };
        for id in check_ids {
            let mut check = self.view.check_box(cx, id);
            script_apply_eval!(cx, check, {
                draw_text +: {
                    color: #(text_main)
                    color_active: #(text_main)
                    color_hover: #(text_hot)
                    color_down: #(text_hot)
                    color_focus: #(text_main)
                }
            });
        }
        let input_text = if dark { vec4(0.88, 0.91, 0.94, 1.0) } else { vec4(0.09, 0.13, 0.16, 1.0) };
        let placeholder = if dark { vec4(0.50, 0.54, 0.58, 1.0) } else { vec4(0.42, 0.47, 0.52, 1.0) };
        let mut input = self.view.text_input(cx, ids!(prompt_input));
        script_apply_eval!(cx, input, {
            draw_text +: {
                color: #(input_text)
                color_hover: #(input_text)
                color_focus: #(input_text)
                color_empty: #(placeholder)
                color_empty_hover: #(placeholder)
                color_empty_focus: #(placeholder)
            }
        });
        self.view.redraw(cx);
    }

    /// Push the layer/theme state to the MapView; lazily starts workers.
    fn apply_layers(&mut self, cx: &mut Cx) {
        self.sync_layer_checkboxes(cx);
        self.apply_ui_theme(cx);
        let map = self.map(cx);
        map.set_overlays(
            cx,
            self.provisioner
                .overlay_sources(&self.layers.overlays, &self.layers.maps_root),
        );
        map.set_theme(cx, self.layers.theme);
        let mut map_widget = self.view.widget(cx, ids!(map));
        let readout = self.layers.debug_cam;
        script_apply_eval!(cx, map_widget, {
            debug_cam: #(readout)
        });

        let bbox = nav_data::radar_display_bbox();
        if self.layers.rain {
            if let Some(radar) = &self.radar {
                if !radar.display_frames.is_empty() {
                    map.set_rain_frames(
                        cx,
                        radar.display_frames.clone(),
                        radar.display_width,
                        radar.display_height,
                        bbox,
                    );
                    map.set_rain_now_hires(cx, radar.now_hires.clone());
                }
            }
        } else {
            map.set_rain_frames(cx, Vec::new(), 0, 0, bbox);
        }

        if self.layers.wind {
            if !self.layers.wind_worker_started {
                self.layers.wind_worker_started = true;
                let cache = self
                    .paths
                    .as_ref()
                    .map(|p| p.wind_cache())
                    .unwrap_or_default();
                layers::start_wind_worker(cx.thread_spawner(), self.wind_rx.sender(), cache);
            }
            if let Some(update) = &self.layers.wind_cache {
                map.set_wind_field(
                    cx,
                    update.nx,
                    update.ny,
                    update.u.clone(),
                    update.v.clone(),
                    update.bbox,
                );
            }
        } else {
            map.set_wind_field(cx, 0, 0, Vec::new(), Vec::new(), (0.0, 0.0, 0.0, 0.0));
        }

        self.terrain_layer.set_enabled(
            cx,
            &map,
            self.layers.terrain,
            Some(&self.layers.maps_root),
        );
    }

    fn save_theme(&mut self, cx: &mut Cx) {
        if let Some(storage) = self.storage.clone() {
            self.theme_preference.save(cx, &storage, self.layers.theme);
        }
    }

    // --- actions -------------------------------------------------------------

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let ui = self.ui(cx);
        if let Some(card) = self.provisioner.card() {
            if card.can_start && self.view.button(cx, ids!(testmap_start)).clicked(actions) {
                self.provisioner.start(cx);
                self.refresh_testmap_ui(cx);
            }
            if self.view.button(cx, ids!(testmap_dismiss)).clicked(actions) {
                self.provisioner.dismiss();
                self.refresh_testmap_ui(cx);
            }
        }
        for panel_action in self.panel.actions(cx, &ui, actions) {
            let PanelAction::Search(text) = panel_action;
            self.view.text_input(cx, ids!(prompt_input)).set_text(cx, "");
            self.send_user_prompt(cx, &text);
        }
        for event in self.assistant.handle_actions(cx, &ui, actions) {
            self.on_assistant_event(cx, event);
        }
        if self.view.button(cx, ids!(layers_button)).clicked(actions) {
            self.layers_panel_open = !self.layers_panel_open;
            self.view
                .widget(cx, ids!(layers_panel))
                .set_visible(cx, self.layers_panel_open);
        }
        if self.view.button(cx, ids!(location_button)).clicked(actions) {
            self.handle_location_click(cx);
        }
        if self.view.button(cx, ids!(assistant_button)).clicked(actions) {
            self.assistant_panel_open = !self.assistant_panel_open;
            self.view
                .widget(cx, ids!(assistant_panel))
                .set_visible(cx, self.assistant_panel_open);
        }
        if overlays::handle_checkboxes(cx, &ui, actions, &mut self.layers.overlays) {
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        let layer_checks: [(&[LiveId], &str); 5] = [
            (ids!(layer_rain), "rain"),
            (ids!(layer_wind), "wind"),
            (ids!(layer_terrain), "terrain"),
            (ids!(tilt_check), "tiltshift"),
            (ids!(cam_check), "readout"),
        ];
        for (id, name) in layer_checks {
            if let Some(on) = self.view.check_box(cx, id).changed(actions) {
                let _ = self.layers.set_layer(name, on);
                self.layers.dirty = false;
                self.apply_layers(cx);
            }
        }
        // The Inception fold: a live rendering mode on the map itself, not
        // a data layer — MapView owns the tween and the close-3D gating
        // (the setting remembers intent while the camera is elsewhere).
        // Grayed = inert: CheckBox still fires Change while disabled, so
        // outside the regime the mark snaps back and nothing arms.
        if let Some(on) = self.view.check_box(cx, ids!(warp_check)).changed(actions) {
            let map = self.map(cx);
            if map.space_warp_available() {
                map.set_space_warp(cx, on);
            } else {
                self.view
                    .check_box(cx, ids!(warp_check))
                    .set_active(cx, map.space_warp(), Animate::No);
            }
        }
        if let Some(on) = self.view.check_box(cx, ids!(theme_night)).changed(actions) {
            let _ = self.layers.set_theme_name(if on { "night" } else { "light" });
            self.save_theme(cx);
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        if let Some(on) = self.view.check_box(cx, ids!(theme_circuit)).changed(actions) {
            let _ = self.layers.set_theme_name(if on { "circuit" } else { "light" });
            self.save_theme(cx);
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        let map = self.map(cx);
        if let Some(id) = map.marker_clicked(actions) {
            if let Some(name) = self.markers.name_of(id) {
                let name = name.to_string();
                self.push_line(cx, &format!("map: {name}"));
            }
        }
        if let Some((lon, lat)) = map.long_pressed(actions) {
            self.push_line(cx, &format!("map: long-press at {lon:.5}, {lat:.5}"));
        }
        if map.viewport_changed(actions).is_some() && self.layers.terrain {
            self.terrain_layer.request(cx, &map);
        }
        // '>' on a trip row: re-apply that snapshot.
        let list = self.view.portal_list(cx, ids!(list));
        for (item_id, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(apply_btn)).clicked(actions) {
                let re_applied = self
                    .chat
                    .entries
                    .get(item_id)
                    .and_then(|e| e.trip)
                    .and_then(|i| self.chat.trips.get(i).cloned());
                if let Some(trip) = re_applied {
                    self.trip = trip;
                    self.resync_trip_display(cx);
                    self.drive_log.log_trip(&self.trip.digest());
                    self.push_line(cx, "↩ trip re-applied");
                }
            }
        }
        if let Some((lon, lat, info)) = map.pin_tapped(actions) {
            let summary: Vec<String> = info
                .iter()
                .filter(|(k, _)| matches!(k.as_str(), "name" | "operator" | "max_kw" | "kind" | "city"))
                .map(|(k, v)| format!("{k}: {v}"))
                .collect();
            let text = if summary.is_empty() {
                format!("map: pin at {lon:.5},{lat:.5}")
            } else {
                format!("map: pin — {}", summary.join(", "))
            };
            self.push_line(cx, &text);
        }
    }
}

impl Widget for RouteView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.ensure_started(cx);
        if let Some(theme) = self.theme_preference.restored(event) {
            self.layers.theme = theme;
            self.layers.dirty = false;
            self.apply_layers(cx);
        }
        let map = self.map(cx);
        self.terrain_layer.handle_event(cx, event, &map);
        // The Space-warp row tracks the camera live: grayed (but visible,
        // so it stays discoverable) outside the near-first-person regime,
        // re-enabled the moment tilt + zoom qualify. Cached so the
        // animator only toggles on transitions.
        let warp_avail = map.space_warp_available();
        if self.warp_check_disabled != Some(!warp_avail) {
            self.warp_check_disabled = Some(!warp_avail);
            self.view
                .check_box(cx, ids!(warp_check))
                .set_disabled(cx, !warp_avail);
        }
        match event {
            Event::Shutdown => self.shutdown(cx),
            Event::LocationUpdate(fix) => self.on_location_update(cx, fix),
            Event::LocationError(error) => {
                let text = match error {
                    LocationErrorEvent::PermissionDenied => {
                        "gps: permission denied — using map center as position".to_string()
                    }
                    LocationErrorEvent::Unavailable(msg) => format!("gps: unavailable ({msg})"),
                };
                self.fail_location(cx, location_error_status(error));
                self.push_line(cx, &text);
            }
            // The test-map download rides the platform's HTTP stack, which
            // reports progress as the body streams. (Image search reads the
            // same responses through the brain and ignores ids that are
            // not its own, so both can listen.)
            Event::NetworkResponses(responses) => {
                let mut changed = false;
                for response in responses.iter() {
                    changed |= self.provisioner.handle_network(cx, response);
                }
                if changed {
                    self.refresh_testmap_ui(cx);
                }
            }
            _ if self.location_timeout.is_event(event).is_some()
                && self.location_state.is_waiting() =>
            {
                self.location_timeout = Timer::empty();
                self.fail_location(cx, "Location timed out — tap to retry");
            }
            _ => (),
        }
        // Esc closes the assistant popover from anywhere, regardless of
        // which widget currently owns key focus.
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::Escape && self.assistant_panel_open {
                self.close_assistant_panel(cx);
            }
        }
        if self.nav_frame.is_event(event).is_some() {
            self.tick_nav_sim(cx);
        }
        let ui = self.ui(cx);
        for assistant_event in self.assistant.handle_event(cx, &ui, event) {
            self.on_assistant_event(cx, assistant_event);
        }
        // The provisioner owns polling and adoption of the native bake.
        let map = self.map(cx);
        let provisioner_update = self.provisioner.handle_event(cx, &map);
        if let Some(basename) = provisioner_update.nav_basename {
            let chargers = self.paths.as_ref().map(|p| p.chargers()).unwrap_or_default();
            nav_data::start_nav_load(cx.task_pool(), self.nav_rx.sender(), basename, chargers);
            self.push_entry(
                cx,
                EntryKind::Info,
                "test map ready: Amsterdam tiles, routing graph and search index",
            );
        }
        if provisioner_update.changed {
            self.refresh_testmap_ui(cx);
        }
        while let Ok(load) = self.nav_rx.try_recv() {
            match load {
                NavLoad::Ready { data, stats } => {
                    self.nav = Some(*data);
                    self.push_entry(cx, EntryKind::Info, &stats);
                }
                NavLoad::Failed { error } => {
                    self.set_status(cx, &format!("nav data failed: {error}"));
                }
            }
        }
        while let Ok(radar) = self.radar_rx.try_recv() {
            self.radar = Some(radar);
            if self.layers.rain {
                self.apply_layers(cx);
            }
        }
        while let Ok(update) = self.wind_rx.try_recv() {
            self.layers.wind_cache = Some(update);
            if self.layers.wind {
                self.apply_layers(cx);
            }
        }
        // Feed the tilt-shift layer: on when the checkbox is set AND the
        // map is actually tilted (strength ramps with tilt angle).
        self.chat.tilt_shift_on = self.layers.tilt_shift;
        let tilt = self.map(cx).tilt() as f32;
        self.chat.tilt_strength = ((tilt - 5.0) / 50.0).clamp(0.0, 1.0);
        // The transcript draws from the chat state, so it rides down the
        // tree as the scope.
        self.view
            .handle_event(cx, event, &mut Scope::with_data(&mut self.chat));
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        self.view
            .draw_walk(cx, &mut Scope::with_data(&mut self.chat), walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> (Cx, WidgetRef) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            crate::side_panel::script_mod(vm);
            crate::chrome::script_mod(vm);
            script_mod(vm);
            let value = script_eval!(vm, {
                use mod.widgets.*
                RouteView {}
            });
            assert!(vm.take_errors().is_empty());
            WidgetRef::script_from_value(vm, value)
        });
        (cx, root)
    }

    #[test]
    fn the_tool_console_answers_from_the_registry_in_every_build() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<RouteView>().unwrap();
        // The camera readout is a switch in the popover, off until asked.
        assert!(!view.view.widget(&cx, ids!(cam_check)).is_empty());
        assert!(!view.layers.debug_cam);
        view.send_user_prompt(&mut cx, "/help");
        let last = view.chat.entries.last().expect("a transcript line");
        assert_eq!(last.kind, EntryKind::Info);
        assert!(last.text.starts_with("tools: ") && last.text.contains("route_plan"), "{}", last.text);
        // A prompt without a brain says so, after echoing the person.
        #[cfg(not(feature = "native"))]
        {
            view.send_user_prompt(&mut cx, "plan a trip to Utrecht");
            let n = view.chat.entries.len();
            assert_eq!(view.chat.entries[n - 2].kind, EntryKind::User);
            assert_eq!(view.chat.entries[n - 1].text, crate::assistant::UNAVAILABLE_REPLY);
        }
    }
}
