//! The sky artwork: one code-native quad that paints a day/night gradient,
//! sun or moon, stars, soft cloud blobs and rain, snow, fog or a lightning
//! bolt from two numbers (`kind`, `is_day`). No textures, any size.
use crate::model::SkyKind;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    mod.widgets.DrawSky = set_type_default() do #(DrawSky::script_shader(vm)) {
        ..mod.draw.DrawQuad
        kind: 0.0
        is_day: 1.0
        border_radius: theme.container_corner_radius
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let uv = self.pos
            let aspect = self.rect_size.x / max(self.rect_size.y, 1.0)
            let q = vec2(uv.x * aspect, uv.y)
            let day = clamp(self.is_day, 0.0, 1.0)

            let is_overcast = step(1.5, self.kind) * step(self.kind, 2.5)
            let is_rain = step(2.5, self.kind) * step(self.kind, 3.5)
            let is_snow = step(3.5, self.kind) * step(self.kind, 4.5)
            let is_fog = step(4.5, self.kind) * step(self.kind, 5.5)
            let is_thunder = step(5.5, self.kind)
            let cover = min(self.kind, 2.0) * 0.5
            let gloom = is_rain * 0.8 + is_thunder * 1.0 + is_overcast * 0.5 + is_snow * 0.3 + is_fog * 0.35

            // Sky
            let top = mix(vec3(0.04, 0.07, 0.20), vec3(0.16, 0.47, 0.90), day)
            let bot = mix(vec3(0.14, 0.22, 0.42), vec3(0.62, 0.84, 0.98), day)
            let mut col = mix(top, bot, uv.y)
            let gray = vec3(0.55, 0.60, 0.68) * mix(0.35, 1.0, day)
            col = mix(col, gray, gloom * 0.75)

            // Stars, only on a clear night
            let cell = floor(q * 14.0)
            let h = fract(sin(dot(cell, vec2(12.9898, 78.233))) * 43758.5453)
            let cp = fract(q * 14.0) - vec2(0.5, 0.5)
            let star = (1.0 - smoothstep(0.04, 0.08, length(cp))) * step(0.93, h)
            col = mix(col, vec3(1.0, 1.0, 1.0), star * (1.0 - day) * (1.0 - cover) * 0.9)

            // Sun by day, moon by night
            let sun_pos = vec2(aspect * 0.78, 0.30)
            let sd = length(q - sun_pos)
            let sun_vis = (1.0 - cover * 0.85) * (1.0 - is_fog * 0.7)
            let sun_glow = (1.0 - smoothstep(0.15, 0.48, sd)) * 0.45
            let sun_core = 1.0 - smoothstep(0.14, 0.152, sd)
            col = mix(col, vec3(1.0, 0.93, 0.6), sun_glow * day * sun_vis)
            col = mix(col, vec3(1.0, 0.86, 0.35), sun_core * day * sun_vis)
            let md = length(q - sun_pos - vec2(0.055, -0.025))
            let moon = (1.0 - smoothstep(0.12, 0.132, sd)) * smoothstep(0.10, 0.112, md)
            col = mix(col, vec3(0.93, 0.93, 0.85), moon * (1.0 - day) * sun_vis)

            // Clouds: unions of discs; a second bank rolls over the sun when overcast
            let cloud_col = mix(vec3(1.0, 1.0, 1.0), vec3(0.45, 0.48, 0.55), gloom) * mix(0.55, 1.0, day)
            let ca = vec2(aspect * 0.36, 0.60)
            let a1 = length((q - ca) * vec2(1.0, 1.7)) - 0.21
            let a2 = length(q - ca - vec2(-0.17, 0.01)) - 0.13
            let a3 = length(q - ca - vec2(0.01, -0.11)) - 0.16
            let a4 = length(q - ca - vec2(0.20, -0.01)) - 0.14
            let cloud_a = min(min(a1, a2), min(a3, a4))
            let ma = (1.0 - smoothstep(0.0, 0.012, cloud_a)) * step(0.25, cover)
            let shade_a = 1.0 - 0.14 * smoothstep(ca.y - 0.05, ca.y + 0.16, q.y)
            col = mix(col, cloud_col * shade_a, ma)

            let cb = vec2(aspect * 0.76, 0.40)
            let b1 = length((q - cb) * vec2(1.0, 1.7)) - 0.17
            let b2 = length(q - cb - vec2(-0.14, 0.0)) - 0.10
            let b3 = length(q - cb - vec2(0.02, -0.09)) - 0.13
            let b4 = length(q - cb - vec2(0.16, 0.0)) - 0.11
            let cloud_b = min(min(b1, b2), min(b3, b4))
            let mb = (1.0 - smoothstep(0.0, 0.012, cloud_b)) * step(0.99, cover)
            let shade_b = 1.0 - 0.14 * smoothstep(cb.y - 0.05, cb.y + 0.14, q.y)
            col = mix(col, cloud_col * shade_b, mb)

            // Rain streaks under the clouds
            let rp = vec2(q.x * 30.0 + q.y * 6.0, q.y * 9.0)
            let row_shift = fract(sin(floor(rp.x) * 7.13) * 3.7)
            let streak = (1.0 - smoothstep(0.05, 0.16, abs(fract(rp.x) - 0.5))) * step(0.5, fract(rp.y + row_shift))
            col = mix(col, vec3(0.75, 0.85, 1.0), streak * smoothstep(0.62, 0.8, q.y) * (is_rain + is_thunder) * 0.7)

            // Snow flakes
            let sp = q * 12.0
            let scell = floor(sp)
            let sh = fract(sin(dot(scell, vec2(41.3, 289.1))) * 22874.3)
            let sc = fract(sp) - vec2(0.5, 0.5) - (vec2(sh, fract(sh * 7.0)) - vec2(0.5, 0.5)) * 0.6
            let flake = (1.0 - smoothstep(0.07, 0.11, length(sc))) * step(0.5, sh)
            col = mix(col, vec3(1.0, 1.0, 1.0), flake * is_snow * smoothstep(0.5, 0.75, q.y) * 0.95)

            // Fog bands
            let band = 0.5 + 0.5 * sin(q.y * 40.0 + q.x * 3.0)
            let fog_col = mix(vec3(0.30, 0.32, 0.38), vec3(0.78, 0.80, 0.84), day)
            col = mix(col, fog_col, is_fog * (0.35 + 0.25 * band) * smoothstep(0.25, 0.8, q.y))

            // Lightning bolt: three segments under the main cloud
            let b0 = vec2(ca.x + 0.03, 0.74)
            let l1 = vec2(ca.x - 0.03, 0.85)
            let l2 = vec2(ca.x + 0.02, 0.85)
            let l3 = vec2(ca.x - 0.04, 0.98)
            let e1 = l1 - b0
            let t1 = clamp(dot(q - b0, e1) / dot(e1, e1), 0.0, 1.0)
            let d1 = length(q - b0 - e1 * t1)
            let e2 = l2 - l1
            let t2 = clamp(dot(q - l1, e2) / dot(e2, e2), 0.0, 1.0)
            let d2 = length(q - l1 - e2 * t2)
            let e3 = l3 - l2
            let t3 = clamp(dot(q - l2, e3) / dot(e3, e3), 0.0, 1.0)
            let d3 = length(q - l2 - e3 * t3)
            let bolt = 1.0 - smoothstep(0.012, 0.022, min(d1, min(d2, d3)))
            col = mix(col, vec3(1.0, 0.95, 0.6), bolt * is_thunder)

            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
            sdf.fill(vec4(col.x, col.y, col.z, 1.0))
            return sdf.result
        }
    }

    mod.widgets.SkyViewBase = #(SkyView::register_widget(vm))
    mod.widgets.SkyView = set_type_default() do mod.widgets.SkyViewBase {
        width: Fill height: Fill
        draw_sky: mod.widgets.DrawSky {}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSky {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    kind: f32,
    #[live(1.0)]
    is_day: f32,
    #[live(12.0)]
    border_radius: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct SkyView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_sky: DrawSky,
}

impl SkyView {
    pub fn set_conditions(&mut self, cx: &mut Cx, kind: SkyKind, is_day: bool) {
        let k = kind as i32 as f32;
        let d = if is_day { 1.0 } else { 0.0 };
        if self.draw_sky.kind != k || self.draw_sky.is_day != d {
            self.draw_sky.kind = k;
            self.draw_sky.is_day = d;
            self.draw_sky.redraw(cx);
        }
    }
}

impl Widget for SkyView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_sky.draw_walk(cx, walk);
        DrawStep::done()
    }
}
