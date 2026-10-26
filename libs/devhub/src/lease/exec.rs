//! The node side of leases: `lease-start`, `lease-run`, `lease-log`, `lease-end`, `lease-beat`,
//! `policy`, `caps`, `disk`, `recover-get` / `recover-del` on the node's master endpoint
//! (LEASE.SPEC.md "Wire").
//!
//! - A lease gets a workspace: with a rev, a detached worktree of the node's git store at
//!   `<work>/wt/<lane>-<id>` (+ `apps/commercial`) and a target per tip `<work>/target/<sha12>`
//!   (the lane's previous one renamed when free: warm); without, `<work>/lane/<lane>/src` and
//!   `<work>/target/lane-<lane>`. Its scratch is `<work>/tmp/<id>` (TMPDIR, run logs).
//! - A run is the argv as given (no shell), in its own session (setsid), under
//!   `systemd-run --user --scope` when the user bus exists, `nice -n 10`, optionally pinned
//!   (`cpus.*` in lease.conf). Its pid and start time go into the ledger
//!   (`~/.makepad/devhub/leases/<id>.json`), so a restarted hub still ends what it started.
//! - End (release, master expiry, or 60 s without `lease-beat`: the node's own watchdog): TERM the
//!   recorded process groups whose leader's start time still matches, 10 s grace, KILL, stop the
//!   scopes; a survivor is a `lease_leak`. Worktrees with dirty files or commits no store ref
//!   holds are committed (`WIP devhub recovery <id>`) and bundled to `<work>/recovered/` first;
//!   never `git stash`. Nothing ends for running long.
//! - An inspect lease's end removes its breakpoints (`rm G<n>`) and probes (`probe rm P<n>`),
//!   lets parked hits go (`continue all`) and resumes a pause (`resume`), over the JIT's socket.

use super::caps::{self, LeaseConf};
use super::disk::{self, Guard, Live, Roots};
use super::wt::{dirty, git, unique_commits};
use super::Kind;
use crate::local::Publish;
use crate::proto::{hub_record, Sev};
use makepad_remote::json_str;
use makepad_remote::{lines, Endpoint};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The services this module adds to the node's master endpoint.
pub const WORDS: &[&str] = &["lease-start", "lease-run", "lease-log", "lease-end", "lease-beat", "policy", "caps", "disk", "recover-get", "recover-del"];

const LOG_CHUNK: usize = 64 << 10;
const RECOVER_CHUNK: usize = 256 << 10;

/// Timing knobs (tests shorten them).
#[derive(Clone, Debug)]
pub struct Opts {
    /// no `lease-beat` for this long: the node ends the lease itself
    pub no_beat: Duration,
    /// how often the watchdog looks
    pub tick: Duration,
    /// TERM to KILL on end
    pub grace: Duration,
    /// run under `systemd-run --user --scope` (None: when the user bus exists)
    pub scopes: Option<bool>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { no_beat: Duration::from_secs(60), tick: Duration::from_secs(5), grace: Duration::from_secs(10), scopes: None }
    }
}

struct Run {
    n: u32,
    pid: u32,
    start: u64,
    unit: Option<String>,
    log: PathBuf,
    /// set by the waiter; None while running (or unknown after a hub restart)
    exit: Mutex<Option<i32>>,
    /// started by this hub process (a waiter reaps it)
    ours: bool,
}

struct Ws {
    path: PathBuf,
    store: PathBuf,
    repo: String,
}

struct Lease {
    id: String,
    lane: String,
    kind: Kind,
    work: PathBuf,
    target: PathBuf,
    tmp: PathBuf,
    env: Vec<(String, String)>,
    /// makepad first, then the nested commercial one
    ws: Vec<Ws>,
    runs: Vec<Arc<Run>>,
    beat: Instant,
}

impl Lease {
    fn answer(&self) -> String {
        let env: Vec<String> = self.env.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        format!(
            "{{\"ok\":1,\"work\":{},\"target\":{},\"tmp\":{},\"env\":{{{}}}}}",
            json_str(&self.work.to_string_lossy()),
            json_str(&self.target.to_string_lossy()),
            json_str(&self.tmp.to_string_lossy()),
            env.join(",")
        )
    }

    fn ledger(&self) -> String {
        let ws: Vec<String> = self.ws.iter().map(|w| format!("{{\"path\":{},\"store\":{},\"repo\":{}}}", json_str(&w.path.to_string_lossy()), json_str(&w.store.to_string_lossy()), json_str(&w.repo))).collect();
        let runs: Vec<String> = self
            .runs
            .iter()
            .map(|r| format!("{{\"n\":{},\"pid\":{},\"start\":{},\"unit\":{},\"log\":{}}}", r.n, r.pid, r.start, r.unit.as_deref().map_or("null".into(), json_str), json_str(&r.log.to_string_lossy())))
            .collect();
        let a = self.answer();
        format!("{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"ws\":[{}],\"runs\":[{}],{}", json_str(&self.id), json_str(&self.lane), self.kind.as_str(), ws.join(","), runs.join(","), &a[1..])
    }

    fn from_ledger(text: &str) -> Option<Lease> {
        let v = makepad_strict_json::parse(text.as_bytes()).ok()?;
        let s = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let n = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(super::num).unwrap_or(0.0) as u64;
        let mut env = Vec::new();
        if let Some(makepad_strict_json::Value::Obj(o)) = v.get("env") {
            for (k, x) in o {
                env.push((k.to_string(), x.as_str()?.to_string()));
            }
        }
        let ws = v.get("ws").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| {
            a.iter().filter_map(|w| Some(Ws { path: s(w, "path")?.into(), store: s(w, "store")?.into(), repo: s(w, "repo")? })).collect()
        });
        let runs = v.get("runs").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| {
            a.iter()
                .filter_map(|r| Some(Arc::new(Run { n: n(r, "n") as u32, pid: n(r, "pid") as u32, start: n(r, "start"), unit: s(r, "unit"), log: s(r, "log")?.into(), exit: Mutex::new(None), ours: false })))
                .collect()
        });
        Some(Lease {
            id: s(&v, "id")?,
            lane: s(&v, "lane")?,
            kind: Kind::parse(&s(&v, "kind")?)?,
            work: s(&v, "work")?.into(),
            target: s(&v, "target")?.into(),
            tmp: s(&v, "tmp")?.into(),
            env,
            ws,
            runs,
            beat: Instant::now(),
        })
    }
}

