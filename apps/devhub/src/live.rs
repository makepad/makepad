//! The live sampler behind the task-manager view: while a devhub window shows it, one thread asks
//! each watched JIT `live` (inspect's instant look: build counters, presented frames, the UI
//! frames since the last look) about 20 times a second, reads the OS for each JIT's CPU and the
//! machine's CPU / RAM / GPU, and keeps the last minute as rings the view draws. When no window
//! shows it the thread parks: no timers, no wakes, no polling.

use makepad_devhub::local::Local;
use makepad_devhub::procinfo::{self, CpuMeter};
use makepad_remote::link::{Client, Target as LinkTarget};
use makepad_strict_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the graphs show, s.
pub const WINDOW_S: f64 = 60.0;
const POLL: Duration = Duration::from_millis(50);
/// compile rates are binned this long (a 20 Hz look at a counter is too coarse for a rate)
const BIN_S: f64 = 0.25;
const PROC_S: f64 = 0.5;
const GPU_S: f64 = 2.0;
/// the JIT list of a local machine is reread this often
const LIST_S: f64 = 1.0;

/// One UI frame of a JIT: when it ended (board seconds), the time since the frame before it and
/// what it worked (ms).
#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pub at: f64,
    pub interval_ms: f32,
    pub work_ms: f32,
}

/// A JIT as the view shows it.
#[derive(Clone, Debug, Default)]
pub struct JitLive {
    pub pid: u32,
    pub app: String,
    /// wall ms it started (0: not known)
    pub started_ms: u64,
    pub frames: VecDeque<Frame>,
    /// (at, first compiles /s, O2 + O3 builds /s)
    pub compiles: VecDeque<(f64, f32, f32)>,
    /// (at, CPU % of one core)
    pub cpu: VecDeque<(f64, f32)>,
    pub cpu_now: Option<f32>,
    pub rss_mb: u64,
    pub threads: u32,
    /// presented frames per second over the last second
    pub fps: Option<f32>,
    /// builds running or queued now
    pub queued: u64,
    /// so far: first compiles (lazy + ahead), O2, O3, warm-cache links
    pub first: u64,
    pub o2: u64,
    pub o3: u64,
    pub linked: u64,
    /// why it gives no live counters (an older JIT, a link down)
    pub err: Option<String>,
    cursor: Option<u64>,
    last_end_ms: Option<f64>,
    bin: Option<(f64, u64, u64)>,
    presented: VecDeque<(f64, u64)>,
}

impl JitLive {
    /// `busy` while it builds (anything finished in the last second or queued), else `idle`.
    pub fn busy(&self, now: f64) -> bool {
        self.queued > 0 || self.compiles.iter().rev().take_while(|c| now - c.0 <= 1.0).any(|c| c.1 + c.2 > 0.0)
    }

    pub fn warm(&self) -> bool {
        self.linked > 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct MachineLive {
    /// (at, % of all cores)
    pub cpu: VecDeque<(f64, f32)>,
    /// (at, RAM in use %)
    pub mem: VecDeque<(f64, f32)>,
    /// (at, first GPU's use %)
    pub gpu: VecDeque<(f64, f32)>,
    pub cpus: u32,
    pub load1: f64,
    pub mem_total_mb: u64,
    pub mem_avail_mb: u64,
    /// (name, use %, VRAM used GB, VRAM GB)
    pub gpus: Vec<(String, f32, f64, f64)>,
}

/// What the sampler shows: board time is seconds since the sampler started.
#[derive(Clone, Debug, Default)]
pub struct Board {
    /// when board time 0 was (the view's clock between samples)
    pub t0: Option<Instant>,
    pub now: f64,
    pub jits: Vec<JitLive>,
    pub machine: MachineLive,
    /// the machine's numbers are this machine's (else only the JITs')
    pub local_machine: bool,
}

/// Which JITs to watch.
#[derive(Clone)]
pub enum Watch {
    None,
    /// every JIT of this machine (a node's view)
    Local(Arc<Local>),
    /// one JIT anywhere (the master's selection): (machine, pid, app, is this machine)
    One(String, u32, String, bool),
}

pub struct Sampler {
    on: Arc<AtomicBool>,
    watch: Arc<Mutex<Watch>>,
    pub board: Arc<Mutex<Board>>,
    thread: Option<std::thread::Thread>,
}

fn num(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Int(i)) => *i as f64,
        Some(Value::F64(f)) => *f,
        _ => 0.0,
    }
}

