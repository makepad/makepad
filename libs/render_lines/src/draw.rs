//! The GPU side: capsule segments and sprites drawn into whatever pass is
//! current.
//!
//! Output is linear and premultiplied, unclamped: in an RGBA16F target
//! (the render graph's HDR accumulation) a glow above 1 survives to bloom;
//! the shaders are declared `Bgra8Unorm` so the platform builds their
//! RGBA16F pipelines on first use (and they also draw into 8-bit targets,
//! clamped). No accumulation, overlay or post of their own.
//!
//! One draw call per batch (the style is uniforms); batches with equal
//! styles in a row merge into one call.

use crate::batch::*;
use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    // =====================================================================
    // SEGMENTS: capsules (or butt-ended bars) in physical pixels, with the
    // exact box-filter coverage of `ink.rs`.
    // =====================================================================
    mod.draw.DrawLineSegment = mod.std.set_type_default() do #(DrawLineSegment::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        color_format: @Bgra8Unorm
        // view_proj columns
        u_c0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        u_c1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        u_c2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        u_c3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        // physical viewport w, h; px_scale; projection y scale
        u_vp: uniform(vec4(1920.0, 1080.0, 1.0, 1.0))
        // world widths, additive, round caps, min px
        u_mode: uniform(vec4(0.0, 0.0, 1.0, 0.7))
        // dash, gap, dash offset, intensity
        u_dash: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        // trim start, trim end, tail, 0
        u_trim: uniform(vec4(0.0, 1.0e30, 0.0, 0.0))
        // fade near, far, active, 0
        u_fade: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // time, birth fade, life, 0
        u_birth: uniform(vec4(1.0e30, 0.0, 0.0, 0.0))
        v_px: varying(vec2f)
        v_len: varying(float)
        v_w: varying(vec2f)
        v_half: varying(vec2f)
        v_alpha: varying(vec2f)
        v_dist: varying(vec2f)
        v_birth: varying(vec2f)
        v_ca: varying(vec4f)
        v_cb: varying(vec4f)

        clip_of: fn(p: vec3) -> vec4 {
            return self.u_c0 * p.x + self.u_c1 * p.y + self.u_c2 * p.z + self.u_c3
        }

        // Physical-pixel width of a (logical px or world) width at clip w.
        phys_width: fn(w: float, cw: float) -> float {
            if self.u_mode.x > 0.5 {
                return w * self.u_vp.w * self.u_vp.y * 0.5 / max(cw, 0.0001)
            }
            return w * self.u_vp.z
        }

        vertex: fn() {
            let corner = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            let ca = self.clip_of(self.s_a.xyz)
            let cb = self.clip_of(self.s_b.xyz)
            // The part in front of the near plane.
            let eps = 0.0001
            var t0 = 0.0
            var t1 = 1.0
            var gone = 0.0
            if ca.w < eps {
                if cb.w < eps {
                    gone = 1.0
                } else {
                    t0 = (eps - ca.w) / (cb.w - ca.w)
                }
            } else {
                if cb.w < eps {
                    t1 = (eps - ca.w) / (cb.w - ca.w)
                }
            }
            let pa = mix(ca, cb, t0)
            let pb = mix(ca, cb, t1)
            let half_px = self.u_vp.xy * 0.5
            let sa = pa.xy / pa.w * half_px
            let sb = pb.xy / pb.w * half_px
            let dv = sb - sa
            let len = max(length(dv), 0.0001)
            var dir = vec2(1.0, 0.0)
            if length(dv) > 0.0001 {
                dir = dv / len
            }
            let nrm = vec2(0.0 - dir.y, dir.x)
            let wa = self.phys_width(mix(self.s_a.w, self.s_b.w, t0), pa.w)
            let wb = self.phys_width(mix(self.s_a.w, self.s_b.w, t1), pb.w)
            let floor = max(self.u_mode.w, 0.001)
            let ha = max(wa, floor) * 0.5
            let hb = max(wb, floor) * 0.5
            let reach = max(ha, hb) + 1.0
            let t = corner.x * 0.5 + 0.5
            let along = mix(0.0 - reach, len + reach, t)
            let across = corner.y * reach
            let s = sa + dir * along + nrm * across
            let f = clamp(along / len, 0.0, 1.0)
            let z = mix(pa.z / pa.w, pb.z / pb.w, f)
            if gone > 0.5 {
                self.vertex_pos = vec4(0.0, 0.0, 2.0, 1.0)
            } else {
                self.vertex_pos = vec4(s.x / half_px.x, s.y / half_px.y, z, 1.0)
            }
            self.v_px = vec2(along, across)
            self.v_len = len
            self.v_w = vec2(pa.w, pb.w)
            self.v_half = vec2(ha, hb)
            self.v_alpha = vec2(min(wa / floor, 1.0), min(wb / floor, 1.0))
            self.v_dist = vec2(mix(self.s_m.x, self.s_m.y, t0), mix(self.s_m.x, self.s_m.y, t1))
            self.v_birth = vec2(mix(self.s_m.z, self.s_m.w, t0), mix(self.s_m.z, self.s_m.w, t1))
            self.v_ca = mix(self.s_ca, self.s_cb, t0)
            self.v_cb = mix(self.s_ca, self.s_cb, t1)
        }

        // Exact overlap of a one-pixel box with [-half, half] at distance d.
        cover: fn(d: float, half: float) -> float {
            return clamp(min(d + 0.5, half) - max(d - 0.5, 0.0 - half), 0.0, 1.0)
        }

        pixel: fn() -> vec4 {
            let x = self.v_px.x
            let len = self.v_len
            let fx = clamp(x / len, 0.0, 1.0)
            let half = mix(self.v_half.x, self.v_half.y, fx)
            var a = 0.0
            if self.u_mode.z > 0.5 {
                let d = length(vec2(x - clamp(x, 0.0, len), self.v_px.y))
                a = self.cover(d, half)
            } else {
                let along = clamp(min(x + 0.5, len) - max(x - 0.5, 0.0), 0.0, 1.0)
                a = self.cover(abs(self.v_px.y), half) * along
            }
            a = a * mix(self.v_alpha.x, self.v_alpha.y, fx)
            // Perspective-correct parameter along the segment.
            let ia = 1.0 / self.v_w.x
            let ib = 1.0 / self.v_w.y
            let u = fx * ib / max(mix(ia, ib, fx), 0.0000001)
            let dist = mix(self.v_dist.x, self.v_dist.y, u)
            let color = mix(self.v_ca, self.v_cb, u)
            // Trim window, with one pixel of AA in distance units.
            let dd = max(abs(self.v_dist.y - self.v_dist.x) / len, 0.000001)
            a = a * clamp((dist - self.u_trim.x) / dd + 0.5, 0.0, 1.0)
            a = a * clamp((self.u_trim.y - dist) / dd + 0.5, 0.0, 1.0)
            if self.u_trim.z > 0.0 {
                a = a * clamp((dist - (self.u_trim.y - self.u_trim.z)) / self.u_trim.z, 0.0, 1.0)
            }
            if self.u_dash.x > 0.0 {
                let period = self.u_dash.x + self.u_dash.y
                let q = dist + self.u_dash.z
                let m = q - floor(q / period) * period
                a = a * clamp((self.u_dash.x - m) / dd + 0.5, 0.0, 1.0) * clamp(m / dd + 0.5, 0.0, 1.0)
            }
            if self.u_fade.z > 0.5 {
                let w = 1.0 / max(mix(ia, ib, fx), 0.0000001)
                a = a * (1.0 - smoothstep(self.u_fade.x, self.u_fade.y, w))
            }
            let birth = mix(self.v_birth.x, self.v_birth.y, u)
            let age = self.u_birth.x - birth
            if age < 0.0 {
                a = 0.0
            }
            if self.u_birth.y > 0.0 {
                a = a * clamp(age / self.u_birth.y, 0.0, 1.0)
            }
            if self.u_birth.z > 0.0 {
                a = a * (1.0 - smoothstep(self.u_birth.z * 0.5, self.u_birth.z, age))
            }
            a = a * color.w
            if a < 0.0005 {
                discard()
            }
            let rgb = color.xyz * (a * self.u_dash.w)
            if self.u_mode.y > 0.5 {
                return vec4(rgb, 0.0)
            }
            return vec4(rgb, a)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // =====================================================================
    // POINTS: camera-facing sprites.
    // =====================================================================
    mod.draw.DrawLinePoint = mod.std.set_type_default() do #(DrawLinePoint::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        color_format: @Bgra8Unorm
        u_c0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        u_c1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        u_c2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        u_c3: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        u_vp: uniform(vec4(1920.0, 1080.0, 1.0, 1.0))
        // world sizes, additive, shape, min px
        u_mode: uniform(vec4(0.0, 1.0, 1.0, 1.0))
        // fade near, far, active, intensity
        u_fade: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        u_birth: uniform(vec4(1.0e30, 0.0, 0.0, 0.0))
        v_uv: varying(vec2f)
        v_color: varying(vec4f)

        vertex: fn() {
            let corner = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            let p = self.p_pos.xyz
            let c = self.u_c0 * p.x + self.u_c1 * p.y + self.u_c2 * p.z + self.u_c3
            let half_px = self.u_vp.xy * 0.5
            var d = self.p_pos.w * self.u_vp.z
            if self.u_mode.x > 0.5 {
                d = self.p_pos.w * self.u_vp.w * half_px.y / max(c.w, 0.0001)
            }
            let floor = max(self.u_mode.w, 0.001)
            let drawn = max(d, floor)
            let ink = min(d / floor, 1.0)
            let r = drawn * 0.5 + 1.0
            let rot = self.p_misc.x
            let cr = cos(rot)
            let sr = sin(rot)
            let q = vec2(corner.x * cr - corner.y * sr, corner.x * sr + corner.y * cr) * r
            var a = self.p_color.w * ink * ink
            if self.u_fade.z > 0.5 {
                a = a * (1.0 - smoothstep(self.u_fade.x, self.u_fade.y, c.w))
            }
            let age = self.u_birth.x - self.p_misc.y
            if age < 0.0 {
                a = 0.0
            }
            if self.u_birth.y > 0.0 {
                a = a * clamp(age / self.u_birth.y, 0.0, 1.0)
            }
            if self.u_birth.z > 0.0 {
                a = a * (1.0 - smoothstep(self.u_birth.z * 0.5, self.u_birth.z, age))
            }
            if c.w < 0.0001 {
                self.vertex_pos = vec4(0.0, 0.0, 2.0, 1.0)
            } else {
                self.vertex_pos = vec4(c.xy / c.w + q / half_px, c.z / c.w, 1.0)
            }
            // uv in units of the drawn radius (1 = rim), with the AA margin.
            self.v_uv = corner * (r / (drawn * 0.5))
            self.v_color = vec4(self.p_color.xyz, a)
        }

        pixel: fn() -> vec4 {
            let r = length(self.v_uv)
            let shape = self.u_mode.z
            var a = 0.0
            if shape < 0.5 {
                a = 1.0 - smoothstep(0.85, 1.0, r)
            } else {
                if shape < 1.5 {
                    a = pow(max(1.0 - r, 0.0), 2.0)
                } else {
                    if shape < 2.5 {
                        a = step(max(abs(self.v_uv.x), abs(self.v_uv.y)), 1.0)
                    } else {
                        if shape < 3.5 {
                            let s = abs(self.v_uv.x * self.v_uv.y)
                            a = max(pow(max(1.0 - r, 0.0), 3.0), clamp(0.08 - s, 0.0, 0.08) * 12.0 * max(1.0 - r, 0.0))
                        } else {
                            a = smoothstep(0.6, 0.75, r) * (1.0 - smoothstep(0.9, 1.0, r))
                        }
                    }
                }
            }
            a = a * self.v_color.w
            if a < 0.0005 {
                discard()
            }
            let rgb = self.v_color.xyz * (a * self.u_fade.w)
            if self.u_mode.y > 0.5 {
                return vec4(rgb, 0.0)
            }
            return vec4(rgb, a)
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }

    // Max-blended variants: overlapping lines and sprites keep the
    // brightest value per channel instead of summing (the platform's
    // `blend_op: @Max` pipelines).
    mod.draw.DrawLineSegmentMax = mod.std.set_type_default() do #(DrawLineSegmentMax::script_shader(vm)){
        ..mod.draw.DrawLineSegment
        blend_op: @Max
    }
    mod.draw.DrawLinePointMax = mod.std.set_type_default() do #(DrawLinePointMax::script_shader(vm)){
        ..mod.draw.DrawLinePoint
        blend_op: @Max
    }
}

