//! The master's panel. **Fleet** (the default tab): every box as a card — its load, CPU, GPU,
//! RAM and disk as meters, its role and dev switch, who holds leases on it (lane, agent, kind,
//! since when, heartbeat, what runs, disk use) and what waits for it, its JITs (each with what it
//! is up to: state, fps, warm/cold, tiers, code memory, parked panics, stale code, recent
//! events) and its recent events. A JIT opens its inspect views and actions; leases can be
//! released, a box overridden (given to the user or a lane) and an AI evicted, each confirmed in
//! the panel. **Inbox**: what needs a look. **Leases**: the whole queue and the block list.
//! Drawn with the shell kit (the shell's theme tokens), like the AI Hub's panel.

use makepad_devhub::explorer::MachineRow;
use makepad_devhub::lease::sched::NodeView;
use makepad_shell_kit::ui::{contains, cut_top, rect, DrawShellFill, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;
use crate::graph::Palette;
use makepad_shell_kit::graph::DrawShellTrace;
use crate::live::Board;
use std::sync::{Arc, Mutex};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DevhubViewBase = #(DevhubView::register_widget(vm))
    mod.widgets.DevhubView = set_type_default() do mod.widgets.DevhubViewBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
        g +: {}
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Fleet,
    Inbox,
    Leases,
}

/// The inspect views a JIT opens (any inspect command works from the CLI).
pub const VIEWS: &[&str] = &["jit", "why-slow", "memory", "threads", "timeline", "panics"];

/// What a JIT row offers besides its views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitDo {
    /// `session relaunch` (any JIT; state kept where the app supports it)
    Restart,
    /// stop a devhub launch (by its ledger pid)
    Stop,
    /// the tail of its log
    Log,
    /// hold an inspect lease on it as the user (AI breakpoints wait for it)
    Lease,
    /// clear its app's JIT cache on its machine (confirmed first)
    ClearCache,
}

/// A user action that is confirmed in the panel first.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    Release(String),
    /// the user's cancel of one running lease
    Cancel(String),
    Evict(String),
    /// (node, to lane or the user)
    Override(String, Option<String>),
    /// a machine's JIT cache, sized first: `question` says what goes and who runs on it; `force`
    /// when JITs run on it (they keep running)
    ClearJit { machine: String, app: Option<String>, question: String, force: bool },
}

impl Confirm {
    pub fn question(&self) -> String {
        match self {
            Confirm::Release(id) => format!("Release {id}? Its processes stop and its dirs are cleaned."),
            Confirm::Cancel(id) => format!("Cancel {id}? Its processes stop and its slot frees; its lane is told to pick another node and is kept off this one for 30 min."),
            Confirm::Evict(who) => format!("Evict {who}? Every lease of it ends and it gets no new ones until unblocked."),
            Confirm::Override(n, None) => format!("Take {n}? Its leases end and it takes no new ones (dev off)."),
            Confirm::Override(n, Some(l)) => format!("Give {n} to {l}? The other leases on it end."),
            Confirm::ClearJit { question, .. } => question.clone(),
        }
    }

