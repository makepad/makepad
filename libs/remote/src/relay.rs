//! The relay: what runs on a remote box (`ssh <host> makepad remote-relay`, or `--dial` to a hub)
//! so the hub can reach the box's endpoints. It is an [`Endpoint`] serving, over the one link:
//!
//! - `list`: the box's endpoints from its discovery files, one JSON line each;
//! - `session`: `launch [cwd=<dir>] <app> [args...]` starts the configured launcher (hidden, muted)
//!   and reports its PID, `kill <pid>` ends a launched session, `relaunch <pid>` restarts its
//!   Relax domain in place (native programs restart as processes), `launched`
//!   lists them. Launches are recorded in a ledger on the host (see [`Config::ledger`]), so a
//!   relay that starts later (the hub exited, ssh was re-run) still knows the sessions and can
//!   kill or relaunch them;
//! - `fwd`: args `<pid|app> <service> [args]`: opens `service` on that local endpoint and bridges
//!   the two channels, so the hub's channel talks to the endpoint as if it were local.

use crate::cmd::json_str;
use crate::conn::{Channel, ChannelTx, Event, CLOSE_LINK_LOST, CLOSE_REFUSED};
use crate::endpoint::{Endpoint, Factory};
use crate::lines;
use crate::link::{Client, Error};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How the relay starts sessions.
#[derive(Clone)]
pub struct Config {
    /// the endpoint name the relay reports in its hello (the host name for a dial relay)
    pub name: String,
    /// the launcher's argv before the app, e.g. `["/home/arch/bin/makepad", "run"]`
    pub launch: Vec<String>,
    /// environment of launched sessions (hidden windows, muted audio)
    pub env: Vec<(String, String)>,
    /// the folder of the launched-session ledger, one `<pid>.json` per session
    /// (`~/.cache/makepad-remote/sessions`, see [`default_ledger`])
    pub ledger: PathBuf,
}

/// `~/.cache/makepad-remote/sessions`
pub fn default_ledger() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(".cache").join("makepad-remote").join("sessions")
}

#[derive(Clone)]
struct Launched {
    pid: u32,
    /// the process start time (clock ticks since boot on Linux, microseconds since the epoch on
    /// macOS): a pid with another start time is another process
    start: u64,
    app: String,
    args: String,
    cwd: String,
    env_keys: Vec<String>,
    log: PathBuf,
    /// set when the process has ended
    exit: Arc<Mutex<Option<String>>>,
    /// a child of this relay (reaped by it); sessions of an earlier relay are watched by pid + start
    own: bool,
}

impl Launched {
    /// None while the session runs, else how it ended.
    fn ended(&self, cfg: &Config) -> Option<String> {
        if let Some(e) = self.exit.lock().unwrap().clone() {
            return Some(e);
        }
        if self.own || proc_start(self.pid) == Some(self.start) {
            return None;
        }
        let why = "exited".to_string();
        *self.exit.lock().unwrap() = Some(why.clone());
        forget(cfg, self.pid);
        Some(why)
    }

    fn ledger_text(&self) -> String {
        format!(
            "{{\"pid\":{},\"start\":{},\"app\":{},\"args\":{},\"cwd\":{},\"env_keys\":{},\"log\":{}}}\n",
            self.pid,
            self.start,
            json_str(&self.app),
            json_str(&self.args),
            json_str(&self.cwd),
            json_str(&self.env_keys.join(",")),
            json_str(&self.log.display().to_string())
        )
    }
}

fn ledger_file(cfg: &Config, pid: u32) -> PathBuf {
    cfg.ledger.join(format!("{pid}.json"))
}

fn forget(cfg: &Config, pid: u32) {
    let _ = std::fs::remove_file(ledger_file(cfg, pid));
}

/// The start time of a running process; None when there is no such process (or it is a zombie).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn proc_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (comm) state ...`: comm may hold spaces and parens, so count from the last `)`
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut f = rest.split_whitespace();
    if f.next()? == "Z" {
        return None;
    }
    // field 22 of the line is the 20th after state (field 3)
    f.nth(18)?.parse().ok()
}

