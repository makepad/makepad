use {
    crate::{
        editor_state::{
            EditorState,
        },
        code_editor::{
            protocol::Request,
            code_editor_impl::{CodeEditorImpl, CodeEditorAction}
        },
        editor_state::{
            SessionId
        },
    },
    makepad_render::*,
};

live_register!{
    use makepad_render::shader::std::*;
    
    RustEditor: {{RustEditor}} {
        editor_impl: {}
    }
}

#[derive(Live, LiveHook)]
pub struct RustEditor {
    editor_impl: CodeEditorImpl
}

impl RustEditor {
    
    pub fn set_session_id(&mut self, session_id:Option<SessionId>) {
        self.editor_impl.session_id = session_id;
    }

    pub fn session_id(&self)->Option<SessionId> {
        self.editor_impl.session_id
    }

    
    pub fn redraw(&self, cx: &mut Cx) {
        self.editor_impl.redraw(cx);
    }
    
    pub fn draw(&mut self, cx: &mut Cx, state: &EditorState) {
       self.editor_impl.draw(cx, state);
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        state: &mut EditorState,
        event: &mut Event,
        send_request: &mut dyn FnMut(Request),
        dispatch_action: &mut dyn FnMut(&mut Cx, CodeEditorAction),
    ) {
       self.editor_impl.handle_event(cx, state, event, send_request, dispatch_action);
    }
    
}

