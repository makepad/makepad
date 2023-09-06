#![allow(unused_imports)]
#![allow(dead_code)]
use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_platform::os::cx_stdin::{
            HostToStdin,
            StdinToHost,
            StdinWindowSize
        },
        build_manager::{
            build_protocol::*,
            build_server::{BuildConnection, BuildServer},
            build_client::BuildClient
        },
        makepad_file_protocol::{
            FileNotification,
            FileRequest,
            FileResponse,
        },
        makepad_widgets::*,
        makepad_widgets::list_view::ListView,
    },
    std::{
        collections::HashMap,
        env,
        cell::Cell,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
        path::PathBuf
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    
    WaitIcon = <View> {
        width: 10, height: 10
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.circle(5., 5., 4.)
                sdf.fill(THEME_COLOR_TEXT_META)
                sdf.move_to(3., 5.)
                sdf.line_to(3., 5.)
                sdf.move_to(5., 5.)
                sdf.line_to(5., 5.)
                sdf.move_to(7., 5.)
                sdf.line_to(7., 5.)
                sdf.stroke(#0, 0.8)
                return sdf.result
            }
        }
    }
    
    LogItem = <RectView> {
        height: Fit, width: Fill
        padding: {top: 7, bottom: 7}
        
        draw_bg: {
            instance is_even: 0.0
            instance selected: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    THEME_COLOR_BG_SELECTED,
                    self.selected
                );
            }
        }
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                    },
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0}
                    }
                }
            }
        }
    }
    
    LogItemWait = <LogItem> {
        icon = <WaitIcon> {},
        label = <Label> {width: Fill draw_text: {wrap: Word}}
        link_label = <LinkLabel> {}
    }
    
    LogItemEmpty = <RectView> {
        height: 20, width: Fill
        draw_bg: {
            instance is_even: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    THEME_COLOR_BG_EDITOR,
                    THEME_COLOR_BG_ODD,
                    self.is_even
                )
            }
        }
    }
    
    LogList = <ListView> {
        grab_key_focus: true
        auto_tail: true
        drag_scrolling: true
        height: Fill, width: Fill
        flow: Down
        WaitEven = <LogItemWait> {draw_bg: {is_even: 1.0}}
        WaitOdd = <LogItemWait> {draw_bg: {is_even: 0.0}}
        EmptyEven = <LogItemEmpty> {draw_bg: {is_even: 1.0}}
        EmptyOdd = <LogItemEmpty> {draw_bg: {is_even: 0.0}}
    }
    
    BuildManager = {{BuildManager}} {
        recompile_timeout: 0.2
    }
}

pub struct BuildClientProcess {
    pub cmd_id: BuildCmdId,
    pub texture: Texture
}

pub struct BuildClientWrap {
    client: BuildClient,
    pub processes: HashMap<String, BuildClientProcess>,
}

#[derive(Live, LiveHook)]
pub struct BuildManager {
    #[live] path: String,
    #[live] recompile_timeout: f64,
    #[rust] pub clients: Vec<BuildClientWrap>,
    #[rust] recompile_timer: Timer,
    #[rust] pub messages: Vec<BuildMsg>,
}

pub enum BuildManagerAction {
    RedrawDoc, // {doc_id: DocumentId},
    StdinToHost {cmd_id: BuildCmdId, msg: StdinToHost},
    RedrawLog,
    ClearLog,
    None
}

const WHAT_TO_BUILD: &'static str = "makepad-example-news-feed";

impl BuildManager {
    
