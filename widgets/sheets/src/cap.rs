//! `Cap`: the fader cap engine the theme sheets draw their sliders with, the
//! slider bench's shaders ported to Splash.
//!
//! The cap is a HEIGHT FIELD over a footprint. The footprint is a rounded box
//! (cut by a plan curve: the half width along the length) or a drop of
//! liquid. The height is lofted from curves: `along` the track, `across` it,
//! and `edge`, how the top meets the wall over the edge width in from the
//! outline; the along and across curves multiply and meet the edge in a
//! smooth minimum. Grip ribs (a tooth curve tiled along the cap, or round a
//! puck), a mark (painted, grooved, split to the base, lit or embossed,
//! across the cap or along it) and a cut (a cylinder or a sphere out of the
//! top) finish the solid. It is lit by one key light with a plain sky and
//! ground reflection, no image-based lighting. Under it: the cast shadow,
//! the contact shadow, the glow, a well sunk into the ground (or a bezel
//! raised on it), the track (slot, rails, flush line, lit tube, rod) with its
//! value fill, and the scale. Glass caps bend and magnify what lies under
//! them, with a Fresnel shade, a lens highlight and a caustic.
//!
//! Frames. Everything is laid out in the fader's own frame `ab`: x ALONG the
//! track from the value's zero, y ACROSS it, as the sheets' fader pixel
//! functions already work (`a`, `b`, `is_vertical`), so a vertical fader is
//! the same drawing read up the screen. A cap-local point `q` is `ab` less
//! the cap's centre and `hs` is the cap's half size (along, across). Lengths
//! are in points; `px` is one device pixel in points (`1.0 / dpi_factor`) and
//! every edge that takes it is antialiased against it.
//!
//! Lights. `l` is the theme's key light in SCREEN space (x right, y down, z
//! toward the viewer, normalised). The shading functions take it with a
//! normal from `normal` and a view from `view`, which turn the cap's frame
//! back to the screen, because the reflected sky has an up and a left.
//! `lt = light(l, vert)` is the same light turned into the fader's frame;
//! the functions that work in `ab` (shadows, wells, tracks, the lit fill on
//! the cap, the spun grain, the lens and the caustic) take that one.
//!
//! Curves. Each is a cubic Bézier through up to eight anchors, handles
//! resolved by the bench's curve rules (the bench prints them under each
//! curve of its Splash output); `curve` returns the height and the slope.
//! Where the bench draws a symmetric curve from the centre to one end, read
//! it at `abs(q.x) / hs.x` and pass the result through `mirror(c, q.x)`;
//! a whole curve is read at `(q.x / hs.x + 1.0) * 0.5`. The across curve is
//! read at `abs(q.y) / hs.y`, the edge at `edge_x(...)`, the tooth at
//! `rib_x(...)`.
//!
//! Packed parameters, as the bench packs its uniforms, so its Splash output
//! carries over number for number:
//! * `shape`   = corner share, edge width share, blend, height (pt)
//! * `rib`     = count, depth, band from, band to (shares of the length)
//! * `mark`    = kind (0 none, 1 paint, 2 groove, 3 split, 4 lit, 5 emboss),
//!               width (pt), depth, glow reach (pt)
//! * `mark_at` = runs (0 across the cap, 1 along it), position 0..1 along the
//!               cap, extent 0..1 of the other side, spare
//! * `cut`     = kind (0 none, 1 cylinder lying across, 2 sphere), centre
//!               (share of the half length, both ends), radius (share of the
//!               half length), centre height (share of the height)
//! * `top`     = the top outline the shadow is thrown from: half length and
//!               half width as shares of the base's, corner as a share of its
//!               smaller half (the bench prints it)
//! * `tbox`, `wbox` = a track's or a well's centre (xy) and half size (zw)
//!
//! One cap, in the bench's order (its `capAt` and `main`):
//! ```text
//! the ground: well, then slot / rails / flush / tube / rod, then ticks
//! let q = ab - vec2(at, mid)
//! let fp = Cap.sd_cap(q, hs, r, plan)              // or Cap.sd_drop
//! let along = Cap.curve(...), across, edge, tooth   // read as above
//! let f = Cap.field(q, hs, fp, along, across, edge, tooth, shape, rib, mark, mark_at, cut)
//!     a tall cap reads fp and f three times, at Cap.perspective's point
//! var cap = Cap.shade(Cap.normal(f.yz, shape.w, vert), l, Cap.view(q, hs, near, vert), f.x, ...)
//!     or Cap.glass_shade over the ground drawn again at ab + Cap.glass_disp
//! then Cap.spun, Cap.emit, Cap.in_well, Cap.fill_light and Cap.mark_glow on `cap`
//! under it: Cap.shadow and Cap.contact (clipped to a sunk well), Cap.caustic, Cap.glow
//! the cap over all at Cap.cover(fp.x, px); Cap.tone; Cap.on_ground outside the widget
//! ```
//! Every function is a pure function of its arguments: nothing samples a
//! texture or reads `draw_pass.time`, so a window using them stays idle at
//! rest. The one loop, the cast shadow's search, has a fixed length.
use makepad_widgets_core::makepad_platform::*;

