//! The GPU renderer: shaders + the pass chain that runs them every frame.
//!
//! One frame of progressive rendering, in submission order (every stage is a
//! child of the next so the platform renders them in this order):
//!
//! ```text
//! gbuf    rasterize the scene (jittered projection) → (tri, u, v, depth)
//! trace   one low-discrepancy sample per pixel in the active row set,
//!         starting from the G-buffer hit (pinhole) or a lens-sampled
//!         primary ray (bokeh); accumulates linear sum + count (ping-pong)
//! gather  grouped scenes fold this dispatch's far entries into the tile atlas
//! resolve converts a transient invalid-sample sentinel back to clean sum/N
//! reject  increments a persistent per-pixel NaN/Inf/traversal diagnostic
//! moments running mean/mean² of luminance for variance (ping-pong)
//! guide   (denoise) G-buffer + straight dielectric continuation → (normal, depth)
//! atrous  (denoise) 4 edge-avoiding wavelet passes guided by normal,
//!         depth and the accumulated variance
//! far     sum the reference tiles at the current RGB gains
//! carry   retain traced base light between resolution rungs
//! tonemap add guided far light, then exposure + ACES + sRGB → BGRA8 view
//! window  the widget draws the view target
//! ```
//!
//! Between scene and group-gain uploads the CPU sends draw uniforms and
//! tile rectangles. Rendering needs no image readback.
//!
//! SHADER LAW (bit us for a day): the script shader parser gives unary
//! minus the LOWEST precedence — `-a - b` emits `-(a - b)`. Never write a
//! unary minus followed by more terms; write `(0.0 - a) - b`. A bare `-x`
//! as a whole expression or argument is fine.

