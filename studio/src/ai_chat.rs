
use {
    crate::{
        app::{AppData},
        makepad_widgets::*,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_code_editor::code_editor::CodeEditor;
    import makepad_widgets::theme_desktop_dark::*;
    
    AiChat = {{AiChat}}{
        height: Fill, width: Fill,
        md = <Markdown> {
        }
    }
} 
 
#[derive(Live, LiveHook, Widget)] 
pub struct AiChat{
    #[deref] view:View
}

impl Widget for AiChat {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, _walk:Walk)->DrawStep{
        // alright we have a scope, and an id, so now we can properly draw the editor.
        // alright lets fetch the chat-id from the scope
        let _chat_id = scope.path.from_end(1);
        self.view.draw_all_unscoped(cx);
        /*
        let session_id = scope.path.from_end(1);
        let app_scope = scope.data.get_mut::<AppData>().unwrap();
        if let Some(session) = app_scope.file_system.get_session_mut(session_id){
            self.editor.draw_walk_editor(cx, session, walk);
        }*/
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.view.handle_event(cx, event, scope);
        // we have an AI connection running on AppData
        let data = scope.data.get_mut::<AppData>().unwrap();
        
        /*
        if let Some(session) = data.file_system.get_session_mut(session_id){
            for action in self.editor.handle_event(cx, event, &mut Scope::empty(), session){
                cx.widget_action(uid, &scope.path, action);
            }
            data.file_system.handle_sessions();
        }*/
    }
}