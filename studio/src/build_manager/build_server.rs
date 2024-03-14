use {
    crate::{
        makepad_code_editor::text::{Position},
        makepad_micro_serde::*,
        makepad_live_id::*,
        makepad_platform::log::LogLevel,
        build_manager::{
            build_protocol::*,
            child_process::{
                ChildStdIn,
                ChildProcess,
                ChildStdIO
            },
            rustc_json::*,
        },
    },
    std::{
        collections::HashMap,
        fmt,
        path::PathBuf,
        sync::{Arc, RwLock, Mutex, mpsc::Sender},
    },
};

struct BuildServerProcess {
    cmd_id: LiveId,
    stdin_sender: Mutex<Sender<ChildStdIn> >,
    line_sender: Mutex<Sender<ChildStdIO> >,
}

struct BuildServerShared {
    path: PathBuf,
    // here we should store our connections send slots
    processes: HashMap<BuildProcess, BuildServerProcess>
}

pub struct BuildServer {
    shared: Arc<RwLock<BuildServerShared >>,
}

impl BuildServer {
    pub fn new<P: Into<PathBuf >> (path: P) -> BuildServer {
        BuildServer {
            shared: Arc::new(RwLock::new(BuildServerShared {
                path: path.into(),
                processes: Default::default()
            })),
        }
    }
    
    pub fn connect(&mut self, msg_sender: Box<dyn MsgSender>) -> BuildConnection {
        BuildConnection {
            shared: self.shared.clone(),
            msg_sender,
        }
    }
}

pub struct BuildConnection {
    //    connection_id: ConnectionId,
    shared: Arc<RwLock<BuildServerShared >>,
    msg_sender: Box<dyn MsgSender>,
}
/*
#[derive(Debug, PartialEq)]
enum StdErrState {
    First,
    Sync,
    Desync,
    Running,
}*/


impl BuildConnection {
    
    pub fn stop(&self, cmd_id: LiveId) {
        let shared = self.shared.clone();
        
        let shared = shared.write().unwrap();
        if let Some(proc) = shared.processes.values().find(|v| v.cmd_id == cmd_id) {
            let line_sender = proc.line_sender.lock().unwrap();
            let _ = line_sender.send(ChildStdIO::Kill);
        }
    }
    