use crate::env_map::{EnvironmentFrame, EnvironmentLight};
use crate::pack::{PackedScene, DATA_SHIFT, DATA_W};
use crate::scene::SceneInput;
use crate::sky::{sky_uniforms, SkyUniforms};
use makepad_draw::*;
use std::collections::VecDeque;
use std::sync::Arc;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.PtGbuf = mod.std.set_type_default() do #(DrawGbuf::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.DepthMeshVertex, geom.DepthMeshGeom)
        color_format: @Rgba32F
        alpha_blend: false
        backface_culling: false
        v_bary: varying(vec4f)
        v_depth: varying(1.0)
        vertex: fn() {
            let view_p = self.view_proj * vec4(self.geom.pos.x, self.geom.pos.y, self.geom.pos.z, 1.0)
            let bias = min(max(self.geom.pos.w, 0.0) * 0.0000001, 0.00005)
            let p = vec4(view_p.x, view_p.y, view_p.z - bias * view_p.w, view_p.w)
            self.v_bary = self.geom.barycentric
            self.v_depth = p.w
            self.vertex_pos = p
        }
        fragment: fn() {
            self.fb0 = vec4(floor(self.v_bary.w + 0.5), self.v_bary.y, self.v_bary.z, self.v_depth)
        }
    }

    mod.draw.PtLightSample = struct {
        base: vec3
        far: vec4

        new: fn(base: vec3) -> Self {
            return self(base: base far: vec4(0.0, 0.0, 0.0, 0.0))
        }
    }
    let PtLightSample = mod.draw.PtLightSample

    mod.draw.PtTrace = mod.std.set_type_default() do #(DrawTrace::script_shader(vm)){
        ..mod.draw.DrawQuad
        draw_call_always: true
        // The float targets hold data (alpha is the sample count): a fragment replaces the texel
        // under it on every backend. Blending, which some backends apply to float targets, would
        // fold each tile into the carried copy beneath it.
        color_format: @Rgba32F
        alpha_blend: false
        tri_tex: texture_2d(float)
        attr_tex: texture_2d(float)
        bvh_tex: texture_2d(float)
        mat_tex: texture_2d(float)
        light_tex: texture_2d(float)
        atlas_tex: texture_2d(float)
        accum_tex: texture_2d(float)
        gbuf_tex: texture_2d(float)
        moment_tex: texture_2d(float)
        rejected_tex: texture_2d(float)
        env_tex: texture_2d(float)

        res: uniform(vec2f)
        inv_res: uniform(vec2f)
        // Ray-generation pixel grid of the CURRENT resolution rung: the whole
        // camera frame maps across `1/cam_inv` pixels. Texture addressing
        // stays `inv_res` (the buffers are native-sized; a coarse rung lives
        // in their top-left corner).
        cam_inv: uniform(vec2f)
        tri_inv: uniform(vec2f)
        attr_inv: uniform(vec2f)
        bvh_inv: uniform(vec2f)
        mat_inv: uniform(vec2f)
        light_inv: uniform(vec2f)
        jitter: uniform(vec2f)
        seed: uniform(1.0)
        tile: uniform(vec4f)
        max_steps: uniform(2048.0)
        spp: uniform(1.0)
        use_gbuffer: uniform(1.0)
        n_lights: uniform(0.0)
        inv_light_power: uniform(0.0)
        env_grid_dim: uniform(0.0)
        env_min: uniform(vec3f)
        env_inv_extent: uniform(vec3f)
        env_on: uniform(0.0)
        env_inv: uniform(vec2f)
        env_size: uniform(vec2f)
        env_row0: uniform(vec4f)
        env_row1: uniform(vec4f)
        env_row2: uniform(vec4f)
        env_strength: uniform(1.0)
        n_nodes: uniform(0.0)
        max_bounces: uniform(8.0)
        max_diffuse: uniform(4.0)
        preview_clamp: uniform(0.0)
        shadow_skin: uniform(0.0)
        // Bounds diagonal and its cached square (PackedScene, CPU twin).
        scene_scale: uniform(vec2f)
        // Texture minification: one texel of the atlas, and the world size
        // one pixel of the CURRENT rung subtends (per unit ray length
        // for perspective, scene units for ortho).
        atlas_inv: uniform(vec2f)
        pixel_world: uniform(0.0)
        adaptive_min: uniform(0.0)
        adaptive_thresh: uniform(0.0005)
        cam_pos: uniform(vec3f)
        cam_right: uniform(vec3f)
        cam_up: uniform(vec3f)
        cam_fwd: uniform(vec3f)
        cam_tan: uniform(vec2f)
        ortho: uniform(vec2f)
        reset: uniform(0.0)
        lens: uniform(vec4f)
        sun_dir: uniform(vec4f)
        sun_radiance: uniform(vec4f)
        sun_pdf: uniform(1.0)
        sun_on: uniform(0.0)
        env_sun_prob: uniform(0.0)
        pz_y: uniform(vec4f)
        pz_x: uniform(vec4f)
        pz_yc: uniform(vec4f)
        pz_e: uniform(vec4f)
        pz_f0: uniform(vec4f)
        zenith: uniform(vec4f)
        sun_model: uniform(vec4f)
        world_up: uniform(vec4f)
        star_r0: uniform(vec4f)
        star_r1: uniform(vec4f)
        star_r2: uniform(vec4f)
        sky_strength: uniform(1.0)
        uniform_sky: uniform(0.0)
        debug_mode: uniform(0.0)
        brute: uniform(0.0)
        dbg_b: uniform(-1.0)

        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        // ---- integer hashing / low-discrepancy sampling (rng.rs twin) ----
        hash: fn(x_in: u32) -> u32 {
            var x = x_in
            x = x ^ (x >> 16u)
            x = x * 2146121005u
            x = x ^ (x >> 15u)
            x = x * 2221713035u
            x = x ^ (x >> 16u)
            return x
        }
        hash2: fn(a: u32, b: u32) -> u32 {
            return self.hash(a ^ self.hash(b + 2654435769u))
        }
        reverse_bits: fn(x_in: u32) -> u32 {
            var x = x_in
            x = ((x & 1431655765u) << 1u) | ((x >> 1u) & 1431655765u)
            x = ((x & 858993459u) << 2u) | ((x >> 2u) & 858993459u)
            x = ((x & 252645135u) << 4u) | ((x >> 4u) & 252645135u)
            x = ((x & 16711935u) << 8u) | ((x >> 8u) & 16711935u)
            return (x << 16u) | (x >> 16u)
        }
        owen: fn(x_in: u32, sd: u32) -> u32 {
            var x = self.reverse_bits(x_in)
            x = x ^ (x * 1025551850u)
            x = x + sd
            x = x * ((sd >> 16u) | 1u)
            x = x ^ (x * 89287766u)
            x = x ^ (x * 1403136100u)
            return self.reverse_bits(x)
        }
        sobol1: fn(index: u32) -> u32 {
            var v = 2147483648u
            var r = 0u
            var i = index
            for k in 0..32 {
                if i == 0u { break }
                if (i & 1u) != 0u { r = r ^ v }
                i = i >> 1u
                v = v ^ (v >> 1u)
            }
            return r
        }
        sobol2: fn(index: u32, pseed: u32, pair: u32) -> vec2 {
            // Each dimension pair shuffles the index with its own seed (Burley's padding): one
            // shared shuffle makes every pair a function of the same point, so the cross-pair
            // projections collapse onto curves and part of a pixel's error stays frozen for its
            // whole run, unseen by its moments (a flat env-lit board kept 4.4x the noise at 256).
            let idx = self.owen(index, self.hash2(pseed, 1374496523u + pair))
            let sx = self.hash2(pseed, pair * 2u)
            let sy = self.hash2(pseed, pair * 2u + 1u)
            let x = self.owen(self.reverse_bits(idx), sx)
            let y = self.owen(self.sobol1(idx), sy)
            return vec2(f32(x >> 8u), f32(y >> 8u)) * 0.000000059604644775390625
        }
        rand: fn(pseed: u32, index: u32, dim: u32) -> f32 {
            return f32(self.hash2(self.hash2(pseed, index), dim + 7777u) >> 8u) * 0.000000059604644775390625
        }

        // ---- data texture fetch ----
        tx: fn(i: u32, inv: vec2) -> vec2 {
            return (vec2(f32(i & 2047u), f32(i >> 11u)) + vec2(0.5, 0.5)) * inv
        }
        fetch_tri: fn(i: u32) -> vec4 { return self.tri_tex.sample_nearest(self.tx(i, self.tri_inv)) }
        fetch_attr: fn(i: u32) -> vec4 { return self.attr_tex.sample_nearest(self.tx(i, self.attr_inv)) }
        fetch_bvh: fn(i: u32) -> vec4 { return self.bvh_tex.sample_nearest(self.tx(i, self.bvh_inv)) }
        fetch_mat: fn(i: u32) -> vec4 { return self.mat_tex.sample_nearest(self.tx(i, self.mat_inv)) }
        fetch_light: fn(i: u32) -> vec4 { return self.light_tex.sample_nearest(self.tx(i, self.light_inv)) }

        hit_tie_eps: fn(t: f32) -> f32 {
            return self.scene_scale.x * 0.00001 + abs(t) * 0.000002
        }

        // ---- BVH4 traversal with Woop watertight triangle tests ----
        trace: fn(ro: vec3, rd_in: vec3, tmax: f32, any_hit: f32, tmin: f32, skip: f32) -> vec4 {
            var safe_rd = rd_in
            if abs(safe_rd.x) < 0.000000001 { safe_rd.x = if safe_rd.x < 0.0 { -0.000000001 } else { 0.000000001 } }
            if abs(safe_rd.y) < 0.000000001 { safe_rd.y = if safe_rd.y < 0.0 { -0.000000001 } else { 0.000000001 } }
            if abs(safe_rd.z) < 0.000000001 { safe_rd.z = if safe_rd.z < 0.0 { -0.000000001 } else { 0.000000001 } }
            let inv = vec3(1.0, 1.0, 1.0) / safe_rd
            let adx = abs(rd_in.x)
            let ady = abs(rd_in.y)
            let adz = abs(rd_in.z)
            var mx = vec3(1.0, 0.0, 0.0)
            var my = vec3(0.0, 1.0, 0.0)
            var mz = vec3(0.0, 0.0, 1.0)
            if adz >= adx && adz >= ady {
                mx = vec3(1.0, 0.0, 0.0)
            } else if adx >= ady {
                mx = vec3(0.0, 1.0, 0.0)
                my = vec3(0.0, 0.0, 1.0)
                mz = vec3(1.0, 0.0, 0.0)
            } else {
                mx = vec3(0.0, 0.0, 1.0)
                my = vec3(1.0, 0.0, 0.0)
                mz = vec3(0.0, 1.0, 0.0)
            }
            let dz = dot(rd_in, mz)
            let sx = dot(rd_in, mx) / dz
            let sy = dot(rd_in, my) / dz
            let sz = 1.0 / dz
            var hit = vec4(tmax, -1.0, 0.0, 0.0)
            var hit_orig = 16777216u
            var hit_front = 0.0
            var hit_priority = 0u
            var hit_group = 0u
            var i = 0u
            let nn = u32(self.n_nodes)
            let ms = u32(self.max_steps)
            var steps = 0u
            loop {
                if i >= nn || steps >= ms { break }
                steps = steps + 1u
                let n0 = self.fetch_bvh(i * 2u)
                let n1 = self.fetch_bvh(i * 2u + 1u)
                let t0x = (n0.x - ro.x) * inv.x
                let t1x = (n1.x - ro.x) * inv.x
                let t0y = (n0.y - ro.y) * inv.y
                let t1y = (n1.y - ro.y) * inv.y
                let t0z = (n0.z - ro.z) * inv.z
                let t1z = (n1.z - ro.z) * inv.z
                let near = max(max(min(t0x, t1x), min(t0y, t1y)), max(min(t0z, t1z), tmin))
                var hit_limit = hit.x
                if any_hit < 0.5 && hit.y >= 0.0 && hit_group > 0u {
                    hit_limit = min(tmax, hit.x + self.hit_tie_eps(hit.x))
                }
                var far = min(min(max(t0x, t1x), max(t0y, t1y)), min(max(t0z, t1z), hit_limit))
                if far >= 0.0 { far = far * 1.00000072 }
                if near <= far {
                    if n0.w < 0.0 {
                        let first = u32(n1.w)
                        let count = u32(0.0 - n0.w)
                        for k in 0..8 {
                            if k >= count { break }
                            let ti = first + k
                            var do_tri = 1.0
                            if skip >= 0.0 && f32(ti) == skip { do_tri = 0.0 }
                            if do_tri > 0.5 {
                            let ta = self.fetch_tri(ti * 3u)
                            let tb = self.fetch_tri(ti * 3u + 1u)
                            let tc = self.fetch_tri(ti * 3u + 2u)
                            let va = ta.xyz - ro
                            let vb = tb.xyz - ro
                            let vc = tc.xyz - ro
                            let az = dot(va, mz)
                            let bz = dot(vb, mz)
                            let cz = dot(vc, mz)
                            let ax = dot(va, mx) - sx * az
                            let ay = dot(va, my) - sy * az
                            let bx = dot(vb, mx) - sx * bz
                            let by = dot(vb, my) - sy * bz
                            let cx = dot(vc, mx) - sx * cz
                            let cy = dot(vc, my) - sy * cz
                            let u = cx * by - cy * bx
                            let v = ax * cy - ay * cx
                            let w = bx * ay - by * ax
                            var valid = 1.0
                            if (u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0) { valid = 0.0 }
                            let det = u + v + w
                            if det == 0.0 { valid = 0.0 }
                            if valid > 0.5 {
                                let t = (u * sz * az + v * sz * bz + w * sz * cz) / det
                                let orig = u32(tb.w)
                                var cand_front = 0.0
                                if det * sz > 0.0 { cand_front = 1.0 }
                                let coplanar = self.fetch_attr(ti * 4u + 3u).w
                                let priority = u32(modf(coplanar, 4096.0))
                                let group = u32(floor(coplanar / 4096.0))
                                var take = 0.0
                                if hit.y < 0.0 {
                                    take = 1.0
                                } else {
                                    let prioritized = group > 0u && group == hit_group
                                    let tied = abs(t - hit.x) <= self.hit_tie_eps(max(t, hit.x))
                                    if prioritized && tied {
                                        // Facing outranks priority inside a
                                        // coplanar tie (CPU twin agrees):
                                        // the exposed side of a stacked
                                        // assembly is the one facing the ray.
                                        if cand_front != hit_front {
                                            take = cand_front
                                        } else if priority > hit_priority {
                                            take = 1.0
                                        } else if priority == hit_priority {
                                            if t < hit.x || (t == hit.x && orig < hit_orig) { take = 1.0 }
                                        }
                                    } else if t < hit.x || (t == hit.x && orig < hit_orig) {
                                        take = 1.0
                                    }
                                }
                                if t > tmin && t <= tmax && take > 0.5 {
                                    hit = vec4(t, f32(ti), v / det, w / det)
                                    hit_orig = orig
                                    hit_front = cand_front
                                    hit_priority = priority
                                    hit_group = group
                                    if any_hit > 0.5 { return hit }
                                }
                            }
                            }
                        }
                    }
                    i = i + 1u
                } else {
                    i = if n0.w < 0.0 { i + 1u } else { u32(n1.w) }
                }
            }
            if i < nn { return vec4(hit.x, -2.0, hit.z, hit.w) }
            return hit
        }

        // ---- the sky (sky.rs twin) ----
        sky: fn(v_in: vec3) -> vec3 {
            if self.uniform_sky > 0.0 {
                return vec3(self.uniform_sky, self.uniform_sky, self.uniform_sky)
            }
            if self.sky_strength <= 0.0 { return vec3(0.0, 0.0, 0.0) }
            let v = normalize(v_in)
            let up = self.world_up.xyz
            let altitude = dot(v, up)
            let ct = max(altitude, 0.01)
            let cg = clamp(dot(v, self.sun_model.xyz), -1.0, 1.0)
            let g = acos(cg)
            let cg2 = cg * cg
            let fy = (1.0 + self.pz_y.x * exp(self.pz_y.y / ct))
                * (1.0 + self.pz_y.z * exp(self.pz_y.w * g) + self.pz_e.x * cg2)
            let fx = (1.0 + self.pz_x.x * exp(self.pz_x.y / ct))
                * (1.0 + self.pz_x.z * exp(self.pz_x.w * g) + self.pz_e.y * cg2)
            let fc = (1.0 + self.pz_yc.x * exp(self.pz_yc.y / ct))
                * (1.0 + self.pz_yc.z * exp(self.pz_yc.w * g) + self.pz_e.z * cg2)
            let yl = self.zenith.x * fy * self.pz_f0.x
            let xc = self.zenith.y * fx * self.pz_f0.y
            let yc = max(self.zenith.z * fc * self.pz_f0.z, 0.0001)
            var yt = max(yl * self.sun_model.w, 0.0)
            yt = yt / (1.0 + yt)
            let bx = xc * (yt / yc)
            let bz = (1.0 - xc - yc) * (yt / yc)
            let r = max(3.2406 * bx - 1.5372 * yt - 0.4986 * bz, 0.0)
            let gr = max((0.0 - 0.9689) * bx + 1.8758 * yt + 0.0415 * bz, 0.0)
            let b = max(0.0557 * bx - 0.204 * yt + 1.057 * bz, 0.0)
            let m = max(max(r, gr), max(b, 1.0))
            var day = pow(vec3(r / m, gr / m, b / m), vec3(0.4545454, 0.4545454, 0.4545454))
            day = day * mix(1.0, 0.35, clamp((0.0 - altitude) * 3.0, 0.0, 1.0))

            let sun_t = self.sun_dir.xyz
            let gt = acos(clamp(dot(v, sun_t), -1.0, 1.0))
            let sun_height = dot(sun_t, up)
            let abss = exp(vec3(0.39, 0.57, 1.0)
                * ((0.0 - 0.485) / pow(max(sun_height + 0.033, 0.012), 0.75))) * 2.0
            let mie_d = clamp(1.0 - pow(gt * 0.55, 0.1), 0.0, 1.0)
            let mie = mie_d * mie_d * (3.0 - 2.0 * mie_d) * 1.4
            day = day + abss * mie
                * clamp((altitude + 0.033) * 90.0 + 0.5, 0.0, 1.0)

            let nb = self.zenith.w
            let nsky = mix(
                vec3(0.010, 0.012, 0.020),
                vec3(0.002, 0.003, 0.006),
                clamp(altitude * 1.4, 0.0, 1.0)
            )
            var result = mix(day, nsky, nb)
            let sd = vec3(
                dot(self.star_r0.xyz, v),
                dot(self.star_r1.xyz, v),
                dot(self.star_r2.xyz, v)
            )
            let su = atan2(sd.z, sd.x) * 0.15915494 + 0.5
            let sv = 0.5 - asin(clamp(sd.y, -1.0, 1.0)) * 0.31830989
            let fade = nb * clamp(altitude * 6.0 + 0.1, 0.0, 1.0)
            let suv = vec2(su * 1600.0, sv * 800.0)
            let sh = fract(sin(dot(floor(suv), vec2(127.1, 311.7))) * 43758.5453)
            let spark = step(0.995, sh)
                * pow(clamp(1.0 - length(fract(suv) - vec2(0.5, 0.5)) * 2.0, 0.0, 1.0), 3.0)
                * (0.3 + 0.7 * fract(sh * 57.31))
            let suv2 = vec2(su * 400.0, sv * 200.0)
            let sh2 = fract(sin(dot(floor(suv2), vec2(269.5, 183.3))) * 43758.5453)
            let spark2 = step(0.992, sh2)
                * pow(clamp(1.0 - length(fract(suv2) - vec2(0.5, 0.5)) * 2.4, 0.0, 1.0), 4.0)
                * (0.5 + 0.5 * fract(sh2 * 43.7))
            result = result + (vec3(0.85, 0.9, 1.0) * spark
                + vec3(1.0, 0.97, 0.9) * spark2) * fade
            return result * self.sky_strength
        }

        visible_sun: fn(v_in: vec3) -> vec3 {
            if self.env_on > 0.5 || self.uniform_sky > 0.0 || self.sky_strength <= 0.0 {
                return vec3(0.0, 0.0, 0.0)
            }
            let v = normalize(v_in)
            let altitude = dot(v, self.world_up.xyz)
            let gt = acos(clamp(dot(v, self.sun_dir.xyz), -1.0, 1.0))
            let absorption = exp(vec3(0.39, 0.57, 1.0)
                * ((0.0 - 0.485) / pow(max(altitude + 0.033, 0.02), 0.75))) * 2.0
            let limb = 1.0 - smoothstep(0.048, 0.055, gt)
            return absorption * (limb * 20.0)
                * clamp((altitude + 0.033) * 90.0 + 0.5, 0.0, 1.0)
                * self.sky_strength
        }

        // ---- BSDF: Lambert + GGX (height-correlated Smith), Schlick Fresnel ----
        // cross as written: the backend builtin may fuse its products, the CPU twin does not
        cross3: fn(a: vec3, b: vec3) -> vec3 {
            return vec3(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x)
        }
        to_world: fn(n: vec3, l: vec3) -> vec3 {
            let sgn = if n.z >= 0.0 { 1.0 } else { -1.0 }
            let a = (0.0 - 1.0) / (sgn + n.z)
            let b = n.x * n.y * a
            let t = vec3(1.0 + sgn * n.x * n.x * a, sgn * b, (0.0 - sgn) * n.x)
            let bt = vec3(b, sgn + n.y * n.y * a, 0.0 - n.y)
            return t * l.x + bt * l.y + n * l.z
        }
        sample_cos: fn(n: vec3, r: vec2) -> vec3 {
            let rr = sqrt(r.x)
            let phi = 6.28318530718 * r.y
            return self.to_world(n, vec3(rr * cos(phi), rr * sin(phi), sqrt(max(1.0 - r.x, 0.0))))
        }
        sample_ggx: fn(n: vec3, v: vec3, alpha: f32, r: vec2) -> vec3 {
            let a2 = alpha * alpha
            let ct = sqrt((1.0 - r.x) / (1.0 + (a2 - 1.0) * r.x))
            let st = sqrt(max(1.0 - ct * ct, 0.0))
            let phi = 6.28318530718 * r.y
            let h = self.to_world(n, vec3(st * cos(phi), st * sin(phi), ct))
            return h * (2.0 * dot(v, h)) - v
        }
        ggx_d: fn(nh: f32, a2: f32) -> f32 {
            let d = nh * nh * (a2 - 1.0) + 1.0
            return a2 / (3.14159265 * d * d)
        }
        bsdf_eval: fn(n: vec3, v: vec3, l: vec3, kd: vec3, f0: vec3, alpha: f32) -> vec3 {
            let h = normalize(v + l)
            let nl = max(dot(n, l), 0.0001)
            let nv = max(dot(n, v), 0.0001)
            let nh = max(dot(n, h), 0.0)
            let vh = max(dot(v, h), 0.0)
            let a2 = alpha * alpha
            let fr = f0 + (vec3(1.0, 1.0, 1.0) - f0) * pow(clamp(1.0 - vh, 0.0, 1.0), 5.0)
            let gv = nl * sqrt(nv * nv * (1.0 - a2) + a2)
            let gl = nv * sqrt(nl * nl * (1.0 - a2) + a2)
            let vis = 0.5 / max(gv + gl, 0.000001)
            return kd * (vec3(1.0, 1.0, 1.0) - fr) * 0.31830988618 + fr * (self.ggx_d(nh, a2) * vis)
        }
        bsdf_pdf: fn(n: vec3, v: vec3, l: vec3, alpha: f32, ps: f32) -> f32 {
            let h = normalize(v + l)
            let nh = max(dot(n, h), 0.0)
            let vh = max(dot(v, h), 0.0001)
            let nl = max(dot(n, l), 0.0)
            let a2 = alpha * alpha
            return ps * self.ggx_d(nh, a2) * nh / (4.0 * vh) + (1.0 - ps) * nl * 0.31830988618
        }
        spec_prob: fn(f0: vec3, kd: vec3, nv: f32) -> f32 {
            let fr = f0 + (vec3(1.0, 1.0, 1.0) - f0) * pow(clamp(1.0 - nv, 0.0, 1.0), 5.0)
            let lf = max(max(fr.x, fr.y), fr.z)
            let lk = max(max(kd.x, kd.y), kd.z)
            return clamp(lf / (lf + lk + 0.0001), 0.05, 0.95)
        }
        // Shader twin of scene::thin_glass_haze.
        thin_glass_haze: fn(roughness: f32) -> f32 {
            let s = clamp(roughness / #(crate::scene::THIN_GLASS_FULL_HAZE_ROUGHNESS), 0.0, 1.0)
            return s * s * (3.0 - 2.0 * s)
        }
        fresnel_dielectric: fn(cosi_in: f32, ior: f32) -> f32 {
            let cosi = clamp(cosi_in, 0.0, 1.0)
            let sint2 = (1.0 - cosi * cosi) / (ior * ior)
            if sint2 >= 1.0 { return 1.0 }
            let cost = sqrt(1.0 - sint2)
            let rs = (cosi - ior * cost) / (cosi + ior * cost)
            let rp = (ior * cosi - cost) / (ior * cosi + cost)
            return 0.5 * (rs * rs + rp * rp)
        }
        // Exact solid-interface Fresnel: eta = incident/transmitted IOR.
        solid_fresnel: fn(cosi_in: f32, eta: f32) -> f32 {
            if eta == 1.0 { return 0.0 }
            let cosi = clamp(cosi_in, 0.0, 1.0)
            let sint2 = eta * eta * (1.0 - cosi * cosi)
            if sint2 >= 1.0 { return 1.0 }
            let cost = sqrt(1.0 - sint2)
            let rs = (eta * cosi - cost) / (eta * cosi + cost)
            let rp = (cosi - eta * cost) / (cosi + eta * cost)
            return 0.5 * (rs * rs + rp * rp)
        }
        // xyz = direction, w = reflected. ng is outward, not face-forward.
        solid_interface: fn(rd: vec3, ng: vec3, ior: f32, sample: f32) -> vec4 {
            let front = dot(ng, rd) < 0.0
            let eta = if front { 1.0 / ior } else { ior }
            let n = if front { ng } else { -ng }
            let cosi = clamp(0.0 - dot(rd, n), 0.0, 1.0)
            let fr = self.solid_fresnel(cosi, eta)
            if fr >= 1.0 || sample < fr {
                return vec4(rd - n * (2.0 * dot(rd, n)), 1.0)
            }
            let cost = sqrt(max(1.0 - eta * eta * (1.0 - cosi * cosi), 0.0))
            return vec4(normalize(rd * eta + n * (eta * cosi - cost)), 0.0)
        }
        medium_transmittance: fn(absorption: vec3, distance: f32) -> vec3 {
            if distance <= 0.0 { return vec3(1.0, 1.0, 1.0) }
            return pow(clamp(absorption, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), vec3(distance, distance, distance))
        }
        // Material classes stay numeric: 1 is dielectric, 0 is opaque.
        // Material row 4.w separately selects a solid versus a thin sheet.
        material_class: fn(m2: vec4) -> f32 {
            if m2.y > 0.0 { return 1.0 }
            return 0.0
        }
        ray_error: fn(p: vec3) -> f32 {
            return self.scene_scale.x * 0.000000005 + max(max(abs(p.x), abs(p.y)), abs(p.z)) * 0.0000019073486
        }
        offset_ray: fn(p: vec3, ng: vec3, dir: vec3) -> vec3 {
            let n = if dot(ng, dir) >= 0.0 { ng } else { -ng }
            let eps = self.ray_error(p)
            return p + n * eps + dir * eps
        }
        spawn_trace: fn(p: vec3, ng: vec3, rd: vec3, tmax: f32, skip: f32) -> vec4 {
            let ro = self.offset_ray(p, ng, rd)
            var tmin = 0.0
            var h = vec4(tmax, -1.0, 0.0, 0.0)
            for step in 0..4 {
                h = self.trace(ro, rd, tmax, 0.0, tmin, skip)
                if h.y < 0.0 { return h }
                if h.x >= self.scene_scale.x * 0.00005 { return h }
                let ti = u32(h.y)
                let t0 = self.fetch_tri(ti * 3u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                let n = normalize(self.cross3(t1.xyz - t0.xyz, t2.xyz - t0.xyz))
                if dot(n, rd) <= 0.0 { return h }
                // A real solid exit must survive even inside the neighbour band.
                let m2 = self.fetch_mat(u32(t0.w) * 5u + 2u)
                let m4 = self.fetch_mat(u32(t0.w) * 5u + 4u)
                if m4.w > 0.5 && m2.y > 0.0 { return h }
                let plane = abs(dot(n, p - t0.xyz)) / max(abs(dot(n, rd)), 0.000001)
                tmin = max(plane, h.x) + self.ray_error(p)
            }
            return h
        }
        // Legacy thin exported-pane pairing, shared by transport and guides.
        thin_transmission_trace: fn(p: vec3, ng: vec3, rd: vec3, ti: u32) -> vec4 {
            var hit = self.spawn_trace(p, ng, rd, self.scene_scale.x * 5000000.0, f32(ti))
            if hit.y >= 0.0 && hit.x < self.scene_scale.x * 0.0001 {
                let back_ti = u32(hit.y)
                let back_t0 = self.fetch_tri(back_ti * 3u)
                let back_t1 = self.fetch_tri(back_ti * 3u + 1u)
                let back_t2 = self.fetch_tri(back_ti * 3u + 2u)
                let back_ng = normalize(self.cross3(back_t1.xyz - back_t0.xyz, back_t2.xyz - back_t0.xyz))
                let t0 = self.fetch_tri(ti * 3u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                let raw_ng = normalize(self.cross3(t1.xyz - t0.xyz, t2.xyz - t0.xyz))
                if back_t0.w == t0.w && dot(back_ng, raw_ng) < -0.9 {
                    let back_w0 = 1.0 - hit.z - hit.w
                    let back_p = back_t0.xyz * back_w0 + back_t1.xyz * hit.z + back_t2.xyz * hit.w
                    hit = self.spawn_trace(back_p, back_ng, rd, self.scene_scale.x * 5000000.0, hit.y)
                }
            }
            return hit
        }
        // Deterministic straight transmission; no RNG, reflection, or bend.
        // No guide when TIR prevents transmission or the budget is exhausted.
        guide_hit: fn(first: vec4, rd: vec3) -> vec4 {
            var hit = first
            for layer in 0..9 {
                if hit.y < 0.0 { return hit }
                let ti = u32(hit.y)
                let t0 = self.fetch_tri(ti * 3u)
                let m2 = self.fetch_mat(u32(t0.w) * 5u + 2u)
                if m2.y <= 0.0 { return hit }
                if layer == 8u { break }
                let m4 = self.fetch_mat(u32(t0.w) * 5u + 4u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                let ng = normalize(self.cross3(t1.xyz - t0.xyz, t2.xyz - t0.xyz))
                let p = t0.xyz * (1.0 - hit.z - hit.w) + t1.xyz * hit.z + t2.xyz * hit.w
                if m4.w > 0.5 {
                    let eta = if dot(ng, rd) < 0.0 { 1.0 / m2.x } else { m2.x }
                    if self.solid_fresnel(abs(dot(ng, -rd)), eta) >= 1.0 { break }
                    hit = self.spawn_trace(p, ng, rd, self.scene_scale.x * 5000000.0, f32(ti))
                } else {
                    hit = self.thin_transmission_trace(p, ng, rd, ti)
                }
            }
            return vec4(self.scene_scale.x * 5000000.0, -1.0, 0.0, 0.0)
        }
        shading_correction: fn(ng: vec3, ns: vec3, v: vec3, l: vec3) -> f32 {
            // Bounded at 4x (CPU twin agrees): the raw ratio is unbounded at
            // grazing geometry and turns modest NEE samples into fireflies.
            return min(abs((dot(v, ns) * dot(l, ng)) / max(dot(v, ng) * dot(l, ns), 0.000001)), 4.0)
        }
        finite3: fn(v: vec3) -> f32 {
            if v.x != v.x || v.y != v.y || v.z != v.z { return 0.0 }
            if abs(v.x) >= 1000000000000000000000000000000.0 || abs(v.y) >= 1000000000000000000000000000000.0 || abs(v.z) >= 1000000000000000000000000000000.0 { return 0.0 }
            return 1.0
        }
        sun_sample: fn(r: vec2) -> vec3 {
            let ct = 1.0 - r.x * (1.0 - self.sun_dir.w)
            let st = sqrt(max(1.0 - ct * ct, 0.0))
            let phi = 6.28318530718 * r.y
            return self.to_world(self.sun_dir.xyz, vec3(st * cos(phi), st * sin(phi), ct))
        }
        aperture: fn(r: vec2) -> vec2 {
            let blades = self.lens.z
            if blades < 2.5 {
                let rr = sqrt(r.x)
                let phi = 6.28318530718 * r.y
                return vec2(rr * cos(phi), rr * sin(phi))
            }
            let seg = floor(r.x * blades)
            let fu = r.x * blades - seg
            let a0 = 6.28318530718 * seg / blades
            let a1 = 6.28318530718 * (seg + 1.0) / blades
            let p0 = vec2(cos(a0), sin(a0))
            let p1 = vec2(cos(a1), sin(a1))
            let su = sqrt(r.y)
            return p0 * (su * (1.0 - fu)) + p1 * (su * fu)
        }
        fetch_env: fn(index: u32) -> vec4 {
            return self.env_tex.sample_nearest(self.tx(index, self.env_inv))
        }
        env_to_map: fn(d: vec3) -> vec3 {
            return vec3(dot(self.env_row0.xyz, d), dot(self.env_row1.xyz, d), dot(self.env_row2.xyz, d))
        }
        env_texel: fn(d: vec3) -> vec2 {
            let u = 0.5 + atan2(d.x, -d.z) / 6.28318530718
            let v = acos(clamp(d.y, -1.0, 1.0)) / 3.14159265359
            return clamp(floor(vec2(u, v) * self.env_size), vec2(0.0, 0.0), self.env_size - vec2(1.0, 1.0))
        }
        env_radiance: fn(d_world: vec3) -> vec3 {
            let ij = self.env_texel(self.env_to_map(d_world))
            let index = u32(ij.y) * u32(self.env_size.x) + u32(ij.x)
            return self.fetch_env(index).xyz * self.env_strength
        }
        env_solid_angle_pdf: fn(probability: f32, sin_theta: f32) -> f32 {
            if sin_theta <= 0.0 { return 0.0 }
            return probability * self.env_size.x * self.env_size.y / (2.0 * 3.14159265359 * 3.14159265359 * sin_theta)
        }
        env_pdf: fn(d_world: vec3) -> f32 {
            let d = self.env_to_map(d_world)
            let ij = self.env_texel(d)
            let i = u32(ij.x)
            let j = u32(ij.y)
            let width = u32(self.env_size.x)
            let marginal = width * u32(self.env_size.y)
            let index = j * width + i
            var row_prev = 0.0
            var col_prev = 0.0
            if j > 0u { row_prev = self.fetch_env(marginal + j - 1u).x }
            if i > 0u { col_prev = self.fetch_env(index - 1u).w }
            let probability = (self.fetch_env(marginal + j).x - row_prev) * (self.fetch_env(index).w - col_prev)
            return self.env_solid_angle_pdf(probability, sqrt(max(0.0, 1.0 - d.y * d.y)))
        }
        env_sample: fn(r: vec2) -> vec4 {
            let width = u32(self.env_size.x)
            let height = u32(self.env_size.y)
            let marginal = width * height
            var lo = 0u
            var hi = height
            for search in 0..24 {
                if lo >= hi { break }
                let mid = (lo + hi) >> 1u
                if r.x < self.fetch_env(marginal + mid).x { hi = mid } else { lo = mid + 1u }
            }
            let j = min(lo, height - 1u)
            var row_prev = 0.0
            if j > 0u { row_prev = self.fetch_env(marginal + j - 1u).x }
            let row_pdf = self.fetch_env(marginal + j).x - row_prev
            var v_off = 0.0
            if row_pdf > 0.0 { v_off = clamp((r.x - row_prev) / row_pdf, 0.0, 0.99999994) }
            lo = 0u
            hi = width
            for search in 0..24 {
                if lo >= hi { break }
                let mid = (lo + hi) >> 1u
                if r.y < self.fetch_env(j * width + mid).w { hi = mid } else { lo = mid + 1u }
            }
            let i = min(lo, width - 1u)
            let index = j * width + i
            var col_prev = 0.0
            if i > 0u { col_prev = self.fetch_env(index - 1u).w }
            let col_pdf = self.fetch_env(index).w - col_prev
            var u_off = 0.0
            if col_pdf > 0.0 { u_off = clamp((r.y - col_prev) / col_pdf, 0.0, 0.99999994) }
            let u = (f32(i) + u_off) / self.env_size.x
            let v = (f32(j) + v_off) / self.env_size.y
            let theta = 3.14159265359 * v
            let phi = 6.28318530718 * (u - 0.5)
            let sin_theta = sin(theta)
            let d = vec3(sin_theta * sin(phi), cos(theta), (0.0 - sin_theta) * cos(phi))
            let world = self.env_row0.xyz * d.x + self.env_row1.xyz * d.y + self.env_row2.xyz * d.z
            return vec4(world, self.env_solid_angle_pdf(row_pdf * col_pdf, sin_theta))
        }
        environment: fn(rd: vec3) -> vec3 {
            if self.env_on > 0.5 { return self.env_radiance(rd) }
            return self.sky(rd)
        }
        sky_pdf: fn(rd: vec3) -> f32 {
            if self.env_on > 0.5 { return self.env_pdf(rd) }
            let z = dot(rd, self.world_up.xyz)
            if z <= 0.0 { return 0.0 }
            return 0.07957747155 + 0.03978873577 / sqrt(max(z, 0.000001))
        }
        environment_distribution: fn(p: vec3, normal: vec3) -> u32 {
            let d = u32(self.env_grid_dim)
            let rel = (p - self.env_min) * self.env_inv_extent
            let x = min(u32(clamp(rel.x, 0.0, 0.99999994) * self.env_grid_dim), d - 1u)
            let y = min(u32(clamp(rel.y, 0.0, 0.99999994) * self.env_grid_dim), d - 1u)
            let z = min(u32(clamp(rel.z, 0.0, 0.99999994) * self.env_grid_dim), d - 1u)
            var axis = 0u
            if abs(normal.x) >= abs(normal.y) && abs(normal.x) >= abs(normal.z) {
                if normal.x < 0.0 { axis = 1u }
            } else if abs(normal.y) >= abs(normal.z) {
                axis = 2u
                if normal.y < 0.0 { axis = 3u }
            } else {
                axis = 4u
                if normal.z < 0.0 { axis = 5u }
            }
            return ((z * d + y) * d + x) * 6u + axis
        }
        guided_environment_sample: fn(p: vec3, normal: vec3, r: vec2) -> vec4 {
            let offset = u32(self.n_lights) + self.environment_distribution(p, normal) * 128u
            var lo = 0u
            var hi = 128u
            for search in 0..24 {
                if lo >= hi { break }
                let mid = (lo + hi) >> 1u
                if r.x < self.fetch_light(offset + mid).w { hi = mid } else { lo = mid + 1u }
            }
            let direction_bin = min(lo, 127u)
            let rec = self.fetch_light(offset + direction_bin)
            var previous = 0.0
            if direction_bin > 0u { previous = self.fetch_light(offset + direction_bin - 1u).w }
            let local_u = clamp((r.x - previous) / max(rec.z, 0.00000001), 0.0, 0.99999994)
            let bin = u32(rec.x)
            let theta_bin = bin >> 4u
            let phi_bin = bin & 15u
            let cos_theta = (f32(theta_bin) + local_u) * 0.125
            let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0))
            let phi = 6.28318530718 * (f32(phi_bin) + r.y) * 0.0625
            let ld = self.to_world(self.world_up.xyz, vec3(sin_theta * cos(phi), sin_theta * sin(phi), cos_theta))
            return vec4(ld, rec.z / max(rec.y, 0.000000000001))
        }
        environment_sample: fn(p: vec3, normal: vec3, r: vec2) -> vec4 {
            if self.env_on > 0.5 { return self.env_sample(r) }
            if self.env_grid_dim > 0.5 {
                return self.guided_environment_sample(p, normal, r)
            }
            let x = clamp(r.x, 0.0, 0.99999994)
            let z = if x < 0.5 { pow(x * 2.0, 2.0) } else { (x - 0.5) * 2.0 }
            let rr = sqrt(max(1.0 - z * z, 0.0))
            let phi = 6.28318530718 * r.y
            let ld = self.to_world(self.world_up.xyz, vec3(rr * cos(phi), rr * sin(phi), z))
            return vec4(ld, self.sky_pdf(ld))
        }
        escape: fn(rd: vec3, prev_pdf: f32, delta: f32) -> vec3 {
            // Brute mode is the BSDF-only oracle: no NEE runs, so nothing
            // may be suppressed here (CPU twin agrees).
            if self.brute > 0.5 { return self.environment(rd) + self.visible_sun(rd) }
            var l = self.environment(rd)
            if delta > 0.5 { l = l + self.visible_sun(rd) }
            if delta > 0.5 { return l }
            if self.env_on < 0.5 && self.env_grid_dim > 0.5 {
                if dot(rd, self.world_up.xyz) < 0.0 { return l }
                return vec3(0.0, 0.0, 0.0)
            }
            let pe = self.sky_pdf(rd)
            return l * (prev_pdf * prev_pdf / (prev_pdf * prev_pdf + pe * pe))
        }
        // Straight NEE ignores bending, counts every solid interface, and
        // applies absorption only on back-side arrivals (CPU shadow twin).
        shadow: fn(p_in: vec3, ng_in: vec3, rd: vec3, tmax_in: f32, skip_in: f32, far_sheet: bool, target: f32) -> vec4 {
            var p = p_in
            var ng = ng_in
            var skip = skip_in
            var tmax = tmax_in
            var tr = vec3(1.0, 1.0, 1.0)
            var travelled = 0.0
            var previous_glass_material = -1.0
            var previous_glass_normal = vec3(0.0, 0.0, 0.0)
            if self.frost_lobe() > 0.5 && far_sheet && skip_in >= 0.0 {
                let ti = u32(skip_in)
                let t0 = self.fetch_tri(ti * 3u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                previous_glass_material = t0.w
                previous_glass_normal = normalize(self.cross3(t1.xyz - t0.xyz, t2.xyz - t0.xyz))
            }
            // One final fixed iteration verifies escape after eight glass hits.
            for layer in 0..9 {
                let h = self.spawn_trace(p, ng, rd, tmax, skip)
                if h.y < -1.5 { return vec4(0.0, 0.0, 0.0, -1.0) }
                if h.y < 0.0 { return vec4(tr, 0.0) }
                // The sampled light's own triangle is the target, never a blocker.
                if h.y == target { return vec4(tr, 0.0) }
                if layer == 8u { return vec4(0.0, 0.0, 0.0, 0.0) }
                let ti = u32(h.y)
                let t0 = self.fetch_tri(ti * 3u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                let m2 = self.fetch_mat(u32(t0.w) * 5u + 2u)
                let m4 = self.fetch_mat(u32(t0.w) * 5u + 4u)
                let solid = m4.w > 0.5 && m2.y > 0.0
                // Self-occlusion skin: a blocker within `shadow_skin` of the
                // ORIGINAL surface is a stacked construction layer of the same
                // assembly (mm separations), not a shadow caster — step past
                // it. `travelled` bounds the total skip to one skin depth.
                if !solid && travelled + h.x < self.shadow_skin {
                    travelled = travelled + h.x
                    ng = normalize(self.cross3(t1.xyz - t0.xyz, t2.xyz - t0.xyz))
                    let w0s = 1.0 - h.z - h.w
                    p = t0.xyz * w0s + t1.xyz * h.z + t2.xyz * h.w
                    skip = h.y
                    if tmax < self.scene_scale.x * 2500000.0 {
                        tmax = tmax - h.x - self.ray_error(p)
                        if tmax <= 0.0 { return vec4(tr, 0.0) }
                    }
                    continue
                }
                let m0 = self.fetch_mat(u32(t0.w) * 5u)
                let material_class = self.material_class(m2)
                if material_class < 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
                ng = normalize(self.cross3(t1.xyz - t0.xyz, t2.xyz - t0.xyz))
                let paired_back_face = previous_glass_material == t0.w && h.x < self.scene_scale.x * 0.0001 && dot(previous_glass_normal, ng) < -0.9
                let w0 = 1.0 - h.z - h.w
                let segment_start = p
                p = t0.xyz * w0 + t1.xyz * h.z + t2.xyz * h.w
                skip = h.y
                if paired_back_face {
                    previous_glass_material = -1.0
                    if tmax < self.scene_scale.x * 2500000.0 {
                        tmax = tmax - h.x - self.ray_error(p)
                        if tmax <= 0.0 { return vec4(tr, 0.0) }
                    }
                    continue
                }
                if solid {
                    let front = dot(ng, rd) < 0.0
                    let eta = if front { 1.0 / m2.x } else { m2.x }
                    if !front {
                        tr = tr * self.medium_transmittance(m4.xyz, length(p - segment_start))
                    }
                    tr = tr * (m2.y * (1.0 - self.solid_fresnel(abs(dot(ng, -rd)), eta)))
                    previous_glass_material = -1.0
                } else {
                    var tint = m0.xyz
                    if m2.z > 0.0 {
                        let a0 = self.fetch_attr(ti * 4u)
                        let a1 = self.fetch_attr(ti * 4u + 1u)
                        let a2 = self.fetch_attr(ti * 4u + 2u)
                        let a3 = self.fetch_attr(ti * 4u + 3u)
                        let uv = vec2(a0.w, a1.w) * w0 + vec2(a2.w, a3.x) * h.z + vec2(a3.y, a3.z) * h.w
                        let m3 = self.fetch_mat(u32(t0.w) * 5u + 3u)
                        let tc = self.atlas_tex.sample_nearest(m3.xy + fract(uv) * m3.zw).xyz
                        tint = tint * pow(tc, vec3(2.2, 2.2, 2.2))
                    }
                    let fr = self.fresnel_dielectric(abs(dot(ng, -rd)), m2.x)
                    var transmission = m2.y
                    if self.frost_lobe() > 0.5 {
                        transmission = transmission * (1.0 - self.thin_glass_haze(m0.w))
                    }
                    tr = tr * tint * (transmission * (1.0 - fr))
                    if self.frost_lobe() > 0.5 && tr.x == 0.0 && tr.y == 0.0 && tr.z == 0.0 { return vec4(0.0, 0.0, 0.0, 0.0) }
                    previous_glass_material = t0.w
                    previous_glass_normal = ng
                }
                if tmax < self.scene_scale.x * 2500000.0 {
                    tmax = tmax - h.x - self.ray_error(p)
                    if tmax <= 0.0 { return vec4(tr, 0.0) }
                }
            }
            return vec4(0.0, 0.0, 0.0, 0.0)
        }

        // The variant overrides these hooks. In an ordinary trace the constant
        // removes the reservoir and leaves only the base radiance and count.
        route_groups: fn() -> f32 { return 0.0 }
        // Only frost variants retain rough-sheet transport.
        frost_lobe: fn() -> f32 { return 0.0 }
        write_group: fn(entry: vec4) {}
        group_term: fn(term: vec3, id: f32) -> vec4 { return vec4(term, id) }
        merge_group: fn(a: vec4, b: vec4, r: f32) -> vec4 { return a }

        // ---- one path ----
        radiance: fn(px: u32, py: u32, pseed: u32, sidx: u32) -> PtLightSample {
            let j = self.sobol2(sidx, pseed, 0u)
            var ro = self.cam_pos
            var rd = vec3(0.0, 0.0, 1.0)
            var hit = vec4(0.0, -1.0, 0.0, 0.0)
            if self.use_gbuffer > 0.5 {
                let ndc = vec2((f32(px) + 0.5 + self.jitter.x) * self.cam_inv.x * 2.0 - 1.0, 1.0 - (f32(py) + 0.5 + self.jitter.y) * self.cam_inv.y * 2.0)
                if self.ortho.x > 0.0 {
                    ro = ro + self.cam_right * (ndc.x * self.ortho.x) + self.cam_up * (ndc.y * self.ortho.y)
                    rd = self.cam_fwd
                } else {
                    rd = normalize(self.cam_fwd + self.cam_right * (ndc.x * self.cam_tan.x) + self.cam_up * (ndc.y * self.cam_tan.y))
                }
                let g = self.gbuf_tex.sample_nearest((vec2(f32(px), f32(py)) + vec2(0.5, 0.5)) * self.inv_res)
                hit = vec4(g.w, g.x, g.y, g.z)
            } else {
                let ndc = vec2((f32(px) + j.x) * self.cam_inv.x * 2.0 - 1.0, 1.0 - (f32(py) + j.y) * self.cam_inv.y * 2.0)
                var dir = normalize(self.cam_fwd + self.cam_right * (ndc.x * self.cam_tan.x) + self.cam_up * (ndc.y * self.cam_tan.y))
                if self.ortho.x > 0.0 {
                    ro = ro + self.cam_right * (ndc.x * self.ortho.x) + self.cam_up * (ndc.y * self.ortho.y)
                    dir = self.cam_fwd
                }
                if self.lens.x > 0.0 && self.ortho.x <= 0.0 {
                    let lr = self.sobol2(sidx, pseed, 1u)
                    let ap = self.aperture(lr) * self.lens.x
                    let fp = ro + dir * (self.lens.y / max(dot(dir, self.cam_fwd), 0.001))
                    ro = ro + self.cam_right * ap.x + self.cam_up * ap.y
                    dir = normalize(fp - ro)
                }
                rd = dir
                hit = self.trace(ro, rd, self.scene_scale.x * 5000000.0, 0.0, 0.0, -1.0)
            }
            if self.debug_mode > 2.5 && self.debug_mode < 3.5 {
                let r = self.sobol2(sidx, pseed, 3u)
                return PtLightSample.new(vec3(r.x, r.y, self.rand(pseed, sidx, 7u)))
            }
            if self.debug_mode > 6.5 && self.debug_mode < 7.5 {
                return PtLightSample.new(vec3(hit.y, hit.z, hit.w))
            }
            if self.debug_mode > 8.5 && self.debug_mode < 9.5 {
                return PtLightSample.new(self.escape(rd, 0.0, 1.0))
            }
            if self.debug_mode > 0.5 && self.debug_mode < 2.5 {
                if hit.y < 0.0 { return PtLightSample.new(vec3(0.0, 0.0, 0.0)) }
                if self.debug_mode < 1.5 {
                    let tii = u32(hit.y)
                    let a0 = self.fetch_attr(tii * 4u)
                    let a1 = self.fetch_attr(tii * 4u + 1u)
                    let a2 = self.fetch_attr(tii * 4u + 2u)
                    let w0 = 1.0 - hit.z - hit.w
                    let nn = normalize(a0.xyz * w0 + a1.xyz * hit.z + a2.xyz * hit.w)
                    return PtLightSample.new(nn * 0.5 + vec3(0.5, 0.5, 0.5))
                }
                let mid = u32(self.fetch_tri(u32(hit.y) * 3u).w)
                return PtLightSample.new(self.fetch_mat(mid * 5u).xyz)
            }
            // Primary parity with the raster pane (CPU twin agrees): the
            // camera ray passes through one-sided backfaces exactly as
            // fixed-function backface culling does. Bounce rays keep
            // flip-shading below.
            // Single-hop continuations only (CPU twin agrees): the
            // spurious-neighbour ladder is material-blind and could step
            // past a paired two-sided glazing backface the raster keeps.
            // A stack deeper than eight falls through to flip-shading.
            for ps in 0..8 {
                if hit.y < 0.0 { break }
                let skt = u32(hit.y)
                let sk0 = self.fetch_tri(skt * 3u)
                let sk1 = self.fetch_tri(skt * 3u + 1u)
                let sk2 = self.fetch_tri(skt * 3u + 2u)
                let skn = self.cross3(sk1.xyz - sk0.xyz, sk2.xyz - sk0.xyz)
                if dot(skn, rd) < 0.0 { break }
                let skm = self.fetch_mat(u32(sk0.w) * 5u + 2u)
                let skm4 = self.fetch_mat(u32(sk0.w) * 5u + 4u)
                if skm.w > 0.5 || (skm4.w > 0.5 && skm.y > 0.0) { break }
                let skw = 1.0 - hit.z - hit.w
                let skp = sk0.xyz * skw + sk1.xyz * hit.z + sk2.xyz * hit.w
                let sko = self.offset_ray(skp, normalize(skn), rd)
                hit = self.trace(sko, rd, self.scene_scale.x * 5000000.0, 0.0, 0.0, f32(skt))
            }
            var tp = vec3(1.0, 1.0, 1.0)
            var lsum = vec3(0.0, 0.0, 0.0)
            var far = vec4(0.0, 0.0, 0.0, 0.0)
            var prev_pdf = 0.0
            var delta = 1.0
            // Distance from the last scattering vertex to the ray origin through windows,
            // so an emitter hit weighs its MIS from that vertex, as its NEE did.
            var carry = 0.0
            var ndiff = 0.0
            var nb = u32(self.max_bounces)
            if self.debug_mode > 7.5 && self.debug_mode < 8.5 { nb = 1u }
            for b in 0..16 {
                if b >= nb { break }
                if hit.y < -1.5 { return PtLightSample.new(vec3(1000000000000000000000000000000.0, 0.0, 0.0)) }
                if self.debug_mode > 5.5 && f32(b) == self.dbg_b {
                    return PtLightSample.new(vec3(hit.y + 1000.0, hit.x, tp.x))
                }
                if hit.y < 0.0 {
                    lsum = lsum + tp * self.escape(rd, prev_pdf, delta)
                    break
                }
                let ti = u32(hit.y)
                let t0 = self.fetch_tri(ti * 3u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                let w0 = 1.0 - hit.z - hit.w
                let p = t0.xyz * w0 + t1.xyz * hit.z + t2.xyz * hit.w
                let e1 = t1.xyz - t0.xyz
                let e2 = t2.xyz - t0.xyz
                let gn = self.cross3(e1, e2)
                var ng = normalize(gn)
                let a0 = self.fetch_attr(ti * 4u)
                let a1 = self.fetch_attr(ti * 4u + 1u)
                let a2 = self.fetch_attr(ti * 4u + 2u)
                let a3 = self.fetch_attr(ti * 4u + 3u)
                var ns = normalize(a0.xyz * w0 + a1.xyz * hit.z + a2.xyz * hit.w)
                let uv = vec2(a0.w, a1.w) * w0 + vec2(a2.w, a3.x) * hit.z + vec2(a3.y, a3.z) * hit.w
                let mi = u32(t0.w)
                let m0 = self.fetch_mat(mi * 5u)
                let m1 = self.fetch_mat(mi * 5u + 1u)
                let m2 = self.fetch_mat(mi * 5u + 2u)
                let m4 = self.fetch_mat(mi * 5u + 4u)
                var front = 1.0
                if dot(ng, rd) >= 0.0 { front = 0.0 }
                if m4.w > 0.5 && m2.y > 0.0 && front < 0.5 {
                    tp = tp * self.medium_transmittance(m4.xyz, length(p - ro))
                }
                if front < 0.5 {
                    // One-sided backfaces shade with the flipped normal
                    // instead of terminating black (CPU twin agrees);
                    // emission stays front-gated below.
                    ng = -ng
                }
                if dot(ns, ng) < 0.0 { ns = -ns }
                if dot(ns, ng) < 0.05 { ns = ng }
                var albedo = m0.xyz
                if m2.z > 0.0 {
                    let m3 = self.fetch_mat(mi * 5u + 3u)
                    var tc = self.atlas_tex.sample_nearest(m3.xy + fract(uv) * m3.zw).xyz
                    // Minification: a nearest fetch on a texture tiled tens
                    // to hundreds of times under one pixel is a per-sample
                    // texel lottery — spatial salt-and-pepper that never
                    // converges pixel-to-pixel and shimmers as samples land.
                    // Blend toward the image's linear mean (the atlas
                    // corner texel) as the pixel footprint crosses texels:
                    // the same value the estimator converges to, at zero
                    // variance. Magnification is untouched.
                    if self.pixel_world > 0.0 {
                        let duv1 = vec2(a2.w, a3.x) - vec2(a0.w, a1.w)
                        let duv2 = vec2(a3.y, a3.z) - vec2(a0.w, a1.w)
                        let uv_area = abs(duv1.x * duv2.y - duv1.y * duv2.x)
                        // Anisotropy matters: a grazing view compresses one
                        // texture axis under a pixel long before the area
                        // metric notices (that is the moire on the roof).
                        // Take the worst of the area metric and the two
                        // edge densities, and stretch the footprint by the
                        // incidence angle.
                        let d_area = sqrt(uv_area / max(length(gn), self.scene_scale.y * 0.000000000000000025))
                        let d1 = length(duv1) / max(length(e1), self.scene_scale.x * 0.000000005)
                        let d2 = length(duv2) / max(length(e2), self.scene_scale.x * 0.000000005)
                        let density = max(d_area, max(d1, d2))
                        var footprint_m = self.pixel_world * hit.x
                        if self.ortho.x > 0.0 { footprint_m = self.pixel_world }
                        footprint_m = footprint_m / max(abs(dot(rd, ng)), 0.1)
                        let texels = density * footprint_m * (m3.z / self.atlas_inv.x)
                        let blend = clamp((texels - 2.0) * 0.071428575, 0.0, 1.0)
                        if blend > 0.0 {
                            let mean = self.atlas_tex.sample_nearest(m3.xy - self.atlas_inv).xyz
                            tc = mix(tc, mean, blend)
                        }
                    }
                    albedo = albedo * pow(tc, vec3(2.2, 2.2, 2.2))
                }
                let rough = max(m0.w, 0.03)
                let emission = m1.xyz
                var metal = m1.w
                let ior = m2.x
                let trans = m2.y
                let material_class = self.material_class(m2)
                if emission.x + emission.y + emission.z > 0.0 && (front > 0.5 || m2.w > 0.5) {
                    var w = 1.0
                    if delta < 0.5 && self.brute < 0.5 {
                        let area = length(gn) * 0.5
                        let raw_n = gn / max(length(gn), self.scene_scale.y * 0.00000000000000000000000025)
                        let cosl = if m2.w > 0.5 { abs(dot(raw_n, -rd)) } else { max(dot(raw_n, -rd), 0.0) }
                        let lum = 0.2126 * emission.x + 0.7152 * emission.y + 0.0722 * emission.z
                        let select_pdf = max(lum * area, 0.0) * self.inv_light_power
                        let d = carry + hit.x
                        let pl = d * d * select_pdf / max(cosl * area, self.scene_scale.y * 0.000000000025)
                        w = prev_pdf * prev_pdf / (prev_pdf * prev_pdf + pl * pl)
                    }
                    let term = tp * emission * w
                    if self.route_groups() > 0.5 && t2.w > 0.0 {
                        far = self.merge_group(far, self.group_term(term, t2.w), self.rand(pseed, sidx, 128u + b * 2u))
                        if self.finite3(far.xyz) < 0.5 { return PtLightSample.new(vec3(1000000000000000000000000000000.0, 0.0, 0.0)) }
                    } else {
                        lsum = lsum + term
                    }
                }
                let dim = 2u + b * 5u
                let r_lobe = self.sobol2(sidx, pseed, dim)
                let r_bsdf = self.sobol2(sidx, pseed, dim + 1u)
                let r_env = self.sobol2(sidx, pseed, dim + 2u)
                let r_light = self.sobol2(sidx, pseed, dim + 3u)
                let r_misc = self.sobol2(sidx, pseed, dim + 4u)
                var v = -rd
                if m4.w > 0.5 && trans > 0.0 {
                    let event = self.solid_interface(rd, normalize(gn), ior, r_lobe.x)
                    rd = event.xyz
                    if event.w > 0.5 {
                        delta = 1.0
                    } else {
                        tp = tp * trans
                        // Retain diffuse NEE ownership, as for thin sheets.
                    }
                    ro = p
                    carry = 0.0
                    hit = self.spawn_trace(p, ng, rd, self.scene_scale.x * 5000000.0, f32(ti))
                    continue
                }
                var rx = r_lobe.x
                var f0 = vec3(0.0, 0.0, 0.0)
                var kd = albedo
                var far_sheet = false
                if material_class > 0.5 {
                    let haze = if self.frost_lobe() > 0.5 { self.thin_glass_haze(m0.w) } else { 0.0 }
                    let fr = self.fresnel_dielectric(abs(dot(ng, v)), ior)
                    if self.frost_lobe() < 0.5 || r_lobe.y >= haze {
                        ro = p
                        if r_lobe.x < fr {
                            rd = rd - ng * (2.0 * dot(rd, ng))
                            delta = 1.0
                            carry = 0.0
                            hit = self.spawn_trace(p, ng, rd, self.scene_scale.x * 5000000.0, f32(ti))
                        } else {
                            tp = tp * albedo * trans
                            carry = carry + hit.x
                            // Preserve a preceding diffuse event so its direct
                            // environment contribution remains owned by NEE.
                            hit = self.thin_transmission_trace(p, ng, rd, ti)
                        }
                        continue
                    }
                    if self.frost_lobe() > 0.5 {
                        // Select a rough-sheet part, then reuse the opaque estimator.
                        metal = 0.0
                        if r_lobe.x < fr {
                            kd = vec3(0.0, 0.0, 0.0)
                            f0 = vec3(1.0, 1.0, 1.0)
                        } else if (r_lobe.x - fr) / (1.0 - fr) < trans {
                            v = v - ng * (2.0 * dot(v, ng))
                            ng = -ng
                            ns = -ns
                            if dot(ns, v) <= 0.0 { ns = ng }
                            far_sheet = true
                        }
                        rx = r_lobe.y / haze
                    }
                } else {
                    f0 = mix(vec3(0.04, 0.04, 0.04), albedo, metal)
                }
                kd = kd * (1.0 - metal)
                let alpha = rough * rough
                let nv = max(dot(ns, v), 0.0001)
                var ps = self.spec_prob(f0, kd, nv)
                // Next events: uniform solar disc plus the smooth sky.
                if self.brute < 0.5 {
                    if self.sun_on > 0.5 {
                        let ld = self.sun_sample(r_light)
                        var n_nee = ns
                        if dot(ng, ld) > 0.0 && dot(ns, ld) <= 0.0 { n_nee = ng }
                        let nl = dot(n_nee, ld)
                        if nl > 0.0 && dot(ng, ld) > 0.0 {
                            let sh = self.shadow(p, ng, ld, self.scene_scale.x * 5000000.0, f32(ti), far_sheet, -1.0)
                            if sh.w < 0.0 { return PtLightSample.new(vec3(1000000000000000000000000000000.0, 0.0, 0.0)) }
                            if sh.x + sh.y + sh.z > 0.0 {
                                let omega = 6.28318530718 * (1.0 - self.sun_dir.w)
                                let e = self.sun_radiance.xyz * omega
                                let f = self.bsdf_eval(n_nee, v, ld, kd, f0, alpha)
                                let corr = self.shading_correction(ng, n_nee, v, ld)
                                lsum = lsum + tp * f * e * sh.xyz * (nl * corr)
                            }
                        }
                    }
                    let env = self.environment_sample(p, ng, r_env)
                    let ld = env.xyz
                    let pe = env.w
                    var n_nee = ns
                    if dot(ng, ld) > 0.0 && dot(ns, ld) <= 0.0 { n_nee = ng }
                    let nl = dot(n_nee, ld)
                    if nl > 0.0 && dot(ng, ld) > 0.0 {
                        let sh = self.shadow(p, ng, ld, self.scene_scale.x * 5000000.0, f32(ti), far_sheet, -1.0)
                        if sh.w < 0.0 { return PtLightSample.new(vec3(1000000000000000000000000000000.0, 0.0, 0.0)) }
                        if pe > 0.0 && sh.x + sh.y + sh.z > 0.0 {
                            let ps_nee = self.spec_prob(f0, kd, max(dot(n_nee, v), 0.0001))
                            let f = self.bsdf_eval(n_nee, v, ld, kd, f0, alpha)
                            let pb = self.bsdf_pdf(n_nee, v, ld, alpha, ps_nee)
                            var w = 1.0
                            if self.env_on > 0.5 || self.env_grid_dim < 0.5 { w = pe * pe / (pe * pe + pb * pb) }
                            let corr = self.shading_correction(ng, n_nee, v, ld)
                            lsum = lsum + tp * f * self.environment(ld) * sh.xyz * (nl * corr / pe * w)
                        }
                    }
                }
                // next event: an emissive triangle
                if self.n_lights > 0.5 && self.inv_light_power > 0.0 && self.brute < 0.5 {
                    var lo = 0u
                    var hi = u32(self.n_lights)
                    for search in 0..24 {
                        if lo >= hi { break }
                        let mid = (lo + hi) >> 1u
                        if r_misc.x < self.fetch_light(mid).w { hi = mid } else { lo = mid + 1u }
                    }
                    let li = min(lo, u32(self.n_lights) - 1u)
                    let lrec = self.fetch_light(li)
                    let lt = u32(lrec.x)
                    let l0 = self.fetch_tri(lt * 3u)
                    let l1 = self.fetch_tri(lt * 3u + 1u)
                    let l2 = self.fetch_tri(lt * 3u + 2u)
                    let su = sqrt(r_light.x)
                    let bw1 = su * (1.0 - r_light.y)
                    let bw2 = su * r_light.y
                    let lp = l0.xyz * (1.0 - su) + l1.xyz * bw1 + l2.xyz * bw2
                    let lnrm = normalize(self.cross3(l1.xyz - l0.xyz, l2.xyz - l0.xyz))
                    let nominal = normalize(lp - p)
                    let origin = self.offset_ray(p, ng, nominal)
                    var ld = lp - origin
                    let dist = length(ld)
                    ld = ld / dist
                    var n_nee = ns
                    if dot(ng, ld) > 0.0 && dot(ns, ld) <= 0.0 { n_nee = ng }
                    let nl = dot(n_nee, ld)
                    let lm2 = self.fetch_mat(u32(l0.w) * 5u + 2u)
                    let cosl = if lm2.w > 0.5 { abs(dot(lnrm, -ld)) } else { max(dot(lnrm, -ld), 0.0) }
                    if lrec.z > 0.0 && nl > 0.0 && dot(ng, ld) > 0.0 && cosl > 0.0001 && dist > self.ray_error(p) + self.ray_error(lp) {
                        let sh = self.shadow(p, ng, ld, dist - self.ray_error(lp), f32(ti), far_sheet, f32(lt))
                        if sh.w < 0.0 { return PtLightSample.new(vec3(1000000000000000000000000000000.0, 0.0, 0.0)) }
                        if sh.x + sh.y + sh.z > 0.0 {
                            let lm = self.fetch_mat(u32(l0.w) * 5u + 1u).xyz
                            let pl = dist * dist * lrec.z / max(cosl * lrec.y, self.scene_scale.y * 0.00000000000025)
                            let ps_nee = self.spec_prob(f0, kd, max(dot(n_nee, v), 0.0001))
                            let f = self.bsdf_eval(n_nee, v, ld, kd, f0, alpha)
                            let pb = self.bsdf_pdf(n_nee, v, ld, alpha, ps_nee)
                            let w = pl * pl / (pl * pl + pb * pb)
                            let corr = self.shading_correction(ng, n_nee, v, ld)
                            let term = tp * f * lm * sh.xyz * (nl * corr / pl * w)
                            if self.route_groups() > 0.5 && l2.w > 0.0 {
                                far = self.merge_group(far, self.group_term(term, l2.w), self.rand(pseed, sidx, 129u + b * 2u))
                                if self.finite3(far.xyz) < 0.5 { return PtLightSample.new(vec3(1000000000000000000000000000000.0, 0.0, 0.0)) }
                            } else {
                                lsum = lsum + term
                            }
                        }
                    }
                }
                // continue the path
                var sample_n = ns
                var ld = vec3(0.0, 0.0, 1.0)
                var sampled_diffuse = 0.0
                if rx < ps {
                    ld = self.sample_ggx(sample_n, v, alpha, r_bsdf)
                } else {
                    ld = self.sample_cos(sample_n, r_bsdf)
                    sampled_diffuse = 1.0
                }
                if dot(ng, ld) <= 0.0 {
                    sample_n = ng
                    ps = self.spec_prob(f0, kd, max(dot(sample_n, v), 0.0001))
                    if rx < ps {
                        ld = self.sample_ggx(sample_n, v, alpha, r_bsdf)
                        sampled_diffuse = 0.0
                    } else {
                        ld = self.sample_cos(sample_n, r_bsdf)
                        sampled_diffuse = 1.0
                    }
                }
                // Every rough-sheet event consumes one diffuse depth.
                if self.frost_lobe() > 0.5 && material_class > 0.5 {
                    ndiff = ndiff + 1.0
                } else {
                    ndiff = ndiff + sampled_diffuse
                }
                let nl = dot(sample_n, ld)
                if nl <= 0.0 || dot(ng, ld) <= 0.0 { break }
                let f = self.bsdf_eval(sample_n, v, ld, kd, f0, alpha)
                let pb = self.bsdf_pdf(sample_n, v, ld, alpha, ps)
                if pb <= 0.0 { break }
                let corr = self.shading_correction(ng, sample_n, v, ld)
                tp = tp * f * (nl * corr / pb)
                prev_pdf = pb
                delta = 0.0
                if ndiff > self.max_diffuse { break }
                if b >= 2u {
                    let q = min(max(max(tp.x, tp.y), tp.z), 0.95)
                    if r_misc.y >= q { break }
                    tp = tp / q
                }
                rd = ld
                ro = p
                carry = 0.0
                if self.frost_lobe() > 0.5 && far_sheet {
                    hit = self.thin_transmission_trace(p, ng, rd, ti)
                } else {
                    hit = self.spawn_trace(p, ng, rd, self.scene_scale.x * 5000000.0, f32(ti))
                }
            }
            let peak = max(max(lsum.x, lsum.y), lsum.z)
            if self.preview_clamp > 0.0 && peak > self.preview_clamp { lsum = lsum * (self.preview_clamp / peak) }
            var result = PtLightSample.new(lsum)
            result.far = far
            return result
        }

        pixel: fn() {
            // The fragment's own pass-space position (dpi 1): each fragment
            // traces exactly the pixel it covers, independent of how the
            // tile quads were batched or aligned. `tile` only keeps the
            // draw calls apart (uniforms that differ are never batched).
            if self.route_groups() > 0.5 { self.write_group(vec4(0.0, 0.0, 0.0, -1.0)) }
            let pix = floor(self.world.xy)
            let px = u32(pix.x)
            let py = u32(pix.y)
            let auv = (pix + vec2(0.5, 0.5)) * self.inv_res
            var prev = self.accum_tex.sample_nearest(auv)
            if self.reset > 0.5 { prev = vec4(0.0, 0.0, 0.0, 0.0) }
            let n = prev.w
            if self.adaptive_min > 0.0 && n >= self.adaptive_min {
                let m = self.moment_tex.sample_nearest(auv)
                let vr = max(m.y - m.x * m.x, 0.0) / max(n, 1.0)
                if vr < self.adaptive_thresh * (m.x * m.x + 0.001) { return prev }
            }
            let pseed = self.hash2(self.hash2(px, py * 40503u), u32(self.seed))
            var acc = prev.xyz
            var cnt = n
            var samples = 0.0
            var far = vec4(0.0, 0.0, 0.0, 0.0)
            var rejected = 0.0
            var attempt = n
            if self.reset < 0.5 { attempt = attempt + self.rejected_tex.sample_nearest(auv).x }
            let ns = u32(self.spp)
            for s in 0..8 {
                if s >= ns { break }
                let sample_index = u32(attempt)
                let split = self.radiance(px, py, pseed, sample_index)
                var l = split.base
                var next_far = far
                if self.route_groups() > 0.5 {
                    next_far = self.merge_group(far, split.far, self.rand(pseed, sample_index, 160u))
                }
                attempt = attempt + 1.0
                if self.finite3(l) > 0.5 && (self.route_groups() < 0.5 || self.finite3(next_far.xyz) > 0.5) {
                    // Preview-only relative clamp (CPU preview twin agrees):
                    // once a pixel has a mean, no single sample may exceed
                    // five times it. The absolute clamp alone leaves a dark
                    // pixel's late outlier hundreds of times its mean — a
                    // white dot that takes hundreds of frames to fade (the
                    // measured non-converging speckle in shadowed areas).
                    // Commonly-lit pixels relax the limit geometrically and
                    // converge; a pixel lit ONLY by a rare path (arrival
                    // probability well under one in five) can settle at the
                    // clamp floor instead of its true mean — an accepted
                    // preview bias. F12/final keeps the unbiased estimator
                    // (preview_clamp off).
                    if self.preview_clamp > 0.0 && cnt >= 8.0 {
                        let mean_peak = max(max(acc.x, acc.y), acc.z) / max(cnt, 1.0)
                        let limit = max(mean_peak * 5.0, 0.05)
                        let pk = max(max(l.x, l.y), l.z)
                        if pk > limit { l = l * (limit / pk) }
                    }
                    far = next_far
                    samples = samples + 1.0
                    cnt = cnt + 1.0
                    acc = acc + l
                } else {
                    rejected = rejected + 1.0
                }
            }
            // Each entry belongs to this dispatch, including base-only samples.
            if self.route_groups() > 0.5 && samples > 0.0 {
                self.write_group(vec4(far.xyz / samples, far.w))
            }
            if rejected > 0.0 { return vec4(acc, (0.0 - cnt) - 1.0) }
            return vec4(acc, cnt)
        }
    }
    mod.draw.PtCopy = mod.std.set_type_default() do #(DrawCopy::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        src_tex: texture_2d(float)
        reset: uniform(0.0)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        pixel: fn() {
            if self.reset > 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
            return self.src_tex.sample_nearest(self.pos)
        }
    }

    // Every draw in the trace pass writes both attachments, including the
    // full-frame base copy. This layout also works with coupled write masks.
    mod.draw.PtCopyGroups = mod.draw.PtCopy{
        fb1: fragment_output(1, vec4f)
        fragment: fn() {
            self.fb0 = self.pixel()
            self.fb1 = vec4(0.0, 0.0, 0.0, -1.0)
        }
    }

    mod.draw.PtGroupGather = mod.std.set_type_default() do #(DrawGroupGather::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        atlas_tex: texture_2d(float)
        entry_tex: texture_2d(float)
        atlas_size: uniform(vec2f)
        tile_size: uniform(vec2f)
        native_inv: uniform(vec2f)
        cols: uniform(1.0)
        footprint: uniform(1.0)
        pixel: fn() {
            let at = floor(self.rect_pos + self.pos * self.rect_size)
            let cell = self.group_id - 1.0
            let row = floor(cell / self.cols)
            let local = at - vec2(cell - row * self.cols, row) * self.tile_size
            let old = self.atlas_tex.sample_nearest((at + vec2(0.5)) / self.atlas_size)
            let first = floor(local * self.footprint)
            let side = u32(max(self.footprint, 1.0))
            // A complete footprint adds one at every rung. One coarse pixel
            // can cover it all; native pixels each cover 1 / scale² of it.
            let weight = 1.0 / max(1.0, self.footprint * self.footprint)
            var sum = vec3(0.0)
            var w = 0.0
            for y in 0..side {
                for x in 0..side {
                    let q = first + vec2(f32(x), f32(y))
                    if q.x >= self.rung_rect.x && q.y >= self.rung_rect.y && q.x < self.rung_rect.z && q.y < self.rung_rect.w {
                        let e = self.entry_tex.sample_nearest((q + vec2(0.5)) * self.native_inv)
                        if e.w >= 0.0 {
                            w += weight
                            if e.w == self.group_id { sum += e.xyz * weight }
                        }
                    }
                }
            }
            if w == 0.0 { return old }
            return vec4((old.xyz * old.w + sum) / (old.w + w), old.w + w)
        }
    }
    mod.draw.PtGroupAtlasCopy = mod.draw.PtGroupGather{
        pixel: fn() {
            let at = floor(self.rect_pos + self.pos * self.rect_size)
            return self.atlas_tex.sample_nearest((at + vec2(0.5)) / self.atlas_size)
        }
    }
    mod.draw.PtGroupFar = mod.std.set_type_default() do #(DrawGroupFar::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba16F
        alpha_blend: false
        atlas_tex: texture_2d(float)
        gains_tex: texture_2d(float)
        grid: uniform(vec2f)
        gain_size: uniform(vec2f)
        group_count: uniform(0.0)
        pixel: fn() {
            var sum = vec3(0.0)
            var has_samples = 0.0
            let count = u32(self.group_count)
            let cols = u32(self.grid.x)
            let rows = u32(self.grid.y)
            for row in 0..rows {
                for col in 0..cols {
                    let k = row * cols + col
                    if k >= count { break }
                    let tile = self.atlas_tex.sample_nearest((vec2(f32(col), f32(row)) + self.pos) / self.grid)
                    let g = vec2(f32(k % u32(self.gain_size.x)), f32(k / u32(self.gain_size.x)))
                    let gain = self.gains_tex.sample_nearest((g + vec2(0.5)) / self.gain_size).xyz
                    sum += tile.xyz * gain
                    if tile.w > 0.0 { has_samples = 1.0 }
                }
            }
            return vec4(sum, has_samples)
        }
    }

    // A scene-edit hold is a display-space copy, independent of accumulation.
    mod.draw.PtHold = mod.std.set_type_default() do #(DrawHold::script_shader(vm)){
        ..mod.draw.DrawQuad
        alpha_blend: false
        src_tex: texture_2d(float)
        pixel: fn() { return self.src_tex.sample_nearest(self.pos) }
    }

    // The display's linear memory. Chooses, per native pixel, the current
    // rung's accumulated linear mean (bilinearly upsampled while coarse) or
    // last frame's carried linear value, in LINEAR radiance. The tonemap
    // then exposes the whole frame with ONE metered exposure — held and
    // fresh pixels alike — so a changing meter can never bake mismatched
    // brightness into held tiles. (The old design held POST-tonemap sRGB
    // pixels: during the early frames of a restart the 4x4 meter has too few
    // valid probes and swings, and every held tile kept the exposure it was
    // stamped with — the lingering dark/flat blocks after quick successive
    // restarts.)
    mod.draw.PtCarry = mod.std.set_type_default() do #(DrawCarry::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        src_tex: texture_2d(float)
        hold_tex: texture_2d(float)
        rej_tex: texture_2d(float)
        inv_res: uniform(vec2f)
        src_res: uniform(vec2f)
        coarse: uniform(0.0)
        hold_on: uniform(0.0)
        src_is_mean: uniform(0.0)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        // Bilinear over the coarse rung's per-texel means; a texel the sweep
        // has not reached carries no weight, so holes never bleed black in.
        coarse_mean: fn(p: vec2) -> vec4 {
            let f = p * self.src_res - vec2(0.5, 0.5)
            let i0 = floor(f)
            let fr = f - i0
            var sum = vec3(0.0, 0.0, 0.0)
            var wsum = 0.0
            var centre_ok = 0.0
            for j in 0..2 {
                for i in 0..2 {
                    let t = clamp(i0 + vec2(f32(i), f32(j)), vec2(0.0, 0.0), self.src_res - vec2(1.0, 1.0))
                    let c = self.src_tex.sample_nearest((t + vec2(0.5, 0.5)) * self.inv_res)
                    if c.w >= 0.5 {
                        let wx = if f32(i) > 0.5 { fr.x } else { 1.0 - fr.x }
                        let wy = if f32(j) > 0.5 { fr.y } else { 1.0 - fr.y }
                        let wgt = wx * wy + 0.0001
                        var linear = c.xyz
                        if self.src_is_mean < 0.5 { linear = c.xyz / max(c.w, 1.0) }
                        sum = sum + linear * wgt
                        wsum = wsum + wgt
                        centre_ok = 1.0
                    }
                }
            }
            if centre_ok < 0.5 || wsum <= 0.0 { return vec4(0.0, 0.0, 0.0, 0.0) }
            return vec4(sum / wsum, 1.0)
        }
        pixel: fn() {
            if self.coarse > 0.5 {
                let c = self.coarse_mean(self.pos)
                if c.w >= 0.5 { return c }
            } else {
                let c = self.src_tex.sample_nearest(self.pos)
                if c.w >= 0.5 {
                    var linear = c.xyz
                    if self.src_is_mean < 0.5 { linear = c.xyz / max(c.w, 1.0) }
                    return vec4(linear, 1.0)
                }
            }
            if self.hold_on > 0.5 {
                // A native pixel whose every attempted sample was rejected
                // (NaN or traversal overflow) would otherwise hold stale
                // coarse content forever; past eight rejected attempts the
                // hold drops and the raster shows through instead.
                var chronic = 0.0
                if self.coarse < 0.5 {
                    if self.rej_tex.sample_nearest(self.pos).x >= 8.0 {
                        let cur = self.src_tex.sample_nearest(self.pos)
                        if cur.w < 0.5 { chronic = 1.0 }
                    }
                }
                if chronic < 0.5 {
                    let held = self.hold_tex.sample_nearest(self.pos)
                    if held.w > 0.5 { return held }
                }
            }
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.draw.PtResolve = mod.std.set_type_default() do #(DrawResolve::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        src_tex: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        pixel: fn() {
            let c = self.src_tex.sample_nearest(self.pos)
            if c.w < 0.0 { return vec4(c.xyz, (0.0 - c.w) - 1.0) }
            return c
        }
    }

    mod.draw.PtReject = mod.std.set_type_default() do #(DrawReject::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        src_tex: texture_2d(float)
        prev_tex: texture_2d(float)
        reset: uniform(0.0)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        pixel: fn() {
            var n = self.prev_tex.sample_nearest(self.pos).x
            if self.reset > 0.5 { n = 0.0 }
            let c = self.src_tex.sample_nearest(self.pos)
            if c.w < 0.0 { n = n + 1.0 }
            return vec4(n, 0.0, 0.0, 1.0)
        }
    }

    mod.draw.PtMoments = mod.std.set_type_default() do #(DrawMoments::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        prev_tex: texture_2d(float)
        cur_tex: texture_2d(float)
        mom_tex: texture_2d(float)
        reset: uniform(0.0)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        pixel: fn() {
            var prev = self.prev_tex.sample_nearest(self.pos)
            let cur = self.cur_tex.sample_nearest(self.pos)
            var m = self.mom_tex.sample_nearest(self.pos)
            if self.reset > 0.5 {
                prev = vec4(0.0, 0.0, 0.0, 0.0)
                m = vec4(0.0, 0.0, 0.0, 0.0)
            }
            if cur.w > prev.w {
                let dn = cur.w - prev.w
                let s = (cur.xyz - prev.xyz) / dn
                let l = dot(s, vec3(0.2126, 0.7152, 0.0722))
                let n = m.z + 1.0
                m = vec4(m.x + (l - m.x) / n, m.y + (l * l - m.y) / n, n, 0.0)
            }
            return m
        }
    }

    // PtTrace with correctly rounded division, roots and vector forms: a sample is the CPU
    // twin's to the bit. It traces about 11% fewer paths per second, so only
    // `RenderSettings::exact` (the parity selftest, a host's Exact setting) draws it.
    mod.draw.PtTraceExact = mod.draw.PtTrace{precise_math: true}
    mod.draw.PtTraceFrost = mod.draw.PtTrace{frost_lobe: fn() -> f32 { return 1.0 }}
    mod.draw.PtTraceFrostExact = mod.draw.PtTraceFrost{precise_math: true}

    mod.draw.PtTraceGroups = mod.draw.PtTrace{
        fb1: fragment_output(1, vec4f)
        route_groups: fn() -> f32 { return 1.0 }
        write_group: fn(entry: vec4) { self.fb1 = entry }
        // Clamp in reference space before reservoir weighting: clamping the
        // survivor would discard the energy its inverse probability restores.
        group_term: fn(term: vec3, id: f32) -> vec4 {
            if self.finite3(term) < 0.5 { return vec4(1000000000000000000000000000000.0, 0.0, 0.0, id) }
            let peak = max(max(term.x, term.y), term.z)
            if self.preview_clamp > 0.0 && peak > self.preview_clamp {
                return vec4(term * (self.preview_clamp / peak), id)
            }
            return vec4(term, id)
        }

        // One representative group, with every group's expectation preserved.
        // Separate hash dimensions leave the path's Sobol samples untouched.
        merge_group: fn(a: vec4, b: vec4, r: f32) -> vec4 {
            if self.finite3(a.xyz) < 0.5 || self.finite3(b.xyz) < 0.5 {
                return vec4(1000000000000000000000000000000.0, 0.0, 0.0, 0.0)
            }
            if b.w == 0.0 { return a }
            if a.w == 0.0 { return b }
            if a.w == b.w { return vec4(a.xyz + b.xyz, a.w) }
            let wa = max(max(max(a.x, a.y), a.z), 0.00000000000000000001)
            let wb = max(max(max(b.x, b.y), b.z), 0.00000000000000000001)
            let pa = wa / (wa + wb)
            let pb = wb / (wa + wb)
            if r < pb { return vec4(b.xyz / pb, b.w) }
            return vec4(a.xyz / pa, a.w)
        }

    }
    mod.draw.PtTraceGroupsExact = mod.draw.PtTraceGroups{precise_math: true}
    mod.draw.PtTraceGroupsFrost = mod.draw.PtTraceGroups{frost_lobe: fn() -> f32 { return 1.0 }}
    mod.draw.PtTraceGroupsFrostExact = mod.draw.PtTraceGroupsFrost{precise_math: true}

    mod.draw.PtGuide = mod.std.set_type_default() do #(DrawGuide::script_shader(vm)){
        // Inherit the exact traversal/offset/material helpers used by transport.
        ..mod.draw.PtTrace
        pixel: fn() {
            let g = self.gbuf_tex.sample_nearest(self.pos)
            if g.x < 0.0 { return vec4(0.0, 0.0, 0.0, -1.0) }
            let ndc = vec2((self.world.x + self.jitter.x) * self.inv_res.x * 2.0 - 1.0, 1.0 - (self.world.y + self.jitter.y) * self.inv_res.y * 2.0)
            var rd = normalize(self.cam_fwd + self.cam_right * (ndc.x * self.cam_tan.x) + self.cam_up * (ndc.y * self.cam_tan.y))
            if self.ortho.x > 0.0 { rd = self.cam_fwd }
            let hit = self.guide_hit(vec4(g.w, g.x, g.y, g.z), rd)
            if hit.y < 0.0 { return vec4(0.0, 0.0, 0.0, -1.0) }
            let ti = u32(hit.y)
            let a0 = self.fetch_attr(ti * 4u)
            let a1 = self.fetch_attr(ti * 4u + 1u)
            let a2 = self.fetch_attr(ti * 4u + 2u)
            let w0 = 1.0 - hit.z - hit.w
            let n = normalize(a0.xyz * w0 + a1.xyz * hit.z + a2.xyz * hit.w)
            var depth = g.w
            if hit.y != g.x {
                let t0 = self.fetch_tri(ti * 3u)
                let t1 = self.fetch_tri(ti * 3u + 1u)
                let t2 = self.fetch_tri(ti * 3u + 2u)
                let p = t0.xyz * w0 + t1.xyz * hit.z + t2.xyz * hit.w
                depth = dot(p - self.cam_pos, self.cam_fwd)
            }
            return vec4(n, depth)
        }
    }

    mod.draw.PtAtrous = mod.std.set_type_default() do #(DrawAtrous::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rgba32F
        alpha_blend: false
        src_tex: texture_2d(float)
        guide_tex: texture_2d(float)
        mom_tex: texture_2d(float)
        inv_res: uniform(vec2f)
        step: uniform(1.0)
        sigma_l: uniform(4.0)
        geom_weight: uniform(1.0)
        scene_scale: uniform(0.0)
        // Absolute luminance sigma (radiance units), fading as 1/sqrt(N):
        // at one sample per pixel the moment variance is identically zero
        // and the edge-stopping weight otherwise refuses to smooth at all —
        // which is exactly the salt-and-pepper the coarse rungs must not
        // show. Converged pixels leave the variance term in charge.
        sigma_floor: uniform(0.0)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        bspline: fn(i: u32) -> f32 {
            if i == 2u { return 0.375 }
            if i == 1u || i == 3u { return 0.25 }
            return 0.0625
        }
        pixel: fn() {
            // Pass-space pixel coordinates, so the quad can cover only the
            // coarse rung's sub-rect and still address the right texels.
            let uv = self.world.xy * self.inv_res
            let c = self.src_tex.sample_nearest(uv)
            let g = self.guide_tex.sample_nearest(uv)
            let m = self.mom_tex.sample_nearest(uv)
            let n = max(c.w, 1.0)
            let cc = if self.step < 1.5 { c.xyz / n } else { c.xyz }
            let sd = sqrt(max(m.y - m.x * m.x, 0.0) / n)
            let lc = dot(cc, vec3(0.2126, 0.7152, 0.0722))
            let inv_l = 1.0 / (self.sigma_l * sd + self.sigma_floor / sqrt(n) + 0.0005)
            var sum = cc
            var wsum = 1.0
            var nb = vec3(0.0, 0.0, 0.0)
            var nbw = 0.0
            for j in 0..5 {
                for i in 0..5 {
                    if i == 2u && j == 2u { continue }
                    let off = vec2(f32(i) - 2.0, f32(j) - 2.0) * (self.step * self.inv_res)
                    let q = self.src_tex.sample_nearest(uv + off)
                    let gq = self.guide_tex.sample_nearest(uv + off)
                    if q.w < 0.5 { continue }
                    let qc = if self.step < 1.5 { q.xyz / max(q.w, 1.0) } else { q.xyz }
                    let lq = dot(qc, vec3(0.2126, 0.7152, 0.0722))
                    var w = self.bspline(i) * self.bspline(j)
                    if self.geom_weight > 0.5 {
                        if g.w < 0.0 {
                            if gq.w >= 0.0 { w = 0.0 }
                        } else {
                            if gq.w < 0.0 { w = 0.0 }
                            let wn = pow(max(dot(g.xyz, gq.xyz), 0.0), 32.0)
                            let wz = exp((0.0 - abs(g.w - gq.w)) / (abs(g.w) * 0.03 * self.step + self.scene_scale * 0.0000005))
                            w = w * wn * wz
                        }
                    }
                    // The plain (edge-aware, luminance-blind) neighbourhood:
                    // the firefly test below needs a mean the outlier itself
                    // cannot veto through the luminance edge-stopper.
                    nb = nb + qc * w
                    nbw = nbw + w
                    w = w * exp((0.0 - abs(lq - lc)) * inv_l)
                    sum = sum + qc * w
                    wsum = wsum + w
                }
            }
            // Fireflies survive an edge-aware filter by definition — every
            // neighbour weight collapses against a 10x-brighter centre, and
            // the spike passes through as an "edge". On the first pass,
            // a centre far above its neighbourhood mean IS the spike (one
            // clamped high-energy path, not detail): show the neighbourhood
            // instead. Display-side only — the accumulation keeps every
            // sample, and once samples pile up the centre stops tripping
            // the test and the estimator's own mean shows unfiltered.
            if self.step < 1.5 && nbw > 0.5 {
                let nbm = nb / nbw
                let nbl = dot(nbm, vec3(0.2126, 0.7152, 0.0722))
                if lc > 4.0 * nbl + 0.02 {
                    return vec4(nbm, c.w)
                }
            }
            return vec4(sum / wsum, c.w)
        }
    }

    mod.draw.PtTonemap = mod.std.set_type_default() do #(DrawTonemap::script_shader(vm)){
        ..mod.draw.DrawQuad
        src_tex: texture_2d(float)
        mom_tex: texture_2d(float)
        gbuf_tex: texture_2d(float)
        attr_tex: texture_2d(float)
        tri_tex: texture_2d(float)
        mat_tex: texture_2d(float)
        hold_tex: texture_2d(float)
        far_tex: texture_2d(float)
        guide_tex: texture_2d(float)
        far_size: uniform(vec2f)
        far_scale: uniform(0.0)
        guide_on: uniform(0.0)
        far_light: fn() -> vec3 {
            let native = self.pos / self.inv_res
            let grid = native / self.far_scale - vec2(0.5)
            let f = fract(grid)
            var guide = vec4(0.0)
            if self.guide_on > 0.5 { guide = self.guide_tex.sample_nearest(self.pos) }
            var sum = vec3(0.0)
            var weights = 0.0
            // The guide can reject all four taps: a wall seen edge-on, a small
            // part at another depth, detail seen through a window. Such a pixel
            // takes the taps' plain bilinear blend; it never drops the light.
            var plain = vec3(0.0)
            var plain_weights = 0.0
            for y in 0..2 {
                for x in 0..2 {
                    let cell = clamp(floor(grid) + vec2(f32(x), f32(y)), vec2(0.0), self.far_size - vec2(1.0))
                    let light = self.far_tex.sample_nearest((cell + vec2(0.5)) / self.far_size)
                    if light.w > 0.0 {
                        let bilinear = mix(1.0 - f.x, f.x, f32(x)) * mix(1.0 - f.y, f.y, f32(y))
                        var w = bilinear
                        if self.guide_on > 0.5 {
                            let lo = cell * self.far_scale
                            let hi = min(lo + vec2(self.far_scale), vec2(1.0) / self.inv_res)
                            let g = self.guide_tex.sample_nearest((lo + hi) * 0.5 * self.inv_res)
                            if (g.w < 0.0) != (guide.w < 0.0) { w = 0.0 }
                            if g.w >= 0.0 && guide.w >= 0.0 {
                                let wn = max(dot(g.xyz, guide.xyz), 0.0)
                                w *= wn * wn * exp(-abs(g.w - guide.w) / max(abs(guide.w) * 0.02, 0.000001))
                            }
                        }
                        sum += light.xyz * w
                        weights += w
                        plain += light.xyz * bilinear
                        plain_weights += bilinear
                    }
                }
            }
            if weights > 0.0 { return sum / weights }
            if plain_weights > 0.0 { return plain / plain_weights }
            return vec3(0.0)
        }
        inv_res: uniform(vec2f)
        exposure: uniform(1.0)
        sky_display: uniform(0.0)
        view_mode: uniform(0.0)
        attr_inv: uniform(vec2f)
        tri_inv: uniform(vec2f)
        mat_inv: uniform(vec2f)
        light_dir: uniform(vec3f)
        gbuf_on: uniform(0.0)
        src_is_mean: uniform(0.0)
        untraced_transparent: uniform(0.0)
        // Resolution ladder: the accumulation of the current rung occupies
        // the top-left `src_res` texels of the native-size buffer. `coarse`
        // is on below native; the display then bilinearly upsamples per-texel
        // means. `hold_on` keeps last frame's finished display pixels under
        // any pixel the current rung has not reached yet, so a rung replaces
        // the previous picture tile by tile with no flash back to the raster.
        src_res: uniform(vec2f)
        coarse: uniform(0.0)
        hold_on: uniform(0.0)
        tx: fn(i: u32, inv: vec2) -> vec2 {
            return (vec2(f32(i & 2047u), f32(i >> 11u)) + vec2(0.5, 0.5)) * inv
        }
        // A pixel no tile has reached yet shows the lit G-buffer (albedo x
        // a hemisphere lambert), never black — the preview reads as the
        // model from its first frame and the tiles replace it as they land.
        fallback: fn() -> vec3 {
            if self.gbuf_on < 0.5 { return vec3(0.07, 0.07, 0.08) }
            let g = self.gbuf_tex.sample_nearest(self.pos)
            if g.x < 0.0 { return vec3(0.16, 0.18, 0.22) }
            let ti = u32(g.x)
            let a0 = self.attr_tex.sample_nearest(self.tx(ti * 4u, self.attr_inv))
            let a1 = self.attr_tex.sample_nearest(self.tx(ti * 4u + 1u, self.attr_inv))
            let a2 = self.attr_tex.sample_nearest(self.tx(ti * 4u + 2u, self.attr_inv))
            let w0 = 1.0 - g.y - g.z
            let n = normalize(a0.xyz * w0 + a1.xyz * g.y + a2.xyz * g.z)
            let mi = u32(self.tri_tex.sample_nearest(self.tx(ti * 3u, self.tri_inv)).w)
            let albedo = self.mat_tex.sample_nearest(self.tx(mi * u32(#(crate::pack::MATERIAL_ROWS as f32)), self.mat_inv)).xyz
            let l = abs(dot(n, self.light_dir))
            return albedo * (0.35 + 0.65 * l)
        }
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        aces: fn(x: vec3) -> vec3 {
            let a = 2.51
            let b = 0.03
            let c = 2.43
            let d = 0.59
            let e = 0.14
            return clamp((x * (x * a + vec3(b, b, b))) / (x * (x * c + vec3(d, d, d)) + vec3(e, e, e)), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        // Over-range colour keeps its hue. The curve per channel drives every
        // channel of a far over-range saturated colour toward 1, so a bright
        // crimson emitter shows magenta and a bright orange one yellow. Past
        // the display range the peak channel takes the curve and the others
        // keep their ratio to it; in range the per-channel curve is unchanged.
        display: fn(x: vec3) -> vec3 {
            let per_channel = self.aces(x)
            let peak = max(max(x.x, x.y), x.z)
            if peak <= 1.0 { return per_channel }
            let kept = x * (self.aces(vec3(peak, peak, peak)).x / peak)
            return mix(per_channel, kept, smoothstep(1.0, 2.0, peak))
        }
        // A fixed 4x4 log-average meter is robust to a bright doorway or a
        // few fireflies and costs no readback. It only lifts dark frames:
        // calibrated daylight at EV 0 is never dimmed. The 16x ceiling maps
        // a 0.005-radiance room to a visible mid-shadow while retaining the
        // existing exposure control as a creative offset.
        metered_exposure: fn() -> f32 {
            var sum_log = 0.0
            var valid = 0.0
            for y in 0..4 {
                for x in 0..4 {
                    let uv = vec2((f32(x) + 0.5) * 0.25, (f32(y) + 0.5) * 0.25)
                    let c = self.hold_tex.sample_nearest(uv)
                    if c.w >= 0.5 {
                        let luminance = max(dot(c.xyz, vec3(0.2126, 0.7152, 0.0722)), 0.0001)
                        sum_log = sum_log + log(luminance)
                        valid = valid + 1.0
                    }
                }
            }
            if valid < 8.0 { return self.exposure }
            let log_average = exp(sum_log / valid)
            return self.exposure * clamp(0.08 / log_average, 1.0, 16.0)
        }
        // Bilinear over the coarse rung's per-texel MEANS; a texel the sweep
        // has not reached carries no weight, so holes never bleed black in.
        coarse_mean: fn(p: vec2) -> vec4 {
            let f = p * self.src_res - vec2(0.5, 0.5)
            let i0 = floor(f)
            let fr = f - i0
            var sum = vec3(0.0, 0.0, 0.0)
            var wsum = 0.0
            var centre_ok = 0.0
            for j in 0..2 {
                for i in 0..2 {
                    let t = clamp(i0 + vec2(f32(i), f32(j)), vec2(0.0, 0.0), self.src_res - vec2(1.0, 1.0))
                    let c = self.src_tex.sample_nearest((t + vec2(0.5, 0.5)) * self.inv_res)
                    if c.w >= 0.5 {
                        let wx = if f32(i) > 0.5 { fr.x } else { 1.0 - fr.x }
                        let wy = if f32(j) > 0.5 { fr.y } else { 1.0 - fr.y }
                        let wgt = wx * wy + 0.0001
                        sum = sum + (c.xyz / max(c.w, 1.0)) * wgt
                        wsum = wsum + wgt
                        centre_ok = 1.0
                    }
                }
            }
            if centre_ok < 0.5 || wsum <= 0.0 { return vec4(0.0, 0.0, 0.0, 0.0) }
            return vec4(sum / wsum, 1.0)
        }
        pixel: fn() {
            if self.view_mode > 0.5 {
                let duv = self.pos * (self.src_res * self.inv_res)
                let c = self.src_tex.sample_nearest(duv)
                if self.view_mode < 1.5 {
                    let s = clamp(c.w / 256.0, 0.0, 1.0)
                    return vec4(s, 1.0 - abs(s - 0.5) * 2.0, 1.0 - s, 1.0)
                }
                let m = self.mom_tex.sample_nearest(duv)
                let vr = sqrt(max(m.y - m.x * m.x, 0.0) / max(c.w, 1.0)) / (abs(m.x) + 0.05)
                let s = clamp(vr * 4.0, 0.0, 1.0)
                return vec4(s, s, s, 1.0)
            }
            // The carried LINEAR display (PtCarry): the current rung where
            // it has data, last frame's carried value elsewhere. One metered
            // exposure below applies to all of it alike — held pixels can
            // never keep a stale exposure.
            let c = self.hold_tex.sample_nearest(self.pos)
            if c.w < 0.5 {
                // Interactive hosts composite the trace OVER their own
                // realtime frame: an untraced pixel is transparent so the
                // raster shows until a tile lands. F12/track keep the lit
                // G-buffer fallback (they draw on a flat editor background).
                if self.untraced_transparent > 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
                let f = self.fallback()
                let m = self.display(f * self.exposure)
                return vec4(pow(m, vec3(0.4545454, 0.4545454, 0.4545454)), 1.0)
            }
            var linear = c.xyz
            if self.far_scale > 0.0 { linear += self.far_light() }
            // The engine sky already owns its display transform. A primary
            // miss therefore passes through unchanged; surfaces retain the
            // tracer's exposure and output curve.
            if self.sky_display > 0.5 {
                let g = self.gbuf_tex.sample_nearest(self.pos)
                if g.x < 0.0 {
                    return vec4(clamp(linear, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), 1.0)
                }
            }
            let mapped = self.display(linear * self.metered_exposure())
            let srgb = pow(mapped, vec3(0.4545454, 0.4545454, 0.4545454))
            return vec4(srgb, 1.0)
        }
    }

    mod.draw.PtView = mod.std.set_type_default() do #(DrawView::script_shader(vm)){
        ..mod.draw.DrawQuad
        view_tex: texture_2d(float)
        pixel: fn() {
            return self.view_tex.sample(self.pos)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGbuf {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub view_proj: Mat4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTrace {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCopy {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGroupGather {
    #[deref] pub draw_super: DrawQuad,
    #[live] pub rung_rect: Vec4f,
    #[live] pub group_id: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGroupFar {
    #[deref] pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawHold {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawResolve {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCarry {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawReject {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMoments {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawGuide {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAtrous {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTonemap {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawView {
    #[deref]
    pub draw_super: DrawQuad,
}

/// What the settings panel edits. Any change restarts accumulation.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    /// Stop after this many samples per pixel (0 = never).
    pub target_spp: u32,
    pub max_bounces: u32,
    pub max_diffuse: u32,
    /// Explicit biased firefly suppression for interactive previews. `None`
    /// is the unbiased reference/final estimator.
    pub preview_clamp: Option<f32>,
    pub exposure: f32,
    pub denoise: bool,
    pub denoise_sigma: f32,
    /// Adaptive sampling: pixels stop after `adaptive_min` samples once
    /// their relative variance drops below `adaptive_thresh`. 0 = off.
    pub adaptive_min: u32,
    pub adaptive_thresh: f32,
    /// Requested trace GPU time per host frame (seconds). The scheduler uses
    /// the smaller of this and `MAKEPAD_PT_BUDGET_MS` (default 4 ms).
    pub frame_budget: f64,
    /// 0 image, 1 spp heatmap, 2 relative noise.
    pub view_mode: u32,
    /// 0 normal, 1 shading normals, 2 albedo (primary hit only).
    pub debug_mode: u32,
    /// Diagnostic: BSDF-sampling-only transport (no NEE, no MIS).
    pub brute: bool,
    /// Diagnostic: with debug_mode 6, return (hit tri, t, tp.x) entering this bounce.
    pub dbg_b: f32,
    /// Rasterize the primary hit (pinhole only; forced off with bokeh).
    pub hybrid_primary: bool,
    /// Traversal step bound per ray (nodes visited); the CPU twin uses the
    /// same bound so a capped ray is capped on both sides.
    pub max_steps: u32,
    /// Shadow-ray self-occlusion skin in scene units: next-event rays ignore
    /// blockers closer than this to the surface they leave (stacked
    /// nearly coincident construction layers otherwise black each other out).
    /// Negative = automatic ([`crate::pack::PackedScene::auto_shadow_skin`]);
    /// 0 disables the skin (unbiased reference).
    pub shadow_skin: f32,
    /// Untraced pixels come out transparent (alpha 0) instead of the lit
    /// G-buffer fallback, so a host can composite the accumulation over its
    /// own realtime frame. Display-only; does not invalidate samples.
    pub untraced_transparent: bool,
    /// The resolution ladder: the first pass covers the WHOLE frame at a
    /// coarse fraction of native (upscaled for display), then each rung
    /// doubles the resolution — full frame per rung — until native, and only
    /// then samples accumulate. A complete traced picture lands within the
    /// first few budgeted dispatches instead of a spiral of tiles crawling
    /// across an empty frame. Off = native-only (headless/F12/parity runs).
    pub progressive: bool,
    /// Trace with `mod.draw.PtTraceExact`: correctly rounded arithmetic, so every sample is
    /// the CPU twin's to the bit, at about 11% of the throughput (1080p, 16 spp: 0.96
    /// against 1.07-1.10 Mpaths/s). Off = the fast trace, whose samples differ from the
    /// twin's by rounding.
    pub exact: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            target_spp: 1024,
            max_bounces: 8,
            max_diffuse: 4,
            preview_clamp: None,
            exposure: 1.0,
            denoise: false,
            denoise_sigma: 4.0,
            // Off: a finite target_spp must mean that many samples in every
            // pixel. Adaptive stopping froze easy sunlit walls at ~64 spp
            // while the host spp counter kept climbing.
            adaptive_min: 0,
            adaptive_thresh: 0.0004,
            frame_budget: 0.008,
            view_mode: 0,
            debug_mode: 0,
            brute: false,
            dbg_b: -1.0,
            // The reference path uses per-pixel scrambled primary jitter.
            // Hybrid raster primaries remain an explicit preview option.
            hybrid_primary: false,
            max_steps: crate::bvh::MAX_STEPS,
            shadow_skin: -1.0,
            untraced_transparent: false,
            progressive: false,
            exact: false,
        }
    }
}

/// Live numbers for the status bar / report.
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    pub width: usize,
    pub height: usize,
    /// Average samples issued per pixel. With adaptive off and a complete
    /// tile sweep, this equals the per-pixel accumulation count (`accum.w`).
    pub spp: f32,
    pub elapsed: f64,
    pub samples_total: f64,
    pub samples_per_sec: f64,
    pub frames: u32,
    /// Tile edge in pixels and tiles queued last frame.
    pub tile_edge: u32,
    pub tiles: u32,
    /// Resolution-ladder rung: 0 = native, k = tracing at native >> k.
    pub rung_shift: u32,
    pub last_frame_ms: f64,
    /// Latest completed trace command buffer and its hard target. Zero GPU
    /// samples means the backend has not reported one yet.
    pub gpu_time_ms: f64,
    pub gpu_budget_ms: f64,
    pub gpu_samples: u64,
    pub tri_count: usize,
    pub bvh_nodes: usize,
    pub bvh_depth: u32,
    pub done: bool,
    pub light_groups: u32,
    pub tile_scale: u32,
    pub gain_uploads: u64,
}

/// The group footprint grows with the square root of the group count.
fn group_scale(n: usize) -> u32 {
    if n == 0 { return 0; }
    ((n as f64).sqrt().ceil() as u32).next_power_of_two().max(4)
}

struct GroupTargets {
    tile: (usize, usize),
    grid: (usize, usize),
    atlas: [Texture; 2],
    atlas_ping: usize,
    /// Previous gather footprints in atlas texels, relative to each group's origin.
    last_rects: Vec<Rect>,
    far: Texture,
}

impl GroupTargets {
    fn new(cx: &mut Cx, size: (usize, usize), count: usize, scale: usize) -> Self {
        let tile = (size.0.div_ceil(scale), size.1.div_ceil(scale));
        let cols = (count as f64).sqrt().ceil() as usize;
        let grid = (cols, count.div_ceil(cols));
        let atlas = std::array::from_fn(|_| Texture::new_with_format(cx, TextureFormat::RenderRGBAf32 {
            size: TextureSize::Fixed { width: tile.0 * grid.0, height: tile.1 * grid.1 }, initial: true,
        }));
        let far = Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 {
            size: TextureSize::Fixed { width: tile.0, height: tile.1 }, initial: true,
        });
        Self { tile, grid, atlas, atlas_ping: 0, last_rects: Vec::new(), far }
    }
}

struct Stage {
    pass: DrawPass,
    draw_list: DrawList,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GpuBudgetUpdate {
    tiles: u32,
    next_edge: u32,
    under_budget: u32,
}

/// The tile edge within [TILE_MIN, TILE_MAX]. Light groups pass their tile scale (at least 8) so
/// every traced tile is whole atlas footprints; ungrouped scenes pass 1 and keep the plain clamp.
fn aligned_edge(edge: u32, alignment: u32) -> u32 {
    if alignment <= 1 {
        return edge.clamp(TILE_MIN, TILE_MAX);
    }
    let lo = TILE_MIN.next_multiple_of(alignment);
    let hi = (TILE_MAX / alignment * alignment).max(lo);
    (edge / alignment * alignment).clamp(lo, hi)
}

fn adapt_gpu_budget(
    tiles: u32,
    next_edge: u32,
    under_budget: u32,
    submitted_tiles: u32,
    submitted_edge: u32,
    gpu_ms: f64,
    budget_ms: f64,
    alignment: u32,
) -> GpuBudgetUpdate {
    let mut out = GpuBudgetUpdate {
        tiles: tiles.clamp(1, TILES_PER_FRAME_MAX),
        next_edge: aligned_edge(next_edge, alignment),
        under_budget,
    };
    if submitted_tiles == 0
        || !gpu_ms.is_finite()
        || !budget_ms.is_finite()
        || gpu_ms <= 0.0
        || budget_ms <= 0.0
    {
        return out;
    }

    // Aim at 80% of the hard cap. The spare 20% absorbs scene/path variance
    // and the fixed full-frame copy in the same trace command buffer.
    let ratio = budget_ms * 0.8 / gpu_ms;
    let desired_tiles = ((submitted_tiles as f64 * ratio).floor() as u32)
        .clamp(1, TILES_PER_FRAME_MAX);
    if gpu_ms > budget_ms || desired_tiles < out.tiles {
        out.tiles = out.tiles.min(desired_tiles);
        if submitted_tiles == 1 && gpu_ms > budget_ms {
            let scaled = submitted_edge as f64 * ratio.max(0.01).sqrt();
            out.next_edge = aligned_edge(scaled.floor() as u32 / 8 * 8, alignment);
        }
        out.under_budget = 0;
        return out;
    }

    if gpu_ms < budget_ms * 0.65 {
        out.under_budget = out.under_budget.saturating_add(1);
        if out.under_budget >= 2 {
            out.under_budget = 0;
            if out.tiles < TILES_PER_FRAME_MAX {
                let step = (out.tiles / 2).max(1);
                out.tiles = desired_tiles.min(out.tiles.saturating_add(step));
            } else if ratio > 1.25 {
                let scaled = submitted_edge as f64 * ratio.sqrt();
                out.next_edge = aligned_edge(scaled.floor() as u32 / 8 * 8, alignment);
            }
        }
    } else {
        out.under_budget = 0;
    }
    out
}

/// Ladder adjustment for a frame on which the trace pass has not reported a
/// completion time since the last restart. `timing_ever` = the backend HAS
/// reported for this tracer before (Metal): hold the earned ladder and wait —
/// the host repaint interval proves nothing about GPU cost. On a backend
/// that never reports, the interval is the only clock, and it only means
/// something on a frame that actually traced (`dispatched`): an idle frame
/// (paused, done, host skip) must leave the earned ladder alone — resetting
/// there is what collapsed the tile after "done", so raising the sample
/// limit restarted the spiral at a tiny tile instead of continuing.
fn adapt_without_timing(
    tiles: u32,
    next_edge: u32,
    under_budget: u32,
    timing_ever: bool,
    dispatch_frames: u32,
    dispatched: bool,
    dt_ms: f64,
    budget_ms: f64,
    alignment: u32,
) -> GpuBudgetUpdate {
    let mut out = GpuBudgetUpdate {
        tiles: tiles.clamp(1, TILES_PER_FRAME_MAX),
        next_edge: aligned_edge(next_edge, alignment),
        under_budget,
    };
    if timing_ever || !dispatched {
        return out;
    }
    if dispatch_frames < 30 {
        // Bring-up: one conservative startup tile until the backend either
        // reports once or proves it never will. Do not infer headroom from
        // a fast CPU repaint.
        out.tiles = 1;
        out.next_edge = aligned_edge(out.next_edge.min(TILE_START), alignment);
        out.under_budget = 0;
        return out;
    }
    if dt_ms > (budget_ms.max(4.0) * 1.25).max(40.0) {
        // A late host frame is the only overrun signal left; halve. The
        // 40 ms floor keeps a host that deliberately ticks its accumulation
        // at ~30 Hz (the interactive pane) from reading its own tick as an
        // overrun.
        out.next_edge = aligned_edge(out.next_edge / 2, alignment);
        out.tiles = (out.tiles / 2).max(1);
        out.under_budget = 0;
        return out;
    }
    out.under_budget = out.under_budget.saturating_add(1);
    if out.under_budget >= 2 {
        out.under_budget = 0;
        if out.next_edge < TILE_MAX {
            out.next_edge = aligned_edge(out.next_edge * 3 / 2 / 8 * 8, alignment);
        } else if out.tiles < TILES_PER_FRAME_MAX {
            out.tiles = (out.tiles * 3 / 2).max(out.tiles + 1).min(TILES_PER_FRAME_MAX);
        }
    }
    out
}

fn should_dispatch_trace(done: bool, paused: bool, skip_trace: bool, has_sweep: bool) -> bool {
    !done && !paused && !skip_trace && has_sweep
}

fn reached_sample_limit(target_spp: u32, spp: f32) -> bool {
    target_spp > 0 && spp >= target_spp as f32
}

/// The hold decision has no GPU state. A repeated restart keeps the original
/// picture; a completed startup sweep switches presentation to the new trace.
#[derive(Default)]
struct PresentationHold {
    holding: bool,
}

impl PresentationHold {
    fn restart(&mut self, hold: bool, has_picture: bool) -> bool {
        let capture = hold && has_picture && !self.holding;
        self.holding = hold && (self.holding || has_picture);
        capture
    }

    fn advance(&mut self, samples: f32, threshold: f32) -> bool {
        if self.holding && samples >= threshold {
            self.holding = false;
            return true;
        }
        false
    }
}

struct HeldPicture {
    texture: Texture,
    size: (usize, usize),
    source: Option<Texture>,
    stage: Stage,
    recorded: Option<u64>,
}

/// Everything the renderer keeps between frames.
pub struct RayTracer {
    packed: Option<Arc<PackedScene>>,
    group_targets: Option<GroupTargets>,
    gains_tex: Texture,
    gain_indices: std::collections::HashMap<u32, usize>,
    gain_values: Vec<f32>,
    gains_dirty: bool,
    // Recording and paint are distinct. Hosts can measure when a gain upload reached a view.
    recorded_gains: u64,
    presented_gains: u64,
    sky: SkyUniforms,
    environment: Option<(Arc<EnvironmentLight>, EnvironmentFrame)>,
    camera: crate::scene::Camera,
    tex_tri: Texture,
    tex_attr: Texture,
    tex_bvh: Texture,
    tex_mat: Texture,
    tex_light: Texture,
    tex_atlas: Texture,
    tex_env: Texture,
    raster_geom: Option<Geometry>,
    size: (usize, usize),
    target_size: (usize, usize),
    accum: Vec<Texture>,
    /// Linear display memory (see `PtCarry`): native-res ping-pong of the
    /// carried linear mean the tonemap exposes.
    display_linear: Vec<Texture>,
    display_ping: usize,
    /// Transient trace output; negative alpha marks one rejected sample.
    trace_scratch: Option<Texture>,
    /// This dispatch's mean reference light and dense id, or alpha -1 when skipped.
    far_entry: Option<Texture>,
    /// Per-pixel rejected-sample counters, ping-ponged with accumulation.
    rejected: Vec<Texture>,
    moments: Vec<Texture>,
    atrous: Vec<Texture>,
    gbuf: Option<Texture>,
    gbuf_depth: Option<Texture>,
    guide: Option<Texture>,
    /// Tonemapped BGRA8 display targets, ping-ponged: last frame's finished
    /// image is the tonemap's per-pixel hold while the next rung sweeps.
    view: Vec<Texture>,
    /// Index of the view texture written LAST (what `view_texture` returns).
    view_ping: usize,
    presentation: PresentationHold,
    held_picture: Option<HeldPicture>,
    stages: Vec<Stage>,
    /// Repaint id and stages of the recording awaiting a whole paint.
    /// A draw call is not a paint: the ping-pong buffers must hold its writes
    /// before the next step reads them.
    recorded: Option<(u64, [bool; 15])>,
    draw_gbuf: DrawGbuf,
    draw_copy: DrawCopy,
    draw_hold: DrawHold,
    draw_resolve: DrawResolve,
    draw_carry: DrawCarry,
    draw_reject: DrawReject,
    draw_trace: DrawTrace,
    /// Lazy variants: bits 2/1/0 select groups/exact/frost; slot 0 uses `draw_trace`.
    draw_trace_variants: [Option<DrawTrace>; 8],
    group_draws: Option<(DrawCopy, DrawGroupGather, DrawGroupGather, DrawGroupFar)>,
    draw_moments: DrawMoments,
    draw_guide: DrawGuide,
    draw_atrous: DrawAtrous,
    draw_tonemap: DrawTonemap,
    pub draw_view: DrawView,
    pub settings: RenderSettings,
    pub stats: RenderStats,
    ping: usize,
    frame: u32,
    seed: u32,
    /// Tile scheduler: edge (px), sweep cursor, and work issued per frame.
    tile_edge: u32,
    /// The edge the NEXT sweep will use (controller output).
    next_edge: u32,
    /// The sweep: tiles in spiral order from the view centre outward
    /// (ring by Chebyshev distance, then by angle), rebuilt when the edge
    /// or the target size changes between sweeps; `sweep_index` walks it.
    sweep_order: Vec<(u32, u32)>,
    sweep_index: usize,
    sweep_key: (u32, usize, usize),
    tiles_per_frame: u32,
    /// Resolution ladder (progressive only): the trace covers the whole
    /// frame at `native >> rung_shift`, halving the shift each time a sweep
    /// completes, until 0 = native, where samples accumulate.
    rung_shift: u32,
    /// The sweep wrapped last frame; the next frame may advance the rung.
    sweep_wrapped: bool,
    /// Paused by the host (user stop, or the window lost focus): frames
    /// present the accumulation as it stands and trace nothing, and
    /// `wants_frame` stops asking for redraws once the last recording paints.
    paused: bool,
    /// A host change the last recording does not show yet (display settings,
    /// pause, framing): a finished or paused render records once more for it.
    display_stale: bool,
    /// `guide` matches the current view. A restart or a new view-projection
    /// clears it; the next frame without a restart recomputes the guide.
    guide_valid: bool,
    /// Skip tracing on this frame only (the host's interactive duty cycle);
    /// the post stages still run so the picture stays on screen.
    skip_trace: bool,
    paths_last_frame: f64,
    draw_budget_ms: f64,
    gpu_under_budget: u32,
    /// The backend has reported at least one completion time for the trace
    /// pass of THIS tracer. Once true, the host-interval fallback never
    /// touches the ladder again — a missing sample only means the buffer is
    /// still in flight (or the tracer sat idle), never that timing broke.
    timing_ever: bool,
    /// Frames that actually dispatched trace tiles since the scene was set
    /// (idle present frames don't count). Gates the no-timing bring-up.
    dispatch_frames: u32,
    /// Recent measured GPU cost per path (ms), tiles > 0 submissions only.
    /// The MINIMUM of this window is the honest compute cost: a completion
    /// sample on a busy compositor frame carries several ms of queue
    /// scheduling that has nothing to do with the paths traced, and a
    /// controller fed raw samples pins itself at the floor (measured live:
    /// the same 16-tile submission reads 0.15 ms on a quiet frame and 14 ms
    /// on a busy one). A genuinely slow GPU never produces a fast sample,
    /// so the min never overstates the machine.
    gpu_ms_per_path: VecDeque<f64>,
    paths_done: f64,
    start_time: f64,
    last_time: f64,
    restart: bool,
    /// Explicit parent for the last stage (a host viewport's own pass); None
    /// = whatever pass is current at draw time.
    parent_pass: Option<DrawPassId>,
    /// Exact view-projection for the G-buffer raster (framing parity with a
    /// host's own rasterizer); None = built from the camera.
    view_proj_override: Option<Mat4f>,
    capture_pending: Vec<CaptureKind>,
    capture_tags: Vec<(TextureId, CaptureKind, f32, f64)>,
    capture_result: Vec<Capture>,
}

/// Which image to read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    /// The tonemapped BGRA8 view (4 bytes/px).
    View,
    /// The raw accumulation, RGBA32F (16 bytes/px): linear RGB sum + count.
    Accum,
    /// The denoised RGBA32F image (only when denoise is on).
    Denoised,
    /// RGBA32F diagnostic image; R is the rejected-sample count per pixel.
    Diagnostics,
    /// Gain-weighted linear group light at tile resolution (RGBA16F). Alpha is
    /// one for a footprint with samples, including when all gains are zero.
    LightGroups,
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub kind: CaptureKind,
    pub width: usize,
    pub height: usize,
    pub bytes: Vec<u8>,
    /// Stats at the time the capture was requested.
    pub spp: f32,
    pub elapsed: f64,
}

/// Picture-level measurements printed by the track renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DisplayMetrics {
    /// Rec. 709 luminance of the tonemapped image, in 0..1 display units.
    pub mean_luminance: f32,
    /// Fraction whose display luminance is greater than 8/255.
    pub above_eight_fraction: f32,
    /// Fraction more than four times the mean of its 9x9 neighbourhood.
    pub firefly_ratio: f32,
}

impl Capture {
    /// Decode the native RGBA32F or RGBA16F capture into linear floats.
    pub fn as_f32(&self) -> Vec<f32> {
        if self.kind == CaptureKind::LightGroups {
            return self.bytes.chunks_exact(2).map(|b| makepad_script::pod_heap::f16_to_f32(
                u16::from_le_bytes([b[0], b[1]]))).collect();
        }
        self.bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    }

    pub fn rejected_samples(&self) -> u64 {
        if self.kind != CaptureKind::Diagnostics {
            return 0;
        }
        self.as_f32().chunks_exact(4).map(|p| p[0].max(0.0) as u64).sum()
    }

    pub fn display_metrics(&self) -> Option<DisplayMetrics> {
        if self.kind != CaptureKind::View
            || self.width == 0
            || self.height == 0
            || self.bytes.len() < self.width * self.height * 4
        {
            return None;
        }
        let luminance: Vec<f32> = self
            .bytes
            .chunks_exact(4)
            .take(self.width * self.height)
            .map(|pixel| {
                // Captured view bytes are BGRA8.
                (0.2126 * pixel[2] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[0] as f32)
                    / 255.0
            })
            .collect();
        let pixels = luminance.len();
        let mean_luminance = luminance.iter().sum::<f32>() / pixels as f32;
        let above_eight_fraction = luminance
            .iter()
            .filter(|&&value| value > 8.0 / 255.0)
            .count() as f32
            / pixels as f32;

        let stride = self.width + 1;
        let mut integral = vec![0.0f64; stride * (self.height + 1)];
        for y in 0..self.height {
            let mut row = 0.0f64;
            for x in 0..self.width {
                row += luminance[y * self.width + x] as f64;
                integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row;
            }
        }
        let mut fireflies = 0usize;
        for y in 0..self.height {
            for x in 0..self.width {
                let x0 = x.saturating_sub(4);
                let y0 = y.saturating_sub(4);
                let x1 = (x + 5).min(self.width);
                let y1 = (y + 5).min(self.height);
                let sum = integral[y1 * stride + x1] - integral[y0 * stride + x1]
                    - integral[y1 * stride + x0]
                    + integral[y0 * stride + x0];
                let local_mean = sum / ((x1 - x0) * (y1 - y0)) as f64;
                if luminance[y * self.width + x] as f64 > 4.0 * local_mean {
                    fireflies += 1;
                }
            }
        }
        Some(DisplayMetrics {
            mean_luminance,
            above_eight_fraction,
            firefly_ratio: fireflies as f32 / pixels as f32,
        })
    }
}

const METER_KEY: f32 = 0.08;
const METER_MAX: f32 = 16.0;

/// CPU mirror of the tonemap shader's fixed-grid meter, used by parity tests.
pub fn metered_exposure_from_rgb(image: &[[f32; 3]], width: usize, height: usize, exposure: f32) -> f32 {
    if width == 0 || height == 0 || image.len() < width * height {
        return exposure;
    }
    let mut sum_log = 0.0f32;
    for grid_y in 0..4 {
        for grid_x in 0..4 {
            let x = ((grid_x * 2 + 1) * width / 8).min(width - 1);
            let y = ((grid_y * 2 + 1) * height / 8).min(height - 1);
            let pixel = image[y * width + x];
            let luminance = (0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2]).max(0.0001);
            sum_log += luminance.ln();
        }
    }
    exposure * (METER_KEY / (sum_log / 16.0).exp()).clamp(1.0, METER_MAX)
}

/// CPU mirror of the display curve.
pub fn tonemap_rgb(pixel: [f32; 3], exposure: f32) -> [f32; 3] {
    pixel.map(|linear| {
        let x = linear * exposure;
        let mapped = (x * (2.51 * x + 0.03) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
        mapped.powf(1.0 / 2.2)
    })
}

impl RayTracer {
    pub fn new(vm: &mut ScriptVm) -> Self {
        let draw_gbuf = DrawGbuf::script_new_with_default(vm);
        let draw_copy = DrawCopy::script_new_with_default(vm);
        let draw_hold = DrawHold::script_new_with_default(vm);
        let draw_resolve = DrawResolve::script_new_with_default(vm);
        let draw_carry = DrawCarry::script_new_with_default(vm);
        let draw_reject = DrawReject::script_new_with_default(vm);
        let draw_trace = DrawTrace::script_new_with_default(vm);
        let draw_moments = DrawMoments::script_new_with_default(vm);
        let draw_guide = DrawGuide::script_new_with_default(vm);
        let draw_atrous = DrawAtrous::script_new_with_default(vm);
        let draw_tonemap = DrawTonemap::script_new_with_default(vm);
        let draw_view = DrawView::script_new_with_default(vm);
        let cx = vm.cx_mut();
        let empty = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 { width: 1, height: 1, data: Some(vec![0.0; 4]), updated: TextureUpdated::Full },
            )
        };
        let atlas = Texture::new_with_format(
            cx,
            TextureFormat::VecBGRAu8_32 { width: 1, height: 1, data: Some(vec![0xffff_ffff]), updated: TextureUpdated::Full },
        );
        let mut me = Self {
            packed: None,
            group_targets: None,
            gains_tex: empty(cx),
            gain_indices: Default::default(),
            gain_values: Vec::new(),
            gains_dirty: false,
            recorded_gains: 0,
            presented_gains: 0,
            sky: SkyUniforms::uniform_white(1.0),
            environment: None,
            camera: Default::default(),
            tex_tri: empty(cx),
            tex_attr: empty(cx),
            tex_bvh: empty(cx),
            tex_mat: empty(cx),
            tex_light: empty(cx),
            tex_atlas: atlas,
            tex_env: empty(cx),
            raster_geom: None,
            size: (0, 0),
            target_size: (0, 0),
            accum: Vec::new(),
            display_linear: Vec::new(),
            display_ping: 0,
            trace_scratch: None,
            far_entry: None,
            rejected: Vec::new(),
            moments: Vec::new(),
            atrous: Vec::new(),
            gbuf: None,
            gbuf_depth: None,
            guide: None,
            view: Vec::new(),
            view_ping: 0,
            presentation: PresentationHold::default(),
            held_picture: None,
            stages: Vec::new(),
            recorded: None,
            draw_gbuf,
            draw_copy,
            draw_hold,
            draw_resolve,
            draw_carry,
            draw_reject,
            draw_trace,
            draw_trace_variants: Default::default(),
            group_draws: None,
            draw_moments,
            draw_guide,
            draw_atrous,
            draw_tonemap,
            draw_view,
            settings: RenderSettings::default(),
            stats: RenderStats::default(),
            ping: 0,
            frame: 0,
            seed: 1,
            tile_edge: TILE_START,
            next_edge: TILE_START,
            sweep_order: Vec::new(),
            sweep_index: 0,
            sweep_key: (0, 0, 0),
            tiles_per_frame: 1,
            rung_shift: 0,
            sweep_wrapped: false,
            paused: false,
            display_stale: false,
            guide_valid: false,
            skip_trace: false,
            paths_last_frame: 0.0,
            draw_budget_ms: draw_budget_ms_from_env(),
            gpu_under_budget: 0,
            timing_ever: false,
            dispatch_frames: 0,
            gpu_ms_per_path: VecDeque::new(),
            paths_done: 0.0,
            start_time: 0.0,
            last_time: 0.0,
            restart: true,
            parent_pass: None,
            view_proj_override: None,
            capture_pending: Vec::new(),
            capture_tags: Vec::new(),
            capture_result: Vec::new(),
        };
        me.restart = true;
        me
    }

    /// The hard GPU budget (ms) and the next sweep's measured tile edge.
    pub fn draw_budget(&self) -> (f64, u32) {
        (self.draw_budget_ms, self.next_edge)
    }

    /// Convenience for callers without a worker; packing runs on the calling thread.
    pub fn set_scene(&mut self, cx: &mut Cx, scene: &SceneInput) {
        self.set_packed(cx, Arc::new(PackedScene::pack(scene)), false);
        self.set_camera(scene.camera.clone());
        self.set_sun(&scene.sun, scene.up);
    }

    /// Upload a worker-produced pack, retaining its immutable geometry by Arc.
    /// Camera and environment remain the host's. With `hold`, keep the shown
    /// picture until the first complete sweep of the new accumulation.
    pub fn set_packed(&mut self, cx: &mut Cx, packed: Arc<PackedScene>, hold: bool) {
        self.hold_picture(cx, hold);
        self.tile_edge = TILE_START;
        self.next_edge = TILE_START;
        self.tiles_per_frame = 1;
        self.gpu_under_budget = 0;
        self.dispatch_frames = 0;
        self.gpu_ms_per_path.clear();
        let upload = |cx: &mut Cx, tex: &Texture, dt: &crate::pack::DataTex| {
            *tex.get_format(cx) = TextureFormat::VecRGBAf32 {
                width: dt.width,
                height: dt.height,
                data: Some(dt.data.clone()),
                updated: TextureUpdated::Full,
            };
        };
        upload(cx, &self.tex_tri, &packed.tri);
        upload(cx, &self.tex_attr, &packed.attr);
        upload(cx, &self.tex_bvh, &packed.bvh);
        upload(cx, &self.tex_mat, &packed.mat);
        upload(cx, &self.tex_light, &packed.light);
        if let Some(atlas) = &packed.atlas {
            *self.tex_atlas.get_format(cx) = TextureFormat::VecBGRAu8_32 {
                width: atlas.width,
                height: atlas.height,
                data: Some(atlas.data.clone()),
                updated: TextureUpdated::Full,
            };
        }
        let geom = self.raster_geom.get_or_insert_with(|| Geometry::new(cx));
        geom.update(cx, packed.raster_indices.clone(), packed.raster_verts.clone());
        self.stats.tri_count = packed.tri_count;
        self.stats.bvh_nodes = packed.accel.nodes.len();
        self.stats.bvh_depth = packed.accel.max_depth;
        let n = packed.light_groups.len();
        self.stats.light_groups = n as u32;
        self.stats.tile_scale = group_scale(n);
        if n != 0 { log!("raytrace light groups={} tile scale={}", n, self.stats.tile_scale); }
        self.gain_indices = packed.light_groups.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        self.group_targets = None;
        self.gains_dirty = n != 0;
        let width = n.clamp(1, DATA_W);
        let height = n.max(1).div_ceil(width);
        self.gain_values = vec![0.0; width * height * 4];
        *self.gains_tex.get_format(cx) = TextureFormat::VecRGBAf32 {
            width, height, data: Some(self.gain_values.clone()), updated: TextureUpdated::Full,
        };
        self.packed = Some(packed);
        self.restart = true;
    }

    /// Change the RGB gains of named groups in this pack. Omitted groups keep
    /// their gains. A new pack starts with all gains zero.
    pub fn set_light_group_gains(&mut self, cx: &mut Cx, gains: &[(u32, [f32; 3])]) {
        let mut changed = false;
        for &(id, rgb) in gains {
            if let Some(&index) = self.gain_indices.get(&id) {
                let row = &mut self.gain_values[index * 4..index * 4 + 3];
                if row != rgb.as_slice() {
                    row.copy_from_slice(&rgb);
                    changed = true;
                }
            }
        }
        if changed {
            let TextureFormat::VecRGBAf32 { data: Some(data), updated, .. } = self.gains_tex.get_format(cx)
                else { unreachable!("the group gain texture always has CPU data") };
            if *data == self.gain_values { return; }
            data.copy_from_slice(&self.gain_values);
            *updated = TextureUpdated::Full;
            self.stats.gain_uploads += 1;
            self.gains_dirty = true;
        }
    }

    /// Gain upload used by the last recorded far sum.
    pub fn recorded_gain_upload(&self) -> u64 {
        self.recorded_gains
    }

    /// Latest gain upload included in a completed view, including a paint
    /// completed since the last call to draw.
    pub fn presented_gain_upload(&self, cx: &Cx) -> u64 {
        if !self.holding() && self.recorded.is_some_and(|(repaint, active)| self.painted(cx, repaint, &active)) {
            self.recorded_gains
        } else { self.presented_gains }
    }

    fn hold_picture(&mut self, cx: &mut Cx, hold: bool) {
        // The most recently recorded view may still owe a paint. Its other
        // ping is the picture the pane actually presented before that draw.
        let unpainted = self.recorded.is_some_and(|(repaint, active)| !self.painted(cx, repaint, &active));
        let has_picture = !self.view.is_empty() && self.frame > u32::from(unpainted);
        if self.presentation.restart(hold, has_picture) {
            self.view_ping ^= usize::from(unpainted);
            let source = self.view[self.view_ping].clone();
            // The next tonemap writes the other ping, keeping this source
            // intact even if the first recording replays before a new draw.
            self.held_picture = Some(HeldPicture {
                texture: Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 {
                    size: TextureSize::Fixed { width: self.size.0, height: self.size.1 }, initial: true,
                }),
                size: self.size,
                source: Some(source),
                stage: Stage { pass: DrawPass::new_with_name(cx, "raytrace hold"), draw_list: DrawList::new(cx) },
                recorded: None,
            });
        } else if !self.presentation.holding {
            self.release_held_picture(cx);
        }
    }

    fn release_held_picture(&mut self, cx: &mut Cx) {
        if let Some(held) = self.held_picture.take() {
            held.stage.pass.detach(cx);
            held.stage.pass.clear_color_textures(cx);
        }
        self.draw_hold.draw_super.draw_vars.empty_texture(0);
    }

    /// Override the environment (the furnace test uses a uniform sky).
    /// Restarts only when it actually changed — hosts call this every frame.
    pub fn set_sky(&mut self, sky: SkyUniforms) {
        if self.sky != sky {
            self.sky = sky;
            self.restart = true;
        }
    }

    /// Set a map that replaces the analytic sky and sun, or clear it.
    /// Reuses the upload for the same Arc and restarts only on a light or frame change.
    pub fn set_environment(&mut self, cx: &mut Cx, env: Option<(Arc<EnvironmentLight>, EnvironmentFrame)>) {
        let same_light = match (&self.environment, &env) {
            (Some((old, _)), Some((new, _))) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
        let same_frame = self.environment.as_ref().map(|(_, frame)| frame)
            == env.as_ref().map(|(_, frame)| frame);
        if same_light && same_frame {
            return;
        }
        if !same_light {
            if let Some((light, _)) = &env {
                let data = light.data_texture();
                *self.tex_env.get_format(cx) = TextureFormat::VecRGBAf32 {
                    width: data.width,
                    height: data.height,
                    data: Some(data.data),
                    updated: TextureUpdated::Full,
                };
            }
        }
        self.environment = env;
        self.restart = true;
    }

    pub fn set_camera(&mut self, camera: crate::scene::Camera) {
        if self.camera != camera {
            self.camera = camera;
            self.restart = true;
        }
    }

    pub fn camera(&self) -> &crate::scene::Camera {
        &self.camera
    }

    pub fn sky(&self) -> &SkyUniforms {
        &self.sky
    }

    pub fn set_sun(&mut self, sun: &crate::scene::Sun, up: Vec3f) {
        self.set_sky(sky_uniforms(sun, up));
    }

    pub fn set_settings(&mut self, s: RenderSettings) {
        // View/debug modes and exposure/denoise don't invalidate the samples.
        let invalidates = s.max_bounces != self.settings.max_bounces
            || s.max_steps != self.settings.max_steps
            || s.brute != self.settings.brute
            || s.dbg_b != self.settings.dbg_b
            || s.max_diffuse != self.settings.max_diffuse
            || s.preview_clamp != self.settings.preview_clamp
            || s.debug_mode != self.settings.debug_mode
            || s.hybrid_primary != self.settings.hybrid_primary
            || s.adaptive_min != self.settings.adaptive_min
            || s.adaptive_thresh != self.settings.adaptive_thresh
            || s.shadow_skin != self.settings.shadow_skin
            || s.progressive != self.settings.progressive
            || s.exact != self.settings.exact;
        if invalidates {
            self.restart = true;
        }
        self.display_stale |= s != self.settings;
        self.settings = s;
    }

    /// Render at this many pixels (the widget's rect × scale, or the final
    /// image size). Changing it restarts.
    pub fn set_size(&mut self, w: usize, h: usize) {
        let w = w.max(4);
        let h = h.max(4);
        if self.target_size != (w, h) {
            self.target_size = (w, h);
            self.restart = true;
        }
    }

    pub fn restart(&mut self) {
        self.restart = true;
    }

    /// Tiles in the current sweep permutation (0 before the first draw).
    pub fn sweep_len(&self) -> u32 {
        self.sweep_order.len() as u32
    }

    /// Parent the pass chain under `pass` (the host viewport's offscreen
    /// pass) so the result is ready when that pass samples it.
    pub fn set_parent_pass(&mut self, pass: Option<DrawPassId>) {
        self.display_stale |= pass != self.parent_pass;
        self.parent_pass = pass;
    }

    /// Use exactly this view-projection for the primary-hit raster (the
    /// host's own camera math, for pixel-for-pixel framing parity). The ray
    /// generator derives the same frustum from the camera's fov/aspect.
    pub fn set_view_projection(&mut self, vp: Option<Mat4f>) {
        if vp != self.view_proj_override {
            self.display_stale = true;
            self.guide_valid = false;
        }
        self.view_proj_override = vp;
    }

    /// A held restart still presents the picture from before it.
    pub fn holding(&self) -> bool {
        self.presentation.holding
    }

    /// Original uploaded scene, including its reference emission.
    pub fn packed(&self) -> Option<&PackedScene> {
        self.packed.as_deref()
    }

    /// The tonemapped BGRA8 target (valid after the first draw).
    pub fn view_texture(&self) -> Option<&Texture> {
        if let Some(held) = &self.held_picture {
            // Keep sampling the original until the copy has actually painted.
            return Some(held.source.as_ref().unwrap_or(&held.texture));
        }
        self.view.get(self.view_ping)
    }

    /// Ask for the next frame's bytes of `kind` (they arrive via `take_captures`).
    pub fn request_capture(&mut self, kind: CaptureKind) {
        if !self.capture_pending.contains(&kind) {
            self.capture_pending.push(kind);
        }
    }

    pub fn take_captures(&mut self) -> Vec<Capture> {
        std::mem::take(&mut self.capture_result)
    }

    /// Camera ray for a pixel of the current target (click-to-focus).
    pub fn pixel_ray(&self, x: f32, y: f32) -> (Vec3f, Vec3f) {
        let (right, up, fwd) = self.camera.basis();
        let (w, h) = self.target_size;
        let aspect = w as f32 / h.max(1) as f32;
        let tan_y = (self.camera.fov_y * 0.5).tan();
        let ndc = vec2f(x / w as f32 * 2.0 - 1.0, 1.0 - y / h as f32 * 2.0);
        if let Some(hh) = self.camera.ortho_height {
            let ro = self.camera.pos + right * (ndc.x * hh * 0.5 * aspect) + up * (ndc.y * hh * 0.5);
            return (ro, fwd);
        }
        let rd = (fwd + right * (ndc.x * tan_y * aspect) + up * (ndc.y * tan_y)).normalize();
        (self.camera.pos, rd)
    }

    /// Distance to the first surface under a target pixel, if any.
    pub fn focus_distance_at(&self, x: f32, y: f32) -> Option<f32> {
        let packed = self.packed.as_ref()?;
        let (ro, rd) = self.pixel_ray(x, y);
        let hit = packed.accel.trace(ro - packed.origin, rd, packed.scene_scale * 5.0e27, false);
        if hit.is_hit() {
            Some(hit.t * rd.dot(self.camera.basis().2))
        } else {
            None
        }
    }

    fn ensure_targets(&mut self, cx: &mut Cx) {
        let groups = self.packed.as_ref().is_some_and(|p| !p.light_groups.is_empty());
        if groups && (self.size != self.target_size || self.group_targets.is_none()) {
            self.group_targets = Some(GroupTargets::new(cx, self.target_size, self.stats.light_groups as usize, self.stats.tile_scale as usize));
        } else if !groups {
            self.group_targets = None;
        }
        if groups && (self.size != self.target_size || self.far_entry.is_none()) {
            let (width, height) = self.target_size;
            // Each traced pixel replaces its entry; no clear or ping-pong.
            self.far_entry = Some(Texture::new_with_format(cx, TextureFormat::RenderRGBAf32 {
                size: TextureSize::Fixed { width, height }, initial: false,
            }));
        } else if !groups {
            self.far_entry = None;
        }
        if self.size == self.target_size && !self.accum.is_empty() {
            return;
        }
        self.size = self.target_size;
        // captures of the targets replaced here never come back
        self.capture_tags.clear();
        let (w, h) = self.size;
        let f32_tex = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::RenderRGBAf32 { size: TextureSize::Fixed { width: w, height: h }, initial: true },
            )
        };
        self.accum = vec![f32_tex(cx), f32_tex(cx)];
        self.display_linear = vec![f32_tex(cx), f32_tex(cx)];
        self.display_ping = 0;
        self.trace_scratch = Some(f32_tex(cx));
        self.rejected = vec![f32_tex(cx), f32_tex(cx)];
        self.moments = vec![f32_tex(cx), f32_tex(cx)];
        // One target per wavelet pass: a texture two passes of one recording write keeps the
        // earlier one owed a paint (its picture is the later writer's), so the chain never
        // paints whole and the paint wait above never lets the next step record.
        self.atrous = vec![f32_tex(cx), f32_tex(cx), f32_tex(cx), f32_tex(cx)];
        self.gbuf = Some(f32_tex(cx));
        self.guide = Some(f32_tex(cx));
        self.gbuf_depth = Some(Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 { size: TextureSize::Fixed { width: w, height: h }, initial: true },
        ));
        let bgra = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: w, height: h }, initial: true },
            )
        };
        self.view = vec![bgra(cx), bgra(cx)];
        self.view_ping = 0;
        const STAGE_NAMES: [&str; 15] = [
            "raytrace gbuffer",
            "raytrace trace",
            "raytrace group atlas 0",
            "raytrace group atlas 1",
            "raytrace resolve",
            "raytrace reject",
            "raytrace moments",
            "raytrace guide",
            "raytrace atrous 0",
            "raytrace atrous 1",
            "raytrace atrous 2",
            "raytrace atrous 3",
            "raytrace group sum",
            "raytrace carry",
            "raytrace tonemap",
        ];
        while self.stages.len() < STAGE_NAMES.len() {
            let name = STAGE_NAMES[self.stages.len()];
            self.stages.push(Stage {
                pass: DrawPass::new_with_name(cx, name),
                draw_list: DrawList::new(cx),
            });
        }
        // Stage 1 contains the full-frame carry plus every trace tile. Its
        // command-buffer duration is the feedback signal for the tile budget.
        self.stages[1].pass.set_gpu_timing_enabled(cx, true);
        self.restart = true;
    }

    fn view_proj(&self, jitter: Vec2f) -> Mat4f {
        let (w, h) = self.size;
        let aspect = w as f32 / h.max(1) as f32;
        // Called only with a packed scene by draw(); no fixed-size fallback.
        let packed = self.packed.as_ref().unwrap();
        let (lo, hi) = packed.bounds;
        let diag = packed.scene_scale;
        let dist = (self.camera.pos - (lo + hi) * 0.5).length();
        let far = (dist + diag) * 2.0;
        let near = (far * 1.0e-5).max(diag * 5.0e-5);
        let mut proj = match self.camera.ortho_height {
            Some(hh) => ortho_matrix(hh * 0.5 * aspect, hh * 0.5, near, far),
            None => Mat4f::perspective(self.camera.fov_y.to_degrees(), aspect, near, far),
        };
        // Sub-pixel jitter in NDC (the trace shader applies the same offset).
        proj.v[8] += jitter.x * 2.0 / w as f32;
        proj.v[9] += jitter.y * -2.0 / h as f32;
        let view = Mat4f::look_at(self.camera.pos, self.camera.target, self.camera.up);
        Mat4f::mul(&proj, &view)
    }

    fn painted(&self, cx: &Cx, repaint: u64, active: &[bool; 15]) -> bool {
        self.stages.iter().zip(active).all(|(st, &on)| !on || st.pass.painted_whole_since(cx, repaint))
    }

    fn attach_stage(cx: &mut Cx2d, stages: &[Stage], active: &[bool; 15], stage: usize, parent: Option<DrawPassId>) {
        let pass = &stages[stage].pass;
        let chain_parent = (stage + 1..active.len()).find(|&i| active[i]).map(|i| stages[i].pass.draw_pass_id());
        match chain_parent.or(parent) {
            Some(parent_id) => pass.set_pass_parent(cx.cx, parent_id),
            None => cx.make_child_pass(pass),
        }
    }

    /// Run one frame of the pipeline. Call from the widget's `draw_walk`;
    /// afterwards `view_texture()` holds the tonemapped image.
    /// Keep the last recording attached until every stage painted whole:
    /// another draw call must not read ping-pong buffers before those writes.
    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.draw_frame(cx, false);
    }

    /// Draw a sample already scheduled by the host and compose the latest
    /// gains with it. A stream of gain changes must not starve those samples.
    pub fn draw_scheduled_sample(&mut self, cx: &mut Cx2d) {
        self.draw_frame(cx, true);
    }

    fn draw_frame(&mut self, cx: &mut Cx2d, scheduled_sample: bool) {
        if self.packed.is_none() || self.target_size.0 == 0 {
            return;
        }
        self.ensure_targets(cx.cx);
        if let Some(held) = &mut self.held_picture {
            if held.recorded.is_some_and(|repaint| held.stage.pass.painted_whole_since(cx.cx, repaint)) {
                held.stage.pass.detach(cx.cx);
                held.source = None;
            }
            if let Some(source) = &held.source {
                // Execute before any stage can overwrite the source view ping.
                held.stage.pass.set_pass_parent(cx.cx, self.stages[0].pass.draw_pass_id());
                if held.recorded.is_none() {
                    let size = dvec2(held.size.0 as f64, held.size.1 as f64);
                    held.stage.pass.set_size(cx, size);
                    held.stage.pass.set_color_texture(cx, &held.texture, zero_clear());
                    cx.begin_pass(&held.stage.pass, Some(1.0));
                    held.stage.draw_list.begin_always(cx);
                    cx.begin_root_turtle(size, Layout::flow_overlay());
                    self.draw_hold.draw_super.draw_vars.set_texture(0, source);
                    self.draw_hold.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
                    cx.end_pass_sized_turtle();
                    held.stage.draw_list.end(cx);
                    cx.end_pass(&held.stage.pass);
                    held.recorded = Some(cx.cx.repaint_id());
                }
            }
        }
        // Replacing targets also sets restart, so their old recording cannot
        // gate the first step at the new size.
        if self.restart {
            self.recorded = None;
        } else if let Some((repaint, active)) = self.recorded {
            if !self.painted(cx.cx, repaint, &active) {
                for (i, &on) in active.iter().enumerate() {
                    if on { Self::attach_stage(cx, &self.stages, &active, i, self.parent_pass); }
                }
                return;
            }
            self.recorded = None;
            // A recorded sample count is not a presentable image. Release only
            // after the threshold recording paints. Before scene holds, tiles
            // replaced a raster fallback immediately, with no whole-frame gate;
            // one complete startup sweep is the first fully traced picture.
            let (w, h) = self.size;
            let shift = if self.settings.progressive { start_rung_shift(w, h) } else { 0 };
            let pixels = (((w - 1) >> shift) + 1) * (((h - 1) >> shift) + 1);
            let threshold = (pixels as f64 / (w * h) as f64) as f32;
            if active[1] && self.presentation.advance(self.stats.spp, threshold) {
                self.release_held_picture(cx.cx);
            }
            if !self.holding() { self.presented_gains = self.recorded_gains; }
            // Painted: the next step records now. A finished (or paused) render records only
            // for a capture or a change it does not show yet, so an idle pane stops asking for
            // frames after its last paint.
            if (self.stats.done || self.paused) && self.capture_pending.is_empty() && !self.display_stale && !self.gains_dirty {
                return;
            }
        }
        // Until the copy paints, a second restart must also write the other
        // ping: view_texture still presents this original source. Do this only
        // when recording a new frame, after examining the previous paint.
        if let Some(source) = self.held_picture.as_ref().and_then(|held| held.source.as_ref()) {
            if let Some(ping) = self.view.iter().position(|view| view.texture_id() == source.texture_id()) {
                self.view_ping = ping;
            }
        }
        let now = cx.time();
        let (w, h) = self.size;
        let s = self.settings.clone();
        let groups = !self.packed.as_ref().unwrap().light_groups.is_empty();
        let frost = self.packed.as_ref().unwrap().rough_thin_glass;
        let trace_variant = ((groups as usize) << 2) | ((s.exact as usize) << 1) | frost as usize;
        if groups {
            if self.group_draws.is_none() {
                self.group_draws = Some(cx.cx.cx.with_vm(|vm| {
                    let value = vm.eval(script! { mod.draw.PtCopyGroups });
                    let copy = DrawCopy::script_from_value(vm, value);
                    let gather = DrawGroupGather::script_new_with_default(vm);
                    let value = vm.eval(script! { mod.draw.PtGroupAtlasCopy });
                    let atlas_copy = DrawGroupGather::script_from_value(vm, value);
                    (copy, gather, atlas_copy, DrawGroupFar::script_new_with_default(vm))
                }));
            }
        }
        if trace_variant != 0 && self.draw_trace_variants[trace_variant].is_none() {
            self.draw_trace_variants[trace_variant] = Some(cx.cx.cx.with_vm(|vm| {
                let v = match (groups, s.exact, frost) {
                    (false, false, true) => vm.eval(script! { mod.draw.PtTraceFrost }),
                    (false, true, false) => vm.eval(script! { mod.draw.PtTraceExact }),
                    (false, true, true) => vm.eval(script! { mod.draw.PtTraceFrostExact }),
                    (true, false, false) => vm.eval(script! { mod.draw.PtTraceGroups }),
                    (true, false, true) => vm.eval(script! { mod.draw.PtTraceGroupsFrost }),
                    (true, true, false) => vm.eval(script! { mod.draw.PtTraceGroupsExact }),
                    (true, true, true) => vm.eval(script! { mod.draw.PtTraceGroupsFrostExact }),
                    (false, false, false) => unreachable!(),
                };
                DrawTrace::script_from_value(vm, v)
            }));
        }
        let packed = self.packed.as_ref().unwrap();
        let alignment = if self.stats.tile_scale > 0 { self.stats.tile_scale.max(8) } else { 1 };
        let scene_scale = vec2f(packed.scene_scale, packed.scene_area);
        let lens_radius = self.camera.lens_radius();
        let use_gbuffer = s.hybrid_primary && lens_radius <= 0.0 && (s.debug_mode == 0 || s.debug_mode == 7 || s.debug_mode == 8);

        if self.restart {
            self.restart = false;
            self.guide_valid = false;
            self.frame = 0;
            self.paths_done = 0.0;
            self.paths_last_frame = 0.0;
            self.start_time = now;
            self.last_time = now;
            self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223) & 0x00ff_ffff | 1;
            self.sweep_index = 0;
            self.sweep_wrapped = false;
            self.rung_shift = if s.progressive { start_rung_shift(w, h) } else { 0 };
            // A restart keeps the tile size it had earned, halved; a NEW scene
            // starts at the conservative startup edge (set_scene resets it).
            self.next_edge = aligned_edge(self.next_edge / 2, alignment);
            self.tile_edge = self.next_edge;
            self.tiles_per_frame = 1;
            self.gpu_under_budget = 0;
            self.stats = RenderStats {
                tri_count: self.stats.tri_count,
                bvh_nodes: self.stats.bvh_nodes,
                bvh_depth: self.stats.bvh_depth,
                tile_scale: self.stats.tile_scale,
                light_groups: self.stats.light_groups,
                gain_uploads: self.stats.gain_uploads,
                width: w,
                height: h,
                ..Default::default()
            };
            // Clear both accumulation/moment buffers by drawing zero rows.
            self.ping = 0;
            if let Some(targets) = &mut self.group_targets {
                targets.atlas_ping = 0;
                targets.last_rects.clear();
            }
        }
        let first_frame = self.frame == 0;
        let done = reached_sample_limit(s.target_spp, self.stats.spp);
        let dt = now - self.last_time;
        self.last_time = now;
        // ---- the dispatch-budget law -------------------------------------
        // Metal completion handlers report the trace pass command buffer's
        // actual GPUStartTime/GPUEndTime. Consume completed samples without
        // waiting, map them to submitted tile counts, and keep the next trace
        // buffer below the hard cap. Unsupported backends keep the safe
        // one-tile startup instead of guessing from CPU frame intervals.
        let dt_ms = dt * 1000.0;
        let requested_ms = if s.frame_budget.is_finite() {
            (s.frame_budget * 1000.0).max(0.5)
        } else {
            self.draw_budget_ms
        };
        let budget_ms = requested_ms.min(self.draw_budget_ms);
        self.stats.gpu_budget_ms = budget_ms;
        // Samples carry the (tiles, edge) tag captured when their command
        // buffer was ENCODED: a pass replays on every window repaint, so
        // arrival order can never say what a duration measured.
        let gpu_samples = self.stages[1].pass.take_gpu_time_samples(cx.cx);
        for (tag, gpu_ms) in gpu_samples {
            let submitted_tiles = (tag >> 16) as u32;
            let submitted_edge = (tag & 0xffff) as u32;
            self.stats.gpu_time_ms = gpu_ms;
            self.stats.gpu_samples = self.stats.gpu_samples.saturating_add(1);
            self.timing_ever = true;
            if submitted_tiles == 0 {
                continue;
            }
            // Min-filter the completion samples per path before the
            // controller sees them: queue-scheduling spikes on busy
            // compositor frames otherwise read as permanent overruns and
            // pin the ladder at one tiny tile (see `gpu_ms_per_path`).
            let submitted_paths =
                submitted_tiles as f64 * (submitted_edge as f64) * (submitted_edge as f64);
            let filtered_ms = if submitted_paths > 0.0 && gpu_ms > 0.0 {
                self.gpu_ms_per_path.push_back(gpu_ms / submitted_paths);
                if self.gpu_ms_per_path.len() > 8 {
                    self.gpu_ms_per_path.pop_front();
                }
                let per_path = self
                    .gpu_ms_per_path
                    .iter()
                    .copied()
                    .fold(f64::INFINITY, f64::min);
                (per_path * submitted_paths).max(0.001)
            } else {
                gpu_ms
            };
            let update = adapt_gpu_budget(
                self.tiles_per_frame,
                self.next_edge,
                self.gpu_under_budget,
                submitted_tiles,
                submitted_edge,
                filtered_ms,
                budget_ms,
                alignment,
            );
            self.tiles_per_frame = update.tiles;
            self.next_edge = update.next_edge;
            self.gpu_under_budget = update.under_budget;
        }
        self.next_edge = aligned_edge(self.next_edge, alignment);
        if self.stats.gpu_samples == 0 {
            let update = adapt_without_timing(
                self.tiles_per_frame,
                self.next_edge,
                self.gpu_under_budget,
                self.timing_ever,
                self.dispatch_frames,
                self.paths_last_frame > 0.0,
                dt_ms,
                budget_ms,
                alignment,
            );
            self.tiles_per_frame = update.tiles;
            self.next_edge = update.next_edge;
            self.gpu_under_budget = update.under_budget;
        }
        if done {
            self.gpu_under_budget = 0;
        }
        // The progressive pane is always denoised: at one sample per pixel
        // the rungs would be salt-and-pepper, and even at 100 spp the raw
        // estimator still shows clamped NEE spikes as lone white pixels.
        // The wavelet weights tighten as variance falls (the sigma floor
        // fades as 1/sqrt N), so a converged pane converges to the raw
        // mean plus the first-pass firefly rule. F12/track keep the
        // explicit switch and the unbiased estimator.
        let denoise = (s.denoise || s.progressive) && s.debug_mode == 0;
        let gains_only = !first_frame && self.gains_dirty && !scheduled_sample;
        let dispatching = should_dispatch_trace(done, self.paused, self.skip_trace || gains_only, true);
        // A gain update on an idle pane reuses the last filtered base and guide.
        let present_only = groups && !first_frame && !dispatching && self.gains_dirty
            && !self.capture_pending.iter().any(|k| !matches!(k, CaptureKind::View | CaptureKind::LightGroups));
        // A settled view records its guide without advancing the trace sweep.
        let guide_frame = denoise && !self.guide_valid && !first_frame && !present_only;
        let dispatching = dispatching && !guide_frame;
        // ---- the resolution ladder ---------------------------------------
        // The first pass after a restart covers the WHOLE frame at
        // `native >> rung_shift` and each completed sweep halves the shift:
        // a complete (coarse, upscaled) picture lands within the first few
        // budgeted dispatches, then sharpens rung by rung to native, where
        // samples accumulate. The accumulation is cleared at every rung
        // boundary — two resolutions never mix in one buffer — while the
        // display's hold keeps the previous rung's picture under every pixel
        // the new sweep has not reached yet.
        let mut rung_clear = false;
        if dispatching && std::mem::take(&mut self.sweep_wrapped) && self.rung_shift > 0 {
            self.rung_shift -= 1;
            rung_clear = true;
            self.paths_done = 0.0;
        }
        let shift = self.rung_shift;
        let (rw, rh) = (((w - 1) >> shift) + 1, ((h - 1) >> shift) + 1);
        let use_gbuffer = use_gbuffer && shift == 0;
        // The edge in use changes ONLY at the start of a sweep: the sweep is
        // a fixed permutation of tiles, and a mid-sweep resize would cover
        // some pixels twice and others never (measured: 2.0 / 0.5 per cell).
        if self.sweep_index == 0 {
            // Spend the budget as few large dispatches, not many small ones:
            // when the measured ladder wants four-plus tiles a frame and the
            // edge can still grow, promote the edge for the coming sweep and
            // cut the tile count to keep paths-per-frame roughly constant —
            // the next completion sample retrims either way.
            if dispatching && self.tiles_per_frame >= 4 && self.next_edge < TILE_MAX {
                let area = self.tiles_per_frame as u64 * (self.next_edge as u64 * self.next_edge as u64);
                self.next_edge = (self.next_edge * 2).min(TILE_MAX);
                self.tiles_per_frame =
                    ((area / (self.next_edge as u64 * self.next_edge as u64)) as u32).clamp(1, TILES_PER_FRAME_MAX);
            }
            // The measured controller can be driven to its floor by QUEUE
            // NOISE: on a busy frame the trace command buffer's wall time
            // carries a fixed several-ms scheduling overhead whatever the
            // tile size, which reads as a permanent overrun and once pinned
            // the pane crawls at 64 paths a frame (measured live: edge 8,
            // one tile, 0.06 spp after 45 s). The controller still shrinks
            // and grows on real signal, but dispatch never drops below
            // `MIN_PATHS_PER_FRAME` — small dispatches, watchdog-safe, and a
            // guaranteed convergence pace.
            self.tile_edge = aligned_edge(self.next_edge.max(32), alignment);
            let key = (self.tile_edge, rw, rh);
            if self.sweep_key != key || self.sweep_order.is_empty() {
                self.sweep_order = spiral_order(rw as u32, rh as u32, self.tile_edge);
                self.sweep_key = key;
            }
        }
        let edge = self.tile_edge;
        // The dispatch envelope. Metal command-buffer intervals under a
        // busy compositor both inflate (queue scheduling, concurrent
        // buffers stretching Start..End) and lie low (samples matched to a
        // lighter replay), so the measured controller only steers WITHIN a
        // hard floor and ceiling: the floor guarantees convergence pace,
        // the ceiling bounds the tracer's GPU appetite per host frame so
        // the machine keeps breathing whatever the readings say.
        let tile_area = edge.saturating_mul(edge);
        let min_tiles = MIN_PATHS_PER_FRAME.div_ceil(tile_area);
        let max_tiles_budget = (MAX_PATHS_PER_FRAME / tile_area).max(1);
        let tiles_this_frame = self
            .tiles_per_frame
            .max(min_tiles)
            .min(max_tiles_budget)
            .min(TILES_PER_FRAME_MAX);
        let mut tiles: Vec<(u32, u32, u32, u32)> = Vec::new();
        if dispatching && !self.sweep_order.is_empty() {
            for _ in 0..tiles_this_frame {
                let (tx, ty) = self.sweep_order[self.sweep_index];
                let x0 = tx * edge;
                let y0 = ty * edge;
                tiles.push((x0, y0, edge.min(rw as u32 - x0), edge.min(rh as u32 - y0)));
                self.sweep_index += 1;
                if self.sweep_index >= self.sweep_order.len() {
                    // A sweep ended: the next one may use a new edge — and
                    // the ladder may climb a rung next frame.
                    self.sweep_index = 0;
                    self.sweep_wrapped = true;
                    break;
                }
            }
        }

        let clear_accum = first_frame || rung_clear;
        let prev = self.ping;
        let cur = if present_only { prev } else { prev ^ 1 };
        let (right, up, fwd) = self.camera.basis();
        let aspect = w as f32 / h as f32;
        let tan_y = (self.camera.fov_y * 0.5).tan();
        let ortho = match self.camera.ortho_height {
            Some(hh) => vec2f(hh * 0.5 * aspect, hh * 0.5),
            None => vec2f(0.0, 0.0),
        };
        // Frame jitter for the raster (a low-discrepancy point of the frame).
        let (jx, jy) = frame_jitter(self.frame);
        let jitter = if use_gbuffer && !guide_frame { vec2f(jx, jy) } else { vec2f(0.0, 0.0) };
        let inv_res = vec2f(1.0 / w as f32, 1.0 / h as f32);
        let sky = self.sky;
        let env_on = self.environment.is_some();
        let (env_size, env_inv, env_frame) = self.environment.as_ref().map(|(light, frame)| {
            let map = light.map();
            let rows = (map.width * map.height + map.height + DATA_W - 1) / DATA_W;
            (vec2f(map.width as f32, map.height as f32), vec2f(1.0 / DATA_W as f32, 1.0 / rows as f32), *frame)
        }).unwrap_or((vec2f(1.0, 1.0), vec2f(1.0, 1.0), EnvironmentFrame::default()));
        let env_rows = env_frame.world_to_map;
        let (packed_inv, n_nodes) = {
            let p = self.packed.as_ref().unwrap();
            (
                (
                    p.tri.inv(),
                    p.attr.inv(),
                    p.bvh.inv(),
                    p.mat.inv(),
                    p.light.inv(),
                    p.n_lights as f32,
                    p.origin,
                    if env_on { 0.0 } else { p.env_grid_dim as f32 },
                    p.env_min,
                    p.env_inv_extent,
                ),
                p.accel.nodes.len() as f32,
            )
        };

        // The raster runs while the frame still has untraced pixels (the
        // fallback), for hybrid primaries, and for the denoiser's guides.
        let gbuf_drawn = use_gbuffer || denoise || (self.stats.spp < 1.0 && s.debug_mode == 0);
        if !denoise {
            self.capture_pending.retain(|k| *k != CaptureKind::Denoised);
        }
        let group_count = self.stats.light_groups as usize;
        let reset_atlas = groups && first_frame;
        let gather = groups && !tiles.is_empty();
        // Stage 2 owns atlas 0 and stage 3 owns atlas 1. At restart the
        // first clears the source and the second clears and seeds its peer.
        let atlas_cur = self.group_targets.as_ref().map_or(0, |t| t.atlas_ping ^ 1);
        let sum_far = groups && (reset_atlas || gather || self.gains_dirty || self.capture_pending.contains(&CaptureKind::LightGroups));
        if !groups { self.capture_pending.retain(|k| *k != CaptureKind::LightGroups); }
        // Keep each pass attached to its own output even when a gain update
        // skips the base passes. Capture and repaint both track the producer.
        let base = !present_only;
        let filter = base && denoise;
        let mut active = [base, base, reset_atlas, reset_atlas, base, base, base, guide_frame,
            filter, filter, filter, filter, sum_far, true, true];
        if gather { active[2 + atlas_cur] = true; }
        for (st, &on) in self.stages.iter().zip(&active) {
            if !on { st.pass.withdraw(cx.cx); }
        }
        let mut active_stages = (0..active.len()).filter(|&i| active[i]);

        macro_rules! run_stage {
            ($target:expr, $depth:expr, $clear:expr, $draw:expr) => {{
                run_stage!($target, $depth, $clear, None::<&Texture>, $draw);
            }};
            ($target:expr, $depth:expr, $clear:expr, $far:expr, $draw:expr) => {{
                run_stage!($target, $depth, $clear, $far, dvec2(w as f64, h as f64), $draw);
            }};
            ($target:expr, $depth:expr, $clear:expr, $far:expr, $size:expr, $draw:expr) => {{
                let size = $size;
                let stage = active_stages.next().unwrap();
                Self::attach_stage(cx, &self.stages, &active, stage, self.parent_pass);
                {
                    let st = &mut self.stages[stage];
                    st.pass.set_size(cx, size);
                    st.pass.clear_color_textures(cx.cx);
                    st.pass.set_color_texture(cx, $target, $clear);
                    if let Some(far) = $far {
                        st.pass.add_color_texture(cx.cx, far, DrawPassClearColor::InitWith(Vec4f::default()));
                    }
                    if let Some(d) = $depth {
                        st.pass.set_depth_texture(cx, d, DrawPassClearDepth::ClearWith(1.0));
                    } else {
                        st.pass.clear_depth_texture(cx.cx);
                    }
                    cx.begin_pass(&st.pass, Some(1.0));
                    st.pass.set_size(cx, size);
                    st.pass.set_dpi_factor(cx, 1.0);
                    st.draw_list.begin_always(cx);
                }
                let pass_size = cx.current_pass_size();
                cx.begin_root_turtle(pass_size, Layout::flow_overlay());
                $draw(cx, Rect { pos: dvec2(0.0, 0.0), size });
                cx.end_pass_sized_turtle();
                {
                    let st = &mut self.stages[stage];
                    st.draw_list.end(cx);
                    cx.end_pass(&st.pass);
                }
            }};
        }

        let mut tonemap_src = if denoise && present_only {
            self.atrous[3].clone()
        } else { self.accum[cur].clone() };
        // 1. G-buffer raster.
        if !present_only {
            let clear_g = DrawPassClearColor::ClearWith(vec4(-1.0, 0.0, 0.0, 0.0));
            let gbuf = self.gbuf.clone().unwrap();
            let depth = self.gbuf_depth.clone().unwrap();
            let vp = match self.view_proj_override {
                Some(m) => {
                    let mut m = m;
                    m.v[8] += jitter.x * 2.0 / w as f32;
                    m.v[9] += jitter.y * -2.0 / h as f32;
                    m
                }
                None => self.view_proj(jitter),
            };
            let geom_id = self.raster_geom.as_ref().unwrap().geometry_id();
            let draw_gbuf = &mut self.draw_gbuf;
            draw_gbuf.view_proj = vp;
            draw_gbuf.draw_vars.geometry_id = Some(geom_id);
            run_stage!(&gbuf, Some(&depth), clear_g, |cx: &mut Cx2d, _r| {
                if gbuf_drawn {
                    cx.new_draw_call(&mut draw_gbuf.draw_vars);
                    if draw_gbuf.draw_vars.can_instance() {
                        let area = cx.add_instance(&mut draw_gbuf.draw_vars);
                        draw_gbuf.draw_vars.area = cx.update_area_refs(draw_gbuf.draw_vars.area, area);
                    }
                }
            });
        }

        // 2. Trace.
        if !present_only {
            let target = self.trace_scratch.clone().unwrap();
            let src = self.accum[prev].clone();
            let mom = self.moments[prev].clone();
            let rejected = self.rejected[prev].clone();
            let gbuf = self.gbuf.clone().unwrap();
            let far = self.far_entry.clone();
            let draw_trace = if trace_variant == 0 {
                &mut self.draw_trace
            } else {
                self.draw_trace_variants[trace_variant].as_mut().unwrap()
            };
            let dv = &mut draw_trace.draw_super.draw_vars;
            dv.set_texture(0, &self.tex_tri);
            dv.set_texture(1, &self.tex_attr);
            dv.set_texture(2, &self.tex_bvh);
            dv.set_texture(3, &self.tex_mat);
            dv.set_texture(4, &self.tex_light);
            dv.set_texture(5, &self.tex_atlas);
            dv.set_texture(6, &src);
            dv.set_texture(7, &gbuf);
            dv.set_texture(8, &mom);
            dv.set_texture(9, &rejected);
            dv.set_texture(10, &self.tex_env);
            let c = &*cx.cx;
            dv.set_uniform(c, id!(env_on), &[if env_on { 1.0 } else { 0.0 }]);
            dv.set_uniform(c, id!(env_inv), &[env_inv.x, env_inv.y]);
            dv.set_uniform(c, id!(env_size), &[env_size.x, env_size.y]);
            dv.set_uniform(c, id!(env_row0), &[env_rows[0].x, env_rows[0].y, env_rows[0].z, 0.0]);
            dv.set_uniform(c, id!(env_row1), &[env_rows[1].x, env_rows[1].y, env_rows[1].z, 0.0]);
            dv.set_uniform(c, id!(env_row2), &[env_rows[2].x, env_rows[2].y, env_rows[2].z, 0.0]);
            dv.set_uniform(c, id!(env_strength), &[env_frame.strength]);
            dv.set_uniform(c, id!(res), &[w as f32, h as f32]);
            dv.set_uniform(c, id!(inv_res), &[inv_res.x, inv_res.y]);
            dv.set_uniform(c, id!(cam_inv), &[1.0 / rw as f32, 1.0 / rh as f32]);
            dv.set_uniform(c, id!(tri_inv), &[packed_inv.0.x, packed_inv.0.y]);
            dv.set_uniform(c, id!(attr_inv), &[packed_inv.1.x, packed_inv.1.y]);
            dv.set_uniform(c, id!(bvh_inv), &[packed_inv.2.x, packed_inv.2.y]);
            dv.set_uniform(c, id!(mat_inv), &[packed_inv.3.x, packed_inv.3.y]);
            dv.set_uniform(c, id!(light_inv), &[packed_inv.4.x, packed_inv.4.y]);
            dv.set_uniform(c, id!(jitter), &[jitter.x, jitter.y]);
            dv.set_uniform(c, id!(seed), &[self.seed as f32]);
            dv.set_uniform(c, id!(max_steps), &[s.max_steps.clamp(64, crate::bvh::MAX_STEPS) as f32]);
            dv.set_uniform(c, id!(spp), &[1.0]);
            dv.set_uniform(c, id!(reset), &[if clear_accum { 1.0 } else { 0.0 }]);
            dv.set_uniform(c, id!(ortho), &[ortho.x, ortho.y]);
            dv.set_uniform(c, id!(use_gbuffer), &[if use_gbuffer { 1.0 } else { 0.0 }]);
            dv.set_uniform(c, id!(n_lights), &[packed_inv.5]);
            dv.set_uniform(c, id!(inv_light_power), &[self.packed.as_ref().unwrap().inv_light_power]);
            dv.set_uniform(c, id!(env_grid_dim), &[packed_inv.7]);
            dv.set_uniform(c, id!(env_min), &[packed_inv.8.x, packed_inv.8.y, packed_inv.8.z]);
            dv.set_uniform(c, id!(env_inv_extent), &[packed_inv.9.x, packed_inv.9.y, packed_inv.9.z]);
            dv.set_uniform(c, id!(n_nodes), &[n_nodes]);
            dv.set_uniform(c, id!(max_bounces), &[s.max_bounces.min(16) as f32]);
            dv.set_uniform(c, id!(max_diffuse), &[s.max_diffuse as f32]);
            // Coarse rungs are 1-sample transient pictures: one NEE spike is
            // a full-white block for a whole rung, so they get a tighter
            // clamp. Native keeps the caller's estimator choice.
            let clamp_eff = if shift > 0 {
                s.preview_clamp.unwrap_or(4.0).min(4.0)
            } else {
                s.preview_clamp.unwrap_or(0.0)
            };
            dv.set_uniform(c, id!(preview_clamp), &[clamp_eff]);
            let skin = if s.shadow_skin < 0.0 {
                self.packed.as_ref().map(|p| p.auto_shadow_skin()).unwrap_or(0.0)
            } else {
                s.shadow_skin
            };
            dv.set_uniform(c, id!(shadow_skin), &[skin]);
            dv.set_uniform(c, id!(scene_scale), &[scene_scale.x, scene_scale.y]);
            let (aw, ah) = self
                .packed
                .as_ref()
                .and_then(|p| p.atlas.as_ref())
                .map(|a| (a.width as f32, a.height as f32))
                .unwrap_or((1.0, 1.0));
            dv.set_uniform(c, id!(atlas_inv), &[1.0 / aw, 1.0 / ah]);
            // One pixel of the current rung: scene units per unit ray
            // length (perspective), or scene units (ortho).
            let pixel_world = match self.camera.ortho_height {
                Some(hh) => hh / rh as f32,
                None => 2.0 * tan_y / rh as f32,
            };
            dv.set_uniform(c, id!(pixel_world), &[pixel_world]);
            dv.set_uniform(c, id!(adaptive_min), &[s.adaptive_min as f32]);
            dv.set_uniform(c, id!(adaptive_thresh), &[s.adaptive_thresh]);
            let local_cam = self.camera.pos - packed_inv.6;
            dv.set_uniform(c, id!(cam_pos), &[local_cam.x, local_cam.y, local_cam.z]);
            dv.set_uniform(c, id!(cam_right), &[right.x, right.y, right.z]);
            dv.set_uniform(c, id!(cam_up), &[up.x, up.y, up.z]);
            dv.set_uniform(c, id!(cam_fwd), &[fwd.x, fwd.y, fwd.z]);
            dv.set_uniform(c, id!(cam_tan), &[tan_y * aspect, tan_y]);
            dv.set_uniform(c, id!(lens), &[lens_radius, self.camera.focus_dist, self.camera.blades as f32, 0.0]);
            dv.set_uniform(c, id!(sun_dir), &[sky.sun_dir.x, sky.sun_dir.y, sky.sun_dir.z, sky.sun_dir.w]);
            dv.set_uniform(c, id!(sun_radiance), &[sky.sun_radiance.x, sky.sun_radiance.y, sky.sun_radiance.z, 0.0]);
            let sun_pdf = 1.0 / (std::f32::consts::TAU * (1.0 - sky.sun_dir.w));
            dv.set_uniform(c, id!(sun_pdf), &[sun_pdf]);
            dv.set_uniform(c, id!(sun_on), &[if !env_on && sky.up.w > 0.5 && sky.sun_radiance.x > 0.0 { 1.0 } else { 0.0 }]);
            dv.set_uniform(c, id!(env_sun_prob), &[sky.sun_sample_probability()]);
            dv.set_uniform(c, id!(pz_y), &[sky.pz_y.x, sky.pz_y.y, sky.pz_y.z, sky.pz_y.w]);
            dv.set_uniform(c, id!(pz_x), &[sky.pz_x.x, sky.pz_x.y, sky.pz_x.z, sky.pz_x.w]);
            dv.set_uniform(c, id!(pz_yc), &[sky.pz_yc.x, sky.pz_yc.y, sky.pz_yc.z, sky.pz_yc.w]);
            dv.set_uniform(c, id!(pz_e), &[sky.pz_e.x, sky.pz_e.y, sky.pz_e.z, sky.pz_e.w]);
            dv.set_uniform(c, id!(pz_f0), &[sky.pz_f0.x, sky.pz_f0.y, sky.pz_f0.z, sky.pz_f0.w]);
            dv.set_uniform(c, id!(zenith), &[sky.zenith.x, sky.zenith.y, sky.zenith.z, sky.zenith.w]);
            dv.set_uniform(c, id!(sun_model), &[sky.sun_model.x, sky.sun_model.y, sky.sun_model.z, sky.sun_model.w]);
            dv.set_uniform(c, id!(world_up), &[sky.up.x, sky.up.y, sky.up.z, sky.up.w]);
            dv.set_uniform(c, id!(star_r0), &[sky.star_r0.x, sky.star_r0.y, sky.star_r0.z, 0.0]);
            dv.set_uniform(c, id!(star_r1), &[sky.star_r1.x, sky.star_r1.y, sky.star_r1.z, 0.0]);
            dv.set_uniform(c, id!(star_r2), &[sky.star_r2.x, sky.star_r2.y, sky.star_r2.z, 0.0]);
            dv.set_uniform(c, id!(sky_strength), &[sky.sky_strength]);
            dv.set_uniform(c, id!(uniform_sky), &[sky.uniform_value]);
            dv.set_uniform(c, id!(debug_mode), &[s.debug_mode as f32]);
            dv.set_uniform(c, id!(brute), &[if s.brute { 1.0 } else { 0.0 }]);
            dv.set_uniform(c, id!(dbg_b), &[s.dbg_b]);
            let draw_copy = if groups { &mut self.group_draws.as_mut().unwrap().0 } else { &mut self.draw_copy };
            draw_copy.draw_super.draw_vars.set_texture(0, &src);
            draw_copy.draw_super.draw_vars.set_uniform(c, id!(reset), &[if clear_accum { 1.0 } else { 0.0 }]);
            let tiles_ref = &tiles;
            run_stage!(&target, None::<&Texture>, zero_clear(), far.as_ref(), |cx: &mut Cx2d, r| {
                // The whole frame carries over (one fetch per pixel)...
                // The grouped copy marks all far entries empty before the tiles draw.
                draw_copy.draw_abs(cx, r);
                // ...and each tile is its OWN draw call (`draw_call_always`)
                // covering only its rect: the per-draw GPU time is the tile's.
                for &(x0, y0, tw, th) in tiles_ref.iter() {
                    draw_trace.draw_super.draw_vars.set_uniform(cx.cx.cx, id!(tile), &[x0 as f32, y0 as f32, tw as f32, th as f32]);
                    draw_trace.draw_abs(cx, Rect { pos: dvec2(x0 as f64, y0 as f64), size: dvec2(tw as f64, th as f64) });
                }
            });
            // Label the pass content for the timing samples this frame's
            // command buffer (and any repaint replays of it) will report.
            self.stages[1]
                .pass
                .set_gpu_time_tag(cx.cx, ((tiles.len() as u64) << 16) | edge as u64);
        }

        if reset_atlas || gather {
            let targets = self.group_targets.as_mut().unwrap();
            let (tw, th) = targets.tile;
            let (cols, rows) = targets.grid;
            let atlas_size = dvec2((tw * cols) as f64, (th * rows) as f64);
            let prev = &targets.atlas[atlas_cur ^ 1];
            let cur = &targets.atlas[atlas_cur];
            if reset_atlas {
                run_stage!(prev, None::<&Texture>, zero_clear(), None::<&Texture>, atlas_size, |_cx: &mut Cx2d, _r| {});
            }
            let scale = self.stats.tile_scale;
            let rung = 1u32 << shift;
            let draw_groups = |cx: &mut Cx2d, draw: &mut DrawGroupGather, rect: Rect| {
                for k in 0..group_count {
                    draw.group_id = (k + 1) as f32;
                    draw.draw_abs(cx, rect.translate(dvec2(((k % cols) * tw) as f64, ((k / cols) * th) as f64)));
                }
            };
            let (_, draw_gather, draw_copy, _) = self.group_draws.as_mut().unwrap();
            let dv = &mut draw_copy.draw_super.draw_vars;
            dv.set_texture(0, prev);
            dv.set_uniform(cx.cx, id!(atlas_size), &[atlas_size.x as f32, atlas_size.y as f32]);
            let dv = &mut draw_gather.draw_super.draw_vars;
            dv.set_texture(0, prev);
            dv.set_texture(1, self.far_entry.as_ref().unwrap());
            dv.set_uniform(cx.cx, id!(atlas_size), &[atlas_size.x as f32, atlas_size.y as f32]);
            dv.set_uniform(cx.cx, id!(tile_size), &[tw as f32, th as f32]);
            dv.set_uniform(cx.cx, id!(cols), &[cols as f32]);
            dv.set_uniform(cx.cx, id!(native_inv), &[inv_res.x, inv_res.y]);
            dv.set_uniform(cx.cx, id!(footprint), &[scale as f32 / rung as f32]);
            let clear = if reset_atlas { zero_clear() } else { DrawPassClearColor::InitWith(Vec4f::default()) };
            let last_rects = &mut targets.last_rects;
            run_stage!(cur, None::<&Texture>, clear, None::<&Texture>, atlas_size, |cx: &mut Cx2d, _r| {
                // Only the previous dispatch's footprints are missing on this
                // ping. Copy them first so a current gather wins any overlap.
                if !last_rects.is_empty() {
                    draw_copy.begin_many_instances(cx);
                    for &rect in last_rects.iter() { draw_groups(cx, draw_copy, rect); }
                    draw_copy.end_many_instances(cx);
                    last_rects.clear();
                }
                if gather {
                    draw_gather.begin_many_instances(cx);
                    for &(x, y, width, height) in &tiles {
                        let x0 = x * rung / scale;
                        let y0 = y * rung / scale;
                        let x1 = ((x + width) * rung).div_ceil(scale).min(tw as u32);
                        let y1 = ((y + height) * rung).div_ceil(scale).min(th as u32);
                        let rect = Rect {
                            pos: dvec2(x0 as f64, y0 as f64),
                            size: dvec2((x1 - x0) as f64, (y1 - y0) as f64),
                        };
                        draw_gather.rung_rect = vec4f(x as f32, y as f32, (x + width) as f32, (y + height) as f32);
                        draw_groups(cx, draw_gather, rect);
                        last_rects.push(rect);
                    }
                    draw_gather.end_many_instances(cx);
                }
            });
            // A replay uses the retained draws. The paint gate above keeps
            // their source intact until this recording has painted whole.
            if gather { targets.atlas_ping = atlas_cur; }
        }

        // 3. Resolve the transient rejection sentinel to clean (sum, N).
        if !present_only {
            let target = self.accum[cur].clone();
            let scratch = self.trace_scratch.as_ref().unwrap().clone();
            let dv = &mut self.draw_resolve.draw_super.draw_vars;
            dv.set_texture(0, &scratch);
            let draw_resolve = &mut self.draw_resolve;
            run_stage!(&target, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, r| draw_resolve.draw_abs(cx, r));
            if self.capture_pending.contains(&CaptureKind::Accum) {
                self.capture_pending.retain(|k| *k != CaptureKind::Accum);
                cx.cx.request_render_texture_capture(&target);
                self.tag_capture(&target, CaptureKind::Accum, now);
            }
        }

        // 4. Persistent per-pixel rejected-sample diagnostic counter.
        if !present_only {
            let target = self.rejected[cur].clone();
            let scratch = self.trace_scratch.as_ref().unwrap().clone();
            let dv = &mut self.draw_reject.draw_super.draw_vars;
            dv.set_texture(0, &scratch);
            dv.set_texture(1, &self.rejected[prev]);
            dv.set_uniform(cx.cx, id!(reset), &[if clear_accum { 1.0 } else { 0.0 }]);
            let draw_reject = &mut self.draw_reject;
            run_stage!(&target, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, r| draw_reject.draw_abs(cx, r));
            if self.capture_pending.contains(&CaptureKind::Diagnostics) {
                self.capture_pending.retain(|k| *k != CaptureKind::Diagnostics);
                cx.cx.request_render_texture_capture(&target);
                self.tag_capture(&target, CaptureKind::Diagnostics, now);
            }
        }

        // 5. Moments.
        if !present_only {
            let target = self.moments[cur].clone();
            let dv = &mut self.draw_moments.draw_super.draw_vars;
            dv.set_texture(0, &self.accum[prev]);
            dv.set_texture(1, &self.accum[cur]);
            dv.set_texture(2, &self.moments[prev]);
            dv.set_uniform(cx.cx, id!(reset), &[if clear_accum { 1.0 } else { 0.0 }]);
            let draw_moments = &mut self.draw_moments;
            run_stage!(&target, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, r| draw_moments.draw_abs(cx, r));
        }

        // 4. Denoise: guide + 4 à-trous iterations.
        if denoise && !present_only {
            let guide = self.guide.clone().unwrap();
            if guide_frame {
                let dv = &mut self.draw_guide.draw_super.draw_vars;
                // PtGuide inherits PtTrace's texture slots and transport helpers.
                dv.set_texture(0, &self.tex_tri);
                dv.set_texture(1, &self.tex_attr);
                dv.set_texture(2, &self.tex_bvh);
                dv.set_texture(3, &self.tex_mat);
                dv.set_texture(4, &self.tex_light);
                dv.set_texture(5, &self.tex_atlas);
                dv.set_texture(6, &self.accum[cur]);
                dv.set_texture(7, self.gbuf.as_ref().unwrap());
                dv.set_texture(8, &self.moments[cur]);
                dv.set_texture(9, &self.rejected[cur]);
                dv.set_texture(10, &self.tex_env);
                dv.set_uniform(cx.cx, id!(env_on), &[if env_on { 1.0 } else { 0.0 }]);
                dv.set_uniform(cx.cx, id!(env_inv), &[env_inv.x, env_inv.y]);
                dv.set_uniform(cx.cx, id!(env_size), &[env_size.x, env_size.y]);
                dv.set_uniform(cx.cx, id!(env_row0), &[env_rows[0].x, env_rows[0].y, env_rows[0].z, 0.0]);
                dv.set_uniform(cx.cx, id!(env_row1), &[env_rows[1].x, env_rows[1].y, env_rows[1].z, 0.0]);
                dv.set_uniform(cx.cx, id!(env_row2), &[env_rows[2].x, env_rows[2].y, env_rows[2].z, 0.0]);
                dv.set_uniform(cx.cx, id!(env_strength), &[env_frame.strength]);
                dv.set_uniform(cx.cx, id!(sun_on), &[0.0]);
                dv.set_uniform(cx.cx, id!(env_grid_dim), &[0.0]);
                dv.set_uniform(cx.cx, id!(tri_inv), &[packed_inv.0.x, packed_inv.0.y]);
                dv.set_uniform(cx.cx, id!(attr_inv), &[packed_inv.1.x, packed_inv.1.y]);
                dv.set_uniform(cx.cx, id!(bvh_inv), &[packed_inv.2.x, packed_inv.2.y]);
                dv.set_uniform(cx.cx, id!(mat_inv), &[packed_inv.3.x, packed_inv.3.y]);
                dv.set_uniform(cx.cx, id!(n_nodes), &[n_nodes]);
                dv.set_uniform(cx.cx, id!(max_steps), &[s.max_steps.clamp(64, crate::bvh::MAX_STEPS) as f32]);
                dv.set_uniform(cx.cx, id!(scene_scale), &[scene_scale.x, scene_scale.y]);
                dv.set_uniform(cx.cx, id!(inv_res), &[inv_res.x, inv_res.y]);
                dv.set_uniform(cx.cx, id!(jitter), &[jitter.x, jitter.y]);
                dv.set_uniform(cx.cx, id!(ortho), &[ortho.x, ortho.y]);
                let local_cam = self.camera.pos - packed_inv.6;
                dv.set_uniform(cx.cx, id!(cam_pos), &[local_cam.x, local_cam.y, local_cam.z]);
                dv.set_uniform(cx.cx, id!(cam_right), &[right.x, right.y, right.z]);
                dv.set_uniform(cx.cx, id!(cam_up), &[up.x, up.y, up.z]);
                dv.set_uniform(cx.cx, id!(cam_fwd), &[fwd.x, fwd.y, fwd.z]);
                dv.set_uniform(cx.cx, id!(cam_tan), &[tan_y * aspect, tan_y]);
                let draw_guide = &mut self.draw_guide;
                run_stage!(&guide, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, r| draw_guide.draw_abs(cx, r));
                self.guide_valid = true;
            }
            let mut src = self.accum[cur].clone();
            // At a coarse rung the image lives in the top-left sub-rect;
            // the wavelet quads cover only that (world-space addressing in
            // the shader), the geometry guide is native-res so it sits out,
            // and the absolute sigma floor does the smoothing that a
            // zero-variance 1-spp buffer otherwise refuses.
            let atrous_rect = Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(rw as f64, rh as f64),
            };
            for it in 0..4 {
                let dst = self.atrous[it].clone();
                let dv = &mut self.draw_atrous.draw_super.draw_vars;
                dv.set_texture(0, &src);
                dv.set_texture(1, &guide);
                dv.set_texture(2, &self.moments[cur]);
                dv.set_uniform(cx.cx, id!(inv_res), &[inv_res.x, inv_res.y]);
                dv.set_uniform(cx.cx, id!(step), &[(1u32 << it) as f32]);
                dv.set_uniform(cx.cx, id!(sigma_l), &[s.denoise_sigma]);
                dv.set_uniform(cx.cx, id!(scene_scale), &[scene_scale.x]);
                dv.set_uniform(
                    cx.cx,
                    id!(geom_weight),
                    &[if lens_radius > 0.0 || shift > 0 || !self.guide_valid { 0.0 } else { 1.0 }],
                );
                dv.set_uniform(
                    cx.cx,
                    id!(sigma_floor),
                    &[if s.progressive { 0.35 } else { 0.0 }],
                );
                let draw_atrous = &mut self.draw_atrous;
                run_stage!(&dst, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, _r| {
                    draw_atrous.draw_abs(cx, atrous_rect)
                });
                src = dst;
            }
            if self.capture_pending.contains(&CaptureKind::Denoised) {
                self.capture_pending.retain(|k| *k != CaptureKind::Denoised);
                cx.cx.request_render_texture_capture(&src);
                self.tag_capture(&src, CaptureKind::Denoised, now);
            }
            tonemap_src = src;
        }

        if sum_far {
            let targets = self.group_targets.as_ref().unwrap();
            let far = targets.far.clone();
            let draw = &mut self.group_draws.as_mut().unwrap().3;
            let dv = &mut draw.draw_super.draw_vars;
            dv.set_texture(0, &targets.atlas[targets.atlas_ping]);
            dv.set_texture(1, &self.gains_tex);
            let width = group_count.min(DATA_W);
            dv.set_uniform(cx.cx, id!(grid), &[targets.grid.0 as f32, targets.grid.1 as f32]);
            dv.set_uniform(cx.cx, id!(gain_size), &[width as f32, group_count.div_ceil(width) as f32]);
            dv.set_uniform(cx.cx, id!(group_count), &[group_count as f32]);
            let size = dvec2(targets.tile.0 as f64, targets.tile.1 as f64);
            run_stage!(&far, None::<&Texture>, zero_clear(), None::<&Texture>, size,
                |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
            if self.capture_pending.contains(&CaptureKind::LightGroups) {
                self.capture_pending.retain(|kind| *kind != CaptureKind::LightGroups);
                cx.cx.request_render_texture_capture(&far);
                self.tag_capture(&far, CaptureKind::LightGroups, now);
            }
        }

        // 5. Carry: fold the current rung's linear means over last frame's
        // carried linear display, in LINEAR radiance (see `PtCarry`). The
        // hold drops on the restart frame: the previous carry still shows
        // the OLD camera — untraced pixels must fall through to the raster.
        let carried = {
            let display_prev = self.display_ping;
            let display_cur = display_prev ^ 1;
            let target = self.display_linear[display_cur].clone();
            let hold = self.display_linear[display_prev].clone();
            let dv = &mut self.draw_carry.draw_super.draw_vars;
            dv.set_texture(0, &tonemap_src);
            dv.set_texture(1, &hold);
            dv.set_texture(2, &self.rejected[cur]);
            dv.set_uniform(cx.cx, id!(inv_res), &[inv_res.x, inv_res.y]);
            dv.set_uniform(cx.cx, id!(src_res), &[rw as f32, rh as f32]);
            dv.set_uniform(cx.cx, id!(coarse), &[if shift > 0 { 1.0 } else { 0.0 }]);
            dv.set_uniform(
                cx.cx,
                id!(hold_on),
                &[if s.progressive && !first_frame { 1.0 } else { 0.0 }],
            );
            dv.set_uniform(cx.cx, id!(src_is_mean), &[if denoise { 1.0 } else { 0.0 }]);
            let draw_carry = &mut self.draw_carry;
            run_stage!(&target, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, r| draw_carry.draw_abs(cx, r));
            self.display_ping = display_cur;
            target
        };

        // 6. Tonemap → view target (+ capture): ONE metered exposure over
        // the whole carried linear frame — held and fresh pixels alike.
        {
            let view_prev = self.view_ping;
            let view_cur = view_prev ^ 1;
            let view = self.view[view_cur].clone();
            let dv = &mut self.draw_tonemap.draw_super.draw_vars;
            dv.set_texture(0, &tonemap_src);
            dv.set_texture(1, &self.moments[cur]);
            dv.set_texture(2, self.gbuf.as_ref().unwrap());
            dv.set_texture(3, &self.tex_attr);
            dv.set_texture(4, &self.tex_tri);
            dv.set_texture(5, &self.tex_mat);
            dv.set_texture(6, &carried);
            let (far, tile_size) = self.group_targets.as_ref().map_or((&self.gains_tex, (1, 1)), |t| (&t.far, t.tile));
            dv.set_texture(7, far);
            dv.set_texture(8, if denoise && groups { self.guide.as_ref().unwrap() } else { &self.gains_tex });
            dv.set_uniform(cx.cx, id!(far_size), &[tile_size.0 as f32, tile_size.1 as f32]);
            dv.set_uniform(cx.cx, id!(far_scale), &[self.stats.tile_scale as f32]);
            dv.set_uniform(cx.cx, id!(guide_on), &[if denoise && self.guide_valid { 1.0 } else { 0.0 }]);
            dv.set_uniform(cx.cx, id!(inv_res), &[inv_res.x, inv_res.y]);
            dv.set_uniform(cx.cx, id!(src_res), &[rw as f32, rh as f32]);
            dv.set_uniform(cx.cx, id!(coarse), &[if shift > 0 { 1.0 } else { 0.0 }]);
            dv.set_uniform(cx.cx, id!(exposure), &[s.exposure]);
            dv.set_uniform(cx.cx, id!(sky_display), &[if !env_on && sky.uniform_value <= 0.0 && sky.sky_strength > 0.0 { 1.0 } else { 0.0 }]);
            dv.set_uniform(cx.cx, id!(view_mode), &[s.view_mode as f32]);
            dv.set_uniform(cx.cx, id!(attr_inv), &[packed_inv.1.x, packed_inv.1.y]);
            dv.set_uniform(cx.cx, id!(tri_inv), &[packed_inv.0.x, packed_inv.0.y]);
            dv.set_uniform(cx.cx, id!(mat_inv), &[packed_inv.3.x, packed_inv.3.y]);
            let ld = if sky.up.w > 0.5 { vec3f(sky.sun_dir.x, sky.sun_dir.y, sky.sun_dir.z) } else { vec3f(sky.up.x, sky.up.y, sky.up.z) };
            dv.set_uniform(cx.cx, id!(light_dir), &[ld.x, ld.y, ld.z]);
            dv.set_uniform(cx.cx, id!(gbuf_on), &[if gbuf_drawn { 1.0 } else { 0.0 }]);
            dv.set_uniform(cx.cx, id!(src_is_mean), &[if denoise { 1.0 } else { 0.0 }]);
            dv.set_uniform(cx.cx, id!(untraced_transparent), &[if s.untraced_transparent { 1.0 } else { 0.0 }]);
            let draw_tonemap = &mut self.draw_tonemap;
            run_stage!(&view, None::<&Texture>, zero_clear(), |cx: &mut Cx2d, r| draw_tonemap.draw_abs(cx, r));
            self.view_ping = view_cur;
            if self.capture_pending.contains(&CaptureKind::View) {
                self.capture_pending.retain(|k| *k != CaptureKind::View);
                cx.cx.request_render_texture_capture(&view);
                self.tag_capture(&view, CaptureKind::View, now);
            }
        }
        debug_assert!(active_stages.next().is_none());

        self.recorded = Some((cx.cx.repaint_id(), active));
        self.recorded_gains = self.stats.gain_uploads;
        self.gains_dirty = false;
        self.display_stale = false;
        if present_only {
            self.stats.tiles = 0;
            self.paths_last_frame = 0.0;
            return;
        }
        // Bookkeeping.
        self.ping = cur;
        let paths_now: f64 = tiles.iter().map(|t| (t.2 * t.3) as f64).sum();
        if paths_now > 0.0 {
            self.dispatch_frames = self.dispatch_frames.saturating_add(1);
        }
        self.paths_last_frame = paths_now;
        self.paths_done += paths_now;
        self.stats.samples_total = self.paths_done;
        self.frame += 1;
        self.stats.frames = self.frame;
        self.stats.spp = (self.paths_done / (w * h) as f64) as f32;
        self.stats.elapsed = now - self.start_time;
        self.stats.samples_per_sec = if self.stats.elapsed > 0.0 { self.stats.samples_total / self.stats.elapsed } else { 0.0 };
        self.stats.tile_edge = edge;
        self.stats.tiles = tiles.len() as u32;
        self.stats.rung_shift = shift;
        self.stats.last_frame_ms = dt_ms;
        self.stats.done = done;
    }

    /// Tag a capture asked of `texture`. The platform folds a request into one still in flight
    /// for the same texture, and a capture whose copy fails never comes back: the texture's
    /// older tag goes, so it cannot label a later capture.
    fn tag_capture(&mut self, texture: &Texture, kind: CaptureKind, now: f64) {
        let id = texture.texture_id();
        self.capture_tags.retain(|(tagged, ..)| *tagged != id);
        self.capture_tags.push((id, kind, self.stats.spp, now - self.start_time));
    }

    /// Poll finished captures; call once per frame from `handle_event`.
    pub fn poll_capture(&mut self, cx: &mut Cx) {
        for (t, w, h, bytes) in cx.take_render_texture_captures() {
            if let Some(at) = self.capture_tags.iter().position(|(tid, ..)| *tid == t) {
                let (_, kind, spp, elapsed) = self.capture_tags.remove(at);
                self.capture_result.push(Capture { kind, width: w, height: h, bytes, spp, elapsed });
            }
        }
    }

    /// The per-render RNG seed (the CPU twin needs it for bit parity).
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Keep drawing while the image is converging.
    /// Per-DRAW GPU budget in ms; the tile cap follows from it through the
    /// scene's cost prior. The interactive preview asks for a small one so
    /// no single command buffer can hold the GPU long enough to stall the
    /// compositor. `MAKEPAD_PT_BUDGET_MS` in the environment wins.
    pub fn set_draw_budget_ms(&mut self, ms: f64) {
        if std::env::var("MAKEPAD_PT_BUDGET_MS").is_ok() {
            return;
        }
        self.draw_budget_ms = ms.clamp(0.5, 16.0);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.display_stale |= paused != self.paused;
        self.paused = paused;
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_skip_trace(&mut self, skip: bool) {
        self.skip_trace = skip;
    }

    pub fn wants_frame(&self) -> bool {
        if self.recorded.is_some() {
            return true;
        }
        if self.gains_dirty { return true; }
        if self.paused {
            return false;
        }
        !self.stats.done || self.restart || !self.capture_pending.is_empty()
    }
}

