use crate::process::*;
use crate::hubmsg::*;
use crate::hubclient::*;

use serde::{Deserialize};
use std::sync::{Arc, Mutex};
use std::fs;
use serde_json::{Result};

pub struct HubWorkspace {
    pub hub_client: HubClient,
    pub workspace: String,
    pub root_path: String,
    pub processes: Arc<Mutex<Vec<HubWsProcess>>>,
    pub restart_connection: bool
}

pub struct HubWsProcess {
    uid: HubUid,
    _process: Process,
    _thread: Option<std::thread::JoinHandle<()>>
}

impl HubWorkspace {
    pub fn run<F>(workspace: &str, root_path:&str, mut event_handler: F)
    where F: FnMut(&mut HubWorkspace, HubToClientMsg) {
        let key = [7u8, 4u8, 5u8, 1u8];
        
        loop {
            
            println!("Workspace {} waiting for hub announcement..", workspace);
            
            // lets wait for a server announce
            let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
            
            println!("Workspace {} got announce, connecting to {:?}", workspace, address);
            
            // ok now connect to that address
            let hub_client = HubClient::connect_to_hub(&key, address, HubLog::All).expect("cannot connect to hub");
            
            println!("Workspace {} connected to {:?}", workspace, hub_client.server_addr);
            
            let mut hub_workspace = HubWorkspace {
                hub_client: hub_client,
                workspace: workspace.to_string(),
                root_path: root_path.to_string(),
                processes: Arc::new(Mutex::new(Vec::<HubWsProcess>::new())),
                restart_connection: false
            };
            
            // lets transmit a BuildServer ack
            hub_workspace.hub_client.tx_write.send(ClientToHubMsg {
                to: HubMsgTo::All,
                msg: HubMsg::ConnectWorkspace(workspace.to_string())
            }).expect("Cannot send login");
            
            // this is the main messageloop, on rx
            while let Ok(htc) = hub_workspace.hub_client.rx_read.recv() {
                //println!("Workspace got message {:?}", htc);
                // we just call the thing.
                event_handler(&mut hub_workspace, htc);
                if hub_workspace.restart_connection {
                    break
                }
            }
        }
    }
    
    pub fn default(&mut self, htc: HubToClientMsg) {
        match htc.msg {
            HubMsg::CargoExec {uid, package, target} => {
                match target{
                    CargoTarget::Check=>{
                        self.cargo(uid, &["check", "-p", &package]);
                    },
                    CargoTarget::Release=>{
                        self.cargo(uid, &["build", "-p", &package, "--release"]);
                    },
                    CargoTarget::IPC=>{
                        self.cargo(uid, &["build", "-p", &package, "--release"]);
                    },
                    CargoTarget::VR=>{
                        self.cargo(uid, &["build", "-p", &package, "--release"]);
                    },
                    CargoTarget::Custom(_s)=>{
                    }
                }
            },
            HubMsg::WorkspaceFileTreeRequest {uid} => {
                self.workspace_file_tree(htc.from, uid, &[".json",".toml",".js",".rs"]);
            },
            HubMsg::FileReadRequest{uid, path}=>{
                self.file_read(htc.from, uid, &path);
            },
            HubMsg::FileWriteRequest{uid, path, data}=>{
                self.file_write(htc.from, uid, &path, data);
            },
            HubMsg::ConnectionError(_e) => {
                self.restart_connection = true;
                println!("Got connection error, need to restart loop TODO kill all processes!");
            },
            _ => ()
        }
    }
    