script_mod! {
    use mod.pod.*
    use mod.math.*

    mod.sdf.Cap = {
        // Helpers the members share are `fn` declarations: those are in this
        // object's own scope, while a call through `Cap.` resolves in the
        // caller's. They reach the compiled shader under their own names,
        // beside every other module's and the sheet's, hence the `cap_`.

        // 1 at and above zero, -1 below: the bench's `x < 0.0 ? -1.0 : 1.0`,
        // which `sign` is not at zero.
        fn cap_sgn(x: float) -> float {
            return step(0.0, x) * 2.0 - 1.0
        }

        // The bench's edge antialiasing: a smoothstep one pixel wide.
        fn cap_cover(d: float, px: float) -> float {
            return 1.0 - smoothstep(-px * 0.5, px * 0.5, d)
        }

        // A rounded box: signed distance (negative inside) in .x, outward
        // gradient in .yz.
        fn cap_rbox(q: vec2, hs: vec2, r: float) -> vec3 {
            let rr = clamp(r, 0.0, min(hs.x, hs.y))
            let dq = abs(q) - (hs - vec2(rr, rr))
            let m = max(dq, vec2(0.0, 0.0))
            let lm = length(m)
            let d = lm + min(max(dq.x, dq.y), 0.0) - rr
            var g = vec2(0.0, 1.0)
            if dq.x > dq.y {
                g = vec2(1.0, 0.0)
            }
            if lm > 0.00001 {
                g = m / lm
            }
            return vec3(d, g.x * cap_sgn(q.x), g.y * cap_sgn(q.y))
        }

        // smoothstep and its derivative, for the mark's soft walls.
        fn cap_sstep(e0: float, e1: float, x: float) -> vec2 {
            let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0)
            return vec2(t * t * (3.0 - 2.0 * t), 6.0 * t * (1.0 - t) / (e1 - e0))
        }

        // One coordinate of a cubic Bézier and its derivative at `s`.
        fn cap_cubic(s: float, p0: float, p1: float, p2: float, p3: float) -> vec2 {
            let u = 1.0 - s
            let v = u * u * u * p0 + 3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s * p3
            let d = 3.0 * u * u * (p1 - p0) + 6.0 * u * s * (p2 - p1) + 3.0 * s * s * (p3 - p2)
            return vec2(v, d)
        }

        // One Newton step toward the parameter where the segment reaches `x`.
        // The curve rules keep x rising along every segment, so the
        // derivative is never negative; a flat start (a point with no
        // handles) only slows the step, and the clamp holds it in the segment.
        fn cap_newton(s: float, x: float, p0: float, p1: float, p2: float, p3: float) -> float {
            let e = cap_cubic(s, p0, p1, p2, p3)
            return clamp(s - (e.x - x) / max(e.y, 0.000001), 0.0, 1.0)
        }

        // One segment from anchor `a` (out handle `ao`) to anchor `b` (in
        // handle `bi`), handles relative to their anchors: the height and
        // the slope at `x`. The parameter is solved by six Newton steps from
        // the linear guess: a quarter-round edge starts with a vertical
        // handle, where x grows as the square of the parameter, and four
        // steps left its rim up to 0.08 low (0.02 past the bake's first
        // texel); six hold every master curve to 0.006 in the first texel
        // and 0.00001 beyond. The slope is read a hair inside the segment,
        // so a straight segment between two handle-less points (where both
        // derivatives vanish at the ends) still gives its own slope, and it
        // is held to the +-8 the bench's bake could store: the bench shades
        // a sharp edge with that slope, not with a vertical wall.
        fn cap_bez(x: float, a: vec2, ao: vec2, bi: vec2, b: vec2) -> vec2 {
            let c1 = a + ao
            let c2 = b + bi
            var s = clamp((x - a.x) / max(b.x - a.x, 0.000001), 0.0, 1.0)
            s = cap_newton(s, x, a.x, c1.x, c2.x, b.x)
            s = cap_newton(s, x, a.x, c1.x, c2.x, b.x)
            s = cap_newton(s, x, a.x, c1.x, c2.x, b.x)
            s = cap_newton(s, x, a.x, c1.x, c2.x, b.x)
            s = cap_newton(s, x, a.x, c1.x, c2.x, b.x)
            s = cap_newton(s, x, a.x, c1.x, c2.x, b.x)
            let y = cap_cubic(s, a.y, c1.y, c2.y, b.y)
            let sd = clamp(s, 0.001, 0.999)
            let dx = cap_cubic(sd, a.x, c1.x, c2.x, b.x)
            let dy = cap_cubic(sd, a.y, c1.y, c2.y, b.y)
            return vec2(clamp(y.x, 0.0, 1.0), clamp(dy.y / max(dx.y, 0.000001), -8.0, 8.0))
        }

        // The plan curve's cut of the footprint: the half width along the
        // length, a share of the box's. Distance and gradient.
        fn cap_plan(q: vec2, hs: vec2, plan: vec2) -> vec3 {
            let dw = hs.y * plan.y * 0.5 / hs.x
            let nrm = sqrt(1.0 + dw * dw)
            return vec3((abs(q.y) - hs.y * plan.x) / nrm, -dw / nrm, cap_sgn(q.y) / nrm)
        }

        // A capsule from the origin to `pb`, radius `ra` at the origin and
        // `rb` at `pb`: the drop's tail.
        fn cap_uneven(p: vec2, pb: vec2, ra: float, rb: float) -> float {
            let h = max(dot(pb, pb), 0.0001)
            let q0 = vec2(dot(p, vec2(pb.y, -pb.x)), dot(p, pb)) / h
            let q = vec2(abs(q0.x), q0.y)
            let b = ra - rb
            let c = vec2(sqrt(max(h - b * b, 0.0)), b)
            let k = c.x * q.y - c.y * q.x
            let m = dot(c, q)
            let n = dot(q, q)
            if k < 0.0 {
                return sqrt(h * n) - ra
            }
            if k > c.x {
                return sqrt(h * (n + 1.0 - 2.0 * q.y)) - rb
            }
            return m - ra
        }

        // A smooth minimum: the value in .x, the share of `a` in .y.
        fn cap_smin(a: float, b: float, k: float) -> vec2 {
            let h = clamp(0.5 + 0.5 * (b - a) / max(k, 0.0001), 0.0, 1.0)
            return vec2(mix(b, a, h) - k * h * (1.0 - h), h)
        }

        // One edge of a polygon's distance: `acc` carries the squared
        // distance so far and the inside sign, flipped each time the edge
        // crosses the horizontal through `p` on the right side.
        fn cap_edge(p: vec2, vi: vec2, vj: vec2, acc: vec2) -> vec2 {
            let e = vj - vi
            let w = p - vi
            let b = w - e * clamp(dot(w, e) / max(dot(e, e), 0.000001), 0.0, 1.0)
            let c = step(vi.y, p.y) + 1.0 - step(vj.y, p.y) + step(e.y * w.x, e.x * w.y)
            let flip = step(2.5, c) + step(c, 0.5)
            return vec2(min(acc.x, dot(b, b)), acc.y * (1.0 - 2.0 * flip))
        }

        // A box of half size `b` swept from `-v` to `v`: the hexagon its
        // corners trace, signed distance. The shape a raised part throws
        // when the light moves its shadow by `2v`.
        fn cap_sweep(p0: vec2, b: vec2, v0: vec2) -> float {
            let sg = vec2(cap_sgn(v0.x), cap_sgn(v0.y))
            let p = p0 * sg
            let v = abs(v0)
            let h0 = vec2(-b.x, -b.y) - v
            let h1 = vec2(b.x, -b.y) - v
            let h2 = vec2(b.x, -b.y) + v
            let h3 = vec2(b.x, b.y) + v
            let h4 = vec2(-b.x, b.y) + v
            let h5 = vec2(-b.x, b.y) - v
            var acc = vec2(dot(p - h5, p - h5), 1.0)
            acc = cap_edge(p, h0, h5, acc)
            acc = cap_edge(p, h1, h0, acc)
            acc = cap_edge(p, h2, h1, acc)
            acc = cap_edge(p, h3, h2, acc)
            acc = cap_edge(p, h4, h3, acc)
            acc = cap_edge(p, h5, h4, acc)
            return acc.y * sqrt(acc.x)
        }

        // Where a ray from `q` along `od` enters and leaves the box's slabs.
        fn cap_span(q: vec2, hs: vec2, od: vec2) -> vec2 {
            let inv = vec2(1.0, 1.0) / (od + vec2(0.0001, 0.0001))
            let s1 = (q - hs) * inv
            let s2 = (q + hs) * inv
            let sn = min(s1, s2)
            let sx = max(s1, s2)
            return vec2(max(sn.x, sn.y), min(sx.x, sx.y))
        }

        // The height 0..1 up a raised box at which the ground point `q` was
        // cast, the light moving the shadow `offl` along `od`: a ray that
        // misses the base is beside the sweep, and the edge nearest to it was
        // cast at the height where the ray leaves the last slab.
        fn cap_height(q: vec2, hs: vec2, od: vec2, offl: float) -> float {
            let cs = cap_span(q, hs, od)
            var t = cs.y
            if cs.x <= cs.y {
                t = cs.x
            }
            return clamp(t / offl, 0.0, 1.0)
        }

        // One box between the base's core (the footprint less its corner)
        // and the top's core thrown `off` away: at `t` 0..1 up the height,
        // interpolated. Signed distance, no corner.
        fn cap_core_at(q: vec2, cb: vec2, ct: vec2, off: vec2, t: float) -> float {
            let dq = abs(q - off * t) - mix(cb, ct, t)
            return length(max(dq, vec2(0.0, 0.0))) + min(max(dq.x, dq.y), 0.0)
        }

        // The cast shadow of a raised rounded box: its distance in .x, the
        // blur in .y, the height it was cast from in .z and the length of
        // the throw in .w. The shape is the bench's: the hull of the base's
        // core and the top's core (`top`: the top outline's half size as
        // shares of the base's and its corner as a share of its smaller
        // half) thrown away from the light by the rise, rounded by the
        // corner of the height the point was cast from. The hull of two
        // boxes is every box interpolated between them, and the distance to
        // that box is convex in the interpolation, so a golden-section
        // search over it finds the hull's distance without the polygon the
        // bench builds on the CPU. The blur grows and the shadow thins with
        // that height: sharp at the foot, soft at the tip.
        fn cap_cast(q: vec2, hs: vec2, r: float, top: vec3, lt: vec3, rise: float, blur: float, px: float) -> vec4 {
            let off = -lt.xy / max(lt.z, 0.25) * max(rise, 0.0) * 0.7
            let offl = max(length(off), 0.001)
            let od = off / offl
            let rr = clamp(r, 0.0, min(hs.x, hs.y))
            let fb = cap_rbox(q, hs, rr)
            let th = clamp(max(fb.x, 0.0) / (offl * max(dot(fb.yz, od), 0.25)), 0.0, 1.0)
            let ht = hs * top.xy
            let rt = top.z * min(ht.x, ht.y)
            let cb = max(hs - vec2(rr, rr), vec2(0.0, 0.0))
            let ct = max(ht - vec2(rt, rt), vec2(0.0, 0.0))
            var a = 0.0
            var b = 1.0
            var c = 0.381966
            var d = 0.618034
            var fc = cap_core_at(q, cb, ct, off, c)
            var fd = cap_core_at(q, cb, ct, off, d)
            for i in 0..10 {
                if fc < fd {
                    b = d
                    d = c
                    fd = fc
                    c = a + 0.381966 * (b - a)
                    fc = cap_core_at(q, cb, ct, off, c)
                } else {
                    a = c
                    c = d
                    fc = fd
                    d = b - 0.381966 * (b - a)
                    fd = cap_core_at(q, cb, ct, off, d)
                }
            }
            let hull = min(min(fc, fd), min(cap_core_at(q, cb, ct, off, 0.0), cap_core_at(q, cb, ct, off, 1.0)))
            let hst = hs * mix(vec2(1.0, 1.0), top.xy, th)
            let dsh = hull - mix(rr, top.z * min(hst.x, hst.y), th)
            let tip = clamp(blur, max(rise * 0.35, 1.0), max(rise, 1.0))
            let bl = max(tip * mix(0.15, 1.0, th), px)
            return vec4(dsh, bl, th, offl)
        }

        fn cap_reflect(i: vec3, n: vec3) -> vec3 {
            return i - n * (2.0 * dot(n, i))
        }

        // The plain sky: lit from above, the ground below, a horizon
        // between, a little brighter on the left; straight up it is the
        // light's own ink. Screen space: y down.
        fn cap_sky(r: vec3, ground: vec3, light_ink: vec3) -> vec3 {
            let horizon = mix(ground, vec3(1.0, 1.0, 1.0), 0.35)
            let floorc = ground * 0.55
            var env = mix(horizon, floorc, min(r.y * 1.4, 1.0))
            if r.y < 0.0 {
                env = mix(horizon, light_ink, min(-r.y * 1.6, 1.0))
            }
            env = env * (1.0 + 0.4 * smoothstep(0.05, 0.6, -r.x) - 0.3 * smoothstep(0.05, 0.6, r.x))
            return mix(env, light_ink * 1.08, smoothstep(0.2, 0.95, clamp(r.z, 0.0, 1.0)) * 0.92)
        }

        // The reflection on a part: a dielectric mixes in the sky by its
        // Fresnel; a metal is its reflection, tinted by its ink.
        fn cap_refl(col: vec3, ink: vec3, n: vec3, v: vec3, amt: float, metal: float, ground: vec3, light_ink: vec3) -> vec3 {
            if amt < 0.001 {
                return col
            }
            let fres = 0.04 + 0.96 * pow(1.0 - max(dot(n, v), 0.0), 5.0)
            let sky = cap_sky(cap_reflect(-v, n), ground, light_ink)
            return mix(col, sky * mix(vec3(1.0, 1.0, 1.0), ink, metal), mix(fres, 1.0, metal) * amt)
        }

        // A wall of a channel, shaded by how it faces the light, darker
        // toward the floor.
        fn cap_wall(floor_ink: vec3, g: vec2, lq: vec2, t: float, light_ink: vec3, shadow_ink: vec3) -> vec3 {
            let facing = dot(-g, lq)
            let c = floor_ink * mix(shadow_ink, vec3(1.0, 1.0, 1.0), clamp(0.65 + 0.35 * facing, 0.0, 1.0)) * (1.0 - 0.3 * t)
            return c + light_ink * (max(facing, 0.0) * 0.16 * (1.0 - t))
        }

        // The halo of a lit fill from `lo` to `hi`: the capsule outline,
        // round at the ends whatever the slot's corners, since light keeps
        // no square corner.
        fn cap_glow_at(ab: vec2, lo: float, hi: float, mid: float, tw: float) -> float {
            let fc = vec2((lo + hi) * 0.5, mid)
            let fhs = vec2(max(hi - lo, 0.0) * 0.5, tw * 0.5)
            let d = max(cap_rbox(ab - fc, fhs, min(fhs.x, fhs.y)).x, 0.0)
            return exp(-d / max(tw * 0.8, 1.5)) * 0.45 + exp(-d / max(tw * 4.0, 5.0)) * 0.09
        }

        // Only the fill still visible beside the cap glows.
        fn cap_glow_vis(ab: vec2, lo: float, hi: float, at: float, hx: float, mid: float, tw: float) -> float {
            var g = 0.0
            let h1 = min(hi, at - hx)
            if h1 > lo + 0.5 {
                g = max(g, cap_glow_at(ab, lo, h1, mid, tw))
            }
            let l2 = max(lo, at + hx)
            if hi > l2 + 0.5 {
                g = max(g, cap_glow_at(ab, l2, hi, mid, tw))
            }
            return g
        }

        // Where the mark runs: its distance across the line in .x, along the
        // line in .y, and the half extent of the other side in .z.
        fn cap_mark_frame(q: vec2, hs: vec2, mark_at: vec4) -> vec3 {
            if mark_at.x > 0.5 {
                return vec3(q.y, q.x, hs.x)
            }
            return vec3(q.x - (mark_at.y * 2.0 - 1.0) * hs.x, q.y, hs.y)
        }

        // CURVES.

        // A curve through `n` anchors (2..8) at `x` 0..1: height in .x, slope
        // in .y (d height / d x). Anchors are `p0..p7`; `o` the out handle
        // and `i` the in handle of each, relative to it, so a segment runs
        // p_k, p_k + o_k, p_k+1 + i_k+1, p_k+1. Pad the unused anchors with
        // the last one. As the bench's baked curves: the height is held to
        // 0..1 and the slope is zero outside 0..1.
        curve: fn(x: float, n: float, p0: vec2, o0: vec2, i1: vec2, p1: vec2, o1: vec2, i2: vec2, p2: vec2, o2: vec2, i3: vec2, p3: vec2, o3: vec2, i4: vec2, p4: vec2, o4: vec2, i5: vec2, p5: vec2, o5: vec2, i6: vec2, p6: vec2, o6: vec2, i7: vec2, p7: vec2) -> vec2 {
            let xc = clamp(x, 0.0, 1.0)
            var a = p0
            var ao = o0
            var bi = i1
            var b = p1
            if n > 2.5 && xc > p1.x {
                a = p1
                ao = o1
                bi = i2
                b = p2
            }
            if n > 3.5 && xc > p2.x {
                a = p2
                ao = o2
                bi = i3
                b = p3
            }
            if n > 4.5 && xc > p3.x {
                a = p3
                ao = o3
                bi = i4
                b = p4
            }
            if n > 5.5 && xc > p4.x {
                a = p4
                ao = o4
                bi = i5
                b = p5
            }
            if n > 6.5 && xc > p5.x {
                a = p5
                ao = o5
                bi = i6
                b = p6
            }
            if n > 7.5 && xc > p6.x {
                a = p6
                ao = o6
                bi = i7
                b = p7
            }
            let r = cap_bez(xc, a, ao, bi, b)
            return vec2(r.x, r.y * step(0.0, x) * step(x, 1.0))
        }

        // A symmetric curve read at `abs(q.x) / hs.x` (its centre to one
        // end), turned into the whole curve's reading at `(q.x / hs.x + 1) / 2`:
        // `s` is `q.x`, the side the point is on.
        mirror: fn(c: vec2, s: float) -> vec2 {
            return vec2(c.x, c.y * 2.0 * cap_sgn(s))
        }

        // Where the edge curve is read: the depth in from the outline over
        // the edge width (`edge_share` of the smaller half size).
        edge_x: fn(d: float, hs: vec2, edge_share: float) -> float {
            return -d / max(edge_share * min(hs.x, hs.y), 0.5)
        }

        // Where the tooth curve is read: 0 at a rib's crest to 1 at its
        // trough, tiled along the rib band, or round a round cap (a puck) by
        // the angle.
        rib_x: fn(q: vec2, hs: vec2, rib: vec4) -> float {
            var u = 0.0
            if abs(hs.x - hs.y) < 0.02 * hs.x {
                u = (atan2(q.y, q.x) / 6.2831853 + 0.5) * rib.x
            } else {
                let t = (q.x / hs.x + 1.0) * 0.5
                u = (t - rib.z) / max(rib.w - rib.z, 0.01) * rib.x
            }
            return 1.0 - abs(fract(u) * 2.0 - 1.0)
        }

        // THE FOOTPRINT. Each returns the distance (negative inside) in .x
        // and the outward gradient in .yz.

        // A rounded box of radius `r` cut by the plan curve (`plan` the plan
        // curve's reading along the length, vec2(1.0, 0.0) for none).
        sd_cap: fn(q: vec2, hs: vec2, r: float, plan: vec2) -> vec3 {
            let b = cap_rbox(q, hs, r)
            let p = cap_plan(q, hs, plan)
            if p.x > b.x {
                return p
            }
            return b
        }

        // The plan curve's distance alone.
        plan_dist: fn(q: vec2, hs: vec2, plan: vec2) -> float {
            return cap_plan(q, hs, plan).x
        }

        // A drop of liquid of rest radius `radius`: round at rest, spread 8%
        // by `press`, stretched by `s` 0..1 toward `dir` (head minus tail, a
        // unit vector in the cap's frame) with a tail merged on smoothly,
        // squashed along by `sqa` and widened across by `sqx` as it lands.
        sd_drop: fn(q: vec2, radius: float, s: float, dir: vec2, press: float, sqa: float, sqx: float) -> vec3 {
            let big = radius * (1.0 + 0.08 * press)
            let m = clamp(s, 0.0, 1.0)
            let v = vec2(-dir.y, dir.x)
            let rh = big * (1.0 - 0.26 * m)
            let ax = vec2(rh * max(sqa, 0.3), rh * max(sqx, 0.3))
            let qe = vec2(dot(q, dir), dot(q, v)) / ax
            let le = max(length(qe), 0.0001)
            var d = (le - 1.0) * min(ax.x, ax.y)
            let ge = qe / (ax * le)
            // The bias only decides the gradient at the exact centre, where
            // it is zero and would otherwise normalise to NaN.
            var g = normalize(dir * (ge.x + 0.000001) + v * ge.y)
            if m > 0.002 {
                let rt = big * (1.0 - 0.73 * m)
                let ct = -dir * (1.69 * big * m)
                let dt = cap_uneven(q, ct, 0.85 * rh, rt)
                let cp = ct * clamp(dot(q, ct) / max(dot(ct, ct), 0.0001), 0.0, 1.0)
                let gt = normalize(q - cp + vec2(0.0001, 0.0))
                let sm = cap_smin(d, dt, 0.7 * big)
                d = sm.x
                g = normalize(mix(gt, g, sm.y))
            }
            return vec3(d, g.x, g.y)
        }

        // THE SOLID: the cap's height 0..1 at `q` in .x and its gradient per
        // point in .yz, from the footprint `fp` and the curves' readings
        // (height, slope): `along` as a whole curve's, `across`, `edge` and
        // `tooth` (vec2(1.0, 0.0) with no ribs). The along and across
        // profiles multiply and meet the edge profile in a smooth minimum of
        // width `blend`, whose gradient is the same mix; then the ribs, cut
        // into the top in proportion to the height so the walls keep their
        // shape; then a grooved, split or embossed mark, soft-walled over a
        // point; then the cut, which takes the lower of itself and the top.
        // .w is spare (0).
        field: fn(q: vec2, hs: vec2, fp: vec3, along: vec2, across: vec2, edge: vec2, tooth: vec2, shape: vec4, rib: vec4, mark: vec4, mark_at: vec4, cut: vec4) -> vec4 {
            let mn = min(hs.x, hs.y)
            let ew = max(shape.y * mn, 0.5)
            let ga = vec2(along.y * 0.5 / hs.x, 0.0)
            let gc = vec2(0.0, across.y * cap_sgn(q.y) / hs.y)
            let ge = fp.yz * (-edge.y / ew)
            let k = max(shape.z, 0.002)
            let m1 = along.x * across.x
            let g1 = ga * across.x + gc * along.x
            let h2 = clamp(0.5 + 0.5 * (edge.x - m1) / k, 0.0, 1.0)
            var m = mix(edge.x, m1, h2) - k * h2 * (1.0 - h2)
            var g = mix(ge, g1, h2)
            if rib.x >= 1.0 && rib.y > 0.001 {
                if abs(hs.x - hs.y) < 0.02 * hs.x {
                    // Round a puck: by the angle, over a band of radii.
                    let r = max(length(q), 0.001)
                    let f = fract((atan2(q.y, q.x) / 6.2831853 + 0.5) * rib.x)
                    let w = smoothstep(rib.z, max(rib.w, rib.z + 0.01), r / mn)
                    let depth = (1.0 - tooth.x) * rib.y * w
                    let dtri = (1.0 - 2.0 * step(0.5, f)) * 2.0 * rib.x / 6.2831853
                    let dcut = vec2(-q.y, q.x) * (-tooth.y * dtri * rib.y * w / (r * r))
                    g = g * (1.0 - depth) - dcut * m
                    m = m * (1.0 - depth)
                } else {
                    // Along the cap: over the band from `rib.z` to `rib.w`.
                    let span = max(rib.w - rib.z, 0.01)
                    let t = (q.x / hs.x + 1.0) * 0.5
                    let f = fract((t - rib.z) / span * rib.x)
                    let w = step(rib.z - 0.002, t) * step(t, rib.w + 0.002)
                    let depth = (1.0 - tooth.x) * rib.y * w
                    let dtri = (1.0 - 2.0 * step(0.5, f)) * 2.0 * rib.x / (span * 2.0 * hs.x)
                    let dcut = -tooth.y * dtri * rib.y * w
                    g = g * (1.0 - depth) - vec2(dcut, 0.0) * m
                    m = m * (1.0 - depth)
                }
            }
            if mark.x > 4.5 || (mark.x > 1.5 && mark.x < 3.5) {
                // A split runs to the base; a groove and an emboss take the
                // mark's depth.
                var depth = mark.z
                if mark.x > 2.5 && mark.x < 3.5 {
                    depth = 1.0
                }
                let mw = mark.y * 0.5
                let fr = cap_mark_frame(q, hs, mark_at)
                let ss = cap_sstep(mw - 0.6, mw + 0.6, abs(fr.x))
                let wn = cap_sstep(mark_at.z * fr.z - 0.8, mark_at.z * fr.z + 0.8, abs(fr.y))
                let win = 1.0 - wn.x
                let gm = (1.0 - ss.x) * win
                let dgm = -ss.y * cap_sgn(fr.x) * win
                let dgo = -(1.0 - ss.x) * wn.y * cap_sgn(fr.y)
                var gv = vec2(dgm, dgo)
                if mark_at.x > 0.5 {
                    gv = vec2(dgo, dgm)
                }
                if mark.x > 4.5 {
                    g = g + gv * depth
                    m = m + depth * gm
                } else {
                    g = g * (1.0 - depth * gm) - gv * (m * depth)
                    m = m * (1.0 - depth * gm)
                }
            }
            if cut.x > 0.5 {
                let cr = cut.z * hs.x
                let dx = abs(q.x) - cut.y * hs.x
                var dy = 0.0
                if cut.x > 1.5 {
                    dy = q.y
                }
                let rr = dx * dx + dy * dy
                if rr < cr * cr {
                    let sr = sqrt(cr * cr - rr)
                    let hh = max(shape.w, 0.5)
                    let hc = cut.w - sr / hh
                    if hc < m {
                        m = hc
                        g = vec2(dx * cap_sgn(q.x), dy) / (hh * max(sr, 0.05))
                    }
                }
            }
            return vec4(m, g.x, g.y, 0.0)
        }

        // The perspective of a tall cap: the top comes toward the viewer,
        // magnified by its height. Read the solid at the returned point
        // instead of `q`, three times: first at `q / (1.0 + persp)`, then
        // twice at `perspective(q, f.x, fp.x, persp)` of the pass before.
        perspective: fn(q: vec2, h: float, d: float, persp: float) -> vec2 {
            return q / (1.0 + persp * clamp(h, 0.0, 1.0) * step(d, 0.0))
        }

        // FRAMES.

        // The light in the fader's frame (for everything that works in `ab`)
        // from the screen light `l`; `vert` 1 for a vertical fader.
        light: fn(l: vec3, vert: float) -> vec3 {
            let n = normalize(l)
            return vec3(mix(n.xy, vec2(-n.y, n.x), vert), n.z)
        }

        // The screen normal of the solid from its gradient `g` (field .yz)
        // and its height in points.
        normal: fn(g: vec2, height: float, vert: float) -> vec3 {
            let gh = g * height
            return normalize(vec3(-mix(gh, vec2(gh.y, -gh.x), vert), 1.0))
        }

        // The screen view for the specular and the reflection: `near` 0 is a
        // far eye straight above, 1 an eye close over each cap.
        view: fn(q: vec2, hs: vec2, near: float, vert: float) -> vec3 {
            let qs = mix(q, vec2(q.y, -q.x), vert)
            let hss = mix(hs, vec2(hs.y, hs.x), vert)
            return normalize(vec3(-(qs / hss) * near, 1.0))
        }

        // SHADING. Screen space: the normal from `normal`, `l` the screen
        // light, `v` from `view`.

        // The lit cap: one key light, wrapped; the shadow ink is the shade;
        // the height as occlusion, so a wall's foot sits in shade; a plain
        // sky reflection (none below 0.001), tinted by the body for a metal;
        // a highlight through the roughness; a gloss sheen; a rim toward
        // grazing that fades on a dark ground. `h` the height 0..1, `paint`
        // and `lit` the mark's coverage (`mark_line`) for a painted and a lit
        // mark, `open` 1 in the open and less where a well's walls hide the
        // sky (`1 - clamp((depth - h * height) / depth, 0, 1)`).
        shade: fn(n: vec3, l: vec3, v: vec3, h: float, base: vec3, paint: float, lit: float, open: float, spec: float, rough: float, gloss: float, rim: float, ao: float, ao_reach: float, reflection: float, metal: float, li: float, mark_ink: vec3, light_ink: vec3, shadow_ink: vec3, ground: vec3) -> vec3 {
            let ndl = dot(n, l)
            let diff = clamp((ndl + 0.3) / 1.3, 0.0, 1.0) * li
            let occ = 1.0 - ao * pow(1.0 - clamp(h, 0.0, 1.0), max(ao_reach, 0.2))
            let ink = mix(base, mark_ink, paint)
            var col = ink * mix(shadow_ink * occ, vec3(1.0, 1.0, 1.0), diff)
            let hv = normalize(l + v)
            let rg = clamp(rough, 0.02, 1.0)
            let shin = mix(6.0, 220.0, (1.0 - rg) * (1.0 - rg))
            let ndh = max(dot(n, hv), 0.0)
            let sp = pow(ndh, shin) * spec * li * (1.0 + metal * 2.0)
            let sheen = pow(ndh, 3.0) * gloss * 0.5
            col = cap_refl(col, ink, n, v, reflection * mix(0.35, 1.0, open) * (1.0 - paint * 0.5), metal, ground, light_ink)
            col = col + light_ink * (mix(vec3(1.0, 1.0, 1.0), ink, metal) * sp + vec3(sheen, sheen, sheen))
            let glum = dot(ground, vec3(0.3, 0.59, 0.11))
            col = col + light_ink * (rim * pow(1.0 - max(n.z, 0.0), 3.0) * 0.6 * max(0.0, 1.0 - ndl * 0.5) * open * (0.3 + 0.7 * glum))
            return mix(col, mark_ink * 1.15, lit)
        }

        // Turned metal: a brightness offset about -0.7..0.7, a fan along the
        // light round a round cap, fine streaks across a long one. `lq` the
        // light in the fader's frame (`lt.xy`). Lay it on with `in_light` of
        // its positive part times 0.55 and `in_shadow` of its negative part
        // times 0.42, scaled by the spun amount and kept off the paint.
        spun: fn(q: vec2, hs: vec2, lq: vec2) -> float {
            if abs(hs.x - hs.y) < 0.02 * hs.x {
                let r = length(q)
                if r < 0.001 {
                    return 0.0
                }
                let c = dot(q / r, normalize(lq + vec2(0.000001, 0.000001)))
                let fan = c * c
                let a = atan2(q.y, q.x)
                let grain = sin(a * 97.0) * 0.5 + sin(a * 233.0 + 1.7) * 0.3 + sin(a * 541.0 + 0.4) * 0.2
                return (fan * 2.0 - 1.0) * 0.6 + grain * 0.12 * (0.5 + 0.5 * fan)
            }
            let y = q.y
            return (sin(y * 3.1) * 0.5 + sin(y * 7.3 + 1.7) * 0.3 + sin(y * 17.1 + 0.4) * 0.2) * 0.1
        }

        // `c` under a shadow of strength `a` 0..1: multiplied toward the
        // shadow ink.
        in_shadow: fn(c: vec3, a: float, shadow_ink: vec3) -> vec3 {
            return c * mix(vec3(1.0, 1.0, 1.0), shadow_ink, clamp(a, 0.0, 1.0))
        }

        // `c` under a light of strength `a` 0..1: screened with the light
        // ink.
        in_light: fn(c: vec3, a: float, light_ink: vec3) -> vec3 {
            let k = clamp(a, 0.0, 1.0)
            return vec3(1.0, 1.0, 1.0) - (vec3(1.0, 1.0, 1.0) - c) * (vec3(1.0, 1.0, 1.0) - light_ink * k)
        }

        // A body that emits (the neon drop): the cap's colour taken toward
        // its own ink, brighter, by `strength`, more on the top.
        emit: fn(col: vec3, h: float, body: vec3, light_ink: vec3, strength: float) -> vec3 {
            return mix(col, body * 1.4 + light_ink * 0.15, strength * (0.55 + 0.45 * clamp(h, 0.0, 1.0)))
        }

        // Where a painted or lit mark lies: 1 on the line over its extent.
        // The bench paints kind 1 and lights kinds 4 and 5 with it. Read it
        // at the cap's own `q`, not the perspective point.
        mark_line: fn(q: vec2, hs: vec2, mark: vec4, mark_at: vec4, px: float) -> float {
            let aa = px * 0.5
            let fr = cap_mark_frame(q, hs, mark_at)
            let win = 1.0 - smoothstep(mark_at.z * fr.z - aa, mark_at.z * fr.z + aa, abs(fr.y))
            return (1.0 - smoothstep(mark.y * 0.5 - aa, mark.y * 0.5 + aa, abs(fr.x))) * win
        }

        // The halo round a lit mark (kinds 4 and 5), over the cap: add
        // `mark_ink * mark_glow(...) * (1.0 - lit)`.
        mark_glow: fn(q: vec2, hs: vec2, mark: vec4, mark_at: vec4) -> float {
            let fr = cap_mark_frame(q, hs, mark_at)
            return exp(-max(abs(fr.x) - mark.y * 0.5, 0.0) / max(mark.w, 0.5)) * 0.55
        }

        // A lit fill lights the cap's end walls beside it, the lower end
        // from the fill below the cap and the upper from the fill above,
        // only as far as that fill is still visible. `g` the height gradient
        // in points (field .yz times the height), `lo`, `hi` the fill, `at`
        // the cap's centre along the track, `tw` the track's width. For a lit
        // fill only; add `fill_ink * fill_light(...) * (1.0 - paint)`.
        fill_light: fn(q: vec2, hs: vec2, g: vec2, lo: float, hi: float, at: float, tw: float) -> float {
            let low_vis = clamp((min(hi, at - hs.x) - lo) / max(tw * 2.0, 2.0), 0.0, 1.0)
            let high_vis = clamp((hi - max(lo, at + hs.x)) / max(tw * 2.0, 2.0), 0.0, 1.0)
            let nl = normalize(vec3(-g.x, -g.y, 1.0))
            let reach = max(hs.x * 0.55, 1.0)
            let low = low_vis * exp(-(q.x + hs.x) / reach) * (0.35 + 0.65 * max(-nl.x, 0.0))
            let high = high_vis * exp(-(hs.x - q.x) / reach) * (0.35 + 0.65 * max(nl.x, 0.0))
            return (low + high) * 0.6
        }

        // A cap sunk in a well: what lies below the well's lip is in the
        // well's ambient shade, the wall toward the light throws its shadow
        // on it, and the walls beside it darken it. `z` the cap's surface
        // height at the pixel in points (h times the height), `wbox`, `wr`
        // and `depth` the well's.
        in_well: fn(col: vec3, ab: vec2, z: float, wbox: vec4, wr: float, depth: float, lt: vec3, ao: float, inner: float, shadow_ink: vec3) -> vec3 {
            let below = clamp((depth - z) / max(depth, 0.001), 0.0, 1.0)
            let amb = below * (0.35 + 0.5 * ao)
            let ioff = -lt.xy / max(lt.z, 0.25) * max(depth - z, 0.0) * 0.7
            let dsh = cap_rbox(ab - wbox.xy - ioff, wbox.zw, wr).x
            let reach = max(depth - z, 0.5)
            let sh = smoothstep(-reach * 0.6, reach * 0.15, dsh) * step(z, depth) * inner
            let dw = max(wbox.w - abs(ab.y - wbox.y), 0.0)
            let side = below * exp(-dw / max(depth * 0.5, 1.0)) * (0.3 + 0.5 * ao)
            return mix(col, col * shadow_ink, clamp(amb + sh * 0.9 + side, 0.0, 1.0))
        }

        // The cap's own coverage: the bench's one-pixel smoothstep.
        cover: fn(d: float, px: float) -> float {
            return cap_cover(d, px)
        }

        // The output: exposure (a linear factor, 2 to the EV), then the
        // highlights rolled off instead of clipped, what passes white in one
        // channel spilling into the others. `rolloff` 0 leaves the colour.
        tone: fn(col: vec3, exposure: float, rolloff: float) -> vec3 {
            let hc = col * exposure
            let ro = clamp(rolloff, 0.0, 1.0)
            if ro < 0.001 {
                return hc
            }
            let mx = max(max(hc.x, hc.y), hc.z)
            let sp = max(mx - 1.0, 0.0) * 0.6 * ro
            let lifted = hc + vec3(sp, sp, sp)
            let k = mix(1.0, 0.72, ro)
            let over = max(lifted - vec3(k, k, k), vec3(0.0, 0.0, 0.0))
            return min(lifted, vec3(k, k, k)) + (vec3(1.0, 1.0, 1.0) - exp(-over / max(1.0 - k, 0.001))) * (1.0 - k)
        }

        // The fader laid over a panel of colour `ground` without painting
        // the panel: premultiplied, the light it adds over the ground and the
        // share of the ground it darkens away. Both colours toned alike.
        on_ground: fn(col: vec3, ground: vec3) -> vec4 {
            let lighter = max(col - ground, vec3(0.0, 0.0, 0.0))
            let dark = max(ground - col, vec3(0.0, 0.0, 0.0)) / max(ground, vec3(0.01, 0.01, 0.01))
            return vec4(lighter, clamp(max(dark.x, max(dark.y, dark.z)), 0.0, 1.0))
        }

        // THE GROUND, in the fader's frame; `lt` from `light`.

        // The cast shadow's strength 0..1 at `q`: the footprint (a rounded
        // box of radius `r`; a drop at rest is the circle of its radius)
        // extruded to its top outline `top` and thrown away from the light
        // by `rise` (the height standing above the surround: the cap's
        // height less a well's depth it is sunk in), sharp at the foot,
        // blurred up to `blur` and thinned toward the tip. `top` is the
        // bench's top outline, printed with its Splash: the half length and
        // half width as shares of the base's, the corner as a share of the
        // smaller; vec3(1.0, 1.0, r / min(hs.x, hs.y)) throws the footprint
        // itself. Lay it under the cap, times 0.45 for glass.
        shadow: fn(q: vec2, hs: vec2, r: float, top: vec3, lt: vec3, rise: float, blur: float, strength: float, px: float) -> float {
            let c = cap_cast(q, hs, r, top, lt, rise, blur, px)
            return strength * (1.0 - smoothstep(-c.y * 0.35, c.y, c.x)) * mix(1.0, 0.3, c.z)
        }

        // The contact shadow hugging the cap's outline `d`. The bench's
        // reach is max(rise * 0.3, 1); in a well, max(that, depth * 0.45),
        // and the strength min(strength * 1.6, 1).
        contact: fn(d: float, reach: float, strength: float) -> float {
            return strength * exp(-max(d, 0.0) / max(reach, 0.001))
        }

        // The glow a body that emits throws on the ground: it falls off from
        // the cap's own outline `d`, never from the shadow, which carries the
        // light's offset. Add `body * glow(...) * 0.9` outside the cap.
        glow: fn(d: float, hs: vec2, strength: float) -> float {
            return strength * exp(-max(d, 0.0) / max(min(hs.x, hs.y) * 0.8, 1.0))
        }

        // A sunk well's floor (its coverage) in .x and its wall band in .y:
        // what `slot` and `tube` take as `sunk` and `wall`, and what clips
        // the shadow of a cap sunk in it. Both 0 for a raised bezel, which
        // neither lights the fill's halo nor holds a cap.
        well_mask: fn(ab: vec2, wbox: vec4, wr: float, depth: float, px: float) -> vec2 {
            if depth < 0.0 {
                return vec2(0.0, 0.0)
            }
            let wb = cap_rbox(ab - wbox.xy, wbox.zw, wr)
            let wcov = cap_cover(wb.x, px)
            let dd = max(depth, 0.5)
            let www = min(dd * 0.3, wbox.w * 0.25)
            return vec2(wcov, wcov * (1.0 - smoothstep(www - px * 0.5, www + px * 0.5, -wb.x)))
        }

        // A well: a channel `depth` points deep sunk into the ground, its
        // floor in the well ink under the shadow of the wall toward the light
        // and the occlusion of every wall, the walls shaded by how they face
        // the light, the far lip lit; or, for a negative depth, a bezel
        // raised on the ground, casting outward, its edge lit toward the
        // light and shaded away from it. `inner` the inner shadow, `blur` and
        // `strength` the cast shadow's.
        well: fn(col: vec3, ab: vec2, wbox: vec4, wr: float, depth: float, lt: vec3, px: float, well_ink: vec3, light_ink: vec3, shadow_ink: vec3, inner: float, ao: float, blur: float, strength: float) -> vec3 {
            let wb = cap_rbox(ab - wbox.xy, wbox.zw, wr)
            let wcov = cap_cover(wb.x, px)
            let dd = max(abs(depth), 0.5)
            if depth >= 0.0 {
                let ioff = -lt.xy / max(lt.z, 0.25) * dd * 0.7
                let dsh = cap_rbox(ab - wbox.xy - ioff, wbox.zw, wr).x
                let ins = smoothstep(-dd * 1.4, dd * 0.25, dsh) * max(inner, 0.0)
                let occ = exp(wb.x / max(dd * 0.9, 0.5)) * ao
                let floorc = mix(well_ink, well_ink * shadow_ink, clamp(ins * 0.95 + occ * 0.8, 0.0, 1.0))
                var c = mix(col, floorc, wcov)
                let www = min(dd * 0.3, wbox.w * 0.25)
                let on_wall = wcov * (1.0 - smoothstep(www - px * 0.5, www + px * 0.5, -wb.x))
                c = mix(c, cap_wall(well_ink, wb.yz, lt.xy, clamp(-wb.x / max(www, 0.01), 0.0, 1.0), light_ink, shadow_ink), on_wall)
                let lip = (1.0 - smoothstep(0.0, px * 1.6, wb.x)) * (1.0 - wcov)
                let lip_lit = max(dot(-wb.yz, lt.xy), 0.0)
                let lip_dark = max(dot(wb.yz, lt.xy), 0.0)
                c = mix(c, c * shadow_ink, lip * lip_dark * 0.35)
                return c + light_ink * (lip * lip_lit * 0.45)
            }
            let off = -lt.xy / max(lt.z, 0.25) * dd * 0.7
            let offl = max(length(off), 0.001)
            let th = cap_height(ab - wbox.xy, wbox.zw, off / offl, offl)
            let rr = clamp(wr, 0.0, min(wbox.z, wbox.w))
            let dsh = cap_sweep(ab - wbox.xy - off * 0.5, max(wbox.zw - vec2(rr, rr), vec2(0.0, 0.0)), off * 0.5) - rr
            let bl = max(clamp(blur, max(dd * 0.35, 1.0), max(dd, 1.0)) * mix(0.15, 1.0, th), px)
            let sh = strength * (1.0 - smoothstep(-bl * 0.35, bl, dsh)) * (1.0 - wcov) * mix(1.0, 0.3, th)
            var c = mix(col, col * shadow_ink, sh)
            let ed = clamp(-wb.x / max(dd * 0.6, px), 0.0, 1.0)
            let edge_lit = max(-dot(wb.yz, lt.xy), 0.0)
            let edge_dark = max(dot(wb.yz, lt.xy), 0.0)
            var top = well_ink * (1.0 + (1.0 - ed) * edge_lit * 0.5)
            top = mix(top, top * shadow_ink, (1.0 - ed) * edge_dark * 0.5)
            return mix(c, top, wcov)
        }

        // THE TRACKS. `tbox` the track's centre and half size, `tr` its end
        // radius (the ends share times the half width), `lo`..`hi` the value
        // fill along the track (the caller's: from the fill's start, or the
        // track's inset end when the fill starts at zero, to the value; the
        // whole track for a lit-end-to-end fill), `fill` its kind (0 none,
        // 1 bar, 2 lit, 3 the whole track lit). `at` and `hx` are the cap's
        // centre and half length along the track, so a lit fill's halo
        // shows only beside the cap; `sunk` and `wall` a well's `well_mask`.

        // A slot cut into the ground: dark, the wall toward the light
        // shading its floor, every wall occluding the floor beside it, the
        // far lip lit. A bar fill lies on the slot's floor and takes its
        // shadow; a lit one is its own light and glows round itself.
        slot: fn(col: vec3, ab: vec2, tbox: vec4, tr: float, fill: float, lo: float, hi: float, at: float, hx: float, lt: vec3, px: float, sunk: float, wall: float, track_ink: vec3, fill_ink: vec3, light_ink: vec3, shadow_ink: vec3, inner: float, ao: float) -> vec3 {
            let tw = tbox.w * 2.0
            let tb = cap_rbox(ab - tbox.xy, tbox.zw, tr)
            let tcov = cap_cover(tb.x, px)
            let sdepth = max(tw * 0.6, 1.0)
            let sww = min(sdepth * 0.3, tw * 0.25)
            let ioff = -lt.xy / max(lt.z, 0.25) * sdepth * 0.7
            let dsh = cap_rbox(ab - tbox.xy - ioff, tbox.zw, tr).x
            let ins = smoothstep(-sdepth * 1.2, sdepth * 0.25, dsh) * max(inner, 0.0)
            let socc = exp(tb.x / max(sdepth * 0.8, 0.5)) * ao
            let dim = clamp(ins * 0.85 + socc * 0.7, 0.0, 1.0)
            let lip = (1.0 - smoothstep(0.0, px * 1.5, tb.x)) * (1.0 - tcov)
            let lip_lit = max(dot(-tb.yz, lt.xy), 0.0)
            var c = mix(col, mix(track_ink, track_ink * shadow_ink, dim), tcov)
            c = c + light_ink * (lip * lip_lit * 0.3)
            let on_wall = tcov * (1.0 - smoothstep(sww - px * 0.5, sww + px * 0.5, -tb.x))
            c = mix(c, cap_wall(track_ink, tb.yz, lt.xy, clamp(-tb.x / max(sww, 0.01), 0.0, 1.0), light_ink, shadow_ink), on_wall)
            let wall_lit = max(wall, on_wall)
            if fill > 0.5 {
                let fcov = step(lo, ab.x) * step(ab.x, hi)
                var fl = mix(fill_ink, fill_ink * shadow_ink, dim)
                if fill > 1.5 {
                    fl = fill_ink * 1.2
                }
                let fin = 1.0 - smoothstep(-px * 0.5, px * 0.5, tb.x + sww + max(px, 1.0))
                c = mix(c, fl, fin * fcov)
                if fill > 1.5 {
                    let gl = cap_glow_vis(ab, lo, hi, at, hx, tbox.y, tw) * (1.0 - fin * fcov) * (1.0 + max(sunk, tcov) * 0.9 + wall_lit * 0.6)
                    c = min(c + fill_ink * gl, max(c, fl * 0.85))
                }
            }
            return c
        }

        // Rails: two raised lines either side of the centre, a bar fill
        // between them.
        rails: fn(col: vec3, ab: vec2, tbox: vec4, fill: float, lo: float, hi: float, px: float, track_ink: vec3, fill_ink: vec3, light_ink: vec3, shadow_ink: vec3) -> vec3 {
            let tw = tbox.w * 2.0
            let rw = max(tw * 0.12, px)
            let off = abs(ab.y - tbox.y)
            let dr = abs(off - tw * 0.5)
            let rail = 1.0 - smoothstep(rw * 0.5 - px * 0.5, rw * 0.5 + px * 0.5, dr)
            let inl = step(tbox.x - tbox.z, ab.x) * step(ab.x, tbox.x + tbox.z)
            var c = mix(col, mix(track_ink, light_ink, 0.35), rail * inl)
            c = mix(c, track_ink * shadow_ink, (1.0 - smoothstep(rw * 0.5 + px * 0.5, rw + px * 1.5, dr)) * inl * 0.35 * (1.0 - rail))
            if fill > 0.5 {
                let fcov = step(lo, ab.x) * step(ab.x, hi)
                c = mix(c, fill_ink, (1.0 - smoothstep(tw * 0.5 - rw * 0.5 - px, tw * 0.5 - rw * 0.5, off)) * fcov * inl)
            }
            return c
        }

        // A flush line: no depth at all, a bar fill over it.
        flush: fn(col: vec3, ab: vec2, tbox: vec4, tr: float, fill: float, lo: float, hi: float, px: float, track_ink: vec3, fill_ink: vec3) -> vec3 {
            let line = cap_cover(cap_rbox(ab - tbox.xy, tbox.zw, tr).x, px)
            let c = mix(col, track_ink, line)
            if fill > 0.5 {
                return mix(c, fill_ink, line * step(lo, ab.x) * step(ab.x, hi))
            }
            return c
        }

        // A lit tube: the track glows where it is filled, whatever the fill
        // kind, with a halo beside the cap.
        tube: fn(col: vec3, ab: vec2, tbox: vec4, tr: float, lo: float, hi: float, at: float, hx: float, px: float, sunk: float, wall: float, track_ink: vec3, fill_ink: vec3) -> vec3 {
            let line = cap_cover(cap_rbox(ab - tbox.xy, tbox.zw, tr).x, px)
            let fcov = step(lo, ab.x) * step(ab.x, hi)
            var c = mix(col, track_ink, line)
            c = mix(c, fill_ink * 1.25, line * fcov)
            let gl = cap_glow_vis(ab, lo, hi, at, hx, tbox.y, tbox.w * 2.0) * (1.0 - line * fcov) * (1.0 + sunk * 0.9 + wall * 0.6)
            return min(c + fill_ink * gl, max(c, fill_ink * (1.25 * 0.85)))
        }

        // A rod: a half-round bar standing on the ground, shaded as a
        // cylinder in the track ink or, where filled, the fill ink, with its
        // own shadow beside it. `li` and `spec` the light's intensity and
        // the specular.
        rod: fn(col: vec3, ab: vec2, tbox: vec4, tr: float, lo: float, hi: float, lt: vec3, li: float, spec: float, px: float, strength: float, track_ink: vec3, fill_ink: vec3, light_ink: vec3, shadow_ink: vec3) -> vec3 {
            let tb = cap_rbox(ab - tbox.xy, tbox.zw, tr)
            let tcov = cap_cover(tb.x, px)
            let fcov = step(lo, ab.x) * step(ab.x, hi)
            let rr = tbox.w
            let off = clamp((ab.y - tbox.y) / max(rr, 0.001), -0.999, 0.999)
            let hz = sqrt(1.0 - off * off)
            let nr = normalize(vec3(0.0, -off / hz, 1.0))
            let diff = clamp((dot(nr, lt) + 0.3) / 1.3, 0.0, 1.0) * li
            let hv = normalize(lt + vec3(0.0, 0.0, 1.0))
            let sp = pow(max(dot(nr, hv), 0.0), 40.0) * spec * li
            let rodc = mix(track_ink, fill_ink, fcov) * mix(shadow_ink, vec3(1.0, 1.0, 1.0), diff) + light_ink * sp
            let roff = -lt.xy / max(lt.z, 0.25) * rr * 0.6
            let roffl = max(length(roff), 0.001)
            let rth = cap_height(ab - tbox.xy, tbox.zw, roff / roffl, roffl)
            let rc = clamp(tr, 0.0, min(tbox.z, tbox.w))
            let rsh = cap_sweep(ab - tbox.xy - roff * 0.5, max(tbox.zw - vec2(rc, rc), vec2(0.0, 0.0)), roff * 0.5) - rc
            let cast = strength * (1.0 - smoothstep(-rr * 0.3, max(rr * 0.9 * mix(0.15, 1.0, rth), px), rsh)) * (1.0 - tcov) * mix(1.0, 0.3, rth)
            let c = mix(col, col * shadow_ink, cast * 0.8)
            return mix(c, rodc, tcov)
        }

        // The scale: `count` ticks over the travel from `foot`, every fifth
        // long, `tick_len` points, on the far side (`side` 1), the near side
        // (2), both (3), or both inside the well (4). They stand clear of the
        // track (`track_w` its width) and of a well (`well_w` its width, 0
        // for none) by three pixels.
        ticks: fn(col: vec3, ab: vec2, mid: float, foot: float, travel: float, count: float, side: float, tick_len: float, track_w: float, well_w: float, px: float, tick_ink: vec3) -> vec3 {
            if count < 1.0 || side < 0.5 {
                return col
            }
            let f = (ab.x - foot) / max(travel, 1.0) * count
            let fi = floor(f + 0.5)
            let da = abs(ab.x - (foot + fi / count * travel))
            let major = 1.0 - step(0.3, abs(fract(fi / 5.0 + 0.5) - 0.5))
            let tl = tick_len * mix(0.6, 1.0, major)
            let inr = step(-0.5, fi) * step(fi, count + 0.5)
            var gap = max(track_w * 0.5, well_w * 0.5) + 3.0 * px
            if well_w > 0.0 && side > 3.5 {
                gap = track_w * 0.5 + 3.0 * px
            }
            let off = ab.y - mid
            let far = step(gap, off) * step(off, gap + tl)
            let near = step(gap, -off) * step(-off, gap + tl)
            var s = far
            if side > 2.5 {
                s = max(far, near)
            } else {
                if side > 1.5 {
                    s = near
                }
            }
            let tk = (1.0 - smoothstep(px * 0.49, px * 0.85, da)) * s * inr
            return mix(col, tick_ink, tk * 0.85)
        }

        // GLASS. Only a glass cap calls these.

        // How far the scene under a glass cap is read from: bent by its
        // surface (`n` from `normal` with `vert` 0: the fader's frame) by
        // `depth` points and magnified by `magnify`, over the cap's
        // coverage `cov`. Add it to `ab` and draw the ground again there.
        glass_disp: fn(q: vec2, n: vec3, depth: float, magnify: float, cov: float) -> vec2 {
            return (-n.xy * depth - q * magnify) * cov
        }

        // Where the red (.xy) and the blue (.zw) samples of the ground are
        // read, as displacements like `glass_disp`'s: split along the bend
        // by the chromatic aberration and spread by the roughness (frost).
        glass_split: fn(disp: vec2, hs: vec2, chroma: float, rough: float, cov: float) -> vec4 {
            let ca = max(chroma, 0.04)
            let fr = clamp(rough, 0.0, 1.0) * min(hs.x, hs.y) * 0.3 * cov
            var gd = vec2(0.0, 1.0)
            if dot(disp, disp) > 0.0001 {
                gd = normalize(disp)
            }
            let gp = vec2(-gd.y, gd.x) * fr
            let red = disp * (1.0 + ca) + gd * (fr * 0.5) + gp
            let blue = disp * (1.0 - ca) - gd * (fr * 0.5) - gp
            return vec4(red.x, red.y, blue.x, blue.y)
        }

        // The three samples of the ground made one: frosted to their mean,
        // and split into their channels as far as the glass is clear.
        glass_mix: fn(red: vec3, mid: vec3, blue: vec3, chroma: float, rough: float) -> vec3 {
            let ca = max(chroma, 0.04)
            let frost = (red + mid + blue) / 3.0
            return mix(frost, vec3(red.x, mid.y, blue.z), clamp(chroma / max(ca, 0.001), 0.0, 1.0) * 0.7 * (1.0 - clamp(rough, 0.0, 1.0)))
        }

        // The glass cap in place of `shade`: the ground seen through it
        // (`bg`) tinted by the body, clouded by the roughness; the plain sky
        // reflected outside by the Fresnel (`outer`) and inside off the back
        // (`inner`), both times `reflection`; a highlight, a sheen and a
        // rim toward the light; the `lens` highlight (from `lens`, 0 for
        // none), which goes under the mark. Screen space.
        glass_shade: fn(n: vec3, l: vec3, v: vec3, bg: vec3, body: vec3, tint: float, spec: float, rough: float, gloss: float, rim: float, li: float, reflection: float, outer: float, inner: float, lens: float, paint: float, lit: float, mark_ink: vec3, light_ink: vec3, ground: vec3) -> vec3 {
            let fres = pow(1.0 - max(n.z, 0.0), 3.0)
            let hh = normalize(l + vec3(0.0, 0.0, 1.0))
            let ndh = max(dot(n, hh), 0.0)
            let sp = pow(ndh, mix(140.0, 8.0, clamp(rough, 0.0, 1.0))) * 0.9 + pow(ndh, 10.0) * 0.1
            let sheen = pow(ndh, 3.0) * gloss * 0.5
            let nn = n.xy / max(length(n.xy), 0.0001)
            let rimv = fres * (0.55 + 0.45 * dot(nn, l.xy)) * (0.4 + 0.6 * li) * mix(0.15, 1.0, rim)
            let rg = clamp(rough, 0.02, 1.0)
            let f = 0.04 + 0.96 * fres
            let glass = mix(mix(bg, body, tint), light_ink * 0.85, rg * 0.25)
            let outc = cap_sky(cap_reflect(-v, n), ground, light_ink) * (f * reflection * outer)
            let ni = normalize(vec3(-n.x, -n.y, n.z))
            let fi = pow(1.0 - max(n.z, 0.0), 2.0)
            let inc = cap_sky(cap_reflect(-v, ni), ground, light_ink) * mix(vec3(1.0, 1.0, 1.0), body, 0.5) * (fi * 0.35 * reflection * inner)
            let add = sp * spec * 0.4 * outer + sheen + rimv * 0.6
            var col = glass * (1.0 - f * 0.6 * outer) + outc + inc + light_ink * (add + lens)
            col = mix(col, mark_ink, paint)
            return mix(col, mark_ink * 1.15, lit)
        }

        // The lens highlight: a soft spot of light focused through the cap,
        // moved from the light by a third of the rise; `glass_shade` takes
        // it. `depth` the glass depth, `strength` the caustic's.
        lens: fn(q: vec2, hs: vec2, lt: vec3, rise: float, depth: float, li: float, strength: float) -> float {
            let offg = -lt.xy / max(lt.z, 0.25) * max(rise, 0.0) * 0.35
            let eu = (q - offg) / max(hs * 0.62, vec2(1.0, 1.0))
            let k = 0.5 + 0.5 * clamp(depth / 10.0, 0.0, 1.0)
            return exp(-dot(eu, eu) * 2.5) * 0.3 * k * li * strength
        }

        // The caustic: the light the glass focuses into a bright rim just
        // inside its cast shadow, on the side away from the light, its
        // colours split by the chromatic aberration. Takes the shadow's own
        // arguments; add `light_ink * caustic(...)` outside the cap.
        caustic: fn(q: vec2, hs: vec2, r: float, top: vec3, lt: vec3, rise: float, blur: float, px: float, chroma: float, depth: float, rough: float, li: float, strength: float) -> vec3 {
            if rise <= 0.5 || strength <= 0.001 {
                return vec3(0.0, 0.0, 0.0)
            }
            let c = cap_cast(q, hs, r, top, lt, rise, blur, px)
            let off = -lt.xy / max(lt.z, 0.25) * rise * 0.7
            let od = off / c.w
            let mn = min(hs.x, hs.y)
            let k = 0.5 + 0.5 * clamp(depth / 10.0, 0.0, 1.0)
            let far = smoothstep(-0.05, 0.65, dot(normalize(q + vec2(0.001, 0.001)), od))
            let wc = c.w * 0.45 + 0.5
            let ws = max(max(c.w * 0.3, 1.5), c.y * 0.45)
            let dc = clamp(chroma, 0.0, 0.4) / 0.05 * mn * 0.04
            let er = (c.x + wc - dc) / ws
            let eg = (c.x + wc) / ws
            let eb = (c.x + wc + dc) / ws
            let fall = exp(-max(length(q) - mn, 0.0) / (mn * 2.5))
            let frost = 1.0 - 0.6 * clamp(rough, 0.0, 1.0)
            return vec3(exp(-er * er), exp(-eg * eg), exp(-eb * eb)) * (far * fall * frost * 0.75 * k * li * strength)
        }
    }
}

