//! The menu-bar icon's dropdown: the desktops this viewer knows, each with whether it answers
//! (and its round trip) or why not; a click shows that desktop in the window (`DeskMenuAction::
//! Open`). Drawn with the shell kit like the AI Hub's and Desk Host's dropdowns.

use makepad_desk_link_panel::{draw_desktops, DesktopRow, Reach};
use makepad_shell_kit::ui::{contains, cut_top, rect, HAlign, Ico, ShellDraw};
use makepad_shell_kit::{darker, ShellTokens};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.DeskMenuViewBase = #(DeskMenuView::register_widget(vm))
    mod.widgets.DeskMenuView = set_type_default() do mod.widgets.DeskMenuViewBase {
        width: Fill
        height: Fill
        d +: {}
    }
}

/// A line for a tray selftest (`MAKEPAD_TRAY_SELFTEST`).
pub fn selftest(what: &str) {
    makepad_shell_kit::tray_app::selftest_note("desklink", what);
}

#[derive(Clone, Debug, Default)]
pub enum DeskMenuAction {
    Open(usize),
    #[default]
    None,
}

const PAD: f64 = 16.0;

#[derive(Script, ScriptHook, Widget)]
pub struct DeskMenuView {
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
    #[live]
    card: bool,
    #[rust]
    rows: Vec<DesktopRow>,
    #[rust]
    hits: Vec<(usize, Rect)>,
    #[rust]
    hot: Option<usize>,
    #[rust]
    content_h: f64,
    #[rust]
    noted: String,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
}

impl DeskMenuView {
    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<DesktopRow>) {
        if rows != self.rows {
            self.rows = rows;
            self.redraw(cx);
        }
    }

    pub fn content_height(&self) -> f64 {
        self.content_h
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
        let online = self.rows.iter().filter(|r| matches!(r.reach, Reach::Online(_))).count();
        let (hero, rest) = cut_top(top, 40.0);
        let meta = match self.rows.len() {
            0 => "No desktops known yet".to_string(),
            n => format!("{online} of {n} answer"),
        };
        self.d.panel_hero(cx, hero, &tok, fg, Ico::Monitor, "Desk Link", &meta, 0.0);
        let (sep, rest) = cut_top(rest, 12.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let rows = self.rows.clone();
        let mut rest = draw_desktops(cx, &mut self.d, &tok, rest, &rows, self.hot, &mut self.hits);
        if rows.is_empty() {
            let (row, r2) = cut_top(rest, 18.0);
            rest = r2;
            self.d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, "Saved desktops: ~/.makepad/desklink/hosts");
        }
        let (sep, r2) = cut_top(rest, 12.0);
        self.d.separator(cx, rect(sep.pos.x, sep.pos.y + 6.0, sep.size.x, 1.0), fg, 0.12);
        let (row, r2) = cut_top(r2, 18.0);
        let key = if cfg!(target_vendor = "apple") { "\u{2318}" } else { "Ctrl+Shift+" };
        self.d.label_elided(cx, row, false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &format!("{key}1..9 a desktop \u{b7} {key}R reconnect \u{b7} right-click: more"));
        self.content_h = r2.pos.y + pad - window.pos.y;
        // where each row is, for a selftest that clicks one
        let note = self.hits.iter().map(|(i, r)| format!("{i}@({:.0},{:.0})", r.pos.x + r.size.x * 0.5 - window.pos.x, r.pos.y + r.size.y * 0.5 - window.pos.y)).collect::<Vec<_>>().join(" ");
        if note != self.noted {
            selftest(&format!("desktop rows {note}"));
            self.noted = note;
        }
    }
}

impl Widget for DeskMenuView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let window = cx.turtle().rect();
        self.draw_all(cx, window);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let hit = |hits: &[(usize, Rect)], p: Vec2d| hits.iter().find(|(_, r)| contains(*r, p)).map(|(i, _)| *i);
        match event {
            Event::MouseMove(e) => {
                let hot = hit(&self.hits, e.abs);
                if hot != self.hot {
                    self.hot = hot;
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                if let Some(i) = hit(&self.hits, e.abs) {
                    cx.widget_action(self.uid, DeskMenuAction::Open(i));
                }
            }
            _ => {}
        }
    }
}
