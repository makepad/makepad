// Source: src/shaders/CORTAOBase.h, CORTAOPrepass.h, CORTAOMainPass.h, CORTAOFilterPass.h, CORTAO v1.0.2, commit f1b18d7772.
// MIT License
//
// Copyright (c) 2026 Daniel Isheden
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
//
// Modified:
// - Splash fullscreen fragment passes, all f32, uniform tier/projection/filter
//   branches and bounded loops; no compute dispatch, half math or bent normals.
// - A minimum occluder height (CortaoConfig::min_occluder, 0 by default): a tap
//   counts only when it rises more than that above the centre's tangent plane, and
//   that plane's normal is then taken from the depth twice the floor away.
// - Hardware-depth surface and reconstructed-normal host hooks, rigid -z view,
//   y-down pixels, clear-depth rejection and relative sliver/zero-length guards.
// - Inverse child lookup writes an R32F atlas, clearing unused texels; normal
//   dither is always the selected level-0 pixel's. Seed arrives as exact halves.
// - The atlas replaces truncated float depth with a 22-bit depth code, linear
//   for orthographic and pseudo-log for perspective: rounded shifted float bits
//   of depth / d_lo, decoded with integer ops, a bitcast and one multiply.
//   The stored code stays a normal float above the ten normal bits.
// - A second R32F target holds 11+11-bit octahedral centre normals in [1, 2);
//   occluder/filter normals retain the original ten-bit spheremap packing.
// - Nearest atlas reads explicitly reject borders and round/clamp mip levels.
// - Skew-aware projection/unprojection uses three columns instead of matrices;
//   guard both Frisvad bases at the negative pole and singular sphere mappings.
// - Per-tap constants are generated on the CPU in f64 and read from a uniform
//   array; the tap loop runs to a runtime count under a literal bound.
// - The default normal takes the central difference where the centre is a
//   one-pixel outlier (z-fighting layers), the closer neighbour elsewhere.
// - LUT is a 2D stack of slices with exact low/high halves and clamped radius.
// - Literal-bound loops avoid shader iteration guards in all three passes.
// - Ray bits start at zero (no descriptor preload); SWAR replaces bitCount.
// - Empty pixels write visibility one. Filter reads individual pixels, preserving
//   gather quad order; off-image quad partners use centre depth, taps weigh zero.
// - A temporal resolve (not in CORTAO) follows the filter: it reprojects each
//   pixel into the previous frame's view, rejects history off-screen, over the
//   background, more than 5% off the expected depth or facing another way
//   (normals within about 37 degrees), clamps it to the current filtered AO's 3x3
//   mean within one deviation (inside the 3x3 range) and blends it in; history
//   keeps the depth and the normal.
// - Half-resolution AO ends with a four-tap depth-aware upsample, reusing the
//   prepass's host surface hook and bindings at full-resolution pixel centres.

//! The only host hooks are the prepass's `surface` and `surface_normal`
//! (which may take `depth_normal`, the normal of `surface`'s positions).
//! The half-resolution upsample mirrors that prepass's `surface` hook.

