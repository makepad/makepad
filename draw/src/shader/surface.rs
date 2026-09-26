//! `Material`: analytic surface shading from a signed distance.
//!
//! The bevel profile and its slope, the normal, the wrapped Lambert, the
//! two-ramp face shade with its face gradient, hairline, concavity
//! occlusion, rim, gloss and specular, the real blurred-box coverage used
//! for inner shadows, the shadow tail, and output dither.
//!
//! Pure functions over a distance `d` (negative inside, in points), so a
//! widget's pixel function can light the shape it already has. Everything is
//! evaluated in the pass that is already running; nothing here samples a
//! texture or reads `draw_pass.time`, so a window using it stays idle at rest.
//!
//! Screen space throughout: x right, y DOWN, z out of the screen. A key light
//! "from the top" therefore has a negative y. `light` is always
//! `vec4(direction.xyz, intensity)`.
//!
//! Packed parameters:
//! * `relief`  = bevel width (pt), profile curve (0 soft .. 1 round), raise (pt), specular
//! * `finish`  = occlusion, rim, gloss, roughness
//! * `tune`    = face gradient, hairline, occlusion reach (x bevel), sink (pt)
use crate::makepad_platform::*;

script_mod! {
    use mod.pod.*
    use mod.math.*

    mod.sdf.Material = {
        // Signed distance to a rounded box centred on `c` with half size `h`.
        sd_box: fn(p: vec2, c: vec2, h: vec2, r: float) -> float {
            let q = abs(p - c) - h + vec2(r, r)
            return min(max(q.x, q.y), 0.0) + length(max(q, vec2(0.0, 0.0))) - r
        }

        // Helpers the members share are `fn` declarations: those are in this
        // object's own scope, while a call through `Material.` resolves in
        // the caller's.

        // The normal from the outward gradient `g`, the profile slope and an
        // extra `bias` slope (a dome's), scaled by the elevation `elev`.
        fn m_normal(d: float, g: vec2, w: float, curve: float, elev: float, bias: float) -> vec3 {
            let t = clamp(-d / max(w, 0.001), 0.0, 1.0)
            let omt = 1.0 - t
            let soft = 6.0 * t * omt
            let rnd = omt / max(sqrt(max(1.0 - omt * omt, 0.0)), 0.125)
            let sl = min(mix(soft, rnd, clamp(curve, 0.0, 1.0)), 8.0)
            let s = (sl + bias) * elev
            return normalize(vec3(g.x * s, g.y * s, 1.0))
        }

        normal: fn(d: float, g: vec2, w: float, curve: float, elev: float, bias: float) -> vec3 {
            return m_normal(d, g, w, curve, elev, bias)
        }

        // Diffuse in .x, specular in .y; `spec` = (strength, power).
        fn m_lit(n: vec3, light: vec4, spec: vec2) -> vec2 {
            let l = normalize(light.xyz)
            let diff = smoothstep(-0.35, 1.0, dot(n, l)) * light.w
            let h = normalize(l + vec3(0.0, 0.0, 1.0))
            return vec2(diff, pow(max(dot(n, h), 0.0), max(spec.y, 1.0)) * spec.x)
        }

        // Occlusion hugging the inside edge.
        fn m_ao(d: float, radius: float) -> float {
            return (1.0 - smoothstep(0.0, max(radius, 0.001), -d)) * step(d, 0.0)
        }

        // The lit band just inside the edge.
        fn m_rim(d: float, n: vec3, light: vec4, w: float) -> float {
            let f = max(dot(n, normalize(light.xyz)), 0.0)
            return (1.0 - smoothstep(0.0, max(w, 0.001), -d)) * pow(f, 0.8) * step(d, 0.0)
        }

        // Gloss is a reflection of a soft light box above and in front, read
        // off the normal with the flat face's share subtracted.
        fn m_gloss(n: vec3) -> float {
            let sky = normalize(vec3(0.0, -0.6, 0.8))
            let s = max(dot(n, sky), 0.0)
            return clamp((s * s - 0.64) * 2.8, 0.0, 1.0)
        }

        // How a shadow or a glow dies away from the edge that throws it:
        // `fall` 0 = linear, gone at three blur lengths; 1 = exponential.
        tail: fn(d: float, bl: float, fall: float) -> float {
            let dd = max(d, 0.0) / max(bl, 0.001)
            return mix(clamp(1.0 - dd / 3.0, 0.0, 1.0), exp(-dd), clamp(fall, 0.0, 1.0))
        }

        // The direction a shadow falls: away from the light, in the plane.
        shadow_dir: fn(light: vec4) -> vec2 {
            let ll = length(light.xy)
            if ll > 0.0001 {
                return -light.xy / ll
            }
            return vec2(0.0, 1.0)
        }

        fn erf2(x0: vec2) -> vec2 {
            let s = sign(x0)
            let a = abs(x0)
            var x1 = vec2(1.0, 1.0) + (vec2(0.278393, 0.278393) + (vec2(0.230389, 0.230389) + 0.078108 * (a * a)) * a) * a
            x1 = x1 * x1
            return s - s / (x1 * x1)
        }

        fn gauss_at(x: float, sigma: float) -> float {
            return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(6.283185307179586) * sigma)
        }

        fn box_cov_x(x: float, y: float, sigma: float, corner: float, hs: vec2) -> float {
            let delta = min(hs.y - corner - abs(y), 0.0)
            let curved = hs.x - corner + sqrt(max(0.0, corner * corner - delta * delta))
            let integral = vec2(0.5, 0.5) + 0.5 * erf2((vec2(x, x) + vec2(-curved, curved)) * (sqrt(0.5) / sigma))
            return integral.y - integral.x
        }

        // A real Gaussian-blurred rounded box (no medial-axis crease), for
        // inner shadows: coverage of the box lower..upper at `pt`.
        box_cov: fn(lower: vec2, upper: vec2, pt: vec2, sigma: float, corner: float) -> float {
            let ctr = (lower + upper) * 0.5
            let hs = (upper - lower) * 0.5
            let q = pt - ctr
            let low = q.y - hs.y
            let high = q.y + hs.y
            let start = clamp(-3.0 * sigma, low, high)
            let end = clamp(3.0 * sigma, low, high)
            let stp = (end - start) / 4.0
            var y = start + stp * 0.5
            var v = 0.0
            v = v + box_cov_x(q.x, q.y - y, sigma, corner, hs) * gauss_at(y, sigma) * stp
            y = y + stp
            v = v + box_cov_x(q.x, q.y - y, sigma, corner, hs) * gauss_at(y, sigma) * stp
            y = y + stp
            v = v + box_cov_x(q.x, q.y - y, sigma, corner, hs) * gauss_at(y, sigma) * stp
            y = y + stp
            v = v + box_cov_x(q.x, q.y - y, sigma, corner, hs) * gauss_at(y, sigma) * stp
            return v
        }

        // The lit face of one layer.
        // `depth` is how far the face sits from its surround, `convex` how
        // the face itself bulges (+) or dishes (-); `dome` an extra slope
        // (a revolve's), `insh` the inner shadow coverage, `turned` 1 for a
        // surface of revolution (no face gradient: its normal carries it).
        face: fn(base: vec3, d: float, g: vec2, uv: vec2, depth: float, convex: float, dome: float, insh: float, turned: float, light: vec4, relief: vec4, finish: vec4, tune: vec4, inner: float, light_ink: vec3, shadow_ink: vec3, spec_scale: float) -> vec3 {
            let bw = mix(relief.x, 0.001, turned)
            let cu = relief.y * (1.0 - turned)
            let n = m_normal(d, g, bw, cu, convex, dome)
            let lt = m_lit(n, light, vec2(relief.w * spec_scale, mix(64.0, 4.0, clamp(finish.w, 0.0, 1.0))))
            let l = normalize(light.xyz)
            let flatv = smoothstep(-0.35, 1.0, l.z) * light.w
            let key = (lt.x - flatv) * 1.5
            // Two ramps that meet flat: smoothstep starts with zero slope on
            // both sides, so a dome carries no crease where they meet.
            var o = mix(base, light_ink, smoothstep(0.0, 1.0, key))
            o = mix(o, shadow_ink, smoothstep(0.0, 1.0, -key))
            // The face gradient: a box face is curved across its whole span.
            let curv = 1.0 - step(0.5, turned)
            if tune.x * curv > 0.001 {
                let ax = normalize(light.xy + vec2(0.000001, 0.000001))
                let t = dot(uv - vec2(0.5, 0.5), ax) * 2.0 * sign(convex)
                let gink = mix(shadow_ink, light_ink, smoothstep(-1.0, 1.0, t))
                o = mix(o, gink, tune.x * curv * 0.5 * smoothstep(0.0, 1.0, abs(t)))
            }
            // Occlusion is concavity: only a face dishing away occludes its rim.
            let concave = clamp(-convex / max(tune.w, 0.001), 0.0, 1.0)
            o = mix(o, shadow_ink, m_ao(d, max(bw, 0.001) * tune.z) * finish.x * concave)
            // The hairline: lit on the side facing the light, dark opposite.
            if tune.y > 0.001 {
                let band = 1.0 - smoothstep(0.0, 1.4, abs(d))
                let facing = dot(g, normalize(light.xy + vec2(0.000001, 0.000001))) * sign(convex)
                o = mix(o, light_ink, band * clamp(facing, 0.0, 1.0) * tune.y)
                o = mix(o, shadow_ink, band * clamp(-facing, 0.0, 1.0) * tune.y)
            }
            let sunk = clamp(-depth / max(tune.w, 0.001), 0.0, 1.0)
            if sunk > 0.001 && inner > 0.001 {
                o = mix(o, shadow_ink, clamp(insh * step(d, 0.0) * inner * sunk, 0.0, 1.0))
            }
            o = mix(o, light_ink, clamp(m_rim(d, n, light, max(bw * 0.5, 0.5)) * finish.y, 0.0, 1.0))
            o = o + vec3(lt.y, lt.y, lt.y)
            o = mix(o, light_ink, clamp(m_gloss(n) * finish.z * step(d, 0.0), 0.0, 1.0))
            return o
        }

        // Triangular dither of +-1 output LSB at a pixel centre: two hashes
        // differenced. Static: it never reads time.
        dither: fn(p: vec2) -> float {
            let a = fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453)
            let b = fract(sin(dot(p + vec2(17.13, 3.71), vec2(12.9898, 78.233))) * 43758.5453)
            return (a - b) / 255.0
        }

        // Film grain from a screen position; never from time.
        grain: fn(p: vec2) -> float {
            return fract(sin(dot(floor(p), vec2(12.9898, 78.233))) * 43758.5453) - 0.5
        }
    }
}
