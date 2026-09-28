//! `Finish`: the small shader functions a theme sheet keeps reaching for.
//!
//! Shapes with cut corners, antialiased coverage, cast and inner shadows,
//! halos and neon tubes, bevel light and a gel sheen, procedural textures
//! (noise, grain, brushed and turned metal, weave, perforation, stripes),
//! screen patterns (scanlines, seven-segment digits, a pixel matrix) and a
//! few colour helpers, among them the two that lay a texture on a
//! translucent colour, so that each sheet does not write its own.
//!
//! The conventions are `Material`'s: screen space with y DOWN, distances in
//! points and negative inside. `px` is one device pixel in points
//! (`1.0 / dpi_factor`) and every edge that takes it is antialiased against
//! it. A `light` here is a direction in the screen plane TOWARD the light,
//! so a light from the top has a negative y. Every function is a pure
//! function of its arguments: nothing samples a texture or reads
//! `draw_pass.time`, so a window using them stays idle at rest.
use crate::makepad_platform::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.sdf.Material

    mod.sdf.Finish = {
        // Helpers the members share are `fn` declarations: those are in this
        // object's own scope, while a call through `Finish.` resolves in the
        // caller's.

        fn f_cover(d: float, px: float) -> float {
            return clamp(0.5 - d / max(px, 0.0001), 0.0, 1.0)
        }

        fn f_rbox(p: vec2, c: vec2, h: vec2, r: float) -> float {
            let rr = clamp(r, 0.0, min(h.x, h.y))
            let q = abs(p - c) - h + vec2(rr, rr)
            return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0, 0.0))) - rr
        }

        // Exact distance to a box whose corners are cut at 45 degrees, `k`
        // along each edge: mirrored into one corner and folded across its
        // diagonal, the point is nearest the side, the cut or the vertex
        // between them.
        fn f_chamfer(p: vec2, c: vec2, h: vec2, k: float) -> float {
            let kk = clamp(k, 0.0, min(h.x, h.y))
            var q = abs(p - c) - h
            if q.y > q.x {
                q = q.yx
            }
            q = vec2(q.x, q.y + kk)
            if q.y < 0.0 && q.y - q.x * 0.41421356 < 0.0 {
                return q.x
            }
            if q.x < q.y {
                return (q.x + q.y) * 0.70710678
            }
            return length(q)
        }

        // A sin-free hash, so it holds on every GPU at any position a
        // window reaches.
        fn f_hash(p: vec2) -> float {
            var p3 = fract(vec3(p.x, p.y, p.x) * 0.1031)
            let k = dot(p3, vec3(p3.y, p3.z, p3.x) + vec3(33.33, 33.33, 33.33))
            p3 = p3 + vec3(k, k, k)
            return fract((p3.x + p3.y) * p3.z)
        }

        fn f_noise(p: vec2) -> float {
            let i = floor(p)
            let f = p - i
            let u = f * f * f * (f * (f * 6.0 - vec2(15.0, 15.0)) + vec2(10.0, 10.0))
            let a = f_hash(i)
            let b = f_hash(i + vec2(1.0, 0.0))
            let c = f_hash(i + vec2(0.0, 1.0))
            let d = f_hash(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
        }

        fn f_lum(c: vec3) -> float {
            return dot(c, vec3(0.2126, 0.7152, 0.0722))
        }

        // One mitred segment: a bar along x (`horiz` 1) or y (0) centred on
        // `c`, pointed at 45 degrees at both ends.
        fn f_seg(q: vec2, c: vec2, horiz: float, half_len: float, s: float) -> float {
            let r = q - c
            let a = abs(mix(vec2(r.y, r.x), r, horiz))
            return max(a.y - s, (a.x + a.y - half_len) * 0.70710678)
        }

        // 1 when bit `b` (a power of two) of `mask` is set.
        fn f_bit(mask: float, b: float) -> float {
            return step(0.25, fract(floor(mask / b) * 0.5))
        }

        // The segments a digit lights, a = 1, b = 2, c = 4, d = 8, e = 16,
        // f = 32, g = 64 (a top, then clockwise, g the middle).
        fn f_mask(digit: float) -> float {
            let n = floor(digit + 0.5)
            if n < 0.5 { return 63.0 }
            if n < 1.5 { return 6.0 }
            if n < 2.5 { return 91.0 }
            if n < 3.5 { return 79.0 }
            if n < 4.5 { return 102.0 }
            if n < 5.5 { return 109.0 }
            if n < 6.5 { return 125.0 }
            if n < 7.5 { return 7.0 }
            if n < 8.5 { return 127.0 }
            if n < 9.5 { return 111.0 }
            if n < 10.5 { return 64.0 }
            return 0.0
        }

        // The cell is twice as tall as it is wide; the digit is laid out in
        // units of the cell's height, centred.
        fn f_seg7(uv: vec2, mask: float, px: float) -> float {
            let q = vec2((uv.x - 0.5) * 0.5, uv.y - 0.5)
            let hw = 0.18
            let hh = 0.41
            let s = 0.045
            let gap = 0.014
            let lh = hw - gap
            let lv = hh * 0.5 - gap
            var d = 10.0
            d = min(d, f_seg(q, vec2(0.0, -hh), 1.0, lh, s) + (1.0 - f_bit(mask, 1.0)) * 10.0)
            d = min(d, f_seg(q, vec2(hw, -hh * 0.5), 0.0, lv, s) + (1.0 - f_bit(mask, 2.0)) * 10.0)
            d = min(d, f_seg(q, vec2(hw, hh * 0.5), 0.0, lv, s) + (1.0 - f_bit(mask, 4.0)) * 10.0)
            d = min(d, f_seg(q, vec2(0.0, hh), 1.0, lh, s) + (1.0 - f_bit(mask, 8.0)) * 10.0)
            d = min(d, f_seg(q, vec2(-hw, hh * 0.5), 0.0, lv, s) + (1.0 - f_bit(mask, 16.0)) * 10.0)
            d = min(d, f_seg(q, vec2(-hw, -hh * 0.5), 0.0, lv, s) + (1.0 - f_bit(mask, 32.0)) * 10.0)
            d = min(d, f_seg(q, vec2(0.0, 0.0), 1.0, lh, s) + (1.0 - f_bit(mask, 64.0)) * 10.0)
            return f_cover(d, px)
        }

        // GEOMETRY. Each returns a signed distance in points.

        // A box centred on `c`, half size `h`, its corners cut at 45 degrees
        // `k` points along each edge.
        sd_chamfer: fn(p: vec2, c: vec2, h: vec2, k: float) -> float {
            return f_chamfer(p, c, h, k)
        }

        // The same with a cut per corner: `k` = (top left, top right,
        // bottom right, bottom left); 0 leaves that corner square.
        sd_notch: fn(p: vec2, c: vec2, h: vec2, k: vec4) -> float {
            let s = p - c
            var kk = k.x
            if s.x >= 0.0 {
                kk = mix(k.y, k.z, step(0.0, s.y))
            } else {
                kk = mix(k.x, k.w, step(0.0, s.y))
            }
            return f_chamfer(p, c, h, kk)
        }

        // A box with fully round ends.
        sd_pill: fn(p: vec2, c: vec2, h: vec2) -> float {
            return f_rbox(p, c, h, min(h.x, h.y))
        }

        sd_circle: fn(p: vec2, c: vec2, r: float) -> float {
            return length(p - c) - r
        }

        // A ring of radius `r` (to the middle of its stroke) and width `w`.
        sd_ring: fn(p: vec2, c: vec2, r: float, w: float) -> float {
            return abs(length(p - c) - r) - w * 0.5
        }

        // COVERAGE. Each returns 0..1.

        // Antialiased coverage of the inside.
        cover: fn(d: float, px: float) -> float {
            return f_cover(d, px)
        }

        // Coverage of the strip `a`..`b` points deep inside the edge. A one
        // device pixel hairline on the edge is `band(d, 0.0, px, px)`, and it
        // fills exactly one pixel row when the edge lies on a pixel boundary.
        band: fn(d: float, a: float, b: float, px: float) -> float {
            return max(f_cover(d + a, px) - f_cover(d + b, px), 0.0)
        }

        // The same strip outside the edge, `a`..`b` points out.
        ring_out: fn(d: float, a: float, b: float, px: float) -> float {
            return max(f_cover(a - d, px) - f_cover(b - d, px), 0.0)
        }

        // Premultiplied `src` laid over premultiplied `dst`.
        over: fn(dst: vec4, src: vec4) -> vec4 {
            return src + dst * (1.0 - src.a)
        }

        // A value rounded to the nearest whole device pixel: snap an edge
        // and a hairline on it lands on one pixel row.
        snap: fn(v: float, px: float) -> float {
            return floor(v / max(px, 0.0001) + 0.5) * px
        }

        // SHADOWS AND LIGHT FROM THE SHAPE.

        // The cast shadow of a rounded box (centre `c`, half size `h`,
        // corner `r`) moved by `off` and blurred by a true Gaussian of
        // `sigma` points (`Material.box_cov`): its coverage 0..1 at `p`. Lay
        // it under the face.
        drop: fn(p: vec2, c: vec2, h: vec2, r: float, off: vec2, sigma: float) -> float {
            return Material.box_cov(c + off - h, c + off + h, p, max(sigma, 0.05), clamp(r, 0.0, min(h.x, h.y)))
        }

        // The inner shadow in a well of that shape: how much of the
        // surround's shadow, moved by `off` (toward the side away from the
        // light) and blurred by `sigma`, falls at `p`. 0..1; the caller
        // clips it to the well.
        inset: fn(p: vec2, c: vec2, h: vec2, r: float, off: vec2, sigma: float) -> float {
            return 1.0 - Material.box_cov(c + off - h, c + off + h, p, max(sigma, 0.05), clamp(r, 0.0, min(h.x, h.y)))
        }

        // A halo: 1 at and inside the edge, falling off outside as a
        // bright Gaussian near part over an inverse-square tail (about 0.12
        // at `size`, 0.03 at twice it), faded out between 1.5 and 4 times
        // `size`, so a quad with four sizes of room shows no cut.
        glow: fn(d: float, size: float) -> float {
            let x = max(d, 0.0) / max(size, 0.001)
            let tail = 1.0 / ((1.0 + 1.5 * x) * (1.0 + 1.5 * x))
            return (0.5 * exp(-2.5 * x * x) + 0.5 * tail) * (1.0 - smoothstep(1.5, 4.0, x))
        }

        // A neon tube `w` points wide centred on the edge: .x the coverage
        // of its white-hot core, .y the coverage of its coloured body.
        tube: fn(d: float, w: float, px: float) -> vec2 {
            let a = abs(d)
            let body = f_cover(a - max(w * 0.5, px * 0.5), px)
            let cw = max(w * 0.12, px * 0.35)
            let core = 1.0 - smoothstep(cw, cw + max(w * 0.22, px), a)
            return vec2(core * body, body)
        }

        // Signed light on a bevelled shoulder `w` points wide just inside
        // the edge: +1 where the shoulder faces the light, -1 where it
        // faces away, 0 on the plateau. `g` is the shape's outward unit
        // gradient; clip the result with `cover`.
        bevel: fn(d: float, g: vec2, w: float, light: vec2) -> float {
            let ll = length(light)
            var l = vec2(0.0, -1.0)
            if ll > 0.0001 {
                l = light / ll
            }
            let ww = max(w, 0.0)
            let soft = clamp(ww * 0.25, 0.1, 0.5)
            let shoulder = 1.0 - smoothstep(ww - soft, ww + soft, -d)
            return clamp(dot(g, l), -1.0, 1.0) * shoulder
        }

        // The highlight band of a gel cap, 0..1, in the cap's own 0..1
        // `uv`: bright from `top` down, fading out at `bottom`, whose edge
        // rises toward the sides by `curve` (a lens-shaped reflection).
        sheen: fn(uv: vec2, top: float, bottom: float, curve: float) -> float {
            let x = uv.x * 2.0 - 1.0
            let yb = bottom - curve * x * x
            let t = (uv.y - top) / max(yb - top, 0.001)
            let body = pow(clamp(1.0 - t, 0.0, 1.0), 1.4)
            return body * smoothstep(0.0, 0.08, t) * (1.0 - smoothstep(0.7, 1.0, t))
        }

        // TEXTURE.

        // A hash 0..1 of a position.
        hash: fn(p: vec2) -> float {
            return f_hash(p)
        }

        // Smooth value noise 0..1 on a lattice of one unit.
        noise: fn(p: vec2) -> float {
            return f_noise(p)
        }

        // Four octaves of value noise, 0..1.
        fbm: fn(p: vec2) -> float {
            var v = 0.5 * f_noise(p)
            var q = vec2(p.x * 1.6 + p.y * 1.2, p.y * 1.6 - p.x * 1.2) + vec2(5.2, 1.3)
            v = v + 0.25 * f_noise(q)
            q = vec2(q.x * 1.6 + q.y * 1.2, q.y * 1.6 - q.x * 1.2) + vec2(1.7, 9.2)
            v = v + 0.125 * f_noise(q)
            q = vec2(q.x * 1.6 + q.y * 1.2, q.y * 1.6 - q.x * 1.2) + vec2(8.3, 2.8)
            v = v + 0.0625 * f_noise(q)
            return v / 0.9375
        }

        // Signed grain, -amount..amount with a triangular spread, one value
        // per device pixel: pass the position in device pixels (`p / px`).
        grain: fn(p: vec2, amount: float) -> float {
            let i = floor(p)
            return (f_hash(i) - f_hash(i + vec2(17.13, 3.71))) * amount
        }

        // Brushed streaks 0..1 running along `dir`, about `scale` points
        // apart across the brushing and long along it.
        brushed: fn(p: vec2, dir: vec2, scale: float) -> float {
            let dl = length(dir)
            var t = vec2(1.0, 0.0)
            if dl > 0.0001 {
                t = dir / dl
            }
            let s = max(scale, 0.05)
            let u = dot(p, t)
            let v = dot(p, vec2(-t.y, t.x))
            let a = f_noise(vec2(u / (s * 48.0), v / s))
            let b = f_noise(vec2(u / (s * 240.0) + 3.1, v / (s * 6.0) + 7.7))
            return clamp(a * 0.75 + b * 0.25, 0.0, 1.0)
        }

        // Turned metal round `c`: the fan a lathe's grooves throw along the
        // light's axis, over fine concentric rings. A brightness factor
        // around 1 (about 0.75..1.35); multiply the metal by it.
        spun: fn(p: vec2, c: vec2, light: vec2) -> float {
            let q = p - c
            let r = length(q)
            let ll = length(light)
            var l = vec2(0.0, -1.0)
            if ll > 0.0001 {
                l = light / ll
            }
            let k = dot(q / max(r, 0.001), l)
            let fan = k * k * k * k
            let rings = (f_noise(vec2(r * 1.3, 0.37)) - 0.5) + 0.6 * (f_noise(vec2(r * 0.41, 9.1)) - 0.5)
            let amt = smoothstep(0.0, 3.0, r)
            return 1.0 + amt * ((fan - 0.375) * 0.5 + rings * 0.16 * (0.5 + fan))
        }

        // A two-over-two twill (carbon fibre or woven cloth), cells of
        // `scale` points: brightness 0..1. Each tow floats over two cells,
        // round across and dipping where it passes under at either end,
        // and the tows across the x axis catch more light than the others.
        weave: fn(p: vec2, scale: float) -> float {
            let q = p / max(scale, 0.5)
            let cell = floor(q)
            let f = q - cell
            let n = floor(fract((cell.x + cell.y) * 0.25) * 4.0 + 0.5)
            let horiz = step(n, 1.5)
            let across = mix(f.x, f.y, horiz)
            let run = n - 2.0 * (1.0 - horiz)
            let along = (run + mix(f.y, f.x, horiz)) * 0.5
            let first = cell - mix(vec2(0.0, run), vec2(run, 0.0), horiz)
            let round = pow(sin(across * PI), 0.6)
            let fibre = 0.82 + 0.18 * f_noise(vec2(along * 3.0 + first.x * 3.7, across * 5.0 + first.y * 5.3))
            let dip = smoothstep(0.0, 0.12, along) * smoothstep(0.0, 0.12, 1.0 - along)
            return clamp(round * fibre * (0.45 + 0.55 * dip) * (0.78 + 0.22 * horiz), 0.0, 1.0)
        }

        // Coverage 0..1 of round holes of radius `r` on a `pitch` grid,
        // every other row shifted half a pitch (a grille).
        perforated: fn(p: vec2, pitch: float, r: float, px: float) -> float {
            let pt = max(pitch, px)
            let row = floor(p.y / pt)
            let q = vec2(p.x - fract(row * 0.5) * pt, p.y)
            let cc = (floor(q / pt) + vec2(0.5, 0.5)) * pt
            return f_cover(length(q - cc) - r, px)
        }

        // Coverage 0..1 of stripes at `angle` radians (their normal), one
        // every `pitch` points, `duty` 0..1 of it filled.
        stripes: fn(p: vec2, angle: float, pitch: float, duty: float, px: float) -> float {
            let pt = max(pitch, px * 2.0)
            let u = dot(p, vec2(cos(angle), sin(angle))) / pt
            let g = fract(u - duty * 0.5 + 0.5) - 0.5
            return f_cover((abs(g) - duty * 0.5) * pt, px)
        }

        // SCREENS.

        // A scanline factor, 1 - depth .. 1: the dark half of every `pitch`
        // points, soft where the pitch is wide, averaged out where it is
        // too fine to draw.
        scan: fn(y: float, px: float, pitch: float, depth: float) -> float {
            let pt = max(pitch, px)
            let g = fract(y / pt + 0.25) - 0.5
            let dark = f_cover((abs(g) - 0.25) * pt, max(px, pt * 0.25))
            let keep = smoothstep(1.5 * px, 2.0 * px, pt)
            return 1.0 - depth * mix(0.5, dark, keep)
        }

        // Coverage 0..1 of a seven-segment digit with mitred segments and a
        // small gap between them. `uv` is 0..1 over a cell twice as tall as
        // it is wide, `px` a device pixel in units of the cell's height.
        // `digit` 0..9, 10 a minus, 11 blank.
        seg7: fn(uv: vec2, digit: float, px: float) -> float {
            return f_seg7(uv, f_mask(digit), px)
        }

        // All seven segments, for the unlit ghost under the digit.
        seg7_ghost: fn(uv: vec2, px: float) -> float {
            return f_seg7(uv, 127.0, px)
        }

        // Coverage 0..1 of a pixel matrix: one slightly rounded square dot
        // per `pitch` points with a gap between, evened out where the gap
        // falls under a device pixel.
        dotgrid: fn(p: vec2, pitch: float, px: float) -> float {
            let pt = max(pitch, px)
            let q = (fract(p / pt) - vec2(0.5, 0.5)) * pt
            let hs = pt * 0.42
            let cov = f_cover(f_rbox(q, vec2(0.0, 0.0), vec2(hs, hs), pt * 0.12), px)
            return mix(0.7, cov, smoothstep(2.0 * px, 4.0 * px, pt))
        }

        // COLOUR.

        // Relative luminance of a linear or sRGB triple (Rec. 709 weights).
        lum: fn(c: vec3) -> float {
            return f_lum(c)
        }

        // Lighter toward white (+amount) or darker toward black (-amount),
        // the hue held.
        lift: fn(c: vec3, amount: float) -> vec3 {
            if amount >= 0.0 {
                return mix(c, vec3(1.0, 1.0, 1.0), min(amount, 1.0))
            }
            return c * max(1.0 + amount, 0.0)
        }

        // `c` coloured by `ink` at its own luminance, by `amount` 0..1.
        tint: fn(c: vec3, ink: vec3, amount: float) -> vec3 {
            let t = ink * (f_lum(c) / max(f_lum(ink), 0.001))
            let tc = min(max(t, vec3(0.0, 0.0, 0.0)), vec3(1.0, 1.0, 1.0))
            return mix(c, tc, clamp(amount, 0.0, 1.0))
        }

        // `c`, premultiplied, made to show `k` times as bright over whatever
        // it is laid on: its own light and the share of the ground it lets
        // through both scale. A factor on the colour alone does nothing
        // where the colour is a translucent shade of black, as a dark
        // theme's containers are; this scales what the eye sees.
        scale: fn(c: vec4, k: float) -> vec4 {
            let kk = max(k, 0.0)
            return vec4(c.rgb * kk, clamp(1.0 - (1.0 - c.a) * kk, 0.0, 1.0))
        }

        // `c`, premultiplied, made to show `s` lighter (or darker, below
        // zero) in luminance over `ground`: exactly `s` there, and the same
        // share of the brightness over any other ground. Grain, streaks and
        // a long fall are measured as such offsets, and this lays them on a
        // translucent panel as surely as on an opaque one.
        shade: fn(c: vec4, s: float, ground: vec3) -> vec4 {
            let shows = c.rgb + ground * (1.0 - c.a)
            let kk = max(1.0 + s / max(f_lum(shows), 0.02), 0.0)
            return vec4(c.rgb * kk, clamp(1.0 - (1.0 - c.a) * kk, 0.0, 1.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::makepad_platform::*;
    use crate::makepad_platform::makepad_script::script_eval;

    /// Every function in the library in one pixel function, which the
    /// compile test below builds for each backend.
    fn probe(vm: &mut ScriptVm) {
        crate::script_mod(vm);
        script_eval!(vm, {
            use mod.pod.*
            use mod.math.*
            let Finish = mod.sdf.Finish
            mod.draw.FinishProbe = mod.draw.DrawQuad{
                pixel: fn() {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let c = self.rect_size * 0.5
                    let h = c - vec2(4.0, 4.0)
                    var d = Finish.sd_chamfer(p, c, h, 3.0)
                    d = min(d, Finish.sd_notch(p, c, h, vec4(4.0, 0.0, 4.0, 0.0)))
                    d = min(d, Finish.sd_pill(p, c, h))
                    d = min(d, Finish.sd_circle(p, c, 5.0))
                    d = min(d, Finish.sd_ring(p, c, 5.0, 1.0))
                    var a = Finish.cover(d, px) + Finish.band(d, 0.0, px, px) + Finish.ring_out(d, 0.0, 2.0, px)
                    a = a + Finish.drop(p, c, h, 3.0, vec2(0.0, 2.0), 4.0) + Finish.inset(p, c, h, 3.0, vec2(0.0, 1.5), 2.0)
                    a = a + Finish.glow(d, 3.0) + Finish.snap(p.x, px)
                    let t = Finish.tube(d, 3.0, px)
                    a = a + t.x + t.y + Finish.bevel(d, vec2(0.0, -1.0), 2.0, vec2(-0.3, -0.9)) + Finish.sheen(self.pos, 0.05, 0.5, 0.2)
                    a = a + Finish.hash(p) + Finish.noise(p) + Finish.fbm(p) + Finish.grain(p / px, 0.02)
                    a = a + Finish.brushed(p, vec2(1.0, 0.0), 1.0) + Finish.spun(p, c, vec2(-0.3, -0.9)) + Finish.weave(p, 4.0)
                    a = a + Finish.perforated(p, 4.0, 1.2, px) + Finish.stripes(p, 0.785, 8.0, 0.5, px)
                    a = a * Finish.scan(p.y, px, 3.0, 0.3) + Finish.dotgrid(p, 3.0, px)
                    a = a + Finish.seg7(self.pos, 8.0, px / self.rect_size.y) + Finish.seg7_ghost(self.pos, px / self.rect_size.y)
                    var col = Finish.tint(Finish.lift(vec3(a, a, a) * 0.1, 0.2), vec3(1.0, 0.5, 0.0), 0.5)
                    col = col * Finish.lum(col)
                    let under = Finish.shade(Finish.scale(vec4(0.0, 0.0, 0.0, 0.3), 1.2), 0.01, vec3(0.3, 0.3, 0.3))
                    return Finish.over(under, vec4(col.x, col.y, col.z, 1.0) * 0.5)
                }
            }
        });
    }

    const FUNCTIONS: [&str; 35] = [
        "sd_chamfer", "sd_notch", "sd_pill", "sd_circle", "sd_ring",
        "cover", "band", "ring_out", "over", "snap",
        "drop", "inset", "glow", "tube", "bevel", "sheen",
        "hash", "noise", "fbm", "grain", "brushed", "spun", "weave", "perforated", "stripes",
        "scan", "seg7", "seg7_ghost", "dotgrid",
        "lum", "lift", "tint", "scale", "shade", "box_cov",
    ];

    fn compiled(vm: &mut ScriptVm, value: ScriptValue) -> String {
        vm.bx
            .heap
            .string_with(value, |_heap, text| text.to_string())
            .expect("the compiler answers with source")
    }

    /// The library compiles on every backend the shader compiler writes,
    /// each function reaches the emitted source, and the shadows find
    /// `Material.box_cov` although the caller only named `Finish`.
    #[test]
    fn every_function_compiles_on_every_backend() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            probe(vm);
            for (backend, value) in [
                ("metal", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.FinishProbe, "metal", false)})),
                ("hlsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.FinishProbe, "hlsl", false)})),
                ("glsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.FinishProbe, "glsl", false)})),
                ("wgsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.FinishProbe, "wgsl", false)})),
            ] {
                let text = compiled(vm, value);
                assert!(!text.starts_with("ERRORS"), "{backend} did not compile: {text}");
                for name in FUNCTIONS {
                    assert!(text.contains(&format!("_{name}(")), "{backend}: {name} is missing from:
{text}");
                }
            }
        });
    }
}
