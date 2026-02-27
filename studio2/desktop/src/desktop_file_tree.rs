use crate::{app::AppData, makepad_widgets::file_tree::FileTreeAction, makepad_widgets::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopFileTreeBase = #(DesktopFileTree::register_widget(vm))

    mod.widgets.DesktopFileTree = set_type_default() do mod.widgets.DesktopFileTreeBase {
        file_tree: FileTree {}
    }
}

#[derive(Script, Widget)]
pub struct DesktopFileTree {
    #[wrap]
    #[live]
    pub file_tree: FileTree,
}

impl ScriptHook for DesktopFileTree {}

impl Widget for DesktopFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            if let Some(data) = scope.data.get_mut::<AppData>() {
                data.file_tree.draw(cx, &mut self.file_tree);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.file_tree.handle_event(cx, event, scope);
    }
}

impl DesktopFileTreeRef {
    pub fn file_clicked(&self, actions: &Actions) -> Option<LiveId> {
        let uid = self.borrow().map(|inner| inner.file_tree.widget_uid())?;
        if let Some(item) = actions.find_widget_action(uid) {
            if let FileTreeAction::FileClicked(file_id) = item.cast() {
                return Some(file_id);
            }
        }
        None
    }

    pub fn folder_clicked(&self, actions: &Actions) -> Option<LiveId> {
        let uid = self.borrow().map(|inner| inner.file_tree.widget_uid())?;
        if let Some(item) = actions.find_widget_action(uid) {
            if let FileTreeAction::FolderClicked(file_id) = item.cast() {
                return Some(file_id);
            }
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
        if let Some(mut inner) = self.borrow_mut() {
            inner
                .file_tree
                .set_folder_is_open(cx, node_id, is_open, animate);
        }
    }
}
