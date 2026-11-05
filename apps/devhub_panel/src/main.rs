//! The devhub control panel in a window of its own (`makepad run makepad-app-devhub-panel`):
//! the running master's fleet, read-only. `DEVHUB_PANEL_DEMO=1` shows a synthetic fleet.
//!
//! Its menu-bar item (where the desktop has a tray) shows the fleet at a glance: devhub's glyph
//! (off while the master is not read, attention while something red needs a look, busy while
//! leases run) and the counts as its tooltip. A click opens the window or brings it to the
//! front; the secondary click's menu has the status lines, the worst needs, "Open panel" and
//! "Quit panel". Closing the window keeps the panel in the tray: nothing is drawn and the
//! master is asked less often until it opens again.

pub use makepad_widgets;
use makepad_app_devhub_panel::glance::Glance;
use makepad_app_devhub_panel::panel::DevhubPanel;
use makepad_devhub_conf::tray_glyph as glyph;
use makepad_widgets::*;

app_main!(App);

const TRAY: LiveId = live_id!(devhub_panel_tray);
const OPEN: LiveId = live_id!(open_panel);
const QUIT: LiveId = live_id!(quit_panel);
/// The menu's status lines: disabled items, never chosen.
const LINE_BASE: u64 = 0x7061_6e00_0000;

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "devhub"
                window.inner_size: vec2(1440, 860)
                pass +: {clear_color: theme.color_bg_app}
                body +: {
                    padding: 0 spacing: 0
                    panel := mod.widgets.DevhubPanel{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// what the tray shows now (None: not set yet)
    #[rust]
    tray: Option<Glance>,
}

impl App {
    /// Sets the tray when the glance changed; before the first fleet it says it is connecting.
    fn set_tray(&mut self, cx: &mut Cx) {
        let glance = self.ui.widget(cx, ids!(panel)).borrow::<DevhubPanel>().and_then(|p| p.glance());
        let glance = glance.unwrap_or_else(|| Glance::of(&Default::default(), &[]));
        if self.tray.as_ref() == Some(&glance) {
            return;
        }
        let mut menu: Vec<TrayMenuItem> = glance.lines.iter().enumerate().map(|(i, l)| TrayMenuItem::Item { id: LiveId(LINE_BASE + i as u64), label: l.clone(), enabled: false, checked: None }).collect();
        menu.push(TrayMenuItem::Line);
        menu.push(TrayMenuItem::item(OPEN, "Open panel"));
        menu.push(TrayMenuItem::item(QUIT, "Quit panel"));
        cx.set_tray(TrayIcon { id: TRAY, image: TrayImage { width: glyph::SIZE, height: glyph::SIZE, alpha: glyph::alpha(glance.glyph) }, tooltip: glance.tooltip.clone(), menu, dropdown: true });
        self.tray = Some(glance);
    }

    /// Opens the window, or brings it to the front.
    fn open(&mut self, cx: &mut Cx) {
        cx.show_in_dock(true);
        self.ui.window(cx, ids!(main_window)).show(cx);
        self.set_shown(cx, true);
    }

    fn set_shown(&mut self, cx: &mut Cx, shown: bool) {
        if let Some(mut p) = self.ui.widget(cx, ids!(panel)).borrow_mut::<DevhubPanel>() {
            p.set_shown(cx, shown);
        }
    }
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // the system's appearance, light or dark, as the window system says at start
        let dark = makepad_widgets::makepad_platform::host_dark().unwrap_or(true);
        makepad_widgets::set_base_theme(vm.cx_mut(), if dark { makepad_widgets::BaseTheme::Dark } else { makepad_widgets::BaseTheme::Light });
        crate::makepad_widgets::script_mod(vm);
        makepad_app_devhub_panel::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::TrayMenu(e) if e.tray == TRAY && e.item == QUIT => return cx.quit(),
            Event::TrayMenu(e) if e.tray == TRAY && e.item == OPEN => self.open(cx),
            Event::TrayClick(e) if e.tray == TRAY => self.open(cx),
            // with the tray there, closing the window only hides it
            Event::WindowCloseRequested(e) if cx.has_tray() => {
                e.accept_close.set(false);
                self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
                cx.show_in_dock(false);
                self.set_shown(cx, false);
            }
            _ => {}
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Event::Startup | Event::Signal = event {
            self.set_tray(cx);
        }
    }
}