/// One segment instance (see [`SEGMENT_FLOATS`]).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLineSegment {
    #[deref]
    pub draw_vars: DrawVars,
    /// a, width a
    #[live]
    pub s_a: Vec4f,
    /// b, width b
    #[live]
    pub s_b: Vec4f,
    #[live]
    pub s_ca: Vec4f,
    #[live]
    pub s_cb: Vec4f,
    /// dist a, dist b, birth a, birth b
    #[live]
    pub s_m: Vec4f,
}

/// One point instance (see [`POINT_FLOATS`]).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLinePoint {
    #[deref]
    pub draw_vars: DrawVars,
    /// position, size
    #[live]
    pub p_pos: Vec4f,
    #[live]
    pub p_color: Vec4f,
    /// rotation, birth, 0, 0
    #[live]
    pub p_misc: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLineSegmentMax {
    #[deref]
    pub draw_super: DrawLineSegment,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLinePointMax {
    #[deref]
    pub draw_super: DrawLinePoint,
}

/// What batches are drawn through: the camera and the target's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineView {
    /// World to clip (`proj * view`); for [`Space::Screen`] batches the
    /// logical-pixel ortho of [`LineView::screen`] is used instead.
    pub view_proj: Mat4f,
    /// The target in physical pixels.
    pub viewport: (f32, f32),
    /// Physical pixels per logical pixel (2 for a 1080p layout at 4K).
    pub px_scale: f32,
}

