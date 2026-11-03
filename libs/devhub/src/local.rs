//! This machine's JITs. A JIT announces itself with its discovery file (written when it starts,
//! removed when it exits: `makepad_remote::discovery`); the hub rescans the registry every
//! [`SCAN`] and follows each JIT's `observe` feed over its local Unix socket. `observe` is the
//! JIT's event ring WITHOUT counting as a watcher, so a hub never makes panics park or arms the
//! costly detectors: the program runs as if nobody were looking.
//!
//! A JIT that goes away is reported: its discovery file still there with the process gone means
//! it never ran its exit cleanup (a crash, with the tail of its session log as detail); the file
//! removed means it exited.

use crate::explorer::AiHub;
use crate::procinfo::{self, CpuMeter};
use crate::proto::{self, hub_record, Record, Sev};
use makepad_remote::discovery;
use makepad_remote::json_str;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
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
const AI_HUB_TTL: Duration = Duration::from_secs(5);
const AI_HUB_TIMEOUT: Duration = Duration::from_secs(5);
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
    /// Serialized by the scanner, outside the lock; `ps` never waits on the HTTP probe.
    ai_hub: Mutex<Option<String>>,
    stop: AtomicBool,
    hot_ready: AtomicBool,
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
            ai_hub: Mutex::new(ai_hub_endpoint().map(|_| AiHub { why: "health probe pending".into(), ..AiHub::default() }.to_json())),
            stop: AtomicBool::new(false),
            hot_ready: AtomicBool::new(false),
        });
        let l = local.clone();
        let _ = std::thread::Builder::new().name("devhub-scan".into()).spawn(move || l.scan_loop());
        let l = local.clone();
        let _ = std::thread::Builder::new().name("devhub-ai-probe".into()).spawn(move || l.ai_probe_loop());
        local
    }

    pub fn ready(&self) { self.hot_ready.store(true, Ordering::Release); }

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

    /// The AI hub's health, on its own thread: three fleet TLS requests never stall the JIT scan.
    fn ai_probe_loop(self: Arc<Self>) {
        let mut watch = AiHubWatch::default();
        while !self.stop.load(Ordering::Acquire) {
            let hub = ai_hub_endpoint().map(probe_ai_hub);
            let json = hub.as_ref().map(AiHub::to_json);
            *self.ai_hub.lock().unwrap() = json;
            if let Some(hub) = hub {
                if let Some((down, usual)) = watch.observe(hub.serving, proto::wall_ms()) {
                    self.emit(hub_record(&self.machine, "", 0, "ai_hub", Sev::Warn, format!("{} for {} s, past its usual start ({} s)", hub.line(), down / 1000, usual / 1000), String::new()));
                }
            } else {
                watch = AiHubWatch::default();
            }
            std::thread::sleep(AI_HUB_TTL);
        }
    }

    fn scan_loop(self: Arc<Self>) {
        let me = std::process::id();
        let mut tick = 0u64;
        let mut hot_replayed = false;
        // the registry is the temp dir on macOS: tens of thousands of entries on a busy Mac, so
        // it is listed only when it changed (a JIT starting or ending adds or removes its file)
        let (mut seen_mtime, mut last_read) = (None, Instant::now());
        while !self.stop.load(Ordering::Acquire) {
            if self.hot_ready.load(Ordering::Acquire) {
                if !hot_replayed {
                    match crate::hot::replay_failures(&|r| self.emit(r)) {
                        Ok(()) => hot_replayed = true,
                        Err(e) => eprintln!("devhub hot: inbox replay pending: {e}"),
                    }
                }
                crate::hot::collect(&self.machine, &|r| self.emit(r));
            }
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
        // its lease ended and the hub stopped it: no exit cleanup, and no crash either
        let stopped = crashed.then(|| crate::lease::exec::stopped_with(&jit.socket)).flatten();
        let rec = if let Some(lease) = stopped {
            let _ = std::fs::remove_file(registry_file(jit.pid));
            hub_record(&self.machine, &jit.app, jit.pid, "exit", Sev::Info, format!("stopped with its lease {lease} after {up} s"), String::new())
        } else if crashed {
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
        let ai_hub = self.ai_hub.lock().unwrap().clone().map_or(String::new(), |json| format!(",\"ai_hub\":{json}"));
        out.push(format!(
            "{{\"machine\":{},\"os\":\"{}\",\"cpus\":{},\"load1\":{:.2},\"mem_total_mb\":{},\"mem_avail_mb\":{},\"uptime_s\":{},\"jits\":{}{}{ai_hub}}}",
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

struct AiHubEndpoint {
    host: Option<String>,
    port: u16,
}

/// Explicit settings, then the WM's config, then the AI Hub app's config, without an AI
/// dependency. A disabled but configured hub still belongs in health reporting; a WM state
/// file identifies a hub started with defaults before any settings file was written.
fn ai_hub_endpoint() -> Option<AiHubEndpoint> {
    let explicit = ["MAKEPAD_AI_HUB_NODE_CONF", "MAKEPAD_WM_AI_HUB_CONF"].into_iter().find_map(|key| std::env::var_os(key).filter(|p| !p.is_empty()));
    let home = makepad_network::fleet::makepad_home();
    let wm = home.join("wm");
    let path = explicit.map(PathBuf::from).or_else(|| [wm.join("ai-hub.conf"), home.join("ai-hub/app.conf")].into_iter().find(|p| p.exists()));
    if let Some(path) = path {
        return Some(ai_hub_config(&std::fs::read_to_string(path).unwrap_or_default()));
    }
    wm.join("ai-hub.state").exists().then(|| ai_hub_config(""))
}

/// Both settings files use `key = value # comment`; invalid ports leave the current value.
fn ai_hub_config(text: &str) -> AiHubEndpoint {
    let mut endpoint = AiHubEndpoint { host: None, port: 8123 };
    for line in text.lines() {
        let Some((key, value)) = line.split('#').next().unwrap_or("").split_once('=') else { continue };
        let value = value.trim();
        match key.trim() {
            "host" => endpoint.host = (!value.is_empty()).then(|| value.to_string()),
            "port" => {
                if let Ok(port) = value.parse::<u16>() {
                    endpoint.port = port;
                }
            }
            _ => {}
        }
    }
    endpoint
}

fn ai_hub_remaining(deadline: Instant) -> io::Result<Duration> {
    deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "health probe timed out"))
}

/// HTTP over either stream, with one deadline shared by all three endpoints. Reapplying the
/// remaining timeout to each write/read prevents a slow trickle from extending the exchange.
fn ai_hub_get<S: Read + Write>(
    mut stream: S, request: &str, deadline: Instant,
    read_timeout: impl Fn(&S, Option<Duration>) -> io::Result<()>,
    write_timeout: impl Fn(&S, Option<Duration>) -> io::Result<()>,
) -> io::Result<Value> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid hub HTTP response");
    let mut pending = request.as_bytes();
    while !pending.is_empty() {
        write_timeout(&stream, Some(ai_hub_remaining(deadline)?))?;
        let n = stream.write(pending)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "hub connection closed"));
        }
        pending = &pending[n..];
    }
    let mut bytes = Vec::new();
    let mut body = None;
    let mut length = None;
    loop {
        read_timeout(&stream, Some(ai_hub_remaining(deadline)?))?;
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if body.is_none() {
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())?;
                if headers.lines().next().and_then(|s| s.split_whitespace().nth(1)) != Some("200") {
                    return Err(invalid());
                }
                for line in headers.lines().skip(1) {
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = Some(value.trim().parse::<usize>().map_err(|_| invalid())?);
                        }
                    }
                }
                body = Some(end + 4);
            }
        }
        if let (Some(start), Some(len)) = (body, length) {
            if bytes.len() - start >= len {
                break;
            }
        }
    }
    let start = body.ok_or_else(invalid)?;
    let payload = &bytes[start..];
    if length.is_some_and(|len| len != payload.len()) {
        return Err(invalid());
    }
    makepad_strict_json::parse(payload).map_err(|_| invalid())
}

