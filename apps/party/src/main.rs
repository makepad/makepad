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

use screen::wall::PartyWall;

app_main!(App, font_assets: [INTER_FONT_ASSET, ROBOTO_FLEX_FONT_ASSET]);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Party"
                window.inner_size: vec2(1600, 900)
                pass +: {clear_color: #14110f}
                body +: {
                    padding: 0
                    spacing: 0
                    wall := PartyWall{}
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

impl MatchEvent for App {
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
        // The window manager asked politely: go now.
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
