//! The city: a voxel-grid city seen from the street, ray-marched block by block (the page's `city3`
//! and `shadeCity`) once per view into two RGBA16F targets in one pass (colour + alpha, light + the
//! depth of the nearest surface in metres), then blitted into the scene list between the props behind
//! it and the props in front of it. Props hidden by the city (`hideC`) read its depth through
//! `SkyCityDepth` and [`bind_city`]. The march is recorded again only when what it reads changes.
use crate::params::{Params, P};
use crate::render::{bind_texture, SceneCtx, SceneTarget};
use makepad_widgets::makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // One ray of the march, the page's globals: origin, direction (never 0), 1 / direction, edge-distance factors, depth of the
    // block tested, rad per px, entry axis of the last box, the block's three nearest surfaces (t, coverage, part id). Its
    // primitives return (t of the first surface, pixel coverage from the signed edge distance).
    let CityRay = struct {
        o: vec3, d: vec3, i: vec3, g: vec3, td: float, px: float, ax: float, sa: vec3, sb: vec3, sc: vec3
        at: fn(o: vec3, rd: vec3, px: float) -> Self {
            let d = (2.0 * step(vec3(0.0), rd) - vec3(1.0)) * max(abs(rd), vec3(0.0002))
            let ab = abs(d); let a2 = ab * ab; let none = vec3(1e9, 0.0, 0.0)
            return self(o: o, d: d, i: vec3(1.0) / d, g: ab.xxy * ab.yzz / sqrt(a2.xxy + a2.yzz), td: 1.0, px: px, ax: 0.0, sa: none, sb: none, sc: none)
        }
        // `xbox`: box at c, half size h; sets `ax` (1 entered through a z-facing wall)
        xbox: fn(c: vec3, h: vec3) -> vec2 {
            let q = (self.o - c) * self.i; let k = abs(self.i) * h; let t1 = -q - k; let t2 = k - q
            let tn = max(max(t1.x, t1.y), t1.z); let tf = min(min(t2.x, t2.y), t2.z)
            self.ax = step(max(t1.x, t1.y), t1.z)
            let e = min((t2.yzx - t1) * self.g.xzy, (t2.zxy - t1) * self.g.yxz)
            return vec2(tn, clamp(0.5 + min(min(e.x, e.y), e.z) / (self.td * self.px), 0.0, 1.0) * step(0.0, tf))
        }
        // `xcyl`: vertical cylinder of radius r about c.xz from y0 to y1
        xcyl: fn(c: vec3, r: float, y0: float, y1: float) -> vec2 {
            let oc = self.o.xz - c.xz; let d = self.d.xz; let a = dot(d, d); let b = dot(oc, d)
            let hd2 = max(dot(oc, oc) - b * b / a, 0.0); let hd = sqrt(hd2); let tc = -b / a; let sq = sqrt(max(r * r - hd2, 0.0) / a)
            let s1 = tc - sq; let s2 = tc + sq; let ya = (y0 - self.o.y) * self.i.y; let yb = (y1 - self.o.y) * self.i.y
            let yn = min(ya, yb); let yf = max(ya, yb); let dy = abs(self.d.y)
            let dn = abs(vec2(dot(d, oc + d * s1), dot(d, oc + d * s2))) / r; let g = max(dn / (sqrt(dn * dn + dy * dy) + 1e-6), vec2(0.35))
            let m = min(r - hd, min((yf - s1) * dy * g.x, (s2 - yn) * dy * g.y))
            return vec2(max(s1, yn), clamp(0.5 + m / (self.td * self.px), 0.0, 1.0) * step(0.0, min(s2, yf)))
        }
        // `xdome`: the upper half of a sphere of radius r resting at c
        xdome: fn(c: vec3, r: float) -> vec2 {
            let oc = self.o - c; let cx = cross(oc, self.d); let hd2 = dot(cx, cx)
            let t = -dot(oc, self.d) - sqrt(max(r * r - hd2, 0.0)); let m = min(r - sqrt(hd2), (self.o.y + self.d.y * t - c.y) * 0.8)
            return vec2(t, clamp(0.5 + m / (self.td * self.px), 0.0, 1.0) * step(0.0, t))
        }
        // `xmast`: a thin vertical mast of height h and width w on p
        xmast: fn(p: vec3, h: float, w: float) -> vec2 {
            let oc = p.xz - self.o.xz; let d = self.d.xz; let a = dot(d, d); let tc = dot(oc, d) / a
            let hd = abs(oc.x * d.y - oc.y * d.x) / sqrt(a); let y = self.o.y + self.d.y * tc - p.y; let tp = max(tc, 0.01) * self.px
            let wm = min(w, tp); let cv = clamp(((max(w, tp) + wm) * 0.5 - hd) / max(wm, 1e-5), 0.0, 1.0) * min(1.0, w / tp)
            return vec2(tc, cv * clamp(y / tp + 0.5, 0.0, 1.0) * clamp((h - y) / tp + 0.5, 0.0, 1.0) * step(0.0, tc))
        }
        // `ins`: keeps the block's three nearest surfaces (a roof-line pixel can need a third)
        ins: fn(v: vec2, id: float) {
            if v.y > 0.012 {
                let s = vec3(v.x, v.y, id)
                if v.x < self.sa.x { self.sc = self.sb; self.sb = self.sa; self.sa = s } else if v.x < self.sb.x { self.sc = self.sb; self.sb = s } else if v.x < self.sc.x { self.sc = s }
            }
        }
    }

    // The march's targets as a scene shader reads them, at the fragment's own azimuth and raw elevation (mirrored in the
    // reflection run): `city_col` premultiplied colour + alpha, `city_lit` light + depth (m). Bound by `bind_city`.
    mod.draw.SkyCityDepth = {
        city_col: texture_2d(float)
        city_lit: texture_2d(float)
        // the page's `prCityHide`: the share of the pixel hidden by a city surface nearer than d m
        city_hide: fn(d: float) -> float {
            let uv = self.up_uv()
            if d > self.city_lit.sample_nearest(uv).w { return self.city_col.sample_nearest(uv).w }
            return 0.0
        }
    }

    // The march (the page's `city3`): fb0 premultiplied body colour + alpha, fb1 window / beacon light + depth (m). 1 block is
    // 70 m and 20 floors, the eye 0.09 above the street; the walk finds the next lot the ray can meet, then its hull and parts.
    // At Size k a block is k x 70 m: the buildings (and the eye) are k times as large and as many over k a row, while the
    // city keeps its place in metres (its distance, the camera's offset, its profile and districts) and its haze by distance.
    mod.draw.DrawSkyCity = mod.std.set_type_default() do #(DrawSkyCity::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        alpha_blend: false
        ct_h: uniform(1.0), ct_d: uniform(0.7), ct_w: uniform(0.3), win_d: uniform(0.45), lights: uniform(3.0), temp: uniform(3200.0), size: uniform(1.0)
        // the page's `box1`: a box-filtered pulse on [a, b] over footprint w; wide footprints fall back to its mean
        box1: fn(f: float, a: float, b: float, w0: float) -> float {
            let w = max(w0, 0.004)
            return mix(clamp((f - a) / w + 0.5, 0.0, 1.0) * clamp((b - f) / w + 0.5, 0.0, 1.0), b - a, smoothstep(0.7, 1.6, w))
        }
        fragment: fn(){
            let vr = self.view_raw(); let u = vr.x; let ang = vr.y; let px = vr.z; let hz = self.haze_at(u); let tau = 6.2831853; let cf = 0.05; let sz = self.size; let cey = 0.09
            // the sun in the march's world frame (x, up, z; azimuth u along (cos, sin) of x, z), and the glass's sky
            let sw = vec3(cos(self.sun_el) * cos(self.sun_u * tau), sin(self.sun_el), cos(self.sun_el) * sin(self.sun_u * tau)); let sky10 = self.sky_at(u, 0.1745329)
            let dd = self.city_d() / sz; let r0 = 0.5 * dd; let r1 = 1.55 * dd; let tf = 6.0 + 44.0 * self.ct_h; let tf2 = tf - 2.0
            var hcap = tf * cf + 0.9
            let aw = vec2(step(0.999, self.ct_w), smoothstep(0.0, 0.02, self.ct_w))
            let hl = cos(ang); let oh = self.city_cam() / sz; let dh = vec2(cos(u * tau), sin(u * tau)); let tan_a = sin(ang) / hl
            var ray = CityRay.at(vec3(oh.x, cey, oh.y), vec3(hl * dh.x, sin(ang), hl * dh.y), px); let sg = sign(ray.d)
            // the ray's horizontal range inside the city band, clipped to heights that can hold a building and to the street
            let bo = dot(oh, dh); let co = dot(oh, oh); let ds = bo * bo - (co - r1 * r1)
            var s0 = max(-bo - sqrt(max(ds, 0.0)), 0.0); var s1 = -bo + sqrt(max(ds, 0.0))
            if ds < 0.0 || self.ct_w < 0.001 { s1 = -1.0 }
            if co < r0 * r0 { s0 = max(s0, -bo + sqrt(bo * bo - (co - r0 * r0))) }
            // the city's shape: a sector about downtown, tallest in the middle, thinning to low blocks at its rim
            let dc = self.city_dir(); let hsec = self.ct_w * 0.5; let al = (hsec + 0.045) * tau; let m28 = max(self.city_d() / 28.0, 1.0); let mh = max(hsec, 0.02)
            if self.off < 0.001 {
                // centred camera: every block on the ray has the ray's own azimuth, so the fall-off toward the rim bounds the heights
                let dz = abs(fract(u - self.ct_c + 0.5) - 0.5)
                hcap = min(hcap, (2.0 + tf2 * mix(0.15, 1.0, 1.0 - 0.74 * smoothstep(0.0, 1.0, min(dz * m28 / mh, 1.0)))) * cf + 0.9)
                if dz > hsec + 0.045 { s1 = -1.0 }
            }
            if tan_a > 1e-5 { s1 = min(s1, (hcap - cey) / tan_a) } else if tan_a < -1e-5 { s1 = min(s1, cey / -tan_a) }
            if al < 1.5 {
                // a narrow sector is convex: clip the ray to both of its edges (1.5 blocks of margin)
                for e in 0..2 {
                    let sd = 1.0 - 2.0 * f32(e); let b = vec2(cos(self.ct_c * tau - sd * al), sin(self.ct_c * tau - sd * al))
                    let ea = sd * (b.x * oh.y - b.y * oh.x) + 1.5; let eb = sd * (b.x * dh.y - b.y * dh.x)
                    if eb > 1e-5 { s0 = max(s0, -ea / eb) } else if eb < -1e-5 { s1 = min(s1, -ea / eb) } else if ea < 0.0 { s1 = -1.0 }
                }
            }
            var pc = vec3(0.0); var em = vec3(0.0); var tr = 1.0; var depth = 6e4
            if s0 < s1 {
                let t1 = s1 / hl; var cell = floor(oh + dh * (s0 + 1e-4)); var tm = (cell + max(sg.xz, vec2(0.0)) - oh) * ray.i.xz; let td = abs(ray.i.xz)
                var tcur = s0 / hl; var cos_max = cos(al); if hsec + 0.045 >= 0.5 { cos_max = -2.0 }
                let hw = min(0.45 * self.city_d(), 12.6) / sz; let pd = clamp(0.03 + self.ct_d, 0.0, 0.98) * 1.15
                // real hits, hulls met (24 at most), walk steps left; the candidate lot (cell centre, tall, floors), its hash and padding
                var hits = 0.0; var nb = 0.0; var left = 120.0; var lot = vec4(0.0); var h1 = 0u; var pad = 0.0
                loop {
                    // the walk: the next lot the ray can meet by its height bounds (at most 120 steps a block met)
                    var fnd = false
                    loop {
                        if left <= 0.0 || tcur > t1 || tr < 0.02 { break }
                        left -= 1.0
                        let cc = cell + 0.5; let r2 = dot(cc, cc); var yr = -1.0; if ray.d.y > 0.0 { yr = cey + tcur * ray.d.y }
                        let r = sqrt(r2); let rr = r / dd; let hq = (r - dd) / hw; let hq2 = hq * hq; let hr = 1.0 / (1.0 + hq2 * (1.0 + 0.5 * hq2))
                        if r2 > r0 * r0 && r2 < r1 * r1 && dot(cc, dc) > cos_max * r && yr <= (2.0 + tf2 * mix(0.15, 1.0, hr)) * cf + 0.9 {
                            // the sector's edge, the camera's clear surroundings, the fall-off from downtown, the band's inner edge
                            let da = abs(fract(atan2(cc.y, cc.x) / tau - self.ct_c + 0.5) - 0.5)
                            let sm = smoothstep(vec4(hsec - 0.04, 3.0 / sz, 0.0, 0.5), vec4(hsec + 0.04, 9.0 / sz, 1.0, 0.75), vec4(da, length(cc - oh), min(da * m28 / mh, 1.0), rr))
                            let mc = mix(0.4, 1.0, sm.y); let w = (1.0 - 0.74 * sm.z) * hr; let mw = mix(0.15, 1.0, w)
                            let pmax = pd * max(1.0 - sm.x, aw.x) * aw.y * sm.w * (1.0 - smoothstep(1.2, 1.55, rr)) * sm.y
                            ray.td = max(0.5 * (tcur + min(tm.x, tm.y)), 0.01)
                            if yr <= (2.0 + tf2 * mw) * cf * mc + 0.9 {
                                let hh = self.hcell(cell, 1.0); let h = self.hb4(hh)
                                if h.x < pmax {
                                    // the district noise (smooth 2-channel value noise over ~5 blocks): each row in x, then the rows in y
                                    let q = cc * (0.2 * sz); let qi = floor(q); let f0 = q - qi; let f = f0 * f0 * (3.0 - 2.0 * f0); var a = vec2(0.0); var dn = vec2(0.0)
                                    for k in 0..4 {
                                        let b = self.hb4(self.hcell(qi + vec2(f32(k & 1u), f32(k >> 1u)), 70.0)).xy
                                        if (k & 1u) == 0u { dn = b } else { dn = mix(dn, b, f.x); if k == 1u { a = dn } }
                                    }
                                    dn = mix(a, dn, f.y)
                                    // heavy-tailed heights: most lots are 3-6 floors, towers only toward downtown
                                    let tall = pow(h.y, mix(7.0, 2.4, w)); let hn = (2.0 + tf2 * tall * mw * (0.75 + 0.25 * dn.y)) * cf * mc
                                    let nt = max(3.0, floor(hn / cf + 2.0 * h.z)); var roof = 0.17; if nt >= 28.0 { roof = 0.75 }
                                    if h.x < pmax * mix(0.7, 1.15, dn.x) * 0.8696 && yr <= nt * cf + roof { lot = vec4(cc, tall, nt); h1 = hh; pad = tcur * px * 1.5 + 0.004; fnd = true }
                                }
                            }
                        }
                        if tm.x < tm.y { cell.x += sg.x; tcur = tm.x; tm.x += td.x } else { cell.y += sg.z; tcur = tm.y; tm.y += td.y }
                        if fnd { break }
                    }
                    if !fnd { break }
                    // the lot's shape: a main section, an upper section (box or cylinder), a stepped crown on tall towers
                    let nt = lot.w; let ytop = nt * cf; var roof = 0.17; if nt >= 28.0 { roof = 0.78 }
                    let top_h = ytop + roof; let pad_h = ray.td * px * 1.5 + 0.004
                    let h2 = self.hb4(self.hu(h1 + 1759714724u)); let h3 = self.hb4(self.hu(h1 + 3039394381u)); var h7 = vec4(0.0); if nt >= 26.0 { h7 = self.hb4(self.hu(h1 + 1327217884u)) }
                    let up = step(10.0, nt) * step(h2.w, 0.7); let n1 = up * floor(nt * (0.18 + 0.35 * h3.x)); let cyl = up * step(h3.y, 0.28)
                    let n2 = step(26.0, nt) * step(h7.x, 0.55) * (1.0 - cyl) * (2.0 + floor(5.99 * h7.y)); let n0 = nt - n1 - n2; let s1u = mix(1.0, 0.55 + 0.35 * h3.z, up)
                    let bxz = max(mix(vec2(0.2), vec2(0.4), h2.xy) * mix(1.0, 0.82, smoothstep(0.3, 0.8, lot.z)), vec2(min(0.44, ytop / 14.0)))
                    let ctr = lot.xy + (vec2(h2.z, h3.w) - 0.5) * 2.0 * max(vec2(0.46) - bxz, vec2(0.0)); let crs = 0.55 + 0.3 * h7.z
                    let tb0 = mix(bxz, mix(bxz * s1u, vec2(min(bxz.x, bxz.y) * s1u), cyl), up); let tb = tb0 * mix(1.0, crs, step(0.5, n2))
                    // its hull up to the roof allowance (a miss resumes the walk), then its parts by id: 0 body, 1 upper section, 2 crown,
                    // 3 / 4 roof boxes, 5 dome, 6 mast; + 8 on a z-facing box wall
                    if ray.xbox(vec3(ctr.x, 0.5 * top_h, ctr.y), vec3(bxz.x + pad, 0.5 * top_h + pad, bxz.y + pad)).y <= 0.0 { continue }
                    let h5 = self.hu(h1 + 2135587861u); let hs = self.hb4(h5); let h4 = self.hb4(self.hu(h1 + 458671337u)); let h6 = self.hb4(self.hu(h1 + 625341585u))
                    let ht = self.hb4(self.hu(h5 ^ 1540483477u))
                    ray.sa = vec3(1e9, 0.0, 0.0); ray.sb = ray.sa; ray.sc = ray.sa; let y0 = n0 * cf; let y1 = (n0 + n1) * cf
                    let rm = ray.xbox(vec3(ctr.x, 0.5 * y0, ctr.y), vec3(bxz.x, 0.5 * y0, bxz.y)); ray.ins(rm, 8.0 * ray.ax)
                    var above = 0.0; var mast = 0.0; var hm = 0.0; var ha = 0.0; var off = vec2(0.0); var mp = vec3(0.0)
                    // a full hit low on the main section is a wall: everything else stands inside the box over the main roof
                    if rm.y <= 0.985 || cey + rm.x * ray.d.y >= y0 - 0.01 {
                        if ray.xbox(vec3(ctr.x, 0.5 * (y0 + top_h), ctr.y), vec3(tb0.x + pad_h, 0.5 * (top_h - y0) + pad_h, tb0.y + pad_h)).y > 0.0 {
                            above = 1.0
                            let dome = step(h6.x, 0.05) * step(nt, 14.0) * (1.0 - up)
                            mast = (1.0 - dome) * step(28.0, nt) * step(h6.y, mix(0.12, 0.4, smoothstep(28.0, 90.0, nt)))
                            hm = min((5.0 + 9.0 * h6.z) * cf, 0.25 * ytop); off = (vec2(h4.w, h3.w) - 0.5) * 0.9 * tb; ha = (1.0 + floor(2.99 * h4.z)) * cf * 0.8
                            mp = vec3(ctr.x + (hs.w - 0.5) * 0.3 * tb.x, ytop, ctr.y)
                            if n1 > 0.5 {
                                if cyl > 0.5 { ray.ins(ray.xcyl(vec3(ctr.x, 0.0, ctr.y), tb0.x, y0, y1), 1.0) }
                                else { let r = ray.xbox(vec3(ctr.x, 0.5 * (y0 + y1), ctr.y), vec3(tb0.x, 0.5 * (y1 - y0), tb0.y)); ray.ins(r, 1.0 + 8.0 * ray.ax) }
                            }
                            if n2 > 0.5 { let r = ray.xbox(vec3(ctr.x, 0.5 * (y1 + ytop), ctr.y), vec3(tb.x, 0.5 * (ytop - y1), tb.y)); ray.ins(r, 2.0 + 8.0 * ray.ax) }
                            if dome > 0.5 { ray.ins(ray.xdome(vec3(ctr.x, ytop, ctr.y), min(0.8 * min(bxz.x, bxz.y), 0.16)), 5.0) }
                            else {
                                let fa = tb * (0.25 + 0.25 * h4.xy); let ra = ray.xbox(vec3(ctr.x + off.x, ytop + 0.5 * ha, ctr.y + off.y), vec3(fa.x, 0.5 * ha, fa.y)); ray.ins(ra, 3.0 + 8.0 * ray.ax)
                                let hb = (1.0 + floor(1.99 * hs.z)) * cf * 0.8; let fb = tb * (0.15 + 0.2 * hs.xy)
                                let rb = ray.xbox(vec3(ctr.x - 0.8 * off.x, ytop + 0.5 * hb, ctr.y + 0.6 * off.y), vec3(fb.x, 0.5 * hb, fb.y)); ray.ins(rb, 4.0 + 8.0 * ray.ax)
                            }
                            if mast > 0.5 { ray.ins(ray.xmast(mp, hm, 0.018 + 0.012 * h6.w), 6.0) }
                        }
                    }
                    left = 120.0
                    // a red aircraft-warning light on most towers of 40+ floors: at the mast tip, else on top of roof box A
                    if above > 0.5 && nt >= 40.0 && h6.z > 0.3 {
                        var pb = vec3(ctr.x + off.x, ytop + ha, ctr.y + off.y); if mast > 0.5 { pb = vec3(mp.x, ytop + hm, mp.z) }
                        pb = pb - ray.o; let tb2 = dot(pb, ray.d); let bd = length(pb - ray.d * tb2) / (max(tb2, 0.01) * px)
                        // it burns day and night; a host's is a point (`beacon_core`, its centre's pixel offset from the
                        // fragment's by the azimuth and elevation it stands at from the camera, as the fragment's own view
                        // places it), the example's 1.22 px
                        var bcore = 0.0
                        if self.point_core.x > 0.0 {
                            var bu = atan2(pb.z, pb.x) / tau - u; bu = bu - floor(bu + 0.5)
                            bcore = self.beacon_core(vec2(bu * tau, atan2(pb.y, length(pb.xz)) - ang) / px, px)
                        } else { bcore = exp(-bd * bd / 3.0) }
                        em += tr * vec3(1.0, 0.08, 0.03) * (bcore * step(0.0, tb2) * self.lights * 0.07 * (0.4 + 0.6 * exp(-tb2 * sz * (0.004 + 0.0185 * self.haze))) * self.design_gain)
                        // where no surface covers the pixel yet, its depth is the beacon's own, so the beacon's halo over
                        // the sky burns at its distance (a band's light plane, `at_lit`); a pixel without a surface has no
                        // coverage, so nothing else reads that depth
                        if hits == 0.0 && bd < 5.0 && tb2 > 0.0 { depth = min(depth, tb2 * hl * (70.0 * sz)) }
                    }
                    if ray.sa.y > 0.0 {
                        // the nearest surface, then (where it only partly covers the pixel) the next two: the page's `shadeCity`
                        let gd = normalize(dd * dc - ctr); var cacc = 0.0; var bc = vec3(0.0); var ec = vec3(0.0)
                        for sl in 0..3 {
                            var sv = ray.sa
                            if sl == 1u { sv = ray.sb } else if sl == 2u { sv = ray.sc }
                            let cw = sv.y * (1.0 - cacc)
                            if cw > 0.004 {
                                let t = min(sv.x, 1e3); let zf = step(7.5, sv.z); let id = sv.z - 8.0 * zf; let p = ray.o + ray.d * t; let y = p.y
                                let cyls = step(0.5, id) * step(id, 1.5) * cyl; let rn = normalize(p.xz - ctr + 1e-6)
                                var n = vec3(-sg.x, 0.0, 0.0); if zf > 0.5 { n = vec3(0.0, 0.0, -sg.z) }
                                n = mix(n, vec3(rn.x, 0.0, rn.y), cyls)
                                if id > 5.5 { n = -ray.d } else if id > 4.5 { n = normalize(p - vec3(ctr.x, ytop, ctr.y)) }
                                // night haze by distance only: a tower's base is never lighter than its top
                                let fog = 1.0 - exp(-t * sz * (0.0015 + 0.0075 * self.haze))
                                var yb = 0.0; if id > 2.5 { yb = ytop } else if id > 1.5 { yb = y1 } else if id > 0.5 { yb = y0 }
                                let sy = smoothstep(vec3(0.0, 0.0, 0.05), vec3(0.45, 0.12, 1.8), vec3(y, y - yb, y))
                                // street canyon, the crease where a section meets what it stands on, walls toward downtown, the sky
                                let aog = mix(0.5, 1.0, sy.x); let aos = mix(1.0, mix(0.62, 1.0, sy.y), step(0.5, id)); let face = mix(0.8, 1.0, 0.5 + 0.5 * dot(n.xz, gd))
                                let tnt = vec3(0.92, 0.96, 1.04) * (1.0 + (ht.xyz - 0.5) * 0.35) * (0.55 + 0.95 * hs.z)
                                // by day: the sun on the face hit and the sky's ambient, darker down in the street canyon (aog);
                                // albedo 0.28 the facades, 0.20 the roof boxes and the dome (0.25 here, their body is dimmed by 0.8
                                // below) and the mast, in the tower's tint
                                var alb = 0.28; if id > 5.5 { alb = 0.20 } else if id > 2.5 { alb = 0.25 }
                                let dayc = self.day_lit(vec3(0.92, 0.96, 1.04) * (1.0 + (ht.xyz - 0.5) * 0.35) * alb, aog, max(dot(n, sw), 0.0))
                                let wall = tnt * (hz * 0.17 * face * mix(0.72, 1.0, sy.z) * aos * aog + vec3(1.0, 0.62, 0.32) * self.glow * (0.012 * self.design_gain) / (1.0 + 8.0 * y * y) * aos) + dayc
                                var body = wall; var e = vec3(0.0)
                                if id < 2.5 {
                                    // the facade: whole window cells only (columns divide the wall, half a pier at each corner)
                                    var hv = bxz; if id > 1.5 { hv = bxz * s1u * crs } else if id > 0.5 { hv = bxz * s1u }
                                    var hf = hv.y; var sc = p.z - ctr.y + hv.y; if zf > 0.5 { hf = hv.x; sc = p.x - ctr.x + hv.x }
                                    var ww = 2.0 * hf; if cyls > 0.5 { ww = tau * min(bxz.x, bxz.y) * s1u; sc = (atan2(p.z - ctr.y, p.x - ctr.x) / tau + 0.5) * ww }
                                    // curtain-wall towers: wide glass, thin mullions; solid wall at both ends of the facade
                                    let curt = step(hs.w, 0.3); let cw0 = mix(0.034, 0.062, hs.y) * mix(1.0, 0.8, curt); let mg = clamp(ww * 0.11, 0.9 * cw0, 0.22 * ww)
                                    let wi = max(ww - 2.0 * mg, cw0 * 0.5); let ncol = max(1.0, floor(wi / cw0 + 0.5)); let cwd = wi / ncol
                                    let cs = (sc - mg) / cwd; let ci = clamp(floor(cs), 0.0, ncol - 1.0); let fx = cs - ci
                                    let yf = y / cf; let fi = floor(yf); let fy = yf - fi
                                    let fs = t * px / (cwd * max(abs(dot(n.xz, normalize(ray.d.xz))), 0.12)); let fv = t * px / cf
                                    let wfx = mix(mix(0.5, 0.7, ht.w), 0.9, curt); let wfy = mix(mix(0.46, 0.58, hs.y), 0.7, curt)
                                    var gm = step(mg, sc) * step(sc, ww - mg) * self.box1(fx, 0.5 - 0.5 * wfx, 0.5 + 0.5 * wfx, min(fs, 0.45)) * self.box1(fy, 0.53 - 0.5 * wfy, 0.53 + 0.5 * wfy, min(fv, 0.45))
                                    // on buildings of 8+ floors the section carrying the roof has a blank top floor, and the lobby stays dark
                                    var topid = 0.0; if n2 > 0.5 { topid = 2.0 } else if n1 > 0.5 { topid = 1.0 }
                                    if nt >= 7.5 && ((id == topid && fi >= nt - 1.5) || (id < 0.5 && fi < 0.5)) { gm = 0.0 }
                                    // floors lit in zones of 3, rooms of 1-3 windows (about a pixel when sub-pixel), wings of a third; one pattern a face
                                    let wk = u32(id * 8.0 + step(0.0, n.x + n.z) + 2.0 * zf); let gsz = (1.0 + floor(2.99 * ht.w)) * max(1.0, floor(fs * 0.9))
                                    let hf2 = self.hu(h1 ^ (u32(fi + 64.0) * 2654435761u) ^ (wk * 2246822507u)); let hfl = self.hb4(hf2)
                                    let zone = self.hb4(self.hu(h1 + u32(floor(fi / 3.0) + 64.0) * 668265263u + wk)).x
                                    let hr = self.hb4(self.hu(hf2 + u32(floor(ci / gsz) + 16.0) * 3266489909u))
                                    let wing = self.hb4(self.hu(hf2 + u32(floor(clamp(sc / ww, 0.0, 0.999) * 3.0) + 1.0) * 1821285621u)).x
                                    let pfl = clamp(clamp(self.win_d * (0.3 + 1.4 * hs.x), 0.0, 1.0) * mix(0.35, 1.5, step(zone, 0.5)), 0.0, 1.0); let occ = 0.3 + 0.7 * hfl.z
                                    // walls whose columns cannot be resolved show no lit windows; unresolved floors fade to an even glow
                                    let rf = smoothstep(vec2(0.6, 1.5), vec2(1.2, 4.0), vec2(fs, fv))
                                    let lit = mix(step(hfl.x + 1e-4, pfl * 0.95) * step(wing, 0.5 + 0.5 * occ) * step(hr.x, occ) * (1.0 - rf.x), pfl * 0.54 * occ, rf.y)
                                    // recessed windows: seen at a grazing angle the piers hide the glass
                                    let cosv = abs(dot(n, ray.d)); let reveal = clamp(1.0 - mix(0.3, 0.12, curt) * sqrt(max(1.0 - cosv * cosv, 0.0)) / max(cosv, 1e-3), 0.0, 1.0)
                                    let ea = gm * lit * (0.4 + 0.8 * hr.y) * self.lights * 0.55 * mix(0.55, 1.0, cosv) * mix(1.0, 0.8, curt) * reveal * (self.lit * self.design_gain)
                                    if ea > 0.003 * self.design_gain { e = mix(self.kelvin(mix(self.temp, min(self.temp * 1.9, 12000.0), step(0.62, hfl.y))), vec3(0.78, 1.0, 0.8) * 0.85, step(0.86, hfl.y)) * ea }
                                    // the glass mirrors the sky low over the horizon (10 degrees up; the haze at night), tinted per tower;
                                    // fall-off across the wall, occlusion at its corners
                                    let fc1 = max(1.0 - cosv, 0.0); let fres = 0.15 + 0.85 * fc1 * fc1 * fc1; let refl = 1.0 / (1.0 + 7.0 * max(ray.d.y, 0.0))
                                    let glass = sky10 * (0.04 + 0.34 * fres) * mix(0.35, 1.0, refl) * (0.8 + 0.4 * hr.z) * mix(0.6, 1.0, aog) * vec3(0.85 + 0.3 * ht.y, 1.0, 0.85 + 0.3 * ht.z)
                                    let sw = clamp(sc, 0.0, ww); let cq = vec2(sw, ww - sw); let co2 = vec2(1.0) / (vec2(1.0) + cq * cq * 200.0)
                                    body = mix(wall, glass, gm * mix(0.5, 1.0, reveal)) * (1.0 + 0.1 * (2.0 * sw / ww - 1.0) * sign(gd.y + 1e-4) * (1.0 - cyls)) * (1.0 - 0.1 * (co2.x + co2.y) * (1.0 - cyls))
                                }
                                if id > 2.5 && id < 5.5 { body = body * 0.8 }
                                if id > 5.5 { body = self.sil * 3.0 + dayc }
                                bc += mix(body, hz * 0.7, fog) * cw; ec += e * (1.0 - 0.6 * fog) * cw; cacc += cw
                            }
                            if cacc > 0.985 { break }
                        }
                        pc += tr * bc; em += tr * ec; tr *= 1.0 - cacc
                        // props behind the city's nearest surface are hidden by it (`city_hide`)
                        if hits == 0.0 { depth = min(min(ray.sa.x, 1e6) * hl * (70.0 * sz), 6e4) }
                        hits += 1.0; if hits >= 8.0 { break }
                    }
                    nb += 1.0; if nb >= 24.0 { break }
                }
            }
            // (the march's pass keeps no distance target: its depth is fb1's w)
            self.fb0 = vec4(pc, 1.0 - tr); self.fb1 = vec4(em, depth); self.fb2 = vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    // The march's result in the scene list (and mirrored in the reflection run), at the march's depth, its lights too;
    // under a host's wet floor only its share over the horizon (`under_floor`: what reaches under it stands behind the
    // floor's water, as it stood behind the ground)
    mod.draw.DrawSkyCityBlit = mod.std.set_type_default() do #(DrawSkyCityBlit::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        ..mod.draw.SkyCityDepth,
        fragment: fn(){
            let uv = self.up_uv(); var c = self.city_col.sample_nearest(uv); var lit = self.city_lit.sample_nearest(uv)
            if self.host_wet > 0.0 { let k = 1.0 - self.under_floor(); c = c * k; lit = vec4(lit.xyz * k, lit.w) }
            self.fb0 = c; self.fb1 = vec4(lit.xyz, c.w); self.fb2 = self.at_lit(c.w, lit.w, lit.xyz, lit.w)
        }
    }

    mod.draw.SkyCityBase = #(City::script_component(vm))
    mod.draw.SkyCity = mod.std.set_type_default() do mod.draw.SkyCityBase{
        draw_city: mod.draw.DrawSkyCity{}
        draw_blit: mod.draw.DrawSkyCityBlit{}
    }
}

