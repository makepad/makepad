//! The history scrubber: a translucent bar floating over the bottom of the picture. A 30 s
//! timeline (the newest picture at its right end) with a playhead, the In..Out range shaded,
//! subtle ticks for key frames and dots for drops; hovering it shows a thumbnail of that moment
//! and its time. Under it: step back / play / step forward, the shown time ("−12.4 s", or the
//! wall clock: a click on it switches), In / Out, the Save menu, the details toggle (the frame's
//! internals in a line of their own) and the LIVE pill. It hides itself when the pointer rests
//! (and comes back when it moves or a key scrubs). Theme tokens only.

use crate::caption::{contains, fade, text_in, text_width, DrawRound};
use crate::history::{flag, Snapshot};
use crate::scrub::Decoded;
use crate::view::{draw_parts, import_parts, ShownPart};
use makepad_widgets::makepad_draw::shader::draw_nv12::DrawNv12;
use makepad_widgets::*;
use std::sync::Arc;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ScrubberBase = #(Scrubber::register_widget(vm))
    mod.widgets.Scrubber = set_type_default() do mod.widgets.ScrubberBase {
        width: Fill height: Fill
        draw_text +: { text_style: theme.font_regular{} }
        draw_mono +: { text_style: theme.font_code{} }
        icon_play +: { svg: crate_resource("self:resources/icons/play.svg") }
        icon_pause +: { svg: crate_resource("self:resources/icons/pause.svg") }
        icon_back +: { svg: crate_resource("self:resources/icons/step-back.svg") }
        icon_forward +: { svg: crate_resource("self:resources/icons/step-forward.svg") }
        icon_in +: { svg: crate_resource("self:resources/icons/mark-in.svg") }
        icon_out +: { svg: crate_resource("self:resources/icons/mark-out.svg") }
        icon_save +: { svg: crate_resource("self:resources/icons/save.svg") }
        icon_details +: { svg: crate_resource("self:resources/icons/details.svg") }
        icon_chevron +: { svg: crate_resource("self:resources/icons/chevron-down.svg") }
        color_bar: theme.color_bg_container
        color_border: theme.color_outline_variant
        color_shadow: theme.color_shadow
        color_text: theme.color_label_outer
        color_dim: theme.color_label_outer_off
        color_hover: theme.color_bg_highlight_inline
        color_accent: theme.color_primary
        color_on_accent: theme.color_on_primary
        color_live: theme.color_error
        color_on_live: theme.color_on_error
        color_drop: theme.color_error
        color_thumb_bg: theme.color_bg_app
    }
}

/// The span of the timeline.
pub const SPAN_NS: u64 = 30_000_000_000;
/// The pointer resting this long hides the bar.
const IDLE_HIDE: f64 = 2.5;

/// What the scrubber shows.
pub struct ScrubData {
    pub snap: Arc<Snapshot>,
    /// Live: the playhead is at the newest picture and the stream goes on.
    pub live: bool,
    pub cur: usize,
    pub range: (Option<usize>, Option<usize>),
    /// The play speed (negative: backward; None: stopped).
    pub playing: Option<f64>,
    /// The scrub decoder's state (details line).
    pub status: String,
    /// The last save, said for a few seconds.
    pub toast: String,
    /// The wall clock of the viewer's `now_ns` 0 (UNIX seconds), for the clock display.
    pub wall0: f64,
}

