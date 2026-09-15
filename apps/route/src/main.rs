//! route — the AI route/trip planner as a plain full-window Makepad app:
//! a `Window` around `RouteView{}` (the lib, src/view.rs), the AI port
//! toward the desktop assistant, and nothing else. Under the window
//! manager the same binary runs unmodified as a tile; the manager can
//! also seat the crate's module in-process (src/module.rs).
//!
//! Needs: `local/maps/*` nav data (see examples/map) or the hosted
//! archive; ANTHROPIC_API_KEY as env var or a file of that name at the
//! repo root for the cloud agent. Run from the repo root.

pub use ::makepad_widgets;

#[cfg(not(feature = "demo"))]
use makepad_ai_services::port::{AiServicePort, PortEvent};
#[cfg(not(feature = "demo"))]
use makepad_app_route::{ai, chrome, side_panel, view, RouteView};
use makepad_widgets::*;

#[cfg(feature = "demo")]
pub use makepad_app_route::nav::api::App;

app_main!(App);

#[cfg(not(feature = "demo"))]
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(3400, 2050)
                pass.clear_color: vec4(0.08, 0.10, 0.12, 1.0)
                // Full bleed: the view lays its own chrome out against its
                // rect, so the window adds no inset of its own.
                body +: {
                    padding: 0.
                    margin: 0.
                    spacing: 0.
                    route := RouteView{}
                }
            }
        }
    }
}

#[cfg(not(feature = "demo"))]
#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
    /// Route's tools toward the WM assistant (or a parked in-process host).
    #[rust]
    ai_port: Option<AiServicePort>,
    /// Last volatile map/trip context sent over the AI bus.
    #[rust]
    ai_context: String,
}

#[cfg(not(feature = "demo"))]
impl App {
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        start_memory_watchdog(None);
    }

    fn with_view<R>(&self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut RouteView) -> R) -> Option<R> {
        let route = self.ui.widget(cx, ids!(route));
        let mut view = route.borrow_mut::<RouteView>()?;
        Some(f(cx, &mut view))
    }

    fn refresh_ai_context(&mut self, cx: &mut Cx) {
        if self.ai_port.is_none() {
            return;
        }
        let Some(text) = self.with_view(cx, |cx, view| view.ai_context_line(cx)) else {
            return;
        };
        if text == self.ai_context {
            return;
        }
        self.ai_context = text.clone();
        if let Some(port) = self.ai_port.as_ref() {
            port.set_context(&text);
        }
    }

    fn drain_ai_port(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => return,
        };
        for event in events {
            match event {
                PortEvent::Registered(endpoint) => {
                    log!("route: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = self
                        .with_view(cx, |cx, view| ai::answer(cx, view, &call))
                        .unwrap_or_else(|| {
                            makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "the map is gone")
                        });
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                // Calls are synchronous, so there is no worker to cancel.
                PortEvent::Cancel { .. } => {}
                PortEvent::Subscribe { .. } | PortEvent::Unsubscribe { .. } => {}
                PortEvent::ChatOpen { open } => {
                    if open {
                        self.with_view(cx, |cx, view| view.close_assistant_panel(cx));
                    }
                }
            }
        }
    }
}

#[cfg(not(feature = "demo"))]
impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // Whisper stays on the F16 default (ggml-large-v3-turbo.bin): the
        // voice Metal library has no quantized matmul kernels, so q5_0/q8_0
        // models fail every GPU op. Port the kernels before re-quantizing.
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        side_panel::script_mod(vm);
        chrome::script_mod(vm);
        view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ensure_started(cx);
        self.drain_ai_port(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // Camera animations, direct manipulation and trip tools all arrive
        // through this event loop; the cached comparison publishes changes.
        self.refresh_ai_context(cx);
    }
}

#[cfg(test)]
mod ui_parity_tests {
    use super::*;

    #[test]
    fn selected_profile_registers_native_chrome_widget_ids() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let app = cx.with_vm(|vm| {
            crate::makepad_widgets::script_mod(vm);
            makepad_wm_theme::apply(vm);
            makepad_app_route::side_panel::script_mod(vm);
            makepad_app_route::chrome::script_mod(vm);
            #[cfg(not(feature = "demo"))]
            makepad_app_route::view::script_mod(vm);
            #[cfg(not(feature = "demo"))]
            let app = App::from_script_mod(vm, crate::script_mod);
            #[cfg(feature = "demo")]
            let app = App::from_script_mod(vm, makepad_app_route::nav::api::script_mod);
            app
        });
        #[cfg(not(feature = "demo"))]
        let ui = &app.ui;
        #[cfg(feature = "demo")]
        let ui = app.ui_ref();
        for id in [
            ids!(map),
            ids!(tilt_shift),
            ids!(layers_panel),
            ids!(layers_button),
            ids!(location_status),
            ids!(location_status_text),
            ids!(location_controls),
            ids!(location_button),
            ids!(tilt_check),
            ids!(layer_rain),
            ids!(layer_wind),
            ids!(theme_night),
            ids!(theme_circuit),
            ids!(assistant_panel),
            ids!(assistant_button),
            ids!(transcript_list),
            ids!(prompt_input),
            ids!(mic_button),
            ids!(speaker_button),
            ids!(testmap_panel),
        ] {
            assert!(!ui.widget(&cx, id).is_empty(), "missing shared UI id {id:?}");
        }
        assert_eq!(
            ui.button(&cx, ids!(location_button)).text(),
            "Fetch current location"
        );
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;

    #[test]
    fn demo_provisioner_configuration_installs_on_real_map_view() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let mut map = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            MapView::script_new_with_default(vm)
        });
        let config = makepad_app_route::provisioner::demo::hosted_tile_source();
        map.set_source_config(&mut cx, config.clone());
        assert_eq!(map.source_config(), Some(&config));
        assert_eq!(
            config,
            TileSourceConfig::http_archive(makepad_app_route::provisioner::demo::HOSTED_CONFIG.tiles)
        );
    }
}

#[cfg(all(test, not(feature = "demo")))]
mod desktop_style_tests {
    include!("../../../widgets/tests/support/app_style.rs");
}
