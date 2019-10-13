use render::*;
use editor::*;
use hub::*;
use crate::app::*;

#[derive(Clone)]
pub struct BuildManager {
    pub signal: Signal,
    pub active_workspace: String,
    pub active_package: String,
    pub active_targets: Vec<BuildActiveTarget>,
    pub exec_when_done: bool,
    pub always_exec_when_done: bool,
    pub log_items: Vec<HubLogItem>,
    pub artifacts: Vec<String>,
}

impl BuildManager{
    pub fn new(cx:&mut Cx)->BuildManager{
        BuildManager{
            signal:cx.new_signal(),
            always_exec_when_done: true,
            exec_when_done: false,
            log_items: Vec::new(),
            artifacts: Vec::new(),
            active_workspace: "makepad".to_string(),
            active_package: "csvproc".to_string(),
            active_targets: vec![
                BuildActiveTarget::new("check"),
                BuildActiveTarget::new("build"),
                //CargoActiveTarget::new("workspace")
            ]
        }
    }
}

const SIGNAL_BUILD_MANAGER_NEW_LOG_ITEM:usize = 0;
const SIGNAL_BUILD_MANAGER_NEW_ARTIFACT:usize = 1;
const SIGNAL_BUILD_MANAGER_CARGO_EXEC_END:usize = 2;
const SIGNAL_BUILD_MANAGER_ARTIFACT_EXEC_END:usize = 3;

#[derive(Clone)]
pub struct BuildActiveTarget {
    target: String,
    artifact_path: Option<String>,
    cargo_uid: HubUid,
    artifact_uid: HubUid,
}

impl BuildActiveTarget {
    fn new(target: &str) -> BuildActiveTarget {
        BuildActiveTarget {
            target: target.to_string(),
            cargo_uid: HubUid::zero(),
            artifact_path: None,
            artifact_uid: HubUid::zero(),
        }
    }
}