/// Whether this node's AI hub answers its /health now (None: no AI hub is set up here).
pub fn ai_hub_answers() -> Option<bool> {
    ai_hub_endpoint().map(|e| probe_ai_hub(e).running == Some(true))
}

/// Reach the TLS front as a fleet peer. Only a refused front connection permits plain
/// loopback HTTP (networking off); only two refused connections mean the hub is stopped.
fn probe_ai_hub(endpoint: AiHubEndpoint) -> AiHub {
    use makepad_network::fleet::id::connect;
    use makepad_network::SocketStream;

    let deadline = Instant::now() + AI_HUB_TIMEOUT;
    let fleet_failed = |e: io::Error| AiHub { why: format!("fleet probe failed: {}", proto::clean(&e.to_string(), 300)), ..AiHub::default() };
    let host = match endpoint.host.map(Ok).unwrap_or_else(|| {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        // UDP connect sends nothing; TEST-NET-1 selects the primary outgoing interface.
        socket.connect("192.0.2.1:9")?;
        socket.local_addr().map(|addr| addr.ip().to_string())
    }) {
        Ok(host) => host,
        Err(e) => return fleet_failed(e),
    };
    let port = endpoint.port;
    let authority = if host.contains(':') && !host.starts_with('[') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
    let fleet_get = |path: &str, stream: SocketStream, authorization: String| {
        let request = format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nAuthorization: {authorization}\r\nConnection: close\r\n\r\n");
        ai_hub_get(stream, &request, deadline, SocketStream::set_read_timeout, SocketStream::set_write_timeout)
    };
    let loopback_get = |path: &str, stream: TcpStream| {
        let request = format!("GET {path} HTTP/1.0\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
        ai_hub_get(stream, &request, deadline, TcpStream::set_read_timeout, TcpStream::set_write_timeout)
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    // The fleet connector owns its TCP/TLS timeouts; never renew our HTTP deadline after it.
    let (loopback, health) = match ai_hub_remaining(deadline).and_then(|_| connect(&host, &port.to_string())) {
        Ok((stream, _, authorization)) => (false, fleet_get("/health", stream, authorization)),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
            match ai_hub_remaining(deadline).and_then(|remaining| TcpStream::connect_timeout(&addr, remaining)) {
                Ok(stream) => (true, loopback_get("/health", stream)),
                Err(e) => {
                    let stopped = e.kind() == io::ErrorKind::ConnectionRefused;
                    return AiHub {
                        running: stopped.then_some(false),
                        serving: stopped.then_some(false),
                        why: if stopped { "hub not running" } else { "health probe failed (status unknown)" }.into(),
                        ..AiHub::default()
                    };
                }
            }
        }
        Err(e) => return fleet_failed(e),
    };
    let health = match health {
        Ok(v) if matches!(v, Value::Obj(_)) => v,
        Ok(_) => return AiHub { why: "health response unknown".into(), ..AiHub::default() },
        Err(e) if !loopback => return fleet_failed(e),
        Err(_) => return AiHub { why: "health probe failed (status unknown)".into(), ..AiHub::default() },
    };
    let get = |path| -> io::Result<Value> {
        let remaining = ai_hub_remaining(deadline)?;
        if loopback {
            loopback_get(path, TcpStream::connect_timeout(&addr, remaining)?)
        } else {
            let (stream, _, authorization) = connect(&host, &port.to_string())?;
            fleet_get(path, stream, authorization)
        }
    };
    let fleet = get("/fleet").ok();
    let models = get("/models").ok();
    ai_hub_status(&health, models.as_ref(), fleet.as_ref())
}

/// Pure projection of the HTTP API; absent additive fields are never guessed from config.
pub(crate) fn ai_hub_status(health: &Value, models: Option<&Value>, fleet: Option<&Value>) -> AiHub {
    let flag = |key| health.get(key).and_then(Value::as_bool).or_else(|| fleet.and_then(|v| v.get(key)).and_then(Value::as_bool));
    let serving_fleet = flag("serving_fleet");
    let lan = flag("lan_listening");
    let serving = match (serving_fleet, lan) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    };
    let entries = models.and_then(Value::as_arr);
    let models = entries.and_then(|a| a.iter().map(|m| {
        Some(format!("{} {}", m.get("id")?.as_str()?, m.get("state").and_then(Value::as_str).unwrap_or("unknown")))
    }).collect());
    let peers = fleet.and_then(|v| v.get("peers")).and_then(Value::as_arr);
    let peers_ok = peers.and_then(|peers| peers.iter().map(|p| p.get("answers").and_then(Value::as_bool).map(u64::from)).collect::<Option<Vec<u64>>>().map(|ok| ok.iter().sum::<u64>()));
    let mut reasons = Vec::new();
    if lan == Some(false) {
        reasons.push(if flag("networked_ai_hub") == Some(false) {
            "listening on loopback only (networking off on this node)".to_string()
        } else {
            "listening on loopback only".to_string()
        });
    } else if serving_fleet == Some(false) {
        let role = health.get("role").and_then(Value::as_str).unwrap_or("unknown");
        reasons.push(format!("not serving peers (role: {role})"));
    } else if serving.is_none() {
        reasons.push("fleet serving status unknown".to_string());
    }
    if entries.is_some_and(|a| a.iter().all(|m| m.get("state").and_then(Value::as_str) == Some("absent"))) {
        reasons.push("no models installed".to_string());
    }
    AiHub {
        running: Some(true), serving, models,
        peers: peers.map(|p| p.len() as u64), peers_ok,
        why: reasons.join("; "),
    }
}