#[cfg(target_os = "macos")]
pub fn proc_start(pid: u32) -> Option<u64> {
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    const PROC_PIDTBSDINFO: i32 = 3;
    // struct proc_bsdinfo: 136 bytes; pbi_status at 4, pbi_start_tvsec at 120, pbi_start_tvusec at 128
    let mut buf = [0u8; 136];
    let n = unsafe { proc_pidinfo(pid as i32, PROC_PIDTBSDINFO, 0, buf.as_mut_ptr(), buf.len() as i32) };
    if n != buf.len() as i32 {
        return None;
    }
    let u32_at = |o: usize| u32::from_ne_bytes(buf[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_ne_bytes(buf[o..o + 8].try_into().unwrap());
    const SZOMB: u32 = 5;
    if u32_at(4) == SZOMB {
        return None;
    }
    Some(u64_at(120) * 1_000_000 + u64_at(128))
}

/// Other Unix targets (iOS): no start time, only whether the process exists (signal 0 probe;
/// the process list module is the server binary's, not this library's).
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
pub fn proc_start(pid: u32) -> Option<u64> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    (unsafe { kill(pid as i32, 0) } == 0).then_some(0)
}

struct State {
    cfg: Config,
    launched: Mutex<Vec<Launched>>,
}

/// The relay endpoint.
pub fn endpoint(cfg: Config) -> Endpoint {
    let ep = Endpoint::new(&cfg.name);
    let st = Arc::new(State { launched: Mutex::new(load_ledger(&cfg)), cfg });
    ep.register("list", 1, lines::service("list", Arc::new(|_, out| list(out))));
    let s = st.clone();
    ep.register("session", 1, lines::service("session", Arc::new(move |line, out| session(&s, line, out))));
    ep.register("fwd", 1, fwd());
    ep
}

fn list(out: &mut Vec<String>) {
    for a in crate::discovery::apps() {
        let caps: Vec<String> = a.caps.iter().map(|c| json_str(c)).collect();
        out.push(format!("{{\"pid\":{},\"app\":{},\"socket\":{},\"started_ms\":{},\"caps\":[{}]}}", a.pid, json_str(&a.app), json_str(&a.socket), a.started_ms, caps.join(",")));
    }
}

fn err(out: &mut Vec<String>, msg: &str) {
    out.push(format!("{{\"err\":{}}}", json_str(msg)));
}

fn session(st: &State, line: &str, out: &mut Vec<String>) {
    let mut words = line.split_whitespace();
    match words.next() {
        Some("launch") => launch(st, words.collect(), out),
        Some("kill") => match words.next().and_then(|p| p.parse::<u32>().ok()) {
            Some(pid) => kill(st, pid, out),
            None => err(out, "usage: kill <pid>"),
        },
        Some("relaunch") => match words.next().and_then(|p| p.parse::<u32>().ok()) {
            Some(pid) => relaunch(st, pid, out),
            None => err(out, "usage: relaunch <pid>"),
        },
        Some("launched") => {
            adopt(st);
            for l in st.launched.lock().unwrap().iter() {
                let state = l.ended(&st.cfg).unwrap_or_else(|| "running".into());
                out.push(format!("{{\"pid\":{},\"app\":{},\"state\":{},\"log\":{}}}", l.pid, json_str(&l.app), json_str(&state), json_str(&l.log.display().to_string())));
            }
        }
        _ => err(out, "session: launch [cwd=<dir>] <app> [args...] | kill <pid> | relaunch <pid> (restart its app domain, keeping the window) | launched"),
    }
}

/// Sessions in the ledger that are still the same process (pid + start time) and that are not in
/// `known`; stale entries are deleted, so a reused pid is never signalled.
fn scan(cfg: &Config, known: &[u32]) -> Vec<Launched> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(&cfg.ledger) else { return out };
    for e in dir.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let r = crate::cmd::Req::parse(&text);
        let num = |k: &str| r.get(k).and_then(|v| v.parse::<u64>().ok());
        let (Some(pid), Some(start)) = (num("pid"), num("start")) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let pid = pid as u32;
        if known.contains(&pid) {
            continue;
        }
        if proc_start(pid) != Some(start) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let text = |k: &str| r.get(k).unwrap_or("").to_string();
        out.push(Launched {
            pid,
            start,
            app: text("app"),
            args: text("args"),
            cwd: text("cwd"),
            env_keys: text("env_keys").split(',').filter(|k| !k.is_empty()).map(str::to_string).collect(),
            log: PathBuf::from(text("log")),
            exit: Arc::new(Mutex::new(None)),
            own: false,
        });
    }
    out.sort_by_key(|l| l.pid);
    out
}

