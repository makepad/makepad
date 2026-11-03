//! The node side of the `kernel` domain (wire: `crate::kernel_wire`): compute programs
//! (`makepad_compute_program`), long-running Splash programs a fleet client opens on its stream
//! (`GET /kernel/stream`; frames: `makepad_compute_program::wire`), steered by its messages,
//! their outputs streamed back.
//!
//! A program is Splash, not Rust: its driver runs in its own VM isolate (domain `compute`: no
//! fs, net, host or cx) under counted limits, its kernels are Splash compute (total, bounded,
//! every access clamped to its buffer). On top of that the node:
//! - caps what a client asks for at its own ceilings (instructions, heap, kernel work, buffer
//!   bytes, inputs) and holds at most [`MAX_SESSIONS_PER_CLIENT`] sessions per credential and
//!   [`MAX_SESSIONS`] in all;
//! - runs a session at the class its client opened it with (`Class`: interactive, visible,
//!   background; the unified workpool's one OS priority table): its kernels on that class's
//!   pool, its driver and sender threads at that class too. A person steering a zoom gets the
//!   cores ahead of the node's background work; fan-out takes only what is left;
//! - sets its share of the box from the highest class open ([`Share`]): a thread's priority
//!   counts only inside its own process group, so against builds next to it (each in its own
//!   scope, weighted by its cores) the node competes with its scope's weight;
//! - sends at most `wire::IN_FLIGHT` unacked versions of an output; a newer one replaces the
//!   one waiting (a slow link skips versions, it never queues them);
//! - ends a session when its stream closes, and only its owner may steer or cancel it.

use crate::kernel_wire::{KernelInfoJson, KERNEL_API_VERSION};
use makepad_compute_program::codec::Codec;
use makepad_compute_program::wire;
use makepad_compute_program::{Backend, Class, Limits, Output, Session, SessionConfig, Updates, Value};
use makepad_script_compute_sched::sched::{Executor, ThreadExecutor};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Sessions one credential may hold.
pub const MAX_SESSIONS_PER_CLIENT: usize = 4;
/// Sessions on the node.
pub const MAX_SESSIONS: usize = 16;
/// Words all of one open's inputs may hold (1 GiB).
const MAX_INPUT_WORDS: usize = 1 << 28;

/// The most a program on this node may use, whatever the client asks.
fn node_ceiling() -> Limits {
    Limits {
        fuel: 1 << 40,
        heap_bytes: 256 << 20,
        stack_values: 1 << 16,
        call_frames: 256,
        work: 1 << 50,
        buffer_bytes: 4 << 30,
        read_words: 1 << 22,
        kernels: 256,
    }
}

fn capped(ask: Limits) -> Limits {
    let c = node_ceiling();
    Limits {
        fuel: ask.fuel.min(c.fuel),
        heap_bytes: ask.heap_bytes.min(c.heap_bytes),
        stack_values: ask.stack_values.min(c.stack_values),
        call_frames: ask.call_frames.min(c.call_frames),
        work: ask.work.min(c.work),
        buffer_bytes: ask.buffer_bytes.min(c.buffer_bytes),
        read_words: ask.read_words.min(c.read_words),
        kernels: ask.kernels.min(c.kernels),
    }
}

/// A client's stream: its frames go out through `tx`, never blocking the sender.
#[derive(Clone)]
pub struct Push {
    pub socket: u64,
    pub tx: std::sync::mpsc::Sender<Vec<u8>>,
}

pub struct ProgramService {
    inner: Arc<Inner>,
}

struct Inner {
    sessions: Mutex<HashMap<u64, Arc<Served>>>,
    next_id: Mutex<u64>,
    /// Open streams: socket -> (owner, its sender).
    streams: Mutex<HashMap<u64, (String, Push)>>,
    /// One pool per class, started when a session of it first opens.
    pools: [OnceLock<Arc<ThreadExecutor>>; 3],
    workers: usize,
    backend: Backend,
    /// None where the node runs in no scope it can weigh.
    share: Mutex<Option<Share>>,
}

