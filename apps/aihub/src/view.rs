//! The AI Hub panel: the serving controls (Enabled, each backend with its inactivity
//! turn-off, the address), the fleet as this hub hears it, then the shared hub panel
//! (`makepad-ai-hub-panel`, the one the WM's top bar shows), all drawn with the shell kit.
//! The tray dropdown draws it as the WM's flyout card (`card: true`: the popups surface with
//! its border), the window on the plain popups ground.

use crate::serve::{Phase, Published};
use crate::settings::{Backend, IdleOff, Settings};
use crate::usage::{Provider, ProviderState};
use std::sync::Arc;

#[path = "usage_view.rs"]
mod usage_view;
use makepad_ai_hub_panel::hub_control::{HubCommand, HubSnapshot};
use makepad_ai_hub_panel::HubPanel;
use makepad_shell_kit::graph::DrawShellTrace;
use makepad_shell_kit::ui::{contains, cut_top, inset, rect, DrawShellFill, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.HubAppViewBase = #(HubAppView::register_widget(vm))
    mod.widgets.HubAppView = set_type_default() do mod.widgets.HubAppViewBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
        g +: {}
    }
}

#[derive(Clone, Debug, Default)]
pub enum HubAppAction {
    SetEnabled(bool),
    SetService(Backend, bool),
    SetIdleOff(Backend, IdleOff),
    /// "Open at login" (where the app offers it)
    SetLogin(bool),
    /// The Usage section's switch: read the CLIs' account usage or not.
    SetUsage(bool),
    /// End a tool's warm CLI and start a fresh one.
    Reconnect(Provider),
    /// Spend one of this account's free resets of its limits (the person confirmed it).
    ResetLimits(Provider, String),
    Hub(HubCommand),
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Enabled,
    Service(Backend),
    IdleOff(Backend, IdleOff),
    Login,
    Usage,
    Reconnect(Provider),
    /// An account's line (an index into the drawn lines): opens or closes its detail.
    Row(usize),
    /// The line's Reset limits button: asks first.
    Reset(usize),
    ResetConfirm(usize),
    ResetCancel(usize),
}

/// What the window shows, set by the app from its settings and the hub owner.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub settings: Settings,
    pub published: Published,
    pub snapshot: HubSnapshot,
    /// Unix ms now, for the "turned off at" / "turns off in" lines.
    pub now_ms: u64,
    /// When each backend's inactivity turn-off fires, if it is on and has a limit.
    pub off_at: Vec<(Backend, u64)>,
    /// "Open at login": None = not offered here, else the switch's state.
    pub login: Option<bool>,
    /// Each tool's usage as its worker last published it.
    pub usage: Vec<Arc<ProviderState>>,
}

const PAD: f64 = 16.0;

#[derive(Script, ScriptHook, Widget)]
pub struct HubAppView {
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
    /// the usage history's sparklines
    #[live]
    g: DrawShellTrace,
    /// drawn as a flyout card (the tray dropdown)
    #[live]
    card: bool,
    #[rust]
    pub state: ViewState,
    #[rust]
    panel: HubPanel,
    #[rust]
    hits: Vec<(Hit, Rect)>,
    #[rust]
    hot: Option<Hit>,
    #[rust]
    scroll: f64,
    #[rust]
    content_h: f64,
    /// the last selftest line about the login switch
    #[rust]
    noted: String,
    #[rust]
    view_h: f64,
    /// The account lines as last drawn, for their hits.
    #[rust]
    rows: Vec<(Provider, String)>,
    /// The account line whose detail is open.
    #[rust]
    expanded: Option<(Provider, String)>,
    /// The account line asking "Use 1 of N resets?".
    #[rust]
    confirm: Option<(Provider, String)>,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

impl HubAppView {
    pub fn set_state(&mut self, cx: &mut Cx, state: ViewState) {
        self.panel.update(cx, &state.snapshot);
        self.state = state;
        self.redraw(cx);
    }

    fn hit_at(&self, p: Vec2d) -> Option<Hit> {
        self.hits.iter().rev().find(|(_, r)| contains(*r, p)).map(|(h, _)| *h)
    }

