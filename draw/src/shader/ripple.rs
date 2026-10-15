//! `Ripple`: water on a sheet of glass, from the drops a pointer leaves.
//!
//! The shader half of the pointer glass (`PointerGlass` in the widgets):
//! the glass gets the trail of the last eight drops the pointer left as it
//! moved, newest first, each a `vec3` of its place in points from the
//! glass's top left and its age in seconds (an age below zero is no drop),
//! and how long a drop lives. `sum` turns them into the water at a point:
//! how far what the glass reads there is displaced, and the slope the light
//! catches. `lay` is one way to draw that glass over what it reads; a sheet
//! with a look of its own takes `sum` and lays its own light and shade.
//!
//! The rings: each drop sends a ring out at 60 points a second, 20 points
//! from crest to crest, landing over its first 0.16 s and fading as it ages
//! and grows. A fresh crest displaces by up to about 4 points each way, and
//! the picture never folds over itself: the rings are trochoids, a broad
//! crest that magnifies and a narrower trough that squeezes, each ring's
//! magnification is bounded, and where rings can meet the newest takes its
//! share of one budget first and the older ones share the rest. The water
//! is still over the last 12 points inside the glass's edge.
//!
//! The conventions are `Finish`'s: screen space with y DOWN, distances in
//! points and negative inside. Every function is a pure function of its
//! arguments, so glass drawn with them costs nothing while no drop is
//! alive.
use crate::makepad_platform::*;

