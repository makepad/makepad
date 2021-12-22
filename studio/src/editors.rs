use {
    crate::{
        editor_state::{
            EditorState,
            DocumentId,
            SessionId,
        },
        code_editor::{
            code_editor_impl::{
                CodeEditorAction
            },
            protocol::{Notification, Request, Response},
        },
        design_editor::{
            live_editor::{
                LiveEditor
            },
        }
    },
    makepad_render::*,
};

enum EditorView {
    LiveEditor(LiveEditor)
}

impl EditorView {
    pub fn redraw(&self, cx: &mut Cx) {
        match self {
            Self::LiveEditor(e) => e.redraw(cx)
        }
    }

    pub fn set_session_id(&mut self, session_id:Option<SessionId>) {
        match self {
            Self::LiveEditor(e) => e.set_session_id(session_id)
        }
    }
    
    pub fn session_id(&self)->Option<SessionId> {
        match self {
            Self::LiveEditor(e) => e.session_id()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        match self {
            Self::LiveEditor(e) => e.draw(cx, state)
        }
    }
    
    pub fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match self {
            Self::LiveEditor(e) => e.apply(cx, apply_from, index, nodes)
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        match self {
            Self::LiveEditor(e) => e.handle_event(cx, state, event, send_request, dispatch_action)
        }
    }
}

live_register!{
    use crate::design_editor::live_editor::LiveEditor;
    
    Editors: {{Editors}} {
        live_editor: LiveEditor {},
    }
}

#[derive(Live)]
pub struct Editors {
    #[rust] view_id_allocator: GenIdAllocator,
    #[rust] views_by_view_id: GenIdMap<EditorViewId, EditorView>,
    
    live_editor: Option<LivePtr>,
}

impl LiveHook for Editors{
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let ApplyFrom::ApplyOver = apply_from{
            for editor_view in self.views_by_view_id.values_mut(){
                if let Some(live_editor) = nodes.child_by_name(index, id!(live_editor)){
                    editor_view.apply(cx, apply_from, live_editor, nodes);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EditorViewId(pub GenId);

impl AsRef<GenId> for EditorViewId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}

impl Editors {
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState, view_id: EditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.draw(cx, state);
    }
    
    pub fn create_view(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        session_id: Option<SessionId>,
    ) -> EditorViewId {
        let view_id = EditorViewId(self.view_id_allocator.allocate());

        // TODO branch here on filetype somehow.
        let mut view = EditorView::LiveEditor(LiveEditor::new_from_ptr(cx, self.live_editor.unwrap()));

        view.set_session_id(session_id);
        self.views_by_view_id.insert(
            view_id,
            view,
        );
        if let Some(session_id) = session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = Some(view_id);
        }
        view_id
    }
    
    pub fn view_session_id(&self, view_id: EditorViewId) -> Option<SessionId> {
        let view = &self.views_by_view_id[view_id];
        view.session_id()
    }
    
    pub fn set_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: EditorViewId,
        session_id: Option<SessionId>,
    ) {
        let view = &mut self.views_by_view_id[view_id];
        if let Some(session_id) = view.session_id() {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = None;
        }
        view.set_session_id(session_id);
        if let Some(session_id) = view.session_id() {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = Some(view_id);
            view.redraw(cx);
        }
    }
    
    pub fn redraw_view(&mut self, cx: &mut Cx, view_id: EditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.redraw(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: EditorViewId,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
    ) {
        let view = &mut self.views_by_view_id[view_id];
        let mut actions = Vec::new();
        view.handle_event(cx, state, event, send_request, &mut | _, action | actions.push(action));
        for action in actions {
            match action {
                CodeEditorAction::RedrawViewsForDocument(document_id) => {
                    self.redraw_views_for_document(cx, state, document_id);
                }
            }
        }
    }
    
    pub fn handle_response(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        response: Response,
        send_request: &mut dyn FnMut(Request),
    ) {
        match response {
            Response::OpenFile(response) => {
                let (file_id, revision, text) = response.unwrap();
                let document_id =
                state.handle_open_file_response(file_id, revision, text, send_request);
                self.redraw_views_for_document(cx, state, document_id);
            }
            Response::ApplyDelta(response) => {
                let file_id = response.unwrap();
                state.handle_apply_delta_response(file_id, send_request);
            }
            _ => {}
        }
    }
    
    pub fn handle_notification(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        notification: Notification,
    ) {
        match notification {
            Notification::DeltaWasApplied(file_id, delta) => {
                let document_id = state.handle_delta_applied_notification(file_id, delta);
                self.redraw_views_for_document(cx, state, document_id);
            }
        }
    }
    
    pub fn redraw_views_for_document(
        &mut self,
        cx: &mut Cx,
        state: &EditorState,
        document_id: DocumentId,
    ) {
        let document = &state.documents_by_document_id[document_id];
        for session_id in &document.session_ids {
            let session = &state.sessions_by_session_id[*session_id];
            if let Some(view_id) = session.session_view {
                let view = &mut self.views_by_view_id[view_id];
                view.redraw(cx);
            }
        }
    }
    
}
