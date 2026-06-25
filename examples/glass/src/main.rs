pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.glass.*

    let TitleLabel = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 26}
    }

    let DetailLabel = Label{
        width: Fill
        height: Fit
        draw_text.color: #xd9e8ffcc
        draw_text.text_style: theme.font_regular{font_size: 12}
    }

    let RadioLabel = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 12}
    }

    let OptionLabel = Label{
        width: Fit
        height: Fit
        draw_text.color: #xffffffff
        draw_text.text_style: theme.font_bold{font_size: 17}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Glass Radio"
                window.inner_size: vec2(430, 760)
                pass.clear_color: #x090b12
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        align: Align{x: 0.5 y: 0.0}
                        show_bg: true
                        draw_bg.color: #x090b12

                        View{
                            width: 430
                            height: Fill
                            flow: Overlay
                            show_bg: true
                            draw_bg.color: #x090b12

                            Svg{
                                width: Fill
                                height: Fill
                                animating: false
                                draw_svg +: {
                                    svg: crate_resource("self:resources/background.svg")
                                }
                            }

                            View{
                                width: Fill
                                height: Fill
                                show_bg: true
                                draw_bg.color: #x02040a18
                            }

                            View{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 12
                                padding: Inset{left: 70, right: 28, top: 235, bottom: 28}

                                View{
                                    width: Fill
                                    height: 50
                                    flow: Right
                                    spacing: 16
                                    align: Align{x: 0.0 y: 0.5}
                                    radio_air := GlassRadio{}
                                    OptionLabel{text: "Air"}
                                }

                                View{
                                    width: Fill
                                    height: 50
                                    flow: Right
                                    spacing: 16
                                    align: Align{x: 0.0 y: 0.5}
                                    radio_water := GlassRadio{}
                                    OptionLabel{text: "Water"}
                                }

                                View{
                                    width: Fill
                                    height: 50
                                    flow: Right
                                    spacing: 16
                                    align: Align{x: 0.0 y: 0.5}
                                    radio_light := GlassRadio{}
                                    OptionLabel{text: "Light"}
                                }

                                View{
                                    width: Fill
                                    height: 50
                                    flow: Right
                                    spacing: 16
                                    align: Align{x: 0.0 y: 0.5}
                                    radio_matter := GlassRadio{}
                                    OptionLabel{text: "Matter"}
                                }
                            }
                        }

                        // Previous mobile UI-kit dashboard is intentionally inactive while
                        // the glass radio button is tuned as a focused widget.
                        Layer{
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 0.0}

                            View{
                                width: 430
                                height: Fill
                                flow: Down
                                spacing: 18
                                padding: Inset{left: 28, right: 28, top: 72, bottom: 28}

                                TitleLabel{text: "Gloopy Glass Radio"}
                                DetailLabel{text: "Single-control test surface for lensing, active blobs, and selected-state feel."}

                                View{
                                    width: Fill
                                    height: 316
                                }

                                ClearPanel{
                                    width: Fill
                                    height: 86
                                    flow: Down
                                    spacing: 5
                                    padding: 16
                                    draw_bg +: {
                                        corner_radius: 14.0
                                        lensing_strength: 24.0
                                        lensing_width: 18.0
                                        tint_alpha: 0.006
                                    }
                                    RadioLabel{text: "Selection"}
                                    radio_status := DetailLabel{text: "Air selected"}
                                }
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
    #[rust]
    init_frame: NextFrame,
}

impl App {
    fn glass_radio_clicked(&self, cx: &mut Cx, actions: &Actions, id: LiveId) -> bool {
        self.ui
            .widget(cx, &[id])
            .borrow::<GlassRadio>()
            .is_some_and(|radio| radio.clicked(actions))
    }

    fn set_glass_radio_active(&mut self, cx: &mut Cx, id: LiveId, active: bool, animate: Animate) {
        if let Some(mut radio) = self.ui.widget(cx, &[id]).borrow_mut::<GlassRadio>() {
            radio.set_active(cx, active, animate);
        }
    }

    fn glass_radio_active(&self, cx: &mut Cx, id: LiveId) -> bool {
        self.ui
            .widget(cx, &[id])
            .borrow::<GlassRadio>()
            .is_some_and(|radio| radio.active(cx))
    }

    const OPTIONS: [(LiveId, &'static str); 4] = [
        (live_id!(radio_air), "Air"),
        (live_id!(radio_water), "Water"),
        (live_id!(radio_light), "Light"),
        (live_id!(radio_matter), "Matter"),
    ];

    fn update_status(&mut self, cx: &mut Cx) {
        let on: Vec<&str> = Self::OPTIONS
            .iter()
            .filter(|(id, _)| self.glass_radio_active(cx, *id))
            .map(|(_, label)| *label)
            .collect();
        let text = if on.is_empty() {
            "None selected".to_string()
        } else {
            format!("{} on", on.join(", "))
        };
        self.ui.label(cx, ids!(radio_status)).set_text(cx, &text);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // The widget tree is built lazily on the first draw, so defer the initial
        // toggle state to the next frame when the radios actually exist.
        self.init_frame = cx.new_next_frame();
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // The widget toggles its own on/off state, so we just refresh the summary
        // whenever any of them reports a click.
        let clicked = Self::OPTIONS
            .iter()
            .any(|(id, _)| self.glass_radio_clicked(cx, actions, *id));
        if clicked {
            self.update_status(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.init_frame.is_event(event).is_some() {
            // Each toggle is an independent checkbox; start with just Air enabled.
            self.set_glass_radio_active(cx, live_id!(radio_air), true, Animate::No);
            self.update_status(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