/// The node's lease executor.
pub struct Exec {
    pub machine: String,
    publish: Publish,
    pub conf: LeaseConf,
    /// the ledger folder (`~/.makepad/devhub/leases`)
    state: PathBuf,
    opts: Opts,
    scopes: bool,
    dev: AtomicBool,
    leases: Mutex<BTreeMap<String, Lease>>,
    pub guard: Guard,
}

/// Lease ids, lanes, env keys: what a path, a unit name and a line can hold.
fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with(['-', '.']) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') && !s.contains("..")
}

fn err(e: impl AsRef<str>) -> String {
    format!("{{\"err\":{}}}", json_str(e.as_ref()))
}

fn user_bus() -> bool {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let on_path = |tool: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()));
    rt.is_some_and(|r| r.join("bus").exists()) && on_path("systemd-run") && on_path("systemctl")
}

impl Exec {
    /// The executor over `conf.work`, its ledger in `state`; leases left in the ledger by a
    /// previous hub are taken back (the watchdog ends them unless the master still beats them).
    pub fn new(machine: &str, conf: LeaseConf, state: PathBuf, opts: Opts, publish: Publish) -> Arc<Exec> {
        let guard = Guard::new(machine, Roots::of(&conf.work), conf.disk_floor_gb, conf.disk_warn_gb, conf.role.users(), publish.clone());
        let scopes = opts.scopes.unwrap_or_else(user_bus);
        let x = Arc::new(Exec { machine: machine.to_string(), publish, dev: AtomicBool::new(!conf.role.users()), conf, state, opts, scopes, leases: Mutex::new(BTreeMap::new()), guard });
        let mut back = Vec::new();
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(l) = std::fs::read_to_string(&p).ok().and_then(|t| Lease::from_ledger(&t)) {
                    back.push(l.id.clone());
                    x.leases.lock().unwrap().insert(l.id.clone(), l);
                }
            }
        }
        if !back.is_empty() {
            x.emit("lease", Sev::Info, format!("lease: hub on {} took back {} lease(s) from its ledger: {}", x.machine, back.len(), back.join(" ")));
        }
        let w = Arc::downgrade(&x);
        let tick = x.opts.tick;
        let _ = std::thread::Builder::new().name("devhub-lease-watchdog".into()).spawn(move || watchdog(w, tick));
        let w = Arc::downgrade(&x);
        let _ = std::thread::Builder::new().name("devhub-disk".into()).spawn(move || disk_loop(w));
        x
    }

    fn emit(&self, kind: &str, sev: Sev, line: String) {
        (self.publish)(hub_record(&self.machine, "", 0, kind, sev, line, String::new()));
    }

    fn save(&self, l: &Lease) {
        let _ = std::fs::create_dir_all(&self.state);
        let p = self.state.join(format!("{}.json", l.id));
        let tmp = self.state.join(format!("{}.json.new", l.id));
        if std::fs::write(&tmp, l.ledger()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    /// What is in use now (for the disk guard).
    pub fn live(&self) -> Live {
        let mut leases = Vec::new();
        for l in self.leases.lock().unwrap().values() {
            for p in [&l.work, &l.target, &l.tmp] {
                leases.push((p.clone(), l.id.clone()));
            }
        }
        Live { leases, cwds: super::wt::live_cwds() }
    }

    /// One request of service `word` (the line may repeat the word); answer lines.
    pub fn handle(&self, word: &str, line: &str) -> Vec<String> {
        let rest = match line.strip_prefix(word) {
            Some(r) if r.is_empty() || r.starts_with(' ') => r.trim(),
            _ => line.trim(),
        };
        match word {
            "lease-start" => vec![self.start(rest)],
            "lease-run" => vec![self.run(rest)],
            "lease-log" => vec![self.log(rest)],
            "lease-end" => vec![self.end_cmd(rest)],
            "lease-beat" => vec![self.beat(rest)],
            "policy" => vec![self.policy(rest)],
            "caps" => {
                let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
                vec![caps::measure(&self.machine, &self.conf, apply).to_json()]
            }
            "disk" => match rest.strip_prefix("clean ") {
                Some(lane) => vec![self.clean_lane(lane.trim())],
                None => self.guard.reclaim(&self.live(), rest == "dry" || rest == "--dry-run"),
            },
            "recover-get" => vec![self.recover_get(rest)],
            "recover-del" => vec![self.recover_del(rest)],
            _ => vec![err(format!("unknown service {word}"))],
        }
    }

    fn policy(&self, rest: &str) -> String {
        match rest {
            "dev=on" => self.dev.store(true, Ordering::Release),
            "dev=off" => self.dev.store(false, Ordering::Release),
            "" => {}
            _ => return err("usage: policy dev=on|off"),
        }
        format!("{{\"dev\":\"{}\"}}", if self.dev.load(Ordering::Acquire) { "on" } else { "off" })
    }

    fn start(&self, json: &str) -> String {
        let Ok(v) = makepad_strict_json::parse(json.as_bytes()) else { return err("lease-start: not JSON") };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let (id, lane) = (s("id"), s("lane"));
        if !valid_name(&id) || !valid_name(&lane) {
            return err("lease-start: id and lane are a-z, 0-9, - _ .");
        }
        let Some(kind) = Kind::parse(&s("kind")) else { return err(format!("lease-start: kind {}", s("kind"))) };
        if let Some(l) = self.leases.lock().unwrap().get(&id) {
            return l.answer();
        }
        if kind.writes() && !self.dev.load(Ordering::Acquire) {
            return err(format!("dev off: {} takes no {} leases", self.machine, kind.as_str()));
        }
        let rev = v.get("rev").filter(|r| r.get("makepad").is_some());
        let rev_of = |k: &str| rev.and_then(|r| r.get(k)).and_then(|x| x.as_str()).map(str::to_string);
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(makepad_strict_json::Value::Obj(o)) = v.get("env") {
            for (k, x) in o {
                match x.as_str() {
                    Some(x) if valid_name(k) && !x.contains('\0') => env.push((k.to_string(), x.to_string())),
                    _ => return err(format!("lease-start: env {k}")),
                }
            }
        }
        let w = &self.conf.work;
        let tmp = w.join("tmp").join(&id);
        let (work, target, ws) = match rev_of("makepad") {
            Some(sha) => match self.workspace(&id, &lane, &sha, rev_of("commercial").as_deref()) {
                Ok(x) => x,
                Err(e) => return err(format!("lease-start {id}: {e}")),
            },
            None => (w.join("lane").join(&lane).join("src"), w.join("target").join(format!("lane-{lane}")), Vec::new()),
        };
        for d in [&tmp, &work, &target] {
            if let Err(e) = std::fs::create_dir_all(d) {
                return err(format!("{}: {e}", d.display()));
            }
        }
        let law = [
            ("CARGO_TARGET_DIR", target.to_string_lossy().into_owned()),
            ("TMPDIR", tmp.to_string_lossy().into_owned()),
            ("CARGO_INCREMENTAL", "0".into()),
            ("MAKEPAD_HIDE_WINDOWS", "1".into()),
            ("SANDBOX_MUTE", "1".into()),
            ("MAKEPAD_LAN", "0".into()),
            ("CARGO_NET_OFFLINE", "true".into()),
        ];
        // the lease's own environment wins over what was asked
        env.retain(|(k, _)| !law.iter().any(|(l, _)| l == k));
        env.extend(law.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let l = Lease { id: id.clone(), lane: lane.clone(), kind, work: work.clone(), target, tmp, env, ws, runs: Vec::new(), beat: Instant::now() };
        let a = l.answer();
        self.save(&l);
        let what = match rev_of("makepad") {
            Some(sha) => format!("worktree at {}", &sha[..12.min(sha.len())]),
            None => "lane dir".into(),
        };
        self.leases.lock().unwrap().insert(id.clone(), l);
        self.guard.touch();
        self.emit("ws", Sev::Info, format!("ws_create: {id} {} for {lane} on {}: {what} {}", kind.as_str(), self.machine, work.display()));
        a
    }

    /// A rev lease's worktree(s) from the store and its target per tip.
    fn workspace(&self, id: &str, lane: &str, sha: &str, commercial: Option<&str>) -> Result<(PathBuf, PathBuf, Vec<Ws>), String> {
        let hex = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex(sha) || commercial.is_some_and(|c| !hex(c)) {
            return Err("rev: a 40-hex commit".into());
        }
        let (store, cstore) = stores()?;
        let w = &self.conf.work;
        let path = w.join("wt").join(format!("{lane}-{id}"));
        std::fs::create_dir_all(w.join("wt")).map_err(|e| e.to_string())?;
        git(None, Some(&store), &["worktree", "add", "--detach", "-q", &path.to_string_lossy(), sha])?;
        let mut ws = vec![Ws { path: path.clone(), store, repo: "makepad".into() }];
        if let Some(c) = commercial {
            let cp = path.join("apps/commercial");
            if let Err(e) = git(None, Some(&cstore), &["worktree", "add", "--detach", "-q", &cp.to_string_lossy(), c]) {
                let _ = git(None, Some(&ws[0].store), &["worktree", "remove", "--force", &path.to_string_lossy()]);
                return Err(e);
            }
            ws.push(Ws { path: cp, store: cstore, repo: "commercial".into() });
        }
        let tip = &sha[..12];
        let targets = w.join("target");
        let target = targets.join(tip);
        if !target.exists() {
            let last = targets.join(format!("lane-{lane}.last"));
            let prev = std::fs::read_to_string(&last).ok().map(|s| s.trim().to_string()).filter(|s| valid_name(s) && s != tip);
            let in_use = |p: &Path| self.leases.lock().unwrap().values().any(|l| l.target == p);
            if let Some(prev) = prev.map(|p| targets.join(p)).filter(|p| p.is_dir() && !in_use(p)) {
                let _ = std::fs::rename(&prev, &target);
            }
            let _ = std::fs::create_dir_all(&targets);
            let tmp = targets.join(format!("lane-{lane}.last.new"));
            if std::fs::write(&tmp, tip).is_ok() {
                let _ = std::fs::rename(&tmp, &last);
            }
        }
        Ok((path, target, ws))
    }

    fn run(&self, rest: &str) -> String {
        let mut w = rest.splitn(3, ' ');
        let (id, cwd, b64) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or("").trim());
        let Some(raw) = b64_decode(b64) else { return err("lease-run: argv is not base64") };
        let argv: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let argv: Vec<String> = match argv.iter().rposition(|a| !a.is_empty()) {
            Some(last) => argv[..=last].to_vec(),
            None => return err("lease-run: empty argv"),
        };
        if argv[0].is_empty() {
            return err("lease-run: empty program");
        }
        let rel = Path::new(cwd);
        if cwd.is_empty() || rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return err("lease-run: cwd is `.` or a path inside the lease's work dir");
        }
        let mut leases = self.leases.lock().unwrap();
        let Some(l) = leases.get_mut(id) else { return err(format!("no lease {id} on {}", self.machine)) };
        let n = l.runs.len() as u32 + 1;
        let log = l.tmp.join(format!("run-{n}.log"));
        let unit = self.scopes.then(|| format!("devhub-lease-{id}-{n}"));
        let mut full: Vec<String> = Vec::new();
        if let Some(u) = &unit {
            full.extend(["systemd-run", "--user", "--scope", "--quiet"].map(String::from));
            full.push(format!("--unit={u}"));
            full.push("--".into());
        }
        full.extend(["nice", "-n", "10"].map(String::from));
        let cpus = if l.kind == Kind::Timed { &self.conf.cpus_timed } else { &self.conf.cpus_shared };
        if !cpus.is_empty() {
            full.extend(["taskset".to_string(), "-c".to_string(), cpus.clone()]);
        }
        full.extend(argv);
        let out = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => return err(format!("{}: {e}", log.display())),
        };
        let err_out = match out.try_clone() {
            Ok(f) => f,
            Err(e) => return err(e.to_string()),
        };
        let mut cmd = std::process::Command::new(&full[0]);
        cmd.args(&full[1..]).current_dir(l.work.join(rel)).stdin(std::process::Stdio::null()).stdout(out).stderr(err_out);
        for (k, v) in &l.env {
            cmd.env(k, v);
        }
        {
            use std::os::unix::process::CommandExt;
            extern "C" {
                fn setsid() -> i32;
            }
            // its own session and process group: the group is what end signals
            unsafe {
                cmd.pre_exec(|| {
                    setsid();
                    Ok(())
                });
            }
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return err(format!("lease-run {id}: {}: {e}", full[0])),
        };
        let pid = child.id();
        let run = Arc::new(Run { n, pid, start: proc_start(pid).unwrap_or(0), unit, log, exit: Mutex::new(None), ours: true });
        l.runs.push(run.clone());
        self.save(l);
        drop(leases);
        let _ = std::thread::Builder::new().name("devhub-lease-wait".into()).spawn(move || {
            use std::os::unix::process::ExitStatusExt;
            let code = child.wait().map(|s| s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))).unwrap_or(-1);
            *run.exit.lock().unwrap() = Some(code);
        });
        format!("{{\"run\":{pid},\"n\":{n}}}")
    }

    fn log(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let (Some(id), Some(Ok(which)), Some(Ok(off))) = (w.first(), w.get(1).map(|x| x.parse::<u32>()), w.get(2).map(|x| x.parse::<u64>())) else {
            return err("usage: lease-log <id> <run n|pid> <offset>");
        };
        let run = {
            let leases = self.leases.lock().unwrap();
            let Some(l) = leases.get(*id) else { return err(format!("no lease {id} on {}", self.machine)) };
            match l.runs.iter().find(|r| r.n == which).or_else(|| l.runs.iter().find(|r| r.pid == which)) {
                Some(r) => r.clone(),
                None => return err(format!("lease {id} has no run {which}")),
            }
        };
        // the exit is known before the last bytes are read: say it only once they are
        let exit = *run.exit.lock().unwrap();
        let exit = exit.or_else(|| (!run.ours && !group_alive(run.pid, run.start)).then_some(-1));
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = Vec::new();
        let mut len = 0;
        if let Ok(mut f) = std::fs::File::open(&run.log) {
            len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if f.seek(SeekFrom::Start(off)).is_ok() {
                let _ = f.take(LOG_CHUNK as u64).read_to_end(&mut buf);
            }
        }
        let end = off + buf.len() as u64;
        let exit = if end >= len { exit.map_or("null".to_string(), |c| c.to_string()) } else { "null".into() };
        format!("{{\"data\":\"{}\",\"off\":{end},\"exit\":{exit}}}", makepad_network::digest::base64_encode(&buf))
    }

    fn beat(&self, rest: &str) -> String {
        let mut leases = self.leases.lock().unwrap();
        let (mut live, mut unknown) = (Vec::new(), Vec::new());
        for id in rest.split_whitespace() {
            match leases.get_mut(id) {
                Some(l) => {
                    l.beat = Instant::now();
                    live.push(json_str(id));
                }
                None => unknown.push(json_str(id)),
            }
        }
        format!("{{\"live\":[{}],\"unknown\":[{}]}}", live.join(","), unknown.join(","))
    }

    /// `lease-end <id> <how> [{"inspect":{"pid":N,"breaks":[..],"probes":[..],"paused":true}}]`
    fn end_cmd(&self, rest: &str) -> String {
        let mut w = rest.splitn(3, ' ');
        let (id, how, extra) = (w.next().unwrap_or(""), w.next().unwrap_or("released"), w.next().unwrap_or("").trim());
        if !valid_name(id) || !matches!(how, "released" | "expired" | "holder-dead" | "denied") {
            return err("usage: lease-end <id> released|expired|holder-dead|denied [inspect json]");
        }
        let mut inspect = String::new();
        if !extra.is_empty() {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-end: the inspect part is not JSON") };
            if let Some(i) = v.get("inspect") {
                let list = |k: &str| i.get(k).and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
                let pid = i.get("pid").and_then(super::num).unwrap_or(0.0) as u32;
                let done = inspect_release(pid, &list("breaks"), &list("probes"), i.get("paused").and_then(|x| x.as_bool()).unwrap_or(false));
                self.emit("lease", Sev::Info, format!("inspect: lease {id} released pid {pid} on {}: {}", self.machine, if done.is_empty() { "nothing held".to_string() } else { done.join(", ") }));
                inspect = format!(",\"inspect\":[{}]", done.iter().map(|d| json_str(d)).collect::<Vec<_>>().join(","));
            }
        }
        let l = self.leases.lock().unwrap().remove(id);
        let Some(l) = l else {
            return format!("{{\"ended\":{},\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[],\"unknown\":true{inspect}}}", json_str(id));
        };
        let e = self.end(l, how);
        format!("{}{inspect}}}", &e[..e.len() - 1])
    }

    /// Ends a lease taken out of the table: processes, scratch, workspace; the answer line.
    fn end(&self, l: Lease, how: &str) -> String {
        let (killed, leak) = stop_runs(&l.runs, self.opts.grace);
        let mut freed = 0u64;
        if l.tmp.exists() {
            freed += disk::dir_size(&l.tmp).0;
            let _ = std::fs::remove_dir_all(&l.tmp);
        }
        let mut recovered = Vec::new();
        let mut kept = false;
        // nested (commercial) first: it lives inside the makepad worktree
        for ws in l.ws.iter().rev() {
            if !ws.path.exists() {
                let _ = git(None, Some(&ws.store), &["worktree", "prune"]);
                continue;
            }
            let except = (ws.repo == "makepad").then_some("apps/commercial");
            let tag = if ws.repo == "makepad" { format!("{}-{}", l.lane, l.id) } else { format!("{}-{}-{}", l.lane, l.id, ws.repo) };
            match recover_worktree(&ws.path, except, &l.id, &self.conf.work.join("recovered"), &tag) {
                Ok(Some(b)) => {
                    self.emit("ws", Sev::Warn, format!("ws_recovered: {} {} on {} had unsaved work in {}: bundled to {}", l.id, l.lane, self.machine, ws.repo, b.display()));
                    recovered.push(b);
                }
                Ok(None) => {}
                Err(e) => {
                    kept = true;
                    self.emit("ws", Sev::Err, format!("ws_unrecoverable: {} {} on {}: {} worktree kept at {}: {e}", l.id, l.lane, self.machine, ws.repo, ws.path.display()));
                    continue;
                }
            }
            let size = disk::dir_size(&ws.path).0;
            match git(None, Some(&ws.store), &["worktree", "remove", "--force", &ws.path.to_string_lossy()]) {
                Ok(_) => freed += size,
                Err(e) => {
                    kept = true;
                    self.emit("ws", Sev::Err, format!("ws_cleanup: {} on {}: {e}", ws.path.display(), self.machine));
                }
            }
            let _ = git(None, Some(&ws.store), &["worktree", "prune"]);
        }
        if !kept {
            let _ = std::fs::remove_file(self.state.join(format!("{}.json", l.id)));
        } else {
            // the worktree stays and so does its ledger entry, without the processes
            let rest = Lease { runs: Vec::new(), beat: Instant::now(), ..l };
            self.save(&rest);
            return self.ended_line(&rest.id, &killed, freed, &recovered, &leak);
        }
        self.guard.touch();
        let mb = freed >> 20;
        self.emit("ws", Sev::Info, format!("ws_cleanup: {} {} on {} freed {mb} MB", l.id, l.lane, self.machine));
        let sev = if how == "released" { Sev::Info } else { Sev::Warn };
        self.emit("lease", sev, format!("lease: {} {} of {} ended on {} ({how}): {} process group(s) stopped, {mb} MB freed", l.id, l.kind.as_str(), l.lane, self.machine, killed.len()));
        if !leak.is_empty() {
            self.emit("lease", Sev::Err, format!("lease_leak: {} {} on {}: pid(s) {:?} survived KILL", l.id, l.lane, self.machine, leak));
        }
        self.ended_line(&l.id, &killed, freed, &recovered, &leak)
    }

    fn ended_line(&self, id: &str, killed: &[u32], freed: u64, recovered: &[PathBuf], leak: &[u32]) -> String {
        let r = |p: Option<&PathBuf>| p.map_or("null".to_string(), |p| json_str(&p.to_string_lossy()));
        format!(
            "{{\"ended\":{},\"killed\":{:?},\"freed_mb\":{},\"recovered\":{},\"recovered_all\":[{}],\"leak\":{:?}}}",
            json_str(id),
            killed,
            freed >> 20,
            r(recovered.last()),
            recovered.iter().map(|p| json_str(&p.to_string_lossy())).collect::<Vec<_>>().join(","),
            leak
        )
    }

    /// Ends the leases that have not been beaten for `no_beat`.
    fn expire(&self) {
        let stale: Vec<Lease> = {
            let mut leases = self.leases.lock().unwrap();
            let ids: Vec<String> = leases.values().filter(|l| l.beat.elapsed() >= self.opts.no_beat).map(|l| l.id.clone()).collect();
            ids.iter().filter_map(|id| leases.remove(id)).collect()
        };
        for l in stale {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} ended by {} itself: no beat from the master for {} s", l.id, l.kind.as_str(), l.lane, self.machine, self.opts.no_beat.as_secs()));
            self.end(l, "expired");
        }
    }

    fn recovered_path(&self, p: &str) -> Result<PathBuf, String> {
        let root = std::fs::canonicalize(self.conf.work.join("recovered")).map_err(|_| "nothing recovered on this node".to_string())?;
        let p = std::fs::canonicalize(p).map_err(|e| format!("{p}: {e}"))?;
        if !p.starts_with(&root) || !p.is_file() {
            return Err("only files under <work>/recovered".into());
        }
        Ok(p)
    }

    fn recover_get(&self, rest: &str) -> String {
        let (p, off) = rest.rsplit_once(' ').unwrap_or((rest, "0"));
        let Ok(off) = off.parse::<u64>() else { return err("usage: recover-get <path> <offset>") };
        let p = match self.recovered_path(p) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = Vec::new();
        let mut len = 0;
        if let Ok(mut f) = std::fs::File::open(&p) {
            len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if f.seek(SeekFrom::Start(off)).is_ok() {
                let _ = f.take(RECOVER_CHUNK as u64).read_to_end(&mut buf);
            }
        }
        let end = off + buf.len() as u64;
        format!("{{\"data\":\"{}\",\"off\":{end},\"eof\":{}}}", makepad_network::digest::base64_encode(&buf), end >= len)
    }

    /// The master fetched the bundle: the node copy goes.
    /// `disk clean <lane>` (the lane landed): its rsync tree, warm target and legacy pool dir go,
    /// unless a live lease of the lane still uses them.
    fn clean_lane(&self, lane: &str) -> String {
        if !crate::trust::valid_name(lane) {
            return err(format!("bad lane name `{lane}`"));
        }
        if let Some(l) = self.leases.lock().unwrap().values().find(|l| l.lane == lane) {
            return err(format!("lane {lane} holds live lease {}", l.id));
        }
        let w = &self.conf.work;
        let paths = [w.join("lane").join(lane), w.join("target").join(format!("lane-{lane}")), w.join("target").join(format!("lane-{lane}.last")), caps::home_dir().join("rc/pool").join(lane)];
        let mut freed = 0u64;
        let mut removed = Vec::new();
        for p in paths.iter().filter(|p| p.exists()) {
            freed += super::disk::dir_size(p).0;
            match super::disk::remove(p) {
                Ok(()) => removed.push(json_str(&p.to_string_lossy())),
                Err(e) => return err(format!("{}: {e}", p.display())),
            }
        }
        self.guard.touch();
        (self.publish)(crate::proto::hub_record(&self.machine, "", 0, "disk", crate::proto::Sev::Info, format!("disk: lane {lane} landed: {} dirs removed, {} MB freed", removed.len(), freed >> 20), String::new()));
        format!("{{\"cleaned\":{},\"removed\":[{}],\"freed_mb\":{}}}", json_str(lane), removed.join(","), freed >> 20)
    }

    fn recover_del(&self, rest: &str) -> String {
        match self.recovered_path(rest).and_then(|p| std::fs::remove_file(&p).map(|_| p).map_err(|e| e.to_string())) {
            Ok(p) => format!("{{\"deleted\":{}}}", json_str(&p.to_string_lossy())),
            Err(e) => err(e),
        }
    }
}