/// Smallest / startup / largest tile edge in pixels. Start conservatively at
/// 32; measured overruns may shrink to 8 so a slow GPU is not pinned there.
pub const TILE_MIN: u32 = 8;
pub const TILE_START: u32 = 32;
pub const TILE_MAX: u32 = 128;
/// Most tiles one frame queues (each its own draw).
pub const TILES_PER_FRAME_MAX: u32 = 32;
/// Paths per dispatching frame never fall below this, whatever the budget
/// controller reads: a fixed queue-scheduling overhead of several ms rides
/// every trace command buffer on a busy compositor frame, and a controller
/// fed that noise pins itself at one tiny tile. Sixteen 32-px dispatches
/// stay individually small (watchdog-safe) while guaranteeing ~half an spp
/// per second at 30 Hz on a 1-Mpixel pane.
pub const MIN_PATHS_PER_FRAME: u32 = 16384;
/// ...and never above this: ~96k paths per 30 Hz tick is roughly a quarter
/// of the measured saturated throughput on the dev machine — over 1.5 spp/s
/// on a megapixel pane while the compositor and the realtime pane keep
/// their frame time. The measured controller wanders inside the envelope.
pub const MAX_PATHS_PER_FRAME: u32 = 98304;
/// Tiles in the first Chebyshev ring (centre + 8 neighbours). The interactive
/// host uses this as a present-gate threshold; the scheduler still restarts
/// at one tile per frame.
pub const FIRST_RING_TILES: u32 = 9;

