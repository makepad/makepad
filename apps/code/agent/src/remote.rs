//! The agent's reach into running makepad apps: every app on this machine (its same-user
//! makepad-remote endpoint) and on the hosts of the remote registry (`hosts.toml`, through the
//! hub). A small fixed set of tools, so a local model's context is not swamped by every app's
//! manifest:
//!
//! - `apps`: what runs where, with each app's AI services (`app` service) and tool counts;
//! - `app_tools`: one target's manifest(s) as compact text;
//! - `app_call`: one AI tool call (`service.tool` + JSON args);
//! - `ui`: the app's built-in remote commands (dump, snap, click, text, key, grab, ...);
//! - `launch`: start an app on a host (into its window manager when one runs there);
//! - `list_apps`, `build_app`, `run_app`, `stop_app`, `app_status`, `app_logs`: build and run the
//!   workspace's own apps through the run manager the RUN panel shows (runs.rs); a run starts
//!   with `--remote` (or under `makepad run`), so the tools above reach it by its pid.
//!
//! Targets: `stage`, a pid, `<host>:<app|pid>` (`n149:wm`). Apps a window manager hosts are
//! reached through the WM for their tools (`n149:wm`, instance = the `w<id>` endpoint) and through
//! their own pid for `ui`. Approvals: a tool runs by its declared risk; Request and Destructive
//! tools (and `ui quit` / `ui close`) ask first, through the same approval card as a shell
//! command. The app keeps its own floor: it validates and refuses as it would for its own chat.

use super::engine::Engine;
use super::event::ToolId;
use super::tools::{Outcome, Verdict};
use makepad_ai_hub::local_llm::ToolSpec;
use makepad_ai_services::wire::{AppLine, AppRequest, Risk, ServiceManifest, ToolOutcome};
use makepad_remote::conn::{Event, CLOSE_LINK_LOST, CLOSE_NORMAL};
use makepad_remote::{discovery, Client};
use makepad_strict_json as json;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const NAMES: [&str; 14] = [
    "apps", "app_tools", "app_call", "ui", "launch", "attach", "detach",
    "list_apps", "start_app", "build_app", "run_app", "stop_app", "app_status", "app_logs",
];

/// Services attached at once: each adds its tools to the model's table.
pub const MAX_ATTACHED: usize = 3;

/// A fixed tool, or a tool of an attached service (`<service>__<tool>`).
pub fn is_tool(name: &str) -> bool {
    NAMES.contains(&name) || attached_tool(name).is_some()
}

/// An attached service: where it runs and its manifest.
#[derive(Clone)]
struct Attached {
    target: String,
    instance: Option<String>,
    manifest: ServiceManifest,
}

fn attached() -> &'static Mutex<Vec<Attached>> {
    static A: OnceLock<Mutex<Vec<Attached>>> = OnceLock::new();
    A.get_or_init(Default::default)
}

/// The attached service and tool a model tool name means.
fn attached_tool(name: &str) -> Option<(Attached, String)> {
    let (service, tool) = name.split_once("__")?;
    let a = attached().lock().unwrap().iter().find(|a| a.manifest.id == service)?.clone();
    a.manifest.tool(tool)?;
    Some((a, tool.to_string()))
}

/// The tools of the attached services as model tools (a lane started after an attach gets them in
/// its table; before that the model calls them by the names `attach` printed).
pub fn attached_specs() -> Vec<ToolSpec> {
    let mut out = Vec::new();
    for a in attached().lock().unwrap().iter() {
        for t in &a.manifest.tools {
            out.push(ToolSpec::new(&makepad_ai_services::wire::api_name(&a.manifest.id, &t.name), &format!("[{} on {}] {}", a.manifest.id, a.target, t.description), &t.parameters));
        }
    }
    out
}

/// The model's named arguments as the JSON object the tool's schema describes: string properties
/// stay strings, the rest are taken as JSON when they parse.
fn args_object(schema: &str, args: &[(String, String)]) -> String {
    let mut types: HashMap<String, String> = HashMap::new();
    if let Ok(json::Value::Obj(fields)) = json::parse(schema.as_bytes()) {
        for (k, v) in fields {
            if let ("properties", json::Value::Obj(props)) = (k.as_str(), v) {
                for (name, prop) in props {
                    let ty = match prop {
                        json::Value::Obj(p) => p.into_iter().find(|(k, _)| k == "type").and_then(|(_, t)| t.as_str().map(str::to_string)).unwrap_or_default(),
                        _ => String::new(),
                    };
                    types.insert(name, ty);
                }
            }
        }
    }
    let pairs: Vec<(String, json::Value)> = args
        .iter()
        .map(|(k, v)| {
            let value = match types.get(k).map(String::as_str) {
                Some("string") => json::s(v.as_str()),
                _ => json::parse(v.trim().as_bytes()).unwrap_or_else(|_| json::s(v.as_str())),
            };
            (k.clone(), value)
        })
        .collect();
    json::Value::Obj(pairs).to_json()
}