fn trim<T>(q: &mut VecDeque<T>, at: impl Fn(&T) -> f64, now: f64) {
    while q.front().is_some_and(|x| now - at(x) > WINDOW_S + 2.0) {
        q.pop_front();
    }
}

/// How one JIT is asked.
enum Link {
    Direct(Client),
    /// through the master's router: `machine:pid`
    Routed(String),
}

impl Link {
    fn ask(&self, line: &str) -> Result<Vec<String>, String> {
        match self {
            Link::Direct(c) => c.cmd(line).map(|l| l.flatten().filter(|l| !l.starts_with("{\"measured\"")).collect()).map_err(|e| e.to_string()),
            Link::Routed(target) => {
                let c = Client::new(LinkTarget::Unix(makepad_devhub::proto::control_socket()), "devhub-live");
                let mut out = Vec::new();
                makepad_remote::hub::ask(&c, target, "cmd", "", line, &mut |l| {
                    if !l.starts_with("{\"measured\"") {
                        out.push(l.to_string());
                    }
                    true
                })?;
                Ok(out)
            }
        }
    }
}

struct Run {
    t0: Instant,
    links: HashMap<u32, Link>,
    meter: CpuMeter,
    last_proc: f64,
    last_gpu: f64,
    last_list: f64,
    stat: Option<(u64, u64)>,
}

impl Sampler {
    pub fn start() -> Sampler {
        let on = Arc::new(AtomicBool::new(false));
        let watch = Arc::new(Mutex::new(Watch::None));
        let board = Arc::new(Mutex::new(Board::default()));
        let (o, w, b) = (on.clone(), watch.clone(), board.clone());
        let thread = std::thread::Builder::new()
            .name("devhub-live".into())
            .spawn(move || {
                let mut run = Run { t0: Instant::now(), links: HashMap::new(), meter: CpuMeter::default(), last_proc: f64::MIN, last_gpu: f64::MIN, last_list: f64::MIN, stat: None };
                loop {
                    if !o.load(Ordering::Acquire) {
                        // closed: sleep until a window shows the view again
                        std::thread::park();
                        continue;
                    }
                    let tick = Instant::now();
                    let watch = w.lock().unwrap().clone();
                    run.tick(&watch, &b);
                    if let Some(rest) = POLL.checked_sub(tick.elapsed()) {
                        std::thread::sleep(rest);
                    }
                }
            })
            .ok()
            .map(|h| h.thread().clone());
        Sampler { on, watch, board, thread }
    }

    /// The view shows (or no longer shows) in a window.
    pub fn set_visible(&self, visible: bool) {
        if self.on.swap(visible, Ordering::AcqRel) != visible && visible {
            if let Some(t) = &self.thread {
                t.unpark();
            }
        }
    }

    pub fn set_watch(&self, watch: Watch) {
        let same = matches!((&*self.watch.lock().unwrap(), &watch), (Watch::One(m, p, ..), Watch::One(m2, p2, ..)) if m == m2 && p == p2);
        if !same {
            *self.watch.lock().unwrap() = watch;
            self.board.lock().unwrap().jits.clear();
        }
    }
}