/// `MAKEPAD_PT_BUDGET_MS`: hard target for one trace command buffer (ms).
pub fn draw_budget_ms_from_env() -> f64 {
    std::env::var("MAKEPAD_PT_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(4.0)
        .clamp(0.5, 50.0)
}

/// The coarsest rung of the resolution ladder for a `w`×`h` target: the
/// deepest shift (≤ 3, i.e. 1/8) that keeps both coarse dimensions ≥ 96 px —
/// enough pixels that the upscaled first picture reads as the scene, few
/// enough that one or two budgeted dispatches cover the whole frame.
pub fn start_rung_shift(w: usize, h: usize) -> u32 {
    let mut s = 0u32;
    while s < 3 && (w >> (s + 1)) >= 96 && (h >> (s + 1)) >= 96 {
        s += 1;
    }
    s
}

/// The sub-pixel jitter the G-buffer raster uses on frame `frame` (the CPU
/// twin reproduces the primary hit with it).
pub fn frame_jitter(frame: u32) -> (f32, f32) {
    let (jx, jy) = crate::rng::sobol_2d(frame, 0x5eed, 9);
    (jx - 0.5, jy - 0.5)
}

/// The tile visiting order of one sweep: a spiral out from the tile under
/// the view centre — rings by Chebyshev distance, each ring clockwise from
/// its top — so the subject converges first. Deterministic.
pub fn spiral_order(w: u32, h: u32, edge: u32) -> Vec<(u32, u32)> {
    let tiles_x = ((w + edge - 1) / edge).max(1);
    let tiles_y = ((h + edge - 1) / edge).max(1);
    let cx = (tiles_x as i64 - 1) / 2;
    let cy = (tiles_y as i64 - 1) / 2;
    let mut tiles: Vec<(i64, f64, u32, u32)> = Vec::with_capacity((tiles_x * tiles_y) as usize);
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let dx = tx as i64 - cx;
            let dy = ty as i64 - cy;
            let ring = dx.abs().max(dy.abs());
            // Angle from "up", clockwise, in [0, 2π): the ring's walk order.
            let ang = (dx as f64).atan2(-(dy as f64));
            let ang = if ang < 0.0 { ang + std::f64::consts::TAU } else { ang };
            tiles.push((ring, ang, tx, ty));
        }
    }
    tiles.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)));
    tiles.into_iter().map(|t| (t.2, t.3)).collect()
}