#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyCity { #[deref] pub draw_super: DrawQuad }
#[derive(Script, ScriptHook)] #[repr(C)] pub struct DrawSkyCityBlit { #[deref] pub draw_super: DrawQuad }

/// The parameters the march reads (with the seed and the view).
const CITY_KEYS: &[P] = &[P::CtH, P::CtD, P::CtW, P::CtC, P::CtR, P::Off, P::OffDir, P::Haze, P::Glow, P::Lights, P::WinD, P::Temp, P::Size];

/// The march's shaders, its two targets and the key of what they hold.
#[derive(Script, ScriptHook)]
pub struct City { #[live] pub draw_city: DrawSkyCity, #[live] pub draw_blit: DrawSkyCityBlit, #[rust] target: Option<SceneTarget>, #[rust] pub key: Option<u64> }

/// The raw elevations (rad) the city can reach, top and bottom: from its tallest roof, mast and beacon over the nearest
/// lot down to the street there. Lots stand beyond the band's inner edge (half downtown's distance) and 3 blocks of the
/// base's clear of the camera; a building keeps within 0.65 blocks of its lot's centre. At Size k a block is k times the
/// base's (in blocks: the distances over k; the eye, k times as high too, still 0.09 above the street).
pub fn reach(p: &Params) -> Option<(f64, f64)> {
    let (d, k) = (28.0 * p[P::CtR], p[P::Size]);
    let near = (0.5 * d - 0.8 * p[P::Off] * d).max(3.0) / k - 0.65;
    (p[P::CtW] >= 0.001).then(|| (((6.0 + 44.0 * p[P::CtH]) * 0.05 + 0.81).atan2(near), -(0.09f64.atan2(near))))
}

/// The city's rows in this view, with four pixels for edges and beacons (flipped in the reflection run).
fn band(sc: &SceneCtx) -> Option<Rect> {
    let ((top, bottom), m) = (reach(sc.p)?, 4.0 * sc.view.px0());
    sc.band(top + m, bottom - m)
}

impl City {
    /// Marches the city into its two targets, a pass the scene pass `consumer` samples; recorded again only when the
    /// march's parameters, the sky's light or the view changed.
    pub fn draw_pass(&mut self, cx: &mut Cx2d, sc: &SceneCtx, consumer: DrawPassId) {
        let v = sc.view;
        let key = sc.p.hash(CITY_KEYS, &[v.u0, v.span, v.w as f64, v.h as f64, v.horizon, f64::from_bits(sc.light.hash())]);
        let target = self.target.get_or_insert_with(|| SceneTarget::new(cx, "skyline_city", true));
        if self.key != Some(key) {
            let city = &mut self.draw_city;
            target.record(cx, v.w, v.h, |cx| if let Some(r) = band(sc) { sc.set_uniforms(cx, &mut city.draw_vars); city.draw_abs(cx, r); });
            self.key = Some(key);
        }
        cx.attach_child_pass(target.pass.draw_pass_id(), consumer, None);
    }

