//! The parked window on the flat desk: the window's capture drawn as one
//! axis-aligned textured rect, only scaled and positioned (never turned or
//! skewed). Below scale 0.6 a 4-tap rotated-grid footprint filter keeps
//! small text from aliasing; there are no mips and no extra pass.
use super::Pose;
use crate::dock_warp::WindowFrame;
use crate::layout::LRect;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*
    mod.widgets.DrawWarpPanel = set_type_default() do #(DrawWarpPanel::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        shown: vec4(0.0, 0.0, 1.0, 1.0)
        desk: vec2(1.0, 1.0)
        shape: vec4(1.0, 1.0, 0.0, 0.0)
        alpha: 1.0
        pixel: fn() {
            let p = self.rect_pos + self.pos * self.rect_size
            let uv = (p - self.shown.xy) / self.shown.zw
            let aa = max(length(vec2(dFdx(uv.x), dFdy(uv.x))), 0.00001)
            let edge = smoothstep(0.0, aa, uv.x) * smoothstep(0.0, aa, 1.0 - uv.x)
            let dx = dFdx(uv)
            let dy = dFdy(uv)
            let sharp = self.image.sample(uv)
            let soft = (self.image.sample(uv + dx * 0.125 + dy * 0.375)
                + self.image.sample(uv - dx * 0.125 - dy * 0.375)
                + self.image.sample(uv + dx * 0.375 - dy * 0.125)
                + self.image.sample(uv - dx * 0.375 + dy * 0.125)) * 0.25
            let color = mix(sharp, soft, self.shape.w)
            let sdf = Sdf2d.viewport(uv * self.desk)
            sdf.box(0.0, 0.0, self.desk.x, self.desk.y, self.shape.z)
            sdf.fill(color * edge * self.alpha)
            return sdf.result
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawWarpPanel {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    shown: Vec4f,
    #[live]
    desk: Vec2f,
    #[live]
    shape: Vec4f,
    #[live]
    alpha: f32,
}

impl DrawWarpPanel {
    /// Draw `frame` (the window captured at its full desk size) as the
    /// pose's rect. `radius` is the window's visible corner radius in
    /// desk points; `alpha` fades it (a cross-fade's outgoing layer).
    pub fn draw(&mut self, cx: &mut Cx2d, frame: &WindowFrame, desk: Rect, pose: &Pose, radius: f32, alpha: f32) {
        self.alpha = alpha;
        self.draw_vars.set_texture(0, frame.texture());
        let s = pose.shown;
        self.shown = vec4(s.x as f32, s.y as f32, s.w as f32, s.h as f32);
        self.desk = vec2(desk.size.x as f32, desk.size.y as f32);
        // Sdf2d.box uses half the visible corner radius.
        let filter = if pose.scale < 0.6 { 1.0 } else { 0.0 };
        self.shape = vec4(1.0, 1.0, radius * 0.5, filter);
        let b: LRect = pose.bounds();
        self.draw_abs(cx, Rect { pos: dvec2(b.x, b.y), size: dvec2(b.w, b.h) });
    }
}
