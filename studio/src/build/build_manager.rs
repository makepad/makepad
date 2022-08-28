#![allow(unused_imports)]
#![allow(dead_code)]
use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        app_state::AppState,
        editor_state::{
            DocumentId,
        },
        build::{
            build_protocol::{BuildMsg, BuildCmd, BuildCmdWrap, BuildMsgWrap, BuildCmdId},
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
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
        path::PathBuf
    },
};

live_register!{
    BuildManager: {{BuildManager}} {
        recompile_timeout: 0.2
    }
}

#[derive(Default)]
pub struct BuildState {
    clients: Vec<BuildClientWrap>,
}

pub struct BuildClientProcess {
    cmd_id: BuildCmdId,
    texture: Texture
}

pub struct BuildClientWrap {
    client: BuildClient,
    processes: HashMap<String, BuildClientProcess>,
}

#[derive(Live, LiveHook)]
pub struct BuildManager {
    path: String,
    recompile_timeout: f64,
    #[rust] recompile_timer: Timer,
}

pub enum BuildManagerAction {
    RedrawDoc {doc_id: DocumentId},
    RedrawLog,
    ClearLog,
    None
}

impl BuildManager {
    pub fn init(&mut self, cx: &mut Cx, state: &mut AppState) {
        let mut client = BuildClientWrap{
            client: BuildClient::new_with_local_server(&self.path),
            processes: HashMap::new()
        };
        
        let texture = Texture::new(cx);
        texture.set_desc(cx, TextureDesc {
            format: TextureFormat::SharedBGRA,
            width: Some(1024),
            height: Some(1024),
            multisample: None
        });
        
        client.processes.insert("cmdline_example".into(), BuildClientProcess {
            texture,
            cmd_id: BuildCmdId(0)
        });
        
        state.build_state.clients.push(client);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn file_change(&mut self, _cx: &mut Cx, state: &mut AppState) {
        for wrap in &mut state.build_state.clients {
            if let Some(process) = wrap.processes.get_mut("cmdline_example"){
                
                process.cmd_id = wrap.client.send_cmd(BuildCmd::CargoRun {
                    what: "cmdline_example".into(),
                    render_to: 0
                });
            }
        }
    }
    
    pub fn handle_event_vec(&mut self, cx: &mut Cx, event: &Event, state: &mut AppState) -> Vec<BuildManagerAction> {
        let mut actions = Vec::new();
        self.handle_event(cx, event, state, &mut | _, action | actions.push(action));
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
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, state: &mut AppState, dispatch_event: &mut dyn FnMut(&mut Cx, BuildManagerAction)) {
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
            wrap.client.handle_event(cx, event, &mut | cx, wrap | {
                let msg_id = editor_state.messages.len();
                match &wrap.msg {
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
                    }
                    _ => ()
                }
                editor_state.messages.push(wrap.msg);
                any_msg = true;
            });
        }
        if any_msg {
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
    }
}
