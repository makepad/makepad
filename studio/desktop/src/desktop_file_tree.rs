use crate::{
    app_data::{is_hidden_virtual_path, AppData, FlatFileTree},
    makepad_widgets::file_tree::{FileTreeAction, GitStatusDotKind},
    makepad_widgets::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopFileTreeBase = #(DesktopFileTree::register_widget(vm))

    mod.widgets.FilteredFileItem = View {
        width: Fill
        height: 30.0
        flow: Right
        align: Align {x: 0.0 y: 0.5}
        padding: Inset {left: 8.0 right: 8.0 top: 0.0 bottom: 0.0}
        spacing: 0.0
        show_bg: true
        draw_bg +: {
            is_even: instance(0.0)
            pixel: fn() {
                return theme.color_bg_even.mix(theme.color_bg_odd, self.is_even)
            }
        }
        status_dot := View {
            width: 6.0
            height: 6.0
            margin: Inset {left: 0.0 right: 8.0 top: 0.0 bottom: 0.0}
            show_bg: true
            draw_bg +: {
                color: instance(#0000)
                pixel: fn() {
                    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                    sdf.circle(
                        0.5 * self.rect_size.x,
                        0.5 * self.rect_size.y,
                        min(self.rect_size.x, self.rect_size.y) * 0.34
                    )
                    return self.color * sdf.fill(#fff).w
                }
            }
        }
        row_button := ButtonFlat {
            width: Fill
            height: Fill
            align: Align {x: 0.0 y: 0.5}
            label_walk: Walk {width: Fit height: Fit}
            text: ""
            draw_bg +: {
                color: #0000
                color_hover: #0000
                color_pressed: #0000
                border_color: #0000
            }
            draw_text +: {
                color: theme.color_label_inner_inactive
                color_hover: theme.color_label_inner_inactive
                color_pressed: theme.color_label_inner_inactive
                color_focus: theme.color_label_inner_inactive
            }
        }
    }

    mod.widgets.FilteredFileEmpty = View {
        width: Fill
        height: 30.0
        padding: Inset {left: 8.0 right: 8.0 top: 6.0 bottom: 6.0}
        show_bg: true
        draw_bg +: {
            is_even: instance(0.0)
            pixel: fn() {
                return theme.color_bg_even.mix(theme.color_bg_odd, self.is_even)
            }
        }
        empty_label := Label {
            width: Fill
            text: ""
            draw_text.color: theme.color_label_outer
        }
    }

    mod.widgets.DesktopFileTree = set_type_default() do mod.widgets.DesktopFileTreeBase {
        width: Fill
        height: Fill
        flow: Down
        page_flip := PageFlip {
            active_page: @file_tree_page
            width: Fill
            height: Fill
            file_tree_page := FileTree {}
            filter_list_page := PortalList {
                width: Fill
                height: Fill
                flow: Down
                max_pull_down: 0.0
                capture_overload: false
                grab_key_focus: false
                auto_tail: false
                drag_scrolling: true
                selectable: false
                Item := mod.widgets.FilteredFileItem {}
                Empty := mod.widgets.FilteredFileEmpty {}
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopFileTreeAction {
    FileClicked(LiveId),
    FolderClicked(LiveId),
    FilteredPathClicked(String),
    #[default]
    None,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum FilteredFileRowData {
    Path(String),
    #[default]
    None,
}

impl ActionDefaultRef for FilteredFileRowData {
    fn default_ref() -> &'static Self {
        &FilteredFileRowData::None
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DesktopFileTree {
    #[deref]
    view: View,
    #[rust]
    filter_active: bool,
}

impl DesktopFileTree {
    const ROW_HEIGHT: f64 = 30.0;

    fn status_dot_color(status: GitStatusDotKind) -> Vec4 {
        match status {
            GitStatusDotKind::New => vec4(0.345, 0.761, 0.427, 1.0),
            GitStatusDotKind::Modified | GitStatusDotKind::Deleted | GitStatusDotKind::Mixed => {
                vec4(0.847, 0.392, 0.392, 1.0)
            }
            GitStatusDotKind::None => vec4(0.42, 0.48, 0.56, 0.45),
        }
    }

    fn empty_fill_rows(list: &PortalList, cx: &Cx2d, used_rows: usize) -> usize {
        let viewport_h = list.area().rect(cx).size.y.max(0.0);
        if viewport_h <= 0.0 {
            return 1usize.saturating_sub(used_rows);
        }
        let visible_rows = ((viewport_h / Self::ROW_HEIGHT).ceil() as usize).max(1);
        visible_rows.saturating_sub(used_rows)
    }

    fn draw_filtered_list(
        &mut self,
        cx: &mut Cx2d,
        list: &mut PortalList,
        filtered_paths: &[String],
        file_tree: &FlatFileTree,
        empty_text: &str,
    ) {
        if filtered_paths.is_empty() {
            let rows = Self::empty_fill_rows(list, cx, 0).max(1);
            list.set_item_range(cx, 0, rows);
            while let Some(item_id) = list.next_visible_item(cx) {
                let mut item = list.item(cx, item_id, id!(Empty)).as_view();
                let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };
                script_apply_eval!(cx, item, {
                    draw_bg +: {is_even: #(is_even_f)}
                });
                item.label(cx, ids!(empty_label))
                    .set_text(cx, if item_id == 0 { empty_text } else { "" });
                item.draw_all(cx, &mut Scope::empty());
            }
            return;
        }

        let empty_rows = Self::empty_fill_rows(list, cx, filtered_paths.len());
        let item_count = filtered_paths.len() + empty_rows;
        list.set_item_range(cx, 0, item_count);
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even_f = if item_id & 1 == 0 { 1.0 } else { 0.0 };
            let Some(path) = filtered_paths.get(item_id) else {
                let mut item = list.item(cx, item_id, id!(Empty)).as_view();
                script_apply_eval!(cx, item, {
                    draw_bg +: {is_even: #(is_even_f)}
                });
                item.label(cx, ids!(empty_label)).set_text(cx, "");
                item.draw_all(cx, &mut Scope::empty());
                continue;
            };

            let mut item = list.item(cx, item_id, id!(Item)).as_view();
            script_apply_eval!(cx, item, {
                draw_bg +: {is_even: #(is_even_f)}
            });
            let status_color = Self::status_dot_color(file_tree.git_status_dot_for_path(path));
            script_apply_eval!(cx, item, {
                status_dot.draw_bg +: {color: #(status_color)}
            });
            let button = item.button(cx, ids!(row_button));
            button.set_text(cx, path);
            button.set_action_data(FilteredFileRowData::Path(path.clone()));
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for DesktopFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut filter_active = false;
        if let Some(data) = scope.data.get_mut::<AppData>() {
            if let Some(active_mount) = data.active_mount.as_deref() {
                if let Some(mount_state) = data.mounts.get(active_mount) {
                    filter_active = !mount_state.file_filter.is_empty();
                }
            }
        }

        if filter_active != self.filter_active {
            self.filter_active = filter_active;
            let page_flip = self.view.page_flip(cx, ids!(page_flip));
            if filter_active {
                page_flip.set_active_page(cx, id!(filter_list_page));
            } else {
                page_flip.set_active_page(cx, id!(file_tree_page));
            }
        }

        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut file_tree) = step.as_file_tree().borrow_mut() {
                if !filter_active {
                    if let Some(data) = scope.data.get_mut::<AppData>() {
                        data.file_tree.draw(cx, &mut *file_tree);
                    }
                }
            } else if let Some(mut list) = step.as_portal_list().borrow_mut() {
                if filter_active {
                    if let Some(data) = scope.data.get_mut::<AppData>() {
                        if let Some(active_mount) = data.active_mount.as_deref() {
                            if let Some(mount_state) = data.mounts.get(active_mount) {
                                let visible_paths: Vec<String> = mount_state
                                    .file_filter_results
                                    .iter()
                                    .filter(|path| !is_hidden_virtual_path(path))
                                    .cloned()
                                    .collect();
                                let empty_text = if (mount_state.file_filter_pending
                                    || mount_state.file_filter_query.is_some())
                                    && visible_paths.is_empty()
                                {
                                    "Searching..."
                                } else {
                                    "No matches"
                                };
                                self.draw_filtered_list(
                                    cx,
                                    &mut *list,
                                    &visible_paths,
                                    &data.file_tree,
                                    empty_text,
                                );
                            } else {
                                self.draw_filtered_list(
                                    cx,
                                    &mut *list,
                                    &[],
                                    &data.file_tree,
                                    "No mount",
                                );
                            }
                        } else {
                            self.draw_filtered_list(
                                cx,
                                &mut *list,
                                &[],
                                &data.file_tree,
                                "No mount",
                            );
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let file_tree = self.view.file_tree(cx, ids!(file_tree_page));
        let filter_list = self.view.portal_list(cx, ids!(filter_list_page));

        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if let Some(item) = actions.find_widget_action(file_tree.widget_uid()) {
                match item.cast() {
                    FileTreeAction::FileClicked(file_id) => {
                        cx.widget_action(uid, DesktopFileTreeAction::FileClicked(file_id));
                    }
                    FileTreeAction::FolderClicked(file_id) => {
                        cx.widget_action(uid, DesktopFileTreeAction::FolderClicked(file_id));
                    }
                    _ => {}
                }
            }

            if filter_list.any_items_with_actions(actions) {
                for (_item_id, item) in filter_list.items_with_actions(actions) {
                    let button = item.button(cx, ids!(row_button));
                    if !button.clicked(actions) {
                        continue;
                    }
                    if let FilteredFileRowData::Path(path) = button.action_data().cast_ref() {
                        cx.widget_action(
                            uid,
                            DesktopFileTreeAction::FilteredPathClicked(path.clone()),
                        );
                    }
                }
            }
        }
    }
}

impl DesktopFileTreeRef {
    pub fn file_clicked(&self, actions: &Actions) -> Option<LiveId> {
        let item = actions.find_widget_action(self.widget_uid())?;
        if let DesktopFileTreeAction::FileClicked(file_id) = item.cast() {
            return Some(file_id);
        }
        None
    }

    pub fn folder_clicked(&self, actions: &Actions) -> Option<LiveId> {
        let item = actions.find_widget_action(self.widget_uid())?;
        if let DesktopFileTreeAction::FolderClicked(file_id) = item.cast() {
            return Some(file_id);
        }
        None
    }

    pub fn filtered_path_clicked(&self, actions: &Actions) -> Option<String> {
        let item = actions.find_widget_action(self.widget_uid())?;
        if let DesktopFileTreeAction::FilteredPathClicked(path) = item.cast() {
            return Some(path);
        }
        None
    }

    pub fn set_folder_is_open(
        &self,
        cx: &mut Cx,
        node_id: LiveId,
        is_open: bool,
        animate: Animate,
    ) {
        if let Some(inner) = self.borrow() {
            inner
                .view
                .file_tree(cx, ids!(file_tree_page))
                .set_folder_is_open(cx, node_id, is_open, animate);
        }
    }
}
