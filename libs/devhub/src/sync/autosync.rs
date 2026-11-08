//! The master follows the followed repositories' branches ([`super::followed`]: work, commercial
//! main) so each deployment receives landed changes independently of leases. A node is behind
//! when `caps.deploy` differs from those tips (a pin, [`Pin`]). Each round attempts one node; the
//! node owns its build, live patches and safe restart rules. Settings are reread from
//! autosync.conf; autosync.state records successful applies across master restarts, a
//! `<node> <pin>` line each. Repeated failures are reported once, to the master's stderr.
//!
//! A landing kicks the watcher (`makepad devhub sync now`, the landing scripts' hook): the next
//! round starts at once and reaches every node that needs the new tips, the first listed node
//! (the user's WM node) alone first, then the rest together. The order carries the landing's
//! time, so the node can say how long landing -> visible took.
//!
//! A node whose apply answered `waits:` (a timed lease runs there) is told to the scheduler
//! ([`Waits`]). A node with a timed ask first in line ([`crate::lease::sched::Sched::update_pin`])
//! stays at what it has until the ask starts; one that fell behind moves to that ask's catch-up
//! and no further. That move is not part of a round: a round's batches each wait for their nodes'
//! builds, minutes per batch, and an ask whose box had nothing to do but this one update sat
//! behind them ([`follow_pins`]).
//!
//! The user's WM node (role `user-wm`) takes only proven tips, as the Mac's shows do: its
//! deployment moves to the newest tip the fast bucket proved ([`crate::bucket::proven_tip`]), never
//! back, and holds what it has until then ([`held`]: said in its shows' rows). What reaches its
//! running apps is the apply's as ever: patched live, and a change that needs a restart offered.
use super::cli::{ask_local, branch_sha, fleet_nodes, push_and_apply};
use super::git::valid_sha;
use crate::lease::sched::{deploy_to, DeployTo};
use crate::lease::Pin;
use crate::proto;
use makepad_remote::link::{Client, Target};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How soon a node that answered `waits:` is asked again.
const WAITS_RETRY_S: u64 = 5;

/// The user's WM nodes that hold their deployment for a tip the fast bucket has not proven: why.
static HELD: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Why `node`'s deployment holds where it is (a user's WM node, for a tip not proven yet).
pub fn held(node: &str) -> Option<String> {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).get(node).cloned()
}

/// The holds are these (one that moves, is at the tips, pinned or no longer followed holds for
/// no proof).
fn set_held(held: BTreeMap<String, String>) {
    *HELD.lock().unwrap_or_else(|e| e.into_inner()) = held;
}

/// A landing's kick: the wall-clock ms of the newest landing not taken by a round yet.
static KICK: (Mutex<Option<u64>>, Condvar) = (Mutex::new(None), Condvar::new());

/// A landing happened at `landed_ms`: the watcher starts a round now.
pub fn kick(landed_ms: u64) {
    let mut k = KICK.0.lock().unwrap_or_else(|e| e.into_inner());
    *k = Some(k.map_or(landed_ms, |t| t.min(landed_ms)));
    KICK.1.notify_all();
}

/// Sleeps up to `interval` unless a landing kicks first; the kick's landing time.
fn wait_kick(interval: Duration) -> Option<u64> {
    let k = KICK.0.lock().unwrap_or_else(|e| e.into_inner());
    let (mut k, _) = KICK.1.wait_timeout_while(k, interval, |k| k.is_none()).unwrap_or_else(|e| e.into_inner());
    k.take()
}

/// What one node's apply of an order to move to `to` answered: the commits it applied of each
/// repository `to` names, or why not.
fn apply_result(code: i32, lines: &[String], to: &Pin) -> Result<Pin, String> {
    let mut success = None;
    let mut error = None;
    for line in lines {
        if line.contains("not a fast-forward") {
            error.get_or_insert_with(|| line.clone());
        }
        let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        // Successful builds also carry an empty err field. A failed build or
        // restart can precede an applied line, so errors take precedence.
        let result = v.get("built").unwrap_or(&v);
        if result.get("err").is_some_and(|e| !e.is_null() && e.as_str() != Some("")) || result.get("ok").and_then(|x| x.as_bool()) == Some(false) {
            error.get_or_insert_with(|| line.clone());
        }
        if let Some(a) = v.get("applied") {
            let sha = |name: &str| a.get(name).and_then(|x| x.as_str()).filter(|s| valid_sha(s)).map(str::to_string);
            if let Some(mp) = sha("makepad") {
                success = to.nested.keys().try_fold(Pin::at(&mp), |pin, name| Some(pin.with(name, &sha(name)?)));
            }
        }
    }
    match (code, error, success) {
        (0, None, Some(revs)) => Ok(revs),
        (_, error, _) => Err(error.unwrap_or_else(|| format!("no successful apply (exit {code})"))),
    }
}

pub struct AutoConf {
    pub enabled: bool,
    pub repo: PathBuf,
    pub interval_s: u64,
    pub nodes: Option<Vec<String>>,
    /// the lane merge bot ([`super::lanes`]): `lanes = on|off`
    pub lanes: bool,
    /// where lane worktrees live (`lanes_root`, default `~/wt.noindex`)
    pub lanes_root: PathBuf,
    /// pre-warm idle dev-on nodes after an apply (`prewarm = on|off`)
    pub prewarm: bool,
}

