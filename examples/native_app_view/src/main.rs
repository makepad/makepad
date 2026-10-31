use makepad_widgets::*;
app_main!(App);
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(900, 700)
                body +: {
                    flow: Overlay
                    page := ScrollYView {
                        width: Fill height: Fill flow: Down spacing: 18
                        padding: Inset{left: 24 right: 24 top: 24 bottom: 24}
                        Label {text: "A Wayland app inside Makepad" draw_text.text_style: theme.font_bold{font_size: 24}}
                        Label {text: "Type in the terminal. Scroll beside it to move the Makepad page."}
                        controls := View {
                            width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                            show_popup := Button{text: "Show popup"}
                            close_app := Button{text: "Close terminal"}
                        }
                        native := NativeAppView {app: "wl.foot" width: Fill height: 420}
                        RoundedView {
                            width: Fill height: 500 padding: Inset{left: 20 top: 20}
                            draw_bg +: {color: theme.color_bg_app}
                            Label{text: "Makepad content below the terminal"}
                        }
                    }
                    popup := RoundedView {
                        visible: false width: 320 height: Fit flow: Down spacing: 10
                        margin: Inset{left: 70 top: 240}
                        padding: Inset{left: 20 right: 20 top: 20 bottom: 20}
                        cursor: MouseCursor.Default
                        draw_bg +: {color: #x293c62 border_radius: 12}
                        Label{text: "Makepad popup above the terminal" draw_text.color: #fff}
                        popup_button := Button{text: "Test popup input"}
                        status := Label{text: "Waiting for a click" draw_text.color: #fff}
                        hide_popup := Button{text: "Close popup"}
                    }
                }
            }
        }
    }
}
#[derive(Script, ScriptHook)]
struct App { #[live] ui: WidgetRef }
impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args: Vec<String> = std::env::args().collect();
        if let Some(app) = args.windows(2).find(|a| a[0] == "--app").map(|a| a[1].as_str()) {
            self.ui.native_app_view(cx, ids!(native)).set_app(cx, app);
        }
    }
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(show_popup)).clicked(actions) { self.ui.view(cx, ids!(popup)).set_visible(cx, true); }
        if self.ui.button(cx, ids!(hide_popup)).clicked(actions) { self.ui.view(cx, ids!(popup)).set_visible(cx, false); }
        if self.ui.button(cx, ids!(popup_button)).clicked(actions) { self.ui.label(cx, ids!(status)).set_text(cx, "Popup received the click"); }
        if self.ui.button(cx, ids!(close_app)).clicked(actions) { self.ui.native_app_view(cx, ids!(native)).close(cx); }
    }
}
impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue { makepad_widgets::script_mod(vm); self::script_mod(vm) }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
