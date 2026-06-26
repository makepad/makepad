pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.glass.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Bugfix - Glass Calendar Header"
                window.inner_size: vec2(560, 520)
                pass.clear_color: #x05070e
                body +: {
                    View{
                        width: Fill
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
                            height: Fit
                            flow: Down
                            spacing: 14
                            padding: 18

                            glass.Card{
                                View{width:Fill height:Fit flow:Right align:Align{y:0.5} spacing:10
                                    glass.GlassButton{text:"<" width:48 height:42}
                                    title := glass.H1{text:"June 2026" width:Fill align:Align{x:0.5}}
                                    glass.GlassButton{text:">" width:48 height:42}
                                }
                            }

                            glass.Panel{
                                View{width:Fill height:Fit flow:Right spacing:6
                                    glass.GlassButton{text:"1" width:Fill height:42}
                                    glass.GlassButton{text:"2" width:Fill height:42}
                                    glass.GlassButton{text:"3" width:Fill height:42}
                                    glass.GlassButton{text:"4" width:Fill height:42}
                                    glass.GlassButton{text:"5" width:Fill height:42}
                                    glass.GlassButton{text:"6" width:Fill height:42}
                                    glass.GlassButton{text:"7" width:Fill height:42}
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
