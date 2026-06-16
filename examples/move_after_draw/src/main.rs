pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let MoveAfterDrawPanelBase = #(MoveAfterDrawPanel::register_widget(vm))

    let MoveAfterDrawPanel = set_type_default() do MoveAfterDrawPanelBase{
        width: Fit
        height: Fit
        flow: Down
        spacing: 12
        padding: 16
        show_bg: true
        draw_bg +: {
            color: #x17202c
            border_color: #x3f7cff
            border_size: 1.4
            border_radius: 8.0
        }
    }

    let DemoRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: 8
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Move After Draw"
                window.inner_size: vec2(920, 640)
                pass.clear_color: #x0a0e14
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        draw_bg.color: #x0a0e14

                        View{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 10
                            padding: 28

                            Label{
                                text: "Move After Draw"
                                draw_text.color: #xf7f9fc
                                draw_text.text_style: theme.font_bold{font_size: 28}
                            }
                            Label{
                                width: Fill
                                text: "The floating component below is laid out normally, then its finished draw entries are shifted to the current mouse position."
                                draw_text.color: #xaab6c5
                            }
                            Label{
                                width: Fill
                                text: "Move the mouse through the window and watch the entire custom component follow as one post-draw shifted unit."
                                draw_text.color: #x7fd3b2
                            }

                            View{
                                width: Fill
                                height: Fill
                            }
                        }

                        moving_panel := MoveAfterDrawPanel{
                            width: 330

                            View{
                                width: Fill
                                height: Fit
                                flow: Down
                                spacing: 4

                                Label{
                                    text: "Custom UI Component"
                                    draw_text.color: #xffffff
                                    draw_text.text_style: theme.font_bold{font_size: 17}
                                }
                                mouse_label := Label{
                                    width: Fill
                                    text: "Mouse: waiting"
                                    draw_text.color: #x8fc7ff
                                }
                                Label{
                                    width: Fill
                                    text: "This panel contains a normal View subtree. The draw entries are moved after drawing with the turtle shift API."
                                    draw_text.color: #xb6c3d2
                                    draw_text.text_style.font_size: 10
                                }
                            }

                            DemoRow{
                                primary_button := Button{text: "Primary"}
                                secondary_button := Button{text: "Secondary"}
                            }

                            DemoRow{
                                CheckBox{text: "Check"}
                                Toggle{text: "Toggle"}
                            }

                            Slider{
                                width: Fill
                                text: "Shift amount"
                                min: 0.0
                                max: 100.0
                                default: 45.0
                            }

                            dropdown := DropDown{
                                width: Fill
                                labels: ["Alpha" "Beta" "Gamma" "Delta"]
                            }

                            note_input := TextInput{
                                width: Fill
                                height: 40
                                empty_text: "Demo text input"
                            }

                            status_label := Label{
                                width: Fill
                                text: "Buttons update this status."
                                draw_text.color: #xffd280
                                draw_text.text_style.font_size: 10
                            }
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

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(primary_button)).clicked(actions) {
            self.ui
                .label(cx, ids!(status_label))
                .set_text(cx, "Primary button clicked.");
        }
        if self.ui.button(cx, ids!(secondary_button)).clicked(actions) {
            self.ui
                .label(cx, ids!(status_label))
                .set_text(cx, "Secondary button clicked.");
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

#[derive(Script, ScriptHook, Widget)]
pub struct MoveAfterDrawPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    mouse_pos: Vec2d,
    #[rust]
    has_mouse_pos: bool,
    #[rust]
    align_range_start: Option<usize>,
}

impl Widget for MoveAfterDrawPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::MouseDown(mouse) => self.set_mouse_pos(cx, mouse.abs),
            Event::MouseMove(mouse) => self.set_mouse_pos(cx, mouse.abs),
            _ => {}
        }

        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.align_range_start.is_none() {
            self.align_range_start = Some(cx.align_list_len());
        }

        match self.view.draw_walk(cx, scope, walk) {
            Ok(()) => {}
            Err(step) => return Err(step),
        }

        let Some(start) = self.align_range_start.take() else {
            return DrawStep::done();
        };

        let rect = self.view.area().rect(cx);
        let target_pos = if self.has_mouse_pos {
            self.mouse_pos
        } else {
            dvec2(96.0, 140.0)
        };
        let shift = target_pos - rect.pos;

        cx.shift_align_range(
            &TurtleAlignRange {
                start,
                end: cx.align_list_len(),
            },
            shift,
        );

        DrawStep::done()
    }
}

impl MoveAfterDrawPanel {
    fn set_mouse_pos(&mut self, cx: &mut Cx, pos: Vec2d) {
        self.mouse_pos = pos;
        self.has_mouse_pos = true;

        let text = format!("Mouse: {:.0}, {:.0}", pos.x, pos.y);
        self.label(cx, ids!(mouse_label)).set_text(cx, &text);
        cx.redraw_all();
    }
}
