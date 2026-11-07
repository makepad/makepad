//! A tray app, for the run's load screen of an app that never takes the run's window
//! (tools/run/fixture/tray_app.sh): it shows itself with a tray icon alone, as AI Hub does on
//! macOS and Windows, and its window is made only when it is first shown. `TRAY_APP_OPEN=<s>`:
//! it opens the window that many seconds after startup (a tray menu's "Open in window"). Where
//! the desktop shows no tray (Linux outside the makepad WM) the window is the app, at once.
pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                create_on_start: false
                window.title: "Tray app fixture"
                window.inner_size: vec2(480, 300)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        align: Center
                        Label{
                            text: "Tray app fixture"
                            draw_text.text_style.font_size: 24
                        }
                    }
                }
            }
        }
    }
}

const TRAY: LiveId = live_id!(tray_app_fixture);

/// A filled dot in a ring, 36x36 (the @2x menu bar size).
fn glyph() -> TrayImage {
    let n = 36u32;
    let c = (n as f32 - 1.0) / 2.0;
    let alpha = (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f32 - c, (i / n) as f32 - c);
            let r = (x * x + y * y).sqrt();
            let ring = 1.0 - ((r - 14.0).abs() - 2.0).clamp(0.0, 1.0);
            let dot = 1.0 - (r - 7.0).clamp(0.0, 1.0);
            (ring.max(dot) * 255.0) as u8
        })
        .collect();
    TrayImage { width: n, height: n, alpha }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    open_timer: Timer,
    /// the window was asked to show; its first draw is logged
    #[rust]
    opened: bool,
    #[rust]
    drew: bool,
}

impl App {
    fn open_window(&mut self, cx: &mut Cx, why: &str) {
        log!("tray_app: opening its window ({why})");
        self.opened = true;
        cx.show_in_dock(true);
        self.ui.window(cx, ids!(main_window)).show(cx);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Startup => {
                cx.set_tray(TrayIcon {
                    id: TRAY,
                    image: glyph(),
                    tooltip: "Tray app fixture".into(),
                    menu: vec![TrayMenuItem::item(live_id!(open), "Open in window"), TrayMenuItem::Line, TrayMenuItem::item(live_id!(quit), "Quit")],
                    dropdown: false,
                });
                if cx.has_tray() {
                    log!("tray_app: in the tray, no window");
                    cx.show_in_dock(false);
                    if let Some(s) = std::env::var("TRAY_APP_OPEN").ok().and_then(|s| s.parse::<f64>().ok()) {
                        self.open_timer = cx.start_timeout(s);
                    }
                } else {
                    self.open_window(cx, "this desktop shows no tray");
                }
            }
            Event::TrayMenu(e) if e.item == live_id!(open) => self.open_window(cx, "its tray menu"),
            Event::TrayMenu(e) if e.item == live_id!(quit) => cx.quit(),
            Event::Draw(_) if self.opened && !self.drew => {
                self.drew = true;
                log!("tray_app: its window drew");
            }
            _ => {}
        }
        if self.open_timer.is_event(event).is_some() {
            self.open_window(cx, "TRAY_APP_OPEN");
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
