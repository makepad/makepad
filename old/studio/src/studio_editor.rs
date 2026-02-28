use {
    crate::{
        app::AppData, file_system::file_system::EditSession, makepad_code_editor::CodeEditor,
        makepad_widgets::*,
    },
    std::env,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.StudioCodeEditorBase = #(StudioCodeEditor::register_widget(vm))

    mod.widgets.StudioCodeEditor = set_type_default() do mod.widgets.StudioCodeEditorBase {
        editor := CodeEditor {}
    }
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct StudioCodeEditor {
    #[uid]
    uid: WidgetUid,
    #[live]
    pub editor: CodeEditor,
}

impl WidgetNode for StudioCodeEditor {
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

impl Widget for StudioCodeEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Get session_id from the widget tree path (second-to-last element)
        let path = cx.widget_tree().path_to(self.widget_uid());
        let session_id = path
            .get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(LiveId(0));
        let app_scope = scope.data.get_mut::<AppData>().unwrap();
        if let Some(EditSession::Code(session)) = app_scope.file_system.get_session_mut(session_id)
        {
            self.editor.draw_walk_editor(cx, session, walk);
        } else {
            self.editor.draw_empty_editor(cx, walk);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let path = cx.widget_tree().path_to(self.widget_uid());
        let session_id = path
            .get(path.len().wrapping_sub(2))
            .copied()
            .unwrap_or(LiveId(0));
        let data = scope.data.get_mut::<AppData>().unwrap();
        let uid = self.widget_uid();
        if let Some(EditSession::Code(session)) = data.file_system.get_session_mut(session_id) {
            for action in self
                .editor
                .handle_event(cx, event, &mut Scope::empty(), session)
            {
                cx.widget_action(uid, action);
            }
            data.file_system.handle_sessions();
        }
    }
}
