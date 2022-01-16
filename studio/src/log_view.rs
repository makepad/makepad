use {
    crate::{
        makepad_studio_component::{
            log_list::{LogList, LogListAction}
        },
        makepad_render::*,
        editor_state::EditorState,
        //builder::{
        //    builder_protocol::BuilderMsg,
        //}
    },
};

live_register!{
    use makepad_render::shader::std::*;
    
    LogView: {{LogView}} {
    }
}

#[derive(Live, LiveHook)]
pub struct LogView {
    log_list: LogList
}

pub enum LogViewAction {
    None
}

impl LogView {
    pub fn redraw(&mut self, cx:&mut Cx){
        self.log_list.redraw(cx)
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, state: &EditorState) {
        let mut file = String::new();
        let mut body = String::new();
        if self.log_list.begin(cx).is_ok(){
            for (index,msg) in state.messages.iter().enumerate(){
                if self.log_list.should_node_draw(cx){
                    file.clear();
                    body.clear();
                    
                    /*let id = id_num!(msg, index).into();
                    match msg{
                        BuilderMsg::Bare(msg)=>{
                            write!(file, "")
                        }
                        BuilderMsg::Location(msg)=>{
                            
                        }
                    }*/
                    //let title = format!("{}", )
                    //self.fold_list.draw_node(cx, , )
                }
                
                //self.log_list.draw_node(cx, id_num!(test,i).into(), "this is a clickable link", true);
            }
            //for i in 0..100{
            // }
            self.log_list.end(cx);
        }
    }
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, LogListAction),
    ) {
        self.log_list.handle_event_with_fn(cx, event, &mut |_cx, _action|{
            
        })
    }
}