/// The node's share of the box against the other work on it: on Linux, the `CPUWeight` of the
/// user-manager scope or service it runs in, which is what the scopes of builds next to it
/// compete with (100 a core of their cost). It follows the highest class of the sessions open:
/// interactive takes the box (the most a weight can be), visible weighs as a build of all the
/// cores, and anything else leaves the scope's own weight (a background program yields inside
/// the node, at its class's priority, and shares the node's own weight with builds).
struct Share {
    unit: String,
    /// The scope's weight before any session.
    base: u32,
    applied: u32,
}

impl Share {
    fn find() -> Option<Share> {
        #[cfg(target_os = "linux")]
        {
            let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
            let path = cgroup.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
            let unit = path.rsplit('/').next()?;
            if !path.contains("/user@") || !(unit.ends_with(".scope") || unit.ends_with(".service")) {
                return None;
            }
            let base = std::fs::read_to_string(format!("/sys/fs/cgroup{path}/cpu.weight")).ok()?.trim().parse().ok()?;
            Some(Share { unit: unit.to_string(), base, applied: base })
        }
        #[cfg(not(target_os = "linux"))]
        None
    }
}

impl Inner {
    fn pool(&self, class: Class) -> Arc<ThreadExecutor> {
        // Each worker takes its class once, at its start (a thread cannot raise its own class
        // back without a privilege, so pools do not trade threads between classes).
        const START: [fn(); 3] = [
            || {
                Class::Interactive.apply();
            },
            || {
                Class::Visible.apply();
            },
            || {
                Class::Background.apply();
            },
        ];
        let k = class.budget() as usize;
        self.pools[k].get_or_init(|| Arc::new(ThreadExecutor::with_thread_start(self.workers, START[k]))).clone()
    }

    /// The node's share from the sessions open now ([`Share`]).
    fn reshare(&self) {
        let mut held = self.share.lock().unwrap();
        let Some(share) = held.as_mut() else { return };
        let top = self.sessions.lock().unwrap().values().map(|s| s.class).min_by_key(|c| c.budget());
        let weight = match top {
            Some(Class::Interactive) => 10_000,
            Some(Class::Visible) => share.base.max(100 * (self.workers as u32 + 1)),
            _ => share.base,
        };
        if weight == share.applied {
            return;
        }
        let set = std::process::Command::new("systemctl")
            .args(["--user", "set-property", "--runtime", &share.unit, &format!("CPUWeight={weight}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .output();
        match set {
            Ok(out) if out.status.success() => share.applied = weight,
            failed => {
                let why = match failed {
                    Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    Err(e) => e.to_string(),
                };
                eprintln!("{}: programs: {} keeps its CPU weight {} (setting {weight}: {why})", crate::SERVICE_NAME, share.unit, share.applied);
                *held = None;
            }
        }
    }
}

/// A session and its link.
struct Served {
    owner: String,
    socket: u64,
    class: Class,
    session: Session,
    codec: Codec,
    flow: Mutex<Flow>,
}

/// Per output (name, state): versions sent and not acked, and the newest one waiting.
#[derive(Default)]
struct Flow {
    in_flight: Vec<((String, bool), Vec<u64>)>,
    waiting: Vec<Output>,
}

impl Flow {
    fn unacked(&mut self, key: &(String, bool)) -> &mut Vec<u64> {
        match self.in_flight.iter().position(|(k, _)| k == key) {
            Some(i) => &mut self.in_flight[i].1,
            None => {
                self.in_flight.push((key.clone(), Vec::new()));
                &mut self.in_flight.last_mut().unwrap().1
            }
        }
    }

    /// The outputs that may go now; the rest wait (the newest per name).
    fn admit(&mut self, outputs: Vec<Output>, all: bool) -> Vec<Output> {
        for o in outputs {
            match self.waiting.iter_mut().find(|w| w.name == o.name && w.state == o.state) {
                Some(w) => *w = o,
                None => self.waiting.push(o),
            }
        }
        let mut send = Vec::new();
        for o in std::mem::take(&mut self.waiting) {
            let key = (o.name.clone(), o.state);
            let unacked = self.unacked(&key);
            if all || unacked.len() < wire::IN_FLIGHT {
                unacked.push(o.version);
                send.push(o);
            } else {
                self.waiting.push(o);
            }
        }
        send
    }

    fn ack(&mut self, key: &(String, bool), version: u64) {
        self.unacked(key).retain(|v| *v > version);
    }
}

pub fn service() -> &'static ProgramService {
    static SERVICE: OnceLock<ProgramService> = OnceLock::new();
    SERVICE.get_or_init(|| {
        // The caller of a kernel works too: the pool is the cores less one.
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).saturating_sub(1).max(1);
        ProgramService {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                next_id: Mutex::new(0),
                streams: Mutex::new(HashMap::new()),
                pools: Default::default(),
                workers,
                backend: if cfg!(any(target_arch = "aarch64", target_arch = "x86_64")) { Backend::Native } else { Backend::Interp },
                share: Mutex::new(Share::find()),
            }),
        }
    })
}

