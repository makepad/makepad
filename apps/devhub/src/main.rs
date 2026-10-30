//! devhub, one app: a **node** when the WM names its conf (`MAKEPAD_DEVHUB_NODE_CONF`, the way it
//! starts the AI Hub), the **master** otherwise.
//!
//! - Master (the user's Mac): a menu-bar icon; a click opens the panel under it (the inbox and the
//!   explorer: machines → JITs → inspect views). It listens on nothing but its control socket and
//!   dials the nodes with the Mac's fleet identity.
//! - Node (a Linux WM): a tray icon in the WM's bar with a status menu; it serves this machine's
//!   JITs to the master. Quit writes `enabled = off` into its conf, so the WM leaves it off until
//!   the person starts it from the launcher.
//! - Node (Windows): the same tray, in the notification area, started at logon by the per-user
//!   task `tools/devhub/windows/install-node.ps1` makes (through `cargo run`, under a headless
//!   console: no window); its conf is `%USERPROFILE%\.makepad\devhub\node.conf` unless the env
//!   names one.
//!
//! Without the fleet identity (`~/.makepad/fleet`, tools/fleet-identity.md) both serve this
//! machine's JITs only. `makepad devhub serve [--node <conf>]` runs the same core without a
//! window or tray (tests, ssh).

pub use makepad_widgets;

use makepad_devhub::tray_glyph as glyph;
mod graph;
mod installed;
mod live;
mod taskview;
mod view;

use makepad_devhub::explorer::{self, Snapshot};
use makepad_devhub::master::Master;
use makepad_devhub::node::{Node, NodeConf, NODE_CONF_ENV};
use makepad_devhub::trust::{FleetTrust, Trust};
use makepad_devhub::lease::sched::{ordinal, Lease, State};
use makepad_devhub::proto;
use makepad_widgets::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use live::{Sampler, Watch};
use makepad_shell_kit::tray_app::{DropdownChange, TrayDropdown};
use taskview::{DevhubTasks, Head, TasksAction};
use view::{Confirm, DevhubAction, DevhubView, EventRow, InboxRow, JitDo, LeaseRow, Tab, ViewState};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                create_on_start: false
                window.title: "devhub"
                window.inner_size: vec2(620, 820)
                body +: {
                    hub_view := mod.widgets.DevhubView{}
                }
            }
            dropdown_window := Window{
                create_on_start: false
                show_caption_bar: false
                chrome: WindowChrome.None
                window.title: "devhub"
                window.dropdown: true
                window.inner_size: vec2(620, 720)
                body +: {
                    drop_view := mod.widgets.DevhubView{card: true}
                }
            }
            // a node's tray dropdown: the task-manager view of this machine's JITs
            node_window := Window{
                create_on_start: false
                show_caption_bar: false
                chrome: WindowChrome.None
                window.title: "devhub"
                window.dropdown: true
                window.inner_size: vec2(560, 640)
                body +: {
                    tasks := mod.widgets.DevhubTasks{}
                }
            }
        }
    }
}

const TRAY: LiveId = live_id!(devhub_tray);
const QUIT: LiveId = live_id!(quit_devhub);
const SEEN: LiveId = live_id!(seen_all);
const DEVSW: LiveId = live_id!(dev_switch);
const OPEN: LiveId = live_id!(open_window);
const LOGIN: LiveId = live_id!(open_at_login);
/// the master's tray menu: node `i`'s dev switch is item `DEV_BASE + i`
const DEV_BASE: u64 = 0x6465_7600_0000;
const DROPDOWN: Vec2d = Vec2d { x: 620.0, y: 720.0 };
const NODE_DROPDOWN_W: f64 = 560.0;
/// the live view's redraw rate while it shows
const LIVE_HZ: f64 = 30.0;

