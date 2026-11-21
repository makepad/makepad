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
//! something it landed. `close <app>` ends what its `native` form opened, for `show stop`. A Mac
//! has no WM: its dock is the Runner's entries (`<Title>.app` in /Applications/Makepad or the
//! user's Applications/Makepad, `nl.makepad.<id>`), and `show` opens the app's entry as a click
//! in the Dock does; `dock` lists them with the tree and arguments their launch records run, and
//! whether the macOS Dock keeps them; `dock add [app ...]` puts those it does not keep in it (its
//! `persistent-apps`, through `defaults`, then launchd restarts the Dock to show them). `dock own`
//! points the Runner's own entry (`$RELAX_HOME/apps/runner.launch`, which its install script
//! writes for the release) at the synced tree's Runner: the one whose builds and pins the dock's
//! entries are (the release's Runner would build each pinned app again from its own sources).
//!
//! A launch is a run of its lease ([`crate::lease::exec::launch`]): under the run wrapper, in its
//! own session and scope, so its exit is the lease's to tell, through a hub restart too.
//!
//! A launch's JIT tells its first frame on glass by appending `present` to its report
//! (`RELAX_REPORT_TO`, an empty `<app>-<ms>.report` beside its log): `list` says how long after
//! the start that was (`present_ms`), the time a dock click takes to show the app. Its log stays
//! after it ends: `log <app>` reads the app's latest.
//!
//! Every launch is in the ledger (`~/.makepad/devhub/sync/launched/<pid>.json`: the app's pid, its
//! start time and its run's group); `stop` signals only for a ledger pid whose start time still
//! matches (never a pattern), its run's whole process group, SIGTERM then SIGKILL.

use super::conf::SyncConf;
use super::jits;
use crate::proto::{self, hub_record, Sev};
use makepad_app_registry::{AppLaunch, BuildPolicy, LaunchKind, Runner};
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
    /// `release`: as the Runner's releases run it (a Mac's Dock entries): standalone, the
    /// registry's features without the package's defaults ([`AppLaunch::start_release`]), so it
    /// compiles, and warms, what a Dock click runs. A Mac with no WM has no host for the default
    /// (hosted) launch there
    pub release: bool,
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
    let mut s = Spec { app: String::new(), visible: false, lan: false, env: Vec::new(), args: Vec::new(), lease: None, release: false };
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "visible" => s.visible = true,
            "hidden" => s.visible = false,
            "lan" => s.lan = true,
            "release" => s.release = true,
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
    let Some(app) = words.get(i) else { return Err("launch [visible|hidden] [lan] [release] [K=V ...] <app> [args...]".into()) };
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
        "{{\"pid\":{pid},\"start\":{start},\"group\":{group},\"app\":{},\"visible\":{},\"lan\":{},\"release\":{},\"env\":[{}],\"args\":[{}],\"log\":{},\"lease\":{}}}",
        json_str(&s.app),
        s.visible,
        s.lan,
        s.release,
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
        release: v.get("release").and_then(|b| b.as_bool()).unwrap_or(false),
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
    // the JIT appends to it (never creates it)
    let report = log.with_extension("report");
    std::fs::write(&report, b"").map_err(|e| format!("{}: {e}", report.display()))?;
    let (visible, lan, env) = (s.visible, s.lan, s.env.clone());
    let lane = crate::lease::exec::lease_lane(lease).unwrap_or_else(|| format!("lease-{lease}"));
    // what it ended with, said once its run ends ([`crate::lease::exec::launch`] waits for it)
    let (publish_end, machine_end, app_id) = (publish.clone(), machine.to_string(), s.app.clone());
    let mut ended: Option<Box<dyn FnOnce(u32, i32) + Send>> = Some(Box::new(move |pid: u32, code: i32| {
        let _ = std::fs::remove_file(ledger_file(pid));
        publish_end(hub_record(&machine_end, &app_id, pid, "launch", Sev::Info, format!("launch: {app_id}#{pid} ended (exit {code})"), String::new()));
    }));
    let spawn = |mut cmd: std::process::Command, _kind: LaunchKind| {
        allowlisted(&mut cmd);
        if visible {
            // as the WM's spawn_jit: the window becomes the WM's tile through its socket
            cmd.env("MAKEPAD_WM_CAPTION_CONTENT", "1");
        } else {
            cmd.env("MAKEPAD_HIDE_WINDOWS", "1").env("MAKEPAD_MUTE", "1");
        }
        cmd.env("MAKEPAD_TEST_LANE", &lane);
        cmd.env("RELAX_REPORT_TO", &report);
        cmd.env("MAKEPAD_LAN", if lan { "1" } else { "0" });
        cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        crate::lease::exec::launch(lease, &cmd, &log, ended.take().ok_or("spawned twice")?)
    };
    let (_launch, run) = if s.release {
        AppLaunch::start_release(app, Some(conf.checkout.clone()), s.args.clone(), spawn)?
    } else {
        AppLaunch::start(app, Runner::Jit, Some(conf.checkout.clone()), s.args.clone(), BuildPolicy::IfMissing, false, spawn)?
    };
    // (a run that ended already leaves an entry `ledger` drops: its start no longer matches)
    let start = makepad_remote::relay::proc_start(run.app).unwrap_or(0);
    let _ = makepad_network::tls::write_private(&ledger_file(run.app), spec_json(run.app, start, run.leader, s, &log).as_bytes());
    Ok((run.app, log))
}

