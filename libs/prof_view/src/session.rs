//! One attached app: a makepad-remote link to its endpoint, the `prof` stream feeding a shared
//! [`Profile`] on a worker, the bytes of the current recording, and `cmd` requests (panics,
//! threads, memory, code versions) answered on another worker. The UI reads the profile under
//! its lock when it draws; the workers wake it with a UI signal.

use makepad_prof_stream::Profile;
use makepad_widgets::{SignalToUI, ThreadOptions, ThreadSpawner};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// A running makepad app that can be attached to.
#[derive(Clone, Debug, PartialEq)]
pub struct AppEntry {
    pub pid: u32,
    pub app: String,
    pub socket: String,
    /// it serves the profiler stream
    pub prof: bool,
    /// a makepad run (JIT) app; else a native build with its endpoint on
    pub jit: bool,
}

/// The local apps with a makepad-remote endpoint (this process left out), newest first.
pub fn local_apps() -> Vec<AppEntry> {
    let me = std::process::id();
    let mut v: Vec<AppEntry> = makepad_remote::discovery::apps()
        .into_iter()
        .filter(|a| a.pid != me)
        .map(|a| AppEntry { pid: a.pid, prof: a.serves("prof"), jit: a.text("run").is_some(), app: a.app, socket: a.socket })
        .collect();
    v.reverse();
    v
}

/// What the session tells the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting,
    Live,
    /// the app went away or refused: why
    Ended(String),
}

/// A `cmd` answer: the request and its lines.
#[derive(Clone, Debug)]
pub struct CmdAnswer {
    pub line: String,
    pub lines: Vec<String>,
    pub error: Option<String>,
}

pub struct Session {
    pub entry: AppEntry,
    /// the live profile (everything since attach)
    pub profile: Arc<Mutex<Profile>>,
    /// recording: the stream bytes since Record (None: not recording)
    pub recording: Arc<Mutex<Option<Vec<u8>>>>,
    /// the stream's prelude and first records (HELLO, strings and lanes before the recording
    /// started): a recording starts with them so it reads on its own
    head: Arc<Mutex<Vec<u8>>>,
    /// the GPU stream's header and string records (a recording needs them too)
    gpu_head: Arc<Mutex<Vec<u8>>>,
    pub status: Arc<Mutex<Status>>,
    /// bytes received
    pub bytes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    cmd_tx: Sender<String>,
    cmd_rx: Receiver<CmdAnswer>,
}