fn log_line(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

fn trust(name: &str) -> Option<Arc<dyn Trust>> {
    match FleetTrust::load(name) {
        Ok(t) => Some(Arc::new(t)),
        Err(e) => {
            log_line(&format!("devhub: {e}: this machine's JITs only"));
            None
        }
    }
}

/// A scheduler lease as the panel's row.
fn lease_row(l: &Lease, now: u64) -> LeaseRow {
    let info = makepad_strict_json::parse(l.info.as_bytes()).ok();
    let info_s = |k: &str| info.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let waiting = l.state == State::Queued;
    LeaseRow {
        id: l.id.clone(),
        node: l.node.clone().or_else(|| l.req.node.clone()).unwrap_or_default(),
        lane: l.req.lane.clone(),
        agent: l.req.agent.clone().unwrap_or_default(),
        kind: l.req.kind.as_str().to_string(),
        waiting,
        since_s: now.saturating_sub(if waiting { l.asked_ms } else { l.granted_ms.unwrap_or(l.asked_ms) }) / 1000,
        beat_s: now.saturating_sub(l.beat_ms) / 1000,
        running: info_s("running"),
        disk_mb: info.as_ref().and_then(|v| v.get("disk_mb")).and_then(makepad_devhub::lease::num),
        work: info_s("work"),
        queue: if waiting { format!("{} in queue ({})", ordinal(l.pos), l.reason) } else { String::new() },
        jit: l.req.jit.clone().map_or(String::new(), |j| format!("jit {j}")),
    }
}

/// What runs behind the UI.
enum Core {
    Master(MasterCore),
    Node(NodeCore),
}

struct MasterCore {
    master: Arc<Master>,
    /// the explorer is collected only while the panel shows (an unwatched fleet costs nothing)
    looking: Arc<AtomicBool>,
    snapshot: Arc<Mutex<Snapshot>>,
    /// (machine, pid, inspect command or `!restart` / `!stop` / `!log`)
    drill: Sender<(String, u32, String)>,
    detail: Arc<Mutex<Vec<String>>>,
    /// the outcome of the last confirmed action
    note: Arc<Mutex<String>>,
    /// a confirmation a background answer prepared (a sized JIT cache clear)
    pending: Arc<Mutex<Option<Confirm>>>,
}

struct NodeCore {
    node: Node,
    conf_path: PathBuf,
    /// the task view's JIT cache clear (answers come from a worker thread)
    cache: Arc<Mutex<taskview::CacheUi>>,
}

impl NodeCore {
    fn lan(&self) -> String {
        match (&self.node.addr, &self.node.lan_off) {
            (Some(a), _) => format!("LAN {a}"),
            (None, Some(w)) => w.clone(),
            (None, None) => "local only".into(),
        }
    }

    /// this machine's JITs (the scan's list: no JIT is asked)
    fn jit_count(&self) -> usize {
        self.node.local.jits().iter().filter(|j| j.run.is_some()).count()
    }
}

/// A node service through the master's router: its answer lines.
fn node_ask(machine: &str, service: &str, line: &str) -> Vec<String> {
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(proto::control_socket()), "devhub-panel");
    let mut out = Vec::new();
    if let Err(e) = makepad_remote::hub::ask(&c, machine, service, "", line, &mut |l| {
        out.push(l.to_string());
        true
    }) {
        out.push(format!("error: {e}"));
    }
    out
}

/// A machine's `jitcache` service (this machine's on the control socket, a node's through the
/// router): its answer lines, an error as an `err` line.
fn jitcache_ask(machine: &str, local: bool, line: &str) -> Vec<String> {
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(proto::control_socket()), "devhub-panel");
    makepad_devhub::cli::ask(&c, (!local).then_some(machine), "jitcache", line).unwrap_or_else(|e| vec![format!("{{\"err\":{}}}", makepad_remote::json_str(&e))])
}

/// The confirmation of a JIT cache clear from its plan: what goes, who runs on it.
fn jitcache_question(machine: &str, app: Option<&str>, plan: &[String]) -> Result<Confirm, String> {
    let (mut bytes, mut caches, mut users, mut unannounced, mut dirs) = (0u64, 0u64, Vec::<String>::new(), false, Vec::<String>::new());
    for l in plan {
        let v = makepad_strict_json::parse(l.as_bytes()).map_err(|_| format!("{machine}: {l}"))?;
        if let Some(e) = v.get("err").and_then(|x| x.as_str()) {
            return Err(format!("{machine}: {e}"));
        }
        if let Some(t) = v.get("total_bytes").and_then(|x| x.as_u64()) {
            bytes = t;
            caches = v.get("caches").and_then(|x| x.as_u64()).unwrap_or(0);
        }
        if let Some(d) = v.get("dir").and_then(|x| x.as_str()) {
            dirs.push(d.to_string());
        }
        if let Some(u) = v.get("users").and_then(|x| x.as_arr()) {
            users.extend(u.iter().filter_map(|x| Some(format!("{}#{}", x.get("app")?.as_str()?.trim_start_matches("makepad-app-"), x.get("pid")?.as_u64()?))));
            unannounced |= u.is_empty() && v.get("held").and_then(|x| x.as_u64()).unwrap_or(0) > 0;
        }
    }
    if caches == 0 {
        return Err(format!("{machine}: no JIT cache{} here", app.map_or(String::new(), |a| format!(" of {a}"))));
    }
    if unannounced {
        users.push("a makepad check/test".into());
    }
    let what = app.map_or(format!("{machine}'s JIT cache"), |a| format!("{a}'s JIT cache on {machine}"));
    let busy = if users.is_empty() { String::new() } else { format!(" Running on it (keeps running): {}.", users.join(", ")) };
    Ok(Confirm::ClearJit {
        machine: machine.to_string(),
        app: app.map(str::to_string),
        question: format!(
            "Clear {what}? Frees {}; the next start is cold.{busy} Removes {}only: {}. Keeps ~/.cargo, every target/ build, downloaded packages and sources.",
            makepad_devhub::jitcache::human(bytes),
            if app.is_some() { "that app's files in " } else { "" },
            dirs.iter().map(|d| format!("{d}/{{{}}}", makepad_devhub::jitcache::CLEARED.join(","))).collect::<Vec<_>>().join(", ")
        ),
        force: !users.is_empty(),
    })
}

