//! browser: a Chrome-like browser as a plain full-window Makepad app.
//! CEF renders the page (GPU-accelerated into a shared IOSurface texture);
//! every bit of chrome is Makepad. Runs standalone or inside makepad-wm /
//! Studio tiles via the shared --stdin-loop client runtime.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_cef::BootstrapResult;
use makepad_widgets::*;

mod ai;
mod chrome;
mod tabs;
mod theme;
mod webview;

use chrome::{TabStrip, TabStripAction};
use tabs::TabId;
use std::cell::RefCell;
use std::rc::Rc;
use theme::Palette;
use webview::{WebView, WebViewAction};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 860)
                window.title: "browser"
                body +: {
                    flow: Overlay
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        tab_strip := TabStrip{}
                        toolbar := MpToolbar{}
                        webview := WebView{}
                    }
                    // The ⋮ menu: a square panel under the button.
                    menu_layer := View{
                        width: Fill
                        height: Fill
                        flow: Right
                        align: Align{x: 1.0 y: 0.0}
                        padding: Inset{top: 78 right: 6 left: 0 bottom: 0}
                        menu := SolidView{
                            visible: false
                            width: 240
                            height: Fit
                            flow: Down
                            padding: Inset{top: 4 bottom: 4 left: 0 right: 0}
                            draw_bg +: {
                                color: mod.browser_theme.background
                            }
                            menu_new_tab := MpMenuItem{text: "New tab"}
                            menu_close_tab := MpMenuItem{text: "Close tab"}
                            menu_reload := MpMenuItem{text: "Reload"}
                            Hr{}
                            menu_gpu := MpMenuItem{text: "chrome://gpu"}
                            menu_about := MpMenuItem{text: "About browser"}
                        }
                    }
                }
            }
        }
    }
}

thread_local! {
    static PALETTE: RefCell<Option<Palette>> = const { RefCell::new(None) };
}

static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Milliseconds since `main` started (startup measurements).
pub fn uptime_ms() -> u128 {
    START.get_or_init(std::time::Instant::now).elapsed().as_millis()
}

/// Microseconds since `main` started (resize tracing).
pub fn uptime_us() -> u128 {
    START.get_or_init(std::time::Instant::now).elapsed().as_micros()
}

fn palette() -> Palette {
    PALETTE.with(|p| {
        p.borrow_mut()
            .get_or_insert_with(Palette::current)
            .clone()
    })
}

/// Cmd shortcuts the browser chrome owns (never forwarded to the page).
pub fn is_app_shortcut(key_event: &KeyEvent) -> bool {
    if !key_event.modifiers.logo {
        return false;
    }
    matches!(
        key_event.key_code,
        KeyCode::KeyT
            | KeyCode::KeyW
            | KeyCode::KeyL
            | KeyCode::KeyR
            | KeyCode::KeyN
            | KeyCode::LBracket
            | KeyCode::RBracket
            | KeyCode::Key1
            | KeyCode::Key2
            | KeyCode::Key3
            | KeyCode::Key4
            | KeyCode::Key5
            | KeyCode::Key6
            | KeyCode::Key7
            | KeyCode::Key8
            | KeyCode::Key9
    )
}

/// URLs given on the command line (everything that is not a flag).
fn initial_urls() -> Vec<String> {
    let mut urls = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--cwd" || arg == "--message-format" {
            let _ = args.next();
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        urls.push(tabs::resolve_omnibox(&arg));
    }
    urls
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    omnibox_focused: bool,
    #[rust]
    shown_url: String,
    #[rust]
    shown_tab: Option<TabId>,
    #[rust]
    menu_open: bool,
    #[rust]
    reported_mode: String,
    #[rust]
    focus_omnibox_pending: bool,
    #[rust]
    focus_frame: NextFrame,
    #[rust]
    focus_retries: u32,
    #[rust]
    ai_port: Option<AiServicePort>,
    #[rust]
    ai_context: String,
}

