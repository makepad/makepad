
use {
    crate::{
        app::{AppScope},
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
        editor: <CodeEditor>{}
    }
} 
 
#[derive(Live)] 
pub struct StudioEditor{
    #[live] pub editor: CodeEditor
} 
 
impl LiveHook for StudioEditor{
    fn before_live_design(cx:&mut Cx){
        register_widget!(cx, StudioEditor)
    }
}

impl Widget for StudioEditor {
    fn redraw(&mut self, cx: &mut Cx) {
        self.editor.redraw(cx);
    }
    
    fn walk(&mut self, cx:&mut Cx) -> Walk {
        self.editor.walk(cx)
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut WidgetScope, walk:Walk)->WidgetDraw{
        // alright we have a scope, and an id, so now we can properly draw the editor.
        let session_id = scope.path.path_id(0);
        let app_scope = scope.data.get_mut::<AppScope>();
        if let Some(session) = app_scope.file_system.get_session_mut(session_id){
            self.editor.draw_walk_editor(cx, session, walk);
        }
        WidgetDraw::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut WidgetScope)->WidgetActions{
        let session_id = scope.path.path_id(0);
        let app_scope = scope.data.get_mut::<AppScope>();
        let uid = self.widget_uid();
        let mut actions = WidgetActions::new(); 
        if let Some(session) = app_scope.file_system.get_session_mut(session_id){
            for action in self.editor.handle_event(cx, event, session){
                actions.push_single(uid, &scope.path, action);
            }
            app_scope.file_system.handle_sessions();
        }
        actions
    }
}
#[derive(Clone, Debug, PartialEq, WidgetRef)]
pub struct StudioEditorRef(WidgetRef);