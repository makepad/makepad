use crate::process::*;
use crate::hubmsg::*;
use crate::hubclient::*;
use crate::httpserver::*;
use crate::wasmstrip::*;

use serde::{Deserialize};
use std::sync::{Arc, Mutex};
use std::fs;
use serde_json::{Result};
use std::sync::{mpsc};

pub struct HubWorkspace {
    pub hub_client: HubClient,
    pub http_server: Arc<Mutex<HttpServer>>,
    pub workspace: String,
    pub root_path: String,
    pub abs_root_path: String,
    pub processes: Arc<Mutex<Vec<HubWsProcess>>>,
    pub restart_connection: bool
}

pub struct HubWsProcess {
    uid: HubUid,
    process: Process,
    _thread: Option<std::thread::JoinHandle<()>>
}

pub enum HttpServe {
    Disabled,
    Local(u16),
    All(u16),
    Interface((u16, [u8; 4]))
}

impl HubWorkspace {
    pub fn run<F>(key: &[u8], workspace: &str, root_path: &str, hub_log: HubLog, http_serve: HttpServe, mut event_handler: F)
    where F: FnMut(&mut HubWorkspace, HubToClientMsg) {
        
        let http_server = match http_serve {
            HttpServe::Disabled => Arc::new(Mutex::new(HttpServer::default())),
            HttpServe::Local(port) => HttpServer::start_http_server(port, [127, 0, 0, 1], root_path),
            HttpServe::All(port) => HttpServer::start_http_server(port, [0, 0, 0, 0], root_path),
            HttpServe::Interface((port, ip)) => HttpServer::start_http_server(port, ip, root_path),
        };
        
        loop {
            if root_path.contains("..") || root_path.contains("./") || root_path.contains("\\") || root_path.starts_with("/") {
                panic!("Root path for workspace cant be relative and must be unix style");
            }
            
            hub_log.msg("Workspace waiting for hub announcement..", &workspace);
            
            // lets wait for a server announce
            let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
            
            hub_log.msg("Workspace got announce, connecting to {:?}", &address);
            
            // ok now connect to that address
            let hub_client = HubClient::connect_to_hub(&key, address, hub_log.clone()).expect("cannot connect to hub");
            
            hub_log.msg("Workspace connected to {:?}", &hub_client.server_addr);
            
            let mut hub_workspace = HubWorkspace {
                hub_client: hub_client,
                http_server: Arc::clone(&http_server),
                workspace: workspace.to_string(),
                root_path: root_path.to_string(),
                abs_root_path: format!("{}/{}", std::env::current_dir().unwrap().display(), root_path),
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
            HubMsg::WorkspaceFileTreeRequest {uid} => {
                self.workspace_file_tree(
                    htc.from,
                    uid,
                    &[".json", ".toml", ".js", ".rs", ".txt", ".text", ".ron", ".html"],
                    None
                );
            },
            HubMsg::FileReadRequest {uid, path} => {
                self.file_read(htc.from, uid, &path);
            },
            HubMsg::FileWriteRequest {uid, path, data} => {
                self.file_write(htc.from, uid, &path, data);
            },
            HubMsg::ConnectionError(_e) => {
                self.restart_connection = true;
                println!("Got connection error, need to restart loop TODO kill all processes!");
            },
            HubMsg::CargoKill {uid} => {
                self.cargo_kill(uid);
            },
            HubMsg::ArtifactKill {uid} => {
                self.artifact_kill(uid);
            },
            HubMsg::ArtifactExec {uid, path, args} => {
                let v: Vec<&str> = args.iter().map( | v | v.as_ref()).collect();
                self.artifact_exec(uid, &path, &v);
            },
            _ => ()
        }
    }
    
    pub fn artifact_kill(&mut self, uid: HubUid) {
        if let Ok(mut procs) = self.processes.lock() {
            for proc in procs.iter_mut() {
                if proc.uid == uid {
                    proc.process.kill();
                    break;
                }
            }
        };
    }
    
    pub fn artifact_exec(&mut self, uid: HubUid, path: &str, args: &[&str]) {
        
        // lets start a thread
        let mut process = Process::start(path, args, &self.root_path, &[("RUST_BACKTRACE", "full")]).expect("Cannot start process");
        
        // we now need to start a subprocess and parse the cargo output.
        let tx_write = self.hub_client.tx_write.clone();
        
        let rx_line = process.rx_line.take().unwrap();
        
        let processes = Arc::clone(&self.processes);
        let workspace = self.workspace.clone();
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::ArtifactExecBegin {uid: uid}
        }).expect("tx_write fail");
        
