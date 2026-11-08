//! `mod.draw.PaintLookup`: the one paint lookup every renderer spreads, over a paint atlas its
//! host writes. The host gives the texels: it defines
//! `paint_texel(i: f32) -> vec4` over whatever texture holds the atlas (its own, or rows after
//! another table in a texture it already binds), so the lookup costs no texture slot of its own.
//!
//! `paint_at(hdr, p, px)`: at point `p` of volume `hdr` (its header texel), with the pixel's
//! footprint `px` (mm, from screen derivatives), returns (slot a, slot b, coverage of a, 1).
//! Slot 0 is unpainted. The eight lattice corners round `p` are read; each corner's distance is
//! signed by whether it shows the nearest corner's slot, so the interpolated value crosses zero
//! on the boundary between the two slots, a pixel wide at any zoom. Where two slots meet in the
//! cell, the value is the Catmull-Rom cubic through the 4x4x4 points round it instead of the
//! eight's trilinear one: smooth across the cell's faces, so the edge is a smooth curve however
//! near the view comes, not a polygon of cell-long pieces. A sample is `slot * 65536 + q`, `q`
//! the distance over the ramp in 0..65535. A point past the volume reads its nearest face: a
//! part that has grown shows the paint there is until its host writes the volume again.
//!
//! `paint_shade(hdr, p, n, px, albedo)`: `paint_albedo`'s colour and, in `.w`, its light floor
//! over the pixel: the volume's header texel 2 `.w` (0 for none) times the share of the pixel
//! its paint covers. A renderer keeps the paint at that share of its colour, as its exposure
//! shows it, wherever the scene's light falls short. Paint that must read on a face no light
//! reaches sets it; colour paint leaves it 0 and keeps its lit look.
//!
//! A header may instead be an overlay: values over a regular lattice, coloured through a palette,
//! and marks per lattice cell, drawn over the part's own paint on whatever surface the renderer
//! drew.
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

        // the index entry of the brick holding lattice point g (in the volume): a sample brick
        // (>= 0) or a whole one (-1 - slot)
        paint_brick: fn(g: vec3, bmin: vec3, dim: vec3, bases: vec4) -> f32 {
            let b = floor(g / 8.0) - bmin
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
            let slot = floor(v / 65536.0 + 0.000001)
            return vec2(slot, (v - slot * 65536.0) / 65535.0)
        }

        // (slot, distance over the ramp 0..1) of lattice point g
        paint_corner: fn(g: vec3, bmin: vec3, dim: vec3, bases: vec4) -> vec2 {
            return self.paint_sample(g, self.paint_brick(g, bmin, dim, bases), bases)
        }

        paint_sign: fn(c: vec2, slot: f32) -> f32 {
            if abs(c.x - slot) < 0.5 { return c.y }
            return 0.0 - c.y
        }

        // the Catmull-Rom weights of the four points round t (0..1 between the middle two)
        paint_catmull: fn(t: f32) -> vec4 {
            let t2 = t * t
            let t3 = t2 * t
            return vec4(((0.0 - t3) + 2.0 * t2) - t, (3.0 * t3 - 5.0 * t2) + 2.0, ((0.0 - 3.0 * t3) + 4.0 * t2) + t, t3 - t2) * 0.5
        }

        // the distance signed by slot `a` (over the ramp), cubic through the 4x4x4 lattice points
        // round cell g at t in it, those past the volume read at its face
        paint_cubic: fn(g: vec3, t: vec3, a: f32, bmin: vec3, dim: vec3, bases: vec4) -> f32 {
            let wx = self.paint_catmull(t.x)
            let wy = self.paint_catmull(t.y)
            let wz = self.paint_catmull(t.z)
            let lo = bmin * 8.0
            let hi = (bmin + dim) * 8.0 - vec3(1.0, 1.0, 1.0)
            var v = 0.0
            for k in 0..4 {
                var vy = 0.0
                for j in 0..4 {
                    var vx = 0.0
                    for i in 0..4 {
                        let q = clamp(g + vec3(float(i) - 1.0, float(j) - 1.0, float(k) - 1.0), lo, hi)
                        vx = vx + self.paint_pick(wx, float(i)) * self.paint_sign(self.paint_corner(q, bmin, dim, bases), a)
                    }
                    vy = vy + self.paint_pick(wy, float(j)) * vx
                }
                v = v + self.paint_pick(wz, float(k)) * vy
            }
            return v
        }

        // the colour of a slot of volume `bases` (linear)
        paint_color: fn(bases: vec4, slot: f32) -> vec3 {
            return self.paint_texel(bases.z + slot).xyz
        }

        // a point through the three affine rows at texel `at`
        paint_rows: fn(at: f32, p: vec3) -> vec3 {
            let r0 = self.paint_texel(at)
            let r1 = self.paint_texel(at + 1.0)
            let r2 = self.paint_texel(at + 2.0)
            return vec3(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w)
        }

        // a direction through them
        paint_turn: fn(at: f32, v: vec3) -> vec3 {
            return vec3(dot(self.paint_texel(at).xyz, v), dot(self.paint_texel(at + 1.0).xyz, v), dot(self.paint_texel(at + 2.0).xyz, v))
        }

        // a point of the shader's frame into volume `hdr`'s
        paint_frame: fn(hdr: f32, p: vec3) -> vec3 {
            return self.paint_rows(hdr + 3.0, p)
        }

        // node g of an overlay's lattice with weight w in the blend: (w v, w), nothing where it
        // has no value
        overlay_node: fn(g: vec3, w: f32, dim: vec3, base: f32) -> vec2 {
            let i = (g.z * dim.y + g.y) * dim.x + g.x
            let v = self.paint_pick(self.paint_texel(base + floor(i / 4.0)), modf(i, 4.0))
            return vec2(v, 1.0) * (w * step(0.0, v))
        }

        // `albedo` under an overlay's value at q, through its palette (`dim.w`): where no node
        // near has a value, `albedo` as it is
        overlay_value: fn(q: vec3, cell: vec4, dim: vec4, base: f32, albedo: vec3) -> vec3 {
            let one = vec3(1.0, 1.0, 1.0)
            let f = (q - cell.xyz) / cell.w
            let g = clamp(floor(f), vec3(0.0, 0.0, 0.0), dim.xyz - one)
            let t = clamp(f - g, vec3(0.0, 0.0, 0.0), one)
            let u = one - t
            let d = dim.xyz + one
            var s = self.overlay_node(g, u.x * u.y * u.z, d, base)
            s = s + self.overlay_node(g + vec3(1.0, 0.0, 0.0), t.x * u.y * u.z, d, base)
            s = s + self.overlay_node(g + vec3(0.0, 1.0, 0.0), u.x * t.y * u.z, d, base)
            s = s + self.overlay_node(g + vec3(1.0, 1.0, 0.0), t.x * t.y * u.z, d, base)
            s = s + self.overlay_node(g + vec3(0.0, 0.0, 1.0), u.x * u.y * t.z, d, base)
            s = s + self.overlay_node(g + vec3(1.0, 0.0, 1.0), t.x * u.y * t.z, d, base)
            s = s + self.overlay_node(g + vec3(0.0, 1.0, 1.0), u.x * t.y * t.z, d, base)
            s = s + self.overlay_node(g + vec3(1.0, 1.0, 1.0), t.x * t.y * t.z, d, base)
            if s.y <= 0.0 { return albedo }
            let x = s.x / s.y
            let over = self.paint_texel(dim.w + 33.0)
            if x > 1.0 && over.w > 0.5 { return over.xyz }
            let k = clamp(x, 0.0, 1.0) * 32.0
            let k0 = min(floor(k), 31.0)
            return mix(self.paint_texel(dim.w + k0).xyz, self.paint_texel(dim.w + k0 + 1.0).xyz, k - k0)
        }

        // `albedo` under the mark of an overlay's cell at q, where the surface (normal n in q's
        // frame) runs along the one marked, in its kind's colour
        overlay_mark: fn(q: vec3, n: vec3, cell: vec4, dim: vec4, at: f32, albedo: vec3) -> vec3 {
            let g = clamp(floor((q - cell.xyz) / cell.w), vec3(0.0, 0.0, 0.0), dim.xyz - vec3(1.0, 1.0, 1.0))
            let i = (g.z * dim.y + g.y) * dim.x + g.x
            let m = self.paint_pick(self.paint_texel(at + floor(i / 4.0)), modf(i, 4.0))
            if m < 0.5 { return albedo }
            let r = floor(m / 16.0)
            let c = vec2(modf(r, 128.0), floor(r / 128.0)) * (2.0 / 127.0) - vec2(1.0, 1.0)
            let marked = vec3(c.x, c.y, sqrt(max(1.0 - dot(c, c), 0.0)))
            if abs(dot(normalize(n), marked)) < 0.6 { return albedo }
            let kind = modf(m, 16.0)
            let stripe = step(0.5, fract((q.x + q.y + q.z) * 0.6))
            if kind > 4.5 {
                return mix(albedo, self.paint_texel(dim.w + 33.0 + kind - 4.0).xyz, 0.8 + 0.2 * stripe)
            }
            return mix(albedo, self.paint_texel(dim.w + 33.0 + kind).xyz, 0.35 + 0.4 * stripe)
        }

        // `albedo` at frame point p of a part (normal n in that frame): the paint of volume `hdr`
        // (footprint px mm); an overlay's header shows its colours and marks over its part's paint
        paint_albedo: fn(hdr: f32, p: vec3, n: vec3, px: f32, albedo: vec3) -> vec3 {
            return self.paint_shade(hdr, p, n, px, albedo).xyz
        }

        // `paint_albedo` and, in w, the share of the pixel at its volume's light floor (an
        // overlay's colours and marks are lit as they are: none)
        paint_shade: fn(hdr: f32, p: vec3, n: vec3, px: f32, albedo: vec3) -> vec4 {
            if hdr < 0.0 { return vec4(albedo, 0.0) }
            let h0 = self.paint_texel(hdr)
            var under = hdr
            var pv = p
            if h0.x < 0.0 {
                under = h0.y
                pv = self.paint_rows(hdr + 11.0, p)
            }
            var shaded = vec4(albedo, 0.0)
            if under >= 0.0 { shaded = self.paint_volume(under, pv, px, albedo) }
            if h0.x >= 0.0 { return shaded }
            var base = shaded.xyz
            let q = self.paint_rows(hdr + 6.0, p)
            let dim = self.paint_texel(hdr + 2.0)
            if h0.z >= 0.0 { base = self.overlay_value(q, self.paint_texel(hdr + 1.0), dim, h0.z, base) }
            if h0.w >= 0.0 {
                let marks = vec4(self.paint_texel(hdr + 10.0).xyz, dim.w)
                base = self.overlay_mark(q, self.paint_turn(hdr + 6.0, n), self.paint_texel(hdr + 9.0), marks, h0.w, base)
            }
            return vec4(base, 0.0)
        }

        // A surface an overlay's values colour reads alike on any material: a metal has no
        // diffuse lobe to show them, so none is metal under them.
        paint_metal: fn(hdr: f32, metal: f32) -> f32 {
            if hdr < 0.0 { return metal }
            let h0 = self.paint_texel(hdr)
            if h0.x < 0.0 && h0.z >= 0.0 { return 0.0 }
            return metal
        }

        // `albedo` with volume `hdr`'s paint at frame point p (footprint px mm), and in w the
        // share of the pixel its paint covers times the volume's light floor
        paint_volume: fn(hdr: f32, p: vec3, px: f32, albedo: vec3) -> vec4 {
            let h0 = self.paint_texel(hdr)
            let h1 = self.paint_texel(hdr + 1.0)
            let bases = self.paint_texel(hdr + 2.0)
            let cell = h0.x
            let bmin = vec3(h0.z, h0.w, h1.x)
            let dim = vec3(h1.y, h1.z, h1.w)
            let f = clamp(p / cell, bmin * 8.0, (bmin + dim) * 8.0 - vec3(1.001, 1.001, 1.001))
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
            if a < 0.5 && b < 0.5 { return vec4(albedo, 0.0) }
            var v = 0.0
            if abs(b - a) > 0.5 {
                // two slots meet in the cell: the cubic, smooth across its faces
                v = self.paint_cubic(g, t, a, bmin, dim, bases) * h0.y
            } else {
                let x0 = mix(self.paint_sign(c000, a), self.paint_sign(c100, a), t.x)
                let x1 = mix(self.paint_sign(c010, a), self.paint_sign(c110, a), t.x)
                let x2 = mix(self.paint_sign(c001, a), self.paint_sign(c101, a), t.x)
                let x3 = mix(self.paint_sign(c011, a), self.paint_sign(c111, a), t.x)
                v = mix(mix(x0, x1, t.y), mix(x2, x3, t.y), t.z) * h0.y
            }
            let cov = clamp(0.5 + v / max(px, cell * 0.02), 0.0, 1.0)
            var ca = albedo
            if a > 0.5 { ca = self.paint_color(bases, a) }
            var cb = albedo
            if b > 0.5 { cb = self.paint_color(bases, b) }
            let painted = mix(step(0.5, b), step(0.5, a), cov)
            return vec4(mix(cb, ca, cov), painted * bases.w)
        }
    }
}