/// The node's own JIT cache clear from its task view: sized, confirmed, cleared, on a worker
/// thread (a walk of the cache), through the node's own control socket.
fn node_clear_jit(n: &NodeCore, a: TasksAction) {
    let (cache, machine) = (n.cache.clone(), n.node.conf.name.clone());
    let line = match a {
        TasksAction::ClearJitCancel => {
            let mut c = cache.lock().unwrap();
            c.ask = None;
            c.note.clear();
            return;
        }
        TasksAction::ClearJit => "jitcache plan".to_string(),
        TasksAction::ClearJitConfirmed(force) => format!("jitcache clear{}", if force { " force" } else { "" }),
        _ => return,
    };
    {
        let mut c = cache.lock().unwrap();
        c.note = if c.ask.take().is_some() { "clearing\u{2026}".into() } else { "sizing\u{2026}".into() };
    }
    std::thread::spawn(move || {
        let out = jitcache_ask(&machine, true, &line);
        let mut c = cache.lock().unwrap();
        if line == "jitcache plan" {
            match jitcache_question(&machine, None, &out) {
                Ok(Confirm::ClearJit { question, force, .. }) => {
                    c.ask = Some((question, force));
                    c.note.clear();
                }
                Ok(_) => {}
                Err(e) => c.note = e,
            }
            return;
        }
        let get = |k: &str| out.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get(k)?.as_str().map(str::to_string));
        c.note = get("err").or_else(|| get("line")).unwrap_or_else(|| "no answer".into());
    });
}

/// Sizes a machine's JIT cache (a walk of it, on the machine: never on the UI thread), then
/// leaves the confirmation for the panel.
fn size_jit_cache(m: &MasterCore, machine: String, app: Option<String>) {
    let (note, pending, local) = (m.note.clone(), m.pending.clone(), machine == m.master.name);
    *note.lock().unwrap() = format!("sizing the JIT cache on {machine}\u{2026}");
    std::thread::spawn(move || {
        let line = format!("jitcache plan{}", app.as_deref().map_or(String::new(), |a| format!(" app={a}")));
        match jitcache_question(&machine, app.as_deref(), &jitcache_ask(&machine, local, &line)) {
            Ok(c) => {
                note.lock().unwrap().clear();
                *pending.lock().unwrap() = Some(c);
            }
            Err(e) => *note.lock().unwrap() = e,
        }
    });
}

fn start_master() -> Result<MasterCore, String> {
    let name = std::env::var("MAKEPAD_DEVHUB_NAME").ok().filter(|n| makepad_devhub::trust::valid_name(n)).unwrap_or_else(|| "mac".into());
    let master = Master::start(&name, proto::HITCH_MS, trust(&name), Arc::new(|_r| {}))?;
    log_line(&format!("devhub: master {name} up; control {}", proto::control_socket().display()));
    let looking = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let (m, l, s) = (master.clone(), looking.clone(), snapshot.clone());
    let _ = std::thread::Builder::new().name("devhub-explorer".into()).spawn(move || loop {
        if l.load(Ordering::Relaxed) {
            let snap = explorer::collect(&m);
            *s.lock().unwrap() = snap;
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    });
    let (drill, rx) = channel::<(String, u32, String)>();
    let detail = Arc::new(Mutex::new(Vec::new()));
    let (m, d) = (master.clone(), detail.clone());
    let _ = std::thread::Builder::new().name("devhub-drill".into()).spawn(move || {
        while let Ok((machine, pid, cmd)) = rx.recv() {
            *d.lock().unwrap() = vec![format!("asking {machine}/{pid}: {cmd} \u{2026}")];
            let local = machine == m.name;
            let lines = match cmd.as_str() {
                "!restart" => explorer::drill(&m, &machine, pid, "session relaunch"),
                "!stop" if local => vec!["stop: this Mac's own JITs are not stopped from the panel".into()],
                "!stop" => node_ask(&machine, "launch", &format!("stop {pid}")),
                "!log" if local => vec!["log: open it with `makepad inspect` on this Mac".into()],
                "!log" => node_ask(&machine, "jitlog", &format!("jitlog {pid} 40"))
                    .into_iter()
                    .map(|l| makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("line").or(v.get("log")).or(v.get("err")).and_then(|x| x.as_str()).map(str::to_string)).unwrap_or(l))
                    .collect(),
                c => explorer::drill(&m, &machine, pid, c),
            };
            *d.lock().unwrap() = lines;
        }
    });
    Ok(MasterCore { master, looking, snapshot, drill, detail, note: Arc::new(Mutex::new(String::new())), pending: Arc::new(Mutex::new(None)) })
}