    fn verb(&self) -> &'static str {
        match self {
            Confirm::Release(_) => "Release",
            Confirm::Cancel(_) => "Cancel lease",
            Confirm::Evict(_) => "Evict",
            Confirm::Override(..) => "Take it",
            Confirm::ClearJit { force: true, .. } => "Clear anyway",
            Confirm::ClearJit { .. } => "Clear",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum DevhubAction {
    Tab(Tab),
    Clear(String),
    ClearAll,
    SeenAll,
    /// fold or unfold a machine's card
    Expand(String),
    /// a JIT row: (machine, pid)
    Select(String, u32),
    /// an inspect view of the selected JIT
    View(&'static str),
    Jit(JitDo),
    CloseDetail,
    /// the node's dev switch: (node, on)
    Dev(String, bool),
    Ask(Confirm),
    /// size a machine's JIT cache (all, or one app's), then ask
    ClearJit(String, Option<String>),
    Confirmed(Confirm),
    Cancel,
    Unblock(String),
    #[default]
    None,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InboxRow {
    pub id: String,
    pub line: String,
    pub count: u64,
    pub seen: bool,
    pub sev: String,
}

/// A lease as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeaseRow {
    pub id: String,
    pub node: String,
    pub lane: String,
    pub agent: String,
    pub kind: String,
    pub waiting: bool,
    /// granted for (live) or waiting for (queued), s
    pub since_s: u64,
    pub beat_s: u64,
    pub running: String,
    pub disk_mb: Option<f64>,
    pub work: String,
    /// queued: `3rd in queue (lane cap 2/2)`
    pub queue: String,
    pub jit: String,
}

/// A record line for a machine's recent events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventRow {
    pub machine: String,
    pub sev: String,
    pub line: String,
    pub age_s: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub tab: Tab,
    pub status: String,
    pub inbox: Vec<InboxRow>,
    pub machines: Vec<MachineRow>,
    /// machines the user folded (every card starts open)
    pub collapsed: Vec<String>,
    pub selected: Option<(String, u32)>,
    pub view: &'static str,
    pub detail: Vec<String>,
    /// the lease scheduler's nodes (caps, dev switch, leases)
    pub nodes: Vec<NodeView>,
    pub leases: Vec<LeaseRow>,
    pub events: Vec<EventRow>,
    pub blocked: Vec<String>,
    pub confirm: Option<Confirm>,
    /// the inspect leases the user holds from the panel: (machine, pid) -> lease id
    pub user_leases: Vec<((String, u32), String)>,
    /// the last action's outcome, one line
    pub note: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Tab(Tab),
    Clear(usize),
    ClearAll,
    SeenAll,
    Expand(usize),
    Jit(usize, usize),
    View(&'static str),
    JitDo(JitDo),
    CloseDetail,
    Dev(usize),
    Release(usize),
    Evict(usize),
    Override(usize),
    ClearJit(usize),
    Confirm,
    Cancel,
    Unblock(usize),
}

const PAD: f64 = 14.0;
const OK: Vec4f = Vec4f { x: 0.45, y: 0.85, z: 0.5, w: 1.0 };
const WARN: Vec4f = Vec4f { x: 1.0, y: 0.78, z: 0.38, w: 1.0 };
const ERR: Vec4f = Vec4f { x: 1.0, y: 0.48, z: 0.42, w: 1.0 };
const REQ: Vec4f = Vec4f { x: 0.5, y: 0.75, z: 1.0, w: 1.0 };

#[derive(Script, ScriptHook, Widget)]
pub struct DevhubView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawShellFill,
    #[live]
    d: ShellDraw,
    #[live]
    tokens: ShellTokens,
    #[live]
    card: bool,
    #[live]
    g: DrawShellTrace,
    /// the live sampler's rings (the selected JIT's card)
    #[rust]
    pub live: Option<Arc<Mutex<Board>>>,
    #[rust]
    pub state: ViewState,
    #[rust]
    hits: Vec<(Hit, Rect)>,
    #[rust]
    hot: Option<Hit>,
    #[rust]
    scroll: f64,
    #[rust]
    content_h: f64,
    #[rust]
    view_h: f64,
    /// the lease rows drawn, in hit order (Release / Evict buttons index them)
    #[rust]
    drawn_leases: Vec<LeaseRow>,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

pub fn dur(s: u64) -> String {
    if s >= 86400 {
        format!("{}d{:02}h", s / 86400, (s / 3600) % 24)
    } else if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn sev_color(sev: &str) -> Vec4f {
    match sev {
        "err" => ERR,
        "req" => REQ,
        "warn" => WARN,
        _ => OK,
    }
}

/// A use fraction's color: calm, then warm, then hot.
fn load_color(f: f64, accent: Vec4f) -> Vec4f {
    if f >= 0.9 {
        ERR
    } else if f >= 0.7 {
        WARN
    } else {
        accent
    }
}

fn lease_line(l: &LeaseRow) -> String {
    let who = if l.agent.is_empty() { l.lane.clone() } else { format!("{} ({})", l.lane, l.agent) };
    let jit = if l.jit.is_empty() { String::new() } else { format!(" {}", l.jit) };
    if l.waiting {
        return format!("{} {}{jit} \u{b7} {who} \u{b7} {} \u{b7} waiting {}", l.id, l.kind, l.queue, dur(l.since_s));
    }
    let run = if l.running.is_empty() { String::new() } else { format!(" \u{b7} runs `{}`", l.running) };
    let disk = l.disk_mb.map_or(String::new(), |m| format!(" \u{b7} {:.1} GB", m / 1024.0));
    let beat = if l.beat_s > 4 { format!(" \u{b7} beat {}s ago", l.beat_s) } else { String::new() };
    format!("{} {}{jit} \u{b7} {who} \u{b7} {}{beat}{run}{disk}", l.id, l.kind, dur(l.since_s))
}

impl DevhubView {
    pub fn set_state(&mut self, cx: &mut Cx, state: ViewState) {
        self.state = state;
        self.redraw(cx);
    }

    fn hit_at(&self, p: Vec2d) -> Option<Hit> {
        self.hits.iter().rev().find(|(_, r)| contains(*r, p)).map(|(h, _)| h.clone())
    }

    fn ctrl(&self, hit: &Hit, selected: bool) -> CtrlState {
        if selected {
            CtrlState::Selected
        } else if self.hot.as_ref() == Some(hit) {
            CtrlState::Hover
        } else {
            CtrlState::Normal
        }
    }

    /// A small button at the right end of `row`, `w` wide, `from_right` px in.
    fn small_button(&mut self, cx: &mut Cx2d, row: Rect, from_right: f64, w: f64, hit: Hit, label: &str) {
        let tok = self.tokens;
        let r = rect(row.pos.x + row.size.x - from_right - w, row.pos.y + 1.0, w, row.size.y - 2.0);
        self.d.button(cx, r, &tok, self.ctrl(&hit, false), None, label, tok.font.caption, tok.popups.text, true);
        self.hits.push((hit, r));
    }

    /// `label  ███░░░  text` meter in `r`.
    fn meter(&mut self, cx: &mut Cx2d, r: Rect, label: &str, frac: Option<f64>, text: &str) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        self.d.label(cx, rect(r.pos.x, r.pos.y, 34.0, r.size.y), false, tok.font.caption, dim, HAlign::Left, label);
        let bar = rect(r.pos.x + 36.0, r.pos.y + r.size.y / 2.0 - 3.0, r.size.x - 36.0, 6.0);
        self.d.solid(cx, bar, alpha(fg, 0.10));
        if let Some(f) = frac {
            let f = f.clamp(0.0, 1.0);
            self.d.solid(cx, rect(bar.pos.x, bar.pos.y, (bar.size.x * f).max(2.0), bar.size.y), load_color(f, tok.bar.active));
        }
        self.d.label_elided(cx, rect(bar.pos.x, r.pos.y - 9.0, bar.size.x, 12.0), false, tok.font.caption * 0.9, dim, HAlign::Right, text);
    }

    fn draw_machine(&mut self, cx: &mut Cx2d, mi: usize, m: &MachineRow, mut rest: Rect, st: &ViewState) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        let node = st.nodes.iter().find(|n| n.name == m.name).cloned();
        let open = !st.collapsed.iter().any(|e| *e == m.name);
        let top = rest.pos.y;
        // header: chevron, name, role, state, dev switch, take
        let (row, r2) = cut_top(rest, 28.0);
        rest = r2;
        let hit = Hit::Expand(mi);
        if self.hot.as_ref() == Some(&hit) {
            self.d.solid(cx, row, alpha(fg, 0.05));
        }
        self.d.icon_centered(cx, if open { Ico::ChevronDown } else { Ico::ChevronRight }, rect(row.pos.x, row.pos.y, 18.0, row.size.y), 12.0, dim);
        let up = m.err.is_none();
        self.d.solid(cx, rect(row.pos.x + 22.0, row.pos.y + 11.0, 7.0, 7.0), if up { OK } else { ERR });
        let jits = m.jits.iter().filter(|j| j.jit).count();
        let role = node.as_ref().map_or(String::new(), |n| format!("  {}", n.role().as_str()));
        let title = format!("{}{}{role}", m.name, if m.local { " (this Mac)" } else { "" });
        self.d.label(cx, rect(row.pos.x + 36.0, row.pos.y, 220.0, row.size.y), true, tok.font.body_small, fg, HAlign::Left, &title);
        let tail = match (&m.err, &node) {
            (Some(e), _) => format!("down: {}", e.chars().take(60).collect::<String>()),
            (None, Some(n)) => format!("{} \u{b7} {jits} JITs \u{b7} {} leases{}", n.state_word(), n.leases, if n.waiting > 0 { format!(", {} waiting", n.waiting) } else { String::new() }),
            (None, None) => format!("{jits} JITs"),
        };
        self.d.label_elided(cx, rect(row.pos.x + 160.0, row.pos.y, row.size.x - 160.0 - 262.0, row.size.y), false, tok.font.caption, dim, HAlign::Left, &tail);
        self.hits.push((hit, rect(row.pos.x, row.pos.y, row.size.x - 262.0, row.size.y)));
        if up {
            // its JIT cache (every app's): sized, then confirmed in the panel
            self.small_button(cx, rect(row.pos.x, row.pos.y + 2.0, row.size.x, 24.0), if node.is_some() { 170.0 } else { 0.0 }, 86.0, Hit::ClearJit(mi), "Clear JIT");
        }
        if let Some(n) = &node {
            let dev_hit = Hit::Dev(mi);
            let r = rect(row.pos.x + row.size.x - 166.0, row.pos.y + 3.0, 84.0, 22.0);
            self.d.button(cx, r, &tok, self.ctrl(&dev_hit, n.dev), None, if n.dev { "dev on" } else { "dev off" }, tok.font.caption, fg, true);
            self.hits.push((dev_hit, r));
            if n.leases > 0 || n.dev {
                self.small_button(cx, rect(row.pos.x, row.pos.y + 2.0, row.size.x, 24.0), 0.0, 76.0, Hit::Override(mi), "Take it");
            }
        }
        // meters: CPU, GPU(s), RAM, disk
        if let Some(c) = node.as_ref().and_then(|n| n.caps.clone()).filter(|_| up) {
            let (row, r2) = cut_top(rest, 22.0);
            rest = r2;
            let w = (row.size.x - 36.0) / 4.0;
            let x = row.pos.x + 36.0;
            let cpu = c.cpu_pct.map(|p| p / 100.0).unwrap_or(c.load1 / c.cores.max(1) as f64);
            self.meter(cx, rect(x, row.pos.y + 8.0, w - 10.0, 14.0), "CPU", Some(cpu), &format!("{:.0}% \u{b7} load {:.1}", cpu * 100.0, c.load1));
            let (gf, gt) = match (c.gpus.first(), c.gpu_busy.first()) {
                (Some((_, vram)), Some((u, used))) => (Some(u / 100.0), format!("{u:.0}% \u{b7} {used:.0}/{vram:.0} GB")),
                (Some((n, vram)), None) => (None, format!("{} {vram:.0} GB", n.trim_start_matches("NVIDIA ").trim_start_matches("GeForce "))),
                _ => (None, "none".into()),
            };
            self.meter(cx, rect(x + w, row.pos.y + 8.0, w - 10.0, 14.0), "GPU", gf, &gt);
            let used = (c.ram_gb - c.ram_avail_gb).max(0.0);
            self.meter(cx, rect(x + 2.0 * w, row.pos.y + 8.0, w - 10.0, 14.0), "RAM", Some(used / c.ram_gb.max(1.0)), &format!("{used:.0}/{:.0} GB", c.ram_gb));
            let dused = (c.disk_total_gb - c.disk_free_gb).max(0.0);
            self.meter(cx, rect(x + 3.0 * w, row.pos.y + 8.0, w - 10.0, 14.0), "Disk", Some(dused / c.disk_total_gb.max(1.0)), &format!("{:.0} GB free", c.disk_free_gb));
            rest = cut_top(rest, 4.0).1;
        }
        // leases on it and what waits for it
        let leases: Vec<LeaseRow> = st.leases.iter().filter(|l| l.node == m.name).cloned().collect();
        for l in leases.iter() {
            let (row, r2) = cut_top(rest, 22.0);
            rest = r2;
            self.d.solid(cx, rect(row.pos.x + 36.0, row.pos.y + 8.0, 6.0, 6.0), if l.waiting { WARN } else if l.beat_s > 6 { ERR } else { OK });
            let li = self.drawn_leases.len();
            self.drawn_leases.push(l.clone());
            self.d.label_elided(cx, rect(row.pos.x + 48.0, row.pos.y, row.size.x - 48.0 - 136.0, row.size.y), false, tok.font.caption, fg, HAlign::Left, &lease_line(l));
            self.small_button(cx, row, 66.0, 64.0, Hit::Release(li), if l.waiting { "Drop" } else { "\u{d7} Cancel" });
            self.small_button(cx, row, 0.0, 62.0, Hit::Evict(li), "Evict AI");
        }
        if open {
            // its JITs, each with what it is up to
            // native endpoints (the WM's own apps, hubs) are one quiet line; JITs get a row each
            let natives: Vec<String> = m.jits.iter().filter(|j| !j.jit).map(|j| j.app.trim_start_matches("makepad-app-").to_string()).collect();
            if !natives.is_empty() {
                let (row, r2) = cut_top(rest, 18.0);
                rest = r2;
                let mut names: Vec<(String, usize)> = Vec::new();
                for n in natives {
                    match names.iter_mut().find(|x| x.0 == n) {
                        Some(x) => x.1 += 1,
                        None => names.push((n, 1)),
                    }
                }
                let list: Vec<String> = names.iter().map(|(n, c)| if *c > 1 { format!("{n} \u{d7}{c}") } else { n.clone() }).collect();
                self.d.label_elided(cx, rect(row.pos.x + 48.0, row.pos.y, row.size.x - 48.0, row.size.y), false, tok.font.caption, dim, HAlign::Left, &format!("native: {}", list.join(", ")));
            }
            for (ji, j) in m.jits.iter().enumerate() {
                if !j.jit {
                    continue;
                }
                let facts = j.facts();
                let lines = 1 + (!facts.is_empty()) as usize + j.recent.len().min(1);
                let (row, r2) = cut_top(rest, 4.0 + 17.0 * lines as f64);
                rest = r2;
                let hit = Hit::Jit(mi, ji);
                let sel = st.selected.as_ref() == Some(&(m.name.clone(), j.pid));
                let body = rect(row.pos.x + 30.0, row.pos.y, row.size.x - 30.0, row.size.y);
                if sel || self.hot.as_ref() == Some(&hit) {
                    self.d.button(cx, body, &tok, self.ctrl(&hit, sel), None, "", tok.font.caption, fg, false);
                }
                let dot = if j.panics > 0 { ERR } else if j.stale().is_some() || j.recent.iter().any(|r| r.0 == "hitch" || r.0 == "config") { WARN } else if j.state.starts_with("busy") { REQ } else { OK };
                if j.jit {
                    self.d.solid(cx, rect(body.pos.x + 6.0, body.pos.y + 8.0, 6.0, 6.0), dot);
                }
                let mut y = body.pos.y + 2.0;
                self.d.label_elided(cx, rect(body.pos.x + 18.0, y, body.size.x - 22.0, 16.0), true, tok.font.caption, if j.jit { fg } else { dim }, HAlign::Left, &j.head());
                y += 17.0;
                if !facts.is_empty() {
                    self.d.label_elided(cx, rect(body.pos.x + 18.0, y, body.size.x - 22.0, 16.0), false, tok.font.caption, dim, HAlign::Left, &facts);
                    y += 17.0;
                }
                if let Some((kind, line, age)) = j.recent.first() {
                    let c = if matches!(kind.as_str(), "panic" | "crash" | "hang") { ERR } else { WARN };
                    self.d.label_elided(cx, rect(body.pos.x + 18.0, y, body.size.x - 22.0, 16.0), false, tok.font.caption, alpha(c, 0.9), HAlign::Left, &format!("{} ago: {line}", dur(*age)));
                }
                self.hits.push((hit, body));
                if sel {
                    rest = self.draw_detail(cx, rest, st);
                }
            }
            if m.jits.is_empty() && up {
                let (row, r2) = cut_top(rest, 20.0);
                rest = r2;
                self.d.label(cx, rect(row.pos.x + 36.0, row.pos.y, row.size.x, row.size.y), false, tok.font.caption, dim, HAlign::Left, "No JITs run here.");
            }
            // its recent events
            let events: Vec<&EventRow> = st.events.iter().filter(|e| e.machine == m.name).take(4).collect();
            for e in events {
                let (row, r2) = cut_top(rest, 18.0);
                rest = r2;
                self.d.solid(cx, rect(row.pos.x + 38.0, row.pos.y + 7.0, 4.0, 4.0), sev_color(&e.sev));
                self.d.label_elided(cx, rect(row.pos.x + 48.0, row.pos.y, row.size.x - 48.0, row.size.y), false, tok.font.caption, dim, HAlign::Left, &format!("{} ago \u{b7} {}", dur(e.age_s), e.line));
            }
        }
        rest = cut_top(rest, 6.0).1;
        // a quiet card edge between machines
        self.d.separator(cx, rect(rest.pos.x, rest.pos.y - 3.0, rest.size.x, 1.0), fg, 0.08);
        let _ = top;
        rest
    }

    /// The selected JIT's views, actions and answer.
    fn draw_detail(&mut self, cx: &mut Cx2d, mut rest: Rect, st: &ViewState) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let x0 = rest.pos.x + 30.0;
        let wd = rest.size.x - 30.0;
        let (row, r2) = cut_top(rest, 28.0);
        rest = r2;
        let w = 66.0;
        for (i, v) in VIEWS.iter().enumerate() {
            let r = rect(x0 + i as f64 * w, row.pos.y + 3.0, w - 3.0, 22.0);
            let hit = Hit::View(v);
            self.d.button(cx, r, &tok, self.ctrl(&hit, st.view == *v), None, v, tok.font.caption, fg, true);
            self.hits.push((hit, r));
        }
        let held = st.selected.as_ref().and_then(|s| st.user_leases.iter().find(|u| u.0 == *s));
        let (row, r2) = cut_top(rest, 28.0);
        rest = r2;
        let acts: [(JitDo, &str); 5] = [(JitDo::Restart, "Restart"), (JitDo::Stop, "Stop"), (JitDo::Log, "Log"), (JitDo::Lease, if held.is_some() { "Let go" } else { "Hold it" }), (JitDo::ClearCache, "Clear cache")];
        for (i, (a, label)) in acts.iter().enumerate() {
            let r = rect(x0 + i as f64 * 82.0, row.pos.y + 3.0, 78.0, 22.0);
            let hit = Hit::JitDo(*a);
            self.d.button(cx, r, &tok, self.ctrl(&hit, *a == JitDo::Lease && held.is_some()), None, label, tok.font.caption, fg, true);
            self.hits.push((hit, r));
        }
        let r = rect(x0 + wd - 22.0, row.pos.y + 3.0, 22.0, 22.0);
        self.d.button(cx, r, &tok, self.ctrl(&Hit::CloseDetail, false), Some(Ico::Close), "", tok.font.caption, fg, false);
        self.hits.push((Hit::CloseDetail, r));
        // its live graphs (the node's task view card): compiles by tier, frame times, CPU
        if let (Some(src), Some((machine, pid))) = (self.live.clone(), st.selected.clone()) {
            let b = src.lock().unwrap();
            if let Some(j) = b.jits.iter().find(|j| j.pid == pid) {
                let p = Palette::new(&tok);
                let cpu = st.machines.iter().find(|m| m.name == machine).and_then(|m| m.jits.iter().find(|j| j.pid == pid)).and_then(|j| j.cpu_pct).map(|c| format!("{c:.0}%"));
                let now = crate::taskview::view_now(&b);
                let h = crate::taskview::jit_card(&mut self.d, &mut self.g, cx, &tok, &p, rect(x0, rest.pos.y + 6.0, wd, 0.0), j, now, cpu.as_deref());
                rest = cut_top(rest, h + 14.0).1;
            }
        }
        let box_h = 16.0 * st.detail.len().min(40) as f64 + 8.0;
        let (area, r2) = cut_top(rest, box_h);
        rest = r2;
        self.d.solid(cx, rect(x0, area.pos.y, wd, area.size.y), alpha(fg, 0.04));
        for (i, l) in st.detail.iter().take(40).enumerate() {
            self.d.label_elided(cx, rect(x0 + 6.0, area.pos.y + 4.0 + i as f64 * 16.0, wd - 12.0, 16.0), false, tok.font.caption, fg, HAlign::Left, l);
        }
        cut_top(rest, 6.0).1
    }

    fn draw_all(&mut self, cx: &mut Cx2d, window: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        self.hits.clear();
        self.drawn_leases.clear();
        let edge = if self.card {
            self.d.card(cx, window, &tok.popups);
            tok.popups.border_width
        } else {
            self.draw_bg.color = tok.popups.bg();
            self.draw_bg.draw_abs(cx, window);
            0.0
        };
        self.view_h = window.size.y;
        let pad = PAD + edge;
        let st = self.state.clone();
        let top = rect(window.pos.x + pad, window.pos.y + pad - self.scroll, window.size.x - 2.0 * pad, 1.0e6);

        let (row, rest) = cut_top(top, 40.0);
        self.d.panel_hero(cx, row, &tok, fg, Ico::Cpu, "devhub", &st.status, 0.0);
        let (row, mut rest) = cut_top(rest, 34.0);
        let new = st.inbox.iter().filter(|r| !r.seen).count();
        let live = st.leases.iter().filter(|l| !l.waiting).count();
        let waiting = st.leases.len() - live;
        let up = st.machines.iter().filter(|m| m.err.is_none()).count();
        let tabs = [
            (Tab::Fleet, format!("Fleet ({up}/{})", st.machines.len())),
            (Tab::Inbox, if new > 0 { format!("Inbox ({new})") } else { "Inbox".to_string() }),
            (Tab::Leases, if waiting > 0 { format!("Leases ({live}, {waiting} wait)") } else { format!("Leases ({live})") }),
        ];
        for (i, (tab, label)) in tabs.iter().enumerate() {
            let r = rect(row.pos.x + i as f64 * 150.0, row.pos.y + 4.0, 144.0, 26.0);
            let hit = Hit::Tab(*tab);
            self.d.button(cx, r, &tok, self.ctrl(&hit, st.tab == *tab), None, label, tok.font.body_small, fg, true);
            self.hits.push((hit, r));
        }
        let (sep, r2) = cut_top(rest, 10.0);
        rest = r2;
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 5.0, sep.size.x, 1.0), fg, 0.12);

