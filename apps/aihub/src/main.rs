//! AI Hub: the desktop app that serves this machine's AI to the fleet while it runs.
//!
//! It runs the hub in-process (the same `makepad-ai-hub` library as the headless
//! `makepad-app-ai-hub`): LAN-only bind, fleet-credential TLS front, the sandboxed,
//! tool-less Claude backend and the local models this machine has on disk, each with
//! its own switch and inactivity turn-off. The window is the hub's control panel (the
//! same panel as the WM's top bar); the tray icon keeps it running with the window
//! closed. Quitting stops serving: no port, no beacon.

pub use makepad_widgets;

mod serve;
mod settings;
mod tray;
mod view;

use makepad_ai_hub_panel::hub_control::{parse_snapshot, HubCommand};
use makepad_widgets::*;
use serve::{Phase, Plan, Published, Serve};
use settings::{apply_idle_offs, Backend, Settings};
use view::{HubAppAction, HubAppView, ViewState};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "AI Hub"
                window.inner_size: vec2(560, 860)
                body +: {
                    hub_view := mod.widgets.HubAppView{}
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
    hub: Option<Hub>,
}

/// The app's state next to the UI: the settings, the hub owner, the clocks.
struct Hub {
    settings: Settings,
    serve: Serve,
    timer: Timer,
    /// Unix ms each backend came on (or the app launched with it on).
    on_since: [(Backend, u64); 2],
    last_seq: u64,
    pulse: bool,
    tray_key: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl Hub {
    fn start(cx: &mut Cx) -> Hub {
        let settings = Settings::load();
        let serve = Serve::start();
        serve.plan(Plan::of(&settings));
        let now = now_ms();
        Hub { settings, serve, timer: cx.start_interval(0.5), on_since: [(Backend::Claude, now), (Backend::Local, now)], last_seq: u64::MAX, pulse: false, tray_key: String::new() }
    }

    fn save_and_plan(&mut self) {
        if let Err(e) = self.settings.save() {
            log!("aihub: settings not saved: {e}");
        }
        self.serve.plan(Plan::of(&self.settings));
    }

    fn switched_on(&mut self, backend: Backend) {
        let now = now_ms();
        for (b, at) in &mut self.on_since {
            if *b == backend {
                *at = now;
            }
        }
    }

    fn set_enabled(&mut self, on: bool) {
        self.settings.enabled = on;
        if on {
            for b in Backend::ALL {
                self.switched_on(b);
            }
        }
        self.save_and_plan();
    }

    fn set_service(&mut self, backend: Backend, on: bool) {
        self.settings.set_service(backend, on);
        if on {
            self.switched_on(backend);
        }
        self.save_and_plan();
    }

    fn on_since(&self, backend: Backend) -> u64 {
        self.on_since.iter().find(|(b, _)| *b == backend).map_or(0, |(_, at)| *at)
    }

    /// The models a backend serves, for its inactivity clock.
    fn models_of(p: &Published, backend: Backend) -> Vec<String> {
        match backend {
            Backend::Claude => vec![makepad_ai_hub::claude_cli::MODEL_ID.to_string()],
            Backend::Local => p.local_models.clone(),
        }
    }

    fn last_used(p: &Published, backend: Backend) -> Option<u64> {
        let now = now_ms();
        Self::models_of(p, backend).iter().filter_map(|m| if p.working(m) { Some(now) } else { p.last_used_ms(m) }).max()
    }

    /// The inactivity turn-offs: a service idle past its limit switches off exactly like
    /// the person switching it, and says so until switched on again.
    fn idle_check(&mut self, p: &Published) -> bool {
        let on_since = self.on_since;
        let since = |b: Backend| on_since.iter().find(|(x, _)| *x == b).map_or(0, |(_, at)| *at);
        let off = apply_idle_offs(&mut self.settings, since, |b| Self::last_used(p, b), now_ms());
        for b in &off {
            log!("aihub: {} idle for {}: turned it off", b.label(), self.settings.service(*b).idle_off.label());
        }
        if !off.is_empty() {
            self.save_and_plan();
        }
        !off.is_empty()
    }

    fn off_at(&self, p: &Published) -> Vec<(Backend, u64)> {
        Backend::ALL
            .into_iter()
            .filter_map(|b| {
                let svc = self.settings.service(b);
                let limit = svc.idle_off.millis()?;
                (svc.on && self.settings.enabled).then(|| (b, Self::last_used(p, b).unwrap_or(0).max(self.on_since(b)) + limit))
            })
            .collect()
    }

    fn status(&self, p: &Published) -> (tray::Glyph, String) {
        let problem = !p.notes.is_empty();
        let working = p.control.as_ref().and_then(|c| c.get("active")).and_then(|a| a.as_arr()).is_some_and(|a| !a.is_empty());
        match &p.phase {
            Phase::Serving if working => (if self.pulse { tray::Glyph::Working } else { tray::Glyph::Serving }, "Serving a request".to_string()),
            Phase::Serving if problem => (tray::Glyph::Problem, format!("Serving, {}", p.notes[0].1.lines().next().unwrap_or(""))),
            Phase::Serving => (tray::Glyph::Serving, p.addr.map_or("Serving".to_string(), |a| format!("Serving on {a}"))),
            Phase::Starting => (tray::Glyph::Serving, "Starting".to_string()),
            Phase::Stopping => (tray::Glyph::Off, "Stopping".to_string()),
            Phase::Failed(why) => (tray::Glyph::Problem, format!("Not serving: {}", why.lines().next().unwrap_or(""))),
            Phase::Off if problem => (tray::Glyph::Problem, format!("Not serving: {}", p.notes[0].1.lines().next().unwrap_or(""))),
            Phase::Off if !self.settings.enabled => (tray::Glyph::Off, "Off".to_string()),
            Phase::Off => (tray::Glyph::Off, "Not serving: no backend is on".to_string()),
        }
    }
}

impl App {
    fn refresh(&mut self, cx: &mut Cx, force: bool) {
        let Some(hub) = self.hub.as_mut() else { return };
        let p = hub.serve.published();
        let turned_off = hub.idle_check(&p);
        hub.pulse = !hub.pulse;
        // The tray: set only when what it shows changed.
        let (glyph, status) = hub.status(&p);
        let key = format!("{glyph:?}|{status}|{}", hub.settings.render());
        if key != hub.tray_key {
            hub.tray_key = key;
            cx.set_tray(tray::icon(&hub.settings, glyph, &status));
        }
        if !(force || turned_off || p.seq != hub.last_seq || glyph == tray::Glyph::Working) {
            return;
        }
        hub.last_seq = p.seq;
        let mut snapshot = p.control.as_ref().map(parse_snapshot).unwrap_or_default();
        // The hub's own chat-model pick and Restart belong to a WM's hub child: none here.
        snapshot.process = None;
        let state = ViewState { settings: hub.settings.clone(), off_at: hub.off_at(&p), published: p, snapshot, now_ms: now_ms() };
        let view = self.ui.widget(cx, ids!(hub_view));
        if let Some(mut v) = view.borrow_mut::<HubAppView>() {
            v.set_state(cx, state);
        };
    }

