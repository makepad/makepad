use render::*;
use editor::*;
use hub::*;
use crate::appstorage::*;

#[derive(Clone)]
pub struct BuildManager {
    pub signal: Signal,
    pub active_builds: Vec<ActiveBuild>,
    pub exec_when_done: bool,
    pub log_items: Vec<HubLogItem>,
    pub artifacts: Vec<String>,
}

impl BuildManager {
    pub fn new(cx: &mut Cx) -> BuildManager {
        BuildManager {
            signal: cx.new_signal(),
            exec_when_done: false,
            log_items: Vec::new(),
            artifacts: Vec::new(),
            active_builds: Vec::new(),
        }
    }
}

const SIGNAL_BUILD_MANAGER_NEW_LOG_ITEM: usize = 0;
const SIGNAL_BUILD_MANAGER_NEW_ARTIFACT: usize = 1;
const SIGNAL_BUILD_MANAGER_CARGO_EXEC_END: usize = 2;
const SIGNAL_BUILD_MANAGER_ARTIFACT_EXEC_END: usize = 3;

#[derive(Clone)]
pub struct ActiveBuild {
    pub build_target: BuildTarget,
    pub build_result: Option<BuildResult>,
    pub build_uid: Option<HubUid>,
    pub run_uid: Option<HubUid>,
}

impl BuildManager {
    
