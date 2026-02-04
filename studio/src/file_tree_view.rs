use crate::{
    file_system::file_system::FileSystem,
    makepad_widgets::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FileFilterListBase = #(FileFilterList::register_widget(vm))

    mod.widgets.FilteredFileItem = View {
        height: Fit
        width: Fill
        padding: theme.mspace_2
        spacing: theme.space_2
        align: Align{ x: 0.0 y: 0.5 }
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
                ).mix(
                    theme.color_highlight,
                    self.hover * 0.3
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
        }
    }

    mod.widgets.FileFilterList = set_type_default() do mod.widgets.FileFilterListBase {
        height: Fill
        width: Fill
        $list: PortalList {
            max_pull_down: 0.
            capture_overload: false
            grab_key_focus: false
            drag_scrolling: false
            height: Fill
            width: Fill
            flow: Down
            $FilteredFileItem: mod.widgets.FilteredFileItem {}
            $Empty: mod.widgets.FilteredFileItem {
                cursor: MouseCursor.Default
                width: Fill
                height: 25
            }
        }
    }

    mod.widgets.FileTreeViewBase = #(FileTreeView::register_widget(vm))

    mod.widgets.FileTreeView = set_type_default() do mod.widgets.FileTreeViewBase {
        width: Fill
        height: Fill
        flow: Down

        $page_flip: PageFlip {
            active_page: $file_tree
            width: Fill
            height: Fill
            
            $file_tree: StudioFileTree {}
            $filter_list: mod.widgets.FileFilterList {}
        }
    }
}

/// Represents a file that matches the filter
#[derive(Clone)]
pub struct FilteredFile {
    pub file_id: LiveId,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub enum FileFilterListAction {
    #[default]
    None,
    FileClicked(LiveId),
}

#[derive(Script, ScriptHook, Widget)]
pub struct FileFilterList {
    #[deref]
    view: View,
    #[rust]
    filter: String,
    #[rust]
    filtered_files: Vec<FilteredFile>,
}

/// Check if a path matches a filter pattern
/// Example: "draw2/app.rs" should match "draw2/src/app.rs"
fn path_matches_filter(path: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return false;
    }
    
    let filter_lower = filter.to_lowercase();
    let path_lower = path.to_lowercase();
    
    // Direct substring match
    if path_lower.contains(&filter_lower) {
        return true;
    }
    
    // Split filter by '/' and try to match each part in sequence
    let filter_parts: Vec<&str> = filter_lower.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path_lower.split('/').filter(|s| !s.is_empty()).collect();
    
    if filter_parts.is_empty() {
        return false;
    }
    
    // Try to find all filter parts in the path in order
    let mut path_idx = 0;
    for filter_part in &filter_parts {
        let mut found = false;
        while path_idx < path_parts.len() {
            if path_parts[path_idx].contains(filter_part) {
                found = true;
                path_idx += 1;
                break;
            }
            path_idx += 1;
        }
        if !found {
            return false;
        }
    }
    
    true
}

impl FileFilterList {
    fn rebuild_filtered_files(&mut self, file_system: &FileSystem) {
        self.filtered_files.clear();
        
        if self.filter.is_empty() {
            return;
        }
        
        // Iterate through all files in the file system
        for (path, file_id) in &file_system.path_to_file_node_id {
            // Only include files (not directories)
            if let Some(node) = file_system.file_nodes.get(file_id) {
                if node.is_file() && path_matches_filter(path, &self.filter) {
                    self.filtered_files.push(FilteredFile {
                        file_id: *file_id,
                        path: path.clone(),
                    });
                }
            }
        }
        
        // Sort by path for consistent ordering
        self.filtered_files.sort_by(|a, b| a.path.cmp(&b.path));
    }

    fn draw_filtered_list(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.filtered_files.len().max(1));
        
        while let Some(item_id) = list.next_visible_item(cx) {
            if let Some(filtered_file) = self.filtered_files.get(item_id) {
                let is_even = item_id & 1 == 0;
                let is_even_f = if is_even { 1.0 } else { 0.0 };
                
                let mut item = list.item(cx, item_id, id!($FilteredFileItem)).as_view();
                script_apply_eval!(cx, item, {
                    draw_bg +: {is_even: #(is_even_f)}
                });
                
                while let Some(step) = item.draw(cx, &mut Scope::empty()).step() {
                    if let Some(mut tf) = step.as_text_flow().borrow_mut() {
                        tf.draw_text(cx, &filtered_file.path);
                    }
                }
            } else {
                // Empty item when no results
                let is_even = item_id & 1 == 0;
                let is_even_f = if is_even { 1.0 } else { 0.0 };
                let mut item = list.item(cx, item_id, id!($Empty)).as_view();
                script_apply_eval!(cx, item, {
                    draw_bg +: {is_even: #(is_even_f)}
                });
                item.draw_all(cx, &mut Scope::empty());
            }
        }
    }
}

impl Widget for FileFilterList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_filtered_list(cx, &mut *list);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        let filter_list = self.view.portal_list(ids!($list));
        self.view.handle_event(cx, event, scope);
        
        if let Event::Actions(actions) = event {
            if filter_list.any_items_with_actions(&actions) {
                for (item_id, item) in filter_list.items_with_actions(&actions) {
                    // Check if there was a finger up (click) on this item
                    if item.as_view().finger_up(&actions).is_some() {
                        if let Some(filtered_file) = self.filtered_files.get(item_id) {
                            cx.widget_action(
                                uid,
                                &scope.path,
                                FileFilterListAction::FileClicked(filtered_file.file_id),
                            );
                        }
                    }
                }
            }
        }
    }
}

impl FileFilterListRef {
    pub fn set_filter(&self, cx: &mut Cx, filter: String, file_system: &FileSystem) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.filter = filter;
            inner.rebuild_filtered_files(file_system);
            inner.view.redraw(cx);
        }
    }

    pub fn file_clicked(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileFilterListAction::FileClicked(file_id) = item.cast() {
                return Some(file_id);
            }
        }
        None
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FileTreeView {
    #[deref]
    view: View,
    #[rust]
    filter_active: bool,
}

impl Widget for FileTreeView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl FileTreeViewRef {
    pub fn set_filter(&self, cx: &mut Cx, filter: String, file_system: &FileSystem) {
        if let Some(mut inner) = self.borrow_mut() {
            let was_active = inner.filter_active;
            inner.filter_active = !filter.is_empty();
            
            // Switch pages using PageFlip
            if inner.filter_active != was_active {
                let page_flip = inner.view.page_flip(ids!($page_flip));
                if inner.filter_active {
                    page_flip.set_active_page(cx, id!($filter_list));
                } else {
                    page_flip.set_active_page(cx, id!($file_tree));
                }
            }
        }
        // Also update the filter list
        if let Some(inner) = self.borrow() {
            inner.view.file_filter_list(ids!($filter_list)).set_filter(cx, filter, file_system);
        }
    }

    pub fn filter_file_clicked(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(inner) = self.borrow() {
            return inner.view.file_filter_list(ids!($filter_list)).file_clicked(actions);
        }
        None
    }
}
