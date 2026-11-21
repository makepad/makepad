//! makepad-app-party: the party's big screen. It runs the party itself (the
//! service guests' phones reach, the store, the hub worker) and shows the
//! wall: every loop and photo guests put up, redrawn by the box, with the
//! code to join beside them.
//!
//! Options: `--port N` (default 8443), `--code C` (default: a new party),
//! `--name X` (another name or address guests reach the box at,
//! repeatable), `--hub URL` (default: the box's own hub).

pub use makepad_widgets;
use makepad_app_party::start::{self, Running};
use makepad_widgets::*;

mod screen;

use screen::wall::{PartyWall, PartyWallAction};

app_main!(App, font_assets: [INTER_FONT_ASSET, ROBOTO_FLEX_FONT_ASSET]);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Party"
                window.inner_size: vec2(1600, 900)
                pass +: {clear_color: #000}
                body +: {
                    padding: 0
                    spacing: 0
                    flow: Overlay
                    wall := PartyWall{}
                    // The prompt set now, small and quiet at the top over a faint shade
                    // (clear of the QR at the top right).
                    now_bar := View{
                        visible: false
                        width: Fill
                        height: 64
                        align: Align{x: 0.5 y: 0.0}
                        padding: Inset{top: 14 left: 240 right: 240}
                        show_bg: true
                        draw_bg +: {
                            pixel: fn() {
                                return vec4(0.0, 0.0, 0.0, 0.3 * (1.0 - self.pos.y))
                            }
                        }
                        now_prompt := Label{
                            text: ""
                            draw_text +: {color: #ffffffa8 text_style: theme.font_regular{font_size: 12}}
                        }
                    }
                    // The prompt the live redraw uses: a phone's, or typed here.
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        align: Align{x: 0.5 y: 1.0}
                        padding: Inset{bottom: 28}
                        spacing: 8
                        // What a guest is typing on their phone, dimmed, until they send it.
                        draft := Label{
                            text: ""
                            draw_text +: {color: #ffffffa0 text_style: theme.font_regular{font_size: 14}}
                        }
                        View{
                            width: Fit
                            height: Fit
                            flow: Right
                            spacing: 8
                            align: Align{y: 0.5}
                            prev := ButtonFlat{text: "‹" width: 48 height: 48 align: Align{x: 0.5 y: 0.5} draw_text +: {color: #fff text_style: theme.font_regular{font_size: 24}}}
                            prompt := TextInput{
                                width: 560
                                height: Fit
                                padding: Inset{left: 20 right: 20 top: 12 bottom: 12}
                                empty_text: "Type a prompt, press Enter"
                                draw_bg +: {
                                    color: #0000008c
                                    color_hover: #000000a6
                                    color_focus: #000000bf
                                    color_empty: #0000008c
                                    border_size: 0.0
                                    border_radius: 22.0
                                }
                                draw_text +: {color: #fff text_style: theme.font_regular{font_size: 14}}
                            }
                            next := ButtonFlat{text: "›" width: 48 height: 48 align: Align{x: 0.5 y: 0.5} draw_text +: {color: #fff text_style: theme.font_regular{font_size: 24}}}
                            // While a phone has the screen: the box's own camera takes it back.
                            use_camera := ButtonFlat{
                                visible: false
                                text: "Use my camera"
                                height: 48
                                padding: Inset{left: 16 right: 16}
                                draw_text +: {color: #fff text_style: theme.font_regular{font_size: 13}}
                            }
                        }
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
    /// The party this screen runs; its service stops with the app.
    #[rust]
    running: Option<Running>,
    /// Looks at the live prompt a few times a second.
    #[rust]
    prompt_tick: Timer,
    /// The wish count the prompt box last showed.
    #[rust]
    prompt_seen: u64,
    /// Someone at the box is typing: phones' prompts wait until they're done.
    #[rust]
    prompt_editing: bool,
    /// The draft line as last shown.
    #[rust]
    draft_shown: String,
    /// Stepping through the prompt history with ‹ ›: the history as it
    /// was at the first step, and where in it.
    #[rust]
    history: Vec<String>,
    #[rust]
    history_at: Option<usize>,
    /// The box's own camera, shown redrawn while no guest is live.
    #[rust]
    box_camera: screen::camera::BoxCamera,
    /// "Use my camera" is shown.
    #[rust]
    use_camera_shown: bool,
    /// The prompt shown at the top.
    #[rust]
    now_shown: String,
}

struct Options {
    port: u16,
    code: Option<String>,
    names: Vec<String>,
    hub: Option<String>,
}

fn options() -> Options {
    let mut options = Options { port: 8443, code: None, names: Vec::new(), hub: None };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => match args.next().and_then(|p| p.parse().ok()) {
                Some(port) => options.port = port,
                None => log!("party: --port needs a number"),
            },
            "--code" => options.code = args.next(),
            "--name" => options.names.extend(args.next()),
            "--hub" => options.hub = args.next(),
            // The window manager and the launchers add their own.
            _ => {}
        }
    }
    options
}

impl App {
    /// The newest prompt (a phone's or the box's) into the prompt box,
    /// unless someone at the box is typing.
    fn show_prompt(&mut self, cx: &mut Cx) {
        let Some(running) = &self.running else { return };
        let (count, text) = running.party.live.wish_text();
        if count != self.prompt_seen && !self.prompt_editing {
            self.prompt_seen = count;
            self.ui.widget(cx, ids!(prompt)).set_text(cx, text.as_deref().unwrap_or(""));
        }
        let now = text.as_deref().map(one_line).unwrap_or_default();
        if now != self.now_shown {
            self.ui.widget(cx, ids!(now_prompt)).set_text(cx, &now);
            self.ui.widget(cx, ids!(now_bar)).set_visible(cx, !now.is_empty());
            self.now_shown = now;
        }
        let offer = running.party.live.box_camera_offer();
        if offer != self.use_camera_shown {
            self.use_camera_shown = offer;
            self.ui.widget(cx, ids!(use_camera)).set_visible(cx, offer);
        }
        let draft = running.party.live.draft_text(DRAFT_SHOWN).map(|(_, text)| text).unwrap_or_default();
        if draft != self.draft_shown {
            self.ui.widget(cx, ids!(draft)).set_text(cx, &draft);
            self.draft_shown = draft;
        }
    }

    /// One step through the party's prompt history (`older`: back), made
    /// the current prompt.
    fn step_history(&mut self, cx: &mut Cx, older: bool) {
        let Some(running) = &self.running else { return };
        if self.history_at.is_none() {
            self.history = running.party.live.history();
        }
        if self.history.is_empty() {
            return;
        }
        let at = match (self.history_at, older) {
            (None, true) => 1.min(self.history.len() - 1),
            (None, false) => 0,
            (Some(at), true) => (at + 1).min(self.history.len() - 1),
            (Some(at), false) => at.saturating_sub(1),
        };
        self.history_at = Some(at);
        let text = self.history[at].clone();
        self.ui.widget(cx, ids!(prompt)).set_text(cx, &text);
        self.prompt_editing = false;
        if let Err(e) = running.party.live.host_wish(Some(text)) {
            log!("party: the box's prompt was not taken: {e:?}");
        }
        // The wish just set is shown already.
        self.prompt_seen = running.party.live.wish_text().0;
    }
}

/// `text` on one line for the top of the screen, a long one cut with an ellipsis.
fn one_line(text: &str) -> String {
    const MOST: usize = 90;
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(MOST) {
        Some((at, _)) => format!("{}…", line[..at].trim_end()),
        None => line,
    }
}

/// Seconds a phone's draft stays on the screen after its last keystroke.
const DRAFT_SHOWN: f64 = 8.0;

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // A double click anywhere on the picture maximises the window, and
        // restores it (out of full screen too), as a caption's would.
        let wall = self.ui.widget(cx, ids!(wall)).widget_uid();
        if actions.find_widget_action(wall).is_some_and(|a| matches!(a.cast::<PartyWallAction>(), PartyWallAction::DoubleClicked)) {
            let window = self.ui.window(cx, ids!(main_window));
            let fullscreen = window.window_id().is_some_and(|id| cx.hosted_fullscreen(id)) || window.is_fullscreen(cx);
            if fullscreen || window.is_maximized(cx) {
                window.restore(cx);
            } else {
                window.maximize(cx);
            }
        }
        if self.ui.button(cx, ids!(use_camera)).clicked(actions) {
            if let Some(running) = &self.running {
                running.party.live.box_takes();
                log!("party: the box took the screen for its camera");
            }
        }
        if self.ui.button(cx, ids!(prev)).clicked(actions) {
            self.step_history(cx, true);
        }
        if self.ui.button(cx, ids!(next)).clicked(actions) {
            self.step_history(cx, false);
        }
        let prompt = self.ui.text_input(cx, ids!(prompt));
        if prompt.changed(actions).is_some() {
            self.prompt_editing = true;
            self.history_at = None;
        }
        if let Some((text, _)) = prompt.returned(actions) {
            self.prompt_editing = false;
            self.history_at = None;
            if let Some(running) = &self.running {
                let wish = Some(text.trim().to_string()).filter(|t| !t.is_empty());
                match running.party.live.host_wish(wish) {
                    Ok(()) => log!("party: the box's prompt: {}", text.trim()),
                    Err(e) => log!("party: the box's prompt was not taken: {e:?}"),
                }
            }
        }
        if prompt.escaped(actions) || prompt.key_focus_lost(actions) {
            self.prompt_editing = false;
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        makepad_wm_api::set_title(cx, "Party");
        let options = options();
        let started = start::party(options.port, options.code, &options.names, options.hub.as_deref());
        let wall_ref = self.ui.widget(cx, ids!(wall));
        let Some(mut wall) = wall_ref.borrow_mut::<PartyWall>() else { return };
        match started {
            Ok(running) => {
                for url in &running.urls {
                    log!("party: guests open {url}");
                }
                if let Some(url) = running.urls.first() {
                    log!("party: the screen for a TV or projector: {url}/screen");
                }
                log!("party: certificate SHA-256 {}", running.fingerprint);
                let join = running.urls.first().cloned().unwrap_or_else(|| format!("port {}, code {}", running.server.port, running.code));
                wall.set_party(cx, running.party.clone(), join);
                self.running = Some(running);
                self.prompt_tick = cx.start_interval(0.25);
            }
            Err(e) => {
                log!("party: {e}");
                wall.set_message(cx, format!("The party did not start: {e}"));
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        screen::wall::script_mod(vm);
        self::script_mod(vm)
    }

    /// A restart under the JIT waits while it would cost a guest: a take
    /// being drawn, or a guest asking within the last minutes.
    fn can_restart(&mut self, _cx: &mut Cx) -> Result<(), String> {
        match self.running.as_ref().and_then(|running| running.party.restart_waits()) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::Shutdown) {
            if let Some(running) = &self.running { running.party.stop_workers(); }
        }
        // The window manager asked politely: go now.
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        if self.prompt_tick.is_event(event).is_some() {
            self.show_prompt(cx);
        }
        // Left / Right step through the prompt history while the pill isn't
        // typed in; Alt or Ctrl with them always (a Mac's remote desktop
        // keeps Ctrl+arrows for its own spaces).
        if let Event::KeyDown(ke) = event {
            if matches!(ke.key_code, KeyCode::ArrowLeft | KeyCode::ArrowRight) {
                let m = &ke.modifiers;
                let typing = cx.has_key_focus(self.ui.widget(cx, ids!(prompt)).area());
                if m.alt || m.control || (!typing && !m.shift && !m.logo) {
                    self.step_history(cx, ke.key_code == KeyCode::ArrowLeft);
                }
            }
        }
        self.match_event(cx, event);
        let feed = self.running.as_ref().map(|running| &running.party.live.camera);
        self.box_camera.handle_event(cx, event, feed);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