impl LineView {
    /// A 3D view: `view` and `proj` as the renderer's camera has them.
    pub fn world(view: &Mat4f, proj: &Mat4f, viewport: (f32, f32), px_scale: f32) -> Self {
        Self { view_proj: Mat4f::mul(proj, view), viewport, px_scale }
    }

    /// A 2D view of `viewport` physical pixels at `px_scale`: positions in
    /// logical pixels, top left origin.
    pub fn screen(viewport: (f32, f32), px_scale: f32) -> Self {
        Self { view_proj: screen_ortho(viewport, px_scale), viewport, px_scale }
    }

    /// Logical size of the target.
    pub fn logical(&self) -> (f32, f32) {
        (self.viewport.0 / self.px_scale, self.viewport.1 / self.px_scale)
    }

    fn matrix(&self, space: Space) -> Mat4f {
        match space {
            Space::World => self.view_proj,
            Space::Screen => screen_ortho(self.viewport, self.px_scale),
        }
    }
}

fn screen_ortho(viewport: (f32, f32), px_scale: f32) -> Mat4f {
    let (w, h) = (viewport.0 / px_scale.max(1e-6), viewport.1 / px_scale.max(1e-6));
    Mat4f::ortho(0.0, w, 0.0, h, 100.0, -100.0, 1.0, 1.0)
}