fn start_node(conf_path: PathBuf) -> Result<Option<NodeCore>, String> {
    let conf = NodeConf::load(&conf_path);
    if !conf.enabled {
        log_line("devhub: node is off in its conf");
        return Ok(None);
    }
    let t = trust(&conf.name);
    let node = Node::start(conf, t, Arc::new(|l: &str| log_line(l)))?;
    log_line(&format!(
        "devhub: node {} up; control {}; {}",
        node.conf.name,
        proto::control_socket().display(),
        match (&node.addr, &node.lan_off) {
            (Some(a), _) => format!("LAN {a}"),
            (None, Some(why)) => why.clone(),
            (None, None) => "local only".into(),
        }
    ));
    Ok(Some(NodeCore { node, conf_path, cache: Arc::new(Mutex::new(Default::default())) }))
}

fn start_core() -> Result<Option<Core>, String> {
    match std::env::var_os(NODE_CONF_ENV).filter(|p| !p.is_empty()) {
        Some(p) => Ok(start_node(PathBuf::from(p))?.map(Core::Node)),
        // the master is the Mac's: on Windows devhub is always a node (its logon task's tray)
        None if cfg!(windows) => Ok(start_node(proto::home().join("node.conf"))?.map(Core::Node)),
        None => Ok(Some(Core::Master(start_master()?))),
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    core: Option<Core>,
    #[rust]
    timer: Timer,
    /// the live view's redraws (only while it shows)
    #[rust]
    live_timer: Timer,
    /// the live graphs' sampler (parked while nothing shows them)
    #[rust]
    sampler: Option<Sampler>,
    /// a node's tray dropdown
    #[rust(TrayDropdown::new("devhub", TRAY, NODE_DROPDOWN_W))]
    node_drop: TrayDropdown,
    #[rust]
    live_on: bool,
    #[rust]
    state: ViewState,
    #[rust]
    open: bool,
    #[rust]
    window_shown: bool,
    #[rust]
    dismissed: Option<std::time::Instant>,
    #[rust]
    tray_key: String,
    /// the node names behind the tray menu's dev switches, in menu order
    #[rust]
    dev_nodes: Vec<String>,
    /// "Open at login": None = not offered (not an installed bundle)
    #[rust]
    login: Option<bool>,
}

impl App {
    fn status(&self) -> (glyph::Glyph, String) {
        match &self.core {
            Some(Core::Master(m)) => {
                let unseen = m.master.inbox.lock().unwrap().unseen();
                let net = makepad_devhub::trust::FleetTrust::load(&m.master.name).is_ok();
                let g = if !net { glyph::Glyph::Off } else if unseen > 0 { glyph::Glyph::Attention } else { glyph::Glyph::Quiet };
                let nodes = m.master.hosts().hosts.len();
                (g, format!("master {} \u{b7} {nodes} nodes \u{b7} {unseen} new{}", m.master.name, if net { "" } else { " \u{b7} no fleet identity" }))
            }
            Some(Core::Node(n)) => (if n.node.addr.is_some() { glyph::Glyph::Quiet } else { glyph::Glyph::Off }, format!("node {} \u{b7} {} JITs \u{b7} {}", n.node.conf.name, n.jit_count(), n.lan())),
            None => (glyph::Glyph::Off, "off".into()),
        }
    }

    fn refresh(&mut self, cx: &mut Cx) {
        let (g, status) = self.status();
        let nodes: Vec<(String, bool)> = match &self.core {
            Some(Core::Master(m)) => m.master.leases.nodes().into_iter().map(|n| (n.name, n.dev)).collect(),
            _ => Vec::new(),
        };
        let key = format!("{g:?}|{status}|{:?}|{nodes:?}|{}", self.login, makepad_devhub::lease::exec::writes_allowed());
        if key != self.tray_key {
            self.tray_key = key;
            // the menu is the secondary click's; the primary click opens the dropdown (the
            // master's panel, a node's task view)
            let mut menu = vec![TrayMenuItem::Item { id: live_id!(status), label: status.clone(), enabled: false, checked: None }, TrayMenuItem::Line];
            if matches!(self.core, Some(Core::Master(_))) {
                menu.push(TrayMenuItem::item(OPEN, "Open panel"));
                menu.push(TrayMenuItem::item(SEEN, "Mark all read"));
                if !nodes.is_empty() {
                    menu.push(TrayMenuItem::Line);
                    self.dev_nodes = nodes.iter().map(|(n, _)| n.clone()).collect();
                    for (i, (name, dev)) in nodes.iter().enumerate() {
                        menu.push(TrayMenuItem::check(LiveId(DEV_BASE + i as u64), format!("Dev on {name}"), *dev));
                    }
                    menu.push(TrayMenuItem::Line);
                }
                if let Some(on) = self.login {
                    menu.push(TrayMenuItem::Item { id: LOGIN, label: "Open at login".into(), enabled: true, checked: Some(on) });
                }
            }
            if matches!(self.core, Some(Core::Node(_))) {
                // the node's own dev switch (the master keeps it; this asks it)
                menu.push(TrayMenuItem::Item { id: DEVSW, label: "Dev: this machine takes work from the network".into(), enabled: true, checked: Some(makepad_devhub::lease::exec::writes_allowed()) });
            }
            menu.push(TrayMenuItem::item(QUIT, "Quit devhub"));
            cx.set_tray(TrayIcon { id: TRAY, image: TrayImage { width: glyph::SIZE, height: glyph::SIZE, alpha: glyph::alpha(g) }, tooltip: format!("devhub \u{b7} {status}"), menu, dropdown: true });
        }
        if let Some(Core::Node(n)) = &self.core {
            if self.node_drop.is_open() {
                let head = Head { name: n.node.conf.name.clone(), lan: n.lan(), dev: makepad_devhub::lease::exec::writes_allowed(), leases: makepad_devhub::lease::exec::held(), cache: n.cache.lock().unwrap().clone() };
                if let Some(mut v) = self.ui.widget(cx, ids!(tasks)).borrow_mut::<DevhubTasks>() {
                    v.head = head;
                    v.redraw(cx);
                }
            }
            return;
        }
        let Some(Core::Master(m)) = &self.core else { return };
        if !(self.open || self.window_shown) {
            return;
        }
        let entries = m.master.inbox.lock().unwrap().entries.clone();
        self.state.inbox = entries
            .iter()
            .rev()
            .map(|e| InboxRow { id: e.record.id.clone(), line: e.record.stream_line(), count: e.count as u64, seen: e.seen, sev: e.record.sev.as_str().to_string() })
            .collect();
        self.state.machines = m.snapshot.lock().unwrap().machines.clone();
        self.state.detail = m.detail.lock().unwrap().clone();
        self.state.nodes = m.master.leases.nodes();
        let now = proto::wall_ms();
        self.state.leases = m.master.leases.view().iter().map(|l| lease_row(l, now)).collect();
        self.state.events = m
            .master
            .recent
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|r| r.kind != "hub")
            .take(200)
            .map(|r| EventRow { machine: r.machine.clone(), sev: r.sev.as_str().to_string(), line: format!("{}{}", if r.app.is_empty() { String::new() } else { format!("{}#{} ", r.app, r.pid) }, r.line), age_s: now.saturating_sub(r.at_ms) / 1000 })
            .collect();
        let mut blocked = Vec::new();
        m.master.leases.serve("blocked", "", &mut blocked);
        self.state.blocked = blocked.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("blocked")?.as_str().map(str::to_string)).collect();
        // the user's own inspect leases stay alive while the panel runs
        for (_, id) in self.state.user_leases.clone() {
            let mut out = Vec::new();
            m.master.leases.serve("beat", &id, &mut out);
            if out.first().is_none_or(|l| l.contains("\"state\":\"ended\"") || l.starts_with("{\"err\"")) {
                self.state.user_leases.retain(|u| u.1 != id);
            }
        }
        self.state.note = m.note.lock().unwrap().clone();
        if let Some(c) = m.pending.lock().unwrap().take() {
            self.state.confirm = Some(c);
        }
        self.state.status = status;
        let st = self.state.clone();
        for id in [ids!(drop_view), ids!(hub_view)] {
            if let Some(mut v) = self.ui.widget(cx, id).borrow_mut::<DevhubView>() {
                v.set_state(cx, st.clone());
            }
        }
    }

    fn set_looking(&self) {
        if let Some(Core::Master(m)) = &self.core {
            m.looking.store(self.open || self.window_shown, Ordering::Relaxed);
        }
    }

    /// Whether a window shows live graphs now: a node's open dropdown, the master's panel with a
    /// JIT selected.
    fn live_shown(&self) -> bool {
        match &self.core {
            Some(Core::Node(_)) => self.node_drop.is_open(),
            Some(Core::Master(_)) => (self.open || self.window_shown) && self.state.selected.is_some(),
            None => false,
        }
    }

    /// Sampling and the live redraws run only while a window shows the graphs.
    fn sync_live(&mut self, cx: &mut Cx) {
        let on = self.live_shown();
        if on && self.sampler.is_none() {
            let sampler = Sampler::start();
            for id in [ids!(tasks), ids!(drop_view), ids!(hub_view)] {
                if let Some(mut v) = self.ui.widget(cx, id).borrow_mut::<DevhubTasks>() {
                    v.board = Some(sampler.board.clone());
                }
                if let Some(mut v) = self.ui.widget(cx, id).borrow_mut::<DevhubView>() {
                    v.live = Some(sampler.board.clone());
                }
            }
            self.sampler = Some(sampler);
        }
        if let Some(s) = &self.sampler {
            match &self.core {
                Some(Core::Node(n)) => s.set_watch(Watch::Local(n.node.local.clone())),
                Some(Core::Master(m)) => match &self.state.selected {
                    Some((machine, pid)) => {
                        let app = self.state.machines.iter().find(|x| x.name == *machine).and_then(|x| x.jits.iter().find(|j| j.pid == *pid)).map(|j| j.app.clone()).unwrap_or_default();
                        s.set_watch(Watch::One(machine.clone(), *pid, app, *machine == m.master.name));
                    }
                    None => s.set_watch(Watch::None),
                },
                None => {}
            }
            s.set_visible(on);
        }
        if on != self.live_on {
            self.live_on = on;
            if on {
                self.live_timer = cx.start_interval(1.0 / LIVE_HZ);
            } else {
                cx.stop_timer(self.live_timer);
            }
        }
    }

    fn redraw_live(&mut self, cx: &mut Cx) {
        for id in [ids!(tasks), ids!(drop_view), ids!(hub_view)] {
            self.ui.widget(cx, id).redraw(cx);
        }
    }

    fn quit(&mut self, cx: &mut Cx) {
        if let Some(Core::Node(n)) = &self.core {
            // the person's Quit: the WM leaves the node off until it is started from the launcher
            let text = std::fs::read_to_string(&n.conf_path).unwrap_or_default();
            let kept: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("enabled")).collect();
            let _ = std::fs::write(&n.conf_path, format!("enabled = off\n{}\n", kept.join("\n")));
        }
        let _ = std::fs::remove_file(proto::control_socket());
        let _ = std::fs::remove_file(proto::home().join("node.pid"));
        installed::clear_pid();
        cx.remove_tray(TRAY);
        cx.quit();
    }

    fn quit_from_view(&mut self, cx: &mut Cx) {
        let window = self.ui.window(cx, ids!(node_window));
        self.node_drop.close(cx, &window, "quit");
        self.sync_live(cx);
        self.quit(cx);
    }

    fn act(&mut self, action: DevhubAction) {
        let Some(Core::Master(m)) = &self.core else { return };
        match action {
            DevhubAction::Tab(t) => self.state.tab = t,
            DevhubAction::Clear(id) => {
                m.master.inbox.lock().unwrap().clear(&id);
            }
            DevhubAction::ClearAll => {
                m.master.inbox.lock().unwrap().clear("all");
            }
            DevhubAction::SeenAll => m.master.inbox.lock().unwrap().mark_seen("all"),
            DevhubAction::Expand(machine) => {
                if self.state.collapsed.contains(&machine) {
                    self.state.collapsed.retain(|e| *e != machine);
                } else {
                    self.state.collapsed.push(machine);
                }
            }
            DevhubAction::Select(machine, pid) => {
                if self.state.selected.as_ref() == Some(&(machine.clone(), pid)) {
                    self.state.selected = None;
                    return;
                }
                self.state.view = "jit";
                self.state.selected = Some((machine.clone(), pid));
                let _ = m.drill.send((machine, pid, "jit".into()));
            }
            DevhubAction::View(v) => {
                if let Some((machine, pid)) = self.state.selected.clone() {
                    self.state.view = v;
                    // `timeline` needs a window to look at: the last 2 s
                    let cmd = if v == "timeline" { "timeline last ms=2000".to_string() } else { v.to_string() };
                    let _ = m.drill.send((machine, pid, cmd));
                }
            }
            DevhubAction::Jit(a) => {
                let Some((machine, pid)) = self.state.selected.clone() else { return };
                match a {
                    JitDo::Restart => {
                        let _ = m.drill.send((machine, pid, "!restart".into()));
                    }
                    JitDo::Stop => {
                        let _ = m.drill.send((machine, pid, "!stop".into()));
                    }
                    JitDo::Log => {
                        self.state.view = "log";
                        let _ = m.drill.send((machine, pid, "!log".into()));
                    }
                    JitDo::ClearCache => {
                        let app = self.state.machines.iter().find(|x| x.name == machine).and_then(|x| x.jits.iter().find(|j| j.pid == pid)).map(|j| j.app.clone());
                        size_jit_cache(m, machine, app);
                    }
                    JitDo::Lease => {
                        let key = (machine.clone(), pid);
                        let mut out = Vec::new();
                        match self.state.user_leases.iter().position(|u| u.0 == key) {
                            Some(i) => {
                                let id = self.state.user_leases.remove(i).1;
                                let lm = m.master.leases.clone();
                                std::thread::spawn(move || lm.serve("release", &id, &mut Vec::new()));
                            }
                            None => {
                                let mut req = makepad_devhub::lease::sched::Req::new("user", makepad_devhub::lease::Kind::Inspect);
                                req.node = Some(machine);
                                req.jit = Some(pid.to_string());
                                m.master.leases.serve("lease", &req.to_json(), &mut out);
                                if let Some(id) = out.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok()).and_then(|v| v.get("id").and_then(|x| x.as_str()).map(str::to_string)) {
                                    self.state.user_leases.push((key, id));
                                }
                            }
                        }
                    }
                }
            }
            DevhubAction::Ask(c) => self.state.confirm = Some(c),
            DevhubAction::ClearJit(machine, app) => size_jit_cache(m, machine, app),
            DevhubAction::Confirmed(Confirm::ClearJit { machine, app, force, .. }) => {
                self.state.confirm = None;
                let (note, local) = (m.note.clone(), machine == m.master.name);
                *note.lock().unwrap() = format!("clearing the JIT cache on {machine}\u{2026}");
                std::thread::spawn(move || {
                    let line = format!("jitcache clear{}{}", app.as_deref().map_or(String::new(), |a| format!(" app={a}")), if force { " force" } else { "" });
                    let out = jitcache_ask(&machine, local, &line);
                    let get = |k: &str| out.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get(k)?.as_str().map(str::to_string));
                    *note.lock().unwrap() = match (get("err"), get("line")) {
                        (Some(e), _) => format!("{machine}: {e}"),
                        (None, Some(l)) => format!("{machine}: {l}"),
                        (None, None) => format!("{machine}: no answer"),
                    };
                });
            }
            DevhubAction::Cancel => self.state.confirm = None,
            DevhubAction::Confirmed(c) => {
                self.state.confirm = None;
                let (lm, note) = (m.master.leases.clone(), m.note.clone());
                *note.lock().unwrap() = format!("{} \u{2026}", c.question().split('?').next().unwrap_or(""));
                // releases wait for the node's cleanup: never on the UI thread
                std::thread::spawn(move || {
                    let mut out = Vec::new();
                    let what = match &c {
                        Confirm::Release(id) => {
                            lm.serve("release", id, &mut out);
                            format!("{id} released")
                        }
                        Confirm::Cancel(id) => {
                            lm.serve("cancel", id, &mut out);
                            format!("{id} cancelled by user")
                        }
                        Confirm::Evict(who) => {
                            lm.serve("evict", who, &mut out);
                            format!("{who} evicted and blocked")
                        }
                        Confirm::ClearJit { .. } => String::new(),
                        Confirm::Override(n, to) => {
                            lm.serve("override", &format!("{n}{}", to.as_ref().map_or(String::new(), |t| format!(" --to {t}"))), &mut out);
                            format!("{n} taken")
                        }
                    };
                    let err = out.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("err")?.as_str().map(str::to_string));
                    *note.lock().unwrap() = match err {
                        Some(e) => format!("failed: {e}"),
                        None => format!("{what}; its processes stopped and dirs cleaned"),
                    };
                });
            }
            DevhubAction::Unblock(who) => {
                let mut out = Vec::new();
                m.master.leases.serve("unblock", &who, &mut out);
                *m.note.lock().unwrap() = format!("{who} unblocked");
            }
            DevhubAction::Dev(node, on) => {
                if let Err(e) = m.master.leases.set_dev(&node, on) {
                    log_line(&format!("devhub: dev switch {node}: {e}"));
                }
            }
            DevhubAction::CloseDetail => {
                self.state.selected = None;
                m.detail.lock().unwrap().clear();
            }
            DevhubAction::None => {}
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        match start_core() {
            Ok(core) => self.core = core,
            Err(e) => {
                log_line(&format!("devhub: {e}"));
                cx.quit();
                return;
            }
        }
        if self.core.is_none() {
            cx.quit();
            return;
        }
        installed::write_pid();
        if matches!(self.core, Some(Core::Node(_))) {
            // what stops this node by pid (a Windows logon task's hub, a test by hand)
            let _ = std::fs::write(proto::home().join("node.pid"), format!("{}\n", std::process::id()));
        }
        self.login = installed::login();
        self.state.tab = Tab::Fleet;
        self.state.view = "jit";
        self.timer = cx.start_interval(0.5);
        if cx.desktop_has_tray_dropdown() {
            cx.show_in_dock(false);
        }
        // `MAKEPAD_DEVHUB_OPEN=1`: the master's panel opens as a window at start (a terminal
        // master, a desktop without a tray, a frame grab); a node's task view opens below
        if matches!(self.core, Some(Core::Master(_))) && std::env::var_os("MAKEPAD_DEVHUB_OPEN").is_some_and(|v| v == "1") {
            self.window_shown = true;
            self.set_looking();
            self.ui.window(cx, ids!(main_window)).show(cx);
        }
        self.refresh(cx);
        // a node's task view the same way, as its dropdown (a frame grab, a check by hand)
        if matches!(self.core, Some(Core::Node(_))) && std::env::var_os("MAKEPAD_DEVHUB_OPEN").is_some_and(|v| v == "1") {
            let window = self.ui.window(cx, ids!(node_window));
            self.node_drop.open(cx, &window, 0.0);
            self.sync_live(cx);
            self.refresh(cx);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(wa) = action.as_widget_action() else { continue };
            let a = wa.cast::<DevhubAction>();
            if matches!(a, DevhubAction::None) {
                continue;
            }
            self.act(a);
            self.refresh(cx);
            self.sync_live(cx);
        }
        for action in actions {
            match action.as_widget_action().map(|wa| wa.cast::<TasksAction>()) {
                Some(TasksAction::Quit) => return self.quit_from_view(cx),
                Some(TasksAction::ToggleDev) => {
                    makepad_devhub::lease::exec::request_dev(!makepad_devhub::lease::exec::writes_allowed());
                    self.refresh(cx);
                }
                Some(a @ (TasksAction::ClearJit | TasksAction::ClearJitConfirmed(_) | TasksAction::ClearJitCancel)) => {
                    if let Some(Core::Node(n)) = &self.core {
                        node_clear_jit(n, a);
                    }
                    self.refresh(cx);
                }
                _ => {}
            }
        }
    }
}

