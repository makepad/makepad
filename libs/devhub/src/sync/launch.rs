//! Remote launch: the master starts an app under `makepad run` on this node, as a window the WM
//! hosts (`visible`) or hidden and muted, and stops it by its JIT id. A launch is the WM's own
//! (`AppLaunch::start`, Runner::Jit: `cargo run --profile host -p makepad-run --bin makepad --
//! run <package>` in the synced checkout, with the registry's environment), so after a sync's
//! prebuild it compiles nothing. `cargo run` execs the tool in its place: the pid it starts is
//! the JIT's. It runs under a lane's lease, so it is that lane's TEST instance
//! (`MAKEPAD_TEST_LANE`): badged in its caption, never the WM dock's app, off the LAN unless the
//! launch says `lan`.
//!
//! `show <app>` is not a launch: it asks the node's WM for its dock app (launch-or-focus, the
//! registry's one launch definition, no lease, env or args), what a lane does to show the user
//! something it landed.
//!
//! Every launch is in the ledger (`~/.makepad/devhub/sync/launched/<pid>.json`, with the process
//! start time); `stop` signals only a ledger pid whose start time still matches (never a pattern),
//! its whole process group, SIGTERM then SIGKILL.

use super::conf::SyncConf;
use super::jits;
use crate::proto::{self, hub_record, Sev};
use makepad_app_registry::{AppLaunch, BuildPolicy, Runner};
use makepad_remote::json_str;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub app: String,
    pub visible: bool,
    pub lan: bool,
    pub env: Vec<(String, String)>,
    pub args: Vec<String>,
    /// the lease it runs under (`lease=<id>`): the lease's end stops it
    pub lease: Option<String>,
}

/// An env word the master may set on a launch: upper-case names only, none that change how the
/// process or its tools load code.
pub fn allowed_env(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.bytes().next().is_some_and(|b| b.is_ascii_uppercase())
        && k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !k.starts_with("LD_")
        && !k.starts_with("CARGO")
        && !k.starts_with("RUST")
        && !k.starts_with("GIT_")
        && !matches!(k, "PATH" | "HOME" | "SHELL" | "MAKEPAD" | "MAKEPAD_HIDE_WINDOWS" | "XDG_RUNTIME_DIR" | "TMPDIR")
}

/// `[visible|hidden] [lan] [K=V ...] <app> [args...]`
pub fn parse(words: &[&str]) -> Result<Spec, String> {
    let mut s = Spec { app: String::new(), visible: false, lan: false, env: Vec::new(), args: Vec::new(), lease: None };
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "visible" => s.visible = true,
            "hidden" => s.visible = false,
            "lan" => s.lan = true,
            w if w.starts_with("lease=") => s.lease = Some(w["lease=".len()..].to_string()),
            w if w.contains('=') && w.split('=').next().is_some_and(|k| k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')) => {
                let (k, v) = w.split_once('=').unwrap();
                if !allowed_env(k) {
                    return Err(format!("launch: {k} may not be set from the network"));
                }
                s.env.push((k.to_string(), v.to_string()));
            }
            _ => break,
        }
        i += 1;
    }
    let Some(app) = words.get(i) else { return Err("launch [visible|hidden] [lan] [K=V ...] <app> [args...]".into()) };
    s.app = app.to_string();
    s.args = words[i + 1..].iter().map(|w| w.to_string()).collect();
    Ok(s)
}

fn ledger_dir() -> PathBuf {
    proto::home().join("sync").join("launched")
}

fn ledger_file(pid: u32) -> PathBuf {
    ledger_dir().join(format!("{pid}.json"))
}

fn spec_json(pid: u32, start: u64, s: &Spec, log: &std::path::Path) -> String {
    let env: Vec<String> = s.env.iter().map(|(k, v)| json_str(&format!("{k}={v}"))).collect();
    let args: Vec<String> = s.args.iter().map(|a| json_str(a)).collect();
    format!(
        "{{\"pid\":{pid},\"start\":{start},\"app\":{},\"visible\":{},\"lan\":{},\"env\":[{}],\"args\":[{}],\"log\":{},\"lease\":{}}}",
        json_str(&s.app),
        s.visible,
        s.lan,
        env.join(","),
        args.join(","),
        json_str(&log.display().to_string()),
        s.lease.as_deref().map_or("null".to_string(), json_str)
    )
}

