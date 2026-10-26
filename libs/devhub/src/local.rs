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
const CMD_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a `ps` keeps a JIT's state answer.
const STATE_TTL: Duration = Duration::from_secs(5);
const LOG_TAIL: usize = 4 << 10;

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

struct State {
    jits: BTreeMap<u32, Jit>,
    /// pid -> (when, first line of `jit`)
    jit_state: BTreeMap<u32, (Instant, String)>,
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
            state: Mutex::new(State { jits: BTreeMap::new(), jit_state: BTreeMap::new(), cpu: CpuMeter::default(), crashed: Default::default() }),
            stop: AtomicBool::new(false),
        });
        let l = local.clone();
        let _ = std::thread::Builder::new().name("devhub-scan".into()).spawn(move || l.scan_loop());
        local
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
            for app in discovery::apps() {
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
                        (self.publish)(r);
                    }
                }
            }
            let mut feed = jit.client.feed("observe");
            loop {
                if self.stop.load(Ordering::Acquire) {
                    return;
                }
                match feed.next(Duration::from_secs(1), 3) {
                    Ok(Some((_, line))) => {
                        let line = String::from_utf8_lossy(&line);
                        if let Some(r) = proto::from_jit_event(&self.machine, &jit.app, jit.pid, &line, self.hitch_ms) {
                            (self.publish)(r);
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
            "{{\"machine\":{},\"os\":\"{}\",\"cpus\":{},\"load1\":{:.2},\"mem_total_mb\":{},\"mem_avail_mb\":{},\"uptime_s\":{},\"jits\":{}}}",
            json_str(&self.machine),
            m.os,
            m.cpus,
            m.load1,
            m.mem_total_mb,
            m.mem_avail_mb,
            m.uptime_s,
            jits.len()
        ));
        for j in jits {
            let p = procinfo::proc(j.pid).unwrap_or_default();
            let pct = self.state.lock().unwrap().cpu.rate(j.pid);
            let state = if j.run.is_some() { self.jit_state(&j) } else { String::new() };
            out.push(format!(
                "{{\"pid\":{},\"app\":{},\"up_s\":{},\"cpu_pct\":{},\"rss_mb\":{},\"threads\":{},\"jit\":{},\"observe\":{},\"run\":{},\"state\":{}}}",
                j.pid,
                json_str(&j.app),
                proto::wall_ms().saturating_sub(j.started_ms) / 1000,
                pct.map_or("null".to_string(), |p| format!("{p:.1}")),
                p.rss_bytes >> 20,
                p.threads,
                j.run.is_some(),
                j.observe,
                json_str(j.run.as_deref().unwrap_or("")),
                json_str(&state)
            ));
        }
        let mut st = self.state.lock().unwrap();
        let live: Vec<u32> = st.jits.keys().copied().collect();
        st.cpu.forget(&|pid| live.contains(&pid));
    }

    /// The first line of the JIT's `jit` answer (tier queues, busy / idle), cached for a few
    /// seconds: a JIT is asked only when someone looks.
    fn jit_state(&self, j: &Jit) -> String {
        if let Some((at, s)) = self.state.lock().unwrap().jit_state.get(&j.pid) {
            if at.elapsed() < STATE_TTL {
                return s.clone();
            }
        }
        // the answer's first line says what was measured; the verdict line follows it
        let s = j.client.cmd("jit").ok().and_then(|mut l| l.find(|l| l.as_ref().map_or(true, |l| !l.starts_with("{\"measured\""))).and_then(|l| l.ok())).map(|l| proto::clean(&l, 400)).unwrap_or_default();
        self.state.lock().unwrap().jit_state.insert(j.pid, (Instant::now(), s.clone()));
        s
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

fn log_tail(path: &str) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL as u64)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
