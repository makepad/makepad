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
//! something it landed. `close <app>` ends what its `native` form opened, for `show stop`.
//!
//! A launch is a run of its lease ([`crate::lease::exec::launch`]): under the run wrapper, in its
//! own session and scope, so its exit is the lease's to tell, through a hub restart too.
//!
//! Every launch is in the ledger (`~/.makepad/devhub/sync/launched/<pid>.json`: the app's pid, its
//! start time and its run's group); `stop` signals only for a ledger pid whose start time still
//! matches (never a pattern), its run's whole process group, SIGTERM then SIGKILL.

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

fn spec_json(pid: u32, start: u64, group: u32, s: &Spec, log: &std::path::Path) -> String {
    let env: Vec<String> = s.env.iter().map(|(k, v)| json_str(&format!("{k}={v}"))).collect();
    let args: Vec<String> = s.args.iter().map(|a| json_str(a)).collect();
    format!(
        "{{\"pid\":{pid},\"start\":{start},\"group\":{group},\"app\":{},\"visible\":{},\"lan\":{},\"env\":[{}],\"args\":[{}],\"log\":{},\"lease\":{}}}",
        json_str(&s.app),
        s.visible,
        s.lan,
        env.join(","),
        args.join(","),
        json_str(&log.display().to_string()),
        s.lease.as_deref().map_or("null".to_string(), json_str)
    )
}