/// A ledger entry whose process still runs (same pid, same start time).
pub fn ledger(pid: u32) -> Option<(Spec, u64, PathBuf)> {
    let text = std::fs::read_to_string(ledger_file(pid)).ok()?;
    let v = makepad_strict_json::parse(text.as_bytes()).ok()?;
    let start = v.get("start")?.as_u64()?;
    if makepad_remote::relay::proc_start(pid) != Some(start) {
        let _ = std::fs::remove_file(ledger_file(pid));
        return None;
    }
    let strs = |k: &str| -> Vec<String> { v.get(k).and_then(|a| a.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
    let spec = Spec {
        app: v.get("app")?.as_str()?.to_string(),
        visible: v.get("visible").and_then(|b| b.as_bool()).unwrap_or(false),
        lan: v.get("lan").and_then(|b| b.as_bool()).unwrap_or(false),
        env: strs("env").into_iter().filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect(),
        args: strs("args"),
        lease: v.get("lease").and_then(|l| l.as_str()).map(str::to_string),
    };
    Some((spec, start, PathBuf::from(v.get("log").and_then(|l| l.as_str()).unwrap_or(""))))
}

pub fn launched() -> Vec<u32> {
    let mut out: Vec<u32> = std::fs::read_dir(ledger_dir())
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".json")?.parse().ok()).collect())
        .unwrap_or_default();
    out.retain(|pid| ledger(*pid).is_some());
    out.sort();
    out
}

/// An app by its registry name (`calculator`), its package (`makepad-app-calculator`) or its
/// binary.
pub fn find_app(name: &str) -> Option<&'static makepad_app_registry::AppEntry> {
    makepad_app_registry::find(name)
        .or_else(|| makepad_app_registry::apps().iter().find(|a| a.package == name))
        .or_else(|| makepad_app_registry::find_by_bin(name))
}

/// Starts `s` from the checkout; returns the pid (the JIT's) and its log.
pub fn start(conf: &SyncConf, s: &Spec, publish: &crate::local::Publish, machine: &str) -> Result<(u32, PathBuf), String> {
    let app = find_app(&s.app).ok_or_else(|| format!("no app `{}` in the registry (a registry name like `calculator` or a package like `makepad-app-calculator`)", s.app))?;
    let _ = std::fs::create_dir_all(ledger_dir());
    let log = ledger_dir().join(format!("{}-{}.log", s.app, proto::wall_ms()));
    let file = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let file2 = file.try_clone().map_err(|e| e.to_string())?;
    let (visible, lan, env) = (s.visible, s.lan, s.env.clone());
    let lane = s.lease.as_deref().map(|id| crate::lease::exec::lease_lane(id).unwrap_or_else(|| format!("lease-{id}")));
    let mut files = Some((file, file2));
    let (_launch, mut child) = AppLaunch::start(app, Runner::Jit, Some(conf.checkout.clone()), s.args.clone(), BuildPolicy::IfMissing, false, move |mut cmd, _kind| {
        if visible {
            // as the WM's spawn_jit: the window becomes the WM's tile through its socket
            cmd.env("MAKEPAD_WM_CAPTION_CONTENT", "1");
        } else {
            cmd.env("MAKEPAD_HIDE_WINDOWS", "1").env("SANDBOX_MUTE", "1");
        }
        if let Some(lane) = &lane {
            cmd.env("MAKEPAD_TEST_LANE", lane);
        }
        cmd.env("MAKEPAD_LAN", if lan { "1" } else { "0" });
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let (out, err) = files.take().ok_or("spawned twice")?;
        cmd.stdin(std::process::Stdio::null()).stdout(out).stderr(err);
        makepad_remote::group::detach(&mut cmd);
        cmd.spawn().map_err(|e| e.to_string())
    })?;
    let pid = child.id();
    let start = makepad_remote::relay::proc_start(pid).unwrap_or(0);
    let _ = makepad_network::tls::write_private(&ledger_file(pid), spec_json(pid, start, s, &log).as_bytes());
    let (publish, machine, app_id) = (publish.clone(), machine.to_string(), s.app.clone());
    let _ = std::thread::Builder::new().name("devhub-sync-reap".into()).spawn(move || {
        let st = child.wait().map_or_else(|e| format!("lost: {e}"), |s| s.to_string());
        let _ = std::fs::remove_file(ledger_file(pid));
        publish(hub_record(&machine, &app_id, pid, "launch", Sev::Info, format!("launch: {app_id}#{pid} ended ({st})"), String::new()));
    });
    Ok((pid, log))
}

