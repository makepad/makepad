
use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_widgets::*,
        makepad_platform::os::cx_stdin::{
            HostToStdin,
            StdinToHost,
        },
        build_manager::{
            run_view::*,
            build_protocol::*,
            build_client::BuildClient
        },
        makepad_shell::*,
    },
    std::{
        cell::Cell,
        collections::HashMap,
        env,
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    BuildManager = {{BuildManager}} {
        recompile_timeout: 0.2
    }
}

pub struct ActiveBuild {
    pub item_id: LiveId,
    pub process: BuildProcess,
    pub run_view_id: LiveId,
    pub cmd_id: Option<BuildCmdId>,
    pub swapchain: [Texture; 2],
    pub present_index: Cell<usize>,
}

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct ActiveBuildId(pub LiveId);

#[derive(Default)]
pub struct ActiveBuilds {
    pub builds: HashMap<ActiveBuildId, ActiveBuild>
}

impl ActiveBuilds {
    pub fn item_id_active(&self, item_id:LiveId)->bool{
        for (_k, v) in &self.builds {
            if v.item_id == item_id{
                return true
            }
        }
        false
    }
    
    pub fn any_binary_active(&self, binary:&str)->bool{
        for (_k, v) in &self.builds {
            if v.process.binary == binary{
                return true
            }
        }
        false
    }
    
    pub fn build_id_from_cmd_id(&self, cmd_id: BuildCmdId) -> Option<ActiveBuildId> {
        for (k, v) in &self.builds {
            if v.cmd_id == Some(cmd_id) {
                return Some(*k);
            }
        }
        None
    }
    
    pub fn build_id_from_run_view_id(&self, run_view_id: LiveId) -> Option<ActiveBuildId> {
        for (k, v) in &self.builds {
            if v.run_view_id == run_view_id {
                return Some(*k);
            }
        }
        None
    }
    
    
    pub fn run_view_id_from_cmd_id(&self, cmd_id: BuildCmdId) -> Option<LiveId> {
        for v in self.builds.values() {
            if v.cmd_id == Some(cmd_id) {
                return Some(v.run_view_id);
            }
        }
        None
    }
    
    pub fn cmd_id_from_run_view_id(&self, run_view_id: LiveId) -> Option<BuildCmdId> {
        for v in self.builds.values() {
            if v.run_view_id == run_view_id {
                return v.cmd_id
            }
        }
        None
    }
    
}

#[derive(Live, LiveHook)]
pub struct BuildManager {
    #[live] path: String,
    #[rust] pub clients: Vec<BuildClient>,
    #[rust] pub log: Vec<(ActiveBuildId, LogItem)>,
    #[live] recompile_timeout: f64,
    #[rust] recompile_timer: Timer,
    #[rust] pub binaries: Vec<BuildBinary>,
    #[rust] pub active: ActiveBuilds
}

pub struct BuildBinary {
    pub open: f64,
    pub name: String
}


pub enum BuildManagerAction {
    RedrawDoc, // {doc_id: DocumentId},
    StdinToHost {run_view_id: LiveId, msg: StdinToHost},
    RedrawLog,
    ClearLog,
    None
}

impl BuildManager {
    
    pub fn init(&mut self, cx: &mut Cx) {
        self.clients = vec![BuildClient::new_with_local_server(&self.path)];
        self.update_run_list(cx);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    
    /*pub fn get_process_texture(&self, run_view_id: LiveId) -> Option<Texture> {
        for v in self.active.builds.values() {
            if v.run_view_id == run_view_id {
                return Some(v.texture.clone())
            }
        }
        None
    }*/
    
    pub fn send_host_to_stdin(&self, run_view_id: LiveId, msg: HostToStdin) {
        if let Some(cmd_id) = self.active.cmd_id_from_run_view_id(run_view_id) {
            self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::HostToStdin(msg.to_json()));
        }
    }
    