fn load_ledger(cfg: &Config) -> Vec<Launched> {
    scan(cfg, &[])
}

/// Picks up sessions another relay launched since this one looked.
fn adopt(st: &State) {
    let mut l = st.launched.lock().unwrap();
    let known: Vec<u32> = l.iter().map(|l| l.pid).collect();
    l.extend(scan(&st.cfg, &known));
}

/// `[cwd=<dir>] <app> [args...]`
struct Spec {
    app: String,
    args: String,
    cwd: String,
}

fn parse_spec(mut words: Vec<&str>) -> Result<Spec, &'static str> {
    let mut cwd = String::new();
    if let Some(dir) = words.first().and_then(|w| w.strip_prefix("cwd=")) {
        cwd = dir.to_string();
        words.remove(0);
    }
    let Some((app, args)) = words.split_first() else { return Err("usage: launch [cwd=<dir>] <app> [args...]") };
    Ok(Spec { app: app.to_string(), args: args.join(" "), cwd })
}

#[cfg(unix)]
fn launch(st: &State, words: Vec<&str>, out: &mut Vec<String>) {
    match parse_spec(words) {
        Ok(spec) => match start(st, &spec) {
            Ok(rec) => out.push(format!("{{\"launched\":{},\"app\":{},\"log\":{}}}", rec.pid, json_str(&rec.app), json_str(&rec.log.display().to_string()))),
            Err(e) => err(out, &e),
        },
        Err(e) => err(out, e),
    }
}

/// Starts the launcher on `spec`, records it in the ledger and in memory.
#[cfg(unix)]
fn start(st: &State, spec: &Spec) -> Result<Launched, String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let Some((exe, prefix)) = st.cfg.launch.split_first() else { return Err("this relay has no launcher configured".into()) };
    let log = std::env::temp_dir().join(format!("makepad-remote-launch-{}-{}.log", std::process::id(), crate::cmd::wall_ms()));
    let logfile = std::fs::File::create(&log).map_err(|_| "cannot create the session log".to_string())?;
    let logfile2 = logfile.try_clone().map_err(|_| "cannot create the session log".to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(prefix).arg(&spec.app).args(spec.args.split_whitespace()).stdin(Stdio::null()).stdout(logfile).stderr(logfile2).process_group(0);
    for (k, v) in &st.cfg.env {
        cmd.env(k, v);
    }
    if !spec.cwd.is_empty() {
        cmd.current_dir(&spec.cwd);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start {exe}: {e}"))?;
    let pid = child.id();
    let exit = Arc::new(Mutex::new(None));
    let e2 = exit.clone();
    let cfg = st.cfg.clone();
    let _ = std::thread::Builder::new().name("makepad-remote-reap".into()).spawn(move || {
        let r = child.wait().map_or_else(|e| format!("lost: {e}"), |s| format!("exited {s}"));
        *e2.lock().unwrap() = Some(r);
        forget(&cfg, pid);
    });
    let rec = Launched {
        pid,
        start: proc_start(pid).unwrap_or(0),
        app: spec.app.clone(),
        args: spec.args.clone(),
        cwd: spec.cwd.clone(),
        env_keys: st.cfg.env.iter().map(|(k, _)| k.clone()).collect(),
        log,
        exit,
        own: true,
    };
    // a ledger that cannot be written only costs the survival across relay restarts
    let _ = std::fs::create_dir_all(&st.cfg.ledger);
    let _ = makepad_network::tls::write_private(&ledger_file(&st.cfg, pid), rec.ledger_text().as_bytes());
    st.launched.lock().unwrap().push(rec.clone());
    Ok(rec)
}

#[cfg(not(unix))]
fn launch(_: &State, _: Vec<&str>, out: &mut Vec<String>) {
    err(out, "session launch is not available on Windows yet");
}

/// Signals a running launched session's process group and returns the record's exit cell.
#[cfg(unix)]
fn signal_session(st: &State, pid: u32) -> Result<(), String> {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    adopt(st);
    let l = st.launched.lock().unwrap();
    let Some(rec) = l.iter().find(|l| l.pid == pid) else { return Err(format!("pid {pid} was not launched by a relay on this host")) };
    if rec.ended(&st.cfg).is_some() {
        return Err(format!("pid {pid} has already exited"));
    }
    // the session is its own process group (its windows, helpers and threads end with it)
    if unsafe { kill(-(pid as i32), 15) } != 0 {
        return Err(format!("cannot signal {pid}"));
    }
    // a session that ignores SIGTERM (or is stuck) is ended for good after 3 s
    let (exit, start, own) = (rec.exit.clone(), rec.start, rec.own);
    let _ = std::thread::Builder::new().name("makepad-remote-reap".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let running = if own { exit.lock().unwrap().is_none() } else { proc_start(pid) == Some(start) };
        if running {
            unsafe { kill(-(pid as i32), 9) };
        }
    });
    Ok(())
}

