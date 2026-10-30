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
//! An apply moves the synced checkout by fast-forward only, then prebuilds, then restarts what the
//! order asks. Every stage is a `sync` record on the node's stream (pushed, checkout, patched,
//! built, restarted, refused), so the master's inbox and `devhub stream` show it.

use super::build;
use super::conf::{Build, SyncConf};
use super::git::{self, Store, CHECKOUT_REF};
use super::jits;
use super::sign::{self, Order, Seen};
use super::NodeCtx;
use crate::proto::{self, hub_record, Sev};
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
    busy: Mutex<()>,
    last: Mutex<String>,
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn err_line(e: &str) -> String {
    format!("{{\"err\":{}}}", json_str(e))
}

impl Sync {
    pub fn new(ctx: Arc<NodeCtx>, conf: SyncConf) -> Arc<Sync> {
        Arc::new(Sync { ctx, conf: Arc::new(conf), seen: Seen::default(), busy: Mutex::new(()), last: Mutex::new("null".into()) })
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
    }

    /// The commit the checkout of `repo` holds: the store's record of it, else what a deploy
    /// script wrote (FLEET-REVISION.txt).
    fn current(&self, store: &Store, repo: &str) -> Option<String> {
        store.rev(CHECKOUT_REF).or_else(|| git::fleet_revision(&self.conf.checkout, repo))
    }

