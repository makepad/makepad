use {
    crate::{
        editor_state::{
            EditorState,
            DocumentId,
            SessionId,
            SessionView
        },
        code_editor::{
            protocol::{Notification, Request, Response},
        },
        genid::{GenId, GenIdMap,GenIdAllocator},
    },
    makepad_render::*,
    makepad_widget::*,
};

live_register!{
    use makepad_widget::scrollview::ScrollView;
    
    DesignEditorView: {{DesignEditorView}} {
        scroll_view:{
            view: {debug_id: code_editor_view}
        }
    }
    
    DesignEditors: {{DesignEditors}} {
        design_editor_view:DesignEditorView{},
    }
}

#[derive(Live, LiveHook)]
pub struct DesignEditorView {
    scroll_view: ScrollView,
    #[rust] session_id: Option<SessionId>,
}

#[derive(Live, LiveHook)]
pub struct DesignEditors {
    #[rust] view_id_allocator: GenIdAllocator,
    #[rust] views_by_view_id: GenIdMap<DesignEditorViewId, DesignEditorView>,
    design_editor_view: Option<LivePtr>,
}

impl DesignEditors{
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState, view_id: DesignEditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.draw(cx, state);
    }
    
    pub fn create_view(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        session_id: Option<SessionId>,
    ) -> DesignEditorViewId {
        let view_id = DesignEditorViewId(self.view_id_allocator.allocate());
        let mut view = DesignEditorView::new_from_ptr(cx, self.design_editor_view.unwrap());
        view.session_id = session_id;
        self.views_by_view_id.insert(
            view_id,
            view,
        );
        if let Some(session_id) = session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = Some(SessionView::DesignEditor(view_id));
        }
        view_id
    }
    
    pub fn view_session_id(&self, view_id: DesignEditorViewId) -> Option<SessionId> {
        let view = &self.views_by_view_id[view_id];
        view.session_id
    }
    
    pub fn set_view_session_id(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: DesignEditorViewId,
        session_id: Option<SessionId>,
    ) {
        let view = &mut self.views_by_view_id[view_id];
        if let Some(session_id) = view.session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = None;
        }
        view.session_id = session_id;
        if let Some(session_id) = view.session_id {
            let session = &mut state.sessions_by_session_id[session_id];
            session.session_view = Some(SessionView::DesignEditor(view_id));
            view.scroll_view.redraw(cx);
        }
    }
    
    pub fn redraw_view(&mut self, cx: &mut Cx, view_id: DesignEditorViewId) {
        let view = &mut self.views_by_view_id[view_id];
        view.scroll_view.redraw(cx);
    }
    
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        view_id: DesignEditorViewId,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
    ) {
        let view = &mut self.views_by_view_id[view_id];
        let mut actions = Vec::new();
        view.handle_event(cx, state, event, send_request, &mut | _, action | actions.push(action));
        for action in actions {
            match action {
                DesignEditorViewAction::RedrawViewsForDocument(document_id) => {
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
            if let Some(SessionView::DesignEditor(view_id)) = session.session_view {
                let view = &mut self.views_by_view_id[view_id];
                view.redraw(cx);
            }
        }
    }
    
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DesignEditorViewId(pub GenId);

impl AsRef<GenId> for DesignEditorViewId {
    fn as_ref(&self) -> &GenId {
        &self.0
    }
}

pub enum DesignEditorViewAction {
    RedrawViewsForDocument(DocumentId)
}

impl DesignEditorView {
    
    fn redraw(&self, cx: &mut Cx) {
        self.scroll_view.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
        if self.scroll_view.begin(cx).is_ok() {
            if let Some(session_id) = self.session_id {
                let session = &state.sessions_by_session_id[session_id];
                let document = &state.documents_by_document_id[session.document_id];
                if let Some(_document_inner) = document.inner.as_ref() {
                    // do stuff here
                }
            }
            self.scroll_view.end(cx);
        }
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        _send_request: &mut dyn FnMut(Request),
        _dispatch_action: &mut dyn FnMut(&mut Cx, DesignEditorViewAction),
    ) {
        if self.scroll_view.handle_event(cx, event) {
            self.scroll_view.redraw(cx);
        }
        match event.hits(cx, self.scroll_view.area(), HitOpt::default()) {
            Event::FingerDown(FingerDownEvent {..}) => {
                cx.set_key_focus(self.scroll_view.area());
                cx.set_down_mouse_cursor(MouseCursor::Text);
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let _document_inner = document.inner.as_ref().unwrap();
                    self.scroll_view.redraw(cx);
                }
            }
            Event::FingerHover(_) => {
                cx.set_hover_mouse_cursor(MouseCursor::Text);
            }
            Event::FingerMove(FingerMoveEvent { ..}) => {
                if let Some(session_id) = self.session_id {
                    let session = &state.sessions_by_session_id[session_id];
                    let document = &state.documents_by_document_id[session.document_id];
                    let _document_inner = document.inner.as_ref().unwrap();
                    self.scroll_view.redraw(cx);
                }
            }
            _ => {}
        }
    }
}