#[cfg(unix)]
fn kill(st: &State, pid: u32, out: &mut Vec<String>) {
    match signal_session(st, pid) {
        Ok(_) => out.push(format!("{{\"killed\":{pid}}}")),
        Err(e) => err(out, &e),
    }
}

#[cfg(not(unix))]
fn kill(_: &State, _: u32, out: &mut Vec<String>) {
    err(out, "session kill is not available on Windows yet");
}

/// Relax owns its domain lifecycle. Native processes retain their process-level lifecycle.
#[cfg(unix)]
fn relaunch(st: &State, pid: u32, out: &mut Vec<String>) {
    // A Relax process keeps its endpoint, window and device. Never signal or replace it.
    // The configured `makepad run` launcher is also authoritative before discovery starts.
    // An unavailable inspect endpoint must never turn a warm relaunch into a process kill.
    if st.cfg.launch.get(1).is_some_and(|arg| arg == "run")
        || crate::discovery::apps().iter().any(|a| a.pid == pid && a.text("run").is_some())
    {
        adopt(st);
        {
            let launched = st.launched.lock().unwrap();
            match launched.iter().find(|r| r.pid == pid) {
                Some(r) if r.ended(&st.cfg).is_none() => {},
                Some(_) => return err(out, &format!("pid {pid} has already exited")),
                None => return err(out, &format!("pid {pid} was not launched by a relay on this host")),
            }
        }
        let client = match Client::connect(&pid.to_string()) {
            Ok(c) => c,
            Err(e) => return err(out, &e.to_string()),
        };
        match client.cmd("session relaunch") {
            Ok(lines) => for line in lines {
                match line { Ok(line) => out.push(line), Err(e) => { err(out, &e.to_string()); break; } }
            }
            Err(e) => err(out, &e.to_string()),
        }
        client.close();
        return;
    }
    let spec = {
        adopt(st);
        let l = st.launched.lock().unwrap();
        match l.iter().find(|l| l.pid == pid) {
            Some(r) => Spec { app: r.app.clone(), args: r.args.clone(), cwd: r.cwd.clone() },
            None => return err(out, &format!("pid {pid} was not launched by a relay on this host")),
        }
    };
    if let Err(e) = signal_session(st, pid) {
        return err(out, &e);
    }
    let end = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let gone = st.launched.lock().unwrap().iter().find(|l| l.pid == pid).is_none_or(|l| l.ended(&st.cfg).is_some());
        if gone {
            break;
        }
        if std::time::Instant::now() >= end {
            return err(out, &format!("pid {pid} did not exit within 10 s"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match start(st, &spec) {
        Ok(rec) => out.push(format!("{{\"relaunched\":{},\"was\":{pid},\"app\":{},\"log\":{}}}", rec.pid, json_str(&rec.app), json_str(&rec.log.display().to_string()))),
        Err(e) => err(out, &e),
    }
}

fn fwd() -> Factory {
    Arc::new(|_, open| {
        let mut it = open.args.splitn(3, ' ');
        let (spec, service, args) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
        if spec.is_empty() || service.is_empty() || spec.contains('/') {
            return Err((CLOSE_REFUSED, format!("fwd: expected `<pid|app> <service> [args]`, got `{}`", open.args)));
        }
        let client = Client::connect(spec).map_err(|e| (CLOSE_REFUSED, e.to_string()))?;
        let up = client.open(service, args).map_err(to_close)?;
        bridge(open.channel, up, move || client.close());
        Ok(())
    })
}

fn to_close(e: Error) -> (u16, String) {
    match e {
        Error::LinkLost(w) => (CLOSE_LINK_LOST, w),
        Error::Refused(c, w) => (c, w),
        Error::Timeout => (CLOSE_REFUSED, e.to_string()),
        Error::Io(e) => (CLOSE_REFUSED, e.to_string()),
    }
}

/// Copies messages both ways between two channels until either closes; the close (with its code
/// and reason) is passed on. `finish` runs once both directions have ended.
pub fn bridge(a: Channel, b: Channel, finish: impl FnOnce() + Send + 'static) {
    let (atx, btx) = (a.tx(), b.tx());
    let dead = Arc::new(AtomicBool::new(false));
    let left = Arc::new(AtomicUsize::new(2));
    let finish: Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>> = Arc::new(Mutex::new(Some(Box::new(finish))));
    for (from, to) in [(a, btx), (b, atx)] {
        let (dead, left, finish) = (dead.clone(), left.clone(), finish.clone());
        let _ = std::thread::Builder::new().name("makepad-remote-bridge".into()).spawn(move || {
            pump(&from, &to, &dead);
            dead.store(true, Relaxed);
            if left.fetch_sub(1, Relaxed) == 1 {
                if let Some(f) = finish.lock().unwrap().take() {
                    f();
                }
            }
        });
    }
}

fn pump(from: &Channel, to: &ChannelTx, dead: &AtomicBool) {
    loop {
        match from.recv_timeout(Duration::from_millis(300)) {
            Ok(Event::Data(d)) => {
                if to.send(d).is_err() {
                    return;
                }
            }
            Ok(Event::Closed(code, why)) => {
                to.close(code, &why);
                return;
            }
            Ok(Event::Opened(_)) => {}
            Err(true) => {
                if dead.load(Relaxed) {
                    return;
                }
            }
            Err(false) => {
                to.close(CLOSE_LINK_LOST, "channel gone");
                return;
            }
        }
    }
}

/// The credentials a dial relay presents: the key in `key_file`, the hub's pinned `fingerprint`
/// (hex; empty: trust on first contact, recorded in `known_hosts`).
#[cfg(not(target_arch = "wasm32"))]
pub fn credentials(key_file: &std::path::Path, fingerprint: &str, known_hosts: PathBuf) -> std::io::Result<makepad_network::tunnel::ClientCredentials> {
    use makepad_network::tunnel::{parse_key_file, ClientCredentials};
    let keys = parse_key_file(&std::fs::read_to_string(key_file)?)?;
    let expect = if fingerprint.is_empty() { None } else { Some(makepad_network::tls::from_hex::<32>(fingerprint).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "the fingerprint must be 64 hex digits"))?) };
    Ok(ClientCredentials { keys, known_hosts: Some(makepad_network::tls::KnownHosts::new(known_hosts)), expect })
}

/// Serves the relay over this process's stdin/stdout until the hub is gone.
pub fn serve_stdio(ep: &Endpoint) -> std::io::Result<()> {
    crate::carrier::stdio::serve_own(ep)
}

/// Dials the hub at `addr` and serves the relay over that connection; when the link drops it
/// dials again (backoff 1 s doubling to 30 s) and sends RESUME with the same link id. Never
/// returns.
#[cfg(not(target_arch = "wasm32"))]
pub fn dial_forever(ep: &Endpoint, addr: &str, creds: &makepad_network::tunnel::ClientCredentials) -> ! {
    let link_id = format!("{}-{:x}", std::process::id(), crate::cmd::wall_ms());
    let mut backoff = 1u64;
    let mut ever = false;
    loop {
        match crate::carrier::tcp::dial(addr, creds) {
            Ok(car) => match ep.connect(car, &link_id) {
                Ok(conn) => {
                    backoff = 1;
                    if ever {
                        conn.send_control(crate::hello::encode_resume(&crate::hello::Resume { link_id: link_id.clone(), seqs: Vec::new() }));
                    }
                    ever = true;
                    eprintln!("remote-relay: linked to {addr}");
                    conn.wait_closed();
                    eprintln!("remote-relay: link to {addr} lost: {}", conn.close_reason());
                }
                Err(e) => eprintln!("remote-relay: {addr}: {e}"),
            },
            Err(e) => eprintln!("remote-relay: cannot reach {addr}: {e}"),
        }
        std::thread::sleep(Duration::from_secs(backoff));
        backoff = (backoff * 2).min(30);
    }
}