impl AutoConf {
    pub fn parse(text: &str, default_repo: &Path) -> Self {
        let mut c = Self { enabled: true, repo: default_repo.to_path_buf(), interval_s: 30, nodes: None, lanes: true, lanes_root: crate::os::home_dir().join("wt.noindex"), prewarm: true };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "enabled" if v == "on" => c.enabled = true,
                "enabled" if v == "off" => c.enabled = false,
                "repo" if !v.is_empty() => c.repo = PathBuf::from(v),
                "interval" => c.interval_s = v.parse().ok().filter(|s| *s > 0).unwrap_or(c.interval_s),
                "lanes" if v == "on" => c.lanes = true,
                "lanes" if v == "off" => c.lanes = false,
                "lanes_root" if !v.is_empty() => c.lanes_root = PathBuf::from(v),
                "prewarm" if v == "on" => c.prewarm = true,
                "prewarm" if v == "off" => c.prewarm = false,
                "nodes" => c.nodes = Some(v.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect()),
                _ => {}
            }
        }
        c
    }
}

/// The pre-warm skips a node whose 1-minute load is over this per core (the pre-warm is optional
/// background work; leases never look at load).
const PREWARM_LOAD_PER_CORE: f64 = 1.25;
/// ...or whose available RAM is under this fraction of its total.
const PREWARM_RAM_FREE: f64 = 0.15;

/// A `nodes` line of a node that should pre-warm after its apply: up, dev on, no lease, not
/// draining, no person at it, under the pre-warm's load limits.
pub fn prewarm_suits(v: &makepad_strict_json::Value) -> bool {
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let leases = v.get("leases").and_then(|x| x.as_u64()).unwrap_or(1);
    let caps = v.get("caps").filter(|c| !matches!(c, makepad_strict_json::Value::Null)).and_then(|c| crate::lease::Caps::parse(&c.to_json())).filter(|c| c.cores > 0);
    let loaded = |c: &crate::lease::Caps| c.load1 > c.cores as f64 * PREWARM_LOAD_PER_CORE || (c.ram_gb > 0.0 && c.ram_avail_gb < c.ram_gb * PREWARM_RAM_FREE);
    b("up") && b("dev") && !b("draining") && !b("user") && leases == 0 && caps.is_some_and(|c| !loaded(&c))
}

/// The tips of the followed repositories ([`super::followed`]) in the checkout at `repo`: each
/// nested one's in its `apps/<name>`.
fn tips(repo: &Path, follows: &[(String, String)]) -> Result<Pin, String> {
    let mut pin = Pin::default();
    for (name, branch) in follows {
        if name == "makepad" {
            pin.makepad = branch_sha(repo, branch)?;
        } else {
            pin.nested.insert(name.clone(), branch_sha(&repo.join(crate::nested::dir(name)), branch)?);
        }
    }
    Ok(pin)
}

/// Whether a deployment at `deploy` is at `tips` (both pins' keys): the same commit of each
/// repository the tips name; one it holds besides (a nested checkout no longer followed) moves
/// nothing.
fn at_tips(deploy: &str, tips: &str) -> bool {
    let (Some(d), Some(t)) = (Pin::parse(deploy), Pin::parse(tips)) else { return false };
    d.makepad == t.makepad && t.nested.iter().all(|(name, sha)| d.nested.get(name) == Some(sha))
}

/// Deployments behind the tips, which nodes can pre-warm after applying, and each node's deployment.
fn nodes_to_apply(nodes: &[String], status: &[String], tips: &str) -> (Vec<String>, BTreeMap<String, bool>, BTreeMap<String, String>) {
    let mut deploys = BTreeMap::<String, String>::new();
    let mut idle = BTreeMap::<String, bool>::new();
    for line in status {
        let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        if let Some(n) = v.get("node").and_then(|x| x.as_str()) {
            idle.insert(n.to_string(), prewarm_suits(&v));
            if let Some(t) = v.get("caps").and_then(|c| c.get("deploy")).and_then(|x| x.as_str()) {
                deploys.insert(n.to_string(), t.to_string());
            }
        }
    }
    let todo = nodes.iter().filter(|n| !deploys.get(*n).is_some_and(|d| at_tips(d, tips))).cloned().collect();
    (todo, idle, deploys)
}

/// The nodes of role `user-wm` among the `nodes` lines: the user's WMs, which take only proven tips.
fn user_wms(status: &[String]) -> BTreeSet<String> {
    status.iter().filter_map(|line| {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let caps = crate::lease::Caps::parse(&v.get("caps")?.to_json())?;
        (caps.role == Some(crate::lease::Role::UserWm)).then(|| v.get("node")?.as_str().map(str::to_string))?
    }).collect()
}

/// Where a user's WM node at `deploy` moves ([`crate::bucket::proven_tip`]): Ok(the tip's pin key,
/// [`Pin::key`], of the repositories it follows, `follows`), or Err(why it holds what it has).
fn proven_move(home: &Path, repo: &Path, deploy: &str, follows: &[(String, String)]) -> Result<String, String> {
    let at = |t: &Pin| {
        let mut t = t.clone();
        t.nested.retain(|name, _| follows.iter().any(|(f, _)| f == name));
        t.key()
    };
    match crate::bucket::proven_tip(home, &|t| deploy == at(t) || super::deploy_has(repo, deploy, &at(t))) {
        crate::bucket::Proven::Move(t) => Ok(at(&t)),
        crate::bucket::Proven::Stay(why) => Err(format!("held at {}: {why}", crate::state::Pair::of_pin_key(deploy).map_or_else(|| deploy.to_string(), |p| p.short()))),
    }
}