    fn quit(&mut self, cx: &mut Cx) {
        if let Some(mut hub) = self.hub.take() {
            hub.serve.quit();
        }
        cx.remove_tray(tray::TRAY);
        cx.quit();
    }

    fn hub_command(&mut self, command: HubCommand) {
        let Some(hub) = self.hub.as_mut() else { return };
        match command {
            HubCommand::SetCompute(mode) => {
                hub.settings.compute = mode;
                hub.save_and_plan();
            }
            // A WM's hub child only: nothing to restart or re-pick here.
            HubCommand::Restart | HubCommand::SetModel(_) | HubCommand::LocalUse { .. } => {}
            other => {
                if let Some((path, body)) = makepad_ai_hub_panel::hub_control::command_request("", &other) {
                    hub.serve.control(&path, body.to_json().into_bytes());
                }
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.hub = Some(Hub::start(cx));
        self.refresh(cx, true);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(wa) = action.as_widget_action() else { continue };
            let Some(hub) = self.hub.as_mut() else { return };
            match wa.cast::<HubAppAction>() {
                HubAppAction::SetEnabled(on) => hub.set_enabled(on),
                HubAppAction::SetService(b, on) => hub.set_service(b, on),
                HubAppAction::SetIdleOff(b, idle) => {
                    hub.settings.service_mut(b).idle_off = idle;
                    hub.save_and_plan();
                }
                HubAppAction::Hub(command) => self.hub_command(command),
                HubAppAction::None => continue,
            }
            self.refresh(cx, true);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_shell_kit::eval_default_theme(vm);
        makepad_shell_kit::script_mod(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Timer(t) if self.hub.as_ref().is_some_and(|h| h.timer.is_timer(t).is_some()) => self.refresh(cx, false),
            Event::TrayMenu(e) if e.tray == tray::TRAY => {
                let Some(hub) = self.hub.as_mut() else { return };
                match e.item {
                    id if id == tray::OPEN => self.ui.window(cx, ids!(main_window)).set_visible(cx, true),
                    id if id == tray::ENABLED => hub.set_enabled(!hub.settings.enabled),
                    id if id == tray::CLAUDE => hub.set_service(Backend::Claude, !hub.settings.claude.on),
                    id if id == tray::LOCAL => hub.set_service(Backend::Local, !hub.settings.local.on),
                    id if id == tray::QUIT => return self.quit(cx),
                    _ => {}
                }
                self.refresh(cx, true);
            }
            Event::TrayClick(e) if e.tray == tray::TRAY => self.ui.window(cx, ids!(main_window)).set_visible(cx, true),
            // Closing the window keeps the hub in the tray where the desktop has one; where it
            // has none, closing is quitting (and quitting stops serving).
            Event::WindowCloseRequested(e) if cx.has_tray() => {
                e.accept_close.set(false);
                self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
            }
            Event::Shutdown => {
                if let Some(mut hub) = self.hub.take() {
                    hub.serve.quit();
                }
            }
            _ => {}
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
