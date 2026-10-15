//! The small components a pocket-sized layout is made of: a big time
//! readout (seven segments by default) and a title that scrolls while the
//! music plays. Both draw only when what they show changes; the title moves
//! frame by frame only while it plays and does not fit.

use crate::widgets::{text_fit, text_width};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The time, as a seven-segment display over its unlit ghosts ("88:88").
    // `digits` = minutes tens, minutes ones, seconds tens, seconds ones
    // (-1 = blank: no time known, or a leading zero); `clock` = position
    // and length in seconds, 1 while playing. `seg` = segment thickness (of
    // the digit height), slant, ghost strength, colon dot size. A style
    // replaces `pixel` for Nixie tubes, flip cards or a plain readout;
    // `seg_digit(p, at, size, digit)` gives (lit, any segment) coverage.
    mod.widgets.HwClockFace = mod.widgets.HwSurface{
        emissive_from_glow: true
        width: Fill
        height: Fill
        depth: -2.0
        radius: 6
        emissive: vec4(0.22, 0.78, 1.0, 0.45)
        draw_bg +: {
            ground: #x0c0d0f
            inner: vec4(0.72, 9.0, 0.0, 0.0)
            seg: uniform(vec4(0.12, 0.10, 0.07, 1.0))
            clock_pad: uniform(8.0)
            digits: uniform(vec4(-1.0, -1.0, -1.0, -1.0))
            clock: uniform(vec4(0.0, 0.0, 0.0, 0.0))
            seg_bits: fn(d: float) -> float {
                if d < -0.5 { return 0.0 }
                if d < 0.5 { return 63.0 }
                if d < 1.5 { return 6.0 }
                if d < 2.5 { return 91.0 }
                if d < 3.5 { return 79.0 }
                if d < 4.5 { return 102.0 }
                if d < 5.5 { return 109.0 }
                if d < 6.5 { return 125.0 }
                if d < 7.5 { return 7.0 }
                if d < 8.5 { return 127.0 }
                return 111.0
            }
            // A bar along x with pointed ends, `len` long, `t` thick.
            seg_bar: fn(q: vec2, len: float, t: float) -> float {
                return max(abs(q.y) - t * 0.5, abs(q.x) + abs(q.y) - len * 0.5)
            }
            seg_on: fn(bits: float, k: float) -> float {
                let v = floor(bits / k)
                return v - 2.0 * floor(v * 0.5)
            }
            seg_digit: fn(p: vec2, at: vec2, size: vec2, d: float) -> vec2 {
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let q = p - at
                let w = size.x
                let h = size.y
                let t = max(h * self.seg.x, 1.0)
                let bits = self.seg_bits(d)
                let hl = w - t * 0.9
                let vl = h * 0.5 - t * 0.9
                let da = self.seg_bar(q - vec2(w * 0.5, t * 0.5), hl, t)
                let dg = self.seg_bar(q - vec2(w * 0.5, h * 0.5), hl, t)
                let dd = self.seg_bar(q - vec2(w * 0.5, h - t * 0.5), hl, t)
                let db = self.seg_bar((q - vec2(w - t * 0.5, h * 0.25 + t * 0.25)).yx, vl, t)
                let dc = self.seg_bar((q - vec2(w - t * 0.5, h * 0.75 - t * 0.25)).yx, vl, t)
                let de = self.seg_bar((q - vec2(t * 0.5, h * 0.75 - t * 0.25)).yx, vl, t)
                let df = self.seg_bar((q - vec2(t * 0.5, h * 0.25 + t * 0.25)).yx, vl, t)
                let ca = 1.0 - smoothstep(-px, px, da)
                let cb = 1.0 - smoothstep(-px, px, db)
                let cc = 1.0 - smoothstep(-px, px, dc)
                let cd = 1.0 - smoothstep(-px, px, dd)
                let ce = 1.0 - smoothstep(-px, px, de)
                let cf = 1.0 - smoothstep(-px, px, df)
                let cg = 1.0 - smoothstep(-px, px, dg)
                let all = max(max(max(ca, cb), max(cc, cd)), max(max(ce, cf), cg))
                var lit = ca * self.seg_on(bits, 1.0)
                lit = max(lit, cb * self.seg_on(bits, 2.0))
                lit = max(lit, cc * self.seg_on(bits, 4.0))
                lit = max(lit, cd * self.seg_on(bits, 8.0))
                lit = max(lit, ce * self.seg_on(bits, 16.0))
                lit = max(lit, cf * self.seg_on(bits, 32.0))
                lit = max(lit, cg * self.seg_on(bits, 64.0))
                return vec2(lit, all)
            }
            pixel: fn() {
                let p0 = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                var o = self.layer(self.surface(p0, self.ground.xyz))
                // MM:SS in digit heights: four digits 0.56 wide, gaps 0.16,
                // the colon 0.30.
                let pad = min(self.clock_pad, self.rect_size.y * 0.12)
                let avail = self.rect_size - vec2(pad, pad) * 2.0
                let h = max(min(avail.y, avail.x / 3.3), 1.0)
                let dw = h * 0.56
                let g = h * 0.16
                let cw = h * 0.30
                let total = h * 3.18
                let x0 = (self.rect_size.x - total) * 0.5
                let y0 = (self.rect_size.y - h) * 0.5
                // The slant leans the tops right, about the middle.
                let p = vec2(p0.x + (p0.y - self.rect_size.y * 0.5) * self.seg.y, p0.y)
                var m = self.seg_digit(p, vec2(x0, y0), vec2(dw, h), self.digits.x)
                m = max(m, self.seg_digit(p, vec2(x0 + dw + g, y0), vec2(dw, h), self.digits.y))
                let cx = x0 + dw * 2.0 + g * 2.0 + cw * 0.5
                m = max(m, self.seg_digit(p, vec2(x0 + dw * 2.0 + g * 3.0 + cw, y0), vec2(dw, h), self.digits.z))
                m = max(m, self.seg_digit(p, vec2(x0 + dw * 3.0 + g * 4.0 + cw, y0), vec2(dw, h), self.digits.w))
                let r = h * self.seg.x * 0.6 * self.seg.w
                let dots = min(length(p - vec2(cx, y0 + h * 0.3)), length(p - vec2(cx, y0 + h * 0.7))) - r
                let colon = 1.0 - smoothstep(-px, px, dots)
                let known = step(-0.5, self.digits.w)
                m = max(m, vec2(colon * known, colon))
                let ghost = self.seg.z * (m.y - m.x)
                var col = mix(o.xyz, self.glow_ink.xyz, clamp(ghost, 0.0, 1.0))
                col = mix(col, self.glow_ink.xyz, m.x)
                o = vec4(col, o.w)
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    mod.widgets.HwClockBase = #(HwClock::register_widget(vm))
    mod.widgets.HwClock = set_type_default() do mod.widgets.HwClockBase{
        width: Fill
        height: Fill
        flow: Overlay
        face := mod.widgets.HwClockFace{}
    }

    // The playing title, scrolling while it plays and does not fit.
    // `speed` in points a second, `gap` between the end and the start.
    mod.widgets.HwMarqueeBase = #(HwMarquee::register_widget(vm))
    mod.widgets.HwMarquee = set_type_default() do mod.widgets.HwMarqueeBase{
        width: Fill
        height: Fill
        speed: 28.0
        gap: 48.0
        draw_text +: {
            color: #xc8ccd2
            text_style: theme.font_regular{font_size: 10.0}
        }
    }
}

// ---- clock ----

#[derive(Script, ScriptHook, Widget)]
pub struct HwClock {
    #[deref]
    view: View,
    /// Whole seconds shown, the length, and whether it plays.
    #[rust]
    shown: Option<(Option<u64>, u64, bool)>,
}

#[derive(Clone, Debug, Default)]
pub enum HwClockAction {
    #[default]
    None,
    DoubleClicked,
}

impl Widget for HwClock {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Hit::FingerDown(down) = event.hits(cx, self.view.area()) {
            if down.tap_count == 2 {
                cx.widget_action(self.widget_uid(), HwClockAction::DoubleClicked);
            }
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let (position, length, playing) = self.shown.unwrap_or((None, 0, false));
        let digits = match position {
            Some(secs) => {
                let minutes = (secs / 60).min(99);
                let seconds = secs % 60;
                let tens = if minutes >= 10 { (minutes / 10) as f32 } else { -1.0 };
                [tens, (minutes % 10) as f32, (seconds / 10) as f32, (seconds % 10) as f32]
            }
            None => [-1.0; 4],
        };
        let clock = [position.unwrap_or(0) as f32, length as f32, if playing { 1.0 } else { 0.0 }, 0.0];
        if let Some(mut face) = self.view.relief_view(cx, ids!(face)).borrow_mut() {
            face.view.draw_bg.set_uniform(cx, live_id!(digits), &digits);
            face.view.draw_bg.set_uniform(cx, live_id!(clock), &clock);
            face.lit = if position.is_some() { if playing { 1.0 } else { 0.5 } } else { 0.0 };
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

impl HwClockRef {
    pub fn double_clicked(&self, actions: &Actions) -> bool {
        actions.find_widget_action(self.widget_uid()).is_some_and(|action| matches!(action.cast(), HwClockAction::DoubleClicked))
    }

    /// The time to show (seconds; `None` = not known), redrawn only when
    /// the second, the length or playing changes.
    pub fn set(&self, cx: &mut Cx, position: Option<f64>, length: f64, playing: bool) {
        let Some(mut inner) = self.borrow_mut() else { return };
        // Nothing known yet reads 0:00, as a stopped player does.
        let key = (Some(position.unwrap_or(0.0).max(0.0) as u64), length.max(0.0) as u64, playing);
        if inner.shown != Some(key) {
            inner.shown = Some(key);
            inner.view.redraw(cx);
        }
    }
}

// ---- marquee ----

#[derive(Script, ScriptHook, Widget)]
pub struct HwMarquee {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_text: DrawText,
    #[live(28.0)]
    speed: f64,
    #[live(48.0)]
    gap: f64,
    #[rust]
    text: String,
    #[rust]
    playing: bool,
    /// When the scroll started, and how far it had got when it last
    /// stopped (so a pause holds the title where it is).
    #[rust]
    started: f64,
    #[rust]
    held: f64,
    #[rust]
    color: Option<Vec4f>,
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
}

impl Widget for HwMarquee {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            self.draw_text.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout { clip_x: true, clip_y: true, ..Layout::default() });
        let rect = cx.turtle().rect();
        if let Some(color) = self.color {
            self.draw_text.color = color;
        }
        if !self.text.is_empty() && rect.size.x > 4.0 {
            let width = text_width(&self.draw_text, cx, &self.text);
            let height = self.draw_text.layout(cx, 0.0, 0.0, None, false, Align::default(), &self.text).size_in_lpxs.height as f64;
            let y = rect.pos.y + (rect.size.y - height) * 0.5;
            if width <= rect.size.x {
                self.draw_text.draw_abs(cx, dvec2(rect.pos.x, y), &self.text);
            } else if !self.playing && self.held == 0.0 {
                // Still: the start of the title, cut with an ellipsis.
                let mid = rect.pos.y + rect.size.y * 0.5;
                text_fit(&mut self.draw_text, cx, rect.pos.x, mid, rect.size.x, &self.text);
            } else {
                let period = width + self.gap;
                let now = cx.seconds_since_app_start();
                let run = if self.playing { self.held + (now - self.started) * self.speed } else { self.held };
                let offset = run.rem_euclid(period);
                self.draw_text.draw_abs(cx, dvec2(rect.pos.x - offset, y), &self.text);
                self.draw_text.draw_abs(cx, dvec2(rect.pos.x - offset + period, y), &self.text);
                if self.playing {
                    self.next_frame = cx.new_next_frame();
                }
            }
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl HwMarqueeRef {
    /// The title, whether it scrolls, and its colour (None keeps the
    /// style's).
    pub fn set(&self, cx: &mut Cx, text: &str, playing: bool, color: Option<Vec4f>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        let now = cx.seconds_since_app_start();
        let mut changed = false;
        if inner.text != text {
            inner.text = text.to_string();
            inner.held = 0.0;
            inner.started = now;
            changed = true;
        }
        if inner.playing != playing {
            if !playing {
                inner.held += (now - inner.started) * inner.speed;
            }
            inner.started = now;
            inner.playing = playing;
            changed = true;
        }
        if inner.color != color {
            inner.color = color;
            changed = true;
        }
        if changed {
            inner.draw_text.redraw(cx);
        }
    }
}
