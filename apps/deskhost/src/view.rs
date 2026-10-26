//! Desk Host's tray dropdown, drawn with the shell kit like the AI Hub's: the serving switch and
//! where it listens, the viewers (the rows the WM's Desk Link flyout shows: who, watching or not,
//! the stream, Disconnect), the allowed peers (read-only), the OS grants it misses, Open at login
//! and Quit.

use makepad_desk_link::host::{HostState, Report, ViewerRow};
use makepad_shell_kit::ui::{contains, cut_top, inset, rect, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{darker, CtrlState, ShellTokens};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DeskHostViewBase = #(DeskHostView::register_widget(vm))
    mod.widgets.DeskHostView = set_type_default() do mod.widgets.DeskHostViewBase {
        width: Fill
        height: Fill
        d +: {}
    }
}

#[derive(Clone, Debug, Default)]
pub enum DeskHostAction {
    SetServing(bool),
    Disconnect(u32),
    SetLogin(bool),
    Quit,
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Serving,
    Disconnect(u32),
    Login,
    Quit,
}

/// What the dropdown shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub report: Report,
    /// The settings' `enabled` (the switch).
    pub enabled: bool,
    /// "Open at login": None = not offered here, else the switch's state.
    pub login: Option<bool>,
    /// This machine's name (the viewers see it).
    pub host: String,
}

const PAD: f64 = 16.0;

/// The host's viewer as the panel's row: its stream line says what it got over the last second
/// (a still screen sends nothing).
fn panel_row(v: &ViewerRow) -> makepad_desk_link_panel::ViewerRow {
    let base = makepad_desk_link_panel::stream_line(v.hevc, false, v.fps, 0, 1);
    let stream = if v.pictures > 0 { format!("{base} \u{b7} {:.1} Mbit/s \u{b7} {} sent/s", v.bytes as f64 * 8.0 / 1e6, v.pictures) } else { format!("{base} \u{b7} idle (the screen is still)") };
    makepad_desk_link_panel::ViewerRow { session: v.session, name: v.name.clone(), peer: v.peer.clone(), paused: v.paused, secs: v.secs, stream }
}

#[derive(Script, ScriptHook, Widget)]
pub struct DeskHostView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    d: ShellDraw,
    #[live]
    tokens: ShellTokens,
    /// drawn as a flyout card (the tray dropdown)
    #[live]
    card: bool,
    #[rust]
    state: ViewState,
    #[rust]
    hits: Vec<(Hit, Rect)>,
    #[rust]
    hot: Option<Hit>,
    #[rust]
    content_h: f64,
    /// the last selftest line about where the controls are
    #[rust]
    noted: String,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

impl DeskHostView {
    pub fn set_state(&mut self, cx: &mut Cx, state: ViewState) {
        if state != self.state {
            self.state = state;
            self.redraw(cx);
        }
    }

    /// The height everything takes (the last draw's), for a dropdown sized to its content.
    pub fn content_height(&self) -> f64 {
        self.content_h
    }

    fn hit_at(&self, p: Vec2d) -> Option<Hit> {
        self.hits.iter().rev().find(|(_, r)| contains(*r, p)).map(|(h, _)| *h)
    }