    fn draw_all(&mut self, cx: &mut Cx2d, window: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        self.hits.clear();
        // the card's border is inside the window: the content keeps its padding inside it
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
        let top = rect(window.pos.x + pad, window.pos.y + pad - self.scroll, window.size.x - 2.0 * pad, 1.0e6);
        let st = self.state.clone();

        // The master switch and the address.
        let (row, rest) = cut_top(top, 40.0);
        let (title, meta) = match &st.published.phase {
            Phase::Serving => ("Serving".to_string(), st.published.addr.map(|a| format!("on {a} \u{b7} LAN only, fleet credentials")).unwrap_or_default()),
            Phase::Starting => ("Starting".to_string(), String::new()),
            Phase::Stopping => ("Stopping".to_string(), String::new()),
            Phase::Failed(why) => ("Not serving".to_string(), why.clone()),
            Phase::Off if !st.settings.enabled => ("Off".to_string(), "nothing listens, nothing is announced".to_string()),
            Phase::Off => ("Not serving".to_string(), "no backend is on".to_string()),
        };
        self.d.panel_hero(cx, row, &tok, fg, Ico::Chip, &format!("AI Hub \u{b7} {title}"), &meta, 64.0);
        let sw = self.d.toggle_switch(cx, dvec2(row.pos.x + row.size.x - 48.0, row.pos.y + 10.0), &tok, st.settings.enabled, fg);
        self.hits.push((Hit::Enabled, inset(sw, -6.0)));
        let (sep, mut rest) = cut_top(rest, 12.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);

        // The backends: one row each (its name, the inactivity turn-off as a small joined pill,
        // its switch) over the line that says what it serves or why not. A panel too narrow for
        // the pill beside the name gives the pill a row of its own under that line.
        let (head, r2) = cut_top(rest, 22.0);
        rest = r2;
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Offered to the fleet");
        let pill_w = self.idle_pill_width(cx, &tok);
        for backend in Backend::ALL {
            let svc = *st.settings.service(backend);
            let label_w = self.d.measure(cx, false, tok.font.body, backend.label());
            let inline = idle_pill_inline(rest.size.x, label_w, pill_w);
            let (block, r2) = cut_top(rest, backend_block_height(inline));
            rest = r2;
            let (row, below) = cut_top(block, ROW_H);
            let sw = self.d.toggle_switch(cx, dvec2(row.pos.x + row.size.x - SWITCH_W, row.pos.y + (ROW_H - 22.0) * 0.5), &tok, svc.on, fg);
            self.hits.push((Hit::Service(backend), inset(sw, -6.0)));
            let dim = if st.settings.enabled { fg } else { darker(fg, 1.4) };
            let pill_x = row.pos.x + row.size.x - SWITCH_W - GAP - pill_w;
            let label_right = if inline { pill_x - GAP } else { row.pos.x + row.size.x - SWITCH_W - GAP };
            self.d.label_elided(cx, rect(row.pos.x, row.pos.y, (label_right - row.pos.x).max(0.0), row.size.y), false, tok.font.body, dim, HAlign::Left, backend.label());
            if inline {
                self.idle_pill(cx, &tok, fg, rect(pill_x, row.pos.y + (ROW_H - PILL_H) * 0.5, pill_w, PILL_H), backend, svc.idle_off);
            }
            let line = state_line(&st, backend);
            let (row, below) = cut_top(below, LINE_H);
            self.d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.2), HAlign::Left, &line);
            if !inline {
                self.idle_pill(cx, &tok, fg, rect(below.pos.x, below.pos.y + 2.0, pill_w.min(below.size.x), PILL_H), backend, svc.idle_off);
            }
        }

        // Open at login (an installed bundle only).
        if let Some(on) = st.login {
            let (row, r2) = cut_top(rest, ROW_H);
            rest = r2;
            let sw = self.d.toggle_switch(cx, dvec2(row.pos.x + row.size.x - SWITCH_W, row.pos.y + (ROW_H - 22.0) * 0.5), &tok, on, fg);
            self.hits.push((Hit::Login, inset(sw, -6.0)));
            self.d.label_elided(cx, rect(row.pos.x, row.pos.y, row.size.x - SWITCH_W - GAP, row.size.y), false, tok.font.body, fg, HAlign::Left, "Open at login");
            let note = format!("login switch at ({}, {}) in the window, {}", sw.pos.x + sw.size.x * 0.5 - window.pos.x, sw.pos.y + sw.size.y * 0.5 - window.pos.y, if on { "on" } else { "off" });
            if self.noted != note {
                crate::selftest_note(&note);
                self.noted = note;
            }
        }