impl Session {
    pub fn attach(spawner: &ThreadSpawner, entry: AppEntry) -> Result<Session, String> {
        let profile = Arc::new(Mutex::new(Profile::new()));
        let recording: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let head = Arc::new(Mutex::new(Vec::new()));
        let gpu_head = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(Mutex::new(Status::Connecting));
        let bytes = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (cmd_tx, cmd_in) = mpsc::channel::<String>();
        let (cmd_out, cmd_rx) = mpsc::channel::<CmdAnswer>();
        {
            let (socket, profile, recording, head, status, bytes, stop) = (entry.socket.clone(), profile.clone(), recording.clone(), head.clone(), status.clone(), bytes.clone(), stop.clone());
            spawner
                .spawn_worker(ThreadOptions::default(), move || stream(&socket, &profile, &recording, &head, &status, &bytes, &stop))
                .map_err(|e| format!("profiler stream worker: {e:?}"))?
                .detach();
        }
        {
            let (socket, profile, recording, gpu_head, stop) = (entry.socket.clone(), profile.clone(), recording.clone(), gpu_head.clone(), stop.clone());
            spawner
                .spawn_worker(ThreadOptions::default(), move || gpu_stream(&socket, &profile, &recording, &gpu_head, &stop))
                .map_err(|e| format!("GPU stream worker: {e:?}"))?
                .detach();
        }
        {
            let (socket, stop) = (entry.socket.clone(), stop.clone());
            spawner
                .spawn_worker(ThreadOptions::default(), move || commands(&socket, cmd_in, cmd_out, &stop))
                .map_err(|e| format!("command worker: {e:?}"))?
                .detach();
        }
        Ok(Session { entry, profile, recording, head, gpu_head, status, bytes, stop, cmd_tx, cmd_rx })
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn recording(&self) -> bool {
        self.recording.lock().unwrap().is_some()
    }

    /// Starts a recording (the stream bytes from now on, after the stream's head).
    pub fn record(&self) {
        let mut head = self.head.lock().unwrap().clone();
        if head.is_empty() {
            head.extend_from_slice(&makepad_prof_stream::prelude());
        }
        head.extend_from_slice(&self.gpu_head.lock().unwrap());
        *self.recording.lock().unwrap() = Some(head);
    }

    /// Ends the recording: its `.mkpf` bytes.
    pub fn stop_recording(&self) -> Option<Vec<u8>> {
        self.recording.lock().unwrap().take()
    }

    /// Sends a `cmd` request line (the answer comes back through [`Session::answers`]).
    pub fn ask(&self, line: &str) {
        let _ = self.cmd_tx.send(line.to_string());
    }

    pub fn answers(&self) -> Vec<CmdAnswer> {
        self.cmd_rx.try_iter().collect()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

/// The records a recording needs before its first data: every STRINGS and THREAD record and the
/// HELLO / CLOCK seen so far (data records refer to them).
fn keep_head(head: &mut Vec<u8>, msg: &[u8]) {
    let mut i = 0;
    if head.is_empty() && msg.len() >= makepad_prof_stream::PRELUDE_LEN && &msg[..4] == makepad_prof_stream::MAGIC {
        head.extend_from_slice(&msg[..makepad_prof_stream::PRELUDE_LEN]);
        i = makepad_prof_stream::PRELUDE_LEN;
    }
    use makepad_prof_stream::kind;
    while i + makepad_prof_stream::HEADER_LEN <= msg.len() {
        let k = u16::from_le_bytes([msg[i], msg[i + 1]]);
        let len = u32::from_le_bytes([msg[i + 4], msg[i + 5], msg[i + 6], msg[i + 7]]) as usize;
        let end = (i + makepad_prof_stream::HEADER_LEN + len).min(msg.len());
        if matches!(k, kind::HELLO | kind::CLOCK | kind::STRINGS | kind::THREAD | kind::GPU_HEADER | kind::GPU_STRINGS) {
            head.extend_from_slice(&msg[i..end]);
        }
        i = end;
    }
}

fn stream(socket: &str, profile: &Mutex<Profile>, recording: &Mutex<Option<Vec<u8>>>, head: &Mutex<Vec<u8>>, status: &Mutex<Status>, bytes: &AtomicU64, stop: &AtomicBool) {
    let end = |why: String| {
        *status.lock().unwrap() = Status::Ended(why);
        SignalToUI::set_ui_signal();
    };
    let client = match makepad_remote::Client::connect(socket) {
        Ok(c) => c,
        Err(e) => return end(format!("cannot connect: {e:?}")),
    };
    // hz=: the stream samples every program thread's CPU stacks too (SAMPLES: `top fn`, the JIT's
    // inline levels and tiers per frame)
    let ch = match client.open("prof", "hz=1000") {
        Ok(c) => c,
        Err(e) => return end(format!("the app has no profiler stream: {e:?}")),
    };
    *status.lock().unwrap() = Status::Live;
    SignalToUI::set_ui_signal();
    while !stop.load(Relaxed) {
        match ch.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(makepad_remote::Event::Data(d)) => {
                bytes.fetch_add(d.len() as u64, Relaxed);
                keep_head(&mut head.lock().unwrap(), &d);
                if let Some(r) = recording.lock().unwrap().as_mut() {
                    r.extend_from_slice(&d);
                }
                profile.lock().unwrap().feed(&d);
                SignalToUI::set_ui_signal();
            }
            Ok(makepad_remote::Event::Closed(_, why)) => return end(if why.is_empty() { "the app closed the stream".into() } else { why }),
            Ok(_) => {}
            Err(true) => {}
            Err(false) => return end("the app went away".into()),
        }
    }
}

/// The GPU profiler's `gpuprof` stream (GPUPROF's producer), when the app serves it: its chunks
/// become GPU records of the profile and of the recording.
fn gpu_stream(socket: &str, profile: &Mutex<Profile>, recording: &Mutex<Option<Vec<u8>>>, gpu_head: &Mutex<Vec<u8>>, stop: &AtomicBool) {
    let Ok(client) = makepad_remote::Client::connect(socket) else { return };
    if !client.caps().iter().any(|c| c.starts_with("gpuprof/")) {
        return;
    }
    let Ok(ch) = client.open("gpuprof", "") else { return };
    while !stop.load(Relaxed) {
        match ch.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(makepad_remote::Event::Data(d)) => {
                let recs = makepad_prof_stream::gpu_records(&d);
                let mut r = makepad_prof_stream::Reader::new();
                r.push(&makepad_prof_stream::prelude());
                r.push(&recs);
                {
                    let mut p = profile.lock().unwrap();
                    let mut head = gpu_head.lock().unwrap();
                    while let Ok(Some((k, payload))) = r.next() {
                        if k == makepad_prof_stream::kind::GPU_HEADER {
                            head.clear();
                        }
                        if k != makepad_prof_stream::kind::GPU_FRAMES {
                            makepad_prof_stream::record(&mut head, k, payload);
                        }
                        let _ = p.apply(k, payload);
                    }
                }
                if let Some(rec) = recording.lock().unwrap().as_mut() {
                    rec.extend_from_slice(&recs);
                }
                SignalToUI::set_ui_signal();
            }
            Ok(makepad_remote::Event::Closed(..)) | Err(false) => return,
            _ => {}
        }
    }
}

fn commands(socket: &str, inbox: Receiver<String>, out: Sender<CmdAnswer>, stop: &AtomicBool) {
    let client = makepad_remote::Client::connect(socket).ok();
    for line in inbox {
        if stop.load(Relaxed) {
            return;
        }
        let answer = match &client {
            None => CmdAnswer { line: line.clone(), lines: Vec::new(), error: Some("not connected".into()) },
            Some(c) => match c.cmd(&line) {
                Ok(lines) => {
                    let mut v = Vec::new();
                    let mut error = None;
                    for l in lines {
                        match l {
                            Ok(l) => v.push(l),
                            Err(e) => error = Some(format!("{e:?}")),
                        }
                    }
                    CmdAnswer { line: line.clone(), lines: v, error }
                }
                Err(e) => CmdAnswer { line: line.clone(), lines: Vec::new(), error: Some(format!("{e:?}")) },
            },
        };
        let _ = out.send(answer);
        SignalToUI::set_ui_signal();
    }
}
