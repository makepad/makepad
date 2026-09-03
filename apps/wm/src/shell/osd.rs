//! `shell/plugins/osd/` — the volume / brightness / mic popup.
//!
//! `Osd.qml`: a card `space(16)` padded around a `displayLarge` (28px)
//! glyph, a 142×6 progress bar and a bold `title` (14px) readout, centered
//! horizontally and `space(67)` up from the bottom edge, on `background` at
//! α .97 behind a 2px `[popups] border`. The bar's track is
//! `popups.text` at α .45 and its fill is the accent; the fill is the one
//! animated thing (140ms ease-out-cubic). It shows for 1200ms by default
//! and never takes clicks (`mask: Region{}`).
//!
//! Without a value it is a message OSD instead: the glyph and one elided
//! line, no bar.

use makepad_widgets::*;

use super::ui::{rect, DrawShellFill, Ico, ShellDraw};
use super::{alpha, ShellTokens};

/// `Osd.qml` numbers.
const PAD: f64 = 16.0;
const BOTTOM_MARGIN: f64 = 67.0;
const BAR_WIDTH: f64 = 142.0;
const BAR_HEIGHT: f64 = 6.0;
const GAP_WITH_BAR: f64 = 16.0;
/// `round(gap * 2/3)` when there is no bar.
const GAP_MESSAGE: f64 = 11.0;
const CARD_ALPHA: f32 = 0.97;
/// `hideTimer.interval` — the default `duration`.
pub const DEFAULT_DURATION: f64 = 1.2;
/// The fill's `Behavior on width`.
const FILL_EASE: f64 = 0.14;

/// What the OSD is showing.
#[derive(Clone, Debug, PartialEq)]
pub struct OsdShow {
    pub icon: Ico,
    /// 0..1, `None` for a message-only OSD.
    pub value: Option<f64>,
    /// The readout — omarchy defaults it to `percent + "%"`.
    pub message: String,
    pub duration: f64,
}

impl OsdShow {
    pub fn volume(level: u32, muted: bool) -> Self {
        Self {
            icon: super::bar::volume_icon(Some(level), muted),
            value: Some(level as f64 / 100.0),
            message: if muted {
                "Muted".to_string()
            } else {
                format!("{}%", level)
            },
            duration: DEFAULT_DURATION,
        }
    }

    pub fn brightness(level: u32) -> Self {
        Self {
            icon: Ico::Brightness,
            value: Some(level as f64 / 100.0),
            message: format!("{}%", level),
            duration: DEFAULT_DURATION,
        }
    }

