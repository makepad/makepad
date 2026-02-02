use {
    crate::{
        app::{AppAction, AppData},
        file_system::file_system::FileSystem,
        makepad_code_editor::code_view::*,
        makepad_file_protocol::SearchItem,
        makepad_platform::studio::JumpToFile,
        makepad_widgets::*,
    },
    std::env,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.SearchBase = #(Search::register_widget(vm))

    mod.widgets.SearchResult = View {
        height: Fit
        width: Fill
        padding: theme.mspace_2
        spacing: theme.space_2
        align: Align{ x: 0.0 y: 0.0 }
        show_bg: true
        draw_bg +: {
            is_even: instance(0.0)
            selected: instance(0.0)
            hover: instance(0.0)
            pixel: fn() {
                return self.is_even.mix(
                    theme.color_bg_even,
                    theme.color_bg_odd
                ).mix(
                    theme.color_outset_active,
                    self.selected
                )
            }
        }
        animator: Animator {
            ignore_missing: true
            hover: {
                default: @off
                off: AnimatorState {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: 0.0}
                    }
                }
                on: AnimatorState {
                    cursor: MouseCursor.Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                    }
                }
            }

            select: {
                default: @off
                off: AnimatorState {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0}
                    }
                }
                on: AnimatorState {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0}
                    }
                }
            }
        }
        $flow: TextFlow {
            width: Fill
            height: Fit

            $code_view: CodeView {
                $editor +: {
                    word_wrap: false
                    draw_bg +: { color: #0000 }
                    margin: Inset{left: 15}
                }
            }

            $fold_button: FoldButton {
                animator: Animator {
                    active: {default: @off}
                }
            }
        }
    }

    mod.widgets.Search = set_type_default() do mod.widgets.SearchBase {
        height: Fill
        width: Fill
        flow: Down
        RectShadowView {
            width: Fill
            height: 38.
            flow: Down
            align: Align{ x: 0. y: 0. }
            margin: Inset{ top: -1. }
            padding: theme.mspace_2
            spacing: 0.
            draw_bg +: {
                border_size: 0.0
                border_color: theme.color_bevel_outset_1
                shadow_color: theme.color_shadow
                shadow_radius: 5.0
                shadow_offset: vec2(0.0, 1.0)
                color: theme.color_fg_app
            }
            $content: View {
                spacing: theme.space_2
                align: Align{ y: 0.5 }
                $search_input: TextInputFlat {
                    width: Fill
                    empty_text: "Search"
                }
            }
        }
        $list: PortalList {
            capture_overload: false
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            max_pull_down: 0
            height: Fill
            width: Fill
            flow: Down
            $SearchResult: mod.widgets.SearchResult {}
            $Empty: mod.widgets.SearchResult {
                cursor: MouseCursor.Default
                width: Fill
                height: 25
                $body: P { margin: 0 text: "" }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum SearchAction {
    JumpTo(JumpToFile),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Search {
    #[deref]
    view: View,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JumpToFileLink {
    item_id: usize,
}

impl Search {
    fn draw_results(&mut self, cx: &mut Cx2d, list: &mut PortalList, file_system: &mut FileSystem) {
        list.set_item_range(cx, 0, file_system.search_results.len());
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even = item_id & 1 == 0;
            let mut location = String::new();
            if let Some(res) = file_system.search_results.get(item_id as usize) {
                let mut item = list.item(cx, item_id, live_id!(SearchResult)).as_view();

                let is_even_f = if is_even { 1.0 } else { 0.0 };
                script_apply_eval!(cx, item, {
                    draw_bg: {is_even: #(is_even_f)}
                });

                while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                    if let Some(mut tf) = step.as_text_flow().borrow_mut() {
                        let fold_button = tf
                            .draw_item_counted_ref(cx, live_id!(fold_button))
                            .as_fold_button();

                        fmt_over!(
                            location,
                            "{}: {}:{}",
                            res.file_name,
                            res.line + 1,
                            res.column_byte + 1
                        );

                        tf.draw_link(cx, live_id!(link), JumpToFileLink { item_id }, &location);

                        let open = fold_button.open_float();
                        cx.turtle_new_line();
                        let code = tf.item_counted(cx, live_id!(code_view));
                        code.set_text(cx, &res.result_line);
                        if let Some(mut code_view) = code.as_code_view().borrow_mut() {
                            code_view.lazy_init_session();
                            let lines = code_view
                                .session
                                .as_ref()
                                .unwrap()
                                .document()
                                .as_text()
                                .as_lines()
                                .len();
                            code_view.editor.height_scale = open.max(1.0 / (lines + 1) as f64);
                        }
                        code.draw_all_unscoped(cx);
                    }
                }
                continue;
            }
            let mut item = list.item(cx, item_id, live_id!(Empty)).as_view();
            let is_even_f = if is_even { 1.0 } else { 0.0 };
            script_apply_eval!(cx, item, {
                draw_bg: {is_even: #(is_even_f)}
            });
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for Search {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_results(
                    cx,
                    &mut *list,
                    &mut scope.data.get_mut::<AppData>().unwrap().file_system,
                )
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let search_results = self.view.portal_list(ids!(list));
        self.view.handle_event(cx, event, scope);
        let data = scope.data.get_mut::<AppData>().unwrap();
        if let Event::Actions(actions) = event {
            if let Some(search) = self.view.text_input(ids!(search_input)).changed(&actions) {
                let mut set = Vec::new();
                for item in search.split("|") {
                    if let Some(item) = item.strip_suffix("\\b") {
                        if let Some(item) = item.strip_prefix("\\b") {
                            set.push(SearchItem {
                                needle: item.to_string(),
                                prefixes: None,
                                pre_word_boundary: true,
                                post_word_boundary: true,
                            })
                        } else {
                            set.push(SearchItem {
                                needle: item.to_string(),
                                prefixes: None,
                                pre_word_boundary: false,
                                post_word_boundary: true,
                            })
                        }
                    } else if let Some(item) = item.strip_prefix("\\b") {
                        set.push(SearchItem {
                            needle: item.to_string(),
                            prefixes: None,
                            pre_word_boundary: true,
                            post_word_boundary: false,
                        })
                    } else {
                        set.push(SearchItem {
                            needle: item.to_string(),
                            prefixes: None,
                            pre_word_boundary: false,
                            post_word_boundary: false,
                        })
                    }
                }
                data.file_system.search_string(cx, set);
            }
            if search_results.any_items_with_actions(&actions) {
                for jtf in actions.filter_actions_data::<JumpToFileLink>() {
                    if let Some(res) = data.file_system.search_results.get(jtf.item_id) {
                        cx.action(AppAction::JumpTo(JumpToFile {
                            file_name: res.file_name.clone(),
                            line: res.line as u32,
                            column: res.column_byte as u32,
                        }));
                    }
                }
            }
        }
    }
}

impl SearchRef {}
