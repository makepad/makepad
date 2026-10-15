//! The procedural backgrounds' shader: one full-screen quad, one pass, no
//! textures. Every background is a function of a virtual-camera view ray, a
//! screen position and a time (`bg_*(ray, uv, t) -> vec4`); `pixel` picks one
//! by `kind` (the same for every pixel of a draw, so the branch is uniform).
//!
//! The ray comes from the spacewarp's *logical* position (`warp_map`), so a
//! background bends exactly the way the windows do: the grid lines, the
//! terrain and the stars are squeezed toward the edge zones with the same
//! map the windows follow (`spacewarp::Zones`: the smoothstep law `s = (1-t)^2 (1+2t)`).
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*
    mod.widgets.DrawBackdrop = set_type_default() do #(DrawBackdrop::script_shader(vm)) {
        ..mod.draw.DrawQuad
        // The instances (see the struct): which background, seconds,
        // width / height, spacewarp on, focus pool on, pointer (0..1, or
        // -1 for none), focus rect (x0 y0 x1 y1, 0..1), the warp zones
        // (centre left, centre right, zone width; 0..1 of the width; and the
        // display's mid-height, 0..1 of the height)
        // and the light direction (xyz).
        kind: 0.0
        time: 0.0
        aspect: 1.78
        warp_on: 0.0
        focus_on: 0.0
        pointer: vec2(-1.0, -1.0)
        focus: vec4(0.0, 0.0, 0.0, 0.0)
        warp: vec4(0.2, 0.8, 0.2, 0.5)
        // The parking cue of a dragged window (warp grid only): the slot it
        // would park in (x0 y0 x1 y1, 0..1) and the eased drag: level (0..1)
        // and the drop's absorb (1..0).
        tgt: vec4(0.0, 0.0, 0.0, 0.0)
        drag: vec4(0.0, 0.0, 0.0, 0.0)
        light: vec4(0.0, 0.5, 0.85, 0.0)
        // The theme tints every background.
        col_accent: uniform(mod.wm_theme.accent)
        col_bg: uniform(mod.wm_theme.background)
        col_dark: uniform(mod.wm_theme.darker_background)

        // Hashes without a sine: the same values on every GPU.
        hash21: fn(p: vec2) -> float {
            var q = fract(vec3(p.x, p.y, p.x) * 0.1031)
            q = q + vec3(1.0, 1.0, 1.0) * dot(q, vec3(q.y, q.z, q.x) + vec3(33.33, 33.33, 33.33))
            return fract((q.x + q.y) * q.z)
        }
        hash22: fn(p: vec2) -> vec2 {
            var q = fract(vec3(p.x, p.y, p.x) * vec3(0.1031, 0.103, 0.0973))
            q = q + vec3(1.0, 1.0, 1.0) * dot(q, vec3(q.y, q.z, q.x) + vec3(33.33, 33.33, 33.33))
            return fract(vec2((q.x + q.y) * q.z, (q.x + q.z) * q.y))
        }
        vnoise: fn(p: vec2) -> float {
            let cell = floor(p)
            let fr = fract(p)
            let w = fr * fr * (vec2(3.0, 3.0) - fr * 2.0)
            let n00 = self.hash21(cell)
            let n10 = self.hash21(cell + vec2(1.0, 0.0))
            let n01 = self.hash21(cell + vec2(0.0, 1.0))
            let n11 = self.hash21(cell + vec2(1.0, 1.0))
            return mix(mix(n00, n10, w.x), mix(n01, n11, w.x), w.y)
        }
        fbm3: fn(p: vec2) -> float {
            let a = self.vnoise(p) * 0.5
            let b = self.vnoise(p * 2.03 + vec2(1.7, 9.2)) * 0.25
            let c = self.vnoise(p * 4.1 + vec2(8.3, 2.8)) * 0.125
            return (a + b + c) / 0.875
        }

        // The spacewarp map (the law lives in spacewarp::Zones; this is
        // its shader mirror). xy: the logical position (what the layout
        // space sees at this shown position), z: the scale s there.
        //   shown distance d into a zone of width Z, s = smoothstep law (1-t)^2 (1+2t), t = d / Z,
        //   logical distance v = Z ((2/9) ln((1+2t)/(1-t)) + t / (3 (1-t))),  y_u = mid_y + (y - mid_y) / s
        warp_map: fn(uv: vec2) -> vec3 {
            var s = 1.0
            var ux = uv.x
            if self.warp_on > 0.5 {
                let cl = self.warp.x
                let cr = self.warp.y
                let z = max(self.warp.z, 0.0001)
                if uv.x < cl {
                    let f = min((cl - uv.x) / z, 0.998)
                    s = (1.0 - f) * (1.0 - f) * (1.0 + 2.0 * f)
                    ux = cl - z * (0.2222222 * log((1.0 + 2.0 * f) / (1.0 - f)) + f / (3.0 * (1.0 - f)))
                }
                if uv.x > cr {
                    let f = min((uv.x - cr) / z, 0.998)
                    s = (1.0 - f) * (1.0 - f) * (1.0 + 2.0 * f)
                    ux = cr + z * (0.2222222 * log((1.0 + 2.0 * f) / (1.0 - f)) + f / (3.0 * (1.0 - f)))
                }
            }
            return vec3(ux, self.warp.w + (uv.y - self.warp.w) / s, s)
        }

        // The virtual camera's view ray for a logical position.
        view_ray: fn(lu: vec2) -> vec3 {
            return normalize(vec3((lu.x - 0.5) * self.aspect * 1.4, (0.5 - lu.y) * 1.4, -1.0))
        }

        // A soft pool of light under the focus window: the distance to its
        // rect, in height units.
        pool: fn(uv: vec2) -> float {
            let c = (self.focus.xy + self.focus.zw) * 0.5
            let hs = (self.focus.zw - self.focus.xy) * 0.5
            let sc = vec2(self.aspect, 1.0)
            let q = abs((uv - c) * sc) - hs * sc
            let d = length(max(q, vec2(0.0, 0.0)))
            return self.focus_on * exp(-d * 5.0)
        }

        // 1. The warp grid: lines in logical space, drawn exactly through
        // the warp, so the windows' scale and the grid agree.
        bg_grid: fn(ray: vec3, uv: vec2, t: float) -> vec4 {
            let m = self.warp_map(uv)
            let g = vec2(m.x * self.aspect, m.y) / 0.07
            let fw = max(vec2(abs(dFdx(g.x)) + abs(dFdy(g.x)), abs(dFdx(g.y)) + abs(dFdy(g.y))), vec2(0.00001, 0.00001))
            let near = abs(fract(g - vec2(0.5, 0.5)) - vec2(0.5, 0.5))
            let dp = near / fw
            let gi = floor(g + vec2(0.5, 0.5))
            let major = vec2(step(fract(gi.x * 0.25), 0.01), step(fract(gi.y * 0.25), 0.01))
            // Lines closer than a few pixels fade instead of aliasing.
            let gap = vec2(smoothstep(2.0, 6.0, 1.0 / fw.x), smoothstep(2.0, 6.0, 1.0 / fw.y))
            let lx = ((1.0 - smoothstep(0.4, 1.4, dp.x)) + 0.18 * exp(-dp.x * 0.4)) * (0.5 + 0.5 * major.x) * gap.x
            let ly = ((1.0 - smoothstep(0.4, 1.4, dp.y)) + 0.18 * exp(-dp.y * 0.4)) * (0.5 + 0.5 * major.y) * gap.y
            var i = max(lx, ly) * 0.55
            // The parking cue: while a dragged window is headed into a side
            // zone, a faint line traces the slot it will park in; the drop
            // settles it with a brief, quiet tightening along that edge.
            let tc = (self.tgt.xy + self.tgt.zw) * 0.5
            let th = (self.tgt.zw - self.tgt.xy) * 0.5
            let tq = abs((uv - tc) * vec2(self.aspect, 1.0)) - th * vec2(self.aspect, 1.0)
            let sd = length(max(tq, vec2(0.0, 0.0))) + min(max(tq.x, tq.y), 0.0)
            let band = exp(-abs(sd) * 38.0)
            let ab = self.drag.y
            // The drop's flash contracts exactly onto the border and fades.
            let flash = exp(-pow(sd - 0.02 * ab * ab, 2.0) / 0.00006) * ab
            i = i * (1.0 + self.drag.x * 0.35 * band + 0.4 * flash)
            let glow = self.drag.x * 0.12 * band + 0.25 * flash
            // Depth: the compressed edges recede, so lines and cells fade
            // and darken with the compression, and a faint vignette closes
            // in toward the edges.
            let s = clamp(self.warp_map(uv).z, 0.0, 1.0)
            let recede = 1.0 - s
            i = i * (0.3 + 0.7 * s)
            let lc = mix(self.col_accent.rgb, vec3(0.75, 0.9, 1.0), 0.3)
            let cell = 0.5 + 0.5 * sin(g.x * 3.14159) * sin(g.y * 3.14159)
            var base = self.col_dark.rgb * 0.5 * (1.0 + 0.6 * ray.y) * (1.0 + 0.12 * cell)
            base = base * (1.0 - 0.45 * recede) * (1.0 - 0.35 * recede * recede)
            return vec4(base + lc * i + self.col_accent.rgb * glow, 1.0)
        }

        // 2. Starfield and nebula: three parallax hash-star layers over
        // three octaves of fbm dust.
        star_layer: fn(p: vec2, sc: float, speed: float, gate: float, t: float) -> vec3 {
            let q = p * sc + vec2(t * speed, 0.0)
            let c = floor(q)
            let f = fract(q)
            let h = self.hash22(c)
            let hb = self.hash21(c + vec2(7.7, 3.1))
            let d = length(f - (vec2(0.2, 0.2) + h * 0.6))
            let r = 0.05 + 0.07 * hb
            let tw = 0.7 + 0.3 * sin(t * 3.0 + hb * 40.0)
            let s = smoothstep(r, 0.0, d) * step(gate, hb) * tw
            return mix(vec3(1.0, 1.0, 1.0), self.col_accent.rgb, h.x * 0.5) * s
        }
        bg_stars: fn(ray: vec3, uv: vec2, t: float) -> vec4 {
            let p = ray.xy / max(-ray.z, 0.2)
            let np = p * 0.9 + vec2(t * 0.004, 0.0)
            let n1 = self.fbm3(np)
            let n2 = self.fbm3(np * 1.7 + vec2(5.0, 5.0))
            var col = self.col_dark.rgb * 0.4
            col = col + self.col_accent.rgb * (n1 * n1 * sqrt(n1)) * 0.6
            col = col + self.col_accent.gbr * (n2 * n2 * n2) * 0.3
            col = col + self.star_layer(p, 7.0, 0.02, 0.8, t) * 1.0
            col = col + self.star_layer(p + vec2(3.3, 1.1), 14.0, 0.05, 0.84, t) * 0.7
            col = col + self.star_layer(p + vec2(9.1, 4.7), 28.0, 0.11, 0.88, t) * 0.5
            return vec4(col * (1.0 + 0.8 * self.pool(uv)), 1.0)
        }

        // 3. Terrain: a low-poly ridge heightfield, flown over, marched in
        // at most 24 steps and shaded flat per triangle under a fog.
        tr_h: fn(c: vec2) -> float {
            let r = 1.0 - abs(sin(c.x * 0.29 + 1.9 * sin(c.y * 0.13)))
            let r2 = 1.0 - abs(sin(c.x * 0.71 + c.y * 0.43))
            return 2.4 * r * r * (0.65 + 0.35 * sin(c.y * 0.21 + 1.0)) + 0.4 * r2 + 0.35 * self.hash21(c)
        }
        // The faceted surface: the height at the cell's corners, flat on
        // each of the cell's two triangles.
        tr_surf: fn(p: vec2) -> float {
            let ci = floor(p)
            let f = fract(p)
            let h00 = self.tr_h(ci)
            let h10 = self.tr_h(ci + vec2(1.0, 0.0))
            let h01 = self.tr_h(ci + vec2(0.0, 1.0))
            let h11 = self.tr_h(ci + vec2(1.0, 1.0))
            if f.x + f.y < 1.0 {
                return h00 + f.x * (h10 - h00) + f.y * (h01 - h00)
            }
            return h11 + (1.0 - f.x) * (h01 - h11) + (1.0 - f.y) * (h10 - h11)
        }
        bg_terrain: fn(ray: vec3, uv: vec2, t: float) -> vec4 {
            let rd = normalize(ray + vec3(0.0, -0.3, 0.0))
            let L = normalize(self.light.xyz)
            let horizon = mix(self.col_dark.rgb, self.col_accent.rgb, 0.35)
            var col = mix(horizon * 0.9, self.col_dark.rgb * 0.3, smoothstep(0.0, 0.55, rd.y))
            col = col + self.col_accent.rgb * pow(max(dot(rd, L), 0.0), 24.0) * 0.8
            if rd.y < 0.03 {
                let ro = vec3(0.0, 4.4, -t * 0.8)
                var tt = 2.0
                var tp = 2.0
                var hp = 1.0
                var hit = 0.0
                for i in 0..24 {
                    if hit < 0.5 {
                        let p = ro + rd * tt
                        let hh = p.y - self.tr_surf(p.xz)
                        if hh < 0.02 * tt {
                            // One secant step between the last two samples.
                            let k = hp / max(hp - hh, 0.0001)
                            tt = tp + (tt - tp) * clamp(k, 0.0, 1.0)
                            hit = 1.0
                        } else {
                            tp = tt
                            hp = hh
                            tt = tt + max(0.12, hh * 0.45) + tt * 0.04
                        }
                    }
                }
                if hit > 0.5 {
                    let p = ro + rd * tt
                    let ci = floor(p.xz)
                    let f = fract(p.xz)
                    let h00 = self.tr_h(ci)
                    let h10 = self.tr_h(ci + vec2(1.0, 0.0))
                    let h01 = self.tr_h(ci + vec2(0.0, 1.0))
                    let h11 = self.tr_h(ci + vec2(1.0, 1.0))
                    var n = vec3(0.0, 1.0, 0.0)
                    if f.x + f.y < 1.0 {
                        n = normalize(vec3(h00 - h10, 1.0, h00 - h01))
                    } else {
                        n = normalize(vec3(h01 - h11, 1.0, h10 - h11))
                    }
                    let diff = 0.12 + 1.1 * pow(max(dot(n, L), 0.0), 1.5)
                    let hn = clamp(p.y / 3.0, 0.0, 1.0)
                    // Facets smaller than about 3 px blend to their mean.
                    let fwp = max(abs(dFdx(p.x)) + abs(dFdy(p.x)), abs(dFdx(p.z)) + abs(dFdy(p.z)))
                    let cellfade = 1.0 - smoothstep(0.1, 0.3, fwp)
                    var surf = mix(self.col_bg.rgb * 1.2, self.col_accent.rgb * 0.9, hn) * mix(0.45, diff, cellfade)
                    // Neon facet edges, near the camera only.
                    let e = min(min(f.x, f.y), min(1.0 - f.x, abs(f.x + f.y - 1.0)))
                    let w = max(0.02, tt * 0.004)
                    surf = surf + self.col_accent.rgb * (1.0 - smoothstep(0.0, max(w, 1.5 * fwp), e)) * 0.5 * exp(-tt * 0.08) * cellfade
                    col = mix(surf, horizon, 1.0 - exp(-tt * 0.03))
                }
            }
            return vec4(col * (1.0 + 0.6 * self.pool(uv)), 1.0)
        }

        // 4. Crystal field: a Voronoi facet floor and ceiling; each facet
        // tilts at random and glints toward the pointer (Fresnel-weighted).
        bg_crystal: fn(ray: vec3, uv: vec2, t: float) -> vec4 {
            let rd = ray
            var sg = -1.0
            if rd.y > 0.0 {
                sg = 1.0
            }
            let tt = 1.6 / max(abs(rd.y), 0.03)
            let q = (vec2(rd.x, rd.z) * tt - vec2(0.0, t * 0.5)) * 0.55
            let ip = floor(q)
            let fp = fract(q)
            var f1 = 8.0
            var f2 = 8.0
            var idv = vec2(0.0, 0.0)
            var oj = -1.0
            for j in 0..3 {
                var oi = -1.0
                for i in 0..3 {
                    let o = vec2(oi, oj)
                    let h = self.hash22(ip + o)
                    let pt = o + vec2(0.1, 0.1) + h * 0.8 - fp
                    let d = dot(pt, pt)
                    if d < f1 {
                        f2 = f1
                        f1 = d
                        idv = h
                    } else if d < f2 {
                        f2 = d
                    }
                    oi = oi + 1.0
                }
                oj = oj + 1.0
            }
            let n = normalize(vec3((idv.x - 0.5) * 1.1, -sg, (idv.y - 0.5) * 1.1))
            var ptr = self.pointer
            if ptr.x < 0.0 {
                ptr = vec2(0.5 + 0.3 * sin(t * 0.3), 0.5 + 0.2 * cos(t * 0.23))
            }
            let pl = (ptr - vec2(0.5, 0.5)) * vec2(self.aspect, -1.0) * 2.0
            let lp = normalize(vec3(pl.x * 1.2, pl.y, -1.0))
            let refl = rd - n * (2.0 * dot(rd, n))
            let fres = 0.2 + 0.8 * pow(1.0 - max(dot(-rd, n), 0.0), 3.0)
            let glint = pow(max(dot(refl, lp), 0.0), 40.0) * fres
            let ld = normalize(self.light.xyz)
            let face = mix(self.col_dark.rgb, self.col_accent.rgb, idv.x * 0.5)
            var col = face * 0.45 * (0.4 + 0.6 * max(dot(n, ld), 0.0))
            // Cells smaller than about 3 px blend out to the fogged face
            // colour instead of aliasing: the lines widen with the pixel
            // footprint, and lines and glints fade with it.
            let fwq = max(abs(dFdx(q.x)) + abs(dFdy(q.x)), abs(dFdx(q.y)) + abs(dFdy(q.y)))
            let cellfade = 1.0 - smoothstep(0.1, 0.3, fwq)
            let edge = 1.0 - smoothstep(0.0, max(0.12, 1.5 * fwq), sqrt(f2) - sqrt(f1))
            col = col + self.col_accent.rgb * edge * 0.5
            col = col + mix(self.col_accent.rgb, vec3(1.0, 1.0, 1.0), 0.6) * glint * 1.2
            col = mix(mix(self.col_dark.rgb, self.col_accent.rgb, 0.25) * 0.2, col, cellfade)
            col = col * exp(-tt * 0.05) * smoothstep(0.02, 0.1, abs(rd.y))
            return vec4(col * (1.0 + 0.8 * self.pool(uv)), 1.0)
        }

        // 5. Flow field: the curl of an analytic potential (so no noise
        // taps for its gradient). A streamline of the curl is a contour of
        // the potential, so the lines are drawn as contours of it: the
        // exact limit of a line-integral convolution along the flow, in
        // one tap.
        fl_psi: fn(p: vec2, t: float) -> float {
            let d = p.y * 0.7 + t * 0.2
            let c = p.x * 0.9 + sin(d)
            let b = p.x * 1.1 - t * 0.15
            let a = p.y * 1.3 + cos(b)
            let e = (p.x + p.y) * 2.1 + t * 0.1
            return sin(c) + 0.7 * sin(a) + 0.4 * sin(e)
        }
        fl_curl: fn(p: vec2, t: float) -> vec2 {
            let d = p.y * 0.7 + t * 0.2
            let c = p.x * 0.9 + sin(d)
            let b = p.x * 1.1 - t * 0.15
            let a = p.y * 1.3 + cos(b)
            let e = (p.x + p.y) * 2.1 + t * 0.1
            let dx = 0.9 * cos(c) - 0.77 * cos(a) * sin(b) + 0.84 * cos(e)
            let dy = 0.7 * cos(c) * cos(d) + 0.91 * cos(a) + 0.84 * cos(e)
            return vec2(dy, -dx)
        }
        bg_flow: fn(ray: vec3, uv: vec2, t: float) -> vec4 {
            let p = ray.xy * 3.0
            let k = self.fl_psi(p, t) * 6.0
            let fw = max(abs(dFdx(k)) + abs(dFdy(k)), 0.00001)
            let px = abs(fract(k) - 0.5) / fw
            let gap = smoothstep(3.0, 8.0, 1.0 / fw)
            let line = ((1.0 - smoothstep(0.5, 1.6, px)) + 0.3 * exp(-px * 0.3)) * gap
            let idx = self.hash21(vec2(floor(k), 3.0))
            let v = self.fl_curl(p, t)
            let speed = clamp(length(v) * 0.5, 0.0, 1.0)
            let ang = atan2(v.y, v.x)
            let tint = mix(self.col_accent.rgb, self.col_accent.gbr, 0.5 + 0.5 * sin(ang * 2.0 + t * 0.2))
            let glow = 0.4 + 0.6 * idx
            let col = self.col_dark.rgb * 0.6 + tint * line * glow * (0.3 + 0.6 * speed)
            return vec4(col * (1.0 + 1.2 * self.pool(uv)), 1.0)
        }

        pixel: fn() {
            let uv = self.pos
            let m = self.warp_map(uv)
            let ray = self.view_ray(m.xy)
            var c = vec4(0.0, 0.0, 0.0, 1.0)
            if self.kind < 0.5 {
                c = self.bg_grid(ray, uv, self.time)
            } else if self.kind < 1.5 {
                c = self.bg_stars(ray, uv, self.time)
            } else if self.kind < 2.5 {
                c = self.bg_terrain(ray, uv, self.time)
            } else if self.kind < 3.5 {
                c = self.bg_crystal(ray, uv, self.time)
            } else {
                c = self.bg_flow(ray, uv, self.time)
            }
            // One LSB of dither keeps the dark gradients from banding.
            let px = floor(uv * self.rect_size)
            let dth = (self.hash21(px) - 0.5) / 255.0
            return vec4(c.rgb + vec3(dth, dth, dth), 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBackdrop {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub kind: f32,
    #[live]
    pub time: f32,
    #[live]
    pub aspect: f32,
    #[live]
    pub warp_on: f32,
    #[live]
    pub focus_on: f32,
    #[live]
    pub pointer: Vec2f,
    #[live]
    pub focus: Vec4f,
    #[live]
    pub warp: Vec4f,
    #[live]
    pub tgt: Vec4f,
    #[live]
    pub drag: Vec4f,
    #[live]
    pub light: Vec4f,
}