    pub fn cargo(&mut self, uid: HubUid, args: &[&str]) {
        // lets start a thread
        let mut extargs = args.to_vec();
        extargs.push("--message-format=json");
        let mut process = Process::start("cargo", &extargs, "./").expect("Cannot start process");
        
        // we now need to start a subprocess and parse the cargo output.
        let tx_write = self.hub_client.tx_write.clone();
        
        let rx_line = process.rx_line.take().unwrap();
        
        let processes = Arc::clone(&self.processes);
        
        let thread = std::thread::spawn(move || {
            while let Ok(line) = rx_line.recv() {
                if let Some(line) = line {
                    
                    // lets parse the line
                    let parsed: Result<RustcCompilerMessage> = serde_json::from_str(&line);
                    match parsed {
                        Err(err) => println!("Json Parse Error {:?} {}", err, line),
                        Ok(_rcm) => {
                            // here we convert the parsed message
                            
                        }
                    }
                    
                    tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::UI,
                        msg: HubMsg::CargoMsg {
                            uid: uid,
                            msg: CargoMsg::Warning {msg: "hi".to_string()}
                        }
                    }).expect("tx_write fail");
                }
                else { // process terminated
                    break;
                }
            }
            
            // process ends as well
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::UI,
                msg: HubMsg::CargoDone {
                    uid: uid,
                }
            }).expect("tx_write fail");
            
            // remove process from process list
            if let Ok(mut processes) = processes.lock() {
                if let Some(index) = processes.iter().position( | p | p.uid == uid) {
                    processes.remove(index);
                }
            };
        });
        
        if let Ok(mut processes) = self.processes.lock() {
            processes.push(HubWsProcess {
                uid: uid,
                _process: process,
                _thread: Some(thread)
            });
        };
    }
    
    pub fn cargo_packages(&mut self, from:HubAddr, uid: HubUid, packages:Vec<CargoPackage>) {
        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::CargoPackagesResponse {
                uid: uid,
                packages: packages
            }
        }).expect("cannot send message");
    }
    
    pub fn file_read(&mut self, from:HubAddr, uid: HubUid, path: &str){
        // lets read a file and send it.
        if let Some(_) = path.find(".."){
            println!("file_read got relative path, ignoring {}", path);
            return
        }
        let data = if let Ok(data) = std::fs::read(format!("{}{}", self.root_path, path)){
            Some(data)
        }
        else{
            None
        };
        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::FileReadResponse {
                uid: uid,
                path: path.to_string(),
                data: data
            }
        }).expect("cannot send message");
    }
    
    pub fn file_write(&mut self, from:HubAddr, uid:HubUid, path:&str, data:Vec<u8>){
        if let Some(_) = path.find(".."){
            println!("file_read got relative path, ignoring {}", path);
            return
        }

        let done = std::fs::write(format!("{}{}", self.root_path, path), &data).is_ok();

        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::FileWriteResponse {
                uid: uid,
                path: path.to_string(),
                done: done
            }
        }).expect("cannot send message");
    }
    
    pub fn workspace_file_tree(&mut self, from:HubAddr, uid: HubUid, ext_inc:&[&str]) {
        let tx_write = self.hub_client.tx_write.clone();
        let path = self.root_path.to_string();
        let workspace = self.workspace.to_string();
        let ext_inc:Vec<String> = ext_inc.to_vec().iter().map(|v|v.to_string()).collect();
        let _thread = std::thread::spawn(move || {
            
            fn read_recur(path:&str, ext_inc:&Vec<String>)->Vec<WorkspaceFileTreeNode>{
                let mut ret = Vec::new();
                if let Ok(read_dir) = fs::read_dir(path){
                    for entry in read_dir{
                        if let Ok(entry) = entry{
                            if let Ok(ty) = entry.file_type(){
                                if let Ok(name) = entry.file_name().into_string(){
                                    if ty.is_dir(){
                                        if name == ".git" || name == "target"{
                                            continue;
                                        }
                                        ret.push(WorkspaceFileTreeNode::Folder{
                                            name: name.clone(),
                                            folder: read_recur(&format!("{}/{}", path, name), ext_inc)
                                        });
                                    }
                                    else{
                                        for ext in ext_inc{
                                            if name.ends_with(ext){
                                                ret.push(WorkspaceFileTreeNode::File{
                                                    name: name
                                                });
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                ret.sort();
                ret
            }
            
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::Client(from),
                msg: HubMsg::WorkspaceFileTreeResponse {
                    uid: uid,
                    tree: WorkspaceFileTreeNode::Folder{
                        name:workspace.clone(),
                        folder:read_recur(&path, &ext_inc)
                    }
                }
            }).expect("cannot send message");
            
        });
    }
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
    target: Option<RustcTarget>,
    message: Option<RustcMessage>,
    profile: Option<RustcProfile>,
    features: Option<Vec<String>>,
    filenames: Option<Vec<String>>,
    executable: Option<String>,
    fresh: Option<bool>
}

/*#[derive(Clone, Deserialize, Default)]
pub struct RustcCompilerArtifact {
    reason: String,
    package_id: String,
    target: RustcTarget,
    profile: RustcProfile,
    features: Vec<String>,
    filenames: Vec<String>,
    executable: Option<String>,
    fresh: bool
}*/