fn zero_clear() -> DrawPassClearColor {
    DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0))
}

/// Orthographic projection, symmetric about the axis, with depth from 0 at `near` to 1 at
/// `far`: the clip range the GPU backends keep. A GL-style [-1, 1] depth would clip the
/// nearer half of the view.
fn ortho_matrix(half_w: f32, half_h: f32, near: f32, far: f32) -> Mat4f {
    let mut m = Mat4f::identity();
    m.v[0] = 1.0 / half_w;
    m.v[5] = 1.0 / half_h;
    m.v[10] = 1.0 / (near - far);
    m.v[14] = near / (near - far);
    m
}

#[cfg(test)]
mod sweep_tests {
    use super::{
        adapt_gpu_budget, adapt_without_timing, reached_sample_limit, should_dispatch_trace, spiral_order, Capture, CaptureKind, RenderSettings, TILE_MIN, metered_exposure_from_rgb, tonemap_rgb,
    };

    #[test]
    fn scene_hold_keeps_the_presented_source_until_the_startup_sweep() {
        let mut hold = super::PresentationHold::default();
        assert!(!hold.restart(true, false), "first scene has no picture to capture");
        assert!(!hold.holding, "first scene presents normally");
        assert!(hold.restart(true, true), "first held restart captures the shown picture once");
        assert!(!hold.advance(0.01, 0.0625), "partial startup sweep keeps the old picture");
        assert!(hold.holding, "presentation still selects the held source");
        assert!(!hold.restart(true, true), "second restart preserves the original copy");
        assert!(!hold.advance(0.0, 0.0625), "second restart begins a fresh hidden accumulation");
        assert!(!hold.advance(0.0624, 0.0625), "old source survives just below the threshold");
        assert!(hold.advance(0.0625, 0.0625), "threshold releases the held target");
        assert!(!hold.holding, "presentation selects the new accumulation at the threshold");
        assert!(!hold.advance(1.0, 0.0625), "release occurs only once");
        assert!(hold.restart(true, true), "a later edit captures its own shown picture");
        assert!(!hold.restart(false, true), "an unheld edit requests no copy");
        assert!(!hold.holding, "without hold the original immediate presentation remains");
    }

