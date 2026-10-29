//! The task-manager view: a machine's header (name, LAN address, CPU / RAM / GPU as live graphs,
//! its dev switch and leases) and a card per running JIT (state, uptime, fps, memory, its
//! builds so far, and live graphs of its compiles per second by tier, its frame times against
//! the display's budget and its CPU). The node's tray dropdown is this view; the master's panel
//! draws the same card for the selected JIT ([`jit_card`]).

use crate::graph::{self, Palette};
use makepad_shell_kit::graph::DrawShellTrace;
use crate::live::{Board, JitLive, MachineLive, WINDOW_S};
use crate::view::dur;
use makepad_shell_kit::ui::{contains, rect, DrawShellFill, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{alpha, CtrlState, ShellTokens};
use makepad_widgets::*;
use std::sync::{Arc, Mutex};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DevhubTasksBase = #(DevhubTasks::register_widget(vm))
    mod.widgets.DevhubTasks = set_type_default() do mod.widgets.DevhubTasksBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
        g +: {}
    }
}

const PAD: f64 = 16.0;
const GAP: f64 = 8.0;
const WELL_H: f64 = 86.0;

/// The machine the view is about.
#[derive(Clone, Debug, Default)]
pub struct Head {
    pub name: String,
    /// `LAN 10.0.0.165:7171`, or why it has none
    pub lan: String,
    /// the dev switch (writes from the network allowed)
    pub dev: bool,
    /// (id, lane, kind)
    pub leases: Vec<(String, String, &'static str)>,
    /// the JIT cache clear: its confirmation and its last outcome
    pub cache: CacheUi,
}

/// The node's JIT cache clear, as the footer shows it.
#[derive(Clone, Debug, Default)]
pub struct CacheUi {
    /// sized and waiting for the click that confirms: (question, force)
    pub ask: Option<(String, bool)>,
    /// sizing / clearing / the outcome
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub enum TasksAction {
    Quit,
    /// the DEV pill: flip this node's dev switch
    ToggleDev,
    /// size this machine's JIT cache, then ask
    ClearJit,
    /// the confirmation: (force)
    ClearJitConfirmed(bool),
    ClearJitCancel,
    #[default]
    None,
}

fn k(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn rate(v: f32) -> String {
    if v >= 1000.0 {
        format!("{:.1}k", v / 1000.0)
    } else {
        format!("{v:.0}")
    }
}

/// The columns of wells that fit `w`.
fn columns(w: f64) -> usize {
    ((w + GAP) / (150.0 + GAP)).floor().clamp(1.0, 3.0) as usize
}

/// `n` well rects in rows of `cols` starting at `y`; returns them and the y under them.
fn grid(x: f64, y: f64, w: f64, n: usize, cols: usize) -> (Vec<Rect>, f64) {
    let cw = (w - GAP * (cols as f64 - 1.0)) / cols as f64;
    let rects = (0..n).map(|i| rect(x + (i % cols) as f64 * (cw + GAP), y + (i / cols) as f64 * (WELL_H + GAP), cw, WELL_H)).collect();
    let rows = n.div_ceil(cols);
    (rects, y + rows as f64 * (WELL_H + GAP) - GAP)
}

/// The board's time now, a little behind the sampler so the newest sample is already in.
pub fn view_now(board: &Board) -> f64 {
    board.t0.map_or(board.now, |t| t.elapsed().as_secs_f64() - 0.06)
}

/// One JIT's card in `r` (its width; the height it takes is returned). `cpu_text` stands in for
/// the CPU graph where the OS numbers are another machine's (the master's routed view).
#[allow(clippy::too_many_arguments)]
pub fn jit_card(d: &mut ShellDraw, g: &mut DrawShellTrace, cx: &mut Cx2d, tok: &ShellTokens, p: &Palette, r: Rect, j: &JitLive, now: f64, cpu_text: Option<&str>) -> f64 {
    let x = r.pos.x;
    let w = r.size.x;
    let mut y = r.pos.y;
    // name, pid, state
    let busy = j.busy(now);
    let dot = if j.err.is_some() { p.warn } else if busy { p.accent } else { p.ok };
    d.solid(cx, rect(x, y + 7.0, 7.0, 7.0), dot);
    let name = j.app.trim_start_matches("makepad-app-");
    let nw = d.measure(cx, true, tok.font.body, name).min(w * 0.6);
    d.label_elided(cx, rect(x + 14.0, y, nw, 20.0), true, tok.font.body, p.fg, HAlign::Left, name);
    d.label(cx, rect(x + 14.0 + nw + 8.0, y, 80.0, 20.0), false, tok.font.caption, p.tertiary, HAlign::Left, &format!("#{}", j.pid));
    let state = if j.err.is_some() { "no live data".to_string() } else { format!("{}{}", if busy { "busy" } else { "idle" }, if j.warm() { " \u{b7} warm" } else { " \u{b7} cold" }) };
    d.label(cx, rect(x, y, w, 20.0), true, tok.font.caption, if busy { p.accent } else { p.secondary }, HAlign::Right, &state.to_uppercase());
    y += 22.0;
    // the facts line, wrapped
    let mut facts = Vec::new();
    if j.started_ms > 0 {
        facts.push(format!("up {}", dur(makepad_devhub::proto::wall_ms().saturating_sub(j.started_ms) / 1000)));
    }
    if let Some(f) = j.fps.filter(|f| *f >= 0.5) {
        facts.push(format!("{f:.0} fps"));
    }
    if j.rss_mb > 0 {
        facts.push(if j.rss_mb >= 1024 { format!("{:.1} GB", j.rss_mb as f64 / 1024.0) } else { format!("{} MB", j.rss_mb) });
    }
    if j.threads > 0 {
        facts.push(format!("{} threads", j.threads));
    }
    if j.err.is_none() {
        facts.push(format!("{} fns compiled, tiered up O2 {} \u{b7} O3 {}", k(j.first), k(j.o2), k(j.o3)));
        if j.linked > 0 {
            facts.push(format!("{} from cache", k(j.linked)));
        }
        if j.queued > 0 {
            facts.push(format!("{} queued", k(j.queued)));
        }
    }
    let lines = d.wrap(cx, false, tok.font.caption, &facts.join("  \u{b7}  "), w - 14.0, 2);
    for l in &lines {
        d.label(cx, rect(x + 14.0, y, w - 14.0, 16.0), false, tok.font.caption, p.secondary, HAlign::Left, l);
        y += 16.0;
    }
    if let Some(e) = &j.err {
        let lines = d.wrap(cx, false, tok.font.caption, e, w - 14.0, 2);
        for l in &lines {
            d.label(cx, rect(x + 14.0, y, w - 14.0, 16.0), false, tok.font.caption, p.warn, HAlign::Left, l);
            y += 16.0;
        }
        return y - r.pos.y;
    }
    y += 6.0;
    let (wells, bottom) = grid(x, y, w, 3, columns(w));
    // compiles per second: first compiles (accent) and O2 / O3 tier-ups (violet)
    let last = j.compiles.back().copied().unwrap_or((now, 0.0, 0.0));
    let peak = j.compiles.iter().filter(|c| now - c.0 <= WINDOW_S).map(|c| c.1.max(c.2)).fold(0.0f32, f32::max);
    let value = format!("{}/s", rate(last.1 + last.2));
    let peak_note = format!("peak {}", rate(peak));
    let plot = graph::well(d, cx, tok, p, wells[0], "Builds", &value, p.fg, &[(p.accent, "first"), (p.tier, "O2/O3")], Some((&peak_note, p.tertiary)));
    cx.push_clip_rect(plot);
    let max = (peak * 1.15).max(10.0);
    graph::trace(g, cx, plot, &mut j.compiles.iter().map(|c| (c.0, c.1)), now, max, 1.0, p.accent, 0.30);
    graph::trace(g, cx, plot, &mut j.compiles.iter().map(|c| (c.0, c.2)), now, max, 1.0, p.tier, 0.22);
    cx.pop_clip_rect();
    // frame times
    let frames: Vec<_> = j.frames.iter().copied().collect();
    let budget = graph::budget_ms(&frames);
    let recent: Vec<f32> = j.frames.iter().rev().take_while(|f| now - f.at <= 1.0).filter(|f| !graph::idle_gap(f)).map(|f| f.interval_ms).collect();
    let worst = j.frames.iter().filter(|f| now - f.at <= WINDOW_S && !graph::idle_gap(f)).map(|f| f.interval_ms).fold(0.0f32, f32::max);
    let (value, vc) = if recent.is_empty() {
        ("\u{2014}".to_string(), p.secondary)
    } else {
        let avg = recent.iter().sum::<f32>() / recent.len() as f32;
        (format!("{avg:.1} ms"), if avg > budget * 1.5 { p.warn } else { p.fg })
    };
    let worst_note = format!("worst {worst:.0} ms");
    let note = (worst > budget * 1.5).then_some((worst_note.as_str(), if worst > budget * 3.0 { p.err } else { p.warn }));
    let plot = graph::well(d, cx, tok, p, wells[1], "Frame time", &value, vc, &[(alpha(p.frame, 0.35), "interval"), (p.frame, "work")], note);
    cx.push_clip_rect(plot);
    if j.frames.is_empty() {
        graph::note(d, cx, tok, p, plot, "no frames drawn");
    } else {
        graph::frame_bars(d, cx, tok, p, plot, &j.frames, now, budget);
    }
    cx.pop_clip_rect();
    // CPU
    let cpu_now = j.cpu_now.map(|c| format!("{c:.0}%"));
    let value = cpu_now.as_deref().or(cpu_text).unwrap_or("\u{2014}").to_string();
    let cpu_peak = j.cpu.iter().map(|c| c.1).fold(0.0f32, f32::max);
    let scale = ((cpu_peak / 100.0).ceil() * 100.0).max(100.0);
    let scale_note = format!("top {scale:.0}%");
    let plot = graph::well(d, cx, tok, p, wells[2], "CPU", &value, p.fg, &[(p.cpu, "1 core = 100%")], Some((&scale_note, p.tertiary)));
    cx.push_clip_rect(plot);
    if j.cpu.is_empty() {
        graph::note(d, cx, tok, p, plot, if cpu_text.is_some() { "sampled on its machine" } else { "measuring\u{2026}" });
    } else {
        graph::trace(g, cx, plot, &mut j.cpu.iter().copied(), now, scale, 1.5, p.cpu, 0.26);
    }
    cx.pop_clip_rect();
    bottom - r.pos.y
}

#[derive(Script, ScriptHook, Widget)]
pub struct DevhubTasks {
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
    g: DrawShellTrace,
    #[live]
    tokens: ShellTokens,
    /// the sampler's rings, read under its lock while drawing (the sampler holds it briefly)
    #[rust]
    pub board: Option<Arc<Mutex<Board>>>,
    #[rust]
    pub head: Head,
    #[rust]
    quit: Rect,
    /// the DEV pill (a button: the node's own dev switch)
    #[rust]
    dev_pill: Rect,
    #[rust]
    mouse: Vec2d,
    /// the footer's JIT cache buttons: clear (or confirm), cancel
    #[rust]
    clear: Rect,
    #[rust]
    cancel: Rect,
    #[rust]
    hot: bool,
    #[rust]
    hot_clear: u8,
    #[rust]
    scroll: f64,
    /// the content's height at the last draw (the dropdown fits itself to it)
    #[rust]
    pub content_h: f64,
    #[rust]
    view_h: f64,
    /// draw cost: (frames, total s) since the last report (MAKEPAD_DEVHUB_DRAWSTATS=1)
    #[rust]
    stats: (u32, f64, Option<std::time::Instant>),
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

impl DevhubTasks {
    fn machine_wells(&mut self, cx: &mut Cx2d, p: &Palette, x: f64, y: f64, w: f64, now: f64, m: &MachineLive) -> f64 {
        let tok = self.tokens;
        let (wells, bottom) = grid(x, y, w, 3, columns(w));
        let cpu = m.cpu.back().map_or("\u{2014}".to_string(), |c| format!("{:.0}%", c.1));
        let cores = format!("{} cores", m.cpus);
        let load = format!("load {:.1}", m.load1);
        let plot = graph::well(&mut self.d, cx, &tok, p, wells[0], "CPU", &cpu, p.fg, &[(p.cpu, &cores)], Some((&load, p.tertiary)));
        cx.push_clip_rect(plot);
        graph::trace(&mut self.g, cx, plot, &mut m.cpu.iter().copied(), now, 100.0, 1.5, p.cpu, 0.26);
        cx.pop_clip_rect();
        let used = m.mem_total_mb.saturating_sub(m.mem_avail_mb) as f64 / 1024.0;
        let total = format!("of {:.0} GB", m.mem_total_mb as f64 / 1024.0);
        let plot = graph::well(&mut self.d, cx, &tok, p, wells[1], "Memory", &format!("{used:.1} GB"), p.fg, &[(p.tier, "in use")], Some((&total, p.tertiary)));
        cx.push_clip_rect(plot);
        graph::trace(&mut self.g, cx, plot, &mut m.mem.iter().copied(), now, 100.0, 1.5, p.tier, 0.22);
        cx.pop_clip_rect();
        let (value, name, vram) = match m.gpus.first() {
            Some((name, util, used, total)) => (format!("{util:.0}%"), name.trim_start_matches("NVIDIA ").trim_start_matches("GeForce ").to_string(), format!("{used:.1}/{total:.0} GB")),
            None => ("\u{2014}".to_string(), String::new(), String::new()),
        };
        let legend = [(p.accent, name.as_str())];
        let plot = graph::well(&mut self.d, cx, &tok, p, wells[2], "GPU", &value, p.fg, if name.is_empty() { &[] } else { &legend }, (!vram.is_empty()).then_some((vram.as_str(), p.tertiary)));
        cx.push_clip_rect(plot);
        if m.gpus.is_empty() {
            graph::note(&mut self.d, cx, &tok, p, plot, "no GPU numbers here");
        } else {
            graph::trace(&mut self.g, cx, plot, &mut m.gpu.iter().copied(), now, 100.0, 3.0, p.accent, 0.26);
        }
        cx.pop_clip_rect();
        bottom
    }

    fn draw_all(&mut self, cx: &mut Cx2d, window: Rect) {
        let tok = self.tokens;
        let p = Palette::new(&tok);
        self.d.card(cx, window, &tok.popups);
        self.view_h = window.size.y;
        let edge = tok.popups.border_width;
        let x = window.pos.x + PAD + edge;
        let w = window.size.x - 2.0 * (PAD + edge);
        let top = window.pos.y + PAD + edge - self.scroll;
        let mut y = top;
        let source = self.board.clone();
        let empty = Mutex::new(Board::default());
        let guard = source.as_deref().unwrap_or(&empty).lock().unwrap();
        let board: &Board = &guard;
        let now = view_now(board);
        let head = self.head.clone();
        // the machine: name and address, dev switch at the right
        self.d.icon_centered(cx, Ico::Cpu, rect(x, y, 22.0, 40.0), 22.0, p.fg);
        let tx = x + 32.0;
        self.d.label_elided(cx, rect(tx, y + 2.0, w - 32.0 - 90.0, 20.0), true, tok.font.title, p.fg, HAlign::Left, &format!("devhub \u{b7} {}", head.name));
        let n = board.jits.len();
        let meta = format!("{n} JIT{}  \u{b7}  {}", if n == 1 { "" } else { "s" }, head.lan);
        let lines = self.d.wrap(cx, false, tok.font.caption, &meta, w - 32.0, 3);
        for (i, l) in lines.iter().enumerate() {
            self.d.label(cx, rect(tx, y + 22.0 + i as f64 * 15.0, w - 32.0, 15.0), false, tok.font.caption, p.secondary, HAlign::Left, l);
        }
        let extra = 15.0 * lines.len().saturating_sub(1) as f64;
        let dev = if head.dev { "DEV ON" } else { "DEV OFF" };
        let dw = self.d.measure(cx, true, tok.font.caption, dev) + 16.0;
        let pill = rect(x + w - dw, y + 4.0, dw, 18.0);
        self.dev_pill = pill;
        let dstate = if contains(pill, self.mouse) { CtrlState::Hover } else { CtrlState::Normal };
        self.d.button(cx, pill, &tok, dstate, None, dev, tok.font.caption, if head.dev { p.ok } else { p.secondary }, true);
        y += 48.0 + extra;
        y = self.machine_wells(cx, &p, x, y, w, now, &board.machine) + 10.0;
        if !head.leases.is_empty() {
            let list: Vec<String> = head.leases.iter().map(|(id, lane, kind)| format!("{lane} ({kind}, {id})")).collect();
            let text = format!("Leases: {}", list.join("  \u{b7}  "));
            for l in self.d.wrap(cx, false, tok.font.caption, &text, w, 4) {
                self.d.label(cx, rect(x, y, w, 16.0), false, tok.font.caption, p.secondary, HAlign::Left, &l);
                y += 16.0;
            }
            y += 6.0;
        }
        // the JITs
        self.d.separator(cx, rect(x, y, w, 1.0), p.fg, 0.10);
        y += 12.0;
        self.d.label(cx, rect(x, y, w, 16.0), true, tok.font.caption, p.secondary, HAlign::Left, "RUNNING JITS");
        self.d.label(cx, rect(x, y, w, 16.0), false, tok.font.caption, p.tertiary, HAlign::Right, "last 60 s");
        y += 24.0;
        if board.jits.is_empty() {
            self.d.label(cx, rect(x, y, w, 20.0), false, tok.font.body_small, p.secondary, HAlign::Left, "No JITs run here.");
            y += 28.0;
        }
        for (i, j) in board.jits.iter().enumerate() {
            if i > 0 {
                self.d.separator(cx, rect(x, y, w, 1.0), p.fg, 0.06);
                y += 12.0;
            }
            y += jit_card(&mut self.d, &mut self.g, cx, &tok, &p, rect(x, y, w, 0.0), j, now, None) + 14.0;
        }
        // the footer: what is sampled, and Quit
        self.d.separator(cx, rect(x, y, w, 1.0), p.fg, 0.10);
        y += 10.0;
        // the JIT cache: Clear sizes it, then the same place asks to confirm
        self.cancel = Rect::default();
        match &head.cache.ask {
            Some((question, force)) => {
                let lines = self.d.wrap(cx, false, tok.font.caption, question, w, 6);
                for l in &lines {
                    self.d.label(cx, rect(x, y, w, 16.0), false, tok.font.caption, p.fg, HAlign::Left, l);
                    y += 16.0;
                }
                y += 4.0;
                let verb = if *force { "Clear anyway" } else { "Clear" };
                let bw = self.d.measure(cx, false, tok.font.caption, verb) + 24.0;
                self.clear = rect(x, y, bw, 24.0);
                self.cancel = rect(x + bw + 8.0, y, 80.0, 24.0);
                self.d.button(cx, self.clear, &tok, if self.hot_clear == 1 { CtrlState::Hover } else { CtrlState::Normal }, None, verb, tok.font.caption, p.fg, true);
                self.d.button(cx, self.cancel, &tok, if self.hot_clear == 2 { CtrlState::Hover } else { CtrlState::Normal }, None, "Cancel", tok.font.caption, p.fg, true);
                y += 32.0;
            }
            None => {
                let label = "Clear JIT cache";
                let bw = self.d.measure(cx, false, tok.font.caption, label) + 24.0;
                self.clear = rect(x, y, bw, 24.0);
                self.d.button(cx, self.clear, &tok, if self.hot_clear == 1 { CtrlState::Hover } else { CtrlState::Normal }, None, label, tok.font.caption, p.fg, true);
                self.d.label_elided(cx, rect(x + bw + 8.0, y, w - bw - 8.0, 24.0), false, tok.font.caption, p.secondary, HAlign::Left, &head.cache.note);
                y += 32.0;
            }
        }
        let label = "Quit devhub";
        let bw = self.d.measure(cx, false, tok.font.caption, label) + 24.0;
        self.quit = rect(x + w - bw, y, bw, 24.0);
        let state = if self.hot { CtrlState::Hover } else { CtrlState::Normal };
        self.d.button(cx, self.quit, &tok, state, None, label, tok.font.caption, p.fg, true);
        self.d.label_elided(cx, rect(x, y, w - bw - 8.0, 24.0), false, tok.font.caption, p.tertiary, HAlign::Left, "live while open \u{b7} 20 Hz \u{b7} sampling stops when closed");
        y += 24.0;
        self.content_h = y + PAD + edge + self.scroll - window.pos.y;
    }
}

impl Widget for DevhubTasks {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let t = std::time::Instant::now();
        cx.begin_turtle(walk, self.layout);
        let window = cx.turtle().rect();
        self.draw_all(cx, window);
        cx.end_turtle_with_area(&mut self.area);
        static STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *STATS.get_or_init(|| std::env::var_os("MAKEPAD_DEVHUB_DRAWSTATS").is_some()) {
            self.stats.0 += 1;
            self.stats.1 += t.elapsed().as_secs_f64();
            let since = *self.stats.2.get_or_insert_with(std::time::Instant::now);
            if since.elapsed().as_secs_f64() >= 5.0 {
                log!("devhub draw: {} frames in 5 s, {:.2} ms each (the view's draw on the UI thread)", self.stats.0, self.stats.1 * 1000.0 / self.stats.0.max(1) as f64);
                self.stats = (0, 0.0, Some(std::time::Instant::now()));
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::MouseMove(e) => {
                self.mouse = e.abs;
                let hot = contains(self.quit, e.abs);
                let hot_clear = if contains(self.clear, e.abs) { 1 } else if contains(self.cancel, e.abs) { 2 } else { 0 };
                if hot != self.hot || hot_clear != self.hot_clear {
                    self.hot = hot;
                    self.hot_clear = hot_clear;
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) if contains(self.quit, e.abs) => cx.widget_action(self.uid, TasksAction::Quit),
            Event::MouseDown(e) if contains(self.dev_pill, e.abs) => cx.widget_action(self.uid, TasksAction::ToggleDev),
            Event::MouseDown(e) if contains(self.clear, e.abs) => {
                let a = match &self.head.cache.ask {
                    Some((_, force)) => TasksAction::ClearJitConfirmed(*force),
                    None => TasksAction::ClearJit,
                };
                cx.widget_action(self.uid, a);
            }
            Event::MouseDown(e) if contains(self.cancel, e.abs) => cx.widget_action(self.uid, TasksAction::ClearJitCancel),
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
