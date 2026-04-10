pub use makepad_widgets_dll as makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let state = {
        clicks: 0
    }
    mod.state = state

    startup() do #(App::script_component(vm)){
        ui: Root{
            on_startup:||{
                ui.main_view.render()
            }
            main_window := Window{
                window.inner_size: vec2(520, 260)
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 18
                    align: Center

                    title := Label{
                        text: "Hotload UI dylib experiment"
                        draw_text.text_style.font_size: 24
                    }

                    main_view := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 10
                        align: Center
                        on_render: ||{
                            counter_label := Label{
                                text: "Clicks through makepad-widgets-dll: " + state.clicks
                                draw_text.text_style.font_size: 18
                            }
                        }
                    }

                    increment_button := Button{
                        text: "Increment"
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

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(increment_button)).clicked(actions) {
            script_eval!(cx, {
                mod.state.clicks += 1
                ui.main_view.render()
            });
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