    #[test]
    fn gpu_budget_cuts_tiles_on_an_overrun_and_grows_only_after_stability() {
        let over = adapt_gpu_budget(8, 64, 2, 8, 64, 8.0, 4.0, 1);
        assert_eq!(over.tiles, 3);
        assert_eq!(over.under_budget, 0);

        let one_tile_over = adapt_gpu_budget(1, 32, 0, 1, 32, 8.0, 4.0, 1);
        assert_eq!(one_tile_over.tiles, 1);
        assert_eq!(one_tile_over.next_edge, 16);

        // Growth needs two consecutive under-budget completions.
        let a = adapt_gpu_budget(1, TILE_MIN, 0, 1, TILE_MIN, 0.5, 4.0, 1);
        let b = adapt_gpu_budget(a.tiles, a.next_edge, a.under_budget, 1, TILE_MIN, 0.5, 4.0, 1);
        assert_eq!(a.tiles, 1);
        assert_eq!(a.under_budget, 1);
        assert_eq!(b.tiles, 2);
        assert_eq!(b.under_budget, 0);
    }

    #[test]
    fn idle_frames_keep_the_earned_tile_ladder() {
        // A converged ("done") pane draws present-only frames: no dispatch,
        // no fresh timing sample. Raising the sample limit must continue at
        // the earned tile size and count — never restart the spiral with a
        // tiny tile because the pane sat idle for a while.
        for timing_ever in [false, true] {
            let out = adapt_without_timing(8, 64, 1, timing_ever, 500, false, 200.0, 4.0, 1);
            assert_eq!((out.tiles, out.next_edge, out.under_budget), (8, 64, 1), "timing_ever {timing_ever}");
        }
    }

