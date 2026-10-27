//! The bridge (the page's `pkBridge`): one per circle, a straight line in the world 2.5-3.6 km out, turned a little from
//! lateral so it recedes in real perspective, between headlands (or valley sides): suspension, steel arch, girder / truss
//! or cable-stayed by seed; deck lamps, car lights, red beacons. One line frame along the axis, parts beside it on their
//! own parallel lines.
use super::kit::{clamp, hypot, Ctx, Pt::{self, *}, I0, Instance};
use crate::params::P;
use crate::rng::{mix, smooth, wd, TAU};

/// A part with its line-frame placement: type, a, b, c (lateral offset, s0, cv0).
type Pa = (Pt, [f64; 4], [f64; 4], [f64; 3]);
/// A tower, pier or stay fan: its s extent, top and parts.
struct Obj { lo: f64, hi: f64, top: f64, p: Vec<Pa> }

pub fn build(c: &mut Ctx) {
    let (z, zx, zz) = (c.z.bridge, c.z.bridge_x, c.z);
    if c.p[P::PrBridge] <= 0.0 || z[1] <= 0.0 { return; }
    let (r, pn, wat) = ((0..8).map(|i| c.rnd(i as f64, 1.0, 1800.0)).collect::<Vec<f64>>(), |x: f64, ly: f64| zz.rng.pn(x, 1e4, ly), if zx[0] >= 0.5 { 1.0 } else { 0.0 });
    // the type: 0 suspension, 1 arch, 2 girder / truss, 3 cable-stayed
    let ty = if wat > 0.0 { if r[0] < 0.28 { 0 } else if r[0] < 0.52 { 1 } else if r[0] < 0.78 { 3 } else { 2 } } else if r[0] < 0.35 { 1 } else if r[0] < 0.7 { 3 } else { 2 };
    let d0 = 2500.0 + 1100.0 * r[1]; // mostly lateral: far decks read level
    let beta = (if r[3] < 0.5 { -1.0 } else { 1.0 }) * 0.0174533 * if ty == 0 { mix(10.0, 22.0, r[2]) } else if ty == 1 { mix(3.0, 16.0, r[2]) } else { mix(0.0, 12.0, r[2]) };
    // the axis: s (size units: Size m each, so the whole bridge is laid out in them, its ends where the zone's are) along it
    // from its middle (bearing z[0], d0 m away), toward larger bearings; lf toward the camera
    let (u0, sb, cb, sz) = (z[0] * TAU, beta.sin(), beta.cos(), c.k);
    let (er, et) = ([u0.cos(), u0.sin()], [-u0.sin(), u0.cos()]);
    let tv = [sb * er[0] + cb * et[0], sb * er[1] + cb * et[1]];
    let (lf, ang, pp, xs) = ([-tv[1], tv[0]], tv[1].atan2(tv[0]), d0 * cb, |s: f64| [d0 * er[0] + s * sz * tv[0], d0 * er[1] + s * sz * tv[1]]);
    // the view at s: cv (a length along the axis shows x cv), sv; a part n m beside the axis at s0 is seen where the axis has s0 - n tph
    let fr = |s: f64| { let x = xs(s); let dd = hypot(x[0], x[1]); let sv = (x[0] * tv[0] + x[1] * tv[1]) / dd; (pp / dd, sv, sv.abs(), sv / (pp / dd)) };
    // the layout along the axis: hills at both ends, the water (or valley) between their feet; the land end straddles the shore
    let a = z[1] * TAU;
    let (sl, sr) = (-d0 * a.sin() / (beta - a).cos() / sz, d0 * a.sin() / (beta + a).cos() / sz);
    let (lt, mut wfl, mut wfr) = (sr - sl, if wat > 0.0 { 0.14 } else { 0.2 }, if wat > 0.0 { 0.14 } else { 0.2 });
    if wat > 0.0 {
        let tsh = wd(zx[1], z[0]) * TAU;
        let ssh = d0 * tsh.sin() / (tsh + beta).cos() / sz;
        if zx[2] > 0.0 { wfl = clamp((ssh - sl) / lt + 0.03, 0.12, 0.34); } else { wfr = clamp((sr - ssh) / lt + 0.03, 0.12, 0.34); }
    }
    let (sfl, sfr) = (sl + wfl * mix(0.85, 1.15, r[4]) * lt, sr - wfr * mix(0.85, 1.15, r[5]) * lt);
    let (lw, sm, scl, scr) = (sfr - sfl, 0.5 * (sfl + sfr), sl + 0.58 * (sfl - sl), sr - 0.58 * (sr - sfr)); // the hill crests: the deck runs between them
    // the type's parameters (m)
    let (mut lm, mut st1, mut st2, mut ht, mut sa1, mut sa2, mut la, mut hcr, mut ysp, mut de, mut dc, mut hump) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let (sty, sp, trs) = (r[6] >= 0.5, 60.0 + 70.0 * r[6], r[7] >= 0.45 && wat > 0.0);
    let (mut np, mut ct1, mut ct2, mut crm, mut crb, mut chp, mut cshp, mut cnc, mut harp) = (1, 0.0, 0.0, 0.0, 0.0, 0.0, 0, 8.0, false);
    let (hd, ddep, wdk, hmax);
    if ty == 0 {
        let dl = clamp(0.05 * lw, 25.0, 90.0);
        lm = (lw - 2.0 * dl).min(1300.0);
        (st1, st2, hd) = (sm - 0.5 * lm, sm + 0.5 * lm, clamp(22.0 + 0.036 * lm, 28.0, 70.0));
        (ht, wdk, ddep) = (hd + 0.105 * lm + 14.0, 18.0 + 0.008 * lm, 3.0 + 0.004 * lm);
        let ls = clamp(0.33 * lm, 60.0, (st1 - scl - 15.0).max(40.0));
        (sa1, sa2, hmax) = (st1 - ls, st2 + ls, ht + 4.0);
    } else if ty == 1 { // a through arch over water, or a deck arch springing low on the valley sides with its crown under the deck
        la = clamp(lw - 40.0, 150.0, 550.0);
        if wat > 0.0 {
            (hd, ddep) = (clamp(18.0 + 0.05 * la, 25.0, 50.0), 3.5); ysp = if sty { 6.0 } else { hd - ddep };
            (de, dc, hcr) = if sty { (0.1 * la + 4.0, 0.032 * la + 3.0, hd + 0.17 * la + 8.0) } else { (0.018 * la + 3.0, 0.018 * la + 3.0, hd + 0.2 * la) };
        }
        else { (hd, ddep) = (55.0 + 50.0 * r[7], 4.0); (de, dc, hcr) = (0.06 * la + 3.0, 0.025 * la + 2.5, hd - ddep - 0.5); }
        (wdk, hmax) = (20.0 + 0.01 * la, hcr.max(hd + 38.0 * sty as u8 as f64 * wat) + 4.0);
    } else if ty == 3 { // H, A or inverted-Y pylons, as a semi-fan or a harp; long crossings: two pylons, else one and an unequal back span
        (harp, cshp) = (r[7] <= 0.35, (r[5] * 2.999).floor() as i32);
        if lw > 650.0 && r[6] > 0.3 {
            (np, lm) = (2, (0.56 * lw).min(900.0));
            (ct1, ct2, crm) = (sm - 0.5 * lm, sm + 0.5 * lm, 0.47 * lm);
            (crb, chp) = (clamp((0.42 * lm).min(ct1 - scl - 20.0), 60.0, 0.42 * lm), 0.21 * lm + 10.0);
        } else { crm = (0.52 * lw).min(330.0); crb = 0.7 * crm; ct1 = sm - 0.5 * (crm - crb); (ct2, chp, lm) = (ct1, 0.5 * crm + 10.0, 2.0 * crm); }
        chp *= mix(1.0, 1.2, harp as u8 as f64); // harps need taller pylons
        hd = if wat > 0.0 { clamp(20.0 + 0.03 * lm, 24.0, 55.0) } else { 55.0 + 50.0 * r[7] };
        (ddep, wdk, cnc, hmax) = (2.5 + 0.002 * lm, 20.0 + 0.008 * lm, clamp((crm / 14.0).floor(), 8.0, 22.0), hd + chp + 6.0);
    } else {
        (hd, hump) = (if wat > 0.0 { 18.0 + 17.0 * r[7] } else { 55.0 + 50.0 * r[7] }, wat * (2.0 + 4.0 * r[5]));
        (ddep, wdk) = (3.0 + 0.035 * sp, 16.0 + 6.0 * r[4]);
        hmax = hd + hump + if trs { clamp(0.14 * sp, 9.0, 18.0) } else { 0.0 } + 12.0;
    }
    let (pkl, pkr) = ((hd + 12.0) * mix(1.1, 1.35, r[4]) + (1.0 - wat) * 25.0, (hd + 12.0) * mix(1.1, 1.35, r[5]) + (1.0 - wat) * 25.0);
    // the deck: level, or (a girder over the water) a gentle rise over the middle fitted by one parabola (least squares)
    let (mut q0, mut q2) = (0.0, 0.0);
    if hump > 0.0 {
        let xm = (sm - scl).max(scr - sm) / (0.45 * lw);
        let (mut s2, mut s4, mut g0, mut g2, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for i in 0..=64 { let x = xm * (i as f64 / 32.0 - 1.0); let x2 = x * x; let g = (-x2).exp(); n += 1.0; s2 += x2; s4 += x2 * x2; g0 += g; g2 += g * x2; }
        let det = n * s4 - s2 * s2;
        (q0, q2) = ((g0 * s4 - g2 * s2) / det, (n * g2 - s2 * g0) / det);
    }
    let yd = |s: f64| { let x = (s - sm) / (0.45 * lw); hd + hump * (q0 + q2 * x * x) };
    let (dtop, dbot, dt, db) = (|s: f64| yd(s) + 1.0, |s: f64| yd(s) - ddep, hd + 1.0, hd - ddep);
    let par = |f: &dyn Fn(f64) -> f64, a: f64, b: f64| { let (ya, yb) = (f(a), f(b)); [ya, yb, 0.5 * (ya + yb) - f(0.5 * (a + b))] }; // end heights, sag
    // the end hills: a long landward rise, a steeper seaward fall with a rounded brow, relief, a ragged woodland edge
    let hill = |x: f64| {
        let w = if x < sfl { (x - sl) / (sfl - sl) } else if x > sfr { (sr - x) / (sr - sfr) } else { -1.0 };
        if w <= 0.0 || w >= 1.0 { return 0.0; }
        let a = if w < 0.58 { w / 0.58 } else { (1.0 - w) / (1.0 - 0.58) };
        let h = if w < 0.58 { a * a * (3.0 - 2.0 * a) } else { 1.0 - (1.0 - a).max(0.0).powf(1.6) };
        let (rel, n1, n2) = (0.5 * pn(x * 0.006 + 40.0, 1810.0) + 0.3 * pn(x * 0.017 + 80.0, 1811.0) + 0.2 * pn(x * 0.045 + 20.0, 1812.0),
            pn(x / 11.0 + 300.0, 1813.0), pn(x / 4.7 + 500.0, 1814.0));
        (if x < sfl { pkl } else { pkr }) * h * (0.8 + 0.4 * rel)
            + (2.0 + 7.0 * ((1.0 - (2.0 * n1 - 1.0).abs()) * 0.65 + n2 * 0.35)) * smooth(0.35, 0.6, pn(x * 0.008 + 7.0, 1815.0)) * smooth(0.15, 0.5, h)
    };
    let (mut cl, mut cr) = (sm, sm); // lamps and cars only where the deck is clear of the hills
    while cl > scl && hill(cl - 2.0) <= dtop(cl - 2.0) { cl -= 2.0; }
    while cr < scr && hill(cr + 2.0) <= dtop(cr + 2.0) { cr += 2.0; }
    // the parts in bridge s: the spine (long S-native parts), the lights, towers and piers, the stay fans
    let (mut spine, mut lights, mut objs, mut fans): (Vec<Pa>, Vec<Pa>, Vec<Obj>, Vec<Obj>) = (vec![], vec![], vec![], vec![]);
    let loc = |t: Pt, s0: f64, a: [f64; 4], b: [f64; 4], n: f64| (t, a, b, [-n, s0, fr(s0).0]); // a LOCAL part at s0, n m beside the axis
    let bx = |s0: f64, x: f64, y0: f64, y1: f64, w0: f64, w1: f64, n: f64| loc(Trap, s0, [x, y0, w0, w1], [y1 - y0, 0., 1., 0.], n);
    let beacons = |l: &mut Vec<Pa>, s0: f64, s1: f64, y: f64| l.push((Slr, [s0, y, s1, y], [0., 0., 0., -1.], [0., (s1 - s0).max(0.01), 1.])); // merged when they overlap
    let obj = |o: &mut Vec<Obj>, s0: f64, ext: f64, top: f64, p: Vec<Pa>| o.push(Obj { lo: s0 - ext, hi: s0 + ext, top, p });
    // a pier from the ground to y1 at s0, half-width w, widening by bat per side and metre toward the ground
    let pier = |o: &mut Vec<Obj>, s0: f64, w: f64, y1: f64, bat: f64| {
        let f = fr(s0); let k = f.0 + f.2; obj(o, s0, (w + bat * y1) * k / f.0 + 6.0, y1 + 1.0, vec![bx(s0, 0.0, 0.0, y1, (w + bat * y1) * k, w * k, 0.0)]);
    };
    let (lo, hi) = (par(&dbot, scl, scr), par(&dtop, scl, scr)); // the deck slab between the crests
    spine.push((PBand, [scl, lo[0], scr, lo[1]], [lo[2], hi[0], hi[1], hi[2]], [0., 0., 0.]));
    let (lk, spl, hl, pc) = (if r[2] < 0.6 { 0.72 } else { 1.25 }, mix(42.0, 56.0, r[5]), 9.0, 0.12 + 0.25 * r[6]);
    for n in [-0.5 * wdk, 0.5 * wdk] { // deck lamps on both edges on 9 m poles
        let (k0, k1) = ((((scl + 15.0).max(cl) - sm) / spl).ceil(), (((scr - 15.0).min(cr) - sm) / spl).floor());
        if k0 > k1 { continue; }
        let (a0, b0) = (sm + k0 * spl, sm + k1.max(k0 + 0.02) * spl);
        let (y, lo) = (par(&|s| dtop(s) + hl, a0, b0), par(&dtop, a0, b0));
        lights.push((Slr, [a0, y[0], b0, y[1]], [y[2], 0.3, 2.2, lk], [-n, spl, 1.])); spine.push((PBand, [a0, lo[0], b0, lo[1]], [lo[2], y[0], y[1], y[2]], [-n, spl, 0.12]));
    }
    let (a0, b0) = ((scl + 10.0).max(cl), (scr - 10.0).min(cr)); // the near lane white headlights, the far lane red tail lights
    for (n, kw) in [(-0.25 * wdk, 1.7), (0.25 * wdk, 0.0)].into_iter().filter(|_| b0 - a0 >= 11.0) {
        let y = par(&|s| dtop(s) + 0.6, a0, b0); lights.push((Slr, [a0, y[0], b0, y[1]], [y[2], 0.2, 1.1, kw], [-n, 11., pc]));
    }
    if ty == 0 { // suspension: two towers (legs on both deck edges, portal struts, X bracing under the deck), cables, suspenders
        let (ytop, ylow, ya, ls, spc, a0) = (ht - 2.0, dt + 2.5, hd + 5.0, st1 - sa1, 12.0 + 0.004 * lm, 3.0 + 0.011 * ht);
        let cab = |x: f64| if x >= st1 && x <= st2 { let f = (x - 0.5 * (st1 + st2)) / (0.5 * (st2 - st1)); ylow + (ytop - ylow) * f * f }
            else { let f = if x < st1 { (x - sa1) / (st1 - sa1) } else { (sa2 - x) / (sa2 - st2) }; mix(ya, ytop, f) - 0.036 * ls * f * (1.0 - f) };
        for t in [st1, st2] {
            let f = fr(t);
            let (kk, hsp, mut p) = (f.0 + 1.4 * f.2, 0.5 * wdk * f.2, vec![]);
            for x in [-0.5 * wdk, 0.5 * wdk].map(|n| -n * f.1) { // below the deck; above it the legs step in at each strut
                p.push(bx(t, x, 0.0, dt, 1.12 * a0 * kk, 1.12 * a0 * kk, 0.0));
                for k in 0..5 {
                    let w = a0 * (1.12 - 0.3 * (k as f64 / 4.3) * (ht - dt) / ht - 0.08) * kk;
                    p.push(bx(t, x, 0.0, if k < 4 { dt + (k + 1) as f64 / 4.3 * (ht - dt) } else { ht }, w, w, 0.0));
                }
            }
            for i in 0..4 { let (yk, hk) = (dt + (ht - dt) * (0.3 + 0.233 * i as f64) - 1.5, 2.0 + 0.01 * ht); p.push(bx(t, 0.0, yk - hk, yk + hk, hsp, hsp, 0.0)); } // portal struts
            p.extend([loc(Seg, t, [-hsp, 12., hsp, db - 1.0], [1., 1., 1., 0.], 0.0), // bracing, the pier
                loc(Seg, t, [hsp, 12., -hsp, db - 1.0], [1., 1., 1., 0.], 0.0), bx(t, 0.0, 0.0, 12.0, hsp + a0 * kk + 4.0, hsp + a0 * kk + 4.0, 0.0)]);
            obj(&mut objs, t, (hsp + a0 * kk + 8.0) / f.0, ht + 2.0, p);
            beacons(&mut lights, t - 0.5 * wdk * f.3.abs(), t + 0.5 * wdk * f.3.abs(), ht + 1.2); // one light per leg top
        }
        for n in [-0.5 * wdk, 0.5 * wdk] {
            spine.extend([(Arc, [st1, ytop, st2, ytop], [ytop - ylow, 0.45, 1., 0.], [-n, 0., 1.]),
                (Arc, [sa1, ya, st1, ytop], [0.009 * ls, 0.45, 1., 0.], [-n, 0., 1.]), (Arc, [st2, ytop, sa2, ya], [0.009 * ls, 0.45, 1., 0.], [-n, 0., 1.])]);
            let km = ((st2 - st1) / spc - 1e-6).floor(); // suspenders every spc from st1 (the side spans too), deck to cable
            for (k0, k1) in [(((sa1 - st1) / spc + 1e-6).ceil(), -1.0), (0.0, km), (km + 1.0, ((sa2 - st1) / spc - 1e-6).floor())].into_iter().filter(|q| q.1 >= q.0) {
                let (a0s, b0s) = (st1 + k0 * spc, st1 + k1.max(k0 + 0.02) * spc);
                let u = par(&cab, a0s, b0s);
                spine.push((PBand, [a0s, dt, b0s, dt], [0., u[0], u[1], u[2]], [-n, spc, 0.22]));
            }
            for s in [sa1, sa2] { let f = fr(s); obj(&mut objs, s, 22.0 + 0.5 * wdk * f.3.abs() + 6.0, ya + 6.0, vec![bx(s, 0.0, 0.0, ya + 5.0, 22.0 * f.0, 22.0 * f.0, n)]); } // anchorages
        }
        for s in (((scl - sm) / 48.0).ceil() as i64..=((scr - sm) / 48.0).floor() as i64).map(|k| sm + 48.0 * k as f64) { // the approach viaduct
            if s < sa1 - 20.0 || s > sa2 + 20.0 { pier(&mut objs, s, 2.5, db, 0.0); }
        }
    } else if ty == 1 { // arch: two lattice ribs, hangers (through arch) or spandrel columns (deck arch), approach piers
        let (pan, rr, rt, lwr) = (la / 28.0, hcr - dc - ysp, hcr - ysp - de, 0.7 + 0.0018 * la);
        let (aa, ba, a0, b0) = (sm - 0.5 * la, sm + 0.5 * la, sm - 13.0 * pan, sm + 13.0 * pan);
        let (yb, yt) = (|s: f64| { let x = (s - sm) / (0.5 * la); ysp + rr * (1.0 - x * x) }, |s: f64| { let x = (s - sm) / (0.5 * la); ysp + de + rt * (1.0 - x * x) });
        for n in [-0.5 * wdk, 0.5 * wdk] {
            spine.push((PBand, [aa, ysp, ba, ysp], [-rr, ysp + de, ysp + de, -rt], [-n, -pan, lwr])); // rib: intrados to extrados, 28 panels
            if wat > 0.0 { let u = par(&yb, a0, b0); spine.push((PBand, [a0, dt, b0, dt], [0., u[0], u[1], u[2]], [-n, pan, 0.3])); } // hangers
            else { let l = par(&yt, a0, b0); spine.push((PBand, [a0, l[0], b0, l[1]], [l[2], db, db, 0.], [-n, pan, 1.1])); } // spandrel columns
        }
        if wat > 0.0 {
            beacons(&mut lights, sm, sm, hcr + 1.2);
            for s in [-1.0, 1.0].map(|sg| sm + sg * (0.5 * la + 14.0)).into_iter().filter(|_| sty) { // stone pylons at the abutments
                let f = fr(s);
                let (pw, hp) = (12.0 * f.0 + 15.0 * f.2, hd + 38.0);
                obj(&mut objs, s, pw / f.0 + 6.0, hp + 1.0, vec![bx(s, 0.0, 0.0, hp - 8.0, pw, pw, 0.0), bx(s, 0.0, hp - 9.0, hp, 0.82 * pw, 0.82 * pw, 0.0)]);
                beacons(&mut lights, s, s, hp + 1.0);
            }
            for sg in [-1.0, 1.0] { pier(&mut objs, sm + sg * 0.5 * la, if sty { 9.0 } else { 6.0 }, if sty { ysp + 2.0 } else { db }, 0.0); } // the main piers
        }
        let pa = if wat > 0.0 { 52.0 } else { 70.0 };
        for s in (((scl - sm) / pa).ceil() as i64..=((scr - sm) / pa).floor() as i64).map(|k| sm + pa * k as f64) {
            if (s - sm).abs() > 0.5 * la + 25.0 { pier(&mut objs, s, 2.5, db, if wat > 0.0 { 0.0 } else { 0.02 }); }
        }
    } else if ty == 3 { // cable-stayed: straight stays from the pylon to both deck edges
        let (ytp, a0, dya, dyh) = (dt + chp, 2.4 + 0.012 * chp, 2.5f64.min(0.28 * chp / cnc), 0.6 * chp / (cnc - 1.0));
        for k in 0..np {
            let t = if k == 1 { ct2 } else { ct1 };
            let f = fr(t);
            let (kk, hsp, ym, mut p) = (f.0 + 1.4 * f.2, 0.5 * wdk * f.2, dt + 0.45 * chp, vec![]);
            for xb in [-0.5, 0.5].map(|sg| -sg * wdk * f.1) { // the two legs: an A converges, a Y merges into one mast
                let w0 = 1.12 * a0 * kk;
                p.push(bx(t, xb, 0.0, dt + 3.0, w0, w0, 0.0));
                p.push(match cshp { 0 => bx(t, xb, dt, ytp, w0, 0.77 * a0 * kk, 0.0), 1 => loc(Seg, t, [xb, dt, 0.06 * xb, ytp - 0.77 * a0 * kk], [w0, 0.77 * a0 * kk, 1., 0.], 0.0),
                    _ => loc(Seg, t, [xb, dt, 0., ym], [w0, (1.12 - 0.35 * 0.45) * a0 * kk, 1., 0.], 0.0) });
            }
            if cshp == 1 { let w = 0.03 * wdk * f.2 + 0.77 * a0 * kk; p.push(bx(t, 0.0, ytp - 0.77 * a0 * kk, ytp, w, w, 0.0)); } // the apex: flat on top
            if cshp == 2 { p.push(bx(t, 0.0, ym, ytp, 1.35 * (1.12 - 0.35 * 0.45) * a0 * kk, 1.35 * 0.77 * a0 * kk, 0.0)); }
            p.push(bx(t, 0.0, db - 3.5, db + 0.5, hsp, hsp, 0.0)); // cross beams under the deck and near the top of an H, a pier at the water
            if cshp == 0 { p.push(bx(t, 0.0, ytp - 0.14 * chp - 2.2, ytp - 0.14 * chp + 2.2, hsp, hsp, 0.0)); }
            if wat > 0.0 { p.push(bx(t, 0.0, 0.0, 8.0, hsp + a0 * kk + 5.0, hsp + a0 * kk + 5.0, 0.0)); }
            obj(&mut objs, t, (hsp + a0 * kk + 10.0) / f.0, ytp + 2.0, p);
            if cshp == 0 { beacons(&mut lights, t - 0.5 * wdk * f.3.abs(), t + 0.5 * wdk * f.3.abs(), ytp + 1.2); } else { beacons(&mut lights, t, t, ytp + 1.2); }
            // stays on both sides of this pylon, both cable planes: a FAN each on the deck-edge plane (an A's or Y's anchors seen there)
            let mut fp = vec![];
            for dir in [-1.0, 1.0] {
                let fwd = dir * if k == 0 { 1.0 } else { -1.0 };
                let reach = if fwd > 0.0 { crm } else { crb };
                let kh = 0.95 * chp / reach.max(1.0); // an unequal back span is steeper
                let dmin = if harp { 0.35 * chp / kh } else { 0.1 * reach };
                for nd in [-0.5 * wdk, 0.5 * wdk] {
                    let sa = t + (nd - match cshp { 0 => nd, 1 => 0.03 * nd, _ => 0.0 }) * f.3;
                    fp.push(if harp { (Fan, [sa, dt + 0.35 * chp, t + dir * 0.35 * chp / kh, dt], [0., dyh, dir * dyh / kh, 0.], [-nd, cnc, 0.28]) }
                        else { (Fan, [sa, ytp - 2.0 - (cnc - 1.0) * dya, t + dir * dmin, dt], [0., dya, dir * ((reach - dmin) / (cnc - 1.0)), 0.], [-nd, cnc, 0.28]) });
                }
            }
            fans.push(Obj { lo: t - if k == 0 { crb } else { crm } - 30.0, hi: t + if k == 0 { crm } else { crb } + 30.0, top: ytp + 2.0, p: fp });
            for q in [0.5, 0.97] { pier(&mut objs, t + if k == 0 { -1.0 } else { 1.0 } * crb * q, 2.3, db, 0.0); } // auxiliary piers under the back span
        }
        let (zl, zr) = (ct1 - crb, if np < 2 { ct1 + crm } else { ct2 + crb }); // approach piers beyond the stayed part
        for s in (((scl - sm) / 55.0).ceil() as i64..=((scr - sm) / 55.0).floor() as i64).map(|k| sm + 55.0 * k as f64) {
            if s > zr + 15.0 || s < zl - 15.0 { pier(&mut objs, s, 2.5, db, if wat > 0.0 { 0.0 } else { 0.02 }); }
        }
    } else { // girder / truss: a haunched box girder on twin-column piers; a through truss over the middle spans
        let ph0 = sm + 0.5 * sp * if r[4] >= 0.5 { 1.0 } else { 0.0 };
        for s in (((scl - ph0) / sp).ceil() as i64..=((scr - ph0) / sp).floor() as i64).map(|k| ph0 + k as f64 * sp) {
            let f = fr(s);
            let (yb, hh, w0) = (yd(s) - 1.6 * ddep, 0.6 * ddep + 1.0, 0.02 * sp * f.0);
            let w1 = w0 + 0.26 * sp * f.0 * hh / (0.6 * ddep);
            let mut p = vec![bx(s, 0.0, yb, yb + hh, w0, w1, 0.0)]; // the haunch over the pier
            for x in [-0.5 * (wdk - 5.0), 0.5 * (wdk - 5.0)].map(|n| -n * f.1) { // twin columns up into the haunch, widening toward a valley floor
                let (k2, wt) = (f.0 + f.2, 2.2 * (f.0 + f.2));
                let top = yb + clamp((x.abs() + wt - w0) / (w1 - w0), 0.0, 1.0) * hh + 0.4;
                p.push(bx(s, x, 0.0, top, (2.2 + (1.0 - wat) * 0.018 * top) * k2, wt, 0.0));
            }
            obj(&mut objs, s, (w1 + wdk * f.2 + 6.0) / f.0, dbot(s) + 2.0, p);
        }
        if trs {
            let (htr, pan) = (clamp(0.14 * sp, 9.0, 18.0), sp / 2f64.max((sp / 13.0 + 0.5).floor()));
            let (a0, b0) = (ph0 + ((sm - 0.36 * lw - ph0) / pan).ceil() * pan, ph0 + ((sm + 0.36 * lw - ph0) / pan).floor() * pan);
            let (lo, hi) = (par(&dtop, a0, b0), par(&|s| dtop(s) + htr, a0, b0));
            spine.push((PBand, [a0, lo[0], b0, lo[1]], [lo[2], hi[0], hi[1], hi[2]], [0., -pan, 0.6]));
        }
    }
    // the instances: one line along the axis (lat size units toward the camera) from s0 to s1, the parts rebased to it; the
    // line's plan in metres
    let emit = |c: &mut Ctx, s0: f64, s1: f64, top: f64, list: &[Pa], lat: f64| {
        let o = xs(s0);
        c.add(Instance { line: true, o: [o[0] + lat * sz * lf[0], o[1] + lat * sz * lf[1]], ang, len: (s1 - s0).max(1.0) * sz, smar: 15.0 * sz, top, hide_i: true, seed: 1803, ..I0 });
        for &(t, mut a, b, mut cc) in list { if matches!(t, Arc | Band | LRow | PBand | Slr | Fan) { a[0] -= s0; a[2] -= s0; } else { cc[1] -= s0; } cc[0] -= lat; c.part_c(t, a, b, cc); }
    };
    for (h0, h1) in [(sl, sfl), (sfr, sr)] { // the hills: profiles of 8 samples, 6 profiles per instance
        let n = 1f64.max(((h1 - h0) / 10.5).ceil()) as i64;
        let (ds, mut cur, mut top, mut c0) = ((h1 - h0) / (7.0 * n as f64), vec![], 0f64, h0);
        for j in 0..n {
            let sj = h0 + 7.0 * j as f64 * ds;
            let hv: Vec<f64> = (0..8).map(|i| hill(sj + i as f64 * ds)).collect();
            top = hv.iter().fold(top, |m, &v| m.max(v));
            cur.push((Prof, [hv[0], hv[1], hv[2], hv[3]], [hv[4], hv[5], hv[6], hv[7]], [0., sj, ds]));
            if cur.len() == 6 || j == n - 1 { emit(c, c0, sj + 7.0 * ds, top + 1.0, &cur, 0.0); (cur, top, c0) = (vec![], 0.0, sj + 7.0 * ds); }
        }
    }
    objs.sort_by(|a, b| a.lo.total_cmp(&b.lo)); // towers and piers: consecutive objects in chunks of at most 160 m and 18 parts
    let mut i = 0;
    while i < objs.len() {
        let (lo, mut hi, mut top, mut p, mut j) = (objs[i].lo, objs[i].hi, objs[i].top, objs[i].p.clone(), i + 1);
        while j < objs.len() && objs[j].hi - lo <= 160.0 && p.len() + objs[j].p.len() <= 18 {
            hi = hi.max(objs[j].hi); top = top.max(objs[j].top); p.extend(objs[j].p.iter().copied()); j += 1;
        }
        emit(c, lo, hi, top, &p, 0.0);
        i = j;
    }
    for f in &fans { emit(c, f.lo, f.hi, f.top, &f.p, 0.0); }
    emit(c, sl, sr, hmax.max(dt + hl) + 4.0, &spine, 0.0);
    if !lights.is_empty() { emit(c, sl, sr, hmax.max(dt + hl) + 6.0, &lights, 6.0); } // a few metres nearer: its own body never dims its lights
}
