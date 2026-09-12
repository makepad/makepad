//! The analog face: one code-native quad. The disc, ticks and hands are
//! signed distances in the pixel shader, so the face stays crisp at any
//! size and follows the theme's text/secondary/accent colours across
//! style reloads. Metrics are the design's for the 292 pt dial and scale
//! with the dial: minute ticks 1×4, hour ticks 2×8, hands 70×5 / 105×3.5
//! / 117×1.5 with rounded ends, a 17 pt second-hand tail, a 7 pt hub, and
//! all twelve numerals on radius 113.
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    mod.widgets.DrawClockFace = set_type_default() do #(DrawClockFace::script_shader(vm)) {
        ..mod.draw.DrawQuad
        hour_angle: 0.0
        minute_angle: 0.0
        second_angle: 0.0
        show_seconds: 1.0
        color_face: theme.color_inset
        color_tick: theme.color_text_disabled
        color_hand: theme.color_text
        color_accent: #ff9500
        pixel: fn() {
            let p = self.pos * self.rect_size
            let c = self.rect_size * 0.5
            let r = min(c.x, c.y)
            // Everything below is in units of the 292 pt reference dial.
            let s = r / 146.0
            let d = p - c
            let dist = length(d)
            let disc = 1.0 - smoothstep(r - 0.8, r + 0.4, dist)
            let mut col = self.color_face.rgb

            // Sixty minute ticks (1×4) and twelve hour ticks (2×8), their
            // outer ends on the dial's edge minus a 6 pt inset.
            let ang = atan2(d.x, -d.y)
            let outer = r - 6.0 * s
            let minute_arc = abs(fract(ang / 6.2831853 * 60.0 + 0.5) - 0.5) * (6.2831853 / 60.0) * dist
            let minute_tick = (1.0 - smoothstep(0.5 * s, 0.5 * s + 0.8, minute_arc))
                * smoothstep(outer - 4.0 * s - 0.6, outer - 4.0 * s + 0.4, dist)
                * (1.0 - smoothstep(outer - 0.4, outer + 0.6, dist))
            col = mix(col, self.color_tick.rgb, minute_tick * 0.7)
            let hour_arc = abs(fract(ang / 6.2831853 * 12.0 + 0.5) - 0.5) * (6.2831853 / 12.0) * dist
            let hour_tick = (1.0 - smoothstep(1.0 * s, 1.0 * s + 0.8, hour_arc))
                * smoothstep(outer - 8.0 * s - 0.6, outer - 8.0 * s + 0.4, dist)
                * (1.0 - smoothstep(outer - 0.4, outer + 0.6, dist))
            col = mix(col, self.color_hand.rgb, hour_tick)

            // Hands: capsules from a short tail to their length, rounded ends.
            let hd = vec2(sin(self.hour_angle), -cos(self.hour_angle))
            let ht = clamp(dot(d, hd), -10.0 * s, 70.0 * s)
            let hand_h = 1.0 - smoothstep(2.5 * s, 2.5 * s + 0.9, length(d - hd * ht))
            col = mix(col, self.color_hand.rgb, hand_h)

            let md = vec2(sin(self.minute_angle), -cos(self.minute_angle))
            let mt = clamp(dot(d, md), -10.0 * s, 105.0 * s)
            let hand_m = 1.0 - smoothstep(1.75 * s, 1.75 * s + 0.9, length(d - md * mt))
            col = mix(col, self.color_hand.rgb, hand_m)

            let sd = vec2(sin(self.second_angle), -cos(self.second_angle))
            let st = clamp(dot(d, sd), -17.0 * s, 117.0 * s)
            let hand_s = (1.0 - smoothstep(0.75 * s, 0.75 * s + 0.9, length(d - sd * st))) * self.show_seconds
            col = mix(col, self.color_accent.rgb, hand_s)

            // The hub: accent over the second hand, a small dark centre.
            let hub = 1.0 - smoothstep(3.5 * s, 3.5 * s + 0.9, dist)
            col = mix(col, mix(self.color_hand.rgb, self.color_accent.rgb, self.show_seconds), hub)
            let pin = 1.0 - smoothstep(1.2 * s, 1.2 * s + 0.8, dist)
            col = mix(col, self.color_face.rgb, pin)
            return vec4(col * disc, disc)
        }
    }

    mod.widgets.ClockFaceBase = #(ClockFace::register_widget(vm))
    mod.widgets.ClockFace = set_type_default() do mod.widgets.ClockFaceBase {
        width: Fill height: Fill
        draw_face: mod.widgets.DrawClockFace {}
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 13.5}}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawClockFace {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hour_angle: f32,
    #[live]
    minute_angle: f32,
    #[live]
    second_angle: f32,
    #[live(1.0)]
    show_seconds: f32,
    #[live]
    color_face: Vec4f,
    #[live]
    color_tick: Vec4f,
    #[live]
    color_hand: Vec4f,
    #[live]
    color_accent: Vec4f,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ClockFace {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_face: DrawClockFace,
    #[live]
    draw_text: DrawText,
    /// Numeral size in makepad points (the design's 18 logical = 13.5;
    /// the tile passes its 12 as 9).
    #[live(13.5)]
    numeral_size: f32,
}

impl ClockFace {
    /// Point the hands at a wall-clock time; redraws only when a hand moved.
    pub fn set_time(&mut self, cx: &mut Cx, hour: u32, minute: u32, second: u32) {
        let tau = std::f32::consts::TAU;
        let s = second as f32;
        let m = minute as f32 + s / 60.0;
        let h = (hour % 12) as f32 + m / 60.0;
        let (ha, ma, sa) = (h / 12.0 * tau, m / 60.0 * tau, s / 60.0 * tau);
        let d = &mut self.draw_face;
        if d.hour_angle != ha || d.minute_angle != ma || d.second_angle != sa {
            d.hour_angle = ha;
            d.minute_angle = ma;
            d.second_angle = sa;
            d.redraw(cx);
        }
    }

    pub fn set_show_seconds(&mut self, cx: &mut Cx, show: bool) {
        let v = if show { 1.0 } else { 0.0 };
        if self.draw_face.show_seconds != v {
            self.draw_face.show_seconds = v;
            self.draw_face.redraw(cx);
        }
    }
}

impl Widget for ClockFace {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_face.draw_walk(cx, walk);
        let rect = self.draw_face.area().rect(cx);
        let center = rect.pos + rect.size * 0.5;
        let r = rect.size.x.min(rect.size.y) * 0.5;
        // Numerals on radius 113 of the 292 dial (57 of 154), at the
        // requested size, all twelve, centred on their ink.
        let radius = if self.numeral_size <= 9.5 { r * (57.0 / 77.0) } else { r * (113.0 / 146.0) };
        self.draw_text.text_style.font_size = self.numeral_size;
        for h in 1..=12u32 {
            let a = h as f64 / 12.0 * std::f64::consts::TAU;
            let text = h.to_string();
            if let Some(run) = self.draw_text.prepare_single_line_run(cx, &text) {
                let ink = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
                let pos = center + dvec2(a.sin() * radius - run.width_in_lpxs as f64 * 0.5, -a.cos() * radius - ink * 0.5);
                self.draw_text.draw_abs(cx, pos, &text);
            }
        }
        DrawStep::done()
    }
}
