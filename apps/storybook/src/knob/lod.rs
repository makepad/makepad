//! The knob engine's detail levels: draw shaders derived from
//! `DrawTurnedKnob` that leave features out of the shader's text, so the
//! Knob cost page can show what each one looks like, costs per frame and
//! takes to compile.
//!
//! A version never branches around a feature: it replaces the functions
//! that carry it. The Direct3D path compiles with FXC's optimiser off
//! (`D3DCOMPILE_SKIP_OPTIMIZATION`), so a branch the uniforms never take is
//! still compiled, and only code that is not there costs nothing.
//!
//! * **Full** is `DrawTurnedKnob` as it is, with only `shadow_dir` swapped
//!   for a copy that carries the launch's salt (below). It draws the same
//!   pixels.
//! * **Bare** and the **Bare + one feature** versions share one pixel
//!   function, [`LodBody`]'s: the knob's own `pixel` with each feature's
//!   code moved behind a hook (`lod_*`). Bare's hooks are trivial; a
//!   feature's version puts that feature's hooks back. `solid` and
//!   `knob_shape` are replaced the same way: the revolve and the grip stay,
//!   the flat, the cut and the wing are hooks.
//! * **Preview** has its own small pixel function: a disc shaded from the
//!   revolve profile's slope (no taps of the solid), a drop shadow, the
//!   marks in paint.
//!
//! One feature at a time means what the features cost together is Full's
//! alone: a cut notching the cast shadow's foot, the wing's shadow on the
//! ground and on the knob's own top, and the contact ring round a notched
//! foot come only with Full.
//!
//! # The salt
//!
//! Every version's text carries a number drawn once per launch, in a branch
//! no pixel takes (`shadow_dir`: a length is never below zero). The text is
//! what every shader cache is keyed on -- Makepad's DXBC cache on Windows,
//! its GL program cache on Linux, the driver's own -- so no cache has seen
//! this launch's text and every compile the page times is a cold one. The
//! comparison reads a uniform, so no compiler can fold it away.
use crate::makepad_widgets::*;
use std::sync::OnceLock;

/// This launch's salt: in 1..2, drawn from the clock once per process.
pub fn launch_salt() -> f64 {
    static SALT: OnceLock<f64> = OnceLock::new();
    *SALT.get_or_init(|| {
        let micros = (Cx::time_now() * 1.0e6) as u64;
        let mixed = micros ^ (micros >> 20) ^ (micros >> 40);
        1.0 + (mixed % (1 << 20)) as f64 / (1u64 << 20) as f64
    })
}

/// Whether the versions print their generated shader text when they compile
/// (`debug_code`): `MAKEPAD_KNOB_COST_DUMP=1`. On Linux the backend prints
/// the GLSL, on Windows the HLSL.
fn dump_shader_text() -> bool {
    matches!(std::env::var("MAKEPAD_KNOB_COST_DUMP").as_deref(), Ok("1") | Ok("true"))
}

/// One version: its key in the page's `versions` template object, its name,
/// what it leaves out and the functions it replaces.
pub struct KnobVersion {
    pub key: &'static str,
    pub name: &'static str,
    pub leaves_out: &'static str,
    pub replaces: &'static str,
}

