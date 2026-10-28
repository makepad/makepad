//! This machine's JITs. A JIT announces itself with its discovery file (written when it starts,
//! removed when it exits: `makepad_remote::discovery`); the hub rescans the registry every
//! [`SCAN`] and follows each JIT's `observe` feed over its local Unix socket. `observe` is the
//! JIT's event ring WITHOUT counting as a watcher, so a hub never makes panics park or arms the
//! costly detectors: the program runs as if nobody were looking.
//!
//! A JIT that goes away is reported: its discovery file still there with the process gone means
//! it never ran its exit cleanup (a crash, with the tail of its session log as detail); the file
//! removed means it exited.

use crate::procinfo::{self, CpuMeter};
use crate::proto::{self, hub_record, Record, Sev};
use makepad_remote::discovery;
use makepad_remote::json_str;
use makepad_remote::link::{Client, Target};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SCAN: Duration = Duration::from_millis(500);
/// The registry directory is read again at least this often even when its mtime says nothing
/// changed (an entry written in the same mtime tick as the last read).
const RESCAN: Duration = Duration::from_secs(10);
const CMD_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a `ps` keeps a JIT's state answer.
const STATE_TTL: Duration = Duration::from_secs(5);
const LOG_TAIL: usize = 4 << 10;
const RECENT: usize = 8;
/// A program's line that says its settings are wrong (printed once by the program to stderr,
/// which the session log keeps): a `config` record.
pub const CONFIG_ERROR: &str = "config error:";

pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;

#[derive(Clone)]
pub struct Jit {
    pub pid: u32,
    pub app: String,
    pub socket: String,
    pub started_ms: u64,
    /// a Relax run (its discovery file names the JIT's provenance)
    pub run: Option<String>,
    pub session_log: Option<String>,
    /// serves the passive event feed
    pub observe: bool,
    client: Client,
}

/// What a JIT is up to, from its own answers (`jit`, `code`, `panics`, `session`), asked only
/// when someone looks and kept for [`STATE_TTL`].
#[derive(Clone, Debug, Default)]
pub struct Detail {
    /// `idle` / `busy …`, what runs, the queues
    pub state: String,
    /// `session: compiled … first, O2 n, O3 n; warm links n; UI blocked …`
    pub session: String,
    pub fps: Option<f64>,
    /// started from stored code (warm links) or compiled everything (cold)
    pub warm: Option<bool>,
    pub code_mb: Option<f64>,
    /// panics parked, waiting for a decision
    pub panics: u32,
    /// edits that wait for a restart (`session pending`)
    pub restart_pending: u32,
}

struct State {
    jits: BTreeMap<u32, Jit>,
    /// pid -> (when, its detail)
    jit_state: BTreeMap<u32, (Instant, Detail)>,
    /// pid -> its recent records (at_ms, kind, line), newest last, at most [`RECENT`]
    recent: BTreeMap<u32, std::collections::VecDeque<(u64, String, String)>>,
    cpu: CpuMeter,
    /// JITs seen gone with their discovery file still there (a lister removes stale files, so
    /// the scan records it before it lists)
    crashed: std::collections::HashSet<u32>,
}

pub struct Local {
    pub machine: String,
    /// observe this process too: devhub embedded in a JIT host (a phone, a headset), where the
    /// programs run in the hub's own process
    include_self: bool,
    hitch_ms: f64,
    publish: Publish,
    state: Mutex<State>,
    stop: AtomicBool,
}

fn registry_file(pid: u32) -> PathBuf {
    discovery::registry_dir().join(format!("makepad-remote-{pid}.json"))
}

impl Local {
    /// Starts following this machine's JITs; every record goes to `publish`.
    pub fn start(machine: &str, hitch_ms: f64, include_self: bool, publish: Publish) -> Arc<Local> {
        let local = Arc::new(Local {
            machine: machine.to_string(),
            include_self,
            hitch_ms,
            publish,
            state: Mutex::new(State { jits: BTreeMap::new(), jit_state: BTreeMap::new(), recent: BTreeMap::new(), cpu: CpuMeter::default(), crashed: Default::default() }),
            stop: AtomicBool::new(false),
        });
        let l = local.clone();
        let _ = std::thread::Builder::new().name("devhub-scan".into()).spawn(move || l.scan_loop());
        local
    }