        // Which account each coding CLI is signed in as, and its limits.
        rest = self.draw_usage(cx, &tok, fg, rest, &st);

        // The fleet as this hub hears it.
        let (sep, r2) = cut_top(rest, 12.0);
        rest = r2;
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let (head, r2) = cut_top(rest, 22.0);
        rest = r2;
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, "Fleet");
        let nodes = &st.published.nodes;
        let (row, r2) = cut_top(rest, 18.0);
        rest = r2;
        let heard = if nodes.is_empty() { "No hubs heard on the LAN".to_string() } else { format!("{} hub{} heard: {}", nodes.len(), if nodes.len() == 1 { "" } else { "s" }, nodes.iter().map(|n| n.trim_start_matches("https://")).collect::<Vec<_>>().join(", ")) };
        self.d.label_elided(cx, row, false, tok.font.caption, fg, HAlign::Left, &heard);
        let (row, r2) = cut_top(rest, 20.0);
        rest = r2;
        self.d.label(cx, row, true, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Connected");
        if st.published.clients.is_empty() {
            let (row, r2) = cut_top(rest, 18.0);
            rest = r2;
            self.d.label(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Nobody in the last 10 min");
        }
        for c in st.published.clients.iter().take(6) {
            let (row, r2) = cut_top(rest, 18.0);
            rest = r2;
            let app = if c.app.is_empty() { String::new() } else { format!(" \u{b7} {}", c.app) };
            let ago = ago(c.ago_ms);
            let aw = self.d.measure(cx, false, tok.font.caption, &ago);
            self.d.label_elided(cx, rect(row.pos.x, row.pos.y, (row.size.x - aw - 10.0).max(0.0), row.size.y), false, tok.font.caption, fg, HAlign::Left, &format!("{} at {}{} \u{b7} {} requests", c.client, c.addr, app, c.requests));
            self.d.label(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Right, &ago);
        }
        if !st.published.failed.is_empty() {
            let (row, r2) = cut_top(rest, 20.0);
            rest = r2;
            self.d.label(cx, row, true, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Recent errors");
            for f in st.published.failed.iter().take(4) {
                let (row, r2) = cut_top(rest, 18.0);
                rest = r2;
                let ago = ago(st.now_ms.saturating_sub(f.finished_ms));
                let aw = self.d.measure(cx, false, tok.font.caption, &ago);
                self.d.label_elided(cx, rect(row.pos.x, row.pos.y, (row.size.x - aw - 10.0).max(0.0), row.size.y), false, tok.font.caption, alpha(vec4(1.0, 0.55, 0.45, 1.0), 1.0), HAlign::Left, &format!("{} for {}: {}", f.model, f.client, first_line(&f.error)));
                self.d.label(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Right, &ago);
            }
        }
        let (sep, rest) = cut_top(rest, 16.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 8.0, sep.size.x, 1.0), fg, 0.12);

        // The hub itself: the same panel as the WM's top bar.
        let h = self.panel.height(&st.snapshot);
        let body = rect(rest.pos.x, rest.pos.y, rest.size.x, h);
        self.panel.draw(cx, &mut self.d, &tok, &st.snapshot, body);
        self.content_h = body.pos.y + h + pad + self.scroll - window.pos.y;
    }

    /// The height everything takes (the last draw's), for a dropdown sized to its content.
    pub fn content_height(&self) -> f64 {
        self.content_h
    }

    /// The inactivity turn-off's width: its caption and the joined segments.
    fn idle_pill_width(&mut self, cx: &mut Cx2d, tok: &ShellTokens) -> f64 {
        let caption = self.d.measure(cx, false, tok.font.caption, IDLE_CAPTION) + 6.0;
        caption + IdleOff::ALL.into_iter().map(|i| self.d.measure(cx, true, tok.font.body_small, i.short()) + 2.0 * SEG_PAD).sum::<f64>()
    }

    /// "Off after [15m|1h|4h|never]": one joined pill, the choice filled.
    #[allow(clippy::too_many_arguments)]
    fn idle_pill(&mut self, cx: &mut Cx2d, tok: &ShellTokens, fg: Vec4f, r: Rect, backend: Backend, chosen: IdleOff) {
        let caption = self.d.measure(cx, false, tok.font.caption, IDLE_CAPTION) + 6.0;
        self.d.label(cx, rect(r.pos.x, r.pos.y, caption, r.size.y), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, IDLE_CAPTION);
        let x0 = r.pos.x + caption;
        let widths: Vec<f64> = IdleOff::ALL.into_iter().map(|i| self.d.measure(cx, true, tok.font.body_small, i.short()) + 2.0 * SEG_PAD).collect();
        let total: f64 = widths.iter().sum();
        self.d.control(cx, rect(x0, r.pos.y, total, r.size.y), &tok.controls, CtrlState::Normal);
        let mut x = x0;
        for (idle, w) in IdleOff::ALL.into_iter().zip(widths) {
            let seg = rect(x, r.pos.y, w, r.size.y);
            let hit = Hit::IdleOff(backend, idle);
            let state = if chosen == idle { CtrlState::Selected } else if self.hot == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };
            if state != CtrlState::Normal {
                self.d.control(cx, seg, &tok.controls, state);
            }
            self.d.label(cx, seg, state == CtrlState::Selected, tok.font.body_small, fg, HAlign::Center, idle.short());
            self.hits.push((hit, seg));
            x += w;
        }
    }
}