impl Run {
    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn tick(&mut self, watch: &Watch, board: &Mutex<Board>) {
        let now = self.now();
        // which JITs: a local machine's list is reread every second
        let wanted: Option<Vec<(u32, String, u64, Link)>> = match watch {
            Watch::None => Some(Vec::new()),
            Watch::Local(local) if now - self.last_list >= LIST_S => {
                self.last_list = now;
                Some(
                    local
                        .jits()
                        .into_iter()
                        .filter(|j| j.run.is_some())
                        .map(|j| {
                            let c = Client::new(LinkTarget::Unix(j.socket.clone().into()), "devhub-live");
                            c.set_timeout(Some(Duration::from_millis(500)));
                            (j.pid, j.app.clone(), j.started_ms, Link::Direct(c))
                        })
                        .collect(),
                )
            }
            Watch::One(machine, pid, app, local) if !self.links.contains_key(pid) => {
                let link = if *local {
                    match Client::connect(&pid.to_string()) {
                        Ok(c) => {
                            c.set_timeout(Some(Duration::from_millis(500)));
                            Link::Direct(c)
                        }
                        Err(_) => Link::Routed(format!("{machine}:{pid}")),
                    }
                } else {
                    Link::Routed(format!("{machine}:{pid}"))
                };
                Some(vec![(*pid, app.clone(), 0, link)])
            }
            _ => None,
        };
        if let Some(wanted) = wanted {
            let mut b = board.lock().unwrap();
            b.jits.retain(|j| wanted.iter().any(|w| w.0 == j.pid));
            self.links.retain(|pid, _| wanted.iter().any(|w| w.0 == *pid));
            for (pid, app, started_ms, link) in wanted {
                if !b.jits.iter().any(|j| j.pid == pid) {
                    b.jits.push(JitLive { pid, app, started_ms, ..JitLive::default() });
                }
                self.links.entry(pid).or_insert(link);
            }
            b.local_machine = matches!(watch, Watch::Local(_)) || matches!(watch, Watch::One(.., true));
        }
        // the JITs' counters and frames (outside the board lock: an answer can take a while)
        let asks: Vec<(u32, Option<u64>)> = board.lock().unwrap().jits.iter().map(|j| (j.pid, j.cursor)).collect();
        for (pid, cursor) in asks {
            let Some(link) = self.links.get(&pid) else { continue };
            let line = match cursor {
                Some(c) => format!("live after={c}"),
                None => "live".to_string(),
            };
            let answer = link.ask(&line);
            let at = self.now();
            let mut b = board.lock().unwrap();
            let Some(j) = b.jits.iter_mut().find(|j| j.pid == pid) else { continue };
            match answer {
                Ok(lines) => match lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok().filter(|v| v.get("now_ms").is_some())) {
                    Some(v) => {
                        j.err = None;
                        take(j, &v, at);
                    }
                    None => {
                        let why = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("err")?.as_str().map(str::to_string));
                        j.err = Some(match why {
                            Some(e) if e.contains("unknown") => "this JIT has no `live` answer yet (an older makepad run): restart it".into(),
                            Some(e) => e,
                            None => "no answer".into(),
                        });
                    }
                },
                // a routed link is made per ask; a direct one reconnects by itself
                Err(e) => j.err = Some(e),
            }
        }
        // the OS's numbers: each JIT's CPU and memory, the machine's CPU / RAM / GPU
        let now = self.now();
        if now - self.last_proc >= PROC_S {
            self.last_proc = now;
            let local = board.lock().unwrap().local_machine;
            let pids: Vec<u32> = board.lock().unwrap().jits.iter().map(|j| j.pid).collect();
            let procs: Vec<(u32, Option<(procinfo::Proc, Option<f64>)>)> = if local { pids.iter().map(|p| (*p, self.meter.sample(*p))).collect() } else { Vec::new() };
            self.meter.forget(&|p| pids.contains(&p));
            let m = procinfo::machine();
            let stat = proc_stat();
            let cpu = match (self.stat, stat) {
                (Some(a), Some(b)) if b.1 > a.1 => Some(makepad_devhub::top::cpu_pct(a, b) as f32),
                _ => None,
            };
            self.stat = stat.or(self.stat);
            let gpus = if local && now - self.last_gpu >= GPU_S {
                self.last_gpu = now;
                Some(makepad_devhub::top::gpu_use())
            } else {
                None
            };
            let mut b = board.lock().unwrap();
            for (pid, p) in procs {
                if let (Some(j), Some((p, pct))) = (b.jits.iter_mut().find(|j| j.pid == pid), p) {
                    j.rss_mb = p.rss_bytes / (1024 * 1024);
                    j.threads = p.threads;
                    if let Some(pct) = pct {
                        j.cpu_now = Some(pct as f32);
                        j.cpu.push_back((now, pct as f32));
                    }
                    trim(&mut j.cpu, |c| c.0, now);
                }
            }
            if local {
                let ml = &mut b.machine;
                ml.cpus = m.cpus;
                ml.load1 = m.load1;
                ml.mem_total_mb = m.mem_total_mb;
                ml.mem_avail_mb = m.mem_avail_mb;
                if let Some(c) = cpu {
                    ml.cpu.push_back((now, c));
                }
                if m.mem_total_mb > 0 {
                    ml.mem.push_back((now, (m.mem_total_mb.saturating_sub(m.mem_avail_mb)) as f32 * 100.0 / m.mem_total_mb as f32));
                }
                if let Some(g) = gpus {
                    ml.gpus = g.iter().map(|g| (g.name.clone(), g.util_pct as f32, g.mem_used_gb, g.mem_total_gb)).collect();
                    if let Some(first) = g.first() {
                        ml.gpu.push_back((now, first.util_pct as f32));
                    }
                }
                trim(&mut ml.cpu, |c| c.0, now);
                trim(&mut ml.mem, |c| c.0, now);
                trim(&mut ml.gpu, |c| c.0, now);
            }
        }
        let mut b = board.lock().unwrap();
        b.now = self.now();
        b.t0 = Some(self.t0);
    }
}

