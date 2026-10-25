//! A window plus a system tray icon whose menu shows the window, toggles a check mark and quits.
//! Closing the window hides it: the app lives on in the tray. Every tray event is logged.
//!
//! `MAKEPAD_TRAY_SELFTEST=1`: the platform shell performs the tray menu's first item itself once
//! the tray exists (macOS: `performActionForItemAtIndex:`, a `TrayMenu`; Windows: a click on the
//! icon, a `TrayClick`); the example logs the event it gets and quits.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(360, 200)
                body +: {
                    flow: Down
                    spacing: 12
                    align: Center
                    status := Label{ text: "Tray example: the menu is in the tray" }
                    hide_button := Button{ text: "Hide window" }
                }
            }
        }
    }
}

const TRAY: LiveId = live_id!(tray_example);

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    enabled: bool,
}

/// A ring with a dot, 36x36 (the @2x menu bar size).
fn glyph() -> TrayImage {
    let n = 36u32;
    let c = (n as f32 - 1.0) / 2.0;
    let alpha = (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32 - c, (i / n) as f32 - c);
            let r = (x * x + y * y).sqrt();
            let ring = 1.0 - ((r - 14.0).abs() - 2.0).clamp(0.0, 1.0);
            let dot = 1.0 - (r - 6.0).clamp(0.0, 1.0);
            (ring.max(dot) * 255.0) as u8
        })
        .collect();
    TrayImage { width: n, height: n, alpha }
}

impl App {
    fn set_tray(&self, cx: &mut Cx) {
        cx.set_tray(TrayIcon {
            id: TRAY,
            image: glyph(),
            tooltip: "Makepad tray example".into(),
            menu: vec![
                TrayMenuItem::item(live_id!(show), "Show window"),
                TrayMenuItem::check(live_id!(enabled), "Enabled", self.enabled),
                TrayMenuItem::Line,
                TrayMenuItem::item(live_id!(quit), "Quit"),
            ],
            dropdown: false,
        });
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.enabled = true;
        self.set_tray(cx);
        log!("tray: created");
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(hide_button)).clicked(actions) {
            log!("tray: hiding the window");
            self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::TrayMenu(e) => {
                log!("tray: menu item {:?} of tray {:?}", e.item, e.tray);
                if std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some() {
                    log!("tray selftest: TrayMenu {:?} received, quitting", e.item);
                    cx.quit();
                    return;
                }
                if e.item == live_id!(show) {
                    self.ui.window(cx, ids!(main_window)).set_visible(cx, true);
                } else if e.item == live_id!(enabled) {
                    self.enabled = !self.enabled;
                    self.set_tray(cx);
                    self.ui.label(cx, ids!(status)).set_text(cx, if self.enabled { "Enabled" } else { "Disabled" });
                } else if e.item == live_id!(quit) {
                    cx.quit();
                }
            }
            Event::TrayClick(e) => {
                log!("tray: click on tray {:?}", e.tray);
                if std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some() {
                    log!("tray selftest: TrayClick received, quitting");
                    cx.quit();
                    return;
                }
                self.ui.window(cx, ids!(main_window)).set_visible(cx, true);
            }
            Event::WindowCloseRequested(e) if cx.has_tray() => {
                // the window goes away, the app stays in the tray
                e.accept_close.set(false);
                log!("tray: close requested, hiding the window");
                self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
            }
            _ => {}
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