/// Unknown probes neither invent a transition nor rearm a warning; recovery does.
/// The AI hub's service as the probe sees it. A loss of service warns once, and only when it
/// outlasts the hub's usual start: a planned restart (a roll's, a WM replacing it) is back within
/// it and says nothing.
#[derive(Default)]
pub(crate) struct AiHubWatch {
    serving: Option<bool>,
    down_since: Option<u64>,
    warned: bool,
    /// the hub's recent starts here (down until serving again), ms
    starts: std::collections::VecDeque<u64>,
}

/// The shortest usual start: a build and a CUDA kernel build before its models load.
const AI_HUB_START_MS: u64 = 3 * 60 * 1000;
const AI_HUB_STARTS: usize = 8;

impl AiHubWatch {
    /// What the probe saw at `now_ms`: (down for, its usual start) once a loss outlasts that.
    pub(crate) fn observe(&mut self, serving: Option<bool>, now_ms: u64) -> Option<(u64, u64)> {
        let mut warn = None;
        match serving {
            Some(true) => {
                if let Some(since) = self.down_since.take() {
                    // an outage that warned was no start
                    if !std::mem::take(&mut self.warned) {
                        if self.starts.len() == AI_HUB_STARTS { self.starts.pop_front(); }
                        self.starts.push_back(now_ms.saturating_sub(since));
                    }
                }
            }
            Some(false) if self.serving == Some(true) || self.down_since.is_some() => {
                let down = now_ms.saturating_sub(*self.down_since.get_or_insert(now_ms));
                let usual = self.usual_start();
                if !self.warned && down > usual {
                    self.warned = true;
                    warn = Some((down, usual));
                }
            }
            _ => {}
        }
        if serving.is_some() { self.serving = serving; }
        warn
    }

    /// Half again its longest recent start, never under [`AI_HUB_START_MS`].
    fn usual_start(&self) -> u64 {
        self.starts.iter().max().map_or(0, |m| m * 3 / 2).max(AI_HUB_START_MS)
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

#[cfg(test)]
mod tests {
    use super::ai_hub_config;

    #[test]
    fn ai_hub_config_reads_host_and_app_port() {
        let endpoint = ai_hub_config("# port = 9000\n enabled = off\n host = hub.local  # fleet front\n port = 8124 # app port\n");
        assert_eq!(endpoint.host.as_deref(), Some("hub.local"));
        assert_eq!(endpoint.port, 8124);
    }
}
