use {
    crate::{
        makepad_widgets::{
            ComponentMap,
            tab_bar::TabId,
        },
        makepad_draw_2d::*,
        editor_state::{
            EditorState,
            DocumentId,
            SessionId,
        },
        code_editor::{
            code_editor_impl::{
                CodeEditorAction
            },
        },
        makepad_collab_protocol::{
            CollabNotification,
            CollabRequest,
            CollabResponse,
        },
        rust_editor::{
            rust_editor::{
               RustEditor
            },
        }
    },

};

enum EditorView {
    RustEditor(RustEditor)
}

impl EditorView {
    pub fn redraw(&self, cx: &mut Cx) {
        match self {
            Self::RustEditor(e) => e.redraw(cx)
        }
    }
    
    pub fn set_session_id(&mut self, session_id: Option<SessionId>) {
        match self {
            Self::RustEditor(e) => e.set_session_id(session_id)
        }
    }
    
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::RustEditor(e) => e.session_id()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, state: &EditorState) {
        match self {
            Self::RustEditor(e) => e.draw(cx, state)
        }
    }
    
    pub fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        match self {
            Self::RustEditor(e) => e.apply(cx, from, index, nodes)
        }
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &Event,
        send_request: &mut dyn FnMut(CollabRequest),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
        match self {
            Self::RustEditor(e) => e.handle_event(cx, state, event, send_request, dispatch_action)
        }
    }
}

live_design!{
    import crate::rust_editor::rust_editor::RustEditor;
    
    Editors= {{Editors}} {
        rust_editor: <RustEditor> {},
    }
}


#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq)]
pub struct EditorViewId(pub TabId);
impl From<TabId> for EditorViewId {
    fn from(id: TabId) -> Self {Self (id)}
}

#[derive(Live)]
pub struct Editors {
    #[rust] editor_views: ComponentMap<EditorViewId, EditorView>,
    
    rust_editor: Option<LivePtr>,
}

impl LiveHook for Editors {
    fn after_apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        if let Some(index) = nodes.child_by_name(index, live_id!(live_editor).as_field()) {
            for editor_view in self.editor_views.values_mut() {
                editor_view.apply(cx, from, index, nodes);
            }
        }
    }
}

impl Editors {
    
    pub fn draw(&mut self, cx: &mut Cx2d, state: &EditorState, view_id: EditorViewId) {
        let view = &mut self.editor_views[view_id];
        view.draw(cx, state);
    }
    
    pub fn view_session_id(&self, view_id: EditorViewId) -> Option<SessionId> {
        let view = &self.editor_views[view_id];
        view.session_id()
    }
    
    pub fn set_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: EditorViewId,
        session_id: Option<SessionId>,
    ) {
        let rust_editor = self.rust_editor;
        let view = self.editor_views.get_or_insert(cx, view_id.into(), | cx | {
            EditorView::RustEditor(RustEditor::new_from_ptr(cx, rust_editor))
        });
        
        if let Some(session_id) = view.session_id() {
            let session = &mut state.sessions[session_id];
            session.session_view = None;
        }
        view.set_session_id(session_id);
        if let Some(session_id) = view.session_id() {
            let session = &mut state.sessions[session_id];
            session.session_view = Some(view_id);
            view.redraw(cx);
        }
    }
    
    pub fn has_editor(&self, view_id: EditorViewId) -> bool {
        self.editor_views.get(&view_id).is_some()
    }
    
    pub fn redraw_view(&mut self, cx: &mut Cx, view_id: EditorViewId) {
        let view = &mut self.editor_views[view_id];
        view.redraw(cx);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: EditorViewId,
        event: &Event,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        let view = &mut self.editor_views[view_id];
        let mut actions = Vec::new();
        view.handle_event_fn(cx, state, event, send_request, &mut | _, action | actions.push(action));
        for action in actions {
            match action {
                CodeEditorAction::RedrawViewsForDocument(document_id) => {
                    self.redraw_views_for_document(cx, state, document_id);
                }
                _ => ()
            }
        }
    }
    
    pub fn handle_collab_response(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        response: CollabResponse,
        send_request: &mut dyn FnMut(CollabRequest),
    ) {
        match response {
            CollabResponse::OpenFile(response) => {
                let (file_id, revision, text) = response.unwrap();
                let document_id = state.handle_open_file_response(file_id, revision, text, send_request);
                self.redraw_views_for_document(cx, state, document_id);
            }
            CollabResponse::ApplyDelta(response) => {
                let file_id = response.unwrap();
                state.handle_apply_delta_response(file_id, send_request);
            }
            _ => {}
        }
    }
    
    pub fn handle_collab_notification(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        notification: CollabNotification,
    ) {
        match notification {
            CollabNotification::DeltaWasApplied(file_id, delta) => {
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
        let document = &state.documents[document_id];
        for session_id in &document.session_ids {
            let session = &state.sessions[*session_id];
            if let Some(view_id) = session.session_view {
                let view = &mut self.editor_views[view_id]; 
                view.redraw(cx);
            }
        }
    }
}
