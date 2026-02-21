use {
    crate::{
        build_manager::{
            build_protocol::*,
            child_process::{ChildProcess, ChildStdIO, ChildStdIn},
            rustc_json::*,
        },
        makepad_code_editor::text::Position,
        makepad_file_server::FileSystemRoots,
        makepad_live_id::*,
        makepad_micro_serde::*,
        makepad_platform::cx_stdin::HostToStdin,
        makepad_platform::log::LogLevel,
    },
    std::{
        collections::HashMap,
        env, fmt,
        sync::{mpsc::Sender, Arc, Mutex, RwLock},
        time::{Instant, SystemTime, UNIX_EPOCH},
    },
};

const TICK_HICCUP_THRESHOLD_FRAMES: f64 = 1.5;
const TICK_HICCUP_FRAME_TIME_SECONDS: f64 = 1.0 / 60.0;
const TICK_HICCUP_THRESHOLD_SECONDS: f64 =
    TICK_HICCUP_THRESHOLD_FRAMES * TICK_HICCUP_FRAME_TIME_SECONDS;

#[derive(Default)]
struct TickMonitorState {
    last_tick: Option<Instant>,
    tick_count: u64,
    hiccup_count: u64,
}

struct TickHiccupEvent {
    wall_clock_ms: u128,
    delta_ms: f64,
    delta_frames_60hz: f64,
    tick_count: u64,
    hiccup_count: u64,
}

struct BuildServerProcess {
    stdin_sender: Mutex<Sender<ChildStdIn>>,
    line_sender: Mutex<Sender<ChildStdIO>>,
    tick_monitor: Mutex<TickMonitorState>,
}

struct BuildServerShared {
    roots: FileSystemRoots,
    // here we should store our connections send slots
    processes: HashMap<LiveId, BuildServerProcess>,
}

pub struct BuildServer {
    shared: Arc<RwLock<BuildServerShared>>,
}