#[derive(Clone, Debug, Default)]
pub enum ScrubAction {
    /// Show picture i (paused there).
    Seek(usize),
    /// The pointer is over the timeline at picture i (a thumbnail is wanted), or left it.
    Hover(Option<usize>),
    Step(i64),
    Play(f64),
    Stop,
    Live,
    In,
    Out,
    /// The Save menu, above this button.
    SaveMenu(Rect),
    #[default]
    None,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Btn {
    Back,
    Play,
    Forward,
    Clock,
    In,
    Out,
    Save,
    Details,
    Live,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Scrubber {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawRound,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_mono: DrawText,
    #[live]
    draw_nv12: DrawNv12,
    #[live]
    icon_play: DrawSvg,
    #[live]
    icon_pause: DrawSvg,
    #[live]
    icon_back: DrawSvg,
    #[live]
    icon_forward: DrawSvg,
    #[live]
    icon_in: DrawSvg,
    #[live]
    icon_out: DrawSvg,
    #[live]
    icon_save: DrawSvg,
    #[live]
    icon_details: DrawSvg,
    #[live]
    icon_chevron: DrawSvg,
    #[live]
    color_bar: Vec4f,
    #[live]
    color_border: Vec4f,
    #[live]
    color_shadow: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_dim: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_accent: Vec4f,
    #[live]
    color_on_accent: Vec4f,
    #[live]
    color_live: Vec4f,
    #[live]
    color_on_live: Vec4f,
    #[live]
    color_drop: Vec4f,
    #[live]
    color_thumb_bg: Vec4f,
    #[rust]
    data: Option<ScrubData>,
    /// The frame internals line is shown.
    #[rust]
    pub details: bool,
    /// The time as the wall clock instead of before the newest.
    #[rust]
    clock: bool,
    #[rust]
    bar: Rect,
    #[rust]
    track: Rect,
    #[rust]
    buttons: Vec<(Rect, Btn)>,
    #[rust]
    hot: Option<Btn>,
    /// The pointer over the timeline: (x, the picture there).
    #[rust]
    /// The moment under the pointer (a host time: it stays put while a live history moves).
    hover: Option<u64>,
    #[rust]
    dragging: bool,
    /// The hovered moment's picture, as textures: (its index, its parts, its size).
    #[rust]
    thumb: Option<(usize, Vec<ShownPart>, [u32; 2])>,
    /// Thumbnails replaced lately: a frame in flight may still sample their GPU pictures.
    #[rust]
    retired: std::collections::VecDeque<Vec<ShownPart>>,
    /// Shown (1) or hidden (0), and where it goes.
    #[rust]
    alpha: f64,
    #[rust]
    want: f64,
    #[rust]
    last_poke: f64,
    #[rust]
    fade_frame: NextFrame,
    #[rust]
    idle_timer: Timer,
    #[rust]
    window: Option<WindowId>,
    /// Where the buttons are, for a selftest that clicks one.
    #[rust]
    noted: String,
}

fn fmt_ago(ns: u64) -> String {
    let s = ns as f64 / 1e9;
    if s < 0.05 {
        "0.0 s".to_string()
    } else {
        format!("\u{2212}{s:.1} s")
    }
}

/// A moment as the wall clock showed it here (the local zone; UTC said as such where the
/// platform has none), to the millisecond.
fn fmt_clock(unix: f64) -> String {
    let t = makepad_civil_time::local_or_utc(unix.floor() as i64);
    let ms = (((unix - unix.floor()) * 1000.0).round() as u32).min(999);
    format!("{}.{ms:03}{}", t.hms(), if t.zoned { "" } else { " UTC" })
}

impl Scrubber {
    pub fn set_data(&mut self, cx: &mut Cx, data: Option<ScrubData>) {
        if data.is_none() {
            self.thumb = None;
            self.hover = None;
            self.dragging = false;
        }
        self.data = data;
        self.draw_bg.redraw(cx);
    }

    /// The thumbnail of the hovered moment (the scrub decoder's picture for it).
    pub fn set_thumb(&mut self, cx: &mut Cx, d: &Decoded) {
        if self.thumb.as_ref().is_some_and(|t| t.0 == d.index) {
            return;
        }
        let mut old = self.thumb.take().map(|t| t.1).unwrap_or_default();
        let parts = import_parts(cx, d.view_parts(), &mut old);
        self.retired.push_back(old);
        while self.retired.len() > 3 {
            self.retired.pop_front();
        }
        self.thumb = Some((d.index, parts, [d.width, d.height]));
        self.draw_bg.redraw(cx);
    }

    /// Something happened (the pointer moved, a key scrubbed): the bar shows, and hides again
    /// once the pointer rests.
    pub fn poke(&mut self, cx: &mut Cx) {
        if self.data.is_none() {
            return;
        }
        self.last_poke = cx.seconds_since_app_start();
        self.set_want(cx, 1.0);
        cx.stop_timer(self.idle_timer);
        self.idle_timer = cx.start_timeout(IDLE_HIDE);
    }

    /// Shows the hovered moment `i` as if the pointer were there (`--check-scrubber`).
    pub fn set_hover(&mut self, cx: &mut Cx, i: usize) {
        self.hover = self.data.as_ref().and_then(|d| d.snap.pictures.get(i)).map(|p| p.built_ns);
        self.poke(cx);
        self.draw_bg.redraw(cx);
    }

    /// Whether a point of the window is on the shown bar (the desktop does not get it).
    pub fn covers(&self, p: Vec2d) -> bool {
        self.data.is_some() && self.alpha > 0.05 && contains(self.bar, p)
    }

    fn set_want(&mut self, cx: &mut Cx, want: f64) {
        if self.want != want {
            self.want = want;
            self.fade_frame = cx.new_next_frame();
        }
    }

    fn time_x(&self, built_ns: u64) -> f64 {
        let Some(d) = &self.data else { return self.track.pos.x };
        let newest = d.snap.pictures.last().map_or(0, |p| p.built_ns);
        let t0 = newest.saturating_sub(SPAN_NS);
        self.track.pos.x + (built_ns.saturating_sub(t0) as f64 / SPAN_NS as f64).clamp(0.0, 1.0) * self.track.size.x
    }

    fn pick(&self, x: f64) -> Option<usize> {
        let d = self.data.as_ref()?;
        let newest = d.snap.pictures.last()?.built_ns;
        let u = ((x - self.track.pos.x) / self.track.size.x.max(1.0)).clamp(0.0, 1.0);
        let t = newest.saturating_sub(SPAN_NS) + (u * SPAN_NS as f64) as u64;
        Some(d.snap.at_time(t))
    }

    /// What a button does and its key, said over the bar while it is hovered.
    fn tip_text(&self, b: Btn) -> String {
        let live = self.data.as_ref().is_some_and(|d| d.live);
        let playing = self.data.as_ref().and_then(|d| d.playing).is_some();
        match b {
            Btn::Back => "Step back  \u{2190}   J plays backward".to_string(),
            Btn::Play if playing => "Pause  Space / K".to_string(),
            Btn::Play => "Play  Space / L".to_string(),
            Btn::Forward => "Step forward  \u{2192}".to_string(),
            Btn::Clock => "Time before now / wall clock".to_string(),
            Btn::In => "Mark in  I".to_string(),
            Btn::Out => "Mark out  O".to_string(),
            Btn::Save => "Save clip or frame".to_string(),
            Btn::Details => "Frame details".to_string(),
            Btn::Live if live => "Live".to_string(),
            Btn::Live => "Back to live  Esc".to_string(),
        }
    }

    fn icon_button(&mut self, cx: &mut Cx2d, r: Rect, b: Btn, on: bool) {
        let hot = self.hot == Some(b);
        let bg = if on { fade(self.color_accent, 0.9) } else if hot { self.color_hover } else { vec4(0.0, 0.0, 0.0, 0.0) };
        self.draw_bg.fill(cx, r, fade(bg, self.alpha as f32), 7.0);
        let fg = fade(if on { self.color_on_accent } else { self.color_text }, self.alpha as f32);
        let s = 16.0;
        let ir = Rect { pos: dvec2(r.pos.x + ((r.size.x - s) * 0.5).floor(), r.pos.y + ((r.size.y - s) * 0.5).floor()), size: dvec2(s, s) };
        let playing = self.data.as_ref().is_some_and(|d| d.playing.is_some());
        let icon = match b {
            Btn::Back => &mut self.icon_back,
            Btn::Play if playing => &mut self.icon_pause,
            Btn::Play => &mut self.icon_play,
            Btn::Forward => &mut self.icon_forward,
            Btn::In => &mut self.icon_in,
            Btn::Out => &mut self.icon_out,
            Btn::Save => &mut self.icon_save,
            Btn::Details => &mut self.icon_details,
            Btn::Clock | Btn::Live => return,
        };
        icon.color = fg;
        icon.draw_abs(cx, ir);
        self.buttons.push((r, b));
    }
}

const BAR_H: f64 = 74.0;
const DETAILS_H: f64 = 22.0;
const BTN: f64 = 28.0;
const THUMB_W: f64 = 200.0;

impl Widget for Scrubber {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.window = cx.get_current_window_id();
        let area = cx.walk_turtle(walk);
        self.buttons.clear();
        let Some(d) = self.data.as_ref() else {
            self.bar = Rect::default();
            return DrawStep::done();
        };
        if self.alpha <= 0.01 {
            self.bar = Rect::default();
            return DrawStep::done();
        }
        let a = self.alpha as f32;
        let snap = d.snap.clone();
        let (live, cur, range, details, status, toast, wall0) = (d.live, d.cur, d.range, self.details, d.status.clone(), d.toast.clone(), d.wall0);
        let n = snap.len();
        let h = BAR_H + if details { DETAILS_H } else { 0.0 };
        let w = (area.size.x - 32.0).min(980.0).max(320.0);
        let bar = Rect { pos: dvec2(area.pos.x + ((area.size.x - w) * 0.5).floor(), area.pos.y + area.size.y - h - 16.0), size: dvec2(w, h) };
        self.bar = bar;
        self.draw_bg.fill(cx, Rect { pos: bar.pos + dvec2(0.0, 3.0), size: bar.size }, fade(self.color_shadow, 0.30 * a), 14.0);
        self.draw_bg.fill(cx, Rect { pos: bar.pos - dvec2(1.0, 1.0), size: bar.size + dvec2(2.0, 2.0) }, fade(self.color_border, 0.6 * a), 14.0);
        self.draw_bg.fill(cx, bar, fade(self.color_bar, 0.86 * a), 13.0);

        // the timeline
        let track = Rect { pos: dvec2(bar.pos.x + 18.0, bar.pos.y + 12.0), size: dvec2(bar.size.x - 36.0, 18.0) };
        self.track = track;
        let mid = track.pos.y + track.size.y * 0.5;
        let line = |x0: f64, x1: f64, hh: f64| Rect { pos: dvec2(x0, mid - hh * 0.5), size: dvec2((x1 - x0).max(0.0), hh) };
        self.draw_bg.fill(cx, line(track.pos.x, track.pos.x + track.size.x, 4.0), fade(self.color_text, 0.16 * a), 4.0);
        if n > 0 {
            let first_x = self.time_x(snap.pictures[0].built_ns);
            let last_x = self.time_x(snap.pictures[n - 1].built_ns);
            // what the history holds
            self.draw_bg.fill(cx, line(first_x, last_x, 4.0), fade(self.color_text, 0.30 * a), 4.0);
            let head = if live { last_x } else { self.time_x(snap.pictures[cur.min(n - 1)].built_ns) };
            // In..Out
            let r = match range {
                (Some(x), Some(y)) => Some((x.min(y), x.max(y))),
                (Some(x), None) | (None, Some(x)) if !live => Some((x.min(cur), x.max(cur))),
                _ => None,
            };
            if let Some((ra, rb)) = r {
                let (xa, xb) = (self.time_x(snap.pictures[ra.min(n - 1)].built_ns), self.time_x(snap.pictures[rb.min(n - 1)].built_ns));
                self.draw_bg.fill(cx, line(xa, xb.max(xa + 2.0), 12.0), fade(self.color_accent, 0.28 * a), 3.0);
            }
            // key frames: a faint tick above; drops: a small dot below
            let mut last_key = f64::MIN;
            let mut last_drop = f64::MIN;
            for i in 0..n {
                let p = &snap.pictures[i];
                let x = self.time_x(p.built_ns);
                if p.key && x - last_key >= 3.0 {
                    last_key = x;
                    self.draw_bg.fill(cx, Rect { pos: dvec2(x.floor(), mid - 9.0), size: dvec2(1.0, 4.0) }, fade(self.color_text, 0.45 * a), 0.0);
                }
                if snap.flags(i) & (flag::GAP | flag::SKIP | flag::FAILED) != 0 && x - last_drop >= 6.0 {
                    last_drop = x;
                    self.draw_bg.fill(cx, Rect { pos: dvec2(x - 1.25, mid + 5.0), size: dvec2(2.5, 2.5) }, fade(self.color_drop, 0.6 * a), 2.5);
                }
            }
            // played part and the playhead
            self.draw_bg.fill(cx, line(first_x, head, 4.0), fade(if live { self.color_live } else { self.color_accent }, 0.85 * a), 4.0);
            if let Some(t) = self.hover {
                let hx = self.time_x(snap.pictures[snap.at_time(t)].built_ns);
                self.draw_bg.fill(cx, Rect { pos: dvec2(hx.floor(), mid - 7.0), size: dvec2(1.0, 14.0) }, fade(self.color_text, 0.5 * a), 0.0);
            }
            let knob = if self.dragging || self.hover.is_some() { 14.0 } else { 12.0 };
            self.draw_bg.fill(cx, Rect { pos: dvec2(head - knob * 0.5, mid - knob * 0.5), size: dvec2(knob, knob) }, fade(self.color_text, a), knob);
        }

        // the controls row
        let row_y = bar.pos.y + 38.0;
        let mut x = bar.pos.x + 12.0;
        for b in [Btn::Back, Btn::Play, Btn::Forward] {
            self.icon_button(cx, Rect { pos: dvec2(x, row_y), size: dvec2(BTN, BTN) }, b, false);
            x += BTN + 2.0;
        }
        // the time
        let time = if n == 0 {
            "no history yet".to_string()
        } else {
            let p = &snap.pictures[if live { n - 1 } else { cur.min(n - 1) }];
            if self.clock {
                fmt_clock(wall0 + p.received_ns as f64 / 1e9)
            } else if live {
                "now".to_string()
            } else {
                fmt_ago(snap.pictures[n - 1].built_ns.saturating_sub(p.built_ns))
            }
        };
        let tw = text_width(&mut self.draw_mono, cx, &time).max(64.0) + 16.0;
        let tr = Rect { pos: dvec2(x + 6.0, row_y), size: dvec2(tw, BTN) };
        if self.hot == Some(Btn::Clock) {
            self.draw_bg.fill(cx, tr, fade(self.color_hover, a), 7.0);
        }
        text_in(&mut self.draw_mono, cx, Rect { pos: tr.pos + dvec2(8.0, 0.0), size: tr.size }, &time, fade(self.color_text, a), 0.0);
        self.buttons.push((tr, Btn::Clock));
        if !toast.is_empty() {
            let r = Rect { pos: dvec2(tr.pos.x + tr.size.x + 10.0, row_y), size: dvec2(bar.size.x * 0.4, BTN) };
            text_in(&mut self.draw_text, cx, r, &toast, fade(self.color_dim, a), 0.0);
        }
        // right: In, Out, Save, details, LIVE
        let live_w = text_width(&mut self.draw_text, cx, "LIVE") + 30.0;
        let mut rx = bar.pos.x + bar.size.x - 12.0 - live_w;
        let pill = Rect { pos: dvec2(rx, row_y + 3.0), size: dvec2(live_w, BTN - 6.0) };
        let pill_hot = self.hot == Some(Btn::Live);
        let (pill_bg, pill_fg) = if live {
            (self.color_live, self.color_on_live)
        } else if pill_hot {
            (self.color_hover, self.color_text)
        } else {
            (fade(self.color_text, 0.12), self.color_dim)
        };
        self.draw_bg.fill(cx, pill, fade(pill_bg, a), BTN);
        self.draw_bg.fill(cx, Rect { pos: dvec2(pill.pos.x + 9.0, pill.pos.y + (pill.size.y - 6.0) * 0.5), size: dvec2(6.0, 6.0) }, fade(pill_fg, a), 6.0);
        text_in(&mut self.draw_text, cx, Rect { pos: pill.pos + dvec2(19.0, 0.0), size: pill.size - dvec2(19.0, 0.0) }, "LIVE", fade(pill_fg, a), 0.0);
        self.buttons.push((pill, Btn::Live));
        rx -= 8.0;
        for (b, on, wide) in [(Btn::Details, details, false), (Btn::Save, false, true), (Btn::Out, range.1.is_some(), false), (Btn::In, range.0.is_some(), false)] {
            let bw = if wide { BTN + 14.0 } else { BTN };
            rx -= bw + 2.0;
            let r = Rect { pos: dvec2(rx, row_y), size: dvec2(bw, BTN) };
            if wide {
                // the Save menu button: the glyph and a small chevron
                self.icon_button(cx, Rect { size: dvec2(BTN, BTN), ..r }, b, on);
                self.buttons.pop();
                self.buttons.push((r, b));
                if self.hot == Some(b) {
                    self.draw_bg.fill(cx, Rect { pos: dvec2(r.pos.x + BTN - 2.0, r.pos.y), size: dvec2(16.0, BTN) }, fade(self.color_hover, a), 7.0);
                }
                self.icon_chevron.color = fade(self.color_dim, a);
                self.icon_chevron.draw_abs(cx, Rect { pos: dvec2(r.pos.x + BTN - 2.0, r.pos.y + (BTN - 12.0) * 0.5), size: dvec2(12.0, 12.0) });
            } else {
                self.icon_button(cx, r, b, on);
            }
        }

        // the frame internals
        if details && n > 0 {
            let i = if live { n - 1 } else { cur.min(n - 1) };
            let p = &snap.pictures[i];
            let marks = snap.marks(i);
            let line = format!(
                "frame {} \u{b7} {}/{} \u{b7} {:.1} KB{}{} \u{b7} decode {:.1} ms \u{b7} dt {:.2} ms{} \u{b7} {}",
                p.frame,
                i + 1,
                n,
                p.bytes as f64 / 1024.0,
                if p.count > 1 { format!(" ({} parts)", p.count) } else { String::new() },
                if p.key { " key" } else { "" },
                p.decode_us as f64 / 1000.0,
                snap.dt_ns(i) as f64 / 1e6,
                if marks.is_empty() { String::new() } else { format!(" \u{b7} {}", marks.join(", ")) },
                status
            );
            let r = Rect { pos: dvec2(bar.pos.x + 18.0, bar.pos.y + BAR_H - 4.0), size: dvec2(bar.size.x - 36.0, DETAILS_H - 4.0) };
            text_in(&mut self.draw_mono, cx, r, &line, fade(self.color_dim, a), 0.0);
        }

        let note = self.buttons.iter().map(|(r, b)| format!("{b:?}@({:.0},{:.0})", r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5)).collect::<Vec<_>>().join(" ");
        if note != self.noted {
            crate::viewer::selftest(&format!("scrubber buttons {note} track ({:.0},{:.0})-({:.0},{:.0})", self.track.pos.x, self.track.pos.y + self.track.size.y * 0.5, self.track.pos.x + self.track.size.x, self.track.pos.y + self.track.size.y * 0.5));
            self.noted = note;
        }
        // the hovered button's tip, over the bar (the bar's own colours: theme tokens)
        if let Some((r, b)) = self.hot.and_then(|h| self.buttons.iter().find(|(_, x)| *x == h).copied()) {
            let text = self.tip_text(b);
            let tw = text_width(&mut self.draw_text, cx, &text) + 20.0;
            let tr = Rect { pos: dvec2((r.pos.x + r.size.x * 0.5 - tw * 0.5).clamp(bar.pos.x, bar.pos.x + bar.size.x - tw), bar.pos.y - 34.0), size: dvec2(tw, 26.0) };
            self.draw_bg.fill(cx, Rect { pos: tr.pos - dvec2(1.0, 1.0), size: tr.size + dvec2(2.0, 2.0) }, fade(self.color_border, a), 8.0);
            self.draw_bg.fill(cx, tr, fade(self.color_bar, 0.96 * a), 7.0);
            text_in(&mut self.draw_text, cx, tr, &text, fade(self.color_text, a), 0.5);
        }
        // the hovered moment: its thumbnail and time over the bar
        if let (Some(t), true) = (self.hover, n > 0) {
            let hi = snap.at_time(t);
            let hx = self.time_x(snap.pictures[hi].built_ns);
            let ago = fmt_ago(snap.pictures[n - 1].built_ns.saturating_sub(snap.pictures[hi].built_ns));
            let label = if self.clock { fmt_clock(wall0 + snap.pictures[hi].received_ns as f64 / 1e9) } else { ago };
            let size = self.thumb.as_ref().map_or(snap.stream_size, |t| t.2);
            let aspect = if size[0] > 0 && size[1] > 0 { size[1] as f64 / size[0] as f64 } else { 9.0 / 16.0 };
            let th = (THUMB_W * aspect).clamp(60.0, 160.0);
            let box_r = Rect { pos: dvec2((hx - THUMB_W * 0.5).clamp(bar.pos.x, bar.pos.x + bar.size.x - THUMB_W), bar.pos.y - th - 34.0), size: dvec2(THUMB_W, th + 26.0) };
            self.draw_bg.fill(cx, Rect { pos: box_r.pos - dvec2(1.0, 1.0), size: box_r.size + dvec2(2.0, 2.0) }, fade(self.color_border, a), 9.0);
            self.draw_bg.fill(cx, box_r, fade(self.color_bar, a), 8.0);
            let pic = Rect { pos: box_r.pos + dvec2(4.0, 4.0), size: dvec2(THUMB_W - 8.0, th - 4.0) };
            self.draw_bg.fill(cx, pic, fade(self.color_thumb_bg, a), 5.0);
            if let Some((_, parts, size)) = &self.thumb {
                // fit, aspect kept
                let s = (pic.size.x / size[0].max(1) as f64).min(pic.size.y / size[1].max(1) as f64);
                let (fw, fh) = (size[0] as f64 * s, size[1] as f64 * s);
                let fit = Rect { pos: dvec2(pic.pos.x + (pic.size.x - fw) * 0.5, pic.pos.y + (pic.size.y - fh) * 0.5), size: dvec2(fw, fh) };
                draw_parts(&mut self.draw_nv12, cx, parts, fit, *size);
            }
            text_in(&mut self.draw_mono, cx, Rect { pos: dvec2(box_r.pos.x, box_r.pos.y + th), size: dvec2(THUMB_W, 22.0) }, &label, fade(self.color_text, a), 0.5);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.data.is_none() {
            return;
        }
        let uid = self.uid;
        if let Some(e) = self.fade_frame.is_event(event) {
            let _ = e;
            let step = 0.18;
            self.alpha = if self.want > self.alpha { (self.alpha + step).min(self.want) } else { (self.alpha - step * 0.6).max(self.want) };
            if self.alpha != self.want {
                self.fade_frame = cx.new_next_frame();
            }
            self.draw_bg.redraw(cx);
        }
        if self.idle_timer.is_event(event).is_some() {
            let resting = cx.seconds_since_app_start() - self.last_poke >= IDLE_HIDE - 0.05;
            if resting && !self.dragging && self.hot.is_none() && self.hover.is_none() {
                self.set_want(cx, 0.0);
            }
        }
        let mine = |w: WindowId| Some(w) == self.window;
        match event {
            Event::MouseMove(e) if mine(e.window_id) => {
                self.poke(cx);
                if self.dragging {
                    if let Some(i) = self.pick(e.abs.x) {
                        self.hover = self.data.as_ref().map(|d| d.snap.pictures[i].built_ns);
                        cx.widget_action(uid, ScrubAction::Seek(i));
                        cx.widget_action(uid, ScrubAction::Hover(Some(i)));
                    }
                    self.draw_bg.redraw(cx);
                    return;
                }
                let over_track = contains(Rect { pos: self.track.pos - dvec2(0.0, 6.0), size: self.track.size + dvec2(0.0, 12.0) }, e.abs) && self.alpha > 0.05;
                let picked = if over_track { self.pick(e.abs.x) } else { None };
                let hover = picked.and_then(|i| self.data.as_ref().map(|d| d.snap.pictures[i].built_ns));
                if hover != self.hover {
                    cx.widget_action(uid, ScrubAction::Hover(picked));
                }
                if hover.is_some() || self.hover.is_some() {
                    self.hover = hover;
                    self.draw_bg.redraw(cx);
                }
                let hot = self.buttons.iter().find(|(r, _)| contains(*r, e.abs)).map(|(_, b)| *b).filter(|_| self.alpha > 0.05);
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            Event::MouseDown(e) if mine(e.window_id) && self.alpha > 0.05 && contains(self.bar, e.abs) => {
                self.poke(cx);
                if let Some(b) = self.buttons.iter().find(|(r, _)| contains(*r, e.abs)).map(|(_, b)| *b) {
                    let playing = self.data.as_ref().and_then(|d| d.playing);
                    let action = match b {
                        Btn::Back => ScrubAction::Step(-1),
                        Btn::Forward => ScrubAction::Step(1),
                        Btn::Play if playing.is_some() => ScrubAction::Stop,
                        Btn::Play => ScrubAction::Play(1.0),
                        Btn::In => ScrubAction::In,
                        Btn::Out => ScrubAction::Out,
                        Btn::Live => ScrubAction::Live,
                        Btn::Save => ScrubAction::SaveMenu(self.buttons.iter().find(|(_, x)| *x == Btn::Save).map(|(r, _)| *r).unwrap_or_default()),
                        Btn::Details => {
                            self.details = !self.details;
                            self.draw_bg.redraw(cx);
                            ScrubAction::None
                        }
                        Btn::Clock => {
                            self.clock = !self.clock;
                            self.draw_bg.redraw(cx);
                            ScrubAction::None
                        }
                    };
                    cx.widget_action(uid, action);
                    return;
                }
                if contains(Rect { pos: self.track.pos - dvec2(0.0, 6.0), size: self.track.size + dvec2(0.0, 12.0) }, e.abs) {
                    self.dragging = true;
                    if let Some(i) = self.pick(e.abs.x) {
                        cx.widget_action(uid, ScrubAction::Seek(i));
                    }
                }
            }
            Event::MouseUp(e) if mine(e.window_id) => {
                self.dragging = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_before_now_or_the_clock() {
        assert_eq!(fmt_ago(12_400_000_000), "\u{2212}12.4 s");
        assert_eq!(fmt_ago(10_000_000), "0.0 s");
        let utc = makepad_civil_time::utc(86400 * 3 + 3600 * 14 + 60 * 3 + 27);
        assert_eq!(utc.hms(), "14:03:27");
        assert!(fmt_clock(86400.0 * 3.0 + 3600.0 * 14.0 + 60.0 * 3.0 + 27.512).contains(":27.512"));
    }
}
