use makehub::*;
use crate::process::*;
use serde::{Deserialize};
use std::sync::{Arc, Mutex};

pub struct Make {
    pub hub_client: HubClient,
    pub processes: Arc<Mutex<Vec<MakeProcess>>>
}

impl Make {
    pub fn proc<F>(mut event_handler: F)
    where F: FnMut(&mut Make, HubToClientMsg) {
        let key = [1u8, 2u8, 3u8, 4u8];
        
        // lets wait for a server announce
        let address = HubClient::wait_for_announce(&key, HUB_DEFAULT_PORT).expect("cannot wait for announce");
        
        // ok now connect to that address
        let hub_client = HubClient::connect_to_hub(&key, address).expect("cannot connect to hub");
        
        let mut make = Make {
            hub_client: hub_client,
            processes: Arc::new(Mutex::new(Vec::<MakeProcess>::new()))
        };
        
        // lets transmit a BuildServer ack
        make.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::All,
            msg: HubMsg::ConnectBuild
        }).expect("Cannot send login");
        
        // this is the main messageloop, on rx
        while let Ok(htc) = make.hub_client.rx_read.recv() {
            // we just call the thing.
            event_handler(&mut make, htc);
        }
    }
    
    pub fn cargo(&mut self, uid:HubUid, args: &[&str]) {
        // lets start a thread
        let mut process = Process::start("cargo", args, "./").expect("Cannot start process");
        
        // we now need to start a subprocess and parse the cargo output.
        let tx_write = self.hub_client.tx_write.clone();

        let rx_line = process.rx_line.take().unwrap();

        let processes = Arc::clone(&self.processes);
        
        let thread = std::thread::spawn(move || {
            while let Ok(line) = rx_line.recv(){
                tx_write.send(ClientToHubMsg{
                    to: HubMsgTo::UI,
                    msg: HubMsg::CargoMsg{
                        uid:uid,
                        msg:CargoMsg::Warning{msg:"hi".to_string()}
                    }
                }).expect("tx_write fail");
            }

            // process ends as well
            tx_write.send(ClientToHubMsg{
                to: HubMsgTo::UI,
                msg: HubMsg::CargoDone{
                    uid:uid,
                }
            }).expect("tx_write fail");
            // remove process from process list
            if let Ok(mut processes) = processes.lock() {
                if let Some(index) = processes.iter().position(|p| p.uid == uid){
                    processes.remove(index);
                }
            };
        });
        
        if let Ok(mut processes) = self.processes.lock() {
            processes.push(MakeProcess {
                process:process,
                uid:uid,
                thread: Some(thread)
            });
            // wait for the thread to terminate?
            
        };
    }
    
    pub fn cargo_has_targets(&mut self, uid: HubUid, tgt: &[&'static str]) {
        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::CargoHasTargets {
                uid: uid,
                targets: tgt.iter().map( | v | v.to_string()).collect()
            }
        }).expect("cannot send message");
    }
}

pub struct MakeProcess{
    uid:HubUid,
    process:Process,
    thread: Option<std::thread::JoinHandle<()>>
}

// rust compiler output json structs
#[derive(Clone, Deserialize, Default)]
pub struct RustcTarget {
    kind: Vec<String>,
    crate_types: Vec<String>,
    name: String,
    src_path: String,
    edition: String
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcText {
    text: String,
    highlight_start: u32,
    highlight_end: u32
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcSpan {
    file_name: String,
    byte_start: u32,
    byte_end: u32,
    line_start: u32,
    line_end: u32,
    column_start: u32,
    column_end: u32,
    is_primary: bool,
    text: Vec<RustcText>,
    label: Option<String>,
    suggested_replacement: Option<String>,
    sugggested_applicability: Option<String>,
    expansion: Option<Box<RustcExpansion>>,
    level: Option<String>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcExpansion {
    span: Option<RustcSpan>,
    macro_decl_name: String,
    def_site_span: Option<RustcSpan>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcCode {
    code: String,
    explanation: Option<String>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcMessage {
    message: String,
    code: Option<RustcCode>,
    level: String,
    spans: Vec<RustcSpan>,
    children: Vec<RustcMessage>,
    rendered: Option<String>
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcProfile {
    opt_level: String,
    debuginfo: Option<u32>,
    debug_assertions: bool,
    overflow_checks: bool,
    test: bool
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcCompilerMessage {
    reason: String,
    package_id: String,
    target: RustcTarget,
    message: RustcMessage
}

#[derive(Clone, Deserialize, Default)]
pub struct RustcCompilerArtifact {
    reason: String,
    package_id: String,
    target: RustcTarget,
    profile: RustcProfile,
    features: Vec<String>,
    filenames: Vec<String>,
    executable: Option<String>,
    fresh: bool
}
