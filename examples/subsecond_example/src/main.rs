pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

fn hotpatch_message() -> &'static str {
    "Hotpatttch marker: v1 (edit me)"
}

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
                window.inner_size: vec2(560, 480)
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 14
                    padding: 20

                    title := Label{
                        text: "Second hotpatch example"
                        draw_text.text_style.font_size: 24
                    }

                    note := Label{
                        text: "Edit hotpatch_message() in Rust, save, then press Render without restarting."
                        draw_text.text_style.font_size: 12
                    }

                    marker_label := Label{
                        text: "Marker: (press Render)"
                        draw_text.text_style.font_size: 16
                    }

                    main_view := View{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 8
                        on_render: ||{
                            clicks := Label{
                                text: "Clickss: " + state.clicks
                                draw_text.text_style.font_size: 16
                            }
                        }
                    }

                    row := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10

                        bump_button := Button{
                            text: "Clickss + render"
                        }

                        render_button := Button{
                            text: "hyyyy"
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
    fn refresh_marker(&self, cx: &mut Cx) {
        self.ui
            .label(cx, ids!(marker_label))
            .set_text(cx, &format!("whaaats Up: {}", hotpatch_message()));
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(bump_button)).clicked(actions) {
            script_eval!(cx, {
                mod.state.clicks += 4
                ui.main_view.render()
            });
            self.refresh_marker(cx);
        }

        if self.ui.button(cx, ids!(render_button)).clicked(actions) {
            script_eval!(cx, { ui.main_view.render() });
            self.refresh_marker(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn on_hotreload(&mut self, cx: &mut Cx) {
        self.refresh_marker(cx);
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Startup) {
            self.refresh_marker(cx);
        }
    }
}
