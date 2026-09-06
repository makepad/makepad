//! The analog face: one code-native quad. Ticks and hands are signed
//! distances in the pixel shader, so the face stays crisp at any size and
//! follows the theme's text/accent colours across style reloads.
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
        color_face: theme.color_bg_container
        color_ring: theme.color_text_disabled
        color_hand: theme.color_text
        color_accent: theme.color_focus
        pixel: fn() {
            let p = self.pos * self.rect_size
            let c = self.rect_size * 0.5
            let r = min(c.x, c.y) - 5.0
            let d = p - c
            let dist = length(d)
            let disc = 1.0 - smoothstep(r - 1.0, r + 0.5, dist)
            let ring = smoothstep(r - 1.5, r - 0.5, dist)*0.18
            let light = clamp(0.5+(d.x+d.y)/max(r*4.0,1.0),0.0,1.0)
            let enamel = mix(self.color_face.rgb,self.color_accent.rgb,0.04+light*0.035)
            let mut col = mix(enamel, self.color_ring.rgb, ring)

            // Twelve hour ticks: arc distance to the nearest 30 degree spoke.
            let ang = atan2(d.x, -d.y)
            let minute_arc=abs(fract(ang / 6.2831853 * 60.0 + 0.5)-0.5)*(6.2831853/60.0)*dist
            let minute_tick=(1.0-smoothstep(0.35,0.9,minute_arc))*smoothstep(r*0.92,r*0.93,dist)*(1.0-smoothstep(r*0.95,r*0.96,dist))
            col=mix(col,self.color_ring.rgb,minute_tick*0.45)
            let tick_arc = abs(fract(ang / 6.2831853 * 12.0 + 0.5) - 0.5) * (6.2831853 / 12.0) * dist
            let tick = (1.0 - smoothstep(0.6, 1.4, tick_arc))
                * smoothstep(r * 0.84, r * 0.86, dist)
                * (1.0 - smoothstep(r * 0.93, r * 0.95, dist))
            col = mix(col, self.color_ring.rgb, tick)

            // Hands: distance to a segment along the hand direction.
            let hd = vec2(sin(self.hour_angle), -cos(self.hour_angle))
            let ht = clamp(dot(d, hd), -r * 0.08, r * 0.52)
            let hdist = length(d - hd * ht)
            let hand_h = 1.0 - smoothstep(r * 0.027, r * 0.027 + 1.0, hdist)
            col = mix(col, self.color_hand.rgb, hand_h)

            let md = vec2(sin(self.minute_angle), -cos(self.minute_angle))
            let mt = clamp(dot(d, md), -r * 0.08, r * 0.76)
            let mdist = length(d - md * mt)
            let hand_m = 1.0 - smoothstep(r * 0.018, r * 0.018 + 1.0, mdist)
            col = mix(col, self.color_hand.rgb, hand_m)

            let sd = vec2(sin(self.second_angle), -cos(self.second_angle))
            let st = clamp(dot(d, sd), -r * 0.16, r * 0.82)
            let sdist = length(d - sd * st)
            let hand_s = (1.0 - smoothstep(0.7, 1.7, sdist)) * self.show_seconds
            col = mix(col, self.color_accent.rgb, hand_s)

            let cap = 1.0 - smoothstep(r * 0.04, r * 0.04 + 1.0, dist)
            col = mix(col, self.color_accent.rgb, cap)
            return vec4(col * disc, disc)
        }
    }

    mod.widgets.ClockFaceBase = #(ClockFace::register_widget(vm))
    mod.widgets.ClockFace = set_type_default() do mod.widgets.ClockFaceBase {
        width: Fill height: Fill
        draw_face: mod.widgets.DrawClockFace {}
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 12}}
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
    color_ring: Vec4f,
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
        let rect=self.draw_face.area().rect(cx);
        let center=rect.pos+rect.size*0.5;
        let radius=rect.size.x.min(rect.size.y)*0.34;
        self.draw_text.text_style.font_size=(radius*0.11).clamp(8.0,12.0) as f32;
        for (text,x,y) in [("12",0.0,-1.0),("3",1.0,0.0),("6",0.0,1.0),("9",-1.0,0.0)] {
            if let Some(run)=self.draw_text.prepare_single_line_run(cx,text) {
                self.draw_text.draw_abs(cx,center+dvec2(x*radius-run.width_in_lpxs as f64*0.5,y*radius-7.0),text);
            }
        }
        DrawStep::done()
    }
}