impl BuildServer {
    pub fn new(roots: FileSystemRoots) -> BuildServer {
        BuildServer {
            shared: Arc::new(RwLock::new(BuildServerShared {
                roots,
                processes: Default::default(),
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
    shared: Arc<RwLock<BuildServerShared>>,
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
    fn observe_tick_hiccup(
        process: &BuildServerProcess,
        msg_json: &str,
    ) -> Option<TickHiccupEvent> {
        if !msg_json.contains("Tick") {
            return None;
        }
        if !matches!(HostToStdin::deserialize_json(msg_json).ok()?, HostToStdin::Tick) {
            return None;
        }

        let now = Instant::now();
        let wall_clock_ms = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_millis();
        let mut tick_monitor = process.tick_monitor.lock().ok()?;
        tick_monitor.tick_count = tick_monitor.tick_count.saturating_add(1);

        let Some(last_tick) = tick_monitor.last_tick.replace(now) else {
            return None;
        };

        let delta_seconds = now.duration_since(last_tick).as_secs_f64();
        if delta_seconds <= TICK_HICCUP_THRESHOLD_SECONDS {
            return None;
        }

        tick_monitor.hiccup_count = tick_monitor.hiccup_count.saturating_add(1);
        Some(TickHiccupEvent {
            wall_clock_ms,
            delta_ms: delta_seconds * 1000.0,
            delta_frames_60hz: delta_seconds / TICK_HICCUP_FRAME_TIME_SECONDS,
            tick_count: tick_monitor.tick_count,
            hiccup_count: tick_monitor.hiccup_count,
        })
    }

    pub fn stop(&self, cmd_id: LiveId) {
        let shared = self.shared.clone();
        let mut shared = shared.write().unwrap();
        if let Some(proc) = shared.processes.remove(&cmd_id) {
            let line_sender = proc.line_sender.lock().unwrap();
            let _ = line_sender.send(ChildStdIO::Kill);
        }
    }

    pub fn run(&self, what: BuildProcess, cmd_id: LiveId, studio_addr: String) {
        let args = default_cargo_args_for_build_target(&what);
        self.run_with_args(what, cmd_id, studio_addr, args, HashMap::new());
    }

    pub fn run_cargo(
        &self,
        what: BuildProcess,
        cmd_id: LiveId,
        studio_addr: String,
        args: Vec<String>,
        extra_env: HashMap<String, String>,
    ) {
        self.run_with_args(what, cmd_id, studio_addr, args, extra_env);
    }

    fn run_with_args(
        &self,
        what: BuildProcess,
        cmd_id: LiveId,
        studio_addr: String,
        args: Vec<String>,
        extra_env: HashMap<String, String>,
    ) {
        self.stop(cmd_id);
        let shared = self.shared.clone();
        let msg_sender = self.msg_sender.clone();
        // alright lets run a cargo check and parse its output
        let path = match shared.read().unwrap().roots.find_root(&what.root) {
            Ok(path) => path.clone(),
            Err(_) => {
                msg_sender.send_bare_message(
                    cmd_id,
                    LogLevel::Error,
                    format!("unknown build root '{}'", what.root),
                );
                return;
            }
        };

        let is_in_studio = matches!(
            what.target,
            BuildTarget::ReleaseStudio | BuildTarget::DebugStudio
        );

        let cmd_id_string = cmd_id.0.to_string();
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("RUST_BACKTRACE".to_string(), "1".to_string());
        env.insert("MAKEPAD".to_string(), "lines".to_string());
        for (key, value) in extra_env {
            env.insert(key, value);
        }
        if is_in_studio && !studio_addr.trim().is_empty() {
            env.insert("STUDIO".to_string(), studio_addr.clone());
            env.insert("STUDIO_BUILD_ID".to_string(), cmd_id_string.clone());
        }
        if matches!(what.target, BuildTarget::Harmony) {
            env.insert("MAKEPAD".to_string(), "no_android_choreographer".to_string());
        }

        // Default to nightly rustc but don't overwrite any user request for a
        // specific nightly version.
        // FIXME: also apply this for overrides set using rustup override rather
        // than using an env var or as commandline argument.
        if !env.contains_key("RUSTUP_TOOLCHAIN")
            && !env::var("RUSTUP_TOOLCHAIN").map_or(false, |toolchain| toolchain.contains("nightly"))
        {
            env.insert("RUSTUP_TOOLCHAIN".to_string(), "nightly".to_string());
        }

        let env: Vec<(String, String)> = env.into_iter().collect();
        let process = ChildProcess::start("cargo", &args, path.to_path_buf(), &env, is_in_studio)
            .expect("Cannot start process");

        shared.write().unwrap().processes.insert(
            cmd_id,
            BuildServerProcess {
                stdin_sender: Mutex::new(process.stdin_sender.clone()),
                line_sender: Mutex::new(process.line_sender.clone()),
                tick_monitor: Mutex::new(TickMonitorState::default()),
            },
        );

        // HACK(eddyb) do this first, as there is no way to actually send the
        // initial swapchain to the client at all, unless we have this first
        // (thankfully sending this before we ever read from the client means
        // it will definitely arrive before C->H ReadyToStart triggers anything)
        if is_in_studio {
            msg_sender.send_message(BuildClientMessageWrap {
                cmd_id,
                message: BuildClientMessage::AuxChanHostEndpointCreated(
                    process.aux_chan_host_endpoint.clone().unwrap(),
                ),
            });
        }

        // let mut stderr_state = StdErrState::First;
        //let stdin_sender = process.stdin_sender.clone();
        std::thread::spawn(move || {
            // lets create a BuildProcess and run it
            while let Ok(line) = process.line_receiver.recv() {
                match line {
                    ChildStdIO::StdOut(line) => {
                        let comp_msg: Result<RustcCompilerMessage, DeJsonErr> =
                            DeJson::deserialize_json(&line);
                        match comp_msg {
                            Ok(msg) => {
                                // alright we have a couple of 'reasons'
                                match msg.reason.as_str() {
                                    "makepad-error-log" | "compiler-message" => {
                                        msg_sender.process_compiler_message(cmd_id, msg);
                                    }
                                    "build-finished" => {
                                        if Some(true) == msg.success {
                                        } else {
                                        }
                                    }
                                    "compiler-artifact" => {}
                                    _ => (),
                                }
                            }
                            Err(_) => {
                                // we should output a log string
                                //eprintln!("GOT ERROR {:?}", err);
                                msg_sender.send_stdin_to_host_msg(cmd_id, line);
                            }
                        }
                    }
                    ChildStdIO::StdErr(line) => {
                        if line.trim().starts_with("Running ") {
                            msg_sender.send_bare_message(cmd_id, LogLevel::Wait, line);
                        } else if line.trim().starts_with("Compiling ") {
                            msg_sender.send_bare_message(cmd_id, LogLevel::Wait, line);
                        } else if line
                            .trim()
                            .starts_with("Blocking waiting for file lock on package cache")
                        {
                            //msg_sender.send_bare_msg(cmd_id, LogItemLevel::Wait, line);
                        } else if line.trim().starts_with("Checking ") {
                            //msg_sender.send_bare_msg(cmd_id, LogItemLevel::Wait, line);
                        } else if line.trim().starts_with("Finished ") {
                            //stderr_state = StdErrState::Running;
                        } else {
                            msg_sender.send_bare_message(cmd_id, LogLevel::Error, line);
                        }
                    }
                    ChildStdIO::Term => {
                        msg_sender.send_bare_message(
                            cmd_id,
                            LogLevel::Log,
                            "process terminated".into(),
                        );
                        break;
                    }
                    ChildStdIO::Kill => {
                        return process.kill();
                    }
                }
            }
        });
    }

    pub fn handle_cmd(&self, cmd_wrap: BuildCmdWrap) {
        match cmd_wrap.cmd {
            BuildCmd::Run(process, studio_addr) => {
                // lets kill all other 'whats'
                self.run(process, cmd_wrap.cmd_id, studio_addr);
            }
            BuildCmd::RunCargo(process, args, studio_addr, env) => {
                self.run_cargo(process, cmd_wrap.cmd_id, studio_addr, args, env);
            }
            BuildCmd::Stop => {
                // lets kill all other 'whats'
                self.stop(cmd_wrap.cmd_id);
            }
            BuildCmd::HostToStdin(msg) => {
                // ok lets fetch the running process from the cmd_id
                // and plug this msg on the standard input as serialiser json
                if let Ok(shared) = self.shared.read() {
                    if let Some(v) = shared.processes.get(&cmd_wrap.cmd_id) {
                        if let Some(hiccup) = Self::observe_tick_hiccup(v, &msg) {
                            self.msg_sender.send_bare_message(
                                cmd_wrap.cmd_id,
                                LogLevel::Warning,
                                format!(
                                    "[tick-hiccup] wall_ms={} dt_ms={:.2} (~{:.2} frames @60hz) tick={} hiccup={}",
                                    hiccup.wall_clock_ms,
                                    hiccup.delta_ms,
                                    hiccup.delta_frames_60hz,
                                    hiccup.tick_count,
                                    hiccup.hiccup_count
                                ),
                            );
                        }
                        if let Ok(stdin_sender) = v.stdin_sender.lock() {
                            let _ = stdin_sender.send(ChildStdIn::Send(msg));
                        }
                    }
                }
            }
        }
    }
}

fn default_cargo_args_for_build_target(what: &BuildProcess) -> Vec<String> {
    match &what.target {
        BuildTarget::ReleaseStudio => vec![
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
            "-p".into(),
            what.binary.clone(),
            "--message-format=json".into(),
            "--".into(),
            "--message-format=json".into(),
            "--stdin-loop".into(),
        ],
        BuildTarget::Release => vec![
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
            "-p".into(),
            what.binary.clone(),
            "--message-format=json".into(),
            "--".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::Profiler => vec![
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
        BuildTarget::IosSim => vec![
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
        BuildTarget::TvosSim => vec![
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
            "makepad".into(),
            "apple".into(),
            "tvos".into(),
            "--org=makepad".into(),
            "--app=aiview".into(),
            "--app=aiview".into(),
            "--cert=61".into(),
            "--device=F8,27".into(),
            "--profile=./local/tvos4.mobileprovision".into(),
            "run-device".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::Android => vec![
            "makepad".into(),
            "android".into(),
            "--variant=default".into(),
            "run".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::Quest => vec![
            "makepad".into(),
            "android".into(),
            "--variant=quest".into(),
            "run".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::Harmony => vec![
            "makepad".into(),
            "android".into(),
            "run".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::WebAssembly => vec![
            "makepad".into(),
            "wasm".into(),
            "run".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::CheckMacos => vec![
            "check".into(),
            "--target=aarch64-apple-darwin".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::CheckWindows => vec![
            "check".into(),
            "--target=x86_64-pc-windows-msvc".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::CheckLinux => vec![
            "check".into(),
            "--target=x86_64-unknown-linux-gnu".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
        BuildTarget::CheckAll => vec![
            "makepad".into(),
            "check".into(),
            "all".into(),
            "-p".into(),
            what.binary.clone(),
            "--release".into(),
            "--message-format=json".into(),
        ],
    }
}

pub trait MsgSender: Send {
    fn box_clone(&self) -> Box<dyn MsgSender>;
    fn send_message(&self, wrap: BuildClientMessageWrap);

    fn send_bare_message(&self, cmd_id: LiveId, level: LogLevel, line: String) {
        let line = line.trim();
        self.send_message(BuildClientMessageWrap {
            cmd_id,
            message: BuildClientMessage::LogItem(LogItem::Bare(LogItemBare {
                line: line.to_string(),
                level,
            })),
        });
    }

    fn send_stdin_to_host_msg(&self, cmd_id: LiveId, line: String) {
        self.send_message(BuildClientMessageWrap {
            cmd_id,
            message: BuildClientMessage::LogItem(LogItem::StdinToHost(line)),
        });
    }

    fn send_location_msg(
        &self,
        cmd_id: LiveId,
        level: LogLevel,
        file_name: String,
        start: Position,
        end: Position,
        message: String,
        explanation: Option<String>,
    ) {
        self.send_message(BuildClientMessageWrap {
            cmd_id,
            message: BuildClientMessage::LogItem(LogItem::Location(LogItemLocation {
                level,
                file_name: file_name.replace("\\", "/"),
                start,
                end,
                message,
                explanation,
            })),
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
                    self.send_bare_message(
                        cmd_id,
                        LogLevel::Error,
                        format!("process_compiler_message: unexpected level {}", other),
                    );
                    return;
                }
            };
            if let LogLevel::Warning = level {
                if msg.message.starts_with("unstable feature specified for") {
                    return;
                }
            }
            if let Some(span) = msg.spans.iter().find(|span| span.is_primary) {
                self.send_location_msg(
                    cmd_id,
                    level,
                    span.file_name.clone(),
                    span.start(),
                    span.end(),
                    msg.message,
                    msg.rendered,
                );
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
            } else {
                if msg
                    .message
                    .trim()
                    .starts_with("Some errors have detailed explanations")
                    || msg
                        .message
                        .trim()
                        .starts_with("For more information about an error")
                    || msg.message.trim().contains("warnings emitted")
                    || msg.message.trim().contains("warning emitted")
                {
                } else {
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
        self(wrap)
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