#[cfg(test)]
mod tests {
    use makepad_widgets_core::makepad_platform::*;
    use makepad_widgets_core::makepad_platform::makepad_script::script_eval;

    /// Every function in the engine in one pixel function, drawn as a
    /// sheet's fader would draw a cap, which the compile test below builds
    /// for each backend.
    fn probe(vm: &mut ScriptVm) {
        makepad_widgets_core::makepad_draw::script_mod(vm);
        crate::script_mod(vm);
        script_eval!(vm, {
            use mod.pod.*
            use mod.math.*
            let Cap = mod.sdf.Cap
            mod.draw.CapProbe = mod.draw.DrawQuad{
                pixel: fn() {
                    let ab = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let vert = 0.0
                    let mid = self.rect_size.y * 0.5
                    let at = self.rect_size.x * 0.5
                    let hs = vec2(12.0, 6.0)
                    let q = ab - vec2(at, mid)
                    let l = normalize(vec3(-0.3, -0.6, 0.72))
                    let lt = Cap.light(l, vert)
                    let shape = vec4(0.3, 0.3, 0.02, 9.0)
                    let rib = vec4(6.0, 0.14, 0.06, 0.94)
                    let mark = vec4(2.0, 1.5, 0.3, 4.0)
                    let mark_at = vec4(0.0, 0.5, 0.9, 0.0)
                    let cut = vec4(1.0, 0.6, 0.9, 2.25)
                    let r = shape.x * min(hs.x, hs.y)
                    let z = vec2(0.0, 0.0)
                    let e = vec2(1.0, 1.0)
                    let pl = Cap.curve(abs(q.x) / hs.x, 2.0, vec2(0.0, 1.0), vec2(0.33, 0.0), vec2(-0.33, 0.0), e, z, z, e, z, z, e, z, z, e, z, z, e, z, z, e, z, z, e)
                    let plan = Cap.mirror(pl, q.x)
                    let w = Cap.perspective(q, 1.0, -1.0, 0.1)
                    var fp = Cap.sd_cap(w, hs, r, plan)
                    let drop = Cap.sd_drop(q, 6.0, 0.3, vec2(1.0, 0.0), 0.5, 1.0, 1.0)
                    fp = vec3(min(fp.x, drop.x), fp.y, fp.z)
                    let along = Cap.curve((w.x / hs.x + 1.0) * 0.5, 3.0, vec2(0.0, 0.0), vec2(0.1, 0.5), vec2(-0.1, 0.0), vec2(0.5, 1.0), vec2(0.1, 0.0), vec2(-0.1, 0.5), vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0))
                    let across = Cap.curve(abs(w.y) / hs.y, 2.0, vec2(0.0, 1.0), vec2(0.2, 0.0), vec2(-0.1, 0.3), vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0))
                    let edge = Cap.curve(Cap.edge_x(fp.x, hs, shape.y), 2.0, z, vec2(0.0, 0.5523), vec2(-0.5523, 0.0), e, z, z, e, z, z, e, z, z, e, z, z, e, z, z, e, z, z, e)
                    let tooth = Cap.curve(Cap.rib_x(w, hs, rib), 3.0, vec2(0.0, 1.0), vec2(0.15, 0.0), vec2(-0.15, 0.2), vec2(0.5, 0.5), vec2(0.15, -0.2), vec2(-0.15, 0.0), vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0), z, z, vec2(1.0, 0.0))
                    let f = Cap.field(w, hs, fp, along, across, edge, tooth, shape, rib, mark, mark_at, cut)
                    let n = Cap.normal(f.yz, shape.w, vert)
                    let v = Cap.view(q, hs, 0.35, vert)
                    let ground = vec3(0.2, 0.2, 0.22)
                    let body = vec3(0.8, 0.8, 0.82)
                    let ink = vec3(1.0, 1.0, 1.0)
                    let dark = vec3(0.3, 0.3, 0.35)
                    let paint = Cap.mark_line(q, hs, mark, mark_at, px)
                    let wbox = vec4(at, mid, at - 4.0, 8.0)
                    let wm = Cap.well_mask(ab, wbox, 8.0, 5.0, px)
                    var col = Cap.well(ground, ab, wbox, 8.0, 5.0, lt, px, dark, ink, dark, 0.7, 0.5, 8.0, 0.6)
                    col = Cap.well(col, ab, wbox, 8.0, -3.0, lt, px, dark, ink, dark, 0.7, 0.5, 8.0, 0.6)
                    let tbox = vec4(at, mid, at - 8.0, 2.5)
                    col = Cap.slot(col, ab, tbox, 2.5, 2.0, 8.0, at, at, hs.x, lt, px, wm.x, wm.y, dark, body, ink, dark, 0.7, 0.5)
                    col = Cap.rails(col, ab, tbox, 1.0, 8.0, at, px, dark, body, ink, dark)
                    col = Cap.flush(col, ab, tbox, 1.5, 1.0, 8.0, at, px, dark, body)
                    col = Cap.tube(col, ab, tbox, 1.5, 8.0, at, at, hs.x, px, wm.x, wm.y, dark, body)
                    col = Cap.rod(col, ab, tbox, 2.5, 8.0, at, lt, 0.9, 0.3, px, 0.6, dark, body, ink, dark)
                    col = Cap.ticks(col, ab, mid, 8.0, self.rect_size.x - 16.0, 10.0, 3.0, 5.0, 5.0, 16.0, px, ink)
                    let disp = Cap.glass_disp(q, Cap.normal(f.yz, shape.w, 0.0), 9.0, 0.1, Cap.cover(fp.x, px))
                    let split = Cap.glass_split(disp, hs, 0.05, 0.1, 1.0)
                    let bg = Cap.glass_mix(col + vec3(split.x, split.y, 0.0), col, col + vec3(split.z, split.w, 0.0), 0.05, 0.1)
                    var cap = Cap.shade(n, l, v, f.x, body, paint, 0.0, 1.0, 0.3, 0.2, 0.3, 0.4, 0.5, 1.2, 0.3, 0.5, 0.95, ink, ink, dark, ground)
                    let lens = Cap.lens(q, hs, lt, 9.0, 9.0, 0.9, 0.6)
                    cap = mix(cap, Cap.glass_shade(n, l, v, bg, body, 0.18, 1.0, 0.1, 0.6, 0.4, 0.9, 0.8, 1.0, 0.6, lens, paint, 0.0, ink, ink, ground), 0.5)
                    let sp = Cap.spun(q, hs, lt.xy)
                    cap = Cap.in_shadow(Cap.in_light(cap, sp * 0.55, ink), -sp * 0.42, dark)
                    cap = Cap.emit(cap, f.x, body, ink, 0.2)
                    cap = Cap.in_well(cap, ab, f.x * shape.w, wbox, 8.0, 5.0, lt, 0.5, 0.7, dark)
                    cap = cap + body * Cap.fill_light(q, hs, f.yz * shape.w, 8.0, at, at, 5.0) + ink * Cap.mark_glow(q, hs, mark, mark_at)
                    let cov = Cap.cover(fp.x, px)
                    let top = vec3(0.7, 0.8, 0.5)
                    let sh = Cap.shadow(q, hs, r, top, lt, 9.0, 8.0, 0.6, px) + Cap.contact(fp.x, 2.7, 0.5)
                    col = mix(col, col * dark, clamp(sh, 0.0, 1.0) * (1.0 - cov))
                    col = col + ink * Cap.caustic(q, hs, r, top, lt, 9.0, 8.0, px, 0.05, 9.0, 0.1, 0.9, 0.6) * (1.0 - cov)
                    col = col + body * Cap.glow(fp.x, hs, 0.5) * 0.9 * (1.0 - cov)
                    col = Cap.tone(mix(col, cap, cov), 1.0, 0.6)
                    let pd = Cap.plan_dist(q, hs, plan)
                    return Cap.on_ground(col * step(-100.0, pd), ground)
                }
            }
        });
    }

    const FUNCTIONS: [&str; 41] = [
        "curve", "mirror", "edge_x", "rib_x",
        "sd_cap", "plan_dist", "sd_drop", "field", "perspective",
        "light", "normal", "view",
        "shade", "spun", "in_shadow", "in_light", "emit", "mark_line", "mark_glow", "fill_light", "in_well",
        "cover", "tone", "on_ground",
        "shadow", "contact", "glow", "well_mask", "well",
        "slot", "rails", "flush", "tube", "rod", "ticks",
        "glass_disp", "glass_split", "glass_mix", "glass_shade", "lens", "caustic",
    ];

    fn compiled(vm: &mut ScriptVm, value: ScriptValue) -> String {
        vm.bx.heap_ref().string_with(value, |_heap, text| text.to_string())
            .expect("the compiler answers with source")
    }

    /// The engine compiles on every backend the shader compiler writes, and
    /// each function reaches the emitted source.
    #[test]
    fn every_cap_function_compiles_on_every_backend() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            probe(vm);
            for (backend, value) in [
                ("metal", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.CapProbe, "metal", false)})),
                ("hlsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.CapProbe, "hlsl", false)})),
                ("glsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.CapProbe, "glsl", false)})),
                ("wgsl", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.draw.CapProbe, "wgsl", false)})),
            ] {
                let text = compiled(vm, value);
                assert!(!text.starts_with("ERRORS"), "{backend} did not compile: {text}");
                for name in FUNCTIONS {
                    assert!(text.contains(&format!("_{name}(")), "{backend}: {name} is missing from:\n{text}");
                }
            }
        });
    }
}
