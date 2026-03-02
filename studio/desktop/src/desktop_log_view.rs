use crate::{
    app_data::{AppData, UiLogEntry},
    makepad_widgets::*,
};
use makepad_studio_protocol::LogLevel;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopLogViewBase = #(DesktopLogView::register_widget(vm))

    mod.widgets.LogIcon = View {
        width: 10.0
        height: 10.0
        margin: Inset {top: 2.0 right: 10.0 left: 0.0 bottom: 0.0}
        show_bg: true
    }

    mod.widgets.LogItem = View {
        height: Fit
        width: Fill
        padding: theme.mspace_2
        spacing: theme.space_2
        align: Align {x: 0.0 y: 0.0}
        show_bg: true
        draw_bg +: {
            color_even: uniform(theme.color_bg_even)
            color_odd: uniform(theme.color_bg_odd)
            color_selected: uniform(theme.color_outset_active)
            is_even: instance(0.0)
            selected: instance(0.0)
            hover: instance(0.0)
            pixel: fn() {
                return self.color_even.mix(
                    self.color_odd,
                    self.is_even
                ).mix(
                    self.color_selected,
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
        selectable := TextFlow {
            width: Fill
            height: Fit
            selectable: true
            code_view := CodeView {
                editor +: {
                    margin: Inset {left: 25.0}
                }
            }
            fold_button := FoldButton {
                animator: Animator {
                    active: {
                        default: @off
                    }
                }
            }
            log_icon := mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.circle(5.0, 5.0, 4.0)
                        sdf.fill(theme.color_label_outer)
                        let sz = 1.0
                        sdf.move_to(5.0, 5.0)
                        sdf.line_to(5.0, 5.0)
                        sdf.stroke(#a, 0.8)
                        return sdf.result
                    }
                }
            }
            warning_icon := mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.move_to(5.0, 1.0)
                        sdf.line_to(9.25, 9.0)
                        sdf.line_to(0.75, 9.0)
                        sdf.close_path()
                        sdf.fill(theme.color_warning)
                        sdf.move_to(5.0, 3.5)
                        sdf.line_to(5.0, 5.25)
                        sdf.stroke(#0, 1.0)
                        sdf.move_to(5.0, 7.25)
                        sdf.line_to(5.0, 7.5)
                        sdf.stroke(#0, 1.0)
                        return sdf.result
                    }
                }
            }
            error_icon := mod.widgets.LogIcon {
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.circle(5.0, 5.0, 4.5)
                        sdf.fill(theme.color_error)
                        let sz = 1.5
                        sdf.move_to(5.0 - sz, 5.0 - sz)
                        sdf.line_to(5.0 + sz, 5.0 + sz)
                        sdf.move_to(5.0 - sz, 5.0 + sz)
                        sdf.line_to(5.0 + sz, 5.0 - sz)
                        sdf.stroke(#0, 0.8)
                        return sdf.result
                    }
                }
            }
        }
    }

    mod.widgets.LogEmptyItem = View {
        cursor: MouseCursor.Default
        width: Fill
        height: 25.0
        show_bg: true
        draw_bg +: {
            color_even: uniform(theme.color_bg_even)
            color_odd: uniform(theme.color_bg_odd)
            is_even: instance(0.0)
            pixel: fn() {
                return self.color_even.mix(
                    self.color_odd,
                    self.is_even
                )
            }
        }
        padding: Inset {left: 8.0 right: 8.0 top: 4.0 bottom: 4.0}
        empty_label := Label {
            width: Fill
            text: ""
        }
    }

    mod.widgets.DesktopLogView = set_type_default() do mod.widgets.DesktopLogViewBase {
        height: Fill
        width: Fill
        list := PortalList {
            max_pull_down: 0.0
            capture_overload: false
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            selectable: true
            height: Fill
            width: Fill
            flow: Down
            LogItem := mod.widgets.LogItem {}
            Empty := mod.widgets.LogEmptyItem {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopLogViewAction {
    OpenLocation {
        path: String,
        line: usize,
        column: usize,
    },
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq)]
struct LogLocationLink {
    path: String,
    line: usize,
    column: usize,
}

#[derive(Script, Widget)]
pub struct DesktopLogView {
    #[deref]
    view: View,
    #[rust]
    tail: bool,
}

impl ScriptHook for DesktopLogView {
    fn on_after_new(&mut self, _vm: &mut ScriptVm) {
        self.tail = true;
    }
}

impl DesktopLogView {
    const EMPTY_ROW_HEIGHT: f64 = 25.0;

    fn tab_id(&self, cx: &Cx) -> LiveId {
        let path = cx.widget_tree().path_to(self.widget_uid());
        path.get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(log_first))
    }

    fn empty_fill_rows(list: &PortalList, cx: &Cx2d, used_rows: usize) -> usize {
        let viewport_h = list.area().rect(cx).size.y.max(0.0);
        if viewport_h <= 0.0 {
            return 1usize.saturating_sub(used_rows);
        }
        let visible_rows = ((viewport_h / Self::EMPTY_ROW_HEIGHT).ceil() as usize).max(1);
        visible_rows.saturating_sub(used_rows)
    }

    fn collect_entries<'a>(data: &'a AppData, tab_id: LiveId) -> Option<&'a VecDeque<UiLogEntry>> {
        if let Some(state) = data.log_tab_state.get(&tab_id) {
            return data.build_log_entries.get(&state.build_id);
        }
        data.active_mount
            .as_ref()
            .and_then(|mount| data.mounts.get(mount))
            .map(|mount| &mount.log_entries)
    }

    fn icon_for_level(level: &LogLevel) -> LiveId {
        match level {
            LogLevel::Error => id!(error_icon),
            LogLevel::Warning => id!(warning_icon),
            LogLevel::Wait => id!(warning_icon),
            LogLevel::Panic => id!(error_icon),
            LogLevel::Log => id!(log_icon),
        }
    }

    fn apply_is_even(cx: &mut Cx2d, item: &mut ViewRef, is_even_f: f32) {
        script_apply_eval!(cx, item, {
            draw_bg +: {is_even: #(is_even_f)}
        });
    }

    fn draw_empty(&mut self, cx: &mut Cx2d, list: &mut PortalList, text: &str) {
        let rows = Self::empty_fill_rows(list, cx, 0).max(1);
        list.set_item_range(cx, 0, rows);
        while let Some(item_id) = list.next_visible_item(cx) {
            let mut item = list.item(cx, item_id, id!(Empty)).as_view();
            let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };
            Self::apply_is_even(cx, &mut item, is_even_f);
            let label = if item_id == 0 { text } else { "" };
            item.label(cx, ids!(empty_label)).set_text(cx, label);
            item.draw_all(cx, &mut Scope::empty());
        }
    }

    fn draw_entries(
        &mut self,
        cx: &mut Cx2d,
        list: &mut PortalList,
        entries: &VecDeque<UiLogEntry>,
    ) {
        if entries.is_empty() {
            self.draw_empty(cx, list, "No logs yet");
            return;
        }
        let empty_rows = Self::empty_fill_rows(list, cx, entries.len());
        let item_count = entries.len() + empty_rows;
        list.set_item_range(cx, 0, item_count);
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };
            let Some(entry) = entries.get(item_id) else {
                let mut item = list.item(cx, item_id, id!(Empty)).as_view();
                Self::apply_is_even(cx, &mut item, is_even_f);
                item.label(cx, ids!(empty_label)).set_text(cx, "");
                item.draw_all(cx, &mut Scope::empty());
                continue;
            };
            let mut item = list.item(cx, item_id, id!(LogItem)).as_view();
            Self::apply_is_even(cx, &mut item, is_even_f);

            while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                if let Some(mut tf) = step.as_text_flow().borrow_mut() {
                    tf.draw_item_counted(cx, Self::icon_for_level(&entry.level));
                    if let Some(location) = &entry.location {
                        tf.draw_link(
                            cx,
                            id!(location_link),
                            LogLocationLink {
                                path: location.path.clone(),
                                line: location.line,
                                column: location.column,
                            },
                            &location.display_label(),
                        );
                        tf.draw_text(cx, " ");
                    }
                    tf.draw_text(cx, &entry.message);
                }
            }
        }
    }
}

