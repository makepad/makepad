//! devhub, one app: a **node** when the WM names its conf (`MAKEPAD_DEVHUB_NODE_CONF`, the way it
//! starts the AI Hub), the **master** otherwise.
//!
//! - Master (the user's Mac): a menu-bar icon; a click opens the panel under it (the inbox and the
//!   explorer: machines → JITs → inspect views). It listens on nothing but its control socket and
//!   dials the nodes with the Mac's fleet identity.
//! - Node (a Linux WM): a tray icon in the WM's bar with a status menu; it serves this machine's
//!   JITs to the master. Quit writes `enabled = off` into its conf, so the WM leaves it off until
//!   the person starts it from the launcher.
//!
//! Without the fleet identity (`~/.makepad/fleet`, tools/fleet-identity.md) both serve this
//! machine's JITs only. `makepad devhub serve [--node <conf>]` runs the same core without a
//! window or tray (tests, ssh).

pub use makepad_widgets;

mod glyph;
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
use view::{DevhubAction, DevhubView, InboxRow, Tab, ViewState};

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
        }
    }
}

const TRAY: LiveId = live_id!(devhub_tray);
const QUIT: LiveId = live_id!(quit_devhub);
const SEEN: LiveId = live_id!(seen_all);
const OPEN: LiveId = live_id!(open_window);
const DROPDOWN: Vec2d = Vec2d { x: 620.0, y: 720.0 };

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

