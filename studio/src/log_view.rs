use {
    std::{
        fmt::Write,
    },
    crate::{
        builder::{
            builder_protocol::{
                BuilderMsg,
                BuilderMsgLevel
            }
        },
        makepad_studio_component::{
            log_icon::LogIconType,
            log_list::{LogList, LogListAction}
        },
        makepad_platform::*,
        editor_state::EditorState,
        //builder::{
        //    builder_protocol::BuilderMsg,
        //}
    },
};

live_register!{
    use makepad_platform::shader::std::*;
    
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

impl Into<LogIconType> for BuilderMsgLevel{
    fn into(self)->LogIconType{
        match self{
            BuilderMsgLevel::Warning=>LogIconType::Warning,
            BuilderMsgLevel::Error=>LogIconType::Error,
            BuilderMsgLevel::Log=>LogIconType::Ok,
        }
    }
}

impl LogView {
    pub fn redraw(&mut self, cx:&mut Cx){
        self.log_list.redraw(cx)
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, state: &EditorState) {
        let mut file = String::new();
        let mut body = String::new();
        if self.log_list.begin(cx).is_ok(){
            for (index, msg) in state.messages.iter().enumerate(){
                if self.log_list.should_node_draw(cx){
                    file.clear();
                    body.clear();
                    let id = LiveId(index as  u64).into();
                    match msg{
                        BuilderMsg::Bare(_msg)=>{
                        }
                        BuilderMsg::Location(msg)=>{
                            write!(file, "{}:{}", msg.file_name, msg.range.start.line).unwrap();
                            self.log_list.draw_node(cx, msg.level.into(), id, &file, &msg.msg, true);
                        }
                    }
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
