#![allow(dead_code)]
use {
    crate::{
        makepad_micro_serde::*,
        makepad_live_tokenizer::{Range},
        builder::{
            builder_protocol::*,
            child_process::{
                ChildProcess,
                ChildLine
            },
            rustc_json::*,
        },
    },
    std::{
        fmt,
        path::{PathBuf},
        sync::{Arc, RwLock},
    },
};

pub struct BuilderServer {
    next_connection_id: usize,
    shared: Arc<RwLock<Shared >>,
}

impl BuilderServer {
    pub fn new<P: Into<PathBuf >> (path: P) -> BuilderServer {
        BuilderServer {
            next_connection_id: 0,
            shared: Arc::new(RwLock::new(Shared {
                path: path.into(),
            })),
        }
    }
    
    pub fn connect(&mut self, msg_sender: Box<dyn MsgSender>) -> BuilderConnection {
        let connection_id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        BuilderConnection {
            connection_id,
            shared: self.shared.clone(),
            msg_sender,
        }
    }
}

pub struct BuilderConnection {
    connection_id: ConnectionId,
    shared: Arc<RwLock<Shared >>,
    msg_sender: Box<dyn MsgSender>,
}

impl BuilderConnection {
    pub fn handle_cmd(&self, cmd_wrap: BuilderCmdWrap) {
        match cmd_wrap.cmd {
            BuilderCmd::CargoCheck => {
                self.cargo_check(cmd_wrap.cmd_id);
            }
        }
    }
    
    fn send_bare_msg(&self, cmd_id: BuilderCmdId, level: BuilderMsgLevel, line: String) {
        self.msg_sender.send_message(
            cmd_id.wrap_msg(BuilderMsg::Bare(BuilderMsgBare {
                line,
                level
            }))
        );
    }
    
    fn send_location_msg(&self, cmd_id: BuilderCmdId, level: BuilderMsgLevel, file_name: String, range: Range, msg: String) {
        self.msg_sender.send_message(
            cmd_id.wrap_msg(BuilderMsg::Location(BuilderMsgLocation {
                level,
                file_name,
                range,
                msg
            }))
        );
    }
    
    fn process_compiler_message(&self, cmd_id: BuilderCmdId, msg: RustcCompilerMessage) {
        if let Some(msg) = msg.message {
            let level = match msg.level.as_ref() {
                "error" => BuilderMsgLevel::Error,
                "warning" => BuilderMsgLevel::Warning,
                other => {
                    self.send_bare_msg(cmd_id, BuilderMsgLevel::Error, format!("process_compiler_message: unexpected level {}", other));
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
                self.send_bare_msg(cmd_id, BuilderMsgLevel::Error, format!("process_compiler_message: no span:  {}", msg.message));
            }
        }
    }
    
    pub fn cargo_check(&self, cmd_id: BuilderCmdId) {
        
        // alright lets run a cargo check and parse its output
        let path = self.shared.read().unwrap().path.clone();
        let args = [
            "check",
            "-p",
            "cmdline_example",
            "--message-format=json"
        ];
        
        let process = ChildProcess::start("cargo", &args, path, &[]).expect("Cannot start process");
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
                                    self.process_compiler_message(cmd_id, msg);
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
                            self.send_bare_msg(cmd_id, BuilderMsgLevel::Log, line);
                        }
                    }
                }
                ChildLine::StdErr(line) => {
                    self.send_bare_msg(cmd_id, BuilderMsgLevel::Error, line);
                }
                ChildLine::Term => {
                    break;
                }
            }
        } 
    }
}

pub trait MsgSender: Send {
    fn box_clone(&self) -> Box<dyn MsgSender>;
    fn send_message(&self, wrap: BuilderMsgWrap);
}

impl<F: Clone + Fn(BuilderMsgWrap) + Send + 'static> MsgSender for F {
    fn box_clone(&self) -> Box<dyn MsgSender> {
        Box::new(self.clone())
    }
    
    fn send_message(&self, wrap: BuilderMsgWrap) {
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

#[derive(Debug)]
struct Shared {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ConnectionId(usize);