struct AiBrowserTarget<'a> {
    cx: &'a mut Cx,
    webview: &'a mut WebView,
}

impl ai::BrowserTarget for AiBrowserTarget<'_> {
    fn page(&self) -> Option<ai::PageState> {
        let info = self.webview.active_info();
        info.id.map(|_| ai::PageState {
            title: info.title,
            url: info.url,
        })
    }

    fn tabs(&self) -> Vec<ai::TabState> {
        self.webview
            .ai_tabs()
            .into_iter()
            .map(|(title, url, active)| ai::TabState { title, url, active })
            .collect()
    }

    fn navigate(&mut self, url: &str) -> bool {
        if self.webview.active_id().is_none() {
            return false;
        }
        self.webview.navigate(self.cx, url);
        true
    }

    fn new_tab(&mut self, url: &str) {
        self.webview.new_tab(self.cx, url, true);
    }
}

impl App {
    fn with_webview<R>(&self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut WebView) -> R) -> Option<R> {
        let webview = self.ui.widget(cx, ids!(webview));
        let mut inner = webview.borrow_mut::<WebView>()?;
        Some(f(cx, &mut inner))
    }

    fn new_tab(&mut self, cx: &mut Cx, url: Option<String>) {
        let url = url.unwrap_or_else(|| palette().new_tab_url());
        let is_ntp = theme::is_new_tab_url(&url);
        self.with_webview(cx, |cx, wv| {
            wv.new_tab(cx, &url, true);
        });
        self.refresh_chrome(cx);
        if is_ntp {
            self.focus_omnibox(cx);
        }
    }

    fn close_active_tab(&mut self, cx: &mut Cx) {
        let closed_last = self
            .with_webview(cx, |cx, wv| {
                if let Some(id) = wv.active_id() {
                    wv.close_tab(cx, id);
                }
                wv.tab_count() == 0
            })
            .unwrap_or(false);
        if closed_last {
            self.new_tab(cx, None);
        } else {
            self.refresh_chrome(cx);
        }
    }

    fn focus_omnibox(&mut self, cx: &mut Cx) {
        let omnibox = self.ui.text_input(cx, ids!(omnibox));
        omnibox.set_key_focus(cx);
        omnibox.borrow_mut().map(|mut inner| inner.select_all(cx));
        // A focus set while a mouse click is still being dispatched (the +
        // button, a tab close) does not survive the rest of that click; the
        // key-path (Cmd+L) proves the focus itself works. Re-assert it on the
        // next frame.
        self.focus_omnibox_pending = true;
        self.focus_frame = cx.new_next_frame();
    }

    fn set_menu_open(&mut self, cx: &mut Cx, open: bool) {
        if self.menu_open == open {
            return;
        }
        self.menu_open = open;
        self.ui.view(cx, ids!(menu)).set_visible(cx, open);
        self.ui.redraw(cx);
    }

    /// Push the model into the chrome: tab strip, omnibox text (unless the
    /// user is typing in it), nav button states, window title.
    fn refresh_chrome(&mut self, cx: &mut Cx) {
        let (summaries, info) = self
            .with_webview(cx, |_cx, wv| (wv.summaries(), wv.active_info()))
            .unwrap_or_default();

        self.refresh_ai_context(&info);

        if let Some(mut strip) = self.ui.widget(cx, ids!(tab_strip)).borrow_mut::<TabStrip>() {
            strip.set_tabs(cx, summaries);
        }

        let shown = if theme::is_new_tab_url(&info.url) {
            String::new()
        } else {
            info.url.clone()
        };
        // The omnibox follows the page unless the user is typing in it — but
        // switching tabs always replaces what it shows.
        let tab_changed = info.id != self.shown_tab;
        self.shown_tab = info.id;
        if shown != self.shown_url || tab_changed {
            self.shown_url = shown.clone();
            if !self.omnibox_focused || tab_changed {
                self.ui.text_input(cx, ids!(omnibox)).set_text(cx, &shown);
            }
        }

        let palette = palette();
        let on = theme::parse_hex(&palette.foreground).unwrap_or_default();
        let off = theme::parse_hex(&palette.muted).unwrap_or_default();
        let mut back = self.ui.button(cx, ids!(back_btn));
        let back_color = if info.can_go_back { on } else { off };
        script_apply_eval!(cx, back, {
            draw_icon +: {
                color: #(back_color)
            }
        });
        let mut forward = self.ui.button(cx, ids!(forward_btn));
        let forward_color = if info.can_go_forward { on } else { off };
        script_apply_eval!(cx, forward, {
            draw_icon +: {
                color: #(forward_color)
            }
        });

        let title = if info.title.is_empty() {
            "browser".to_string()
        } else {
            format!("{} — browser", info.title)
        };
        self.ui.window(cx, ids!(main_window)).set_title(cx, &title);
        // The bar shows the page title too when the window manager hosts us.
        makepad_wm_api::set_title(cx, &title);

        if info.render_mode != self.reported_mode && info.render_mode != "None" {
            self.reported_mode = info.render_mode.clone();
            log!(
                "browser: page rendering is {} (accelerated frames so far: {}, last blit {}us)",
                info.render_mode,
                info.accelerated_frames,
                info.last_blit_micros
            );
        }
        self.ui.redraw(cx);
    }

    fn refresh_ai_context(&mut self, info: &webview::ActiveInfo) {
        let context = if info.id.is_some() {
            format!("active tab: {} — {}", info.title, info.url)
        } else {
            "no active tab".to_string()
        };
        if context == self.ai_context {
            return;
        }
        self.ai_context = context.clone();
        if let Some(port) = self.ai_port.as_ref() {
            port.set_context(&context);
        }
    }

    fn refresh_ai_context_from_webview(&mut self, cx: &mut Cx) {
        let info = self
            .with_webview(cx, |_cx, webview| webview.active_info())
            .unwrap_or_default();
        self.refresh_ai_context(&info);
    }

    fn drain_ai_port(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => return,
        };
        for event in events {
            match event {
                PortEvent::Registered(endpoint) => {
                    log!("browser: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context_from_webview(cx);
                }
                PortEvent::Call(call) => {
                    let result = self
                        .with_webview(cx, |cx, webview| {
                            let mut target = AiBrowserTarget { cx, webview };
                            ai::answer(&call, &mut target)
                        })
                        .unwrap_or_else(|| {
                            makepad_ai_services::wire::ToolResult::unavailable(
                                &call.call_id,
                                "the browser view is not ready",
                            )
                        });
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                    self.refresh_chrome(cx);
                }
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
                PortEvent::Subscribe { .. } | PortEvent::Unsubscribe { .. } => {}
            }
        }
    }

    fn navigate_from_omnibox(&mut self, cx: &mut Cx, text: &str) {
        let url = tabs::resolve_omnibox(text);
        if url.is_empty() {
            return;
        }
        self.with_webview(cx, |cx, wv| {
            if wv.tab_count() == 0 {
                wv.new_tab(cx, &url, true);
            } else {
                wv.navigate(cx, &url);
            }
        });
        self.refresh_chrome(cx);
    }

    fn handle_shortcut(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        // Ctrl+Tab / Ctrl+Shift+Tab cycle tabs (Chrome), as do Cmd+Shift+] / [.
        if ke.modifiers.control && ke.key_code == KeyCode::Tab {
            let delta = if ke.modifiers.shift { -1 } else { 1 };
            self.with_webview(cx, |cx, wv| wv.activate_offset(cx, delta));
            self.refresh_chrome(cx);
            return true;
        }
        if !ke.modifiers.logo {
            return false;
        }
        if ke.modifiers.shift
            && matches!(ke.key_code, KeyCode::LBracket | KeyCode::RBracket)
        {
            let delta = if ke.key_code == KeyCode::LBracket { -1 } else { 1 };
            self.with_webview(cx, |cx, wv| wv.activate_offset(cx, delta));
            self.refresh_chrome(cx);
            return true;
        }
        match ke.key_code {
            KeyCode::KeyT | KeyCode::KeyN => self.new_tab(cx, None),
            KeyCode::KeyW => self.close_active_tab(cx),
            KeyCode::KeyL => self.focus_omnibox(cx),
            KeyCode::KeyR => {
                self.with_webview(cx, |cx, wv| wv.reload(cx));
            }
            KeyCode::LBracket => {
                self.with_webview(cx, |cx, wv| wv.go_back(cx));
            }
            KeyCode::RBracket => {
                self.with_webview(cx, |cx, wv| wv.go_forward(cx));
            }
            KeyCode::Key1
            | KeyCode::Key2
            | KeyCode::Key3
            | KeyCode::Key4
            | KeyCode::Key5
            | KeyCode::Key6
            | KeyCode::Key7
            | KeyCode::Key8 => {
                let index = match ke.key_code {
                    KeyCode::Key1 => 0,
                    KeyCode::Key2 => 1,
                    KeyCode::Key3 => 2,
                    KeyCode::Key4 => 3,
                    KeyCode::Key5 => 4,
                    KeyCode::Key6 => 5,
                    KeyCode::Key7 => 6,
                    _ => 7,
                };
                self.with_webview(cx, |cx, wv| wv.activate_index(cx, index));
                self.refresh_chrome(cx);
            }
            KeyCode::Key9 => {
                self.with_webview(cx, |cx, wv| {
                    let last = wv.tab_count().saturating_sub(1);
                    wv.activate_index(cx, last);
                });
                self.refresh_chrome(cx);
            }
            _ => return false,
        }
        true
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // A warm-pool standby is not a running Browser: its service opens on
        // `WmEvent::Adopted`, never while dormant — the assistant must not
        // steer a page nobody can see.
        if !makepad_wm_api::warm_start() {
            self.ai_port = AiServicePort::open(cx, ai::manifest());
        }
        match makepad_cef::startup_phases() {
            Some((bundle_ms, exec_gap_ms)) => log!(
                "browser: window up at {} ms after main (app bundle prepared in {} ms, exec-to-main gap {} ms)",
                uptime_ms(),
                bundle_ms,
                exec_gap_ms
            ),
            None => log!("browser: window up at {} ms after main", uptime_ms()),
        }
        let urls = initial_urls();
        if urls.is_empty() {
            // Never boot empty: the first tab opens the web (Cmd+T tabs
            // still get the themed New Tab page).
            self.new_tab(cx, Some("https://www.google.com/".to_string()));
        } else {
            for url in urls {
                self.new_tab(cx, Some(url));
            }
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Tab strip.
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            match action.cast::<TabStripAction>() {
                TabStripAction::Activate(id) => {
                    self.with_webview(cx, |cx, wv| wv.activate(cx, id));
                    self.refresh_chrome(cx);
                }
                TabStripAction::Close(id) => {
                    let closed_last = self
                        .with_webview(cx, |cx, wv| {
                            wv.close_tab(cx, id);
                            wv.tab_count() == 0
                        })
                        .unwrap_or(false);
                    if closed_last {
                        self.new_tab(cx, None);
                    } else {
                        self.refresh_chrome(cx);
                    }
                }
                TabStripAction::New => self.new_tab(cx, None),
                TabStripAction::None => {}
            }
            if let WebViewAction::TabsChanged = action.cast::<WebViewAction>() {
                self.refresh_chrome(cx);
            }
        }

        // Toolbar.
        if self.ui.button(cx, ids!(back_btn)).clicked(actions) {
            self.with_webview(cx, |cx, wv| wv.go_back(cx));
        }
        if self.ui.button(cx, ids!(forward_btn)).clicked(actions) {
            self.with_webview(cx, |cx, wv| wv.go_forward(cx));
        }
        if self.ui.button(cx, ids!(reload_btn)).clicked(actions) {
            // Chrome semantics: the button stops a page that is still loading.
            self.with_webview(cx, |cx, wv| {
                if wv.active_info().loading {
                    wv.stop(cx);
                } else {
                    wv.reload(cx);
                }
            });
        }
        if self.ui.button(cx, ids!(star_btn)).clicked(actions) {
            // Bookmarks are not wired yet; the star just re-focuses the page.
            self.with_webview(cx, |cx, wv| wv.focus(cx));
        }
        if self.ui.button(cx, ids!(menu_btn)).clicked(actions) {
            let open = !self.menu_open;
            self.set_menu_open(cx, open);
        }

        // Menu.
        if self.ui.button(cx, ids!(menu_new_tab)).clicked(actions) {
            self.set_menu_open(cx, false);
            self.new_tab(cx, None);
        }
        if self.ui.button(cx, ids!(menu_close_tab)).clicked(actions) {
            self.set_menu_open(cx, false);
            self.close_active_tab(cx);
        }
        if self.ui.button(cx, ids!(menu_reload)).clicked(actions) {
            self.set_menu_open(cx, false);
            self.with_webview(cx, |cx, wv| wv.reload(cx));
        }
        if self.ui.button(cx, ids!(menu_gpu)).clicked(actions) {
            self.set_menu_open(cx, false);
            self.new_tab(cx, Some("chrome://gpu".to_string()));
        }
        if self.ui.button(cx, ids!(menu_about)).clicked(actions) {
            self.set_menu_open(cx, false);
            let info = self
                .with_webview(cx, |_cx, wv| wv.active_info())
                .unwrap_or_default();
            let html = format!(
                "<!doctype html><title>About browser</title><body style=\"background:{bg};color:{fg};font:14px -apple-system,Helvetica,sans-serif;padding:32px\">\
                 <h2 style=\"font-weight:500\">browser</h2>\
                 <p>Makepad chrome, Chromium Embedded Framework {cef} page rendering.</p>\
                 <p>Page rendering path: <b>{mode}</b> (accelerated frames: {frames}, last GPU blit: {blit}µs)</p>\
                 <p>ANGLE backend: {angle}</p></body>",
                bg = palette().darker_background,
                fg = palette().foreground,
                cef = makepad_cef::CEF_VERSION,
                mode = info.render_mode,
                frames = info.accelerated_frames,
                blit = info.last_blit_micros,
                angle = std::env::var("MAKEPAD_CEF_USE_ANGLE").unwrap_or_else(|_| "default".into()),
            );
            let url = format!("data:text/html;charset=utf-8,{}", theme::percent_encode(&html));
            self.new_tab(cx, Some(url));
        }

        // Omnibox.
        let omnibox = self.ui.text_input(cx, ids!(omnibox));
        if let Some((text, _modifiers)) = omnibox.returned(actions) {
            self.navigate_from_omnibox(cx, &text);
        }
        if omnibox.escaped(actions) {
            let shown = self.shown_url.clone();
            omnibox.set_text(cx, &shown);
            self.with_webview(cx, |cx, wv| wv.focus(cx));
        }
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            if widget_action.widget_uid != omnibox.widget_uid() {
                continue;
            }
            match widget_action.cast::<TextInputAction>() {
                TextInputAction::KeyFocus => {
                    self.omnibox_focused = true;
                    if let Some(mut inner) = omnibox.borrow_mut() {
                        inner.select_all(cx);
                    }
                }
                TextInputAction::KeyFocusLost => {
                    if std::env::var_os("MAKEPAD_CEF_DEBUG").is_some() {
                        let now = cx.keyboard.key_focus();
                        let webview = self.ui.widget(cx, ids!(webview)).area();
                        log!(
                            "omnibox lost key focus to {}",
                            if now == webview { "the webview" } else if now == Area::Empty { "nothing (Area::Empty)" } else { "another widget" }
                        );
                    }
                    self.omnibox_focused = false;
                    let shown = self.shown_url.clone();
                    omnibox.set_text(cx, &shown);
                }
                _ => {}
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        // The family theme bridge retints the stock widgets from the WM
        // theme; the chrome roles go into mod.browser_theme.
        makepad_wm_theme::apply(vm);
        palette().apply(vm);
        chrome::script_mod(vm);
        webview::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // The window manager asked politely (SUPER+W): go now, ahead of the
        // kill that follows its grace. A warm-pool browser needs no waking —
        // CEF paints nothing until a tile presents it.
        if let Event::Custom(json) = event {
            match makepad_wm_api::WmEvent::parse(json) {
                Some(makepad_wm_api::WmEvent::Adopted) if self.ai_port.is_none() => {
                    self.ai_port = AiServicePort::open(cx, ai::manifest());
                }
                _ => {}
            }
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        self.drain_ai_port(cx, event);
        if let Event::KeyDown(ke) = event {
            if self.handle_shortcut(cx, ke) {
                return;
            }
        }
        if self.focus_omnibox_pending && self.focus_frame.is_event(event).is_some() {
            let omnibox = self.ui.text_input(cx, ids!(omnibox));
            if omnibox.area().is_valid(cx) || self.focus_retries >= 60 {
                self.focus_omnibox_pending = false;
                self.focus_retries = 0;
                omnibox.set_key_focus(cx);
                omnibox.borrow_mut().map(|mut inner| inner.select_all(cx));
            } else {
                // At startup the first tab arrives before the chrome has been
                // drawn: no area to focus yet, so try again next frame.
                self.focus_retries += 1;
                self.focus_frame = cx.new_next_frame();
            }
        }
        if let Event::MouseDown(_) = event {
            if self.menu_open {
                // Any click outside the menu closes it; the menu's own
                // buttons still get the event through the ui below.
                let menu = self.ui.view(cx, ids!(menu)).area();
                if let Event::MouseDown(md) = event {
                    if !(menu.is_valid(cx) && menu.rect(cx).contains(md.abs)) {
                        self.set_menu_open(cx, false);
                    }
                }
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

fn main() {
    app_main();
}

/// The CEF-aware entry point: helper-process bootstrap (`cef_execute_process`),
/// the app-bundle re-exec macOS needs for Chromium's subprocesses, then the
/// ordinary Makepad event loop with the `--remote` control surface.
#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
pub fn app_main() {
    let _ = uptime_ms();
    if let Err(err) = makepad_cef::reexec_into_app_bundle_if_needed() {
        panic!("CEF bundle re-exec failed: {err}");
    }
    match makepad_cef::bootstrap() {
        Ok(BootstrapResult::Continue) => {}
        Ok(BootstrapResult::Exit(code)) => std::process::exit(code),
        Err(err) => panic!("CEF bootstrap failed: {err}"),
    }

    Cx::init_log();
    if Cx::pre_start() {
        return;
    }
    // Chromium composites the page on this colour. Left at CEF's default the
    // first frames of every page are BLACK — a dark dip then a bright jump on
    // open, and a black margin wherever a resize outruns the reflow.
    makepad_cef::set_background_color(palette().page_background_argb());
    // Only the cheap NSApp/pump preparation here: the window goes up first,
    // the WebView runs `cef_initialize` on the frame after it is drawn.
    if let Err(err) = makepad_cef::prepare() {
        panic!("CEF prepare failed: {err}");
    }

    let cx = Rc::new(RefCell::new(Cx::new(
        makepad_widgets::_app_main_event_closure!(App),
    )));
    let studio_http = makepad_widgets::resolve_studio_http();
    cx.borrow_mut().init_websockets(&studio_http);
    if makepad_widgets::should_run_stdin_loop_from_env() {
        cx.borrow_mut().in_makepad_studio = true;
    }
    cx.borrow_mut().init_cx_os();
    makepad_widgets::makepad_platform::remote::start_if_requested();
    Cx::event_loop(cx.clone());
    drop(cx);
    makepad_cef::shutdown();
}

#[cfg(any(target_arch = "wasm32", target_os = "android", target_env = "ohos"))]
pub fn app_main() {
    panic!("browser is desktop-only");
}