pub fn specs() -> Vec<ToolSpec> {
    let t = |name: &str, desc: &str, params: &str| ToolSpec::new(name, desc, params);
    vec![
        t("apps", "List the running makepad apps on this machine and on the paired hosts: target name, pid, and the AI services each offers (with tool counts). Apps inside a window manager are listed under it.",
          r#"{"type":"object","properties":{}}"#),
        t("app_tools", "Show the AI tools of a running app: name, risk, what it does, its arguments.",
          r#"{"type":"object","properties":{"target":{"type":"string","description":"app name, pid, or host:app (from apps)"},"service":{"type":"string","description":"only this service"}},"required":["target"]}"#),
        t("app_call", "Call one AI tool of a running app and get its result.",
          r#"{"type":"object","properties":{"target":{"type":"string","description":"app name, pid, or host:app"},"tool":{"type":"string","description":"service.tool, e.g. calculator.eval"},"args":{"type":"string","description":"the arguments as a JSON object, default {}"},"instance":{"type":"string","description":"which instance when several run (the endpoint from apps, e.g. w4)"}},"required":["target","tool"]}"#),
        t("ui", "Drive an app's window directly: `dump` (widget tree), `snap` (widgets with their positions), `click x=.. y=..`, `text text=..`, `key key=..`, `grab` (a PNG), `status`, `help` (all commands).",
          r#"{"type":"object","properties":{"target":{"type":"string","description":"app name, pid, or host:pid"},"command":{"type":"string","description":"the command line, e.g. click x=120 y=40"}},"required":["target","command"]}"#),
        t("attach", "Add the tools of one app service to your own tools for this session (at most 3 services), so you call them directly as <service>__<tool> with named arguments instead of through app_call.",
          r#"{"type":"object","properties":{"target":{"type":"string","description":"app name, pid, or host:app"},"service":{"type":"string","description":"the service id from apps"},"instance":{"type":"string","description":"which instance when several run"}},"required":["target","service"]}"#),
        t("detach", "Remove an attached service's tools.",
          r#"{"type":"object","properties":{"service":{"type":"string"}},"required":["service"]}"#),
        t("list_apps", "Every makepad app on this machine (the app registry): id, label, package, whether it is built and running; and what the window manager runs, when one runs. Use it for \"what apps are there\".",
          r#"{"type":"object","properties":{}}"#),
        t("start_app", "Start an app by its name (\"Stage\", \"Scope\", \"calculator\") the way the dock does: inside this machine's window manager when one runs (it builds it first when needed), else built from the checkout and started with --remote. Waits until it runs and returns its pid (drive it with ui / app_tools target=<pid>) and the errors it logged at startup.",
          r#"{"type":"object","properties":{"app":{"type":"string","description":"the app's name or id, e.g. Stage"},"timeout_secs":{"type":"integer","description":"how long to wait for it to come up, default 600"}},"required":["app"]}"#),
        t("build_app", "Build one app of this workspace (cargo build --release of its package, queued, one build at a time) and wait for the result: progress, then errors and warnings as file:line.",
          r#"{"type":"object","properties":{"app":{"type":"string","description":"app id from list_apps, e.g. calculator"},"timeout_secs":{"type":"integer","description":"how long to wait, default 1200"}},"required":["app"]}"#),
        t("run_app", "Build and start one app of this workspace from source and wait until it runs (to just open an app, use start_app). mode native: the release binary with --remote; makepad_run: from source under `makepad run` (relaunches warm when its files are saved). The result names the pid (drive it with ui / app_tools / app_call target=<pid>) and lists the errors the app logged at startup: Splash mistakes compile fine and only show there.",
          r#"{"type":"object","properties":{"app":{"type":"string","description":"app id from list_apps"},"mode":{"type":"string","enum":["native","makepad_run"],"description":"default native"},"timeout_secs":{"type":"integer","description":"how long to wait for the build, default 1200"}},"required":["app"]}"#),
        t("stop_app", "Stop an app started with run_app (by app id or pid), or cancel its build.",
          r#"{"type":"object","properties":{"app":{"type":"string","description":"app id or pid"}},"required":["app"]}"#),
        t("app_status", "The builds (state, progress, errors) and the apps started with run_app (pid, mode, uptime, exit).",
          r#"{"type":"object","properties":{"app":{"type":"string","description":"only this app"}}}"#),
        t("app_logs", "The last lines an app started with run_app printed, or the output of its last build.",
          r#"{"type":"object","properties":{"app":{"type":"string","description":"app id or pid"},"lines":{"type":"integer","description":"default 60"},"build":{"type":"boolean","description":"the last build's output instead"}},"required":["app"]}"#),
        t("launch", "Start an app on a host (inside its window manager when one runs there) or on this machine.",
          r#"{"type":"object","properties":{"host":{"type":"string","description":"a host from apps; empty = this machine"},"app":{"type":"string","description":"the app id, e.g. calculator"},"wm":{"type":"string","description":"the window manager to start it in (a target from apps), when the host has several"}},"required":["app"]}"#),
    ]
}

/// How long one `ui` or tool answer may take before the agent stops waiting.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// The hub client (dialled once) and each target's manifests as last listed, for approvals.
#[derive(Default)]
struct Reach {
    hub: Option<Client>,
    manifests: HashMap<String, Vec<ServiceManifest>>,
    /// The pid start_app saw each app come up as, by registry id.
    started: HashMap<String, u32>,
}

fn reach() -> &'static Mutex<Reach> {
    static R: OnceLock<Mutex<Reach>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The hub of the remote registry: started from the `makepad` next to this program (or
/// `MAKEPAD_RUN_BIN`) when none runs.
fn hub() -> Result<Client, String> {
    if let Some(c) = reach().lock().unwrap().hub.clone() {
        return Ok(c);
    }
    #[cfg(unix)]
    {
        let relax = std::env::var_os("MAKEPAD_RUN_BIN").map(std::path::PathBuf::from).or_else(|| {
            let exe = std::env::current_exe().ok()?;
            let sibling = exe.parent()?.join("makepad");
            sibling.exists().then_some(sibling)
        });
        let c = makepad_remote::hub::connect(relax.as_deref())?;
        reach().lock().unwrap().hub = Some(c.clone());
        Ok(c)
    }
    #[cfg(not(unix))]
    Err("remote hosts are not available on this platform yet".into())
}

/// `makepad_remote::hub::ask`; the hub is Unix-only so far (see `hub`).
fn hub_ask(hub: &Client, target: &str, service: &str, args: &str, line: &str, each: &mut dyn FnMut(&str) -> bool) -> Result<bool, String> {
    #[cfg(unix)]
    return makepad_remote::hub::ask(hub, target, service, args, line, each);
    #[cfg(not(unix))]
    {
        let _ = (hub, target, service, args, line, each);
        Err("remote hosts are not available on this platform yet".into())
    }
}

/// A client of `target`: local by app name or pid, or `<host>:<app|pid>` through the hub.
fn client(target: &str) -> Result<Client, String> {
    let target = target.trim();
    if target.contains(':') {
        let hub = hub()?;
        return Ok(Client::routed(&hub, target));
    }
    let c = Client::connect(target).map_err(|e| e.to_string())?;
    c.set_timeout(Some(ANSWER_TIMEOUT));
    Ok(c)
}

/// One `app` request: every answer line, parsed.
fn app_request(client: &Client, request: &AppRequest, progress: &mut dyn FnMut(&str)) -> Result<Vec<AppLine>, String> {
    let ch = client.open("app", &request.to_args()).map_err(|e| match e {
        makepad_remote::link::Error::Refused(_, why) if why.contains("unsupported service") => "this app offers no AI tools (no `app` service); use ui".to_string(),
        e => e.to_string(),
    })?;
    let mut lines = Vec::new();
    loop {
        match ch.recv_timeout(ANSWER_TIMEOUT) {
            Ok(Event::Data(d)) => {
                for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                    match AppLine::parse(l) {
                        Some(AppLine::Progress { note, .. }) => progress(&note),
                        Some(line) => lines.push(line),
                        None => return Err(format!("an answer line I cannot read: {}", cut(l, 200))),
                    }
                }
            }
            Ok(Event::Closed(CLOSE_NORMAL, _)) => return Ok(lines),
            Ok(Event::Closed(CLOSE_LINK_LOST, why)) => return Err(format!("the link was lost: {why}")),
            Ok(Event::Closed(_, why)) => return Err(why),
            Ok(Event::Opened(_)) => {}
            // dropping the channel closes it: the app cancels the call
            Err(true) => return Err(format!("no answer within {} s", ANSWER_TIMEOUT.as_secs())),
            Err(false) => return Err("the channel closed".into()),
        }
    }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// The services every app has (its remote commands): listed by name only, their tools shown when
/// asked for by service.
const BUILT_IN: [&str; 4] = ["ui", "edit", "logs", "inspect"];

fn is_built_in(endpoint: &str) -> bool {
    BUILT_IN.contains(&endpoint)
}

/// One process's AI services in a line: `calculator (1 tool)`, with the instances a WM hosts.
fn services_line(lines: &[AppLine]) -> String {
    let mut parts = Vec::new();
    let mut idle = Vec::new();
    for l in lines {
        match l {
            AppLine::Instance { endpoint, .. } if is_built_in(endpoint.as_str()) => {}
            AppLine::Instance { endpoint, meta, manifest } => {
                let mut p = format!("{} ({} tools", manifest.id, manifest.tools.len());
                if meta.pid != 0 {
                    p.push_str(&format!(", instance {} pid {}", endpoint.as_str(), meta.pid));
                }
                p.push(')');
                parts.push(p);
            }
            AppLine::Launchable { id, .. } => idle.push(id.clone()),
            _ => {}
        }
    }
    let mut s = if parts.is_empty() { "no AI tools".to_string() } else { format!("AI: {}", parts.join(", ")) };
    if !idle.is_empty() {
        s.push_str(&format!("; can launch: {}", idle.join(", ")));
    }
    s
}

fn manifests_of(lines: &[AppLine]) -> Vec<ServiceManifest> {
    let mut out: Vec<ServiceManifest> = Vec::new();
    for l in lines {
        if let AppLine::Instance { manifest, .. } = l {
            if !out.iter().any(|m| m.id == manifest.id) {
                out.push(manifest.clone());
            }
        }
    }
    out
}

/// One app row of `apps`, asking its `app` service when it has one.
fn app_row(target: &str, label: &str, serves_app: bool, client: Result<Client, String>) -> String {
    if !serves_app {
        return format!("{target}  {label}  (ui only)");
    }
    let listed = client.and_then(|c| app_request(&c, &AppRequest::List, &mut |_| {}));
    match listed {
        Ok(lines) => {
            reach().lock().unwrap().manifests.insert(target.to_string(), manifests_of(&lines));
            format!("{target}  {label}  {}", services_line(&lines))
        }
        Err(e) => format!("{target}  {label}  (AI tools unreadable: {})", cut(&e, 120)),
    }
}

fn list_apps() -> Outcome {
    let me = std::process::id();
    let mut rows = Vec::new();
    for a in discovery::apps() {
        if a.pid == me {
            continue;
        }
        let target = a.pid.to_string();
        rows.push(app_row(&target, &format!("{} pid {}", a.app, a.pid), a.serves("app"), client(&target)));
    }
    let mut remote_err = Vec::new();
    match makepad_remote::hosts::Hosts::load(&makepad_remote::hosts::registry_path()) {
        Ok(hosts) if !hosts.hosts.is_empty() => match hub() {
            Ok(hub) => {
                for h in &hosts.hosts {
                    let mut found = Vec::new();
                    let r = hub_ask(&hub, &h.name, "list", "", "list", &mut |l| {
                        if let Some(a) = discovery::parse(l) {
                            found.push(a);
                        }
                        true
                    });
                    if let Err(e) = r {
                        remote_err.push(format!("{}: {}", h.name, cut(&e, 120)));
                        continue;
                    }
                    for a in found {
                        let target = format!("{}:{}", h.name, a.pid);
                        rows.push(app_row(&target, &a.app, a.serves("app"), Ok(Client::routed(&hub, &target))));
                    }
                }
            }
            Err(e) => remote_err.push(format!("hub: {e}")),
        },
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => remote_err.push(format!("hosts.toml: {e}")),
    }
    let mut text = if rows.is_empty() { "no running makepad apps found".to_string() } else { rows.join("\n") };
    if !remote_err.is_empty() {
        text.push_str(&format!("\nnot reachable: {}", remote_err.join("; ")));
    }
    let n = rows.len();
    Outcome { ok: true, text, summary: format!("{n} apps"), diff: None }
}

fn arg<'a>(args: &'a [(String, String)], key: &str) -> Option<&'a str> {
    args.iter().find(|(k, _)| k == key).map(|(_, v)| v.trim()).filter(|v| !v.is_empty())
}

fn err(text: impl Into<String>) -> Outcome {
    let text = text.into();
    Outcome { ok: false, summary: text.lines().next().unwrap_or("").to_string(), text, diff: None }
}

fn risk_name(r: Risk) -> &'static str {
    match r {
        Risk::Read => "read",
        Risk::Act => "act",
        Risk::Request => "makes something",
        Risk::Destructive => "destructive",
    }
}

/// The required argument names of a JSON schema, and all its property names.
fn schema_args(schema: &str) -> (Vec<String>, Vec<String>) {
    let Ok(json::Value::Obj(fields)) = json::parse(schema.as_bytes()) else { return (Vec::new(), Vec::new()) };
    let mut required = Vec::new();
    let mut all = Vec::new();
    for (k, v) in &fields {
        match (k.as_str(), v) {
            ("required", json::Value::Arr(items)) => required = items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect(),
            ("properties", json::Value::Obj(props)) => all = props.iter().map(|(n, _)| n.clone()).collect(),
            _ => {}
        }
    }
    (required, all)
}

fn tools_text(manifests: &[ServiceManifest], only: Option<&str>) -> String {
    let mut out = String::new();
    // the built-in services are long and the same everywhere: shown when named
    for m in manifests.iter().filter(|m| only.map_or(!is_built_in(&m.id), |s| s == m.id)) {
        out.push_str(&format!("## {} — {}\n", m.id, cut(m.brief.trim(), 300)));
        for t in &m.tools {
            let (required, all) = schema_args(&t.parameters);
            let args: Vec<String> = all.iter().map(|a| if required.contains(a) { format!("{a}*") } else { a.clone() }).collect();
            out.push_str(&format!("{}.{} [{}] {} args: {}\n", m.id, t.name, risk_name(t.risk), cut(t.description.trim(), 200), if args.is_empty() { "none".into() } else { args.join(", ") }));
        }
    }
    if only.is_none() {
        out.push_str("(every app also has the services ui, edit, logs, inspect: app_tools with service=ui lists them; the ui tool runs them by command line)\n");
    }
    if out.trim().is_empty() {
        out = match only {
            Some(s) => format!("no service `{s}` there"),
            None => "no AI tools there".into(),
        };
    }
    out
}

fn app_tools(target: &str, service: Option<&str>) -> Outcome {
    let lines = match client(target).and_then(|c| app_request(&c, &AppRequest::List, &mut |_| {})) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let manifests = manifests_of(&lines);
    let text = tools_text(&manifests, service);
    let n: usize = manifests.iter().map(|m| m.tools.len()).sum();
    reach().lock().unwrap().manifests.insert(target.to_string(), manifests);
    Outcome { ok: true, text, summary: format!("{n} tools"), diff: None }
}

/// The declared risk of `service.tool` on `target` (listing the target when it is not known yet).
fn declared_risk(target: &str, service: &str, tool: &str) -> Option<Risk> {
    let known = reach().lock().unwrap().manifests.get(target).cloned();
    let manifests = match known {
        Some(m) => m,
        None => {
            let lines = client(target).and_then(|c| app_request(&c, &AppRequest::List, &mut |_| {})).ok()?;
            let m = manifests_of(&lines);
            reach().lock().unwrap().manifests.insert(target.to_string(), m.clone());
            m
        }
    };
    manifests.iter().find(|m| m.id == service)?.tool(tool).map(|t| t.risk)
}

fn call(target: &str, service: &str, tool: &str, instance: Option<&str>, args: String, progress: &mut dyn FnMut(&str)) -> Outcome {
    let request = AppRequest::Call { service: service.to_string(), tool: tool.to_string(), instance: instance.map(str::to_string), args, caller: None };
    let lines = match client(target).and_then(|c| app_request(&c, &request, progress)) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let Some(result) = lines.into_iter().find_map(|l| match l {
        AppLine::Result(r) => Some(r),
        _ => None,
    }) else {
        return err("the app ended the call without a result");
    };
    let mut text = result.text.clone();
    if !result.data.is_empty() {
        text.push_str(&format!("\ndata: {}", result.data));
    }
    let ok = result.outcome == ToolOutcome::Ok;
    let summary = if result.note.is_empty() { result.outcome.slug().to_string() } else { cut(&result.note, 100) };
    if !ok {
        text = format!("{}: {text}", result.outcome.slug());
    }
    Outcome { ok, text, summary, diff: None }
}

/// The `ui` words that end or close the app: asked first.
fn ui_needs_approval(command: &str) -> bool {
    matches!(command.split_whitespace().next(), Some("quit" | "close" | "gq"))
}

fn ui(target: &str, command: &str) -> Outcome {
    let c = match client(target) {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let mut lines = Vec::new();
    let r = makepad_remote::app::run_line(&c, command, &mut |l| {
        lines.push(l.to_string());
        true
    });
    match r {
        Ok(had_err) => {
            let text = lines.join("\n");
            let summary = cut(lines.first().map(String::as_str).unwrap_or(""), 100);
            Outcome { ok: !had_err, text, summary, diff: None }
        }
        Err(e) => err(e.to_string()),
    }
}

/// The window managers on `host`: the endpoints whose `app` service offers `os`.
fn window_managers(hub: &Client, host: &str) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    hub_ask(hub, host, "list", "", "list", &mut |l| {
        if let Some(a) = discovery::parse(l) {
            if a.serves("app") {
                found.push(format!("{host}:{}", a.pid));
            }
        }
        true
    })?;
    Ok(found
        .into_iter()
        .filter(|t| app_request(&Client::routed(hub, t), &AppRequest::List, &mut |_| {}).is_ok_and(|l| manifests_of(&l).iter().any(|m| m.id == "os")))
        .collect())
}

fn launch(host: Option<&str>, app: &str, wm: Option<&str>) -> Outcome {
    let wm = match (wm, host.filter(|h| *h != "local")) {
        (Some(wm), _) => Some(wm.to_string()),
        (None, None) => return err("to start an app of this workspace here use run_app"),
        (None, Some(host)) => {
            let hub = match hub() {
                Ok(h) => h,
                Err(e) => return err(e),
            };
            match window_managers(&hub, host) {
                Ok(wms) if wms.len() == 1 => wms.into_iter().next(),
                Ok(wms) if wms.len() > 1 => return err(format!("several window managers run on {host}: {}; say which with `wm`", wms.join(", "))),
                Ok(_) => None,
                Err(e) => return err(e),
            }
        }
    };
    // a window manager: the app comes up as its tile
    if let Some(wm) = wm {
        return call(&wm, "os", "launch", None, json::obj(vec![("app", json::s(app))]).to_json(), &mut |_| {});
    }
    let host = host.unwrap_or_default();
    let hub = match hub() {
        Ok(h) => h,
        Err(e) => return err(e),
    };
    let mut lines = Vec::new();
    match hub_ask(&hub, host, "session", "", &format!("launch {app}"), &mut |l| {
        lines.push(l.to_string());
        true
    }) {
        Ok(had_err) => Outcome { ok: !had_err, summary: cut(lines.first().map(String::as_str).unwrap_or(""), 100), text: lines.join("\n"), diff: None },
        Err(e) => err(e),
    }
}

/// This machine's window manager, when one runs: the pid of the WM process whose `app` service
/// offers `os` (its launcher: `os.list`, `os.launch`).
fn local_wm() -> Option<String> {
    let me = std::process::id();
    discovery::apps()
        .into_iter()
        .filter(|a| a.pid != me && a.serves("app") && a.app.contains("wm"))
        .map(|a| a.pid.to_string())
        .find(|t| client(t).and_then(|c| app_request(&c, &AppRequest::List, &mut |_| {})).is_ok_and(|l| manifests_of(&l).iter().any(|m| m.id == "os")))
}

/// The pid of the app `app_id` as a client of the window manager `wm` (its hosted instance),
/// once it has connected.
fn wm_client_pid(wm: &str, app_id: &str) -> Option<u32> {
    let lines = client(wm).and_then(|c| app_request(&c, &AppRequest::List, &mut |_| {})).ok()?;
    lines.iter().find_map(|l| match l {
        AppLine::Instance { meta, .. } if meta.app_id == app_id && meta.pid != 0 => Some(meta.pid),
        _ => None,
    })
}

/// The running processes of `entry` (by the crate name their endpoint announces).
fn pids_of(entry: &makepad_app_registry::AppEntry) -> Vec<u32> {
    let norm = |s: &str| s.to_ascii_lowercase().replace('_', "-");
    discovery::apps().into_iter().filter(|a| [entry.bin, entry.package].iter().any(|n| norm(n) == norm(&a.app))).map(|a| a.pid).collect()
}

/// The lines an app logged as errors (`[E]`, panics) so far, read from its log pool over its
/// endpoint; at most 12, repeats folded.
fn logged_errors(pid: u32) -> Option<Vec<String>> {
    let c = client(&pid.to_string()).ok()?;
    let mut text = String::new();
    makepad_remote::app::run_line(&c, "log n=400", &mut |l| {
        text.push_str(l);
        text.push('\n');
        true
    })
    .ok()?;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(json::Value::Obj(fields)) = json::parse(line.as_bytes()) else { continue };
        for (k, v) in fields {
            if let ("l", json::Value::Arr(items)) = (k.as_str(), v) {
                for l in items.iter().filter_map(|i| i.as_str()) {
                    let err = l.starts_with("[E]") || l.contains("panicked at");
                    if err && !out.iter().any(|o| o == l) {
                        out.push(l.to_string());
                    }
                }
            }
        }
    }
    out.truncate(12);
    Some(out)
}

/// Let a fresh app run its startup, then add the errors it logged (or that it logged none).
fn add_startup_errors(text: &mut String, pid: u32, fallback: impl FnOnce() -> Vec<String>) {
    std::thread::sleep(Duration::from_secs(3));
    let errors = logged_errors(pid).unwrap_or_else(|| fallback().into_iter().filter(|l| l.contains("[E]") || l.contains("panicked at")).take(12).collect());
    if errors.is_empty() {
        text.push_str("\nno errors logged at startup\n");
    } else {
        text.push_str("\nERRORS it logged at startup (Splash mistakes compile fine and only show here):\n");
        for e in errors {
            text.push_str(&cut(&e, 400));
            text.push('\n');
        }
    }
}

fn attach(target: &str, service: &str, instance: Option<&str>) -> Outcome {
    let lines = match client(target).and_then(|c| app_request(&c, &AppRequest::List, &mut |_| {})) {
        Ok(l) => l,
        Err(e) => return err(e),
    };
    let Some(manifest) = manifests_of(&lines).into_iter().find(|m| m.id == service) else {
        return err(format!("{target} has no service `{service}` (see app_tools)"));
    };
    let mut list = attached().lock().unwrap();
    list.retain(|a| a.manifest.id != service);
    if list.len() >= MAX_ATTACHED {
        let names: Vec<&str> = list.iter().map(|a| a.manifest.id.as_str()).collect();
        return err(format!("{MAX_ATTACHED} services are attached already ({}); detach one first", names.join(", ")));
    }
    let mut text = format!("attached {service} on {target}; call these directly:\n");
    for t in &manifest.tools {
        let (required, all) = schema_args(&t.parameters);
        let args: Vec<String> = all.iter().map(|a| if required.contains(a) { format!("{a}*") } else { a.clone() }).collect();
        text.push_str(&format!("{} [{}] {} args: {}\n", makepad_ai_services::wire::api_name(service, &t.name), risk_name(t.risk), cut(t.description.trim(), 160), if args.is_empty() { "none".into() } else { args.join(", ") }));
    }
    let n = manifest.tools.len();
    list.push(Attached { target: target.to_string(), instance: instance.map(str::to_string), manifest });
    Outcome { ok: true, text, summary: format!("{n} tools attached"), diff: None }
}

impl Engine {
    /// A call of an app tool after its approval by declared risk.
    fn gated_call(&mut self, id: ToolId, target: &str, service: &str, tool: &str, instance: Option<&str>, call_args: String) -> Outcome {
        let risk = declared_risk(target, service, tool);
        if matches!(risk, Some(Risk::Request | Risk::Destructive)) {
            let prompt = format!("app: {target} {service}.{tool} {}\n({})", cut(&call_args, 300), risk_name(risk.unwrap()));
            if let Verdict::No = self.verdict(id, prompt, format!("app:{target}:{service}.{tool}"), false) {
                return err("the user denied this call");
            }
        }
        call(target, service, tool, instance, call_args, &mut |_| {})
    }

    /// One of [`NAMES`] or an attached tool: gated by its risk, then run.
    pub(super) fn remote_tool(&mut self, id: ToolId, name: &str, args: &[(String, String)]) -> Outcome {
        if let Some((a, tool)) = attached_tool(name) {
            let schema = a.manifest.tool(&tool).map(|t| t.parameters.clone()).unwrap_or_default();
            let call_args = args_object(&schema, args);
            return self.gated_call(id, &a.target, &a.manifest.id, &tool, a.instance.as_deref(), call_args);
        }
        if super::runs::NAMES.contains(&name) {
            return self.runs_tool(id, name, args);
        }
        let target = arg(args, "target").unwrap_or("");
        let needs_target = !matches!(name, "apps" | "launch" | "detach");
        if needs_target && target.is_empty() {
            return err("missing 'target' (see apps)");
        }
        match name {
            "apps" => list_apps(),
            "app_tools" => app_tools(target, arg(args, "service")),
            "app_call" => {
                let Some((service, tool)) = arg(args, "tool").and_then(|t| t.split_once('.')) else {
                    return err("'tool' is service.tool, e.g. calculator.eval");
                };
                let call_args = arg(args, "args").unwrap_or("{}").to_string();
                if !matches!(json::parse(call_args.as_bytes()), Ok(json::Value::Obj(_))) {
                    return err("'args' must be a JSON object");
                }
                self.gated_call(id, target, service, tool, arg(args, "instance"), call_args)
            }
            "ui" => {
                let Some(command) = arg(args, "command") else { return err("missing 'command'") };
                if ui_needs_approval(command) {
                    if let Verdict::No = self.verdict(id, format!("ui: {target} {command}"), format!("ui:{target}:{command}"), false) {
                        return err("the user denied this command");
                    }
                }
                ui(target, command)
            }
            "attach" => match arg(args, "service") {
                Some(service) => attach(target, service, arg(args, "instance")),
                None => err("missing 'service'"),
            },
            "detach" => {
                let service = arg(args, "service").unwrap_or("");
                let mut list = attached().lock().unwrap();
                let before = list.len();
                list.retain(|a| a.manifest.id != service);
                if list.len() == before {
                    err(format!("`{service}` is not attached"))
                } else {
                    Outcome { ok: true, text: format!("detached {service}"), summary: "detached".into(), diff: None }
                }
            }
            "launch" => match arg(args, "app") {
                Some(app) => launch(arg(args, "host"), app, arg(args, "wm")),
                None => err("missing 'app'"),
            },
            other => err(format!("not a remote tool: {other}")),
        }
    }
}

impl Engine {
    /// The workspace's build and run tools (runs.rs): reading ones run, the rest ask first unless
    /// approvals are off.
    fn runs_tool(&mut self, id: ToolId, name: &str, args: &[(String, String)]) -> Outcome {
        use super::runs::{Goal, JobState, Mode, RunManager, Started};
        let m = RunManager::global();
        let wm = if matches!(name, "list_apps" | "start_app") { local_wm() } else { None };
        if !m.set_root(self.root()) && wm.is_none() {
            return err("no makepad checkout here or on this machine, and no window manager runs: there are no apps to build or start");
        }
        let wait = Duration::from_secs(arg(args, "timeout_secs").and_then(|t| t.parse().ok()).unwrap_or(1200));
        let app_name = arg(args, "app");
        if name == "list_apps" {
            let mut text = m.apps_text();
            let n = text.lines().count();
            if let Some(wm) = &wm {
                let listed = call(wm, "os", "list", None, "{}".into(), &mut |_| {});
                text.push_str(&format!("\nThe window manager (pid {wm}) lists:\n{}\nStart any of them with start_app.\n", listed.text));
            }
            return Outcome { ok: true, summary: format!("{n} apps"), text, diff: None };
        }
        if name == "start_app" {
            let Some(app_name) = app_name else { return err("missing 'app' (see list_apps)") };
            let all: Vec<&'static makepad_app_registry::AppEntry> = makepad_app_registry::apps().iter().collect();
            let Some(entry) = super::runs::find_entry(&all, app_name) else {
                let known: Vec<&str> = all.iter().filter(|a| a.menu_visible).map(|a| a.label).collect();
                return err(format!("no app `{app_name}`; the apps are: {}", known.join(", ")));
            };
            let Some(wm) = wm else {
                // no window manager: build it from the checkout and start it, as run_app does
                let args = vec![("app".to_string(), entry.id.to_string())];
                return self.runs_tool(id, "run_app", &args);
            };
            if let Verdict::No = self.verdict(id, format!("start {}", entry.label), format!("runs:start_app:{}", entry.id), false) {
                return err("the user denied this");
            }
            let before = pids_of(entry);
            let launched = call(&wm, "os", "launch", None, json::obj(vec![("app", json::s(entry.id))]).to_json(), &mut |_| {});
            if !launched.ok {
                return launched;
            }
            let mut text = format!("{} ({}) launched by the window manager (pid {wm}): {}\n", entry.label, entry.id, launched.text.trim());
            if launched.text.contains("already running") {
                // it was up before: the WM brought it to the front
                let mine = reach().lock().unwrap().started.get(entry.id).copied().filter(|p| before.contains(p));
                match wm_client_pid(&wm, entry.id).or(mine).map(|p| vec![p]).unwrap_or(before) {
                    pids if pids.len() == 1 => text.push_str(&format!("reach it with ui / app_tools target={}\n", pids[0])),
                    pids if pids.is_empty() => text.push_str("`apps` shows its pid\n"),
                    pids => text.push_str(&format!("processes of it on this machine: {}; `apps` shows which is which\n", pids.iter().map(u32::to_string).collect::<Vec<_>>().join(", "))),
                }
                return Outcome { ok: true, summary: format!("{} already running", entry.label), text, diff: None };
            }
            let wait = Duration::from_secs(arg(args, "timeout_secs").and_then(|t| t.parse().ok()).unwrap_or(600));
            let started = std::time::Instant::now();
            // Its pid: the WM's own client of it, else a process of it that was not there before
            // (other copies of the app may run on this machine outside this WM).
            let pid = loop {
                if let Some(pid) = wm_client_pid(&wm, entry.id).or_else(|| pids_of(entry).into_iter().find(|p| !before.contains(p))) {
                    break Some(pid);
                }
                if started.elapsed() > wait || self.interrupted {
                    break None;
                }
                self.pump_commands();
                std::thread::sleep(Duration::from_millis(500));
            };
            let Some(pid) = pid else {
                text.push_str(&format!("it is not up after {} s (a first start builds it); `apps` shows it once it runs\n", started.elapsed().as_secs()));
                return Outcome { ok: true, summary: format!("{} starting", entry.label), text, diff: None };
            };
            reach().lock().unwrap().started.insert(entry.id.to_string(), pid);
            text.push_str(&format!("{} runs as pid {pid}: reach it with ui / app_tools target={pid}\n", entry.label));
            add_startup_errors(&mut text, pid, Vec::new);
            return Outcome { ok: true, summary: format!("{} pid {pid}", entry.label), text, diff: None };
        }
        if name == "app_status" {
            let app = app_name.and_then(|n| m.find_app(n));
            if app_name.is_some() && app.is_none() {
                return err(format!("no app `{}` here (see list_apps)", app_name.unwrap_or("")));
            }
            let text = m.status_text(app);
            return Outcome { ok: true, summary: cut(text.lines().next().unwrap_or(""), 100), text, diff: None };
        }
        let Some(app_name) = app_name else { return err("missing 'app' (see list_apps)") };
        let snap = m.snapshot();
        // An app by id, or a run by pid.
        let pid: Option<u32> = app_name.parse().ok();
        let by_pid = pid.and_then(|p| snap.running.iter().rev().find(|r| r.pid == p).cloned());
        let app = match (&by_pid, m.find_app(app_name)) {
            (Some(r), _) => r.app,
            (None, Some(a)) => a,
            (None, None) => return err(format!("no app `{app_name}` here (see list_apps)")),
        };
        let run = by_pid.or_else(|| snap.running.iter().rev().find(|r| r.app.id == app.id).cloned());
        match name {
            "app_logs" => {
                let n = arg(args, "lines").and_then(|n| n.parse().ok()).unwrap_or(60usize).clamp(1, 400);
                let build = arg(args, "build").is_some_and(|b| b == "true");
                let lines = match (&run, build) {
                    (Some(r), false) => m.run_log(r.id, n),
                    _ => match snap.jobs.iter().rev().find(|j| j.app.id == app.id) {
                        Some(j) => m.job_log(j.id, n),
                        None => return err(format!("{} has no build or run yet", app.id)),
                    },
                };
                let text = if lines.is_empty() { "(no output yet)".to_string() } else { lines.join("\n") };
                Outcome { ok: true, summary: format!("{} lines", lines.len()), text, diff: None }
            }
            "stop_app" => {
                if let Verdict::No = self.verdict(id, format!("stop {}", app.id), format!("runs:stop:{}", app.id), false) {
                    return err("the user denied this");
                }
                let mut done = Vec::new();
                for j in snap.jobs.iter().filter(|j| j.app.id == app.id && !j.state.is_done()) {
                    m.cancel(j.id);
                    done.push(format!("cancelled the build of {}", j.target));
                }
                if let Some(r) = run.filter(|r| r.exited.is_none()) {
                    match m.stop(r.id) {
                        Ok(()) => done.push(format!("stopped {} pid {}", app.id, r.pid)),
                        Err(e) => return err(e),
                    }
                }
                if done.is_empty() {
                    return err(format!("{} is not building or running", app.id));
                }
                let text = done.join("\n");
                Outcome { ok: true, summary: cut(&text, 100), text, diff: None }
            }
            "build_app" | "run_app" => {
                let mode = match arg(args, "mode").map(Mode::parse).unwrap_or(Some(Mode::Native)) {
                    Some(mode) => mode,
                    None => return err("mode is native or makepad_run"),
                };
                let what = if name == "build_app" { format!("build {}", app.id) } else { format!("run {} ({})", app.id, mode.name()) };
                if let Verdict::No = self.verdict(id, what.clone(), format!("runs:{name}:{}", app.id), false) {
                    return err("the user denied this");
                }
                let started = if name == "build_app" { m.build(app).map(Started::Job) } else { m.run(app, mode) };
                let job = match started {
                    Ok(Started::Job(job)) => job,
                    Ok(Started::Running(rid)) => {
                        // under `makepad run` cargo first builds the tool: wait for it like a build
                        let started = std::time::Instant::now();
                        let r = loop {
                            let r = m.snapshot().running.into_iter().find(|r| r.id == rid);
                            let busy = r.as_ref().is_some_and(|r| r.exited.is_none() && r.building.is_some());
                            if !busy || started.elapsed() > wait || self.interrupted {
                                break r;
                            }
                            self.pump_commands();
                            std::thread::sleep(Duration::from_millis(500));
                        };
                        let mut text = r.as_ref().map(RunManager::running_text).unwrap_or_default();
                        if let Some(r) = r.as_ref().filter(|r| r.exited.is_none() && r.building.is_none()) {
                            add_startup_errors(&mut text, r.pid, || m.run_log(r.id, 200));
                        }
                        return Outcome { ok: true, summary: cut(&text, 100), text, diff: None };
                    }
                    Err(e) => return err(e),
                };
                let Some(view) = m.wait_job(job, wait) else { return err("the build vanished") };
                let mut text = m.job_text(&view, 30);
                let ok = match &view.state {
                    JobState::Succeeded => true,
                    JobState::Failed(_) | JobState::Cancelled => false,
                    _ => {
                        text.push_str("still building: check app_status later\n");
                        return Outcome { ok: true, summary: cut(&view.status_line(), 100), text, diff: None };
                    }
                };
                if ok && matches!(view.goal, Goal::Run(_)) {
                    // the app was started right after its build
                    match m.running_of(app) {
                        Some(r) => {
                            text.push_str(&RunManager::running_text(&r));
                            add_startup_errors(&mut text, r.pid, || m.run_log(r.id, 200));
                        }
                        None => {
                            text.push_str("but it did not start:\n");
                            text.push_str(&m.job_log(job, 5).join("\n"));
                            return Outcome { ok: false, summary: "did not start".into(), text, diff: None };
                        }
                    }
                }
                Outcome { ok, summary: cut(&view.status_line(), 100), text, diff: None }
            }
            other => err(format!("not a run tool: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_services::wire::{EndpointId, InstanceMeta, ToolDef};

    fn calc() -> ServiceManifest {
        ServiceManifest::new("calculator", "Calculator", "Evaluates.").with_tool(ToolDef::new(
            "eval",
            "Evaluate an expression.",
            r#"{"type":"object","properties":{"expression":{"type":"string"},"angle":{"type":"string"}},"required":["expression"]}"#,
            Risk::Read,
        ))
    }

    #[test]
    fn manifests_read_as_compact_text() {
        let text = tools_text(&[calc()], None);
        assert!(text.contains("calculator.eval [read] Evaluate an expression. args: expression*, angle"), "{text}");
        assert_eq!(tools_text(&[calc()], Some("os")), "no service `os` there");
    }

    #[test]
    fn a_wm_listing_names_its_hosted_instances() {
        let meta = InstanceMeta { app_id: "calculator".into(), pid: 4242, ..Default::default() };
        let lines = vec![
            AppLine::Instance { endpoint: EndpointId("w4".into()), meta, manifest: calc() },
            AppLine::Launchable { id: "notes".into(), label: "Notes".into() },
        ];
        assert_eq!(services_line(&lines), "AI: calculator (1 tools, instance w4 pid 4242); can launch: notes");
    }

    #[test]
    fn named_arguments_become_the_schema_s_json() {
        let schema = r#"{"type":"object","properties":{"expression":{"type":"string"},"n":{"type":"integer"},"opts":{"type":"object"}}}"#;
        let args = vec![("expression".to_string(), "7+8".to_string()), ("n".to_string(), "3".to_string()), ("opts".to_string(), "{\"a\":1}".to_string())];
        assert_eq!(args_object(schema, &args), r#"{"expression":"7+8","n":3,"opts":{"a":1}}"#);
        // a string property keeps a value that looks like a number as text
        let args = vec![("expression".to_string(), "15".to_string())];
        assert_eq!(args_object(schema, &args), r#"{"expression":"15"}"#);
    }

    #[test]
    fn only_closing_ui_words_ask() {
        assert!(ui_needs_approval("quit"));
        assert!(ui_needs_approval("close w=1"));
        assert!(!ui_needs_approval("click x=1 y=2"));
        assert!(!ui_needs_approval("dump"));
    }
}
