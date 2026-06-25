pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.glass.*

    let ToggleRow = View{
        width: Fill
        height: 40
        flow: Right
        spacing: 16
        align: Align{x: 0.0, y: 0.5}
    }

    let Row = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: 10
    }

    let TabItem = Label{
        width: Fit
        height: Fit
        draw_text.color: #xc9d8f0ff
        draw_text.text_style: theme.font_bold{font_size: 12}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Glass Kit"
                window.inner_size: vec2(430, 880)
                pass.clear_color: #x05070e
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        align: Align{x: 0.5, y: 0.0}
                        show_bg: true
                        draw_bg.color: #x05070e

                        View{
                            width: 430
                            height: Fill
                            flow: Overlay
                            show_bg: true
                            draw_bg.color: #x05070e

                            // Vibrant background so the lensing has something to refract.
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
                                draw_bg.color: #x02040a14
                            }

                            // Content scrolls in the BACKGROUND pass so each toggle's lens
                            // can refract its own track/knob (a Layer would hide the base).
                            content := ScrollYView{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 12
                                padding: Inset{left: 24, right: 24, top: 54, bottom: 120}

                                glass.H1{text: "Glass Kit"}
                                glass.Body{text: "A liquid-glass component showcase."}

                                glass.Caption{text: "SEGMENTED"}
                                glass.GlassSegmented{ width: Fill, labels: ["Day", "Week", "Month"] }

                                glass.Caption{text: "TOGGLES"}
                                ToggleRow{ radio_air := GlassRadio{}  glass.OptionLabel{text: "Air"} }
                                ToggleRow{ radio_water := GlassRadio{}  glass.OptionLabel{text: "Water"} }
                                ToggleRow{ radio_light := GlassRadio{}  glass.OptionLabel{text: "Light"} }
                                ToggleRow{ radio_matter := GlassRadio{}  glass.OptionLabel{text: "Matter"} }

                                glass.Caption{text: "BUTTONS"}
                                Row{
                                    glass.GlassButtonProminent{text: "Continue"}
                                    glass.GlassButton{text: "Cancel"}
                                }

                                glass.Caption{text: "SLIDER"}
                                glass.GlassSlider{ width: Fill }

                                glass.Caption{text: "SEARCH & CHIPS"}
                                glass.SearchField{ width: Fill }
                                Row{
                                    glass.Chip{text: "All"}
                                    glass.Chip{text: "Photos"}
                                    glass.Chip{text: "Videos"}
                                }

                                glass.Caption{text: "CARD"}
                                glass.Card{
                                    width: Fill
                                    height: Fit
                                    flow: Down
                                    spacing: 6
                                    padding: 18
                                    glass.H2{text: "Liquid Glass"}
                                    glass.Body{text: "Glass floats on the navigation layer, refracting the content beneath it."}
                                }
                            }

                            // Floating glass tab bar.
                            Layer{
                                width: Fill
                                height: Fill
                                align: Align{x: 0.5, y: 1.0}
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Down
                                    align: Align{x: 0.5, y: 1.0}
                                    padding: Inset{left: 24, right: 24, top: 0, bottom: 22}
                                    glass.TabBar{
                                        width: Fill
                                        height: 60
                                        flow: Right
                                        align: Align{x: 0.5, y: 0.5}
                                        spacing: 40
                                        TabItem{text: "Home"}
                                        TabItem{text: "Browse"}
                                        TabItem{text: "You"}
                                    }
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
    fn set_glass_radio_active(&mut self, cx: &mut Cx, id: LiveId, active: bool, animate: Animate) {
        if let Some(mut radio) = self.ui.widget(cx, &[id]).borrow_mut::<GlassRadio>() {
            radio.set_active(cx, active, animate);
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.init_frame = cx.new_next_frame();
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if self.init_frame.is_event(event).is_some() {
            self.set_glass_radio_active(cx, live_id!(radio_air), true, Animate::No);
            self.set_glass_radio_active(cx, live_id!(radio_light), true, Animate::No);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