#[cfg(feature = "checkout")]
fn stores() -> Result<(PathBuf, PathBuf), String> {
    let s = crate::sync::store_dir("makepad");
    if !s.join("HEAD").exists() {
        return Err(format!("no git store at {} (sync push first)", s.display()));
    }
    Ok((s, crate::sync::store_dir("commercial")))
}

#[cfg(not(feature = "checkout"))]
fn stores() -> Result<(PathBuf, PathBuf), String> {
    Err("this host has no git stores: leases with a rev need a desktop node".into())
}

fn watchdog(w: Weak<Exec>, tick: Duration) {
    loop {
        std::thread::sleep(tick);
        let Some(x) = w.upgrade() else { return };
        x.expire();
    }
}

fn disk_loop(w: Weak<Exec>) {
    nice_this_thread();
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let Some(x) = w.upgrade() else { return };
        x.guard.check(&|| x.live());
    }
}

/// The measuring walk runs at nice 10 (Linux: per thread).
fn nice_this_thread() {
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
            fn gettid() -> i32;
        }
        unsafe {
            setpriority(0, gettid() as u32, 10);
        }
    }
}

/// Should a worktree be bundled back before it goes: uncommitted changes, or commits only it has.
pub fn needs_recovery(dirty: bool, unique: bool) -> bool {
    dirty || unique
}

