//! Desk Host as a tray app. The icon (a screen: faint when off, outlined while it serves,
//! filled while someone watches) sits in the macOS menu bar / the Windows notification area;
//! its tooltip says the same in words. A click opens the dropdown (`view::DeskHostView`, the AI
//! Hub's look): the serving switch, the viewers with their stream, who may watch (the admin Mac
//! only, by fleet identity), Open at login (an installed macOS app) and Quit.
//! The right click's menu has the switch and Quit. No window, no Dock icon (`menu_bar_app`).
//!
//! The host itself (capture, encode, the server: `makepad_desk_link::host`) runs on its own
//! thread (`crate::host_main`): the app tells it through `HostControl` and reads its `Report`
//! twice a second.

use crate::settings::Settings;
use crate::view::{DeskHostAction, DeskHostView, ViewState};
use makepad_desk_link::host::{Cmd, HostControl, HostState, Report, ViewerRow};
use makepad_desk_link_panel::tray_glyph::{image, Glyph};
use makepad_shell_kit::tray_app::{selftest_note, DropdownChange, LoginItem, TrayDropdown};
use makepad_widgets::*;
use std::sync::Arc;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            // the tray's dropdown: made on the first click, placed under the icon
            dropdown_window := Window{
                create_on_start: false
                show_caption_bar: false
                chrome: WindowChrome.None
                window.title: "Desk Host"
                window.dropdown: true
                window.inner_size: vec2(420, 420)
                body +: {
                    drop_view := mod.widgets.DeskHostView{card: true}
                }
            }
        }
    }
}

pub const TRAY: LiveId = live_id!(deskhost_tray);
const SERVING: LiveId = live_id!(serving);
const QUIT: LiveId = live_id!(quit_deskhost);
const DROPDOWN_W: f64 = 420.0;
const NAME: &str = "deskhost";

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(TrayDropdown::new(NAME, TRAY, DROPDOWN_W))]
    dropdown: TrayDropdown,
    #[rust]
    host: Option<Host>,
    #[rust]
    timer: Timer,
    #[rust]
    tray_key: String,
}

struct Host {
    ctl: Arc<HostControl>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// "Open at login" (macOS): the LaunchAgent `apps/deskhost/macos/install.sh` writes, naming the
/// installed launcher app (its identity holds the grants); offered only when this is the
/// installed host (`~/.makepad/deskhost/bin/deskhost`) and the app is there.
fn login_item() -> Option<LoginItem> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let exe = std::env::current_exe().ok()?;
    if exe != home.join(".makepad/deskhost/bin/deskhost") {
        return None;
    }
    let program = [std::path::PathBuf::from("/Applications"), home.join("Applications")].into_iter().map(|d| d.join("Desk Host.app/Contents/MacOS/Desk Host")).find(|p| p.exists())?;
    let log = home.join(".makepad/deskhost.stderr.log");
    let extra = format!(
        "    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n    <key>ThrottleInterval</key>\n    <integer>10</integer>\n    <key>ProcessType</key>\n    <string>Interactive</string>\n    <key>StandardErrorPath</key>\n    <string>{}</string>\n",
        makepad_shell_kit::tray_app::xml(&log.to_string_lossy())
    );
    Some(LoginItem { label: "nl.makepad.deskhost", program, extra })
}

fn glyph_and_status(r: &Report) -> (Glyph, String) {
    let watching: Vec<&ViewerRow> = r.viewers.iter().filter(|v| !v.paused).collect();
    match &r.state {
        HostState::Off => (Glyph::Off, "Off".to_string()),
        HostState::Waiting(why) => (Glyph::Off, format!("Waiting: {}", why.lines().next().unwrap_or(""))),
        HostState::Serving if !watching.is_empty() => (Glyph::Watched, format!("Watched by {}", watching.iter().map(|v| format!("{} ({})", v.name, v.peer)).collect::<Vec<_>>().join(", "))),
        HostState::Serving if !r.viewers.is_empty() => (Glyph::Serving, format!("Serving on {}, {} connected", r.addr, r.viewers.len())),
        HostState::Serving => (Glyph::Serving, format!("Serving on {}", r.addr)),
    }
}

impl App {
    fn report(&self) -> Report {
        self.host.as_ref().map(|h| h.ctl.report.lock().unwrap().clone()).unwrap_or_default()
    }

    fn content_height(&self, cx: &mut Cx) -> f64 {
        self.ui.widget(cx, ids!(drop_view)).borrow::<DeskHostView>().map_or(0.0, |v| v.content_height())
    }

