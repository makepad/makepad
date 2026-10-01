//! The sun's cascaded shadow receive path, ONE copy for every lit family.
//!
//! Hosts declare `csm_map: texture_depth(float)` themselves, at the texture
//! slot their binding ABI already fixed, and spread this for the uniforms and
//! the filter. Realtime cascaded shadow maps (shadow_csm.rs): 4 sun-depth
//! tiles in a 2x2 grid of one sampled D32 target, read with hardware compare.
//! csm_p = (tier on, one tile's inverse resolution, 0, 0); csm_r*N are
//! cascade N's world->map rows; csm_da/db map a z01 reference to stored
//! depth per cascade; csm_zw = z01 per world unit; csm_texel = texel size in
//! world units. When the tier is on, `csm_vis` REPLACES every baked
//! sun-visibility path — one receive path for statics, dynamics and
//! characters alike.

use super::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.geom

    mod.draw.SunCascades = {
        // Shadow debug view (Renderer::set_shadow_debug, the host's shadow debug setting):
        // each lane's final colour becomes its cascade's tint (red, green,
        // blue; grey past the last) scaled by the filtered sun visibility.
        csm_debug: uniform(0.0)
        csm_p: uniform(vec4(0.0, 0.001, 0.0, 0.0))
        // Stored depth of a z01 reference in cascade i: z01 * csm_da[i] +
        // csm_db[i] (the tile's depth generation and the backend's clip-z
        // to depth mapping, both folded in on the CPU).
        csm_da: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        csm_db: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        csm_zw: uniform(vec4(0.01, 0.01, 0.01, 0.01))
        csm_texel: uniform(vec4(0.01, 0.01, 0.01, 0.01))
        csm_rx0: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry0: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz0: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx1: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry1: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz1: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx2: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry2: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz2: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        csm_rx3: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        csm_ry3: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        csm_rz3: uniform(vec4(0.0, 0.0, 1.0, 0.0))

        // Cascade ci's rows and per-cascade scalars.
        csm_rx: fn(ci: float) -> vec4 {
            if ci < 0.5 { return self.csm_rx0 }
            if ci < 1.5 { return self.csm_rx1 }
            if ci < 2.5 { return self.csm_rx2 }
            return self.csm_rx3
        }
        csm_ry: fn(ci: float) -> vec4 {
            if ci < 0.5 { return self.csm_ry0 }
            if ci < 1.5 { return self.csm_ry1 }
            if ci < 2.5 { return self.csm_ry2 }
            return self.csm_ry3
        }
        csm_rz: fn(ci: float) -> vec4 {
            if ci < 0.5 { return self.csm_rz0 }
            if ci < 1.5 { return self.csm_rz1 }
            if ci < 2.5 { return self.csm_rz2 }
            return self.csm_rz3
        }
        csm_pick: fn(v: vec4, ci: float) -> float {
            if ci < 0.5 { return v.x }
            if ci < 1.5 { return v.y }
            if ci < 2.5 { return v.z }
            return v.w
        }
        // (ndc x, ndc y, z01) of `p` in cascade ci.
        csm_proj: fn(ci: float, p: vec3) -> vec3 {
            let rx = self.csm_rx(ci)
            let ry = self.csm_ry(ci)
            let rz = self.csm_rz(ci)
            return vec3(dot(rx.xyz, p) + rx.w, dot(ry.xyz, p) + ry.w, dot(rz.xyz, p) + rz.w)
        }
        // 1 when q lies inside a cascade's full XYZ window (xy within `m`).
        csm_inside: fn(q: vec3, m: float) -> float {
            if max(abs(q.x), abs(q.y)) > m || q.z < 0.0 || q.z > 1.0 { return 0.0 }
            return 1.0
        }

        // One hardware-compared bilinear tap (2x2 texels) at tile-local uv
        // (u, v) of cascade ci. Clamped one texel inside the tile so the
        // bilinear footprint never reads a neighbour cascade; tiles sit in
        // a 2x2 grid (shadow_csm::csm_tile_origin).
        csm_tap: fn(u: float, v: float, ci: float, depth: float) -> float {
            let e = self.csm_p.y
            let row = floor(ci * 0.5)
            let col = ci - row * 2.0
            let uv = vec2((clamp(u, e, 1.0 - e) + col) * 0.5, (clamp(v, e, 1.0 - e) + row) * 0.5)
            return self.csm_map.sample_compare(uv, depth)
        }

        // The kernel's taps on the unit disk (the centre, the four rim taps,
        // then the eight inner ones), in the rotated disk's (cs, sn) frame.
        csm_disk: fn(k: float) -> vec2 {
            if k < 0.5 { return vec2(0.0, 0.0) }
            if k < 1.5 { return vec2(-0.840, -0.074) }
            if k < 2.5 { return vec2(0.962, -0.195) }
            if k < 3.5 { return vec2(0.519, 0.767) }
            if k < 4.5 { return vec2(-0.322, -0.933) }
            if k < 5.5 { return vec2(-0.326, -0.406) }
            if k < 6.5 { return vec2(-0.696, 0.457) }
            if k < 7.5 { return vec2(-0.203, 0.621) }
            if k < 8.5 { return vec2(0.473, -0.480) }
            if k < 9.5 { return vec2(0.185, -0.893) }
            if k < 10.5 { return vec2(0.507, 0.064) }
            if k < 11.5 { return vec2(0.896, 0.412) }
            return vec2(-0.792, -0.598)
        }

        // Filtered visibility of wp in cascade ci: 12 Poisson taps plus the
        // centre, each a hardware 2x2 compare, on a disk rotated per quarter
        // shadow texel (stable on the surface, no temporal crawl) so the
        // kernel's taps never line up into bands. The disk radius holds a
        // roughly constant world penumbra (2.5 cm) within 1.5..3 texels, so
        // the penumbra does not jump in width at every cascade seam.
        //
        // One bias strategy: a normal offset in texel units (the receiver
        // slides off the knife edge along n, most at grazing sun) plus a
        // slope-scaled depth bias sized to the kernel (a sloped receiver's
        // depth changes by r * texel * tan(theta) across the disk). No
        // receiver-plane term: from the interpolated shading normal it
        // leaked light at creases.
        csm_sample: fn(ci: float, wp: vec3, n: vec3, ndl: float) -> float {
            let tw = self.csm_pick(self.csm_texel, ci)
            let nl = clamp(ndl, 0.0, 1.0)
            let wp2 = wp + normalize(n) * (tw * (0.6 + 1.4 * (1.0 - nl)))
            let q = self.csm_proj(ci, wp2)
            let r = clamp(0.025 / tw, 1.5, 3.0)
            let tan_t = min(sqrt(max(1.0 - nl * nl, 0.0)) / max(nl, 0.05), 4.0)
            let ref01 = q.z - tw * (0.5 + 0.5 * r * tan_t) * self.csm_pick(self.csm_zw, ci)
            let depth = ref01 * self.csm_pick(self.csm_da, ci) + self.csm_pick(self.csm_db, ci)
            let u = q.x * 0.5 + 0.5
            let v = 0.5 - q.y * 0.5
            let e = self.csm_p.y
            let cell = floor(vec2(u, v) / e * 4.0)
            let a = 6.2831853 * fract(52.9829189 * fract(dot(cell, vec2(0.06711056, 0.00583715))))
            let cs = vec2(cos(a), sin(a)) * (r * e)
            let sn = vec2(-cs.y, cs.x)
            // The centre and the four taps on the disk's rim first: where
            // all five agree fully (open ground in sun, the inside of a
            // shadow; most pixels) the other eight would agree too and are
            // skipped. A penumbra pixel takes the whole kernel. One tap call
            // site in a loop (a D3D compile inlines every site).
            var s = 0.0
            for k in 0..13 {
                if k == 5 {
                    if s > 4.999 { return 1.0 }
                    if s < 0.001 { return 0.0 }
                }
                let d = self.csm_disk(float(k))
                s = s + self.csm_tap(u + d.x * cs.x + d.y * sn.x, v + d.x * cs.y + d.y * sn.y, ci, depth)
            }
            return s * 0.07692308
        }

        // Sun visibility: the tightest cascade that contains the point in
        // full XYZ (the slice spheres can overlap in light-space XY without
        // sharing a depth interval, so XY alone is not coverage). The outer
        // band of each cascade cross-fades into the next; the last one fades
        // out to lit instead of cutting on a hard line.
        csm_vis: fn(wp: vec3, n: vec3, ndl: float) -> float {
            if self.csm_p.x < 0.5 {
                return 1.0
            }
            // The tightest cascade holding the point (one projection call
            // site in a loop).
            var ci = 4.0
            var q = vec3(0.0, 0.0, 0.0)
            for c in 0..4 {
                if ci > 3.5 {
                    let qc = self.csm_proj(float(c), wp)
                    if self.csm_inside(qc, 0.99) > 0.5 {
                        ci = float(c)
                        q = qc
                    }
                }
            }
            if ci > 3.5 {
                return 1.0
            }
            // This cascade, and in its outer band the next one too (one
            // sample call site in a loop of at most two).
            let edge = max(abs(q.x), abs(q.y))
            var blend = 0.0
            if ci < 2.5 && edge > 0.93 && self.csm_inside(self.csm_proj(ci + 1.0, wp), 0.99) > 0.5 {
                blend = smoothstep(0.93, 0.99, edge)
            }
            var s = 0.0
            for j in 0..2 {
                let jf = float(j)
                if jf < 0.5 || blend > 0.0 {
                    s = s + self.csm_sample(ci + jf, wp, n, ndl) * mix(1.0 - blend, blend, jf)
                }
            }
            if ci > 2.5 {
                s = mix(s, 1.0, smoothstep(0.85, 0.98, edge))
            }
            return s
        }

        // One hardware 2x2 compare instead of the 13-tap disk: for foliage
        // and grass (swaying leaf cards overdraw several layers deep, and a
        // leaf's shadow edge never needs the soft kernel). The seam band
        // still cross-fades into the next cascade (a second tap there only),
        // or a tree's shadow on the grass would pop where its caster changes
        // from leaves to stand-in.
        csm_vis_fast: fn(wp: vec3, n: vec3, ndl: float) -> float {
            if self.csm_p.x < 0.5 {
                return 1.0
            }
            // The tightest cascade holding the point (one projection call
            // site in a loop).
            var ci = 4.0
            var q = vec3(0.0, 0.0, 0.0)
            for c in 0..4 {
                if ci > 3.5 {
                    let qc = self.csm_proj(float(c), wp)
                    if self.csm_inside(qc, 0.99) > 0.5 {
                        ci = float(c)
                        q = qc
                    }
                }
            }
            if ci > 3.5 {
                return 1.0
            }
            let edge = max(abs(q.x), abs(q.y))
            var blend = 0.0
            if ci < 2.5 && edge > 0.9 && self.csm_inside(self.csm_proj(ci + 1.0, wp), 0.99) > 0.5 {
                blend = smoothstep(0.9, 0.99, edge)
            }
            var s = 0.0
            for j in 0..2 {
                let jf = float(j)
                if jf < 0.5 || blend > 0.0 {
                    s = s + self.csm_tap1(ci + jf, wp, n, ndl) * mix(1.0 - blend, blend, jf)
                }
            }
            if ci > 2.5 {
                s = mix(s, 1.0, smoothstep(0.85, 0.98, edge))
            }
            return s
        }

        csm_tap1: fn(ci: float, wp: vec3, n: vec3, ndl: float) -> float {
            let tw = self.csm_pick(self.csm_texel, ci)
            let nl = clamp(ndl, 0.0, 1.0)
            let q2 = self.csm_proj(ci, wp + normalize(n) * (tw * (1.0 + 1.5 * (1.0 - nl))))
            let depth = (q2.z - tw * 2.0 * self.csm_pick(self.csm_zw, ci)) * self.csm_pick(self.csm_da, ci) + self.csm_pick(self.csm_db, ci)
            return self.csm_tap(q2.x * 0.5 + 0.5, 0.5 - q2.y * 0.5, ci, depth)
        }

        csm_debug_view: fn(color: vec4, wp: vec3, n: vec3) -> vec4 {
            if self.csm_debug < 0.5 || self.csm_p.x < 0.5 { return color }
            // The tightest cascade's tint (red, green, blue, yellow).
            var tint = vec3(0.55, 0.55, 0.55)
            for c in 0..4 {
                let cf = 3.0 - float(c)
                if self.csm_inside(self.csm_proj(cf, wp), 0.99) > 0.5 {
                    tint = vec3(1.0, 0.4, 0.35)
                    if cf > 0.5 { tint = vec3(0.35, 1.0, 0.4) }
                    if cf > 1.5 { tint = vec3(0.35, 0.45, 1.0) }
                    if cf > 2.5 { tint = vec3(1.0, 0.85, 0.3) }
                }
            }
            let nn = normalize(n)
            let vis = self.csm_vis(wp, nn, max(dot(nn, normalize(self.light_dir)), 0.0))
            return vec4(tint * (0.2 + 0.8 * vis), 1.0)
        }
    }
}