    fn section(&mut self, cx: &mut Cx2d, rest: Rect, title: &str) -> Rect {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let (sep, rest) = cut_top(rest, 12.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let (head, rest) = cut_top(rest, 22.0);
        self.d.section_header(cx, rect(head.pos.x, head.pos.y + 4.0, head.size.x, head.size.y - 4.0), &tok, fg, title);
        rest
    }

    fn caption(&mut self, cx: &mut Cx2d, rest: Rect, text: &str, strength: f32) -> Rect {
        let tok = self.tokens;
        let (row, rest) = cut_top(rest, 18.0);
        self.d.label_elided(cx, row, false, tok.font.caption, darker(tok.popups.text, strength), HAlign::Left, text);
        rest
    }

    fn draw_all(&mut self, cx: &mut Cx2d, window: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        self.hits.clear();
        let edge = if self.card {
            self.d.card(cx, window, &tok.popups);
            tok.popups.border_width
        } else {
            0.0
        };
        let pad = PAD + edge;
        let top = rect(window.pos.x + pad, window.pos.y + pad, window.size.x - 2.0 * pad, 1.0e6);
        let st = self.state.clone();
        let r = &st.report;

        // the switch and where it listens
        let (row, rest) = cut_top(top, 40.0);
        let (title, meta) = match (&r.state, st.enabled) {
            (HostState::Serving, _) => ("Serving".to_string(), format!("{} on {} \u{b7} LAN only, fleet credentials", st.host, r.addr)),
            (HostState::Waiting(why), true) => ("Waiting".to_string(), why.lines().next().unwrap_or("").to_string()),
            (_, true) => ("Starting".to_string(), String::new()),
            _ => ("Off".to_string(), "nothing listens".to_string()),
        };
        self.d.panel_hero(cx, row, &tok, fg, Ico::Monitor, &format!("Desk Host \u{b7} {title}"), &meta, 64.0);
        let sw = self.d.toggle_switch(cx, dvec2(row.pos.x + row.size.x - 48.0, row.pos.y + 10.0), &tok, st.enabled, fg);
        self.hits.push((Hit::Serving, inset(sw, -6.0)));
        let mut rest = rest;

        // the viewers
        rest = self.section(cx, rest, "Viewers");
        if r.viewers.is_empty() {
            rest = self.caption(cx, rest, if r.state == HostState::Serving { "Nobody is viewing" } else { "Nobody: not serving" }, 1.4);
        } else {
            let hot = match self.hot {
                Some(Hit::Disconnect(s)) => Some(s),
                _ => None,
            };
            let mut hits = Vec::new();
            let rows: Vec<makepad_desk_link_panel::ViewerRow> = r.viewers.iter().map(panel_row).collect();
            rest = makepad_desk_link_panel::draw_viewers(cx, &mut self.d, &tok, rest, &rows, hot, &mut hits);
            self.hits.extend(hits.into_iter().map(|(s, r)| (Hit::Disconnect(s), r)));
        }

        // who may connect (read-only)
        rest = self.section(cx, rest, "Who may watch");
        rest = self.caption(cx, rest, "The admin Mac only: its fleet device credential, from its bound address", 1.0);
        rest = self.caption(cx, rest, "tools/fleet-identity.md; this machine needs its own fleet identity to serve", 1.4);

        // grants it misses (macOS)
        if !r.grants.is_empty() && r.grants.contains("MISSING") {
            rest = self.section(cx, rest, "Permissions");
            for part in r.grants.split(';') {
                rest = self.caption(cx, rest, part.trim(), 1.0);
            }
        }

        // Open at login (an installed macOS app)
        if let Some(on) = st.login {
            let (sep, r2) = cut_top(rest, 12.0);
            self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
            let (row, r2) = cut_top(r2, 30.0);
            rest = r2;
            let sw = self.d.toggle_switch(cx, dvec2(row.pos.x + row.size.x - 48.0, row.pos.y + 6.0), &tok, on, fg);
            self.hits.push((Hit::Login, inset(sw, -6.0)));
            self.d.label_elided(cx, rect(row.pos.x, row.pos.y, row.size.x - 64.0, row.size.y), false, tok.font.body, fg, HAlign::Left, "Open at login");
        }

        // Quit
        let (sep, r2) = cut_top(rest, 14.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 7.0, sep.size.x, 1.0), fg, 0.12);
        let (row, r2) = cut_top(r2, 34.0);
        rest = r2;
        let label = "Quit Desk Host";
        let w = self.d.button_width(cx, &tok, false, label, tok.font.body_small);
        let button = rect(row.pos.x + row.size.x - w, row.pos.y + 4.0, w, 28.0);
        let state = if self.hot == Some(Hit::Quit) { CtrlState::Hover } else { CtrlState::Normal };
        self.d.button(cx, button, &tok, state, None, label, tok.font.body_small, fg, true);
        self.hits.push((Hit::Quit, button));
        self.d.label_elided(cx, rect(row.pos.x, row.pos.y + 4.0, row.size.x - w - 8.0, 28.0), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Stops serving until it starts again");
        self.content_h = rest.pos.y + pad - window.pos.y;
        // where the switch and Quit are in the window, for a selftest that clicks them
        let at = |h: Hit| self.hits.iter().find(|(x, _)| *x == h).map(|(_, r)| format!("({:.0}, {:.0})", r.pos.x + r.size.x * 0.5 - window.pos.x, r.pos.y + r.size.y * 0.5 - window.pos.y)).unwrap_or_default();
        let note = format!("serving switch at {} {}, quit at {}, {} viewers", at(Hit::Serving), if st.enabled { "on" } else { "off" }, at(Hit::Quit), r.viewers.len());
        if note != self.noted {
            makepad_shell_kit::tray_app::selftest_note("deskhost", &note);
            self.noted = note;
        }
    }
}

impl Widget for DeskHostView {
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
                let action = match self.hit_at(e.abs) {
                    Some(Hit::Serving) => DeskHostAction::SetServing(!self.state.enabled),
                    Some(Hit::Disconnect(s)) => DeskHostAction::Disconnect(s),
                    Some(Hit::Login) => DeskHostAction::SetLogin(!self.state.login.unwrap_or(false)),
                    Some(Hit::Quit) => DeskHostAction::Quit,
                    None => return,
                };
                cx.widget_action(self.uid, action);
            }
            _ => {}
        }
    }
}
