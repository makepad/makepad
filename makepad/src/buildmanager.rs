use makepad_render::*;
use makepad_widget::*;
use makepad_hub::*;
use crate::appstorage::*;
use crate::searchindex::*;

#[derive(Clone)]
pub struct BuildManager {
    pub signal: Signal,
    pub active_builds: Vec<ActiveBuild>,
    pub exec_when_done: bool,
    pub log_items: Vec<HubLogItem>,
    pub search_index: SearchIndex,
    pub tail_log_items: bool,
    pub artifacts: Vec<String>,
}

impl BuildManager {
    pub fn new(cx: &mut Cx) -> BuildManager {
        BuildManager {
            signal: cx.new_signal(),
            exec_when_done: false,
            log_items: Vec::new(),
            tail_log_items: true, 
            artifacts: Vec::new(),
            active_builds: Vec::new(),
            search_index: SearchIndex::new(),
        }
    }
    
    pub fn status_new_log_item()->StatusId{uid!()}
    pub fn status_new_artifact()->StatusId{uid!()}
    pub fn status_cargo_end()->StatusId{uid!()}
    pub fn status_program_end()->StatusId{uid!()}
}

#[derive(Clone)]
pub struct ActiveBuild {
    pub build_target: BuildTarget,
    pub build_result: Option<BuildResult>,
    pub build_uid: Option<HubUid>,
    pub run_uid: Option<HubUid>,
}

impl BuildManager {
    
    fn clear_textbuffer_messages(&self, cx: &mut Cx, storage: &mut AppStorage) {
        // clear all files we missed
        for atb in &mut storage.text_buffers {
            //if atb.text_buffer.messages.gc_id != cx.event_id {
                atb.text_buffer.markers.message_cursors.truncate(0);
                atb.text_buffer.markers.message_bodies.truncate(0);
                cx.send_signal(atb.text_buffer.signal, TextBuffer::status_message_update());
           // }
            //else {
            //    cx.send_signal(atb.text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            //}
        }
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
            HubMsg::ListBuildersResponse {..} => {
                self.restart_build(cx, storage);
            },
            HubMsg::CargoBegin {uid} => if self.is_running_uid(uid) {
            },
            HubMsg::LogItem {uid, item} => if self.is_running_uid(uid) {
                if self.log_items.len() >= 700000 { // out of memory safety
                    if self.tail_log_items{
                        self.log_items.truncate(500000);
                        self.log_items.push(HubLogItem::Message("------------ Log truncated here -----------".to_string()));
                    }
                    else{ // if not tailing, just throw it away
                        if self.log_items.len() != 700001{
                            self.log_items.push(HubLogItem::Message("------------ Log skipping, press tail to resume -----------".to_string()));
                            cx.send_signal(self.signal, BuildManager::status_new_log_item());
                        }
                        return
                    }
                }
                
                self.log_items.push(item.clone());
                if let Some(loc_message) = item.get_loc_message() {
                    let atb = storage.text_buffer_from_path(cx, &storage.remap_sync_path(&loc_message.path));
                    let markers = &mut atb.text_buffer.markers;
                    markers.mutation_id = atb.text_buffer.mutation_id.max(1);
                    if markers.message_cursors.len() > 100000{ // crash saftey
                        return
                    }
                    if let Some((head, tail)) = loc_message.range {
                        let mut inserted = None;
                        if markers.message_cursors.len()>0 {
                            for i in (0..markers.message_cursors.len()).rev() {
                                if head >= markers.message_cursors[i].head {
                                    break;
                                }
                                if head < markers.message_cursors[i].head && (i == 0 || head >= markers.message_cursors[i - 1].head) {
                                    markers.message_cursors.insert(i, TextCursor {
                                        head: head,
                                        tail: tail,
                                        max: 0
                                    });
                                    inserted = Some(i);
                                    break;
                                }
                            }
                        }
                        if inserted.is_none() {
                            if let Some((head, tail)) = loc_message.range {
                                markers.message_cursors.push(TextCursor {
                                    head: head,
                                    tail: tail,
                                    max: 0
                                })
                            }
                        }
                        
                        let msg = TextBufferMessage {
                            body: loc_message.body.clone(),
                            level: match item {
                                HubLogItem::LocPanic(_) => TextBufferMessageLevel::Log,
                                HubLogItem::LocError(_) => TextBufferMessageLevel::Error,
                                HubLogItem::LocWarning(_) => TextBufferMessageLevel::Warning,
                                HubLogItem::LocMessage(_) => TextBufferMessageLevel::Log,
                                HubLogItem::Error(_) => TextBufferMessageLevel::Error,
                                HubLogItem::Warning(_) => TextBufferMessageLevel::Warning,
                                HubLogItem::Message(_) => TextBufferMessageLevel::Log,
                            }
                        };
                        if let Some(pos) = inserted {
                            atb.text_buffer.markers.message_bodies.insert(pos, msg);
                        }
                        else {
                            atb.text_buffer.markers.message_bodies.push(msg);
                        }
                        cx.send_signal(atb.text_buffer.signal, TextBuffer::status_message_update());
                    }
                }
                cx.send_signal(self.signal, BuildManager::status_new_log_item());
            },
            
            HubMsg::CargoArtifact {uid, package_id, fresh: _} => if self.is_running_uid(uid) {
                self.artifacts.push(package_id.clone());
                cx.send_signal(self.signal, BuildManager::status_new_artifact());
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
                cx.send_signal(self.signal, BuildManager::status_cargo_end());
            },
            HubMsg::ProgramEnd {uid} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for ab in &mut self.active_builds {
                    if ab.run_uid == Some(*uid) {
                        ab.run_uid = None;
                    }
                }
                cx.send_signal(self.signal, BuildManager::status_program_end());
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
                            to: HubMsgTo::Builder(ab.build_target.builder.clone()),
                            msg: HubMsg::ProgramKill {
                                uid: run_uid,
                            }
                        });
                    }
                    ab.run_uid = Some(uid);
                    hub_ui.route_send.send(ToHubMsg {
                        to: HubMsgTo::Builder(ab.build_target.builder.clone()),
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
        self.clear_textbuffer_messages(cx, storage);
        
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        self.exec_when_done = storage.settings.exec_when_done;
        for ab in &mut self.active_builds {
            ab.build_result = None;
            if let Some(build_uid) = ab.build_uid {
                hub_ui.route_send.send(ToHubMsg {
                    to: HubMsgTo::Builder(ab.build_target.builder.clone()),
                    msg: HubMsg::BuildKill {
                        uid: build_uid,
                    }
                });
                ab.build_uid = None
            }
            if let Some(run_uid) = ab.run_uid {
                hub_ui.route_send.send(ToHubMsg {
                    to: HubMsgTo::Builder(ab.build_target.builder.clone()),
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
                to: HubMsgTo::Builder(build_target.builder.clone()),
                msg: HubMsg::Build {
                    uid: uid.clone(),
                    workspace: build_target.workspace.clone(),
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