fn set_camera(dv: &mut DrawVars, cx: &Cx, m: &Mat4f, view: &LineView) {
    for (k, id) in [live_id!(u_c0), live_id!(u_c1), live_id!(u_c2), live_id!(u_c3)].into_iter().enumerate() {
        dv.set_uniform(cx, id, &m.v[4 * k..4 * k + 4]);
    }
    dv.set_uniform(cx, live_id!(u_vp), &[view.viewport.0, view.viewport.1, view.px_scale, m.v[5]]);
}

fn flag(b: bool) -> f32 {
    if b {
        1.0
    } else {
        0.0
    }
}

/// Draws line and point batches into the current pass.
pub struct LineRenderer {
    seg: Box<DrawLineSegment>,
    pts: Box<DrawLinePoint>,
    seg_max: Box<DrawLineSegmentMax>,
    pts_max: Box<DrawLinePointMax>,
    /// Draw calls issued since the last [`LineRenderer::take_calls`].
    calls: usize,
}

impl LineRenderer {
    /// None until the VM is free (try again next frame).
    pub fn new(cx: &mut Cx) -> Option<Self> {
        cx.try_with_vm(|vm| Self {
            seg: Box::new(DrawLineSegment::script_new_with_default(vm)),
            pts: Box::new(DrawLinePoint::script_new_with_default(vm)),
            seg_max: Box::new(DrawLineSegmentMax::script_new_with_default(vm)),
            pts_max: Box::new(DrawLinePointMax::script_new_with_default(vm)),
            calls: 0,
        })
    }

    /// Whether both shaders can draw now (`float16`: into an RGBA16F
    /// target; asking starts that pipeline's build). A frame drawn before
    /// they are ready is not whole.
    pub fn ready(&self, cx: &Cx, float16: bool) -> bool {
        [self.seg.draw_vars.draw_shader_id, self.pts.draw_vars.draw_shader_id, self.seg_max.draw_vars.draw_shader_id, self.pts_max.draw_vars.draw_shader_id].into_iter().all(|id| id.is_some_and(|id| cx.draw_shader_ready(id, float16)))
    }

