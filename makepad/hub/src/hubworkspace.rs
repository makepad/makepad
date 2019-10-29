use crate::process::*;
use crate::hubmsg::*;
use crate::hubclient::*;
use crate::httpserver::*;
use crate::wasmstrip::*;

use serde::{Deserialize};
use std::sync::{Arc, Mutex};
use std::fs;
use std::sync::{mpsc};
use toml::Value;
use std::collections::HashMap;

pub struct HubWorkspace {
    pub tx_write: Arc<Mutex<mpsc::Sender<ClientToHubMsg>>>,
    pub http_server: Arc<Mutex<Option<HttpServer>>>,
    pub projects: Arc<Mutex<HashMap<String, String>>>,
    pub workspace: String,
    pub abs_cwd_path: String,
    pub processes: Arc<Mutex<Vec<HubWsProcess>>>,
}

pub struct HubWsProject {
    pub name: String,
    pub abs_path: String
}

pub struct HubWsProcess {
    uid: HubUid,
    process: Process,
}

pub enum HubWsError {
    Error(String),
    LocErrors(Vec<LocMessage>)
}

impl HubWorkspace {
    pub fn run_networked<F>(key: &[u8], workspace: &str, hub_log: HubLog, event_handler: &'static F)
    where F: Fn(&mut HubWorkspace, HubToClientMsg) -> Result<(), HubWsError> + Clone + Send {
        //let args: Vec<String> = std::env::args().collect();
        // lets check our cmdline args
        let projects = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let http_server = Arc::new(Mutex::new(None));
        let abs_cwd_path = format!("{}", std::env::current_dir().unwrap().display());
        let processes = Arc::new(Mutex::new(Vec::<HubWsProcess>::new()));
        /*match http_serve {
            HttpServe::Disabled => Arc::new(Mutex::new(HttpServer::default())),
            HttpServe::Local(port) => HttpServer::start_http_server(port, [127, 0, 0, 1], root_path),
            HttpServe::All(port) => HttpServer::start_http_server(port, [0, 0, 0, 0], root_path),
            HttpServe::Interface((port, ip)) => HttpServer::start_http_server(port, ip, root_path),
        };*/
        //let abs_root_path = format!("{}/{}", std::env::current_dir().unwrap().display(), root_path);
        
        loop {
            //if root_path.contains("..") || root_path.contains("./") || root_path.contains("\\") || root_path.starts_with("/") {
            //    panic!("Root path for workspace cant be relative and must be unix style");
            //}
            
            hub_log.msg("Workspace waiting for hub announcement..", &workspace);
            
            // lets wait for a server announce
            let address = HubClient::wait_for_announce(&key).expect("cannot wait for announce");
            
            hub_log.msg("Workspace got announce, connecting to {:?}", &address);
            
            // ok now connect to that address
            let mut hub_client = HubClient::connect_to_hub(&key, address, hub_log.clone()).expect("cannot connect to hub");
            
            let rx_read = hub_client.rx_read.take().unwrap();
            let tx_write = hub_client.tx_write.clone();
            
            hub_log.msg("Workspace connected to {:?}", &hub_client.server_addr);
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::All,
                msg: HubMsg::ConnectWorkspace(workspace.to_string())
            }).expect("Cannot send login");
            
            let tx_write_arc = Arc::new(Mutex::new(tx_write));
            
            // this is the main messageloop, on rx
            while let Ok(htc) = rx_read.recv() {
                match &htc.msg {
                    HubMsg::ConnectionError(_e) => {
                        println!("Got connection error, need to restart loop TODO kill all processes!");
                        break;
                    },
                    _ => ()
                }
                let _thread = {
                    let event_handler = event_handler.clone();
                    let mut hub_workspace = HubWorkspace {
                        tx_write: Arc::clone(&tx_write_arc),
                        http_server: Arc::clone(&http_server),
                        projects: Arc::clone(&projects),
                        processes: Arc::clone(&processes),
                        workspace: workspace.to_string(),
                        abs_cwd_path: abs_cwd_path.clone(),
                    };
                    std::thread::spawn(move || {
                        let is_build_uid = if let HubMsg::Build {uid, ..} = &htc.msg {Some(*uid)}else {None};
                        let result = event_handler(&mut hub_workspace, htc);
                        if let Some(is_build_uid) = is_build_uid {
                            if let Ok(tx_write) = hub_workspace.tx_write.lock() {
                                if result.is_ok() {
                                    tx_write.send(ClientToHubMsg {
                                        to: HubMsgTo::UI,
                                        msg: HubMsg::BuildFailure {uid: is_build_uid}
                                    }).expect("tx_write fail");
                                }
                                else {
                                    tx_write.send(ClientToHubMsg {
                                        to: HubMsgTo::UI,
                                        msg: HubMsg::BuildSuccess {uid: is_build_uid}
                                    }).expect("tx_write fail");
                                }
                            }
                        }
                    })
                };
            }
        }
    }
    
    pub fn run_commandline<F>(args: Vec<String>, workspace: &str, mount: &str, event_handler: &'static F)
    where F: Fn(&mut HubWorkspace, HubToClientMsg) -> Result<(), HubWsError> + Clone + Send {
        
        let projects = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let http_server = Arc::new(Mutex::new(None));
        let processes = Arc::new(Mutex::new(Vec::<HubWsProcess>::new()));
        let abs_cwd_path = format!("{}", std::env::current_dir().unwrap().display());
        if let Ok(mut projects) = projects.lock() {
            projects.insert(
                "main".to_string(),
                rel_to_abs_path(&abs_cwd_path, mount)
            );
        };
        
        let thread = {
            let (tx_write, rx_write) = mpsc::channel::<ClientToHubMsg>();
            
            let mut hub_workspace = HubWorkspace {
                tx_write: Arc::new(Mutex::new(tx_write.clone())),
                http_server: Arc::clone(&http_server),
                workspace: workspace.to_string(),
                processes: Arc::clone(&processes),
                projects: Arc::clone(&projects),
                abs_cwd_path: abs_cwd_path.clone()
            };
            
            // lets check our command. its either build or list
            let thread = std::thread::spawn(move || {
                while let Ok(htc) = rx_write.recv() {
                    match htc.msg {
                        HubMsg::PackagesResponse {packages, ..} => {
                            println!("{:?}", packages);
                        },
                        HubMsg::LogItem {item, ..} => {
                            println!("{:?}", item)
                        },
                        HubMsg::CargoArtifact {package_id, ..} => {
                            println!("{}", package_id)
                        },
                        HubMsg::CargoEnd {build_result, ..} => {
                            println!("CargoEnd {:?}", build_result)
                        }
                        _ => ()
                    }
                }
            });
            
            match args[1].as_ref() {
                "list" => {
                    let _ = event_handler(&mut hub_workspace, HubToClientMsg {
                        from: HubAddr::zero(),
                        msg: HubMsg::PackagesRequest {
                            uid: HubUid::zero()
                        }
                    });
                }
                "build" => {
                    if args.len() != 4 {
                        println!("Incorrect usage, please use:\nbuild <package> <config>");
                    }
                    else {
                        let result = event_handler(&mut hub_workspace, HubToClientMsg {
                            from: HubAddr::zero(),
                            msg: HubMsg::Build {
                                uid: HubUid::zero(),
                                project: "main".to_string(),
                                package: args[2].clone(),
                                config: args[3].clone()
                            }
                        });
                        if result.is_ok() {
                            println!("BuildSuccess!");
                        }
                        else {
                            println!("BuildFailure!");
                        }
                    }
                },
                _ => {
                    println!("Workspace commandline interface. Commands available:\nlist\nbuild <package> <config>");
                }
            }
            
            thread
        };
        let _ = thread.join();
        /*
        let result = event_handler(&mut hub_workspace, htc);
        if let Some(is_build_uid) = is_build_uid {
            if let Ok(hub_client) = hub_workspace.hub_client.lock() {
                if result.is_ok() {
                    hub_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::UI,
                        msg: HubMsg::BuildFailure {uid: is_build_uid}
                    }).expect("tx_write fail");
                }
                else {
                    hub_client.tx_write.send(ClientToHubMsg {
                        to: HubMsgTo::UI,
                        msg: HubMsg::BuildSuccess {uid: is_build_uid}
                    }).expect("tx_write fail");
                }
            }
        }*/
    }
    
    pub fn set_config(&mut self, _uid:HubUid, _config: HubWsConfig) -> Result<(), HubWsError> {
        // if we have a http server. just shut it down
        if let Ok(mut http_server) = self.http_server.lock() {
            if let Some(http_server) = &mut *http_server{
                http_server.terminate();
            }
            *http_server = None;
        }
        // start it anew if we need to
        
        Ok(())
    }
    
    pub fn default(&mut self, htc: HubToClientMsg) -> Result<(), HubWsError> {
        let ws = self;
        match htc.msg {
            HubMsg::SetWorkspaceConfig {uid, config} => {
                ws.set_config(uid, config)
            },
            HubMsg::WorkspaceFileTreeRequest {uid} => {
                ws.workspace_file_tree(
                    htc.from,
                    uid,
                    &[".json", ".toml", ".js", ".rs", ".txt", ".text", ".ron", ".html"],
                );
                Ok(())
            },
            HubMsg::FileReadRequest {uid, path} => {
                ws.file_read(htc.from, uid, &path);
                Ok(())
            },
            HubMsg::FileWriteRequest {uid, path, data} => {
                ws.file_write(htc.from, uid, &path, data);
                Ok(())
            },
            HubMsg::BuildKill {uid} => {
                ws.process_kill(uid);
                Ok(())
            },
            HubMsg::ProgramKill {uid} => {
                ws.process_kill(uid);
                Ok(())
            },
            HubMsg::ProgramRun {uid, path, args} => {
                let v: Vec<&str> = args.iter().map( | v | v.as_ref()).collect();
                ws.program_run(uid, &path, &v)?;
                Ok(())
            },
            _ => Ok(())
        }
    }
    
    pub fn process_kill(&mut self, uid: HubUid) {
        if let Ok(mut procs) = self.processes.lock() {
            for proc in procs.iter_mut() {
                if proc.uid == uid {
                    proc.process.kill();
                }
            }
        };
    }
    
    pub fn project_split_from_path(&mut self, uid: HubUid, path: &str) -> Result<(String, String, String), HubWsError> {
        if let Some(project_pos) = path.find("/") {
            let (project, rest) = path.split_at(project_pos);
            let (_, rest) = rest.split_at(1);
            let abs_dir = self.get_project_abs(uid, project) ?;
            return Ok((abs_dir.to_string(), project.to_string(), rest.to_string()));
        }
        Err(
            self.error(uid, format!("workspace {} path {} incorrent", self.workspace, path))
        )
    }
    
    pub fn get_project_abs(&mut self, uid: HubUid, project: &str) -> Result<String, HubWsError> {
        if let Ok(projects) = self.projects.lock() {
            if let Some(abs_dir) = projects.get(project) {
                return Ok(abs_dir.to_string())
            }
        }
        Err(
            self.error(uid, format!("workspace {} project {} not found", self.workspace, project))
        )
    }
    
    pub fn program_run(&mut self, uid: HubUid, path: &str, args: &[&str]) -> Result<(), HubWsError> {
        // we have to turn our path which is in the form project/... into a root path
        let (abs_dir, project, sub_path) = self.project_split_from_path(uid, path) ?;
        // lets start a thread
        let process = Process::start(&sub_path, args, &abs_dir, &[("RUST_BACKTRACE", "full")]);
        if process.is_err() {
            return Err(
                self.error(uid, format!("workspace {} program run {} not found", self.workspace, path))
            );
        }
        let mut process = process.unwrap();
        
        // we now need to start a subprocess and parse the cargo output.
        let tx_write = if let Ok(tx_write) = self.tx_write.lock() {
            tx_write.clone()
        }
        else {
            panic!("Cannot lock hub client");
        };
        
        let rx_line = process.rx_line.take().unwrap();
        
        if let Ok(mut processes) = self.processes.lock() {
            processes.push(HubWsProcess {
                uid: uid,
                process: process,
            });
        };
        
        let workspace = self.workspace.clone();
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::ProgramBegin {uid: uid}
        }).expect("tx_write fail");
        
        fn starts_with_digit(val: &str) -> bool {
            if val.len() <= 1 {
                return false;
            }
            let c1 = val.chars().next().unwrap();
            c1 >= '0' && c1 <= '9'
        }
        
        let mut tracing_panic = false;
        let mut panic_stack: Vec<String> = Vec::new();
        
        fn send_panic(uid: HubUid, workspace: &str, project: &str, panic_stack: &Vec<String>, tx_write: &mpsc::Sender<ClientToHubMsg>) {
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
                        let proc_path = format!("{}/{}/{}", workspace, project, panic_line.get(3..end).unwrap().to_string());
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
                    item: HubLogItem::LocPanic(LocMessage {
                        path: if let Some(path) = path {path}else {"".to_string()},
                        row: row,
                        col: 1,
                        range: None,
                        body: rendered[0].clone(),
                        rendered: Some(rendered.join("")),
                        explanation: Some(panic_stack[2..].join("")),
                    })
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
                            send_panic(uid, &workspace, &project, &panic_stack, &tx_write);
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
                                            item: HubLogItem::LocMessage(LocMessage {
                                                path: format!("{}/{}/{}", workspace, project, line.get(1..row_pos).unwrap().to_string()),
                                                row: line.get((row_pos + 1)..end_pos).unwrap().parse::<usize>().unwrap(),
                                                col: 1,
                                                range: None,
                                                body: line.get((end_pos + 1)..).unwrap().to_string(),
                                                rendered: Some(line.clone()),
                                                explanation: None,
                                            })
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
                            item: HubLogItem::Message(line.clone())
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
                    send_panic(uid, &workspace, &project, &panic_stack, &tx_write);
                }
                // do we have any errors?
                break;
            }
        }
        
        // process ends as well
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::ProgramEnd {
                uid: uid,
            }
        }).expect("tx_write fail");
        
        // remove process from process list
        if let Ok(mut processes) = self.processes.lock() {
            if let Some(index) = processes.iter().position( | p | p.uid == uid) {
                processes.remove(index);
            }
        };
        
        Ok(())
    }
    
    
    pub fn cannot_find_build(&mut self, uid: HubUid, package: &str, target: &str) -> Result<(), HubWsError> {
        Err(
            self.error(uid, format!("Workspace {} Cannot find package {} and target {}", self.workspace, package, target))
        )
    }
    
    pub fn cargo(&mut self, uid: HubUid, project: &str, args: &[&str], env: &[(&str, &str)]) -> Result<BuildResult, HubWsError> {
        
        if let Ok(mut http_server) = self.http_server.lock() {
            if let Some(http_server) = &mut *http_server {
                http_server.send_build_start();
            }
        };
        
        let abs_root_path = self.get_project_abs(uid, project) ?;
        
        // lets start a thread
        let mut extargs = args.to_vec();
        extargs.push("--message-format=json");
        let mut process = Process::start("cargo", &extargs, &abs_root_path, env).expect("Cannot start process");
        
        //let print_args: Vec<String> = extargs.to_vec().iter().map( | v | v.to_string()).collect();
        
        // we now need to start a subprocess and parse the cargo output.
        let tx_write = if let Ok(tx_write) = self.tx_write.lock() {
            tx_write.clone()
        }
        else {
            panic!("Cannot lock hub client");
        };
        
        let rx_line = process.rx_line.take().unwrap();
        
        if let Ok(mut processes) = self.processes.lock() {
            processes.push(HubWsProcess {
                uid: uid,
                process: process,
            });
        };
        
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::CargoBegin {uid: uid}
        }).expect("tx_write fail");
        
        let workspace = self.workspace.clone();
        
        let mut errors = Vec::new();
        let mut build_result = BuildResult::NoOutput;
        while let Ok(line) = rx_line.recv() {
            if let Some((_is_stderr, line)) = line {
                
                // lets parse the line
                let mut parsed: serde_json::Result<RustcCompilerMessage> = serde_json::from_str(&line);
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
                                
                                for child in &message.children {
                                    msg.push_str(" - ");
                                    msg.push_str(&child.message);
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
                                let loc_message = LocMessage {
                                    //package_id: parsed.package_id.clone(),
                                    path: format!("{}/{}/{}", workspace, project, de_relativize_path(&path)),
                                    row: row,
                                    col: col,
                                    range: Some((span.byte_start as usize, span.byte_end as usize)),
                                    body: msg,
                                    rendered: message.rendered.clone(),
                                    explanation: if let Some(code) = &message.code {code.explanation.clone()}else {None},
                                };
                                let item = match message.level.as_ref() {
                                    "error" => {
                                        errors.push(loc_message.clone());
                                        HubLogItem::LocError(loc_message)
                                    },
                                    _ => HubLogItem::LocWarning(loc_message),
                                };
                                
                                tx_write.send(ClientToHubMsg {
                                    to: HubMsgTo::UI,
                                    msg: HubMsg::LogItem {
                                        uid: uid,
                                        item: item
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
                            if errors.len() == 0 {
                                build_result = BuildResult::NoOutput;
                                if let Some(executable) = &parsed.executable {
                                    if !executable.ends_with(".rmeta") && abs_root_path.len() + 1 < executable.len() {
                                        let last = executable.clone().split_off(abs_root_path.len() + 1);
                                        build_result = BuildResult::Executable {path: format!("{}/{}", project, last)};
                                    }
                                }
                                // detect wasm files being build and tell the http server
                                if let Some(filenames) = &parsed.filenames {
                                    for filename in filenames {
                                        if filename.ends_with(".wasm") && abs_root_path.len() + 1 < filename.len() {
                                            let last = filename.clone().split_off(abs_root_path.len() + 1);
                                            if let Ok(mut http_server) = self.http_server.lock() {
                                                if let Some(http_server) = &mut *http_server {
                                                    http_server.send_file_change(&last);
                                                }
                                            };
                                            build_result = BuildResult::Wasm {path: format!("{}/{}", project, last)};
                                        }
                                    }
                                }
                            }
                            else {
                                build_result = BuildResult::Error;
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
            msg: HubMsg::CargoEnd {
                uid: uid,
                build_result: build_result.clone()
            }
        }).expect("tx_write fail");
        
        // remove process from process list
        if let Ok(mut processes) = self.processes.lock() {
            if let Some(index) = processes.iter().position( | p | p.uid == uid) {
                processes[index].process.wait();
                processes.remove(index);
            }
        };
        if let BuildResult::Error = build_result {
            return Err(HubWsError::LocErrors(errors))
        }
        return Ok(build_result);
    }
    
    pub fn packages_response(&mut self, from: HubAddr, uid: HubUid, packages: Vec<HubPackage>) {
        let tx_write = self.tx_write.lock().expect("cannot lock hub client");
        
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::PackagesResponse {
                uid: uid,
                packages: packages
            }
        }).expect("cannot send message");
    }
    
    pub fn error(&mut self, uid: HubUid, msg: String) -> HubWsError {
        let tx_write = self.tx_write.lock().expect("cannot lock hub client");
        
        // MAKE THIS A LOG ERROR
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::LogItem {uid: uid, item: HubLogItem::Error(msg.clone())}
        }).expect("tx_write fail");
        
        return HubWsError::Error(msg)
    }
    
    pub fn message(&mut self, uid: HubUid, msg: String) {
        let tx_write = self.tx_write.lock().expect("cannot lock hub client");
        
        // MAKE THIS A LOG ERROR
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::UI,
            msg: HubMsg::LogItem {uid: uid, item: HubLogItem::Message(msg.clone())}
        }).expect("tx_write fail");
    }
    
    pub fn wasm_strip_debug(&mut self, uid: HubUid, path: &str) -> Result<BuildResult, HubWsError> {
        
        let (abs_root_path, _project, sub_path) = self.project_split_from_path(uid, path) ?;
        
        let filepath = format!("{}/{}", abs_root_path, sub_path);
        // lets strip this wasm file
        if let Ok(data) = fs::read(&filepath) {
            if let Ok(strip) = wasm_strip_debug(&data) {
                let uncomp_len = strip.len();
                let mut enc = snap::Encoder::new();
                let comp_len = if let Ok(compressed) = enc.compress_vec(&strip) {compressed.len()}else {0};
                
                if let Err(_) = fs::write(&filepath, strip) {
                    return Err(self.error(uid, format!("Cannot write stripped wasm {}", filepath)));
                }
                else {
                    self.message(uid, format!("Wasm file stripped size: {} kb uncompressed {} kb with snap", uncomp_len>>10, comp_len>>10));
                    return Ok(BuildResult::Wasm {path: path.to_string()})
                }
            }
            else {
                return Err(self.error(uid, format!("Cannot parse wasm {}", filepath)));
            }
        }
        Err(self.error(uid, format!("Cannot read wasm {}", filepath)))
    }
    
    pub fn read_packages(&mut self, uid: HubUid) -> Vec<(String, String)> {
        // we need to loop over all project paths, and read cargo
        let mut packages = Vec::new();
        let projects = Arc::clone(&self.projects);
        if let Ok(projects) = projects.lock() {
            for (abs_path, project) in projects.iter() {
                let vis_path = format!("{}/{}/Cargo.toml", self.workspace, project);
                let root_cargo = match std::fs::read_to_string(format!("{}/Cargo.toml", abs_path)) {
                    Err(_) => {
                        self.error(uid, format!("Cannot read cargo {}", vis_path));
                        continue;
                    },
                    Ok(v) => v
                };
                let value = match root_cargo.parse::<Value>() {
                    Err(e) => {
                        self.error(uid, format!("Cannot parse {} {:?}", vis_path, e));
                        continue;
                    },
                    Ok(v) => v
                };
                let mut ws_members = Vec::new();
                if let Value::Table(table) = &value {
                    if let Some(table) = table.get("workspace") {
                        if let Value::Table(table) = table {
                            if let Some(members) = table.get("members") {
                                if let Value::Array(members) = members {
                                    for member in members {
                                        if let Value::String(member) = member {
                                            ws_members.push(member);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    else if let Some(table) = table.get("package") {
                        if let Value::Table(table) = table {
                            if let Some(name) = table.get("name") {
                                if let Value::String(name) = name {
                                    packages.push((project.clone(), name.clone()));
                                }
                            }
                        }
                    }
                }
                for member in ws_members {
                    let file_path = format!("{}/{}/Cargo.toml", abs_path, member);
                    let vis_path = format!("{}/{}/{}/Cargo.toml", self.workspace, project, member);
                    let cargo = match std::fs::read_to_string(&file_path) {
                        Err(_) => {
                            self.error(uid, format!("Cannot read cargo {}", vis_path));
                            continue;
                        },
                        Ok(v) => v
                    };
                    let value = match cargo.parse::<Value>() {
                        Err(e) => {
                            self.error(uid, format!("Cannot parse cargo {} {:?}", vis_path, e));
                            continue;
                        },
                        Ok(v) => v
                    };
                    if let Value::Table(table) = &value {
                        if let Some(table) = table.get("package") {
                            if let Value::Table(table) = table {
                                if let Some(name) = table.get("name") {
                                    if let Value::String(name) = name {
                                        packages.push((project.clone(), name.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        return packages
    }
    
    pub fn file_read(&mut self, from: HubAddr, uid: HubUid, path: &str) {
        // lets read a file and send it.
        if let Ok((abs_dir, _project, sub_path)) = self.project_split_from_path(uid, path) {
            
            if let Some(_) = sub_path.find("..") {
                self.error(uid, format!("file_read got relative path, ignoring {}", path));
                return
            }
            
            let data = if let Ok(data) = std::fs::read(format!("{}/{}", abs_dir, path)) {
                Some(data)
            }
            else {
                None
            };
            
            let tx_write = self.tx_write.lock().expect("cannot lock hub client");
            
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::Client(from),
                msg: HubMsg::FileReadResponse {
                    uid: uid,
                    path: path.to_string(),
                    data: data
                }
            }).expect("cannot send message");
        }
    }
    
    pub fn file_write(&mut self, from: HubAddr, uid: HubUid, path: &str, data: Vec<u8>) {
        if let Ok((abs_dir, _project, sub_path)) = self.project_split_from_path(uid, path) {
            
            if path.contains("..") {
                self.error(uid, format!("file_write got relative path, ignoring {}", path));
                println!("file_read got relative path, ignoring {}", path);
                return
            }
            
            let done = std::fs::write(format!("{}/{}", abs_dir, sub_path), &data).is_ok();
            
            // lets check if any of our http friends had this file
            if let Ok(mut http_server) = self.http_server.lock() {
                if let Some(http_server) = &mut *http_server {
                    http_server.send_file_change(path);
                }
            };
            
            let tx_write = self.tx_write.lock().expect("cannot lock hub client");
            
            tx_write.send(ClientToHubMsg {
                to: HubMsgTo::Client(from),
                msg: HubMsg::FileWriteResponse {
                    uid: uid,
                    path: path.to_string(),
                    done: done
                }
            }).expect("cannot send message");
        }
    }
    
    pub fn workspace_file_tree(&mut self, from: HubAddr, uid: HubUid, ext_inc: &[&str]) {
        let tx_write = if let Ok(tx_write) = self.tx_write.lock() {
            tx_write.clone()
        }
        else {
            panic!("Cannot lock hub client");
        };
        
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
        
        let ext_inc: Vec<String> = ext_inc.to_vec().iter().map( | v | v.to_string()).collect();
        //let ron_out = if let Some(ron_out) = ron_out {Some(ron_out.to_string())}else {None};
        
        let mut folder = Vec::new();
        
        if let Ok(projects) = self.projects.lock() {
            for (abs_path, project) in projects.iter() {

                let tree = WorkspaceFileTreeNode::Folder {
                    name: project.clone(),
                    folder: read_recur(&abs_path, &ext_inc)
                };
                
                //if let Some(ron_out) = ron_out {
                //    let ron = ron::ser::to_string_pretty(&tree, ron::ser::PrettyConfig::default()).unwrap();
                //    let _ = fs::write(format!("{}/{}", path, ron_out), ron.as_bytes());
               // }
                folder.push(tree);
            }
            
        }
        let root = WorkspaceFileTreeNode::Folder {
            name: self.workspace.clone(),
            folder: folder
        };
        
        tx_write.send(ClientToHubMsg {
            to: HubMsgTo::Client(from),
            msg: HubMsg::WorkspaceFileTreeResponse {
                uid: uid,
                tree: root
            }
        }).expect("cannot send message");
    }
}

fn rel_to_abs_path(abs_root: &str, path: &str) -> String {
    if path.starts_with("/") {
        return path.to_string();
    }
    de_relativize_path(&format!("{}/{}", abs_root, path))
}

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
