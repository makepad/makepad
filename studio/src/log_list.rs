use {
    crate::{
        app::{AppAction, AppData},
        build_manager::{build_manager::*, build_protocol::*},
        makepad_code_editor::code_view::*,
        makepad_platform::studio::JumpToFile,
        makepad_widgets::*,
    },
    std::env,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.LogListBase = #(LogList::register_widget(vm))

    mod.widgets.LogIcon = View {
        width: 10
        height: 10
        margin: Inset{top: 2. right: 10.}
        show_bg: true
    }

    mod.widgets.LogItem = View {
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
                return theme.color_bg_even.mix(
                    theme.color_bg_odd,
                    self.is_even
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
                editor +: {
                    margin: Inset{left: 25.}
                }
            }

            $fold_button: FoldButton {
                animator: Animator {
                    active: {default: @off}
                }
            }

            $wait_icon: mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.circle(5., 5., 4.)
                        sdf.fill(theme.color_label_outer)
                        sdf.move_to(3., 5.)
                        sdf.line_to(3., 5.)
                        sdf.move_to(5., 5.)
                        sdf.line_to(5., 5.)
                        sdf.move_to(7., 5.)
                        sdf.line_to(7., 5.)
                        sdf.stroke(#0, 0.8)
                        return sdf.result
                    }
                }
            }
            $log_icon: mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.circle(5., 5., 4.)
                        sdf.fill(theme.color_label_outer)
                        let sz = 1.
                        sdf.move_to(5., 5.)
                        sdf.line_to(5., 5.)
                        sdf.stroke(#a, 0.8)
                        return sdf.result
                    }
                }
            }
            $error_icon: mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.circle(5., 5., 4.5)
                        sdf.fill(theme.color_error)
                        let sz = 1.5
                        sdf.move_to(5. - sz, 5. - sz)
                        sdf.line_to(5. + sz, 5. + sz)
                        sdf.move_to(5. - sz, 5. + sz)
                        sdf.line_to(5. + sz, 5. - sz)
                        sdf.stroke(#0, 0.8)
                        return sdf.result
                    }
                }
            }
            $warning_icon: mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.move_to(5., 1.)
                        sdf.line_to(9.25, 9.)
                        sdf.line_to(0.75, 9.)
                        sdf.close_path()
                        sdf.fill(theme.color_warning)
                        sdf.move_to(5., 3.5)
                        sdf.line_to(5., 5.25)
                        sdf.stroke(#0, 1.0)
                        sdf.move_to(5., 7.25)
                        sdf.line_to(5., 7.5)
                        sdf.stroke(#0, 1.0)
                        return sdf.result
                    }
                }
            }
            $panic_icon: mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.move_to(5., 1.)
                        sdf.line_to(9., 9.)
                        sdf.line_to(1., 9.)
                        sdf.close_path()
                        sdf.fill(theme.color_panic)
                        let sz = 1.
                        sdf.move_to(5. - sz, 6.25 - sz)
                        sdf.line_to(5. + sz, 6.25 + sz)
                        sdf.move_to(5. - sz, 6.25 + sz)
                        sdf.line_to(5. + sz, 6.25 - sz)
                        sdf.stroke(#0, 0.8)
                        return sdf.result
                    }
                }
            }
        }
    }

    mod.widgets.LogList = set_type_default() do mod.widgets.LogListBase {
        height: Fill
        width: Fill
        $list: PortalList {
            max_pull_down: 0.
            capture_overload: false
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            height: Fill
            width: Fill
            flow: Down
            $LogItem: mod.widgets.LogItem {}
            $Empty: mod.widgets.LogItem {
                cursor: MouseCursor.Default
                width: Fill
                height: 25
                $body: P { margin: 0. text: "" }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum LogListAction {
    JumpTo(JumpToFile),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct LogList {
    #[deref]
    view: View,
    #[rust]
    filter: String,
    #[rust]
    filtered_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JumpToFileLink {
    item_id: usize,
}

fn log_item_matches_filter(log_item: &LogItem, filter: &str) -> bool {
    let filter_lower = filter.to_lowercase();
    match log_item {
        LogItem::Bare(msg) => msg.line.to_lowercase().contains(&filter_lower),
        LogItem::Location(msg) => {
            msg.file_name.to_lowercase().contains(&filter_lower)
                || msg.message.to_lowercase().contains(&filter_lower)
        }
        LogItem::StdinToHost(line) => line.to_lowercase().contains(&filter_lower),
    }
}

impl LogList {
    fn rebuild_filtered_indices(&mut self, log: &[(LiveId, LogItem)]) {
        self.filtered_indices.clear();
        if self.filter.is_empty() {
            return;
        }
        for (i, (_build_id, log_item)) in log.iter().enumerate() {
            if log_item_matches_filter(log_item, &self.filter) {
                self.filtered_indices.push(i);
            }
        }
    }

    fn get_log_index(&self, item_id: usize) -> Option<usize> {
        if self.filter.is_empty() {
            Some(item_id)
        } else {
            self.filtered_indices.get(item_id).copied()
        }
    }

    fn get_item_count(&self, log_len: usize) -> usize {
        if self.filter.is_empty() {
            log_len
        } else {
            self.filtered_indices.len()
        }
    }

    fn draw_log(&mut self, cx: &mut Cx2d, list: &mut PortalList, build_manager: &mut BuildManager) {
        // Rebuild filtered indices if filter is active
        if !self.filter.is_empty() {
            self.rebuild_filtered_indices(&build_manager.log);
        }
        let item_count = self.get_item_count(build_manager.log.len());
        list.set_item_range(cx, 0, item_count);
        while let Some(item_id) = list.next_visible_item(cx) {
            let log_index = self.get_log_index(item_id);
            let is_even = item_id & 1 == 0;
            fn map_level_to_icon(level: LogLevel) -> LiveId {
                match level {
                    LogLevel::Warning => id!($warning_icon),
                    LogLevel::Error => id!($error_icon),
                    LogLevel::Log => id!($log_icon),
                    LogLevel::Wait => id!($wait_icon),
                    LogLevel::Panic => id!($panic_icon),
                }
            }
            let mut location = String::new();
            if let Some((build_id, log_item)) = log_index.and_then(|i| build_manager.log.get(i)) {
                let _binary = if build_manager.active.builds.len() > 1 {
                    if let Some(build) = build_manager.active.builds.get(&build_id) {
                        &build.log_index
                    } else {
                        ""
                    }
                } else {
                    ""
                };
                let mut item = list.item(cx, item_id, id!($LogItem)).as_view();
                let is_even_f = if is_even { 1.0 } else { 0.0 };
                script_apply_eval!(cx, item, {
                    draw_bg +: {is_even: #(is_even_f)}
                });
                while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                    if let Some(mut tf) = step.as_text_flow().borrow_mut() {
                        match log_item {
                            LogItem::Bare(msg) => {
                                tf.draw_item_counted(cx, map_level_to_icon(msg.level));
                                tf.draw_text(cx, &msg.line);
                            }
                            LogItem::Location(msg) => {
                                tf.draw_item_counted(cx, map_level_to_icon(msg.level));
                                let fold_button = if msg.explanation.is_some() {
                                    tf.draw_item_counted_ref(cx, id!($fold_button))
                                        .as_fold_button()
                                } else {
                                    Default::default()
                                };
                                fmt_over!(
                                    location,
                                    "{}: {}:{}",
                                    msg.file_name,
                                    msg.start.line_index + 1,
                                    msg.start.byte_index + 1
                                );
                                tf.draw_link(
                                    cx,
                                    id!($link),
                                    JumpToFileLink {
                                        item_id: log_index.unwrap(),
                                    },
                                    &location,
                                );

                                tf.draw_text(cx, &msg.message);
                                if let Some(explanation) = &msg.explanation {
                                    let open = fold_button.open_float();
                                    if open > 0.0 {
                                        cx.turtle_new_line();
                                        let code = tf.item_counted(cx, id!($code_view));
                                        code.set_text(cx, explanation);
                                        code.as_code_view()
                                            .borrow_mut()
                                            .unwrap()
                                            .editor
                                            .height_scale = open;
                                        code.draw_all_unscoped(cx);
                                    }
                                };
                            }
                            _ => {}
                        }
                    }
                }
                continue;
            }
            let mut item = list.item(cx, item_id, id!($Empty)).as_view();
            let is_even_f = if is_even { 1.0 } else { 0.0 };
            script_apply_eval!(cx, item, {
                draw_bg +: {is_even: #(is_even_f)}
            });
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for LogList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_log(
                    cx,
                    &mut *list,
                    &mut scope.data.get_mut::<AppData>().unwrap().build_manager,
                )
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let log_list = self.view.portal_list(ids!($list));
        self.view.handle_event(cx, event, scope);
        let data = scope.data.get::<AppData>().unwrap();
        if let Event::Actions(actions) = event {
            if log_list.any_items_with_actions(&actions) {
                // alright lets figure out if someone clicked a link
                // alright so how do we now filter which link was clicked
                for jtf in actions.filter_actions_data::<JumpToFileLink>() {
                    // ok we have a JumpToFile link
                    if let Some((_build_id, log_item)) = data.build_manager.log.get(jtf.item_id) {
                        match log_item {
                            LogItem::Location(msg) => {
                                cx.action(AppAction::JumpTo(JumpToFile {
                                    file_name: msg.file_name.clone(),
                                    line: msg.start.line_index as u32,
                                    column: msg.start.byte_index as u32,
                                }));
                            }
                            _ => (),
                        }
                    }
                }
            }
        }
    }
}

impl LogListRef {
    pub fn reset_scroll(&self, cx: &mut Cx) {
        if let Some(inner) = self.borrow_mut() {
            let log_list = inner.view.portal_list(ids!($list));
            log_list.set_first_id_and_scroll(0, 0.0);
            log_list.redraw(cx);
        }
    }

    pub fn is_at_end(&self) -> bool {
        if let Some(inner) = self.borrow() {
            inner.view.portal_list(ids!($list)).is_at_end()
        } else {
            false
        }
    }

    pub fn set_tail(&self, tail: bool) {
        if let Some(inner) = self.borrow() {
            inner.view.portal_list(ids!($list)).set_tail_range(tail);
        }
    }

    pub fn scrolled(&self, actions: &Actions) -> bool {
        if let Some(inner) = self.borrow() {
            inner.view.portal_list(ids!($list)).scrolled(actions)
        } else {
            false
        }
    }

    pub fn set_filter(&self, cx: &mut Cx, filter: String, log: &[(LiveId, LogItem)]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.filter = filter;
            inner.rebuild_filtered_indices(log);
            inner.view.redraw(cx);
        }
    }
}
