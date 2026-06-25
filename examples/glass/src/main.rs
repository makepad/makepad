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

    fn set_radio_selection(&mut self, cx: &mut Cx, index: usize, animate: Animate) {
        self.set_glass_radio_active(cx, live_id!(radio_air), index == 0, animate);
        self.set_glass_radio_active(cx, live_id!(radio_water), index == 1, animate);
        self.set_glass_radio_active(cx, live_id!(radio_light), index == 2, animate);
        self.set_glass_radio_active(cx, live_id!(radio_matter), index == 3, animate);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.set_radio_selection(cx, 0, Animate::No);
        self.ui
            .label(cx, ids!(radio_status))
            .set_text(cx, "Air selected");
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let options = [
            (live_id!(radio_air), "Air selected"),
            (live_id!(radio_water), "Water selected"),
            (live_id!(radio_light), "Light selected"),
            (live_id!(radio_matter), "Matter selected"),
        ];
        for (index, (id, label)) in options.iter().enumerate() {
            if self.glass_radio_clicked(cx, actions, *id) {
                self.set_radio_selection(cx, index, Animate::Yes);
                self.ui.label(cx, ids!(radio_status)).set_text(cx, *label);
                break;
            };
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
