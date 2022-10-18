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
        app_state::AppState,
        editor_state::{
            DocumentId,
        },
        build::{
            build_protocol::*,
            build_server::{BuildConnection, BuildServer},
            build_client::BuildClient
        },
        makepad_collab_protocol::{
            CollabNotification,
            CollabRequest,
            CollabResponse,
            unix_path::{UnixPath},
        },
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
    BuildManager= {{BuildManager}} {
        recompile_timeout: 0.2
    }
}

#[derive(Default)]
pub struct BuildState {
    pub clients: Vec<BuildClientWrap>,
}

impl BuildState {
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
    path: String,
    recompile_timeout: f64,
    #[rust] recompile_timer: Timer,
}

pub enum BuildManagerAction {
    RedrawDoc {doc_id: DocumentId},
    StdinToHost {cmd_id: BuildCmdId, msg: StdinToHost},
    RedrawLog,
    ClearLog,
    None
}

const WHAT_TO_BUILD:&'static str = "fractal_zoom";

impl BuildManager {
    pub fn init(&mut self, cx: &mut Cx, state: &mut AppState) {
        let mut client = BuildClientWrap {
            client: BuildClient::new_with_local_server(&self.path),
            processes: HashMap::new()
        };
        
        let texture = Texture::new(cx);
        
        client.processes.insert(WHAT_TO_BUILD.into(), BuildClientProcess {
            texture,
            cmd_id: BuildCmdId(0)
        });
        
        state.build_state.clients.push(client);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn file_change(&mut self, _cx: &mut Cx, state: &mut AppState) {
        for wrap in &mut state.build_state.clients {
            if let Some(process) = wrap.processes.get_mut(WHAT_TO_BUILD) {
                
                process.cmd_id = wrap.client.send_cmd(BuildCmd::CargoRun {
                    what: WHAT_TO_BUILD.into(),
                });
            }
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, state: &mut AppState) -> Vec<BuildManagerAction> {
        let mut actions = Vec::new();
        self.handle_event_fn(cx, event, state, &mut | _, action | actions.push(action));
        actions
    }
    
    pub fn handle_collab_response(
        &mut self,
        cx: &mut Cx,
        _state: &mut AppState,
        response: &CollabResponse,
    ) {
        match response {
            CollabResponse::ApplyDelta(response) => {
                // something changed for file_id
                let _file_id = response.clone().unwrap();
                cx.stop_timer(self.recompile_timer);
                self.recompile_timer = cx.start_timeout(self.recompile_timeout);
            }
            _ => {}
        }
    }
    
    pub fn handle_event_fn(&mut self, cx: &mut Cx, event: &Event, state: &mut AppState, dispatch_event: &mut dyn FnMut(&mut Cx, BuildManagerAction)) {
        if self.recompile_timer.is_event(event) {
            self.file_change(cx, state);
            state.editor_state.messages.clear();
            for doc in &mut state.editor_state.documents.values_mut() {
                if let Some(inner) = &mut doc.inner {
                    inner.msg_cache.clear();
                }
            }
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
        let mut any_msg = false;
        for wrap in &mut state.build_state.clients {
            let editor_state = &mut state.editor_state;
            wrap.client.handle_event_fn(cx, event, &mut | cx, wrap | {
                let msg_id = editor_state.messages.len();
                // ok we have a cmd_id in wrap.msg
                match wrap.msg {
                    BuildMsg::Location(loc) => {
                        if let Some(doc_id) = editor_state.documents_by_path.get(UnixPath::new(&loc.file_name)) {
                            let doc = &mut editor_state.documents[*doc_id];
                            if let Some(inner) = &mut doc.inner {
                                inner.msg_cache.add_range(&inner.text, msg_id, loc.range);
                            }
                            dispatch_event(cx, BuildManagerAction::RedrawDoc {
                                doc_id: *doc_id
                            })
                        }
                        editor_state.messages.push(BuildMsg::Location(loc));
                    }
                    BuildMsg::Bare(_) => {
                        editor_state.messages.push(wrap.msg);
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
                                editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                    level: BuildMsgLevel::Log,
                                    line
                                }));
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
