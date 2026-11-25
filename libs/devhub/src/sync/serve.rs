//! The node side of sync, on the master endpoint (the master, or this machine's own user on the
//! control socket):
//!
//! - `sync` (lines): `status`; `apply <order>`; `wm-restart <order>`; `tick <pid> <hex inspect
//!   line>`. Orders are signed by the master ([`super::sign`]).
//! - `sync-push` (binary): the first message is a signed `push <repo> <branch> <sha> <bundle
//!   sha256> <bytes>` order, then the bundle's bytes. The bundle is checked (size, hash bound to
//!   the order, `git bundle verify`) and fetched into the repo's store; nothing in it runs.
//! - `launch` (lines): remote launch ([`super::launch`]).
//!
//! An apply moves the deployment checkout to the landed tips independently of leases, then
//! prebuilds and follows the order's safe restart rules. An idle work tree catches up too.
//! Every stage is a `sync` record (pushed, checkout, patched, built, restarted, refused), so the
//! master's inbox and `devhub stream` show it.

use super::build;
use super::conf::{Build, SyncConf};
use super::git::{self, Store, CHECKOUT_REF, SYNCED_REF};
use super::jits;
use super::sign::{self, Order, Seen};
use super::toast::{self, News, Outcome};
use super::NodeCtx;
use crate::proto::{self, hub_record, Sev};
use makepad_app_registry::machine::Launcher;
use makepad_network::digest::Sha256;
use makepad_remote::conn::{Event, CLOSE_REFUSED};
use makepad_remote::endpoint::Factory;
use makepad_remote::{json_str, lines, Endpoint};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The largest bundle a node takes (a first push of the whole history is well under it).
const MAX_BUNDLE: u64 = 8 << 30;
const CHUNK_WAIT: Duration = Duration::from_secs(120);

pub struct Sync {
    pub ctx: Arc<NodeCtx>,
    pub conf: Arc<SyncConf>,
    seen: Seen,
    busy: crate::held::Held<()>,
    /// One WM restart at a time, whoever asks it (an order, an apply, a waiter in a lease's gap):
    /// each sees the restart before it ([`restart_once`]). Held for the restart alone: a waiter
    /// that finds a gap takes it, and never the sync lock an apply holds through its build.
    restart: Mutex<()>,
    last: Mutex<String>,
    /// the JITs already told to restart onto the new binary: said once each, not every sync
    stale_said: Mutex<std::collections::BTreeSet<(u32, String)>>,
}

/// The words of an apply order that are not repositories.
const ORDER_WORDS: [&str; 11] = ["at", "build", "jit", "jobs", "landed", "prewarm", "repoint", "restart", "session", "tickcmd", "wait_ms"];

/// Refuse an unread word before an apply moves or builds anything.
fn order_unknown(o: &Order) -> Option<String> {
    o.words.iter().skip(1).find(|word| {
        let Some((key, value)) = word.split_once('=') else { return true };
        !ORDER_WORDS.contains(&key) && !((key == "makepad" || crate::nested::valid(key)) && git::valid_sha(value))
    }).cloned()
}

/// The repositories an apply order moves, each to the commit it names: `makepad=<sha>` and each
/// nested repository's `<name>=<sha>`, makepad first.
fn order_repos(o: &Order) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = o.words.iter().skip(1).filter_map(|w| w.split_once('='))
        .filter(|(k, _)| (*k == "makepad" || crate::nested::valid(k)) && !ORDER_WORDS.contains(k))
        .map(|(k, v)| (k.to_string(), v.to_string())).collect();
    out.sort_by_key(|(k, _)| k != "makepad");
    out
}

