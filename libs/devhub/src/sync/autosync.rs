//! The master follows work and commercial main so dock apps receive landed changes without
//! a manual push, and moves a node's one tree back to them after a lease moved it elsewhere.
//! Each round attempts one node, passing over nodes that hold a lease until it ends (one box per
//! lease: the node refuses a sync while a lease holds its tree). The node still refuses older
//! trees and owns its build and safe restart. Settings are reread from autosync.conf;
//! autosync.state keeps the last successful revisions across master restarts. Repeated skips are
//! reported once, to the master's stderr.
//!
//! A landing kicks the watcher (`makepad devhub sync now`, the landing scripts' hook): the next
//! round starts at once and reaches every node that needs the new tips, the first listed node
//! (the user's WM node) alone first, then the rest together. The order carries the landing's
//! time, so the node can say how long landing -> visible took.
use super::cli::{ask_local, branch_sha, fleet_nodes, push_and_apply};
use super::git::valid_sha;
use crate::proto;
use makepad_remote::link::{Client, Target};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

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

/// What one node's apply answered: its applied revisions, or why not.
fn apply_result(code: i32, lines: &[String]) -> Result<(String, String), String> {
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
            if let (Some(mp), Some(cm)) = (a.get("makepad").and_then(|x| x.as_str()), a.get("commercial").and_then(|x| x.as_str())) {
                if valid_sha(mp) && valid_sha(cm) {
                    success = Some((mp.to_string(), cm.to_string()));
                }
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

/// A `nodes` line of a node that should pre-warm after its apply: up, dev on, no lease, not
/// draining, under its load limits.
pub fn prewarm_suits(v: &makepad_strict_json::Value) -> bool {
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let leases = v.get("leases").and_then(|x| x.as_u64()).unwrap_or(1);
    let caps = v.get("caps").filter(|c| !matches!(c, makepad_strict_json::Value::Null)).and_then(|c| crate::lease::Caps::parse(&c.to_json())).filter(|c| c.cores > 0);
    b("up") && b("dev") && !b("draining") && leases == 0 && caps.is_some_and(|c| !crate::lease::sched::overloaded(&c))
}

pub fn start(control_socket: PathBuf) {
    if let Err(e) = std::thread::Builder::new().name("devhub-autosync".into()).spawn(move || {
        let c = Client::new(Target::Unix(control_socket), "devhub-autosync");
        let home = proto::home();
        let path = home.join("autosync.state");
        let mut applied = BTreeMap::<String, (String, String)>::new();
        match std::fs::read_to_string(&path) {
            Ok(text) => for line in text.lines() {
                let words: Vec<_> = line.split_whitespace().collect();
                if let [node, mp, cm] = words.as_slice() {
                    if valid_sha(mp) && valid_sha(cm) {
                        applied.insert(node.to_string(), (mp.to_string(), cm.to_string()));
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("devhub autosync: {}: {e}", path.display()),
        }
        let mut dirty = false;
        let mut last_node = None;
        let mut summaries = BTreeMap::<String, String>::new();
        let mut report = |node: &str, line: String| {
            if summaries.get(node) != Some(&line) {
                eprintln!("devhub autosync: {node} {line}");
                summaries.insert(node.to_string(), line);
            }
        };
        let mut interval_s = 30;
        let mut landed: Option<u64> = None;
        loop {
            let round = (|| -> Result<(), String> {
                let text = match std::fs::read_to_string(home.join("autosync.conf")) {
                    Ok(text) => text,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(e) => return Err(format!("autosync.conf: {e}")),
                };
                let conf = AutoConf::parse(&text, &makepad_app_registry::repo_root().unwrap_or_default());
                interval_s = conf.interval_s;
                if !conf.enabled {
                    return Ok(());
                }
                if conf.repo.as_os_str().is_empty() {
                    return Err("no checkout found; set repo in autosync.conf".into());
                }
                let shas = (branch_sha(&conf.repo, "work")?, branch_sha(&conf.repo.join("apps/commercial"), "main")?);
                let nodes = match conf.nodes {
                    Some(nodes) => nodes,
                    None => fleet_nodes(&c)?,
                };
                if nodes.is_empty() {
                    return Ok(());
                }
                // what each node's one tree holds now (a lease moves it to its own commits)
                let mut trees = BTreeMap::<String, String>::new();
                // the nodes a pre-warm suits: dev on, under their load limits
                let mut idle = BTreeMap::<String, bool>::new();
                for line in ask_local(&c, "nodes", "nodes").unwrap_or_default() {
                    let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
                    if let Some(n) = v.get("node").and_then(|x| x.as_str()) {
                        idle.insert(n.to_string(), prewarm_suits(&v));
                    }
                    if let (Some(n), Some(t)) = (v.get("node").and_then(|x| x.as_str()), v.get("caps").and_then(|c| c.get("tree")).and_then(|x| x.as_str())) {
                        trees.insert(n.to_string(), t.to_string());
                    }
                }
                // the nodes a lease holds: passed over until it ends
                let mut held = BTreeMap::<String, String>::new();
                for line in ask_local(&c, "leases", "leases all")? {
                    let v = makepad_strict_json::parse(line.as_bytes()).map_err(|_| "leases: bad answer")?;
                    if v.get("err").is_some() {
                        return Err(format!("leases: {line}"));
                    }
                    // every state but these holds the node (granted, starting, setting up)
                    let state = v.get("state").and_then(|x| x.as_str()).unwrap_or("");
                    if matches!(state, "queued" | "ended") {
                        continue;
                    }
                    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let running = v.get("info").and_then(|i| i.get("running")).and_then(|x| x.as_str()).unwrap_or("");
                    let (kind, lane) = (field("kind"), field("lane"));
                    // one box per lease: a node runs its lease on its tree undisturbed
                    if kind != "inspect" {
                        held.insert(field("node"), format!("skip: {kind} lease held by {lane}{}", if running.is_empty() { String::new() } else { format!(" ({running})") }));
                    }
                }
                // which nodes this round reaches, in batches pushed together
                let mut todo = Vec::new();
                for node in &nodes {
                    let moved_off = trees.get(node).is_some_and(|t| !t.is_empty() && *t != format!("{},{}", shas.0, shas.1));
                    if applied.get(node) == Some(&shas) && !moved_off {
                        continue;
                    }
                    match held.get(node) {
                        Some(why) => report(node, why.clone()),
                        None => todo.push(node.clone()),
                    }
                }
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
                let extra = landed.map_or(String::new(), |ms| format!(" landed={ms}"));
                for batch in batches {
                    last_node = batch.last().cloned();
                    let prewarm = batch.iter().all(|n| suits(n));
                    let mut lines = BTreeMap::<String, Vec<String>>::new();
                    let code = push_and_apply(&c, &conf.repo, "work", Some("main"), &batch, "on", "auto", prewarm, &extra, &mut |n, l| lines.entry(n.to_string()).or_default().push(l.to_string()));
                    for node in &batch {
                        let mine = lines.remove(node).unwrap_or_default();
                        // a batch's exit code is every node's: one node's is its own error lines
                        let code = if batch.len() == 1 { code } else { mine.iter().any(|l| l.starts_with("{\"err\":")) as i32 };
                        match apply_result(code, &mine) {
                            Ok(revs) => {
                                let after = landed.map_or(String::new(), |ms| format!(" ({} ms after the landing)", proto::wall_ms().saturating_sub(ms)));
                                report(node, format!("applied work={} commercial={}{after}", revs.0, revs.1));
                                applied.insert(node.clone(), revs);
                                dirty = true;
                            }
                            Err(e) => report(node, format!("skip work={} commercial={}: {e}", shas.0, shas.1)),
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = round {
                report("", e);
            }
            // Retry a failed save even when no further node needs an apply.
            if dirty {
                let text: String = applied.iter().map(|(n, (mp, cm))| format!("{n} {mp} {cm}\n")).collect();
                let tmp = home.join("autosync.state.tmp");
                match std::fs::create_dir_all(&home).and_then(|_| std::fs::write(&tmp, text)).and_then(|_| std::fs::rename(&tmp, &path)) {
                    Ok(()) => dirty = false,
                    Err(e) => report("state", format!("could not save autosync.state: {e}")),
                }
            }
            landed = wait_kick(Duration::from_secs(interval_s));
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
}
