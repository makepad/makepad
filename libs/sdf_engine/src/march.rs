//! The march: one draw per object over its screen rect, sphere tracing the
//! object's bricks in its own frame.
//!
//! Per step, the brick index says whether the brick holds surface:
//! - an empty brick (no sample within the band, so no surface in it) is
//!   left at once: the ray jumps to the brick's exit (or its skip distance,
//!   when that is farther);
//! - a baked brick gives the distance from the render copy (two bilinear
//!   R16F reads, mixed across z: hardware trilinear on a 2D atlas); in
//!   exact mode, within half the band, the brick's own culled program is
//!   evaluated instead (CAD edges stay sharp).
//!
//! The hit writes real depth (`fragment_depth(@any)`, the pass camera's
//! projection), so raster geometry and SDF objects occlude each other per
//! pixel. Shading here is a plain lit albedo from the material table; the
//! deferred G-buffer path comes with the renderer proper.
//!
//! SHADER LAW: unary minus has the lowest precedence in the script shader
//! parser; write `(0.0 - a) - b`, never `-a - b`.

use crate::bake::{Baker, COPY_W, INDEX_W, PROG_W};
use crate::volume::Volume;
use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.SdfMarch = mod.std.set_type_default() do #(DrawSdfMarch::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SdfInterp,
        depth_out: fragment_depth(@any)
        copy_tex: texture_2d(float)
        copy_slots: texture_2d(float)
        index_tex: texture_2d(float)
        mat_tex: texture_2d(float)
        // per slot: (linear rgb, a = 1 replaces the albedo), e.g. the filament a slot prints in
        override_tex: texture_2d(float)
        copy_inv: uniform(vec2(1.0, 1.0))
        index_inv: uniform(vec2(1.0, 1.0))
        vol_min: uniform(vec3(0.0, 0.0, 0.0))
        vol_dim: uniform(vec3(1.0, 1.0, 1.0))
        cell: uniform(1.0)
        band: uniform(1.0)
        view_size: uniform(vec2(1.0, 1.0))
        pix_angle: uniform(0.001)
        exact: uniform(1.0)
        // 1: fb0 = (t, slot, normal x, normal y) into an RGBA32F target (checks)
        debug_out: uniform(0.0)
        light_dir: uniform(vec3(0.4, 0.8, 0.45))

        // pixels (y down, as the turtle's) to ndc (y up)
        to_ndc: fn(px: vec2) -> vec2 {
            return vec2(px.x / self.view_size.x * 2.0 - 1.0, 1.0 - px.y / self.view_size.y * 2.0)
        }

        // the quad in clip space straight from its pixel rect (the pass has a
        // 3D camera; the ray uses the same pixel -> ndc map)
        vertex: fn() {
            self.pos = self.geom.pos
            let ndc = self.to_ndc(self.rect_pos + self.geom.pos * self.rect_size)
            self.vertex_pos = vec4(ndc.x, ndc.y, 0.0, 1.0)
        }

        // the index entry of brick b (object frame brick coords): (slot, skip, program, inside)
        brick_entry: fn(b: vec3) -> vec4 {
            let ib = b - self.vol_min
            if ib.x < 0.0 || ib.y < 0.0 || ib.z < 0.0 || ib.x >= self.vol_dim.x || ib.y >= self.vol_dim.y || ib.z >= self.vol_dim.z {
                return vec4(0.0 - 1.0, self.band, 0.0, 0.0)
            }
            let i = (ib.z * self.vol_dim.y + ib.y) * self.vol_dim.x + ib.x
            let row = floor(i * 0.0009765625)
            return vec4(self.index_tex.sample_nearest((vec2(i - row * 1024.0, row) + vec2(0.5, 0.5)) * self.index_inv).xyz, 1.0)
        }

        // the render copy's texel coordinates of local sample l (0..8) in slot s, slice z0
        copy_uv: fn(s: f32, l: vec2, z0: f32) -> vec2 {
            let row = floor(s * 0.02)
            let col = s - row * 50.0
            return (vec2(col * 81.0 + z0 * 9.0 + l.x + 0.5, row * 9.0 + l.y + 0.5)) * self.copy_inv
        }

        // the copy's distance at object point p inside baked brick b of slot s
        copy_dist: fn(p: vec3, b: vec3, s: f32) -> f32 {
            let l = p / self.cell - b * 8.0
            let z0 = min(floor(l.z), 7.0)
            let f = l.z - z0
            let d0 = self.copy_tex.sample(self.copy_uv(s, l.xy, z0)).x
            let d1 = self.copy_tex.sample(self.copy_uv(s, l.xy, z0 + 1.0)).x
            return mix(d0, d1, f)
        }

        // the field at object point p: the band (with sign) outside baked bricks
        field: fn(p: vec3) -> f32 {
            let b = floor(p / (self.cell * 8.0))
            let e = self.brick_entry(b)
            if e.x < 0.0 {
                if e.y < 0.0 { return 0.0 - self.band }
                return self.band
            }
            let d = self.copy_dist(p, b, e.x)
            if self.exact > 0.5 && e.z >= 0.0 && abs(d) < self.band * 0.9 {
                return self.eval_program(e.z, p.x, p.y, p.z).x
            }
            return d
        }

        fragment: fn() {
            let pix = self.rect_pos + self.pos * self.rect_size
            let ndc = self.to_ndc(pix)
            let a = self.inv_view_proj * vec4(ndc.x, ndc.y, 0.0, 1.0)
            let bw = self.inv_view_proj * vec4(ndc.x, ndc.y, 1.0, 1.0)
            let ro_w = a.xyz / a.w
            let rd_w = normalize(bw.xyz / bw.w - ro_w)
            let ro = (self.obj_from_world * vec4(ro_w, 1.0)).xyz
            let rd = (self.obj_from_world * vec4(rd_w, 0.0)).xyz
            // the volume box
            let bs = self.cell * 8.0
            let lo = self.vol_min * bs
            let hi = (self.vol_min + self.vol_dim) * bs
            let inv = vec3(1.0, 1.0, 1.0) / rd
            let ta = (lo - ro) * inv
            let tb = (hi - ro) * inv
            let tn = min(ta, tb)
            let tf = max(ta, tb)
            let t0 = max(max(tn.x, tn.y), max(tn.z, 0.0))
            let t1 = min(min(tf.x, tf.y), tf.z)
            if t1 <= t0 { discard() }
            var t = t0 + self.cell * 0.01
            var hit = 0.0
            var slot = 0.0
            for i in 0..256 {
                if t > t1 { break }
                let p = ro + rd * t
                let b = floor(p / bs)
                let e = self.brick_entry(b)
                if e.x < 0.0 {
                    // an empty brick: no surface in it, leave it
                    let bmin = b * bs
                    let bmax = bmin + vec3(bs, bs, bs)
                    let te = (mix(bmin, bmax, step(vec3(0.0, 0.0, 0.0), rd)) - ro) * inv
                    let t_exit = min(min(te.x, te.y), te.z)
                    t = max(t_exit, t + abs(e.y)) + self.cell * 0.01
                } else {
                    // trilinear can overestimate near edges and corners: 0.9 of it
                    var d = self.copy_dist(p, b, e.x) * 0.9
                    if self.exact > 0.5 && e.z >= 0.0 && abs(d) < self.band * 0.9 {
                        let ev = self.eval_program(e.z, p.x, p.y, p.z)
                        d = ev.x
                        slot = ev.y
                    } else {
                        let l = floor(p / self.cell - b * 8.0 + vec3(0.5, 0.5, 0.5))
                        slot = floor(self.copy_slots.sample_nearest(self.copy_uv(e.x, l.xy, min(l.z, 8.0))).x * 255.0 + 0.5)
                    }
                    let eps = max(self.cell * 0.02, t * self.pix_angle)
                    if d < eps {
                        hit = 1.0
                        break
                    }
                    t = t + max(d, self.cell * 0.05)
                }
            }
            if hit < 0.5 { discard() }
            let p = ro + rd * t
            // the normal: a tetrahedral gradient of the field
            let h = self.cell * 0.5
            let k0 = vec3(1.0, 0.0 - 1.0, 0.0 - 1.0)
            let k1 = vec3(0.0 - 1.0, 0.0 - 1.0, 1.0)
            let k2 = vec3(0.0 - 1.0, 1.0, 0.0 - 1.0)
            let k3 = vec3(1.0, 1.0, 1.0)
            let g = k0 * self.field(p + k0 * h) + k1 * self.field(p + k1 * h) + k2 * self.field(p + k2 * h) + k3 * self.field(p + k3 * h)
            let n = normalize((self.world_from_obj * vec4(g, 0.0)).xyz)
            let su = vec2((slot + 0.5) / 256.0, 0.5)
            let ov = self.override_tex.sample_nearest(su)
            let albedo = mix(self.mat_tex.sample_nearest(su).xyz, ov.xyz, ov.w)
            let lit = 0.25 + 0.75 * max(dot(n, normalize(self.light_dir)), 0.0)
            let hit_w = self.world_from_obj * vec4(p, 1.0)
            self.depth_out = self.depth_of(vec4(hit_w.xyz / hit_w.w, 1.0))
            if self.debug_out > 0.5 {
                self.fb0 = vec4(t, slot, n.x, n.y)
            } else {
                self.fb0 = vec4(albedo * lit, 1.0)
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfMarch {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub inv_view_proj: Mat4f,
    #[live]
    pub obj_from_world: Mat4f,
    #[live]
    pub world_from_obj: Mat4f,
}

/// The view of one frame: the pass camera (view, projection) and its size.
#[derive(Clone, Copy, Debug)]
pub struct MarchView {
    pub view: Mat4f,
    pub proj: Mat4f,
    pub size: DVec2,
    /// Vertical field of view (radians), for the cone stopping test.
    pub fov_y: f32,
}

/// Draws one object (`world_from_obj` places its frame) over `rect` (its
/// screen rect, or the whole view) into the current pass, which must use the
/// same camera and have a depth target.
#[allow(clippy::too_many_arguments)]
pub fn draw_object(cx: &mut Cx2d, draw: &mut DrawSdfMarch, rect: Rect, view: &MarchView, world_from_obj: Mat4f, vol: &Volume, baker: &Baker, materials: &Texture, slot_override: &Texture, exact: bool) {
    let c = &*cx.cx;
    let vp = Mat4f::mul(&view.proj, &view.view);
    let dv = &mut draw.draw_super.draw_vars;
    dv.set_texture(0, baker.prog_texture());
    dv.set_texture(1, &baker.copy);
    dv.set_texture(2, &baker.copy_slots);
    dv.set_texture(3, baker.index_texture());
    dv.set_texture(4, materials);
    dv.set_texture(5, slot_override);
    dv.set_uniform(c, id!(prog_inv), &[1.0 / PROG_W as f32, 1.0 / baker.prog_rows() as f32]);
    dv.set_uniform(c, id!(copy_inv), &[1.0 / COPY_W as f32, 1.0 / baker.copy_rows as f32]);
    dv.set_uniform(c, id!(index_inv), &[1.0 / INDEX_W as f32, 1.0 / baker.index_rows() as f32]);
    dv.set_uniform(c, id!(vol_min), &vol.min.map(|v| v as f32));
    dv.set_uniform(c, id!(vol_dim), &vol.dim.map(|v| v as f32));
    dv.set_uniform(c, id!(cell), &[vol.cell]);
    dv.set_uniform(c, id!(band), &[vol.band]);
    dv.set_uniform(c, id!(view_size), &[view.size.x as f32, view.size.y as f32]);
    dv.set_uniform(c, id!(pix_angle), &[2.0 * (view.fov_y * 0.5).tan() / view.size.y as f32]);
    dv.set_uniform(c, id!(exact), &[if exact { 1.0 } else { 0.0 }]);
    draw.inv_view_proj = vp.invert();
    draw.obj_from_world = world_from_obj.invert();
    draw.world_from_obj = world_from_obj;
    draw.draw_super.draw_abs(cx, rect);
}

/// A slot override table (256 x 1, `(rgb, a)`; a = 1 replaces the slot's
/// albedo): all zero by default.
pub fn override_texture(cx: &mut Cx, overrides: &[(u8, [f32; 3])]) -> Texture {
    let mut data = vec![0.0f32; 256 * 4];
    for (slot, rgb) in overrides {
        let i = *slot as usize * 4;
        data[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 1.0]);
    }
    Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: 256, height: 1, data: Some(data), updated: TextureUpdated::Full })
}

/// The material table the march reads: albedo per slot (256 x 1).
pub fn material_texture(cx: &mut Cx, materials: &[makepad_csg_sdf::SdfMaterial]) -> Texture {
    let mut data = vec![0.8f32; 256 * 4];
    for (i, m) in materials.iter().enumerate().take(256) {
        data[i * 4..i * 4 + 4].copy_from_slice(&[m.albedo[0], m.albedo[1], m.albedo[2], m.rough]);
    }
    Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: 256, height: 1, data: Some(data), updated: TextureUpdated::Full })
}