/// The deployment's pin once `moved` (repository, commit) moved it from `was` (its
/// FLEET-REVISION.txt's): None while it holds no makepad commit.
fn deployed_after(was: Option<crate::lease::Pin>, moved: &[(String, String)]) -> Option<crate::lease::Pin> {
    let mut pin = was.unwrap_or_default();
    for (repo, sha) in moved {
        if repo == "makepad" { pin.makepad = sha.clone() } else { pin.nested.insert(repo.clone(), sha.clone()); }
    }
    (!pin.makepad.is_empty()).then_some(pin)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn err_line(e: &str) -> String {
    format!("{{\"err\":{}}}", json_str(e))
}

impl Sync {
    pub fn new(ctx: Arc<NodeCtx>, conf: SyncConf) -> Arc<Sync> {
        Arc::new(Sync { ctx, conf: Arc::new(conf), seen: Seen::default(), busy: crate::held::Held::new("sync apply", ()), restart: Mutex::new(()), last: Mutex::new("null".into()), stale_said: Mutex::new(Default::default()) })
    }

    fn event(&self, sev: Sev, line: String, detail: String) {
        (self.ctx.publish)(hub_record(&self.ctx.machine, "", 0, "sync", sev, line, detail));
    }

    fn jit_event(&self, sev: Sev, app: &str, pid: u32, line: String, detail: String) {
        (self.ctx.publish)(hub_record(&self.ctx.machine, app, pid, "sync", sev, line, detail));
    }

    pub fn register(self: &Arc<Self>, ep: &Endpoint) {
        let s = self.clone();
        ep.register("sync", 1, lines::service("sync", Arc::new(move |line, out| s.sync_line(line, out))));
        ep.register("sync-push", 1, push_factory(self.clone()));
        ep.register("launch", 1, lines::service("launch", super::launch::service(self.conf.clone(), self.ctx.publish.clone(), self.ctx.machine.clone())));
        ep.register("put", 1, lines::service("put", super::put::service(self.conf.clone(), self.ctx.publish.clone(), self.ctx.machine.clone())));
        self.after_wm_restart();
        #[cfg(target_os = "linux")]
        self.roll_other_wire();
    }

    /// A WM on another hosted wire hosts no app of today (Fab could not start on a node,
    /// 2026-11-02; caps reports no display meanwhile): restarted onto the deployed one as soon as
    /// nothing is lost by it ([`Self::wm_restart`] waits for apps, viewers and the person). Never
    /// on a box a person used in the last 12 h (their session is theirs to restart), and at most
    /// once in 6 h. Before that, the WM restart that the hub before this one waited for goes on
    /// waiting ([`Self::resume_wm_restart`]).
    #[cfg(target_os = "linux")]
    fn roll_other_wire(self: &Arc<Self>) {
        let me = self.clone();
        let _ = std::thread::Builder::new().name("devhub-wm-wire".into()).spawn(move || {
            let mut last: Option<Instant> = None;
            let mut nap = Duration::from_secs(5);
            loop {
                std::thread::sleep(nap);
                nap = Duration::from_secs(60);
                me.resume_wm_restart();
                let Some(answer) = crate::lease::caps::other_wire() else { continue };
                if crate::lease::exec::person_within(12 * 3_600_000) || last.is_some_and(|t| t.elapsed() < Duration::from_secs(6 * 3600)) || !crate::lease::exec::writes_allowed() {
                    continue;
                }
                last = Some(Instant::now());
                let asked = wm_generation();
                let _busy = me.busy.lock().unwrap();
                if let Err(e) = me.wm_restart(&format!("its WM speaks another hosted wire ({answer}): no app starts under it"), asked) {
                    me.event(Sev::Warn, format!("sync: WM restart refused: {e}"), String::new());
                }
            }
        });
    }

    /// The commit the checkout of `repo` holds: the store's record of it, else what a deploy
    /// script wrote (FLEET-REVISION.txt).
    fn current(&self, store: &Store, repo: &str) -> Option<String> {
        store.rev(CHECKOUT_REF).or_else(|| git::fleet_revision(&self.conf.checkout, repo))
    }

    fn status(&self, out: &mut Vec<String>) {
        let mut repos = Vec::new();
        // makepad, the paired nested repository the sync deploys, and every store a lease pushed
        let names: std::collections::BTreeSet<String> = ["makepad", crate::state::PAIRED].into_iter().map(str::to_string).chain(git::store_names()).collect();
        for repo in names.iter().map(String::as_str) {
            let Ok(store) = Store::open(repo) else { continue };
            let cur = self.current(&store, repo);
            let refs: Vec<String> = store.refs().iter().map(|(r, sha)| format!("{{\"ref\":{},\"sha\":{}}}", json_str(r), json_str(sha))).collect();
            // a tree a deploy script shipped names its commit before the store holds it
            let held = cur.as_deref().is_some_and(|c| store.has(c));
            repos.push(format!("{}:{{\"checkout\":{},\"held\":{held},\"refs\":[{}]}}", json_str(repo), cur.map_or("null".into(), |c| json_str(&c)), refs.join(",")));
        }
        out.push(format!(
            "{{\"machine\":{},\"checkout\":{},\"build\":\"{}\",\"wm_service\":{},\"dev\":{},\"busy\":{},\"prewarm\":{},\"stale\":{},\"waiting\":{},\"repos\":{{{}}},\"last\":{}}}",
            json_str(&self.ctx.machine),
            json_str(&self.conf.checkout.display().to_string()),
            match self.conf.build {
                Build::Wm => "wm",
                Build::Apps => "apps",
                Build::Off => "off",
            },
            self.conf.wm_service.as_deref().map_or("null".into(), json_str),
            crate::lease::exec::writes_allowed(),
            self.busy.try_lock().is_err(),
            super::prewarm::running(),
            json_str(&stale_runs(&self.conf.checkout, proto::wall_ms()).0),
            json_str(&waiting_apps(proto::wall_ms())),
            repos.join(","),
            self.last.lock().unwrap()
        ));
    }

    fn sync_line(self: &Arc<Self>, line: &str, out: &mut Vec<String>) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["status"] => self.status(out),
            ["apply", order] => match self.seen.check(order) {
                Ok(o) if o.words.first().map(String::as_str) == Some("apply") => self.apply(&o, out),
                Ok(_) => out.push(err_line("not an apply order")),
                Err(e) => self.refuse(out, format!("sync: apply refused: {e}")),
            },
            ["prune", order] => match self.seen.check(order) {
                Ok(o) => match o.words.as_slice() {
                    [w, repo, branches @ ..] if w == "prune" && git::valid_repo(repo) && branches.iter().all(|b| git::valid_branch(b)) => {
                        let branches: Vec<&str> = branches.iter().map(String::as_str).collect();
                        match Store::open(repo).and_then(|s| s.prune_pushed(&branches)) {
                            Ok(()) => out.push(format!("{{\"pruned\":{}}}", branches.len())),
                            Err(e) => out.push(err_line(&e)),
                        }
                    }
                    _ => out.push(err_line("not a prune order")),
                },
                Err(e) => self.refuse(out, format!("sync: prune refused: {e}")),
            },
            ["wm-restart", order] => match self.seen.check(order) {
                Ok(o) if o.words.first().map(String::as_str) == Some("wm-restart") => {
                    // asked now: a restart made while it waits for an apply is the one it asked for
                    let asked = wm_generation();
                    let _busy = self.busy.lock().unwrap();
                    match self.wm_restart(&format!("asked by {}", o.signer), asked) {
                        Ok(l) => out.push(l),
                        Err(e) => self.refuse(out, format!("sync: WM restart refused: {e}")),
                    }
                }
                Ok(_) => out.push(err_line("not a wm-restart order")),
                Err(e) => self.refuse(out, format!("sync: WM restart refused: {e}")),
            },
            ["tick", pid, cmd] => {
                let cmd = sign::unhex(cmd).and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
                match pid.parse::<u32>().map_err(|e| e.to_string()).and_then(|pid| jits::Ticker::new(pid, &cmd)?.read()) {
                    Ok(t) => out.push(format!("{{\"tick\":{t},\"at_ms\":{}}}", proto::wall_ms())),
                    Err(e) => out.push(err_line(&e)),
                }
            }
            _ => out.push(err_line("sync status | apply <order> | prune <order> | wm-restart <order> | tick <pid> <hex inspect line>")),
        }
    }

    fn refuse(&self, out: &mut Vec<String>, why: String) {
        self.event(Sev::Warn, why.clone(), String::new());
        out.push(err_line(&why));
    }

    /// Move the deployment independently of leases, follow live patches, prebuild and apply
    /// the order's safe restart rules; warm the work tree only when idle.
    fn apply(self: &Arc<Self>, o: &Order, out: &mut Vec<String>) {
        if let Some(word) = order_unknown(o) {
            return self.refuse(out, format!("sync: apply refused: this hub does not read the word `{word}`"));
        }
        let Ok(_busy) = self.busy.try_lock() else { return out.push(err_line("an apply is running on this node")) };
        // its hub does not restart under it (the build would start over, 10-30). Said before it
        // looks for a timed lease: a timed start marks first and looks at this after, so one of
        // the two always sees the other. A hub that promised its restart exits in a moment: no
        // apply starts to be cut by it ([`restart_begin`]).
        if !begin_beside(&APPLYING, &RESTARTING) {
            return out.push(err_line("waits: this hub restarts for its self-update"));
        }
        struct Applying;
        impl Drop for Applying {
            fn drop(&mut self) {
                if MOVED_IN_APPLY.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    MOVED_MS.store(proto::wall_ms(), std::sync::atomic::Ordering::SeqCst);
                }
                APPLYING.store(false, std::sync::atomic::Ordering::SeqCst);
                *BUILDING_JIT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
        let _applying = Applying;
        // a timed lease runs alone: the deployment moves after it. Moved under it, the tree
        // rebuilds under the run: this apply's build, and the hosted JIT apps' (the WM, the AI hub)
        // own rebuilds of the makepad they run (rustc at 265% of a core outside a timed lease on a
        // bench box, 11-03). The master asks again (`waits:`).
        if let Some(timed) = timed_here() {
            return out.push(err_line(&format!("waits: {timed} runs here: the deployment moves after it")));
        }
        if HUB_BUILDING.load(std::sync::atomic::Ordering::SeqCst) {
            return out.push(err_line("waits: its hub builds its next self from the checkout: the deployment moves after it"));
        }
        let moves = order_repos(o);
        // the commits this apply builds the box's JIT of: a gate of exactly them waits for it
        if self.conf.jit_app().is_some() && o.get("build") != Some("off") {
            if let Some(pin) = deployed_after(git::fleet_pin(&self.conf.checkout), &moves) {
                *BUILDING_JIT.lock().unwrap_or_else(|e| e.into_inner()) = Some(pin);
            }
        }
        let t0 = Instant::now();
        let mut plan: Vec<(&str, Store, Option<String>, String)> = Vec::new();
        for (repo, to) in &moves {
            let (repo, to) = (repo.as_str(), to.as_str());
            let store = match Store::open(repo) {
                Ok(s) => s,
                Err(e) => return self.refuse(out, format!("sync: {repo} store: {e}")),
            };
            if !store.has(to) {
                return self.refuse(out, format!("sync: apply refused: {repo} {} was not pushed to this node", short(to)));
            }
            let dir = self.conf.repo_dir(repo).unwrap();
            let from = self.current(&store, repo);
            // forward from what the last sync applied
            let synced = store.rev(SYNCED_REF).or_else(|| from.clone());
            match &from {
                Some(f) if !store.has(f) => return self.refuse(out, format!("sync: apply refused: the {repo} checkout is at {}, which this node's store does not hold (push a branch that contains it)", short(f))),
                // `repoint=<repo>`: the master vouches that what this node synced sits on a retired chain
                // (`devhub promote` re-pointed the branch): the one move across histories
                _ if !o.get("repoint").is_some_and(|r| r == repo) && synced.as_deref().is_some_and(|s| s != to && store.has(s) && !store.is_ancestor(s, to)) => {
                    return self.refuse(out, format!("sync: apply refused: {repo} {} -> {} is not a fast-forward", short(synced.as_deref().unwrap_or("")), short(to)));
                }
                None if dir.read_dir().is_ok_and(|mut d| d.next().is_some()) && !(repo == "makepad" && only_commercial(&dir)) => {
                    return self.refuse(out, format!("sync: apply refused: {} holds files but no known commit (no FLEET-REVISION.txt)", dir.display()));
                }
                _ => {}
            }
            plan.push((repo, store, from, to.to_string()));
        }
        if plan.is_empty() {
            return out.push(err_line("apply names no commit (makepad=<sha> <nested repository>=<sha>..)"));
        }
        for (repo, store, from, _) in &plan {
            let Some(from) = from.as_deref() else { continue };
            let dir = self.conf.repo_dir(repo).unwrap();
            match store.repair(&dir, from) {
                Ok(lines) => for line in lines {
                    self.event(Sev::Warn, format!("sync: {repo} {}: put back after a cut-short move: {line}", dir.display()), String::new());
                },
                Err(e) => return self.refuse(out, format!("sync: apply waits: {}: {e}", dir.display())),
            }
        }
        // edits in a checkout that its move would overwrite are someone's work: they stay, and no
        // repository of the deployment moves until they are gone (no move half done, no reset)
        for (repo, store, from, to) in &plan {
            let Some(from) = from.as_deref().filter(|f| f != to) else { continue };
            let dir = self.conf.repo_dir(repo).unwrap();
            if let Err(e) = store.check_move(&dir, from, to) {
                return self.refuse(out, format!("sync: apply waits: {} holds edits its move to {repo} {} would overwrite, kept: {e}", dir.display(), short(to)));
            }
        }
        let session = o.get("session").unwrap_or("").to_string();
        // the JITs that run from this checkout, and where their patch records start
        let watched: Vec<(jits::Jit, PathBuf, u64)> = jits::list()
            .into_iter()
            .filter(|j| jits::runs_from(j.pid, &self.conf.checkout))
            .map(|j| {
                let log = jits::live_log(j.pid);
                let len = jits::log_len(&log);
                (j, log, len)
            })
            .collect();

        // coordinated patch: the files move when the session's JIT reaches the agreed tick
        let mut session_line = String::new();
        let mut ticker = None;
        if let (Some(at), Some(pid)) = (o.get("at").and_then(|t| t.parse::<u64>().ok()), o.get("jit").and_then(|p| p.parse::<u32>().ok())) {
            let cmd = o.get("tickcmd").and_then(sign::unhex).and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
            let deadline = Instant::now() + Duration::from_millis(o.get("wait_ms").and_then(|w| w.parse().ok()).unwrap_or(60_000));
            let t = match jits::Ticker::new(pid, &cmd) {
                Ok(t) => t,
                Err(e) => return self.refuse(out, format!("sync: session {session}: {e}")),
            };
            let reached = loop {
                match t.read() {
                    Ok(now) if now >= at => break Ok(now),
                    Ok(_) if Instant::now() > deadline => break Err(format!("tick {at} not reached in time")),
                    Ok(_) => std::thread::sleep(Duration::from_millis(2)),
                    Err(e) => break Err(e),
                }
            };
            match reached {
                Ok(now) => session_line = format!("session {session}: tick {now} (target {at}) at {} ms", proto::wall_ms()),
                Err(e) => return self.refuse(out, format!("sync: session {session}: apply refused: {e}")),
            }
            ticker = Some((pid, t));
        }

        // what this sync brings, for the toasts of the apps it reaches (the nested repositories
        // first: the apps' own changes)
        let mut news = News { subjects: Vec::new(), landed_ms: o.get("landed").and_then(|t| t.parse().ok()) };
        for (_, store, from, to) in plan.iter().filter(|p| p.0 != "makepad").chain(plan.iter().filter(|p| p.0 == "makepad")) {
            if let Some(from) = from {
                news.subjects.extend(store.subjects(from, to, 200));
            }
        }

        // a running pre-warm is for the commits the deployment leaves now
        super::prewarm::cancel("a newer sync");
        let mut moved_lines = Vec::new();
        let mut revs: Vec<(String, String)> = Vec::new();
        for (repo, store, from, to) in &plan {
            let dir = self.conf.repo_dir(repo).unwrap();
            let r = store.move_checkout(&dir, from.as_deref(), to).and_then(|m| store.set_ref(SYNCED_REF, to).map(|_| m));
            match r {
                Ok(m) => {
                    for line in &m.repaired {
                        self.event(Sev::Warn, format!("sync: {repo} {}: put back after a cut-short move: {line}", dir.display()), String::new());
                    }
                    if !m.files.is_empty() {
                        MOVED_IN_APPLY.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    let line = format!("sync: {repo} {} -> {} ({} files){}", from.as_deref().map_or("(new)", short), short(to), m.files.len(), if session_line.is_empty() { String::new() } else { format!(", {session_line}") });
                    let detail = format!("{{\"repo\":{},\"from\":{},\"to\":{},\"files\":[{}]}}", json_str(repo), from.as_deref().map_or("null".into(), json_str), json_str(to), m.files.iter().take(64).map(|f| json_str(f)).collect::<Vec<_>>().join(","));
                    self.event(Sev::Info, line.clone(), detail);
                    moved_lines.push(format!("{{\"repo\":{},\"from\":{},\"to\":{},\"files\":{}}}", json_str(repo), from.as_deref().map_or("null".into(), json_str), json_str(to), m.files.len()));
                }
                Err(e) => return self.refuse(out, format!("sync: {repo} checkout refused: {e}")),
            }
            revs.push((repo.to_string(), to.clone()));
        }
        let work = crate::lease::caps::LeaseConf::load().work;
        for (repo, _, _, to) in &plan {
            match crate::lease::relaxgit::feed(&work, repo, to, "work") {
                Ok((objects, bytes)) => self.event(Sev::Info, format!("sync: {repo} RELAXGIT {objects} objects, {bytes} bytes"), String::new()),
                Err(e) => return self.refuse(out, e),
            }
        }
        let deployed = deployed_after(git::fleet_pin(&self.conf.checkout), &revs);
        if let Some(pin) = &deployed {
            git::write_fleet_revision(&self.conf.checkout, pin);
            // the dev run dirs' older Relax builds go, off the apply's path (a scan of every
            // store's header, then the files of the builds it retires)
            let me = self.clone();
            let _ = std::thread::Builder::new().name("devhub-sync-retire".into()).spawn(move || {
                let (n, bytes) = crate::jitcache::deployment_moved(&me.conf.checkout);
                if n > 0 {
                    me.event(Sev::Info, format!("sync: the dev run dirs and the tree runs' targets retired {n} older Relax builds' stores ({:.1} GB): no run used them since the deployment before this one", bytes as f64 / 1e9), String::new());
                }
            });
            let conf = crate::lease::caps::LeaseConf::load();
            if conf.slots(0).iter().any(|(k, n)| k.writes() && *n > 0) {
                match crate::lease::tree::sync(&conf.work, pin) {
                    Ok(Some(t)) => {
                        for b in t.cleared.bundles {
                            self.event(Sev::Warn, format!("sync: work tree edits recovered to {}", b.display()), String::new());
                        }
                        for e in t.cleared.unclean {
                            self.event(Sev::Err, format!("sync: work tree: {e}"), String::new());
                        }
                        self.event(Sev::Info, format!("sync: work tree at {} ({} files)", pin.short(), t.moved.iter().map(|(_, n)| n).sum::<usize>()), String::new());
                    }
                    Ok(None) => {}
                    Err(e) => self.event(Sev::Err, format!("sync: work tree: {e}"), String::new()),
                }
            }
        }
        out.extend(moved_lines);

        // what the JITs' live patchers did with it: a coordinated patch answers with it (the
        // master reads the ticks), otherwise the records follow on the stream
        if ticker.is_some() {
            out.extend(self.follow_patches(watched, ticker, &session, None));
        } else if !watched.is_empty() {
            let me = self.clone();
            let _ = std::thread::Builder::new().name("devhub-sync-patches".into()).spawn(move || {
                me.follow_patches(watched, None, "", Some(&news));
            });
        }

        // prebuild
        let wm_bin = self.conf.checkout.join("target/release/makepad-app-wm");
        let wm_before = build::mtime(&wm_bin);
        let mut built_line = "null".to_string();
        let mut built_ok = true;
        if o.get("build") != Some("off") && self.conf.build != Build::Off {
            self.event(Sev::Info, format!("sync: building ({:?})", self.conf.build).to_lowercase(), String::new());
            // the JIT it builds is kept by content: the work tree's leases (a gate) run on it
            let target = self.conf.checkout.join("target");
            let jit = self.conf.jit_app().is_some() && std::fs::create_dir_all(target.join("host")).and_then(|_| std::fs::write(target.join(crate::lease::loaders::MARK), "")).is_ok();
            let jit_built = || {
                if jit {
                    match crate::lease::loaders::publish(&self.conf.checkout, &target, None) {
                        Ok(Some(k)) => self.event(Sev::Info, format!("sync: kept the JIT binary {k} for this box's tree leases"), String::new()),
                        Ok(None) => {}
                        Err(e) => self.event(Sev::Info, format!("sync: the deployment's JIT is not kept: {e}"), String::new()),
                    }
                }
                *BUILDING_JIT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            };
            // on the cores its room was granted, when fewer than it measured (`jobs=N`, `deploy-jobs`)
            let jobs = o.get("jobs").and_then(|n| n.parse::<u32>().ok()).filter(|n| *n > 0);
            let b = build::prebuild(&self.conf, jobs, &jit_built);
            *BUILDING_JIT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let n = b.compiled.len();
            built_ok = b.ok;
            if b.ok {
                self.event(Sev::Info, format!("sync: built in {:.0} s: {n} crates compiled", b.secs), format!("{{\"compiled\":[{}],\"log\":{}}}", b.compiled.iter().take(200).map(|c| json_str(c)).collect::<Vec<_>>().join(","), json_str(&b.log.display().to_string())));
            } else {
                self.event(Sev::Err, format!("sync: build failed after {:.0} s: {}", b.secs, b.err), format!("{{\"log\":{}}}", json_str(&b.log.display().to_string())));
            }
            built_line = format!("{{\"ok\":{},\"crates\":{n},\"s\":{:.1},\"err\":{},\"log\":{}}}", b.ok, b.secs, json_str(&b.err), json_str(&b.log.display().to_string()));
        }
        out.push(format!("{{\"built\":{built_line}}}"));

        // restarts
        let restart = o.get("restart").unwrap_or("none");
        let wm_replaced = build::mtime(&wm_bin) != wm_before;
        let stale: Vec<jits::Jit> = jits::list().into_iter().filter(|j| jits::runs_from(j.pid, &self.conf.checkout) && jits::exe_replaced(j.pid)).collect();
        match restart {
            "wm" => self.restart_or_refuse("the order asks for it", out),
            "auto" => {
                if let Some(why) = auto_wm_restart(wm_replaced, &stale) {
                    self.restart_or_refuse(why, out);
                } else if crate::lease::exec::dev_box() && !stale.is_empty() {
                    // (off the apply's answer: each app's ask may take seconds)
                    let (me, stale) = (self.clone(), stale.clone());
                    let _ = std::thread::Builder::new().name("devhub-sync-latest".into()).spawn(move || me.to_the_latest(&stale));
                }
            }
            "apps" => {
                for j in &stale {
                    if super::launch::ledger(j.pid).is_none() {
                        continue;
                    }
                    let mut lines = Vec::new();
                    (super::launch::service(self.conf.clone(), self.ctx.publish.clone(), self.ctx.machine.clone()))(&format!("relaunch {}", j.pid), &mut lines);
                    out.extend(lines);
                }
            }
            _ => {}
        }
        {
            // once per process: every sync after a landing found the same ones again (several
            // lines per box per 5 min, 11-02); one that ended leaves the set
            let mut said = self.stale_said.lock().unwrap();
            said.retain(|(pid, app)| stale.iter().any(|j| j.pid == *pid && j.app == *app));
            for j in stale.iter().filter(|j| restart != "apps" || super::launch::ledger(j.pid).is_none()) {
                if said.insert((j.pid, j.app.clone())) {
                    self.jit_event(Sev::Warn, &j.app, j.pid, format!("sync: {}#{} runs a makepad binary the build replaced: restart it to run the new JIT", j.app, j.pid), String::new());
                }
            }
        }
        // each repository it moved, by name (makepad always: null when it did not move)
        let moved: String = revs.iter().filter(|(r, _)| r != "makepad").map(|(r, sha)| format!(",{}:{}", json_str(r), json_str(sha))).collect();
        let summary = format!(
            "{{\"applied\":{{\"makepad\":{}{moved},\"session\":{},\"wm_replaced\":{wm_replaced},\"stale_jits\":{},\"ms\":{}}}}}",
            revs.iter().find(|(r, _)| r == "makepad").map_or("null".into(), |(_, sha)| json_str(sha)),
            json_str(&session),
            stale.len(),
            t0.elapsed().as_millis()
        );
        *self.last.lock().unwrap() = summary.clone();
        out.push(summary);
        // pre-warm (asked by the master for an idle dev-on node): the JIT built and its cache warm
        // for the smoke apps, in the background, preempted by the next lease
        if o.get("prewarm") == Some("on") {
            use crate::lease::exec;
            let why_not = super::prewarm::refusal(exec::writes_allowed(), exec::bench_box(), !exec::held().is_empty(), built_ok);
            let at = deployed.as_ref().map_or("-".to_string(), crate::lease::Pin::short);
            match why_not {
                None => super::prewarm::start(self.conf.checkout.clone(), at, self.ctx.publish.clone(), self.ctx.machine.clone()),
                Some(w) => (self.ctx.publish)(hub_record(&self.ctx.machine, "", 0, "prewarm", Sev::Info, format!("prewarm: skipped at {at}: {w}"), String::new())),
            }
        }
    }

    /// Follows each watched JIT's live-edit records for the edit this apply caused, up to 60 s
    /// (a coordinated patch also reads the session tick the edit landed at). With `news` (an
    /// ordinary sync), an edit that needs a restart is followed on to its end (the restarted run
    /// on screen, the app postponing it, makepad rebuilding itself first), and each app's outcome
    /// is logged; one the person has to act on shows as a toast in the WM on a box a person uses
    /// ([`toast`]).
    fn follow_patches(&self, watched: Vec<(jits::Jit, PathBuf, u64)>, ticker: Option<(u32, jits::Ticker)>, session: &str, news: Option<&News>) -> Vec<String> {
        struct Follow {
            jit: jits::Jit,
            log: PathBuf,
            at: u64,
            /// an edit needs a restart: what follows it is awaited until then
            restarting: Option<Instant>,
            /// patched: shown as applied once the app still runs a moment later (a patch that
            /// takes the app down must not be reported as applied)
            settling: Option<Instant>,
        }
        let mut out = Vec::new();
        let end = Instant::now() + Duration::from_secs(60);
        let mut pending: Vec<Follow> = watched.into_iter().map(|(jit, log, at)| Follow { jit, log, at, restarting: None, settling: None }).collect();
        let shown = |j: &jits::Jit, outcome: Outcome| {
            let Some(news) = news else { return };
            let person = crate::lease::exec::person_box(toast::PERSON_MS);
            match toast::post(&j.app, j.pid, news, &outcome, person) {
                Ok((said, after)) => {
                    let timing = after.map_or(String::new(), |ms| format!(" {:.1} s after the landing", ms as f64 / 1000.0));
                    let line = match said {
                        Some(text) => format!("sync: shown{timing}: {text}"),
                        None if outcome.asks_a_person() => format!("sync: {outcome:?}{timing}; no toast: nobody uses this box"),
                        None => format!("sync: {outcome:?}{timing}"),
                    };
                    self.jit_event(Sev::Info, &j.app, j.pid, line, String::new());
                }
                Err(e) => self.jit_event(Sev::Warn, &j.app, j.pid, format!("sync: no toast for {}#{}: {e}", j.app, j.pid), String::new()),
            }
        };
        while !pending.is_empty() {
            let mut still = Vec::new();
            for mut f in pending {
                let (edits, at) = jits::edits_since(&f.log, f.at);
                f.at = at;
                // settling: only whether the app lives on decides
                let edits = if f.settling.is_some() { Vec::new() } else { edits };
                let mut done = false;
                for e in edits {
                    let j = &f.jit;
                    let outcome = match (e.rung.as_str(), f.restarting.is_some()) {
                        ("unchanged", _) => continue,
                        ("patch" | "partial", false) => Some(Outcome::Live),
                        ("restart", false) if e.raw.contains("\"restart_required\":true") => Some(Outcome::NeedsRestart(e.reasons.clone())),
                        ("restart", false) => None,
                        ("refuse" | "incomplete", false) => Some(Outcome::NotApplied(e.reasons.clone())),
                        ("rebuild_started", _) => {
                            // makepad rebuilds itself first (minutes): said now; the app restarts into it by itself
                            shown(j, Outcome::Rebuilding);
                            f.restarting = Some(Instant::now() + Duration::from_secs(900));
                            continue;
                        }
                        ("rebuild_failed", _) => Some(Outcome::NotApplied(e.reasons.clone())),
                        ("restart_postponed", _) => Some(Outcome::Postponed(e.reasons.clone())),
                        ("restart_offered", _) => Some(Outcome::Offered(e.reasons.clone())),
                        ("restart_present", _) => Some(Outcome::Restarted),
                        _ => continue,
                    };
                    if f.restarting.is_none() && matches!(e.rung.as_str(), "patch" | "partial" | "restart" | "refuse" | "incomplete") {
                        let tick = ticker.as_ref().filter(|(p, _)| *p == j.pid).and_then(|(_, t)| t.read().ok());
                        let at = tick.map_or(String::new(), |t| format!(" at session {session} tick {t}"));
                        // (never over unsaved work)
                        let own = restarts_with_the_landing(&j.app, &e.rung, crate::lease::exec::user_watching(), crate::lease::exec::dev_box())
                            && !matches!(crate::lease::exec::app_lease(j.pid), crate::lease::exec::AppLease::Live { .. }) && !crate::lease::exec::unsaved_in(j.pid);
                        let (sev, what) = match e.rung.as_str() {
                            "patch" | "partial" => (Sev::Info, format!("patched {} fns in {:.0} ms", e.fns, e.ms)),
                            "restart" if own => match jits::relaunch(j.pid) {
                                Ok(how) => (Sev::Info, format!("restarted in place for the landing ({}): {}; it answered {how}", if crate::lease::exec::dev_box() { "a dev box runs the latest" } else { "the WM's AI pane, a node service" }, e.reasons)),
                                Err(err) => (Sev::Warn, format!("needs a restart: {}; asking it for one failed: {err}", e.reasons)),
                            },
                            "restart" => (Sev::Warn, format!("needs a restart: {}", e.reasons)),
                            _ => (Sev::Warn, format!("refused the edit: {}", e.reasons)),
                        };
                        self.jit_event(sev, &j.app, j.pid, format!("sync: {}#{} {what}{at}", j.app, j.pid), e.raw.clone());
                        out.push(format!("{{\"jit\":{},\"pid\":{},\"rung\":{},\"fns\":{},\"ms\":{},\"tick\":{}}}", json_str(&j.app), j.pid, json_str(&e.rung), e.fns, e.ms, tick.map_or("null".into(), |t| t.to_string())));
                        // no toast: nobody is at the box to see it
                        if own {
                            done = true;
                            break;
                        }
                    }
                    match outcome {
                        Some(Outcome::Live) if news.is_some() => {
                            f.settling = Some(Instant::now() + Duration::from_secs(3));
                            break;
                        }
                        Some(o) => {
                            shown(j, o);
                            done = true;
                            break;
                        }
                        // a restart in place follows (the app's next safe point)
                        None if news.is_some() => f.restarting = Some(Instant::now() + Duration::from_secs(60)),
                        None => {
                            done = true;
                            break;
                        }
                    }
                }
                if done {
                    continue;
                }
                if makepad_remote::relay::proc_start(f.jit.pid).is_none() {
                    if f.settling.is_some() || f.restarting.is_some() {
                        shown(&f.jit, Outcome::NotApplied("the app closed while taking the change (a JIT error: see its log); open it again from the dock".into()));
                    }
                    continue;
                }
                if f.settling.is_some_and(|t| Instant::now() > t) {
                    shown(&f.jit, Outcome::Live);
                    continue;
                }
                if f.settling.is_some() {
                    still.push(f);
                    continue;
                }
                match f.restarting {
                    Some(t) if Instant::now() > t => shown(&f.jit, Outcome::NeedsRestart("it did not restart by itself".into())),
                    None if Instant::now() > end => out.push(format!("{{\"jit\":{},\"pid\":{},\"rung\":\"none\"}}", json_str(&f.jit.app), f.jit.pid)),
                    _ => still.push(f),
                }
            }
            pending = still;
            if !pending.is_empty() {
                // a session's tick is read promptly; a toast within a frame or two is soon enough
                std::thread::sleep(Duration::from_millis(if ticker.is_some() { 5 } else { 20 }));
            }
        }
        out
    }

    fn restart_or_refuse(self: &Arc<Self>, why: &str, out: &mut Vec<String>) {
        match self.wm_restart(why, wm_generation()) {
            Ok(l) => out.push(l),
            Err(e) => self.refuse(out, format!("sync: WM restart refused: {e}")),
        }
    }

    /// A dev box runs the latest (the user, 11-22: "yes all dev linux boxes can always update to
    /// latest"): each app here a build left on an older JIT binary (`stale`: its binary replaced)
    /// restarts onto it with its state (`session relaunch`), a person's open terminal among them,
    /// as [`takes_the_latest`] lets it. Four boxes' WM apps said "runs older code than its
    /// checkout" for days: a keeping WM's restart leaves them open, and nothing restarted them.
    fn to_the_latest(&self, stale: &[jits::Jit]) {
        for j in stale {
            let lease = crate::lease::exec::app_lease(j.pid);
            let headless = crate::lease::exec::in_headless_lease(j.pid);
            match takes_the_latest(&j.app, lease, headless, || jits::own_inputs_changed(j.pid), || crate::lease::exec::unsaved_in(j.pid)) {
                Ok(()) => {
                    let (sev, what) = match jits::relaunch(j.pid) {
                        Ok(how) => (Sev::Info, format!("restarted onto the latest JIT binary with its state (a dev box runs the latest); it answered {how}")),
                        Err(e) => (Sev::Warn, format!("runs an older JIT binary; asking it to restart onto the latest failed: {e}")),
                    };
                    self.jit_event(sev, &j.app, j.pid, format!("sync: {}#{} {what}", j.app, j.pid), String::new());
                }
                Err(Some(why)) => self.jit_event(Sev::Info, &j.app, j.pid, format!("sync: {}#{} runs an older JIT binary, not restarted: {why}", j.app, j.pid), String::new()),
                Err(None) => {}
            }
        }
    }

    fn marker() -> PathBuf {
        proto::home().join("sync").join("wm-restart.pending")
    }

    /// Restarts the WM when nothing is lost by it, else as soon as nothing is: a WM restart ends
    /// every app window on the node, so it waits while an app has unsaved work
    /// (`makepad_app_registry::machine::unsaved_work`, what apps publish) and while a lease shows
    /// on the WM ([`crate::lease::exec::on_the_wm`]; a headless one runs on through it). The wait
    /// is reported once; one waiter per node, which restarts the moment the node is clear.
    /// `asked`: the WM's restart generation when it was asked ([`wm_generation`]).
    fn wm_restart(self: &Arc<Self>, why: &str, asked: u64) -> Result<String, String> {
        if restart_blockers().is_empty() {
            match self.restart_wm_now(&format!("{why}, with nothing to wait for"), asked, true) {
                Ok(line) => return Ok(line),
                Err(NotRestarted::Failed(e)) => return Err(e),
                // a lease that started while the hub built: waited for like any other
                Err(NotRestarted::Blocked(_)) => {}
            }
        }
        self.wait_restart(why, asked, 0)
    }

    /// Waits for the WM restart ([`Self::wm_restart`]) on a thread of its own, one per node
    /// ([`restart_wait`]); a wait already running takes it. `began_ms`: when the wait began, a
    /// hub before this one's (0: now); its restart says how long it waited and on what
    /// ([`waited_said`]).
    fn wait_restart(self: &Arc<Self>, why: &str, asked: u64, began_ms: u64) -> Result<String, String> {
        if !crate::lease::exec::writes_allowed() {
            return Err(format!("{} is not available for dev: WM restarts need its dev switch on", self.ctx.machine));
        }
        let blockers = restart_blockers();
        let list = blockers.join("; ");
        *WM_RESTART_WAITS.lock().unwrap_or_else(|e| e.into_inner()) = list.clone();
        if WM_RESTART_WAITING.swap(true, std::sync::atomic::Ordering::AcqRel) {
            note_waited(&mut WM_RESTART_WAITED.lock().unwrap_or_else(|e| e.into_inner()).1, &blockers);
            return Ok(format!("{{\"wm_restart\":\"waiting\",\"for\":{}}}", json_str(&list)));
        }
        let began_ms = if began_ms == 0 { proto::wall_ms() } else { began_ms };
        *WM_RESTART_WAITED.lock().unwrap_or_else(|e| e.into_inner()) = (began_ms, blockers);
        if list.is_empty() {
            self.event(Sev::Info, format!("sync: the WM restart ({why}) waits for nothing now: it restarts at its first look"), String::new());
        } else {
            self.event(Sev::Warn, format!("sync: the WM restart ({why}) waits for: {list}"), String::new());
        }
        // the hub that takes over from this one goes on waiting ([`Self::resume_wm_restart`])
        let _ = std::fs::create_dir_all(proto::home().join("sync"));
        let _ = std::fs::write(Self::waiting_marker(), format!("{why}\n{began_ms}\n"));
        let me = self.clone();
        let why = why.to_string();
        let spawned = std::thread::Builder::new().name("devhub-wm-restart-wait".into()).spawn(move || {
            // the hub's build at the deployment, while it waits: its last outcome
            let mut hub: Option<(String, Result<(), String>)> = None;
            restart_wait(
                || {
                    std::thread::sleep(Duration::from_secs(1));
                    true
                },
                // built in the background, once per deployment, so the gap it finds waits on no
                // build; one the hub does not build at holds the restart until a later one builds
                || {
                    let Some(now) = crate::lease::exec::hub_built_now(&me.conf.checkout) else { return };
                    if hub.as_ref() == Some(&now) {
                        return;
                    }
                    let unbuilt = now.1.as_ref().err().map(|e| unbuilt_said(&now.0, e)).unwrap_or_default();
                    // (the build's own failure is a warning already: [`crate::lease::exec::hub_built_now`])
                    if !unbuilt.is_empty() {
                        me.event(Sev::Info, format!("sync: the WM restart ({why}) waits: {unbuilt}"), String::new());
                    }
                    *WM_RESTART_UNBUILT.lock().unwrap_or_else(|e| e.into_inner()) = unbuilt;
                    hub = Some(now);
                },
                || {
                    let (waits, drained) = restart_look();
                    *WM_RESTART_WAITS.lock().unwrap_or_else(|e| e.into_inner()) = waits.join("; ");
                    note_waited(&mut WM_RESTART_WAITED.lock().unwrap_or_else(|e| e.into_inner()).1, &waits);
                    (waits, drained)
                },
                leases_show,
                // (not under the sync lock: an apply that builds meanwhile goes on, and restarts
                // the WM again after it if its build replaced the WM's binary)
                || match me.restart_wm_now(&{ let w = WM_RESTART_WAITED.lock().unwrap_or_else(|e| e.into_inner()); waited_said(&why, w.0, &w.1, proto::wall_ms()) }, asked, false) {
                    Ok(_) => true,
                    Err(NotRestarted::Blocked(list)) => {
                        me.event(Sev::Info, format!("sync: the WM restart ({why}) waits on: {list} (it started since the look)"), String::new());
                        false
                    }
                    Err(NotRestarted::Failed(e)) => {
                        me.event(Sev::Err, format!("sync: WM restart refused: {e}"), String::new());
                        true
                    }
                },
            );
            WM_RESTART_UNBUILT.lock().unwrap_or_else(|e| e.into_inner()).clear();
            let _ = std::fs::remove_file(Self::waiting_marker());
            WM_RESTART_WAITING.store(false, std::sync::atomic::Ordering::Release);
        });
        if let Err(e) = spawned {
            let _ = std::fs::remove_file(Self::waiting_marker());
            WM_RESTART_WAITING.store(false, std::sync::atomic::Ordering::Release);
            return Err(format!("cannot wait for the restart: {e}"));
        }
        Ok(format!("{{\"wm_restart\":\"waiting\",\"for\":{}}}", json_str(&list)))
    }

    /// A WM restart this node waits for: why, while the waiting hub lives and across its own
    /// restarts.
    fn waiting_marker() -> PathBuf {
        proto::home().join("sync").join("wm-restart.waiting")
    }

    /// Takes over the WM restart that the hub before this one waited for, when the WM still runs
    /// what it waited to replace. The wait lived only in that hub and ended with its self-update,
    /// so until the next landing's apply the box took screen leases again, undrained. Devhub
    /// landings roll every hub, and two busy boxes' WMs went hours without restarting (11-07).
    #[cfg(target_os = "linux")]
    fn resume_wm_restart(self: &Arc<Self>) {
        // (not before this hub has its leases back, nor while its dev switch is off: a WM restart waits on them)
        if WM_RESTART_WAITING.load(std::sync::atomic::Ordering::Acquire) || !crate::lease::exec::writes_allowed() {
            return;
        }
        let Ok(marker) = std::fs::read_to_string(Self::waiting_marker()) else { return };
        let (why, began_ms) = waiting_read(&marker);
        let _ = std::fs::remove_file(Self::waiting_marker());
        let stale: Vec<jits::Jit> = jits::list().into_iter().filter(|j| jits::runs_from(j.pid, &self.conf.checkout) && jits::exe_replaced(j.pid)).collect();
        if auto_wm_restart(false, &stale).is_none() && crate::lease::caps::other_wire().is_none() {
            return;
        }
        // (not under the sync lock: this hub's first apply holds it through its build, minutes, and
        // a lease's gap meanwhile is the restart's; the wait takes its first look at once)
        if let Err(e) = self.wait_restart(why, wm_generation(), began_ms) {
            self.event(Sev::Warn, format!("sync: WM restart refused: {e}"), String::new());
        }
    }

    /// Restarts the WM (and with it the AI Hub it starts) through its systemd service, with the
    /// sudoers rule's exact command (no password prompt). The services the WM supervises (this
    /// hub, the AI Hub) run in scopes of their own and outlive it; the marker lets a hub that
    /// does restart report it. Its last look at what the restart ends and the restart run while
    /// no lease starts here.
    /// `asked`: the WM's restart generation when it was asked; a restart made since is the one
    /// it asked for, and it skips ([`restart_once`]). `build`: the hub builds first (a waiting
    /// restart built it while it waited).
    fn restart_wm_now(&self, why: &str, asked: u64, build: bool) -> Result<String, NotRestarted> {
        let _one = self.restart.lock().unwrap_or_else(|e| e.into_inner());
        match restart_once(&WM_GENERATION, asked, || self.restart_wm(why, build))? {
            Some(line) => Ok(line),
            None => {
                let skipped = format!("sync: the WM restart ({why}) skipped: the WM restarted since it was asked");
                self.event(Sev::Info, skipped.clone(), String::new());
                Ok(format!("{{\"wm_restart\":\"skipped\",\"why\":{}}}", json_str(&skipped)))
            }
        }
    }

    fn restart_wm(&self, why: &str, build: bool) -> Result<String, NotRestarted> {
        let failed = NotRestarted::Failed;
        if !crate::lease::exec::writes_allowed() {
            return Err(failed(format!("{} is not available for dev: WM restarts need its dev switch on", self.ctx.machine)));
        }
        let svc = self.conf.wm_service.clone().ok_or_else(|| failed("this node has no WM service (wm_service = none)".into()))?;
        // The new WM starts this hub again when the deployment moved its binary: built first, that
        // start is a no-op `cargo run` (a node's compiled 13 s behind a package-cache lock while its
        // hub refused connections, 11-02). A hub that does not build keeps the WM and this hub.
        #[cfg(feature = "checkout")]
        if build {
            crate::lease::exec::build_hub(&self.conf.checkout, None).map_err(|e| failed(format!("the hub does not build at this deployment ({e}): the WM is not restarted, this hub keeps running")))?;
        }
        crate::lease::exec::with_no_lease_starting(|| {
            let blockers = restart_blockers();
            if !blockers.is_empty() {
                return Err(NotRestarted::Blocked(blockers.join("; ")));
            }
            let _ = std::fs::create_dir_all(proto::home().join("sync"));
            let _ = std::fs::write(Self::marker(), format!("{} {why}\n", proto::wall_ms()));
            // the WM session's own apps it ends end by it, no crash (each deploy's WM restart
            // filed the WM's aichat as one, 11-06)
            crate::lease::exec::note_stopped_pids(&format!("by the WM restart ({why})"), super::jits::list().into_iter().filter(|j| wm_own(&j.app)).map(|j| j.pid).collect());
            // what it closes is said where the master sees it: a restart that closed the user's app
            // left no line but the waits before it (11-21)
            let closes: Vec<String> = super::jits::list().into_iter().filter(|j| !wm_own(&j.app) && !crate::lease::exec::in_headless_lease(j.pid)).map(|j| format!("{} (pid {}, {})", j.app, j.pid, opened_said(launched_by(j.pid).as_ref(), wm_child(j.pid)))).collect();
            // (a keeping WM leaves them open for the next one: they come back to it)
            let keeps = wm_notice().is_some_and(|n| notice_keeps(&n));
            match (closes.is_empty(), keeps) {
                (true, _) => self.event(Sev::Info, format!("sync: restarting the WM ({svc}): {why}"), String::new()),
                (false, true) => self.event(Sev::Info, format!("sync: restarting the WM ({svc}): {why}; its apps stay open for the next WM: {}", closes.join(", ")), String::new()),
                (false, false) => self.event(Sev::Warn, format!("sync: restarting the WM ({svc}): {why}; it closes {}", closes.join(", ")), String::new()),
            }
            let st = makepad_remote::group::command(crate::os::tool("sudo")).args(["-n", "/usr/bin/systemctl", "restart", &svc]).stdin(std::process::Stdio::null()).output().map_err(|e| failed(format!("sudo: {e}")))?;
            if !st.status.success() {
                let _ = std::fs::remove_file(Self::marker());
                return Err(failed(format!("sudo -n systemctl restart {svc}: {}", proto::clean(String::from_utf8_lossy(&st.stderr).trim(), 300))));
            }
            let active = makepad_remote::group::command(crate::os::tool("systemctl")).args(["is-active", &svc]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
            let _ = std::fs::remove_file(Self::marker());
            // with why and its wait ([`waited_said`]): the master keeps it in `devhub rolls`
            let line = format!("sync: WM restarted ({svc} {active}): {why}");
            self.event(if active == "active" { Sev::Info } else { Sev::Err }, line, String::new());
            Ok(format!("{{\"wm_restarted\":{},\"active\":{}}}", json_str(&svc), json_str(&active)))
        })
    }

    /// A hub that started after a WM restart it asked for reports it.
    fn after_wm_restart(&self) {
        let Ok(text) = std::fs::read_to_string(Self::marker()) else { return };
        let _ = std::fs::remove_file(Self::marker());
        let at: u64 = text.split_whitespace().next().and_then(|t| t.parse().ok()).unwrap_or(0);
        self.event(Sev::Info, format!("sync: WM restarted; this hub is back {} s after the restart began", proto::wall_ms().saturating_sub(at) / 1000), String::new());
    }

    /// Receives one bundle (the `sync-push` channel).
    fn receive(&self, ch: &makepad_remote::Channel) -> Result<String, String> {
        let first = match ch.recv_timeout(CHUNK_WAIT) {
            Ok(Event::Data(d)) => d,
            _ => return Err("no push order".into()),
        };
        let o = self.seen.check(std::str::from_utf8(&first).map_err(|_| "the push order is not text")?.trim())?;
        let (repo, branch, sha, hash, size) = match o.words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["push", repo, branch, sha, hash, size] => (repo.to_string(), branch.to_string(), sha.to_string(), hash.to_string(), size.parse::<u64>().map_err(|_| "bad size")?),
            _ => return Err("not a push order".into()),
        };
        if !git::valid_repo(&repo) || !git::valid_branch(&branch) || !git::valid_sha(&sha) || hash.len() != 64 || size == 0 || size > MAX_BUNDLE {
            return Err("malformed push order".into());
        }
        let dir = proto::home().join("sync").join("incoming");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // one file per push: two leases shipping the same commit at once send the same bundle (same
        // hash), and must neither truncate nor delete each other's copy
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let file = dir.join(format!("{hash}-{}-{}.bundle", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let r = (|| {
            let mut f = std::fs::File::create(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let mut h = Sha256::new();
            let mut got = 0u64;
            while got < size {
                match ch.recv_timeout(CHUNK_WAIT) {
                    Ok(Event::Data(d)) => {
                        got += d.len() as u64;
                        if got > size {
                            return Err("the bundle is longer than its order says".to_string());
                        }
                        h.update(&d);
                        f.write_all(&d).map_err(|e| e.to_string())?;
                    }
                    Ok(Event::Opened(_)) => {}
                    Ok(Event::Closed(_, why)) => return Err(format!("the push ended after {got} of {size} bytes: {why}")),
                    Err(_) => return Err(format!("the push stalled after {got} of {size} bytes")),
                }
            }
            f.flush().map_err(|e| e.to_string())?;
            if sign::hex(&h.finalise()) != hash {
                return Err("the bundle's hash is not the one the master signed".into());
            }
            let store = Store::open(&repo)?;
            let fetched = store.fetch_bundle(&file, &branch)?;
            if fetched != sha {
                return Err(format!("the bundle's {branch} is {}, the order names {}", short(&fetched), short(&sha)));
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&file);
        r?;
        self.event(Sev::Info, format!("sync: pushed {repo} {branch} {} ({:.1} MB) by {}", short(&sha), size as f64 / 1048576.0, o.signer), String::new());
        Ok(format!("{{\"fetched\":{},\"repo\":{},\"branch\":{},\"bytes\":{size}}}", json_str(&sha), json_str(&repo), json_str(&branch)))
    }
}

/// An apply (its tree's move, its build) runs on this node.
static APPLYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The hub exits for its self-update ([`restart_begin`]); it stays marked until the exit.
static RESTARTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Marks the hub's exit for its self-update unless an apply runs ([`begin_beside`]): no apply
/// starts after it. A hub that answered its restart exits half a second later, and an apply the
/// master sent in that half second was cut in its index write: its `index.lock` stayed, and the
/// box's applies and its self-update waited on it for 9 h (11-19).
pub fn restart_begin() -> bool {
    begin_beside(&RESTARTING, &APPLYING)
}

/// The hub builds its next self from the checkout ([`hub_build_begin`]): an apply waits for it.
static HUB_BUILDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The hub's own build of the checkout, marked while it runs: unless an apply runs, it starts
/// and no apply moves the checkout under it until the mark ends with the guard. An apply moved a
/// box's checkout under its hub's build, which compiled the newer main.rs against the library
/// it had compiled before and said the commit it named does not build (11-17). None: an apply
/// runs, and the build waits for it.
pub fn hub_build_begin() -> Option<HubBuilding> {
    begin_beside(&HUB_BUILDING, &APPLYING).then_some(HubBuilding)
}

/// Marks `mine` unless `other` is marked: each side marks first and looks after, so of two that
/// start together one always sees the other ([`hub_build_begin`], the apply's check).
fn begin_beside(mine: &std::sync::atomic::AtomicBool, other: &std::sync::atomic::AtomicBool) -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    mine.store(true, SeqCst);
    if other.load(SeqCst) {
        mine.store(false, SeqCst);
        return false;
    }
    true
}

/// [`hub_build_begin`]'s mark; it ends when this drops.
pub struct HubBuilding;

impl Drop for HubBuilding {
    fn drop(&mut self) {
        HUB_BUILDING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The running apply moved files of the checkout the box's apps run from.
static MOVED_IN_APPLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// When the last apply that moved the checkout ended (wall ms, 0: none since this hub started).
static MOVED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How long ago the last apply that moved the checkout under the box's apps ended: they take
/// the move, and one that runs makepad from it rebuilds what it runs (`Exec`'s timed start
/// waits until the box is quiet again).
pub fn moved_ago() -> Option<std::time::Duration> {
    let at = MOVED_MS.load(std::sync::atomic::Ordering::SeqCst);
    (at > 0).then(|| std::time::Duration::from_millis(proto::wall_ms().saturating_sub(at)))
}

/// The timed lease that runs on this node, if one does: the quiet mark its hub keeps while it
/// lives (`<home>/quiet/<id>.cmd`, lease/exec.rs).
fn timed_here() -> Option<String> {
    std::fs::read_dir(proto::home().join("quiet")).into_iter().flatten().flatten()
        .find(|e| e.path().extension().is_some_and(|x| x == "cmd"))
        .map(|e| e.path().file_stem().map_or_else(|| "a timed lease".to_string(), |id| format!("timed lease {}", id.to_string_lossy())))
}

pub fn applying() -> bool {
    APPLYING.load(std::sync::atomic::Ordering::SeqCst)
}

/// The commits (makepad, and the paired nested repository's) whose JIT this node's apply is
/// building now.
static BUILDING_JIT: std::sync::Mutex<Option<crate::lease::Pin>> = std::sync::Mutex::new(None);

/// This node's apply is building the JIT of exactly these commits (kept for its tree leases
/// once built: a lease of them waits for it rather than build it again).
pub fn building_jit(pin: &crate::lease::Pin) -> bool {
    BUILDING_JIT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() == Some(pin)
}

/// A WM restart is waiting on this node (`Sync::wm_restart`).
static WM_RESTART_WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// What the waiting WM restart waited on at its last look ([`stale_runs`] says it).
static WM_RESTART_WAITS: Mutex<String> = Mutex::new(String::new());
/// When the waiting WM restart began to wait (wall ms) and what it has waited on since, each once,
/// in the order it came: its restart says them ([`waited_said`]). Only its start was said, an
/// inbox line, and nothing kept how long a box's WM waited (11-24).
static WM_RESTART_WAITED: Mutex<(u64, Vec<String>)> = Mutex::new((0, Vec::new()));

/// Adds to `on` what a look found the restart waits on (`waits`) that it had not yet.
fn note_waited(on: &mut Vec<String>, waits: &[String]) {
    for w in waits {
        if !on.contains(w) {
            on.push(w.clone());
        }
    }
}

/// Why a WM restart that waited ran, with its wait: from `began_ms` to `now_ms`, on what (`on`).
fn waited_said(why: &str, began_ms: u64, on: &[String], now_ms: u64) -> String {
    let long = crate::lease::sched::dur(now_ms.saturating_sub(began_ms));
    match on.is_empty() {
        true => format!("{why}, after {long} waiting for its first look"),
        false => format!("{why}, after {long} waiting on: {}", on.join("; ")),
    }
}

/// The waiting marker's why and when its wait began (wall ms; 0: a marker from before it said).
#[cfg(any(target_os = "linux", test))]
fn waiting_read(marker: &str) -> (&str, u64) {
    match marker.split_once('\n') {
        Some((why, began)) => (why.trim(), began.trim().parse().unwrap_or(0)),
        None => (marker.trim(), 0),
    }
}

/// Why the waiting WM restart waits for a deployment the hub builds at ([`unbuilt_said`]; its
/// build while it waits failed at this one): empty while it builds.
static WM_RESTART_UNBUILT: Mutex<String> = Mutex::new(String::new());

/// A waiting WM restart's hold while the hub does not build at the deployment `at` (`e`, why):
/// the WM and this hub go on as they run, and the box does not drain for a restart that cannot
/// run ([`holding`]).
fn unbuilt_said(at: &str, e: &str) -> String {
    format!("the hub does not build at {} ({}): it restarts the WM once a deployment builds it, and the WM and this hub go on meanwhile", short(at), proto::clean(e, 300))
}

/// The WM's restart generation: each restart this hub makes moves it on ([`restart_once`]).
static WM_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn wm_generation() -> u64 {
    WM_GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// Runs `restart` for an ask made at generation `asked` (a waiter's, an order's) unless the WM
/// restarted since: then the restart it asked for was made, and it skips (None). A restart that
/// went through moves the generation on; a refused or failed one does not. Called under the WM
/// restart lock (`Sync::restart`), so asks queued behind one another see each other's restart: a deferred waiter and an
/// order that queued behind an apply restarted the WM 2 s apart, every session on the box ended
/// twice (11-14).
fn restart_once(generation: &std::sync::atomic::AtomicU64, asked: u64, restart: impl FnOnce() -> Result<String, NotRestarted>) -> Result<Option<String>, NotRestarted> {
    use std::sync::atomic::Ordering::SeqCst;
    if generation.load(SeqCst) != asked {
        return Ok(None);
    }
    let line = restart()?;
    generation.fetch_add(1, SeqCst);
    Ok(Some(line))
}

/// Why a WM restart did not happen.
enum NotRestarted {
    /// what it would end, seen in its last look (a lease that started after the wait's look)
    Blocked(String),
    Failed(String),
}

/// What a WM restart would cost now: the leases using the WM ([`crate::lease::exec::on_the_wm`]),
/// and what holds it beyond them ([`held_now`]). Empty: it may restart.
fn restart_blockers() -> Vec<String> {
    restart_look().0
}

/// [`restart_blockers`], and whether only leases hold it ([`drain_said`]).
fn restart_look() -> (Vec<String>, bool) {
    let (people, apps) = held_now();
    let leases = leases_on_the_wm();
    let drained = !drain_said(&people, &leases, &apps).is_empty();
    let mut out: Vec<String> = leases.into_iter().chain(people).collect();
    // a WM restart ends every app in its session where its WM keeps none: there it never takes a
    // user's app with it (it killed Fab on a pool box, 3 min after the last input, 2026-10-30)
    // (a headless lease's JIT runs on through it: its lease is not on the WM)
    out.extend(apps.into_iter().map(|(app, pid, leased)| if leased { format!("{app} (pid {pid}) of a lease") } else { format!("{app} runs (pid {pid}, {}): close it to let the WM update", opened_said(launched_by(pid).as_ref(), wm_child(pid))) }));
    (out, drained)
}

/// Whether a lease shows on the WM now: one on the WM ([`leases_on_the_wm`]), or an app a lease
/// runs there ([`apps_on_the_wm`]'s leased ones), without its whole look.
fn leases_show() -> bool {
    use crate::lease::exec::{app_lease, in_headless_lease, AppLease};
    let jits = super::jits::list();
    !crate::lease::exec::on_the_wm(&jits.iter().map(|j| j.pid).collect::<Vec<_>>()).is_empty()
        || jits.iter().any(|j| !wm_own(&j.app) && matches!(app_lease(j.pid), AppLease::Live { .. }) && !in_headless_lease(j.pid))
}

/// The leases that hold a WM restart now ([`crate::lease::exec::on_the_wm`]), by the JIT apps here.
fn leases_on_the_wm() -> Vec<String> {
    crate::lease::exec::on_the_wm(&super::jits::list().into_iter().map(|j| j.pid).collect::<Vec<_>>())
}

/// Whether a waiting WM restart looks again now, `since` its last whole look, which found only
/// leases holding it (`drained`, [`drain_said`]): every [`RESTART_LOOK`], and in between when
/// none of those leases shows on the WM any more (`showing`, a cheap look: the JIT apps and the
/// lease table). A lease between two app launches holds it for seconds, and a look every 10 s
/// missed those gaps (11-24). A wait that has not looked yet (`since` None) looks now.
fn restart_looks(since: Option<Duration>, drained: bool, showing: impl FnOnce() -> bool) -> bool {
    since.map_or(true, |s| s >= RESTART_LOOK) || (drained && !showing())
}

/// A waiting WM restart ([`Sync::wait_restart`]'s thread): each `tick` (a second; false: its hub
/// exits, and the next hub resumes the wait, [`Sync::resume_wm_restart`]), the hub's build
/// (`hub`, in the background), a whole `look` when due ([`restart_looks`], `showing` the cheap
/// one; its first at once) and, when nothing holds it, the restart (`claim`: true when the wait
/// ends, restarted or refused). Neither it nor its claim takes the sync lock an apply holds
/// through its build: a waiter queued on it through a lease's gap, and so did the next hub's
/// resume after a self-update (11-24).
fn restart_wait(mut tick: impl FnMut() -> bool, mut hub: impl FnMut(), mut look: impl FnMut() -> (Vec<String>, bool), mut showing: impl FnMut() -> bool, mut claim: impl FnMut() -> bool) {
    let (mut looked, mut drained) = (None::<std::time::Instant>, false);
    while tick() {
        hub();
        if !restart_looks(looked.map(|t: std::time::Instant| t.elapsed()), drained, &mut showing) {
            continue;
        }
        looked = Some(std::time::Instant::now());
        let waits;
        (waits, drained) = look();
        if waits.is_empty() && claim() {
            return;
        }
    }
}

/// How often a waiting WM restart takes its whole look (what it waits on, people's reasons among
/// them: some read files of every app).
const RESTART_LOOK: Duration = Duration::from_secs(10);

/// [`holding`] on this box now.
fn held_now() -> (Vec<String>, Vec<(String, u32, bool)>) {
    use crate::lease::exec::{dev_box, user_blockers, watched_said};
    let keeps = wm_notice().is_some_and(|n| notice_keeps(&n));
    let unbuilt = Some(WM_RESTART_UNBUILT.lock().unwrap_or_else(|e| e.into_inner()).clone()).filter(|u| !u.is_empty());
    holding(dev_box(), keeps, user_blockers(), watched_said(), shown_apps(), apps_on_the_wm(), unbuilt)
}

/// What holds a WM restart beyond its leases: `people`, a person's reasons (`unsaved` work in its
/// apps, someone who `watched` its screen, a viewer `shown` its apps), and the `apps` it would end
/// (app, pid, whether a lease runs it). A dev box (`always`, [`crate::lease::exec::dev_box`]) runs
/// the latest (the user, 11-22: "yes all dev linux boxes can always update to latest"): only a
/// lease's apps hold it there, and unsaved work only where the running WM cannot keep its apps
/// (`keeps`): a keeping WM leaves them open over its restart and loses nothing, and no restart
/// goes over unsaved work. Elsewhere all of it holds. A deployment the hub does not build at
/// (`unbuilt`, [`unbuilt_said`]) holds it everywhere, like a person's reason: the box stays at
/// work, undrained, until one builds.
fn holding(always: bool, keeps: bool, unsaved: Vec<String>, watched: Option<String>, shown: Option<String>, apps: Vec<(String, u32, bool)>, unbuilt: Option<String>) -> (Vec<String>, Vec<(String, u32, bool)>) {
    if !always {
        return (unsaved.into_iter().chain(watched).chain(shown).chain(unbuilt).collect(), apps);
    }
    ((if keeps { Vec::new() } else { unsaved }).into_iter().chain(unbuilt).collect(), apps.into_iter().filter(|a| a.2).collect())
}

/// The node's WM's notice beside its socket (`makepad-wm-0.state`: its `epoch=`, `keep=1` when it
/// keeps its apps over its restart, `left_ms=` as it went), if it wrote one.
fn wm_notice() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let rt = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)
            .unwrap_or_else(|| format!("/run/user/{}", std::fs::metadata("/proc/self").map_or(0, |m| m.uid())).into());
        return std::fs::read_to_string(rt.join("makepad-wm-0.state")).ok();
    }
    #[allow(unreachable_code)]
    None
}

/// Whether a WM notice ([`wm_notice`]) says its WM keeps its apps: an exact `keep=1` line.
fn notice_keeps(notice: &str) -> bool {
    notice.lines().any(|l| l == "keep=1")
}

/// A Desk Link viewer that shows this node while apps no lease runs are on its WM
/// ([`viewer_holds`]; a live lease's app holds the restart as its lease, and its end lets the
/// restart's drain run it).
fn shown_apps() -> Option<String> {
    use crate::lease::exec::{app_lease, in_headless_lease, viewer_shows, AppLease};
    if !viewer_shows() {
        return None;
    }
    let apps: Vec<(String, u32)> = super::jits::list().into_iter().filter(|j| !wm_own(&j.app) && !in_headless_lease(j.pid) && !matches!(app_lease(j.pid), AppLease::Live { .. })).map(|j| (j.app, j.pid)).collect();
    viewer_holds(true, &apps)
}

/// What a WM restart waits on while a Desk Link viewer shows the node (`shown`) and `apps` no
/// lease runs are on its WM (app, pid), whoever opened them: a person sees them, input or not,
/// and the restart would close them. The user watched a picture app a show had opened through Desk Link, with no
/// input for 10 min, and a WM restart closed it mid-test, twice in one evening (11-21). A viewer
/// on a desk with no app holds nothing (a restart closes nothing there; viewers kept on tabs held
/// WM restarts for good, 11-02).
///
/// A bridge until apps survive a WM restart (they reattach to the new WM, the design in hand):
/// it keeps a WM stale while a viewer is left on a desk with an app open, the lesser evil next to
/// closing an app a person looks at, and [`stale_runs`] says what the restart waits on. Once apps
/// reattach, a restart closes nothing, and this hold goes.
fn viewer_holds(shown: bool, apps: &[(String, u32)]) -> Option<String> {
    if !shown || apps.is_empty() {
        return None;
    }
    let names: Vec<String> = apps.iter().map(|(app, pid)| format!("{app} (pid {pid})")).collect();
    Some(format!("a Desk Link viewer shows this node while {} runs on its WM: a person sees it, and a restart would close it", names.join(", ")))
}

/// Who opened app process `pid` on the WM: an agent's launch carries the mark in its environment
/// (and its children with it), and the WM's client log of it names any launcher, the latest last
/// (a warm instance opened later). None: the WM's launches do not name it (a shell started it, or
/// a WM from before the mark).
fn launched_by(pid: u32) -> Option<Launcher> {
    #[cfg(target_os = "linux")]
    if let Some(mark) = makepad_app_registry::machine::launched_by_env(pid) {
        return Some(mark);
    }
    let log = crate::node::wm_client_log(&std::env::temp_dir(), pid)?;
    Launcher::last_in(&std::fs::read_to_string(log).ok()?)
}

/// For a WM restart's "waits for" line: who opened the app; one no launch mark names, through the
/// WM (`wm_child`: the WM started it) or not. A pool box's Fab, opened from the WM's dock over a
/// Desk Link by a WM from before the marks, read "not opened through the WM", and was taken for a
/// lease's left behind (11-13).
fn opened_said(by: Option<&Launcher>, wm_child: bool) -> String {
    match by {
        Some(by) => format!("opened by {}", by.said()),
        None if wm_child => "opened through the WM before it named who opens apps: its opener is unknown, a person's as far as the WM tells".to_string(),
        None => "not opened through the WM: a shell started it".to_string(),
    }
}

/// Whether the WM's own process started process `pid` (its parent is the WM's).
fn wm_child(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let parent = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|stat| stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse::<u32>().ok());
        parent.is_some_and(|ppid| super::jits::list().into_iter().any(|j| j.pid == ppid && (j.app == "wm" || j.app == "makepad-app-wm")))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Whether a WM restart waits for an app no lease here runs (`lease`: [`crate::lease::exec::app_lease`])
/// that `by` opened: a person's, or one whose opener nothing names; not one an ended lease left
/// behind, nor an agent's (a terminal an agent's request had opened held a node's WM restart for
/// days, 11-13).
fn waits_for(lease: crate::lease::exec::AppLease, by: Option<&Launcher>) -> bool {
    use crate::lease::exec::AppLease;
    match lease {
        AppLease::Live { .. } => true,
        AppLease::Left => false,
        AppLease::Nobody => !matches!(by, Some(Launcher::Agent(_))),
    }
}

/// The apps a WM restart waits for, with whether a lease runs each: (app, pid, leased). Not the
/// WM's own, not a headless lease's (it runs on through the restart), not one an ended lease left
/// behind (nothing waits for it, and the restart ends it), nor one an agent opened that no lease
/// runs ([`waits_for`]).
fn apps_on_the_wm() -> Vec<(String, u32, bool)> {
    use crate::lease::exec::{app_lease, in_headless_lease, AppLease};
    super::jits::list().into_iter().filter(|j| !wm_own(&j.app) && !in_headless_lease(j.pid)).filter_map(|j| {
        let lease = app_lease(j.pid);
        let by = (lease == AppLease::Nobody).then(|| launched_by(j.pid)).flatten();
        waits_for(lease, by.as_ref()).then(|| (j.app, j.pid, lease != AppLease::Nobody))
    }).collect()
}

/// What on this box runs older code than its checkout `checkout`, for its caps
/// ([`crate::lease::Caps::stale`]), and since when the oldest of it has (wall ms, when a build
/// replaced what it runs: [`jits::replaced_ms`]; 0: none): each JIT
/// app of the checkout whose binary a build replaced (the WM's own among them), and a waiting WM
/// restart with what it waits on. A node's WM ran the build from 7 days before, from before an
/// app was added, its restart waiting on a Sandbox there all that time, and nothing said so (11-21).
///
/// Cheap per caps look: the registry's few files and a link per JIT; what the restart waits on is
/// the waiter's last look ([`WM_RESTART_WAITS`]), not a new one.
///
/// A JIT whose loader a build replaced counts only when what it is built from changed
/// ([`own_stale`]): asked once a process says so (it stays so until it ends), else at each look,
/// one local `session pending` each (a box's 2-4 such JITs after a devhub landing). A program that
/// does not say (the AI hub, no JIT) counts only when the file a build put at its path holds other
/// bytes than the one it runs ([`rebuilt_same`]): a landing that changed nothing it is built of
/// relinks it byte for byte, and the AI hub's restart rightly said it ran the deployment's build,
/// while every Linux box was said to run older code for 4 h (11-23).
/// With the pids of what runs older code: the update roll restarts a WM-supervised service among
/// them ([`crate::lease::Caps::services_stale`]), and what its restart waits on is said beside it
/// ([`crate::lease::exec::service_waits`]).
pub fn stale_runs(checkout: &std::path::Path, now_ms: u64) -> (String, u64, Vec<u32>) {
    let replaced: Vec<(String, u32, u64)> = jits::list().into_iter().filter(|j| jits::runs_from(j.pid, checkout) && jits::exe_replaced(j.pid)).map(|j| (j.app, j.pid, jits::started_ms(j.pid))).collect();
    let known: std::collections::BTreeSet<(u32, u64)> = OWN_CHANGED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let asked: Vec<(String, u32, u64, Option<bool>)> = replaced.into_iter().map(|(app, pid, at)| {
        let own = if known.contains(&(pid, at)) { Some(true) } else { jits::own_inputs_changed(pid) };
        (app, pid, at, own)
    }).collect();
    REBUILT_SAME.lock().unwrap_or_else(|e| e.into_inner()).retain(|pid, _| asked.iter().any(|r| r.1 == *pid));
    *OWN_CHANGED.lock().unwrap_or_else(|e| e.into_inner()) = asked.iter().filter(|r| r.3 == Some(true)).map(|r| (r.1, r.2)).collect();
    let runs = own_stale(asked, rebuilt_same);
    let wait = WM_RESTART_WAITING.load(std::sync::atomic::Ordering::Acquire).then(|| WM_RESTART_WAITS.lock().unwrap_or_else(|e| e.into_inner()).clone());
    let (said, since) = stale_said(&runs, wait.as_deref(), &crate::lease::exec::service_waits, &jits::replaced_ms, now_ms);
    (said, since, runs.iter().map(|r| r.1).collect())
}

/// The apps the node's keeping WM left waiting for its next WM, while no WM that hosts them
/// answers on the session (`caps::display`), for `sync status`: each kept app's scope
/// (`makepad-wm-app-<epoch>-<client>.scope`) by its first process, and since when its WM went: the
/// `left_ms=` that WM wrote into the notice beside its socket (`makepad-wm-0.state`) as it went; a
/// WM that crashed wrote none. Empty while a WM answers: its apps came back to it (or it passed
/// them over, which they log).
pub fn waiting_apps(now_ms: u64) -> String {
    #[cfg(target_os = "linux")]
    {
        if crate::lease::caps::display().is_some() {
            return String::new();
        }
        let mut first: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            if let Some(scope) = kept_scope(&std::fs::read_to_string(e.path().join("cgroup")).unwrap_or_default()) {
                let at = first.entry(scope.to_string()).or_insert(pid);
                *at = (*at).min(pid);
            }
        }
        let apps: Vec<(String, u32)> = first.into_values().map(|pid| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let argv0 = String::from_utf8_lossy(cmd.split(|b| *b == 0).next().unwrap_or_default()).into_owned();
            (argv0.rsplit('/').next().unwrap_or_default().to_string(), pid)
        }).collect();
        let left = wm_notice().and_then(|t| left_ms(&t));
        return waiting_said(&apps, left, now_ms);
    }
    #[allow(unreachable_code)]
    { let _ = now_ms; String::new() }
}

/// The kept-app scope a process's cgroup (`/proc/<pid>/cgroup`) names, if any.
#[cfg(any(target_os = "linux", test))]
fn kept_scope(cgroup: &str) -> Option<&str> {
    cgroup.lines().filter_map(|l| l.rsplit('/').next()).find(|n| n.starts_with("makepad-wm-app-") && n.ends_with(".scope"))
}

/// When a WM's notice says it left its apps waiting (`left_ms=`, wall ms).
#[cfg(any(target_os = "linux", test))]
fn left_ms(notice: &str) -> Option<u64> {
    notice.lines().find_map(|l| l.strip_prefix("left_ms=")?.parse().ok())
}

/// [`waiting_apps`]' words: each waiting app and its pid, and since when its WM left it (`left`).
#[cfg(any(target_os = "linux", test))]
fn waiting_said(apps: &[(String, u32)], left: Option<u64>, now_ms: u64) -> String {
    if apps.is_empty() {
        return String::new();
    }
    let list = apps.iter().map(|(app, pid)| format!("{app}#{pid}")).collect::<Vec<_>>().join(", ");
    match left {
        Some(at) => format!("{list} wait for a WM, {} now: theirs went away for a next one", crate::lease::sched::dur(now_ms.saturating_sub(at))),
        None => format!("{list} wait for a WM: theirs ended without saying when (a crash)"),
    }
}

/// Whether the file a build put at the path of `pid`'s replaced executable holds the same bytes as
/// the one it runs (Linux): the code it runs is its checkout's. Compared once per process and
/// version of that file ([`REBUILT_SAME`]): a whole read of both, once per rebuild that replaced it.
fn rebuilt_same(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let exe = std::path::PathBuf::from(format!("/proc/{pid}/exe"));
        let Some(path) = std::fs::read_link(&exe).ok().and_then(|l| l.to_str().and_then(|p| p.strip_suffix(" (deleted)")).map(std::path::PathBuf::from)) else { return false };
        let Ok(m) = std::fs::metadata(&path) else { return false };
        let version = (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec());
        if let Some((v, same)) = REBUILT_SAME.lock().unwrap_or_else(|e| e.into_inner()).get(&pid) {
            if *v == version { return *same; }
        }
        let same = makepad_app_registry::same_program(&exe, &path);
        REBUILT_SAME.lock().unwrap_or_else(|e| e.into_inner()).insert(pid, (version, same));
        same
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// [`rebuilt_same`]'s answers, by pid: the version of the file it compared (device, inode, length,
/// write time) and whether its bytes were the running ones.
static REBUILT_SAME: Mutex<std::collections::BTreeMap<u32, ((u64, u64, u64, i64, i64), bool)>> = Mutex::new(std::collections::BTreeMap::new());

/// The JITs that said what they are built from changed, by pid and start ([`stale_runs`]).
static OWN_CHANGED: Mutex<std::collections::BTreeSet<(u32, u64)>> = Mutex::new(std::collections::BTreeSet::new());

/// Of the JITs whose loader a build replaced (app, pid, start, and whether what each is built from
/// changed: [`jits::own_inputs_changed`]), those that run older code than their tree: all but one
/// that says nothing of its own changed. A devhub landing rebuilds the makepad binary every app
/// runs from, and every WM node then read 2-4 apps as stale whose code was the tree's (11-21); one
/// that does not say counts unless `same` says its rebuilt file holds the bytes it runs
/// ([`rebuilt_same`]).
fn own_stale(runs: Vec<(String, u32, u64, Option<bool>)>, same: impl Fn(u32) -> bool) -> Vec<(String, u32, u64)> {
    runs.into_iter().filter(|r| r.3.unwrap_or_else(|| !same(r.1))).map(|(app, pid, at, _)| (app, pid, at)).collect()
}

/// [`stale_runs`]' words: `runs` each stale app, its pid and when it started (wall ms; 0 unknown);
/// `wait` what a waiting WM restart waits on; `replaced_at` when a build replaced what a pid runs
/// ([`jits::replaced_ms`]; 0 unknown). Each says when its checkout replaced its build, its start
/// beside it: "started 25m ago, and its checkout has since rebuilt it" was read as 25 min behind
/// its checkout, and the check-in's "for" (its start) as 37 min, of a WM whose restart was 34 s
/// into its wait for a lease and ran 11 s later (11-24). Since when: the oldest replacement (its
/// start where that is unknown), a key that holds while that process runs (a later build
/// replaces a file it no longer runs).
fn stale_said(runs: &[(String, u32, u64)], wait: Option<&str>, restart_waits: &dyn Fn(u32) -> Option<String>, replaced_at: &dyn Fn(u32) -> u64, now_ms: u64) -> (String, u64) {
    let ago = |at: u64| crate::lease::sched::dur(now_ms.saturating_sub(at));
    let mut since: Option<u64> = None;
    let mut parts: Vec<String> = runs.iter().map(|(app, pid, at)| {
        let replaced = replaced_at(*pid);
        if let Some(t) = [replaced, *at].into_iter().find(|t| *t > 0) {
            since = Some(since.map_or(t, |s| s.min(t)));
        }
        let line = match (replaced, at) {
            (0, 0) => format!("{app}#{pid} runs a build its checkout has since rebuilt"),
            (0, at) => format!("{app}#{pid} started {} ago, and its checkout has since rebuilt it", ago(*at)),
            (replaced, 0) => format!("{app}#{pid} runs the build its checkout replaced {} ago", ago(replaced)),
            (replaced, at) => format!("{app}#{pid} runs the build its checkout replaced {} ago (it started {} ago)", ago(replaced), ago(*at)),
        };
        // a service's restart (the update roll's) and what it waits on
        match restart_waits(*pid) {
            Some(why) => format!("{line}; its restart waits: {why}"),
            None => line,
        }
    }).collect();
    if let Some(what) = wait {
        parts.push(format!("its WM restart waits for: {}", if what.is_empty() { "nothing now (it runs at its next look)" } else { what }));
    }
    (parts.join("; "), since.unwrap_or(0))
}

/// What the box's waiting WM restart waits on when only leases hold it ([`drain_said`]), for its
/// caps: the master starts no new lease there the restart would wait for, so it restarts in the
/// gap. Back-to-back landings kept four boxes' WMs waiting for hours: each landing's sync apply
/// held the waiter while the next tests run took the box (11-07).
pub fn wm_drain() -> String {
    if !WM_RESTART_WAITING.load(std::sync::atomic::Ordering::Acquire) {
        return String::new();
    }
    let (people, apps) = held_now();
    drain_said(&people, &leases_on_the_wm(), &apps)
}

/// A waiting WM restart's drain: what it waits on (`leases`, and the apps it would end: app, pid,
/// whether a lease runs it) when only leases hold it, and while nothing does until its next look
/// restarts it (the gap, which a new lease would close again); empty when a person's reason holds
/// it (`people`: at the box, unsaved work, a viewer) or an app no lease runs (a person's): those it
/// waits for with the box at work, never drained for.
///
/// Every waiting restart drains, whatever it restarts for. A restart waited for a gap that never
/// came on a busy box: a node's WM restart, which brings up the AI hub a party waited on, waited
/// from 17:46Z on one lease after another while the master granted the box five new window leases
/// (live tests, a hot bucket run, a landing's gate), and a second box's did the same (11-17). The
/// leases there run to their end; new screen and gui work goes to another box or waits, and an
/// open-ended session there (a visible live session) holds the box's screen work until it ends.
fn drain_said(people: &[String], leases: &[String], apps: &[(String, u32, bool)]) -> String {
    if !people.is_empty() || apps.iter().any(|(_, _, leased)| !leased) {
        return String::new();
    }
    let mut on: Vec<String> = leases.to_vec();
    on.extend(apps.iter().map(|(app, pid, _)| format!("{app} (pid {pid}) of a lease")));
    if on.is_empty() { return DRAIN_GAP.to_string(); }
    on.join("; ")
}

/// A drain's line in the gap: nothing holds the restart, and its next look runs it.
const DRAIN_GAP: &str = "nothing now: it restarts at its next look, within 10 s";

/// Why an `auto` sync restarts the WM: this build replaced its binary, or an earlier one did and
/// the WM still runs the old one. A waiting restart lives in the hub that waits, and goes with it
/// when that hub updates itself: a node's WM ran its replaced binary 4 h through one devhub
/// landing after another, none of which replaced it again (11-07).
fn auto_wm_restart(wm_replaced: bool, stale: &[super::jits::Jit]) -> Option<&'static str> {
    if wm_replaced {
        return Some("the build replaced the WM binary");
    }
    stale.iter().any(|j| j.app == "wm" || j.app == "makepad-app-wm").then_some("an earlier build replaced the WM binary, and it still runs the old one")
}

/// What the WM's session runs by itself: not a user's apps.
const WM_SERVICES: [&str; 4] = ["wm", "aichat", "aihub", "devhub"];

/// Whether `app` is one of [`WM_SERVICES`].
fn wm_own(app: &str) -> bool {
    WM_SERVICES.iter().any(|s| app == *s || app == format!("makepad-app-{s}"))
}

/// What of the WM's own apps keeps the box busy now, if anything: a timed lease starts after it
/// (`Exec::builds_here`). A WM restart starts its AI pane again, and the pane's JIT compiles for a
/// while: a 12 s timed lease granted as it began ran beside `makepad` at 107% of a core (11-06).
pub fn wm_apps_busy() -> Option<String> {
    let own: Vec<(String, u32)> = super::jits::list().into_iter().filter(|j| wm_own(&j.app)).map(|j| (j.app, j.pid)).collect();
    if own.is_empty() {
        return None;
    }
    let pids: Vec<u32> = own.iter().map(|o| o.1).collect();
    busy_wm_apps(&own, &crate::top::cpu_of(&pids))
}

/// [`wm_apps_busy`]'s verdict on the WM's own apps (`own`: app, pid) at their CPU (pid, % of a
/// core): those at [`crate::lease::master::NOISY_CPU_PCT`] or more, which a timed run beside them
/// would count as noise.
fn busy_wm_apps(own: &[(String, u32)], cpu: &[(u32, f64)]) -> Option<String> {
    let busy: Vec<String> = cpu.iter().filter(|(_, c)| *c >= crate::lease::master::NOISY_CPU_PCT)
        .filter_map(|(pid, c)| own.iter().find(|o| o.1 == *pid).map(|(app, _)| format!("{app}#{pid} at {c:.0}% of a core")))
        .collect();
    (!busy.is_empty()).then(|| format!("the WM's own {} (starting, or taking a landing)", busy.join(", ")))
}

/// Whether the sync restarts `app` itself for an edit at `rung`: the WM's AI pane child (aichat)
/// on a box nobody watches (`person`: someone does, [`crate::lease::exec::user_watching`]) is a
/// node service, not a person's app, so an edit
/// it can take only by a restart is applied at once (its JIT's `session relaunch`). It waited for
/// a click nobody makes, and every sync said "needs a restart" again on five boxes (11-06). The
/// WM itself restarts only when nothing is lost by it (`Sync::wm_restart`).
fn restarts_with_the_landing(app: &str, rung: &str, person: bool, always: bool) -> bool {
    rung == "restart" && match always {
        // a dev box runs the latest (the user, 11-22), a person at it or not
        true => !restarts_by_its_own_rules(app),
        false => !person && wm_own(app) && matches!(app.trim_start_matches("makepad-app-"), "aichat"),
    }
}

/// The apps whose restarts are not a landing's relaunch: the WM (its restart, [`auto_wm_restart`]),
/// the AI hub it starts (with it) and the devhub app (the hub updates itself).
fn restarts_by_its_own_rules(app: &str) -> bool {
    matches!(app.trim_start_matches("makepad-app-"), "wm" | "aihub" | "devhub")
}

/// Whether an app on a dev box that runs a replaced JIT binary restarts onto the latest
/// (the sync's `to_the_latest`): not one that restarts by its own rules, a live lease's (its lease
/// decides) or a headless lease's run, nor one whose own inputs did not change (`own`: its code is
/// the latest); never over unsaved work (`unsaved`). Err(Some): why not, said; Err(None): nothing
/// to say.
fn takes_the_latest(app: &str, lease: crate::lease::exec::AppLease, headless: bool, own: impl FnOnce() -> Option<bool>, unsaved: impl FnOnce() -> bool) -> Result<(), Option<&'static str>> {
    if restarts_by_its_own_rules(app) || headless || matches!(lease, crate::lease::exec::AppLease::Live { .. }) || own() == Some(false) {
        return Err(None);
    }
    if unsaved() {
        return Err(Some("it has unsaved work"));
    }
    Ok(())
}

/// A makepad checkout dir that holds only the nested commercial checkout (a fresh tree whose
/// commercial side came first).
pub(crate) fn only_commercial(dir: &std::path::Path) -> bool {
    dir.read_dir().is_ok_and(|d| d.flatten().all(|e| e.file_name() == "apps")) && dir.join("apps").read_dir().is_ok_and(|d| d.flatten().all(|e| e.file_name() == "commercial"))
}

fn push_factory(sync: Arc<Sync>) -> Factory {
    Arc::new(move |conn, open| {
        let hooks = conn.hooks();
        let sync = sync.clone();
        std::thread::Builder::new()
            .name("devhub-sync-push".into())
            .spawn(move || {
                (hooks.enter)();
                let ch = open.channel;
                let line = match sync.receive(&ch) {
                    Ok(l) => l,
                    Err(e) => {
                        sync.event(Sev::Warn, format!("sync: push refused: {e}"), String::new());
                        err_line(&e)
                    }
                };
                let _ = ch.send(format!("{line}\n{{\"end\":\"push\",\"ms\":0}}\n").into_bytes());
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (CLOSE_REFUSED, format!("cannot start the push thread: {e}")))
    })
}

#[cfg(test)]
mod tests {

    /// A hub's own build and an apply never overlap: each marks itself and then looks at the
    /// other, so of two that start together one always sees the other and waits.
    #[test]
    fn a_hub_build_and_an_apply_of_its_checkout_exclude_each_other() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let (build, apply) = (AtomicBool::new(false), AtomicBool::new(false));
        assert!(super::begin_beside(&build, &apply), "nothing applies: the build starts");
        assert!(!super::begin_beside(&apply, &build) && !apply.load(SeqCst), "an apply waits for the build, unmarked");
        build.store(false, SeqCst);
        assert!(super::begin_beside(&apply, &build), "the build ended: the apply runs");
        assert!(!super::begin_beside(&build, &apply) && !build.load(SeqCst), "a build waits for the apply");
    }
    use super::*;

    /// A deferred waiter and an order that queued behind an apply both asked for a WM restart at
    /// one generation: whichever gets the sync lock first restarts, the other skips (one restart,
    /// not two 2 s apart). An ask after it restarts again, and a failed restart moves nothing.
    #[test]
    fn a_waiter_and_a_queued_order_restart_the_wm_once() {
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};
        let (generation, restarts, busy) = (&AtomicU64::new(7), &AtomicUsize::new(0), &Mutex::new(()));
        let asked = generation.load(SeqCst);
        let outcomes: Vec<Option<String>> = std::thread::scope(|s| {
            let ask = |who: &'static str| s.spawn(move || {
                let _busy = busy.lock().unwrap();
                restart_once(generation, asked, || {
                    restarts.fetch_add(1, SeqCst);
                    Ok(format!("restarted for the {who}"))
                }).ok().flatten()
            });
            let (waiter, order) = (ask("waiter"), ask("order"));
            vec![waiter.join().unwrap(), order.join().unwrap()]
        });
        assert_eq!(restarts.load(SeqCst), 1, "two asks at one generation restarted the WM twice: {outcomes:?}");
        assert_eq!(outcomes.iter().filter(|o| o.is_none()).count(), 1, "the second ask skips: {outcomes:?}");
        assert_eq!(generation.load(SeqCst), 8);
        // a new ask, after that restart, is one of its own
        let again = restart_once(generation, generation.load(SeqCst), || Ok("again".into()));
        assert!(matches!(again, Ok(Some(_))), "an ask after the restart restarts: {:?}", again.as_ref().err().map(|_| ()));
        // a refused restart (blocked, failed) moves nothing: the ask's later try still restarts
        let asked = generation.load(SeqCst);
        assert!(restart_once(generation, asked, || Err(NotRestarted::Blocked("a screen lease".into()))).is_err());
        assert!(matches!(restart_once(generation, asked, || Ok("after the wait".into())), Ok(Some(_))), "a blocked try kept the ask");
    }

    /// An apply moves each repository its order names, makepad first, and the deployment's
    /// FLEET-REVISION.txt keeps every repository it holds, moved or not.
    #[test]
    fn an_apply_moves_each_repository_its_order_names() {
        let (a, b, c, e) = ("a".repeat(40), "b".repeat(40), "c".repeat(40), "e".repeat(40));
        let o = Order { words: format!("apply repo-b={e} makepad={a} commercial={c} build=on restart=auto session=s1 landed=17 prewarm=on jobs=3").split(' ').map(str::to_string).collect(), signer: String::new() };
        assert_eq!(order_unknown(&o), None);
        for word in ["jbos=3", "jobs", "commercial=abc", "makepad=bad", "bad/name=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"] {
            let mut bad = Order { words: o.words.clone(), signer: String::new() };
            bad.words.push(word.into());
            assert_eq!(order_unknown(&bad).as_deref(), Some(word));
        }
        assert_eq!(order_repos(&o), [("makepad".to_string(), a.clone()), ("repo-b".to_string(), e.clone()), ("commercial".to_string(), c.clone())]);
        let was = crate::lease::Pin::at(&b).with("commercial", &b);
        assert_eq!(deployed_after(Some(was.clone()), &[("repo-b".into(), e.clone())]), Some(was.clone().with("repo-b", &e)), "what did not move stays");
        assert_eq!(deployed_after(None, &[("commercial".into(), c.clone())]), None, "no makepad commit: no deployment pin");
        let dir = std::env::temp_dir().join(format!("devhub-fleet-pin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pin = crate::lease::Pin::at(&a).with("commercial", &c).with("repo-b", &e);
        git::write_fleet_revision(&dir, &pin);
        assert_eq!(git::fleet_pin(&dir), Some(pin));
        assert_eq!(git::fleet_revision(&dir, "repo-b"), Some(e));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A waiting WM restart drains its box when leases alone hold it, whatever it restarts for, and
    /// on through the gap until it runs: a person at it, unsaved work, a viewer or an app no lease
    /// runs keep the box at work while it waits.
    #[test]
    fn a_wm_restart_drains_for_leases_and_never_for_a_person() {
        // whose app it waits for: a lease's and a person's, or one nothing names; not an
        // agent's no lease runs, nor a leftover (11-13)
        use crate::lease::exec::AppLease;
        let (person, agent) = (Launcher::Person("at the WM".into()), Launcher::Agent("devhub show, through the WM socket".into()));
        assert!(super::waits_for(AppLease::Live { on_wm: true }, None));
        assert!(super::waits_for(AppLease::Nobody, Some(&person)) && super::waits_for(AppLease::Nobody, Some(&Launcher::Pool)) && super::waits_for(AppLease::Nobody, None));
        assert!(!super::waits_for(AppLease::Nobody, Some(&agent)) && !super::waits_for(AppLease::Left, Some(&person)));
        // but a Desk Link viewer showing the node holds it while any app runs there, an agent's
        // show among them, input or not: a person sees it (11-21); on an empty desk it holds nothing
        let party = vec![("makepad-app-party".to_string(), 51)];
        assert_eq!(super::viewer_holds(true, &party).as_deref(), Some("a Desk Link viewer shows this node while makepad-app-party (pid 51) runs on its WM: a person sees it, and a restart would close it"));
        assert_eq!(super::viewer_holds(false, &party), None, "no viewer shows it");
        assert_eq!(super::viewer_holds(true, &[]), None, "nothing to close");
        assert_eq!(super::opened_said(Some(&agent), true), "opened by an agent (devhub show, through the WM socket)");
        assert!(super::opened_said(None, true).starts_with("opened through the WM before it named who opens apps"), "a WM from before launch marks opened it");
        assert_eq!(super::opened_said(None, false), "not opened through the WM: a shell started it");
        let leases = vec!["screen lease L7 of lane-s (its windows)".to_string()];
        let leased = vec![("makepad-sqlite".to_string(), 41, true)];
        assert_eq!(super::drain_said(&[], &leases, &leased), "screen lease L7 of lane-s (its windows); makepad-sqlite (pid 41) of a lease", "a restart for any reason drains while leases hold it");
        assert_eq!(super::drain_said(&[], &leases, &[]), "screen lease L7 of lane-s (its windows)");
        assert_eq!(super::drain_said(&["a user is at the node".into()], &leases, &leased), "", "a person: no drain");
        let mine = vec![("makepad-fab".to_string(), 42, false), leased[0].clone()];
        assert_eq!(super::drain_said(&[], &leases, &mine), "", "a person's app: no drain");
        assert_eq!(super::drain_said(&[], &[], &[]), super::DRAIN_GAP, "nothing holds it: it drains on until its next look restarts it");
        // a dev box runs the latest (11-22): only a lease's apps hold it, and unsaved work where its
        // WM keeps no apps; elsewhere a person's reasons and apps hold it as before
        let unsaved = vec!["unsaved work in makepad-fab (pid 42): a sketch".to_string()];
        let (watched, shown) = (Some("someone watched its screen 20 s ago".to_string()), Some("a Desk Link viewer shows this node".to_string()));
        let held = |always, keeps| super::holding(always, keeps, unsaved.clone(), watched.clone(), shown.clone(), mine.clone(), None);
        assert_eq!(held(false, true), ([unsaved.clone(), vec![watched.clone().unwrap(), shown.clone().unwrap()]].concat(), mine.clone()), "a person's box keeps every hold");
        assert_eq!(held(true, true), (vec![], leased.clone()), "a keeping WM loses nothing: only a lease's app holds it");
        assert_eq!(held(true, false), (unsaved.clone(), leased.clone()), "where the WM closes its apps, unsaved work holds it");
        let (people, apps) = held(true, true);
        assert_eq!(super::drain_said(&people, &leases, &apps), "screen lease L7 of lane-s (its windows); makepad-sqlite (pid 41) of a lease", "a person's app on a dev box: it drains for the lease");
        assert!(super::notice_keeps("epoch=3c47398a2a4781b9\nkeep=1\nlane=\n") && !super::notice_keeps("keep=0\nlane=\n") && !super::notice_keeps("keep=10\n"));
        // a restart only leases hold looks again the second none of them shows on the WM: a lease
        // between two app launches holds it for seconds, and a look every 10 s missed those gaps
        // (11-24); a person's reason is looked at every 10 s
        let s = std::time::Duration::from_secs;
        assert!(super::restart_looks(Some(s(1)), true, || false), "a lease's gap: it looks now");
        assert!(!super::restart_looks(Some(s(1)), true, || true) && !super::restart_looks(Some(s(9)), false, || panic!("no cheap look while a person holds it")));
        assert!(super::restart_looks(Some(s(10)), false, || true) && super::restart_looks(Some(s(10)), true, || true), "its whole look every 10 s");
        // a hub that restarts mid-wait (its self-update) ends its wait with it, and the next hub's
        // resumed wait takes the lease's gap at its first look while an apply still builds under
        // the sync lock: the resume queued on that lock, and the gap went by (11-24)
        let busy = crate::held::Held::new("sync apply", ());
        let apply = busy.lock().unwrap();
        let (mut ticks, mut looks) = (0, 0);
        super::restart_wait(|| { ticks += 1; ticks <= 3 }, || {}, || { looks += 1; (leases.clone(), true) }, || true, || panic!("a lease holds it: no restart"));
        assert_eq!((ticks, looks), (4, 1), "the first hub looked once, then its lease showed until it exited");
        let (mut ticks, mut claims) = (0, 0);
        super::restart_wait(|| { ticks += 1; ticks <= 5 }, || {}, || (vec![], true), || false, || {
            assert!(busy.try_lock().is_err(), "the apply still builds");
            claims += 1;
            true
        });
        assert_eq!((ticks, claims), (1, 1), "the resumed wait restarted at its first second, beside the apply");
        drop(apply);
        // the hub builds while the restart waits; a deployment it does not build at holds the
        // restart on every box, and the box stays at work: no drain for a restart that cannot run
        let unbuilt = super::unbuilt_said("0123456789abcdef", "error[E0425]: cannot find value `x`");
        assert_eq!(unbuilt, "the hub does not build at 0123456789 (error[E0425]: cannot find value `x`): it restarts the WM once a deployment builds it, and the WM and this hub go on meanwhile");
        for (always, keeps) in [(true, true), (true, false), (false, true)] {
            let (people, apps) = super::holding(always, keeps, vec![], None, None, leased.clone(), Some(unbuilt.clone()));
            assert_eq!(people, vec![unbuilt.clone()], "it holds the restart (dev box {always}, keeping WM {keeps})");
            assert_eq!(super::drain_said(&people, &leases, &apps), "", "and drains nothing (dev box {always}, keeping WM {keeps})");
            assert_eq!(super::drain_said(&people, &[], &[]), "", "not even in a lease's gap (dev box {always}, keeping WM {keeps})");
        }
        // each restart says how long it waited and on what, each thing once in the order it came,
        // and keeps that over a hub that took the wait over: only its start was said (11-24)
        let mut on = vec![leases[0].clone()];
        super::note_waited(&mut on, &[leases[0].clone(), "makepad-sandbox (pid 9) of a lease".into()]);
        super::note_waited(&mut on, &["makepad-sandbox (pid 9) of a lease".into()]);
        assert_eq!(on, [leases[0].clone(), "makepad-sandbox (pid 9) of a lease".to_string()]);
        let why = "the build replaced the WM binary";
        assert_eq!(super::waited_said(why, 1_000, &on, 46_000), format!("{why}, after 45 s waiting on: {}; makepad-sandbox (pid 9) of a lease", leases[0]));
        assert_eq!(super::waited_said(why, 1_000, &[], 3_000), format!("{why}, after 2 s waiting for its first look"));
        assert_eq!(super::waiting_read(&format!("{why}\n1000\n")), (why, 1000), "the wait a hub before this one began");
        assert_eq!(super::waiting_read(&format!("{why}\n")), (why, 0), "a marker from before it said when: from now");
    }

    /// An `auto` sync restarts a WM whose binary this build replaced, or an earlier one did (its
    /// waiting restart went with the hub that updated itself); a stale JIT app is no reason.
    #[test]
    fn a_wm_an_earlier_build_replaced_restarts_at_the_next_sync() {
        let jit = |app: &str| super::super::jits::Jit { pid: 7, app: app.into() };
        assert_eq!(auto_wm_restart(true, &[]), Some("the build replaced the WM binary"));
        assert_eq!(auto_wm_restart(false, &[jit("makepad-app-wm")]), Some("an earlier build replaced the WM binary, and it still runs the old one"));
        assert_eq!(auto_wm_restart(false, &[jit("makepad-app-aichat"), jit("makepad-fab")]), None);
        assert_eq!(auto_wm_restart(false, &[]), None);
        // what still runs a replaced build, said for the box's caps: when its checkout replaced
        // that build, and its start beside it; since its oldest replacement, which a later build
        // moves no further. Its start alone read as how far behind it was: a WM 25 min old whose
        // restart was 34 s into its wait for a lease (11-24)
        let at = 1_000_000_000u64;
        let unknown = |_: u32| 0u64;
        let runs = [("makepad-app-wm".to_string(), 41, at), ("makepad-fab".to_string(), 52, at + 3_600_000), ("makepad-x".to_string(), 53, 0)];
        let (said, since) = super::stale_said(&runs, Some("makepad-sandbox runs (pid 60, opened by a person)"), &|_| None, &unknown, at + 7_200_000);
        assert_eq!(said, "makepad-app-wm#41 started 2h00m ago, and its checkout has since rebuilt it; makepad-fab#52 started 1h00m ago, and its checkout has since rebuilt it; makepad-x#53 runs a build its checkout has since rebuilt; its WM restart waits for: makepad-sandbox runs (pid 60, opened by a person)");
        assert_eq!(since, at, "no replacement time known: since its oldest stale app started");
        let replaced = |pid: u32| match pid { 41 => at + 7_200_000 - 34_000, 53 => at + 7_000_000, _ => 0 };
        let (said, since) = super::stale_said(&runs, None, &|_| None, &replaced, at + 7_200_000);
        assert_eq!(said, "makepad-app-wm#41 runs the build its checkout replaced 34 s ago (it started 2h00m ago); makepad-fab#52 started 1h00m ago, and its checkout has since rebuilt it; makepad-x#53 runs the build its checkout replaced 3m20s ago");
        assert_eq!(since, at + 3_600_000, "since its oldest replacement, a start standing in where none is known");
        assert_eq!(super::stale_said(&runs[..1], None, &|_| None, &replaced, at + 7_200_000).1, at + 7_200_000 - 34_000, "behind its checkout for 34 s, not for its 2 h");
        assert_eq!(super::stale_said(&[], None, &|_| None, &unknown, at), (String::new(), 0), "nothing stale: nothing said");
        assert_eq!(super::stale_said(&[], Some(""), &|_| None, &unknown, at).0, "its WM restart waits for: nothing now (it runs at its next look)");
        // a rebuilt AI hub whose restart waits on its jobs says so beside it (11-23: a Linux node's ran its
        // old build 1 h, and nothing said why)
        let hub = [("makepad-app-aihub".to_string(), 70, at)];
        let waits = |pid: u32| (pid == 70).then(|| crate::lease::exec::busy_said(Some(1)).unwrap());
        assert_eq!(super::stale_said(&hub, None, &waits, &unknown, at + 60_000).0, "makepad-app-aihub#70 started 1m00s ago, and its checkout has since rebuilt it; its restart waits: busy: 1 job");
        assert_eq!(crate::lease::exec::busy_said(Some(3)).as_deref(), Some("busy: 3 jobs"));
        assert_eq!((crate::lease::exec::busy_said(Some(0)), crate::lease::exec::busy_said(None)), (None, None), "an idle hub, or one that says no count, restarts");
        // the apps a keeping WM left waiting while no WM takes them back, for `sync status`
        assert_eq!(super::kept_scope("0::/user.slice/user-1000.slice/user@1000.service/app.slice/makepad-wm-app-0123456789abcdef-17.scope\n"),
            Some("makepad-wm-app-0123456789abcdef-17.scope"));
        assert_eq!(super::kept_scope("0::/user.slice/user-1000.slice/session-4.scope\n"), None, "a process outside a kept scope waits for nothing");
        assert_eq!(super::left_ms("epoch=ab12\nkeep=1\nlane=\nleft_ms=1000\n"), Some(1000));
        assert_eq!(super::left_ms("epoch=ab12\nkeep=1\nlane=\n"), None, "a WM that crashed said nothing");
        let apps = [("makepad-app-terminal".to_string(), 81762), ("makepad-app-calc".to_string(), 81800)];
        assert_eq!(super::waiting_said(&apps, Some(at), at + 180_000), format!("makepad-app-terminal#81762, makepad-app-calc#81800 wait for a WM, {} now: theirs went away for a next one", crate::lease::sched::dur(180_000)));
        assert_eq!(super::waiting_said(&apps[..1], None, at), "makepad-app-terminal#81762 wait for a WM: theirs ended without saying when (a crash)");
        assert_eq!(super::waiting_said(&[], Some(at), at), "", "no app waits: nothing said");
        // a replaced loader alone is no older code: only a JIT whose own inputs changed, or one that
        // does not say, runs older code than its tree
        let asked = vec![("makepad-app-wm".to_string(), 41, at, None), ("makepad-fab".to_string(), 52, at, Some(false)), ("makepad-x".to_string(), 53, at, Some(true))];
        assert_eq!(super::own_stale(asked.clone(), |_| false), [("makepad-app-wm".to_string(), 41, at), ("makepad-x".to_string(), 53, at)]);
        // one that does not say (the AI hub, no JIT), relinked byte for byte by a landing that
        // changed nothing it is built of, runs its checkout's build: every Linux box was said to
        // run older code for 4 h while its restart rightly said it ran the deployment's (11-23)
        let asked = [asked, vec![("makepad-app-aihub".to_string(), 61, at, None)]].concat();
        assert_eq!(super::own_stale(asked, |pid| pid == 61), [("makepad-app-wm".to_string(), 41, at), ("makepad-x".to_string(), 53, at)], "a rebuilt file of the same bytes is no older code");
        // what tells them apart: the bytes of the file a build put at the running binary's path
        #[cfg(target_os = "linux")]
        {
            let dir = std::env::temp_dir().join(format!("devhub-rebuilt-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = dir.join("prog");
            std::fs::copy("/bin/sleep", &bin).unwrap();
            let mut child = std::process::Command::new(&bin).arg("30").spawn().unwrap();
            let pid = child.id();
            let bytes = std::fs::read(&bin).unwrap();
            assert_eq!(super::super::jits::replaced_ms(pid), 0, "a binary no build replaced");
            // a build writes a new file and renames it over the old one, as cargo's link does
            let rebuild = |b: &[u8]| {
                let tmp = dir.join("prog.new");
                std::fs::write(&tmp, b).unwrap();
                std::fs::rename(&tmp, &bin).unwrap();
            };
            let before = crate::proto::wall_ms();
            rebuild(&bytes);
            let after = crate::proto::wall_ms();
            assert!(super::super::jits::exe_replaced(pid) && super::rebuilt_same(pid), "relinked byte for byte: the checkout's build");
            // when the build replaced what it runs: the file's change time, which no later build moves
            let replaced = super::super::jits::replaced_ms(pid);
            assert!(replaced + 1000 >= before && replaced <= after + 1000, "replaced at {replaced}, the build ran {before}..{after}");
            rebuild(&[bytes.as_slice(), b"\0"].concat());
            assert!(!super::rebuilt_same(pid), "other bytes: older code");
            assert_eq!(super::super::jits::replaced_ms(pid), replaced, "a later build replaces a file it no longer runs");
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The AI pane's restart is the sync's on a box nobody uses; a person's app, the WM itself
    /// and a box in use keep their offer.
    /// A timed lease waits while the WM's own apps are busy (its AI pane compiling after a WM
    /// restart), not for their idle hum, and never for an app that is not the WM's.
    #[test]
    fn a_busy_wm_app_holds_a_timed_start() {
        let own = [("makepad-app-wm".to_string(), 10), ("makepad-app-aichat".to_string(), 11)];
        assert_eq!(busy_wm_apps(&own, &[(10, 15.0), (11, 107.0)]).as_deref(), Some("the WM's own makepad-app-aichat#11 at 107% of a core (starting, or taking a landing)"));
        assert_eq!(busy_wm_apps(&own, &[(10, 15.0), (11, 4.0)]), None, "the WM's idle hum");
        assert_eq!(busy_wm_apps(&own, &[(99, 300.0)]), None, "not the WM's");
    }

    #[test]
    fn the_ai_panes_restart_is_taken_by_the_sync_on_an_unused_box() {
        assert!(restarts_with_the_landing("makepad-app-aichat", "restart", false, false));
        assert!(restarts_with_the_landing("aichat", "restart", false, false));
        assert!(!restarts_with_the_landing("makepad-app-aichat", "restart", true, false), "a person uses the box");
        assert!(!restarts_with_the_landing("makepad-app-aichat", "patch", false, false));
        assert!(!restarts_with_the_landing("makepad-app-wm", "restart", false, false), "the WM restarts by its own rules");
        assert!(!restarts_with_the_landing("makepad-app-fab", "restart", false, false), "a person's app");
        // a dev box runs the latest (11-22): any app's restart, a person there or not
        assert!(restarts_with_the_landing("makepad-app-terminal", "restart", true, true), "a person's open terminal on a dev box");
        assert!(restarts_with_the_landing("makepad-app-aichat", "restart", true, true));
        assert!(!restarts_with_the_landing("makepad-app-terminal", "patch", true, true), "a patch needs no restart");
        assert!(!restarts_with_the_landing("makepad-app-wm", "restart", false, true) && !restarts_with_the_landing("makepad-app-aihub", "restart", false, true), "the WM and its hub restart by their own rules");
        // and an app a build left on an older JIT binary restarts onto it there, with its state
        use crate::lease::exec::AppLease;
        let takes = |app: &str, lease, headless, own: Option<bool>, unsaved: bool| super::takes_the_latest(app, lease, headless, || own, || unsaved);
        assert_eq!(takes("makepad-app-terminal", AppLease::Nobody, false, Some(true), false), Ok(()), "a person's terminal");
        assert_eq!(takes("makepad-app-aichat", AppLease::Left, false, None, false), Ok(()), "a JIT that cannot say restarts too");
        assert_eq!(takes("makepad-app-fab", AppLease::Nobody, false, Some(true), true), Err(Some("it has unsaved work")), "never over unsaved work");
        assert_eq!(takes("makepad-app-fab", AppLease::Nobody, false, Some(false), true), Err(None), "its own code did not change");
        assert_eq!(takes("makepad-app-fab", AppLease::Live { on_wm: true }, false, Some(true), false), Err(None), "its lease decides");
        assert_eq!(takes("makepad-app-fab", AppLease::Nobody, true, Some(true), false), Err(None), "a headless lease's run");
        assert_eq!(takes("makepad-app-wm", AppLease::Nobody, false, Some(true), false), Err(None), "the WM restarts by its own rules");
    }
}
