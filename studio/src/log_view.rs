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
    pub fn redraw(&mut self, cx:&mut Cx){
        self.fold_list.redraw(cx)
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, _editor_state: &EditorState) {
        if self.fold_list.begin(cx).is_ok(){
            for i in 0..100{
                self.fold_list.draw_node(cx, id_num!(test,i).into(), "this is a clickable link", true);
            }
            self.fold_list.end(cx);
        }
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, FoldListAction),
    ) {
        self.fold_list.handle_event_with_fn(cx, event, &mut |_cx, _action|{
            
        })
    }
}