/// `L12 build@217 · gapx · granted 5m` / `L31 test · gapx · 3rd in queue (lane cap 2/2) · 40 s`.
fn queue_line(l: &Lease, now: u64) -> String {
    let secs = |from: u64| {
        let s = now.saturating_sub(from) / 1000;
        if s >= 3600 { format!("{}h{:02}m", s / 3600, (s / 60) % 60) } else if s >= 60 { format!("{}m{:02}s", s / 60, s % 60) } else { format!("{s} s") }
    };
    let jit = l.req.jit.as_deref().map_or(String::new(), |j| format!(" \u{b7} jit {j}"));
    match l.state {
        State::Queued => format!("{} \u{b7} {}{jit} \u{b7} {} in queue ({}) \u{b7} {}", l.tag(), l.req.lane, ordinal(l.pos), l.reason, secs(l.asked_ms)),
        _ => format!("{} \u{b7} {}{jit} \u{b7} {} {}", l.tag(), l.req.lane, l.state.as_str(), secs(l.granted_ms.unwrap_or(l.asked_ms))),
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
    drill: Sender<(String, u32, &'static str)>,
    detail: Arc<Mutex<Vec<String>>>,
}

struct NodeCore {
    node: Node,
    conf_path: PathBuf,
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
    let (drill, rx) = channel::<(String, u32, &'static str)>();
    let detail = Arc::new(Mutex::new(Vec::new()));
    let (m, d) = (master.clone(), detail.clone());
    let _ = std::thread::Builder::new().name("devhub-drill".into()).spawn(move || {
        while let Ok((machine, pid, cmd)) = rx.recv() {
            *d.lock().unwrap() = vec![format!("asking {machine}/{pid}: {cmd} …")];
            let lines = explorer::drill(&m, &machine, pid, cmd);
            *d.lock().unwrap() = lines;
        }
    });
    Ok(MasterCore { master, looking, snapshot, drill, detail })
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
    Ok(Some(NodeCore { node, conf_path }))
}

fn start_core() -> Result<Option<Core>, String> {
    match std::env::var_os(NODE_CONF_ENV).filter(|p| !p.is_empty()) {
        Some(p) => Ok(start_node(PathBuf::from(p))?.map(Core::Node)),
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
            Some(Core::Node(n)) => {
                let jits = n.node.local.jits().iter().filter(|j| j.run.is_some()).count();
                let lan = match (&n.node.addr, &n.node.lan_off) {
                    (Some(a), _) => format!("LAN {a}"),
                    (None, Some(w)) => w.clone(),
                    (None, None) => "local only".into(),
                };
                (if n.node.addr.is_some() { glyph::Glyph::Quiet } else { glyph::Glyph::Off }, format!("node {} \u{b7} {jits} JITs \u{b7} {lan}", n.node.conf.name))
            }
            None => (glyph::Glyph::Off, "off".into()),
        }
    }

    fn refresh(&mut self, cx: &mut Cx) {
        let (g, status) = self.status();
        let key = format!("{g:?}|{status}");
        if key != self.tray_key {
            self.tray_key = key;
            let mut menu = vec![TrayMenuItem::Item { id: live_id!(status), label: status.clone(), enabled: false, checked: None }, TrayMenuItem::Line];
            if matches!(self.core, Some(Core::Master(_))) {
                menu.push(TrayMenuItem::item(SEEN, "Mark all read"));
                if !cx.desktop_has_tray_dropdown() {
                    menu.push(TrayMenuItem::item(OPEN, "Open in window"));
                }
            }
            menu.push(TrayMenuItem::item(QUIT, "Quit devhub"));
            cx.set_tray(TrayIcon { id: TRAY, image: glyph::image(g), tooltip: format!("devhub \u{b7} {status}"), menu, dropdown: matches!(self.core, Some(Core::Master(_))) });
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
        self.state.queue = m.master.leases.view().iter().map(|l| (queue_line(l, now), l.state == State::Queued)).collect();
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

    fn quit(&mut self, cx: &mut Cx) {
        if let Some(Core::Node(n)) = &self.core {
            // the person's Quit: the WM leaves the node off until it is started from the launcher
            let text = std::fs::read_to_string(&n.conf_path).unwrap_or_default();
            let kept: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("enabled")).collect();
            let _ = std::fs::write(&n.conf_path, format!("enabled = off\n{}\n", kept.join("\n")));
        }
        let _ = std::fs::remove_file(proto::control_socket());
        cx.remove_tray(TRAY);
        cx.quit();
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
            DevhubAction::Select(machine, pid) => {
                self.state.view = "jit";
                self.state.selected = Some((machine.clone(), pid));
                let _ = m.drill.send((machine, pid, "jit"));
            }
            DevhubAction::View(v) => {
                if let Some((machine, pid)) = self.state.selected.clone() {
                    self.state.view = v;
                    let _ = m.drill.send((machine, pid, v));
                }
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
        self.state.tab = Tab::Inbox;
        self.state.view = "jit";
        self.timer = cx.start_interval(0.5);
        if cx.desktop_has_tray_dropdown() {
            cx.show_in_dock(false);
        }
        self.refresh(cx);
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
        }
    }
}

impl AppMain for App {
    fn outlives_window_manager() -> bool {
        true
    }

    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_shell_kit::eval_default_theme(vm);
        makepad_shell_kit::script_mod(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Timer(t) if self.timer.is_timer(t).is_some() => self.refresh(cx),
            Event::TrayMenu(e) if e.tray == TRAY => match e.item {
                id if id == QUIT => return self.quit(cx),
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
            }
            Event::PopupDismissed(e) if Some(e.window_id) == self.ui.window(cx, ids!(dropdown_window)).window_id() => {
                if self.open {
                    self.dismissed = Some(std::time::Instant::now());
                    self.open = false;
                    self.ui.window(cx, ids!(dropdown_window)).set_visible(cx, false);
                    self.set_looking();
                }
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape && self.open => {
                self.open = false;
                self.ui.window(cx, ids!(dropdown_window)).set_visible(cx, false);
                self.set_looking();
            }
            Event::WindowCloseRequested(e) if cx.has_tray() => {
                e.accept_close.set(false);
                self.ui.window(cx, ids!(main_window)).set_visible(cx, false);
                self.window_shown = false;
                self.set_looking();
                cx.show_in_dock(false);
            }
            _ => {}
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