/// Says to the scheduler which nodes' applies a timed run made wait (`Some(why)`) and when one
/// waits no more (`None`: applied, at the tips some other way, or no longer followed).
pub type SayWaits = Box<dyn Fn(&str, Option<String>) + Send>;

/// The timed ask first in line on a node and the deployment it holds or catches up to until that
/// ask starts ([`crate::lease::sched::Sched::update_pin`]).
pub type PinOf = Arc<dyn Fn(&str) -> Option<(String, String)> + Send + Sync>;

/// The nodes whose apply waits for a timed run, as said through [`SayWaits`].
struct Waits {
    on: BTreeSet<String>,
    say: SayWaits,
}

impl Waits {
    fn wait(&mut self, node: &str, why: String) {
        self.on.insert(node.to_string());
        (self.say)(node, Some(why));
    }

    fn done(&mut self, node: &str) {
        if self.on.remove(node) {
            (self.say)(node, None);
        }
    }

    /// Every node not in `keep` waits no more.
    fn keep(&mut self, keep: &[String]) {
        let gone: Vec<String> = self.on.iter().filter(|n| !keep.contains(n)).cloned().collect();
        for n in gone {
            self.done(&n);
        }
    }
}

/// The nodes' last successful applies (`autosync.state`, read by `devhub autosync status`): a
/// `<node> <pin>` line each ([`Pin::key`]).
struct Applied {
    path: PathBuf,
    revs: BTreeMap<String, Pin>,
    dirty: bool,
}

impl Applied {
    fn load(path: PathBuf) -> Self {
        let mut revs = BTreeMap::new();
        match std::fs::read_to_string(&path) {
            Ok(text) => for line in text.lines() {
                let words: Vec<_> = line.split_whitespace().collect();
                if let [node, key] = words.as_slice() {
                    if let Some(pin) = Pin::parse(key) {
                        revs.insert(node.to_string(), pin);
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("devhub autosync: {}: {e}", path.display()),
        }
        Self { path, revs, dirty: false }
    }

    fn save(&mut self) -> std::io::Result<()> {
        let text: String = self.revs.iter().map(|(n, pin)| format!("{n} {}\n", pin.key())).collect();
        let tmp = self.path.with_extension("state.tmp");
        std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new("."))).and_then(|_| std::fs::write(&tmp, text)).and_then(|_| std::fs::rename(&tmp, &self.path))?;
        self.dirty = false;
        Ok(())
    }
}

/// Says a node's state when it changes (a repeated failure once), to the master's stderr.
#[derive(Default)]
struct Report(BTreeMap<String, String>);

impl Report {
    fn say(&mut self, node: &str, line: String) {
        if self.0.get(node) != Some(&line) {
            eprintln!("devhub autosync: {node} {line}");
            self.0.insert(node.to_string(), line);
        }
    }
}

/// What a node did with an apply order.
#[derive(Debug, PartialEq)]
enum Answer {
    Applied,
    /// a timed lease runs there
    Waits,
    Failed,
}

/// What the rounds and the pins' moves share.
struct Follow {
    waits: Mutex<Waits>,
    applied: Mutex<Applied>,
}

impl Follow {
    fn waits(&self) -> std::sync::MutexGuard<'_, Waits> {
        self.waits.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes in what `node` answered an order to move to `to`.
    fn answer(&self, report: &mut Report, node: &str, to: &Pin, after: &str, result: Result<Pin, String>) -> Answer {
        match result {
            Ok(revs) => {
                report.say(node, format!("applied {}{after}", revs.key()));
                self.waits().done(node);
                let mut applied = self.applied.lock().unwrap_or_else(|e| e.into_inner());
                applied.revs.insert(node.to_string(), revs);
                applied.dirty = true;
                // (a failed save stays dirty: the watcher's next round says it and tries again)
                let _ = applied.save();
                Answer::Applied
            }
            Err(e) => {
                let waits = e.contains("\"waits: ");
                if waits {
                    self.waits().wait(node, format!("its deployment moves to {}", to.short()));
                } else {
                    // it failed or was unreachable: it waits for no timed run
                    self.waits().done(node);
                }
                report.say(node, format!("skip {}: {e}", to.key()));
                if waits { Answer::Waits } else { Answer::Failed }
            }
        }
    }
}

/// The moves to pins: which run, and what the last ones said.
#[derive(Default)]
struct PinMoves {
    /// the nodes whose move runs now
    running: BTreeSet<String>,
    /// the pin each node moved to last: its `caps.deploy` says it a few seconds later
    moved: BTreeMap<String, String>,
    /// no new order before this after a failed one (each one that fails says so on the node)
    not_before: BTreeMap<String, Instant>,
    report: Report,
}

/// A node's move to a pin: the commits it applied, or why not.
type MoveTo<'a> = Arc<dyn Fn(&str, &Pin) -> Result<Pin, String> + Send + Sync + 'a>;

/// One look at the pinned nodes (node, its pin): a node that holds its pin waits for no update,
/// whatever an earlier order said (it reached the pin some other way: a move by hand, a round's
/// order sent before the pin); any other moves to the pin now, on its own thread.
fn pin_pass<'s>(scope: &'s std::thread::Scope<'s, '_>, follow: &'s Follow, moves: &'s Mutex<PinMoves>, pinned: Vec<(String, (String, String))>, deploys: &BTreeMap<String, String>, has: impl Fn(&str, &str) -> bool, failed_wait: Duration, apply: MoveTo<'s>) {
    for (node, pin) in pinned {
        let mut m = moves.lock().unwrap_or_else(|e| e.into_inner());
        if m.running.contains(&node) {
            continue;
        }
        let deploy = deploys.get(&node).map_or("", String::as_str);
        match deploy_to(Some(pin), deploy, |d, a| m.moved.get(&node).is_some_and(|at| at == a) || has(d, a)) {
            DeployTo::Tips => {}
            DeployTo::Held { .. } => {
                drop(m);
                follow.waits().done(&node);
            }
            DeployTo::Pin(_) if m.not_before.get(&node).is_some_and(|t| *t > Instant::now()) => {}
            DeployTo::Pin(at) => {
                m.running.insert(node.clone());
                drop(m);
                let apply = apply.clone();
                scope.spawn(move || {
                    let to = Pin::parse(&at).ok_or_else(|| format!("`{at}` is no pin"));
                    let result = to.as_ref().map_err(String::clone).and_then(|to| apply(&node, to));
                    let mut m = moves.lock().unwrap_or_else(|e| e.into_inner());
                    let answer = follow.answer(&mut m.report, &node, &to.unwrap_or_default(), "", result);
                    m.not_before.remove(&node);
                    match answer {
                        Answer::Applied => { m.moved.insert(node.clone(), at.clone()); }
                        Answer::Waits => {}
                        Answer::Failed => { m.not_before.insert(node.clone(), Instant::now() + failed_wait); }
                    }
                    m.running.remove(&node);
                });
            }
        }
    }
}

