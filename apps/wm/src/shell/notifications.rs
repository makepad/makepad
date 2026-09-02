//! `shell/plugins/notifications/` — the toast stack.
//!
//! `components/NotificationCard.qml`: a 380-wide card on
//! `[notifications] background` behind a 2px `[notifications] border`, 12px
//! side margins, 10px top/bottom (7 for a single-line toast), a 40×40 icon
//! slot 12px off the text column, a bold `title` (14px) summary of at most
//! two lines and a `title` body of at most three at `darker(text, 1.15)`,
//! and an 18×18 close button 3px inside the top-right corner that only
//! appears on hover.
//!
//! `Service.qml` / `NotificationLogic.js`: cards stack top-right with 8px
//! between them, newest first, cleared of the bar by `barSize + gapsOut`;
//! lifetimes are Critical → never, Low → clamp(5s..30s), Normal →
//! clamp(8s..30s), and hovering a card PAUSES its countdown.
//!
//! One deliberate addition over the QML: the card draws the countdown as a
//! bar in `[notifications] countdown` (the accent) along its bottom edge.
//! Omarchy computes that color and never draws it — the brief asks for it,
//! and it is the only way a toast says how long it has left.

use makepad_widgets::*;

use super::ui::{contains, inset, rect, DrawShellFill, Ico, ShellDraw};
use super::{darker, fade, ShellTokens};

pub const CARD_WIDTH: f64 = 380.0;
const SIDE_MARGIN: f64 = 12.0;
const V_MARGIN: f64 = 10.0;
const V_MARGIN_TOAST: f64 = 7.0;
const ICON_SLOT: f64 = 40.0;
const ICON_GAP: f64 = 12.0;
const TEXT_RIGHT_MARGIN: f64 = 10.0;
const TEXT_SPACING: f64 = 2.0;
const CLOSE_SIZE: f64 = 18.0;
const CLOSE_INSET: f64 = 3.0;
const STACK_SPACING: f64 = 8.0;
const COUNTDOWN_HEIGHT: f64 = 2.0;
const SUMMARY_LINES: usize = 2;
const BODY_LINES: usize = 3;

/// libnotify urgency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

/// `NotificationLogic.snapshotOf`.
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u64,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub icon: Option<Ico>,
    pub urgency: Urgency,
    /// The sender's hint in seconds; 0 means "you decide".
    pub requested: f64,
}

