//! The running timer's ring: a 6 pt track with the remaining share drawn
//! as a clockwise arc from twelve o'clock whose endpoint returns toward
//! twelve as time expires; the remaining time centred inside it with
//! stable digits, the status line ("Ends 10:24" / "Paused") below.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    mod.widgets.DrawTimerRing = set_type_default() do #(DrawTimerRing::script_shader(vm)) {
        ..mod.draw.DrawQuad
        fraction: 1.0
        stroke: 6.0
        color_track: theme.color_text
        track_alpha: 0.10
        color_progress: #c86400
        pixel: fn() {
            let p = self.pos * self.rect_size
            let c = self.rect_size * 0.5
            let r = min(c.x, c.y) - self.stroke * 0.5 - 1.0
            let d = p - c
            let dist = length(d)
            let band = 1.0 - smoothstep(self.stroke * 0.5 - 0.6, self.stroke * 0.5 + 0.6, abs(dist - r))
            let track = vec4(self.color_track.rgb, self.track_alpha) * band
            // Clockwise angle from twelve: 0 at the top, 1 all the way round.
            let a = atan2(d.x, -d.y)
            let turn = fract(a / 6.2831853 + 1.0)
            let f = clamp(self.fraction, 0.0, 1.0)
            let inside = step(turn, f) * step(0.0005, f)
            // Round ends: caps at twelve and at the moving endpoint.
            let end_a = f * 6.2831853
            let end_p = c + vec2(sin(end_a), -cos(end_a)) * r
            let cap_end = 1.0 - smoothstep(self.stroke * 0.5 - 0.6, self.stroke * 0.5 + 0.6, length(p - end_p))
            let start_p = c + vec2(0.0, -r)
            let cap_start = 1.0 - smoothstep(self.stroke * 0.5 - 0.6, self.stroke * 0.5 + 0.6, length(p - start_p))
            let progress = max(band * inside, max(cap_end, cap_start) * step(0.0005, f))
            return mix(track, vec4(self.color_progress.rgb, 1.0), progress)
        }
    }

    mod.widgets.TimerRingBase = #(TimerRing::register_widget(vm))
    mod.widgets.TimerRing = set_type_default() do mod.widgets.TimerRingBase {
        width: 308 height: 308
        draw_ring: mod.widgets.DrawTimerRing{}
        readout +: {text_style: theme.font_regular{font_size: 45} color: theme.color_text}
        status +: {text_style: theme.font_regular{font_size: 11.25} color: theme.color_text_disabled}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTimerRing {
    #[deref]
    draw_super: DrawQuad,
    #[live(1.0)]
    fraction: f32,
    #[live(6.0)]
    stroke: f32,
    #[live]
    color_track: Vec4f,
    #[live(0.10)]
    track_alpha: f32,
    #[live]
    color_progress: Vec4f,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TimerRing {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_ring: DrawTimerRing,
    #[live]
    readout: DrawText,
    #[live]
    status: DrawText,
    #[rust]
    text: String,
    #[rust]
    status_text: String,
}

impl TimerRing {
    /// The remaining share, the readout and the status line.
    pub fn set(&mut self, cx: &mut Cx, fraction: f64, text: &str, status: &str) {
        let f = fraction.clamp(0.0, 1.0) as f32;
        let mut changed = false;
        if (self.draw_ring.fraction - f).abs() > 0.0005 {
            self.draw_ring.fraction = f;
            changed = true;
        }
        if self.text != text {
            self.text = text.to_string();
            changed = true;
        }
        if self.status_text != status {
            self.status_text = status.to_string();
            changed = true;
        }
        if changed {
            self.draw_ring.redraw(cx);
        }
    }

    pub fn set_stroke(&mut self, cx: &mut Cx, stroke: f32) {
        if self.draw_ring.stroke != stroke {
            self.draw_ring.stroke = stroke;
            self.draw_ring.redraw(cx);
        }
    }
}

/// Digits in equal cells, other characters at their own width.
pub fn draw_tabular(text: &mut DrawText, cx: &mut Cx2d, center: Vec2d, s: &str) {
    let Some(zero) = text.prepare_single_line_run(cx, "0") else { return };
    let cell = zero.width_in_lpxs as f64;
    let ink = (zero.ascender_in_lpxs - zero.descender_in_lpxs) as f64;
    let mut widths = Vec::new();
    let mut total = 0.0;
    for ch in s.chars() {
        let w = if ch.is_ascii_digit() {
            cell
        } else {
            text.prepare_single_line_run(cx, &ch.to_string()).map(|r| r.width_in_lpxs as f64).unwrap_or(0.0)
        };
        widths.push(w);
        total += w;
    }
    let mut x = center.x - total * 0.5;
    let y = center.y - ink * 0.5;
    for (ch, w) in s.chars().zip(widths) {
        let g = ch.to_string();
        if ch.is_ascii_digit() {
            let gw = text.prepare_single_line_run(cx, &g).map(|r| r.width_in_lpxs as f64).unwrap_or(w);
            text.draw_abs(cx, dvec2(x + (w - gw) * 0.5, y), &g);
        } else {
            text.draw_abs(cx, dvec2(x, y), &g);
        }
        x += w;
    }
}

impl Widget for TimerRing {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_ring.draw_walk(cx, walk);
        let r = self.draw_ring.area().rect(cx);
        let c = r.pos + r.size * 0.5;
        // A long readout (HH:MM:SS) drops to 44; the design's 60 otherwise.
        let base = self.readout.text_style.font_size;
        let size = if self.text.len() > 5 { base * (44.0 / 60.0) } else { base };
        self.readout.text_style.font_size = size;
        let text = self.text.clone();
        draw_tabular(&mut self.readout, cx, c, &text);
        self.readout.text_style.font_size = base;
        let status = self.status_text.clone();
        if let Some(run) = self.status.prepare_single_line_run(cx, &status) {
            let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
            self.status.draw_abs(cx, dvec2(c.x - run.width_in_lpxs as f64 * 0.5, c.y + size as f64 * 0.62 + 6.0 - ink * 0.5 + ink * 0.5), &status);
        }
        DrawStep::done()
    }
}