impl AppMain for App {
    /// the master lives in the menu bar: no Dock icon until its panel window opens
    fn menu_bar_app() -> bool {
        true
    }

    fn outlives_window_manager() -> bool {
        true
    }

    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_shell_kit::eval_default_theme(vm);
        makepad_shell_kit::script_mod(vm);
        crate::view::script_mod(vm);
        crate::taskview::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Timer(t) if self.timer.is_timer(t).is_some() => self.refresh(cx),
            Event::Timer(t) if self.live_on && self.live_timer.is_timer(t).is_some() => self.redraw_live(cx),
            Event::TrayMenu(e) if e.tray == TRAY => match e.item {
                id if id == QUIT => return self.quit(cx),
                id if id == LOGIN => {
                    let on = !self.login.unwrap_or(false);
                    match installed::set_login(on) {
                        Ok(()) => log_line(&format!("devhub: open at login {}", if on { "on" } else { "off" })),
                        Err(e) => log_line(&format!("devhub: open at login: {e}")),
                    }
                    self.login = installed::login();
                    self.refresh(cx);
                }
                id if id.0 >= DEV_BASE && id.0 < DEV_BASE + 4096 => {
                    let i = (id.0 - DEV_BASE) as usize;
                    if let (Some(Core::Master(m)), Some(name)) = (&self.core, self.dev_nodes.get(i)) {
                        let on = !m.master.leases.nodes().iter().any(|n| &n.name == name && n.dev);
                        if let Err(e) = m.master.leases.set_dev(name, on) {
                            log_line(&format!("devhub: dev {name}: {e}"));
                        }
                    }
                    self.refresh(cx);
                }
                id if id == DEVSW => {
                    makepad_devhub::lease::exec::request_dev(!makepad_devhub::lease::exec::writes_allowed());
                    self.refresh(cx);
                }
                id if id == SEEN => {
                    self.act(DevhubAction::SeenAll);
                    self.refresh(cx);
                }
                id if id == OPEN => {
                    self.window_shown = true;
                    self.set_looking();
                    cx.show_in_dock(true);
                    self.ui.window(cx, ids!(main_window)).show(cx);
                    self.refresh(cx);
                }
                _ => {}
            },
            Event::TrayClick(e) if e.tray == TRAY && matches!(self.core, Some(Core::Master(_))) => {
                if !cx.has_tray_dropdown() {
                    self.window_shown = true;
                    self.set_looking();
                    self.ui.window(cx, ids!(main_window)).show(cx);
                } else if self.open {
                    self.open = false;
                    self.ui.window(cx, ids!(dropdown_window)).set_visible(cx, false);
                } else if self.dismissed.take().is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(350)) {
                    // the click that closed it
                } else {
                    self.open = true;
                    self.ui.window(cx, ids!(dropdown_window)).show_at_tray(cx, TRAY, DROPDOWN);
                }
                self.set_looking();
                self.refresh(cx);
                self.sync_live(cx);
            }
            Event::PopupDismissed(e) if Some(e.window_id) == self.ui.window(cx, ids!(dropdown_window)).window_id() => {
                if self.open {
                    self.dismissed = Some(std::time::Instant::now());
                    self.open = false;
                    self.ui.window(cx, ids!(dropdown_window)).set_visible(cx, false);
                    self.set_looking();
                    self.sync_live(cx);
                }
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape && self.open => {
                self.open = false;
                self.ui.window(cx, ids!(dropdown_window)).set_visible(cx, false);
                self.set_looking();
                self.sync_live(cx);
            }
            Event::Shutdown => installed::clear_pid(),
            Event::WindowCloseRequested(e) if cx.has_tray() => {
                e.accept_close.set(false);
                self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
                self.window_shown = false;
                self.set_looking();
                self.sync_live(cx);
                cx.show_in_dock(false);
            }
            _ => {}
        }
        // a node's tray click toggles its task view; a click elsewhere or Esc closes it
        if matches!(self.core, Some(Core::Node(_))) {
            let window = self.ui.window(cx, ids!(node_window));
            let content = self.ui.widget(cx, ids!(tasks)).borrow::<DevhubTasks>().map_or(0.0, |v| v.content_h);
            if let Some(change) = self.node_drop.handle_event(cx, event, &window, content) {
                self.sync_live(cx);
                if change == DropdownChange::Opened {
                    self.refresh(cx);
                }
            }
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let (Event::Draw(_), Some(Core::Node(_))) = (event, &self.core) {
            let window = self.ui.window(cx, ids!(node_window));
            let content = self.ui.widget(cx, ids!(tasks)).borrow::<DevhubTasks>().map_or(0.0, |v| v.content_h);
            self.node_drop.fit(cx, &window, content);
        }
    }
}
