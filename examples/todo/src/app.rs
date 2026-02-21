use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    // ---- Vector Icons ----

    let IconCheck = Vector{width: 18 height: 18 viewbox: vec4(0 0 24 24)
        Path{d: "M20 6L9 17L4 12" fill: false stroke: theme.color_highlight stroke_width: 2.5
            stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconTrash = Vector{width: 14 height: 14 viewbox: vec4(0 0 24 24)
        Path{d: "M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.8 stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconPlus = Vector{width: 16 height: 16 viewbox: vec4(0 0 24 24)
        Path{d: "M12 5v14M5 12h14" fill: false stroke: theme.color_white stroke_width: 2.5
            stroke_linecap: "round"}
    }

    let IconClipboard = Vector{width: 40 height: 40 viewbox: vec4(0 0 24 24)
        Path{d: "M9 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.2 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v0a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1z" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.2 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 12h6M9 16h4" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.2 stroke_linecap: "round"}
    }

    let IconRocket = Vector{width: 28 height: 28 viewbox: vec4(0 0 24 24)
        Path{d: "M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z" fill: theme.color_bg_highlight_inline stroke: theme.color_highlight stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M12 15l-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z" fill: false stroke: theme.color_highlight stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5" fill: false stroke: theme.color_highlight stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
    }

    // ---- Tag colors ----

    fn tag_color(tag) {
        if tag == "dev" theme.color_highlight
        else if tag == "design" theme.color_selection_focus
        else if tag == "personal" theme.color_outset_focus
        else if tag == "urgent" theme.color_warning
        else theme.color_highlight
    }

    // ---- Templates ----

    let TodoItem = RoundedView{
        width: Fill height: Fit
        padding: theme.mspace_2{left: theme.space_3, right: theme.space_3}
        flow: Right spacing: theme.space_2
        align: Align{y: 0.5}
        draw_bg.color: theme.color_bg_container
        draw_bg.border_radius: 10.0

        check := CheckBox{text: ""}
        label := Label{
            width: Fill
            text: "task"
            draw_text.color: theme.color_label_inner
            draw_text.text_style.font_size: theme.font_size_p
        }
        tag := RoundedView{
            width: Fit height: Fit
            padding: theme.mspace_h_1{left: theme.space_2, right: theme.space_2}
            draw_bg.color: theme.color_bg_highlight_inline
            draw_bg.border_radius: 4.0
            tag_label := Label{
                text: ""
                draw_text.color: theme.color_highlight
                draw_text.text_style.font_size: theme.font_size_code
                draw_text.text_style: theme.font_bold{}
            }
        }
        delete := ButtonFlatter{
            text: "x"
            width: 28 height: 28
            draw_text +: {
                color: theme.color_label_inner_inactive
                text_style +: {font_size: theme.font_size_p}
            }
        }
    }

    let EmptyState = View{
        width: Fill height: 260
        align: Center
        flow: Down spacing: theme.space_2
        IconClipboard{}
        Label{text: "No tasks yet" draw_text.color: theme.color_label_inner_inactive draw_text.text_style.font_size: theme.font_size_4}
        Label{text: "Add one below to get started" draw_text.color: theme.color_label_inner_inactive * 0.8 draw_text.text_style.font_size: theme.font_size_p}
    }

    // ---- State ----
    let todos = []

    // Seed sample todos
    todos.push({text: "Get AI to control UI", tag: "dev", done: true})

    fn add_todo(text, tag){
        todos.push({text: text, tag: tag, done: false})
        ui.todo_list.render()
    }

    fn toggle_todo(index, checked){
        todos[index].done = checked
    }

    fn delete_todo(index){
        todos.remove(index)
        ui.todo_list.render()
    }

    fn count_remaining(){
        let n = 0
        for todo in todos {
            if !todo.done { n = n + 1 }
        }
        n
    }

    // ---- UI ----

    let app = startup() do #(App::script_component(vm)){
        ui: Root{
            on_startup: ||{
                ui.todo_list.render()
            }
            main_window := Window{
                pass.clear_color: theme.color_bg_app
                window.inner_size: vec2(520, 720)
                body +: {
                    width: Fill height: Fill
                    flow: Down spacing: 0
                    align: Align{x: 0.5}

                    // Header
                    SolidView{
                        width: Fill height: Fit
                        padding: theme.mspace_3{left: theme.space_3 * 2, right: theme.space_3 * 2}
                        flow: Right spacing: theme.space_2
                        align: Align{y: 0.5}
                        draw_bg.color: theme.color_app_caption_bar

                        IconRocket{}

                        View{
                            width: Fill height: Fit
                            flow: Down spacing: 4
                            Label{
                                text: "Todo"
                                draw_text.color: theme.color_label_inner
                                draw_text.text_style: theme.font_bold{font_size: theme.font_size_2}
                            }
                            Label{
                                text: "Stay organized, get things done."
                                draw_text.color: theme.color_label_inner_inactive
                                draw_text.text_style.font_size: theme.font_size_p
                            }
                        }
                    }

                    // Add bar
                    SolidView{
                        width: Fill height: Fit
                        padding: theme.mspace_2{left: theme.space_3 * 2, right: theme.space_3 * 2}
                        draw_bg.color: theme.color_bg_container

                        View{
                            width: Fill height: Fit
                            flow: Right spacing: 10
                            align: Align{y: 0.5}

                            todo_input := TextInput{
                                width: Fill height: 9. * theme.space_1
                                empty_text: "What needs to be done?"
                                on_return: || ui.add_button.on_click()
                            }
                            add_button := Button{
                                text: "+"
                                width: 40 height: 34
                                draw_text +: {text_style +: {font_size: theme.font_size_3}}
                                on_click: ||{
                                    let text = ui.todo_input.text()
                                    if text != "" {
                                        add_todo(text, "")
                                        ui.todo_input.set_text("")
                                    }
                                }
                            }
                        }
                    }

                    // Divider
                    SolidView{
                        width: Fill height: 1
                        draw_bg.color: theme.color_bg_highlight
                    }

                    // Todo list
                    todo_list := ScrollYView{
                        width: Fill height: Fill
                        padding: theme.mspace_2{left: theme.space_3, right: theme.space_3}
                        flow: Down spacing: theme.space_1
                        new_batch: true
                        on_render: ||{
                            if todos.len() == 0
                                EmptyState{}
                            else for i, todo in todos {
                                TodoItem{
                                    label.text: todo.text
                                    tag.tag_label.text: todo.tag
                                    check.active: todo.done
                                    check.on_click: |checked| toggle_todo(i, checked)
                                    delete.on_click: || delete_todo(i)
                                }
                            }
                        }
                        EmptyState{}
                    }

                    // Footer
                    SolidView{
                        width: Fill height: Fit
                        padding: theme.mspace_2{left: theme.space_3 * 2, right: theme.space_3 * 2}
                        draw_bg.color: theme.color_app_caption_bar
                        flow: Right
                        align: Align{y: 0.5}

                        status := Label{
                            text: ""
                            draw_text.color: theme.color_label_inner_inactive
                            draw_text.text_style.font_size: theme.font_size_code
                        }
                        Filler{}
                        clear_done := ButtonFlatter{
                            text: "Clear completed"
                            on_click: ||{
                                todos.retain(|todo| !todo.done)
                                ui.todo_list.render()
                            }
                        }
                    }
                }
            }
        }
    }
    app
}

// ---- Rust boilerplate ----

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::theme_mod(vm);
        script_eval!(vm,{
            mod.theme = mod.themes.light
        });
        crate::makepad_widgets::widgets_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(clear_done)).clicked(actions) {
            log!("Icon button clicked!");
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