    pub fn update_run_list(&mut self, _cx: &mut Cx) {
        let cwd = std::env::current_dir().unwrap();
        self.binaries.clear();
        match shell_env_cap(&[], &cwd, "cargo", &["run", "--bin"]) {
            Ok(_) => {}
            // we expect it on stderr
            Err(e) => {
                let mut after_av = false;
                for line in e.split("\n") {
                    if after_av {
                        let binary = line.trim().to_string();
                        if binary.len()>0 {
                            self.binaries.push(BuildBinary {
                                open: 0.0,
                                name: binary
                            });
                        }
                    }
                    if line.contains("Available binaries:") {
                        after_av = true;
                    }
                }
            }
        }
    }
    
    pub fn handle_tab_close(&mut self, tab_id:LiveId)->bool{
        let len = self.active.builds.len();
        self.active.builds.retain(|_,v|{
            if v.run_view_id == tab_id{
                if let Some(cmd_id) = v.cmd_id {
                    self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
                }
                return false
            }
            true
        });
        if len != self.active.builds.len(){
            self.log.clear();
            true
        }
        else{
            false
        }
    }
    
    pub fn start_recompile(&mut self, _cx: &mut Cx) {
        // alright so. a file was changed. now what.
        for active_build in self.active.builds.values_mut() {
            if let Some(cmd_id) = active_build.cmd_id {
                self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
            }
            let cmd_id = self.clients[0].send_cmd(BuildCmd::Run(active_build.process.clone()));
            active_build.cmd_id = Some(cmd_id);
        }
    }
    
    pub fn clear_active_builds(&mut self) {
        // alright so. a file was changed. now what.
        for active_build in self.active.builds.values_mut() {
            if let Some(cmd_id) = active_build.cmd_id {
                self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
            }
        }
        self.active.builds.clear();
    }
    
    pub fn clear_log(&mut self) {
        self.log.clear();
    }
    
    pub fn start_recompile_timer(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
        for active_build in self.active.builds.values_mut() {
            let view = ui.run_view(&[active_build.run_view_id]);
            view.recompile_started(cx);
        }
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<BuildManagerAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, action | actions.push(action));
        actions
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_event: &mut dyn FnMut(&mut Cx, BuildManagerAction)) {
        if self.recompile_timer.is_event(event) {
            self.start_recompile(cx);
            self.clear_log();
            /*state.editor_state.messages.clear();
            for doc in &mut state.editor_state.documents.values_mut() {
                if let Some(inner) = &mut doc.inner {
                    inner.msg_cache.clear();
                }
            }*/
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
        let log = &mut self.log;
        let active = &mut self.active;
        //let editor_state = &mut state.editor_state;
        self.clients[0].handle_event_with(cx, event, &mut | cx, wrap | {
            //let msg_id = editor_state.messages.len();
            // ok we have a cmd_id in wrap.msg
            match wrap.item {
                LogItem::Location(loc) => {
                    if let Some(id) = active.build_id_from_cmd_id(wrap.cmd_id) {
                        log.push((id, LogItem::Location(loc)));
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                    }
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
                LogItem::Bare(bare) => {
                    if let Some(id) = active.build_id_from_cmd_id(wrap.cmd_id) {
                        log.push((id, LogItem::Bare(bare)));
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                    }
                    //editor_state.messages.push(wrap.msg);
                }
                LogItem::StdinToHost(line) => {
                    let msg: Result<StdinToHost, DeJsonErr> = DeJson::deserialize_json(&line);
                    match msg {
                        Ok(msg) => {
                            dispatch_event(cx, BuildManagerAction::StdinToHost {
                                run_view_id: active.run_view_id_from_cmd_id(wrap.cmd_id).unwrap_or(LiveId(0)),
                                msg
                            });
                        }
                        Err(_) => { // we should output a log string
                            if let Some(id) = active.build_id_from_cmd_id(wrap.cmd_id) {
                                log.push((id, LogItem::Bare(LogItemBare {
                                    level: LogItemLevel::Log,
                                    line: line.trim().to_string()
                                })));
                                dispatch_event(cx, BuildManagerAction::RedrawLog)
                            }
                            /*editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                level: BuildMsgLevel::Log,
                                line
                            }));*/
                        }
                    }
                }
            }
        });
    }
}