    /// The city in the scene list: the march blitted over what lies behind it (`sc.mirror`: the reflection run).
    pub fn draw_blit(&mut self, cx: &mut Cx2d, sc: &SceneCtx) {
        let Some(r) = band(sc) else { return };
        let vars = &mut self.draw_blit.draw_vars; sc.set_uniforms(cx, vars); bind_city(cx, vars, sc);
        self.draw_blit.draw_abs(cx, r);
    }

    /// The march's targets: colour + alpha, light + depth (m), once there are any; its pass.
    pub fn textures(&self) -> Option<(Texture, Texture)> { self.target.as_ref().map(|t| (t.color.clone(), t.light.clone())) }
    pub fn pass_id(&self) -> Option<DrawPassId> { self.target.as_ref().map(|t| t.pass.draw_pass_id()) }
    /// Lets go of the targets (an export core between jobs).
    pub fn release(&mut self) { (self.target, self.key) = (None, None); }
}

/// Binds the march's targets to a shader with `..mod.draw.SkyCityDepth` (the blit, and props hidden by the city).
pub fn bind_city(cx: &Cx, vars: &mut DrawVars, sc: &SceneCtx) {
    if let Some((col, lit)) = &sc.city { bind_texture(cx, vars, live_id!(city_col), col); bind_texture(cx, vars, live_id!(city_lit), lit); }
}