/// Waits until `pid` announces its inspect endpoint (the JIT is up) or ends.
pub fn wait_announced(pid: u32, wait: Duration) -> Option<String> {
    let end = Instant::now() + wait;
    while Instant::now() < end {
        if let Some(app) = jits::announced(pid) {
            return Some(app);
        }
        if makepad_remote::relay::proc_start(pid).is_none() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Stops a ledger pid's process group; Err when the pid is not one of ours.
pub fn stop(pid: u32) -> Result<String, String> {
    let (_, start, _) = ledger(pid).ok_or_else(|| format!("pid {pid} is not a running launch of this hub"))?;
    makepad_remote::group::signal(pid, false);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if makepad_remote::relay::proc_start(pid) != Some(start) {
            let _ = std::fs::remove_file(ledger_file(pid));
            return Ok("SIGTERM".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if makepad_remote::relay::proc_start(pid) == Some(start) {
        makepad_remote::group::signal(pid, true);
    }
    let _ = std::fs::remove_file(ledger_file(pid));
    Ok("SIGKILL after 10 s".into())
}

/// The `launch` service: `launch …`, `stop <pid>`, `relaunch <pid>`, `list`, `log <pid> [n]`.
pub fn service(conf: Arc<SyncConf>, publish: crate::local::Publish, machine: String) -> Arc<makepad_remote::lines::Handler> {
    Arc::new(move |line: &str, out: &mut Vec<String>| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let err = |out: &mut Vec<String>, e: &str| out.push(format!("{{\"err\":{}}}", json_str(e)));
        // writes need the node's dev switch on (DESIGN.md §10.7; user WMs default off)
        let guard = |out: &mut Vec<String>| {
            if !crate::lease::exec::writes_allowed() {
                err(out, &format!("{machine} is not available for dev: launches and stops need its dev switch on"));
                return false;
            }
            true
        };
        // a launch runs under a write lease, whose end stops it
        let leased = |out: &mut Vec<String>, spec: &Spec| match spec.lease.as_deref() {
            Some(id) if crate::lease::exec::lease_live(id) => true,
            Some(id) => {
                err(out, &format!("launch: no live write lease {id} on {machine}"));
                false
            }
            None => {
                err(out, "launch: needs a write lease (makepad devhub launch <node> --lease <id> <app>; take one with `makepad devhub lease screen --node <node>`)");
                false
            }
        };
        match words.as_slice() {
            ["launch", rest @ ..] => {
                if !guard(out) {
                    return;
                }
                let spec = match parse(rest) {
                    Ok(s) => s,
                    Err(e) => return err(out, &e),
                };
                if !leased(out, &spec) {
                    return;
                }
                launch_reply(&conf, &spec, &publish, &machine, out);
            }
            ["relaunch", pid] => {
                if !guard(out) {
                    return;
                }
                let Some((spec, _, _)) = pid.parse().ok().and_then(ledger) else { return err(out, &format!("pid {pid} is not a running launch of this hub")) };
                if !leased(out, &spec) {
                    return;
                }
                if let Err(e) = stop(pid.parse().unwrap_or(0)) {
                    return err(out, &e);
                }
                launch_reply(&conf, &spec, &publish, &machine, out);
            }
            ["stop", pid] => {
                if !guard(out) {
                    return;
                }
                let Ok(pid) = pid.parse::<u32>() else { return err(out, "stop <pid>") };
                match stop(pid) {
                    Ok(how) => {
                        publish(hub_record(&machine, "", pid, "launch", Sev::Info, format!("launch: stopped #{pid} ({how})"), String::new()));
                        out.push(format!("{{\"stopped\":{pid},\"how\":{}}}", json_str(&how)));
                    }
                    Err(e) => err(out, &e),
                }
            }
            ["show", app] => {
                if !guard(out) {
                    return;
                }
                match show(app) {
                    Ok(line) => {
                        publish(hub_record(&machine, app, 0, "launch", Sev::Info, format!("launch: showed the dock's {app}"), String::new()));
                        out.push(line);
                    }
                    Err(e) => err(out, &e),
                }
            }
            ["list"] => {
                for pid in launched() {
                    if let Some((s, _, log)) = ledger(pid) {
                        out.push(format!("{{\"pid\":{pid},\"id\":{},\"app\":{},\"visible\":{},\"jit\":{},\"log\":{}}}", json_str(&format!("{machine}/{}#{pid}", s.app)), json_str(&s.app), s.visible, jits::announced(pid).is_some(), json_str(&log.display().to_string())));
                    }
                }
            }
            ["log", pid, rest @ ..] => {
                let n = rest.first().and_then(|n| n.parse::<usize>().ok()).unwrap_or(40).min(400);
                let Some((_, _, log)) = pid.parse().ok().and_then(ledger) else { return err(out, &format!("pid {pid} is not a running launch of this hub")) };
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                for l in &lines[lines.len().saturating_sub(n)..] {
                    out.push(format!("{{\"log\":{}}}", json_str(&proto::clean(l, 400))));
                }
            }
            _ => err(out, "launch [visible|hidden] [lan] [K=V ...] <app> [args...] | show <app> | stop <pid> | relaunch <pid> | list | log <pid> [n]"),
        }
    })
}

/// Asks this node's WM for its dock app `app` (launch-or-focus): the user's one instance of it,
/// started the dock's way when it is not running (`makepad_hal::wm_socket::request_launch`).
pub fn show(app: &str) -> Result<String, String> {
    let entry = find_app(app).ok_or_else(|| format!("no app `{app}` in the registry"))?;
    #[cfg(target_os = "linux")]
    {
        let path = makepad_app_registry::machine::wm_socket().ok_or("no WM runs on this node (no makepad-wm socket)")?;
        makepad_app_registry::machine::request_launch(&path, entry.id)?;
        Ok(format!("{{\"shown\":{}}}", json_str(entry.id)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = entry;
        Err("show: only a Linux node's WM has a dock to ask".into())
    }
}

fn launch_reply(conf: &SyncConf, spec: &Spec, publish: &crate::local::Publish, machine: &str, out: &mut Vec<String>) {
    match start(conf, spec, publish, machine) {
        Ok((pid, log)) => {
            if let Some(id) = &spec.lease {
                if let Err(e) = crate::lease::exec::adopt(id, pid, log.clone()) {
                    let _ = stop(pid);
                    return out.push(format!("{{\"err\":{}}}", json_str(&format!("launch: {e}"))));
                }
            }
            let jit = wait_announced(pid, Duration::from_secs(180));
            let id = format!("{machine}/{}#{pid}", spec.app);
            let mode = if spec.visible { "visible" } else { "hidden" };
            let line = match &jit {
                Some(_) => format!("launch: {} {mode} as {id}, JIT up", spec.app),
                None if makepad_remote::relay::proc_start(pid).is_none() => format!("launch: {} {mode} #{pid} ended before its JIT came up (log {})", spec.app, log.display()),
                None => format!("launch: {} {mode} #{pid} started; its JIT has not announced itself yet", spec.app),
            };
            publish(hub_record(machine, &spec.app, pid, "launch", if jit.is_some() { Sev::Info } else { Sev::Warn }, line, String::new()));
            out.push(format!("{{\"launched\":{pid},\"id\":{},\"app\":{},\"mode\":\"{mode}\",\"jit\":{},\"log\":{}}}", json_str(&id), json_str(&spec.app), jit.is_some(), json_str(&log.display().to_string())));
        }
        Err(e) => {
            publish(hub_record(machine, &spec.app, 0, "launch", Sev::Warn, format!("launch: {} failed: {e}", spec.app), String::new()));
            out.push(format!("{{\"err\":{}}}", json_str(&e)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_and_env_policy() {
        let s = parse(&["visible", "lan", "SANDBOX_HOST=1", "SANDBOX_BIND_IP=10.0.0.217", "sandbox", "--x"]).unwrap();
        assert!(s.visible && s.lan);
        assert_eq!(s.env, vec![("SANDBOX_HOST".to_string(), "1".to_string()), ("SANDBOX_BIND_IP".to_string(), "10.0.0.217".to_string())]);
        assert_eq!((s.app.as_str(), s.args.clone()), ("sandbox", vec!["--x".to_string()]));
        for bad in ["LD_PRELOAD=x", "PATH=/tmp", "CARGO_HOME=/x", "RUSTFLAGS=-x", "MAKEPAD_HIDE_WINDOWS=0"] {
            assert!(parse(&[bad, "sandbox"]).is_err(), "{bad}");
        }
        assert!(parse(&["hidden"]).is_err());
    }
}
