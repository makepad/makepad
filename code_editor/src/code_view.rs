
use {
    crate::{
        {CodeDocument, decoration::{DecorationSet}, CodeSession},
        makepad_widgets::*,
        CodeEditor,
        code_editor::KeepCursorInView,
    },
    std::{
        env,
    },
};

live_design!{
    use crate::code_editor::CodeEditor;
        
    pub CodeView = {{CodeView}}{
        editor: <CodeEditor>{
            pad_left_top: vec2(0.0,-0.0)
            height:Fit
            empty_page_at_end: false,
            read_only: true,
            show_gutter: false
        }
    }
} 


#[derive(Live, LiveHook, Widget)] 
pub struct CodeView{
    #[wrap] #[live] pub editor: CodeEditor,
    // alright we have to have a session and a document.
     #[rust] pub session: Option<CodeSession>,
     #[live(false)] keep_cursor_at_end: bool,
     
    #[live] text: ArcStringMut,
}

impl CodeView{
    pub fn lazy_init_session(&mut self){
        if self.session.is_none(){
            let dec = DecorationSet::new();
            let doc = CodeDocument::new(self.text.as_ref().into(), dec);
            self.session = Some(CodeSession::new(doc));
            self.session.as_mut().unwrap().handle_changes();
            if self.keep_cursor_at_end{
                self.session.as_mut().unwrap().set_cursor_at_file_end();
                self.editor.keep_cursor_in_view = KeepCursorInView::Once
            }
        }
    }
}

impl Widget for CodeView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk:Walk)->DrawStep{
        // alright so. 
        self.lazy_init_session();
        // alright we have a scope, and an id, so now we can properly draw the editor.
        let session = self.session.as_mut().unwrap();
        
        self.editor.draw_walk_editor(cx, session, walk);
        
        DrawStep::done()
    }
        
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope){
        self.lazy_init_session();
        let session = self.session.as_mut().unwrap();
        for _action in self.editor.handle_event(cx, event, &mut Scope::empty(), session){
            //cx.widget_action(uid, &scope.path, action);
            session.handle_changes();
        }
    }
    
    fn text(&self)->String{
        self.text.as_ref().to_string()
    }
        
    fn set_text(&mut self, cx:&mut Cx, v:&str){
        if self.text.as_ref() != v{
            self.text.as_mut_empty().push_str(v);
            self.session = None;
            self.redraw(cx);
        }
    }
}