    #[test]
    fn a_backend_that_reported_once_never_falls_back_to_the_host_interval() {
        // Metal has reported before: a frame without a fresh sample only
        // means the buffer is still in flight. Even a huge host interval on
        // a dispatching frame must not shrink the earned ladder.
        let out = adapt_without_timing(8, 64, 0, true, 500, true, 200.0, 4.0, 1);
        assert_eq!((out.tiles, out.next_edge), (8, 64));
    }

    #[test]
    fn no_timing_backend_brings_up_conservatively_then_uses_the_interval() {
        // Bring-up: one startup tile until the backend proves it never
        // reports (30 dispatched frames).
        let out = adapt_without_timing(8, 128, 2, false, 2, true, 8.0, 4.0, 1);
        assert_eq!((out.tiles, out.next_edge, out.under_budget), (1, 32, 0));
        // Established no-timing backend: a late host frame halves (the
        // late threshold has a 40 ms floor so a ~30 Hz accumulation tick
        // is not read as an overrun)...
        let late = adapt_without_timing(8, 64, 0, false, 60, true, 60.0, 4.0, 1);
        assert_eq!((late.tiles, late.next_edge), (4, 32));
        let ticked = adapt_without_timing(8, 64, 0, false, 60, true, 33.0, 4.0, 1);
        assert_eq!((ticked.tiles, ticked.next_edge), (8, 64));
        // ...and two consecutive on-time traced frames grow, edge first.
        let g1 = adapt_without_timing(4, 32, 0, false, 60, true, 4.0, 4.0, 1);
        assert_eq!(g1.under_budget, 1);
        let g2 = adapt_without_timing(g1.tiles, g1.next_edge, g1.under_budget, false, 60, true, 4.0, 4.0, 1);
        assert_eq!(g2.next_edge, 48);
        assert_eq!(g2.under_budget, 0);
    }

