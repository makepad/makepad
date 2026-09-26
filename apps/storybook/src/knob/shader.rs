//! The knob engine's shaders: the Material Bench's knob, ported from its
//! GLSL to the shader DSL.
//!
//! One set of functions (`KnobCore`) holds the solid, the outline, the
//! shadows, the shading and the marks; two draw shaders spread it, and a
//! fill for the page under them shares its exposure:
//!
//! * `DrawTurnedKnob`, the 2D knob: the bench's `layerFlat` (the wells),
//!   `layerTurned` and `knob()` in one quad;
//! * `DrawKnobView3d`, the bench's `VIEW3D` program: a ray march of the same
//!   solid, with the same materials;
//! * `mod.storybook.KnobGroundFill`, a fill for a view's `draw_bg`: the
//!   page the knobs stand on, through the same exposure and roll-off, so a
//!   knob's quad meets it without a seam.
//!
//! What the bench kept in globals is threaded through arguments and return
//! vectors: the solid returns (height, wing weight, distance to the nearest
//! feature), the outline (distance, the disc's distance, the wing's half
//! width). Where the bench computed something from uniforms alone (the
//! profile under a dimple, `vEnvRef`), the host computes it and hands it in.
//!
//! Compile time on Direct3D matters (FXC unrolls loops whose trip count it
//! can see). So every loop that holds heavy work is a `loop` whose count the
//! compiler cannot know -- it stops on a runtime value, or on a bound with
//! `knob_zero` added, a uniform that is always 0 -- and each heavy function
//! has one call site: the solid in one loop, the face shading in one loop
//! over the layers, the reflection in one loop over the crease's normals.
//!
//! Every counter starts from a literal 0. Mesa 25.2's llvmpipe ran a
//! `loop` whose counter started from a uniform and whose first statement
//! was the exit test to its 65536-iteration guard (a miscompile: the same
//! loop with a literal start and a uniform bound runs its three turns), so
//! the uniform goes into the bound, never the start.
use crate::makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The exposure every knob surface and the page under them share:
    // `m_env` is reflection, perspective, exposure (linear), roll-off.
    let KnobExposure = {
        m_env: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        m_ground: uniform(vec4(0.93, 0.94, 0.96, 1.0))

        // Exposure and the highlight roll-off.
        hdr_out: fn(hc0: vec3) -> vec3 {
            var hc = hc0 * self.m_env.z
            let ro = clamp(self.m_env.w, 0.0, 1.0)
            if ro < 0.001 { return hc }
            let mx = max(max(hc.x, hc.y), hc.z)
            let lift = max(mx - 1.0, 0.0) * 0.6 * ro
            hc = hc + vec3(lift, lift, lift)
            let k = mix(1.0, 0.72, ro)
            let over = max(hc - vec3(k, k, k), vec3(0.0, 0.0, 0.0))
            return min(hc, vec3(k, k, k)) + (1.0 - k) * (vec3(1.0, 1.0, 1.0) - exp(-over / max(1.0 - k, 0.001)))
        }
    }

    // THE PAGE UNDER THE KNOBS, for a view's `draw_bg`: the ground through
    // the knobs' exposure, so their quads meet it without a seam.
    mod.storybook.KnobGroundFill = {
        ..KnobExposure,
        pixel: fn() {
            return vec4(self.hdr_out(self.m_ground.xyz), 1.0)
        }
    }

    let KnobCore = {
        ..KnobExposure,
        // Always 0: added to a loop's bound, so no compiler can count it.
        knob_zero: uniform(0.0)

        // Material. light: x, y, z, intensity; relief: bevel width and
        // curve, raise, specular; finish: occlusion, rim, gloss, roughness;
        // env: reflection, perspective, exposure (linear), roll-off;
        // surf: metal, clear coat, its roughness, studio; shadow: strength,
        // blur, falloff, contact; inner: inner shadow, its blur, lip, glow;
        // tune: tier, sink, hairline, occlusion reach; knob: profile depth,
        // mark finish, mark bevel, face gradient; env_ref: the flat face's
        // reflection (luminance, clear coat rgb).
        m_light: uniform(vec4(-0.35, -0.55, 0.66, 0.7))
        m_relief: uniform(vec4(4.0, 0.7, 4.0, 0.0))
        m_finish: uniform(vec4(0.25, 0.4, 0.0, 0.85))
        m_surf: uniform(vec4(0.0, 0.0, 0.08, 0.0))
        m_shadow: uniform(vec4(0.85, 12.0, 1.0, 0.25))
        m_inner: uniform(vec4(0.55, 10.0, 0.7, 0.0))
        m_tune: uniform(vec4(1.0, 4.0, 0.0, 1.2))
        m_knob: uniform(vec4(0.55, 0.0, 1.5, 0.45))
        m_env_ref: uniform(vec4(0.05, 0.0, 0.0, 0.0))
        m_body: uniform(vec4(0.93, 0.94, 0.96, 1.0))
        m_light_ink: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        m_shadow_ink: uniform(vec4(0.61, 0.64, 0.73, 1.0))
        m_glow_ink: uniform(vec4(0.49, 0.3, 1.0, 1.0))
        m_ptr_ink: uniform(vec4(0.49, 0.3, 1.0, 1.0))

        // Style. flute: columns, depth, sharpness, taper; cap: grip band
        // from and to, cap radius, spun; ptr: type, inner, outer, width;
        // ticks: count, radius, length, stroke; arc: radius, width, well,
        // knob well; wing: span, root, tip, width; wing2: crest height,
        // constant height, fillet, flat cut; wing_wc: constant width
        // (slope, value), end widths; wing_geo: ends' axis positions and
        // slopes; wing3: ends' radii, mode, base; cut: kind, count,
        // position, size; cut2: half length, floor, wall, fillet; cut3:
        // sphere, its centre, cutters through, foot notched; sil: outline
        // knots, their height scale, wing knots, wing floor; wt: the wing
        // section's taper; pre: profile height at 0.9 R and under a dimple.
        s_flute: uniform(vec4(0.0, 2.0, 0.5, 0.0))
        s_cap: uniform(vec4(0.44, 0.5, 0.0, 0.0))
        s_cap_ink: uniform(vec4(0.79, 0.8, 0.83, 1.0))
        s_ptr: uniform(vec4(1.0, 0.22, 0.58, 3.5))
        s_ticks: uniform(vec4(0.0, 0.78, 0.14, 1.8))
        s_arc: uniform(vec4(1.18, 0.0, 0.0, 0.0))
        s_wing: uniform(vec4(0.0, 0.0, 0.0, 0.5))
        s_wing2: uniform(vec4(0.6, 0.3, 0.06, 0.0))
        s_wing_wc: uniform(vec4(-1.0, -1.0, 1.0, 0.0))
        s_wing_geo: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        s_wing3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        s_cut: uniform(vec4(0.0, 8.0, 0.0, 0.3))
        s_cut2: uniform(vec4(0.6, 0.75, 0.1, 0.05))
        s_cut3: uniform(vec4(0.0, 0.2, -1.0, 0.0))
        s_sil: uniform(vec4(2.0, 1.0, 0.0, 0.0))
        s_wt: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        s_pre: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        // The curves, 16 bits a value as the bench packs them (BGRA u8):
        // profile, flute, wing width, wing height, wing section.
        knob_data: texture_2d(float)
        // The knots as floats: eight of the outline, four of the wing.
        knob_knots: texture_2d(float)
        // The self-shadow over the disc: .x the 2D measure, .y the 3D one.
        knob_self: texture_2d(float)

        // ---- lookups ----

        // One texel of a curve: (slope code, value), each from two bytes.
        kd_tap: fn(i: float, v: float) -> vec2 {
            let t = self.knob_data.sample_as_bgra_nearest(vec2((i + 0.5) / 256.0, v))
            return vec2(t.x * 255.0 * 256.0 + t.z * 255.0, t.y * 255.0 * 256.0 + t.w * 255.0) / 65535.0
        }

        // A baked curve at x in 0..1: two nearest taps lerped by hand.
        kd_row: fn(row: float, x: float) -> vec2 {
            let xx = clamp(x, 0.0, 1.0) * 255.0
            let i0 = floor(xx)
            let fr = xx - i0
            let v = (row + 0.5) / 5.0
            return mix(self.kd_tap(i0, v), self.kd_tap(min(i0 + 1.0, 255.0), v), fr)
        }

        // Knot i: 0..7 the outline, 8..11 the wing.
        kd_knot: fn(i: float) -> vec4 {
            let u = i * 4.0 + 0.5
            return vec4(
                self.knob_knots.sample_nearest(vec2(u / 48.0, 0.5), 0.0).x
                self.knob_knots.sample_nearest(vec2((u + 1.0) / 48.0, 0.5), 0.0).x
                self.knob_knots.sample_nearest(vec2((u + 2.0) / 48.0, 0.5), 0.0).x
                self.knob_knots.sample_nearest(vec2((u + 3.0) / 48.0, 0.5), 0.0).x
            )
        }

        rev_h: fn(r: float, R: float) -> float {
            let rn = r / max(R, 0.001)
            return self.kd_row(0.0, clamp(rn, 0.0, 1.0)).y - max(rn - 1.0, 0.0) * 8.0
        }

        // ---- the solid ----

        tooth: fn(x: float) -> float {
            let u = fract(x + 0.5)
            var m = (1.0 - u) * 2.0
            if u < 0.5 { m = u * 2.0 }
            let e = mix(1.0, 0.35, clamp(self.s_flute.z, 0.0, 1.0))
            return self.kd_row(1.0, clamp(pow(m, e), 0.0, 1.0)).y
        }

        grip_mod: fn(ang: float, rr: float, R: float, foot: float) -> float {
            let n = max(self.s_flute.x, 1.0)
            let period = 6.2831853 * max(rr, 0.5) / n
            let fade = smoothstep(2.5 * foot, 6.0 * foot, period)
            let rn = clamp(rr / max(R, 0.001), 0.0, 1.0)
            let band = clamp((rn - self.s_cap.x) / max(self.s_cap.y - self.s_cap.x, 0.001), 0.0, 1.0)
            let h = self.tooth(ang * n / 6.2831853)
            let tap = mix(1.0, 1.0 - band, clamp(self.s_flute.w, 0.0, 1.0))
            return (h * 2.0 - 1.0) * self.s_flute.y * fade * tap
        }

        // A wing end: distance, and where the end reaches along the axis.
        end_d: fn(x: float, y: float, xt: float, hwe: float, m: float, rcn: float) -> vec2 {
            let s = sqrt(1.0 + m * m)
            let rc = max(rcn, 0.5)
            let c = vec2(xt + m * rc / s, max(hwe - rc / s, 0.0))
            let xe = c.x + rc
            let dq = vec2(x, y) - c
            if dq.y >= 0.0 && dq.x + m * dq.y >= 0.0 {
                return vec2(length(dq) - rc, xe)
            }
            return vec2(max((y - hwe - m * (x - xt)) / s, x - xe), xe)
        }

        // The wing's outline from above: distance, and its half width here.
        bar_d: fn(q: vec2, R: float, bdir: vec2) -> vec2 {
            let al = dot(q, bdir)
            let ac = dot(q, vec2(bdir.y, -bdir.x))
            let w2 = self.s_wing.w * R * 0.5
            let hws = max(self.s_wing_wc.z * w2, 0.5)
            let hwe = max(self.s_wing_wc.w * w2, 0.5)
            let geo = self.s_wing_geo
            let a_start = geo.x * R
            let a_end = geo.y * R
            let l = max(a_end - a_start, 0.001)
            let along = clamp((al - a_start) / l, 0.0, 1.0)
            var ww = self.s_wing_wc.xy
            if self.s_wing_wc.y < 0.0 { ww = self.kd_row(2.0, along) }
            let hw = max(ww.y * w2, 0.0)
            let dhw = -(ww.x * 2.0 - 1.0) * 8.0 * w2 / l
            let y = abs(ac)
            let e = self.end_d(al, y, a_end, hwe, geo.w, self.s_wing3.y * R)
            let s = self.end_d(-al, y, -a_start, hws, geo.z, self.s_wing3.x * R)
            var halfw = hw
            if al > a_end {
                halfw = hwe + geo.w * (al - a_end)
            } else if al < a_start {
                halfw = hws + geo.z * (a_start - al)
            }
            halfw = max(halfw, 0.5)
            let dmid = max((y - hw) / sqrt(1.0 + dhw * dhw), max(al - e.y, -s.y - al))
            let mend = geo.w
            let mstart = geo.z
            let re = max(self.s_wing3.y * R, 0.5)
            let rs = max(self.s_wing3.x * R, 0.5)
            let be = min(a_end + mend * re / sqrt(1.0 + mend * mend), a_end) - 0.5 * re - 0.5
            let bs = min(-a_start + mstart * rs / sqrt(1.0 + mstart * mstart), -a_start) - 0.5 * rs - 0.5
            let d = mix(mix(dmid, s.x, smoothstep(bs, -a_start, -al)), e.x, smoothstep(be, a_end, al))
            return vec2(d, halfw)
        }

        blend_band: fn(kpx: float, dg: float, hk: float) -> float {
            let hpx = max(hk, 0.001)
            return max(kpx, 0.5) * dg / sqrt(1.0 + dg * dg * hpx * hpx)
        }

        // The cut's own distance field, from above.
        cut_sd: fn(q: vec2, R: float, spin: float, dir: vec2) -> float {
            let cut = self.s_cut.x
            if cut < 0.5 { return 1e9 }
            let sz = self.s_cut.w * R
            if cut < 1.5 {
                return length(q - dir * (self.s_cut.z * R)) - sz
            }
            if cut < 2.5 {
                let al = dot(q, dir)
                let ac = dot(q, vec2(dir.y, -dir.x))
                let l = self.s_cut2.x * R
                return length(vec2(al - clamp(al, -l, l), ac)) - sz
            }
            if cut < 3.5 {
                let n = max(self.s_cut.y, 1.0)
                let st = 6.2831853 / n
                var a = 0.0
                if length(q) > 0.001 { a = atan2(q.x, -q.y) }
                a = a - spin
                let ai = floor(a / st + 0.5) * st + spin
                return length(q - vec2(sin(ai), -cos(ai)) * (self.s_cut.z * R)) - sz
            }
            let e = length(q) - self.s_cut.z * R
            return abs(e) - sz
        }

        cut_min: fn(hu: float, hc: float, k: float) -> float {
            let hh = clamp(0.5 - 0.5 * (hc - hu) / k, 0.0, 1.0)
            return mix(hu, hc, hh) - k * hh * (1.0 - hh)
        }

        // THE SOLID: the knob's height at q over the knob's height, as the
        // bench's solidHeight -- the revolve, the grip, the flat, the wing
        // (a ridge added or two cutters taken away) and the cut -- with the
        // wing's weight at q and the distance to the nearest feature.
        solid: fn(q: vec2, R: float, spin: float, dir: vec2, foot: float, shsoft: float) -> vec3 {
            let rr = length(q)
            var nearf = 1e9
            var m = 0.0
            let r0 = self.s_cap.x
            let r1 = self.s_cap.y
            if self.s_flute.x >= 1.0 {
                let rn2 = rr / max(R, 0.001)
                let win = smoothstep(r0 - 0.05, r0 + 0.05, rn2) * (1.0 - smoothstep(r1 - 0.05, r1 + 0.05, rn2))
                var ang = 0.0
                if rr > 0.001 { ang = atan2(q.y, q.x) }
                m = self.grip_mod(ang - spin, rr, R, foot) * win
                nearf = min(nearf, max(r0 - 0.05 - rn2, rn2 - r1 - 0.05) * R)
            }
            let rn = (rr - m) / max(R, 0.001)
            let prf = self.kd_row(0.0, clamp(rn, 0.0, 1.0))
            var h = prf.y - max(rn - 1.0, 0.0) * 8.0
            let sl_r = abs(prf.x * 2.0 - 1.0) * 8.0 / max(R, 0.001)
            var wgt = 0.0
            let hk = R * max(self.m_knob.x, 0.001)
            let flatc = self.s_wing2.w
            if flatc > 0.001 && flatc < 0.999 {
                let df = flatc * R - dot(q, dir)
                nearf = min(nearf, df - 0.05 * R)
                let wall = 1.0 / max(0.02 * R, 0.5)
                h = self.cut_min(h, max(df, 0.0) * wall, self.blend_band(0.5, wall, hk))
            }
            let bar = self.s_wing.x
            let wmode = self.s_wing3.z
            let barfil = self.s_wing2.z
            if bar > 0.01 {
                let al = dot(q, dir)
                let along = clamp((al - self.s_wing.y * R) / max((self.s_wing.z - self.s_wing.y) * R, 0.001), 0.0, 1.0)
                var hwc = self.s_wing2.y
                if hwc < 0.0 { hwc = self.kd_row(3.0, along).y }
                if wmode < 0.5 {
                    // The ridge (armH), unioned with the body.
                    let hcrest = hwc * 2.0
                    let b = self.bar_d(q, R, dir)
                    let dw = b.x
                    let hw = b.y
                    nearf = min(nearf, dw - 2.0 * barfil * R - 0.1 * hw)
                    let t = 1.0 + dw / hw
                    let wp = self.kd_row(4.0, clamp(t, 0.0, 1.0))
                    var pf = wp.y
                    let dp = -(wp.x * 2.0 - 1.0) * 8.0
                    if t > 1.0 { pf = wp.y + min(dp, -1.0) * (t - 1.0) }
                    let rr2 = rr / max(R, 0.001)
                    var hold0 = self.s_pre.x
                    if self.s_flute.x >= 1.0 {
                        hold0 = self.kd_row(0.0, min(rr2, 0.9)).y
                    } else if rr2 <= 0.9 {
                        hold0 = prf.y
                    }
                    let hold = hold0 * (1.0 - smoothstep(0.95, 1.35, rr2))
                    let wbase = self.s_wing3.w
                    let base = min(wbase, max(max(h, 0.0), hold))
                    let ridge = hcrest + wbase
                    let slp = abs((hcrest + wbase - base) * dp / hw)
                    var ha = base + (hcrest + wbase - base) * pf - max(t - 1.0, 0.0) * 8.0
                    if shsoft > 0.0 {
                        ha = base + (hcrest + wbase - base) * wp.y * smoothstep(-0.5 * shsoft, 0.5 * shsoft, -dw)
                    }
                    let k = self.blend_band(barfil * R, sqrt(slp * slp + sl_r * sl_r), hk)
                    let gate = smoothstep(0.0, 0.03, ridge - h)
                    let hh = mix(step(0.0, ha - h), clamp(0.5 + 0.5 * (ha - h) / k, 0.0, 1.0), gate)
                    wgt = hh
                    h = mix(h, ha, hh) + k * hh * (1.0 - hh) * gate
                } else {
                    // The two cutters (cutterH), taken away.
                    let ac = abs(dot(q, vec2(dir.y, -dir.x)))
                    var wc = self.s_wing_wc.y
                    if wc < 0.0 { wc = self.kd_row(2.0, along).y }
                    let gap = max(wc * self.s_wing.w * R * 0.5, 0.0)
                    let depth = hwc * 2.0
                    let x = (ac - gap) / max(R, 0.001)
                    nearf = min(nearf, gap - ac - 2.0 * barfil * R)
                    if x >= 0.0 {
                        let wp = self.kd_row(4.0, min(x, 1.0))
                        let dv = -(wp.x * 2.0 - 1.0) * 8.0
                        var v = wp.y
                        if x > 1.0 { v = wp.y + dv * (x - 1.0) }
                        let slc = abs(depth * dv / max(R, 0.001))
                        let hc2 = 1.0 - depth * (1.0 - v)
                        h = self.cut_min(h, hc2, self.blend_band(barfil * R, sqrt(slc * slc + sl_r * sl_r), hk))
                    }
                }
            }
            let cut = self.s_cut.x
            if cut > 0.5 {
                var hc = 0.0
                var dgc = 1.0 / max(self.s_cut2.z * R, 0.5)
                if cut < 1.5 && self.s_cut3.x > 0.5 {
                    // A spherical dish (sphereH).
                    let rs = max(self.s_cut.w * R, 0.5)
                    let d = length(q - dir * (self.s_cut.z * R))
                    nearf = min(nearf, d - rs - 0.05 * R)
                    let zc = self.s_pre.y + self.s_cut3.y * R / hk
                    if d >= rs {
                        dgc = 8.0 / hk
                        hc = zc + (d - rs) * 8.0 / hk
                    } else {
                        let w = sqrt(rs * rs - d * d)
                        dgc = min(d / max(w, 0.001), 8.0) / hk
                        hc = zc - w / hk
                    }
                } else {
                    // A walled cut with a floor (cutHH).
                    let d = self.cut_sd(q, R, spin, dir)
                    let cw = max(self.s_cut2.z * R, 0.5)
                    let kf = max(self.s_cut2.w * R, 0.5)
                    nearf = min(nearf, d - (1.1 - self.s_cut2.y) * cw - kf)
                    var mm = 0.0
                    if d >= kf {
                        mm = d
                    } else if d > -kf {
                        let tt = d + kf
                        mm = tt * tt / (4.0 * kf)
                    }
                    hc = self.s_cut2.y + mm / cw
                }
                h = self.cut_min(h, hc, self.blend_band(max(self.s_cut2.w * R, 0.5), dgc, hk))
            }
            return vec3(h, wgt, nearf)
        }

        // What cuts the foot's outline: the flat, a through cut, the
        // cutters.
        foot_cut: fn(q: vec2, R: float, spin: float, dir: vec2) -> float {
            var cd = -1e9
            let flatc = self.s_wing2.w
            if flatc > 0.001 && flatc < 0.999 { cd = max(cd, dot(q, dir) - flatc * R) }
            let cut = self.s_cut.x
            if cut > 0.5 && self.s_cut2.y < 0.001 && (cut > 1.5 || self.s_cut3.x < 0.5) {
                let kf = max(self.s_cut2.w * R, 0.5)
                cd = max(cd, -(self.cut_sd(q, R, spin, dir) + kf))
                if cut > 3.5 && length(q) > self.s_cut.z * R {
                    cd = max(cd, length(q) - (self.s_cut.z - self.s_cut.w) * R - kf)
                }
            }
            if self.s_wing.x > 0.01 && self.s_wing3.z > 0.5 && self.s_cut3.z >= 0.0 {
                let wmax = max(max(self.s_wing_wc.z, self.s_wing_wc.w) * self.s_wing.w * R * 0.5, 0.0)
                cd = max(cd, abs(dot(q, vec2(-dir.y, dir.x))) - (wmax + self.s_cut3.z * R))
            }
            return cd
        }

        // THE OUTLINE from above (the bench's shapeD for a turned knob):
        // distance, the disc's own distance, and the wing's half width.
        knob_shape: fn(q: vec2, R: float, spin: float, dir: vec2, foot: float) -> vec3 {
            let rq = length(q)
            let rn2 = rq / max(R, 0.001)
            let r0 = self.s_cap.x
            let r1 = self.s_cap.y
            var rad = R
            if self.s_flute.x >= 1.0 {
                let win = smoothstep(r0 - 0.05, r0 + 0.05, rn2) * (1.0 - smoothstep(r1 - 0.05, r1 + 0.05, rn2))
                var a2 = 0.0
                if rq > 0.001 { a2 = atan2(q.y, q.x) }
                rad = rad + self.grip_mod(a2 - spin, rq, R, foot) * win
            }
            var cd = rq - rad
            let flatc = self.s_wing2.w
            if flatc > 0.001 && flatc < 0.999 { cd = max(cd, dot(q, dir) - flatc * R) }
            let disc = cd
            var bar_half = R
            let bar = self.s_wing.x
            let wmode = self.s_wing3.z
            if bar > 0.01 && wmode > 0.5 && self.s_cut3.z >= 0.0 {
                let al2 = dot(q, dir)
                let ac2 = abs(dot(q, vec2(dir.y, -dir.x)))
                let along2 = clamp((al2 - self.s_wing.y * R) / max((self.s_wing.z - self.s_wing.y) * R, 0.001), 0.0, 1.0)
                var wc = self.s_wing_wc.y
                if wc < 0.0 { wc = self.kd_row(2.0, along2).y }
                let bs = ac2 - (max(wc * self.s_wing.w * R * 0.5, 0.0) + self.s_cut3.z * R)
                let ks = max(self.s_wing2.z * R, 0.5)
                let hs = clamp(0.5 + 0.5 * (bs - cd) / ks, 0.0, 1.0)
                cd = mix(cd, bs, hs) + ks * hs * (1.0 - hs)
            }
            if bar > 0.01 && wmode < 0.5 {
                let b = self.bar_d(q, R, dir)
                bar_half = b.y
                let kk = max(self.s_wing2.z * R, 0.001)
                let hh = clamp(0.5 + 0.5 * (b.x - cd) / kk, 0.0, 1.0)
                cd = mix(b.x, cd, hh) - kk * hh * (1.0 - hh)
            }
            let cut = self.s_cut.x
            if cut > 0.5 && self.s_cut2.y < 0.001 && (cut > 1.5 || self.s_cut3.x < 0.5) {
                let dc = self.cut_sd(q, R, spin, dir) + max(self.s_cut2.w * R, 0.5)
                let kc = max(self.s_cut2.w * R, 0.001)
                let hc = clamp(0.5 - 0.5 * (cd + dc) / kc, 0.0, 1.0)
                cd = mix(cd, -dc, hc) + kc * hc * (1.0 - hc)
                if cut > 3.5 && rq > self.s_cut.z * R {
                    cd = max(cd, rq - (self.s_cut.z - self.s_cut.w) * R - max(self.s_cut2.w * R, 0.5))
                }
            }
            return vec3(cd, disc, bar_half)
        }

        // ---- shadows ----

        shadow_dir: fn() -> vec2 {
            let ll = length(self.m_light.xy)
            if ll > 0.0001 { return -self.m_light.xy / ll }
            return vec2(0.0, 1.0)
        }

        tail: fn(d: float, bl: float) -> float {
            let dd = max(d, 0.0) / max(bl, 0.001)
            return mix(clamp(1.0 - dd / 3.0, 0.0, 1.0), exp(-dd), clamp(self.m_shadow.z, 0.0, 1.0))
        }

        // The hull of two discs: distance, and where along a to b.
        hull2: fn(p: vec2, a: vec2, b: vec2, ra: float, rb: float) -> vec2 {
            let ba = b - a
            let l = length(ba)
            let dr = ra - rb
            if l <= abs(dr) + 0.0001 {
                let da = length(p - a) - ra
                let db = length(p - b) - rb
                if da < db { return vec2(da, 0.0) }
                return vec2(db, 1.0)
            }
            let ax = ba / l
            let pa = p - a
            let y = dot(pa, ax)
            let x = abs(ax.x * pa.y - ax.y * pa.x)
            let sb = dr / l
            let cb = sqrt(1.0 - sb * sb)
            let k = cb * y - sb * x
            if k < 0.0 { return vec2(length(pa) - ra, 0.0) }
            if k > cb * l { return vec2(length(p - b) - rb, 1.0) }
            return vec2(x * cb + y * sb - ra, k / (cb * l))
        }

        cross_e: fn(a: vec2, b: vec2, p: vec2) -> float {
            let ea = b - a
            let wa = p - a
            return ea.x * wa.y - ea.y * wa.x
        }

        sweep_n: fn(d: float, z: float, zp: float, blur: float, thr: float, mode: float, px: float) -> float {
            var bl = max(0.5 * blur * mix(0.3, 1.0, clamp(z - zp, 0.0, 1.0)), (z - zp) * thr)
            if mode > 0.5 { bl = max(0.5 * bl, 1.5 * px) }
            bl = max(bl, 0.001)
            if mode < 0.5 { return max(d + 0.3 * bl, 0.0) / bl }
            return d / bl
        }

        // THE ANALYTIC SWEEP (the bench's sweepDD): the solid laid down
        // along the light, as the revolve's outline knots swept by height
        // and the wing's knots as hulls, in blur lengths. Mode 0 is the
        // cast shadow on the ground, mode 1 the wing's shadow on the knob's
        // own top from height zp.
        sweep_dd: fn(q: vec2, R: float, spin: float, dir: vec2, sh: vec2, zp: float, blur: float, mode: float, px: float) -> float {
            let thr = 0.11 * length(sh)
            var best = 1e9
            if mode < 0.5 {
                let sdv = self.shadow_dir()
                let qn = q / R
                let ya = dot(qn, sdv)
                let xa = abs(sdv.x * qn.y - sdv.y * qn.x)
                let notch = self.s_wing2.w > 0.001 || self.s_cut.x > 0.5 || self.s_wing3.z > 0.5
                var k0 = self.kd_knot(0.0)
                var i = 0.0
                loop {
                    if i + 1.0 >= self.s_sil.x { break }
                    let k1 = self.kd_knot(i + 1.0)
                    let y = ya - k0.x
                    let l = k1.x - k0.x
                    let da = length(vec2(xa, y)) - k0.y
                    let db = length(vec2(xa, ya - k1.x)) - k1.y
                    let kk = k0.w * y - k0.z * xa
                    var d = 0.0
                    var u = 0.0
                    if k0.w < 0.001 {
                        d = min(da, db)
                        u = step(db, da)
                    } else {
                        if kk < 0.0 {
                            d = da
                        } else if kk > k0.w * l {
                            d = db
                        } else {
                            d = xa * k0.w + y * k0.z - k0.y
                        }
                        u = clamp(kk / max(k0.w * l, 0.00001), 0.0, 1.0)
                    }
                    let z = mix(k0.x, k1.x, u) / self.s_sil.y
                    d = d * R
                    if notch { d = max(d, self.foot_cut(q - sh * z, R, spin, dir)) }
                    best = min(best, self.sweep_n(d, z, zp, blur, thr, mode, px))
                    k0 = k1
                    i = i + 1.0
                }
            }
            let wkn = self.s_sil.z
            if wkn > 0.5 {
                let wkb = self.s_sil.w
                let wt = self.s_wt
                var pb0 = vec2(0.0, 0.0)
                var pt0 = vec2(0.0, 0.0)
                var rb0 = 0.0
                var rt0 = 0.0
                var zb0 = 0.0
                var zt0 = 0.0
                var lv0 = 0.0
                var i = 0.0
                loop {
                    if i >= wkn { break }
                    let kn = self.kd_knot(8.0 + i)
                    var zofs = 0.12
                    if mode < 0.5 { zofs = 0.02 }
                    let z0 = max(wkb, zp + zofs)
                    let live = step(z0 + 0.001, kn.z)
                    let f = clamp((z0 - wkb) / max(kn.z - wkb, 0.001), 0.0, 1.0) * 3.0
                    var tw = mix(wt.z, wt.w, f - 2.0)
                    if f < 1.0 {
                        tw = mix(wt.x, wt.y, f)
                    } else if f < 2.0 {
                        tw = mix(wt.y, wt.z, f - 1.0)
                    }
                    let cc = dir * (kn.x * R)
                    let pb = cc + sh * (z0 - zp) * live
                    let pt = cc + sh * (kn.z - zp) * live
                    let rb = kn.y * R * tw * live
                    let rt = kn.y * R * wt.w * live
                    if i > 0.5 && max(live, lv0) > 0.5 {
                        var dm = 1e9
                        var zm = 0.0
                        var e = 0.0
                        loop {
                            if e > 3.5 + self.knob_zero { break }
                            // The four sides: bottoms, tops, then each end.
                            var ea = pb
                            var eb = pt
                            var ra = rb
                            var rbb = rt
                            var za = z0
                            var zb = kn.z
                            if e < 0.5 {
                                ea = pb0
                                eb = pb
                                ra = rb0
                                rbb = rb
                                za = zb0
                                zb = z0
                            } else if e < 1.5 {
                                ea = pt0
                                eb = pt
                                ra = rt0
                                rbb = rt
                                za = zt0
                                zb = kn.z
                            } else if e < 2.5 {
                                ea = pb0
                                eb = pt0
                                ra = rb0
                                rbb = rt0
                                za = zb0
                                zb = zt0
                            }
                            let hu = self.hull2(q, ea, eb, ra, rbb)
                            if hu.x < dm {
                                dm = hu.x
                                zm = mix(za, zb, hu.y)
                            }
                            e = e + 1.0
                        }
                        let s1 = self.cross_e(pb0, pb, q)
                        let s2 = self.cross_e(pb, pt, q)
                        let s3 = self.cross_e(pt, pt0, q)
                        let s4 = self.cross_e(pt0, pb0, q)
                        if (s1 > 0.0 && s2 > 0.0 && s3 > 0.0 && s4 > 0.0) || (s1 < 0.0 && s2 < 0.0 && s3 < 0.0 && s4 < 0.0) {
                            dm = min(dm, -R)
                        }
                        best = min(best, self.sweep_n(dm, zm, zp, blur, thr, mode, px))
                    }
                    pb0 = pb
                    pt0 = pt
                    rb0 = rb
                    rt0 = rt
                    zb0 = z0
                    zt0 = kn.z
                    lv0 = live
                    i = i + 1.0
                }
            }
            return best
        }

        // ---- shading ----

        in_shadow: fn(c: vec3, a: float) -> vec3 {
            return c * mix(vec3(1.0, 1.0, 1.0), self.m_shadow_ink.xyz, clamp(a, 0.0, 1.0))
        }

        in_light: fn(c: vec3, a: float) -> vec3 {
            return vec3(1.0, 1.0, 1.0) - (vec3(1.0, 1.0, 1.0) - c) * (vec3(1.0, 1.0, 1.0) - self.m_light_ink.xyz * clamp(a, 0.0, 1.0))
        }

        light_face: fn(base: vec3, dl: float, vis: float, ao: float) -> vec3 {
            let si = self.m_shadow_ink.xyz
            let lf = si * clamp(ao, 0.0, 1.0) + (vec3(1.0, 1.0, 1.0) - si) * (clamp(dl, 0.0, 1.0) * clamp(vis, 0.0, 1.0))
            return self.in_light(base * lf, max(dl - 1.0, 0.0) * clamp(vis, 0.0, 1.0))
        }

        lam: fn(ndl: float) -> float {
            return smoothstep(-0.35, 1.0, ndl)
        }

        ggx_lobe: fn(nh: float, rgh: float) -> float {
            let a = max(rgh, 0.03)
            let a2 = a * a
            let dd = nh * nh * (a2 - 1.0) + 1.0
            return 0.25 * a2 / max(dd * dd, 0.000001)
        }

        // Diffuse and a GGX highlight, the flat face's own highlight taken
        // away, the roughness widened by the pixel's normal spread.
        lit_of_v: fn(n: vec3, spec: vec2, v: vec3, rgh_aa: float) -> vec2 {
            let l = normalize(self.m_light.xyz)
            let diff = self.lam(dot(n, l)) * self.m_light.w
            let h = normalize(l + v)
            let rk = sqrt(spec.y * spec.y + 0.01 + 0.5 * rgh_aa * rgh_aa)
            let lobe = self.ggx_lobe(max(dot(n, h), 0.0), rk) - self.ggx_lobe(normalize(l + vec3(0.0, 0.0, 1.0)).z, rk)
            return vec2(diff, max(lobe, 0.0) * spec.x * self.m_light.w * step(0.0, dot(n, l)))
        }

        slope_of: fn(d: float, w: float, curve: float) -> float {
            let t = clamp(-d / max(w, 0.001), 0.0, 1.0)
            let omt = 1.0 - t
            let soft = 6.0 * t * omt
            let rnd = omt / max(sqrt(max(1.0 - omt * omt, 0.0)), 0.125)
            return min(mix(soft, rnd, clamp(curve, 0.0, 1.0)), 8.0)
        }

        normal_of: fn(d: float, g: vec2, w: float, curve: float, elev: float, bias: float) -> vec3 {
            let s = (self.slope_of(d, w, curve) + bias) * elev
            return normalize(vec3(g.x * s, g.y * s, 1.0))
        }

        rect_d: fn(rv: vec3, dir: vec3, hs: vec2, rc: float) -> float {
            let rlu = normalize(cross(vec3(0.0, 0.0, 1.0), dir) + vec3(0.0001, 0.0, 0.0))
            let rlv = cross(dir, rlu)
            let rlc = dot(rv, dir)
            if rlc < 0.05 { return 1e9 }
            let rlq = abs(vec2(dot(rv, rlu), dot(rv, rlv)) / rlc) - hs + vec2(rc, rc)
            return length(max(rlq, vec2(0.0, 0.0))) + min(max(rlq.x, rlq.y), 0.0) - rc
        }

        rect_cov: fn(d: float, w: float) -> float {
            return 1.0 - smoothstep(-w, w, d)
        }

        // THE STUDIOS (the bench's envHDR2 at one roughness): 0 a softbox
        // studio, 1 the chrome studio (ceiling panel, key box, three strips,
        // a horizon line), 2 outdoors (sky, ground, a sun). `fp` is the
        // reflection's footprint, which widens every edge it crosses.
        env_hdr: fn(rv: vec3, rgh: float, kvis: float, fp: float) -> vec3 {
            let kl = normalize(self.m_light.xyz)
            let li = self.m_light.w
            let w = 0.03 + rgh * 0.45
            let en = 1.0 / (1.0 + rgh * 3.0)
            let wf = sqrt(w * w + fp * fp)
            let envk = self.m_surf.w
            if envk > 1.5 {
                let sky = mix(vec3(1.5, 1.455, 1.38), vec3(0.28, 0.46, 0.85), sqrt(clamp(rv.z, 0.0, 1.0)))
                let gnd = vec3(0.16, 0.145, 0.13) * mix(0.7, 1.0, clamp(-rv.z * 3.0, 0.0, 1.0))
                let sun = vec3(1.0, 0.94, 0.84) * 40.0 * li * kvis
                let ck = 1.0 - dot(rv, kl)
                let ks = mix(900.0, 20.0, rgh)
                let ksf = ks / (1.0 + 0.5 * ks * fp * fp)
                return mix(gnd, sky, smoothstep(-0.5 * w, 0.5 * w, rv.z)) + sun * en * (ksf / ks) * exp(-ck * ksf)
            }
            if envk > 0.5 {
                let floor_c = mix(vec3(0.55, 0.55, 0.55), self.m_ground.xyz, 0.3) * 0.75
                let wv = 0.03 + 0.6 * smoothstep(0.0, 0.9, rv.z)
                let wall_c = vec3(wv, wv, wv)
                let dt = self.rect_d(rv, vec3(0.0, 0.0, 1.0), vec2(0.36, 0.36), 0.36)
                let dk = self.rect_d(rv, kl, vec2(0.42, 0.26), 0.08)
                var ec = mix(floor_c, wall_c, smoothstep(-0.4 * w, 0.4 * w, rv.z))
                ec = ec + vec3(1.1, 1.1, 1.1) * en * self.rect_cov(dt, wf)
                ec = ec + vec3(3.5, 3.5, 3.5) * (li * en * kvis) * self.rect_cov(dk, wf)
                let az0 = atan2(kl.y, kl.x)
                var si = 0.0
                loop {
                    if si > 2.5 + self.knob_zero { break }
                    let az = az0 + 1.5708 * (si + 1.0)
                    let ds = self.rect_d(rv, normalize(vec3(cos(az), sin(az), 0.35)), vec2(0.05, 0.8), 0.02)
                    let sv = 5.0 * en * (0.05 + w) / (0.05 + wf) * self.rect_cov(ds, wf)
                    ec = ec + vec3(sv, sv, sv)
                    si = si + 1.0
                }
                let hz = abs(rv.z - 0.12)
                let lw = w * 0.6 + 0.01
                let lwf = sqrt(lw * lw + fp * fp)
                let lv = 2.5 * en * lw / lwf * (1.0 - smoothstep(0.0, lwf, hz))
                return ec + vec3(lv, lv, lv)
            }
            let wall = mix(vec3(0.5, 0.5, 0.5), self.m_ground.xyz, 0.5)
            let eb = wall * mix(0.25, 0.75, smoothstep(-0.2, 1.0, rv.z)) * mix(0.2, 1.0, smoothstep(-0.3, 0.05, rv.z))
            let dk = self.rect_d(rv, kl, vec2(0.42, 0.26), 0.08)
            let df = self.rect_d(rv, normalize(vec3(-kl.x + 0.0001, -kl.y, 0.45)), vec2(0.07, 0.55), 0.0)
            let a = 7.0 * li * en * kvis * self.rect_cov(dk, wf) + 2.2 * en * (0.07 + w) / (0.07 + wf) * self.rect_cov(df, wf)
            return eb + vec3(a, a, a)
        }

        // THE REFLECTION: the studio in the face, Fresnel-weighted, in the
        // part's own colour for a metal; and the clear coat's over it. The
        // body's and the coat's reflections are one call site in a loop.
        env_refl: fn(col: vec3, n: vec3, v: vec3, vis: float, env_rgh: float, rgh_aa: float) -> vec3 {
            let env = self.m_env.x
            if env < 0.001 { return col }
            let rgh0 = clamp(self.m_finish.w, 0.0, 1.0)
            let mr = max(rgh0, env_rgh)
            let rgh = min(sqrt(mr * mr + 0.5 * rgh_aa * rgh_aa), 1.0)
            let mtl = clamp(self.m_surf.x, 0.0, 1.0)
            let nv = clamp(dot(n, v), 0.0, 1.0)
            let fr5 = pow(1.0 - nv, 5.0) * (1.0 - 0.7 * rgh)
            let rv = n * (2.0 * dot(n, v)) - v
            let coat = self.m_surf.y
            let cr = clamp(self.m_surf.z, 0.0, 1.0)
            var e1 = vec3(0.0, 0.0, 0.0)
            var ec = vec3(0.0, 0.0, 0.0)
            var ep = 0.0
            loop {
                if ep > 1.5 + self.knob_zero || (ep > 0.5 && coat < 0.001) { break }
                var r = rgh
                if ep > 0.5 { r = cr }
                let e = self.env_hdr(rv, r, vis, 2.0 * rgh_aa)
                if ep < 0.5 {
                    e1 = e
                } else {
                    ec = e
                }
                ep = ep + 1.0
            }
            let l0 = self.m_env_ref.x
            let od = col + env * ((0.04 + 0.96 * fr5) * e1 - vec3(0.04, 0.04, 0.04) * l0)
            let fm = col + (vec3(1.0, 1.0, 1.0) - col) * fr5
            var o = mix(od, mix(col, fm * e1 / l0, env), mtl)
            if coat > 0.001 {
                let fc = 0.04 + 0.96 * fr5 * (1.0 - 0.7 * cr)
                o = o + env * coat * (fc * ec - 0.04 * self.m_env_ref.yzw)
            }
            return max(o, vec3(0.0, 0.0, 0.0))
        }

        // THE FACE (the bench's shadeFace): lit from its normal, occluded,
        // with the hairline, the well's inner shadow, the reflection -- over
        // the crease's two one-sided normals where the surface folds inside
        // the pixel, keeping the darker, so a thin bright line cannot sparkle
        // -- the highlight, the coat's highlight, the rim and the gloss.
        // aa: roughness from the solid's curvature, crease, the one-sided
        // domes; sab: the one-sided gradients.
        shade_face: fn(base: vec3, d: float, g: vec2, uv: vec2, depth: float, convex: float, bw: float, curve: float, dome: float, insh: float, turned: float, vis0: float, px: float, aa: vec4, sab: vec4, env_rgh: float, spec_scale: float) -> vec3 {
            let level = self.m_tune.x
            if level < 0.5 { return base }
            let l = self.m_light
            let n = self.normal_of(d, g, bw, curve, convex, dome)
            let v = normalize(vec3(-(uv - vec2(0.5, 0.5)) * 2.0 * self.m_env.y, 1.0))
            let rgh_aa = max(aa.x, min(length(self.normal_of(d + px, g, bw, curve, convex, dome) - n), 1.0))
            let lt = self.lit_of_v(n, vec2(self.m_relief.w * spec_scale, clamp(self.m_finish.w, 0.0, 1.0)), v, rgh_aa)
            let flatv = self.lam(normalize(l.xyz).z) * l.w
            let key = (lt.x - flatv) * 1.5
            var dl = 1.0 + smoothstep(0.0, 1.0, key) - smoothstep(0.0, 1.0, -key)
            var o = base
            var curv = 1.0
            if turned > 0.5 { curv = 0.0 }
            let fg = self.m_knob.w
            if fg * curv > 0.001 {
                let ax = normalize(l.xy + vec2(0.000001, 0.000001))
                let t = dot(uv - vec2(0.5, 0.5), ax) * 2.0 * sign(convex)
                let gink = mix(self.in_shadow(o, 1.0), self.in_light(o, 1.0), smoothstep(-1.0, 1.0, t))
                o = mix(o, gink, fg * curv * 0.5 * smoothstep(0.0, 1.0, abs(t)))
            }
            let sink = self.m_tune.y
            let concave = clamp(-convex / max(sink, 0.001), 0.0, 1.0)
            let aor = (1.0 - smoothstep(0.0, max(max(bw, 0.001) * self.m_tune.w, 0.001), -d)) * step(d, 0.0)
            let ao = 1.0 - aor * self.m_finish.x * concave
            let hair = self.m_tune.z
            if hair > 0.001 {
                let band = 1.0 - smoothstep(0.0, 1.4, abs(d))
                let facing = dot(g, normalize(l.xy + vec2(0.000001, 0.000001))) * sign(convex)
                dl = dl + band * facing * hair
            }
            let sunk = clamp(-depth / max(sink, 0.001), 0.0, 1.0)
            var vis = vis0
            let inner = self.m_inner.x
            if sunk > 0.001 && inner > 0.001 {
                vis = vis * (1.0 - clamp(insh * step(d, 0.0) * inner * sunk, 0.0, 1.0))
            }
            let mtl = clamp(self.m_surf.x, 0.0, 1.0)
            o = self.light_face(o, mix(dl, 1.0, mtl), mix(vis, 1.0, mtl), ao)
            let crease = aa.y
            var oe = o
            var oside = vec3(0.0, 0.0, 0.0)
            var k = 0.0
            loop {
                if k > 2.5 + self.knob_zero || (k > 0.5 && crease < 0.001) { break }
                var nk = n
                if k > 1.5 {
                    nk = self.normal_of(d, sab.zw, bw, curve, convex, aa.w)
                } else if k > 0.5 {
                    nk = self.normal_of(d, sab.xy, bw, curve, convex, aa.z)
                }
                let refl = self.env_refl(o, nk, v, vis, env_rgh, rgh_aa)
                if k < 0.5 {
                    oe = refl
                } else {
                    oside = max(oside, refl)
                }
                k = k + 1.0
            }
            if crease > 0.001 { oe = mix(oe, min(oe, oside), crease) }
            o = oe
            o = o + mix(vec3(1.0, 1.0, 1.0), base * 1.6, mtl) * lt.y * vis
            let coat = self.m_surf.y
            if coat > 0.001 {
                let cs = self.lit_of_v(n, vec2(coat * 0.6, self.m_surf.z), v, rgh_aa).y * vis
                o = o + vec3(cs, cs, cs)
            }
            if level < 1.5 { return o }
            let fw = max(bw * 0.5, 0.5)
            let rim = (1.0 - smoothstep(0.0, fw, -d)) * pow(max(dot(n, normalize(l.xyz)), 0.0), 0.8) * step(d, 0.0)
            o = self.in_light(o, clamp(rim * self.m_finish.y, 0.0, 1.0))
            let sky = normalize(vec3(0.0, -0.6, 0.8))
            let s = max(dot(n, sky), 0.0)
            let gloss = clamp((s * s - 0.64) * 2.8, 0.0, 1.0)
            o = self.in_light(o, clamp(gloss * self.m_finish.z * step(d, 0.0), 0.0, 1.0))
            return o
        }

        // ---- marks ----

        ptr_d: fn(q: vec2, ang: float, R: float) -> float {
            let ptype = self.s_ptr.x
            if ptype < 0.5 { return 1e9 }
            let dir = vec2(sin(ang), -cos(ang))
            let pw = max(self.s_ptr.w * R / 56.0, 1.2)
            let al = dot(q, dir)
            let ac = dot(q, vec2(dir.y, -dir.x))
            let r0 = self.s_ptr.y * R
            let r1 = self.s_ptr.z * R
            if ptype < 1.5 {
                let d2 = vec2(abs(ac) - pw * 0.5, max(r0 - al, al - r1))
                return min(max(d2.x, d2.y), 0.0) + length(max(d2, vec2(0.0, 0.0)))
            }
            if ptype < 2.5 {
                let t = clamp((al - r0) / max(r1 - r0, 0.001), 0.0, 1.0)
                let w = mix(pw, 0.6, t) * 0.5
                let d2 = vec2(abs(ac) - w, max(r0 - al, al - r1))
                return min(max(d2.x, d2.y), 0.0) + length(max(d2, vec2(0.0, 0.0)))
            }
            return length(q - dir * r1) - pw * 0.6
        }

        tick_seg: fn(q: vec2, R: float, ta: float) -> float {
            let dir = vec2(sin(ta), -cos(ta))
            let pa = q - dir * (self.s_ticks.y * R)
            let ba = dir * (self.s_ticks.z * R)
            let tt = clamp(dot(pa, ba) / max(dot(ba, ba), 0.000001), 0.0, 1.0)
            return length(pa - ba * tt) - max(self.s_ticks.w * R / 56.0, 1.0) * 0.5
        }

        // The nearest tick: distance and its angle.
        tick_d: fn(q: vec2, R: float) -> vec2 {
            let ticks = self.s_ticks.x
            if ticks < 1.0 { return vec2(1e9, 0.0) }
            var a = 0.0
            if length(q) > 0.001 { a = atan2(q.x, -q.y) }
            let sweep = 2.35619
            let step2 = (sweep * 2.0) / max(ticks, 1.0)
            if abs(a) > sweep + step2 * 0.49 { return vec2(1e9, 0.0) }
            let ta = -sweep + floor((a + sweep) / step2 + 0.5) * step2
            return vec2(self.tick_seg(q, R, ta), ta)
        }

        arc_d: fn(q: vec2, R: float, spin: float) -> float {
            let arcw = self.s_arc.y
            if arcw < 0.01 { return 1e9 }
            var a = 0.0
            if length(q) > 0.001 { a = atan2(q.x, -q.y) }
            let lo = min(-2.35619, spin)
            let hi = max(-2.35619, spin)
            let ca = clamp(a, lo, hi)
            let on = vec2(sin(ca), -cos(ca)) * self.s_arc.x * R
            return length(q - on) - arcw * 0.5
        }

        // A tick (m 0) or the pointer (m 1), for the marks' gradients.
        mark_d: fn(q: vec2, m: float, R: float, spin: float, ta: float) -> float {
            if m < 0.5 { return self.tick_seg(q, R, ta) }
            return self.ptr_d(q, spin, R)
        }

        // THE MARK'S FINISH: paint, engraved, embossed, or an LED in a
        // housing with its halo.
        mark_ink: fn(col: vec3, d: float, gm: vec2, wdt: float, lit: float, inkc: vec3, px: float, spec_scale: float) -> vec3 {
            let cov = 1.0 - smoothstep(-px, px, d)
            let pfin = self.m_knob.y
            if pfin < 0.5 { return mix(col, inkc, cov) }
            let mbev = self.m_knob.z
            let up = vec3(0.0, 0.0, 1.0)
            let li = self.m_light_ink.xyz
            if pfin < 2.5 {
                var elev = 1.0
                if pfin < 1.5 { elev = -1.0 }
                let n = self.normal_of(d, gm, max(mbev, 0.6), 0.7, elev, 0.0)
                let lt = self.lit_of_v(n, vec2(self.m_relief.w * spec_scale, 0.35), up, 0.0)
                var base = mix(col, inkc, 0.3)
                if elev < 0.0 { base = base * 0.8 }
                let sh = base * (0.55 + 0.75 * lt.x) + li * lt.y * 0.6
                return mix(col, sh, cov)
            }
            var c2 = col
            if mbev > 0.05 {
                let hcov = 1.0 - smoothstep(-px, px, d - mbev)
                let nh = self.normal_of(d - mbev, gm, mbev, 0.7, -1.0, 0.0)
                let lh = self.lit_of_v(nh, vec2(self.m_relief.w * spec_scale, 0.35), up, 0.0)
                let ring = c2 * 0.5 * (0.5 + 0.8 * lh.x) + li * lh.y * 0.5
                c2 = mix(c2, ring, hcov * (1.0 - cov))
            }
            let nl = self.normal_of(d, gm, wdt * 0.5, 1.0, 1.0, 0.0)
            let ll = self.lit_of_v(nl, vec2(0.6, 0.45), up, 0.0)
            let glow = self.m_glow_ink.xyz
            let core = mix(glow, vec3(1.0, 1.0, 1.0), 0.35) * (0.8 + 0.3 * ll.x) + li * ll.y * 0.5
            let offc = mix(c2, inkc, 0.5) * 0.6 * (0.7 + 0.5 * ll.x) + li * ll.y * 0.35
            let hl = self.tail(d, wdt * 0.9) * 0.5
            c2 = mix(c2, glow, hl * lit * (1.0 - cov))
            return mix(c2, mix(offc, core, lit), cov)
        }

        inner_glow: fn(face0: vec3, d: float, qa: vec2) -> vec3 {
            let glow = self.m_inner.w
            let gi = self.m_glow_ink.xyz
            let amt = min(glow * 1.6, 1.0)
            let w = 5.0 + 9.0 * glow
            let inner = 1.0 - (1.0 - exp(min(qa.x, 0.0) / w)) * (1.0 - exp(min(qa.y, 0.0) / w))
            let rim_l = 1.0 - smoothstep(0.0, 1.6, abs(d + 0.8))
            var face = mix(face0, face0 * 0.7 + gi * 0.22, 0.6 * amt)
            face = vec3(1.0, 1.0, 1.0) - (vec3(1.0, 1.0, 1.0) - face) * (vec3(1.0, 1.0, 1.0) - gi * inner * 0.75 * amt)
            face = vec3(1.0, 1.0, 1.0) - (vec3(1.0, 1.0, 1.0) - face) * (vec3(1.0, 1.0, 1.0) - mix(gi, vec3(1.0, 1.0, 1.0), 0.35) * rim_l * 0.75 * amt)
            return face
        }

        spun_of: fn(q: vec2, r: float) -> float {
            if r < 0.001 { return 0.0 }
            let rad = q / r
            let l2 = normalize(self.m_light.xy + vec2(0.000001, 0.000001))
            let c = dot(rad, l2)
            let fan = c * c
            let a = atan2(q.y, q.x)
            let grain = sin(a * 97.0) * 0.5 + sin(a * 233.0 + 1.7) * 0.3 + sin(a * 541.0 + 0.4) * 0.2
            return (fan * 2.0 - 1.0) * 0.6 + grain * 0.12 * (0.5 + 0.5 * fan)
        }

        // A well's outline: 1 a disc, 2 the value arc's track.
        flat_d: fn(p: vec2, h: vec2, isc: float) -> float {
            if isc > 1.5 {
                var a = 0.0
                if length(p) > 0.001 { a = atan2(p.x, -p.y) }
                let ca = clamp(a, -2.35619, 2.35619)
                return length(p - vec2(sin(ca), -cos(ca)) * h.x) - h.y
            }
            return length(p) - h.x
        }
    }

    // THE 2D KNOB. k_state: value, lit, edge fade (pt), unused; k_geom:
    // centre (pt), radius (pt), radius in bench units.
    set_type_default() do #(DrawTurnedKnob::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..KnobCore
        k_state: uniform(vec4(0.34, 0.0, 6.0, 0.0))
        k_geom: uniform(vec4(50.0, 50.0, 30.0, 56.0))

        pixel: fn() {
            let dpi = max(self.draw_pass.dpi_factor, 0.5)
            let geom = self.k_geom
            let unit = geom.w / max(geom.z, 0.001)
            let px = unit / dpi
            let R = geom.w
            let pp = self.pos * self.rect_size
            let p = (pp - geom.xy) * unit
            let edge = min(min(pp.x, pp.y), min(self.rect_size.x - pp.x, self.rect_size.y - pp.y))
            let qa = smoothstep(0.0, max(self.k_state.z, 0.001), edge)

            let spin = (self.k_state.x * 2.0 - 1.0) * 2.35619
            let dir = vec2(sin(spin), -cos(spin))
            let lit = self.k_state.y
            let l = self.m_light
            let level = self.m_tune.x
            let raise = self.m_relief.z
            let sink = self.m_tune.y
            let blur = max(self.m_shadow.y, 0.001)
            let tanel = max(l.z, 0.05) / max(length(l.xy), 0.05)
            let sdir = self.shadow_dir()
            let kk = R / 56.0
            let hk = R * max(self.m_knob.x, 0.001)
            let glow = self.m_inner.w
            let dist = length(p)

            var col = self.m_ground.xyz
            var touched = 0.0
            var turned_d = 1e9
            var spec_k = 1.0

            // THE LAYERS: the knob's well, the value arc's well, the knob.
            // Each ends in the face shading, so that runs at one call site.
            var layer = 0.0
            loop {
                if layer > 2.5 + self.knob_zero { break }
                var face_on = 0.0
                var fd = 1e9
                var fg = vec2(0.0, 1.0)
                var fuv = vec2(0.5, 0.5)
                var fdepth = 0.0
                var fconvex = 0.0
                var fbw = 0.001
                var fcurve = 0.0
                var fdome = 0.0
                var fturned = 1.0
                var fvis = 1.0
                var fbase = self.m_ground.xyz
                var faa = vec4(0.0, 0.0, 0.0, 0.0)
                var fsab = vec4(0.0, 1.0, 0.0, 1.0)
                var fenv = 0.0
                var fspec = 1.0
                var finsh = 0.0
                var capm = 0.0
                var rn = 0.0
                var kvis = 1.0
                if layer < 1.5 {
                    // A WELL (the bench's layerFlat for a sunk disc or arc).
                    var isc = 0.0
                    var lh = vec2(1.0, 1.0)
                    if layer < 0.5 {
                        if self.s_arc.w > 1.01 {
                            isc = 1.0
                            lh = vec2(R * self.s_arc.w, R * self.s_arc.w)
                        }
                    } else if self.s_arc.z > 0.01 && self.s_arc.y >= 0.01 {
                        isc = 2.0
                        lh = vec2(self.s_arc.x * R, self.s_arc.y * self.s_arc.z * 0.5)
                    }
                    if isc > 0.5 {
                        let depth = -sink * 0.6 * kk
                        var reach = 0.0
                        if level > 0.5 { reach = 4.0 * blur }
                        var ext_r = length(lh) + 3.0 * px
                        if isc > 1.5 { ext_r = lh.x + lh.y + 3.0 * px }
                        if dist <= ext_r + reach {
                            touched = 1.0
                            let d = self.flat_d(p, lh, isc)
                            let e = 0.5
                            var g = vec2(self.flat_d(p + vec2(e, 0.0), lh, isc) - self.flat_d(p - vec2(e, 0.0), lh, isc), self.flat_d(p + vec2(0.0, e), lh, isc) - self.flat_d(p - vec2(0.0, e), lh, isc))
                            if length(g) > 0.00001 {
                                g = normalize(g)
                            } else {
                                g = vec2(0.0, 1.0)
                            }
                            if level > 0.5 {
                                // A sunk face casts nothing and has no lip:
                                // only the contact ring round its edge.
                                let outside = smoothstep(-3.0 * px, 0.0, d)
                                let contact = exp(-max(d, 0.0) / (blur * 0.30)) * outside
                                col = self.light_face(col, 1.0, 1.0, 1.0 - clamp(contact * self.m_shadow.w, 0.0, 1.0))
                            }
                            if self.m_inner.x > 0.001 && depth < 0.0 {
                                let ioff = sdir * abs(depth) * 1.6
                                let din = self.flat_d(p - ioff, lh, isc)
                                finsh = 1.0 - smoothstep(0.0, max(self.m_inner.y, 0.001), -din)
                            }
                            if d < px {
                                face_on = 1.0
                                fd = d
                                fg = g
                                fuv = p / (2.0 * lh) + vec2(0.5, 0.5)
                                fdepth = depth
                                fconvex = depth
                                fbw = self.m_relief.x
                                fcurve = self.m_relief.y
                                fturned = 0.0
                                if isc > 1.5 { fturned = 1.0 }
                            }
                        }
                    }
                } else {
                    // THE KNOB (the bench's layerTurned).
                    let depth = raise * 1.2 * kk
                    let convex = raise * kk
                    let wing_h = self.s_wing2.x
                    var reach = 0.0
                    if level > 0.5 { reach = max(1.0, wing_h) * hk / tanel + 4.0 * blur }
                    if lit > 0.5 && glow > 0.001 { reach = max(reach, 4.0 * glow * 14.0) }
                    let wr = max(abs(self.s_wing.y), abs(self.s_wing.z))
                    let ext_r = R * 1.41421356 * (1.0 + wr) + 3.0 * px
                    var solid_r = R * max(1.0, wr)
                    if self.s_wing.x > 0.01 { solid_r = solid_r + self.s_wing.w * R * 0.5 }
                    if self.s_flute.x >= 1.0 { solid_r = solid_r + abs(self.s_flute.y) }
                    let off = max(depth, 0.0) / tanel
                    var cast_l = 0.0
                    if level > 0.5 { cast_l = max(1.0, wing_h) * hk / tanel }
                    var margin = 3.0 * px
                    if level > 0.5 { margin = margin + 4.0 * blur + off }
                    if lit > 0.5 && glow > 0.001 { margin = max(margin, 4.0 * glow * 14.0) }
                    let along_l = length(p - sdir * clamp(dot(p, sdir), 0.0, cast_l))
                    if dist <= ext_r + reach && along_l <= solid_r + margin {
                        touched = 1.0
                        let win = 1.0 - smoothstep(ext_r + 0.75 * reach, ext_r + reach, dist)
                        // The outline, and the lip's outline one offset
                        // toward the light: one call site.
                        var lite_d = 1e9
                        var d = 1e9
                        var disc_d = 0.0
                        var bar_half = R
                        var ks = 0.0
                        loop {
                            if ks > 1.5 + self.knob_zero { break }
                            if ks > 0.5 || (level > 0.5 && self.m_inner.z > 0.001) {
                                var pk = p
                                if ks < 0.5 { pk = p + sdir * off }
                                let s = self.knob_shape(pk, R, spin, dir, px)
                                if ks < 0.5 {
                                    lite_d = s.x
                                } else {
                                    d = s.x
                                    disc_d = s.y
                                    bar_half = s.z
                                }
                            }
                            ks = ks + 1.0
                        }
                        turned_d = d
                        spec_k = clamp(R / (px * 40.0), 0.15, 1.0)
                        var g = vec2(0.0, 1.0)
                        if dist > 0.001 { g = p / dist }
                        rn = clamp(1.0 + disc_d / max(R, 0.001), 0.0, 1.0)
                        let capr = self.s_cap.z
                        if capr > 0.001 { capm = 1.0 - smoothstep(capr - 0.015, capr + 0.015, rn) }
                        let base = mix(self.m_body.xyz, self.s_cap_ink.xyz, capm)
                        var gs = g
                        var pvy = 1.0
                        var w1 = 0.0
                        var hsum = 0.0
                        var hxp = 1.0
                        var hxm = 1.0
                        var hyp = 1.0
                        var hym = 1.0
                        var gux = 0.0
                        var guy = 0.0
                        let ldir = normalize(l.xy + vec2(0.000001, 0.000001))
                        var fon = 0.0
                        if d < px { fon = 1.0 }
                        let ntap = 5.0 * fon
                        var cast_v = 0.0
                        var occ = 0.0
                        var ns = 0.0
                        var tb = 0.0
                        var cast_on = 0.0
                        var fast = 0.0
                        var self_w = 0.0
                        let cast_d0 = max(d, 0.0)
                        if d > -3.0 * px {
                            cast_v = self.tail(cast_d0, blur * 0.3)
                            if cast_d0 < cast_l + 4.0 * blur { cast_on = 1.0 }
                        }
                        // THE SOLID'S ONE CALL SITE: five taps for the
                        // normal, then (where the revolve's own profile
                        // might shade this point) two samples toward the
                        // light for the self-shadow.
                        var ntot = ntap
                        var j = 0.0
                        loop {
                            if j >= ntot { break }
                            var qs = p
                            var t = 0.0
                            var shsoft = 0.0
                            if j < ntap {
                                if j > 3.5 {
                                    qs = p + vec2(0.0, -px)
                                } else if j > 2.5 {
                                    qs = p + vec2(0.0, px)
                                } else if j > 1.5 {
                                    qs = p + vec2(-px, 0.0)
                                } else if j > 0.5 {
                                    qs = p + vec2(px, 0.0)
                                }
                            } else {
                                let fi = (j - ntap + 0.5) / ns
                                t = tb * fi * sqrt(fi)
                                qs = p + ldir * t
                                shsoft = max(2.0 * px, tb / max(ns, 1.0) * 0.8)
                            }
                            var hj = 0.0
                            var wj = 0.0
                            var nearj = 1e9
                            if fast > 0.5 && j < ntap {
                                hj = self.rev_h(length(qs), R)
                            } else {
                                let sj = self.solid(qs, R, spin, dir, px, shsoft)
                                hj = sj.x
                                wj = sj.y
                                nearj = sj.z
                            }
                            if j < ntap {
                                if j > 0.5 { hsum = hsum + hj }
                                if j < 0.5 {
                                    pvy = hj
                                    w1 = wj
                                    if nearj > 2.5 * px { fast = 1.0 }
                                } else if j < 1.5 {
                                    gux = gux + hj
                                    hxp = hj
                                } else if j < 2.5 {
                                    gux = gux - hj
                                    hxm = hj
                                } else if j < 3.5 {
                                    guy = guy + hj
                                    hyp = hj
                                } else {
                                    guy = guy - hj
                                    hym = hj
                                }
                                if j > 3.5 && d <= 0.0 && fast < 0.5 {
                                    let hrev = self.rev_h(dist, R)
                                    if pvy < hrev - 0.02 {
                                        tb = (hrev - pvy) * hk / tanel
                                        ns = 2.0
                                        ntot = ntot + ns
                                    }
                                }
                            } else {
                                occ = occ + clamp(((hj * hk - pvy * hk) - t * tanel) / max(hk * 0.25, 0.001), 0.0, 1.0)
                            }
                            j = j + 1.0
                        }
                        let gu = vec2(gux, guy) / (2.0 * px)
                        let pd = self.m_knob.x
                        let dome = length(gu) * R * pd
                        var rgh_in = min(abs(hsum - 4.0 * pvy) / px * R * pd / (1.0 + dome * dome), 1.0)
                        let s_one = R * pd / px
                        let turn = max(abs(atan((hxp - pvy) * s_one) - atan((pvy - hxm) * s_one)), abs(atan((hyp - pvy) * s_one) - atan((pvy - hym) * s_one)))
                        rgh_in = min(max(rgh_in, turn), 1.0)
                        if length(gu) > 0.00001 { gs = -gu / length(gu) }
                        let ga = vec2(hxp - pvy, hyp - pvy) / px
                        let gb = vec2(pvy - hxm, pvy - hym) / px
                        var sa = gs
                        if length(ga) > 0.00001 { sa = -ga / length(ga) }
                        var sb = gs
                        if length(gb) > 0.00001 { sb = -gb / length(gb) }
                        let crease = smoothstep(0.35, 0.8, turn)
                        // THE SWEEPS: the cast shadow, and the wing's on the
                        // knob's top, one call site.
                        let sh = sdir * hk / tanel
                        var kw = 0.0
                        loop {
                            if kw > 1.5 + self.knob_zero { break }
                            var run = cast_on
                            var zp = 0.0
                            if kw > 0.5 {
                                run = 0.0
                                if d <= 0.0 && fon > 0.5 && self.s_sil.z > 0.5 && w1 <= 0.999 { run = 1.0 }
                                zp = pvy
                            }
                            if run > 0.5 {
                                let sv = self.sweep_dd(p, R, spin, dir, sh, zp, blur, kw, px)
                                if kw < 0.5 {
                                    let fade = 1.0 - smoothstep(solid_r + 0.6 * margin, solid_r + margin, along_l)
                                    cast_v = max(cast_v, mix(clamp(1.0 - sv / 3.0, 0.0, 1.0), exp(-sv), clamp(self.m_shadow.z, 0.0, 1.0)) * fade)
                                } else {
                                    self_w = 1.0 - smoothstep(-1.0, 1.0, sv)
                                }
                            }
                            kw = kw + 1.0
                        }
                        var selfsh = 0.0
                        if ns > 0.5 { selfsh = clamp(occ / (ns * 0.375), 0.0, 1.0) * step(d, 0.0) }
                        if d <= 0.0 && fon > 0.5 {
                            let st = self.knob_self.sample_lod(p / (2.0 * R) + vec2(0.5, 0.5), 0.0).x
                            selfsh = max(selfsh, max(st, self_w) * (1.0 - w1))
                        }
                        if level > 0.5 {
                            let rel = clamp(depth / max(raise, 0.001), 0.0, 1.0)
                            let outside = smoothstep(-3.0 * px, 0.0, d)
                            let facing = clamp(-dot(g, sdir), 0.0, 1.0)
                            let dark = cast_v * outside * win
                            let lite = self.tail(lite_d, blur) * outside * rel * smoothstep(0.0, 0.7, facing) * win
                            var enc = 0.0
                            if self.s_cut3.w > 0.5 && d > -3.0 * px && d < blur * 1.2 {
                                // A notched foot: how much of a ring round
                                // the point the foot covers, less what a
                                // straight edge would.
                                let rho = blur * 0.8
                                var ke = 0.0
                                loop {
                                    if ke > 7.5 + self.knob_zero { break }
                                    let ak = ke * 0.7853982
                                    let qk = p + vec2(cos(ak), sin(ak)) * rho
                                    let dk = max(length(qk) - R, self.foot_cut(qk, R, spin, dir))
                                    enc = enc + clamp(0.5 - dk / (2.0 * px), 0.0, 1.0)
                                    ke = ke + 1.0
                                }
                                let ex8 = 8.0 * acos(clamp(d / rho, -1.0, 1.0)) / 3.14159
                                enc = clamp((enc - ex8) / 1.5, 0.0, 1.0)
                            }
                            let contact = max(exp(-max(d, 0.0) / (blur * 0.30)), enc) * outside
                            col = self.light_face(col, 1.0, 1.0 - clamp(dark * self.m_shadow.x, 0.0, 1.0), 1.0 - clamp(contact * self.m_shadow.w, 0.0, 1.0))
                            col = self.in_light(col, clamp(lite * self.m_inner.z, 0.0, 1.0))
                        }
                        if lit > 0.5 && glow > 0.001 {
                            col = mix(col, self.m_glow_ink.xyz, clamp(self.tail(d, glow * 14.0) * glow, 0.0, 1.0) * 0.5 * win)
                        }
                        var ext = R
                        if self.s_wing.x > 0.01 { ext = max(R, wr * R) }
                        kvis = 1.0 - selfsh * self.m_shadow.x * 0.7
                        if fon > 0.5 {
                            face_on = 1.0
                            fd = d
                            fg = gs
                            fuv = p / (2.0 * ext) + vec2(0.5, 0.5)
                            fdepth = depth
                            fconvex = convex / max(raise * kk, 0.001)
                            fbw = 0.001
                            fcurve = 0.0
                            fdome = dome
                            fturned = 1.0
                            fvis = kvis
                            fbase = base
                            faa = vec4(rgh_in, crease, length(ga) * R * pd, length(gb) * R * pd)
                            fsab = vec4(sa.x, sa.y, sb.x, sb.y)
                            fspec = spec_k
                            fenv = 0.4 * self.s_cap.w
                            if capr > 0.001 { fenv = fenv * capm }
                        }
                    }
                }
                if face_on > 0.5 {
                    // THE FACE SHADING'S ONE CALL SITE.
                    var face = self.shade_face(fbase, fd, fg, fuv, fdepth, fconvex, fbw, fcurve, fdome, finsh, fturned, fvis, px, faa, fsab, fenv, fspec)
                    if layer > 1.5 {
                        let spun = self.s_cap.w
                        let capr = self.s_cap.z
                        if spun > 0.001 {
                            var smk = 1.0
                            if capr > 0.001 { smk = capm }
                            let sp = self.spun_of(p, dist) * spun * smk
                            face = self.in_light(face, clamp(sp, 0.0, 1.0) * 0.55 * kvis)
                            face = self.in_shadow(face, clamp(-sp, 0.0, 1.0) * 0.42 * kvis)
                        }
                        if capr > 0.001 {
                            let seam = 1.0 - smoothstep(0.0, 2.2 * px / max(R, 0.001), abs(rn - capr))
                            face = self.in_shadow(face, seam * 0.55)
                        }
                        if lit > 0.5 { face = self.inner_glow(face, fd, vec2(fd, -1000.0)) }
                    }
                    col = mix(col, face, 1.0 - smoothstep(-px, px, fd))
                }
                layer = layer + 1.0
            }

            // THE MARKS: the value arc, the ticks and the pointer.
            let ptype = self.s_ptr.x
            let ticks = self.s_ticks.x
            let arcw = self.s_arc.y
            var mr = 0.0
            if ticks >= 1.0 { mr = self.s_ticks.y + self.s_ticks.z }
            if ptype > 0.5 { mr = max(mr, self.s_ptr.z) }
            if arcw >= 0.01 { mr = max(mr, self.s_arc.x) }
            let mark_r = R * mr + arcw + max(max(self.s_ptr.w, self.s_ticks.w) * R / 56.0, 1.2) + 42.0 + 2.0 * px
            if dist <= mark_r {
                touched = 1.0
                let ad = self.arc_d(p, R, spin)
                let td = self.tick_d(p, R)
                var under = 1.0
                if ad < 2.0 * px || td.x < 40.0 { under = smoothstep(-px, px, turned_d) }
                col = mix(col, self.m_glow_ink.xyz, (1.0 - smoothstep(-px, px, ad)) * under)
                let pd = self.ptr_d(p, spin, R)
                var m = 0.0
                loop {
                    if m > 1.5 + self.knob_zero { break }
                    var md = td.x
                    var mw = max(self.s_ticks.w * R / 56.0, 1.0)
                    var ml = 0.0
                    if td.y <= spin + 0.001 { ml = 1.0 }
                    var mk = under
                    if m > 0.5 {
                        md = pd
                        mw = max(self.s_ptr.w * R / 56.0, 1.2)
                        ml = 1.0
                        mk = 1.0
                    }
                    if md < 40.0 {
                        let e3 = 0.5
                        var mg = vec2(self.mark_d(p + vec2(e3, 0.0), m, R, spin, td.y) - self.mark_d(p - vec2(e3, 0.0), m, R, spin, td.y), self.mark_d(p + vec2(0.0, e3), m, R, spin, td.y) - self.mark_d(p - vec2(0.0, e3), m, R, spin, td.y))
                        if length(mg) > 0.00001 {
                            mg = normalize(mg)
                        } else {
                            mg = vec2(0.0, 1.0)
                        }
                        col = mix(col, self.mark_ink(col, md, mg, mw, ml, self.m_ptr_ink.xyz, px, spec_k), mk)
                    }
                    m = m + 1.0
                }
            }
            // Opaque: the quad is the ground, what the knob does to it fading
            // out over the last points of the quad before the exposure, so
            // its edge is the page's own colour by the same arithmetic.
            if touched < 0.5 { return vec4(self.hdr_out(self.m_ground.xyz), 1.0) }
            return vec4(self.hdr_out(mix(self.m_ground.xyz, col, qa)), 1.0)
        }
    }

    // THE 3D VIEW (the bench's VIEW3D program): the same solid ray marched
    // under an orbiting camera. k_cam: yaw, elevation, zoom, and the
    // samples per pixel along each axis (1 is the bench's one ray; 2 is four
    // rays on a rotated grid, which is what takes the stairs off the
    // silhouettes and the rims).
    set_type_default() do #(DrawKnobView3d::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..KnobCore
        k_state: uniform(vec4(0.34, 0.0, 0.0, 0.0))
        k_cam: uniform(vec4(0.55, 0.62, 1.0, 1.0))

        // One ray through `pos` (0..1 over the view): premultiplied colour.
        view_ray: fn(pos: vec2, res: vec2) -> vec4 {
            let R = 56.0
            let spin = (self.k_state.x * 2.0 - 1.0) * 2.35619
            let dir = vec2(sin(spin), -cos(spin))
            let hk = R * max(self.m_knob.x, 0.001)
            let cam = self.k_cam
            let span = R * 1.75 / max(cam.z, 0.2)
            let uv = vec2((pos.x - 0.5) * 2.0 * res.x / res.y, (0.5 - pos.y) * 2.0)
            let px = 2.0 * span / res.y
            let yaw = cam.x
            let el = clamp(cam.y, 0.12, 1.55)
            let cdir = vec3(sin(yaw) * cos(el), cos(yaw) * cos(el), sin(el))
            let fwd = -cdir
            let right = vec3(cos(yaw), -sin(yaw), 0.0)
            let up = cross(fwd, right)
            let target = vec3(0.0, 0.0, hk * 0.35)
            let ro = target + right * (uv.x * span) + up * (uv.y * span) + cdir * (R * 6.0)
            let v3 = normalize(cdir - (right * uv.x + up * uv.y) * 0.35 * self.m_env.y)
            let rd = fwd
            let hmax = hk * max(1.0, self.s_wing2.x) * 1.1 + 1.0
            let rb = R * (1.0 + max(abs(self.s_wing.y), abs(self.s_wing.z))) * 1.05 + 2.0
            var col = self.m_ground.xyz
            var n = vec3(0.0, 0.0, 1.0)
            var hp = vec3(0.0, 0.0, 0.0)
            var hit_s = 0.0
            var hit_g = 0.0
            var t_gnd = 0.0
            var t_in = 0.0
            var t_out = 0.0
            let down = rd.z < -0.001
            if down {
                let t_top = (hmax - ro.z) / rd.z
                t_gnd = -ro.z / rd.z
                let a = dot(rd.xy, rd.xy)
                let b = dot(ro.xy, rd.xy)
                let cc = dot(ro.xy, ro.xy) - rb * rb
                let disc = b * b - a * cc
                t_in = t_gnd
                t_out = t_gnd
                if disc > 0.0 && a > 0.0001 {
                    let sq = sqrt(disc)
                    t_in = max(t_top, (-b - sq) / a)
                    t_out = min(t_gnd, (-b + sq) / a)
                }
            }
            // THE SOLID'S ONE CALL SITE: 64 steps through the knob's
            // bounds, five of bisection on a crossing, then the five taps
            // of the normal at the hit.
            var mode = 2.0
            if down && t_in < t_out { mode = 0.0 }
            var i = 0.0
            var t_prev = t_in
            var ta = 0.0
            var tb = 0.0
            var bis = 0.0
            var kb = 0.0
            var tap = 0.0
            var hx = 0.0
            var hy = 0.0
            var z3 = 0.0
            var w3 = 0.0
            let e = max(px, 0.35)
            loop {
                if mode > 1.5 { break }
                var xy = hp.xy
                var t = 0.0
                if mode < 0.5 {
                    t = t_in + (t_out - t_in) * (i + 0.5) / 64.0
                    if bis > 0.5 { t = 0.5 * (ta + tb) }
                    xy = ro.xy + rd.xy * t
                } else if tap > 3.5 {
                    xy = hp.xy + vec2(0.0, -e)
                } else if tap > 2.5 {
                    xy = hp.xy + vec2(0.0, e)
                } else if tap > 1.5 {
                    xy = hp.xy + vec2(-e, 0.0)
                } else if tap > 0.5 {
                    xy = hp.xy + vec2(e, 0.0)
                }
                let s = self.solid(xy, R, spin, dir, px, 0.0)
                if mode < 0.5 {
                    var under = 0.0
                    if ro.z + rd.z * t - max(s.x * hk, 0.0) < 0.0 { under = 1.0 }
                    if bis > 0.5 {
                        if under > 0.5 {
                            tb = t
                        } else {
                            ta = t
                        }
                        kb = kb + 1.0
                        if kb > 4.5 {
                            hp = ro + rd * (0.5 * (ta + tb))
                            hit_s = 1.0
                            mode = 1.0
                        }
                    } else if under > 0.5 {
                        ta = t_prev
                        tb = t
                        bis = 1.0
                    } else {
                        t_prev = t
                        if i > 62.5 { mode = 2.0 }
                    }
                    i = i + 1.0
                } else {
                    if tap < 0.5 {
                        z3 = s.x
                        w3 = s.y
                    } else {
                        let hj = max(s.x * hk, 0.0)
                        if tap < 1.5 {
                            hx = hx + hj
                        } else if tap < 2.5 {
                            hx = hx - hj
                        } else if tap < 3.5 {
                            hy = hy + hj
                        } else {
                            hy = hy - hj
                        }
                    }
                    tap = tap + 1.0
                    if tap > 4.5 { mode = 2.0 }
                }
            }
            if down && hit_s < 0.5 {
                hp = ro + rd * t_gnd
                hit_g = 1.0
            }
            if hit_s < 0.5 && hit_g < 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
            if hit_s > 0.5 { n = normalize(vec3(-hx, -hy, 2.0 * e)) }
            let l3 = normalize(self.m_light.xyz)
            var sh = 0.0
            if l3.z > 0.02 {
                let tanel3 = max(self.m_light.z, 0.05) / max(length(self.m_light.xy), 0.05)
                let sh3 = self.shadow_dir() * hk / tanel3
                var swd = 1e9
                var zp = 0.0
                var smode = 0.0
                if hit_s > 0.5 {
                    zp = z3
                    smode = 1.0
                }
                if hit_g > 0.5 || self.s_sil.z > 0.5 { swd = self.sweep_dd(hp.xy, R, spin, dir, sh3, zp, 1.0, smode, px) }
                if hit_s > 0.5 {
                    var sw3 = 0.0
                    if self.s_sil.z > 0.5 { sw3 = 1.0 - smoothstep(-1.0, 1.0, swd) }
                    sh = max(self.knob_self.sample_lod(hp.xy / (2.0 * R) + vec2(0.5, 0.5), 0.0).y, sw3) * (1.0 - w3)
                    sh = max(sh, 1.0 - smoothstep(-0.1, 0.1, dot(n, l3)))
                } else {
                    sh = mix(clamp(1.0 - swd / 3.0, 0.0, 1.0), exp(-swd), clamp(self.m_shadow.z, 0.0, 1.0))
                }
            }
            if hit_g > 0.5 && sh < 0.004 { return vec4(0.0, 0.0, 0.0, 0.0) }
            var capm3 = 0.0
            let capr = self.s_cap.z
            if hit_s > 0.5 && capr > 0.001 { capm3 = 1.0 - smoothstep(capr - 0.015, capr + 0.015, length(hp.xy) / R) }
            var base = self.m_ground.xyz
            if hit_s > 0.5 { base = mix(self.m_body.xyz, self.s_cap_ink.xyz, capm3) }
            let lt = self.lit_of_v(n, vec2(self.m_relief.w * 0.7, clamp(self.m_finish.w, 0.0, 1.0)), v3, 0.0)
            let flatv = self.lam(l3.z) * self.m_light.w
            let key = (lt.x - flatv) * 1.5
            let vis3 = 1.0 - sh * self.m_shadow.x * 0.7
            let m3 = hit_s * clamp(self.m_surf.x, 0.0, 1.0)
            col = self.light_face(base, mix(1.0 + smoothstep(0.0, 1.0, key) - smoothstep(0.0, 1.0, -key), 1.0, m3), mix(vis3, 1.0, m3), 1.0)
            let spun = self.s_cap.w
            if hit_s > 0.5 {
                var env_rgh = 0.4 * spun
                if capr > 0.001 { env_rgh = env_rgh * capm3 }
                col = self.env_refl(col, n, v3, vis3, env_rgh, 0.0)
                if spun > 0.001 {
                    var sm3 = 1.0
                    if capr > 0.001 { sm3 = capm3 }
                    let r3 = length(hp.xy)
                    var tg = vec3(1.0, 0.0, 0.0)
                    if r3 > 0.001 { tg = vec3(-hp.y, hp.x, 0.0) / r3 }
                    tg = normalize(tg - n * dot(tg, n))
                    let tl = dot(tg, l3)
                    let tv = dot(tg, cdir)
                    let kq = sqrt(max(1.0 - tl * tl, 0.0)) * sqrt(max(1.0 - tv * tv, 0.0)) - tl * tv
                    let fan3 = pow(max(kq, 0.0), mix(16.0, 4.0, clamp(self.m_finish.w, 0.0, 1.0)))
                    let a3 = atan2(hp.y, hp.x)
                    let grain3 = sin(a3 * 97.0) * 0.5 + sin(a3 * 233.0 + 1.7) * 0.3 + sin(a3 * 541.0 + 0.4) * 0.2
                    let sp3 = ((fan3 * 2.0 - 1.0) * 0.6 + grain3 * 0.12 * (0.5 + 0.5 * fan3)) * spun * sm3
                    col = self.in_light(col, clamp(sp3, 0.0, 1.0) * 0.55 * vis3)
                    col = self.in_shadow(col, clamp(-sp3, 0.0, 1.0) * 0.42 * vis3)
                }
                col = col + mix(vec3(1.0, 1.0, 1.0), base * 1.6, m3) * lt.y * vis3
                if self.m_surf.y > 0.001 {
                    let cs = self.lit_of_v(n, vec2(self.m_surf.y * 0.6, self.m_surf.z), v3, 0.0).y * vis3
                    col = col + vec3(cs, cs, cs)
                }
                let pdd = self.ptr_d(hp.xy, spin, R)
                col = mix(col, self.m_ptr_ink.xyz, 1.0 - smoothstep(-px, px, pdd))
            }
            return vec4(self.hdr_out(col), 1.0)
        }

        // The rays of one pixel, the ray's work at one call site.
        pixel: fn() {
            let dpi = max(self.draw_pass.dpi_factor, 0.5)
            let res = self.rect_size * dpi
            let ns = clamp(floor(self.k_cam.w + 0.5), 1.0, 2.0)
            let nsub = ns * ns
            var acc = vec4(0.0, 0.0, 0.0, 0.0)
            var k = 0.0
            loop {
                if k >= nsub { break }
                var off = vec2(0.0, 0.0)
                if nsub > 1.5 {
                    if k < 0.5 {
                        off = vec2(0.125, 0.375)
                    } else if k < 1.5 {
                        off = vec2(0.375, -0.125)
                    } else if k < 2.5 {
                        off = vec2(-0.125, -0.375)
                    } else {
                        off = vec2(-0.375, 0.125)
                    }
                }
                acc = acc + self.view_ray(self.pos + off / res, res)
                k = k + 1.0
            }
            return acc / nsub
        }
    }
}

/// The 2D knob's shader. Every value is a DSL uniform, set per draw.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTurnedKnob {
    #[deref]
    pub draw_super: DrawQuad,
}

/// The 3D view's shader.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKnobView3d {
    #[deref]
    pub draw_super: DrawQuad,
}
