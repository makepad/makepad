pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.glass.*

    let days = [
      {name:"Monday" abbr:"Mon" icon:"☀" desc:"Sunny" hi:78 lo:56 graph:"06:00 ▇▇▇ 56°\n09:00 ▇▇▇▇▇ 64°\n12:00 ▇▇▇▇▇▇▇▇ 74°\n15:00 ▇▇▇▇▇▇▇▇▇ 78°\n18:00 ▇▇▇▇▇▇▇ 72°\n21:00 ▇▇▇▇ 60°"}
      {name:"Tuesday" abbr:"Tue" icon:"⛅" desc:"Partly Cloudy" hi:74 lo:54 graph:"06:00 ▇▇ 54°\n09:00 ▇▇▇▇ 60°\n12:00 ▇▇▇▇▇▇▇ 70°\n15:00 ▇▇▇▇▇▇▇▇ 74°\n18:00 ▇▇▇▇▇▇ 68°\n21:00 ▇▇▇ 58°"}
      {name:"Wednesday" abbr:"Wed" icon:"☁" desc:"Cloudy" hi:68 lo:54 graph:"06:00 ▇▇ 54°\n09:00 ▇▇▇ 58°\n12:00 ▇▇▇▇▇ 64°\n15:00 ▇▇▇▇▇▇ 68°\n18:00 ▇▇▇▇ 62°\n21:00 ▇▇▇ 56°"}
      {name:"Thursday" abbr:"Thu" icon:"🌧" desc:"Rainy" hi:62 lo:52 graph:"06:00 ▇▇ 52°\n09:00 ▇▇▇ 56°\n12:00 ▇▇▇▇ 60°\n15:00 ▇▇▇▇▇ 62°\n18:00 ▇▇▇ 58°\n21:00 ▇▇ 54°"}
      {name:"Friday" abbr:"Fri" icon:"☀" desc:"Sunny" hi:80 lo:58 graph:"06:00 ▇▇▇ 58°\n09:00 ▇▇▇▇▇ 66°\n12:00 ▇▇▇▇▇▇▇▇ 76°\n15:00 ▇▇▇▇▇▇▇▇▇▇ 80°\n18:00 ▇▇▇▇▇▇▇ 74°\n21:00 ▇▇▇▇ 62°"}
    ]

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Bugfix - Glass Weather"
                window.inner_size: vec2(560, 760)
                pass.clear_color: #x05070e
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        show_bg: true
                        draw_bg.color: #x05070e

                        Svg{
                            width: Fill
                            height: Fill
                            animating: false
                            draw_svg +: {
                                svg: crate_resource("self:resources/background.svg")
                            }
                        }

                        ScrollYView{
                            width: Fill height: Fill
                            flow: Down spacing: 14 padding: 18

                            View{width:Fill height:Fit flow:Down spacing:2
                                glass.H1{text:"Weather"}
                                glass.Caption{text:"SAN FRANCISCO · TAP A DAY"}
                            }

                            glass.Panel{
                                View{width:Fill height:Fit flow:Right spacing:8
                                    glass.GlassButton{text:"Mon\n78°" width:Fill height:64 on_click:|| {
                                        ui.bigicon.set_text(days[0].icon); ui.sel.set_text(days[0].name)
                                        ui.cond.set_text(days[0].desc); ui.hilo.set_text("H:"+days[0].hi+"°   L:"+days[0].lo+"°")
                                        ui.graph.set_text(days[0].graph)
                                    }}
                                    glass.GlassButton{text:"Tue\n74°" width:Fill height:64 on_click:|| {
                                        ui.bigicon.set_text(days[1].icon); ui.sel.set_text(days[1].name)
                                        ui.cond.set_text(days[1].desc); ui.hilo.set_text("H:"+days[1].hi+"°   L:"+days[1].lo+"°")
                                        ui.graph.set_text(days[1].graph)
                                    }}
                                    glass.GlassButton{text:"Wed\n68°" width:Fill height:64 on_click:|| {
                                        ui.bigicon.set_text(days[2].icon); ui.sel.set_text(days[2].name)
                                        ui.cond.set_text(days[2].desc); ui.hilo.set_text("H:"+days[2].hi+"°   L:"+days[2].lo+"°")
                                        ui.graph.set_text(days[2].graph)
                                    }}
                                    glass.GlassButton{text:"Thu\n62°" width:Fill height:64 on_click:|| {
                                        ui.bigicon.set_text(days[3].icon); ui.sel.set_text(days[3].name)
                                        ui.cond.set_text(days[3].desc); ui.hilo.set_text("H:"+days[3].hi+"°   L:"+days[3].lo+"°")
                                        ui.graph.set_text(days[3].graph)
                                    }}
                                    glass.GlassButton{text:"Fri\n80°" width:Fill height:64 on_click:|| {
                                        ui.bigicon.set_text(days[4].icon); ui.sel.set_text(days[4].name)
                                        ui.cond.set_text(days[4].desc); ui.hilo.set_text("H:"+days[4].hi+"°   L:"+days[4].lo+"°")
                                        ui.graph.set_text(days[4].graph)
                                    }}
                                }
                            }

                            glass.Card{
                                View{width:Fill height:Fit flow:Right align:Align{y:0.5} spacing:14
                                    bigicon := glass.H1{text:"☀"}
                                    View{width:Fill height:Fit flow:Down spacing:2
                                        sel := glass.H2{text:"Monday"}
                                        cond := glass.Body{text:"Sunny"}
                                    }
                                    hilo := glass.OptionLabel{text:"H:78°   L:56°"}
                                }
                            }

                            glass.Panel{
                                glass.Caption{text:"HOURLY TEMPERATURE"}
                                graph := glass.Body{text:"06:00 ▇▇▇ 56°\n09:00 ▇▇▇▇▇ 64°\n12:00 ▇▇▇▇▇▇▇▇ 74°\n15:00 ▇▇▇▇▇▇▇▇▇ 78°\n18:00 ▇▇▇▇▇▇▇ 72°\n21:00 ▇▇▇▇ 60°"}
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