        fn starts_with_digit(val: &str) -> bool {
            if val.len() <= 1 {
                return false;
            }
            let c1 = val.chars().next().unwrap();
            c1 >= '0' && c1 <= '9'
        }
        
        let thread = std::thread::spawn(move || {
            let mut tracing_panic = false;
            let mut panic_stack: Vec<String> = Vec::new();
            
            fn send_panic(uid: HubUid, workspace: &str, panic_stack: &Vec<String>, tx_write: &mpsc::Sender<ClientToHubMsg>) {
                // this has to be the uglies code in the whole project. If only stacktraces were more cleanly machine readable.
                let mut path = None;
                let mut row = 0;
                let mut rendered = Vec::new();
                rendered.push(panic_stack[0].clone());
                let mut last_fn_name = String::new();
                for panic_line in panic_stack {
                    
                    if panic_line.starts_with("at ")
                        && !panic_line.starts_with("at /")
                        && !panic_line.starts_with("at src/libstd/")
                        && !panic_line.starts_with("at src/libcore/")
                        && !panic_line.starts_with("at src/libpanic_unwind/") {
                        if let Some(end) = panic_line.find(":") {
                            let proc_path = format!("{}/{}", workspace, panic_line.get(3..end).unwrap().to_string());
                            let proc_row = panic_line.get((end + 1)..(panic_line.len() - 1)).unwrap().parse::<usize>().unwrap();
                            
                            rendered.push(format!("{}:{} - {}", proc_path, proc_row, last_fn_name));
                            if path.is_none() {
                                path = Some(proc_path);
                                row = proc_row
                            }
                        }
                    }
                    else if starts_with_digit(panic_line) {
                        if let Some(pos) = panic_line.find(" 0x") {
                            last_fn_name = panic_line.get((pos + 15)..).unwrap().to_string();
                        }
                    }
                }
                rendered.push("\n".to_string());
                
                if panic_stack.len()<3 {
                    return;
                }
                
                tx_write.send(ClientToHubMsg {
                    to: HubMsgTo::UI,
                    msg: HubMsg::LogItem {
                        uid: uid,
                        item: HubLogItem {
                            path: path,
                            row: row,
                            col: 1,
                            tail: 0,
                            head: 0,
                            body: rendered[0].clone(),
                            rendered: Some(rendered.join("")),
                            explanation: Some(panic_stack[2..].join("")),
                            level: HubLogItemLevel::Panic
                        }
                    }
                }).expect("tx_write fail");
            }
            
            while let Ok(line) = rx_line.recv() {
                if let Some((is_stderr, line)) = line {
                    if is_stderr { // start collecting stderr
                        if line.starts_with("thread '") { // this is how we recognise a stacktrace start..Very sturdy.
                            tracing_panic = true;
                            panic_stack.truncate(0);
                        }
                        if tracing_panic {
                            let trimmed = line.trim_start().to_string();
                            // we terminate the stacktrace on a double 'no-source' line
                            // this usually is around the beginning. If only someone thought of marking the 'end'
                            // of the stacktrace in a recognisable form
                            if panic_stack.len()>2
                                && starts_with_digit(panic_stack.last().unwrap())
                                && starts_with_digit(&trimmed) {
                                tracing_panic = false;
                                send_panic(uid, &workspace, &panic_stack, &tx_write);
                            }
                            else {
                                panic_stack.push(trimmed);
                            }
                        }
                        else {
                            if line.starts_with("[") { // a dbg! style output with line information
                                if let Some(row_pos) = line.find(":") {
                                    if let Some(end_pos) = line.find("]") {
                                        tx_write.send(ClientToHubMsg {
                                            to: HubMsgTo::UI,
                                            msg: HubMsg::LogItem {
                                                uid: uid,
                                                item: HubLogItem {
                                                    path: Some(format!("{}/{}", workspace, line.get(1..row_pos).unwrap().to_string())),
                                                    row: line.get((row_pos + 1)..end_pos).unwrap().parse::<usize>().unwrap(),
                                                    col: 1,
                                                    tail: 0,
                                                    head: 0,
                                                    body: line.get((end_pos + 1)..).unwrap().to_string(),
                                                    rendered: Some(line.clone()),
                                                    explanation: None,
                                                    level: HubLogItemLevel::Log
                                                }
                                            }
                                        }).expect("tx_write fail");
                                    }
                                }
                            }
                        }
                    }
                    else {
                        // lets parse/process our log line
                        tx_write.send(ClientToHubMsg {
                            to: HubMsgTo::UI,
                            msg: HubMsg::LogItem {
                                uid: uid,
                                item: HubLogItem {
                                    //package_id: parsed.package_id.clone(),
                                    path: None,
                                    row: 0,
                                    col: 0,
                                    tail: 0,
                                    head: 0,
                                    body: line.clone(),
                                    rendered: Some(line),
                                    explanation: None,
                                    level: HubLogItemLevel::Log
                                }
                            }
                        }).expect("tx_write fail");
                    }
                    /*
                    tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::UI,
                        msg: HubMsg::ArtifactMsg {
                            uid: uid,
                            msg: line
                        }
                    }).expect("tx_write fail");*/
                }
                else { // process terminated
                    if tracing_panic {
                        send_panic(uid, &workspace, &panic_stack, &tx_write);
                    }
                    // do we have any errors?
                    break;
                }
            }
            
            // process ends as well
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::UI,
                msg: HubMsg::ArtifactExecEnd {
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
                process: process,
                _thread: Some(thread)
            });
        };
    }
    
    
    pub fn cargo_kill(&mut self, uid: HubUid) {
        if let Ok(mut procs) = self.processes.lock() {
            for proc in procs.iter_mut() {
                if proc.uid == uid {
                    proc.process.kill();
                    break;
                }
            }
        };
    }
    
    pub fn cargo_exec_fail(&mut self, uid: HubUid, package: &str, target: &str) {
        println!("Workspace {} Cannot find package {} and target {} for exec", self.workspace, package, target);
        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::CargoExecFail {uid: uid}
        }).expect("tx_write fail");
    }
    
    pub fn cargo_exec(&mut self, uid: HubUid, args: &[&str], env: &[(&str, &str)]) {
        if let Ok(mut http_server) = self.http_server.lock() {
            http_server.send_build_start();
        };
        // lets start a thread
        let mut extargs = args.to_vec();
        extargs.push("--message-format=json");
        let mut process = Process::start("cargo", &extargs, &self.root_path, env).expect("Cannot start process");
        //let print_args: Vec<String> = extargs.to_vec().iter().map( | v | v.to_string()).collect();
        
        // we now need to start a subprocess and parse the cargo output.
        let tx_write = self.hub_client.tx_write.clone();
        
        let rx_line = process.rx_line.take().unwrap();
        
        let processes = Arc::clone(&self.processes);
        
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::CargoExecBegin {uid: uid}
        }).expect("tx_write fail");
        
        let workspace = self.workspace.clone();
        let abs_root_path = self.abs_root_path.clone();
        
        fn de_relativize_path(path: &str) -> String {
            let splits: Vec<&str> = path.split("/").collect();
            let mut out = Vec::new();
            for split in splits {
                if split == ".." && out.len()>0 {
                    out.pop();
                }
                else {
                    out.push(split);
                }
            }
            out.join("/")
        }
        
        let http_server = Arc::clone(&self.http_server);
        let thread = std::thread::spawn(move || {
            
            let mut any_errors = false;
            let mut artifact_path = None;
            while let Ok(line) = rx_line.recv() {
                if let Some((_is_stderr, line)) = line {
                    
                    // lets parse the line
                    let mut parsed: Result<RustcCompilerMessage> = serde_json::from_str(&line);
                    match &mut parsed {
                        Err(_) => (), //self.hub_log.log(&format!("Json Parse Error {:?} {}", err, line)),
                        Ok(parsed) => {
                            // here we convert the parsed message
                            if let Some(message) = &mut parsed.message { //.spans;
                                let spans = &message.spans;
                                for i in 0..spans.len() {
                                    let span = spans[i].clone();
                                    if !span.is_primary {
                                        continue
                                    }
                                    
                                    let mut msg = message.message.clone();
                                    
                                    //let mut more_lines = vec![];
                                    //more_lines.push(line.clone());
                                    
                                    //if let Some(label) = span.label {
                                    //    msg.push_str(&label);
                                    //}
                                    // if we have children fo process
                                    for child in &message.children {
                                        msg.push_str(" - ");
                                        msg.push_str(&child.message);
                                    }
                                    //if let Some(rendered) = &mut message.rendered{
                                    //    *rendered = format!("{}\n{}", rendered, line.clone());
                                    //}
                                    let level = match message.level.as_ref() {
                                        "warning" => HubLogItemLevel::Warning,
                                        "error" => HubLogItemLevel::Error,
                                        _ => HubLogItemLevel::Warning
                                    };
                                    if level == HubLogItemLevel::Error {
                                        any_errors = true;
                                    }
                                    // lets try to pull path out of rendered, this fixes some rust bugs
                                    let mut path = span.file_name;
                                    let row = span.line_start as usize;
                                    let col = span.column_start as usize;
                                    if let Some(rendered) = &message.rendered {
                                        let lines: Vec<&str> = rendered.split('\n').collect();
                                        if lines.len() >= 1 {
                                            if let Some(start) = lines[1].find("--> ") {
                                                if let Some(end) = lines[1].find(":") {
                                                    path = lines[1].get((start + 4)..end).unwrap().to_string();
                                                    // TODO parse row/col from this line
                                                    
                                                }
                                            }
                                        }
                                    }
                                    tx_write.send(ClientToHubMsg {
                                        to: HubMsgTo::UI,
                                        msg: HubMsg::LogItem {
                                            uid: uid,
                                            item: HubLogItem {
                                                //package_id: parsed.package_id.clone(),
                                                path: Some(format!("{}/{}", workspace, de_relativize_path(&path))),
                                                row: row,
                                                col: col,
                                                tail: span.byte_start as usize,
                                                head: span.byte_end as usize,
                                                body: msg,
                                                rendered: message.rendered.clone(),
                                                explanation: if let Some(code) = &message.code {code.explanation.clone()}else {None},
                                                level: level
                                            }
                                        }
                                    }).expect("tx_write fail");
                                }
                            }
                            else { // other type of message
                                tx_write.send(ClientToHubMsg {
                                    to: HubMsgTo::UI,
                                    msg: HubMsg::CargoArtifact {
                                        uid: uid,
                                        package_id: parsed.package_id.clone(),
                                        fresh: if let Some(fresh) = parsed.fresh {fresh}else {false}
                                    }
                                }).expect("tx_write fail");
                                if !any_errors {
                                    artifact_path = None;
                                    if let Some(executable) = &parsed.executable {
                                        if !executable.ends_with(".rmeta") && abs_root_path.len() + 1 < executable.len() {
                                            let last = executable.clone().split_off(abs_root_path.len() + 1);
                                            artifact_path = Some(last);
                                        }
                                    }
                                    // detect wasm files being build and tell the http server
                                    if let Some(filenames) = &parsed.filenames {
                                        for filename in filenames {
                                            if filename.ends_with(".wasm") && abs_root_path.len() + 1 < filename.len() {
                                                let last = filename.clone().split_off(abs_root_path.len() + 1);
                                                // lets strip this wasm file
                                                if let Ok(data) = fs::read(&last){
                                                    if let Ok(strip) = wasm_strip(&data){
                                                        if let Err(_) = fs::write(&last, strip){
                                                            println!("Cannot write stripped wasm {}", last);
                                                        }
                                                    }
                                                    else{
                                                        println!("Cannot parse wasm {}", last);
                                                    }
                                                }
                                                
                                                // let our http server know of our filechange
                                                if let Ok(mut http_server) = http_server.lock() {
                                                    http_server.send_file_change(&last);
                                                };
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                else { // process terminated
                    // do we have any errors?
                    break;
                }
            }
            
            // process ends as well
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::UI,
                msg: HubMsg::CargoExecEnd {
                    uid: uid,
                    artifact_path: artifact_path
                }
            }).expect("tx_write fail");
            
            
            // remove process from process list
            if let Ok(mut processes) = processes.lock() {
                if let Some(index) = processes.iter().position( | p | p.uid == uid) {
                    processes[index].process.wait();
                    processes.remove(index);
                }
            };
        });
        
        if let Ok(mut processes) = self.processes.lock() {
            processes.push(HubWsProcess {
                uid: uid,
                process: process,
                _thread: Some(thread)
            });
        };
    }
    
    pub fn cargo_packages(&mut self, from: HubAddr, uid: HubUid, packages: Vec<HubCargoPackage>) {
        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::CargoPackagesResponse {
                uid: uid,
                packages: packages
            }
        }).expect("cannot send message");
    }
    
    pub fn file_read(&mut self, from: HubAddr, uid: HubUid, path: &str) {
        // lets read a file and send it.
        
        if let Some(_) = path.find("..") {
            println!("file_read got relative path, ignoring {}", path);
            return
        }
        
        let data = if let Ok(data) = std::fs::read(format!("{}/{}", self.root_path, path)) {
            Some(data)
        }
        else {
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
    
    pub fn file_write(&mut self, from: HubAddr, uid: HubUid, path: &str, data: Vec<u8>) {
        if path.contains("..") {
            println!("file_read got relative path, ignoring {}", path);
            return
        }
        
        let done = std::fs::write(format!("{}/{}", self.root_path, path), &data).is_ok();
        
        // lets check if any of our http friends had this file
        if let Ok(mut http_server) = self.http_server.lock() {
            http_server.send_file_change(path);
        };
        
        self.hub_client.tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::FileWriteResponse {
                uid: uid,
                path: path.to_string(),
                done: done
            }
        }).expect("cannot send message");
    }
    
    pub fn workspace_file_tree(&mut self, from: HubAddr, uid: HubUid, ext_inc: &[&str], ron_out:Option<&str>) {
        let tx_write = self.hub_client.tx_write.clone();
        let path = self.root_path.to_string();
        let workspace = self.workspace.to_string();
        let ext_inc: Vec<String> = ext_inc.to_vec().iter().map( | v | v.to_string()).collect();
        let ron_out = if let Some(ron_out) = ron_out{Some(ron_out.to_string())}else{None};
        let _thread = std::thread::spawn(move || {
            
            fn read_recur(path: &str, ext_inc: &Vec<String>) -> Vec<WorkspaceFileTreeNode> {
                let mut ret = Vec::new();
                if let Ok(read_dir) = fs::read_dir(path) {
                    for entry in read_dir {
                        if let Ok(entry) = entry {
                            if let Ok(ty) = entry.file_type() {
                                if let Ok(name) = entry.file_name().into_string() {
                                    if ty.is_dir() {
                                        if name == ".git" || name == "target" || name == "edit_repo" {
                                            continue;
                                        }
                                        ret.push(WorkspaceFileTreeNode::Folder {
                                            name: name.clone(),
                                            folder: read_recur(&format!("{}/{}", path, name), ext_inc)
                                        });
                                    }
                                    else {
                                        for ext in ext_inc {
                                            if name.ends_with(ext) {
                                                ret.push(WorkspaceFileTreeNode::File {
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
            let tree = WorkspaceFileTreeNode::Folder {
                name: workspace.clone(),
                folder: read_recur(&path, &ext_inc)
            };
            
            if let Some(ron_out) = ron_out{
                let ron = ron::ser::to_string_pretty(&tree, ron::ser::PrettyConfig::default()).unwrap();
                let _ = fs::write(format!("{}/{}", path, ron_out), ron.as_bytes());
            }

            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::Client(from),
                msg: HubMsg::WorkspaceFileTreeResponse {
                    uid: uid,
                    tree: tree
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
