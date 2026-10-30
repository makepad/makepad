//! The master follows work and commercial main so dock apps receive landed changes without
//! a manual push. Each round attempts one node, passing over gate and landing leases until
//! they end. The node still refuses older trees and owns its build and safe restart.
//! Settings are reread from autosync.conf; autosync.state keeps the last successful revisions
//! across master restarts. Repeated skips are reported once, to the master's stderr.

use super::cli::{ask_local, branch_sha, fleet_nodes, push_and_apply};
use super::git::valid_sha;
use crate::proto;
use makepad_remote::link::{Client, Target};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct AutoConf {
    pub enabled: bool,
    pub repo: PathBuf,
    pub interval_s: u64,
    pub nodes: Option<Vec<String>>,
}

impl AutoConf {
    pub fn parse(text: &str, default_repo: &Path) -> Self {
        let mut c = Self { enabled: true, repo: default_repo.to_path_buf(), interval_s: 30, nodes: None };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "enabled" if v == "on" => c.enabled = true,
                "enabled" if v == "off" => c.enabled = false,
                "repo" if !v.is_empty() => c.repo = PathBuf::from(v),
                "interval" => c.interval_s = v.parse().ok().filter(|s| *s > 0).unwrap_or(c.interval_s),
                "nodes" => c.nodes = Some(v.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect()),
                _ => {}
            }
        }
        c
    }
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
        loop {
            let mut interval_s = 30;
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
                // Rotate after every attempt, including a refusal or an unreachable node.
                let first = last_node.as_ref().and_then(|last| nodes.iter().position(|n| n == last)).map_or(0, |i| (i + 1) % nodes.len());
                for i in 0..nodes.len() {
                    let node = &nodes[(first + i) % nodes.len()];
                    if applied.get(node) == Some(&shas) {
                        continue;
                    }
                    let mut held = None;
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
                        // a landing's gate: a gate lease, a landing lane, or a lease that runs the land gate
                        if field("node") == *node && (kind == "gate" || lane.starts_with("land") || running.contains("land_gate")) {
                            held = Some(format!("skip: {kind} lease held by {lane}"));
                        }
                    }
                    if let Some(why) = held {
                        report(node, why);
                        continue;
                    }
                    last_node = Some(node.clone());
                    let mut lines = Vec::new();
                    let code = push_and_apply(&c, &conf.repo, "work", Some("main"), std::slice::from_ref(node), "on", "auto", &mut |_, l| lines.push(l.to_string()));
                    let mut success = None;
                    let mut error = None;
                    for line in &lines {
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
                        (0, None, Some(revs)) => {
                            report(node, format!("applied work={} commercial={}", revs.0, revs.1));
                            applied.insert(node.clone(), revs);
                            dirty = true;
                        }
                        (_, error, _) => report(node, format!("skip work={} commercial={}: {}", shas.0, shas.1, error.unwrap_or_else(|| format!("no successful apply (exit {code})")))),
                    }
                    break;
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
            std::thread::sleep(Duration::from_secs(interval_s));
        }
    }) {
        eprintln!("devhub autosync: cannot start watcher: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
