//! The march and the shade: SDF objects drawn per object over a screen rect,
//! in their own frame.
//!
//! **March** (`DrawSdfMarch`): sphere tracing over the object's bricks.
//! Per step, the brick index says whether the brick holds surface:
//! - an empty brick (no sample within the band, so no surface in it) is
//!   left at once: the ray jumps to the brick's exit (or its skip distance,
//!   when that is farther);
//! - a baked brick gives the distance from the render copy (two bilinear
//!   R16F reads, mixed across z: hardware trilinear on a 2D atlas); in
//!   exact mode, within the band, the brick's own culled program is
//!   evaluated instead (CAD edges stay sharp).
//! The hit writes real depth (`fragment_depth(@any)`, the pass camera's
//! projection), so raster geometry and SDF objects occlude each other per
//! pixel, and a G-buffer texel (view distance, octahedral view normal, id;
//! a section plane's cut face as a back face, id negated) or, for the checks,
//! (t, slot, normal x, y). Section planes clip the ray span.
//!
//! **Shade** (`DrawSdfShade`): no march. It reads the G-buffer the march
//! wrote, rebuilds the hit from the view distance, and shades it: PBR from
//! the material table as the model lanes shade, SDF ambient occlusion and
//! soft shadows over the render copy. A deferred light pass of its own, so
//! the march's registers never carry the shading (that alone cost the
//! march a third of its speed) and a frame marches each object once.
//!
//! **Stereo** (XR): both eyes march in ONE pass into a side-by-side target
//! (two draws per object, each its eye's half: `MarchView::eye`), the
//! coarse pass likewise at half size. The shade then runs in the XR pass,
//! whose draw list is replayed per eye (or once with multiview): it tells
//! the eyes apart by the pass camera's position and reads its eye's half of
//! the G-buffer (`shade_object_stereo`). Fragment passes only; an XR view
//! takes a smaller step budget (`MarchView::steps`).
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

    // What both passes read: the brick index, the render copy, the field.
    mod.draw.SdfField = {
        copy_tex: texture_2d(float)
        copy_slots: texture_2d(float)
        index_tex: texture_2d(float)
        copy_inv: uniform(vec2(1.0, 1.0))
        index_inv: uniform(vec2(1.0, 1.0))
        vol_min: uniform(vec3(0.0, 0.0, 0.0))
        vol_dim: uniform(vec3(1.0, 1.0, 1.0))
        cell: uniform(1.0)
        band: uniform(1.0)
        view_size: uniform(vec2(1.0, 1.0))
        // the view's origin in its target (pixels; a stereo eye's half) and
        // the target's size: the quad's clip space
        view_off: uniform(vec2(0.0, 0.0))
        target_size: uniform(vec2(1.0, 1.0))
        // sphere-tracing steps per ray (at most 256; XR takes fewer)
        max_steps: uniform(256.0)
        // per draw (uniforms, not instance fields: an instance matrix is a
        // varying the march pays for per pixel)
        inv_view_proj: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        view_m: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        proj_m: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        obj_from_world: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        world_from_obj: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        exact: uniform(1.0)
        // subtracted from the written depth
        depth_bias: uniform(0.0)

        // target pixels (y down, as the turtle's) to the view's ndc (y up)
        to_ndc: fn(px: vec2) -> vec2 {
            let v = px - self.view_off
            return vec2(v.x / self.view_size.x * 2.0 - 1.0, 1.0 - v.y / self.view_size.y * 2.0)
        }

        // target pixels to the target's clip space (the quad)
        to_clip: fn(px: vec2) -> vec2 {
            return vec2(px.x / self.target_size.x * 2.0 - 1.0, 1.0 - px.y / self.target_size.y * 2.0)
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

        // the field from the render copy only (no exact evaluation): what AO,
        // shadows and the section test read, so the program interpreter is
        // emitted at two call sites only (the march and the normal)
        field_copy: fn(p: vec3) -> f32 {
            let b = floor(p / (self.cell * 8.0))
            let e = self.brick_entry(b)
            if e.x < 0.0 {
                if e.y < 0.0 { return 0.0 - self.band }
                return self.band
            }
            return self.copy_dist(p, b, e.x)
        }

        // Sphere tracing from t to t1 along (ro, rd) (object frame; inv =
        // 1 / rd): (t, slot, hit, the last t whose step was free). A hit is
        // a distance under `cone` x t (the pixel's footprint; a coarse pass
        // passes its own, bigger pixel) or a hundredth of a cell. `use_exact`
        // is a constant at each call site: the coarse pass compiles no
        // program interpreter.
        trace: fn(ro: vec3, rd: vec3, inv: vec3, t_from: f32, t1: f32, cone: f32, use_exact: f32) -> vec4 {
            let bs = self.cell * 8.0
            var t = t_from
            var t_free = t_from
            var hit = 0.0
            var slot = 0.0
            var steps = 0.0
            for i in 0..256 {
                if t > t1 || steps >= self.max_steps { break }
                steps = steps + 1.0
                let p = ro + rd * t
                let b = floor(p / bs)
                let e = self.brick_entry(b)
                if e.x < 0.0 {
                    // an empty brick: no surface in it, leave it
                    let bmin = b * bs
                    let bmax = bmin + vec3(bs, bs, bs)
                    let te = (mix(bmin, bmax, step(vec3(0.0, 0.0, 0.0), rd)) - ro) * inv
                    let t_exit = min(min(te.x, te.y), te.z)
                    t_free = t
                    t = max(t_exit, t + abs(e.y)) + self.cell * 0.01
                } else {
                    // trilinear can overestimate near edges and corners: 0.9 of it
                    var d = self.copy_dist(p, b, e.x) * 0.9
                    if use_exact > 0.5 && self.exact > 0.5 && e.z >= 0.0 && abs(d) < self.cell * 1.5 {
                        let ev = self.eval_program(e.z, p.x, p.y, p.z)
                        d = ev.x
                        slot = ev.y
                    } else if use_exact > 0.5 {
                        let l = floor(p / self.cell - b * 8.0 + vec3(0.5, 0.5, 0.5))
                        slot = floor(self.copy_slots.sample_nearest(self.copy_uv(e.x, l.xy, min(l.z, 8.0))).x * 255.0 + 0.5)
                    }
                    let eps = max(self.cell * 0.02, t * cone)
                    if d < eps {
                        hit = 1.0
                        break
                    }
                    t_free = t
                    t = t + max(d, self.cell * 0.05)
                }
            }
            return vec4(t, slot, hit, t_free)
        }

        // the material slot at object point p (a baked brick's; 0 elsewhere)
        slot_at: fn(p: vec3) -> f32 {
            let b = floor(p / (self.cell * 8.0))
            let e = self.brick_entry(b)
            if e.x < 0.0 { return 0.0 }
            let l = floor(p / self.cell - b * 8.0 + vec3(0.5, 0.5, 0.5))
            return floor(self.copy_slots.sample_nearest(self.copy_uv(e.x, l.xy, min(l.z, 8.0))).x * 255.0 + 0.5)
        }

    }

    mod.draw.SdfMarch = mod.std.set_type_default() do #(DrawSdfMarch::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SdfInterp,
        ..mod.draw.SdfField,
        depth_out: fragment_depth(@any)
        pix_angle: uniform(0.001)
        // the coarse pass's free distances (world units; 1e30 = no surface
        // near): the march starts there
        coarse_tex: texture_2d(float)
        coarse_in: uniform(0.0)
        // 1 / the coarse target's size in pixels
        coarse_size_inv: uniform(vec2(1.0, 1.0))
        // world units per object unit (a uniform scale)
        obj_scale: uniform(1.0)
        // 1: fb0 = (t, slot, normal x, normal y) into an RGBA32F target (checks)
        debug_out: uniform(0.0)
        gbuf_id: uniform(0.0)
        // section planes in the OBJECT frame: keep a x + b y + c z + d >= 0;
        // x = count (0..6)
        clip_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip5: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // (in the shader body: a mixin's vertex loses to DrawQuad's)
        // the quad in clip space straight from its pixel rect (the pass has a
        // 3D camera; the ray uses the same pixel -> ndc map)
        vertex: fn() {
            self.pos = self.geom.pos
            let ndc = self.to_clip(self.rect_pos + self.geom.pos * self.rect_size)
            self.vertex_pos = vec4(ndc.x, ndc.y, 0.0, 1.0)
        }

        // the ray span (t0, t1, plane) cut by plane pl (number k): plane is
        // the number of the plane that last raised t0 (0 = none)
        clip_span: fn(pl: vec4, k: f32, ro: vec3, rd: vec3, s: vec3) -> vec3 {
            let dn = dot(pl.xyz, rd)
            let d0 = dot(pl.xyz, ro) + pl.w
            if abs(dn) < 0.000000001 {
                if d0 < 0.0 { return vec3(1.0, 0.0, 0.0) }
                return s
            }
            let tp = (0.0 - d0) / dn
            if dn > 0.0 {
                if tp > s.x { return vec3(tp, s.y, k) }
                return s
            }
            return vec3(s.x, min(s.y, tp), s.z)
        }

        clip_plane: fn(k: f32) -> vec4 {
            if k < 1.5 { return self.clip0 }
            if k < 2.5 { return self.clip1 }
            if k < 3.5 { return self.clip2 }
            if k < 4.5 { return self.clip3 }
            if k < 5.5 { return self.clip4 }
            return self.clip5
        }

        oct_encode: fn(n: vec3) -> vec2 {
            let l1 = abs(n.x) + abs(n.y) + abs(n.z)
            let e = n / max(l1, 0.00000001)
            if e.z >= 0.0 {
                return vec2(e.x, e.y)
            }
            let sx = step(0.0, e.x) * 2.0 - 1.0
            let sy = step(0.0, e.y) * 2.0 - 1.0
            return vec2((1.0 - abs(e.y)) * sx, (1.0 - abs(e.x)) * sy)
        }

        fragment: fn() {
            let pix = self.rect_pos + self.pos * self.rect_size
            let ndc = self.to_ndc(pix)
            let a = self.inv_view_proj * vec4(ndc.x, ndc.y, 0.0, 1.0)
            let bw = self.inv_view_proj * vec4(ndc.x, ndc.y, 1.0, 1.0)
            let ro_w = a.xyz / a.w
            let rd_w = normalize(bw.xyz / bw.w - ro_w)
            let ro = (self.obj_from_world * vec4(ro_w, 1.0)).xyz
            // unit length in the object frame (a scaled placement): t and the
            // sphere-tracing steps are object distances
            let rd = normalize((self.obj_from_world * vec4(rd_w, 0.0)).xyz)
            // the volume box
            let bs = self.cell * 8.0
            let lo = self.vol_min * bs
            let hi = (self.vol_min + self.vol_dim) * bs
            let inv = vec3(1.0, 1.0, 1.0) / rd
            let ta = (lo - ro) * inv
            let tb = (hi - ro) * inv
            let tn = min(ta, tb)
            let tf = max(ta, tb)
            var span = vec3(max(max(tn.x, tn.y), max(tn.z, 0.0)), min(min(tf.x, tf.y), tf.z), 0.0)
            // section planes only when a section is on: one uniform branch
            if self.clip_ctl.x > 0.5 {
                span = self.clip_span(self.clip0, 1.0, ro, rd, span)
                if self.clip_ctl.x > 1.5 { span = self.clip_span(self.clip1, 2.0, ro, rd, span) }
                if self.clip_ctl.x > 2.5 { span = self.clip_span(self.clip2, 3.0, ro, rd, span) }
                if self.clip_ctl.x > 3.5 { span = self.clip_span(self.clip3, 4.0, ro, rd, span) }
                if self.clip_ctl.x > 4.5 { span = self.clip_span(self.clip4, 5.0, ro, rd, span) }
                if self.clip_ctl.x > 5.5 { span = self.clip_span(self.clip5, 6.0, ro, rd, span) }
            }
            let t0 = span.x
            var t1 = span.y
            if t1 <= t0 { discard() }
            var t = t0 + self.cell * 0.01
            var hit = 0.0
            var slot = 0.0
            // a section plane entering the solid: the cut face is the hit
            var cap = 0.0
            if span.z > 0.5 {
                if self.field_copy(ro + rd * t0) < 0.0 {
                    cap = span.z
                    hit = 1.0
                    t = t0
                    slot = self.slot_at(ro + rd * t0)
                    t1 = t0 - 1.0
                }
            }
            if hit < 0.5 {
                // the coarse pass's free distance (world, the nearest object's)
                if self.coarse_in > 0.5 {
                    let cuv = pix / self.target_size
                    let c = self.coarse_tex.sample_nearest(cuv)
                    if c.x > 1000000000.0 { discard() }
                    t = max(t, c.x / self.obj_scale)
                    // the four coarse hits around this pixel: where they all
                    // hit and agree within two cells, start a cell before the
                    // nearest (a smooth surface between them), not at the
                    // conservative free distance
                    let ct = self.coarse_size_inv * 0.5
                    let h0 = self.coarse_tex.sample_nearest(cuv + vec2(0.0 - ct.x, 0.0 - ct.y)).y
                    let h1 = self.coarse_tex.sample_nearest(cuv + vec2(ct.x, 0.0 - ct.y)).y
                    let h2 = self.coarse_tex.sample_nearest(cuv + vec2(0.0 - ct.x, ct.y)).y
                    let h3 = self.coarse_tex.sample_nearest(cuv + vec2(ct.x, ct.y)).y
                    let hmin = min(min(h0, h1), min(h2, h3))
                    let hmax = max(max(h0, h1), max(h2, h3))
                    if hmax < 1000000000.0 && (hmax - hmin) < self.cell * 2.0 * self.obj_scale {
                        t = max(t, hmin / self.obj_scale - self.cell)
                    }
                }
                let tr = self.trace(ro, rd, inv, t, t1, self.pix_angle, 1.0)
                t = tr.x
                slot = tr.y
                hit = tr.z
            }
            if hit < 0.5 { discard() }
            let p = ro + rd * t
            // the normal: a tetrahedral gradient of the field
            let h = self.cell * 0.5
            let k0 = vec3(1.0, 0.0 - 1.0, 0.0 - 1.0)
            let k1 = vec3(0.0 - 1.0, 0.0 - 1.0, 1.0)
            let k2 = vec3(0.0 - 1.0, 1.0, 0.0 - 1.0)
            let k3 = vec3(1.0, 1.0, 1.0)
            var g = k0 * self.field(p + k0 * h) + k1 * self.field(p + k1 * h) + k2 * self.field(p + k2 * h) + k3 * self.field(p + k3 * h)
            if cap > 0.5 {
                // the cut face looks back along the ray
                g = 0.0 - self.clip_plane(cap).xyz
            }
            let n = normalize((self.world_from_obj * vec4(g, 0.0)).xyz)
            let hit_w4 = self.world_from_obj * vec4(p, 1.0)
            let hit_w = vec4(hit_w4.xyz / hit_w4.w, 1.0)
            // the view's own camera (a stereo eye's, not the pass's)
            let clip = self.proj_m * (self.view_m * hit_w)
            self.depth_out = clip.z / clip.w - self.depth_bias
            if self.debug_out > 0.5 {
                self.fb0 = vec4(t, slot, n.x, n.y)
            } else {
                let vp = (self.view_m * hit_w).xyz
                var vn = normalize((self.view_m * vec4(n, 0.0)).xyz)
                if dot(vn, vp) > 0.0 { vn = 0.0 - vn }
                let e = self.oct_encode(vn)
                var id = self.gbuf_id
                if cap > 0.5 { id = 0.0 - id }
                self.fb0 = vec4(length(vp), e.x, e.y, id)
            }
        }
    }

    // The coarse pass: the march at a fraction of the resolution with the
    // coarse pixel's cone, no exact evaluation, no normal: per coarse pixel
    // the farthest distance (world) its whole cone is known free of surface,
    // depth-tested so the nearest object's wins. The full march starts there.
    mod.draw.SdfCoarse = mod.std.set_type_default() do #(DrawSdfCoarse::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SdfInterp,
        ..mod.draw.SdfField,
        color_format: @Rgba32F
        alpha_blend: false
        depth_out: fragment_depth(@any)
        pix_angle: uniform(0.001)
        obj_scale: uniform(1.0)

        // (in the shader body: a mixin's vertex loses to DrawQuad's)
        vertex: fn() {
            self.pos = self.geom.pos
            let ndc = self.to_clip(self.rect_pos + self.geom.pos * self.rect_size)
            self.vertex_pos = vec4(ndc.x, ndc.y, 0.0, 1.0)
        }

        fragment: fn() {
            let pix = self.rect_pos + self.pos * self.rect_size
            let ndc = self.to_ndc(pix)
            let a = self.inv_view_proj * vec4(ndc.x, ndc.y, 0.0, 1.0)
            let bw = self.inv_view_proj * vec4(ndc.x, ndc.y, 1.0, 1.0)
            let ro_w = a.xyz / a.w
            let rd_w = normalize(bw.xyz / bw.w - ro_w)
            let ro = (self.obj_from_world * vec4(ro_w, 1.0)).xyz
            let rd = normalize((self.obj_from_world * vec4(rd_w, 0.0)).xyz)
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
            // the cone covers every full-resolution pixel of this coarse one
            let tr = self.trace(ro, rd, inv, t0 + self.cell * 0.01, t1, self.pix_angle * 1.5, 0.0)
            if tr.z < 0.5 { discard() }
            // then this coarse ray's own hit (its pixel's cone): the full
            // march interpolates where four coarse hits agree
            let th = self.trace(ro, rd, inv, tr.w, t1, self.pix_angle * 0.5, 0.0)
            var t_hit = 1.0e30
            if th.z > 0.5 { t_hit = th.x * self.obj_scale }
            let hit_w4 = self.world_from_obj * vec4(ro + rd * tr.w, 1.0)
            let clip = self.proj_m * (self.view_m * vec4(hit_w4.xyz / hit_w4.w, 1.0))
            self.depth_out = clip.z / clip.w
            self.fb0 = vec4(tr.w * self.obj_scale, t_hit, 0.0, 1.0)
        }
    }

    mod.draw.SdfShade = mod.std.set_type_default() do #(DrawSdfShade::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SdfInterp,
        ..mod.draw.SdfField,
        depth_out: fragment_depth(@any)
        mat_tex: texture_2d(float)
        // per slot: (rgb, a = 1 replaces the albedo), e.g. the filament a slot prints in
        override_tex: texture_2d(float)
        // the G-buffer the march wrote (view distance, oct normal, id)
        gbuf_tex: texture_2d(float)
        gbuf_id: uniform(0.0)
        // the G-buffer's size (pixels) and the right eye's offset in it
        gbuf_size: uniform(vec2(1.0, 1.0))
        gbuf_off_r: uniform(vec2(0.0, 0.0))
        // stereo (x = 1): the eyes' world positions tell the pass's eye
        stereo: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        eye_l: uniform(vec3(0.0, 0.0, 0.0))
        eye_r: uniform(vec3(0.0, 0.0, 0.0))
        inv_proj: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        inv_view: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        inv_proj_r: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        inv_view_r: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        ortho: uniform(0.0)
        // the sun (world, toward the light), its colour, the hemisphere
        sun_dir: uniform(vec3(0.35, 0.8, 0.45))
        sun_color: uniform(vec3(0.72, 0.72, 0.72))
        sun_sky: uniform(vec3(0.28, 0.28, 0.28))
        sun_ground: uniform(vec3(0.28, 0.28, 0.28))
        world_up: uniform(vec3(0.0, 1.0, 0.0))
        // x = AO strength (0 off), y = soft shadows (0 off, else the penumbra
        // sharpness k), z = exposure, w unused
        shade_ctl: uniform(vec4(1.0, 8.0, 1.0, 0.0))
        // x = 1: tone map + display gamma (a BGRA8 target next to a renderer
        // that does the same)
        display: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // the predicted surface finish: (pattern 0 none..5, amplitude mm,
        // scale mm, layer height mm; 0 = off); the pattern frame (centre x,
        // centre y, repeats around); plate mm from and to the pass world
        finish_ctl: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        finish_frame: uniform(vec4(0.0, 0.0, 12.0, 0.0))
        plate_from_world: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        world_from_plate: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))

        // (in the shader body: a mixin's vertex loses to DrawQuad's)
        // the quad in clip space straight from its pixel rect (the pass has a
        // 3D camera; the ray uses the same pixel -> ndc map)
        vertex: fn() {
            self.pos = self.geom.pos
            let ndc = self.to_clip(self.rect_pos + self.geom.pos * self.rect_size)
            self.vertex_pos = vec4(ndc.x, ndc.y, 0.0, 1.0)
        }

        // ---- the predicted surface finish (printing) -------------------
        // The printed wall sits inward of the model by the bead's layer
        // profile and, with a wall texture, by its pattern depth: the
        // toolpath textures of the slicer's experimental modes, ported (the
        // same pattern fields, a float hash in place of the integer one).
        // Plate millimetres, Z up.
        finish_hash3: fn(p: vec3) -> f32 {
            return fract(sin(dot(p, vec3(127.1, 311.7, 74.7))) * 43758.5453)
        }

        finish_noise3: fn(p: vec3) -> f32 {
            let i = floor(p)
            let f = p - i
            let u = f * f * (vec3(3.0, 3.0, 3.0) - f * 2.0)
            let c000 = self.finish_hash3(i)
            let c100 = self.finish_hash3(i + vec3(1.0, 0.0, 0.0))
            let c010 = self.finish_hash3(i + vec3(0.0, 1.0, 0.0))
            let c110 = self.finish_hash3(i + vec3(1.0, 1.0, 0.0))
            let c001 = self.finish_hash3(i + vec3(0.0, 0.0, 1.0))
            let c101 = self.finish_hash3(i + vec3(1.0, 0.0, 1.0))
            let c011 = self.finish_hash3(i + vec3(0.0, 1.0, 1.0))
            let c111 = self.finish_hash3(i + vec3(1.0, 1.0, 1.0))
            return mix(mix(mix(c000, c100, u.x), mix(c010, c110, u.x), u.y), mix(mix(c001, c101, u.x), mix(c011, c111, u.x), u.y), u.z)
        }

        finish_smooth: fn(t: f32) -> f32 {
            let c = clamp(t, 0.0, 1.0)
            return c * c * (3.0 - 2.0 * c)
        }

        // the pattern depth P in [0, 1] (1 = deepest); finish_ctl.x =
        // pattern (1 fuzzy, 2 knurl, 3 scales, 4 woven, 5 wood grain), z =
        // scale; finish_frame = (centre x, centre y, repeats around)
        finish_pattern: fn(q: vec3) -> f32 {
            let k = self.finish_ctl.x
            let sc = self.finish_ctl.z
            let theta = atan2(q.y - self.finish_frame.y, q.x - self.finish_frame.x)
            let u = (theta / 6.2831853 + 0.5) * self.finish_frame.z
            let vv = q.z / sc
            if k < 1.5 {
                let n = 0.7 * self.finish_noise3(q / sc) + 0.3 * self.finish_noise3(q * (2.0 / sc) + vec3(17.0, 0.0, 0.0))
                return self.finish_smooth((n - 0.15) / 0.7)
            }
            if k < 2.5 {
                let g1 = pow(cos(3.14159265 * (u + vv)), 2.0)
                let g2 = pow(cos(3.14159265 * (u - vv)), 2.0)
                return max(g1 * g1 * g1, g2 * g2 * g2)
            }
            if k < 3.5 {
                let row = floor(vv)
                let fu = fract(u + 0.5 * (row - floor(row * 0.5) * 2.0))
                let fz = vv - row
                let r = sqrt((2.0 * (fu - 0.5)) * (2.0 * (fu - 0.5)) + (1.0 - fz) * (1.0 - fz))
                return self.finish_smooth((r - 0.7) / 0.3)
            }
            if k < 4.5 {
                let cu = floor(u)
                let cv = floor(vv)
                let fu = u - cu
                let fv = vv - cv
                var across = fu
                if (cu + cv) - floor((cu + cv) * 0.5) * 2.0 < 0.5 { across = fv }
                let strand = sin(3.14159265 * across)
                let edge = min(sin(3.14159265 * fu), sin(3.14159265 * fv))
                return clamp(1.0 - 0.75 * strand * sqrt(max(edge, 0.0)), 0.0, 1.0)
            }
            let wobble = 0.6 * self.finish_noise3(vec3(u * 0.35, vv * 0.12, 3.0)) + 0.25 * self.finish_noise3(vec3(u * 1.1, vv * 0.5, 9.0))
            let line = fract(u + wobble)
            let d = abs(line - 0.5) * 2.0
            return self.finish_smooth((d - 0.7) / 0.3) * (0.6 + 0.4 * self.finish_noise3(vec3(u * 0.2, vv * 0.05, 5.0)))
        }

        // how far the printed wall sits inside the model at plate point q on
        // a wall facing `wall` (1 vertical, 0 flat): the bead's rounded
        // profile per layer (finish_ctl.w = layer height) and the texture's
        // depth (finish_ctl.y = amplitude)
        finish_depth: fn(q: vec3, wall: f32) -> f32 {
            let h = self.finish_ctl.w
            let f = (fract(q.z / h) - 0.5) * 2.0
            let bead = h * 0.5 * (1.0 - sqrt(max(1.0 - f * f, 0.0)))
            var tex = 0.0
            if self.finish_ctl.x > 0.5 {
                tex = self.finish_ctl.y * self.finish_pattern(q)
            }
            return (bead + tex) * wall
        }

        // the shading normal of the printed surface at world hit w with the
        // model's world normal n: the gradient of the inward depth along
        // the surface tilts it
        finish_normal: fn(w: vec4, n: vec3) -> vec3 {
            let q = (self.plate_from_world * w).xyz
            let nq = normalize((self.plate_from_world * vec4(n, 0.0)).xyz)
            let wall = clamp(1.0 - abs(nq.z) * 1.25, 0.0, 1.0)
            if wall <= 0.0 { return n }
            // wall > 0: the normal is off vertical, the cross is well defined
            let t1 = normalize(cross(nq, vec3(0.0, 0.0, 1.0)))
            let t2 = cross(nq, t1)
            let e = max(min(self.finish_ctl.w, self.finish_ctl.z) * 0.125, 0.005)
            let g1 = (self.finish_depth(q + t1 * e, wall) - self.finish_depth(q - t1 * e, wall)) / (2.0 * e)
            let g2 = (self.finish_depth(q + t2 * e, wall) - self.finish_depth(q - t2 * e, wall)) / (2.0 * e)
            // depth grows inward: the surface tilts toward rising depth
            let np = normalize(nq + t1 * g1 + t2 * g2)
            return normalize((self.world_from_plate * vec4(np, 0.0)).xyz)
        }

        oct_decode: fn(e: vec2) -> vec3 {
            var n = vec3(e.x, e.y, 1.0 - abs(e.x) - abs(e.y))
            if n.z < 0.0 {
                let sx = step(0.0, n.x) * 2.0 - 1.0
                let sy = step(0.0, n.y) * 2.0 - 1.0
                n = vec3((1.0 - abs(e.y)) * sx, (1.0 - abs(e.x)) * sy, n.z)
            }
            return normalize(n)
        }

        to_display: fn(linear: vec3) -> vec3 {
            if self.display.x < 0.5 { return linear }
            let mapped = clamp(
                (linear * (linear * 2.51 + vec3(0.03, 0.03, 0.03)))
                    / (linear * (linear * 2.43 + vec3(0.59, 0.59, 0.59)) + vec3(0.14, 0.14, 0.14)),
                vec3(0.0, 0.0, 0.0),
                vec3(1.0, 1.0, 1.0)
            )
            return pow(mapped, vec3(0.4545454, 0.4545454, 0.4545454))
        }

        // ambient occlusion at object point p with object normal n
        occlusion: fn(p: vec3, n: vec3) -> f32 {
            var occ = 0.0
            var w = 1.0
            var h = self.cell * 0.75
            for i in 0..5 {
                occ = occ + (h - self.field_copy(p + n * h)) * w
                w = w * 0.6
                h = h + self.cell * 1.5
            }
            return clamp(1.0 - self.shade_ctl.x * occ / (self.cell * 6.0), 0.0, 1.0)
        }

        // soft shadow toward object-space direction l (unit) from p, within
        // the volume box (lo, hi)
        soft_shadow: fn(p: vec3, l: vec3, lo: vec3, hi: vec3) -> f32 {
            let inv = vec3(1.0, 1.0, 1.0) / l
            let te = (mix(lo, hi, step(vec3(0.0, 0.0, 0.0), l)) - p) * inv
            let t_end = min(min(te.x, te.y), te.z)
            var t = self.cell * 2.0
            var res = 1.0
            for i in 0..64 {
                if t > t_end { break }
                let d = self.field_copy(p + l * t)
                res = min(res, self.shade_ctl.y * d / t)
                if res < 0.01 { break }
                t = t + clamp(d, self.cell * 0.5, self.cell * 16.0)
            }
            return clamp(res, 0.0, 1.0)
        }

        // the material slot's row r (0: albedo, rough; 1: metal, emissive,
        // unlit, fade; 2: emissive colour)
        mat_row: fn(slot: f32, r: f32) -> vec4 {
            return self.mat_tex.sample_nearest(vec2((slot + 0.5) / 256.0, (r + 0.5) / 5.0))
        }

        // The model lanes' shading (libs/render `shade_mr`), so an SDF part
        // sits in the same light as a mesh beside it: Lambert without 1/pi
        // under a rig that sums to one, a GGX sun lobe, the hemisphere along
        // the reflection gated by a roughness-aware Fresnel; a metal has no
        // diffuse. With the display transform the table's colours are
        // encoded and decode as the renderer's texels do (2.2).
        pbr: fn(n: vec3, v: vec3, slot: f32, ao: f32, shadow: f32) -> vec3 {
            let m0 = self.mat_row(slot, 0.0)
            let m1 = self.mat_row(slot, 1.0)
            let ov = self.override_tex.sample_nearest(vec2((slot + 0.5) / 256.0, 0.5))
            var albedo = mix(m0.xyz, ov.xyz, ov.w)
            if self.display.x > 0.5 {
                albedo = pow(albedo, vec3(2.2, 2.2, 2.2))
            }
            let emit = self.mat_row(slot, 2.0).xyz * m1.y
            if m1.z > 0.5 {
                return albedo + emit
            }
            let rough = clamp(m0.w, 0.04, 1.0)
            let metal = m1.x
            let l = normalize(self.sun_dir)
            let h = normalize(v + l)
            let ndl = max(dot(n, l), 0.0001)
            let ndv = max(dot(n, v), 0.0001)
            let ndh = max(dot(n, h), 0.0001)
            let vdh = max(dot(v, h), 0.0)
            let f0 = mix(vec3(0.04, 0.04, 0.04), albedo, metal)
            let f = f0 + (vec3(1.0, 1.0, 1.0) - f0) * pow(1.0 - vdh, 5.0)
            let a2 = rough * rough * rough * rough
            let den = ndh * ndh * (a2 - 1.0) + 1.0
            let dist = a2 / max(3.14159265 * den * den, 0.0001)
            let k = (rough + 1.0) * (rough + 1.0) * 0.125
            let geo = (ndv / max(ndv * (1.0 - k) + k, 0.0001)) * (ndl / max(ndl * (1.0 - k) + k, 0.0001))
            let direct = self.sun_color * (max(dot(n, l), 0.0) * shadow)
            let sun_spec = direct * (dist * geo / max(4.0 * ndv * ndl, 0.0001))
            let up = normalize(self.world_up)
            let ambient = mix(self.sun_ground, self.sun_sky, clamp(dot(n, up) * 0.5 + 0.5, 0.0, 1.0))
            let r = n * (2.0 * ndv) - v
            let env = mix(self.sun_ground, self.sun_sky, clamp(dot(r, up) * 0.5 + 0.5, 0.0, 1.0))
            let g = 1.0 - rough
            let f_env = f0 + (max(vec3(g, g, g), f0) - f0) * pow(1.0 - ndv, 5.0)
            let lit = albedo * ((1.0 - metal) * (ambient * ao + direct)) + sun_spec * f + env * f_env * ao
            return lit * self.shade_ctl.z + emit
        }

        fragment: fn() {
            let pix = self.rect_pos + self.pos * self.rect_size
            // stereo: the eye this pass renders (its camera's position) and
            // its half of the G-buffer
            var inv_proj = self.inv_proj
            var inv_view = self.inv_view
            var goff = vec2(0.0, 0.0)
            if self.stereo.x > 0.5 {
                let c4 = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
                let c = c4.xyz / c4.w
                if length(c - self.eye_r) < length(c - self.eye_l) {
                    inv_proj = self.inv_proj_r
                    inv_view = self.inv_view_r
                    goff = self.gbuf_off_r
                }
            }
            let gb = self.gbuf_tex.sample_nearest((pix + goff) / self.gbuf_size)
            if abs(abs(gb.w) - self.gbuf_id) > 0.5 { discard() }
            // the view-space ray through the pixel, then the point at the
            // G-buffer's view distance on it (perspective or orthographic)
            let ndc = self.to_ndc(pix)
            let a4 = inv_proj * vec4(ndc.x, ndc.y, 0.0, 1.0)
            let b4 = inv_proj * vec4(ndc.x, ndc.y, 1.0, 1.0)
            let av = a4.xyz / a4.w
            let dv = b4.xyz / b4.w - av
            // |av + s dv| = dist: the root in front
            let qa = dot(dv, dv)
            let qb = 2.0 * dot(av, dv)
            let qc = dot(av, av) - gb.x * gb.x
            let s = ((0.0 - qb) + sqrt(max(qb * qb - 4.0 * qa * qc, 0.0))) / (2.0 * qa)
            let vp = av + dv * s
            let hit_w4 = inv_view * vec4(vp, 1.0)
            let hit_w = vec4(hit_w4.xyz / hit_w4.w, 1.0)
            let vn = self.oct_decode(gb.yz)
            let n = normalize((inv_view * vec4(vn, 0.0)).xyz)
            let eye_w4 = inv_view * vec4(0.0, 0.0, 0.0, 1.0)
            var v = normalize(eye_w4.xyz / eye_w4.w - hit_w.xyz)
            if self.ortho > 0.5 {
                // orthographic (w row of the inverse is constant): the view axis
                v = normalize((inv_view * vec4(0.0, 0.0, 1.0, 0.0)).xyz)
            }
            self.depth_out = self.depth_of(hit_w) - self.depth_bias
            let p = (self.obj_from_world * hit_w).xyz
            let no = normalize((self.obj_from_world * vec4(n, 0.0)).xyz)
            let slot = self.slot_at(p - no * (self.cell * 0.25))
            var ao = 1.0
            if self.shade_ctl.x > 0.0 { ao = self.occlusion(p, no) }
            var shadow = 1.0
            if self.shade_ctl.y > 0.0 {
                let lo4 = normalize((self.obj_from_world * vec4(normalize(self.sun_dir), 0.0)).xyz)
                if dot(no, lo4) > 0.0 {
                    let bs = self.cell * 8.0
                    shadow = self.soft_shadow(p + no * (self.cell * 0.5), lo4, self.vol_min * bs, (self.vol_min + self.vol_dim) * bs)
                }
            }
            var ns = n
            if self.finish_ctl.w > 0.0 {
                ns = self.finish_normal(hit_w, n)
            }
            self.fb0 = vec4(self.to_display(self.pbr(ns, v, slot, ao, shadow)), 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfMarch {
    #[deref]
    pub draw_super: DrawQuad,
    /// A volume's kernels are compiled in (`kernel::KernelBake::march`):
    /// exact evaluation near the surface works for its kernels.
    #[rust]
    pub kernels: bool,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfCoarse {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSdfShade {
    #[deref]
    pub draw_super: DrawQuad,
}

/// The view of one frame: the pass camera (view, projection) and its size.
#[derive(Clone, Copy, Debug)]
pub struct MarchView {
    pub view: Mat4f,
    pub proj: Mat4f,
    pub size: DVec2,
    /// Vertical field of view (radians), for the cone stopping test.
    pub fov_y: f32,
    /// The view's origin in its target (a stereo eye's half) and the
    /// target's size, pixels.
    pub origin: DVec2,
    pub target: DVec2,
    /// Sphere-tracing steps per ray (1..=256).
    pub steps: u32,
}

/// Steps per ray of an XR eye: a frame at the headset's rate, at most a
/// little past what a smooth part needs (most rays hit in under 40).
pub const XR_STEPS: u32 = 96;

impl MarchView {
    /// A whole-target view.
    pub fn new(view: Mat4f, proj: Mat4f, size: DVec2, fov_y: f32) -> MarchView {
        MarchView { view, proj, size, fov_y, origin: DVec2::default(), target: size, steps: 256 }
    }

    /// Eye `eye` (0 left, 1 right) of a stereo pair marched side by side into
    /// one target `2 eye_size.x` wide (scaled by `scale`: the coarse pass's
    /// half size, unrounded, so its target is exactly half the march's), with
    /// the XR step budget. The vertical field of view comes from the
    /// (asymmetric) projection.
    pub fn eye(view: Mat4f, proj: Mat4f, eye_size: DVec2, eye: usize, scale: f64) -> MarchView {
        let size = DVec2 { x: eye_size.x * scale, y: eye_size.y * scale };
        let fov_y = 2.0 * (1.0 / proj.v[5]).atan();
        MarchView { view, proj, size, fov_y, origin: DVec2 { x: size.x * eye as f64, y: 0.0 }, target: DVec2 { x: size.x * 2.0, y: size.y }, steps: XR_STEPS }
    }

    /// This view's rect in its target.
    pub fn rect(&self) -> Rect {
        Rect { pos: self.origin, size: self.size }
    }

    /// World units one pixel spans at world point `p` (orthographic: everywhere alike).
    pub fn pixel_at(&self, p: [f32; 3]) -> f32 {
        let h = self.size.y.max(1.0) as f32;
        if self.proj.v[11] == 0.0 {
            return 2.0 / (self.proj.v[5].abs().max(1e-12) * h);
        }
        let v = &self.view.v;
        let z = -(v[2] * p[0] + v[6] * p[1] + v[10] * p[2] + v[14]);
        2.0 * z.max(0.0) / (self.proj.v[5].abs().max(1e-12) * h)
    }

    /// Object units one pixel spans at the volume's point nearest the eye (`world_from_obj`
    /// places it): the finest detail the view can show of it (0 with the eye inside).
    pub fn volume_pixel(&self, world_from_obj: &Mat4f, vol: &Volume) -> f32 {
        let eye = self.view.invert();
        let obj_from_world = world_from_obj.invert();
        let e = obj_from_world.transform_vec4(vec4(eye.v[12], eye.v[13], eye.v[14], 1.0));
        let bs = vol.cell * crate::volume::BS as f32;
        let lo: [f32; 3] = std::array::from_fn(|i| vol.min[i] as f32 * bs);
        let hi: [f32; 3] = std::array::from_fn(|i| (vol.min[i] + vol.dim[i]) as f32 * bs);
        let e = [e.x / e.w, e.y / e.w, e.z / e.w];
        let q: [f32; 3] = std::array::from_fn(|i| e[i].clamp(lo[i], hi[i]));
        let w = world_from_obj.transform_vec4(vec4(q[0], q[1], q[2], 1.0));
        self.pixel_at([w.x / w.w, w.y / w.w, w.z / w.w]) / obj_scale(world_from_obj).max(1e-12)
    }
}

/// What a march writes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarchOut {
    /// (t, slot, object normal x, y), RGBA32F: the checks.
    Debug,
    /// (view distance, octahedral view normal, `id`; cut faces `-id`).
    GBuffer { id: f32 },
}

/// A baker's textures as a march binds them (handles: no borrow of the baker).
struct FieldTex {
    prog: Texture,
    copy: Texture,
    copy_slots: Texture,
    index: Texture,
    prog_rows: usize,
    copy_rows: usize,
    index_rows: usize,
}

impl FieldTex {
    fn of(b: &Baker) -> FieldTex {
        FieldTex { prog: b.prog_texture().clone(), copy: b.copy.clone(), copy_slots: b.copy_slots.clone(), index: b.index_texture().clone(), prog_rows: b.prog_rows(), copy_rows: b.copy_rows, index_rows: b.index_rows() }
    }

    fn set(&self, c: &Cx, dv: &mut DrawVars, view: &MarchView, vol: &Volume, exact: bool) {
        dv.set_texture(0, &self.prog);
        dv.set_texture(1, &self.copy);
        dv.set_texture(2, &self.copy_slots);
        dv.set_texture(3, &self.index);
        dv.set_uniform(c, id!(prog_inv), &[1.0 / PROG_W as f32, 1.0 / self.prog_rows as f32]);
        dv.set_uniform(c, id!(copy_inv), &[1.0 / COPY_W as f32, 1.0 / self.copy_rows as f32]);
        dv.set_uniform(c, id!(index_inv), &[1.0 / INDEX_W as f32, 1.0 / self.index_rows as f32]);
        dv.set_uniform(c, id!(vol_min), &vol.min.map(|v| v as f32));
        dv.set_uniform(c, id!(vol_dim), &vol.dim.map(|v| v as f32));
        dv.set_uniform(c, id!(cell), &[vol.cell]);
        dv.set_uniform(c, id!(band), &[vol.band]);
        set_view(c, dv, view);
        dv.set_uniform(c, id!(exact), &[if exact { 1.0 } else { 0.0 }]);
    }
}

fn set_field(c: &Cx, dv: &mut DrawVars, view: &MarchView, vol: &Volume, baker: &Baker, exact: bool) {
    dv.set_texture(0, baker.prog_texture());
    dv.set_texture(1, &baker.copy);
    dv.set_texture(2, &baker.copy_slots);
    dv.set_texture(3, baker.index_texture());
    dv.set_uniform(c, id!(prog_inv), &[1.0 / PROG_W as f32, 1.0 / baker.prog_rows() as f32]);
    dv.set_uniform(c, id!(copy_inv), &[1.0 / COPY_W as f32, 1.0 / baker.copy_rows as f32]);
    dv.set_uniform(c, id!(index_inv), &[1.0 / INDEX_W as f32, 1.0 / baker.index_rows() as f32]);
    dv.set_uniform(c, id!(vol_min), &vol.min.map(|v| v as f32));
    dv.set_uniform(c, id!(vol_dim), &vol.dim.map(|v| v as f32));
    dv.set_uniform(c, id!(cell), &[vol.cell]);
    dv.set_uniform(c, id!(band), &[vol.band]);
    set_view(c, dv, view);
    dv.set_uniform(c, id!(exact), &[if exact { 1.0 } else { 0.0 }]);
}

fn set_view(c: &Cx, dv: &mut DrawVars, view: &MarchView) {
    dv.set_uniform(c, id!(view_size), &[view.size.x as f32, view.size.y as f32]);
    dv.set_uniform(c, id!(view_off), &[view.origin.x as f32, view.origin.y as f32]);
    dv.set_uniform(c, id!(target_size), &[view.target.x.max(1.0) as f32, view.target.y.max(1.0) as f32]);
    dv.set_uniform(c, id!(max_steps), &[view.steps.clamp(1, 256) as f32]);
}

/// Marches one object (`world_from_obj` places its frame) over `rect` (its
/// screen rect, or the whole view) into the current pass, which must use the
/// same camera and have a depth target. `planes`: section planes in the
/// world frame (keep `a x + b y + c z + d >= 0`, at most 6).
#[allow(clippy::too_many_arguments)]
pub fn draw_object(cx: &mut Cx2d, draw: &mut DrawSdfMarch, rect: Rect, view: &MarchView, world_from_obj: Mat4f, vol: &Volume, baker: &mut Baker, out: MarchOut, planes: &[[f32; 4]], coarse: Option<&Texture>, exact: bool) {
    // the march with the volume's kernels compiled in, when it has any
    let field = FieldTex::of(baker);
    let draw = baker.march(cx.cx, draw);
    let c = &*cx.cx;
    let vp = Mat4f::mul(&view.proj, &view.view);
    let dv = &mut draw.draw_super.draw_vars;
    // a volume with kernels evaluates them exactly only in a march that has
    // them compiled in; else it marches its render copy. Level of detail: where
    // a pixel spans two cells or more, the copy's sub-cell error is sub-pixel
    // and the exact evaluation is skipped (a far part costs a copy lookup a step).
    let exact = exact && (vol.kernels.is_empty() || draw.kernels) && view.volume_pixel(&world_from_obj, vol) < 2.0 * vol.cell;
    field.set(c, dv, view, vol, exact);
    match coarse {
        Some(t) => {
            dv.set_texture(4, t);
            dv.set_uniform(c, id!(coarse_in), &[1.0]);
            let (cw, ch) = (view.target.x.max(1.0) / 2.0, view.target.y.max(1.0) / 2.0);
            dv.set_uniform(c, id!(coarse_size_inv), &[1.0 / cw.ceil() as f32, 1.0 / ch.ceil() as f32]);
        }
        None => {
            // something bound: the slot is sampled only when on
            dv.set_texture(4, &field.copy);
            dv.set_uniform(c, id!(coarse_in), &[0.0]);
        }
    }
    dv.set_uniform(c, id!(obj_scale), &[obj_scale(&world_from_obj)]);
    dv.set_uniform(c, id!(pix_angle), &[2.0 * (view.fov_y * 0.5).tan() / view.size.y as f32]);
    match out {
        MarchOut::Debug => dv.set_uniform(c, id!(debug_out), &[1.0]),
        MarchOut::GBuffer { id } => {
            dv.set_uniform(c, id!(debug_out), &[0.0]);
            dv.set_uniform(c, id!(gbuf_id), &[id]);
        }
    }
    let n = planes.len().min(6);
    dv.set_uniform(c, id!(clip_ctl), &[n as f32, 0.0, 0.0, 0.0]);
    let m = &world_from_obj.v;
    for (k, name) in [id!(clip0), id!(clip1), id!(clip2), id!(clip3), id!(clip4), id!(clip5)].into_iter().enumerate() {
        let pl = planes.get(k).copied().unwrap_or([0.0; 4]);
        // (a b c) . (M p) + d: object normal = M^T n, offset = n . t + d (column-major M)
        let mut o = [0.0f32; 4];
        for j in 0..3 {
            o[j] = pl[0] * m[j * 4] + pl[1] * m[j * 4 + 1] + pl[2] * m[j * 4 + 2];
        }
        o[3] = pl[0] * m[12] + pl[1] * m[13] + pl[2] * m[14] + pl[3];
        dv.set_uniform(c, name, &o);
    }
    dv.set_uniform(c, id!(inv_view_proj), &vp.invert().v);
    dv.set_uniform(c, id!(view_m), &view.view.v);
    dv.set_uniform(c, id!(proj_m), &view.proj.v);
    dv.set_uniform(c, id!(obj_from_world), &world_from_obj.invert().v);
    dv.set_uniform(c, id!(world_from_obj), &world_from_obj.v);
    // one draw call per object: its textures and uniforms are its own
    draw.draw_super.new_draw_call(cx);
    draw.draw_super.draw_abs(cx, rect);
}

/// World units per object unit of a uniformly scaled placement.
fn obj_scale(m: &Mat4f) -> f32 {
    (m.v[0] * m.v[0] + m.v[1] * m.v[1] + m.v[2] * m.v[2]).sqrt()
}

/// The coarse pass of one object: into the current pass, whose target is
/// half the view (an RGBA32F colour cleared to `COARSE_CLEAR` and a depth:
/// x = the free distance, y = the coarse hit, both world units),
/// `view` describing that coarse size. Objects drawn into one coarse pass
/// leave the nearest one's free distance per pixel; `draw_object` with that
/// target as `coarse` starts its rays there.
#[allow(clippy::too_many_arguments)]
pub fn coarse_object(cx: &mut Cx2d, draw: &mut DrawSdfCoarse, rect: Rect, view: &MarchView, world_from_obj: Mat4f, vol: &Volume, baker: &Baker) {
    let c = &*cx.cx;
    let vp = Mat4f::mul(&view.proj, &view.view);
    let dv = &mut draw.draw_super.draw_vars;
    set_field(c, dv, view, vol, baker, false);
    dv.set_uniform(c, id!(pix_angle), &[2.0 * (view.fov_y * 0.5).tan() / view.size.y as f32]);
    dv.set_uniform(c, id!(obj_scale), &[obj_scale(&world_from_obj)]);
    dv.set_uniform(c, id!(inv_view_proj), &vp.invert().v);
    dv.set_uniform(c, id!(view_m), &view.view.v);
    dv.set_uniform(c, id!(proj_m), &view.proj.v);
    dv.set_uniform(c, id!(obj_from_world), &world_from_obj.invert().v);
    dv.set_uniform(c, id!(world_from_obj), &world_from_obj.v);
    draw.draw_super.new_draw_call(cx);
    draw.draw_super.draw_abs(cx, rect);
}

/// The predicted surface finish of a part (its print: layer lines and the
/// wall texture of the experimental modes), in plate millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finish {
    /// 0 none, 1 fuzzy, 2 knurl, 3 scales, 4 woven, 5 wood grain.
    pub pattern: u32,
    pub amplitude: f32,
    pub scale: f32,
    pub layer_height: f32,
    /// The pattern frame: the part's centre (x, y) and repeats around it.
    pub centre: [f32; 2],
    pub repeats: f32,
    /// Plate millimetres from the shade pass's world.
    pub plate_from_world: Mat4f,
}

/// Sets (or clears) the predicted surface finish for the next shades.
pub fn set_finish(cx: &Cx, draw: &mut DrawSdfShade, finish: Option<&Finish>) {
    let dv = &mut draw.draw_super.draw_vars;
    match finish {
        Some(f) => {
            dv.set_uniform(cx, id!(finish_ctl), &[f.pattern as f32, f.amplitude, f.scale.max(0.01), f.layer_height.max(0.01)]);
            dv.set_uniform(cx, id!(finish_frame), &[f.centre[0], f.centre[1], f.repeats.max(1.0), 0.0]);
            dv.set_uniform(cx, id!(plate_from_world), &f.plate_from_world.v);
            dv.set_uniform(cx, id!(world_from_plate), &f.plate_from_world.invert().v);
        }
        None => dv.set_uniform(cx, id!(finish_ctl), &[0.0, 0.0, 1.0, 0.0]),
    }
}

/// The coarse clear value: no surface near.
pub const COARSE_CLEAR: f32 = 1.0e30;

/// Shades one object from the G-buffer `gbuffer` its march wrote with `id`
/// (the same view size), into the current pass (its camera may be another
/// frame of the same view: `view` is this pass's). `depth_bias` nudges the
/// written depth toward the eye, for a depth target the march already
/// wrote with LessEqual.
#[allow(clippy::too_many_arguments)]
pub fn shade_object(cx: &mut Cx2d, draw: &mut DrawSdfShade, rect: Rect, view: &MarchView, world_from_obj: Mat4f, vol: &Volume, baker: &Baker, materials: &Texture, slot_override: &Texture, gbuffer: &Texture, id: f32, light: &MarchLight, depth_bias: f32) {
    shade(cx, draw, rect, view, None, world_from_obj, vol, baker, materials, slot_override, gbuffer, id, light, depth_bias);
}

/// Shades one object into an XR pass from a stereo G-buffer (`eyes`: the
/// two `MarchView::eye` views the march drew, full scale). The XR pass draws
/// its list once per eye (or once, multiview); this one draw serves both:
/// the shader tells the eyes apart by the pass camera's position. The quad
/// covers an eye's viewport.
#[allow(clippy::too_many_arguments)]
pub fn shade_object_stereo(cx: &mut Cx2d, draw: &mut DrawSdfShade, eyes: &[MarchView; 2], world_from_obj: Mat4f, vol: &Volume, baker: &Baker, materials: &Texture, slot_override: &Texture, gbuffer: &Texture, id: f32, light: &MarchLight, depth_bias: f32) {
    // the eye's own viewport: its rect at the origin of the pass
    let mut mono = eyes[0];
    mono.origin = DVec2::default();
    mono.target = eyes[0].size;
    shade(cx, draw, Rect { pos: DVec2::default(), size: mono.size }, &mono, Some(eyes), world_from_obj, vol, baker, materials, slot_override, gbuffer, id, light, depth_bias);
}

#[allow(clippy::too_many_arguments)]
fn shade(cx: &mut Cx2d, draw: &mut DrawSdfShade, rect: Rect, view: &MarchView, eyes: Option<&[MarchView; 2]>, world_from_obj: Mat4f, vol: &Volume, baker: &Baker, materials: &Texture, slot_override: &Texture, gbuffer: &Texture, id: f32, light: &MarchLight, depth_bias: f32) {
    let c = &*cx.cx;
    let dv = &mut draw.draw_super.draw_vars;
    set_field(c, dv, view, vol, baker, true);
    dv.set_texture(4, materials);
    dv.set_texture(5, slot_override);
    dv.set_texture(6, gbuffer);
    dv.set_uniform(c, id!(gbuf_id), &[id]);
    dv.set_uniform(c, id!(depth_bias), &[depth_bias]);
    // an orthographic projection keeps w = 1 (no z term in its last row)
    dv.set_uniform(c, id!(ortho), &[if view.proj.v[11] == 0.0 { 1.0 } else { 0.0 }]);
    dv.set_uniform(c, id!(display), &[if light.display { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);
    dv.set_uniform(c, id!(sun_dir), &light.dir);
    dv.set_uniform(c, id!(sun_color), &light.color);
    dv.set_uniform(c, id!(sun_sky), &light.sky);
    dv.set_uniform(c, id!(sun_ground), &light.ground);
    dv.set_uniform(c, id!(world_up), &light.up);
    dv.set_uniform(c, id!(shade_ctl), &[light.ao, light.shadow_k, light.exposure, 0.0]);
    let (inv_proj, inv_view) = (view.proj.invert(), view.view.invert());
    dv.set_uniform(c, id!(inv_proj), &inv_proj.v);
    dv.set_uniform(c, id!(inv_view), &inv_view.v);
    match eyes {
        Some([l, r]) => {
            let pos = |m: &Mat4f| [m.v[12] / m.v[15], m.v[13] / m.v[15], m.v[14] / m.v[15]];
            let inv_view_r = r.view.invert();
            dv.set_uniform(c, id!(inv_proj_r), &r.proj.invert().v);
            dv.set_uniform(c, id!(inv_view_r), &inv_view_r.v);
            dv.set_uniform(c, id!(gbuf_size), &[l.target.x.max(1.0) as f32, l.target.y.max(1.0) as f32]);
            dv.set_uniform(c, id!(gbuf_off_r), &[r.origin.x as f32, r.origin.y as f32]);
            dv.set_uniform(c, id!(stereo), &[1.0, 0.0, 0.0, 0.0]);
            dv.set_uniform(c, id!(eye_l), &pos(&inv_view));
            dv.set_uniform(c, id!(eye_r), &pos(&inv_view_r));
        }
        None => {
            dv.set_uniform(c, id!(inv_proj_r), &inv_proj.v);
            dv.set_uniform(c, id!(inv_view_r), &inv_view.v);
            dv.set_uniform(c, id!(gbuf_size), &[view.target.x.max(1.0) as f32, view.target.y.max(1.0) as f32]);
            dv.set_uniform(c, id!(gbuf_off_r), &[0.0, 0.0]);
            dv.set_uniform(c, id!(stereo), &[0.0, 0.0, 0.0, 0.0]);
        }
    }
    dv.set_uniform(c, id!(obj_from_world), &world_from_obj.invert().v);
    dv.set_uniform(c, id!(world_from_obj), &world_from_obj.v);
    draw.draw_super.new_draw_call(cx);
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

/// The material table the march and the splats read (256 x 5, a column per
/// slot): row 0 (albedo, rough), row 1 (metal, emissive, unlit, fade), row 2
/// (emissive colour), row 3 (splat size, opacity, fluff, size jitter), row 4
/// (splat position, rotation, brightness, colour jitter). Slots past the
/// table are the default material.
pub fn material_texels(materials: &[makepad_csg_sdf::SdfMaterial]) -> Vec<f32> {
    let mut data = vec![0.0f32; 256 * 4 * 5];
    let dflt = makepad_csg_sdf::SdfMaterial::default();
    for i in 0..256 {
        let m = materials.get(i).unwrap_or(&dflt);
        let rows = [
            [m.albedo[0], m.albedo[1], m.albedo[2], m.rough],
            [m.metal, m.emissive, if m.unlit { 1.0 } else { 0.0 }, m.fade],
            [m.emissive_color[0], m.emissive_color[1], m.emissive_color[2], 0.0],
            [m.splat.size, m.splat.opacity, m.splat.fluff, m.splat.jitter_size],
            [m.splat.jitter_pos, m.splat.jitter_rot, m.splat.jitter_bright, m.splat.jitter_color],
        ];
        for (r, row) in rows.iter().enumerate() {
            let o = (r * 256 + i) * 4;
            data[o..o + 4].copy_from_slice(row);
        }
    }
    data
}

pub fn material_texture(cx: &mut Cx, materials: &[makepad_csg_sdf::SdfMaterial]) -> Texture {
    let data = material_texels(materials);
    Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: 256, height: 5, data: Some(data), updated: TextureUpdated::Full })
}

/// The light the shaded output uses (world frame): `dir` toward the sun.
#[derive(Clone, Copy, Debug)]
pub struct MarchLight {
    pub dir: [f32; 3],
    pub color: [f32; 3],
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    pub up: [f32; 3],
    /// AO strength (0 = off), shadow sharpness (0 = no shadows), exposure.
    pub ao: f32,
    pub shadow_k: f32,
    pub exposure: f32,
    /// Tone map + display gamma (a BGRA8 target).
    pub display: bool,
}

impl Default for MarchLight {
    fn default() -> Self {
        MarchLight { dir: [0.35, 0.8, 0.45], color: [0.72; 3], sky: [0.28; 3], ground: [0.28; 3], up: [0.0, 1.0, 0.0], ao: 1.0, shadow_k: 8.0, exposure: 1.0, display: false }
    }
}
