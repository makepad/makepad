use crate::{app::AppData, makepad_widgets::file_tree::FileTree, makepad_widgets::*};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.StudioFileTreeBase = #(StudioFileTree::register_widget(vm))

    mod.widgets.StudioFileTree = set_type_default() do mod.widgets.StudioFileTreeBase {
        width: Fill
        height: Fill
        file_tree: FileTree {}
    }
}

#[derive(Script, Widget)]
pub struct StudioFileTree {
    #[wrap]
    #[live]
    pub file_tree: FileTree,
}

impl ScriptHook for StudioFileTree {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.file_tree
                .set_folder_is_open(cx, live_id!(makepad).into(), true, Animate::No);
        });
    }
}

impl Widget for StudioFileTree {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            scope
                .data
                .get_mut::<AppData>()
                .unwrap()
                .file_system
                .draw_file_node(cx, live_id!(root).into(), 0, &mut self.file_tree);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.file_tree.handle_event(cx, event, scope);
    }
}

impl StudioFileTreeRef {
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