/// Waits until `pid` announces its inspect endpoint (the JIT is up) or ends: the JIT's own pid.
pub fn wait_announced(pid: u32, wait: Duration) -> Option<u32> {
    let end = Instant::now() + wait;
    while Instant::now() < end {
        if let Some((_, jit)) = jits::announced_jit(pid) {
            return Some(jit);
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

/// Whether `pid` is a running instance of `app` here that no lease started (a JIT's run or the
/// compiled app, opened from the WM's dock or by a show through it): what `show --replace` may quit
/// ([`quit_instance`]). Names match as the registry knows the app (its id, package, binary or label).
fn instance_of(apps: &[makepad_remote::discovery::App], pid: u32, app: &str) -> bool {
    let entry = |name: &str| makepad_app_registry::apps().iter().position(|e| e.id == name || e.package == name || e.bin == name || e.label == name);
    let same = |name: &str| name == app || entry(name).is_some_and(|e| entry(app) == Some(e));
    apps.iter().any(|a| a.answers_to(pid) && same(&a.app) && a.text("lease").is_none())
}

/// Quits `app`'s running instance `pid`, which no launch of this hub started (the WM's dock opened
/// it, for a show or a person), as its person would: its remote's `quit`, which it refuses while
/// the person uses it, and never while it holds unsaved work. Then it waits for the app to end; one
/// that does not end is said, never killed. `show --replace` of a node's dock app failed on "not a
/// running launch of this hub", its show knowing the pid by the node's ps alone (11-21).
fn quit_instance(pid: u32, app: &str) -> Result<String, String> {
    if !instance_of(&makepad_remote::discovery::apps(), pid, app) {
        return Err(format!("pid {pid} is not a running launch of this hub, nor a running {app} no lease started"));
    }
    #[cfg(unix)]
    {
        let unsaved: Vec<String> = makepad_app_registry::machine::unsaved_work().into_iter().filter(|w| w.0 == pid).map(|w| w.2).collect();
        if !unsaved.is_empty() {
            return Err(format!("{app} (pid {pid}) holds unsaved work ({}): its person saves or closes it first", unsaved.join("; ")));
        }
    }
    let start = makepad_remote::relay::proc_start(pid);
    let client = makepad_remote::link::Client::connect(&pid.to_string()).map_err(|e| format!("{app} (pid {pid}): {e}"))?;
    // (an app answers its `quit` at once; the master's ask of this waits 60 s in all)
    client.set_timeout(Some(Duration::from_secs(5)));
    let said: Vec<String> = client.cmd("quit").map(|lines| lines.map_while(Result::ok).collect()).map_err(|e| format!("{app} (pid {pid}): {e}"))?;
    client.close();
    if let Some(e) = said.iter().find(|l| l.starts_with("{\"err\":")) {
        return Err(format!("{app} (pid {pid}) did not quit: {}", proto::clean(e, 300)));
    }
    let end = Instant::now() + Duration::from_secs(30);
    while makepad_remote::relay::proc_start(pid) == start && start.is_some() {
        if Instant::now() >= end {
            return Err(format!("{app} (pid {pid}) took the quit and still runs 30 s later: nothing stopped it"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok("quit through its remote".into())
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
            // (`stop <pid> <app>`: a show's, which may be the dock's instance of the app: [`quit_instance`])
            ["stop", pid, app @ ..] if app.len() <= 1 => {
                if !guard(out) {
                    return;
                }
                let Ok(pid) = pid.parse::<u32>() else { return err(out, "stop <pid> [app]") };
                let stopped = match (ledger(pid), app.first()) {
                    (None, Some(app)) => quit_instance(pid, app),
                    _ => stop(pid),
                };
                match stopped {
                    Ok(how) => {
                        publish(hub_record(&machine, "", pid, "launch", Sev::Info, format!("launch: stopped #{pid} ({how})"), String::new()));
                        out.push(format!("{{\"stopped\":{pid},\"how\":{}}}", json_str(&how)));
                    }
                    Err(e) => err(out, &e),
                }
            }
            // what `show <app> native` opened: a person's to close if one watches the screen
            ["close", app] => {
                if !guard(out) {
                    return;
                }
                if crate::lease::exec::user_watching() {
                    return err(out, &format!("someone watches {machine}'s screen: what its WM opened is theirs to close"));
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
            ["dock"] => {
                let tiles = dock_tiles();
                for e in dock_entries() {
                    out.push(format!(
                        "{{\"dock\":{},\"entry\":{},\"in_dock\":{},\"icon\":{},\"runs\":{},\"agent\":{},\"cwd\":{},\"synced_tree\":{},\"args\":{}}}",
                        json_str(&e.id),
                        json_str(&e.bundle.display().to_string()),
                        tiles.contains(&e.bundle),
                        e.icon.as_ref().map_or("null".into(), |i| json_str(&i.display().to_string())),
                        e.runs,
                        e.agent,
                        e.cwd.as_ref().map_or("null".into(), |c| json_str(&c.display().to_string())),
                        e.cwd.as_ref().is_some_and(|c| c.starts_with(&conf.checkout)),
                        json_str(&e.args.join(" "))
                    ));
                }
            }
            ["dock", "own"] => match own_record(&conf.checkout) {
                Ok(path) => {
                    publish(hub_record(&machine, "runner", 0, "launch", Sev::Info, format!("launch: the Runner's own entry runs the synced tree's Runner ({})", path.display()), String::new()));
                    out.push(format!("{{\"dock_own\":{}}}", json_str(&path.display().to_string())));
                }
                Err(e) => err(out, &e),
            },
            ["dock", "remove", words @ ..] if !words.is_empty() => match dock_words(words).and_then(|apps| dock_remove(&apps.iter().map(String::as_str).collect::<Vec<_>>())) {
                Ok(lines) => {
                    publish(hub_record(&machine, "dock", 0, "launch", Sev::Info, format!("launch: the Dock no longer keeps {}", words.join(", ")), String::new()));
                    out.extend(lines);
                }
                Err(e) => err(out, &e),
            },
            ["dock", "add", apps @ ..] => match dock_add(apps) {
                Ok(lines) => {
                    publish(hub_record(&machine, "dock", 0, "launch", Sev::Info, format!("launch: the Dock keeps {} more of the Runner's entries", lines.iter().filter(|l| l.contains("\"added\":true")).count()), String::new()));
                    out.extend(lines);
                }
                Err(e) => err(out, &e),
            },
            ["list"] => {
                for pid in launched() {
                    if let Some((s, _, log, _)) = ledger(pid) {
                        let present = present_ms(&log).map_or("null".to_string(), |ms| ms.to_string());
                        let jit = jits::announced_jit(pid).map(|(_, p)| p);
                        let inspect = jit.map_or("null".to_string(), |p| json_str(&format!("{machine}/{p}")));
                        out.push(format!("{{\"pid\":{pid},\"id\":{},\"app\":{},\"visible\":{},\"jit\":{},\"inspect\":{inspect},\"present_ms\":{present},\"log\":{}}}", json_str(&format!("{machine}/{}#{pid}", s.app)), json_str(&s.app), s.visible, jit.is_some(), json_str(&log.display().to_string())));
                    }
                }
            }
            ["log", pid, rest @ ..] => {
                let n = rest.first().and_then(|n| n.parse::<usize>().ok()).unwrap_or(40).min(400);
                // a running launch by pid, else an app's latest launch (ended ones too)
                let Some(log) = pid.parse().ok().and_then(ledger).map(|(_, _, log, _)| log).or_else(|| latest_log(pid)) else {
                    return err(out, &format!("{pid} is neither a running launch of this hub nor an app launched here"));
                };
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                for l in &lines[lines.len().saturating_sub(n)..] {
                    out.push(format!("{{\"log\":{}}}", json_str(&proto::clean(l, 400))));
                }
            }
            _ => err(out, "launch [visible|hidden] [lan] [release] [K=V ...] <app> [args...] | show <app> [native] | dock | dock add [app ...] | dock own | stop <pid> | relaunch <pid> | list | log <pid> [n]"),
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
    #[cfg(target_os = "macos")]
    {
        let _ = native;
        // its compiled entry (`<id>.native`) first, as the Runner pins one once it is built
        let native_id = format!("{}.native", entry.id);
        let mut entries: Vec<DockEntry> = dock_entries().into_iter().filter(|e| e.id == entry.id || e.id == native_id).collect();
        entries.sort_by_key(|e| (e.id != native_id, !e.bundle.starts_with("/Applications/")));
        let e = entries.into_iter().next().ok_or_else(|| format!("show: this Mac's dock has no {} (the Runner adds it: Install in Applications)", entry.id))?;
        let st = std::process::Command::new("/usr/bin/open")
            .arg(&e.bundle)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|err| format!("show: open {}: {err}", e.bundle.display()))?;
        if !st.success() {
            return Err(format!("show: open {} ended {st}", e.bundle.display()));
        }
        Ok(format!("{{\"shown\":{},\"entry\":{}}}", json_str(entry.id), json_str(&e.bundle.display().to_string())))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (entry, native);
        Err("show: only a Linux node's WM or a Mac's Runner entries make a dock to ask".into())
    }
}

/// One of a Mac's dock entries (the Runner's pin.rs): its app id, its bundle, and what its launch
/// record (`$RELAX_HOME/apps/<id>.launch`) runs: the tree (`cwd`) and the `makepad` arguments.
pub struct DockEntry {
    pub id: String,
    pub bundle: PathBuf,
    pub cwd: Option<PathBuf>,
    pub args: Vec<String>,
    /// the icon file its Info.plist names, when the bundle holds it (none: the Dock shows a blank app)
    pub icon: Option<PathBuf>,
    /// its executable is there (a live entry's: a link whose loader is there)
    pub runs: bool,
    /// an agent app (its Info.plist's `LSUIElement` is true: a Runner companion such as the WM's
    /// AI pane, or a hidden runner's entry): never a Dock tile, so `dock add` leaves it out
    pub agent: bool,
}

/// This Mac's dock entries, by id (none elsewhere: a WM's dock is the WM's to say, `show`).
pub fn dock_entries() -> Vec<DockEntry> {
    let mut out = Vec::new();
    for dir in [PathBuf::from("/Applications/Makepad"), crate::os::home_dir().join("Applications/Makepad")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for bundle in rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "app")) {
            let Ok(plist) = std::fs::read_to_string(bundle.join("Contents/Info.plist")) else { continue };
            let Some(id) = plist_string(&plist, "CFBundleIdentifier").and_then(|b| b.strip_prefix("nl.makepad.").map(str::to_string)) else { continue };
            let record = plist_string(&plist, "RELAX_HOME").and_then(|h| std::fs::read_to_string(PathBuf::from(h).join("apps").join(format!("{id}.launch"))).ok()).unwrap_or_default();
            let (cwd, args) = launch_record(&record);
            let icon = plist_string(&plist, "CFBundleIconFile").and_then(|name| {
                let res = bundle.join("Contents/Resources");
                [res.join(&name), res.join(format!("{name}.icns"))].into_iter().find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0))
            });
            let runs = plist_string(&plist, "CFBundleExecutable").is_some_and(|exe| bundle.join("Contents/MacOS").join(exe).is_file());
            let agent = plist.find("<key>LSUIElement</key>").is_some_and(|at| plist[at + "<key>LSUIElement</key>".len()..].trim_start().starts_with("<true/>"));
            out.push(DockEntry { id, bundle, cwd, args, icon, runs, agent });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The Runner's own launch record (`<home>/apps/runner.launch`, the home its entries name, else
/// RELAX_HOME, else ~/.relax), written for the Runner of the tree `checkout`: what the Runner
/// writes for an app of a checkout (its launch.rs `record`), from the registry's `runner` row.
fn own_record(checkout: &std::path::Path) -> Result<PathBuf, String> {
    let row = makepad_app_registry::find("runner").ok_or("dock own: the registry has no runner row")?;
    let home = dock_entries()
        .iter()
        .find_map(|e| {
            let plist = std::fs::read_to_string(e.bundle.join("Contents/Info.plist")).ok()?;
            plist_string(&plist, "RELAX_HOME")
        })
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("RELAX_HOME").filter(|h| !h.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| crate::os::home_dir().join(".relax"));
    let mut text = format!("title\t{}\ncwd\t{}\narg\trun\narg\t--quiet\narg\t{}\narg\t--no-default-features\n", row.label, checkout.display(), row.package);
    if !row.features.is_empty() {
        text.push_str(&format!("arg\t--features\narg\t{}\n", row.features.join(",")));
    }
    let dir = home.join("apps");
    std::fs::create_dir_all(&dir).map_err(|e| format!("dock own: {}: {e}", dir.display()))?;
    let path = dir.join("runner.launch");
    let part = dir.join("runner.launch.part");
    std::fs::write(&part, text).and_then(|_| std::fs::rename(&part, &path)).map_err(|e| format!("dock own: {}: {e}", path.display()))?;
    Ok(path)
}

/// The macOS Dock's kept apps (its `persistent-apps`), as bundle paths: `defaults export
/// com.apple.dock -`, the Dock's own preferences through cfprefsd. None elsewhere.
fn dock_tiles() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/bin/defaults").args(["export", "com.apple.dock", "-"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
        out.map(|o| persistent_apps(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    Vec::new()
}

/// The bundle paths of the `persistent-apps` tiles in an export of the Dock's preferences (their
/// `file://` URLs, percent-decoded, without the trailing slash).
#[cfg(any(target_os = "macos", test))]
fn persistent_apps(xml: &str) -> Vec<PathBuf> {
    let Some(at) = xml.find("<key>persistent-apps</key>") else { return Vec::new() };
    let rest = &xml[at..];
    let Some(open) = rest.find("<array>") else { return Vec::new() };
    // the array's own end: arrays nest in tiles
    let (mut depth, mut i, mut end) = (0usize, open, rest.len());
    while i < rest.len() {
        if rest[i..].starts_with("<array>") {
            depth += 1;
            i += 7;
        } else if rest[i..].starts_with("</array>") {
            depth -= 1;
            if depth == 0 {
                end = i;
                break;
            }
            i += 8;
        } else {
            i += rest[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    let mut out = Vec::new();
    let mut tiles = &rest[open..end];
    while let Some(k) = tiles.find("<key>_CFURLString</key>") {
        tiles = &tiles[k + "<key>_CFURLString</key>".len()..];
        let Some(s) = tiles.find("<string>") else { break };
        let Some(e) = tiles[s..].find("</string>") else { break };
        let url = tiles[s + 8..s + e].replace("&amp;", "&");
        if let Some(path) = url.strip_prefix("file://") {
            out.push(PathBuf::from(percent_decode(path.trim_end_matches('/'))));
        }
    }
    out
}

#[cfg(any(target_os = "macos", test))]
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A Dock tile of the bundle at `path`, as `defaults write com.apple.dock persistent-apps
/// -array-add` takes it.
#[cfg(any(target_os = "macos", test))]
fn dock_tile(path: &std::path::Path) -> String {
    let mut url = String::from("file://");
    for &c in path.to_string_lossy().as_bytes() {
        if c.is_ascii_alphanumeric() || b"/-._~".contains(&c) {
            url.push(c as char);
        } else {
            url.push_str(&format!("%{c:02X}"));
        }
    }
    format!("<dict><key>tile-data</key><dict><key>file-data</key><dict><key>_CFURLString</key><string>{url}/</string><key>_CFURLStringType</key><integer>15</integer></dict></dict><key>tile-type</key><string>file-tile</string></dict>")
}

/// The Runner's entries the macOS Dock does not keep yet, added to it (all, or the apps named):
/// one per app (its compiled build's entry and its live one share an id; the one in
/// /Applications/Makepad, where the Runner pins when it may, before the user's own folder), then
/// the Dock restarted by launchd once to show them. One line per entry: added, or kept already.
fn dock_add(apps: &[&str]) -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let mut chosen: Vec<DockEntry> = Vec::new();
        for e in dock_entries() {
            let id = e.id.trim_end_matches(".native").to_string();
            if e.agent || (!apps.is_empty() && !apps.contains(&id.as_str())) {
                continue;
            }
            let system = e.bundle.starts_with("/Applications/");
            match chosen.iter_mut().find(|c| c.id == id) {
                Some(c) if system && !c.bundle.starts_with("/Applications/") => c.bundle = e.bundle,
                Some(_) => {}
                None => chosen.push(DockEntry { id, ..e }),
            }
        }
        if chosen.is_empty() {
            return Err("dock add: no Runner entry to add (the Runner pins them: Install in Applications, or its `pin` query)".into());
        }
        let tiles = dock_tiles();
        let mut lines = Vec::new();
        let mut added = 0;
        for e in &chosen {
            let new = !tiles.contains(&e.bundle);
            if new {
                let st = std::process::Command::new("/usr/bin/defaults")
                    .args(["write", "com.apple.dock", "persistent-apps", "-array-add", &dock_tile(&e.bundle)])
                    .stdin(std::process::Stdio::null())
                    .status()
                    .map_err(|err| format!("dock add: defaults: {err}"))?;
                if !st.success() {
                    return Err(format!("dock add: defaults write for {} ended {st}", e.bundle.display()));
                }
                added += 1;
            }
            lines.push(format!("{{\"dock_add\":{},\"entry\":{},\"added\":{new}}}", json_str(&e.id), json_str(&e.bundle.display().to_string())));
        }
        if added > 0 {
            restart_dock().map_err(|e| format!("dock add: {added} added; {e}: they show at the Dock's next start"))?;
        }
        Ok(lines)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = apps;
        Err("dock add: only a Mac has a Dock to keep the Runner's entries".into())
    }
}

/// The Dock restarted by launchd (`launchctl kickstart -k gui/<uid>/com.apple.Dock.agent`), so it
/// reads its kept apps again.
#[cfg(target_os = "macos")]
fn restart_dock() -> Result<(), String> {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions
    let service = format!("gui/{}/com.apple.Dock.agent", unsafe { getuid() });
    let st = std::process::Command::new("/bin/launchctl").args(["kickstart", "-k", &service]).stdin(std::process::Stdio::null()).status().map_err(|err| format!("launchctl: {err}"))?;
    if !st.success() {
        return Err(format!("launchctl kickstart -k {service} ended {st}"));
    }
    Ok(())
}

/// The named apps' Runner entries (compiled and live) taken out of the macOS Dock's kept apps:
/// the Dock's preferences exported, those tiles left out, imported again through cfprefsd, then
/// the Dock restarted once. The entries themselves stay (the Runner's Pin makes and removes them).
/// One line per entry: removed, or not in the Dock.
/// `dock remove`'s words: app ids, and entry paths as `b64:<path>` (a title has spaces).
fn dock_words(words: &[&str]) -> Result<Vec<String>, String> {
    words
        .iter()
        .map(|w| match w.strip_prefix("b64:") {
            Some(b) => crate::lease::exec::b64_decode(b).and_then(|p| String::from_utf8(p).ok()).ok_or_else(|| format!("dock remove: {w} is not base64 UTF-8")),
            None => Ok(w.to_string()),
        })
        .collect()
}

fn dock_remove(apps: &[&str]) -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        // an app's entries, or one entry by its path (an app with two entries keeps the other)
        let chosen: Vec<DockEntry> = dock_entries().into_iter().filter(|e| apps.iter().any(|a| *a == e.id.trim_end_matches(".native") || std::path::Path::new(a) == e.bundle)).collect();
        if chosen.is_empty() {
            return Err(format!("dock remove: no Runner entry of {}", apps.join(", ")));
        }
        let export = std::process::Command::new("/usr/bin/defaults").args(["export", "com.apple.dock", "-"]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("dock remove: defaults export: {e}"))?;
        if !export.status.success() {
            return Err(format!("dock remove: defaults export ended {}", export.status));
        }
        let paths: Vec<PathBuf> = chosen.iter().map(|e| e.bundle.clone()).collect();
        let (xml, removed) = without_tiles(&String::from_utf8_lossy(&export.stdout), &paths);
        if !removed.is_empty() {
            use std::io::Write;
            let mut import = std::process::Command::new("/usr/bin/defaults").args(["import", "com.apple.dock", "-"]).stdin(std::process::Stdio::piped()).spawn().map_err(|e| format!("dock remove: defaults import: {e}"))?;
            import.stdin.take().ok_or("dock remove: defaults import has no stdin")?.write_all(xml.as_bytes()).map_err(|e| format!("dock remove: defaults import: {e}"))?;
            let st = import.wait().map_err(|e| format!("dock remove: defaults import: {e}"))?;
            if !st.success() {
                return Err(format!("dock remove: defaults import ended {st}"));
            }
            restart_dock().map_err(|e| format!("dock remove: removed; {e}: they go at the Dock's next start"))?;
        }
        Ok(chosen.iter().map(|e| format!("{{\"dock_remove\":{},\"entry\":{},\"removed\":{}}}", json_str(&e.id), json_str(&e.bundle.display().to_string()), removed.contains(&e.bundle))).collect())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = apps;
        Err("dock remove: only a Mac has a Dock".into())
    }
}

/// An export of the Dock's preferences without the `persistent-apps` tiles of the bundles at
/// `paths`, and the bundles whose tiles it left out.
#[cfg(any(target_os = "macos", test))]
fn without_tiles(xml: &str, paths: &[PathBuf]) -> (String, Vec<PathBuf>) {
    let Some(at) = xml.find("<key>persistent-apps</key>") else { return (xml.to_string(), Vec::new()) };
    let Some(open) = xml[at..].find("<array>").map(|o| at + o + "<array>".len()) else { return (xml.to_string(), Vec::new()) };
    // the array's own tiles: its top-level dicts (dicts and arrays nest inside a tile)
    let (mut cut, mut removed) = (Vec::new(), Vec::new());
    let (mut depth, mut arrays, mut i, mut start) = (0usize, 0usize, open, open);
    while i < xml.len() {
        let rest = &xml[i..];
        if rest.starts_with("<dict>") {
            if depth == 0 {
                start = i;
            }
            depth += 1;
            i += 6;
        } else if rest.starts_with("</dict>") {
            depth = depth.saturating_sub(1);
            i += 7;
            if depth == 0 {
                let tile = &xml[start..i];
                if let Some(path) = persistent_apps(&format!("<key>persistent-apps</key><array>{tile}</array>")).into_iter().find(|p| paths.contains(p)) {
                    // with the whitespace before it, back to the previous element
                    let from = xml[..start].trim_end().len();
                    cut.push((from, i));
                    removed.push(path);
                }
            }
        } else if rest.starts_with("<array>") {
            arrays += 1;
            i += 7;
        } else if rest.starts_with("</array>") {
            if arrays == 0 && depth == 0 {
                break;
            }
            arrays = arrays.saturating_sub(1);
            i += 8;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    let mut out = xml.to_string();
    for (from, to) in cut.into_iter().rev() {
        out.replace_range(from..to, "");
    }
    (out, removed)
}

/// The `<string>` after `<key>{key}</key>` in an Info.plist (the first such key, nested dicts too).
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let rest = &plist[plist.find(&format!("<key>{key}</key>"))?..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    let v = &rest[start..start + end];
    Some(v.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&"))
}

/// A Runner launch record's tree (`cwd\t<dir>`) and `makepad` arguments (`arg\t<word>` lines).
fn launch_record(text: &str) -> (Option<PathBuf>, Vec<String>) {
    let mut cwd = None;
    let mut args = Vec::new();
    for line in text.lines() {
        match line.split_once('\t') {
            Some(("cwd", d)) => cwd = Some(PathBuf::from(d)),
            Some(("arg", a)) => args.push(a.to_string()),
            _ => {}
        }
    }
    (cwd, args)
}

/// How long after its start (the ms in its log's name, `<app>-<ms>.log`) a launch's JIT reported
/// its first frame on glass (its report's `present` line, the report's last write): None before.
fn present_ms(log: &std::path::Path) -> Option<u64> {
    let report = log.with_extension("report");
    let text = std::fs::read_to_string(&report).ok()?;
    if !text.lines().any(|l| l.trim() == "present") {
        return None;
    }
    let started: u64 = log.file_stem()?.to_str()?.rsplit('-').next()?.parse().ok()?;
    let at = std::fs::metadata(&report).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some(at.saturating_sub(started))
}

/// The log of `app`'s latest launch here (`<app>-<ms>.log`, named as the launch named it: its
/// registry id or its package), kept after it ended.
fn latest_log(app: &str) -> Option<PathBuf> {
    let entry = find_app(app)?;
    std::fs::read_dir(ledger_dir())
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let rest = [entry.id, entry.package].iter().find_map(|w| name.strip_prefix(w)?.strip_prefix('-'))?;
            let ms: u64 = rest.strip_suffix(".log")?.parse().ok()?;
            Some((ms, e.path()))
        })
        .max_by_key(|(ms, _)| *ms)
        .map(|(_, p)| p)
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
            // `id` is the launch's (stop, launch-log); `inspect` the JIT's (a Mac's runs in a child)
            let inspect = jit.map_or("null".to_string(), |p| json_str(&format!("{machine}/{p}")));
            out.push(format!("{{\"launched\":{pid},\"id\":{},\"app\":{},\"mode\":\"{mode}\",\"jit\":{},\"inspect\":{inspect},\"log\":{}}}", json_str(&id), json_str(&spec.app), jit.is_some(), json_str(&log.display().to_string())));
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
        // `show --replace` quits the app's running instance that no lease started, its JIT's run
        // or the compiled app, the dock's as much as a launch's of this hub: never a lease's, nor
        // another app, nor a pid no instance answers to
        let mut behind = app(7, "clock", &[("run", "r2")]);
        behind.restart_parent = Some(6);
        let apps = [app(1, "clock", &[]), app(2, "clock", &[("run", "r1")]), app(3, "clock", &[("lease", "L1")]), app(4, "other", &[]), behind];
        let quits: Vec<u32> = (1..=8).filter(|&pid| instance_of(&apps, pid, "clock")).collect();
        assert_eq!(quits, [1, 2, 6, 7], "the dock's and a run's instances, by its pid or the parent it runs behind");
    }

    #[test]
    fn specs_and_env_policy() {
        let s = parse(&["visible", "lan", "FOO_HOST=1", "FOO_BIND_IP=192.0.2.89", "foo", "--x"]).unwrap();
        assert!(s.visible && s.lan && !s.release);
        assert!(parse(&["hidden", "release", "foo"]).is_ok_and(|s| s.release && s.app == "foo"));
        assert_eq!(s.env, vec![("FOO_HOST".to_string(), "1".to_string()), ("FOO_BIND_IP".to_string(), "192.0.2.89".to_string())]);
        assert_eq!((s.app.as_str(), s.args.clone()), ("foo", vec!["--x".to_string()]));
        for bad in ["LD_PRELOAD=x", "PATH=/tmp", "CARGO_HOME=/x", "RUSTFLAGS=-x", "MAKEPAD_HIDE_WINDOWS=0"] {
            assert!(parse(&[bad, "foo"]).is_err(), "{bad}");
        }
        assert!(parse(&["hidden"]).is_err());
    }

    /// A Mac's dock entry is read from the Runner's bundle and launch record: its id, home, tree
    /// and arguments.
    #[test]
    fn the_docks_kept_apps_round_trip_through_a_tile() {
        let a = std::path::Path::new("/Applications/Makepad/Makepad Scope.app");
        let word = format!("b64:{}", makepad_network::digest::base64_encode(a.to_string_lossy().as_bytes()));
        assert_eq!(dock_words(&["scope", &word]).unwrap(), ["scope", "/Applications/Makepad/Makepad Scope.app"], "an entry's path keeps its spaces");
        let tile = dock_tile(a);
        assert!(tile.contains("file:///Applications/Makepad/Makepad%20Scope.app/"), "a tile is the bundle's file URL: {tile}");
        let export = format!("<plist><dict><key>persistent-apps</key><array>{tile}<dict><key>tile-data</key><dict><key>file-data</key><dict><key>_CFURLString</key><string>file:///System/Applications/Mail.app/</string></dict><key>x</key><array><string>y</string></array></dict></dict></array><key>persistent-others</key><array><dict><key>_CFURLString</key><string>file:///Users/x/Downloads/</string></dict></array></dict></plist>");
        assert_eq!(persistent_apps(&export), vec![a.to_path_buf(), PathBuf::from("/System/Applications/Mail.app")], "only persistent-apps' tiles, decoded, past a nested array");
        // removed: that tile alone, past an empty dict, the other tiles and lists as they were
        let export = export.replacen("<key>x</key>", "<key>e</key><dict/><key>x</key>", 1);
        let (left, removed) = without_tiles(&export, &[a.to_path_buf(), PathBuf::from("/Applications/Gone.app")]);
        assert_eq!(removed, vec![a.to_path_buf()], "only a tile the Dock keeps is removed");
        assert_eq!(persistent_apps(&left), vec![PathBuf::from("/System/Applications/Mail.app")], "the other tile stays: {left}");
        assert_eq!(left, export.replacen(&tile, "", 1), "nothing else changes");
        assert_eq!(without_tiles(&left, &[a.to_path_buf()]), (left.clone(), Vec::new()), "a second remove changes nothing");
    }

    #[test]
    fn a_mac_dock_entry_reads_its_bundle_and_record() {
        let plist = "<dict>\n    <key>CFBundleIdentifier</key>\n    <string>nl.makepad.calculator</string>\n    <key>LSEnvironment</key>\n    <dict>\n        <key>RELAX_APP</key>\n        <string>calculator</string>\n        <key>RELAX_HOME</key>\n        <string>/Users/x/.relax &amp; co</string>\n    </dict>\n</dict>\n";
        assert_eq!(plist_string(plist, "CFBundleIdentifier").as_deref(), Some("nl.makepad.calculator"));
        assert_eq!(plist_string(plist, "RELAX_HOME").as_deref(), Some("/Users/x/.relax & co"));
        assert_eq!(plist_string(plist, "CFBundleIconFile"), None);
        let (cwd, args) = launch_record("title\tCalculator\ncwd\t/t/tree\narg\trun\narg\t--quiet\narg\tmakepad-app-calculator\n");
        assert_eq!((cwd.as_deref(), args.join(" ").as_str()), (Some(std::path::Path::new("/t/tree")), "run --quiet makepad-app-calculator"));
        assert_eq!(launch_record("title\tStore app\n"), (None, Vec::new()));
    }

    /// A launch's first frame is its report's `present`, timed from the start in its log's name.
    #[test]
    fn present_ms_is_the_reports_present_after_the_start() {
        let dir = std::env::temp_dir().join(format!("devhub-present-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let started = proto::wall_ms() - 1500;
        let log = dir.join(format!("calculator-{started}.log"));
        std::fs::write(log.with_extension("report"), b"").unwrap();
        assert_eq!(present_ms(&log), None, "no frame yet");
        std::fs::write(log.with_extension("report"), b"pid\t42\t0\npresent\n").unwrap();
        assert!(present_ms(&log).is_some_and(|ms| (1400..60_000).contains(&ms)), "{:?}", present_ms(&log));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
