use {
    crate::{
        makepad_micro_serde::*,
        makepad_editor_core::range::{Range},
        build::{
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
        path::{PathBuf},
        sync::{Arc, RwLock, Mutex, mpsc::Sender},
    },
};

struct BuildServerProcess{
    cmd_id: BuildCmdId,
    stdin_sender: Mutex<Sender<ChildStdIn> >,
    line_sender: Mutex<Sender<ChildStdIO> >,
}

struct BuildServerShared {
    path: PathBuf,
    // here we should store our connections send slots
    processes: HashMap<String, BuildServerProcess>
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

#[derive(Debug, PartialEq)]
enum StdErrState{
    First,
    Sync,
    Desync,
    Running,
}

impl BuildConnection {
    
    pub fn cargo_run(&self, what: &str, cmd_id: BuildCmdId) {
        let shared = self.shared.clone();
        let msg_sender = self.msg_sender.clone();
        // alright lets run a cargo check and parse its output
        let path = shared.read().unwrap().path.clone();
        
        if let Ok(shared) = shared.write() {
            if let Some(proc) = shared.processes.get(what) {
                let line_sender = proc.line_sender.lock().unwrap();
                let _ = line_sender.send(ChildStdIO::Kill);
            }
        }
        
        let args = [
            "+nightly",
            "run",
            "-p",
            what,
            "--message-format=json",
            "--release",
            "--features=nightly",
            "--",
            "--message-format=json",
            &format!("--stdin-loop"),
        ];
        
        let process = ChildProcess::start("cargo", &args, path, &[]).expect("Cannot start process");

        shared.write().unwrap().processes.insert(
            what.to_string(),
            BuildServerProcess{
                cmd_id,
                stdin_sender: Mutex::new(process.stdin_sender.clone()),
                line_sender: Mutex::new(process.line_sender.clone())
            }
        );
        let mut stderr_state = StdErrState::First;
        let stdin_sender = process.stdin_sender.clone();
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
                                    "makepad-signal"=>{
                                        let _ = stdin_sender.send(ChildStdIn::Send(format!("{{\"Signal\":[{}]}}\n", msg.signal.unwrap())));
                                    }
                                    "makepad-error-log" | "compiler-message" => {
                                        msg_sender.process_compiler_message(cmd_id, msg);
                                    }
                                    "build-finished"=>{
                                        if Some(true) == msg.success{
                                        }
                                        else{
                                        }
                                    }
                                    "compiler-artifact" => {
                                    }
                                    _ => ()
                                }
                            }
                            Err(_) => { // we should output a log string
                                //eprintln!("GOT ERROR {:?}", err);
                                //log!("{:?}", err);
                                msg_sender.send_stdin_to_host_msg(cmd_id, line);
                            }
                        }
                    }
                    ChildStdIO::StdErr(line) => {
                        // attempt to clean up stderr of cargo
                        match stderr_state{
                            StdErrState::First=>{
                                if line.trim().starts_with("Compiling "){
                                    msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Wait, line);
                                }
                                else if line.trim().starts_with("Finished "){
                                    stderr_state = StdErrState::Running;
                                }
                                else if line.trim().starts_with("error: could not compile "){
                                    msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Log, line);
                                }
                                else{
                                    stderr_state = StdErrState::Desync;
                                    msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Error, line);                                    
                                }
                            }                            
                            StdErrState::Sync | StdErrState::Desync => {
                                msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Error, line);
                            }
                            StdErrState::Running=>{
                                if line.trim().starts_with("Running "){
                                     msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Wait, format!("{}",line.trim()));
                                    stderr_state = StdErrState::Sync
                                }
                                else{
                                    stderr_state = StdErrState::Desync;
                                    msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Error, line);
                                }
                            }
                        }
                    }
                    ChildStdIO::Term => {
                        msg_sender.send_bare_msg(cmd_id, BuildMsgLevel::Log, "process terminated".into());
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
            BuildCmd::CargoRun {what} => {
                // lets kill all other 'whats'
                self.cargo_run(&what, cmd_wrap.cmd_id);
            }
            BuildCmd::HostToStdin(msg)=>{
                // ok lets fetch the running process from the cmd_id
                // and plug this msg on the standard input as serialiser json
                if let Ok(shared) = self.shared.read(){
                    for v in shared.processes.values(){
                        if v.cmd_id == cmd_wrap.cmd_id{
                            // lets send it on sender
                            if let Ok(stdin_sender) = v.stdin_sender.lock(){
                                let _= stdin_sender.send(ChildStdIn::Send(msg));
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
    fn send_message(&self, wrap: BuildMsgWrap);
    
    fn send_bare_msg(&self, cmd_id: BuildCmdId, level: BuildMsgLevel, line: String) {
        let line = line.trim();
        self.send_message(
            cmd_id.wrap_msg(BuildMsg::Bare(BuildMsgBare {
                line:line.to_string(),
                level
            }))
        );
    }
    
    fn send_stdin_to_host_msg(&self, cmd_id: BuildCmdId, line: String) {
        self.send_message(
            cmd_id.wrap_msg(BuildMsg::StdinToHost(line))
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
                "log" => BuildMsgLevel::Log,
                "failure-note" => BuildMsgLevel::Error,
                "panic"=>BuildMsgLevel::Panic,
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
                if msg.message.trim().starts_with("aborting due to ") ||
                    msg.message.trim().starts_with("For more information about this error") ||
                        msg.message.trim().ends_with("warning emitted") ||
                        msg.message.trim().ends_with("warnings emitted"){
                }
                else{
                    self.send_bare_msg(cmd_id, BuildMsgLevel::Warning, msg.message);
                }
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