pub const VERSIONS: &[KnobVersion] = &[
    KnobVersion {
        key: "preview",
        name: "Preview",
        leaves_out: "the solid: no height taps, no grip, cuts or wing; a disc shaded from the revolve profile's \
                     slope, a drop shadow, marks in paint, no wells",
        replaces: "pixel (its own), shadow_dir (salt)",
    },
    KnobVersion {
        key: "bare",
        name: "Bare",
        leaves_out: "cuts and the flat, the wing, the self-shadow, the cast shadow and the ground lip, the mark \
                     finishes",
        replaces: "pixel (the hooked copy), solid, knob_shape, shadow_dir (salt); every lod_* hook trivial",
    },
    KnobVersion {
        key: "cuts",
        name: "Bare + cuts",
        leaves_out: "as Bare, with the cut (dimple, slot, scallops, ring) and the flat back",
        replaces: "as Bare; lod_solid_flat, lod_solid_cut, lod_shape_flat, lod_shape_cut restored",
    },
    KnobVersion {
        key: "wings",
        name: "Bare + wings",
        leaves_out: "as Bare, with the wing (ridge or cutters) back, its shadows still out",
        replaces: "as Bare; lod_wing_reach, lod_solid_wing, lod_shape_wing restored",
    },
    KnobVersion {
        key: "self_shadow",
        name: "Bare + self-shadow",
        leaves_out: "as Bare, with the self-shadow back: two taps toward the light and the baked table",
        replaces: "as Bare; lod_ss_extend, lod_ss_top restored",
    },
    KnobVersion {
        key: "cast_shadow",
        name: "Bare + cast shadow",
        leaves_out: "as Bare, with the cast shadow back: the revolve's analytic sweep and the ground lip",
        replaces: "as Bare; lod_cast (with lod_sweep) and lod_outline (the lip's second outline) restored",
    },
    KnobVersion {
        key: "marks",
        name: "Bare + mark finishes",
        leaves_out: "as Bare, with the marks' finishes back: engraved, embossed, LED, and their gradients",
        replaces: "as Bare; lod_mark_grad, lod_mark_ink (mark_ink) restored",
    },
    KnobVersion {
        key: "full",
        name: "Full",
        leaves_out: "nothing: today's TurnedKnob",
        replaces: "shadow_dir (salt) only",
    },
];

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // THE SALT: `shadow_dir` as the knob has it, and a branch no pixel
    // takes that carries this launch's number into the shader's text.
    let KnobSalt = {
        debug_code: #(dump_shader_text())
        shadow_dir: fn() -> vec2 {
            let ll = length(self.m_light.xy)
            if ll > 0.0001 { return -self.m_light.xy / ll }
            if ll < #(-launch_salt()) { return vec2(#(launch_salt()), 0.0) }
            return vec2(0.0, 1.0)
        }
    }

    // THE PREVIEW: a disc shaded from the revolve profile's slope, a drop
    // shadow and the marks in paint. No taps of the solid, no wells.
    let KnobPreview = {
        // The face: diffuse and the highlight from the normal, the studio's
        // reflection (`env_refl`), the coat's highlight.
        preview_face: fn(base: vec3, n: vec3, v: vec3, env_rgh: float, spec_scale: float) -> vec3 {
            if self.m_tune.x < 0.5 { return base }
            let l = self.m_light
            let lt = self.lit_of_v(n, vec2(self.m_relief.w * spec_scale, clamp(self.m_finish.w, 0.0, 1.0)), v, 0.0)
            let flatv = self.lam(normalize(l.xyz).z) * l.w
            let key = (lt.x - flatv) * 1.5
            let dl = 1.0 + smoothstep(0.0, 1.0, key) - smoothstep(0.0, 1.0, -key)
            let mtl = clamp(self.m_surf.x, 0.0, 1.0)
            var o = self.light_face(base, mix(dl, 1.0, mtl), 1.0, 1.0)
            o = self.env_refl(o, n, v, 1.0, env_rgh, 0.0)
            // The key's highlight through the glazing, as the knob has it.
            o = o + mix(vec3(1.0, 1.0, 1.0), base * 1.6, mtl) * lt.y * self.key_glaze(n, v)
            if self.m_surf.y > 0.001 {
                let cs = self.lit_of_v(n, vec2(self.m_surf.y * 0.6, self.m_surf.z), v, 0.0).y
                o = o + vec3(cs, cs, cs)
            }
            return o
        }

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
            let l = self.m_light
            let level = self.m_tune.x
            let blur = max(self.m_shadow.y, 0.001)
            let tanel = max(l.z, 0.05) / max(length(l.xy), 0.05)
            let sdir = self.shadow_dir()
            let hk = R * max(self.m_knob.x, 0.001)
            let dist = length(p)
            let d = dist - R
            var col = self.m_ground.xyz
            var cover = 0.0
            if level > 0.5 {
                // The disc swept along the light to the knob's height and
                // narrowing as it rises (a cone, where the sweep takes the
                // real outline), softened as the sweep softens with height.
                let cast_l = hk / tanel
                let t = clamp(dot(p, sdir) / max(cast_l, 0.001), 0.0, 1.0)
                let ds = length(p - sdir * (t * cast_l)) - R * mix(1.0, 0.6, t)
                let bl = max(max(0.5 * blur * mix(0.3, 1.0, t), t * 0.11 * cast_l), 0.001)
                let sv = max(ds + 0.3 * bl, 0.0) / bl
                let outside = smoothstep(-3.0 * px, 0.0, d)
                let dark = mix(clamp(1.0 - sv / 3.0, 0.0, 1.0), exp(-sv), clamp(self.m_shadow.z, 0.0, 1.0)) * outside
                let contact = exp(-max(d, 0.0) / (blur * 0.30)) * outside
                col = self.light_face(col, 1.0, 1.0 - clamp(dark * self.m_shadow.x, 0.0, 1.0), 1.0 - clamp(contact * self.m_shadow.w, 0.0, 1.0))
            }
            let spec_k = clamp(R / (px * 40.0), 0.15, 1.0)
            if d < px {
                let rn = clamp(dist / max(R, 0.001), 0.0, 1.0)
                let prf = self.kd_row(0.0, rn)
                // The profile's slope per radius, as the bake codes it.
                let dhdr = -(prf.x * 2.0 - 1.0) * 8.0
                var g = vec2(0.0, 1.0)
                if dist > 0.001 { g = p / dist }
                if dhdr > 0.0 { g = -g }
                let dome = abs(dhdr) * max(self.m_knob.x, 0.001)
                let n = normalize(vec3(g.x * dome, g.y * dome, 1.0))
                let capr = self.s_cap.z
                var capm = 0.0
                if capr > 0.001 { capm = 1.0 - smoothstep(capr - 0.015, capr + 0.015, rn) }
                let base = mix(self.m_body.xyz, self.s_cap_ink.xyz, capm)
                let uv = p / (2.0 * R) + vec2(0.5, 0.5)
                let v = normalize(vec3(-(uv - vec2(0.5, 0.5)) * 2.0 * self.m_env.y, 1.0))
                var env_rgh = 0.4 * self.s_cap.w
                if capr > 0.001 { env_rgh = env_rgh * capm }
                var face = self.preview_face(base, n, v, env_rgh, spec_k)
                if capr > 0.001 {
                    let seam = 1.0 - smoothstep(0.0, 2.2 * px / max(R, 0.001), abs(rn - capr))
                    face = self.in_shadow(face, seam * 0.55)
                }
                cover = 1.0 - smoothstep(-px, px, d)
                col = mix(col, face, cover)
            }
            // The marks, in paint.
            let ptype = self.s_ptr.x
            let ticks = self.s_ticks.x
            let arcw = self.s_arc.y
            var mr = 0.0
            if ticks >= 1.0 { mr = self.s_ticks.y + self.s_ticks.z }
            if ptype > 0.5 { mr = max(mr, self.s_ptr.z) }
            if arcw >= 0.01 { mr = max(mr, self.s_arc.x) }
            let mark_r = R * mr + arcw + max(max(self.s_ptr.w, self.s_ticks.w) * R / 56.0, 1.2) + 2.0 * px
            if dist <= mark_r {
                let ad = self.arc_d(p, R, spin)
                let td = self.tick_d(p, R)
                var under = 1.0
                if ad < 2.0 * px || td.x < 40.0 { under = smoothstep(-px, px, d) }
                let acov = (1.0 - smoothstep(-px, px, ad)) * under
                let tcov = (1.0 - smoothstep(-px, px, td.x)) * under
                let pcov = 1.0 - smoothstep(-px, px, self.ptr_d(p, spin, R))
                col = mix(col, self.m_glow_ink.xyz, acov)
                col = mix(col, self.m_ptr_ink.xyz, tcov)
                col = mix(col, self.m_ptr_ink.xyz, pcov)
                cover = max(cover, max(acov, max(tcov, pcov)))
            }
            return self.knob_out(self.hdr_out(mix(self.m_ground.xyz, col, qa)), cover * qa)
        }
    }

    // BARE'S HOOKS: every feature a version may put back, trivial.
    let LodHooks = {
        // The wing's reach for the knob's bounds: its span in radii, its
        // half width, its crest height.
        lod_wing_reach: fn(R: float) -> vec3 {
            return vec3(0.0, 0.0, 0.0)
        }
        // The solid's flat, wing and cut: the height and the distance to
        // the feature (the wing's also its weight).
        lod_solid_flat: fn(q: vec2, R: float, dir: vec2, h: float, hk: float) -> vec2 {
            return vec2(h, 1e9)
        }
        lod_solid_wing: fn(q: vec2, R: float, dir: vec2, rr: float, prf: vec2, h: float, sl_r: float, hk: float, shsoft: float) -> vec3 {
            return vec3(h, 0.0, 1e9)
        }
        lod_solid_cut: fn(q: vec2, R: float, spin: float, dir: vec2, h: float, hk: float) -> vec2 {
            return vec2(h, 1e9)
        }
        // The outline's flat, wing (and its half width) and cut.
        lod_shape_flat: fn(q: vec2, R: float, dir: vec2, cd: float) -> float {
            return cd
        }
        lod_shape_wing: fn(q: vec2, R: float, dir: vec2, cd: float) -> vec2 {
            return vec2(cd, R)
        }
        lod_shape_cut: fn(q: vec2, R: float, spin: float, dir: vec2, cd: float, rq: float) -> float {
            return cd
        }
        // The outline: distance, the disc's distance, the wing's half width,
        // and the lip's outline one offset toward the light (none here).
        lod_outline: fn(p: vec2, R: float, spin: float, dir: vec2, px: float, lip: vec2, level: float) -> vec4 {
            let s = self.knob_shape(p, R, spin, dir, px)
            return vec4(s.x, s.y, s.z, 1e9)
        }
        // The cast shadow's darkness here.
        lod_cast: fn(p: vec2, R: float, sh: vec2, blur: float, px: float, d: float, cast_l: float, solid_r: float, margin: float, along_l: float) -> float {
            return 0.0
        }
        // The self-shadow: whether (and how far) to tap toward the light,
        // and the shadow on the top.
        lod_ss_extend: fn(dist: float, R: float, pvy: float, hk: float, tanel: float) -> vec2 {
            return vec2(0.0, 0.0)
        }
        lod_ss_top: fn(p: vec2, R: float, d: float, fon: float, w1: float, occ: float, ns: float) -> float {
            return 0.0
        }
        // The marks: their gradient and their ink, here in paint.
        lod_mark_grad: fn(p: vec2, m: float, R: float, spin: float, ta: float) -> vec2 {
            return vec2(0.0, 1.0)
        }
        lod_mark_ink: fn(col: vec3, d: float, gm: vec2, wdt: float, lit: float, inkc: vec3, px: float, spec_scale: float) -> vec3 {
            return mix(col, inkc, 1.0 - smoothstep(-px, px, d))
        }
    }

    // CUTS: the flat and the cut, as `solid` and `knob_shape` have them.
    let LodCuts = {
        lod_solid_flat: fn(q: vec2, R: float, dir: vec2, h0: float, hk: float) -> vec2 {
            var h = h0
            var nearf = 1e9
            let flatc = self.s_wing2.w
            if flatc > 0.001 && flatc < 0.999 {
                let df = flatc * R - dot(q, dir)
                nearf = df - 0.05 * R
                let wall = 1.0 / max(0.02 * R, 0.5)
                h = self.cut_min(h, max(df, 0.0) * wall, self.blend_band(0.5, wall, hk))
            }
            return vec2(h, nearf)
        }
        lod_solid_cut: fn(q: vec2, R: float, spin: float, dir: vec2, h0: float, hk: float) -> vec2 {
            var h = h0
            var nearf = 1e9
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
            return vec2(h, nearf)
        }
        lod_shape_flat: fn(q: vec2, R: float, dir: vec2, cd0: float) -> float {
            var cd = cd0
            let flatc = self.s_wing2.w
            if flatc > 0.001 && flatc < 0.999 { cd = max(cd, dot(q, dir) - flatc * R) }
            return cd
        }
        lod_shape_cut: fn(q: vec2, R: float, spin: float, dir: vec2, cd0: float, rq: float) -> float {
            var cd = cd0
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
            return cd
        }
    }

    // WINGS: the ridge or the cutters, as `solid` and `knob_shape` have them.
    let LodWings = {
        lod_wing_reach: fn(R: float) -> vec3 {
            var half = 0.0
            if self.s_wing.x > 0.01 { half = self.s_wing.w * R * 0.5 }
            return vec3(max(abs(self.s_wing.y), abs(self.s_wing.z)), half, self.s_wing2.x)
        }
        lod_solid_wing: fn(q: vec2, R: float, dir: vec2, rr: float, prf: vec2, h0: float, sl_r: float, hk: float, shsoft: float) -> vec3 {
            var h = h0
            var wgt = 0.0
            var nearf = 1e9
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
            return vec3(h, wgt, nearf)
        }
        lod_shape_wing: fn(q: vec2, R: float, dir: vec2, cd0: float) -> vec2 {
            var cd = cd0
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
            return vec2(cd, bar_half)
        }
    }

    // SELF-SHADOW: two taps of the solid toward the light where the
    // revolve's own profile may shade the point, and the baked table.
    let LodSelfShadow = {
        lod_ss_extend: fn(dist: float, R: float, pvy: float, hk: float, tanel: float) -> vec2 {
            let hrev = self.rev_h(dist, R)
            if pvy < hrev - 0.02 { return vec2((hrev - pvy) * hk / tanel, 2.0) }
            return vec2(0.0, 0.0)
        }
        lod_ss_top: fn(p: vec2, R: float, d: float, fon: float, w1: float, occ: float, ns: float) -> float {
            var selfsh = 0.0
            if ns > 0.5 { selfsh = clamp(occ / (ns * 0.375), 0.0, 1.0) * step(d, 0.0) }
            if d <= 0.0 && fon > 0.5 {
                let st = self.knob_self.sample_lod(p / (2.0 * R) + vec2(0.5, 0.5), 0.0).x
                selfsh = max(selfsh, st * (1.0 - w1))
            }
            return selfsh
        }
    }

    // CAST SHADOW: the revolve's outline swept along the light (the
    // sweep's first half, without the wing's knots or a notched foot), and
    // the ground lip, the outline once more one offset toward the light.
    let LodCast = {
        lod_outline: fn(p: vec2, R: float, spin: float, dir: vec2, px: float, lip: vec2, level: float) -> vec4 {
            var lite_d = 1e9
            var o = vec3(1e9, 0.0, R)
            var ks = 0.0
            loop {
                if ks > 1.5 + self.knob_zero { break }
                if ks > 0.5 || (level > 0.5 && self.m_inner.z > 0.001) {
                    var pk = p
                    if ks < 0.5 { pk = p + lip }
                    let s = self.knob_shape(pk, R, spin, dir, px)
                    if ks < 0.5 {
                        lite_d = s.x
                    } else {
                        o = s
                    }
                }
                ks = ks + 1.0
            }
            return vec4(o.x, o.y, o.z, lite_d)
        }
        lod_sweep: fn(q: vec2, R: float, sh: vec2, blur: float) -> float {
            let thr = 0.11 * length(sh)
            var best = 1e9
            let sdv = self.shadow_dir()
            let qn = q / R
            let ya = dot(qn, sdv)
            let xa = abs(sdv.x * qn.y - sdv.y * qn.x)
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
                best = min(best, self.sweep_n(d * R, z, 0.0, blur, thr, 0.0, 0.0))
                k0 = k1
                i = i + 1.0
            }
            return best
        }
        lod_cast: fn(p: vec2, R: float, sh: vec2, blur: float, px: float, d: float, cast_l: float, solid_r: float, margin: float, along_l: float) -> float {
            var cast_v = 0.0
            let cast_d0 = max(d, 0.0)
            if d > -3.0 * px {
                cast_v = self.tail(cast_d0, blur * 0.3)
                if cast_d0 < cast_l + 4.0 * blur {
                    let sv = self.lod_sweep(p, R, sh, blur)
                    let fade = 1.0 - smoothstep(solid_r + 0.6 * margin, solid_r + margin, along_l)
                    cast_v = max(cast_v, mix(clamp(1.0 - sv / 3.0, 0.0, 1.0), exp(-sv), clamp(self.m_shadow.z, 0.0, 1.0)) * fade)
                }
            }
            return cast_v
        }
    }

    // MARK FINISHES: the marks' gradients and `mark_ink` itself.
    let LodMarks = {
        lod_mark_grad: fn(p: vec2, m: float, R: float, spin: float, ta: float) -> vec2 {
            let e3 = 0.5
            var mg = vec2(self.mark_d(p + vec2(e3, 0.0), m, R, spin, ta) - self.mark_d(p - vec2(e3, 0.0), m, R, spin, ta), self.mark_d(p + vec2(0.0, e3), m, R, spin, ta) - self.mark_d(p - vec2(0.0, e3), m, R, spin, ta))
            if length(mg) > 0.00001 {
                mg = normalize(mg)
            } else {
                mg = vec2(0.0, 1.0)
            }
            return mg
        }
        lod_mark_ink: fn(col: vec3, d: float, gm: vec2, wdt: float, lit: float, inkc: vec3, px: float, spec_scale: float) -> vec3 {
            return self.mark_ink(col, d, gm, wdt, lit, inkc, px, spec_scale)
        }
    }

    // THE HOOKED KNOB: the solid and the outline with their features behind
    // hooks, and the knob's pixel with each feature's code behind a hook.
    let LodBody = {
        ..KnobSalt,
        ..LodHooks,

        // The revolve and the grip; the flat, the wing and the cut hooked,
        // in the order `solid` takes them.
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
            let hk = R * max(self.m_knob.x, 0.001)
            let fl = self.lod_solid_flat(q, R, dir, h, hk)
            h = fl.x
            nearf = min(nearf, fl.y)
            let wg = self.lod_solid_wing(q, R, dir, rr, prf, h, sl_r, hk, shsoft)
            h = wg.x
            nearf = min(nearf, wg.z)
            let ct = self.lod_solid_cut(q, R, spin, dir, h, hk)
            h = ct.x
            nearf = min(nearf, ct.y)
            return vec3(h, wg.y, nearf)
        }

        // The disc and the grip; the flat, the wing and the cut hooked.
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
            let cd0 = self.lod_shape_flat(q, R, dir, rq - rad)
            let wg = self.lod_shape_wing(q, R, dir, cd0)
            let cd = self.lod_shape_cut(q, R, spin, dir, wg.x, rq)
            return vec3(cd, cd0, wg.y)
        }

        // THE 2D KNOB, hooked: the knob's `pixel` with the lip, the cast
        // shadow, the self-shadow, the wing's reach and the marks' finishes
        // behind the hooks above. The wells and the face shading are the
        // knob's own.
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
            var cover = 0.0
            var turned_d = 1e9
            var spec_k = 1.0

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
                    // A WELL, as the knob has it.
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
                    // THE KNOB, its features behind the hooks.
                    let depth = raise * 1.2 * kk
                    let convex = raise * kk
                    let wreach = self.lod_wing_reach(R)
                    let wing_h = wreach.z
                    var reach = 0.0
                    if level > 0.5 { reach = max(1.0, wing_h) * hk / tanel + 4.0 * blur }
                    if lit > 0.5 && glow > 0.001 { reach = max(reach, 4.0 * glow * 14.0) }
                    let wr = wreach.x
                    let ext_r = R * 1.41421356 * (1.0 + wr) + 3.0 * px
                    var solid_r = R * max(1.0, wr) + wreach.y
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
                        // THE OUTLINE'S ONE CALL SITE (and the lip's).
                        let ol = self.lod_outline(p, R, spin, dir, px, sdir * off, level)
                        let d = ol.x
                        let disc_d = ol.y
                        let lite_d = ol.w
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
                        var occ = 0.0
                        var ns = 0.0
                        var tb = 0.0
                        var fast = 0.0
                        // THE SOLID'S ONE CALL SITE: five taps for the
                        // normal, and (with the self-shadow) two toward the
                        // light.
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
                                    let ext2 = self.lod_ss_extend(dist, R, pvy, hk, tanel)
                                    if ext2.y > 0.5 {
                                        tb = ext2.x
                                        ns = ext2.y
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
                        // THE CAST SHADOW and THE SELF-SHADOW.
                        let cast_v = self.lod_cast(p, R, sdir * hk / tanel, blur, px, d, cast_l, solid_r, margin, along_l)
                        let selfsh = self.lod_ss_top(p, R, d, fon, w1, occ, ns)
                        if level > 0.5 {
                            let rel = clamp(depth / max(raise, 0.001), 0.0, 1.0)
                            let outside = smoothstep(-3.0 * px, 0.0, d)
                            let facing = clamp(-dot(g, sdir), 0.0, 1.0)
                            let dark = cast_v * outside * win
                            let lite = self.tail(lite_d, blur) * outside * rel * smoothstep(0.0, 0.7, facing) * win
                            let contact = exp(-max(d, 0.0) / (blur * 0.30)) * outside
                            col = self.light_face(col, 1.0, 1.0 - clamp(dark * self.m_shadow.x, 0.0, 1.0), 1.0 - clamp(contact * self.m_shadow.w, 0.0, 1.0))
                            col = self.in_light(col, clamp(lite * self.m_inner.z, 0.0, 1.0))
                        }
                        if lit > 0.5 && glow > 0.001 {
                            col = mix(col, self.m_glow_ink.xyz, clamp(self.tail(d, glow * 14.0) * glow, 0.0, 1.0) * 0.5 * win)
                        }
                        let ext = max(R, wr * R)
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
                    let fcov = 1.0 - smoothstep(-px, px, fd)
                    col = mix(col, face, fcov)
                    cover = max(cover, fcov)
                }
                layer = layer + 1.0
            }

            // THE MARKS: the value arc, the ticks and the pointer, their
            // gradient and ink behind the hooks.
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
                let acov = (1.0 - smoothstep(-px, px, ad)) * under
                col = mix(col, self.m_glow_ink.xyz, acov)
                cover = max(cover, acov)
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
                        let mg = self.lod_mark_grad(p, m, R, spin, td.y)
                        col = mix(col, self.lod_mark_ink(col, md, mg, mw, ml, self.m_ptr_ink.xyz, px, spec_k), mk)
                        cover = max(cover, (1.0 - smoothstep(-px, px, md)) * mk)
                    }
                    m = m + 1.0
                }
            }
            if touched < 0.5 { return vec4(0.0, 0.0, 0.0, 0.0) }
            return self.knob_out(self.hdr_out(mix(self.m_ground.xyz, col, qa)), cover * qa)
        }
    }

    // THE VERSIONS, as knobs. A spread never overwrites what an earlier one
    // put in, so a feature's hooks go first and the body's trivial ones only
    // fill what is left.
    mod.storybook.KnobVersionPreview = mod.storybook.TurnedKnob{
        draw_knob +: {..KnobSalt, ..KnobPreview}
    }
    mod.storybook.KnobVersionBare = mod.storybook.TurnedKnob{
        draw_knob +: {..LodBody}
    }
    mod.storybook.KnobVersionCuts = mod.storybook.TurnedKnob{
        draw_knob +: {..LodCuts, ..LodBody}
    }
    mod.storybook.KnobVersionWings = mod.storybook.TurnedKnob{
        draw_knob +: {..LodWings, ..LodBody}
    }
    mod.storybook.KnobVersionSelfShadow = mod.storybook.TurnedKnob{
        draw_knob +: {..LodSelfShadow, ..LodBody}
    }
    mod.storybook.KnobVersionCastShadow = mod.storybook.TurnedKnob{
        draw_knob +: {..LodCast, ..LodBody}
    }
    mod.storybook.KnobVersionMarks = mod.storybook.TurnedKnob{
        draw_knob +: {..LodMarks, ..LodBody}
    }
    mod.storybook.KnobVersionFull = mod.storybook.TurnedKnob{
        draw_knob +: {..KnobSalt}
    }
}
