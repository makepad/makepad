use crate::{
    app_data::AppData,
    makepad_code_editor::CodeEditor,
    makepad_widgets::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DesktopCodeEditorBase = #(DesktopCodeEditor::register_widget(vm))

    mod.widgets.DesktopCodeEditor = set_type_default() do mod.widgets.DesktopCodeEditorBase {
        editor := CodeEditor {}
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct DesktopCodeEditor {
    #[uid]
    uid: WidgetUid,
    #[live]
    pub editor: CodeEditor,
}

impl WidgetNode for DesktopCodeEditor {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.editor.walk(cx)
    }

    fn area(&self) -> Area {
        self.editor.area()
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.editor.redraw(cx)
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.editor.find_widgets_from_point(cx, point, found)
    }

    fn visible(&self) -> bool {
        self.editor.visible()
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.editor.set_visible(cx, visible)
    }
}

impl Widget for DesktopCodeEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let path = cx.widget_tree().path_to(self.widget_uid());
        let tab_id = path
            .get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(editor_first));
        if let Some(data) = scope.data.get_mut::<AppData>() {
            if let Some(session) = data.sessions.get_mut(&tab_id) {
                self.editor.draw_walk_editor(cx, session, walk);
            } else {
                self.editor.draw_empty_editor(cx, walk);
            }
        } else {
            self.editor.draw_empty_editor(cx, walk);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let path = cx.widget_tree().path_to(self.widget_uid());
        let tab_id = path
            .get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(id!(editor_first));
        if let Some(data) = scope.data.get_mut::<AppData>() {
            if let Some(session) = data.sessions.get_mut(&tab_id) {
                for action in self
                    .editor
                    .handle_event(cx, event, &mut Scope::empty(), session)
                {
                    cx.widget_action(self.uid, action);
                }
            }
        }
    }
}
