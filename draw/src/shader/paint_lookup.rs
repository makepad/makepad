//! `mod.draw.PaintLookup`: the one paint lookup every renderer spreads, over the atlas built by
//! `libs/paint_field` (`bake::Atlas`). The host gives the texels: it defines
//! `paint_texel(i: f32) -> vec4` over whatever texture holds the atlas (its own, or rows after
//! another table in a texture it already binds), so the lookup costs no texture slot of its own.
//!
//! `paint_at(hdr, p, px)`: at point `p` of volume `hdr` (its header texel), with the pixel's
//! footprint `px` (mm, from screen derivatives), returns (slot a, slot b, coverage of a, 1).
//! Slot 0 is unpainted. The eight lattice corners round `p` are read; each corner's distance is
//! signed by whether it shows the nearest corner's slot, so the interpolated value crosses zero
//! on the boundary between the two slots, a pixel wide at any zoom.
//!
//! SHADER LAW: unary minus has the lowest precedence; write `(0.0 - a) - b`.

use crate::makepad_platform::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.PaintLookup = {
        paint_pick: fn(v: vec4, k: f32) -> f32 {
            let m = vec4(1.0 - step(0.5, k), step(0.5, k) - step(1.5, k), step(1.5, k) - step(2.5, k), step(2.5, k))
            return dot(v, m)
        }

        // the index entry of the brick holding lattice point g: a sample brick (>= 0) or a
        // whole one (-1 - slot; outside the volume: slot 0)
        paint_brick: fn(g: vec3, bmin: vec3, dim: vec3, bases: vec4) -> f32 {
            let b = floor(g / 8.0) - bmin
            if b.x < 0.0 || b.y < 0.0 || b.z < 0.0 || b.x >= dim.x || b.y >= dim.y || b.z >= dim.z {
                return 0.0 - 1.0
            }
            let i = (b.z * dim.y + b.y) * dim.x + b.x
            return self.paint_pick(self.paint_texel(bases.x + floor(i / 4.0)), modf(i, 4.0))
        }

        // (slot, distance over the ramp 0..1) of lattice point g in the brick of entry e
        paint_sample: fn(g: vec3, e: f32, bases: vec4) -> vec2 {
            if e < 0.0 {
                return vec2((0.0 - e) - 1.0, 1.0)
            }
            let l = g - floor(g / 8.0) * 8.0
            let li = (l.z * 8.0 + l.y) * 8.0 + l.x
            let v = self.paint_pick(self.paint_texel(bases.y + e * 128.0 + floor(li / 4.0)), modf(li, 4.0))
            let slot = floor(v / 256.0 + 0.001)
            return vec2(slot, (v - slot * 256.0) / 255.0)
        }

        // (slot, distance over the ramp 0..1) of lattice point g
        paint_corner: fn(g: vec3, bmin: vec3, dim: vec3, bases: vec4) -> vec2 {
            return self.paint_sample(g, self.paint_brick(g, bmin, dim, bases), bases)
        }

        paint_sign: fn(c: vec2, slot: f32) -> f32 {
            if abs(c.x - slot) < 0.5 { return c.y }
            return 0.0 - c.y
        }

        // the colour of a slot of volume `bases` (linear)
        paint_color: fn(bases: vec4, slot: f32) -> vec3 {
            return self.paint_texel(bases.z + slot).xyz
        }

        // a point of the shader's frame into volume `hdr`'s
        paint_frame: fn(hdr: f32, p: vec3) -> vec3 {
            let r0 = self.paint_texel(hdr + 3.0)
            let r1 = self.paint_texel(hdr + 4.0)
            let r2 = self.paint_texel(hdr + 5.0)
            return vec3(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w)
        }

        // `albedo` with volume `hdr`'s paint at frame point p (footprint px mm)
        paint_albedo: fn(hdr: f32, p: vec3, px: f32, albedo: vec3) -> vec3 {
            if hdr < 0.0 { return albedo }
            let h0 = self.paint_texel(hdr)
            let h1 = self.paint_texel(hdr + 1.0)
            let bases = self.paint_texel(hdr + 2.0)
            let cell = h0.x
            let bmin = vec3(h0.z, h0.w, h1.x)
            let dim = vec3(h1.y, h1.z, h1.w)
            let f = p / cell
            let g = floor(f)
            let t = f - g
            // the eight corners: one index read when they share a brick (7 in 8 cells per
            // axis), else one per corner
            var c000 = vec2(0.0, 1.0)
            var c100 = vec2(0.0, 1.0)
            var c010 = vec2(0.0, 1.0)
            var c110 = vec2(0.0, 1.0)
            var c001 = vec2(0.0, 1.0)
            var c101 = vec2(0.0, 1.0)
            var c011 = vec2(0.0, 1.0)
            var c111 = vec2(0.0, 1.0)
            let l0 = g - floor(g / 8.0) * 8.0
            if l0.x < 6.5 && l0.y < 6.5 && l0.z < 6.5 {
                let e = self.paint_brick(g, bmin, dim, bases)
                c000 = self.paint_sample(g, e, bases)
                c100 = self.paint_sample(g + vec3(1.0, 0.0, 0.0), e, bases)
                c010 = self.paint_sample(g + vec3(0.0, 1.0, 0.0), e, bases)
                c110 = self.paint_sample(g + vec3(1.0, 1.0, 0.0), e, bases)
                c001 = self.paint_sample(g + vec3(0.0, 0.0, 1.0), e, bases)
                c101 = self.paint_sample(g + vec3(1.0, 0.0, 1.0), e, bases)
                c011 = self.paint_sample(g + vec3(0.0, 1.0, 1.0), e, bases)
                c111 = self.paint_sample(g + vec3(1.0, 1.0, 1.0), e, bases)
            } else {
                c000 = self.paint_corner(g, bmin, dim, bases)
                c100 = self.paint_corner(g + vec3(1.0, 0.0, 0.0), bmin, dim, bases)
                c010 = self.paint_corner(g + vec3(0.0, 1.0, 0.0), bmin, dim, bases)
                c110 = self.paint_corner(g + vec3(1.0, 1.0, 0.0), bmin, dim, bases)
                c001 = self.paint_corner(g + vec3(0.0, 0.0, 1.0), bmin, dim, bases)
                c101 = self.paint_corner(g + vec3(1.0, 0.0, 1.0), bmin, dim, bases)
                c011 = self.paint_corner(g + vec3(0.0, 1.0, 1.0), bmin, dim, bases)
                c111 = self.paint_corner(g + vec3(1.0, 1.0, 1.0), bmin, dim, bases)
            }
            // the nearest corner's slot, and another slot among the eight
            let r = step(vec3(0.5, 0.5, 0.5), t)
            var a = mix(mix(mix(c000, c100, r.x), mix(c010, c110, r.x), r.y), mix(mix(c001, c101, r.x), mix(c011, c111, r.x), r.y), r.z).x
            var b = a
            if abs(c000.x - a) > 0.5 { b = c000.x }
            if abs(c100.x - a) > 0.5 { b = c100.x }
            if abs(c010.x - a) > 0.5 { b = c010.x }
            if abs(c110.x - a) > 0.5 { b = c110.x }
            if abs(c001.x - a) > 0.5 { b = c001.x }
            if abs(c101.x - a) > 0.5 { b = c101.x }
            if abs(c011.x - a) > 0.5 { b = c011.x }
            if abs(c111.x - a) > 0.5 { b = c111.x }
            if a < 0.5 && b < 0.5 { return albedo }
            let x0 = mix(self.paint_sign(c000, a), self.paint_sign(c100, a), t.x)
            let x1 = mix(self.paint_sign(c010, a), self.paint_sign(c110, a), t.x)
            let x2 = mix(self.paint_sign(c001, a), self.paint_sign(c101, a), t.x)
            let x3 = mix(self.paint_sign(c011, a), self.paint_sign(c111, a), t.x)
            let v = mix(mix(x0, x1, t.y), mix(x2, x3, t.y), t.z) * h0.y
            let cov = clamp(0.5 + v / max(px, cell * 0.02), 0.0, 1.0)
            var ca = albedo
            if a > 0.5 { ca = self.paint_color(bases, a) }
            var cb = albedo
            if b > 0.5 { cb = self.paint_color(bases, b) }
            return mix(cb, ca, cov)
        }
    }
}