    fn gc_textbuffer_messages(&self, cx: &mut Cx, storage: &mut AppStorage) {
        // clear all files we missed
        for (_, atb) in &mut storage.text_buffers {
            if atb.text_buffer.messages.gc_id != cx.event_id {
                atb.text_buffer.messages.cursors.truncate(0);
                atb.text_buffer.messages.bodies.truncate(0);
                cx.send_signal(atb.text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            }
            else {
                cx.send_signal(atb.text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            }
        }
    }
    
    pub fn export_messages_to_textbuffers(&self, cx: &mut Cx, storage: &mut AppStorage) {
        
        for dm in &self.log_items {
            //println!("{:?}", dm.item.level);
            if let Some(loc_message) = dm.get_loc_message() {
                
                let text_buffer = storage.text_buffer_from_path(cx, &loc_message.path);
                
                let messages = &mut text_buffer.messages;
                messages.mutation_id = text_buffer.mutation_id;
                if messages.gc_id != cx.event_id {
                    messages.gc_id = cx.event_id;
                    messages.cursors.truncate(0);
                    messages.bodies.truncate(0);
                }
                
                // search for insert point
                let mut inserted = false;
                if messages.cursors.len()>0 {
                    for i in (0..messages.cursors.len()).rev() {
                        if let Some((head, tail)) = loc_message.range {
                            if head < messages.cursors[i].head && (i == 0 || head >= messages.cursors[i - 1].head) {
                                messages.cursors.insert(i, TextCursor {
                                    head: head,
                                    tail: tail,
                                    max: 0
                                });
                                inserted = true;
                                break;
                            }
                        }
                    }
                }
                if !inserted {
                    if let Some((head, tail)) = loc_message.range {
                        messages.cursors.push(TextCursor {
                            head: head,
                            tail: tail,
                            max: 0
                        })
                    }
                }
                
                text_buffer.messages.bodies.push(TextBufferMessage {
                    body: loc_message.body.clone(),
                    level: match dm {
                        HubLogItem::LocPanic(_) => TextBufferMessageLevel::Log,
                        HubLogItem::LocError(_) => TextBufferMessageLevel::Error,
                        HubLogItem::LocWarning(_) => TextBufferMessageLevel::Warning,
                        HubLogItem::LocMessage(_) => TextBufferMessageLevel::Log,
                        HubLogItem::Error(_) => TextBufferMessageLevel::Error,
                        HubLogItem::Warning(_) => TextBufferMessageLevel::Warning,
                        HubLogItem::Message(_) => TextBufferMessageLevel::Log,
                    }
                });
            }
            else {
                break
            }
        }
        self.gc_textbuffer_messages(cx, storage);
    }
    
    pub fn is_running_uid(&self, uid: &HubUid) -> bool {
        for ab in &self.active_builds {
            if ab.build_uid == Some(*uid) {
                return true
            }
            if ab.run_uid == Some(*uid) {
                return true
            }
        }
        return false
    }
    
    pub fn is_any_cargo_running(&self) -> bool {
        for ab in &self.active_builds {
            if ab.build_uid.is_some() {
                return true
            }
        }
        return false
    }
    
    pub fn is_any_artifact_running(&self) -> bool {
        for ab in &self.active_builds {
            if ab.run_uid.is_some() {
                return true
            }
        }
        return false
    }
    
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, storage: &mut AppStorage, htc: &FromHubMsg) {
        //let hub_ui = storage.hub_ui.as_mut().unwrap();
        match &htc.msg {
            HubMsg::ListPackagesResponse {uid: _, packages: _} => {
            },
            HubMsg::CargoBegin {uid} => if self.is_running_uid(uid) {
            },
            HubMsg::LogItem {uid, item} => if self.is_running_uid(uid) {
                let mut export = false;
                if let Some(loc_msg) = item.get_loc_message() {
                    for check_msg in &self.log_items {
                        if let Some(check_msg) = check_msg.get_loc_message() {
                            if check_msg.body == loc_msg.body
                                && check_msg.row == loc_msg.row
                                && check_msg.col == loc_msg.col
                                && check_msg.path == loc_msg.path { // ignore duplicates
                                return
                            }
                        }
                        else {
                            break;
                        }
                    }
                    export = true;
                }
                else {
                }
                self.log_items.push(item.clone());
                if export {
                    self.export_messages_to_textbuffers(cx, storage);
                }
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_NEW_LOG_ITEM);
                //println!("LOG ITEM RECEIVED");
            },
            
            HubMsg::CargoArtifact {uid, package_id, fresh: _} => if self.is_running_uid(uid) {
                self.artifacts.push(package_id.clone());
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_NEW_ARTIFACT);
            },
            HubMsg::BuildFailure {uid} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for ab in &mut self.active_builds {
                    if ab.build_uid == Some(*uid) {
                        ab.build_uid = None;
                    }
                }
            },
            HubMsg::CargoEnd {uid, build_result} => if self.is_running_uid(uid) {
                for ab in &mut self.active_builds {
                    if ab.build_uid == Some(*uid) {
                        ab.build_uid = None;
                        ab.build_result = Some(build_result.clone());
                    }
                }
                if !self.is_any_cargo_running() && self.exec_when_done {
                    self.run_all_artifacts(storage)
                }
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_CARGO_EXEC_END);
            },
            HubMsg::ProgramEnd {uid} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for ab in &mut self.active_builds {
                    if ab.run_uid == Some(*uid) {
                        ab.run_uid = None;
                    }
                }
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_ARTIFACT_EXEC_END);
            },
            _ => ()
        }
    }
    
    pub fn run_all_artifacts(&mut self, storage: &mut AppStorage) {
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        // otherwise execute all we have artifacts for
        for ab in &mut self.active_builds {
            if let Some(build_result) = &ab.build_result {
                if let BuildResult::Executable {path} = build_result {
                    let uid = hub_ui.route_send.alloc_uid();
                    if let Some(run_uid) = ab.run_uid {
                        hub_ui.route_send.send(ToHubMsg {
                            to: HubMsgTo::Workspace(ab.build_target.workspace.clone()),
                            msg: HubMsg::ProgramKill {
                                uid: run_uid,
                            }
                        });
                    }
                    ab.run_uid = Some(uid);
                    hub_ui.route_send.send(ToHubMsg {
                        to: HubMsgTo::Workspace(ab.build_target.workspace.clone()),
                        msg: HubMsg::ProgramRun {
                            uid: ab.run_uid.unwrap(),
                            path: path.clone(),
                            args: Vec::new()
                        }
                    });
                }
            }
        }
    }
    
    pub fn artifact_run(&mut self, storage: &mut AppStorage) {
        if self.is_any_cargo_running() {
            self.exec_when_done = true;
        }
        else {
            self.run_all_artifacts(storage)
        }
    }
    
    pub fn restart_build(&mut self, cx: &mut Cx, storage: &mut AppStorage) {
        if !cx.platform_type.is_desktop() {
            return
        }
        
        self.artifacts.truncate(0);
        self.log_items.truncate(0);
        //self.selection.truncate(0);
        self.gc_textbuffer_messages(cx, storage);
        
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        self.exec_when_done = storage.settings.exec_when_done;
        for ab in &mut self.active_builds {
            ab.build_result = None;
            if let Some(build_uid) = ab.build_uid {
                hub_ui.route_send.send(ToHubMsg {
                    to: HubMsgTo::Workspace(ab.build_target.workspace.clone()),
                    msg: HubMsg::BuildKill {
                        uid: build_uid,
                    }
                });
                ab.build_uid = None
            }
            if let Some(run_uid) = ab.run_uid {
                hub_ui.route_send.send(ToHubMsg {
                    to: HubMsgTo::Workspace(ab.build_target.workspace.clone()),
                    msg: HubMsg::ProgramKill {
                        uid: run_uid,
                    }
                });
                ab.run_uid = None
            }
        }
        
        // lets reset active targets
        self.active_builds.truncate(0);
        
        for build_target in &storage.settings.builds {
            let uid = hub_ui.route_send.alloc_uid();
            hub_ui.route_send.send(ToHubMsg {
                to: HubMsgTo::Workspace(build_target.workspace.clone()),
                msg: HubMsg::Build {
                    uid: uid.clone(),
                    project: build_target.project.clone(),
                    package: build_target.package.clone(),
                    config: build_target.config.clone()
                }
            });
            self.active_builds.push(ActiveBuild {
                build_target: build_target.clone(),
                build_result: None,
                build_uid: Some(uid),
                run_uid: None
            })
        }
    }
}
