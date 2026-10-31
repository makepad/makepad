//! The node side of leases: `lease-start`, `lease-run`, `lease-log`, `lease-end`, `lease-beat`,
//! `policy`, `caps`, `disk`, `recover-get` / `recover-del` on the node's master endpoint
//! (LEASE.SPEC.md "Wire").
//!
//! - A lease gets a workspace: with a rev, the node's ONE tree ([`super::tree`]: its synced
//!   checkout, moved in place to the pinned commits) and its one target `<tree>/target` (one JIT
//!   binary and one JIT cache per node); without, `<work>/lane/<lane>/src` and
//!   `<work>/target/lane-<lane>`. Its scratch is `<work>/tmp/<id>` (TMPDIR, run logs).
//! - A run is the argv as given (no shell), in its own session (setsid), under
//!   `systemd-run --user --scope` when the user bus exists, `nice -n 10`, optionally pinned
//!   (`cpus.*` in lease.conf). Its pid and start time go into the ledger
//!   (`~/.makepad/devhub/leases/<id>.json`), so a restarted hub still ends what it started.
//! - End (release, master expiry, or 60 s without `lease-beat`: the node's own watchdog): TERM the
//!   recorded process groups whose leader's start time still matches, 10 s grace, KILL, stop the
//!   scopes; a survivor is a `lease_leak`. The last lease on the tree that leaves changes in it:
//!   they are committed (`WIP devhub recovery <id>`), bundled to `<work>/recovered/` and the tree
//!   goes back to the pin; never `git stash`. The tree itself stays (warm). Nothing ends for
//!   running long.
//! - An inspect lease's end removes its breakpoints (`rm G<n>`) and probes (`probe rm P<n>`),
//!   lets parked hits go (`continue all`) and resumes a pause (`resume`), over the JIT's socket.

use super::caps::{self, LeaseConf};
use super::disk::{self, Guard, Live, Roots};
use super::Kind;
use crate::local::Publish;
use crate::os;
use crate::proto::{hub_record, Sev};
use makepad_remote::json_str;
use makepad_remote::{lines, Endpoint};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The services this module adds to the node's master endpoint.
pub const WORDS: &[&str] = &["lease-start", "lease-run", "lease-log", "lease-end", "lease-beat", "ship-have", "ship-put", "ship-link", "ship-done", "lease-owns", "hub-restart", "service-restart", "get-ls", "get-read", "policy", "lease-conf", "caps", "disk", "recover-get", "recover-del"];

/// Marks every process a lease's runs start ([`lease_mark`]): a process that moved to its own session (a
/// WM's apps, a detached helper) is still stopped with the lease. One that outlives its lease on
/// purpose (the check server) removes it.
const LEASE_ENV: &str = "MAKEPAD_DEVHUB_LEASE";
const LOG_CHUNK: usize = 64 << 10;
/// The value of [`LEASE_ENV`]: this hub's ledger dir and the lease (the same after a hub restart).
fn lease_mark(state: &Path, id: &str) -> String {
    format!("{}/{id}", state.display())
}
const RECOVER_CHUNK: usize = 256 << 10;
/// `get-read` chunk and `get-ls` limit (results, not trees: big artifacts stay on the node)
const GET_CHUNK: usize = 256 << 10;
const GET_MAX_FILES: usize = 20_000;

/// Timing knobs (tests shorten them) and the node's tree.
#[derive(Clone, Debug)]
pub struct Opts {
    /// the node's one tree ([`super::tree::root`]); None: no rev leases (tests, JIT hosts)
    pub tree: Option<PathBuf>,
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
        Opts { tree: None, no_beat: Duration::from_secs(60), tick: Duration::from_secs(5), grace: Duration::from_secs(10), scopes: None }
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
    /// what keeps its processes together (Windows: its job)
    hold: os::Hold,
    /// where its exit code goes when it ends ([`exit_file`])
    exit_file: PathBuf,
}

struct Lease {
    id: String,
    lane: String,
    kind: Kind,
    work: PathBuf,
    target: PathBuf,
    tmp: PathBuf,
    env: Vec<(String, String)>,
    /// the commits it holds the node's tree at (`<makepad>[,<commercial>]`, [`super::tree::Pin`])
    pin: Option<String>,
    /// what `devhub run` shipped (and deleted) on top of the pin: undone at the end
    overlay: Vec<String>,
    runs: Vec<Arc<Run>>,
    beat: Instant,
}

impl Lease {
    fn answer(&self) -> String {
        let env: Vec<String> = self.env.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        format!(
            "{{\"ok\":1,\"user\":{},\"work\":{},\"target\":{},\"tmp\":{},\"env\":{{{}}}}}",
            json_str(&login()),
            json_str(&self.work.to_string_lossy()),
            json_str(&self.target.to_string_lossy()),
            json_str(&self.tmp.to_string_lossy()),
            env.join(",")
        )
    }

    fn ledger(&self) -> String {
        let runs: Vec<String> = self
            .runs
            .iter()
            .map(|r| format!("{{\"n\":{},\"pid\":{},\"start\":{},\"unit\":{},\"log\":{}}}", r.n, r.pid, r.start, r.unit.as_deref().map_or("null".into(), json_str), json_str(&r.log.to_string_lossy())))
            .collect();
        let a = self.answer();
        format!("{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"pin\":{},\"overlay\":[{}],\"runs\":[{}],{}", json_str(&self.id), json_str(&self.lane), self.kind.as_str(), self.pin.as_deref().map_or("null".into(), json_str), self.overlay.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), runs.join(","), &a[1..])
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
        let runs = v.get("runs").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| {
            a.iter()
                .filter_map(|r| Some(Arc::new(Run { n: n(r, "n") as u32, pid: n(r, "pid") as u32, start: n(r, "start"), unit: s(r, "unit"), log: s(r, "log")?.into(), exit: Mutex::new(None), ours: false, hold: os::Hold::none(), exit_file: PathBuf::new() })))
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
            pin: s(&v, "pin"),
            overlay: v.get("overlay").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()),
            runs,
            beat: Instant::now(),
        })
    }
}

/// The node's lease executor.
pub struct Exec {
    pub machine: String,
    publish: Publish,
    conf: std::sync::RwLock<LeaseConf>,
    /// the ledger folder (`~/.makepad/devhub/leases`)
    state: PathBuf,
    opts: Opts,
    scopes: bool,
    dev: AtomicBool,
    /// a flip asked for on this node itself, until the master's policy push answers it (0 none, 1 on, 2 off)
    dev_req: std::sync::atomic::AtomicU8,
    leases: Mutex<BTreeMap<String, Lease>>,
    pub guard: Guard,
    /// lease id -> (when measured, MB in its work + target + tmp dirs): measured off the beat
    disk_use: Arc<Mutex<std::collections::HashMap<String, (Instant, u64)>>>,
    /// a person at the node, as its WM saw them ([`super::user`])
    user: Mutex<super::user::UserTrack>,
    /// `hub-restart` was answered: this hub exits in a moment and starts no run before it does
    restarting: AtomicBool,
}

/// What a lease run inherits from the hub's environment: the system's basics (paths, user,
/// locale, toolchain homes, the user bus `systemd-run --user` needs, Windows' system vars). The
/// WM session's (`MAKEPAD_WM_*`, the display) only for a screen lease: the kind that shows
/// windows opts in; every other run must not reach the node's live WM.
pub fn base_env(kind: Kind, vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LANGUAGE", "TERM", "TZ", "CARGO_HOME", "RUSTUP_HOME", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS",
        // platform SDKs (check_platforms, Android and Apple builds)
        "ANDROID_HOME", "ANDROID_SDK_ROOT", "ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "JAVA_HOME", "DEVELOPER_DIR", "SDKROOT",
        // Windows
        "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "USERPROFILE", "USERNAME", "APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432", "COMMONPROGRAMFILES", "COMSPEC", "PATHEXT", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "OS", "SYSTEMDRIVE", "HOMEDRIVE", "HOMEPATH",
    ];
    const SCREEN: &[&str] = &["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"];
    vars.filter(|(k, _)| {
        let up = k.to_ascii_uppercase();
        KEEP.contains(&up.as_str()) || up.starts_with("LC_") || (kind == Kind::Screen && (SCREEN.contains(&up.as_str()) || up.starts_with("MAKEPAD_WM_")))
    })
    .collect()
}

/// Lease ids, lanes, env keys: what a path, a unit name and a line can hold.
fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with(['-', '.']) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') && !s.contains("..")
}

/// Where a run's exit code is written when it ends: beside the ledger, not in the lease's tmp (a
/// run stopped by the lease's end would write into the tmp being removed).
fn exit_file(state: &Path, id: &str, n: u32) -> PathBuf {
    state.join(format!("{id}-{n}.exit"))
}

impl Run {
    /// The exit the waiter wrote (a run of an earlier hub process).
    fn recorded_exit(&self) -> Option<i32> {
        std::fs::read_to_string(&self.exit_file).ok()?.trim().parse().ok()
    }