/// Commits a dirty worktree as `WIP devhub recovery <id>` and bundles HEAD (minus what the
/// store's refs hold) to `<out>/<tag>.bundle`; None when there was nothing to save.
pub fn recover_worktree(path: &Path, except: Option<&str>, id: &str, out: &Path, tag: &str) -> Result<Option<PathBuf>, String> {
    let d = dirty(path, except);
    if !needs_recovery(d, unique_commits(path)) {
        return Ok(None);
    }
    if d {
        let ex = except.map(|e| format!(":(exclude){e}"));
        let mut add = vec!["add", "-A", "--", "."];
        if let Some(e) = &ex {
            add.push(e);
        }
        git(Some(path), None, &add)?;
        git(Some(path), None, &["-c", "user.name=devhub", "-c", "user.email=devhub@localhost", "commit", "-q", "--no-verify", "-m", &format!("WIP devhub recovery {id}")])?;
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let bundle = out.join(format!("{tag}.bundle"));
    let refs = git(Some(path), None, &["for-each-ref", "--format=%(objectname)"]).unwrap_or_default();
    let b = bundle.to_string_lossy().into_owned();
    let mut args = vec!["bundle", "create", "-q", &b, "HEAD"];
    let shas: Vec<&str> = refs.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
    if !shas.is_empty() {
        args.push("--not");
        args.extend(shas);
    }
    git(Some(path), None, &args)?;
    Ok(Some(bundle))
}

/// A process's start time (Linux: clock ticks since boot; macOS: µs since the epoch).
pub fn proc_start(pid: u32) -> Option<u64> {
    proc_stat(pid).map(|s| s.start)
}

struct Stat {
    start: u64,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pgrp: u32,
    zombie: bool,
}

#[cfg(target_os = "linux")]
fn proc_stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let f: Vec<&str> = stat[stat.rfind(')')? + 2..].split_whitespace().collect();
    // f[0] = field 3 (state), f[2] = 5 (pgrp), f[19] = 22 (starttime)
    Some(Stat { start: f.get(19)?.parse().ok()?, pgrp: f.get(2)?.parse().ok()?, zombie: f.first() == Some(&"Z") })
}

