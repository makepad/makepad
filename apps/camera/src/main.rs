//! camera — the viewfinder, the shutter, the front/back switch and the last
//! shot, as a plain full-window Makepad app (the WM hosts it as an app of
//! its own; on the phone it is an icon on the home screen).

pub use makepad_widgets;
use makepad_widgets::*;

app_main!(App, font_assets: [INTER_FONT_ASSET, ROBOTO_FLEX_FONT_ASSET]);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Camera"
                window.inner_size: vec2(480, 860)
                pass +: {clear_color: #000}
                body +: {
                    padding: 0
                    spacing: 0
                    camera := CameraView{}
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

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        makepad_wm_api::set_title(cx, "Camera");
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        makepad_widgets::script_mod(vm);
        makepad_app_camera::view::script_mod(vm);
        self::script_mod(vm)
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