        // a pending confirmation sits on top, in the panel
        if let Some(c) = &st.confirm {
            // the question wraps (a cache clear names every folder it removes)
            let lines = self.d.wrap(cx, true, tok.font.caption, &c.question(), rest.size.x - 190.0, 8);
            let (row, r2) = cut_top(rest, (16.0 * lines.len() as f64 + 10.0).max(34.0));
            rest = r2;
            self.d.solid(cx, row, alpha(WARN, 0.14));
            for (i, l) in lines.iter().enumerate() {
                self.d.label(cx, rect(row.pos.x + 8.0, row.pos.y + 5.0 + 16.0 * i as f64, row.size.x - 190.0, 16.0), true, tok.font.caption, fg, HAlign::Left, l);
            }
            self.small_button(cx, rect(row.pos.x, row.pos.y + 5.0, row.size.x - 6.0, 24.0), 84.0, 80.0, Hit::Confirm, c.verb());
            self.small_button(cx, rect(row.pos.x, row.pos.y + 5.0, row.size.x - 6.0, 24.0), 0.0, 80.0, Hit::Cancel, "Cancel");
            rest = cut_top(rest, 6.0).1;
        } else if !st.note.is_empty() {
            let (row, r2) = cut_top(rest, 22.0);
            rest = r2;
            self.d.label_elided(cx, row, false, tok.font.caption, dim, HAlign::Left, &st.note);
        }

