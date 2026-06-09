pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

const SPLASH_BODY: &str = r#"
width: Fill
height: Fit
flow: Down
spacing: 10
padding: 12
draw_bg +: {
    color: #252525
}

title := Label {
    text: "Splash isolate"
}

status := Label {
    text: "NOT CLICKED"
    draw_text.text_style.font_size: 24
}

clicker := Button {
    text: "Click isolated button"
    on_click: || {
        ui.status.set_text("CLICKED: isolated on_click ran")
    }
}

runaway := Button {
    text: "Run infinite loop"
    on_click: || {
        ui.status.set_text("RUNAWAY: loop started")
        loop {}
    }
}
"#;

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(520, 320)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        padding: 16
                        draw_bg +: {
                            color: theme.color_bg_app
                        }

                        host_title := Label{
                            text: "Host app VM"
                        }

                        isolate_splash := Splash{
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
                .widget(cx, ids!(isolate_splash))
                .set_text(cx, SPLASH_BODY);
        }

        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