impl ProgramService {
    /// Sessions open on the node.
    pub fn sessions(&self) -> usize {
        self.inner.sessions.lock().unwrap().len()
    }

    /// `GET /kernel`.
    pub fn info(&self) -> KernelInfoJson {
        KernelInfoJson {
            api: KERNEL_API_VERSION,
            cores: self.inner.workers as u32 + 1,
            arch: std::env::consts::ARCH.to_string(),
            backend: if self.inner.backend == Backend::Native { "native" } else { "interp" }.to_string(),
            programs: self.sessions() as u32,
        }
    }

    /// A stream opened by `owner`; its frames go out through `tx`.
    pub fn stream_open(&self, socket: u64, owner: &str, tx: std::sync::mpsc::Sender<Vec<u8>>) {
        self.inner.streams.lock().unwrap().insert(socket, (owner.to_string(), Push { socket, tx }));
    }

    /// Whether `socket` is one of this service's streams.
    pub fn is_stream(&self, socket: u64) -> bool {
        self.inner.streams.lock().unwrap().contains_key(&socket)
    }

    /// A frame from a stream.
    pub fn stream_frame(&self, socket: u64, data: &[u8]) {
        let Some((owner, push)) = self.inner.streams.lock().unwrap().get(&socket).cloned() else { return };
        self.frame(&owner, &push, data);
    }

    /// A program frame from `owner`'s stream (kinds `wire::OPEN` ..= `wire::CANCEL`).
    fn frame(&self, owner: &str, push: &Push, data: &[u8]) {
        if data.len() < 9 {
            return;
        }
        let word = u64::from_le_bytes(data[1..9].try_into().unwrap());
        let payload = &data[9..];
        match data[0] {
            wire::OPEN => {
                let (inner, owner, sender, payload) = (self.inner.clone(), owner.to_string(), push.clone(), payload.to_vec());
                // Inputs decode and the program starts off the routing thread.
                let started = std::thread::Builder::new().name("program-open".into()).spawn(move || {
                    let reply = open(&inner, &owner, &sender, &payload);
                    send(&sender, wire::OPENED, word, wire::encode_opened(reply));
                });
                if started.is_err() {
                    send(push, wire::OPENED, word, wire::encode_opened(Err((503, "the node cannot start a session now".into()))));
                }
            }
            wire::MSG => {
                let Some(served) = self.owned(owner, word) else { return };
                if let Ok((key, value, interrupt)) = wire::decode_msg(payload) {
                    if interrupt {
                        served.session.interrupt(&key, value);
                    } else {
                        served.session.send(&key, value);
                    }
                }
            }
            wire::ACK => {
                let Some(served) = self.owned(owner, word) else { return };
                if let Ok((name, state, version)) = wire::decode_ack(payload) {
                    served.flow.lock().unwrap().ack(&(name, state), version);
                    served.session.wake();
                }
            }
            wire::CANCEL => {
                if let Some(served) = self.owned(owner, word) {
                    served.session.cancel();
                }
            }
            _ => {}
        }
    }