    /// Publishes a JIT's record and keeps it in that JIT's recent list (the explorer's events).
    fn emit(&self, r: Record) {
        if r.pid != 0 && r.kind != "exit" {
            let mut st = self.state.lock().unwrap();
            let q = st.recent.entry(r.pid).or_default();
            q.push_back((r.at_ms, r.kind.clone(), proto::clean(&r.line, 200)));
            while q.len() > RECENT {
                q.pop_front();
            }
        }
        (self.publish)(r);
    }

    /// New lines of a JIT's session log since `off` that report a config error.
    fn config_errors(&self, jit: &Jit, off: &mut u64) {
        use std::io::{Read, Seek, SeekFrom};
        let Some(path) = jit.session_log.as_deref().filter(|p| !p.is_empty()) else { return };
        let Ok(mut f) = std::fs::File::open(path) else { return };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len <= *off {
            *off = len.min(*off);
            return;
        }
        let from = (*off).max(len.saturating_sub(256 << 10));
        let _ = f.seek(SeekFrom::Start(from));
        let mut b = Vec::new();
        let _ = f.take(len - from).read_to_end(&mut b);
        *off = len;
        for line in String::from_utf8_lossy(&b).lines() {
            if let Some(i) = line.find(CONFIG_ERROR) {
                let what = proto::clean(line[i + CONFIG_ERROR.len()..].trim(), 300);
                self.emit(hub_record(&self.machine, &jit.app, jit.pid, "config", Sev::Warn, format!("config error: {what}"), String::new()));
            }
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn jits(&self) -> Vec<Jit> {
        self.state.lock().unwrap().jits.values().cloned().collect()
    }

    fn scan_loop(self: Arc<Self>) {
        let me = std::process::id();
        let mut tick = 0u64;
        // the registry is the temp dir on macOS: tens of thousands of entries on a busy Mac, so
        // it is listed only when it changed (a JIT starting or ending adds or removes its file)
        let (mut seen_mtime, mut last_read) = (None, Instant::now());
        while !self.stop.load(Ordering::Acquire) {
            tick += 1;
            if tick % 4 == 0 {
                // keep a CPU baseline per JIT so `ps` has a rate at once
                let mut st = self.state.lock().unwrap();
                let pids: Vec<u32> = st.jits.keys().copied().collect();
                for pid in pids {
                    let _ = st.cpu.sample(pid);
                }
            }
            {
                let mut st = self.state.lock().unwrap();
                let gone: Vec<u32> = st.jits.keys().copied().filter(|p| !procinfo::alive(*p) && registry_file(*p).exists()).collect();
                st.crashed.extend(gone);
            }
            let mtime = std::fs::metadata(discovery::registry_dir()).and_then(|m| m.modified()).ok();
            let changed = mtime.is_none() || mtime != seen_mtime || last_read.elapsed() >= RESCAN;
            let apps = if changed {
                seen_mtime = mtime;
                last_read = Instant::now();
                discovery::apps()
            } else {
                Vec::new()
            };
            for app in apps {
                if (app.pid == me && !self.include_self) || app.socket.starts_with("tcp:") {
                    continue;
                }
                let mut st = self.state.lock().unwrap();
                if st.jits.contains_key(&app.pid) {
                    continue;
                }
                let client = Client::new(Target::Unix(app.socket.clone().into()), "devhub");
                client.set_timeout(Some(CMD_TIMEOUT));
                let jit = Jit {
                    pid: app.pid,
                    app: proto::clean(&app.app, 64),
                    socket: app.socket.clone(),
                    started_ms: app.started_ms,
                    run: app.text("run"),
                    session_log: app.text("session_log"),
                    observe: app.serves("observe"),
                    client,
                };
                st.jits.insert(app.pid, jit.clone());
                drop(st);
                // the cache it runs on, for a later JIT cache clear (crate::jitcache)
                if let Some(dir) = app.text("run_dir") {
                    crate::jitcache::remember(&dir);
                }
                let l = self.clone();
                let _ = std::thread::Builder::new().name("devhub-observe".into()).spawn(move || l.follow(jit));
            }
            std::thread::sleep(SCAN);
        }
    }

    /// One JIT's life: its events while it runs, then how it ended.
    fn follow(self: Arc<Self>, jit: Jit) {
        if jit.observe {
            // requests posted before this hub attached (the feed starts now): the JIT keeps them
            if let Ok(lines) = jit.client.cmd("engine-requests after=0") {
                for l in lines.flatten() {
                    let Some(body) = l.strip_prefix('{').filter(|_| l.contains("\"summary\"")) else { continue };
                    let ev = format!("{{\"event\":\"engine_request\",{body}");
                    if let Some(r) = proto::from_jit_event(&self.machine, &jit.app, jit.pid, &ev, self.hitch_ms) {
                        self.emit(r);
                    }
                }
            }
            let mut feed = jit.client.feed("observe");
            let mut log_off = 0u64;
            let mut log_look = Instant::now();
            loop {
                if self.stop.load(Ordering::Acquire) {
                    return;
                }
                if log_look.elapsed() >= Duration::from_secs(3) {
                    log_look = Instant::now();
                    self.config_errors(&jit, &mut log_off);
                }
                match feed.next(Duration::from_secs(1), 3) {
                    Ok(Some((_, line))) => {
                        let line = String::from_utf8_lossy(&line);
                        if let Some(r) = proto::from_jit_event(&self.machine, &jit.app, jit.pid, &line, self.hitch_ms) {
                            self.emit(r);
                        }
                    }
                    Ok(None) => {
                        if !procinfo::alive(jit.pid) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        // no feed (an endpoint that is not a JIT, or an older build): watch the pid only
        while procinfo::alive(jit.pid) {
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        self.ended(&jit);
    }

    fn ended(&self, jit: &Jit) {
        let crashed = {
            let mut st = self.state.lock().unwrap();
            st.jits.remove(&jit.pid);
            st.jit_state.remove(&jit.pid);
            st.recent.remove(&jit.pid);
            st.crashed.remove(&jit.pid) || registry_file(jit.pid).exists()
        };
        let up = proto::wall_ms().saturating_sub(jit.started_ms) / 1000;
        let rec = if crashed {
            let _ = std::fs::remove_file(registry_file(jit.pid));
            let tail = jit.session_log.as_deref().map(log_tail).unwrap_or_default();
            let last = tail.lines().rev().find(|l| !l.trim().is_empty()).map(|l| proto::clean(l, 200)).unwrap_or_default();
            let detail = format!("{{\"socket\":{},\"session_log\":{},\"log_tail\":{}}}", json_str(&jit.socket), json_str(jit.session_log.as_deref().unwrap_or("")), json_str(&tail));
            hub_record(&self.machine, &jit.app, jit.pid, "crash", Sev::Err, format!("crash: ended without its exit cleanup after {up} s; last log: {last}"), detail)
        } else {
            hub_record(&self.machine, &jit.app, jit.pid, "exit", Sev::Info, format!("exit after {up} s"), String::new())
        };
        (self.publish)(rec);
    }

    /// `ps`: the machine line, then one line per JIT (JSON objects).
    pub fn ps(&self, out: &mut Vec<String>) {
        let m = procinfo::machine();
        let jits = self.jits();
        out.push(format!(
            "{{\"machine\":{},\"os\":\"{}\",\"cpus\":{},\"load1\":{:.2},\"mem_total_mb\":{},\"mem_avail_mb\":{},\"uptime_s\":{},\"jits\":{}{}}}",
            json_str(&self.machine),
            m.os,
            m.cpus,
            m.load1,
            m.mem_total_mb,
            m.mem_avail_mb,
            m.uptime_s,
            jits.len(),
            m.battery.map_or(String::new(), |(level, charging)| format!(",\"battery\":{level},\"charging\":{charging}"))
        ));
        // every JIT's detail at once (each `jit` answer samples 250 ms)
        let details: Vec<Detail> = std::thread::scope(|sc| {
            let hs: Vec<_> = jits.iter().map(|j| sc.spawn(move || if j.run.is_some() { self.detail(j) } else { Detail::default() })).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        let now = proto::wall_ms();
        for (j, d) in jits.iter().zip(details) {
            let p = procinfo::proc(j.pid).unwrap_or_default();
            let pct = self.state.lock().unwrap().cpu.rate(j.pid);
            let recent: Vec<String> = self
                .state
                .lock()
                .unwrap()
                .recent
                .get(&j.pid)
                .map(|q| q.iter().rev().map(|(at, k, l)| format!("{{\"kind\":{},\"line\":{},\"age_s\":{}}}", json_str(k), json_str(l), now.saturating_sub(*at) / 1000)).collect())
                .unwrap_or_default();
            let opt = |v: Option<f64>| v.map_or("null".to_string(), |x| format!("{x:.1}"));
            out.push(format!(
                "{{\"pid\":{},\"app\":{},\"up_s\":{},\"cpu_pct\":{},\"rss_mb\":{},\"threads\":{},\"jit\":{},\"observe\":{},\"run\":{},\"state\":{},\"session\":{},\"fps\":{},\"warm\":{},\"code_mb\":{},\"panics\":{},\"restart_pending\":{},\"exe_replaced\":{},\"recent\":[{}]}}",
                j.pid,
                json_str(&j.app),
                now.saturating_sub(j.started_ms) / 1000,
                pct.map_or("null".to_string(), |p| format!("{p:.1}")),
                p.rss_bytes >> 20,
                p.threads,
                j.run.is_some(),
                j.observe,
                json_str(j.run.as_deref().unwrap_or("")),
                json_str(&d.state),
                json_str(&d.session),
                opt(d.fps),
                d.warm.map_or("null".to_string(), |w| w.to_string()),
                opt(d.code_mb),
                d.panics,
                d.restart_pending,
                exe_replaced(j.pid),
                recent.join(",")
            ));
        }
        let mut st = self.state.lock().unwrap();
        let live: Vec<u32> = st.jits.keys().copied().collect();
        st.cpu.forget(&|pid| live.contains(&pid));
    }

    /// A JIT's detail, cached for a few seconds: a JIT is asked only when someone looks.
    fn detail(&self, j: &Jit) -> Detail {
        if let Some((at, d)) = self.state.lock().unwrap().jit_state.get(&j.pid) {
            if at.elapsed() < STATE_TTL {
                return d.clone();
            }
        }
        let ask = |cmd: &str| -> Vec<String> { j.client.cmd(cmd).map(|l| l.flatten().filter(|l| !l.starts_with("{\"measured\"")).collect()).unwrap_or_default() };
        let mut d = Detail::default();
        let jit = ask("jit");
        d.state = jit.first().map(|l| proto::clean(l, 400)).unwrap_or_default();
        if let Some(sess) = jit.iter().find(|l| l.starts_with("session:")) {
            d.session = proto::clean(sess, 300);
            d.warm = sess.split("warm links ").nth(1).and_then(|n| n.split(';').next()?.trim().parse::<u64>().ok()).map(|n| n > 0);
        }
        d.fps = jit.iter().find_map(|l| l.strip_prefix("frames: ")).and_then(|r| r.split_whitespace().next()).and_then(|f| f.parse().ok());
        d.code_mb = ask("code n=0").iter().find_map(|l| l.strip_prefix("code installed ")).and_then(|r| r.split_whitespace().next()).and_then(|m| m.parse().ok());
        d.panics = ask("panics").iter().filter(|l| l.starts_with('{') && !l.starts_with("{\"err\"")).count() as u32;
        d.restart_pending = ask("session").iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("restart_pending")?.as_u64()).unwrap_or(0) as u32;
        self.state.lock().unwrap().jit_state.insert(j.pid, (Instant::now(), d.clone()));
        d
    }

    /// Hands an answer to the JIT that posted engine request `id` (`<machine>.<pid>.<seq>`).
    pub fn reply(&self, id: &str, status: &str, message: &str) -> Result<String, String> {
        if !proto::STATUSES.contains(&status) {
            return Err(format!("status {status}: one of {}", proto::STATUSES.join(", ")));
        }
        if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
            return Err("not a request id".into());
        }
        let pid: u32 = id.rsplitn(3, '.').nth(1).and_then(|p| p.parse().ok()).ok_or_else(|| format!("{id}: not <machine>.<pid>.<seq>"))?;
        let jit = self.state.lock().unwrap().jits.get(&pid).cloned().ok_or_else(|| format!("no JIT with pid {pid} on {}", self.machine))?;
        let message: String = message.chars().filter(|c| *c != '\0').take(2048).collect();
        let line = format!("{{\"cmd\":\"engine-reply\",\"id\":{},\"status\":{},\"message\":{}}}", json_str(id), json_str(status), json_str(&message));
        let first = jit.client.cmd(&line).map_err(|e| format!("pid {pid} did not answer: {e}"))?.next().transpose().map_err(|e| format!("pid {pid} did not answer: {e}"))?.unwrap_or_default();
        if first.starts_with("{\"err\":") {
            return Err(proto::clean(&first, 400));
        }
        Ok(first)
    }
}

/// The program's binary was replaced on disk since it started (a rebuild it has not restarted
/// into).
fn exe_replaced(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|p| p.to_string_lossy().ends_with(" (deleted)"))
}

fn log_tail(path: &str) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL as u64)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