    #[test]
    fn pause_skip_done_and_limit_gate_trace_dispatch() {
        assert!(should_dispatch_trace(false, false, false, true));
        assert!(!should_dispatch_trace(true, false, false, true));
        assert!(!should_dispatch_trace(false, true, false, true));
        assert!(!should_dispatch_trace(false, false, true, true));
        assert!(!should_dispatch_trace(false, false, false, false));

        assert!(reached_sample_limit(64, 64.0));
        assert!(!reached_sample_limit(64, 63.999));
        assert!(!reached_sample_limit(0, f32::MAX));
    }

    #[test]
    fn spiral_visits_every_tile_once_centre_first() {
        let order = spiral_order(617, 744, 112);
        let (tx, ty) = ((617 + 111) / 112, (744 + 111) / 112);
        assert_eq!(order.len(), (tx * ty) as usize);
        let mut seen = std::collections::HashSet::new();
        for t in &order {
            assert!(seen.insert(*t), "tile visited twice: {t:?}");
        }
        // The first tile is the centre one; rings never go backwards.
        assert_eq!(order[0], ((tx - 1) / 2, (ty - 1) / 2));
        let (cx, cy) = (((tx - 1) / 2) as i64, ((ty - 1) / 2) as i64);
        let mut last = 0;
        for (x, y) in &order {
            let ring = (*x as i64 - cx).abs().max((*y as i64 - cy).abs());
            assert!(ring >= last);
            last = ring;
        }
    }

    #[test]
    fn first_ring_tiles_are_centre_plus_eight_neighbours() {
        let order = spiral_order(640, 640, 32);
        assert!(order.len() > super::FIRST_RING_TILES as usize);
        let (cx, cy) = (order[0].0 as i64, order[0].1 as i64);
        assert_eq!((cx, cy), ((640 / 32 - 1) as i64 / 2, (640 / 32 - 1) as i64 / 2));
        for (i, (x, y)) in order.iter().take(super::FIRST_RING_TILES as usize).enumerate() {
            let ring = (*x as i64 - cx).abs().max((*y as i64 - cy).abs());
            if i == 0 {
                assert_eq!(ring, 0, "first tile must be the centre");
            } else {
                assert_eq!(ring, 1, "tiles 1..=8 must be the first Chebyshev ring, got {ring} at {i}");
            }
        }
        // Tile 9 (0-based) starts ring 2.
        let (x, y) = order[super::FIRST_RING_TILES as usize];
        let ring = (x as i64 - cx).abs().max((y as i64 - cy).abs());
        assert!(ring >= 2);
    }

    #[test]
    fn the_ladder_starts_coarse_and_scales_with_the_pane() {
        // Big pane: 1/8 first — a complete traced frame within a dispatch
        // or two. Small panes start finer; tiny ones go straight to native.
        assert_eq!(super::start_rung_shift(1234, 1488), 3);
        assert_eq!(super::start_rung_shift(960, 540), 2);
        assert_eq!(super::start_rung_shift(300, 200), 1);
        assert_eq!(super::start_rung_shift(150, 150), 0);
    }

    #[test]
    fn the_dispatch_floor_survives_a_pinned_controller() {
        // A controller pinned at one tile still dispatches at least
        // MIN_PATHS_PER_FRAME paths within the tile-count cap.
        for edge in [32u32, 64, 128] {
            let min_tiles = super::MIN_PATHS_PER_FRAME.div_ceil(edge * edge);
            assert!(min_tiles <= super::TILES_PER_FRAME_MAX, "edge {edge}");
            assert!(min_tiles * edge * edge >= super::MIN_PATHS_PER_FRAME, "edge {edge}");
        }
    }

    #[test]
    fn diagnostic_capture_sums_rejected_samples() {
        let floats = [2.0f32, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0];
        let c = Capture {
            kind: CaptureKind::Diagnostics,
            width: 2,
            height: 1,
            bytes: floats.into_iter().flat_map(f32::to_le_bytes).collect(),
            spp: 4.0,
            elapsed: 0.0,
        };
        assert_eq!(c.rejected_samples(), 5);
    }

    #[test]
    fn metered_exposure_makes_five_milliradiance_visible() {
        let image = vec![[0.005; 3]; 16 * 16];
        let exposure = metered_exposure_from_rgb(&image, 16, 16, 1.0);
        let display = tonemap_rgb(image[0], exposure);
        let mean = (display[0] + display[1] + display[2]) / 3.0;
        assert_eq!(exposure, 16.0);
        assert!(mean > 80.0 / 255.0, "metered display mean {mean}");
    }

    #[test]
    fn view_capture_reports_picture_metrics() {
        let mut bytes = vec![0u8; 9 * 9 * 4];
        for pixel in bytes.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        bytes[(4 * 9 + 4) * 4..(4 * 9 + 4) * 4 + 4].copy_from_slice(&[255; 4]);
        let capture = Capture {
            kind: CaptureKind::View,
            width: 9,
            height: 9,
            bytes,
            spp: 256.0,
            elapsed: 0.0,
        };
        let metrics = capture.display_metrics().unwrap();
        assert!((metrics.mean_luminance - 1.0 / 81.0).abs() < 1.0e-6);
        assert!((metrics.above_eight_fraction - 1.0 / 81.0).abs() < 1.0e-6);
        assert!((metrics.firefly_ratio - 1.0 / 81.0).abs() < 1.0e-6);
    }

    #[test]
    fn reference_defaults_are_unbiased_and_per_pixel_jittered() {
        let s = RenderSettings::default();
        assert_eq!(s.preview_clamp, None);
        assert!(!s.hybrid_primary);
        assert_eq!(s.exposure, 1.0);
        assert_eq!(s.max_bounces, 8);
        assert_eq!(s.adaptive_min, 0);
    }

    #[test]
    fn full_sweep_covers_every_pixel_once() {
        let (w, h, edge) = (617u32, 744u32, 112u32);
        let order = spiral_order(w, h, edge);
        let mut counts = vec![0u32; (w * h) as usize];
        for (tx, ty) in order {
            let x0 = tx * edge;
            let y0 = ty * edge;
            let tw = edge.min(w - x0);
            let th = edge.min(h - y0);
            for y in y0..y0 + th {
                for x in x0..x0 + tw {
                    counts[(y * w + x) as usize] += 1;
                }
            }
        }
        assert!(counts.iter().all(|&c| c == 1), "after one sweep some pixels have {:?}", counts.iter().copied().filter(|c| *c != 1).take(8).collect::<Vec<_>>());
    }
}

// Keep the packing constants honest against the shader's literals.
const _: () = assert!(DATA_W == 2048 && DATA_SHIFT == 11);
const _: () = assert!(crate::pack::MAX_SHADOW_GLASS_HITS == 8);