impl Widget for DesktopLogView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let tab_id = self.tab_id(cx);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                if let Some(data) = scope.data.get_mut::<AppData>() {
                    if let Some(entries) = Self::collect_entries(data, tab_id) {
                        self.draw_entries(cx, &mut *list, entries);
                    } else {
                        self.draw_empty(cx, &mut *list, "No logs yet");
                    }
                } else {
                    self.draw_empty(cx, &mut *list, "No app state");
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let log_list = self.view.portal_list(cx, ids!(list));
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if !log_list.any_items_with_actions(actions) {
                return;
            }
            for location in actions.filter_actions_data::<LogLocationLink>() {
                cx.widget_action(
                    uid,
                    DesktopLogViewAction::OpenLocation {
                        path: location.path.clone(),
                        line: location.line,
                        column: location.column,
                    },
                );
            }
        }
    }
}

impl DesktopLogViewRef {
    pub fn set_tail(&self, cx: &mut Cx, tail: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.tail = tail;
            let list = inner.view.portal_list(cx, ids!(list));
            list.set_tail_range(tail);
            if tail {
                list.scroll_to_end(cx);
            }
        }
    }

    pub fn tail(&self) -> bool {
        self.borrow().map(|inner| inner.tail).unwrap_or(true)
    }

    pub fn scrolled(&self, cx: &mut Cx, actions: &Actions) -> bool {
        let Some(inner) = self.borrow() else {
            return false;
        };
        inner.view.portal_list(cx, ids!(list)).scrolled(actions)
    }

    pub fn is_at_end(&self, cx: &mut Cx) -> bool {
        let Some(inner) = self.borrow() else {
            return true;
        };
        inner.view.portal_list(cx, ids!(list)).is_at_end()
    }

    pub fn open_location_requested(&self, actions: &Actions) -> Option<(String, usize, usize)> {
        let item = actions.find_widget_action(self.widget_uid())?;
        if let DesktopLogViewAction::OpenLocation { path, line, column } = item.cast() {
            return Some((path, line, column));
        }
        None
    }
}