    fn refresh(&mut self, cx: &mut Cx) {
        let report = self.report();
        let enabled = Settings::load().enabled;
        let (glyph, status) = glyph_and_status(&report);
        let key = format!("{glyph:?}|{status}|{enabled}");
        if key != self.tray_key {
            self.tray_key = key;
            let menu = vec![
                TrayMenuItem::Item { id: live_id!(status), label: status.clone(), enabled: false, checked: None },
                TrayMenuItem::Line,
                TrayMenuItem::check(SERVING, "Serving", enabled),
                TrayMenuItem::Line,
                TrayMenuItem::item(QUIT, "Quit Desk Host (stops serving)"),
            ];
            cx.set_tray(TrayIcon { id: TRAY, image: image(glyph), tooltip: format!("Desk Host \u{b7} {status}"), menu, dropdown: true });
        }
        if self.dropdown.is_open() {
            let state = ViewState { report, enabled, login: login_item().map(|i| i.is_on()), host: crate::settings::host_name() };
            if let Some(mut v) = self.ui.widget(cx, ids!(drop_view)).borrow_mut::<DeskHostView>() {
                v.set_state(cx, state);
            }
        }
    }

    fn set_serving(&mut self, cx: &mut Cx, on: bool) {
        match Settings::set_enabled(on) {
            Ok(()) => crate::log(&format!("serving switched {} from the tray", if on { "on" } else { "off" })),
            Err(e) => crate::log(&format!("serving not switched: {e}")),
        }
        selftest_note(NAME, &format!("serving {}", if on { "on" } else { "off" }));
        if let Some(h) = &self.host {
            h.ctl.send(Cmd::Settings);
        }
        self.refresh(cx);
    }

    fn quit(&mut self, cx: &mut Cx) {
        selftest_note(NAME, "quit from the tray");
        crate::log("quit from the tray");
        self.stop_host();
        cx.remove_tray(TRAY);
        cx.quit();
    }

    /// The host says goodbye to its viewers and stops capturing before the process goes.
    fn stop_host(&mut self) {
        if let Some(mut h) = self.host.take() {
            h.ctl.send(Cmd::Quit);
            if let Some(t) = h.thread.take() {
                let _ = t.join();
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // the host's thread: its desktop (capture, cursor, input) lives there
        let (ctl, wake_rx) = HostControl::new();
        let ctl = Arc::new(ctl);
        let ctl2 = ctl.clone();
        let thread = std::thread::Builder::new()
            .name("deskhost".into())
            .spawn(move || {
                #[cfg(target_os = "macos")]
                let mut desktop = crate::macos::MacDesktop::new();
                #[cfg(target_os = "windows")]
                let mut desktop = crate::windows::WinDesktop::new();
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                crate::host_main(&mut desktop, &ctl2, &wake_rx);
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let _ = (ctl2, wake_rx);
            })
            .ok();
        self.host = Some(Host { ctl, thread });
        cx.show_in_dock(false);
        self.refresh(cx);
        self.timer = cx.start_interval(0.5);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(wa) = action.as_widget_action() else { continue };
            match wa.cast::<DeskHostAction>() {
                DeskHostAction::SetServing(on) => self.set_serving(cx, on),
                DeskHostAction::Disconnect(session) => {
                    if let Some(h) = &self.host {
                        h.ctl.send(Cmd::Disconnect(session));
                    }
                }
                DeskHostAction::SetLogin(on) => match login_item().map(|i| i.set(on)) {
                    Some(Ok(())) => selftest_note(NAME, &format!("open at login {}", if on { "on" } else { "off" })),
                    Some(Err(e)) => crate::log(&format!("open at login: {e}")),
                    None => {}
                },
                DeskHostAction::Quit => return self.quit(cx),
                DeskHostAction::None => continue,
            }
            self.refresh(cx);
        }
    }
}

impl AppMain for App {
    fn menu_bar_app() -> bool {
        true
    }

    fn single_instance() -> bool {
        true
    }

    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_shell_kit::eval_default_theme(vm);
        makepad_shell_kit::script_mod(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Timer(t) if self.timer.is_timer(t).is_some() => self.refresh(cx),
            Event::TrayMenu(e) if e.tray == TRAY => match e.item {
                id if id == SERVING => {
                    let on = !Settings::load().enabled;
                    self.set_serving(cx, on);
                }
                id if id == QUIT => return self.quit(cx),
                _ => {}
            },
            Event::Shutdown => self.stop_host(),
            _ => {}
        }
        let window = self.ui.window(cx, ids!(dropdown_window));
        let content = self.content_height(cx);
        if self.dropdown.handle_event(cx, event, &window, content) == Some(DropdownChange::Opened) {
            self.refresh(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Event::Draw(_) = event {
            let content = self.content_height(cx);
            self.dropdown.fit(cx, &window, content);
        }
    }
}