/// One `live` answer into its JIT's rings (`at`: when it came, board seconds).
fn take(j: &mut JitLive, v: &Value, at: f64) {
    let now_ms = num(v, "now_ms");
    let first = (num(v, "lazy") + num(v, "ahead")) as u64;
    let (o2, o3) = (num(v, "o2") as u64, num(v, "o3") as u64);
    j.first = first;
    j.o2 = o2;
    j.o3 = o3;
    j.linked = num(v, "linked") as u64;
    j.queued = v.get("q").and_then(|q| q.as_arr()).map_or(0, |q| q.iter().map(|x| x.as_u64().unwrap_or(0)).sum());
    j.cursor = v.get("next").and_then(|n| n.as_u64());
    // compile rates per bin
    match j.bin {
        None => j.bin = Some((at, first, o2 + o3)),
        Some((t, f, o)) if at - t >= BIN_S => {
            let dt = (at - t) as f32;
            j.compiles.push_back((at, first.saturating_sub(f) as f32 / dt, (o2 + o3).saturating_sub(o) as f32 / dt));
            j.bin = Some((at, first, o2 + o3));
        }
        _ => {}
    }
    trim(&mut j.compiles, |c| c.0, at);
    // presented frames per second over the last second
    let presented = num(v, "frames") as u64;
    j.presented.push_back((at, presented));
    while j.presented.len() > 2 && at - j.presented[1].0 >= 1.0 {
        j.presented.pop_front();
    }
    j.fps = j.presented.front().filter(|p| at - p.0 >= 0.5).map(|p| (presented.saturating_sub(p.1) as f64 / (at - p.0)) as f32);
    // the UI frames since the last look
    if let Some(ft) = v.get("ft").and_then(|f| f.as_arr()) {
        for f in ft {
            let Some(pair) = f.as_arr() else { continue };
            let n = |i: usize| match pair.get(i) {
                Some(Value::Int(x)) => *x as f64,
                Some(Value::F64(x)) => *x,
                _ => 0.0,
            };
            let (ago, work) = (n(0), n(1));
            let end_ms = now_ms - ago;
            if let Some(prev) = j.last_end_ms {
                let interval = (end_ms - prev).max(0.0);
                j.frames.push_back(Frame { at: at - ago / 1000.0, interval_ms: interval as f32, work_ms: work as f32 });
            }
            j.last_end_ms = Some(end_ms);
        }
    }
    trim(&mut j.frames, |f| f.at, at);
}

/// (busy, total) jiffies of all cores (Linux; elsewhere none).
fn proc_stat() -> Option<(u64, u64)> {
    if cfg!(any(target_os = "linux", target_os = "android")) {
        makepad_devhub::top::parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
    } else {
        None
    }
}