        match st.tab {
            Tab::Fleet => {
                if st.machines.is_empty() {
                    let (row, r2) = cut_top(rest, 24.0);
                    rest = r2;
                    self.d.label(cx, row, false, tok.font.caption, dim, HAlign::Left, "Looking at the fleet\u{2026} (nodes.conf lists the boxes)");
                }
                for (mi, m) in st.machines.iter().enumerate() {
                    rest = self.draw_machine(cx, mi, m, rest, &st);
                }
                // leases waiting for any node
                let anywhere: Vec<LeaseRow> = st.leases.iter().filter(|l| l.node.is_empty()).cloned().collect();
                if !anywhere.is_empty() {
                    let (row, r2) = cut_top(rest, 22.0);
                    rest = r2;
                    self.d.section_header(cx, row, &tok, fg, "Waiting for any box");
                    for l in anywhere {
                        let (row, r2) = cut_top(rest, 22.0);
                        rest = r2;
                        let li = self.drawn_leases.len();
                        self.drawn_leases.push(l.clone());
                        self.d.solid(cx, rect(row.pos.x + 6.0, row.pos.y + 8.0, 6.0, 6.0), WARN);
                        self.d.label_elided(cx, rect(row.pos.x + 18.0, row.pos.y, row.size.x - 156.0, row.size.y), false, tok.font.caption, fg, HAlign::Left, &lease_line(&l));
                        self.small_button(cx, row, 66.0, 64.0, Hit::Release(li), "Drop");
                        self.small_button(cx, row, 0.0, 62.0, Hit::Evict(li), "Evict AI");
                    }
                }
                rest = self.draw_blocked(cx, rest, &st);
            }
            Tab::Inbox => {
                let (row, r2) = cut_top(rest, 28.0);
                rest = r2;
                if st.inbox.is_empty() {
                    self.d.label(cx, row, false, tok.font.caption, dim, HAlign::Left, "Nothing needs a look.");
                } else {
                    for (i, (hit, label)) in [(Hit::SeenAll, "Mark all read"), (Hit::ClearAll, "Clear all")].into_iter().enumerate() {
                        let r = rect(row.pos.x + row.size.x - 220.0 + i as f64 * 112.0, row.pos.y + 2.0, 106.0, 24.0);
                        self.d.button(cx, r, &tok, self.ctrl(&hit, false), None, label, tok.font.body_small, fg, true);
                        self.hits.push((hit, r));
                    }
                }
                for (i, e) in st.inbox.iter().enumerate().take(200) {
                    let (row, r2) = cut_top(rest, 24.0);
                    rest = r2;
                    let text_color = if e.seen { dim } else { fg };
                    self.d.solid(cx, rect(row.pos.x, row.pos.y + 9.0, 6.0, 6.0), alpha(sev_color(&e.sev), if e.seen { 0.45 } else { 1.0 }));
                    let count = if e.count > 1 { format!("x{}", e.count) } else { String::new() };
                    let cw = if count.is_empty() { 0.0 } else { self.d.measure(cx, false, tok.font.caption, &count) + 8.0 };
                    self.d.label_elided(cx, rect(row.pos.x + 12.0, row.pos.y, row.size.x - 12.0 - 30.0 - cw, row.size.y), false, tok.font.caption, text_color, HAlign::Left, &e.line);
                    if cw > 0.0 {
                        self.d.label(cx, rect(row.pos.x, row.pos.y, row.size.x - 30.0, row.size.y), false, tok.font.caption, dim, HAlign::Right, &count);
                    }
                    let r = rect(row.pos.x + row.size.x - 24.0, row.pos.y + 2.0, 22.0, 20.0);
                    let hit = Hit::Clear(i);
                    self.d.button(cx, r, &tok, self.ctrl(&hit, false), Some(Ico::Close), "", tok.font.caption, fg, false);
                    self.hits.push((hit, r));
                }
            }
            Tab::Leases => {
                if st.leases.is_empty() {
                    let (row, r2) = cut_top(rest, 24.0);
                    rest = r2;
                    self.d.label(cx, row, false, tok.font.caption, dim, HAlign::Left, "No leases.");
                }
                for l in st.leases.iter().take(300) {
                    let (row, r2) = cut_top(rest, 22.0);
                    rest = r2;
                    let li = self.drawn_leases.len();
                    self.drawn_leases.push(l.clone());
                    self.d.solid(cx, rect(row.pos.x, row.pos.y + 8.0, 6.0, 6.0), if l.waiting { WARN } else { OK });
                    let at = if l.node.is_empty() { String::new() } else { format!("@{} ", l.node) };
                    self.d.label_elided(cx, rect(row.pos.x + 12.0, row.pos.y, row.size.x - 150.0, row.size.y), false, tok.font.caption, fg, HAlign::Left, &format!("{at}{}", lease_line(l)));
                    self.small_button(cx, row, 66.0, 64.0, Hit::Release(li), if l.waiting { "Drop" } else { "\u{d7} Cancel" });
                    self.small_button(cx, row, 0.0, 62.0, Hit::Evict(li), "Evict AI");
                }
                rest = self.draw_blocked(cx, rest, &st);
            }
        }
        self.content_h = rest.pos.y + pad + self.scroll - window.pos.y;
    }

    fn draw_blocked(&mut self, cx: &mut Cx2d, mut rest: Rect, st: &ViewState) -> Rect {
        if st.blocked.is_empty() {
            return rest;
        }
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (row, r2) = cut_top(rest, 26.0);
        rest = r2;
        self.d.section_header(cx, row, &tok, fg, "Evicted (no new leases)");
        for (i, b) in st.blocked.iter().enumerate() {
            let (row, r2) = cut_top(rest, 22.0);
            rest = r2;
            self.d.solid(cx, rect(row.pos.x + 6.0, row.pos.y + 8.0, 6.0, 6.0), ERR);
            self.d.label(cx, rect(row.pos.x + 18.0, row.pos.y, row.size.x - 120.0, row.size.y), false, tok.font.caption, fg, HAlign::Left, b);
            self.small_button(cx, row, 0.0, 80.0, Hit::Unblock(i), "Unblock");
        }
        rest
    }
}

