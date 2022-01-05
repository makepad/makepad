use {
    crate::{
        editor_state::EditorState
    },
    makepad_component::{
        fold_list::{FoldList, FoldListAction}
    },
    makepad_component::makepad_render::*,
};

live_register!{
    use makepad_render::shader::std::*;
    
    LogView: {{LogView}} {
    }
}

#[derive(Live, LiveHook)]
pub struct LogView {
    fold_list: FoldList
}

pub enum LogViewAction {
    None
}

impl LogView {
    
    pub fn redraw(&mut self, _cx:&mut Cx){
    }
    
    pub fn draw(&mut self, _cx: &mut Cx, _editor_state: &EditorState) {
    }
    
    pub fn handle_event(
        &mut self,
        _cx: &mut Cx,
        _event: &mut Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, FoldListAction),
    ) {
        
    }
}
