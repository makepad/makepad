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
        Arc::new(Sync { ctx, conf: Arc::new(conf), seen: Seen::default(), busy: crate::held::Held::new("sync apply", ()), last: Mutex::new("null".into()), stale_said: Mutex::new(Default::default()) })
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
                let _busy = me.busy.lock().unwrap();
                if let Err(e) = me.wm_restart(&format!("its WM speaks another hosted wire ({answer}): no app starts under it")) {
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
            "{{\"machine\":{},\"checkout\":{},\"build\":\"{}\",\"wm_service\":{},\"dev\":{},\"busy\":{},\"prewarm\":{},\"repos\":{{{}}},\"last\":{}}}",
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
                    let _busy = self.busy.lock().unwrap();
                    match self.wm_restart(&format!("asked by {}", o.signer)) {
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
        // the two always sees the other.
        APPLYING.store(true, std::sync::atomic::Ordering::SeqCst);
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
            let why_not = if !crate::lease::exec::writes_allowed() {
                Some("dev off")
            } else if crate::lease::exec::user_present() {
                Some("a person uses it")
            } else if !crate::lease::exec::held().is_empty() {
                Some("a lease holds the node")
            } else if !built_ok {
                Some("the prebuild failed")
            } else {
                None
            };
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
                        let own = restarts_with_the_landing(&j.app, &e.rung, crate::lease::exec::user_present());
                        let (sev, what) = match e.rung.as_str() {
                            "patch" | "partial" => (Sev::Info, format!("patched {} fns in {:.0} ms", e.fns, e.ms)),
                            "restart" if own => match jits::relaunch(j.pid) {
                                Ok(how) => (Sev::Info, format!("restarted for the landing (the WM's AI pane, a node service): {}; it answered {how}", e.reasons)),
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
        match self.wm_restart(why) {
            Ok(l) => out.push(l),
            Err(e) => self.refuse(out, format!("sync: WM restart refused: {e}")),
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
    fn wm_restart(self: &Arc<Self>, why: &str) -> Result<String, String> {
        if restart_blockers().is_empty() {
            match self.restart_wm_now(why) {
                Ok(line) => return Ok(line),
                Err(NotRestarted::Failed(e)) => return Err(e),
                // a lease that started while the hub built: waited for like any other
                Err(NotRestarted::Blocked(_)) => {}
            }
        }
        if !crate::lease::exec::writes_allowed() {
            return Err(format!("{} is not available for dev: WM restarts need its dev switch on", self.ctx.machine));
        }
        let list = restart_blockers().join("; ");
        if WM_RESTART_WAITING.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Ok(format!("{{\"wm_restart\":\"waiting\",\"for\":{}}}", json_str(&list)));
        }
        self.event(Sev::Warn, format!("sync: the WM restart ({why}) waits for: {list}"), String::new());
        // the hub that takes over from this one goes on waiting ([`Self::resume_wm_restart`])
        let _ = std::fs::create_dir_all(proto::home().join("sync"));
        let _ = std::fs::write(Self::waiting_marker(), &why);
        let me = self.clone();
        let why = why.to_string();
        let spawned = std::thread::Builder::new().name("devhub-wm-restart-wait".into()).spawn(move || {
            loop {
                // (a WM on another wire drains its box meanwhile, [`wm_drain`]: the restart follows the
                // last lease's end; one that still hosts every app restarts in the next gap)
                std::thread::sleep(Duration::from_secs(10));
                if !restart_blockers().is_empty() {
                    continue;
                }
                let _busy = me.busy.lock().unwrap();
                match me.restart_wm_now(&format!("{why}, after the wait")) {
                    Ok(_) => break,
                    Err(NotRestarted::Blocked(list)) => me.event(Sev::Info, format!("sync: the WM restart ({why}) waits on: {list} (it started while the hub built)"), String::new()),
                    Err(NotRestarted::Failed(e)) => {
                        me.event(Sev::Err, format!("sync: WM restart refused: {e}"), String::new());
                        break;
                    }
                }
            }
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
        let Ok(why) = std::fs::read_to_string(Self::waiting_marker()) else { return };
        let _ = std::fs::remove_file(Self::waiting_marker());
        let stale: Vec<jits::Jit> = jits::list().into_iter().filter(|j| jits::runs_from(j.pid, &self.conf.checkout) && jits::exe_replaced(j.pid)).collect();
        if auto_wm_restart(false, &stale).is_none() && crate::lease::caps::other_wire().is_none() {
            return;
        }
        let _busy = self.busy.lock().unwrap();
        if let Err(e) = self.wm_restart(why.trim()) {
            self.event(Sev::Warn, format!("sync: WM restart refused: {e}"), String::new());
        }
    }

    /// Restarts the WM (and with it the AI Hub it starts) through its systemd service, with the
    /// sudoers rule's exact command (no password prompt). The services the WM supervises (this
    /// hub, the AI Hub) run in scopes of their own and outlive it; the marker lets a hub that
    /// does restart report it. Its last look at what the restart ends and the restart run while
    /// no lease starts here.
    fn restart_wm_now(&self, why: &str) -> Result<String, NotRestarted> {
        let failed = NotRestarted::Failed;
        if !crate::lease::exec::writes_allowed() {
            return Err(failed(format!("{} is not available for dev: WM restarts need its dev switch on", self.ctx.machine)));
        }
        let svc = self.conf.wm_service.clone().ok_or_else(|| failed("this node has no WM service (wm_service = none)".into()))?;
        // The new WM starts this hub again when the deployment moved its binary: built first, that
        // start is a no-op `cargo run` (a node's compiled 13 s behind a package-cache lock while its
        // hub refused connections, 11-02). A hub that does not build keeps the WM and this hub.
        #[cfg(feature = "checkout")]
        crate::lease::exec::build_hub(&self.conf.checkout, None).map_err(|e| failed(format!("the hub does not build at this deployment ({e}): the WM is not restarted, this hub keeps running")))?;
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
            self.event(Sev::Info, format!("sync: restarting the WM ({svc}): {why}"), String::new());
            let st = makepad_remote::group::command("sudo").args(["-n", "/usr/bin/systemctl", "restart", &svc]).stdin(std::process::Stdio::null()).output().map_err(|e| failed(format!("sudo: {e}")))?;
            if !st.status.success() {
                let _ = std::fs::remove_file(Self::marker());
                return Err(failed(format!("sudo -n systemctl restart {svc}: {}", proto::clean(String::from_utf8_lossy(&st.stderr).trim(), 300))));
            }
            let active = makepad_remote::group::command("systemctl").args(["is-active", &svc]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
            let _ = std::fs::remove_file(Self::marker());
            let line = format!("sync: WM restarted ({svc} {active})");
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

/// Why a WM restart did not happen.
enum NotRestarted {
    /// what it would end, seen in its last look (a lease that started after the wait's look)
    Blocked(String),
    Failed(String),
}

/// What a WM restart would cost now: the leases using the WM ([`crate::lease::exec::on_the_wm`]), a user at the node,
/// the unsaved work of its apps ([`crate::lease::exec::user_blockers`]), a viewer watching and any
/// user app running. Empty: it may restart.
fn restart_blockers() -> Vec<String> {
    let mut out: Vec<String> = crate::lease::exec::on_the_wm().into_iter().chain(crate::lease::exec::user_blockers()).collect();
    if crate::lease::exec::user_watching() {
        out.push("a Desk Link viewer shows this node (input within 10 min)".to_string());
    }
    // a WM restart ends every app in its session: it never takes a user's app with it (it
    // killed Fab on a pool box, 3 min after the last input, 2026-10-30)
    // (a headless lease's JIT runs on through it: its lease is not on the WM)
    out.extend(apps_on_the_wm().into_iter().map(|(app, pid, leased)| if leased { format!("{app} (pid {pid}) of a lease") } else { format!("{app} runs (pid {pid}, {}): close it to let the WM update", opened_said(launched_by(pid).as_ref(), wm_child(pid))) }));
    out
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

/// What the box's waiting WM restart waits on when only leases hold it ([`drain_said`]), for its
/// caps: the master starts no new lease there the restart would wait for, so it restarts in the
/// gap. Back-to-back landings kept four boxes' WMs waiting for hours: each landing's sync apply
/// held the waiter while the next tests run took the box (11-07).
pub fn wm_drain() -> String {
    if !WM_RESTART_WAITING.load(std::sync::atomic::Ordering::Acquire) {
        return String::new();
    }
    let mut people = crate::lease::exec::user_blockers();
    if crate::lease::exec::user_watching() {
        people.push("a Desk Link viewer shows this node".into());
    }
    #[cfg(target_os = "linux")]
    let wire = crate::lease::caps::other_wire().is_some();
    #[cfg(not(target_os = "linux"))]
    let wire = false;
    drain_said(&people, &crate::lease::exec::on_the_wm(), &apps_on_the_wm(), wire)
}

/// A waiting WM restart's drain: what it waits on (`leases`, and the apps it would end: app, pid,
/// whether a lease runs it) when only leases hold it; empty when a person's reason does (`people`:
/// at the box, unsaved work, a viewer) or an app no lease runs (a person's): those it waits for
/// with the box at work, never drained for.
///
/// Only a WM on another hosted wire than the box's build (`wire`) is drained for: no app starts
/// under it, so the screen work the drain keeps off could not run there anyway. A WM whose binary
/// was only replaced still hosts every app; drained for, a box kept every new screen lease off
/// while a lane's open-ended screen session held its restart: the Source Library lanes' screens
/// waited on two boxes for one session that had no end (11-07). It restarts in a gap instead.
fn drain_said(people: &[String], leases: &[String], apps: &[(String, u32, bool)], wire: bool) -> String {
    if !wire || !people.is_empty() || apps.iter().any(|(_, _, leased)| !leased) {
        return String::new();
    }
    let mut on: Vec<String> = leases.to_vec();
    on.extend(apps.iter().map(|(app, pid, _)| format!("{app} (pid {pid}) of a lease")));
    on.join("; ")
}

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
/// on a box nobody uses (`person`: someone does) is a node service, not a person's app, so an edit
/// it can take only by a restart is applied at once (its JIT's `session relaunch`). It waited for
/// a click nobody makes, and every sync said "needs a restart" again on five boxes (11-06). The
/// WM itself restarts only when nothing is lost by it (`Sync::wm_restart`).
fn restarts_with_the_landing(app: &str, rung: &str, person: bool) -> bool {
    rung == "restart" && !person && wm_own(app) && matches!(app.trim_start_matches("makepad-app-"), "aichat")
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
    use super::*;

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

    /// A waiting WM restart drains its box only when leases alone hold it and its WM speaks another
    /// wire: a person at it, unsaved work, a viewer, an app no lease runs, or a WM that still hosts
    /// every app keep the box at work while it waits.
    #[test]
    fn a_wm_restart_drains_only_for_leases() {
        // whose app it waits for: a lease's and a person's, or one nothing names; not an
        // agent's no lease runs, nor a leftover (11-13)
        use crate::lease::exec::AppLease;
        let (person, agent) = (Launcher::Person("at the WM".into()), Launcher::Agent("devhub show, through the WM socket".into()));
        assert!(super::waits_for(AppLease::Live { on_wm: true }, None));
        assert!(super::waits_for(AppLease::Nobody, Some(&person)) && super::waits_for(AppLease::Nobody, Some(&Launcher::Pool)) && super::waits_for(AppLease::Nobody, None));
        assert!(!super::waits_for(AppLease::Nobody, Some(&agent)) && !super::waits_for(AppLease::Left, Some(&person)));
        assert_eq!(super::opened_said(Some(&agent), true), "opened by an agent (devhub show, through the WM socket)");
        assert!(super::opened_said(None, true).starts_with("opened through the WM before it named who opens apps"), "a WM from before launch marks opened it");
        assert_eq!(super::opened_said(None, false), "not opened through the WM: a shell started it");
        let leases = vec!["screen lease L7 of lane-s (its windows)".to_string()];
        let leased = vec![("makepad-sqlite".to_string(), 41, true)];
        assert_eq!(super::drain_said(&[], &leases, &leased, true), "screen lease L7 of lane-s (its windows); makepad-sqlite (pid 41) of a lease");
        assert_eq!(super::drain_said(&[], &leases, &[], true), "screen lease L7 of lane-s (its windows)");
        assert_eq!(super::drain_said(&[], &leases, &leased, false), "", "a WM on the build's wire hosts every app: no drain");
        assert_eq!(super::drain_said(&["a user is at the node".into()], &leases, &leased, true), "", "a person: no drain");
        let mine = vec![("makepad-fab".to_string(), 42, false), leased[0].clone()];
        assert_eq!(super::drain_said(&[], &leases, &mine, true), "", "a person's app: no drain");
        assert_eq!(super::drain_said(&[], &[], &[], true), "", "nothing holds it: it restarts now");
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
        assert!(restarts_with_the_landing("makepad-app-aichat", "restart", false));
        assert!(restarts_with_the_landing("aichat", "restart", false));
        assert!(!restarts_with_the_landing("makepad-app-aichat", "restart", true), "a person uses the box");
        assert!(!restarts_with_the_landing("makepad-app-aichat", "patch", false));
        assert!(!restarts_with_the_landing("makepad-app-wm", "restart", false), "the WM restarts by its own rules");
        assert!(!restarts_with_the_landing("makepad-app-fab", "restart", false), "a person's app");
    }
}