/// The rows of the backend list (the switch row, the state line, the pill's own row when the
/// panel is narrow, the gap after a backend).
const ROW_H: f64 = 24.0;
const LINE_H: f64 = 16.0;
const PILL_H: f64 = 20.0;
const BLOCK_GAP: f64 = 6.0;
/// The toggle switch's track width (the kit's: 1.9 × its 22 px height).
const SWITCH_W: f64 = 42.0;
const GAP: f64 = 10.0;
const SEG_PAD: f64 = 7.0;
const IDLE_CAPTION: &str = "Off after";

/// Whether a backend's inactivity pill fits on its switch row beside its name (else it gets a
/// row of its own under the state line).
fn idle_pill_inline(row_w: f64, label_w: f64, pill_w: f64) -> bool {
    label_w + GAP + pill_w + GAP + SWITCH_W <= row_w
}

/// One backend's height in the list: the switch row and state line, plus the pill's own row on
/// a narrow panel.
fn backend_block_height(inline: bool) -> f64 {
    ROW_H + LINE_H + if inline { 0.0 } else { PILL_H + 4.0 } + BLOCK_GAP
}

/// The line under a backend's switch: what it serves now, why not, or when it turned off.
fn state_line(st: &ViewState, backend: Backend) -> String {
    let svc = st.settings.service(backend);
    let p = &st.published;
    if let Some(at) = svc.auto_off_at.filter(|_| !svc.on) {
        return format!("Turned off after {} without a request, {} ago; stays off until you turn it on", svc.idle_off.label(), ago(st.now_ms.saturating_sub(at)));
    }
    if !svc.on {
        return match backend {
            Backend::ClaudeLan => "Off: the hub does not offer Claude to the LAN (Sandboxes drop the row)".to_string(),
            Backend::ClaudeIntake => "Off: the hub reviews no PR (the intake's reviews wait)".to_string(),
            Backend::Local => match p.local_models.len() {
                0 => "Off: no local model on disk yet (Download one under Models)".to_string(),
                n => format!("Off \u{b7} {n} on disk: {}", p.local_models.join(", ")),
            },
        };
    }
    if let Some((_, note)) = p.notes.iter().find(|(b, _)| *b == backend) {
        return format!("Not serving: {}", first_line(note));
    }
    let mut line = if !st.settings.enabled {
        "On, but the hub is off".to_string()
    } else if p.serving.contains(&backend) {
        match backend {
            Backend::ClaudeLan | Backend::ClaudeIntake => "Serving claude-cli: chat only, no tools, sandboxed".to_string(),
            Backend::Local => format!("Serving {}", p.local_models.join(", ")),
        }
    } else {
        "Starting".to_string()
    };
    if let Some((_, at)) = st.off_at.iter().find(|(b, _)| *b == backend) {
        line.push_str(&format!(" \u{b7} turns off in {} if idle", left(at.saturating_sub(st.now_ms))));
    }
    line
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

fn ago(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

fn left(ms: u64) -> String {
    let m = (ms + 59_999) / 60_000;
    if m < 60 { format!("{m} min") } else { format!("{} h {} min", m / 60, m % 60) }
}

impl Widget for HubAppView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let window = cx.turtle().rect();
        self.draw_all(cx, window);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.panel.is_glow_frame(event) {
            self.redraw(cx);
        }
        match event {
            Event::MouseMove(e) => {
                let hot = self.hit_at(e.abs);
                let hub_hot = self.panel.hover(e.abs);
                if hot != self.hot || hub_hot {
                    self.hot = hot;
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                if self.panel.hit_at_point(e.abs) {
                    if let Some(command) = self.panel.click(e.abs, &self.state.snapshot) {
                        cx.widget_action(self.uid, HubAppAction::Hub(command));
                    }
                    self.redraw(cx);
                    return;
                }
                let hit = self.hit_at(e.abs);
                // a question not answered is dropped by any other click
                if self.confirm.is_some() && !matches!(hit, Some(Hit::ResetConfirm(_) | Hit::ResetCancel(_))) {
                    self.confirm = None;
                    self.redraw(cx);
                }
                let line = |i: usize| self.rows.get(i).cloned();
                let st = &self.state.settings;
                let action = match hit {
                    Some(Hit::Row(i)) => {
                        let line = line(i);
                        self.expanded = if self.expanded == line { None } else { line };
                        self.redraw(cx);
                        return;
                    }
                    Some(Hit::Reset(i)) => {
                        self.confirm = line(i);
                        self.redraw(cx);
                        return;
                    }
                    Some(Hit::ResetCancel(_)) => {
                        self.confirm = None;
                        self.redraw(cx);
                        return;
                    }
                    Some(Hit::ResetConfirm(i)) => {
                        let Some((provider, account)) = line(i).filter(|l| self.confirm.as_ref() == Some(l)) else { return };
                        self.confirm = None;
                        self.redraw(cx);
                        HubAppAction::ResetLimits(provider, account)
                    }
                    Some(Hit::Enabled) => HubAppAction::SetEnabled(!st.enabled),
                    Some(Hit::Service(b)) => HubAppAction::SetService(b, !st.service(b).on),
                    Some(Hit::IdleOff(b, i)) => HubAppAction::SetIdleOff(b, i),
                    Some(Hit::Login) => HubAppAction::SetLogin(!self.state.login.unwrap_or(false)),
                    Some(Hit::Usage) => HubAppAction::SetUsage(!st.usage),
                    Some(Hit::Reconnect(p)) => HubAppAction::Reconnect(p),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The user asked for less height here: a backend was 84 px (its switch row 30, the state
    /// line 18, the "Turn off when idle for" row 28, a gap 8).
    #[test]
    fn a_backend_takes_about_half_its_old_height_and_a_narrow_panel_wraps_its_pill() {
        const OLD: f64 = 84.0;
        // The dropdown's content (460 px less its padding) holds the longest name ("Claude for PR
        // reviews", about 125 px in the body font), the pill (about 150 px) and the switch.
        let (name, pill) = (130.0, 155.0);
        assert!(idle_pill_inline(460.0 - 2.0 * PAD, name, pill), "the pill sits on the switch row");
        assert!(backend_block_height(true) <= 0.55 * OLD, "{} px against {OLD}", backend_block_height(true));
        // A narrow window: the pill takes a row of its own rather than running under the name or
        // the switch, and the block still stays under the old height.
        assert!(!idle_pill_inline(300.0, name, pill));
        assert!(backend_block_height(false) < OLD);
    }
}