/// A ledger entry whose process still runs (same pid, same start time): its launch, start time,
/// log and the process group its run leads.
pub fn ledger(pid: u32) -> Option<(Spec, u64, PathBuf, u32)> {
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
    let group = v.get("group")?.as_u64()? as u32;
    Some((spec, start, PathBuf::from(v.get("log").and_then(|l| l.as_str()).unwrap_or("")), group))
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

/// A launch's environment is built, not inherited: the hub's own variables pass only when they
/// are on a lease's allowlist (`base_env` of a screen lease: user, locale, toolchain, display,
/// WM); what the registry set on the command (cargo, machine settings) and the launch's own
/// words come on top. Whatever the hub was started with (a test switch exported in a shell that
/// restarted it) never reaches an app.
fn allowlisted(cmd: &mut std::process::Command) {
    let set: std::collections::HashSet<std::ffi::OsString> = cmd.get_envs().map(|(k, _)| k.to_owned()).collect();
    let keep: std::collections::HashSet<String> = crate::lease::exec::base_env(crate::lease::Kind::Screen, std::env::vars()).into_iter().map(|(k, _)| k).collect();
    for (k, _) in std::env::vars_os() {
        if !set.contains(&k) && !k.to_str().is_some_and(|k| keep.contains(k)) {
            cmd.env_remove(&k);
        }
    }
}

/// Starts `s` from the checkout as a run of lease `lease`; returns the app's pid (the JIT's) and its log.
pub fn start(conf: &SyncConf, s: &Spec, lease: &str, publish: &crate::local::Publish, machine: &str) -> Result<(u32, PathBuf), String> {
    let app = find_app(&s.app).ok_or_else(|| format!("no app `{}` in the registry (a registry name like `calculator` or a package like `makepad-app-calculator`)", s.app))?;
    let _ = std::fs::create_dir_all(ledger_dir());
    let log = ledger_dir().join(format!("{}-{}.log", s.app, proto::wall_ms()));
    let (visible, lan, env) = (s.visible, s.lan, s.env.clone());
    let lane = crate::lease::exec::lease_lane(lease).unwrap_or_else(|| format!("lease-{lease}"));
    // what it ended with, said once its run ends ([`crate::lease::exec::launch`] waits for it)
    let (publish_end, machine_end, app_id) = (publish.clone(), machine.to_string(), s.app.clone());
    let mut ended: Option<Box<dyn FnOnce(u32, i32) + Send>> = Some(Box::new(move |pid: u32, code: i32| {
        let _ = std::fs::remove_file(ledger_file(pid));
        publish_end(hub_record(&machine_end, &app_id, pid, "launch", Sev::Info, format!("launch: {app_id}#{pid} ended (exit {code})"), String::new()));
    }));
    let (_launch, run) = AppLaunch::start(app, Runner::Jit, Some(conf.checkout.clone()), s.args.clone(), BuildPolicy::IfMissing, false, |mut cmd, _kind| {
        allowlisted(&mut cmd);
        if visible {
            // as the WM's spawn_jit: the window becomes the WM's tile through its socket
            cmd.env("MAKEPAD_WM_CAPTION_CONTENT", "1");
        } else {
            cmd.env("MAKEPAD_HIDE_WINDOWS", "1").env("MAKEPAD_MUTE", "1");
        }
        cmd.env("MAKEPAD_TEST_LANE", &lane);
        cmd.env("MAKEPAD_LAN", if lan { "1" } else { "0" });
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        crate::lease::exec::launch(lease, &cmd, &log, ended.take().ok_or("spawned twice")?)
    })?;
    // (a run that ended already leaves an entry `ledger` drops: its start no longer matches)
    let start = makepad_remote::relay::proc_start(run.app).unwrap_or(0);
    let _ = makepad_network::tls::write_private(&ledger_file(run.app), spec_json(run.app, start, run.leader, s, &log).as_bytes());
    Ok((run.app, log))
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

/// Stops a ledger pid's launch: its run's process group, on purpose (its lease's end says no
/// abnormal end for it); Err when the pid is not one of ours.
pub fn stop(pid: u32) -> Result<String, String> {
    let (_, start, _, group) = ledger(pid).ok_or_else(|| format!("pid {pid} is not a running launch of this hub"))?;
    crate::lease::exec::launch_stopped(group);
    makepad_remote::group::signal(group, false);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if makepad_remote::relay::proc_start(pid) != Some(start) {
            let _ = std::fs::remove_file(ledger_file(pid));
            return Ok("SIGTERM".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if makepad_remote::relay::proc_start(pid) == Some(start) {
        makepad_remote::group::signal(group, true);
    }
    let _ = std::fs::remove_file(ledger_file(pid));
    Ok("SIGKILL after 10 s".into())
}

/// The pids of `app`'s compiled instances here that no lease started: what `show <app> native`
/// asked the WM to open.
fn compiled_open(apps: &[makepad_remote::discovery::App], app: &str) -> Vec<u32> {
    apps.iter().filter(|a| a.app == app && a.text("run").is_none() && a.text("lease").unwrap_or_default().is_empty()).map(|a| a.pid).collect()
}

/// Closes what `show <app> native` opened here, SIGTERM then SIGKILL as [`stop`] does, and says how
/// many. It is the WM's child, no launch of this hub and no lease's, so nothing else ends it, and
/// one left open on a box nobody sits at holds that box's WM restart for good (11-09).
fn close_compiled(app: &str) -> usize {
    let open = compiled_open(&makepad_remote::discovery::apps(), app);
    crate::lease::exec::note_stopped_pids("by show stop", open.clone());
    let started: Vec<(u32, Option<u64>)> = open.iter().map(|&pid| (pid, makepad_remote::relay::proc_start(pid))).collect();
    for &pid in &open {
        crate::os::signal_one(pid, false);
    }
    let runs = |&(pid, start): &(u32, Option<u64>)| start.is_some() && makepad_remote::relay::proc_start(pid) == start;
    let end = Instant::now() + Duration::from_secs(10);
    while started.iter().any(runs) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    for s in started.iter().filter(|s| runs(s)) {
        crate::os::signal_one(s.0, true);
    }
    open.len()
}

/// The `launch` service: `launch …`, `stop <pid>`, `relaunch <pid>`, `close <app>`, `list`, `log <pid> [n]`.
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
                let Some((spec, _, _, _)) = pid.parse().ok().and_then(ledger) else { return err(out, &format!("pid {pid} is not a running launch of this hub")) };
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
            // what `show <app> native` opened: a person's to close if one is here
            ["close", app] => {
                if !guard(out) {
                    return;
                }
                if crate::lease::exec::user_present() {
                    return err(out, &format!("a person uses {machine}: what its WM opened is theirs to close"));
                }
                let closed = close_compiled(app);
                if closed > 0 {
                    publish(hub_record(&machine, app, 0, "launch", Sev::Info, format!("launch: closed the dock's compiled {app}"), String::new()));
                }
                out.push(format!("{{\"closed\":{closed}}}"));
            }
            // the user's own dock app on their display (launch-or-focus): no dev work, so a node
            // whose dev switch is off (the user's display node) shows it too
            ["show", app, how @ ..] if matches!(how, [] | ["native"]) => {
                let native = !how.is_empty();
                match show(app, native) {
                    Ok(line) => {
                        publish(hub_record(&machine, app, 0, "launch", Sev::Info, format!("launch: showed the dock's {app}{}", if native { ", compiled" } else { "" }), String::new()));
                        out.push(line);
                    }
                    Err(e) => err(out, &e),
                }
            }
            ["list"] => {
                for pid in launched() {
                    if let Some((s, _, log, _)) = ledger(pid) {
                        out.push(format!("{{\"pid\":{pid},\"id\":{},\"app\":{},\"visible\":{},\"jit\":{},\"log\":{}}}", json_str(&format!("{machine}/{}#{pid}", s.app)), json_str(&s.app), s.visible, jits::announced(pid).is_some(), json_str(&log.display().to_string())));
                    }
                }
            }
            ["log", pid, rest @ ..] => {
                let n = rest.first().and_then(|n| n.parse::<usize>().ok()).unwrap_or(40).min(400);
                let Some((_, _, log, _)) = pid.parse().ok().and_then(ledger) else { return err(out, &format!("pid {pid} is not a running launch of this hub")) };
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                for l in &lines[lines.len().saturating_sub(n)..] {
                    out.push(format!("{{\"log\":{}}}", json_str(&proto::clean(l, 400))));
                }
            }
            _ => err(out, "launch [visible|hidden] [lan] [K=V ...] <app> [args...] | show <app> [native] | stop <pid> | relaunch <pid> | list | log <pid> [n]"),
        }
    })
}