#[cfg(target_os = "macos")]
fn proc_stat(pid: u32) -> Option<Stat> {
    #[repr(C)]
    struct BsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        ppid: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        rfu: u32,
        comm: [u8; 16],
        name: [u8; 32],
        nfiles: u32,
        pgid: u32,
        pjobc: u32,
        tdev: u32,
        tpgid: u32,
        nice: i32,
        start_sec: u64,
        start_usec: u64,
    }
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut core::ffi::c_void, size: i32) -> i32;
    }
    let mut b: BsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<BsdInfo>() as i32;
    // PROC_PIDTBSDINFO = 3
    if unsafe { proc_pidinfo(pid as i32, 3, 0, &mut b as *mut BsdInfo as *mut _, size) } != size {
        return None;
    }
    // SZOMB = 5
    Some(Stat { start: b.start_sec * 1_000_000 + b.start_usec, pgrp: b.pgid, zombie: b.status == 5 })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn proc_stat(_pid: u32) -> Option<Stat> {
    None
}

/// The recorded leader still is the process we started (not gone, not a reused pid).
fn same(pid: u32, start: u64) -> bool {
    proc_stat(pid).is_some_and(|s| !s.zombie && s.start == start)
}

/// Whether the run's process group still has a live member. Its pgid is the leader's pid,
/// which the OS does not hand out again while the group exists; a reused leader pid (a
/// different start time) means the group is gone.
fn group_alive(pid: u32, start: u64) -> bool {
    if let Some(s) = proc_stat(pid) {
        if s.start != start {
            return false;
        }
        if !s.zombie {
            return true;
        }
    }
    #[cfg(target_os = "linux")]
    {
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            if let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() {
                if p != pid && proc_stat(p).is_some_and(|s| s.pgrp == pid && !s.zombie) {
                    return true;
                }
            }
        }
    }
    false
}