    fn owned(&self, owner: &str, id: u64) -> Option<Arc<Served>> {
        self.inner.sessions.lock().unwrap().get(&id).filter(|s| s.owner == owner).cloned()
    }

    /// A stream closed: its sessions end.
    pub fn stream_closed(&self, socket: u64) {
        if self.inner.streams.lock().unwrap().remove(&socket).is_none() {
            return;
        }
        let ended: Vec<Arc<Served>> = self.inner.sessions.lock().unwrap().values().filter(|s| s.socket == socket).cloned().collect();
        for s in ended {
            s.session.cancel();
        }
    }
}

fn send(push: &Push, kind: u8, word: u64, payload: Vec<u8>) {
    let mut frame = Vec::with_capacity(9 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&word.to_le_bytes());
    frame.extend_from_slice(&payload);
    let _ = push.tx.send(frame);
}

fn open(inner: &Arc<Inner>, owner: &str, push: &Push, payload: &[u8]) -> Result<(u64, Value), (u16, String)> {
    let threads = inner.workers + 1;
    let open = wire::decode_open(payload, MAX_INPUT_WORDS, threads).map_err(|e| (400, e))?;
    let class = open.class;
    class.apply();
    let id = {
        let sessions = inner.sessions.lock().unwrap();
        if sessions.len() >= MAX_SESSIONS {
            return Err((503, format!("the node runs {MAX_SESSIONS} sessions")));
        }
        if sessions.values().filter(|s| s.owner == owner).count() >= MAX_SESSIONS_PER_CLIENT {
            return Err((429, format!("{MAX_SESSIONS_PER_CLIENT} sessions per client")));
        }
        let mut next = inner.next_id.lock().unwrap();
        *next += 1;
        *next
    };
    let config = SessionConfig {
        params: open.params,
        inputs: open.inputs,
        limits: capped(open.limits),
        executor: Some(inner.pool(class) as Arc<dyn Executor>),
        threads,
        backend: inner.backend,
        seed: open.seed,
        class,
    };
    let served = Arc::new(Served { owner: owner.to_string(), socket: push.socket, class, session: Session::open(&open.program, config), codec: open.codec, flow: Mutex::new(Flow::default()) });
    inner.sessions.lock().unwrap().insert(id, served.clone());
    inner.reshare();
    let (pump_inner, pump_push) = (inner.clone(), push.clone());
    let pumped = std::thread::Builder::new().name("program-send".into()).spawn(move || {
        class.apply();
        pump(&served, &pump_push, id, threads);
        pump_inner.sessions.lock().unwrap().remove(&id);
        pump_inner.reshare();
    });
    if pumped.is_err() {
        if let Some(s) = inner.sessions.lock().unwrap().remove(&id) {
            s.session.cancel();
        }
        inner.reshare();
        return Err((503, "the node cannot start a session now".into()));
    }
    let device = Value::object(&[
        ("kind", Value::String("cpu".into())),
        ("backend", Value::String(if inner.backend == Backend::Native { "native" } else { "interp" }.into())),
        ("threads", Value::Number(threads as f64)),
        ("class", Value::String(format!("{class:?}").to_lowercase())),
        // the node's CPU weight against the builds next to it (absent: no scope it can weigh)
        ("weight", inner.share.lock().unwrap().as_ref().map_or(Value::Nil, |s| Value::Number(s.applied as f64))),
    ]);
    Ok((id, device))
}

/// The session's updates to its stream until it ends.
fn pump(served: &Served, push: &Push, id: u64, threads: usize) {
    loop {
        let u = served.session.wait();
        let ending = u.end.is_some();
        let outputs = served.flow.lock().unwrap().admit(u.outputs, ending);
        let update = Updates { outputs, ..u };
        if !update.is_empty() {
            send(push, wire::UPDATE, id, wire::encode_update(&update, served.codec, threads));
        }
        if ending {
            return;
        }
    }
}
