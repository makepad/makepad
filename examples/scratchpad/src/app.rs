use makepad_widgets2::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let TodoItem = View{
        width: Fill height: Fit
        padding: Inset{top: 10 bottom: 10 left: 14 right: 14}
        flow: Right spacing: 10
        align: Align{y: 0.5}
        show_bg: true
        draw_bg +: {
            color: uniform(#ffffff)
            color_hover: uniform(#f0f0f0)
            hover: instance(0.0)
            pixel: fn(){
                return Pal.premul(self.color.mix(self.color_hover, self.hover))
            }
        }
        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    apply: {draw_bg: {hover: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    apply: {draw_bg: {hover: 1.0}}
                }
            }
        }
        check := CheckBox{text: ""}
        label := Label{text: "task" draw_text.color: #333333 draw_text.text_style.font_size: 11}
        Filler{}
        tag := RoundedView{
            width: Fit height: Fit
            padding: Inset{top: 3 bottom: 3 left: 8 right: 8}
            draw_bg.color: #e8e8e8
            draw_bg.border_radius: 10.0
            text := Label{text: "" draw_text.color: #888888 draw_text.text_style.font_size: 9}
        }
    }
    let Tasks =["A", "B", "C"]
    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: vec4(1.0, 1.0, 1.0, 1.0)
                window.inner_size: vec2(500, 400)
                body +: {
                    align: Align{x: 0.5 y: 0.3}

                    RoundedView{
                        width: 400 height: Fit
                        flow: Down spacing: 0
                        padding: 0
                        draw_bg.color: #ffffff
                        draw_bg.border_radius: 12.0
                        draw_bg.border_size: 1.0
                        draw_bg.border_color: #e0e0e0

                        RoundedView{
                            width: Fill height: Fit
                            padding: Inset{top: 18 bottom: 14 left: 18 right: 18}
                            draw_bg.color: #fafafa
                            draw_bg.border_radius: 0.0
                            flow: Right
                            align: Align{y: 0.5}
                            Label{text: "My Tasks" draw_text.color: #222222 draw_text.text_style: theme.font_bold{font_size: 15}}
                            Filler{}
                            Label{text: "4 items" draw_text.color: #aaaaaa draw_text.text_style.font_size: 10}
                        }

                        Hr{}

                        task_view := View{
                            width: Fill height: Fit
                            flow: Down spacing: 0
                            new_batch: true
                            render: |view| {
                                for task in tasks{
                                    TodoItem{label.text: task}
                                }
                            }
                        }
                        
                        new_task := TextInput{
                            on_return: |text|{
                                tasks.push(text)
                                // lets regenerate something
                                mod.ui.regen(@task_view)
                            }
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