    fn status(&self, out: &mut Vec<String>) {
        let mut repos = Vec::new();
        for repo in ["makepad", "commercial"] {
            let Ok(store) = Store::open(repo) else { continue };
            let cur = self.current(&store, repo);
            let refs: Vec<String> = store.refs().iter().map(|(r, sha)| format!("{{\"ref\":{},\"sha\":{}}}", json_str(r), json_str(sha))).collect();
            // a tree a deploy script shipped names its commit before the store holds it
            let held = cur.as_deref().is_some_and(|c| store.has(c));
            repos.push(format!("{}:{{\"checkout\":{},\"held\":{held},\"refs\":[{}]}}", json_str(repo), cur.map_or("null".into(), |c| json_str(&c)), refs.join(",")));
        }
        out.push(format!(
            "{{\"machine\":{},\"checkout\":{},\"build\":\"{}\",\"wm_service\":{},\"dev\":{},\"busy\":{},\"repos\":{{{}}},\"last\":{}}}",
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
            _ => out.push(err_line("sync status | apply <order> | wm-restart <order> | tick <pid> <hex inspect line>")),
        }
    }

    fn refuse(&self, out: &mut Vec<String>, why: String) {
        self.event(Sev::Warn, why.clone(), String::new());
        out.push(err_line(&why));
    }

    /// One apply: check every repo can move, wait for the session tick when asked, move the
    /// checkouts, follow the JITs' patches, prebuild, restart.
    fn apply(self: &Arc<Self>, o: &Order, out: &mut Vec<String>) {
        let Ok(_busy) = self.busy.try_lock() else { return out.push(err_line("an apply is running on this node")) };
        let t0 = Instant::now();
        let mut plan: Vec<(&str, Store, Option<String>, String)> = Vec::new();
        for repo in ["makepad", "commercial"] {
            let Some(to) = o.get(repo) else { continue };
            if !git::valid_sha(to) {
                return self.refuse(out, format!("sync: apply refused: {repo}={to} is not a commit id"));
            }
            let store = match Store::open(repo) {
                Ok(s) => s,
                Err(e) => return self.refuse(out, format!("sync: {repo} store: {e}")),
            };
            if !store.has(to) {
                return self.refuse(out, format!("sync: apply refused: {repo} {} was not pushed to this node", short(to)));
            }
            let dir = self.conf.repo_dir(repo).unwrap();
            let from = self.current(&store, repo);
            match &from {
                Some(f) if !store.has(f) => return self.refuse(out, format!("sync: apply refused: the {repo} checkout is at {}, which this node's store does not hold (push a branch that contains it)", short(f))),
                Some(f) if f != to && !store.is_ancestor(f, to) => {
                    return self.refuse(out, format!("sync: apply refused: {repo} {} -> {} is not a fast-forward", short(f), short(to)));
                }
                None if dir.read_dir().is_ok_and(|mut d| d.next().is_some()) && !(repo == "makepad" && only_commercial(&dir)) => {
                    return self.refuse(out, format!("sync: apply refused: {} holds files but no known commit (no FLEET-REVISION.txt)", dir.display()));
                }
                _ => {}
            }
            plan.push((repo, store, from, to.to_string()));
        }
        if plan.is_empty() {
            return out.push(err_line("apply names no commit (makepad=<sha> commercial=<sha>)"));
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

        let mut moved_lines = Vec::new();
        let mut revs: (Option<String>, Option<String>) = (None, None);
        for (repo, store, from, to) in &plan {
            let dir = self.conf.repo_dir(repo).unwrap();
            match store.move_checkout(&dir, from.as_deref(), to) {
                Ok(m) => {
                    let line = format!("sync: {repo} {} -> {} ({} files){}", from.as_deref().map_or("(new)", short), short(to), m.files.len(), if session_line.is_empty() { String::new() } else { format!(", {session_line}") });
                    let detail = format!("{{\"repo\":{},\"from\":{},\"to\":{},\"files\":[{}]}}", json_str(repo), from.as_deref().map_or("null".into(), json_str), json_str(to), m.files.iter().take(64).map(|f| json_str(f)).collect::<Vec<_>>().join(","));
                    self.event(Sev::Info, line.clone(), detail);
                    moved_lines.push(format!("{{\"repo\":{},\"from\":{},\"to\":{},\"files\":{}}}", json_str(repo), from.as_deref().map_or("null".into(), json_str), json_str(to), m.files.len()));
                }
                Err(e) => return self.refuse(out, format!("sync: {repo} checkout refused: {e}")),
            }
            if *repo == "makepad" {
                revs.0 = Some(to.clone());
            } else {
                revs.1 = Some(to.clone());
            }
        }
        let mp = revs.0.clone().or_else(|| git::fleet_revision(&self.conf.checkout, "makepad"));
        let cm = revs.1.clone().or_else(|| git::fleet_revision(&self.conf.checkout, "commercial"));
        if let Some(mp) = &mp {
            git::write_fleet_revision(&self.conf.checkout, mp, cm.as_deref());
        }
        out.extend(moved_lines);

        // what the JITs' live patchers did with it: a coordinated patch answers with it (the
        // master reads the ticks), otherwise the records follow on the stream
        if ticker.is_some() {
            out.extend(self.follow_patches(watched, ticker, &session));
        } else if !watched.is_empty() {
            let me = self.clone();
            let _ = std::thread::Builder::new().name("devhub-sync-patches".into()).spawn(move || {
                me.follow_patches(watched, None, "");
            });
        }

        // prebuild
        let wm_bin = self.conf.checkout.join("target/release/makepad-app-wm");
        let wm_before = build::mtime(&wm_bin);
        let mut built_line = "null".to_string();
        if o.get("build") != Some("off") && self.conf.build != Build::Off {
            self.event(Sev::Info, format!("sync: building ({:?})", self.conf.build).to_lowercase(), String::new());
            let b = build::prebuild(&self.conf);
            let n = b.compiled.len();
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
            "auto" if wm_replaced => self.restart_or_refuse("the build replaced the WM binary", out),
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
        for j in stale.iter().filter(|j| restart != "apps" || super::launch::ledger(j.pid).is_none()) {
            self.jit_event(Sev::Warn, &j.app, j.pid, format!("sync: {}#{} runs a makepad binary the build replaced: restart it to run the new JIT", j.app, j.pid), String::new());
        }
        let summary = format!(
            "{{\"applied\":{{\"makepad\":{},\"commercial\":{},\"session\":{},\"wm_replaced\":{wm_replaced},\"stale_jits\":{},\"ms\":{}}}}}",
            revs.0.as_deref().map_or("null".into(), json_str),
            revs.1.as_deref().map_or("null".into(), json_str),
            json_str(&session),
            stale.len(),
            t0.elapsed().as_millis()
        );
        *self.last.lock().unwrap() = summary.clone();
        out.push(summary);
    }

    /// Follows each watched JIT's live-edit records for the edit this apply caused, up to 60 s
    /// (a coordinated patch also reads the session tick the edit landed at).
    fn follow_patches(&self, watched: Vec<(jits::Jit, PathBuf, u64)>, ticker: Option<(u32, jits::Ticker)>, session: &str) -> Vec<String> {
        let mut out = Vec::new();
        let end = Instant::now() + Duration::from_secs(60);
        let mut pending = watched;
        while !pending.is_empty() && Instant::now() < end {
            let mut still = Vec::new();
            for (j, log, from) in pending {
                let (edits, _) = jits::edits_since(&log, from);
                match edits.into_iter().rfind(|e| e.rung != "unchanged") {
                    Some(e) => {
                        let tick = ticker.as_ref().filter(|(p, _)| *p == j.pid).and_then(|(_, t)| t.read().ok());
                        let at = tick.map_or(String::new(), |t| format!(" at session {session} tick {t}"));
                        let (sev, what) = match e.rung.as_str() {
                            "patch" | "partial" => (Sev::Info, format!("patched {} fns in {:.0} ms", e.fns, e.ms)),
                            "restart" => (Sev::Warn, format!("needs a restart: {}", e.reasons)),
                            "refuse" => (Sev::Warn, format!("refused the edit: {}", e.reasons)),
                            other => (Sev::Info, format!("{other} ({} files)", e.files)),
                        };
                        self.jit_event(sev, &j.app, j.pid, format!("sync: {}#{} {what}{at}", j.app, j.pid), e.raw.clone());
                        out.push(format!("{{\"jit\":{},\"pid\":{},\"rung\":{},\"fns\":{},\"ms\":{},\"tick\":{}}}", json_str(&j.app), j.pid, json_str(&e.rung), e.fns, e.ms, tick.map_or("null".into(), |t| t.to_string())));
                    }
                    None if makepad_remote::relay::proc_start(j.pid).is_none() => {}
                    None => still.push((j, log, from)),
                }
            }
            pending = still;
            if !pending.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        for (j, _, _) in pending {
            out.push(format!("{{\"jit\":{},\"pid\":{},\"rung\":\"none\"}}", json_str(&j.app), j.pid));
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
    /// (`makepad_app_registry::machine::unsaved_work`, what apps publish) and while a gate lease
    /// runs here (a landing's gate must not see its node go away). The wait is reported once; one
    /// waiter per node, which restarts the moment the node is clear.
    fn wm_restart(self: &Arc<Self>, why: &str) -> Result<String, String> {
        let blockers = restart_blockers();
        if blockers.is_empty() {
            return self.restart_wm_now(why);
        }
        if !crate::lease::exec::writes_allowed() {
            return Err(format!("{} is not available for dev: WM restarts need its dev switch on", self.ctx.machine));
        }
        let list = blockers.join("; ");
        if WM_RESTART_WAITING.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Ok(format!("{{\"wm_restart\":\"waiting\",\"for\":{}}}", json_str(&list)));
        }
        self.event(Sev::Warn, format!("sync: the WM restart ({why}) waits for: {list}"), String::new());
        let me = self.clone();
        let why = why.to_string();
        let spawned = std::thread::Builder::new().name("devhub-wm-restart-wait".into()).spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                if restart_blockers().is_empty() {
                    break;
                }
            }
            let _busy = me.busy.lock().unwrap();
            if let Err(e) = me.restart_wm_now(&format!("{why}, after the wait")) {
                me.event(Sev::Err, format!("sync: WM restart refused: {e}"), String::new());
            }
            WM_RESTART_WAITING.store(false, std::sync::atomic::Ordering::Release);
        });
        if let Err(e) = spawned {
            WM_RESTART_WAITING.store(false, std::sync::atomic::Ordering::Release);
            return Err(format!("cannot wait for the restart: {e}"));
        }
        Ok(format!("{{\"wm_restart\":\"waiting\",\"for\":{}}}", json_str(&list)))
    }

    /// Restarts the WM (and with it the AI Hub it starts) through its systemd service, with the
    /// sudoers rule's exact command (no password prompt). The services the WM supervises (this
    /// hub, the AI Hub) run in scopes of their own and outlive it; the marker lets a hub that
    /// does restart report it.
    fn restart_wm_now(&self, why: &str) -> Result<String, String> {
        if !crate::lease::exec::writes_allowed() {
            return Err(format!("{} is not available for dev: WM restarts need its dev switch on", self.ctx.machine));
        }
        let svc = self.conf.wm_service.clone().ok_or("this node has no WM service (wm_service = none)")?;
        let _ = std::fs::create_dir_all(proto::home().join("sync"));
        let _ = std::fs::write(Self::marker(), format!("{} {why}\n", proto::wall_ms()));
        self.event(Sev::Info, format!("sync: restarting the WM ({svc}): {why}"), String::new());
        let st = makepad_remote::group::command("sudo").args(["-n", "/usr/bin/systemctl", "restart", &svc]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("sudo: {e}"))?;
        if !st.status.success() {
            let _ = std::fs::remove_file(Self::marker());
            return Err(format!("sudo -n systemctl restart {svc}: {}", proto::clean(String::from_utf8_lossy(&st.stderr).trim(), 300)));
        }
        let active = makepad_remote::group::command("systemctl").args(["is-active", &svc]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let _ = std::fs::remove_file(Self::marker());
        let line = format!("sync: WM restarted ({svc} {active})");
        self.event(if active == "active" { Sev::Info } else { Sev::Err }, line, String::new());
        Ok(format!("{{\"wm_restarted\":{},\"active\":{}}}", json_str(&svc), json_str(&active)))
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

/// A WM restart is waiting on this node (`Sync::wm_restart`).
static WM_RESTART_WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What a WM restart would cost now: the gate leases on this node (landings) and the unsaved
/// work of its apps. Empty: it may restart.
fn restart_blockers() -> Vec<String> {
    let mut why: Vec<String> = crate::lease::exec::held()
        .into_iter()
        .filter(|(_, lane, kind)| *kind == "gate" || lane.starts_with("land"))
        .map(|(id, lane, kind)| format!("{kind} lease {id} of {lane}"))
        .collect();
    #[cfg(unix)]
    why.extend(makepad_app_registry::machine::unsaved_work().into_iter().map(|(pid, app, what)| format!("unsaved work in {app} (pid {pid}): {what}")));
    why
}

/// A makepad checkout dir that holds only the nested commercial checkout (a fresh tree whose
/// commercial side came first).
fn only_commercial(dir: &std::path::Path) -> bool {
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
