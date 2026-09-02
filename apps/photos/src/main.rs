//! photos — the picture wall as a plain full-window Makepad app, part of
//! the Makepad family (wm hosts it as a tile, or in-process as a module;
//! terminal, files and sheets are its siblings).
//!
//! The wall is the stock `TileGrid` from makepad-image-tiles over a baked
//! library — the SMBC comic archive by default. Standalone the window
//! carries the F10 assistant overlay and exposes the wall's tools to it
//! (src/ai.rs); under the WM the same tools ride the bus.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_photos::{ai, view, PhotosView};
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1320, 840)
                window.title: "photos"
                body +: {
                    padding: 0.
                    margin: 0.
                    spacing: 0.
                    photos := PhotosView{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// The app's service toward the assistant: the WM's bus when hosted,
    /// the window's own F10 overlay when standalone.
    #[rust]
    ai_port: Option<AiServicePort>,
}

impl App {
    fn drain_ai_port(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => return,
        };
        for ev in events {
            match ev {
                PortEvent::Registered(endpoint) => {
                    log!("photos: AI service registered as {}", endpoint.as_str());
                }
                PortEvent::Call(call) => {
                    let result = match self.ui.widget(cx, ids!(photos)).borrow_mut::<PhotosView>() {
                        Some(mut view) => ai::answer(cx, &mut view, &call),
                        None => makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the wall is gone"),
                    };
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                // Nothing here runs long enough to cancel, and the wall has
                // no chat of its own to step aside.
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        makepad_wm_api::set_title(cx, "Photos");
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        // Retint the stock widgets from the WM palette so the wall's chrome
        // matches the desk it sits in.
        makepad_wm_theme::apply(vm);
        // The assistant's panel and overlay root, so the window's F10 slot
        // finds `mod.widgets.AiChatOverlay` by name.
        makepad_aichat::script_mod(vm);
        makepad_image_tiles::script_mod(vm);
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // The window manager asked politely (SUPER+W): go now.
        if let Event::Custom(json) = event {
            if let Some(makepad_wm_api::WmEvent::CloseRequested) = makepad_wm_api::WmEvent::parse(json) {
                cx.quit();
                return;
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