    pub fn draw_log_list(&self, cx: &mut Cx2d, list: &mut ListView) {

        list.set_item_range(cx, 0, self.messages.len() as u64);
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even = item_id&1 == 0;
            //log!("GOT ITEM ID {}", item_id);
            if let Some(msg) = self.messages.get(item_id as usize){
                match msg {
                    BuildMsg::Bare(msg) => {
                        let template = if is_even{live_id!(WaitEven)}else{live_id!(WaitOdd)};
                        let item = list.item(cx, item_id, template).unwrap().as_view();
                        item.label(id!(label)).set_text(&msg.line);
                        item.draw_widget_all(cx);
                    }
                    BuildMsg::Location(msg) => {
                        let template = if is_even{live_id!(WaitEven)}else{live_id!(WaitOdd)};
                        let item = list.item(cx, item_id, template).unwrap().as_view();
                        item.label(id!(link_label)).set_text(&msg.file_name);
                        item.label(id!(label)).set_text(&msg.msg);
                        item.draw_widget_all(cx);
                    }
                    _=>()
                }
            }
            else{ // draw empty items
                let template = if is_even{live_id!(EmptyEven)}else{live_id!(EmptyOdd)};
                let item = list.item(cx, item_id, template).unwrap().as_view();
                item.draw_widget_all(cx);
            }
        }
    }
    
    pub fn get_process(&mut self, cmd_id: BuildCmdId) -> Option<&mut BuildClientProcess> {
        for wrap in &mut self.clients {
            for process in wrap.processes.values_mut() {
                if process.cmd_id == cmd_id {
                    return Some(process)
                }
            }
        }
        return None
    }
    
    pub fn send_host_to_stdin(&self, cmd_id: Option<BuildCmdId>, msg: HostToStdin) {
        for wrap in &self.clients {
            for process in wrap.processes.values() {
                if cmd_id.is_none() || Some(process.cmd_id) == cmd_id {
                    wrap.client.send_cmd_with_id(process.cmd_id, BuildCmd::HostToStdin(msg.to_json()));
                    return;
                }
            }
        }
        log!("Send host to stdin process not found");
    }
    
    pub fn init(&mut self, cx: &mut Cx) {
        let mut client = BuildClientWrap {
            client: BuildClient::new_with_local_server(&self.path),
            processes: HashMap::new()
        };
        
        let texture = Texture::new(cx);
        
        client.processes.insert(WHAT_TO_BUILD.into(), BuildClientProcess {
            texture,
            cmd_id: BuildCmdId(0)
        });
        
        self.clients.push(client);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn file_change(&mut self, _cx: &mut Cx) {
        for wrap in &mut self.clients {
            if let Some(process) = wrap.processes.get_mut(WHAT_TO_BUILD) {
                self.messages.clear();
                process.cmd_id = wrap.client.send_cmd(BuildCmd::CargoRun {
                    what: WHAT_TO_BUILD.into(),
                });
            }
        }
    }
    
    pub fn start_recompile_timer(&mut self, cx:&mut Cx){
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<BuildManagerAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, action | actions.push(action));
        actions
    }
    /*
    pub fn handle_file_response(
        &mut self,
        cx: &mut Cx,
        response: &FileResponse,
    ) {
        match response {
            FileResponse::SaveFile(response) => {
                // lets see if we need to recompile at all
                
                // something changed for file_id
                let _file_id = response.clone().unwrap();
                cx.stop_timer(self.recompile_timer);
                self.recompile_timer = cx.start_timeout(self.recompile_timeout);
            }
            _ => {}
        }
    }*/
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_event: &mut dyn FnMut(&mut Cx, BuildManagerAction)) {
        if self.recompile_timer.is_event(event) {
            self.file_change(cx);
            self.messages.clear();
            /*state.editor_state.messages.clear();
            for doc in &mut state.editor_state.documents.values_mut() {
                if let Some(inner) = &mut doc.inner {
                    inner.msg_cache.clear();
                }
            }*/
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
        let mut any_msg = false;
        for wrap in &mut self.clients {
            let messages = &mut self.messages;
            //let editor_state = &mut state.editor_state;
            wrap.client.handle_event_with(cx, event, &mut | cx, wrap | {
                
                //let msg_id = editor_state.messages.len();
                // ok we have a cmd_id in wrap.msg
                match &wrap.msg {
                    BuildMsg::Location(_loc) => {
                        messages.push(wrap.msg);
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                        /*if let Some(doc_id) = editor_state.documents_by_path.get(UnixPath::new(&loc.file_name)) {
                            let doc = &mut editor_state.documents[*doc_id];
                            if let Some(inner) = &mut doc.inner {
                                inner.msg_cache.add_range(&inner.text, msg_id, loc.range);
                            }
                            dispatch_event(cx, BuildManagerAction::RedrawDoc {
                                doc_id: *doc_id
                            })
                        }*/
                        //editor_state.messages.push(BuildMsg::Location(loc));
                    }
                    BuildMsg::Bare(_) => {
                        messages.push(wrap.msg);
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                        //editor_state.messages.push(wrap.msg);
                    }
                    BuildMsg::StdinToHost(line) => {
                        let msg: Result<StdinToHost, DeJsonErr> = DeJson::deserialize_json(&line);
                        match msg {
                            Ok(msg) => {
                                dispatch_event(cx, BuildManagerAction::StdinToHost {
                                    cmd_id: wrap.cmd_id,
                                    msg
                                });
                            }
                            Err(_) => { // we should output a log string
                                /*editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                    level: BuildMsgLevel::Log,
                                    line
                                }));*/
                            }
                        }
                    }
                }
                any_msg = true;
            });
        }
        if any_msg {
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
    }
}
