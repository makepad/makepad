use {
    std::{
        fmt::Write,
    },
    crate::{
        build::{
            build_protocol::{
                BuildMsg,
                BuildMsgLevel
            }
        },
        makepad_widgets::{
            log_icon::LogIconType,
            log_list::{LogList, LogListAction}
        },
        makepad_draw_2d::*,
        editor_state::EditorState,
        //builder::{
        //    builder_protocol::BuilderMsg,
        //}
    },
};

live_design!{
    LogView = {{LogView}} {
    }
}

#[derive(Live, LiveHook)]
pub struct LogView {
    log_list: LogList
}

pub enum LogViewAction {
    None
}

impl Into<LogIconType> for BuildMsgLevel{
    fn into(self)->LogIconType{
        match self{
            BuildMsgLevel::Warning=>LogIconType::Warning,
            BuildMsgLevel::Error=>LogIconType::Error,
            BuildMsgLevel::Log=>LogIconType::Log,
            BuildMsgLevel::Wait=>LogIconType::Wait,
            BuildMsgLevel::Panic=>LogIconType::Panic,
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
        self.log_list.begin(cx);
        for (index, msg) in state.messages.iter().enumerate(){
            if self.log_list.should_node_draw(cx){
                file.clear();
                body.clear();
                let id = LiveId(index as  u64).into();
                match msg{
                    BuildMsg::Bare(msg)=>{
                        self.log_list.draw_node(cx, msg.level.into(), id, "", &msg.line, true);
                    }
                    BuildMsg::Location(msg)=>{
                        write!(file, "{}:{}", msg.file_name, msg.range.start.line).unwrap();
                        self.log_list.draw_node(cx, msg.level.into(), id, &file, &msg.msg, true);
                    }
                    _=>()
                }
            }
        }
        self.log_list.end(cx);
    }
    
    pub fn handle_event_fn(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        _dispatch_action: &mut dyn FnMut(&mut Cx, LogListAction),
    ) {
        self.log_list.handle_event_fn(cx, event, &mut |_,_|{})
    }
}