    /// Still running: no exit known, and (not ours) its group alive.
    fn live(&self) -> bool {
        self.exit.lock().unwrap().is_none() && (self.ours || (self.recorded_exit().is_none() && os::group_alive(self.pid, self.start, &self.hold)))
    }
}

fn err(e: impl AsRef<str>) -> String {
    format!("{{\"err\":{}}}", json_str(e.as_ref()))
}

/// This node's login (what `devhub run` / `get` ship with over ssh).
fn login() -> String {
    os::login().filter(|u| valid_name(u)).or_else(|| caps::home_dir().file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default()
}

/// A relative path that stays inside its root: no `..`, not absolute, not empty.
/// Removes `rel`'s parent dirs under `work` that are empty, deepest first, stopping at the first
/// one that is not (`remove_dir` fails on a non-empty dir, so a `target/` or any untracked content
/// keeps its dirs) and never touching `work` itself.
fn prune_empty_parents(work: &Path, rel: &str) {
    let mut p = Path::new(rel);
    while let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        let full = work.join(parent);
        if !std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_dir()) || std::fs::remove_dir(&full).is_err() {
            break;
        }
        p = parent;
    }
}

fn inside(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty() && !p.is_absolute() && p.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir))
}

/// A relative path with `/` between its parts (what the lane's side joins, whatever the node's OS).
fn slashed(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Files under `p` (itself when it is one), as paths relative to `base`, at most `max`.
fn files_under(base: &Path, p: &Path, max: usize, out: &mut Vec<(String, u64)>) -> Result<(), String> {
    let md = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if md.is_dir() {
        let mut kids: Vec<PathBuf> = std::fs::read_dir(p).map_err(|e| format!("{}: {e}", p.display()))?.flatten().map(|e| e.path()).collect();
        kids.sort();
        for k in kids {
            files_under(base, &k, max, out)?;
        }
    } else if md.is_file() {
        if out.len() >= max {
            return Err(format!("more than {max} files: get a narrower path"));
        }
        out.push((slashed(p.strip_prefix(base).unwrap_or(p)), md.len()));
    }
    Ok(())
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
        let x = Arc::new(Exec { machine: machine.to_string(), publish, dev: AtomicBool::new(true), dev_req: Default::default(), conf: std::sync::RwLock::new(conf), state, opts, scopes, leases: Mutex::new(BTreeMap::new()), guard, disk_use: Default::default(), user: Default::default(), restarting: AtomicBool::new(false) });
        let mut back = Vec::new();
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(mut l) = std::fs::read_to_string(&p).ok().and_then(|t| Lease::from_ledger(&t)) {
                    for r in &mut l.runs {
                        if let Some(run) = Arc::get_mut(r) {
                            run.exit_file = exit_file(&x.state, &l.id, run.n);
                        }
                    }
                    back.push(l.id.clone());
                    #[cfg(feature = "checkout")]
                    if let Some(pin) = l.pin.as_deref().and_then(super::Pin::parse) {
                        super::tree::hold(&l.id, &pin);
                    }
                    x.leases.lock().unwrap().insert(l.id.clone(), l);
                }
            }
        }
        // the exit files of leases this hub no longer has
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if p.extension().is_some_and(|e| e == "exit") && !back.iter().any(|b| id.split('-').next() == Some(b.as_str())) {
                let _ = std::fs::remove_file(&p);
            }
        }
        // quiet marks of leases this hub no longer has (it stopped while one ran)
        for e in std::fs::read_dir(x.quiet()).into_iter().flatten().flatten() {
            let p = e.path();
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if p.extension().is_some_and(|e| e == "cmd") && !back.contains(&id) {
                let _ = std::fs::remove_file(&p);
            }
        }
        if !back.is_empty() {
            x.emit("lease", Sev::Info, format!("lease: hub on {} took back {} lease(s) from its ledger: {}", x.machine, back.len(), back.join(" ")));
        }
        // a tree no lease holds sits on the shared commits (a hub that died mid-lease left it elsewhere)
        #[cfg(feature = "checkout")]
        if let Some(root) = &x.opts.tree {
            let r = super::tree::restore_if_free(root, &x.conf().work.join("recovered"), x.conf().role.users());
            if let Some(to) = &r.restored {
                x.emit("ws", Sev::Warn, format!("ws_restore: {}: no lease held the tree, back at the shared {to}", x.machine));
            }
            x.report_tree(&format!("{} starting", x.machine), &r);
        }
        let w = Arc::downgrade(&x);
        let tick = x.opts.tick;
        let _ = std::thread::Builder::new().name("devhub-lease-watchdog".into()).spawn(move || watchdog(w, tick));
        let w = Arc::downgrade(&x);
        let _ = std::thread::Builder::new().name("devhub-disk".into()).spawn(move || disk_loop(w));
        x
    }

    /// The folder a timed lease marks while it lives (`<id>.cmd`), next to the ledger
    /// (`~/.makepad/devhub/quiet`): other tenants of the machine (the mini's CI: `yield_to` in its
    /// ci.toml) pause while it holds one.
    fn quiet(&self) -> PathBuf {
        self.state.parent().unwrap_or(&self.state).join("quiet")
    }

    /// The lease policy now (`lease-conf set` changes it while the hub runs).
    pub fn conf(&self) -> LeaseConf {
        self.conf.read().unwrap().clone()
    }

    /// `lease-conf` (show) or `lease-conf set k=v ...`: the master sets this node's role and
    /// budgets; written to `lease.conf` (kept across restarts) and in force at once.
    fn lease_conf(&self, rest: &str) -> String {
        let mut w = rest.split_whitespace();
        if w.next() == Some("set") {
            let pairs: Vec<(String, String)> = w.filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
            if pairs.is_empty() {
                return err("lease-conf set <key=value>...");
            }
            for (k, v) in &pairs {
                if let Err(e) = caps::settable(k, v) {
                    return err(e);
                }
            }
            let path = caps::conf_path();
            let text = caps::conf_with(&std::fs::read_to_string(&path).unwrap_or_default(), &pairs);
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
            let tmp = path.with_extension("conf.new");
            if let Err(e) = std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &path)) {
                return err(format!("{}: {e}", path.display()));
            }
            let conf = LeaseConf::parse(&text, &caps::home_dir());
            self.guard.set_policy(conf.disk_floor_gb, conf.disk_warn_gb, conf.role.users());
            *self.conf.write().unwrap() = conf;
            let what: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
            self.emit("lease", Sev::Info, format!("lease: {} lease.conf set {}", self.machine, what.join(" ")));
        }
        let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
        let mut c = caps::measure(&self.machine, &self.conf(), apply);
        c.tree = self.tree_key();
        c.to_json()
    }

    /// One look at the person: the WM's answer (`Err`: no WM of ours), its flips as events.
    pub fn sense_user(&self, answer: Result<&str, String>) {
        let idle_min = self.conf().user_idle_min;
        let (flip, sense) = {
            let mut t = self.user.lock().unwrap();
            let flip = t.update(crate::proto::wall_ms(), answer, idle_min);
            (flip, t.sense.clone().unwrap_or_default())
        };
        let dev = self.dev.load(Ordering::Acquire);
        let running = self.leases.lock().unwrap().values().filter(|l| l.kind.writes()).count();
        match flip {
            Some(super::user::Flip::Present) => self.emit(
                "lease",
                Sev::Info,
                format!(
                    "node_user_present: {}: a person uses it ({}): {}",
                    self.machine,
                    sense.what,
                    if !dev { "its dev switch is off anyway".to_string() } else if running > 0 { format!("no new leases, {running} running finish") } else { "no new leases".to_string() }
                ),
            ),
            Some(super::user::Flip::Idle) => self.emit(
                "lease",
                Sev::Info,
                format!("node_user_idle: {}: no input for {idle_min} min: {}", self.machine, if dev { "takes leases again" } else { "its dev switch stays off (the user's)" }),
            ),
            None => {}
        }
    }

    /// The commits the node's tree holds (`caps`'s `tree`; empty: no tree, or unknown).
    fn tree_key(&self) -> String {
        #[cfg(feature = "checkout")]
        if let Some(root) = &self.opts.tree {
            return super::tree::current(root).map_or(String::new(), |p| p.key());
        }
        String::new()
    }

    fn emit(&self, kind: &str, sev: Sev, line: String) {
        (self.publish)(hub_record(&self.machine, "", 0, kind, sev, line, String::new()));
    }

    /// [`Exec::emit`] for a background thread.
    fn emitter(&self) -> impl Fn(&str, Sev, String) + Send + 'static {
        let (publish, machine) = (self.publish.clone(), self.machine.clone());
        move |kind: &str, sev: Sev, line: String| publish(hub_record(&machine, "", 0, kind, sev, line, String::new()))
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
            // the hub exits for its self-update in a moment: the holder tries again on the new one
            "lease-start" | "lease-run" if self.restarting.load(Ordering::Acquire) => vec![err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine))],
            "lease-start" => vec![self.start(rest)],
            "lease-run" => vec![self.run(rest)],
            "lease-log" => vec![self.log(rest)],
            "lease-end" => vec![self.end_cmd(rest)],
            "lease-beat" => vec![self.beat(rest)],
            "ship-done" => vec![self.ship_done(rest)],
            "lease-owns" => vec![self.owns(rest)],
            "hub-restart" => vec![self.hub_restart()],
            "service-restart" => vec![self.service_restart(rest)],
            "ship-have" | "ship-put" | "ship-link" => vec![self.ship(word, rest)],
            "get-ls" => self.get_ls(rest),
            "get-read" => vec![self.get_read(rest)],
            "policy" => vec![self.policy(rest)],
            "lease-conf" => vec![self.lease_conf(rest)],
            "caps" => {
                let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
                let mut c = caps::measure(&self.machine, &self.conf(), apply);
                c.desk_link = crate::embed::desk_link_state();
                c.dev_req = match self.dev_req.load(Ordering::Acquire) { 1 => Some(true), 2 => Some(false), _ => None };
                c.tree = self.tree_key();
                c.user = self.user.lock().unwrap().sense.clone();
                vec![c.to_json()]
            }
            "disk" => match rest.strip_prefix("clean ") {
                Some(lane) => vec![self.clean_lane(lane.trim())],
                None => {
                    let need_gb: f64 = rest.strip_prefix("need ").and_then(|n| n.trim().parse().ok()).unwrap_or(0.0);
                    self.guard.reclaim(&self.live(), rest == "dry" || rest == "--dry-run", (need_gb * (1u64 << 30) as f64) as u64)
                }
            },
            "recover-get" => vec![self.recover_get(rest)],
            "recover-del" => vec![self.recover_del(rest)],
            _ => vec![err(format!("unknown service {word}"))],
        }
    }

    fn policy(&self, rest: &str) -> String {
        match rest {
            "dev=on" => {
                self.dev.store(true, Ordering::Release);
                self.dev_req.store(0, Ordering::Release);
            }
            "dev=off" => {
                self.dev.store(false, Ordering::Release);
                self.dev_req.store(0, Ordering::Release);
            }
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
        // a real lease preempts the background pre-warm at once (its processes are gone before
        // the lease takes the tree, its JIT cache consistent)
        #[cfg(feature = "checkout")]
        if kind.writes() {
            crate::sync::prewarm::cancel(&format!("lease {id} ({lane})"));
        }
        let rev = v.get("rev").filter(|r| r.get("makepad").is_some());
        let rev_of = |k: &str| rev.and_then(|r| r.get(k)).and_then(|x| x.as_str()).map(str::to_string);
        // the tree is the node's own (its WM runs from it): only a write lease on a dev-on node moves it
        if rev.is_some() && !kind.writes() {
            return err(format!("lease-start: an {} lease never moves the node's tree (no rev)", kind.as_str()));
        }
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(makepad_strict_json::Value::Obj(o)) = v.get("env") {
            for (k, x) in o {
                match x.as_str() {
                    Some(x) if valid_name(k) && !x.contains('\0') => env.push((k.to_string(), x.to_string())),
                    _ => return err(format!("lease-start: env {k}")),
                }
            }
        }
        let conf = self.conf();
        let w = &conf.work;
        let tmp = w.join("tmp").join(&id);
        let (work, target, pin, what) = match rev_of("makepad") {
            Some(sha) => match self.take_tree(&id, &sha, rev_of("commercial").as_deref()) {
                Ok(x) => x,
                Err(e) => return err(format!("lease-start {id}: {e}")),
            },
            None => (w.join("lane").join(&lane).join("src"), w.join("target").join(format!("lane-{lane}")), None, "lane dir".to_string()),
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
            // every run is the lane's test instance: badged `TEST: lane <lane>` in its caption,
            // never the WM dock's app, off the LAN (XR, printer, rooms) unless it asks
            // (MAKEPAD_LAN=1 in its own command)
            ("MAKEPAD_TEST_LANE", lane.clone()),
            // what a run prints as its verdict names its own lease (a lane that reads a gate's
            // NODEGATE-OK from a file shared with another lane's run must tell them apart)
            ("DEVHUB_LEASE", id.clone()),
            // one JIT session per lease: its many check/test/present calls share one JIT setup
            ("MAKEPAD_JIT_SESSION", id.clone()),
            ("CARGO_NET_OFFLINE", "true".into()),
        ];
        // the lease's own environment wins over what was asked
        env.retain(|(k, _)| !law.iter().any(|(l, _)| l == k));
        env.extend(law.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let l = Lease { id: id.clone(), lane: lane.clone(), kind, work: work.clone(), target, tmp, env, pin, overlay: Vec::new(), runs: Vec::new(), beat: Instant::now() };
        let a = l.answer();
        self.save(&l);
        if kind == Kind::Timed {
            let _ = std::fs::create_dir_all(self.quiet());
            let _ = std::fs::write(self.quiet().join(format!("{id}.cmd")), format!("timed lease {id} of {lane}\n"));
        }
        self.leases.lock().unwrap().insert(id.clone(), l);
        self.guard.touch();
        self.emit("ws", Sev::Info, format!("ws_create: {id} {} for {lane} on {}: {what} {}", kind.as_str(), self.machine, work.display()));
        a
    }

    /// A rev lease takes the node's one tree at its commits (moved in place; refused while
    /// other commits hold it): (work, target, pin, what for the event).
    #[cfg(feature = "checkout")]
    fn take_tree(&self, id: &str, sha: &str, commercial: Option<&str>) -> Result<(PathBuf, PathBuf, Option<String>, String), String> {
        let pin = super::Pin::parse(&match commercial {
            Some(c) => format!("{sha},{c}"),
            None => sha.to_string(),
        })
        .ok_or("rev: a 40-hex commit")?;
        let root = self.opts.tree.as_ref().ok_or("this node runs no tree for rev leases")?;
        let t = super::tree::take(id, &pin, root, &self.conf().work.join("recovered"), self.conf().role.users())?;
        self.report_tree(&format!("{id} on {}, before its start", self.machine), &t.cleared);
        let moved: Vec<String> = t.moved.iter().map(|(r, n)| format!("{r} {n} files")).collect();
        let what = format!("tree at {} ({})", &sha[..12], if t.moved.iter().all(|(_, n)| *n == 0) { "there already".to_string() } else { format!("moved: {}", moved.join(", ")) });
        Ok((t.root.clone(), t.root.join("target"), Some(pin.key()), what))
    }

    #[cfg(not(feature = "checkout"))]
    fn take_tree(&self, _id: &str, _sha: &str, _commercial: Option<&str>) -> Result<(PathBuf, PathBuf, Option<String>, String), String> {
        Err("this host has no git stores: leases with a rev need a desktop node".into())
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
        let conf = self.conf();
        let cpus = if l.kind == Kind::Timed { &conf.cpus_timed } else { &conf.cpus_shared };
        let full = os::lease_argv(argv, unit.as_deref(), cpus);
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
        // a clean base, never the hub's whole environment (on a WM node that is the WM session's:
        // a run would talk to the live WM); then the lease's own
        cmd.env_clear();
        for (k, v) in base_env(l.kind, std::env::vars()) {
            cmd.env(k, v);
        }
        for (k, v) in &l.env {
            cmd.env(k, v);
        }
        // what it starts carries this, in whatever session: the lease's end finds it by this
        cmd.env(LEASE_ENV, lease_mark(&self.state, id));
        // its JITs announce where this hub looks, whatever the lease's TMPDIR
        cmd.env("MAKEPAD_REMOTE_REGISTRY", makepad_remote::discovery::registry_dir());
        let (mut child, hold) = match os::lease_spawn(&mut cmd) {
            Ok(c) => c,
            Err(e) => return err(format!("lease-run {id}: {}: {e}", full[0])),
        };
        let pid = child.id();
        let run = Arc::new(Run { n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log, exit: Mutex::new(None), ours: true, hold, exit_file: exit_file(&self.state, id, n) });
        l.runs.push(run.clone());
        self.save(l);
        drop(leases);
        let _ = std::thread::Builder::new().name("devhub-lease-wait".into()).spawn(move || {
            let code = child.wait().map(os::exit_code).unwrap_or(-1);
            // beside its log too: a hub that restarts before the holder read it still says it
            let _ = std::fs::write(&run.exit_file, code.to_string());
            *run.exit.lock().unwrap() = Some(code);
        });
        format!("{{\"run\":{pid},\"n\":{n}}}")
    }

    /// `hub-restart`: the self-update, at a run boundary: refused (`waits:`) while a lease's run
    /// is in flight (its exit would be lost: Windows' job ends with the hub, and a process this hub
    /// does not parent has no exit code for the next). Leases between runs stay: the new hub takes
    /// them back from the ledger (pins, overlays, finished runs' exits), and their holders ride
    /// through the restart. Else this hub exits a moment after answering, starting no run before,
    /// and what keeps it (the WM's supervisor, the Mac's LaunchAgent) starts it again through
    /// `cargo run` in its tree: the new code.
    fn hub_restart(&self) -> String {
        let leases = self.leases.lock().unwrap();
        let live: Vec<String> = leases.values().filter(|l| l.runs.iter().any(|r| r.live())).map(|l| l.id.clone()).collect();
        if !live.is_empty() {
            return err(format!("waits: a run of {} is in flight on {} (the hub restarts at its end)", live.join(" "), self.machine));
        }
        #[cfg(feature = "checkout")]
        if crate::sync::serve::applying() {
            return err(format!("waits: a sync runs on {} (the hub restarts after it)", self.machine));
        }
        self.restarting.store(true, Ordering::Release);
        drop(leases);
        self.emit("hub", Sev::Info, format!("hub: {} restarts for the self-update (was at {})", self.machine, &super::caps::hub_rev()[..super::caps::hub_rev().len().min(10)]));
        let _ = std::thread::Builder::new().name("devhub-hub-restart".into()).spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            std::process::exit(0);
        });
        format!("{{\"restarting\":{}}}", json_str(&self.machine))
    }

    /// `service-restart <name>`: the self-update of a service the WM supervises (`ai-hub`): it
    /// gets SIGTERM (it closes admission and hands its jobs back), and the WM starts it again
    /// through `cargo run` in the tree: the new code.
    fn service_restart(&self, name: &str) -> String {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') || name == "devhub" {
            return err("usage: service-restart <name of a WM-supervised service> (devhub: hub-restart)");
        }
        let state = makepad_network::fleet::makepad_home().join("wm").join(format!("{name}.state"));
        let pid = std::fs::read_to_string(&state).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("pid = ").and_then(|p| p.trim().parse::<u32>().ok())));
        let Some(pid) = pid.filter(|p| crate::procinfo::alive(*p)) else { return err(format!("{name} does not run on {}", self.machine)) };
        // a user-facing service never restarts under its user (the master's roll waits and asks again)
        let blockers = user_blockers();
        if !blockers.is_empty() {
            return err(format!("waits: {}", blockers.join("; ")));
        }
        // kill by the pid its supervisor recorded
        let ok = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status().is_ok_and(|s| s.success());
        if !ok {
            return err(format!("{name} (pid {pid}): kill -TERM failed"));
        }
        self.emit("hub", Sev::Info, format!("hub: {} restarts {name} (pid {pid}) for the self-update", self.machine));
        format!("{{\"restarting\":{},\"pid\":{pid}}}", json_str(name))
    }

    /// `lease-owns <id> <pid>`: whether the process is one of the lease's runs (a JIT a job
    /// started: the master pauses that job on its parked panic).
    fn owns(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let (Some(id), Some(Ok(pid))) = (w.first(), w.get(1).map(|p| p.parse::<u32>())) else { return err("usage: lease-owns <id> <pid>") };
        let leases = self.leases.lock().unwrap();
        let Some(l) = leases.get(*id) else { return err(format!("no lease {id} on {}", self.machine)) };
        let owns = l.runs.iter().any(|r| r.pid == pid || os::in_group(pid, r.pid, &r.hold));
        format!("{{\"owns\":{owns}}}")
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
        let exit = exit.or_else(|| (!run.ours).then(|| run.recorded_exit().or_else(|| (!os::group_alive(run.pid, run.start, &run.hold)).then_some(-1))).flatten());
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
        let (mut live, mut unknown, mut info) = (Vec::new(), Vec::new(), Vec::new());
        for id in rest.split_whitespace() {
            match leases.get_mut(id) {
                Some(l) => {
                    l.beat = Instant::now();
                    live.push(json_str(id));
                    // what it runs now (its newest live process) and its disk use, for the panel
                    let running = l.runs.iter().rev().find(|r| os::proc_start(r.pid) == Some(r.start)).and_then(|r| crate::top::label(r.pid).map(|c| (r.pid, c)));
                    let dirs = vec![l.work.clone(), l.target.clone(), l.tmp.clone()];
                    let mb = self.disk_of(id, dirs);
                    info.push(format!(
                        "{}:{{\"running\":{},\"pid\":{},\"work\":{},\"disk_mb\":{}}}",
                        json_str(id),
                        running.as_ref().map_or("null".into(), |(_, c)| json_str(c)),
                        running.as_ref().map_or(0, |(p, _)| *p),
                        json_str(&l.work.to_string_lossy()),
                        mb.map_or("null".into(), |m| m.to_string())
                    ));
                }
                None => unknown.push(json_str(id)),
            }
        }
        format!("{{\"live\":[{}],\"unknown\":[{}],\"info\":{{{}}}}}", live.join(","), unknown.join(","), info.join(","))
    }

    /// A lease's disk use in MB, measured at most every 2 min on a thread of its own (a target
    /// dir walk can take seconds); None until the first measure.
    fn disk_of(&self, id: &str, dirs: Vec<PathBuf>) -> Option<u64> {
        let mut m = self.disk_use.lock().unwrap();
        let known = m.get(id).copied();
        if known.is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(120)) {
            // mark it measured now, so one walk runs at a time
            m.insert(id.to_string(), (Instant::now(), known.map_or(u64::MAX, |k| k.1)));
            let (map, id) = (self.disk_use.clone(), id.to_string());
            let _ = std::thread::Builder::new().name("devhub-lease-du".into()).spawn(move || {
                let mut seen = std::collections::HashSet::new();
                let bytes: u64 = dirs.iter().filter(|d| seen.insert((*d).clone())).map(|d| super::disk::dir_size(d).0).sum();
                map.lock().unwrap().insert(id, (Instant::now(), bytes >> 20));
            });
        }
        known.map(|k| k.1).filter(|m| *m != u64::MAX)
    }

    /// `lease-end <id> <how> [{"inspect":{"pid":N,"breaks":[..],"probes":[..],"paused":true}}]`
    fn end_cmd(&self, rest: &str) -> String {
        let mut w = rest.splitn(3, ' ');
        let (id, how, extra) = (w.next().unwrap_or(""), w.next().unwrap_or("released"), w.next().unwrap_or("").trim());
        if !valid_name(id) || !matches!(how, "released" | "expired" | "holder-dead" | "denied") {
            return err("usage: lease-end <id> released|expired|holder-dead|denied [inspect json]");
        }
        let mut inspect = String::new();
        let mut handoff = false;
        if !extra.is_empty() {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-end: the inspect part is not JSON") };
            // a lease waits for this box: the tree stays where it is for it (no restore first)
            handoff = v.get("handoff").and_then(|x| x.as_bool()).unwrap_or(false);
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
        let e = self.end(l, how, handoff);
        format!("{}{inspect}}}", &e[..e.len() - 1])
    }

    /// Ends a lease taken out of the table: processes, scratch, workspace; the answer line.
    fn end(&self, l: Lease, how: &str, handoff: bool) -> String {
        // what the next lease needs first: the processes stopped, the tree let go; the rest (its scratch
        // dir measured and removed, the disk guard) after the answer, in the background
        let (mut killed, mut leak) = stop_runs(&l.runs, self.opts.grace);
        let (strays, stray_leak) = stop_strays(&lease_mark(&self.state, &l.id), self.opts.grace);
        if !strays.is_empty() {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} on {}: stopped {} process(es) that had left its process groups: {strays:?}", l.id, l.kind.as_str(), l.lane, self.machine, strays.len()));
        }
        killed.extend(strays);
        leak.extend(stray_leak);
        let _ = std::fs::remove_file(self.quiet().join(format!("{}.cmd", l.id)));
        let recovered = self.release_tree(&l, handoff);
        let _ = std::fs::remove_file(self.state.join(format!("{}.json", l.id)));
        for r in &l.runs {
            let _ = std::fs::remove_file(&r.exit_file);
        }
        let sev = if how == "released" { Sev::Info } else { Sev::Warn };
        self.emit("lease", sev, format!("lease: {} {} of {} ended on {} ({how}): {} process group(s) stopped{}", l.id, l.kind.as_str(), l.lane, self.machine, killed.len(), if handoff { ", tree handed to the next lease" } else { "" }));
        self.guard.touch();
        let (tmp, emit, tag) = (l.tmp.clone(), self.emitter(), format!("{} {} on {}", l.id, l.lane, self.machine));
        let _ = std::thread::Builder::new().name("devhub-lease-cleanup".into()).spawn(move || {
            let mut freed = 0u64;
            if tmp.exists() {
                freed += disk::dir_size(&tmp).0;
                let _ = std::fs::remove_dir_all(&tmp);
            }
            emit("ws", Sev::Info, format!("ws_cleanup: {tag} freed {} MB", freed >> 20));
        });
        let freed = 0u64;
        if !leak.is_empty() {
            self.emit("lease", Sev::Err, format!("lease_leak: {} {} on {}: pid(s) {:?} survived KILL", l.id, l.lane, self.machine, leak));
        }
        self.ended_line(&l.id, &killed, freed, &recovered, &leak)
    }

    /// The lease lets go of the node's tree (the last one bundles what it left there): the bundles.
    #[cfg(feature = "checkout")]
    fn release_tree(&self, l: &Lease, handoff: bool) -> Vec<PathBuf> {
        let (Some(pin), Some(root)) = (&l.pin, &self.opts.tree) else { return Vec::new() };
        let conf = self.conf();
        let (work, user_box) = (conf.work.clone(), conf.role.users());
        if !l.overlay.is_empty() {
            match super::Pin::parse(pin).ok_or_else(|| format!("bad pin {pin}")).and_then(|p| super::tree::undo_overlay(root, &p, &l.overlay)) {
                Ok(n) => self.emit("ws", Sev::Info, format!("ws_overlay: {} {} on {}: {n} shipped file(s) undone", l.id, l.lane, self.machine)),
                Err(e) => self.emit("ws", Sev::Err, format!("ws_overlay: {} {} on {}: undoing the shipped files failed: {e}", l.id, l.lane, self.machine)),
            }
        }
        if handoff {
            // the next lease moves the tree from here; when none takes it after all, it goes back
            let (root, out, emit, machine) = (root.clone(), work.join("recovered"), self.emitter(), self.machine.clone());
            let _ = std::thread::Builder::new().name("devhub-tree-handoff".into()).spawn(move || {
                std::thread::sleep(super::tree::HANDOFF_GRACE);
                let r = super::tree::restore_if_free(&root, &out, user_box);
                if let Some(to) = &r.restored {
                    emit("ws", Sev::Info, format!("ws_restore: {machine}: no lease took the handed-over tree, back at the shared {to}"));
                }
                for e in &r.unclean {
                    emit("ws", Sev::Err, format!("ws_unclean: {machine}: the handed-over tree stays off the shared commits: {e}"));
                }
            });
        }
        let r = super::tree::release(&l.id, root, &work.join("recovered"), &format!("{}-{}", l.lane, l.id), handoff, user_box);
        self.report_tree(&format!("{} {} on {}", l.id, l.lane, self.machine), &r);
        if let Some(to) = &r.restored {
            self.emit("ws", Sev::Info, format!("ws_restore: {} {} on {}: the tree is back at the shared {to}", l.id, l.lane, self.machine));
        }
        r.bundles
    }

    /// What clearing the tree took out, and an error event when it could not.
    #[cfg(feature = "checkout")]
    fn report_tree(&self, who: &str, r: &super::tree::Released) {
        for b in &r.bundles {
            self.emit("ws", Sev::Warn, format!("ws_recovered: {who} left changes in the tree: bundled to {}", b.display()));
        }
        if !r.removed.is_empty() {
            let shown: Vec<&str> = r.removed.iter().take(8).map(String::as_str).collect();
            self.emit("ws", Sev::Info, format!("ws_strays: {who}: {} untracked path(s) removed from the tree: {}{}", r.removed.len(), shown.join(" "), if r.removed.len() > shown.len() { " .." } else { "" }));
        }
        for e in &r.unclean {
            self.emit("ws", Sev::Err, format!("ws_unclean: {who}: the tree is not clean, later leases may fail on it: {e}"));
        }
    }

    #[cfg(not(feature = "checkout"))]
    fn release_tree(&self, _l: &Lease, _handoff: bool) -> Vec<PathBuf> {
        Vec::new()
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
            self.end(l, "expired", false);
        }
    }

    fn recovered_path(&self, p: &str) -> Result<PathBuf, String> {
        let root = std::fs::canonicalize(self.conf().work.join("recovered")).map_err(|_| "nothing recovered on this node".to_string())?;
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

    /// `ship-done <id> <b64 {"shipped":[..],"deleted":[..]}>`: `devhub run` shipped its worktree's
    /// changes on top of the lease's pin; the deleted files go now, and every path is undone at
    /// the lease's end.
    fn ship_done(&self, rest: &str) -> String {
        let (id, b64) = rest.split_once(' ').unwrap_or((rest, ""));
        let Some(v) = b64_decode(b64.trim()).and_then(|b| makepad_strict_json::parse(&b).ok()) else { return err("ship-done: not base64 JSON") };
        let list = |k: &str| v.get(k).and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect::<Vec<_>>());
        let (shipped, deleted) = (list("shipped"), list("deleted"));
        if let Some(bad) = shipped.iter().chain(&deleted).find(|p| !inside(p)) {
            return err(format!("ship-done: {bad} is not a path inside the tree"));
        }
        let mut leases = self.leases.lock().unwrap();
        let Some(l) = leases.get_mut(id) else { return err(format!("no lease {id} on {}", self.machine)) };
        if l.pin.is_none() {
            return err(format!("{id} holds no tree: ship-done is for a lease at a revision"));
        }
        let mut removed = 0;
        for f in &deleted {
            let p = l.work.join(f);
            if std::fs::symlink_metadata(&p).is_ok_and(|m| !m.is_dir()) && std::fs::remove_file(&p).is_ok() {
                removed += 1;
                prune_empty_parents(&l.work, f);
            }
        }
        for p in shipped.into_iter().chain(deleted) {
            if !l.overlay.contains(&p) {
                l.overlay.push(p);
            }
        }
        self.save(l);
        format!("{{\"overlay\":{},\"deleted\":{removed}}}", l.overlay.len())
    }

    /// `ship-have|ship-put|ship-link <id> ...`: `devhub run` ships its tree into the lease's
    /// work dir ([`super::ship`]).
    fn ship(&self, word: &str, rest: &str) -> String {
        let (id, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let work = match self.leases.lock().unwrap().get(id) {
            Some(l) => l.work.clone(),
            None => return err(format!("no lease {id} on {}", self.machine)),
        };
        let r = match word {
            "ship-have" => super::ship::have(&work, rest.trim()),
            "ship-put" => super::ship::put(&work, rest),
            _ => super::ship::link(&work, rest),
        };
        r.unwrap_or_else(err)
    }

    /// The folder `devhub get` reads and the prefix that names it: a live lease's work
    /// (`target/`, `tmp/`: its target and scratch), else the lane's dir (`src/`: the tree `run`
    /// ships and runs in; `target/`: the lane's warm target). Returns (root, prefix, path under root).
    fn get_root(&self, lane: &str, lease: &str, rel: &str) -> Result<(PathBuf, &'static str, String), String> {
        if !valid_name(lane) || !inside(rel) {
            return Err("usage: <lane> <lease|-> <relative path inside the lane's dir>".into());
        }
        let under = |pre: &str| if rel == pre { Some(".".to_string()) } else { rel.strip_prefix(pre).and_then(|r| r.strip_prefix('/')).map(str::to_string) };
        if lease != "-" {
            let leases = self.leases.lock().unwrap();
            let l = leases.get(lease).filter(|l| l.lane == lane).ok_or_else(|| format!("no live lease {lease} of lane {lane} on {}", self.machine))?;
            return Ok(match (under("target"), under("tmp")) {
                (Some(r), _) => (l.target.clone(), "target/", r),
                (_, Some(r)) => (l.tmp.clone(), "tmp/", r),
                _ => (l.work.clone(), "", rel.to_string()),
            });
        }
        // no live lease: the node's tree (a lane's results that outlive its lease are in the
        // shared target); a node without one: the lane's dir
        if let Some(t) = &self.opts.tree {
            return Ok(match (under("target"), under("src")) {
                (Some(r), _) => (t.join("target"), "target/", r),
                (_, Some(r)) => (t.clone(), "src/", r),
                _ => return Err("without --lease: `src/..` (the node's tree) or `target/..` (its target)".into()),
            });
        }
        let w = self.conf().work;
        Ok(match under("target") {
            Some(r) => (w.join("target").join(format!("lane-{lane}")), "target/", r),
            None => (w.join("lane").join(lane), "", rel.to_string()),
        })
    }

    /// `get-ls <lane> <lease|-> <rel>`: one `{"p":..,"n":..,"r":..}` line per file under it; `p`
    /// starts with the last component of <rel> (where rsync would put it in the target folder),
    /// `r` is what `get-read` takes.
    fn get_ls(&self, rest: &str) -> Vec<String> {
        let w: Vec<&str> = rest.splitn(3, ' ').collect();
        let [lane, lease, rel] = w[..] else { return vec![err("usage: get-ls <lane> <lease|-> <rel>")] };
        let (root, pre, sub) = match self.get_root(lane, lease, rel.trim()) {
            Ok(x) => x,
            Err(e) => return vec![err(e)],
        };
        let p = root.join(&sub);
        let base = if sub == "." { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone()) };
        let mut files = Vec::new();
        if let Err(e) = files_under(&base, &p, GET_MAX_FILES, &mut files) {
            return vec![err(e)];
        }
        files
            .into_iter()
            .map(|(f, n)| {
                let under_root = base.join(&f);
                let r = format!("{pre}{}", slashed(under_root.strip_prefix(&root).unwrap_or(&under_root)));
                format!("{{\"p\":{},\"n\":{n},\"r\":{}}}", json_str(&f), json_str(&r))
            })
            .collect()
    }

    /// `get-read <lane> <lease|-> <off> <r>`: a chunk of one file `get-ls` listed.
    fn get_read(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.splitn(4, ' ').collect();
        let [lane, lease, off, rel] = w[..] else { return err("usage: get-read <lane> <lease|-> <offset> <rel>") };
        let Ok(off) = off.parse::<u64>() else { return err("get-read: bad offset") };
        let (root, _, sub) = match self.get_root(lane, lease, rel.trim()) {
            Ok(x) => x,
            Err(e) => return err(e),
        };
        let p = root.join(sub);
        use std::io::{Read, Seek, SeekFrom};
        let mut f = match std::fs::File::open(&p) {
            Ok(f) => f,
            Err(e) => return err(format!("{}: {e}", p.display())),
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(off)).is_ok() {
            let _ = f.take(GET_CHUNK as u64).read_to_end(&mut buf);
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
        let conf = self.conf();
        let w = &conf.work;
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

fn watchdog(w: Weak<Exec>, tick: Duration) {
    loop {
        std::thread::sleep(tick);
        let Some(x) = w.upgrade() else { return };
        x.expire();
    }
}

fn disk_loop(w: Weak<Exec>) {
    os::nice_this_thread();
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let Some(x) = w.upgrade() else { return };
        x.guard.check(&|| x.live());
    }
}

/// TERM the runs' groups, `grace`, KILL, stop their scopes: (stopped leaders, survivors).
fn stop_runs(runs: &[Arc<Run>], grace: Duration) -> (Vec<u32>, Vec<u32>) {
    let alive: Vec<&Arc<Run>> = runs.iter().filter(|r| r.start != 0 && os::group_alive(r.pid, r.start, &r.hold)).collect();
    for r in &alive {
        os::signal_group(r.pid, false, &r.hold);
    }
    let t = Instant::now();
    while t.elapsed() < grace && alive.iter().any(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        std::thread::sleep(Duration::from_millis(100));
    }
    for r in alive.iter().filter(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        os::signal_group(r.pid, true, &r.hold);
    }
    for u in runs.iter().filter_map(|r| r.unit.as_ref()) {
        let _ = makepad_remote::group::command("systemctl").args(["--user", "stop", &format!("{u}.scope")]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) && alive.iter().any(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let leak = alive.iter().filter(|r| os::group_alive(r.pid, r.start, &r.hold) || os::same(r.pid, r.start)).map(|r| r.pid).collect();
    (alive.iter().map(|r| r.pid).collect(), leak)
}

/// Stops what the lease's runs started and their groups did not take with them: TERM, looking again
/// while the grace lasts (a WM on its way out may still start an app), then KILL. (stopped, left)
fn stop_strays(mark: &str, grace: Duration) -> (Vec<u32>, Vec<u32>) {
    let marker = format!("{LEASE_ENV}={mark}");
    let me = std::process::id();
    let find = || os::with_env(&marker).into_iter().filter(|p| *p != me).collect::<Vec<u32>>();
    let mut stopped: Vec<u32> = Vec::new();
    let t = Instant::now();
    loop {
        let now = find();
        if now.is_empty() {
            return (stopped, Vec::new());
        }
        if t.elapsed() >= grace {
            break;
        }
        for &p in &now {
            if !stopped.contains(&p) {
                stopped.push(p);
                os::signal_one(p, false);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let t = Instant::now();
    loop {
        let now = find();
        if now.is_empty() || t.elapsed() >= Duration::from_secs(2) {
            return (stopped, now);
        }
        for &p in &now {
            if !stopped.contains(&p) {
                stopped.push(p);
            }
            os::signal_one(p, true);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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

/// A person uses this node now ([`super::user`]): it takes no work until they leave.
pub fn user_present() -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().present())
}

/// A Desk Link viewer attached that may be watching ([`super::user::UserTrack::watched`]).
pub fn user_watching() -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().watched())
}

/// What keeps a user-facing service of this node (the WM, the AI hub) from restarting now: a
/// person at the node (input within `user_idle_min`) or unsaved work in its apps. Empty: it may
/// restart. The hub itself restarts regardless (invisible to the user).
pub fn user_blockers() -> Vec<String> {
    let mut why = Vec::new();
    if user_present() {
        why.push("a user is at the node (it restarts once they are idle: lease.conf user_idle_min)".to_string());
    }
    #[cfg(all(unix, feature = "checkout"))]
    why.extend(makepad_app_registry::machine::unsaved_work().into_iter().map(|(pid, app, what)| format!("unsaved work in {app} (pid {pid}): {what}")));
    why
}

/// The user flips this node's dev switch on the node itself (tray / task view): shown at once,
/// and asked of the master with the next caps answer, which saves it in nodes.conf and pushes
/// the policy (the master stays the one place the switch is kept).
pub fn request_dev(on: bool) {
    if let Some(x) = EXEC.get() {
        x.dev.store(on, Ordering::Release);
        x.dev_req.store(if on { 1 } else { 2 }, Ordering::Release);
    }
}

/// The leases live on this node now: (id, lane, kind), for its own status view.
pub fn held() -> Vec<(String, String, &'static str)> {
    EXEC.get().map_or_else(Vec::new, |x| x.leases.lock().unwrap().values().map(|l| (l.id.clone(), l.lane.clone(), l.kind.as_str())).collect())
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
    l.runs.push(Arc::new(Run { n, pid, start: os::proc_start(pid).unwrap_or(0), unit: None, log, exit: Mutex::new(None), ours: false, hold: os::Hold::none(), exit_file: PathBuf::new() }));
    x.save(l);
    Ok(())
}

/// Whether lease `id` is live here and may launch.
/// The lane lease `id` belongs to.
pub fn lease_lane(id: &str) -> Option<String> {
    EXEC.get().and_then(|x| x.leases.lock().unwrap().get(id).map(|l| l.lane.clone()))
}

pub fn lease_live(id: &str) -> bool {
    EXEC.get().is_some_and(|x| x.leases.lock().unwrap().get(id).is_some_and(|l| l.kind.writes()))
}

pub fn register(ep: &Endpoint, ctx: &crate::sync::NodeCtx) -> Arc<Exec> {
    let conf = LeaseConf::load();
    #[cfg(feature = "checkout")]
    let opts = Opts { tree: Some(super::tree::root(&conf.work)), ..Opts::default() };
    #[cfg(not(feature = "checkout"))]
    let opts = Opts::default();
    let x = Exec::new(&ctx.machine, conf, crate::proto::home().join("leases"), opts, ctx.publish.clone());
    let _ = EXEC.set(x.clone());
    // the person at the box: our WM's answer every 2 s (a node without one senses nothing)
    let w = Arc::downgrade(&x);
    let _ = std::thread::Builder::new().name("devhub-user".into()).spawn(move || loop {
        let Some(x) = w.upgrade() else { return };
        #[cfg(all(unix, feature = "checkout"))]
        let answer = makepad_app_registry::machine::wm_socket().ok_or_else(|| "no WM".to_string()).and_then(|p| makepad_app_registry::machine::request_activity(&p));
        #[cfg(not(all(unix, feature = "checkout")))]
        let answer: Result<String, String> = Err("no WM".into());
        x.sense_user(answer.as_deref().map_err(|e| e.clone()));
        drop(x);
        // no WM, or one from before `activity` (it logs each line it does not know): ask seldom
        std::thread::sleep(if answer.is_ok() { super::user::POLL } else { super::user::POLL * 15 });
    });
    for &word in WORDS {
        let e = x.clone();
        ep.register(word, 1, lines::service(word, Arc::new(move |line, out| out.extend(e.handle(word, line)))));
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    // what the tests run: a line out, the env, a failure, a long sleeper with a child (Windows)
    #[cfg(unix)]
    const ECHO_HI: &[&str] = &["/bin/echo", "hi"];
    #[cfg(unix)]
    const ENV: &[&str] = &["/usr/bin/env"];
    #[cfg(unix)]
    const FAIL: &[&str] = &["false"];
    #[cfg(unix)]
    const SLEEP: &[&str] = &["/bin/sleep", "300"];
    #[cfg(unix)]
    const SLEEP_LABEL: &str = "sleep 300";
    #[cfg(windows)]
    const ECHO_HI: &[&str] = &["cmd", "/c", "echo hi"];
    #[cfg(windows)]
    const ENV: &[&str] = &["cmd", "/c", "set"];
    #[cfg(windows)]
    const FAIL: &[&str] = &["cmd", "/c", "exit 1"];
    #[cfg(windows)]
    const SLEEP: &[&str] = &["cmd", "/c", "ping -n 300 127.0.0.1 >nul"];
    #[cfg(windows)]
    const SLEEP_LABEL: &str = "cmd.exe";

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

    #[cfg(unix)]
    #[test]
    fn two_leases_logs_hold_only_their_own_output() {
        let (x, base, _) = test_exec("iso", Opts::default());
        for (id, lane) in [("L10", "lane-a"), ("L11", "lane-b")] {
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"{lane}","kind":"test","rev":null}}"#));
            assert_eq!(field(&a[0], "ok").and_then(|v| super::super::num(&v)), Some(1.0), "{a:?}");
        }
        // interleaved runs on both leases: each log carries its lease's marker and never the other's
        let mut runs = Vec::new();
        for round in 0..3 {
            for (id, who) in [("L10", "from-lane-a"), ("L11", "from-lane-b")] {
                let r = x.handle("lease-run", &format!("{id} . {}", b64(&["/bin/echo", &format!("{who}-{round}")])));
                runs.push((id, who, round, field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32));
            }
        }
        for (id, who, round, n) in runs {
            let (text, code) = log_until_exit(&x, id, n);
            assert_eq!(code, 0);
            assert_eq!(text.trim(), format!("{who}-{round}"), "{id} run {n}");
        }
        let _ = std::fs::remove_dir_all(&base);
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

    #[test]
    fn runs_get_a_clean_env_and_only_screen_leases_the_wm() {
        let hub = vec![("PATH".to_string(), "/bin".to_string()), ("HOME".into(), "/h".into()), ("LC_ALL".into(), "C".into()), ("MAKEPAD_WM_ROOT".into(), "/wm".into()), ("MAKEPAD_WM_SOCKET".into(), "/s".into()), ("DISPLAY".into(), ":0".into()), ("CARGO_TARGET_DIR".into(), "/wm/target".into()), ("RUSTFLAGS".into(), "-g".into())];
        let keys = |kind| base_env(kind, hub.clone().into_iter()).into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(Kind::Test), vec!["PATH", "HOME", "LC_ALL"]);
        assert_eq!(keys(Kind::Build), vec!["PATH", "HOME", "LC_ALL"]);
        assert_eq!(keys(Kind::Screen), vec!["PATH", "HOME", "LC_ALL", "MAKEPAD_WM_ROOT", "MAKEPAD_WM_SOCKET", "DISPLAY"]);
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

        let r = x.handle("lease-run", &format!("L1 . {}", b64(ECHO_HI)));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
        let (text, code) = log_until_exit(&x, "L1", n);
        assert_eq!((text.trim(), code), ("hi", 0));
        // the env reaches the program; a failing one reports its code
        let r = x.handle("lease-run", &format!("lease-run L1 . {}", b64(ENV)));
        let (env, _) = log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32);
        assert!(env.contains("FOO=bar") && env.contains("SANDBOX_MUTE=1") && env.contains(&format!("TMPDIR={}", tmp.display())));
        // every run knows its own lease and lane: its verdict lines say whose they are
        assert!(env.contains("DEVHUB_LEASE=L1") && env.contains("MAKEPAD_TEST_LANE=gapx") && env.contains("MAKEPAD_JIT_SESSION=L1"), "{env}");
        // nothing of the hub's own environment beyond the basics
        assert!(env.lines().all(|l| !l.starts_with("CARGO_PKG_") && !l.starts_with("RUST_TEST_")), "{env}");
        let r = x.handle("lease-run", &format!("L1 . {}", b64(FAIL)));
        assert_eq!(log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32).1, 1);
        assert!(x.handle("lease-run", &format!("L1 ../x {}", b64(&["true"])))[0].contains("err"));

        // a sleeping child (and its own child) is stopped on end
        let r = x.handle("lease-run", &format!("L1 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = os::proc_start(pid).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(os::group_alive(pid, start, &os::Hold::none()));
        let beat = x.handle("lease-beat", "L1 L404");
        assert!(beat[0].starts_with(&format!(r#"{{"live":["L1"],"unknown":["L404"],"info":{{"L1":{{"running":"{SLEEP_LABEL}","pid":"#)), "{beat:?}");
        let e = x.handle("lease-end", "L1 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")) && e[0].contains("\"leak\":[]"), "{e:?}");
        assert!(!os::group_alive(pid, start, &os::Hold::none()));
        // the tmp goes on a cleanup thread (the lease's end does not wait for it)
        let t = Instant::now();
        while tmp.exists() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!tmp.exists() && !base.join("leases/L1.json").exists());
        assert!(work.exists(), "a lane dir stays warm");
        assert!(x.handle("lease-end", "L1 released")[0].contains("\"unknown\":true"));
        assert!(events.lock().unwrap().iter().any(|l| l.contains("ws_create")) && events.lock().unwrap().iter().any(|l| l.contains("ended on t (released)")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn get_and_quiet_marks() {
        let (x, base, _) = test_exec("get", Opts::default());
        let a = x.handle("lease-start", r#"{"id":"L2","lane":"getx","kind":"timed","rev":null}"#);
        assert!(field(&a[0], "user").and_then(|u| u.as_str().map(|u| !u.is_empty())).unwrap_or(false), "{a:?}");
        let work = PathBuf::from(field(&a[0], "work").unwrap().as_str().unwrap());
        let tmp = PathBuf::from(field(&a[0], "tmp").unwrap().as_str().unwrap());
        assert!(base.join("quiet/L2.cmd").exists(), "a timed lease marks the machine quiet");
        std::fs::create_dir_all(work.join("out/deep")).unwrap();
        for f in ["a.txt", "out/r.txt", "out/deep/s.bin"] {
            std::fs::write(work.join(f), f).unwrap();
        }
        // ship-done is for a lease at a revision (the node's tree)
        assert!(x.handle("ship-done", &format!("L2 {}", makepad_network::digest::base64_encode(br#"{"shipped":["a.txt"],"deleted":[]}"#)))[0].contains("holds no tree"));
        // get: a folder lists with its own name first, `r` reads it back; a lease's tmp/ too
        let ls = x.handle("get-ls", "getx - src/out");
        assert_eq!(ls.len(), 2, "{ls:?}");
        assert!(ls[0].contains(r#""p":"out/deep/s.bin","n":14,"r":"src/out/deep/s.bin""#), "{ls:?}");
        let r = x.handle("get-read", "getx - 0 src/out/r.txt");
        assert_eq!(field(&r[0], "data").and_then(|d| d.as_str().and_then(b64_decode)), Some(b"out/r.txt".to_vec()));
        assert_eq!(field(&r[0], "eof").and_then(|e| e.as_bool()), Some(true));
        std::fs::write(tmp.join("result.jsonl"), "{}").unwrap();
        let ls = x.handle("get-ls", "getx L2 tmp/result.jsonl");
        assert!(ls[0].contains(r#""p":"result.jsonl","n":2,"r":"tmp/result.jsonl""#), "{ls:?}");
        assert!(x.handle("get-read", "getx L2 0 tmp/result.jsonl")[0].contains("\"eof\":true"));
        assert!(x.handle("get-ls", "getx - ../x")[0].contains("err"));
        assert!(x.handle("get-ls", "other L2 tmp")[0].contains("err"), "a lease answers only for its lane");
        // ended: the quiet mark goes, the lane dir stays readable
        x.handle("lease-end", "L2 released");
        assert!(!base.join("quiet/L2.cmd").exists());
        assert!(x.handle("get-ls", "getx - src/a.txt")[0].contains(r#""p":"a.txt""#));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A rev lease runs in the node's tree at its pin, `devhub run`'s shipped changes sit on top,
    /// and its end (released, or the watchdog's) leaves the tree on the shared commits with
    /// nothing of the lane in it; the warm target stays.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_rev_lease_leaves_the_box_on_the_shared_commits() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-exec-box-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root) = (base.join("src"), base.join("makepad"));
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(src.join("a.rs"), "landed").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "landed"]);
        let landed = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(src.join("a.rs"), "lane").unwrap();
        g(&["commit", "-q", "-am", "lane"]);
        let lane = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", base.join("stores"));
        let store = crate::sync::git::Store::open("makepad").unwrap();
        let bundle = base.join("b.bundle");
        g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
        store.fetch_bundle(&bundle, "work").unwrap();
        store.move_checkout(&root, None, &landed).unwrap();
        store.set_ref(crate::sync::git::SYNCED_REF, &landed).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/warm"), "x").unwrap();
        let (x, _, _) = test_exec("tree", Opts { tree: Some(root.clone()), ..Opts::default() });

        for (id, how) in [("R1", "released"), ("R2", "expired")] {
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"lanex","kind":"test","rev":{{"makepad":"{lane}","commercial":null}}}}"#));
            assert_eq!(field(&a[0], "work").and_then(|w| w.as_str().map(PathBuf::from)), Some(root.clone()), "{a:?}");
            assert!(a[0].contains(&format!("\"CARGO_TARGET_DIR\":{}", json_str(&root.join("target").to_string_lossy()))));
            assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "lane");
            // `devhub run` shipped a new file on top
            std::fs::write(root.join("new.rs"), "from the lane's worktree").unwrap();
            let d = x.handle("ship-done", &format!("{id} {}", makepad_network::digest::base64_encode(br#"{"shipped":["new.rs"],"deleted":[]}"#)));
            assert!(d[0].contains("\"overlay\":1"), "{d:?}");
            assert!(x.handle("caps", "caps")[0].contains(&format!("\"tree\":\"{lane}\"")));
            x.handle("lease-end", &format!("{id} {how}"));
            // no box stays on a lane rev after its lease ends
            assert_eq!(super::super::tree::current(&root).map(|p| p.makepad), Some(landed.clone()), "{id}");
            assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "landed");
            assert!(!root.join("new.rs").exists() && root.join("target/warm").exists());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A file shipped on top of a lease stays out of the way however the lease ended: the ship
    /// never reached `ship-done`, or a command wrote it. A commit that adds that path lands, and
    /// the next lease starts on it; a stray from outside any lease cannot fail a start either.
    #[cfg(feature = "checkout")]
    #[test]
    fn no_stray_can_fail_a_later_lease() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-exec-nostray-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root) = (base.join("src"), base.join("makepad"));
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(src.join("a.rs"), "one").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "one"]);
        let one = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", base.join("stores"));
        let store = crate::sync::git::Store::open("makepad").unwrap();
        let bundle = base.join("b.bundle");
        let fetch = || {
            g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
            store.fetch_bundle(&bundle, "work").unwrap();
        };
        fetch();
        store.move_checkout(&root, None, &one).unwrap();
        store.set_ref(crate::sync::git::SYNCED_REF, &one).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/warm"), "x").unwrap();
        let (x, _, events) = test_exec("stray", Opts { tree: Some(root.clone()), ..Opts::default() });
        let start = |id: &str, rev: &str| x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"lanex","kind":"test","rev":{{"makepad":"{rev}","commercial":null}}}}"#));

        // a shipped file ship-done never named, and what a command wrote, in a new dir: both go
        // at the lease's end, the target stays, and the event names them
        let a = start("S1", &one);
        assert!(a[0].contains("\"work\""), "{a:?}");
        std::fs::create_dir_all(root.join("tools/steam")).unwrap();
        std::fs::write(root.join("tools/steam/new.rs"), "shipped, not committed yet").unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/result.txt"), "a command's output").unwrap();
        // a nested target dir stays, ignored or not
        std::fs::create_dir_all(root.join("nested/target")).unwrap();
        std::fs::write(root.join("nested/target/warm"), "x").unwrap();
        x.handle("lease-end", "S1 released");
        assert!(root.join("nested/target/warm").exists());
        assert!(!root.join("tools").exists() && !root.join("out").exists() && root.join("target/warm").exists());
        assert!(events.lock().unwrap().iter().any(|e| e.contains("ws_strays") && e.contains("tools/")), "{:?}", events.lock().unwrap());

        // the commit that adds that path lands, the box syncs to it, and the next lease starts
        std::fs::create_dir_all(src.join("tools/steam")).unwrap();
        std::fs::write(src.join("tools/steam/new.rs"), "landed").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "two"]);
        let two = g(&["rev-parse", "HEAD"]).trim().to_string();
        fetch();
        let a = start("S2", &two);
        assert!(a[0].contains("\"work\""), "{a:?}");
        assert_eq!(std::fs::read_to_string(root.join("tools/steam/new.rs")).unwrap(), "landed");
        x.handle("lease-end", "S2 released");
        assert!(!root.join("tools").exists(), "back on the shared commits");

        // a stray put there outside any lease, at a path the next pin adds: cleared at the start
        std::fs::create_dir_all(root.join("tools/steam")).unwrap();
        std::fs::write(root.join("tools/steam/new.rs"), "a stray from a hand copy").unwrap();
        let a = start("S3", &two);
        assert!(a[0].contains("\"work\""), "{a:?}");
        assert_eq!(std::fs::read_to_string(root.join("tools/steam/new.rs")).unwrap(), "landed");
        x.handle("lease-end", "S3 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn watchdog_ends_a_lease_without_beats() {
        let (x, base, events) = test_exec("wd", Opts { no_beat: Duration::from_millis(600), tick: Duration::from_millis(100), grace: Duration::from_secs(10), ..Opts::default() });
        let a = x.handle("lease-start", r#"{"id":"L7","lane":"lost","kind":"long","rev":null}"#);
        assert!(a[0].contains("\"ok\":1"));
        let r = x.handle("lease-run", &format!("L7 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = os::proc_start(pid).unwrap();
        // beaten: it stays
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(100));
            x.handle("lease-beat", "L7");
        }
        assert!(x.leases.lock().unwrap().contains_key("L7"));
        // not beaten: the node ends it and stops its process
        let t = Instant::now();
        while x.leases.lock().unwrap().contains_key("L7") || os::group_alive(pid, start, &os::Hold::none()) {
            assert!(t.elapsed() < Duration::from_secs(20), "the watchdog did not end L7");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(events.lock().unwrap().iter().any(|l| l.contains("no beat from the master")));
        // dev off refuses write kinds, not inspect
        assert_eq!(x.handle("policy", "dev=off"), vec![r#"{"dev":"off"}"#.to_string()]);
        // a flip made on the node is asked of the master with the caps answer; the master's policy push clears it
        x.dev_req.store(1, Ordering::Release);
        assert!(x.handle("caps", "caps")[0].contains("\"dev_req\":true"));
        x.handle("policy", "dev=off");
        assert!(x.handle("caps", "caps")[0].contains("\"dev_req\":null"));
        assert!(x.handle("lease-start", r#"{"id":"L8","lane":"a","kind":"build","rev":null}"#)[0].contains("dev off"));
        assert!(x.handle("lease-start", r#"{"id":"L9","lane":"a","kind":"inspect","rev":null}"#)[0].contains("\"ok\":1"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A process in a group of its own. (This binary, not `sleep`: macOS hides the environment of
    /// its platform binaries.)
    #[cfg(unix)]
    #[test]
    fn lease_end_role_stray() {
        if std::env::var_os("DEVHUB_TEST_STRAY").is_some() {
            std::thread::sleep(Duration::from_secs(300));
        }
    }

    /// Under a lease (only there): starts a stray that keeps the lease's mark and one that removed
    /// it, says their pids in its work dir, and waits.
    #[cfg(unix)]
    #[test]
    fn lease_end_role_escape() {
        // (this test's lease, not the one a real hub runs these tests under)
        if !std::env::var(LEASE_ENV).is_ok_and(|m| m.contains("devhub-exec-strays")) || std::env::var_os("DEVHUB_TEST_STRAY").is_some() {
            return;
        }
        use std::os::unix::process::CommandExt;
        let spawn = |keep: bool| {
            let mut c = std::process::Command::new(std::env::current_exe().unwrap());
            c.args(["--exact", "lease::exec::tests::lease_end_role_stray"]).env("DEVHUB_TEST_STRAY", "1").process_group(0);
            if !keep {
                c.env_remove(LEASE_ENV);
            }
            c.spawn().unwrap().id()
        };
        let (gone, stays) = (spawn(true), spawn(false));
        std::fs::write("pids.new", format!("{gone} {stays}")).unwrap();
        std::fs::rename("pids.new", "pids").unwrap();
        std::thread::sleep(Duration::from_secs(300));
    }

    #[cfg(unix)]
    #[test]
    fn a_leases_end_stops_what_left_its_process_group() {
        let (x, base, _) = test_exec("strays", Opts { grace: Duration::from_secs(2), scopes: Some(false), ..Opts::default() });
        let a = x.handle("lease-start", r#"{"id":"S1","lane":"strays","kind":"test","rev":null}"#);
        let work = field(&a[0], "work").and_then(|w| w.as_str().map(PathBuf::from)).unwrap();
        let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
        let r = x.handle("lease-run", &format!("S1 . {}", b64(&[&exe, "--exact", "lease::exec::tests::lease_end_role_escape", "--nocapture"])));
        assert!(field(&r[0], "run").is_some(), "{r:?}");
        let t = Instant::now();
        while !work.join("pids").exists() {
            assert!(t.elapsed() < Duration::from_secs(30), "the role did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        let text = std::fs::read_to_string(work.join("pids")).unwrap();
        let pids: Vec<u32> = text.split_whitespace().map(|p| p.parse().unwrap()).collect();
        let (gone, stays) = (pids[0], pids[1]);
        assert!(os::pid_alive(gone) && os::pid_alive(stays));
        let e = x.handle("lease-end", "S1 released");
        assert!(e[0].contains(&gone.to_string()) && !e[0].contains(&stays.to_string()), "gone {gone} stays {stays}: {e:?}");
        assert_eq!(os::with_env(&format!("{LEASE_ENV}={}", lease_mark(&base.join("leases"), "S1"))), Vec::<u32>::new());
        assert!(os::pid_alive(stays), "one that removed the mark outlives the lease");
        os::signal_one(stays, true);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_restarted_hub_takes_its_leases_back() {
        let (x, base, _) = test_exec("ledger", Opts::default());
        x.handle("lease-start", r#"{"id":"L3","lane":"b","kind":"screen","rev":null}"#);
        let r = x.handle("lease-run", &format!("L3 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let conf = x.conf();
        let y = Exec::new("t", conf, base.join("leases"), Opts::default(), Arc::new(|_| {}));
        assert!(y.handle("lease-beat", "L3")[0].contains("\"live\":[\"L3\"]"));
        let e = y.handle("lease-end", "L3 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")), "{e:?}");
        assert!(!os::group_alive(pid, os::proc_start(pid).unwrap_or(1), &os::Hold::none()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
