pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(560, 280)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        align: Center
                        spacing: theme.space_3
                        padding: Inset{top: 28 left: 28 right: 28 bottom: 28}

                        title := Label{
                            text: "wasm-bindgen browser APIs"
                            draw_text.color: #x0f172a
                            draw_text.text_style: theme.font_bold{font_size: 24}
                        }

                        status := Label{
                            text: "Press the button to probe browser APIs."
                            draw_text.color: #x475569
                            draw_text.text_style.font_size: 12
                        }

                        check_button := Button{
                            text: "Check local storage"
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
}

impl App {
    fn set_status(&self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(status)).set_text(cx, text);
    }

    #[cfg(target_arch = "wasm32")]
    fn browser_status(&self) -> String {
        let now = js_sys::Date::new_0().to_iso_string();

        match web_sys::window().and_then(|window| window.local_storage().ok().flatten()) {
            Some(storage) => {
                let key = "makepad-example-bindgen-web";
                let value = format!("stored {key} at {now}");
                let _ = storage.set_item(key, &value);
                match storage.get_item(key).ok().flatten() {
                    Some(saved) => format!("wasm32: {saved}"),
                    None => format!("wasm32: wrote {value}, but readback was empty"),
                }
            }
            None => format!("wasm32: localStorage unavailable at {now}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn browser_status(&self) -> String {
        "browser APIs require wasm32".to_string()
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(check_button)).clicked(actions) {
            let status = self.browser_status();
            self.set_status(cx, &status);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
