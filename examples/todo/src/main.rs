pub use makepad_widgets;

use makepad_widgets::*;
use std::sync::{LazyLock, RwLock};

app_main!(App);

#[derive(Clone, Debug)]
struct TodoItemData {
    text: String,
    tag: String,
    done: bool,
}

fn initial_todos() -> Vec<TodoItemData> {
    vec![TodoItemData {
        text: "Get AI to control UI".to_string(),
        tag: "dev".to_string(),
        done: true,
    }]
}

static TODOS: LazyLock<RwLock<Vec<TodoItemData>>> = LazyLock::new(|| RwLock::new(initial_todos()));

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let IconCheck = Vector{width: 18 height: 18 viewbox: vec4(0 0 24 24)
        Path{d: "M20 6L9 17L4 12" fill: false stroke: theme.color_highlight stroke_width: 2.5
            stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconTrash = Vector{width: 14 height: 14 viewbox: vec4(0 0 24 24)
        Path{d: "M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.8 stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconRocket = Vector{width: 28 height: 28 viewbox: vec4(0 0 24 24)
        Path{d: "M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z" fill: theme.color_bg_highlight_inline stroke: theme.color_highlight stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M12 15l-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z" fill: false stroke: theme.color_highlight stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5" fill: false stroke: theme.color_highlight stroke_width: 1.0 stroke_linecap: "round" stroke_linejoin: "round"}
    }

    let IconClipboard = Vector{width: 40 height: 40 viewbox: vec4(0 0 24 24)
        Path{d: "M9 5H7a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.2 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 5a2 2 0 0 1 2-2h2a2 2 0 0 1 2 2v0a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1z" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.2 stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M9 12h6M9 16h4" fill: false stroke: theme.color_label_inner_inactive stroke_width: 1.2 stroke_linecap: "round"}
    }

    let TodoRow = RoundedView{
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

    mod.widgets.TodoListBase = #(TodoList::register_widget(vm))
    mod.widgets.TodoList = set_type_default() do mod.widgets.TodoListBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            padding: theme.mspace_2{left: theme.space_3, right: theme.space_3}
            spacing: theme.space_1
            scroll_bar: ScrollBar{}
            Item := CachedView{TodoRow{}}
            Empty := CachedView{EmptyState{}}
        }
    }

    let app = startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: theme.color_bg_app
                window.inner_size: vec2(520, 720)
                body +: {
                    width: Fill height: Fill
                    flow: Down spacing: 0
                    align: Align{x: 0.5}

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
                                return_key_type: Done
                            }
                            add_button := Button{
                                text: "+"
                                width: 40 height: 34
                                draw_text +: {text_style +: {font_size: theme.font_size_3}}
                            }
                        }
                    }

                    SolidView{
                        width: Fill height: 1
                        draw_bg.color: theme.color_bg_highlight
                    }

                    todo_list := mod.widgets.TodoList{}

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
                        clear_done := ButtonFlatter{text: "Clear completed"}
                    }
                }
            }
        }
    }
    app
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

#[derive(Script, ScriptHook, Widget)]
struct TodoList {
    #[deref]
    view: View,
}

impl Widget for TodoList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let todos = TODOS.read().unwrap();

        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                if todos.is_empty() {
                    list.set_item_range(cx, 0, 1);
                    while let Some(item_id) = list.next_visible_item(cx) {
                        let item = list.item(cx, item_id, id!(Empty));
                        item.draw_all_unscoped(cx);
                    }
                } else {
                    list.set_item_range(cx, 0, todos.len());
                    while let Some(item_id) = list.next_visible_item(cx) {
                        let Some(todo) = todos.get(item_id) else {
                            continue;
                        };
                        let item = list.item(cx, item_id, id!(Item));
                        item.check_box(cx, ids!(check)).set_active(cx, todo.done);
                        item.label(cx, ids!(label)).set_text(cx, &todo.text);
                        item.label(cx, ids!(tag.tag_label)).set_text(cx, &todo.tag);
                        item.view(cx, ids!(tag))
                            .set_visible(cx, !todo.tag.is_empty());
                        item.draw_all_unscoped(cx);
                    }
                }
            }
        }

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl App {
    fn add_todo(&mut self, cx: &mut Cx, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        TODOS.write().unwrap().push(TodoItemData {
            text: text.to_string(),
            tag: String::new(),
            done: false,
        });
        self.ui.text_input(cx, ids!(todo_input)).set_text(cx, "");
        self.sync_status(cx);
        self.ui.redraw(cx);
    }

    fn clear_done(&mut self, cx: &mut Cx) {
        TODOS.write().unwrap().retain(|todo| !todo.done);
        self.sync_status(cx);
        self.ui.redraw(cx);
    }

    fn toggle_item(&mut self, cx: &mut Cx, item_id: usize, checked: bool) {
        if let Some(todo) = TODOS.write().unwrap().get_mut(item_id) {
            todo.done = checked;
        }
        self.sync_status(cx);
        self.ui.redraw(cx);
    }

    fn delete_item(&mut self, cx: &mut Cx, item_id: usize) {
        let mut todos = TODOS.write().unwrap();
        if item_id < todos.len() {
            todos.remove(item_id);
        }
        drop(todos);
        self.sync_status(cx);
        self.ui.redraw(cx);
    }

    fn sync_status(&mut self, cx: &mut Cx) {
        let todos = TODOS.read().unwrap();
        let remaining = todos.iter().filter(|todo| !todo.done).count();
        let total = todos.len();
        let label = format!("{remaining} remaining / {total} total");
        self.ui.label(cx, ids!(status)).set_text(cx, &label);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some((text, _mods)) = self.ui.text_input(cx, ids!(todo_input)).returned(actions) {
            self.add_todo(cx, &text);
        }

        if self.ui.button(cx, ids!(add_button)).clicked(actions) {
            let text = self.ui.text_input(cx, ids!(todo_input)).text();
            self.add_todo(cx, &text);
        }

        if self.ui.button(cx, ids!(clear_done)).clicked(actions) {
            self.clear_done(cx);
        }

        let todo_list = self.ui.widget(cx, ids!(todo_list));
        let list = todo_list.portal_list(cx, ids!(list));
        for (item_id, item) in list.items_with_actions(actions) {
            if let Some(checked) = item.check_box(cx, ids!(check)).changed(actions) {
                self.toggle_item(cx, item_id, checked);
            }
            if item.button(cx, ids!(delete)).clicked(actions) {
                self.delete_item(cx, item_id);
            }
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        *TODOS.write().unwrap() = initial_todos();
        self.sync_status(cx);
        self.ui.redraw(cx);
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