fn signal_group(pid: u32, sig: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    if pid > 1 {
        unsafe {
            kill(-(pid as i32), sig);
        }
    }
}

/// TERM the runs' groups, `grace`, KILL, stop their scopes: (stopped leaders, survivors).
fn stop_runs(runs: &[Arc<Run>], grace: Duration) -> (Vec<u32>, Vec<u32>) {
    const TERM: i32 = 15;
    const KILL: i32 = 9;
    let alive: Vec<&Arc<Run>> = runs.iter().filter(|r| r.start != 0 && group_alive(r.pid, r.start)).collect();
    for r in &alive {
        signal_group(r.pid, TERM);
    }
    let t = Instant::now();
    while t.elapsed() < grace && alive.iter().any(|r| group_alive(r.pid, r.start)) {
        std::thread::sleep(Duration::from_millis(100));
    }
    for r in alive.iter().filter(|r| group_alive(r.pid, r.start)) {
        signal_group(r.pid, KILL);
    }
    for u in runs.iter().filter_map(|r| r.unit.as_ref()) {
        let _ = std::process::Command::new("systemctl").args(["--user", "stop", &format!("{u}.scope")]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) && alive.iter().any(|r| group_alive(r.pid, r.start)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let leak = alive.iter().filter(|r| group_alive(r.pid, r.start) || same(r.pid, r.start)).map(|r| r.pid).collect();
    (alive.iter().map(|r| r.pid).collect(), leak)
}

/// An inspect lease's end: its breakpoint groups and probes removed, parked hits let go, a
/// pause resumed, over the JIT's own socket. The commands that answered.
pub fn inspect_release(pid: u32, breaks: &[String], probes: &[String], paused: bool) -> Vec<String> {
    let mut cmds = inspect_commands(breaks, probes, paused);
    if cmds.is_empty() {
        return Vec::new();
    }
    let Some(app) = makepad_remote::discovery::apps().into_iter().find(|a| a.pid == pid && !a.socket.starts_with("tcp:")) else {
        return vec![format!("pid {pid}: no JIT")];
    };
    let client = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(app.socket.into()), "devhub");
    client.set_timeout(Some(Duration::from_secs(3)));
    for c in cmds.iter_mut() {
        let first = client.cmd(c).ok().and_then(|mut l| l.next()).and_then(|l| l.ok()).unwrap_or_default();
        if first.starts_with("{\"err\"") || first.is_empty() {
            c.push_str(" (failed)");
        }
    }
    cmds
}

/// The JIT's own command words: `rm G<n>` per breakpoint group, `probe rm P<n>` per probe,
/// `continue all` after breakpoints, `resume` after a pause.
pub fn inspect_commands(breaks: &[String], probes: &[String], paused: bool) -> Vec<String> {
    let ok = |s: &str, p: char| s.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let mut cmds: Vec<String> = breaks.iter().filter(|b| ok(b, 'G')).map(|b| format!("rm {b}")).collect();
    cmds.extend(probes.iter().filter(|p| ok(p, 'P')).map(|p| format!("probe rm {p}")));
    if !breaks.is_empty() {
        cmds.push("continue all".into());
    }
    if paused {
        cmds.push("resume".into());
    }
    cmds
}

/// Standard base64 (padding optional); None on a stray character.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Registers the lease services on the node's master endpoint.
/// This process's executor (the node's), for sync's launches.
static EXEC: std::sync::OnceLock<Arc<Exec>> = std::sync::OnceLock::new();

/// Whether this node takes writes from the network (launches, stops, WM restarts): its dev
/// switch as the master last pushed it, or its role's default. A process with no node: no.
pub fn writes_allowed() -> bool {
    EXEC.get().is_some_and(|x| x.dev.load(Ordering::Acquire))
}

/// Puts a process started for lease `id` (a sync launch) under that lease: its end stops it.
pub fn adopt(id: &str, pid: u32, log: PathBuf) -> Result<(), String> {
    let x = EXEC.get().ok_or("this process runs no lease executor")?;
    let mut leases = x.leases.lock().unwrap();
    let l = leases.get_mut(id).ok_or_else(|| format!("no live lease {id} on {}", x.machine))?;
    if !l.kind.writes() {
        return Err(format!("{id} is an {} lease: launches need a write lease", l.kind.as_str()));
    }
    let n = l.runs.iter().map(|r| r.n).max().unwrap_or(0) + 1;
    l.runs.push(Arc::new(Run { n, pid, start: proc_start(pid).unwrap_or(0), unit: None, log, exit: Mutex::new(None), ours: false }));
    x.save(l);
    Ok(())
}

/// Whether lease `id` is live here and may launch.
pub fn lease_live(id: &str) -> bool {
    EXEC.get().is_some_and(|x| x.leases.lock().unwrap().get(id).is_some_and(|l| l.kind.writes()))
}

pub fn register(ep: &Endpoint, ctx: &crate::sync::NodeCtx) -> Arc<Exec> {
    let x = Exec::new(&ctx.machine, LeaseConf::load(), crate::proto::home().join("leases"), Opts::default(), ctx.publish.clone());
    let _ = EXEC.set(x.clone());
    for &word in WORDS {
        let e = x.clone();
        ep.register(word, 1, lines::service(word, Arc::new(move |line, out| out.extend(e.handle(word, line)))));
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(argv: &[&str]) -> String {
        makepad_network::digest::base64_encode(argv.join("\0").as_bytes())
    }

    fn field(line: &str, k: &str) -> Option<makepad_strict_json::Value> {
        makepad_strict_json::parse(line.as_bytes()).ok()?.get(k).cloned()
    }

    fn test_exec(name: &str, opts: Opts) -> (Arc<Exec>, PathBuf, Arc<Mutex<Vec<String>>>) {
        let base = std::env::temp_dir().join(format!("devhub-exec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let conf = LeaseConf::parse(&format!("work = {}\n", base.join("devhub").display()), &base);
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let x = Exec::new("t", conf, base.join("leases"), opts, Arc::new(move |r: crate::proto::Record| ev.lock().unwrap().push(r.line)));
        (x, base, events)
    }

    fn log_until_exit(x: &Exec, id: &str, n: u32) -> (String, i64) {
        let (mut text, mut off) = (Vec::new(), 0u64);
        for _ in 0..300 {
            let a = x.handle("lease-log", &format!("{id} {n} {off}"));
            let d = b64_decode(field(&a[0], "data").unwrap().as_str().unwrap()).unwrap();
            text.extend(d);
            off = super::super::num(&field(&a[0], "off").unwrap()).unwrap() as u64;
            if let Some(c) = field(&a[0], "exit").and_then(|e| super::super::num(&e)) {
                return (String::from_utf8_lossy(&text).into_owned(), c as i64);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("run {n} of {id} did not exit");
    }

    #[test]
    fn base64_and_inspect_words() {
        assert_eq!(b64_decode(&b64(&["/bin/echo", "hi"])).unwrap(), b"/bin/echo\0hi");
        assert_eq!(b64_decode("aGk"), Some(b"hi".to_vec()));
        assert_eq!(b64_decode("a b"), None);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(inspect_commands(&s(&["G3", "G; rm"]), &s(&["P2"]), true), vec!["rm G3", "probe rm P2", "continue all", "resume"]);
        assert!(inspect_commands(&[], &[], false).is_empty());
    }

    /// The recovery decision on a real worktree of a bare store: clean = nothing, dirty and
    /// unique commits = a bundle that holds the work.
    #[test]
    fn recovery_bundles_unsaved_work_only() {
        assert!(!needs_recovery(false, false) && needs_recovery(true, false) && needs_recovery(false, true));
        let base = std::env::temp_dir().join(format!("devhub-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let g = |d: &Path, a: &[&str]| git(Some(d), None, a).unwrap();
        g(&src, &["init", "-q", "-b", "work"]);
        std::fs::write(src.join("a"), "a").unwrap();
        g(&src, &["add", "-A"]);
        g(&src, &["-c", "user.name=t", "-c", "user.email=t@localhost", "commit", "-q", "-m", "a"]);
        let store = base.join("store.git");
        g(&base, &["clone", "-q", "--bare", &src.to_string_lossy(), &store.to_string_lossy()]);
        let sha = g(&src, &["rev-parse", "HEAD"]).trim().to_string();
        let out = base.join("recovered");
        let mk = |name: &str| {
            let p = base.join(name);
            git(None, Some(&store), &["worktree", "add", "--detach", "-q", &p.to_string_lossy(), &sha]).unwrap();
            p
        };
        let clean = mk("clean");
        assert_eq!(recover_worktree(&clean, None, "L1", &out, "x-L1").unwrap(), None);
        let dirty_wt = mk("dirty");
        std::fs::write(dirty_wt.join("b"), "b").unwrap();
        let b = recover_worktree(&dirty_wt, None, "L2", &out, "x-L2").unwrap().unwrap();
        assert!(g(&dirty_wt, &["log", "-1", "--format=%s"]).contains("WIP devhub recovery L2"));
        // the bundle fetches into the lane's repo (it has the base) and holds the file
        g(&src, &["fetch", "-q", &b.to_string_lossy(), "HEAD:refs/heads/devhub/recovered/x-L2"]);
        assert_eq!(g(&src, &["show", "devhub/recovered/x-L2:b"]), "b");
        let ahead = mk("ahead");
        std::fs::write(ahead.join("c"), "c").unwrap();
        g(&ahead, &["add", "-A"]);
        g(&ahead, &["-c", "user.name=t", "-c", "user.email=t@localhost", "commit", "-q", "-m", "c"]);
        assert!(recover_worktree(&ahead, None, "L3", &out, "x-L3").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn round_trip_run_log_end() {
        let (x, base, events) = test_exec("rt", Opts { grace: Duration::from_secs(10), ..Opts::default() });
        let a = x.handle("lease-start", r#"{"id":"L1","lane":"gapx","kind":"test","rev":null,"env":{"FOO":"bar","CARGO_INCREMENTAL":"1"}}"#);
        assert_eq!(field(&a[0], "ok").and_then(|v| super::super::num(&v)), Some(1.0), "{a:?}");
        let work = PathBuf::from(field(&a[0], "work").unwrap().as_str().unwrap());
        let tmp = PathBuf::from(field(&a[0], "tmp").unwrap().as_str().unwrap());
        assert!(work.ends_with("devhub/lane/gapx/src") && work.is_dir() && tmp.is_dir());
        assert!(a[0].contains("\"CARGO_INCREMENTAL\":\"0\"") && a[0].contains("\"FOO\":\"bar\""));
        assert!(base.join("leases/L1.json").exists());
        // the same start again answers the same
        assert_eq!(x.handle("lease-start", r#"{"id":"L1","lane":"gapx","kind":"test","rev":null}"#), a);

        let r = x.handle("lease-run", &format!("L1 . {}", b64(&["/bin/echo", "hi"])));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
        let (text, code) = log_until_exit(&x, "L1", n);
        assert_eq!((text.trim(), code), ("hi", 0));
        // the env reaches the program; a failing one reports its code
        let r = x.handle("lease-run", &format!("lease-run L1 . {}", b64(&["/usr/bin/env"])));
        let (env, _) = log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32);
        assert!(env.contains("FOO=bar") && env.contains("SANDBOX_MUTE=1") && env.contains(&format!("TMPDIR={}", tmp.display())));
        let r = x.handle("lease-run", &format!("L1 . {}", b64(&["false"])));
        assert_eq!(log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32).1, 1);
        assert!(x.handle("lease-run", &format!("L1 ../x {}", b64(&["true"])))[0].contains("err"));

        // a sleeping child (and its own child) is stopped on end
        let r = x.handle("lease-run", &format!("L1 . {}", b64(&["/bin/sleep", "300"])));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = proc_start(pid).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(group_alive(pid, start));
        let beat = x.handle("lease-beat", "L1 L404");
        assert_eq!(beat[0], r#"{"live":["L1"],"unknown":["L404"]}"#);
        let e = x.handle("lease-end", "L1 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")) && e[0].contains("\"leak\":[]"), "{e:?}");
        assert!(!group_alive(pid, start));
        assert!(!tmp.exists() && !base.join("leases/L1.json").exists());
        assert!(work.exists(), "a lane dir stays warm");
        assert!(x.handle("lease-end", "L1 released")[0].contains("\"unknown\":true"));
        assert!(events.lock().unwrap().iter().any(|l| l.contains("ws_create")) && events.lock().unwrap().iter().any(|l| l.contains("ended on t (released)")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn watchdog_ends_a_lease_without_beats() {
        let (x, base, events) = test_exec("wd", Opts { no_beat: Duration::from_millis(600), tick: Duration::from_millis(100), grace: Duration::from_secs(10), scopes: None });
        let a = x.handle("lease-start", r#"{"id":"L7","lane":"lost","kind":"long","rev":null}"#);
        assert!(a[0].contains("\"ok\":1"));
        let r = x.handle("lease-run", &format!("L7 . {}", b64(&["/bin/sleep", "300"])));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = proc_start(pid).unwrap();
        // beaten: it stays
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(100));
            x.handle("lease-beat", "L7");
        }
        assert!(x.leases.lock().unwrap().contains_key("L7"));
        // not beaten: the node ends it and stops its process
        let t = Instant::now();
        while x.leases.lock().unwrap().contains_key("L7") || group_alive(pid, start) {
            assert!(t.elapsed() < Duration::from_secs(20), "the watchdog did not end L7");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(events.lock().unwrap().iter().any(|l| l.contains("no beat from the master")));
        // dev off refuses write kinds, not inspect
        assert_eq!(x.handle("policy", "dev=off"), vec![r#"{"dev":"off"}"#.to_string()]);
        assert!(x.handle("lease-start", r#"{"id":"L8","lane":"a","kind":"build","rev":null}"#)[0].contains("dev off"));
        assert!(x.handle("lease-start", r#"{"id":"L9","lane":"a","kind":"inspect","rev":null}"#)[0].contains("\"ok\":1"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_restarted_hub_takes_its_leases_back() {
        let (x, base, _) = test_exec("ledger", Opts::default());
        x.handle("lease-start", r#"{"id":"L3","lane":"b","kind":"screen","rev":null}"#);
        let r = x.handle("lease-run", &format!("L3 . {}", b64(&["/bin/sleep", "300"])));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let conf = x.conf.clone();
        let y = Exec::new("t", conf, base.join("leases"), Opts::default(), Arc::new(|_| {}));
        assert!(y.handle("lease-beat", "L3")[0].contains("\"live\":[\"L3\"]"));
        let e = y.handle("lease-end", "L3 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")), "{e:?}");
        assert!(!group_alive(pid, proc_start(pid).unwrap_or(1)));
        let _ = std::fs::remove_dir_all(&base);
    }
}