impl Widget for DevhubView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let window = cx.turtle().rect();
        self.draw_all(cx, window);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::MouseMove(e) => {
                let hot = self.hit_at(e.abs);
                if hot != self.hot {
                    self.hot = hot;
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                let st = &self.state;
                let lease = |i: usize| self.drawn_leases.get(i).cloned();
                let action = match self.hit_at(e.abs) {
                    Some(Hit::Tab(t)) => DevhubAction::Tab(t),
                    Some(Hit::Clear(i)) => match st.inbox.get(i) {
                        Some(r) => DevhubAction::Clear(r.id.clone()),
                        None => return,
                    },
                    Some(Hit::ClearAll) => DevhubAction::ClearAll,
                    Some(Hit::SeenAll) => DevhubAction::SeenAll,
                    Some(Hit::Expand(mi)) => match st.machines.get(mi) {
                        Some(m) => DevhubAction::Expand(m.name.clone()),
                        None => return,
                    },
                    Some(Hit::Jit(mi, ji)) => match st.machines.get(mi).and_then(|m| m.jits.get(ji).map(|j| (m.name.clone(), j.pid))) {
                        Some((m, pid)) => DevhubAction::Select(m, pid),
                        None => return,
                    },
                    Some(Hit::View(v)) => DevhubAction::View(v),
                    Some(Hit::JitDo(a)) => DevhubAction::Jit(a),
                    Some(Hit::CloseDetail) => DevhubAction::CloseDetail,
                    Some(Hit::Dev(mi)) => match st.machines.get(mi).and_then(|m| st.nodes.iter().find(|n| n.name == m.name)) {
                        Some(n) => DevhubAction::Dev(n.name.clone(), !n.dev),
                        None => return,
                    },
                    Some(Hit::Release(i)) => match lease(i) {
                        Some(l) => DevhubAction::Ask(if l.waiting { Confirm::Release(l.id) } else { Confirm::Cancel(l.id) }),
                        None => return,
                    },
                    Some(Hit::Evict(i)) => match lease(i) {
                        Some(l) => DevhubAction::Ask(Confirm::Evict(if l.agent.is_empty() { l.lane } else { l.agent })),
                        None => return,
                    },
                    Some(Hit::Override(mi)) => match st.machines.get(mi) {
                        Some(m) => DevhubAction::Ask(Confirm::Override(m.name.clone(), None)),
                        None => return,
                    },
                    Some(Hit::ClearJit(mi)) => match st.machines.get(mi) {
                        Some(m) => DevhubAction::ClearJit(m.name.clone(), None),
                        None => return,
                    },
                    Some(Hit::Confirm) => match &st.confirm {
                        Some(c) => DevhubAction::Confirmed(c.clone()),
                        None => return,
                    },
                    Some(Hit::Cancel) => DevhubAction::Cancel,
                    Some(Hit::Unblock(i)) => match st.blocked.get(i) {
                        Some(b) => DevhubAction::Unblock(b.clone()),
                        None => return,
                    },
                    None => return,
                };
                cx.widget_action(self.uid, action);
            }
            Event::Scroll(e) => {
                let max = (self.content_h - self.view_h).max(0.0);
                let next = (self.scroll + e.scroll.y).clamp(0.0, max);
                if next != self.scroll {
                    self.scroll = next;
                    self.redraw(cx);
                }
            }
            _ => {}
        }
    }
}
