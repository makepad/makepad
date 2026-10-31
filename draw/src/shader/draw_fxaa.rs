//! FXAA (the 3.11 quality algorithm: the local edge's direction, a walk along it to both of
//! its ends, the pixel moved across the edge by its distance to the nearer end, plus a sub-pixel
//! blend) over an image drawn into a target of its size: an anti-aliasing pass over a finished
//! picture. `split` > 0: the image is that many columns side by side (a stereo pair, each eye a
//! column): every tap stays inside its own column, no eye bleeds into the other.

use crate::{makepad_platform::*, shader::draw_quad::DrawQuad};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawFxaa = mod.std.set_type_default() do #(DrawFxaa::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        // the image's size in pixels
        extent: vec2(1.0, 1.0)
        // columns side by side (0 or 1: one image)
        split: 1.0
        // the sub-pixel blend's strength (0: edges only, 1: the softest)
        subpix: 0.5
        // the column of pixel position `p` (its x range)
        column: fn(p: vec2) -> vec2 {
            let cols = max(self.split, 1.0)
            let cw = self.extent.x / cols
            let c = clamp(floor(p.x / cw), 0.0, cols - 1.0)
            return vec2(c * cw + 0.5, (c + 1.0) * cw - 0.5)
        }
        // a bilinear tap at pixel position `p`, kept inside column `col`
        tap: fn(p: vec2, col: vec2) -> vec4 {
            let q = vec2(clamp(p.x, col.x, col.y), clamp(p.y, 0.5, self.extent.y - 0.5))
            return self.image.sample(q / self.extent)
        }
        luma: fn(c: vec4) -> float {
            return dot(c.xyz, vec3(0.299, 0.587, 0.114))
        }
        lum: fn(p: vec2, col: vec2) -> float {
            return self.luma(self.tap(p, col))
        }
        pixel: fn() {
            let p = floor(self.pos * self.rect_size) + vec2(0.5, 0.5)
            let col = self.column(p)
            let rgb_m = self.tap(p, col)
            let m = self.luma(rgb_m)
            var n = self.lum(p + vec2(0.0, -1.0), col)
            var s = self.lum(p + vec2(0.0, 1.0), col)
            let e = self.lum(p + vec2(1.0, 0.0), col)
            let w = self.lum(p + vec2(-1.0, 0.0), col)
            let lmax = max(max(max(n, s), max(e, w)), m)
            let lmin = min(min(min(n, s), min(e, w)), m)
            let range = lmax - lmin
            let nsew = n + s + e + w
            // no edge here: the pixel as it is
            if range < max(0.0312, lmax * 0.125) {
                return rgb_m
            }
            let nw = self.lum(p + vec2(-1.0, -1.0), col)
            let se = self.lum(p + vec2(1.0, 1.0), col)
            let ne = self.lum(p + vec2(1.0, -1.0), col)
            let sw = self.lum(p + vec2(-1.0, 1.0), col)
            // the edge's direction: horizontal (it runs along x, 1) or vertical (0)
            let edge_h = abs(-2.0 * w + nw + sw) + abs(-2.0 * m + n + s) * 2.0 + abs(-2.0 * e + ne + se)
            let edge_v = abs(-2.0 * n + nw + ne) + abs(-2.0 * m + w + e) * 2.0 + abs(-2.0 * s + sw + se)
            let horz = step(edge_v, edge_h)
            if horz < 0.5 {
                n = w
                s = e
            }
            // which side of the pixel the edge lies on (1: the n side)
            let grad_n = n - m
            let grad_s = s - m
            let pair_n = step(abs(grad_s), abs(grad_n))
            let gradient = max(abs(grad_n), abs(grad_s))
            let step_len = mix(1.0, -1.0, pair_n)
            let l_edge = mix(s + m, n + m, pair_n) * 0.5
            // the walk starts on the edge, half a pixel across, and goes along it both ways
            let along = mix(vec2(0.0, 1.0), vec2(1.0, 0.0), horz)
            let across = mix(vec2(step_len * 0.5, 0.0), vec2(0.0, step_len * 0.5), horz)
            let start = p + across
            var pn = start - along
            var pp = start + along
            var end_n = self.lum(pn, col) - l_edge
            var end_p = self.lum(pp, col) - l_edge
            let scaled = gradient * 0.25
            var done_n = step(scaled, abs(end_n))
            var done_p = step(scaled, abs(end_p))
            var k = 0.0
            while k < 10.0 {
                // the steps grow along the walk (1.5, 2, 2, 2, 2, 4, 8, 8, 8, 8 pixels)
                var stride = 2.0
                if k < 0.5 { stride = 1.5 }
                if k > 4.5 { stride = 4.0 }
                if k > 5.5 { stride = 8.0 }
                if done_n < 0.5 {
                    pn = pn - along * stride
                    end_n = self.lum(pn, col) - l_edge
                    done_n = step(scaled, abs(end_n))
                }
                if done_p < 0.5 {
                    pp = pp + along * stride
                    end_p = self.lum(pp, col) - l_edge
                    done_p = step(scaled, abs(end_p))
                }
                k = k + 1.0
            }
            let dist_n = dot(p - pn, along)
            let dist_p = dot(pp - p, along)
            // the nearer end decides: its luma must turn the other way than the pixel's
            let m_below = 1.0 - step(l_edge, m)
            let near_n = 1.0 - step(dist_p, dist_n)
            let end_near = mix(end_p, end_n, near_n)
            let good = abs((1.0 - step(0.0, end_near)) - m_below)
            let dist = min(dist_n, dist_p)
            var offset = good * (0.5 - dist / max(dist_n + dist_p, 0.0001))
            // the sub-pixel blend: how much the pixel differs from its neighbours' average
            let avg = (nsew * 2.0 + nw + ne + sw + se) / 12.0
            let sub_c = clamp(abs(avg - m) / range, 0.0, 1.0)
            let sub_f = (-2.0 * sub_c + 3.0) * sub_c * sub_c
            offset = max(offset, sub_f * sub_f * self.subpix)
            let q = p + mix(vec2(step_len * offset, 0.0), vec2(0.0, step_len * offset), horz)
            return self.tap(q, col)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFxaa {
    #[deref]
    pub draw_super: DrawQuad,
    /// The image's size in pixels.
    #[live]
    pub extent: Vec2f,
    /// Columns side by side, each anti-aliased on its own (a stereo pair: 2).
    #[live]
    pub split: f32,
    /// The sub-pixel blend's strength (0..1).
    #[live]
    pub subpix: f32,
}
