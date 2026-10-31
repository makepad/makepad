//! Tower cranes (the page's `pkCranes`), seen with the city camera, each on a city lot the city certainly leaves empty:
//! beyond 1.25 x the downtown distance slot 1, nearer the props layer behind nearer city surfaces. Hammerhead, flat-top
//! or luffing jib slewed in 3D, red lights, a work light; most beside an unfinished concrete frame.
use super::kit::{clamp, hypot, town_td, Ctx, Pt::*, I0, Instance, EYE};
use crate::params::P;
use crate::rng::{fract, mix, smooth, wd, TAU};

pub fn build(c: &mut Ctx) {
    let (am, z, p, zs) = (c.p[P::PrCrane], c.z.crane, c.p, c.z);
    if am <= 0.0 || z[1] <= 0.0 { return; }
    // the city's lots: blocks of bl m (Size times the base's 70), downtown cdk of them out (`city.rs`)
    let (cd, hsec, sz) = (28.0 * p[P::CtR], p[P::CtW] * 0.5, c.k);
    let (dd, bl, cdk) = (cd * 70.0, 70.0 * sz, cd / sz);
    let cam = [0.8 * p[P::Off] * dd * (p[P::OffDir] * TAU).cos(), 0.8 * p[P::Off] * dd * (p[P::OffDir] * TAU).sin()];
    let (hb, hc) = (|h: u32, k: u32| ((h >> (8 * k)) & 255) as f64 * 0.003921569, |x: f64, y: f64, k: i32| zs.rng.hcell(x as i32, y as i32, k));
    let empty = |cx: f64, cy: f64, bu: f64| { // the city never builds on lot (cx, cy)
        let (rr, da) = (hypot(cx + 0.5, cy + 0.5) / cdk, wd(bu, p[P::CtC]).abs());
        let aenv = (1.0 - smooth(hsec - 0.04, hsec + 0.04, da)).max(if p[P::CtW] >= 0.999 { 1.0 } else { 0.0 }) * smooth(0.0, 0.02, p[P::CtW]);
        hb(hc(cx, cy, 1), 0) >= clamp(0.03 + p[P::CtD], 0.0, 0.98) * 1.15 * aenv * (smooth(0.5, 0.75, rr) * (1.0 - smooth(1.2, 1.55, rr)))
    };
    // a world point from the city camera: bearing, distance, the ray and its tangent
    let view = |w: [f64; 2]| { let q = [w[0] - cam[0], w[1] - cam[1]]; let th = q[1].atan2(q[0]); (fract(th / TAU), hypot(q[0], q[1]), [th.cos(), th.sin()], [-th.sin(), th.cos()]) };
    for fi in (0..1.max(((0.5 + 12.0 * am) / sz).floor() as i32)).map(|f| f as f64) { // Size times as large: as many over Size
        let h0 = hc(fi, 0.0, 1300);
        let (r1, r2, r3) = (hb(h0, 0), hb(h0, 1), hb(h0, 2));
        let b = if r1 <= 0.55 { z[0] + if r2 < 0.5 { -1.0 } else { 1.0 } * (hsec + (fract(r2 * 7.31) - 0.6) * 0.05) } else { z[0] + (2.0 * r2 - 1.0) * hsec * 0.85 };
        let rad = dd * if r1 <= 0.55 { mix(0.62, 1.45, r3) } else if r3 < 0.5 { mix(0.58, 0.85, r3 * 2.0) } else { mix(1.15, 1.48, r3 * 2.0 - 1.0) };
        let (near, w) = (rad <= 1.25 * dd, [rad * (b * TAU).cos(), rad * (b * TAU).sin()]);
        if view(w).1 < 150.0 { continue; }
        let cell = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], // the site: here or next door
            [0.0, -1.0]].into_iter().map(|o| [(w[0] / bl).floor() + o[0], (w[1] / bl).floor() + o[1]])
            .find(|&[cx, cy]| { let cm = [(cx + 0.5) * bl, (cy + 0.5) * bl]; empty(cx, cy, b + (w[0] * cm[1] - w[1] * cm[0]) / ((w[0] * w[0] + w[1] * w[1]) * TAU)) });
        let on_land = if wd(b, p[P::LandC]).abs() < p[P::Land] * 0.5 - 0.11 && p[P::Land] > 0.04 { 1.0 } else { c.z.land(fract(b)) };
        let Some(cell) = cell.filter(|_| on_land >= 0.5) else { continue }; // no free lot, or the water
        let (h1, h2, h3, h4) = (hc(fi, 1.0, 1300), hc(fi, 2.0, 1300), hc(fi, 3.0, 1300), hc(fi, 4.0, 1300));
        // the site (m); on it, in size units times Size, the frame and the mast
        let (s, has_f, hx, hz) = ([(cell[0] + 0.5) * bl, (cell[1] + 0.5) * bl], hb(h0, 3) <= 0.65, 10.0 + 13.0 * hb(h1, 0), 10.0 + 13.0 * hb(h1, 1));
        let (nf, f) = ((4.0 + 12.0 * hb(h1, 2).powf(1.4)).floor(), [s[0] + (hb(h1, 3) - 0.5) * (26.0 - hx) * sz, s[1] + (hb(h2, 0) - 0.5) * (26.0 - hz) * sz]);
        let (k4, jt) = ((hb(h2, 1) * 3.99).floor() as usize, hb(h2, 2) - 0.5);
        let m = if has_f { [f[0] + [hx + 3.5, -hx - 3.5, jt * hx, jt * hx][k4] * sz,
            f[1] + [jt * hz, jt * hz, hz + 3.5, -hz - 3.5][k4] * sz] } else { [s[0] + (hb(h0, 1) - 0.5) * 30.0 * sz, s[1] + (hb(h0, 2) - 0.5) * 30.0 * sz] };
        let ty = if hb(h2, 3) < 0.5 { 0 } else if hb(h2, 3) < 0.85 { 1 } else { 2 }; // hammerhead, flat-top, luffing
        let hm = (if has_f { clamp(nf * 3.5 + 16.0 + 14.0 * hb(h3, 0), 38.0, 85.0) } else { 38.0 + 40.0 * hb(h3, 0) }).min(if ty == 2 { 55.0 } else { f64::MAX });
        let (ln, ph, al, at) = (50.0 + 20.0 * hb(h3, 1), hb(h3, 2) * TAU, mix(0.45, 1.1, hb(h3, 3)), 0.25 + 0.65 * hb(h4, 1));
        let (lc, yh, vm) = (0.3 * ln + 2.0, mix(12.0, hm - 3.0, hb(h4, 0)), view(m));
        let (sj, cj) = (ph.cos() * vm.3[0] + ph.sin() * vm.3[1], ph.cos() * vm.2[0] + ph.sin() * vm.2[1]);
        let (sa, ca, sg, mw, yj) = (sj.abs(), cj.abs(), if sj < 0.0 { -1.0 } else { 1.0 }, vm.3[0].abs() + vm.3[1].abs(), hm + 3.0);
        let i0 = Instance { slot: if near { 2 } else { 1 }, view: 1, uc: vm.0, d: vm.1, hide_c: near, glow: 1.5, ..I0 };
        let bx = |c: &mut Ctx, x: f64, y: f64, hx: f64, hy: f64| c.part(Box, [x, y, hx, hy], [0., 0., 1., 0.]);
        // three instances with tight bounds: the mast; the head (cab, jib, counter-jib, A-frame, ties, lights); the hook
        c.begin(i0.clone(), 0.0, 0.0); c.part(Lat, [0., 0., 0., hm], [mw, mw, 2.2, 0.11]); bx(c, 0.0, hm + 1.5, mw + 0.3, 1.5); c.end(0.5, 0.5); // lattice mast, slewing unit
        c.begin(i0.clone(), hm, 0.0); bx(c, sg * (mw + 1.3 * sa), hm + 1.6, 1.3 * sa + 0.9 * ca + 0.2, 1.3); // cab on the jib side
        let (tip, yb, hk);
        if ty < 2 { // horizontal jib, counter-jib with ballast, trolley and hook
            let (jl, cl) = (ln * sa, lc * sa);
            let xt = sg * jl * at;
            c.part(Lat, [-0.3 * sg, yj + 0.85, sg * jl, yj + 0.85], [0.85, 0.85, (2.6 * sa).max(0.05), 0.09]); c.seg([-sg * cl, yj + 0.5, 0.3 * sg, yj + 0.5], 0.45, 0.45, 1.0);
            bx(c, -sg * cl * 0.86, yj - 0.7, 2.6 * sa + 1.2 * ca, 1.3); bx(c, xt, yj - 0.4, 1.2 * sa + 0.6 * ca + 0.2, 0.4); // ballast, trolley
            (tip, hk) = ([sg * jl, yj + 1.7], [xt, yh, yj - 0.4, 0.07]);
            yb = if ty == 0 { // hammerhead: A-frame tower head, pendant ties
                let ah = 8.0 + 3.0 * at;
                c.trap(0.0, yj, 1.0, 1.0, 1.7, 1.0); c.trap(0.0, yj + 1.7, 1.0, 0.3, ah, 1.0);
                c.seg([0., yj + 1.7 + ah, sg * jl * 0.68, yj + 1.7], 0.05, 0.05, 1.0); c.seg([0., yj + 1.7 + ah, -sg * cl * 0.95, yj + 0.9], 0.05, 0.05, 1.0);
                yj + 1.7 + ah + 0.3
            } else { yj + 2.0 };
        } else { // luffing jib, short counter-jib, A-frame with the luffing rope
            let dir = [al.cos() * sj, al.sin()];
            let (ls, cl) = (hypot(dir[0], dir[1]), lc * 0.6 * sa);
            tip = [dir[0] * ln, yj + 0.5 + dir[1] * ln];
            c.part(Lat, [-0.3 * dir[0] / ls, yj + 0.5 - 0.3 * dir[1] / ls, tip[0], tip[1]], [0.75, 0.75, 2.6 * ls, 0.09]); c.seg([-sg * cl, yj + 0.4, 0.3 * sg, yj + 0.4], 0.45, 0.45, 1.0);
            bx(c, -sg * cl * 0.85, yj - 0.5, 2.4 * sa + 1.2 * ca, 1.5); c.trap(0.0, yj, 0.9, 0.3, 9.0, 1.0); c.seg([0., yj + 9.0, tip[0], tip[1]], 0.07, 0.07, 1.0);
            (yb, hk) = (yj + 9.0 + 0.3, [tip[0], yh.min(tip[1] - 6.0), tip[1], 0.06]);
        }
        c.part(Beacon, [0., yb, 0., 0.], [0.; 4]); c.part(Beacon, [tip[0], tip[1] + 0.3, 0., 0.], [0.; 4]); // red lights: top and jib tip
        if hb(h4, 2) <= 0.5 { c.part(Light, [sg * (mw + 3.0 * sa + 0.6), yj - 0.5, 0.3, 1.], [0., 1.2, 0., 1.4]); } // work light under the jib
        c.end(1.0, 1.0);
        c.begin(i0, hk[1] - 1.2, hk[0]); c.seg([hk[0], hk[1], hk[0], hk[2]], hk[3], hk[3], 1.0); bx(c, hk[0], hk[1] - 0.5, 0.45, 0.6); c.end(0.5, 0.5); // hook rope and block
        if !has_f { continue; }
        // the frame: open floors (their slab undersides seen from below), a core shaft a storey ahead, a climbing screen, work lights
        let vf = view(f);
        let (tx, tz, ht) = (vf.3[0].abs(), vf.3[1].abs(), nf * 3.5);
        let (ww, de) = (hx * tx + hz * tz, hx * vf.2[0].abs() + hz * vf.2[1].abs());
        let i1 = Instance { slot: if near { 2 } else { 1 }, view: 1, uc: vf.0, d: vf.1, hide_c: near, td: town_td(vf.1, 1.08), glow: 1.0, ..I0 };
        c.begin(i1.clone(), 0.0, 0.0);
        for ys in (1..=nf as i32).map(|fl| fl as f64 * 3.5) { bx(c, 0.0, ys, ww, 0.2 + (ys - EYE).max(0.0) * de * sz / vf.1); }
        let x = (c.rnd(fi, 30.0, 1301.0) - 0.5) * ww * 0.8;
        c.trap(x, 0.0, 3.2 * (tx + tz), 3.2 * (tx + tz), ht + 3.5, 1.0);
        if c.rnd(fi, 31.0, 1301.0) < 0.45 { c.trap(0.0, ht - 6.0, ww + 0.4, ww + 0.4, 7.5, 0.92); }
        for k in (0..3).map(|k| k as f64) {
            let (a, bb) = (c.rnd(fi * 4.0 + k, 32.0, 1301.0), c.rnd(fi * 4.0 + k, 33.0, 1301.0));
            if bb <= 0.8 { c.light((a * 2.0 - 1.0) * ww * 0.8, ((bb * 1.25 * (nf - 1.0)).floor() + 1.7) * 3.5, 0.3, 1.1, if a < 0.6 { 0.6 } else { 1.4 }); }
        }
        c.end(1.0, 1.0);
        // the columns of all four faces (the floors are open), six per instance
        let (cw, nx, nz) = (0.3 * (tx + tz), 2f64.max((2.0 * hx / 7.0 + 0.5).floor()), 2f64.max((2.0 * hz / 7.0 + 0.5).floor()));
        let mut cols = vec![];
        for s in [-1.0, 1.0] {
            if tx > 0.08 { cols.extend((0..=nx as i32).map(|i| s * hz * vf.3[1] + (-hx + i as f64 * 2.0 * hx / nx) * vf.3[0])); }
            if tz > 0.08 { cols.extend((0..=nz as i32).map(|i| s * hx * vf.3[0] + (-hz + i as f64 * 2.0 * hz / nz) * vf.3[1])); }
        }
        cols.sort_by(f64::total_cmp);
        for g in cols.chunks(6) { c.begin(i1.clone(), 0.0, 0.5 * (g[0] + g[g.len() - 1])); for &x in g { c.trap(x, 0.0, cw, cw, ht + 1.2, 1.0); } c.end(0.5, 0.5); }
    }
}