    pub fn run(&self, what: BuildProcess, cmd_id: LiveId, http:String) {
        let shared = self.shared.clone();
        let msg_sender = self.msg_sender.clone();
        // alright lets run a cargo check and parse its output
        let path = shared.read().unwrap().path.clone();
        
        let args: Vec<String> = match &what.target {
            BuildTarget::ReleaseStudio => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "run".into(),
                "-p".into(),
                what.binary.clone(),
                "--message-format=json".into(),
                "--release".into(),
                "--".into(),
                "--message-format=json".into(),
                "--stdin-loop".into(),
            ],
            BuildTarget::DebugStudio => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "run".into(),
                "-p".into(),
                what.binary.clone(),
                "--message-format=json".into(),
                "--".into(),
                "--message-format=json".into(),
                "--stdin-loop".into(),
            ],
            BuildTarget::Release => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "run".into(),
                "-p".into(),
                what.binary.clone(),
                "--message-format=json".into(),
                "--release".into(),
                "--".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::Debug => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "run".into(),
                "-p".into(),
                what.binary.clone(),
                "--message-format=json".into(),
                "--".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::Profiler => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "instruments".into(),
                "-t".into(),
                "time".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
                "--".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::IosSim  => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "apple".into(),
                "ios".into(),
                format!("--org={}", "makepad"),
                format!("--app={}", "example"),
                "run-sim".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::IosDevice => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "ios".into(),
                format!("--org={}", "makepad"),
                format!("--app={}", "example"),
                "run-device".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::TvosSim  => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "apple".into(),
                "tvos".into(),
                format!("--org={}", "makepad"),
                format!("--app={}", "example"),
                "run-sim".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::TvosDevice => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "apple".into(),
                "tvos".into(),
                format!("--org={}", "makepad"),
                format!("--app={}", "example"),
                "run-device".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::Android => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "android".into(),
                "run".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::WebAssembly => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "wasm".into(),
                "build".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::CheckMacos => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "check".into(),
                "--target=aarch64-apple-darwin".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::CheckWindows => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "check".into(),
                "--target=x86_64-pc-windows-msvc".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],
            BuildTarget::CheckLinux => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "check".into(),
                "--target=x86_64-unknown-linux-gnu".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ],            
            BuildTarget::CheckAll => vec![
                "run".into(),
                "nightly".into(),
                "cargo".into(),
                "makepad".into(),
                "check".into(),
                "all".into(),
                "-p".into(),
                what.binary.clone(),
                "--release".into(),
                "--message-format=json".into(),
            ]
        };
        
        let http = format!("{}/{}", http, cmd_id.0);
        let env = [
            ("MAKEPAD_STUDIO_HTTP", http.as_str()),
            ("MAKEPAD", "lines")
        ];

        let process = ChildProcess::start("rustup", &args, path, &env).expect("Cannot start process");
        
        shared.write().unwrap().processes.insert(
            what,
            BuildServerProcess {
                cmd_id,
                stdin_sender: Mutex::new(process.stdin_sender.clone()),
                line_sender: Mutex::new(process.line_sender.clone()),
            }
        );

        // HACK(eddyb) do this first, as there is no way to actually send the
        // initial swapchain to the client at all, unless we have this first
        // (thankfully sending this before we ever read from the client means
        // it will definitely arrive before C->H ReadyToStart triggers anything).
        msg_sender.send_message(BuildClientMessageWrap{
            cmd_id,
            message: BuildClientMessage::AuxChanHostEndpointCreated(process.aux_chan_host_endpoint.clone()),
        });

       // let mut stderr_state = StdErrState::First;
        //let stdin_sender = process.stdin_sender.clone();
        std::thread::spawn(move || {
            // lets create a BuildProcess and run it
            while let Ok(line) = process.line_receiver.recv() {
                
                match line {
                    ChildStdIO::StdOut(line) => {
                        let comp_msg: Result<RustcCompilerMessage, DeJsonErr> = DeJson::deserialize_json(&line);
                        match comp_msg {
                            Ok(msg) => {
                                // alright we have a couple of 'reasons'
                                match msg.reason.as_str() {
                                    "makepad-error-log" | "compiler-message" => {
                                        msg_sender.process_compiler_message(cmd_id, msg);
                                    }
                                    "build-finished" => {
                                        if Some(true) == msg.success {
                                        }
                                        else {
                                        }
                                    }
                                    "compiler-artifact" => {
                                    }
                                    _ => ()
                                }
                            }
                            Err(_) => { // we should output a log string
                                //eprintln!("GOT ERROR {:?}", err);
                                msg_sender.send_stdin_to_host_msg(cmd_id, line);
                            }
                        }                        
                    }
                    ChildStdIO::StdErr(line) => {
                        if line.trim().starts_with("Running ") {
                           msg_sender.send_bare_message(cmd_id, LogLevel::Wait, line);
                        }
                        else if line.trim().starts_with("Compiling ") {
                           msg_sender.send_bare_message(cmd_id, LogLevel::Wait, line);
                        }
                        else if line.trim().starts_with("Blocking waiting for file lock on package cache") {
                            //msg_sender.send_bare_msg(cmd_id, LogItemLevel::Wait, line);
                        }
                        else if line.trim().starts_with("Checking ") {
                            //msg_sender.send_bare_msg(cmd_id, LogItemLevel::Wait, line);
                        }
                        else if line.trim().starts_with("Finished ") {
                            //stderr_state = StdErrState::Running;
                        }
                        else{
                            msg_sender.send_bare_message(cmd_id, LogLevel::Error, line);
                        }
                    }
                    ChildStdIO::Term => {
                        msg_sender.send_bare_message(cmd_id, LogLevel::Log, "process terminated".into());
                        break;
                    }
                    ChildStdIO::Kill => {
                        return process.kill();
                    }
                }
            };
        });
    }
    
    pub fn handle_cmd(&self, cmd_wrap: BuildCmdWrap) {
        match cmd_wrap.cmd {
            BuildCmd::Run(process, http) => {
                // lets kill all other 'whats'
                self.run(process, cmd_wrap.cmd_id, http);
            }
            BuildCmd::Stop => {
                // lets kill all other 'whats'
                self.stop(cmd_wrap.cmd_id);
            }
            BuildCmd::HostToStdin(msg) => {
                // ok lets fetch the running process from the cmd_id
                // and plug this msg on the standard input as serialiser json
                if let Ok(shared) = self.shared.read() {
                    for v in shared.processes.values() {
                        if v.cmd_id == cmd_wrap.cmd_id {
                            // lets send it on sender
                            if let Ok(stdin_sender) = v.stdin_sender.lock() {
                                let _ = stdin_sender.send(ChildStdIn::Send(msg));
                            }
                            break;
                        }
                    }
                }
            }
        }
    }
    
}

