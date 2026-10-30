//! The splat look: soft discs scattered over an object's surface, each with
//! the size, opacity, fluff and jitter of its material slot (the material
//! table's splat rows). Drawn after the shaded surface, depth-tested against
//! it, one instanced quad per splat.
//!
//! The splats are made once per volume build on the CPU: per surface brick a
//! fixed number of hashed points, each pulled onto the zero set by Newton
//! steps over the brick's own program (the twin, the same field the GPU
//! baked). They are handed to the draw as retained instances: no per-frame
//! copy or upload.

use crate::twin;
use crate::volume::{Brick, Volume, BS};
use makepad_draw::*;
use makepad_draw::makepad_platform::retained_instances::RetainedInstances;
use std::sync::Arc;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.SdfSplat = mod.std.set_type_default() do #(DrawSdfSplat::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)
        alpha_blend: false
        depth_write: true
        mat_tex: texture_2d(float)
        override_tex: texture_2d(float)
        world_from_obj: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        // x = the default splat radius (object units: the cell), y = 1 with
        // the display transform, z = world units per object unit
        splat_ctl: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        sun_dir: uniform(vec3(0.35, 0.8, 0.45))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        sun_ground: uniform(vec3(0.28, 0.28, 0.28))
        world_up: uniform(vec3(0.0, 1.0, 0.0))
        v_uv: varying(vec2f)
        v_col: varying(vec3f)
        v_alpha: varying(float)
        v_seed: varying(float)

        hash: fn(x: float) -> float {
            return fract(sin(x * 12.9898 + 4.1414) * 43758.5453)
        }

        mat_row: fn(slot: float, r: float) -> vec4 {
            return self.mat_tex.sample_nearest(vec2((slot + 0.5) / 256.0, (r + 0.5) / 5.0))
        }

        vertex: fn() {
            let slot = self.pos_slot.w
            let seed = self.nrm_seed.w
            let m0 = self.mat_row(slot, 0.0)
            // row 3: size (object units, 0 = default), opacity, fluff, size jitter
            let m3 = self.mat_row(slot, 3.0)
            // row 4: position, rotation, brightness, colour jitter
            let m4 = self.mat_row(slot, 4.0)
            let ov = self.override_tex.sample_nearest(vec2((slot + 0.5) / 256.0, 0.5))
            var albedo = mix(m0.xyz, ov.xyz, ov.w)
            if self.splat_ctl.y > 0.5 {
                albedo = pow(albedo, vec3(2.2, 2.2, 2.2))
            }
            var size = m3.x
            if size <= 0.0 { size = self.splat_ctl.x }
            size = size * (1.0 + m3.w * (self.hash(seed + 1.0) - 0.5))
            var opacity = m3.y
            if opacity <= 0.0 { opacity = 1.0 }
            let n = self.nrm_seed.xyz
            // fluff lifts the splat off the surface, jitter moves it along it
            let jp = vec3(self.hash(seed + 2.0) - 0.5, self.hash(seed + 3.0) - 0.5, self.hash(seed + 4.0) - 0.5) * (m4.x * size)
            let p = self.pos_slot.xyz + n * (m3.z * size * self.hash(seed + 5.0)) + jp
            let w = self.world_from_obj * vec4(p, 1.0)
            let nw = normalize((self.world_from_obj * vec4(n, 0.0)).xyz)
            var vp = self.draw_pass.camera_view * vec4(w.xyz / w.w, 1.0)
            // a billboard, turned by the rotation jitter
            let a = m4.y * 6.2831853 * self.hash(seed + 6.0)
            let c = cos(a)
            let s = sin(a)
            let q = self.geom.pos * 2.0 - vec2(1.0, 1.0)
            let r = vec2(q.x * c - q.y * s, q.x * s + q.y * c) * (size * self.splat_ctl.z)
            vp = vec4(vp.x + r.x, vp.y + r.y, vp.z, vp.w)
            self.vertex_pos = self.draw_pass.camera_projection * vp
            self.v_uv = q
            // lit per splat: the hemisphere and the sun, the brightness and
            // colour jitter on top
            let up = normalize(self.world_up)
            let amb = mix(self.sun_ground, self.sun_sky, clamp(dot(nw, up) * 0.5 + 0.5, 0.0, 1.0))
            // as the shade lights the surface below: a metal keeps only the
            // hemisphere's reflection
            let metal = self.mat_row(slot, 1.0).x
            let lit = (amb + self.sun_color * max(dot(nw, normalize(self.sun_dir)), 0.0)) * (1.0 - metal) + amb * metal
            let bright = 1.0 + m4.z * (self.hash(seed + 7.0) - 0.5)
            let tint = vec3(self.hash(seed + 8.0), self.hash(seed + 9.0), self.hash(seed + 10.0)) - vec3(0.5, 0.5, 0.5)
            var col = albedo * lit * bright + tint * (m4.w * 0.3)
            col = max(col, vec3(0.0, 0.0, 0.0))
            if self.splat_ctl.y > 0.5 {
                let mapped = clamp((col * (col * 2.51 + vec3(0.03, 0.03, 0.03))) / (col * (col * 2.43 + vec3(0.59, 0.59, 0.59)) + vec3(0.14, 0.14, 0.14)), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
                col = pow(mapped, vec3(0.4545454, 0.4545454, 0.4545454))
            }
            self.v_col = col
            self.v_alpha = opacity
            self.v_seed = seed
        }

        fragment: fn() {
            let d2 = dot(self.v_uv, self.v_uv)
            if d2 > 1.0 { discard() }
            // soft edge without sorting: a screen-door dither against the
            // falloff, so depth stays exact
            let a = self.v_alpha * (1.0 - d2)
            let dither = self.hash(dot(self.v_uv, vec2(127.1, 311.7)) + self.v_seed)
            if a < dither { discard() }
            self.fb0 = vec4(self.v_col, 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfSplat {
    #[deref]
    pub draw_vars: DrawVars,
    /// object-frame position, material slot
    #[live]
    pub pos_slot: Vec4f,
    /// object-frame normal, a random seed
    #[live]
    pub nrm_seed: Vec4f,
}

/// Floats per splat instance (`pos_slot`, `nrm_seed`).
pub const SPLAT_SLOTS: usize = 8;

fn hash(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// The splats of a built volume: `per_brick` hashed points per surface
/// brick, pulled onto the surface (the twin's field) and kept where they
/// land within a quarter cell of it inside the brick. Instance data for
/// [`DrawSdfSplat`], `SPLAT_SLOTS` floats each.
pub fn generate(vol: &Volume, per_brick: usize) -> Vec<f32> {
    let c = vol.cell;
    let h = c * 0.25;
    let mut out = Vec::new();
    for (i, b) in vol.bricks.iter().enumerate() {
        let Brick::Baked { program, .. } = *b else { continue };
        let prog = &vol.programs[program as usize].texels;
        let bc = vol.brick_of(i);
        let lo = bc.map(|v| (v * BS) as f32 * c);
        let span = BS as f32 * c;
        for k in 0..per_brick {
            let s = hash(((i as u64) << 20) ^ k as u64 ^ 0x5eed);
            let mut q = [lo[0] + unit(s) * span, lo[1] + unit(hash(s)) * span, lo[2] + unit(hash(s ^ 0xa5a5)) * span];
            let mut done = None;
            for _ in 0..6 {
                let (d, slot) = twin::eval_with(prog, q, &vol.kernels);
                let g = [
                    twin::eval_with(prog, [q[0] + h, q[1], q[2]], &vol.kernels).0 - twin::eval_with(prog, [q[0] - h, q[1], q[2]], &vol.kernels).0,
                    twin::eval_with(prog, [q[0], q[1] + h, q[2]], &vol.kernels).0 - twin::eval_with(prog, [q[0], q[1] - h, q[2]], &vol.kernels).0,
                    twin::eval_with(prog, [q[0], q[1], q[2] + h], &vol.kernels).0 - twin::eval_with(prog, [q[0], q[1], q[2] - h], &vol.kernels).0,
                ];
                let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                if !(l > 1e-12) || !d.is_finite() {
                    break;
                }
                let n = g.map(|x| x / l);
                if d.abs() < c * 0.02 {
                    done = Some((n, slot));
                    break;
                }
                q = [q[0] - n[0] * d, q[1] - n[1] * d, q[2] - n[2] * d];
            }
            let Some((n, slot)) = done else { continue };
            if (0..3).any(|a| q[a] < lo[a] || q[a] > lo[a] + span) {
                continue;
            }
            out.extend_from_slice(&[q[0], q[1], q[2], slot as f32, n[0], n[1], n[2], unit(hash(s ^ 0x1234)) * 1000.0]);
        }
    }
    out
}

/// The retained instances of `generate`'s output (built off the UI thread).
pub fn instances(data: Vec<f32>) -> Option<RetainedInstances> {
    RetainedInstances::new(SPLAT_SLOTS, Arc::from(data)).ok()
}

/// Draws an object's splats into the current pass (its camera; a depth
/// target the surface was drawn into).
#[allow(clippy::too_many_arguments)]
pub fn draw_splats(cx: &mut Cx2d, draw: &mut DrawSdfSplat, splats: &RetainedInstances, world_from_obj: Mat4f, vol: &Volume, materials: &Texture, slot_override: &Texture, light: &crate::march::MarchLight) {
    let c = &*cx.cx;
    let dv = &mut draw.draw_vars;
    dv.set_texture(0, materials);
    dv.set_texture(1, slot_override);
    dv.set_uniform(c, id!(world_from_obj), &world_from_obj.v);
    let scale = (world_from_obj.v[0] * world_from_obj.v[0] + world_from_obj.v[1] * world_from_obj.v[1] + world_from_obj.v[2] * world_from_obj.v[2]).sqrt();
    // the default radius covers the spacing of `per_brick` splats on a brick's face
    dv.set_uniform(c, id!(splat_ctl), &[vol.cell * 2.5, if light.display { 1.0 } else { 0.0 }, scale, 0.0]);
    dv.set_uniform(c, id!(sun_dir), &light.dir);
    dv.set_uniform(c, id!(sun_color), &light.color);
    dv.set_uniform(c, id!(sun_sky), &light.sky);
    dv.set_uniform(c, id!(sun_ground), &light.ground);
    dv.set_uniform(c, id!(world_up), &light.up);
    cx.add_retained_instances(&draw.draw_vars, splats);
}
