pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

const APP_SPLASH: &str = include_str!("../app.splash");

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Picture Search"
                window.inner_size: vec2(760, 820)
                body +: {
                    flow: Overlay
                    show_bg: true
                    draw_bg.pixel: fn(){
                        let p = self.pos
                        var col = vec3(0.05, 0.06, 0.10).mix(vec3(0.10, 0.08, 0.16), p.y)
                        col = col + vec3(0.10, 0.34, 0.70) * smoothstep(0.55, 0.0, length((p - vec2(0.18, 0.20)) * vec2(1.0, 1.2)))
                        col = col + vec3(0.65, 0.20, 0.45) * smoothstep(0.52, 0.0, length((p - vec2(0.88, 0.18)) * vec2(1.2, 1.0)))
                        col = col + vec3(0.90, 0.58, 0.24) * smoothstep(0.60, 0.0, length(p - vec2(0.72, 0.94)))
                        return vec4(col, 1.0)
                    }
                    ScrollYView{
                        width: Fill
                        height: Fill
                        padding: Inset{left: 18 top: 18 right: 18 bottom: 18}
                        ddgo_splash := Splash{
                            allow_net: true
                            width: Fill
                            height: Fit
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
    #[rust]
    loaded: bool,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::Startup) && !self.loaded {
            self.loaded = true;
            self.ui
                .widget(cx, ids!(ddgo_splash))
                .set_text(cx, APP_SPLASH);
        }

        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
