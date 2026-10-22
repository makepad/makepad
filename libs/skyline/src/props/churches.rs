//! Churches (the page's `pkChurches`): behind the town's back row on the slope it climbs, by the town's style (a village
//! church with a spire, a campanile beside a basilica, a domed church), each level on a churchyard knoll; floodlit (a
//! warm wash up the tower), lit windows at an evening service, a lit clock face, lit belfry openings.
use super::kit::{Ctx, Pt::*, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix, wd};

pub fn build(c: &mut Ctx) {
    let z = c.z.church;
    if c.p[P::PrChurch] <= 0.0 || z[1] <= 0.0 { return; }
    // one or two, as many over Size
    let (s, sty, n_c) = (c.town_site(), c.rnd(5.0, 1.0, 1600.0), ((if c.p[P::PrChurch] >= 0.55 { 2.0 } else { 1.0 }) / c.k).round().clamp(1.0, 2.0) as usize);
    let (ra, rb, rt0) = (c.rnd(1.0, 1.0, 1500.0), c.rnd(2.0, 1.0, 1500.0), c.rnd(3.0, 1.0, 1500.0));
    let t0 = if sty < 0.5 { if rt0 < 0.45 { 1 } else if rt0 < 0.8 { 2 } else { 0 } } else if rt0 < 0.7 { 0 } else if rt0 < 0.85 { 1 } else { 2 };
    // offsets from the town's centre within the smallest town, so the Town slider does not move the churches
    let (t1, u0, o1) = ((t0 + 1 + (rb >= 0.5) as usize) % 3, fract(z[0] + (ra - 0.5) * 1.1 * 0.017), (if rb < 0.5 { -1.0 } else { 1.0 }) * 0.017 * mix(0.5, 0.9, c.rnd(4.0, 1.0, 1500.0)));
    let u1 = fract(u0 + if wd(u0 + o1, z[0]).abs() > 0.85 * z[1] { -o1 } else { o1 });
    for i in (0..n_c).rev() {
        let fi = i as f64;
        let (uc, t, kf) = (if i == 1 { u1 } else { u0 }, if i == 1 { t1 } else { t0 }, 3.45 + 0.25 * fi);
        let (d, g, hwc) = (s[0] + kf * s[1], c.town_ground(uc, kf, &s) - 0.6, [32.0, 48.0, 30.0][t]); // a level floor at g
        let (wm, sg) = (hwc + 3.0 * g.max(0.0) + 30.0, if c.rnd(fi, 5.0, 1500.0) < 0.5 { -1.0 } else { 1.0 });
        let r: Vec<f64> = (0..6).map(|k| c.rnd(fi + 7.0, k as f64, 1510.0)).collect();
        let (flood, lit, y0, church) = (c.rnd(fi, 6.0, 1500.0) <= 0.6, c.rnd(fi, 7.0, 1500.0) + 1e-4 <= c.p[P::WinD] * 0.45, (g - 1.0).max(0.0), Instance { uc, d, glow: 2.0, ..I0 });
        let tri = |c: &mut Ctx, x: f64, ya: f64, hw: f64, h: f64| c.trap(x, ya, hw, 0.0, h, 1.0);
        let cr = |c: &mut Ctx, x: f64, y: f64, hx: f64, hy: f64| c.part(Box, [x, y, hx, hy], [0., 0., 1., 0.]);
        let lt = |c: &mut Ctx, x: f64, y: f64, hx: f64, hy: f64, rc: f64, amp: f64, kw: f64| c.part(Box, [x, y, hx, hy], [rc, amp, 0., kw]);
        let wash = |c: &mut Ctx, ht: f64| if flood { c.part(Wash, [0., g, 0., 0.33 * ht], [g, 0.04, 0., 0.8]); }; // a faint warm wash from below, not below the floor
        let grp = |c: &mut Ctx, x: f64| { c.end(1.0, 1.0); c.begin(church.clone(), y0, x); }; // then the floodlit part, in its own tight bounds
        if g > 0.0 { // the churchyard: a level terrace on a smooth knoll, so the church never floats
            c.begin(Instance { uc, d, ..I0 }, 0.0, 0.0);
            c.part(GProf, [-wm, 0., -hwc, g], [0., 1., 1., 0.]); c.part(GProf, [-hwc, g, hwc, g], [0., 0., 1., 0.]); c.part(GProf, [hwc, g, wm, 0.], [0., 1., 1., 0.]);
            c.end(0.5, 0.5);
        }
        if t == 0 { // a village church side-on: nave, lower apse, a west tower with an octagonal spire, the cross, pinnacles, a clock
            let (nl, nh, tw, th, sp) = (mix(14.0, 21.0, r[1]), mix(10.0, 13.0, r[2]), mix(3.4, 4.4, r[2]), mix(24.0, 34.0, r[4]), mix(18.0, 30.0, r[5]));
            let (rh, xt, ys) = (nh + mix(6.0, 8.5, r[3]), sg * (nl + tw * 0.8), g + th + 0.7);
            c.begin(church.clone(), y0, 0.0); c.bx(0.0, y0, g + rh, nl); c.bx(-sg * (nl + 2.0), y0, g + nh - 1.0, 3.0); tri(c, -sg * nl, g + nh - 1.0, 5.0, 3.5);
            if lit { let mut xn = 5.0 - nl; while xn <= nl - 2.0 { lt(c, sg * xn, g + 5.575, 0.75, 2.575, -0.75, 0.3, 0.8); xn += 5.0; } } // tall arched windows
            grp(c, xt); c.bx(xt, y0, g + th, tw); c.bx(xt, g + th, ys, tw + 0.35); tri(c, xt, ys, tw * 0.9, sp); cr(c, xt, ys + sp + 0.85, 0.13, 1.85); cr(c, xt, ys + sp + 1.8, 0.8, 0.13);
            if r[3] > 0.5 { for s in [-1.0, 1.0] { tri(c, xt + s * (tw - 0.35), g + th, 0.38, 3.4); } }
            wash(c, 70.0);
            if r[5] >= 0.55 { lt(c, xt, g + th - 5.5, 1.1, 1.1, 1.1, 0.45, 1.25); }
        } else if t == 1 { // a basilica (gabled nave front, lower aisles, a small dome, a rose window) beside a campanile with a spire and statue
            let (cw, ch, bh, sp) = (mix(5.0, 6.2, r[1]), g + mix(42.0, 60.0, r[2]), mix(9.0, 11.0, r[3]), mix(14.0, 22.0, r[4]));
            let (ya, bl, wh) = (ch + bh + 5.5, mix(13.0, 18.0, r[5]), mix(15.0, 19.0, r[3]));
            let xb = sg * (cw + 3.0 + bl);
            c.begin(church.clone(), y0, xb); c.bx(xb, y0, g + wh, bl * 0.55); tri(c, xb, g + wh, bl * 0.55, bl * 0.55 * 0.42); c.bx(xb, y0, g + wh * 0.62, bl);
            tri(c, xb, g + wh * 0.62, bl, bl * 0.25);
            if r[4] > 0.55 { c.ell(xb, g + wh + 2.0, 7.0, 7.0 / 1.15); }
            if lit { lt(c, xb, g + wh * 0.62, 1.4, 1.4, 1.4, 0.3, 0.8); }
            grp(c, 0.0); c.bx(0.0, y0, ch, cw); c.bx(0.0, ch - 0.4, ch + 0.5, cw + 0.5); c.bx(0.0, ch, ch + bh, cw - 0.2); c.bx(0.0, ch + bh - 0.2, ch + bh + 0.9, cw + 0.5);
            c.bx(0.0, ch + bh, ya, cw * 0.9); tri(c, 0.0, ya, cw * 0.95, sp); cr(c, 0.0, ya + sp + 1.1, 0.22, 2.1);
            wash(c, 90.0);
            if flood { // lit belfry
                let bw = 2.0 * (cw - 0.2) / 3.0;
                for j in 0..3 { lt(c, -(cw - 0.2) + bw * (j as f64 + 0.5), ch + (0.9 * bh + 0.27 * bw) / 2.0, 0.27 * bw, (0.6 * bh + 0.27 * bw) / 2.0, -0.27 * bw, 0.36, 0.8); }
            }
        } else { // a domed church: body, pediment, side wings, two small bell towers; then drum, dome, lantern, cupola, the cross
            let (bw, bh, dw, dh, ph) = (mix(14.0, 19.0, r[1]), mix(17.0, 23.0, r[2]), mix(8.0, 11.0, r[3]), mix(8.0, 12.0, r[4]), mix(1.0, 1.3, r[5]));
            let (rw, y1, lw) = (dw * 1.06, g + bh + dh, mix(1.6, 2.2, r[1]));
            let dt = y1 + rw * ph;
            c.begin(church.clone(), y0, 0.0); c.bx(0.0, y0, g + bh, bw); tri(c, 0.0, g + bh, bw * 0.45, bw * 0.45 * 0.35); c.bx(0.0, y0, g + bh * 0.62, bw + 9.0);
            if r[2] > 0.45 { let bt = g + bh + mix(12.0, 18.0, r[4]); for s in [-1.0, 1.0] { c.bx(s * (bw - 2.8), y0, bt, 2.6); tri(c, s * (bw - 2.8), bt, 2.7, 4.5); } }
            if lit { let mut xc = 2.0 - bw; while xc <= bw - 1.5 { if xc.abs() > bw * 0.45 { lt(c, xc, g + bh * 0.5, 0.7, 2.0, 0.0, 0.3, 0.8); } xc += 4.0; } }
            grp(c, 0.0); c.bx(0.0, y0, y1, dw); c.part(Ell, [0., y1, rw, rw * ph], [0., 1., 1., 0.]); c.bx(0.0, dt - 1.5, dt + 6.5, lw); c.ell(0.0, dt + 6.5, lw * 1.1, lw * 1.1 / 1.4);
            cr(c, 0.0, dt + 9.0, 0.15, 2.0); cr(c, 0.0, dt + 9.8, 0.8, 0.15);
            wash(c, 60.0);
        }
        c.end(1.0, 1.0);
    }
}