/// A box that catches up for a timed ask moves to that ask's pin now, whatever the rounds are doing: asked again
/// every [`WAITS_RETRY_S`] while a timed run there says `waits:`, so the gap after that run is
/// its update's. In a round it came after that round's batches, each waiting for its nodes'
/// builds: the box sat idle with its ask 11 min behind them (11-04), and a landing's kick does
/// not reach a round that runs.
fn follow_pins(control_socket: PathBuf, follow: Arc<Follow>, pin_of: PinOf) {
    let c = Client::new(Target::Unix(control_socket), "devhub-autosync-pins");
    let home = proto::home();
    let moves = Mutex::new(PinMoves::default());
    let c = &c;
    std::thread::scope(|s| loop {
        let look = (|| -> Result<(), String> {
            let conf = read_conf(&home)?;
            if !conf.enabled || conf.repo.as_os_str().is_empty() {
                return Ok(());
            }
            let nodes = match conf.nodes {
                Some(nodes) => nodes,
                None => fleet_nodes(c)?,
            };
            let pinned: Vec<_> = nodes.iter().filter_map(|n| Some((n.clone(), pin_of(n)?))).collect();
            if pinned.is_empty() {
                return Ok(());
            }
            let status = ask_local(c, "nodes", "nodes").unwrap_or_default();
            let (_, _, deploys) = nodes_to_apply(&nodes, &status, "");
            let repo = conf.repo;
            let order = repo.clone();
            let apply: MoveTo = Arc::new(move |node: &str, to: &Pin| {
                let mut lines = Vec::new();
                let code = push_and_apply(c, &order, &super::followed(&order), Some(&to.key()), &[node.to_string()], "on", "auto", false, "", &mut |n, l| if n == node { lines.push(l.to_string()) });
                apply_result(code, &lines, to)
            });
            pin_pass(s, &follow, &moves, pinned, &deploys, |d, a| super::deploy_has(&repo, d, a), Duration::from_secs(conf.interval_s), apply);
            Ok(())
        })();
        if let Err(e) = look {
            moves.lock().unwrap_or_else(|e| e.into_inner()).report.say("pins", e);
        }
        std::thread::sleep(Duration::from_secs(WAITS_RETRY_S));
    })
}

fn read_conf(home: &Path) -> Result<AutoConf, String> {
    let text = match std::fs::read_to_string(home.join("autosync.conf")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("autosync.conf: {e}")),
    };
    Ok(AutoConf::parse(&text, &makepad_app_registry::repo_root().unwrap_or_default()))
}