pub trait MsgSender: Send {
    fn box_clone(&self) -> Box<dyn MsgSender>;
    fn send_message(&self, wrap: BuildClientMessageWrap);
    
    fn send_bare_message(&self, cmd_id: LiveId, level: LogLevel, line: String) {
        let line = line.trim();
        self.send_message(BuildClientMessageWrap{
            cmd_id,
            message:BuildClientMessage::LogItem(LogItem::Bare(LogItemBare {
                line: line.to_string(),
                level
            }))
        });
    }
    
    fn send_stdin_to_host_msg(&self, cmd_id: LiveId, line: String) {
        self.send_message(BuildClientMessageWrap{
            cmd_id,
            message:BuildClientMessage::LogItem(LogItem::StdinToHost(line))
        });
    }
    

    fn send_location_msg(&self, cmd_id: LiveId, level: LogLevel, file_name: String, start: Position, end: Position, message: String) {
        self.send_message(
            BuildClientMessageWrap{
                cmd_id,
                message:BuildClientMessage::LogItem(LogItem::Location(LogItemLocation {
                level,
                file_name,
                start,
                end,
                message
            }))
        });
    }
    
    fn process_compiler_message(&self, cmd_id: LiveId, msg: RustcCompilerMessage) {
        if let Some(msg) = msg.message {
            
            let level = match msg.level.as_ref() {
                "error" => LogLevel::Error,
                "warning" => LogLevel::Warning,
                "log" => LogLevel::Log,
                "failure-note" => LogLevel::Error,
                "panic" => LogLevel::Panic,
                other => {
                    self.send_bare_message(cmd_id, LogLevel::Error, format!("process_compiler_message: unexpected level {}", other));
                    return
                }
            };
            if let LogLevel::Warning = level{
                if msg.message.starts_with("unstable feature specified for"){
                    return
                }
            }
            if let Some(span) = msg.spans.iter().find( | span | span.is_primary) {
               
                self.send_location_msg(cmd_id, level, span.file_name.clone(),span.start(), span.end(), msg.message.clone());
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
                if msg.message.trim().starts_with("Some errors have detailed explanations") ||
                msg.message.trim().starts_with("For more information about an error") ||
                msg.message.trim().contains("warnings emitted") ||
                msg.message.trim().contains("warning emitted") {
                }
                else {
                    self.send_bare_message(cmd_id, LogLevel::Warning, msg.message);
                }
            }
        }
    }
}

impl<F: Clone + Fn(BuildClientMessageWrap) + Send + 'static> MsgSender for F {
    fn box_clone(&self) -> Box<dyn MsgSender> {
        Box::new(self.clone())
    }
    
    fn send_message(&self, wrap: BuildClientMessageWrap) {
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