impl BuildManager{

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
            if let Some(path) = &dm.path {
                let text_buffer = storage.text_buffer_from_path(cx, &path);
                
                let messages = &mut text_buffer.messages;
                messages.mutation_id = text_buffer.mutation_id;
                if messages.gc_id != cx.event_id {
                    messages.gc_id = cx.event_id;
                    messages.cursors.truncate(0);
                    messages.bodies.truncate(0);
                }
                //println!("{:?}", dm.item.level);
                if dm.level == HubLogItemLevel::Log {
                    break
                }
                // search for insert point
                let mut inserted = false;
                if messages.cursors.len()>0 {
                    for i in (0..messages.cursors.len()).rev() {
                        if dm.head < messages.cursors[i].head && (i == 0 || dm.head >= messages.cursors[i - 1].head) {
                            messages.cursors.insert(i, TextCursor {
                                head: dm.head,
                                tail: dm.tail,
                                max: 0
                            });
                            inserted = true;
                            break;
                        }
                    }
                }
                if !inserted {
                    messages.cursors.push(TextCursor {
                        head: dm.head,
                        tail: dm.tail,
                        max: 0
                    })
                }
                
                text_buffer.messages.bodies.push(TextBufferMessage {
                    body: dm.body.clone(),
                    level: match dm.level {
                        HubLogItemLevel::Warning => TextBufferMessageLevel::Warning,
                        HubLogItemLevel::Error => TextBufferMessageLevel::Error,
                        HubLogItemLevel::Log => TextBufferMessageLevel::Log,
                        HubLogItemLevel::Panic => TextBufferMessageLevel::Log,
                    }
                });
            }
            if dm.level == HubLogItemLevel::Log {
                break
            }
        }
        self.gc_textbuffer_messages(cx, storage);
    }
    
    pub fn is_running_uid(&self, uid: &HubUid) -> bool {
        for active_target in &self.active_targets {
            if active_target.cargo_uid == *uid {
                return true
            }
            if active_target.artifact_uid == *uid {
                return true
            }
        }
        return false
    }
    
    pub fn is_any_cargo_running(&self) -> bool {
        for active_target in &self.active_targets {
            if active_target.cargo_uid != HubUid::zero() {
                return true
            }
        }
        return false
    }
    
    pub fn is_any_artifact_running(&self) -> bool {
        for active_target in &self.active_targets {
            if active_target.artifact_uid != HubUid::zero() {
                return true
            }
        }
        return false
    }
    
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, storage: &mut AppStorage, htc: &HubToClientMsg)  {
        //let hub_ui = storage.hub_ui.as_mut().unwrap();
        match &htc.msg {
            HubMsg::CargoPackagesResponse {uid: _, packages: _} => {
            },
            HubMsg::CargoExecBegin {uid} => if self.is_running_uid(uid) {
            },
            HubMsg::LogItem {uid, item} => if self.is_running_uid(uid) {
                let mut export = false;
                if item.level == HubLogItemLevel::Warning || item.level == HubLogItemLevel::Error {
                    for check_msg in &self.log_items {
                        if *check_msg == *item { // ignore duplicates
                            return 
                        }
                        if check_msg.level != HubLogItemLevel::Warning && check_msg.level != HubLogItemLevel::Error {
                            break;
                        }
                    }
                    export = true;
                }
                self.log_items.push(item.clone());
                /*LogItemDraw {
                    animator: Animator::new(self.style.get_default_anim(cx, self.log_items.len(), false)),
                    item: item.clone(),
                    is_selected: false
                });*/
                if export {
                    self.export_messages_to_textbuffers(cx, storage);
                }
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_NEW_LOG_ITEM);
            },
            
            HubMsg::CargoArtifact {uid, package_id, fresh: _} => if self.is_running_uid(uid) {
                self.artifacts.push(package_id.clone());
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_NEW_ARTIFACT);
            },
            HubMsg::CargoExecEnd {uid, artifact_path} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for active_target in &mut self.active_targets {
                    if active_target.cargo_uid == *uid {
                        active_target.cargo_uid = HubUid::zero();
                        active_target.artifact_path = artifact_path.clone();
                    }
                }
                if !self.is_any_cargo_running() && self.exec_when_done {
                    self.exec_all_artifacts(storage)
                }
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_CARGO_EXEC_END);
            },
            HubMsg::ArtifactExecEnd {uid} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for active_target in &mut self.active_targets {
                    if active_target.artifact_uid == *uid {
                        active_target.artifact_uid = HubUid::zero();
                    }
                }
                cx.send_signal(self.signal, SIGNAL_BUILD_MANAGER_ARTIFACT_EXEC_END);
            },
            _ => ()
        }
    }
    
    pub fn exec_all_artifacts(&mut self, storage: &mut AppStorage) {
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        // otherwise execute all we have artifacts for
        for active_target in &mut self.active_targets {
            if let Some(artifact_path) = &active_target.artifact_path {
                let uid = hub_ui.alloc_uid();
                if active_target.artifact_uid != HubUid::zero() {
                    hub_ui.send(ClientToHubMsg {
                        to: HubMsgTo::Workspace(self.active_workspace.clone()),
                        msg: HubMsg::ArtifactKill {
                            uid: active_target.artifact_uid,
                        }
                    });
                }
                active_target.artifact_uid = uid;
                hub_ui.send(ClientToHubMsg {
                    to: HubMsgTo::Workspace(self.active_workspace.clone()),
                    msg: HubMsg::ArtifactExec {
                        uid: active_target.artifact_uid,
                        path: artifact_path.clone(),
                        args: Vec::new()
                    }
                });
            }
        }
    }
    
    pub fn artifact_exec(&mut self, storage: &mut AppStorage) {
        if self.is_any_cargo_running() {
            self.exec_when_done = true;
        }
        else {
            self.exec_all_artifacts(storage)
        }
    }
    
    pub fn restart_cargo(&mut self, cx: &mut Cx, storage: &mut AppStorage) {
        self.artifacts.truncate(0);
        self.log_items.truncate(0);
        //self.selection.truncate(0);
        self.gc_textbuffer_messages(cx, storage);
        
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        self.exec_when_done = self.always_exec_when_done;
        for active_target in &mut self.active_targets {
            active_target.artifact_path = None;
            if active_target.cargo_uid != HubUid::zero() {
                hub_ui.send(ClientToHubMsg {
                    to: HubMsgTo::Workspace(self.active_workspace.clone()),
                    msg: HubMsg::CargoKill {
                        uid: active_target.cargo_uid,
                    }
                });
                active_target.cargo_uid = HubUid::zero();
            }
            if active_target.artifact_uid != HubUid::zero() {
                hub_ui.send(ClientToHubMsg {
                    to: HubMsgTo::Workspace(self.active_workspace.clone()),
                    msg: HubMsg::ArtifactKill {
                        uid: active_target.artifact_uid,
                    }
                });
                active_target.artifact_uid = HubUid::zero();
            }
        }
        
        for active_target in &mut self.active_targets {
            let uid = hub_ui.alloc_uid();
            hub_ui.send(ClientToHubMsg {
                to: HubMsgTo::Workspace(self.active_workspace.clone()),
                msg: HubMsg::CargoExec {
                    uid: uid.clone(),
                    package: self.active_package.clone(),
                    target: active_target.target.clone()
                }
            });
            active_target.cargo_uid = uid;
        }
    }    
}
