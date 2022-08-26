#![allow(dead_code)]
use {
    crate::{
        makepad_error_log::*,
        makepad_micro_serde::*,
        makepad_editor_core::range::{Range},
        build::{
            build_protocol::*,
            child_process::{
                ChildProcess,
                ChildLine
            },
            rustc_json::*,
        },
    },
    std::{
        collections::HashMap,
        fmt,
        path::{PathBuf},
        sync::{Arc, RwLock, Mutex, mpsc::Sender},
    },
};

struct BuildServerShared {
    path: PathBuf,
    // here we should store our connections send slots
    processes: HashMap<String, Mutex<Sender<ChildLine> >>
}

pub struct BuildServer {
    next_connection_id: usize,
    shared: Arc<RwLock<BuildServerShared >>,
}

impl BuildServer {
    pub fn new<P: Into<PathBuf >> (path: P) -> BuildServer {
        BuildServer {
            next_connection_id: 0,
            shared: Arc::new(RwLock::new(BuildServerShared {
                path: path.into(),
                processes: Default::default()
            })),
        }
    }
    
    pub fn connect(&mut self, msg_sender: Box<dyn MsgSender>) -> BuildConnection {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        BuildConnection {
            connection_id,
            shared: self.shared.clone(),
            msg_sender,
        }
    }
}

pub struct BuildConnection {
    connection_id: ConnectionId,
    shared: Arc<RwLock<BuildServerShared >>,
    msg_sender: Box<dyn MsgSender>,
}

impl BuildConnection {
    
    pub fn cargo_run(&self, what: &str, cmd_id: BuildCmdId) {
        let shared = self.shared.clone();
        let msg_sender = self.msg_sender.clone();
        // alright lets run a cargo check and parse its output
        let path = shared.read().unwrap().path.clone();
        
        if let Ok(shared) = shared.write() {
            if let Some(proc) = shared.processes.get(what) {
                let sender = proc.lock().unwrap();
                let _ = sender.send(ChildLine::Kill);
            }
        }
        
        let args = [
            "run",
            "-p",
            what,
            "--message-format=json",
            "--features=nightly",
        ];
        
        let process = ChildProcess::start("cargo", &args, path, &[]).expect("Cannot start process");

        shared.write().unwrap().processes.insert(
            what.to_string(),
            Mutex::new(process.line_sender.clone())
        );
        
        std::thread::spawn(move || {
            // lets create a BuildProcess and run it
            while let Ok(line) = process.line_receiver.recv() {
                match line {
                    ChildLine::StdOut(line) => {
                        let parsed: Result<RustcCompilerMessage, DeJsonErr> = DeJson::deserialize_json(&line);
                        match parsed {
                            Ok(msg) => {
                                // alright we have a couple of 'reasons'
                                match msg.reason.as_str() {
                                    "compiler-message" => {
                                        //println!("OK MSG {:#?}", msg);
                                        msg_sender.process_compiler_message(cmd_id, msg);
                                        //println!("MSG");
                                    }
                                    "compiler-artifact" => {
                                        //println!("ARTIFACT");
                                    }
                                    _ => ()
                                }
                                //println!("OK MSG {:#?}", msg);
                            }
                            Err(_) => { // we should output a log string
                                msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Log, line);
                            }
                        }
                    }
                    ChildLine::StdErr(line) => {
                        log!("STDERR {}", line);
                        msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Error, line);
                    }
                    ChildLine::Term => {
                        break;
                    }
                    ChildLine::Kill => {
                        return process.kill();
                    }
                }
            };
            process.wait();
        });
    }
    
    pub fn handle_cmd(&self, cmd_wrap: BuildCmdWrap) {
        match cmd_wrap.cmd {
            BuildCmd::CargoRun {what} => {
                // lets kill all other 'whats'
                self.cargo_run(&what, cmd_wrap.cmd_id);
            }
        }
    }
    
}

pub trait MsgSender: Send {
    fn box_clone(&self) -> Box<dyn MsgSender>;
    fn send_message(&self, wrap: BuildMsgWrap);
    
    fn send_bare_msg(&self, cmd_id: BuildCmdId, level: BuildMsgLevel, line: String) {
        self.send_message(
            cmd_id.wrap_msg(BuildMsg::Bare(BuildMsgBare {
                line,
                level
            }))
        );
    }
    
    fn send_location_msg(&self, cmd_id: BuildCmdId, level: BuildMsgLevel, file_name: String, range: Range, msg: String) {
        self.send_message(
            cmd_id.wrap_msg(BuildMsg::Location(BuildMsgLocation {
                level,
                file_name,
                range,
                msg
            }))
        );
    }
    
    fn process_compiler_message(&self, cmd_id: BuildCmdId, msg: RustcCompilerMessage) {
        if let Some(msg) = msg.message {
            let level = match msg.level.as_ref() {
                "error" => BuildMsgLevel::Error,
                "warning" => BuildMsgLevel::Warning,
                other => {
                    self.send_bare_msg(cmd_id, BuildMsgLevel::Error, format!("process_compiler_message: unexpected level {}", other));
                    return
                }
            };
            if let Some(span) = msg.spans.iter().find( | span | span.is_primary) {
                let range = span.to_range();
                self.send_location_msg(cmd_id, level, span.file_name.clone(), range, msg.message.clone());
                /*
                if let Some(label) = &span.label {
                    self.send_location_msg(cmd_id, level, span.file_name.clone(), range, label.clone());
                }
                else if let Some(text) = span.text.iter().next() {
                    self.send_location_msg(cmd_id, level, span.file_name.clone(), range, text.text.clone());
                }
                else {
                    self.send_location_msg(cmd_id, level, span.file_name.clone(), range, msg.message.clone());
                }*/
            }
            else {
                self.send_bare_msg(cmd_id, BuildMsgLevel::Error, format!("process_compiler_message: no span:  {}", msg.message));
            }
        }
    }
}

impl<F: Clone + Fn(BuildMsgWrap) + Send + 'static> MsgSender for F {
    fn box_clone(&self) -> Box<dyn MsgSender> {
        Box::new(self.clone())
    }
    
    fn send_message(&self, wrap: BuildMsgWrap) {
        self (wrap)
    }
}

impl Clone for Box<dyn MsgSender> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

impl fmt::Debug for dyn MsgSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MsgSender")
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);