/// Asks this node's WM for its dock app `app` (launch-or-focus): the user's one instance of it,
/// started the dock's way when it is not running (`makepad_hal::wm_socket::request_launch`).
/// `native`: its compiled build, whatever the WM runs it under otherwise.
pub fn show(app: &str, native: bool) -> Result<String, String> {
    let entry = find_app(app).ok_or_else(|| format!("no app `{app}` in the registry"))?;
    #[cfg(target_os = "linux")]
    {
        let path = makepad_app_registry::machine::wm_socket().ok_or("no WM runs on this node (no makepad-wm socket)")?;
        makepad_app_registry::machine::request_launch(&path, entry.id, native)?;
        Ok(format!("{{\"shown\":{}}}", json_str(entry.id)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (entry, native);
        Err("show: only a Linux node's WM has a dock to ask".into())
    }
}

fn launch_reply(conf: &SyncConf, spec: &Spec, publish: &crate::local::Publish, machine: &str, out: &mut Vec<String>) {
    let Some(lease) = spec.lease.as_deref() else { return out.push(format!("{{\"err\":{}}}", json_str("launch: needs a write lease"))) };
    match start(conf, spec, lease, publish, machine) {
        Ok((pid, log)) => {
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

    /// `show stop` closes the compiled app the WM opened, never a JIT of that name or a lease's.
    #[test]
    fn only_a_compiled_app_no_lease_started_is_closed() {
        let app = |pid: u32, name: &str, fields: &[(&str, &str)]| makepad_remote::discovery::App {
            pid,
            app: name.into(),
            socket: String::new(),
            started_ms: 0,
            caps: Vec::new(),
            restart_parent: None,
            fields: fields.iter().map(|(k, v)| (k.to_string(), format!("\"{v}\""))).collect(),
        };
        let apps = [
            app(1, "clock", &[]),
            app(2, "clock", &[("run", "r1")]),
            app(3, "clock", &[("lease", "L1")]),
            app(4, "other", &[]),
        ];
        assert_eq!(compiled_open(&apps, "clock"), vec![1]);
        assert_eq!(compiled_open(&apps, "none"), Vec::<u32>::new());
    }

    #[test]
    fn specs_and_env_policy() {
        let s = parse(&["visible", "lan", "FOO_HOST=1", "FOO_BIND_IP=192.0.2.89", "foo", "--x"]).unwrap();
        assert!(s.visible && s.lan);
        assert_eq!(s.env, vec![("FOO_HOST".to_string(), "1".to_string()), ("FOO_BIND_IP".to_string(), "192.0.2.89".to_string())]);
        assert_eq!((s.app.as_str(), s.args.clone()), ("foo", vec!["--x".to_string()]));
        for bad in ["LD_PRELOAD=x", "PATH=/tmp", "CARGO_HOME=/x", "RUSTFLAGS=-x", "MAKEPAD_HIDE_WINDOWS=0"] {
            assert!(parse(&[bad, "foo"]).is_err(), "{bad}");
        }
        assert!(parse(&["hidden"]).is_err());
    }
}