impl Notification {
    /// `Service.durationFor`: Critical never expires, Low is clamped to
    /// 5..30s and Normal to 8..30s.
    pub fn lifetime(&self) -> f64 {
        match self.urgency {
            Urgency::Critical => 0.0,
            Urgency::Low => {
                if self.requested > 0.0 {
                    self.requested.clamp(5.0, 30.0)
                } else {
                    5.0
                }
            }
            Urgency::Normal => {
                if self.requested > 0.0 {
                    self.requested.clamp(8.0, 30.0)
                } else {
                    8.0
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Live {
    note: Notification,
    lifetime: f64,
    left: f64,
    hovered: bool,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellNotificationsBase = #(ShellNotifications::register_widget(vm))
    mod.widgets.ShellNotifications = set_type_default() do mod.widgets.ShellNotificationsBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShellNotificationsAction {
    /// The card body was clicked: run the default action, then dismiss.
    Activated(u64),
    /// The ✕ or a right-click: dismiss only.
    Dismissed(u64),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellNotifications {
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
    #[rust]
    live: Vec<Live>,
    #[rust]
    next_id: u64,
    /// How far the stack is pushed down (the bar's height + gapsOut).
    #[rust]
    pub bar_clearance: f64,
    #[rust]
    area: Area,
    #[rust]
    screen: Rect,
    #[rust]
    card_rects: Vec<(u64, Rect, Rect)>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    pub inert: bool,
}

impl ShellNotifications {
    /// The in-process notification API: the WM (or anything holding this
    /// widget) posts, and the stack owns the rest.
    pub fn post(&mut self, cx: &mut Cx, mut note: Notification) -> u64 {
        self.next_id += 1;
        note.id = self.next_id;
        let lifetime = note.lifetime();
        self.live.insert(
            0,
            Live {
                note,
                lifetime,
                left: lifetime,
                hovered: false,
            },
        );
        self.last_time = 0.0;
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
        self.next_id
    }

    /// The one-call API the WM uses for `WmRequest::Notify{title, body}`.
    pub fn notify(&mut self, cx: &mut Cx, title: &str, body: &str) -> u64 {
        self.post(
            cx,
            Notification {
                id: 0,
                app: "wm".into(),
                summary: title.to_string(),
                body: body.to_string(),
                icon: Some(Ico::Bell),
                urgency: Urgency::Normal,
                requested: 0.0,
            },
        )
    }

    pub fn dismiss(&mut self, cx: &mut Cx, id: u64) {
        self.live.retain(|l| l.note.id != id);
        self.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.live.clear();
        self.redraw(cx);
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// The height a card needs for its text.
    fn card_height(&mut self, cx: &mut Cx2d, note: &Notification) -> f64 {
        let tok = self.tokens;
        let text_w = CARD_WIDTH
            - SIDE_MARGIN * 2.0
            - ICON_SLOT
            - ICON_GAP
            - TEXT_RIGHT_MARGIN
            - tok.notifications.surface.border_width * 2.0;
        let summary = self
            .d
            .wrap(cx, true, tok.font.title, &note.summary, text_w, SUMMARY_LINES)
            .len()
            .max(1);
        let body = if note.body.is_empty() {
            0
        } else {
            self.d
                .wrap(cx, false, tok.font.title, &note.body, text_w, BODY_LINES)
                .len()
        };
        let line = tok.font.title * 1.35;
        let single = summary == 1 && body == 0;
        let v = if single { V_MARGIN_TOAST } else { V_MARGIN };
        let text_h = summary as f64 * line
            + if body > 0 {
                TEXT_SPACING + body as f64 * line
            } else {
                0.0
            };
        (text_h.max(ICON_SLOT) + v * 2.0 + tok.notifications.surface.border_width * 2.0).ceil()
    }

    pub fn draw_surface(&mut self, cx: &mut Cx2d, screen: Rect) {
        self.card_rects.clear();
        self.screen = screen;
        if self.live.is_empty() {
            return;
        }
        let tok = self.tokens;
        let gaps_out = tok.spacing.gaps_out;
        let border = tok.notifications.surface.border_width;
        let mut y = screen.pos.y + self.bar_clearance.max(gaps_out);
        let x = screen.pos.x + screen.size.x - gaps_out - CARD_WIDTH;

        for entry in self.live.clone() {
            let note = entry.note.clone();
            let h = self.card_height(cx, &note);
            let card = rect(x, y, CARD_WIDTH, h);
            self.d.card(cx, card, &tok.notifications.surface);

            let inner = inset(card, border);
            let single = h <= ICON_SLOT + V_MARGIN_TOAST * 2.0 + border * 2.0;
            let v = if single { V_MARGIN_TOAST } else { V_MARGIN };
            let mut tx = inner.pos.x + SIDE_MARGIN;
            if let Some(ico) = note.icon {
                self.d.icon_centered(
                    cx,
                    ico,
                    rect(tx, inner.pos.y + v, ICON_SLOT, ICON_SLOT),
                    tok.font.display_large,
                    tok.notifications.surface.text,
                );
                tx += ICON_SLOT + ICON_GAP;
            }
            let text_w = inner.pos.x + inner.size.x - SIDE_MARGIN - TEXT_RIGHT_MARGIN - tx;
            let line = tok.font.title * 1.35;
            let mut ty = inner.pos.y + v;
            let summary = self
                .d
                .wrap(cx, true, tok.font.title, &note.summary, text_w, SUMMARY_LINES);
            for l in &summary {
                self.d.label(
                    cx,
                    rect(tx, ty, text_w, line),
                    true,
                    tok.font.title,
                    tok.notifications.surface.text,
                    super::ui::HAlign::Left,
                    l,
                );
                ty += line;
            }
            if !note.body.is_empty() {
                ty += TEXT_SPACING;
                let body = self
                    .d
                    .wrap(cx, false, tok.font.title, &note.body, text_w, BODY_LINES);
                let body_color = darker(tok.notifications.surface.text, 1.15);
                for l in &body {
                    self.d.label(
                        cx,
                        rect(tx, ty, text_w, line),
                        false,
                        tok.font.title,
                        body_color,
                        super::ui::HAlign::Left,
                        l,
                    );
                    ty += line;
                }
            }

            // The ✕, visible on hover only.
            let close = rect(
                inner.pos.x + inner.size.x - CLOSE_INSET - CLOSE_SIZE,
                inner.pos.y + CLOSE_INSET,
                CLOSE_SIZE,
                CLOSE_SIZE,
            );
            if entry.hovered {
                self.d.icon_centered(
                    cx,
                    Ico::Close,
                    close,
                    CLOSE_SIZE * 0.6,
                    darker(tok.notifications.surface.text, 1.4),
                );
            }

            // The countdown, in the accent.
            if entry.lifetime > 0.0 {
                let p = (entry.left / entry.lifetime).clamp(0.0, 1.0);
                self.d.solid(
                    cx,
                    rect(
                        card.pos.x,
                        card.pos.y + card.size.y - COUNTDOWN_HEIGHT,
                        card.size.x * p,
                        COUNTDOWN_HEIGHT,
                    ),
                    fade(tok.notifications.countdown, if entry.hovered { 0.5 } else { 1.0 }),
                );
            }

            self.card_rects.push((note.id, card, close));
            y += h + STACK_SPACING;
        }
    }
}

impl Widget for ShellNotifications {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let screen = cx.turtle().rect();
        self.draw_surface(cx, screen);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.inert {
            return;
        }
        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = if self.last_time <= 0.0 {
                1.0 / 60.0
            } else {
                (ne.time - self.last_time).clamp(0.001, 0.05)
            };
            self.last_time = ne.time;
            let mut busy = false;
            for entry in self.live.iter_mut() {
                // Hovering pauses the countdown (`ticking: !card.hovered`).
                if entry.lifetime > 0.0 && !entry.hovered {
                    entry.left -= dt;
                    busy = true;
                }
            }
            let before = self.live.len();
            self.live
                .retain(|l| l.lifetime <= 0.0 || l.left > 0.0);
            if before != self.live.len() || busy {
                self.redraw(cx);
            }
            if !self.live.is_empty() {
                self.next_frame = cx.new_next_frame();
            }
        }
        match event {
            Event::MouseMove(e) => {
                let rects = self.card_rects.clone();
                let mut changed = false;
                for entry in self.live.iter_mut() {
                    let over = rects
                        .iter()
                        .find(|(id, _, _)| *id == entry.note.id)
                        .map(|(_, card, _)| contains(*card, e.abs))
                        .unwrap_or(false);
                    if over != entry.hovered {
                        entry.hovered = over;
                        changed = true;
                    }
                }
                if changed {
                    self.redraw(cx);
                }
            }
            Event::MouseDown(e) => {
                let rects = self.card_rects.clone();
                for (id, card, close) in rects {
                    if contains(close, e.abs) {
                        cx.widget_action(self.uid, ShellNotificationsAction::Dismissed(id));
                        self.dismiss(cx, id);
                        return;
                    }
                    if contains(card, e.abs) {
                        if e.button.contains(MouseButton::SECONDARY) {
                            cx.widget_action(self.uid, ShellNotificationsAction::Dismissed(id));
                        } else {
                            cx.widget_action(self.uid, ShellNotificationsAction::Activated(id));
                        }
                        self.dismiss(cx, id);
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The three fixtures the gallery shows.
pub fn fixtures() -> Vec<Notification> {
    vec![
        Notification {
            id: 0,
            app: "wm".into(),
            summary: "Theme imported".into(),
            body: "tokyo-night is now the active theme, with 4 backgrounds.".into(),
            icon: Some(Ico::Moon),
            urgency: Urgency::Normal,
            requested: 0.0,
        },
        Notification {
            id: 0,
            app: "terminal".into(),
            summary: "Build finished".into(),
            body: String::new(),
            icon: Some(Ico::Check),
            urgency: Urgency::Low,
            requested: 0.0,
        },
        Notification {
            id: 0,
            app: "system".into(),
            summary: "Battery low".into(),
            body: "18% left. Plug in soon — this one never expires on its own.".into(),
            icon: Some(Ico::Battery),
            urgency: Urgency::Critical,
            requested: 0.0,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifetimes_follow_duration_for() {
        let mut n = fixtures()[0].clone();
        n.urgency = Urgency::Critical;
        assert_eq!(n.lifetime(), 0.0);
        n.urgency = Urgency::Low;
        n.requested = 1.0;
        assert_eq!(n.lifetime(), 5.0);
        n.requested = 45.0;
        assert_eq!(n.lifetime(), 30.0);
        n.urgency = Urgency::Normal;
        n.requested = 0.0;
        assert_eq!(n.lifetime(), 8.0);
        n.requested = 2.0;
        assert_eq!(n.lifetime(), 8.0);
    }
}
