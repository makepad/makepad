//! The master's panel: the inbox (what needs a look, newest first; clear one or all), the
//! explorer (machines → JITs with process and JIT state; a click opens a JIT's inspect views;
//! per node its capabilities and the dev switch) and the lease queue (positions and reasons).
//! Drawn with the shell kit, like the AI Hub's panel.

use makepad_devhub::explorer::{JitRow, MachineRow};
use makepad_devhub::lease::sched::NodeView;
use makepad_shell_kit::ui::{contains, cut_top, rect, DrawShellFill, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DevhubViewBase = #(DevhubView::register_widget(vm))
    mod.widgets.DevhubView = set_type_default() do mod.widgets.DevhubViewBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Inbox,
    Explorer,
    Leases,
}

/// The inspect views a JIT row opens (any inspect command works from the CLI).
pub const VIEWS: &[&str] = &["jit", "threads", "memory", "cache", "events", "why-slow"];

#[derive(Clone, Debug, Default)]
pub enum DevhubAction {
    Tab(Tab),
    Clear(String),
    ClearAll,
    SeenAll,
    /// a JIT row: (machine, pid)
    Select(String, u32),
    /// an inspect view of the selected JIT
    View(&'static str),
    CloseDetail,
    /// the node's dev switch: (node, on)
    Dev(String, bool),
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

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub tab: Tab,
    pub status: String,
    pub inbox: Vec<InboxRow>,
    pub machines: Vec<MachineRow>,
    pub selected: Option<(String, u32)>,
    pub view: &'static str,
    pub detail: Vec<String>,
    /// the lease scheduler's nodes (caps, dev switch, leases)
    pub nodes: Vec<NodeView>,
    /// live then waiting leases: (line, waiting)
    pub queue: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Tab(Tab),
    Clear(usize),
    ClearAll,
    SeenAll,
    Jit(usize, usize),
    View(&'static str),
    CloseDetail,
    Dev(usize),
}

const PAD: f64 = 14.0;

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
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

fn dur(s: u64) -> String {
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

fn jit_line(j: &JitRow) -> String {
    let cpu = j.cpu_pct.map_or("-".to_string(), |c| format!("{c:.0}%"));
    let kind = if j.jit { "" } else { " (native)" };
    let state: String = j.state.chars().take(60).collect();
    format!("{} #{}{kind} \u{b7} up {} \u{b7} cpu {cpu} \u{b7} {} MB \u{b7} {} thr \u{b7} {state}", j.app, j.pid, dur(j.up_s), j.rss_mb, j.threads)
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

    fn draw_all(&mut self, cx: &mut Cx2d, window: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.4);
        self.hits.clear();
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
        let waiting = st.queue.iter().filter(|q| q.1).count();
        let tabs = [
            (Tab::Inbox, if new > 0 { format!("Inbox ({new})") } else { "Inbox".to_string() }),
            (Tab::Explorer, format!("Explorer ({})", st.machines.len())),
            (Tab::Leases, if waiting > 0 { format!("Leases ({}, {waiting} wait)", st.queue.len() - waiting) } else { format!("Leases ({})", st.queue.len()) }),
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

        match st.tab {
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
                    let color = match e.sev.as_str() {
                        "err" => vec4(1.0, 0.5, 0.42, 1.0),
                        "req" => vec4(0.5, 0.75, 1.0, 1.0),
                        _ => vec4(1.0, 0.8, 0.4, 1.0),
                    };
                    let text_color = if e.seen { dim } else { fg };
                    self.d.solid(cx, rect(row.pos.x, row.pos.y + 9.0, 6.0, 6.0), alpha(color, if e.seen { 0.45 } else { 1.0 }));
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
            Tab::Leases => {}
            Tab::Explorer => {
                for (mi, m) in st.machines.iter().enumerate() {
                    let (row, r2) = cut_top(rest, 26.0);
                    rest = r2;
                    let head = match &m.err {
                        Some(e) => format!("{} \u{b7} {}", m.name, e),
                        None => format!(
                            "{}{} \u{b7} {} \u{b7} load {:.1}/{} \u{b7} {}/{} GB free \u{b7} {} JITs",
                            m.name,
                            if m.local { " (this Mac)" } else { "" },
                            m.os,
                            m.load1,
                            m.cpus,
                            m.mem_avail_mb / 1024,
                            m.mem_total_mb / 1024,
                            m.jits.iter().filter(|j| j.jit).count()
                        ),
                    };
                    self.d.label_elided(cx, row, true, tok.font.body_small, if m.err.is_some() { dim } else { fg }, HAlign::Left, &head);
                    // a lease node: its capabilities, its state and the user's dev switch (user
                    // WMs and devices start off)
                    if let Some(n) = st.nodes.iter().find(|n| n.name == m.name) {
                        let (row, r2) = cut_top(rest, 22.0);
                        rest = r2;
                        let line = format!("{} \u{b7} {} \u{b7} {}", n.role().as_str(), n.state_word(), n.caps_line());
                        self.d.label_elided(cx, rect(row.pos.x + 12.0, row.pos.y, row.size.x - 112.0, row.size.y), false, tok.font.caption, if n.dev { fg } else { dim }, HAlign::Left, &line);
                        let r = rect(row.pos.x + row.size.x - 96.0, row.pos.y + 1.0, 96.0, 20.0);
                        let hit = Hit::Dev(mi);
                        self.d.button(cx, r, &tok, self.ctrl(&hit, n.dev), None, if n.dev { "dev: on" } else { "dev: off" }, tok.font.caption, fg, true);
                        self.hits.push((hit, r));
                    }
                    for (ji, j) in m.jits.iter().enumerate() {
                        let (row, r2) = cut_top(rest, 22.0);
                        rest = r2;
                        let hit = Hit::Jit(mi, ji);
                        let sel = st.selected.as_ref() == Some(&(m.name.clone(), j.pid));
                        if sel || self.hot.as_ref() == Some(&hit) {
                            self.d.button(cx, row, &tok, self.ctrl(&hit, sel), None, "", tok.font.caption, fg, false);
                        }
                        self.d.label_elided(cx, rect(row.pos.x + 12.0, row.pos.y, row.size.x - 12.0, row.size.y), false, tok.font.caption, if j.jit { fg } else { dim }, HAlign::Left, &jit_line(j));
                        self.hits.push((hit, row));
                    }
                    rest = cut_top(rest, 6.0).1;
                }
                if let Some((machine, pid)) = &st.selected {
                    let (sep, r2) = cut_top(rest, 10.0);
                    rest = r2;
                    self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 5.0, sep.size.x, 1.0), fg, 0.12);
                    let (row, r2) = cut_top(rest, 22.0);
                    rest = r2;
                    self.d.section_header(cx, row, &tok, fg, &format!("{machine}/{pid}: {}", st.view));
                    let r = rect(row.pos.x + row.size.x - 22.0, row.pos.y, 22.0, 20.0);
                    self.d.button(cx, r, &tok, self.ctrl(&Hit::CloseDetail, false), Some(Ico::Close), "", tok.font.caption, fg, false);
                    self.hits.push((Hit::CloseDetail, r));
                    let (row, r2) = cut_top(rest, 30.0);
                    rest = r2;
                    let w = (row.size.x / VIEWS.len() as f64).min(90.0);
                    for (i, v) in VIEWS.iter().enumerate() {
                        let r = rect(row.pos.x + i as f64 * w + 1.0, row.pos.y + 3.0, w - 2.0, 24.0);
                        let hit = Hit::View(v);
                        self.d.button(cx, r, &tok, self.ctrl(&hit, st.view == *v), None, v, tok.font.body_small, fg, true);
                        self.hits.push((hit, r));
                    }
                    for l in st.detail.iter().take(60) {
                        let (row, r2) = cut_top(rest, 17.0);
                        rest = r2;
                        self.d.label_elided(cx, row, false, tok.font.caption, fg, HAlign::Left, l);
                    }
                }
            }
        }
        if st.tab == Tab::Leases {
            if st.queue.is_empty() {
                let (row, r2) = cut_top(rest, 24.0);
                rest = r2;
                self.d.label(cx, row, false, tok.font.caption, dim, HAlign::Left, "No leases.");
            }
            for (line, waiting) in st.queue.iter().take(300) {
                let (row, r2) = cut_top(rest, 22.0);
                rest = r2;
                let color = if *waiting { vec4(1.0, 0.8, 0.4, 1.0) } else { vec4(0.45, 0.85, 0.5, 1.0) };
                self.d.solid(cx, rect(row.pos.x, row.pos.y + 8.0, 6.0, 6.0), color);
                self.d.label_elided(cx, rect(row.pos.x + 12.0, row.pos.y, row.size.x - 12.0, row.size.y), false, tok.font.caption, fg, HAlign::Left, line);
            }
        }
        self.content_h = rest.pos.y + pad + self.scroll - window.pos.y;
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
                let action = match self.hit_at(e.abs) {
                    Some(Hit::Tab(t)) => DevhubAction::Tab(t),
                    Some(Hit::Clear(i)) => match st.inbox.get(i) {
                        Some(r) => DevhubAction::Clear(r.id.clone()),
                        None => return,
                    },
                    Some(Hit::ClearAll) => DevhubAction::ClearAll,
                    Some(Hit::SeenAll) => DevhubAction::SeenAll,
                    Some(Hit::Jit(mi, ji)) => match st.machines.get(mi).and_then(|m| m.jits.get(ji).map(|j| (m.name.clone(), j.pid))) {
                        Some((m, pid)) => DevhubAction::Select(m, pid),
                        None => return,
                    },
                    Some(Hit::View(v)) => DevhubAction::View(v),
                    Some(Hit::CloseDetail) => DevhubAction::CloseDetail,
                    Some(Hit::Dev(mi)) => match st.machines.get(mi).and_then(|m| st.nodes.iter().find(|n| n.name == m.name)) {
                        Some(n) => DevhubAction::Dev(n.name.clone(), !n.dev),
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