use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // Lexical functions keep the algorithm outside the host's draw overrides.
    let cortao_seed = fn(p: vec2u) -> u32 {
        let q = p * vec2u(1719413u * 929u, 140473u * 2467u * 11u)
        return q.x ^ q.y
    }
    let cortao_hash = fn(n: u32) -> u32 {
        return n * (n ^ (n >> 15u))
    }
    let cortao_hash2 = fn(n: vec2u) -> vec2u {
        return vec2u(cortao_hash(n.x), cortao_hash(n.y))
    }
    let cortao_sign = fn(v: vec2f) -> vec2f {
        var s = vec2(1.0, 1.0)
        if v.x < 0.0 { s.x = -1.0 }
        if v.y < 0.0 { s.y = -1.0 }
        return s
    }
    let cortao_oct_encode = fn(n: vec3f) -> u32 {
        let v = n / (abs(n.x) + abs(n.y) + abs(n.z))
        var oct = v.xy
        if v.z < 0.0 {
            oct = (vec2(1.0, 1.0) - abs(oct.yx)) * cortao_sign(oct)
        }
        let q = round((oct * 0.5 + vec2(0.5, 0.5)) * 2047.0)
        return 1065353216u | (u32(q.x) << 11u) | u32(q.y)
    }
    let cortao_oct_decode = fn(bits: u32) -> vec3f {
        let e = vec2(f32((bits >> 11u) & 2047u), f32(bits & 2047u)) * (2.0 / 2047.0) - vec2(1.0, 1.0)
        var n = vec3(e, 1.0 - abs(e.x) - abs(e.y))
        if n.z < 0.0 {
            n = vec3((vec2(1.0, 1.0) - abs(e.yx)) * cortao_sign(e), n.z)
        }
        return n / sqrt(dot(n, n))
    }
    let cortao_encode_normal = fn(n: vec3f, dither: vec2f) -> u32 {
        let e = n.xy / sqrt(max(fma(n.z, 2.0, 2.0), 0.000000000001))
        let q = floor(fma(e, vec2(15.0, 15.0), dither))
        return (u32(i32(q.x)) & 31u) | ((u32(i32(q.y)) & 31u) << 5u)
    }
    // A 22-bit depth code above the normal's ten bits: the sign bit, then the
    // 21-bit field `code % 2080768 + 8192`, whose exponent stays in 1..=254, so
    // the texel is always a normal float and never 0 (no surface).
    // Law 1 is linear, [1, d_lo, s, 1/s]. Law 0 is pseudo-log,
    // [0, d_lo, 1/d_lo, k]: round the ratio's float bits above 1.0 by k bits.
    // Steps are d_lo * 2^(floor(log2(depth / d_lo)) + k - 23); open windows
    // use k = 6, about 31.75 float-bit binades, closed ones the least fitting k.
    let cortao_depth_pack = fn(depth: float, code: vec4f) -> u32 {
        var c = 0u
        if code.x > 0.5 {
            c = u32(clamp(round((depth - code.y) * code.z), 0.0, 4161535.0))
        } else {
            let k = u32(code.w)
            c = (asuint(max(depth * code.z, 1.0)) - 1065353216u + ((1u << k) >> 1u)) >> k
            if c > 4161535u { c = 4161535u }
        }
        // The upper half takes the sign bit: skip exponents 255 (sign 0) and 0 (sign 1).
        if c >= 2080768u { c = c + 16384u }
        return (c + 8192u) << 10u
    }
    let cortao_depth_unpack = fn(packed: u32, code: vec4f) -> float {
        let c = (packed >> 10u) - 8192u - ((packed >> 31u) << 14u)
        if code.x > 0.5 { return code.y + f32(c) * code.w }
        return code.y * asfloat((c << u32(code.w)) + 1065353216u)
    }
    let cortao_normal_fields = fn(bits: u32) -> vec2f {
        // Sign extend each five-bit field without signed-shift assumptions.
        return vec2(f32((bits & 31u) ^ 16u) - 16.0, f32(((bits >> 5u) & 31u) ^ 16u) - 16.0)
    }
    let cortao_decode_normal = fn(bits: u32) -> vec3f {
        let e = cortao_normal_fields(bits) * (1.0 / 15.0)
        // Dithering at the spheremap rim can round just beyond its disc.
        let z = max(fma(-e.x, e.x, fma(-e.y, e.y, 1.0)), 0.0)
        return vec3(e * (2.0 * sqrt(z)), fma(z, 2.0, -1.0))
    }
    let cortao_level_x = fn(k: u32, a: vec4f, b: vec4f) -> float {
        if k == 1u { return a.x }
        if k == 2u { return a.y }
        if k == 3u { return a.z }
        if k == 4u { return a.w }
        if k == 5u { return b.x }
        if k == 6u { return b.y }
        if k == 7u { return b.z }
        return 0.0
    }
    let cortao_level_size = fn(size: vec2f, k: u32) -> vec2f {
        return vec2(f32(max(u32(size.x) >> k, 1u)), f32(max(u32(size.y) >> k, 1u)))
    }
    let cortao_unproject_offset = fn(uv: vec2f, depth: float, u: vec4f, skew: float, ortho: float, offset: vec3f) -> vec3f {
        let dir = vec2(fma(uv.x, u.x, fma(uv.y, skew, u.z)), fma(uv.y, u.y, u.w))
        if ortho > 0.5 { return vec3(dir, -depth) + offset }
        // Keep the relative position's subtraction inside the original FMA.
        return fma(vec3(dir, -1.0), vec3(depth, depth, depth), offset)
    }
    let cortao_project = fn(p: vec3f, projection: vec4f, skew: float) -> vec3f {
        return vec3(projection.x * p.x + skew * p.y + projection.z * p.z,
                    projection.y * p.y + projection.w * p.z, -p.z)
    }
    let cortao_tangent = fn(n: vec3f) -> vec3f {
        // Frisvad's arbitrary orthonormal tangent basis construction.
        if n.z < -0.999 { return vec3(0.0, -1.0, 0.0) }
        let a = 1.0 / (1.0 + n.z)
        return vec3(fma(n.x * n.x, a, -1.0), n.x * n.y * a, n.x)
    }
    let cortao_bitangent = fn(n: vec3f) -> vec3f {
        if n.z < -0.999 { return vec3(-1.0, 0.0, 0.0) }
        let a = 1.0 / (1.0 + n.z)
        return vec3(n.x * n.y * a, fma(n.y * n.y, a, -1.0), n.y)
    }
    // Both neighbours' depth steps from the centre point the same way and are
    // within a factor of two of each other.
    let cortao_outlier = fn(a: float, b: float) -> bool {
        return a * b > 0.0 && min(abs(a), abs(b)) > 0.5 * max(abs(a), abs(b))
    }
    let cortao_popcount = fn(bits: u32) -> u32 {
        var x = bits - ((bits >> 1u) & 1431655765u)
        x = (x & 858993459u) + ((x >> 2u) & 858993459u)
        x = (x + (x >> 4u)) & 252645135u
        return (x * 16843009u) >> 24u
    }

    mod.draw.CortaoTaps = struct { taps: array{vec4f 48} }

    mod.draw.DrawCortaoPrepass = mod.std.set_type_default() do #(DrawCortaoPrepass::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Rf32
        depth_tex: texture_2d(float)
        // xy = input pixels, z = levels, w = atlas height.
        u_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_depth_code: uniform(vec4(1.0, 0.0, 1.0, 1.0))
        u_offsets_a: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_offsets_b: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_seed: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_proj_mode: uniform(0.0)
        u_view_from_clip: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        frag: fragment_position()
        fb1: fragment_output(1, vec4f)
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        // Host hook: an out-of-image pixel or missing surface has w = 0.
        surface: fn(pixel: vec2f) -> vec4f {
            if pixel.x < 0.0 || pixel.y < 0.0 || pixel.x >= self.u_size.x || pixel.y >= self.u_size.y {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let uv = (pixel + vec2(0.5, 0.5)) / self.u_size.xy
            let depth = self.depth_tex.sample_nearest(uv).x
            if depth >= 0.999999 { return vec4(0.0, 0.0, 0.0, 0.0) }
            let h = self.u_view_from_clip * vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0)
            if abs(h.w) <= 0.000000000001 { return vec4(0.0, 0.0, 0.0, 0.0) }
            let p = h.xyz / h.w
            if p.z >= 0.0 { return vec4(0.0, 0.0, 0.0, 0.0) }
            return vec4(p, 1.0)
        }

        // Host hook: the surface's view-space normal. A host whose surface stores
        // none (or not at every pixel) takes `depth_normal`'s.
        surface_normal: fn(pixel: vec2f, p: vec3f) -> vec3f {
            return self.depth_normal(pixel, p)
        }

        // The normal of `surface`'s positions around the pixel.
        depth_normal: fn(pixel: vec2f, p: vec3f) -> vec3f {
            let cl = self.surface(pixel + vec2(-1.0, 0.0))
            let cr = self.surface(pixel + vec2(1.0, 0.0))
            let cu = self.surface(pixel + vec2(0.0, -1.0))
            let cd = self.surface(pixel + vec2(0.0, 1.0))
            // Per axis the side whose depth is closer to the centre's wins, so a
            // silhouette's far side never smears a false slope across the edge.
            // A centre both sides step away from by comparable amounts on the
            // same side is a one-pixel outlier (z-fighting layers, a crest): the
            // central difference spans it, where either side alone would tilt
            // the normal and let the surface occlude itself.
            var dx = vec3(0.0, 0.0, 0.0)
            var dy = vec3(0.0, 0.0, 0.0)
            if cr.w > 0.0 { dx = cr.xyz - p }
            if cl.w > 0.0 && (cr.w <= 0.0 || abs(cl.z - p.z) < abs(cr.z - p.z)) { dx = p - cl.xyz }
            if cl.w > 0.0 && cr.w > 0.0 && cortao_outlier(cl.z - p.z, cr.z - p.z) { dx = (cr.xyz - cl.xyz) * 0.5 }
            if cd.w > 0.0 { dy = cd.xyz - p }
            if cu.w > 0.0 && (cd.w <= 0.0 || abs(cu.z - p.z) < abs(cd.z - p.z)) { dy = p - cu.xyz }
            if cu.w > 0.0 && cd.w > 0.0 && cortao_outlier(cu.z - p.z, cd.z - p.z) { dy = (cd.xyz - cu.xyz) * 0.5 }
            var eye = vec3(0.0, 0.0, 1.0)
            if self.u_proj_mode < 0.5 && dot(p, p) > 0.0 { eye = -p / sqrt(dot(p, p)) }
            var n = cross(dy, dx)
            // Relative area keeps close surfaces from being mistaken for slivers.
            if length(n) <= 0.000001 * length(dx) * length(dy) { return eye }
            n = n / sqrt(dot(n, n))
            if dot(n, eye) < 0.0 { n = -n }
            return n
        }

        fragment: fn() {
            self.fb0 = vec4(0.0, 0.0, 0.0, 1.0)
            self.fb1 = vec4(0.0, 0.0, 0.0, 1.0)
            let texel = floor(self.frag.xy)
            var pixel = texel
            var level = 0u
            var found = texel.y < self.u_size.y
            let levels = u32(self.u_size.z)
            for k in 1..8 {
                if k < levels {
                    let size = cortao_level_size(self.u_size.xy, k)
                    let origin = vec2(cortao_level_x(k, self.u_offsets_a, self.u_offsets_b), self.u_size.y)
                    let t = texel - origin
                    if t.x >= 0.0 && t.y >= 0.0 && t.x < size.x && t.y < size.y {
                        level = k
                        pixel = t
                        found = true
                    }
                }
            }
            if found {
                let seed = u32(self.u_seed.x) | (u32(self.u_seed.y) << 16u)
                var x = u32(pixel.x)
                var y = u32(pixel.y)
                for s in 0..7 {
                    let j = 7u - s
                    if j <= level {
                        let nx = 2u * x + ((y ^ (seed >> (2u * j))) & 1u)
                        let ny = 2u * y + ((x ^ (seed >> (2u * j + 1u))) & 1u)
                        x = nx
                        y = ny
                    }
                }
                let source = vec2(f32(x), f32(y))
                let surface = self.surface(source)
                if surface.w > 0.0 && surface.z < 0.0 {
                    let normal = self.surface_normal(source, surface.xyz)
                    let random = cortao_hash(cortao_seed(vec2u(x, y)) + seed)
                    let dither = vec2(f32(random & 65535u), f32(random >> 16u)) * (1.0 / 65535.0)
                    let packed = cortao_depth_pack(-surface.z, self.u_depth_code) | (cortao_encode_normal(normal, dither) & 1023u)
                    self.fb0 = vec4(asfloat(packed), 0.0, 0.0, 1.0)
                    if level == 0u { self.fb1 = vec4(asfloat(cortao_oct_encode(normal)), 0.0, 0.0, 1.0) }
                }
            }
        }
    }

    mod.draw.DrawCortaoMain = mod.std.set_type_default() do #(DrawCortaoMain::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Ru8
        atlas_tex: texture_2d(float)
        normals_tex: texture_2d(float)
        table_tex: texture_2d(float)
        taps: uniform_buffer(draw.CortaoTaps)
        u_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_depth_code: uniform(vec4(1.0, 0.0, 1.0, 1.0))
        u_offsets_a: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_offsets_b: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_seed: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        u_proj_mode: uniform(0.0)
        u_counts: uniform(vec4(12.0, 16.0, 0.0, 0.0))
        u_unproject: uniform(vec4(1.0, -1.0, -0.5, 0.5))
        u_projection: uniform(vec4(1.0, -1.0, -0.5, -0.5))
        // xy = unprojection/projection skew, zw = normal bias / radius.
        u_geometry: uniform(vec4(0.0, 0.0, 0.001, 0.6))
        // apparent-radius bound, mip bias, inverse slope limit, backface threshold.
        u_limits: uniform(vec4(0.2, -4.0, 0.25, 0.5))
        u_backface_boost: uniform(0.5)
        // The smallest relief that occludes (CortaoConfig::min_occluder), world units.
        u_min_occluder: uniform(0.0)
        frag: fragment_position()
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        // The view-space step from the centre to the atlas surface at pixel `q` (xyz) and that
        // surface's depth less the centre's (w); w is 1e30 where `q` is off the image or empty.
        floor_point: fn(q: vec2f, atlas_size: vec2f, position: vec3f, depth: float) -> vec4f {
            if q.x < 0.0 || q.y < 0.0 || q.x >= self.u_size.x || q.y >= self.u_size.y {
                return vec4(0.0, 0.0, 0.0, 1000000000000000000000000000000.0)
            }
            let packed = asuint(self.atlas_tex.sample_nearest((q + vec2(0.5, 0.5)) / atlas_size).x)
            if packed == 0u {
                return vec4(0.0, 0.0, 0.0, 1000000000000000000000000000000.0)
            }
            let d = cortao_depth_unpack(packed, self.u_depth_code)
            return vec4(cortao_unproject_offset((q + vec2(0.5, 0.5)) / self.u_size.xy, d, self.u_unproject, self.u_geometry.x, self.u_proj_mode, -position), d - depth)
        }

        // One axis of the centre's plane at the relief floor's scale, as a view-space step along
        // +offset, from the surfaces `offset` pixels away either side. Where their depths part
        // from the centre's by amounts within twice the floor of each other, the centre's own
        // relief (a crest, a groove, a bead) is under the floor and the central difference
        // spans it; otherwise the side nearer the centre's depth (an edge's far side is
        // skipped). Zero when neither side has a surface.
        floor_axis: fn(pixel: vec2f, offset: vec2f, atlas_size: vec2f, position: vec3f, depth: float) -> vec3f {
            let lo = self.floor_point(pixel - offset, atlas_size, position, depth)
            let hi = self.floor_point(pixel + offset, atlas_size, position, depth)
            let lo_ok = abs(lo.w) < 100000000000000000000000000000.0
            let hi_ok = abs(hi.w) < 100000000000000000000000000000.0
            if lo_ok && hi_ok && abs(abs(lo.w) - abs(hi.w)) <= 2.0 * self.u_min_occluder {
                return (hi.xyz - lo.xyz) * 0.5
            }
            if hi_ok && (abs(hi.w) <= abs(lo.w)) {
                return hi.xyz
            }
            if lo_ok {
                return -lo.xyz
            }
            return vec3(0.0, 0.0, 0.0)
        }

        tap: fn(spirals: vec4f, weight: float, mip: float,
                dither: float, m0: vec3f, m1: vec3f, m2: vec3f,
                mip_bias: float, atlas_size: vec2f, position: vec3f, normal: vec3f,
                view_direction: vec3f, radius: float, tangent: vec3f, bitangent: vec3f) -> u32 {
            let offset = fma(spirals.zw - spirals.xy, vec2(dither, dither), spirals.xy)
            let projected = m0 * offset.x + m1 * offset.y + m2
            var coords = projected.xy
            if self.u_proj_mode < 0.5 { coords = coords / projected.z }
            var sample_packed = 0u
            // Reject the border before translating into the atlas's mip row.
            if coords.x >= 0.0 && coords.y >= 0.0 && coords.x <= 1.0 && coords.y <= 1.0 {
                let k = u32(clamp(floor(mip_bias + mip + 0.5), 0.0, self.u_size.z - 1.0))
                let size = cortao_level_size(self.u_size.xy, k)
                var origin = vec2(0.0, 0.0)
                if k > 0u { origin = vec2(cortao_level_x(k, self.u_offsets_a, self.u_offsets_b), self.u_size.y) }
                let texel = min(floor(coords * size), size - vec2(1.0, 1.0)) + origin
                sample_packed = asuint(self.atlas_tex.sample_nearest((texel + vec2(0.5, 0.5)) / atlas_size).x)
            }
            let delta = cortao_unproject_offset(coords, cortao_depth_unpack(sample_packed, self.u_depth_code), self.u_unproject, self.u_geometry.x, self.u_proj_mode, -position)
            let distance2 = dot(delta, delta)
            if sample_packed != 0u && distance2 > 0.0 && distance2 < radius * radius && dot(delta, normal) > self.u_min_occluder {
                let inverse_length = 1.0 / sqrt(distance2)
                var direction = delta * inverse_length
                let sample_normal = cortao_decode_normal(sample_packed)
                let slope = 1.0 / sqrt(max(dot(sample_normal, view_direction), self.u_limits.z))
                var occluder_radius = (radius * weight) * (slope * inverse_length)
                if dot(sample_normal, direction) < self.u_limits.w {
                    // Move a front face's disc one radius into the occluder.
                    let adjusted = -occluder_radius / max(1.0 - occluder_radius * occluder_radius * 0.5, 0.000001)
                    let shifted = fma(sample_normal, vec3(adjusted, adjusted, adjusted), direction)
                    let length2 = dot(shifted, shifted)
                    if length2 > 0.000000000001 { direction = shifted / sqrt(length2) }
                } else {
                    occluder_radius = occluder_radius * mix(1.0, slope, self.u_backface_boost)
                }
                let hemisphere = vec3(dot(direction, tangent), dot(direction, bitangent), dot(direction, normal))
                // Mask before division as the south-pole spheremap is singular.
                if hemisphere.z > -0.99 {
                    let mul = 1.0 / sqrt(fma(hemisphere.z, 8.0, 8.0))
                    let lookup = vec3(hemisphere.xy * mul + vec2(0.5, 0.5), occluder_radius * sqrt(0.5))
                    let x = clamp(floor(lookup.x * 24.0), 0.0, 23.0)
                    let y = clamp(floor(lookup.y * 24.0), 0.0, 23.0)
                    let z = clamp(floor(lookup.z * 16.0), 0.0, 15.0)
                    let value = self.table_tex.sample_nearest(vec2((x + 0.5) / 24.0, (y + 24.0 * z + 0.5) / 384.0))
                    return u32(value.x) | (u32(value.y) << 16u)
                }
            }
            return 0u
        }

        fragment: fn() {
            self.fb0 = vec4(1.0, 0.0, 0.0, 1.0)
            let pixel = floor(self.frag.xy)
            let atlas_size = vec2(self.u_size.x, self.u_size.w)
            let center_uv = (pixel + vec2(0.5, 0.5)) / atlas_size
            let packed = asuint(self.atlas_tex.sample_nearest(center_uv).x)
            if packed != 0u {
                let depth = cortao_depth_unpack(packed, self.u_depth_code)
                var normal = cortao_oct_decode(asuint(self.normals_tex.sample_nearest(center_uv).x))
                let uv = (pixel + vec2(0.5, 0.5)) / self.u_size.xy
                var position = cortao_unproject_offset(uv, depth, self.u_unproject, self.u_geometry.x, self.u_proj_mode, vec3(0.0, 0.0, 0.0))
                if self.u_min_occluder > 0.0 {
                    // Relief under the floor must not tilt the centre's plane either (a bead's
                    // flank would see the flat face beyond it as an occluder): the normal comes
                    // from the depth twice the floor away on each axis, not the surface's own.
                    let footprint = length(cortao_unproject_offset(uv + vec2(1.0 / self.u_size.x, 0.0), depth, self.u_unproject, self.u_geometry.x, self.u_proj_mode, -position))
                    // No reach limit: zoomed in, the floor spans many pixels, and a side past
                    // the image's edge is simply absent.
                    let k = max(ceil(2.0 * self.u_min_occluder / max(footprint, 0.000000000001)), 1.0)
                    let ax = self.floor_axis(pixel, vec2(k, 0.0), atlas_size, position, depth)
                    let ay = self.floor_axis(pixel, vec2(0.0, k), atlas_size, position, depth)
                    let n = cross(ax, ay)
                    if dot(n, n) > 0.0 {
                        normal = normalize(n)
                        if dot(normal, cortao_oct_decode(asuint(self.normals_tex.sample_nearest(center_uv).x))) < 0.0 { normal = -normal }
                    }
                }
                let seed = u32(self.u_seed.x) | (u32(self.u_seed.y) << 16u)
                let pixel_seed = cortao_seed(vec2u(u32(pixel.x), u32(pixel.y)))
                let random = cortao_hash2(vec2u(pixel_seed + (seed & 65535u), pixel_seed + (seed >> 16u)))
                let rotation = f32(random.x) * (6.283185307179586 / 4294967295.0)
                let dither = f32(random.y) * (1.0 / 4294967295.0)
                let tangent_rotation = f32(random.x ^ random.y ^ seed) * (6.283185307179586 / 4294967295.0)
                let sample_rotation = vec2(sin(rotation), cos(rotation))
                var radius = self.u_geometry.w
                var view_direction = vec3(0.0, 0.0, 1.0)
                var mip_bias = self.u_limits.y
                var m0 = vec3(0.0, 0.0, 0.0)
                var m1 = vec3(0.0, 0.0, 0.0)
                var m2 = vec3(uv, 1.0)
                if self.u_proj_mode < 0.5 {
                    radius = min(radius, depth * self.u_limits.x)
                    let inverse_circle_distance = 1.0 / sqrt(depth * depth - radius * radius)
                    let apparent_radius = radius * inverse_circle_distance
                    let circle_radius = apparent_radius * depth
                    view_direction = -position / sqrt(dot(position, position))
                    let stretch = 1.0 / sqrt(view_direction.z)
                    mip_bias = log2(apparent_radius * stretch) + mip_bias
                    // Push the center slightly along its normal to avoid self-shadowing.
                    let bias = self.u_geometry.z * depth
                    position = fma(normal, vec3(bias, bias, bias), position)
                    let scaled = sample_rotation * circle_radius
                    var right = cortao_tangent(view_direction)
                    let up = cortao_bitangent(view_direction)
                    // Cancel errors by switching the spiral's handedness.
                    if (random.x & 1u) == 0u { right = -right }
                    // GLSL columns: rotation (sx,-sy,0),(sy,sx,0),(0,0,1),
                    // circle (right,up,position), then the skew-aware projection.
                    m0 = cortao_project(right * scaled.x - up * scaled.y, self.u_projection, self.u_geometry.y)
                    m1 = cortao_project(right * scaled.y + up * scaled.x, self.u_projection, self.u_geometry.y)
                    m2 = cortao_project(position, self.u_projection, self.u_geometry.y)
                } else {
                    position = fma(normal, vec3(self.u_geometry.z, self.u_geometry.z, self.u_geometry.z), position)
                    let c0 = vec3(radius * self.u_projection.x, 0.0, 0.0)
                    let c1 = vec3(radius * self.u_geometry.y, radius * self.u_projection.y, 0.0)
                    m0 = c0 * sample_rotation.x - c1 * sample_rotation.y
                    m1 = c0 * sample_rotation.y + c1 * sample_rotation.x
                }
                let t = cortao_tangent(normal)
                let b = cortao_bitangent(normal)
                let s = sin(tangent_rotation)
                let c = cos(tangent_rotation)
                let tangent = c * t + s * b
                let bitangent = c * b - s * t
                var ray_bits = 0u
                let count = u32(self.u_counts.x)
                for i in 0..24 {
                    if i >= count { break }
                    let spirals = self.taps.taps[2u * i]
                    let tap = self.taps.taps[2u * i + 1u]
                    ray_bits = ray_bits | self.tap(spirals, tap.x, tap.y, dither, m0, m1, m2, mip_bias, atlas_size, position, normal, view_direction, radius, tangent, bitangent)
                }
                self.fb0 = vec4(1.0 - f32(cortao_popcount(ray_bits)) / self.u_counts.y, 0.0, 0.0, 1.0)
            }
        }
    }

    mod.draw.DrawCortaoFilter = mod.std.set_type_default() do #(DrawCortaoFilter::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Ru8
        atlas_tex: texture_2d(float)
        ao_tex: texture_2d(float)
        u_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_depth_code: uniform(vec4(1.0, 0.0, 1.0, 1.0))
        // Depth sensitivity, normal sensitivity / 32, large, symmetric.
        u_filter: uniform(vec4(512.0, 0.5, 1.0, 1.0))
        frag: fragment_position()
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        fragment: fn() {
            self.fb0 = vec4(1.0, 0.0, 0.0, 1.0)
            let p = floor(self.frag.xy)
            let atlas_size = vec2(self.u_size.x, self.u_size.w)
            let packed = asuint(self.atlas_tex.sample_nearest((p + vec2(0.5, 0.5)) / atlas_size).x)
            if packed != 0u {
                let depth = cortao_depth_unpack(packed, self.u_depth_code)
                let normal = cortao_normal_fields(packed)
                let quad = vec2(f32(u32(p.x) & 4294967294u), f32(u32(p.y) & 4294967294u))
                // Gather order (x,y,z,w) = (left-bottom,right-bottom,right-top,left-top).
                // xxww-yyzz is left-right on either row; wzzw-xyyx is top-bottom
                // in either column. The derivative signs do not flip within the quad.
                // Keep empty partners at zero depth; off-image partners use the centre.
                let left_packed = asuint(self.atlas_tex.sample_nearest((vec2(quad.x, p.y) + vec2(0.5, 0.5)) / atlas_size).x)
                let top_packed = asuint(self.atlas_tex.sample_nearest((vec2(p.x, quad.y) + vec2(0.5, 0.5)) / atlas_size).x)
                var left = 0.0
                var top = 0.0
                if left_packed != 0u { left = cortao_depth_unpack(left_packed, self.u_depth_code) }
                if top_packed != 0u { top = cortao_depth_unpack(top_packed, self.u_depth_code) }
                var right = depth
                var bottom = depth
                if quad.x + 1.0 < self.u_size.x {
                    let right_packed = asuint(self.atlas_tex.sample_nearest((vec2(quad.x + 1.0, p.y) + vec2(0.5, 0.5)) / atlas_size).x)
                    right = 0.0
                    if right_packed != 0u { right = cortao_depth_unpack(right_packed, self.u_depth_code) }
                }
                if quad.y + 1.0 < self.u_size.y {
                    let bottom_packed = asuint(self.atlas_tex.sample_nearest((vec2(p.x, quad.y + 1.0) + vec2(0.5, 0.5)) / atlas_size).x)
                    bottom = 0.0
                    if bottom_packed != 0u { bottom = cortao_depth_unpack(bottom_packed, self.u_depth_code) }
                }
                let gradient = vec2(left - right, top - bottom) * (self.u_filter.x / depth)
                var sum = self.ao_tex.sample_nearest((p + vec2(0.5, 0.5)) / self.u_size.xy).x
                var weight_sum = 1.0
                var count = 2u
                if self.u_filter.z > 0.5 { count = 6u }
                for i in 0..6 {
                    if i < count {
                        var offset = vec2(3.0, 0.0)
                        if self.u_filter.z > 0.5 {
                            if i == 1u { offset = vec2(6.0, 0.0) }
                            if i == 2u { offset = vec2(3.0, 3.0) }
                            if i == 3u { offset = vec2(0.0, 3.0) }
                            if i == 4u { offset = vec2(0.0, 6.0) }
                            if i == 5u { offset = vec2(3.0, -3.0) }
                        } else {
                            if i == 1u { offset = vec2(0.0, 3.0) }
                        }
                        var weights = vec2(0.0, 0.0)
                        var values = vec2(0.0, 0.0)
                        for side in 0..2 {
                            var o = -offset
                            if side == 1u { o = offset }
                            let q = p + o
                            if q.x >= 0.0 && q.y >= 0.0 && q.x < self.u_size.x && q.y < self.u_size.y {
                                let data = asuint(self.atlas_tex.sample_nearest((q + vec2(0.5, 0.5)) / atlas_size).x)
                                if data != 0u {
                                    let dn = cortao_normal_fields(data) - normal
                                    let error = abs(dot(gradient, o) + self.u_filter.x * (cortao_depth_unpack(data, self.u_depth_code) / depth - 1.0)) + dot(dn, dn) * self.u_filter.y
                                    let weight = exp2(-error)
                                    let ao = self.ao_tex.sample_nearest((q + vec2(0.5, 0.5)) / self.u_size.xy).x
                                    if side == 0u { weights.x = weight; values.x = ao }
                                    else { weights.y = weight; values.y = ao }
                                }
                            }
                        }
                        if self.u_filter.w > 0.5 {
                            let weight = min(weights.x, weights.y)
                            weights = vec2(weight, weight)
                        }
                        sum = sum + dot(values, weights)
                        weight_sum = weight_sum + weights.x + weights.y
                    }
                }
                self.fb0 = vec4(sum / weight_sum, 0.0, 0.0, 1.0)
            }
        }
    }

    mod.draw.DrawCortaoUpsample = mod.std.set_type_default() do #(DrawCortaoUpsample::script_shader(vm)){
        ..mod.draw.DrawQuad
        color_format: @Ru8
        alpha_blend: false
        depth_tex: texture_2d(float)
        upsample_atlas_tex: texture_2d(float)
        upsample_ao_tex: texture_2d(float)
        // The surface hook sees full-resolution pixels, just as in a full prepass.
        u_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_depth_code: uniform(vec4(1.0, 0.0, 1.0, 1.0))
        u_proj_mode: uniform(0.0)
        u_view_from_clip: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        // xy = internal pixels, w = internal atlas height.
        u_upsample_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        frag: fragment_position()
        depth_clip: 0.0

        vertex: mod.draw.DrawCortaoPrepass.vertex
        surface: mod.draw.DrawCortaoPrepass.surface

        fragment: fn() {
            self.fb0 = vec4(1.0, 0.0, 0.0, 1.0)
            let pixel = floor(self.frag.xy)
            let surface = self.surface(pixel)
            if surface.w > 0.0 && surface.z < 0.0 {
                let depth = -surface.z
                let size = self.u_upsample_size.xy
                let uv = (pixel + vec2(0.5, 0.5)) / self.u_size.xy
                let footprint = uv * size - vec2(0.5, 0.5)
                let base = floor(footprint)
                let fraction = footprint - base
                // Relative depth differences of 5% or more get zero weight.
                // Squaring this linear falloff keeps silhouettes from bleeding.
                let depth_falloff = 20.0
                let depth_epsilon = 0.000001
                var sum = 0.0
                var weight_sum = 0.0
                var nearest = 1.0
                var nearest_error = 0.0
                var have_nearest = false
                for i in 0..4 {
                    let offset = vec2(f32(i & 1u), f32(i >> 1u))
                    let q = clamp(base + offset, vec2(0.0, 0.0), size - vec2(1.0, 1.0))
                    let atlas_uv = (q + vec2(0.5, 0.5)) / vec2(size.x, self.u_upsample_size.w)
                    let packed = asuint(self.upsample_atlas_tex.sample_nearest(atlas_uv).x)
                    // Use atlas depth even with temporal AO; unpack ignores the normal bits.
                    let sample_depth = cortao_depth_unpack(packed, self.u_depth_code)
                    let ao = self.upsample_ao_tex.sample_nearest((q + vec2(0.5, 0.5)) / size).x
                    let error = abs(sample_depth - depth)
                    let bilinear = mix(vec2(1.0, 1.0) - fraction, fraction, offset)
                    var similarity = 0.0
                    if packed != 0u {
                        // Missing surfaces have no depth to match. If all four
                        // are missing, the fallback remains unoccluded.
                        if !have_nearest || error < nearest_error {
                            nearest = ao
                            nearest_error = error
                            have_nearest = true
                        }
                        similarity = max(1.0 - depth_falloff * error / max(depth, depth_epsilon), 0.0)
                    }
                    let weight = bilinear.x * bilinear.y * similarity * similarity
                    sum = sum + weight * ao
                    weight_sum = weight_sum + weight
                }
                var result = nearest
                if weight_sum > 0.0 { result = sum / weight_sum }
                self.fb0 = vec4(result, 0.0, 0.0, 1.0)
            }
        }
    }

    mod.draw.DrawCortaoResolve = mod.std.set_type_default() do #(DrawCortaoResolve::script_shader(vm)){
        ..mod.draw.DrawQuad
        // r = visibility (the output contract), g = the linear depth it is for (0: background),
        // ba = its view normal's octahedral fields in [0, 1].
        color_format: @Rgba16F
        // ba carries the normal: a raw write, never blended over the old history.
        alpha_blend: false
        atlas_tex: texture_2d(float)
        ao_tex: texture_2d(float)
        history_tex: texture_2d(float)
        normals_tex: texture_2d(float)
        // xy = pixels, w = atlas height.
        u_size: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        u_depth_code: uniform(vec4(1.0, 0.0, 1.0, 1.0))
        u_unproject: uniform(vec4(1.0, -1.0, -0.5, 0.5))
        // x = unprojection skew, y = orthographic, z = history weight (0: none).
        u_current: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // The previous frame's projection (px, py, pz, pw) and (skew, orthographic).
        u_previous: uniform(vec4(1.0, -1.0, 0.5, 0.5))
        u_previous_mode: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // Current view -> previous view (rigid, column-major).
        u_previous_from_view: uniform(mat4x4f(1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0))
        frag: fragment_position()
        depth_clip: 0.0

        vertex: fn() {
            self.pos = self.geom.pos
            self.world = vec4(self.geom.pos.x, self.geom.pos.y, 0.0, 1.0)
            self.vertex_pos = vec4(
                self.geom.pos.x * 2.0 - 1.0
                1.0 - self.geom.pos.y * 2.0
                0.0
                1.0
            )
        }

        fragment: fn() {
            let p = floor(self.frag.xy)
            let size = self.u_size.xy
            let uv = (p + vec2(0.5, 0.5)) / size
            let current = self.ao_tex.sample_nearest(uv).x
            let atlas_uv = (p + vec2(0.5, 0.5)) / vec2(size.x, self.u_size.w)
            let packed = asuint(self.atlas_tex.sample_nearest(atlas_uv).x)
            self.fb0 = vec4(current, 0.0, 0.0, 1.0)
            if packed != 0u {
                let depth = cortao_depth_unpack(packed, self.u_depth_code)
                let normal_bits = asuint(self.normals_tex.sample_nearest(atlas_uv).x)
                let normal = cortao_oct_decode(normal_bits)
                let fields = vec2(f32((normal_bits >> 11u) & 2047u), f32(normal_bits & 2047u)) * (1.0 / 2047.0)
                var result = current
                if self.u_current.z > 0.0 {
                    let view = cortao_unproject_offset(uv, depth, self.u_unproject, self.u_current.x, self.u_current.y, vec3(0.0, 0.0, 0.0))
                    let q = (self.u_previous_from_view * vec4(view, 1.0)).xyz
                    let expected = -q.z
                    if expected > 0.0 {
                        let projection = self.u_previous
                        var previous_uv = vec2(projection.x * q.x + self.u_previous_mode.x * q.y + projection.z,
                                               projection.y * q.y + projection.w)
                        if self.u_previous_mode.y < 0.5 {
                            previous_uv = vec2(projection.x * q.x + self.u_previous_mode.x * q.y + projection.z * q.z,
                                               projection.y * q.y + projection.w * q.z) / expected
                        }
                        if previous_uv.x >= 0.0 && previous_uv.y >= 0.0 && previous_uv.x <= 1.0 && previous_uv.y <= 1.0 {
                            let history = self.history_tex.sample_nearest(previous_uv)
                            let history_bits = 1065353216u | (u32(round(history.z * 2047.0)) << 11u) | u32(round(history.w * 2047.0))
                            // The surface it saw must face as this one does: a floor's history never
                            // lands on the side of a box that arrived over it, nor the reverse.
                            let turned = (self.u_previous_from_view * vec4(normal, 0.0)).xyz
                            if history.y > 0.0 && abs(history.y - expected) <= 0.05 * expected && dot(turned, cortao_oct_decode(history_bits)) > 0.8 {
                                // The current 3x3 neighbourhood bounds what the history may say: its
                                // mean within one deviation, inside its range. A contact shadow left by
                                // a moved actor falls back in a frame; one noisy dark neighbour does
                                // not hold it there (a bare min/max range would).
                                var low = current
                                var high = current
                                var m1 = 0.0
                                var m2 = 0.0
                                for dy in 0..3 {
                                    for dx in 0..3 {
                                        let q2 = clamp(p + vec2(f32(dx) - 1.0, f32(dy) - 1.0), vec2(0.0, 0.0), size - vec2(1.0, 1.0))
                                        let v = self.ao_tex.sample_nearest((q2 + vec2(0.5, 0.5)) / size).x
                                        low = min(low, v)
                                        high = max(high, v)
                                        m1 = m1 + v
                                        m2 = m2 + v * v
                                    }
                                }
                                let mean = m1 * (1.0 / 9.0)
                                let deviation = sqrt(max(m2 * (1.0 / 9.0) - mean * mean, 0.0))
                                low = max(low, mean - deviation)
                                high = min(high, mean + deviation)
                                result = mix(current, clamp(history.x, low, high), self.u_current.z)
                            }
                        }
                    }
                }
                self.fb0 = vec4(result, depth, fields.x, fields.y)
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCortaoPrepass {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCortaoMain {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCortaoFilter {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCortaoResolve {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCortaoUpsample {
    #[deref]
    pub draw_super: DrawQuad,
}

impl DrawCortaoUpsample {
    /// Mirror the actual host prepass, including helper functions and extra
    /// surface inputs. Inheriting it directly would also declare its normal MRT:
    /// the shader compiler collects outputs even if the fragment never uses them.
    pub(super) fn from_prepass(vm: &mut ScriptVm, source: ScriptObject) -> Option<Box<Self>> {
        let template = vm.heap().type_default_for_id(Self::script_type_id_static())?;
        let object = vm.heap_mut().new_with_proto_no_vec(template.into());
        let mut chain = Vec::new();
        let mut next = Some(source);
        while let Some(source) = next {
            chain.push(source);
            next = vm.heap().proto(source).as_object();
        }
        // Base first, then host overrides, exactly as on the prepass. Keep the
        // upsample's entry points and single R8 output; do not copy the MRT.
        for source in chain.into_iter().rev() {
            for entry in vm.heap().map_entries_ordered(source) {
                if [
                    id!(fragment),
                    id!(vertex),
                    id!(fb1),
                    id!(color_format),
                    id!(alpha_blend),
                ]
                .into_iter()
                .any(|id| entry.key.as_id() == Some(id))
                {
                    continue;
                }
                if !vm.set_value(object, entry.key, entry.value).is_nil() {
                    return None;
                }
            }
        }
        let mut draw = Self::script_new(vm);
        draw.draw_vars.draw_shader_id = None;
        draw.script_apply(vm, &Apply::New, &mut Scope::empty(), object.into());
        draw.draw_vars.draw_shader_id?;
        Some(Box::new(draw))
    }
}
