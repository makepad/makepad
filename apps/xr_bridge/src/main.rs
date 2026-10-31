pub use makepad_widgets;
use makepad_widgets::*;
#[cfg(target_os = "linux")]
mod ipc;
#[cfg(target_os = "linux")]
mod bridge;

app_main!(App);
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "XR Bridge"
                window.inner_size: vec2(560, 260)
                body +: {
                    padding: 28 spacing: 18 flow: Down
                    View{
                        width: Fill height: Fit flow: Right align: Align{y: 0.5}
                        Label{text: "Play in your headset" draw_text +: {text_style: theme.font_bold{font_size: 24}}}
                        View{width: Fill height: 1}
                        xr := XrCaptionButton{app: "xr-bridge"}
                    }
                    Label{text: "Open XR Remote on your Quest, then enter its PIN in the headset menu above." width: Fill draw_text +: {wrap: Words}}
                    game_status := Label{text: "Waiting for the headset and a game." width: Fill draw_text +: {wrap: Words}}
                    Label{text: "Experimental OpenXR · Vulkan · SteamVR is not required"}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[cfg(target_os = "linux")]
    #[rust] bridge: Option<bridge::Bridge>,
    #[rust] status: String,
}
impl App {
    fn status(&mut self, cx: &mut Cx, value: String) {
        if self.status == value { return; }
        let label = self.ui.label(cx, ids!(game_status));
        if label.borrow().is_none() { return; }
        label.set_text(cx, &value);
        self.status = value;
    }
}
impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        #[cfg(target_os = "linux")]
        match bridge::Bridge::new(cx) {
            Ok(bridge) => self.bridge = Some(bridge),
            Err(e) => self.status(cx, e),
        }
        #[cfg(not(target_os = "linux"))]
        self.status(cx, "The game bridge currently runs on Linux with Vulkan.".into());
    }
}
impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_xr_stream::serve_script_mod(vm);
        self::script_mod(vm)
    }
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        #[cfg(target_os = "linux")]
        if !matches!(event, Event::Draw(_)) {
            if let Some(bridge) = &mut self.bridge {
                bridge.pump(cx);
                if matches!(event, Event::XrUpdate(_)) { bridge.xr_update(cx); }
                if self.status != bridge.status {
                    let status = bridge.status.clone();
                    self.status(cx, status);
                }
            }
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
        #[cfg(target_os = "linux")]
        if let Event::Draw(e) = event {
            if let Some(bridge) = &mut self.bridge { bridge.draw(cx, e); }
        }
    }
}