    pub fn take_calls(&mut self) -> usize {
        std::mem::take(&mut self.calls)
    }

    /// Record `batch` into the current draw list.
    pub fn draw_lines(&mut self, cx: &mut Cx2d, view: &LineView, batch: &LineBatch) {
        if batch.is_empty() {
            return;
        }
        let s = &batch.style;
        let dv = if s.blend == Blend::Max { &mut self.seg_max.draw_vars } else { &mut self.seg.draw_vars };
        set_camera(dv, cx.cx, &view.matrix(s.space), view);
        let world = s.width_unit == WidthUnit::World && s.space == Space::World;
        dv.set_uniform(cx.cx, live_id!(u_mode), &[flag(world), flag(s.blend == Blend::Add), flag(s.round_caps), s.min_px]);
        dv.set_uniform(cx.cx, live_id!(u_dash), &[s.dash.max(0.0), s.gap.max(0.0), s.dash_offset, s.intensity]);
        let end = if s.trim_end < 0.0 { 1.0e30 } else { s.trim_end };
        dv.set_uniform(cx.cx, live_id!(u_trim), &[s.trim_start, end, s.tail.max(0.0), 0.0]);
        let fade_on = s.space == Space::World && s.fade.1 > s.fade.0;
        dv.set_uniform(cx.cx, live_id!(u_fade), &[s.fade.0, s.fade.1, flag(fade_on), 0.0]);
        dv.set_uniform(cx.cx, live_id!(u_birth), &[s.time, s.birth_fade.max(0.0), s.life.max(0.0), 0.0]);
        dv.options.depth_write = false;
        if let Some(mut mi) = cx.begin_many_instances(dv) {
            mi.instances.extend_from_slice(&batch.data);
            cx.end_many_instances(mi);
            self.calls += 1;
        }
    }

    /// Record `batch` into the current draw list.
    pub fn draw_points(&mut self, cx: &mut Cx2d, view: &LineView, batch: &PointBatch) {
        if batch.is_empty() {
            return;
        }
        let s = &batch.style;
        let dv = if s.blend == Blend::Max { &mut self.pts_max.draw_vars } else { &mut self.pts.draw_vars };
        set_camera(dv, cx.cx, &view.matrix(s.space), view);
        let world = s.size_unit == WidthUnit::World && s.space == Space::World;
        dv.set_uniform(cx.cx, live_id!(u_mode), &[flag(world), flag(s.blend == Blend::Add), s.shape.code(), s.min_px]);
        let fade_on = s.space == Space::World && s.fade.1 > s.fade.0;
        dv.set_uniform(cx.cx, live_id!(u_fade), &[s.fade.0, s.fade.1, flag(fade_on), s.intensity]);
        dv.set_uniform(cx.cx, live_id!(u_birth), &[s.time, s.birth_fade.max(0.0), s.life.max(0.0), 0.0]);
        dv.options.depth_write = false;
        if let Some(mut mi) = cx.begin_many_instances(dv) {
            mi.instances.extend_from_slice(&batch.data);
            cx.end_many_instances(mi);
            self.calls += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_view_is_proj_times_view() {
        let view = Mat4f::look_at(vec3(1.0, 2.0, 5.0), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));
        let proj = Mat4f::perspective(40.0, 16.0 / 9.0, 0.1, 100.0);
        let v = LineView::world(&view, &proj, (1920.0, 1080.0), 1.0);
        let p = vec4(0.3, -0.2, 0.7, 1.0);
        let a = v.view_proj.transform_vec4(p);
        let b = proj.transform_vec4(view.transform_vec4(p));
        assert!((a.x - b.x).abs() < 1e-5 && (a.w - b.w).abs() < 1e-5);
    }

    #[test]
    fn screen_view_maps_logical_pixels_to_the_corners() {
        let v = LineView::screen((3840.0, 2160.0), 2.0);
        let tl = v.view_proj.transform_vec4(vec4(0.0, 0.0, 0.0, 1.0));
        let br = v.view_proj.transform_vec4(vec4(1920.0, 1080.0, 0.0, 1.0));
        assert!((tl.x + 1.0).abs() < 1e-5 && (br.x - 1.0).abs() < 1e-5);
        assert!((tl.y - br.y).abs() > 1.9, "y spans the target");
    }
}
