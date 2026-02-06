use crate::{
    code_editor::KeepCursorInView, decoration::DecorationSet, makepad_widgets::*, CodeDocument,
    CodeEditor, CodeSession,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.CodeViewBase = set_type_default() do #(CodeView::register_widget(vm)){
        editor +: {
            pad_left_top: vec2(0.0, -0.0)
            height: Fit
            empty_page_at_end: false
            read_only: true
            show_gutter: false
            word_wrap: false
            draw_bg +: { color: #0000 }
        }
    }

    mod.widgets.CodeView = mod.widgets.CodeViewBase {}
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct CodeView {
    #[live]
    pub editor: CodeEditor,
    // alright we have to have a session and a document.
    #[rust]
    pub session: Option<CodeSession>,
    #[live(false)]
    keep_cursor_at_end: bool,

    #[live]
    text: ArcStringMut,
}

impl WidgetNode for CodeView {
    fn walk(&mut self, cx: &mut Cx) -> Walk {
        self.editor.walk(cx)
    }
    fn area(&self) -> Area {
        self.editor.area()
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.editor.redraw(cx)
    }
    fn uid_to_widget(&self, uid: WidgetUid) -> WidgetRef {
        self.editor.uid_to_widget(uid)
    }
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        self.editor.find_widgets(path, cached, results)
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

    // Selection API - map to code editor document text
    fn selection_text_len(&self) -> usize {
        self.text.as_ref().len()
    }

    fn selection_point_to_char_index(&self, abs: DVec2) -> Option<usize> {
        // Use the editor's cached viewport rect for hit testing
        let rect = self.editor.viewport_rect();
        if rect.size.y <= 0.0 {
            return None;
        }
        let text = self.text.as_ref();
        let text_len = text.len();
        if text_len == 0 {
            return Some(0);
        }
        // Linear interpolation based on y position within the widget
        let local_y = (abs.y - rect.pos.y).max(0.0);
        let fraction = (local_y / rect.size.y).min(1.0);
        Some((fraction * text_len as f64) as usize)
    }

    fn selection_set(&mut self, _anchor: usize, _cursor: usize) {
        // Visual highlight in code editor is a future enhancement
    }

    fn selection_clear(&mut self) {
    }

    fn selection_select_all(&mut self) {
    }

    fn selection_get_text_for_range(&self, start: usize, end: usize) -> String {
        let text = self.text.as_ref();
        let start = start.min(text.len());
        let end = end.min(text.len());
        if start >= end {
            return String::new();
        }
        text[start..end].to_string()
    }

    fn selection_get_full_text(&self) -> String {
        self.text.as_ref().to_string()
    }
}

impl CodeView {
    pub fn lazy_init_session(&mut self) {
        if self.session.is_none() {
            let dec = DecorationSet::new();
            let doc = CodeDocument::new(self.text.as_ref().into(), dec);
            self.session = Some(CodeSession::new(doc));
            self.session.as_mut().unwrap().handle_changes();
            if self.keep_cursor_at_end {
                self.session.as_mut().unwrap().set_cursor_at_file_end();
                self.editor.keep_cursor_in_view = KeepCursorInView::Once
            }
        }
    }
}

impl Widget for CodeView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        // alright so.
        self.lazy_init_session();
        // alright we have a scope, and an id, so now we can properly draw the editor.
        let session = self.session.as_mut().unwrap();

        self.editor.draw_walk_editor(cx, session, walk);

        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.lazy_init_session();
        let session = self.session.as_mut().unwrap();
        for _action in self
            .editor
            .handle_event(cx, event, &mut Scope::empty(), session)
        {
            //cx.widget_action(uid, &scope.path, action);
            session.handle_changes();
        }
    }

    fn text(&self) -> String {
        self.text.as_ref().to_string()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text.as_ref() != v {
            self.text.as_mut_empty().push_str(v);
            self.session = None;
            self.redraw(cx);
        }
    }
}
