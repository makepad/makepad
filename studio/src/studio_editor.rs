
use {
    crate::{
        app::{AppData},
        makepad_widgets::*,
        makepad_code_editor::CodeEditor,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_code_editor::code_editor::CodeEditor;
    
    StudioEditor = {{StudioEditor}}{
        editor: <CodeEditor>{
        }
    }
} 
 
#[derive(Live, LiveHook, Widget)] 
pub struct StudioEditor{
    #[wrap] #[live] pub editor: CodeEditor
}

impl Widget for StudioEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        // alright we have a scope, and an id, so now we can properly draw the editor.
        let session_id = scope.path.get(0);
        let app_scope = scope.data.get_mut::<AppData>();
        if let Some(session) = app_scope.file_system.get_session_mut(session_id){
            self.editor.draw_walk_editor(cx, session, walk);
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let session_id = scope.path.get(0);
        let data = scope.data.get_mut::<AppData>();
        let uid = self.widget_uid();
        if let Some(session) = data.file_system.get_session_mut(session_id){
            for action in self.editor.handle_event(cx, event, session){
                cx.widget_action(uid, &scope.path, action);
            }
            data.file_system.handle_sessions();
        }
    }
}