    pub fn mic(muted: bool) -> Self {
        Self {
            icon: if muted { Ico::MicOff } else { Ico::Mic },
            value: None,
            message: if muted {
                "Microphone muted".to_string()
            } else {
                "Microphone live".to_string()
            },
            duration: DEFAULT_DURATION,
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellOsdBase = #(ShellOsd::register_widget(vm))
    mod.widgets.ShellOsd = set_type_default() do mod.widgets.ShellOsdBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellOsd {
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
    pub show: Option<OsdShow>,
    /// The animated fill, chasing `show.value`.
    #[rust]
    fill: f64,
    #[rust]
    area: Area,
    #[rust]
    screen: Rect,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: f64,
    #[rust]
    left: f64,
    /// Fixture mode: hold the card open and animate nothing.
    #[rust]
    pub inert: bool,
}

impl ShellOsd {
    /// `osd show` — replaces whatever is up and restarts the timer.
    pub fn present(&mut self, cx: &mut Cx, show: OsdShow) {
        self.left = show.duration;
        if self.show.is_none() {
            self.fill = show.value.unwrap_or(0.0);
        }
        self.show = Some(show);
        self.last_time = 0.0;
        self.next_frame = cx.new_next_frame();
        self.redraw(cx);
    }

    pub fn hide(&mut self, cx: &mut Cx) {
        self.show = None;
        self.redraw(cx);
    }

    /// A live theme switch: new tokens, redraw.
    pub fn set_tokens(&mut self, cx: &mut Cx, tokens: ShellTokens) {
        self.tokens = tokens;
        self.redraw(cx);
    }

    /// The card rect inside `screen`, per `Osd.qml`'s anchors.
    fn card_rect(&mut self, cx: &mut Cx2d, screen: Rect, show: &OsdShow) -> (Rect, f64) {
        let tok = self.tokens;
        let icon = tok.font.display_large;
        let gap = if show.value.is_some() {
            GAP_WITH_BAR
        } else {
            GAP_MESSAGE
        };
        let text_w = if show.value.is_some() {
            // Sized for "100%" so the digits never jitter.
            self.d.measure(cx, true, tok.font.title, "100%")
        } else {
            self.d
                .measure(cx, true, tok.font.title, &show.message)
                .min(190.0)
        };
        let content = if show.value.is_some() {
            icon + gap + BAR_WIDTH + gap + text_w
        } else {
            icon + gap + text_w
        };
        let border = tok.popups.border_width;
        let w = content + PAD * 2.0 + border * 2.0;
        let h = icon + PAD * 2.0 + border * 2.0;
        let x = (screen.pos.x + (screen.size.x - w) * 0.5).floor();
        let y = (screen.pos.y + screen.size.y - BOTTOM_MARGIN - h).floor();
        (rect(x, y, w, h), text_w)
    }

    /// Draw the whole surface into `screen`. Under glass it draws in its
    /// own overlay list, claimed every frame, open or not.
    pub fn draw_surface(&mut self, cx: &mut Cx2d, screen: Rect) {
        let tokens = self.tokens;
        self.d.begin_surface(cx, &tokens);
        self.draw_surface_inner(cx, screen);
        self.d.end_surface(cx);
    }

    fn draw_surface_inner(&mut self, cx: &mut Cx2d, screen: Rect) {
        self.screen = screen;
        let Some(show) = self.show.clone() else {
            return;
        };
        let tok = self.tokens;
        let (card, text_w) = self.card_rect(cx, screen, &show);
        // The OSD card is the popup surface at α .97.
        let mut surface = tok.popups;
        surface.background_alpha = CARD_ALPHA;
        self.d.card(cx, card, &surface);

        let border = tok.popups.border_width;
        let inner = rect(
            card.pos.x + border + PAD,
            card.pos.y + border + PAD,
            card.size.x - (border + PAD) * 2.0,
            card.size.y - (border + PAD) * 2.0,
        );
        let icon_size = tok.font.display_large;
        self.d.icon_centered(
            cx,
            show.icon,
            rect(inner.pos.x, inner.pos.y, icon_size, inner.size.y),
            icon_size,
            tok.popups.text,
        );
        let gap = if show.value.is_some() {
            GAP_WITH_BAR
        } else {
            GAP_MESSAGE
        };
        let mut x = inner.pos.x + icon_size + gap;
        if show.value.is_some() {
            let track = rect(
                x,
                inner.pos.y + (inner.size.y - BAR_HEIGHT) * 0.5,
                BAR_WIDTH,
                BAR_HEIGHT,
            );
            self.d
                .solid(cx, track, alpha(tok.popups.text, 0.45));
            let p = self.fill.clamp(0.0, 1.0);
            self.d.solid(
                cx,
                rect(track.pos.x, track.pos.y, BAR_WIDTH * p, BAR_HEIGHT),
                tok.notifications.countdown,
            );
            x += BAR_WIDTH + gap;
        }
        self.d.label(
            cx,
            rect(x, inner.pos.y, text_w, inner.size.y),
            true,
            tok.font.title,
            tok.popups.text,
            super::ui::HAlign::Right,
            &show.message,
        );
    }
}

impl Widget for ShellOsd {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let screen = cx.turtle().rect();
        self.draw_surface(cx, screen);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // `mask: Region{}` — the OSD never takes a click.
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
            if let Some(show) = self.show.clone() {
                if let Some(target) = show.value {
                    if (self.fill - target).abs() > 0.001 {
                        // 140ms ease-out toward the new value.
                        self.fill += (target - self.fill) * (dt / FILL_EASE).min(1.0);
                        busy = true;
                    } else {
                        self.fill = target;
                    }
                }
                if show.duration > 0.0 {
                    self.left -= dt;
                    if self.left <= 0.0 {
                        self.show = None;
                    } else {
                        busy = true;
                    }
                }
                self.redraw(cx);
            }
            if busy {
                self.next_frame = cx.new_next_frame();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_volume_osd_reads_the_level() {
        let s = OsdShow::volume(42, false);
        assert_eq!(s.message, "42%");
        assert_eq!(s.value, Some(0.42));
        assert_eq!(s.icon, Ico::Volume2);
        assert_eq!(s.duration, DEFAULT_DURATION);
        // Muted says so, and shows the silent glyph.
        let m = OsdShow::volume(42, true);
        assert_eq!(m.message, "Muted");
        assert_eq!(m.icon, Ico::Volume0);
    }

    #[test]
    fn a_message_osd_has_no_bar() {
        let s = OsdShow::mic(true);
        assert!(s.value.is_none());
        assert_eq!(s.icon, Ico::MicOff);
    }
}
