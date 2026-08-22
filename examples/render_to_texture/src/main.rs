//! Render-to-texture smoke: a `CachedView` renders its children into an
//! offscreen draw pass and composites the resulting texture back with a quad.
//! The four quadrants are deliberately different solid colours, so a grab of
//! this window pins three things at once: that the child pass ran at all (a
//! skipped pass leaves the composite sampling an empty texture), and that the
//! target's U and V axes are not flipped.
pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let Quadrant = SolidView{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(320, 240)
                body +: {
                    flow: Down
                    cached_pane := CachedView{
                        width: Fill
                        height: Fill
                        flow: Down
                        top_row := View{
                            width: Fill
                            height: Fill
                            flow: Right
                            top_left := Quadrant{ draw_bg +: { color: #ff0000 } }
                            top_right := Quadrant{ draw_bg +: { color: #00cc00 } }
                        }
                        bottom_row := View{
                            width: Fill
                            height: Fill
                            flow: Right
                            bottom_left := Quadrant{ draw_bg +: { color: #0000ff } }
                            bottom_right := Quadrant{ draw_bg +: { color: #ffff00 } }
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

impl MatchEvent for App {}

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