script_mod! {
    use mod.pod.*
    use mod.math.*

    mod.sdf.Ripple = {
        // Helpers the members share are `fn` declarations, in this object's
        // own scope.

        fn r_amp(d: vec3, life: float) -> float {
            if d.z < 0.0 {
                return 0.0
            }
            let left = clamp(1.0 - d.z / max(life, 0.01), 0.0, 1.0)
            return 1.575 * left * sqrt(left) * smoothstep(0.0, 0.16, d.z) / (1.0 + d.z * 0.6667)
        }

        // One drop's ring at `p` as a displacement, for a drop of strength
        // `a`: a packet two crests deep round the front, its phase run
        // through the trochoid (Kepler's equation in three Newton steps),
        // and nothing at the drop's own centre.
        fn r_ring(p: vec2, d: vec3, a: float) -> vec2 {
            if a <= 0.0 {
                return vec2(0.0, 0.0)
            }
            let q = p - d.xy
            let r = max(length(q), 0.001)
            let u = r - d.z * 60.0
            let phi = u * 0.3141593
            var t = phi + 0.35 * sin(phi)
            t = t - (t - 0.35 * sin(t) - phi) / (1.0 - 0.35 * cos(t))
            t = t - (t - 0.35 * sin(t) - phi) / (1.0 - 0.35 * cos(t))
            t = t - (t - 0.35 * sin(t) - phi) / (1.0 - 0.35 * cos(t))
            let env = exp(0.0 - u * u / 196.0) * (1.0 - exp(0.0 - r * r / 36.0))
            return q * (a * env * sin(t) * 3.183099 / r)
        }

        // 1 where two rings' packets (31 points either side of their
        // fronts) can meet, falling to 0 over 6 points apart.
        fn r_meet(da: vec3, db: vec3) -> float {
            let dist = length(da.xy - db.xy)
            let fa = max(da.z, 0.0) * 60.0
            let fb = max(db.z, 0.0) * 60.0
            let gap = max(dist - (fa + fb + 61.6), max(max(fa - 30.8, 0.0) - fb - 30.8, max(fb - 30.8, 0.0) - fa - 30.8) - dist)
            return 1.0 - smoothstep(0.0, 6.0, gap)
        }

        // A drop's share of the magnification budget: what the newer drops
        // it can meet left of 0.88, over its own need `n`.
        fn r_share(used: float, n: float) -> float {
            return clamp((0.88 - used) / max(n, 0.00001), 0.0, 1.0)
        }

        // A drop's strength: full a little after it lands, fading with what
        // is left of its `life` and as its ring grows; 0 for no drop.
        amp: fn(d: vec3, life: float) -> float {
            return r_amp(d, life)
        }

        // The water at `p`, the glass's edge `edge_d` away (negative
        // inside): `xy` the displacement of what the glass reads there,
        // `zw` the slope the light catches (about 4 at a fresh crest), both
        // still over the last 12 points inside the edge. `d0` to `d7` are
        // the trail's drops, newest first.
        sum: fn(p: vec2, edge_d: float, life: float, d0: vec3, d1: vec3, d2: vec3, d3: vec3, d4: vec3, d5: vec3, d6: vec3, d7: vec3) -> vec4 {
            let a0 = r_amp(d0, life)
            let a1 = r_amp(d1, life)
            let a2 = r_amp(d2, life)
            let a3 = r_amp(d3, life)
            let a4 = r_amp(d4, life)
            let a5 = r_amp(d5, life)
            let a6 = r_amp(d6, life)
            let a7 = r_amp(d7, life)
            let s0 = r_share(0.0, a0 * 0.612)
            let t0 = s0 * a0 * 0.612
            let s1 = r_share(r_meet(d1, d0) * t0, a1 * 0.612)
            let t1 = s1 * a1 * 0.612
            let s2 = r_share(r_meet(d2, d0) * t0 + r_meet(d2, d1) * t1, a2 * 0.612)
            let t2 = s2 * a2 * 0.612
            let s3 = r_share(r_meet(d3, d0) * t0 + r_meet(d3, d1) * t1 + r_meet(d3, d2) * t2, a3 * 0.612)
            let t3 = s3 * a3 * 0.612
            let s4 = r_share(r_meet(d4, d0) * t0 + r_meet(d4, d1) * t1 + r_meet(d4, d2) * t2 + r_meet(d4, d3) * t3, a4 * 0.612)
            let t4 = s4 * a4 * 0.612
            let s5 = r_share(r_meet(d5, d0) * t0 + r_meet(d5, d1) * t1 + r_meet(d5, d2) * t2 + r_meet(d5, d3) * t3 + r_meet(d5, d4) * t4, a5 * 0.612)
            let t5 = s5 * a5 * 0.612
            let s6 = r_share(r_meet(d6, d0) * t0 + r_meet(d6, d1) * t1 + r_meet(d6, d2) * t2 + r_meet(d6, d3) * t3 + r_meet(d6, d4) * t4 + r_meet(d6, d5) * t5, a6 * 0.612)
            let t6 = s6 * a6 * 0.612
            let s7 = r_share(r_meet(d7, d0) * t0 + r_meet(d7, d1) * t1 + r_meet(d7, d2) * t2 + r_meet(d7, d3) * t3 + r_meet(d7, d4) * t4 + r_meet(d7, d5) * t5 + r_meet(d7, d6) * t6, a7 * 0.612)
            let w0 = r_ring(p, d0, a0)
            let w1 = r_ring(p, d1, a1)
            let w2 = r_ring(p, d2, a2)
            let w3 = r_ring(p, d3, a3)
            let w4 = r_ring(p, d4, a4)
            let w5 = r_ring(p, d5, a5)
            let w6 = r_ring(p, d6, a6)
            let w7 = r_ring(p, d7, a7)
            let disp = w0 * s0 + w1 * s1 + w2 * s2 + w3 * s3 + w4 * s4 + w5 * s5 + w6 * s6 + w7 * s7
            let slope = w0 + w1 + w2 + w3 + w4 + w5 + w6 + w7
            let keep = smoothstep(0.0, 12.0, 0.0 - edge_d)
            return vec4(disp * keep, slope * keep)
        }

        // Clear glass over the water, premultiplied: `under` is what lies
        // under it read along the displacement (when `seen` is 1), a white
        // `sheen` over it, light on the flanks that face the upper left and
        // shade on the others, from the `slope`; without the scene
        // (`seen` 0), the sheen, light and shade alone, over what is drawn
        // under the glass. `cov` is the glass's coverage.
        lay: fn(under: vec3, seen: float, slope: vec2, sheen: float, cov: float) -> vec4 {
            let lit = dot(slope, vec2(-0.6, -0.8)) * 0.25
            let light = clamp(lit * 0.8, 0.0, 0.6)
            let shade = clamp(0.0 - lit * 0.5, 0.0, 0.45)
            if seen > 0.5 {
                var col = under * (1.0 - sheen) + vec3(sheen, sheen, sheen)
                col = mix(col, vec3(1.0, 1.0, 1.0), light)
                col = col * (1.0 - shade)
                return vec4(col * cov, cov)
            }
            var out = vec4(sheen, sheen, sheen, sheen)
            out = vec4(light, light, light, light) + out * (1.0 - light)
            out = vec4(0.0, 0.0, 0.0, shade) + out * (1.0 - shade)
            return out * cov
        }
    }
}