pub fn start(control_socket: PathBuf, say_waits: SayWaits, pin_of: PinOf) {
    let home = proto::home();
    let follow = Arc::new(Follow { waits: Mutex::new(Waits { on: BTreeSet::new(), say: say_waits }), applied: Mutex::new(Applied::load(home.join("autosync.state"))) });
    {
        let (socket, follow, pin_of) = (control_socket.clone(), follow.clone(), pin_of.clone());
        if let Err(e) = std::thread::Builder::new().name("devhub-autosync-pins".into()).spawn(move || follow_pins(socket, follow, pin_of)) {
            eprintln!("devhub autosync: cannot start the pins' watcher: {e}");
        }
    }
    if let Err(e) = std::thread::Builder::new().name("devhub-autosync".into()).spawn(move || {
        let c = Client::new(Target::Unix(control_socket), "devhub-autosync");
        let mut last_node = None;
        let mut report = Report::default();
        let mut interval_s = 30;
        let mut landed: Option<u64> = None;
        loop {
            // a node that answered `waits:` (a timed lease runs there) is asked again soon: the
            // gap between two timed runs is seconds
            let mut waiting = false;
            let round = (|| -> Result<(), String> {
                let conf = read_conf(&home)?;
                interval_s = conf.interval_s;
                if !conf.enabled {
                    follow.waits().keep(&[]);
                    set_held(BTreeMap::new());
                    return Ok(());
                }
                if conf.repo.as_os_str().is_empty() {
                    return Err("no checkout found; set repo in autosync.conf".into());
                }
                let follows = super::followed(&conf.repo);
                let tips = tips(&conf.repo, &follows)?;
                let nodes = match conf.nodes {
                    Some(nodes) => nodes,
                    None => fleet_nodes(&c)?,
                };
                if nodes.is_empty() {
                    follow.waits().keep(&[]);
                    set_held(BTreeMap::new());
                    return Ok(());
                }
                let status = ask_local(&c, "nodes", "nodes").unwrap_or_default();
                let (todo, idle, deploys) = nodes_to_apply(&nodes, &status, &tips.key());
                let users = user_wms(&status);
                // a node a timed ask pinned moves to its pin ([`follow_pins`]), or holds it until
                // the ask starts; the user's WM node moves to the newest proven tip, or holds
                let mut pinned: Vec<String> = Vec::new();
                let mut proven: BTreeMap<String, String> = BTreeMap::new();
                let mut holding: BTreeMap<String, String> = BTreeMap::new();
                let todo: Vec<String> = todo.into_iter().filter(|n| {
                    let deploy = deploys.get(n).map_or("", String::as_str);
                    match deploy_to(pin_of(n), deploy, |d, a| super::deploy_has(&conf.repo, d, a)) {
                        DeployTo::Tips if users.contains(n) => match proven_move(&home, &conf.repo, deploy, &follows) {
                            Ok(at) => {
                                proven.insert(n.clone(), at);
                                true
                            }
                            Err(why) => {
                                report.say(n, why.clone());
                                holding.insert(n.clone(), why);
                                false
                            }
                        },
                        DeployTo::Tips => true,
                        DeployTo::Pin(_) => {
                            pinned.push(n.clone());
                            false
                        }
                        DeployTo::Held { .. } => false,
                    }
                }).collect();
                set_held(holding);
                // at the tips, held at a pin, or no longer followed: its apply waits for nothing
                follow.waits().keep(&todo.iter().chain(pinned.iter()).cloned().collect::<Vec<_>>());
                let suits = |n: &String| conf.prewarm && idle.get(n).copied().unwrap_or(false);
                let batches: Vec<Vec<String>> = if landed.is_some() {
                    // a landing: the first listed node (the user's) alone, then every other at once
                    // (one order per batch, so the ones to pre-warm go together)
                    let mut todo = todo.into_iter();
                    let first: Vec<String> = todo.next().into_iter().collect();
                    let (warm, cold): (Vec<String>, Vec<String>) = todo.partition(|n| suits(n));
                    [first, warm, cold].into_iter().filter(|b| !b.is_empty()).collect()
                } else {
                    // one node, rotating after every attempt (a refusal or an unreachable node too)
                    let first = last_node.as_ref().and_then(|last| nodes.iter().position(|n| n == last)).map_or(0, |i| (i + 1) % nodes.len());
                    let next = (0..nodes.len()).map(|i| &nodes[(first + i) % nodes.len()]).find(|n| todo.contains(n));
                    next.map(|n| vec![n.clone()]).into_iter().collect()
                };
                // a user's WM node goes alone, to its proven tip
                let batches: Vec<(Vec<String>, Option<String>)> = batches.into_iter().flat_map(|batch| {
                    let (own, rest): (Vec<String>, Vec<String>) = batch.into_iter().partition(|n| proven.contains_key(n));
                    let own: Vec<(Vec<String>, Option<String>)> = own.into_iter().map(|n| { let at = proven.get(&n).cloned(); (vec![n], at) }).collect();
                    own.into_iter().chain((!rest.is_empty()).then_some((rest, None)))
                }).collect();
                let extra = landed.map_or(String::new(), |ms| format!(" landed={ms}"));
                for (batch, at) in batches {
                    // a timed ask that got first in line since the round began: its pin's
                    let batch: Vec<String> = batch.into_iter().filter(|n| pin_of(n).is_none()).collect();
                    if batch.is_empty() {
                        continue;
                    }
                    last_node = batch.last().cloned();
                    let prewarm = batch.iter().all(|n| suits(n));
                    let mut lines = BTreeMap::<String, Vec<String>>::new();
                    let code = push_and_apply(&c, &conf.repo, &follows, at.as_deref(), &batch, "on", "auto", prewarm, &extra, &mut |n, l| lines.entry(n.to_string()).or_default().push(l.to_string()));
                    let to = at.as_deref().and_then(Pin::parse).unwrap_or_else(|| tips.clone());
                    for node in &batch {
                        let mine = lines.remove(node).unwrap_or_default();
                        // a batch's exit code is every node's: one node's is its own error lines
                        let code = if batch.len() == 1 { code } else { mine.iter().any(|l| l.starts_with("{\"err\":")) as i32 };
                        let after = landed.map_or(String::new(), |ms| format!(" ({} ms after the landing)", proto::wall_ms().saturating_sub(ms)));
                        waiting |= follow.answer(&mut report, node, &to, &after, apply_result(code, &mine, &to)) == Answer::Waits;
                    }
                }
                Ok(())
            })();
            if let Err(e) = round {
                report.say("", e);
            }
            // Retry a failed save even when no further node needs an apply.
            let mut applied = follow.applied.lock().unwrap_or_else(|e| e.into_inner());
            if applied.dirty {
                if let Err(e) = applied.save() {
                    report.say("state", format!("could not save autosync.state: {e}"));
                }
            }
            drop(applied);
            landed = wait_kick(Duration::from_secs(if waiting { WAITS_RETRY_S.min(interval_s) } else { interval_s }));
        }
    }) {
        eprintln!("devhub autosync: cannot start watcher: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prewarm_only_on_idle_dev_on_nodes() {
        let caps = |load: f64, avail: f64| crate::lease::Caps { cores: 16, load1: load, ram_gb: 64.0, ram_avail_gb: avail, ..Default::default() }.to_json();
        let line = |up: bool, dev: bool, leases: u32, draining: bool, c: String| makepad_strict_json::parse(format!("{{\"node\":\"n\",\"up\":{up},\"dev\":{dev},\"leases\":{leases},\"draining\":{draining},\"caps\":{c}}}").as_bytes()).unwrap();
        assert!(prewarm_suits(&line(true, true, 0, false, caps(2.0, 40.0))));
        assert!(!prewarm_suits(&line(true, false, 0, false, caps(2.0, 40.0))), "dev off");
        assert!(!prewarm_suits(&line(true, true, 1, false, caps(2.0, 40.0))), "busy");
        assert!(!prewarm_suits(&line(true, true, 0, true, caps(2.0, 40.0))), "draining");
        assert!(!prewarm_suits(&line(false, true, 0, false, caps(2.0, 40.0))), "down");
        assert!(!prewarm_suits(&line(true, true, 0, false, caps(16.0 * 1.25 + 1.0, 40.0))), "over its load limit");
        assert!(!prewarm_suits(&line(true, true, 0, false, caps(2.0, 4.0))), "low on RAM");
        assert!(!prewarm_suits(&line(true, true, 0, false, "null".into())), "no caps yet");
        let c = AutoConf::parse("prewarm = off\nlanes = off\nlanes_root = /w\n", Path::new("/r"));
        assert!(!c.prewarm && !c.lanes && c.lanes_root == Path::new("/w"));
    }

    /// A deployment is at the tips when each repository the tips name is at its commit there: one
    /// it lacks (a nested repository newly followed) or holds at another commit moves it, one it
    /// holds besides (no longer followed) does not.
    #[test]
    fn a_deployment_is_at_the_tips_by_each_repository_they_name() {
        let (a, b, c, e) = ("a".repeat(40), "b".repeat(40), "c".repeat(40), "e".repeat(40));
        let tips = Pin::at(&a).with("commercial", &c).with("repo-b", &e).key();
        assert!(at_tips(&tips, &tips));
        assert!(!at_tips(&Pin::at(&a).with("commercial", &c).key(), &tips), "repo-b newly followed");
        assert!(!at_tips(&Pin::at(&a).with("commercial", &c).with("repo-b", &b).key(), &tips));
        assert!(!at_tips(&Pin::at(&b).with("commercial", &c).with("repo-b", &e).key(), &tips));
        assert!(at_tips(&Pin::at(&a).with("commercial", &c).with("repo-b", &e).with("repo-c", &b).key(), &tips), "repo-c no longer followed");
        assert!(!at_tips("", &tips), "no deployment yet");
        let status = |deploy: &str| format!("{{\"node\":\"n\",\"up\":true,\"dev\":true,\"leases\":0,\"user\":false,\"caps\":{}}}", crate::lease::Caps { deploy: deploy.into(), cores: 16, ..Default::default() }.to_json());
        let (todo, _, _) = nodes_to_apply(&["n".to_string()], &[status(&Pin::at(&a).with("commercial", &c).key())], &tips);
        assert_eq!(todo, vec!["n"]);
    }

    #[test]
    fn deployment_follows_tips_while_the_work_tree_is_held() {
        let tips = Pin::at(&"a".repeat(40)).with("commercial", &"c".repeat(40)).key();
        let old = Pin::at(&"b".repeat(40)).with("commercial", &"d".repeat(40)).key();
        assert_eq!(tips, format!("{},commercial={}", "a".repeat(40), "c".repeat(40)), "the tips in a node's caps' form");
        let status = |node: &str, deploy: &str, tree: &str, leases: u32, user: bool| {
            let caps = crate::lease::Caps { deploy: deploy.into(), tree: tree.into(), cores: 16, ram_gb: 64.0, ram_avail_gb: 40.0, ..Default::default() };
            format!("{{\"node\":\"{node}\",\"up\":true,\"dev\":true,\"leases\":{leases},\"user\":{user},\"caps\":{}}}", caps.to_json())
        };
        let nodes = vec!["busy".into(), "current".into(), "idle".into(), "unknown".into()];
        let lines = vec![status("busy", &old, &tips, 3, true), status("current", &tips, &old, 2, false), status("idle", &old, &old, 0, false)];
        let (todo, idle, deploys) = nodes_to_apply(&nodes, &lines, &tips);
        assert_eq!(todo, vec!["busy", "idle", "unknown"]);
        assert_eq!(deploys.get("current"), Some(&tips));
        assert_eq!(idle.get("busy"), Some(&false), "the deployment applies under leases and user input, but pre-warm stays off");
        assert_eq!(idle.get("current"), Some(&false), "a held work tree at another pin does not trigger an apply");
        assert_eq!(idle.get("idle"), Some(&true));
    }

    /// A node sat idle 11 min with a timed ask first in line (11-04): the run before it had ended,
    /// the node would take the update, and the order came after a round's batches, each waiting
    /// for its nodes' builds. Its move runs beside them, once at a time per box.
    #[test]
    fn a_timed_asks_box_moves_to_its_pin_now_and_says_when_it_has() {
        let dir = std::env::temp_dir().join(format!("devhub-autosync-pin-{}", std::process::id()));
        let said = Arc::new(Mutex::new(Vec::<(String, Option<String>)>::new()));
        let s2 = said.clone();
        let follow = Follow {
            waits: Mutex::new(Waits { on: BTreeSet::new(), say: Box::new(move |n, why| s2.lock().unwrap().push((n.to_string(), why))) }),
            applied: Mutex::new(Applied { path: dir.join("autosync.state"), revs: BTreeMap::new(), dirty: false }),
        };
        let moves = Mutex::new(PinMoves::default());
        let (pin, later) = (format!("{},commercial={}", "a".repeat(40), "c".repeat(40)), format!("{},commercial={}", "b".repeat(40), "c".repeat(40)));
        let revs = |at: &str| Pin::parse(at).unwrap();
        let on = || follow.waits().on.contains("node-a");
        let (order_tx, order_rx) = std::sync::mpsc::channel::<String>();
        let (answer_tx, answer_rx) = std::sync::mpsc::channel::<Result<Pin, String>>();
        let (order_tx, answer_rx) = (Mutex::new(order_tx), Mutex::new(answer_rx));
        let settled = || while moves.lock().unwrap().running.contains("node-a") { std::thread::sleep(Duration::from_millis(1)); };
        // (the box's caps have not said a move yet)
        let deploys = BTreeMap::from([("node-a".to_string(), "old".to_string())]);
        std::thread::scope(|s| {
            let apply: MoveTo = Arc::new(|_: &str, at: &Pin| {
                order_tx.lock().unwrap().send(at.key()).unwrap();
                answer_rx.lock().unwrap().recv().unwrap()
            });
            let pass = |at: &str, holds: bool| pin_pass(s, &follow, &moves, vec![("node-a".to_string(), ("L2".to_string(), at.to_string()))], &deploys, |_, _| holds, Duration::from_secs(3600), apply.clone());
            // a round's order had said the box's timed run made it wait; that round builds on
            follow.waits().wait("node-a", "its deployment moves to old".into());
            pass(&pin, false);
            assert_eq!(order_rx.recv().unwrap(), pin, "the move does not wait for the round");
            pass(&pin, false);
            assert!(order_rx.try_recv().is_err(), "one order at a time per box");
            // the run there has not ended: asked again by the next look, and timed work still waits
            answer_tx.send(Err("{\"err\":\"waits: timed lease L1 runs here\"}".into())).unwrap();
            settled();
            assert!(on(), "the box says its update waits");
            pass(&pin, false);
            assert_eq!(order_rx.recv().unwrap(), pin, "the gap after the run is the update's");
            answer_tx.send(Ok(revs(&pin))).unwrap();
            settled();
            assert!(!on(), "moved: timed work starts");
            assert_eq!(follow.applied.lock().unwrap().revs.get("node-a"), Some(&revs(&pin)));
            pass(&pin, false);
            assert!(order_rx.try_recv().is_err(), "the caps are late: moved is moved");
            // the box holds the pin by some other way (a push by hand) and an old order's word
            // still stands: nothing is left to wait for
            follow.waits().wait("node-a", "its deployment moves to old".into());
            pass(&later, true);
            assert!(!on(), "a target the box passed holds nothing up");
            assert!(order_rx.try_recv().is_err(), "and takes no order");
            // a failed move holds no timed work, and its next order waits a while
            follow.waits().wait("node-a", "its deployment moves to old".into());
            pass(&later, false);
            assert_eq!(order_rx.recv().unwrap(), later);
            answer_tx.send(Err("link lost".into())).unwrap();
            settled();
            assert!(!on(), "it failed: it waits for no timed run");
            pass(&later, false);
            assert!(order_rx.try_recv().is_err(), "no order every few seconds to a box that fails them");
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_apply_that_waits_is_said_until_it_applies_or_needs_nothing() {
        let said = std::sync::Arc::new(Mutex::new(Vec::<(String, Option<String>)>::new()));
        let s2 = said.clone();
        let mut w = Waits { on: BTreeSet::new(), say: Box::new(move |n, why| s2.lock().unwrap().push((n.to_string(), why))) };
        w.wait("a", "x".into());
        w.wait("b", "y".into());
        w.done("a");
        w.done("a");
        // `b` is at the tips now (or no longer followed): its mark goes too
        w.keep(&["c".to_string()]);
        let none = None::<String>;
        assert_eq!(*said.lock().unwrap(), vec![("a".into(), Some("x".into())), ("b".into(), Some("y".into())), ("a".into(), none.clone()), ("b".into(), none)]);
    }

    /// The state keeps each node's applied pin, every repository of it; a line in another form
    /// is not read (that node's next apply writes it again). An apply's answer counts once it
    /// names a commit of every repository the order moved.
    #[test]
    fn the_state_and_an_apply_answer_carry_every_repository_of_the_pin() {
        let dir = std::env::temp_dir().join(format!("devhub-autosync-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, c, e) = ("a".repeat(40), "c".repeat(40), "e".repeat(40));
        let to = Pin::at(&a).with("commercial", &c).with("repo-b", &e);
        let mut applied = Applied { path: dir.join("autosync.state"), revs: BTreeMap::from([("node-a".to_string(), to.clone())]), dirty: true };
        applied.save().unwrap();
        let text = std::fs::read_to_string(dir.join("autosync.state")).unwrap();
        assert_eq!(text, format!("node-a {a},commercial={c},repo-b={e}\n"));
        std::fs::write(dir.join("autosync.state"), format!("{text}node-b {a} {c}\n")).unwrap();
        assert_eq!(Applied::load(dir.join("autosync.state")).revs, BTreeMap::from([("node-a".to_string(), to.clone())]), "an old three-word line is not read");
        let answer = |b: &str| vec![format!("{{\"applied\":{{\"makepad\":\"{a}\",\"commercial\":\"{c}\"{b},\"session\":\"\",\"ms\":5}}}}")];
        assert_eq!(apply_result(0, &answer(&format!(",\"repo-b\":\"{e}\"")), &to), Ok(to.clone()));
        assert!(apply_result(0, &answer(""), &to).is_err(), "a repository the order moved is missing from the answer");
        assert_eq!(apply_result(0, &answer(""), &Pin::at(&a).with("commercial", &c)), Ok(Pin::at(&a).with("commercial", &c)), "the answer of an order of makepad and commercial");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conf_parses() {
        let repo = Path::new("/checkout/makepad");
        let c = AutoConf::parse("# defaults\n", repo);
        assert!(c.enabled);
        assert_eq!(c.repo, repo);
        assert_eq!(c.interval_s, 30);
        assert_eq!(c.nodes, None);
        let c = AutoConf::parse(" enabled = off # paused\nrepo = /other tree\ninterval = 12\nnodes = a, b,,c, # fleet order\n", repo);
        assert!(!c.enabled);
        assert_eq!(c.repo, Path::new("/other tree"));
        assert_eq!(c.interval_s, 12);
        assert_eq!(c.nodes, Some(vec!["a".into(), "b".into(), "c".into()]));
        let c = AutoConf::parse("enabled = off\nenabled = on\ninterval = nope\ninterval = 0\nrepo =\nnodes =\nunknown = value\nmalformed", repo);
        assert!(c.enabled);
        assert_eq!(c.repo, repo);
        assert_eq!(c.interval_s, 30);
        assert_eq!(c.nodes, Some(Vec::new()));
    }

    #[test]
    fn the_users_wm_node_moves_to_the_newest_proven_tip_and_says_why_it_holds() {
        let line = |node: &str, role: crate::lease::Role| format!("{{\"node\":\"{node}\",\"up\":true,\"caps\":{}}}", crate::lease::Caps { role: Some(role), ..Default::default() }.to_json());
        let status = [line("box-wm", crate::lease::Role::UserWm), line("box-pool", crate::lease::Role::Pool), "{\"node\":\"box-new\",\"caps\":null}".to_string()];
        assert_eq!(user_wms(&status), BTreeSet::from(["box-wm".to_string()]));
        let home = std::env::temp_dir().join(format!("devhub-autosync-proven-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let at = |c: char| format!("{},commercial={}", c.to_string().repeat(40), "f".repeat(40));
        let row = |c: char, failing: &str| {
            let (w, m) = (c.to_string().repeat(40), "f".repeat(40));
            format!("{{\"bucket\":\"fast\",\"tip\":{{\"work\":\"{w}\",\"main\":\"{m}\"}},\"started_ms\":1,\"failing\":[{failing}],\"passed\":[],\"log\":\"l-{c}\"}}\n")
        };
        // (no checkout: a deployment holds only the tip it is at)
        let none = Path::new("/nonexistent-devhub-repo");
        let follows = super::super::followed(none);
        std::fs::write(home.join("buckets.jsonl"), row('a', "") + &row('b', "") + &row('c', "\"check pkg-x\"")).unwrap();
        assert_eq!(proven_move(&home, none, &at('a'), &follows), Ok(at('b')), "b proven, c has a new red unit");
        let held = proven_move(&home, none, &at('b'), &follows).unwrap_err();
        let (b, c, f) = ("b".repeat(10), "c".repeat(10), "f".repeat(10));
        assert!(held.starts_with(&format!("held at {b},{f}: tip {c},{f} has check pkg-x failing")) && held.ends_with("(log l-c)"), "{held}");
        assert_eq!(proven_move(&home, none, "", &follows), Ok(at('c')), "a box new to the rule starts at the newest run");
        // a run that also pinned a nested repository the deployments do not follow: the move
        // names only the ones they do
        let (d, e, f) = ("d".repeat(40), "e".repeat(40), "f".repeat(40));
        let mut rows = std::fs::read_to_string(home.join("buckets.jsonl")).unwrap();
        rows.push_str(&format!("{{\"bucket\":\"fast\",\"tip\":{{\"work\":\"{d}\",\"main\":\"{f}\",\"nested\":{{\"repo-b\":\"{e}\"}}}},\"started_ms\":1,\"failing\":[],\"passed\":[\"check pkg-x\"],\"log\":\"l-d\"}}\n"));
        std::fs::write(home.join("buckets.jsonl"), rows).unwrap();
        assert_eq!(proven_move(&home, none, &at('b'), &follows), Ok(at('d')));
        let _ = std::fs::remove_dir_all(&home);
    }
}
