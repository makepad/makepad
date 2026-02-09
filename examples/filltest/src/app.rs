use makepad_widgets2::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(400, 400)
                body +: {
                    SolidView{
                        width: Fill
                        height: Fill
                        draw_bg: {color: #333}
                        align: Align{x: 0.5 y: 0.5}
                        flow: Down
                        spacing: 20

                        Icon{
                            draw_icon.svg: crate_resource("self:resources/Icon_Search.svg")
                            draw_icon.color: #fff
                            icon_walk: Walk{width: 128 height: 128}
                        }

                        Icon{
                            draw_icon.svg: crate_resource("self:resources/Icon_Search.svg")
                            draw_icon.color: #ff0
                            icon_walk: Walk{width: 64 height: 64}
                        }

                        Icon{
                            draw_icon.svg: crate_resource("self:resources/Icon_Search.svg")
                            draw_icon.color: #0ff
                            icon_walk: Walk{width: 32 height: 32}
                        }

                        Icon{
                            draw_icon.svg: crate_resource("self:resources/icon_auto.svg")
                            draw_icon.color: #f80
                            icon_walk: Walk{width: 64 height: 64}
                        }
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
