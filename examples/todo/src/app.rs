use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    // ---- Vector Icons ----

    let IconCheck = Vector{width: 18 height: 18 viewbox: vec4(0 0 24 24)
        Path{d: "M20 6L9 17L4 12" fill: false stroke: #x6c6cff stroke_width: 2.5
            stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconTrash = Vector{width: 14 height: 14 viewbox: vec4(0 0 24 24)
        Path{d: "M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" fill: false stroke: #x555 stroke_width: 1.8 stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconPlus = Vector{width: 16 height: 16 viewbox: vec4(0 0 24 24)
        Path{d: "M12 5v14M5 12h14" fill: false stroke: #xfff stroke_width: 2.5
            stroke_linecap: "round"}
    }

    let IconClipboard = Vector{width: 40 height: 40 viewbox: vec4(0 0 24 24)
        Path{d: "M9 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2" fill: false stroke: #x334 stroke_width: 1.2 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v0a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1z" fill: false stroke: #x334 stroke_width: 1.2 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 12h6M9 16h4" fill: false stroke: #x334 stroke_width: 1.2 stroke_linecap: "round"}
    }

    let IconRocket = Vector{width: 28 height: 28 viewbox: vec4(0 0 24 24)
        Path{d: "M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z" fill: #x3a3a66 stroke: #x6c6cff stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M12 15l-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z" fill: false stroke: #x6c6cff stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5" fill: false stroke: #x6c6cff stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
    }

    // ---- Tag colors ----

    fn tag_color(tag) {
        if tag == "dev" "#x4466ee"
        else if tag == "design" "#x44bb88"
        else if tag == "personal" "#xee8844"
        else if tag == "urgent" "#xee4455"
        else "#x7a7acc"
    }

    // ---- Templates ----

    let TodoItem = RoundedView{
        width: Fill height: Fit
        padding: Inset{top: 12 bottom: 12 left: 16 right: 16}
        flow: Right spacing: 14
        align: Align{y: 0.5}
        draw_bg.color: #x1f1f35
        draw_bg.border_radius: 10.0

        check := CheckBox{text: ""}
        label := Label{
            width: Fill
            text: "task"
            draw_text.color: #xccccdd
            draw_text.text_style.font_size: 12.5
        }
        tag := RoundedView{
            width: Fit height: Fit
            padding: Inset{top: 3 bottom: 3 left: 8 right: 8}
            draw_bg.color: #x33335a
            draw_bg.border_radius: 4.0
            tag_label := Label{
                text: ""
                draw_text.color: #x7a7acc
                draw_text.text_style.font_size: 9
                draw_text.text_style: theme.font_bold{}
            }
        }
        delete := ButtonFlatter{
            text: "x"
            width: 28 height: 28
            draw_text +: {
                color: #x556
                text_style +: {font_size: 12}
            }
        }
    }

    let EmptyState = View{
        width: Fill height: 260
        align: Center
        flow: Down spacing: 12
        IconClipboard{}
        Label{text: "No tasks yet" draw_text.color: #x445 draw_text.text_style.font_size: 15}
        Label{text: "Add one below to get started" draw_text.color: #x334 draw_text.text_style.font_size: 11}
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
                pass.clear_color: vec4(0.06, 0.06, 0.10, 1.0)
                window.inner_size: vec2(520, 720)
                body +: {
                    width: Fill height: Fill
                    flow: Down spacing: 0
                    align: Align{x: 0.5}

                    // Header
                    SolidView{
                        width: Fill height: Fit
                        padding: Inset{top: 32 bottom: 24 left: 28 right: 28}
                        flow: Right spacing: 14
                        align: Align{y: 0.5}
                        draw_bg.color: #x12122a

                        IconRocket{}

                        View{
                            width: Fill height: Fit
                            flow: Down spacing: 4
                            Label{
                                text: "Todo"
                                draw_text.color: #xfff
                                draw_text.text_style: theme.font_bold{font_size: 22}
                            }
                            Label{
                                text: "Stay organized, get things done."
                                draw_text.color: #x446
                                draw_text.text_style.font_size: 11
                            }
                        }
                    }

                    // Add bar
                    SolidView{
                        width: Fill height: Fit
                        padding: Inset{top: 14 bottom: 14 left: 28 right: 28}
                        draw_bg.color: #x161630

                        View{
                            width: Fill height: Fit
                            flow: Right spacing: 10
                            align: Align{y: 0.5}

                            todo_input := TextInput{
                                width: Fill height: Fit
                                empty_text: "What needs to be done?"
                                on_return: || ui.add_button.on_click()
                            }
                            add_button := Button{
                                text: "+"
                                width: 40 height: 34
                                draw_bg +: {
                                    color: uniform(#x4a4aee)
                                    color_hover: uniform(#x5b5bff)
                                    color_down: uniform(#x3939cc)
                                    border_radius: 8.0
                                }
                                draw_text +: {
                                    color: #xfff
                                    text_style +: {font_size: 16}
                                }
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
                        draw_bg.color: #x222244
                    }

                    // Todo list
                    todo_list := ScrollYView{
                        width: Fill height: Fill
                        padding: Inset{top: 14 bottom: 14 left: 20 right: 20}
                        flow: Down spacing: 8
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
                        padding: Inset{top: 12 bottom: 16 left: 28 right: 28}
                        draw_bg.color: #x12122a
                        flow: Right
                        align: Align{y: 0.5}

                        status := Label{
                            text: ""
                            draw_text.color: #x446
                            draw_text.text_style.font_size: 10
                        }
                        Filler{}
                        clear_done := ButtonFlatter{
                            text: "Clear completed"
                            draw_text +: {
                                color: #x5a5aee
                            }
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
        crate::makepad_widgets::script_mod(vm);
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
