//! Street lamps (the page's `pkLamps`): rows along straight roads in the plan (the approach road to the town, a road
//! along the city's near edge, a harbour pier from a waterfront town's quay, an expressway with twin-arm masts), a lamp
//! every few tens of metres in true perspective, their light pools along the road.
use super::kit::{hypot, Ctx, Pt::*, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix, TAU};

pub fn build(c: &mut Ctx) {
    let (am, z, sz) = (c.p[P::PrLamp], c.z, c.k);
    if am <= 0.0 { return; }
    let (rr, sx, sea) = (|i: f64, k: f64| z.rnd(i, k, 1700.0), mix(1250.0, 1800.0, z.rnd(1.0, 1.0, 1600.0)), z.town_x[1]);
    let at = |d: f64, u: f64| [d * (u * TAU).cos(), d * (u * TAU).sin()];
    let toward = |a: [f64; 2], b: [f64; 2]| [a[0] + (b[0] - a[0]) * mix(0.4, 1.0, am), a[1] + (b[1] - a[1]) * mix(0.4, 1.0, am)]; // fewer lamps: shorter
    for i in (0..4).rev() {
        let fi = i as f64;
        let (r1, r2, r3, r4, r5) = (rr(fi, 1.0), rr(fi, 2.0), rr(fi, 3.0), rr(fi, 4.0), rr(fi, 5.0));
        let l0 = if sea != 0.0 { -sea } else if rr(0.0, 5.0) < 0.5 { -1.0 } else { 1.0 };
        let (a, b, pp, q);
        if i == 0 || (i == 2 && sea == 0.0) { // an approach road into the town from its land side (row 2: the other side)
            if z.lamp_t[1] <= 0.0 || (i == 2 && am < 0.25) { continue; }
            let ls = if i == 0 { l0 } else { -l0 };
            a = at((sx - 25.0) * mix(0.72, 0.85, r2), z.town[0] + ls * mix(0.035, 0.06, r1));
            (b, pp, q) = (toward(a, at(sx - 25.0, z.town[0] + ls * mix(0.004, 0.012, r3))),
                [0.0, mix(9.0, 11.5, r4), mix(30.0, 38.0, r3), mix(2.0, 4.0, r2)], [if r5 < 0.6 { 0.75 } else { 1.6 }, 1.8, mix(1.5, 2.2, r1)]);
        } else if i == 2 { // the harbour pier from the town's quay across the view over the water (embankment < 0)
            if z.lamp_t[1] <= 0.0 || am < 0.25 { continue; }
            b = at(sx - 9.0, z.town_x[0] + sea * 0.012);
            (a, pp, q) = (toward(b, at((sx - 9.0) * mix(0.93, 0.98, r2), z.town_x[0] + sea * mix(0.035, 0.05, r1))), [0.0, mix(7.5, 9.0, r3), mix(28.0, 34.0, r4), -1.8], [0.75, 1.6, 1.4]);
        } else { // lateral roads in front of the city (or of the town): 1 street lamps, 3 masts; at most 40 deg to each side
            let zz = if z.lamp_c[1] > 0.0 { z.lamp_c } else { [z.town[0], 0.06, 400.0, 1000.0] };
            if (z.lamp_c[1] <= 0.0 && z.lamp_t[1] <= 0.0) || (i == 3 && am < 0.45) { continue; }
            let p1 = mix(560.0, 580f64.max(zz[3] - 200.0), rr(1.0, 2.0));
            let p = if i == 1 { p1 } else { (p1 + mix(120.0, 260.0, r2)).min(zz[3] - 40.0) };
            if i == 3 && p < p1 + 60.0 { continue; }
            let (b0, kq) = (zz[0] + (r1 - 0.5) * 0.4 * zz[1], if r5 < 0.5 { 0.75 } else { 1.6 });
            let (sm, n0) = ((zz[3] * zz[3] - p * p).max(0.0).sqrt().min(p * (zz[1] * TAU * 0.8).min(0.7).tan()) * mix(0.45, 1.0, am), [(b0 * TAU).cos(), (b0 * TAU).sin()]);
            (a, b) = ([p * n0[0] - sm * -n0[1], p * n0[1] - sm * n0[0]], [p * n0[0] + sm * -n0[1], p * n0[1] + sm * n0[0]]);
            (pp, q) = if i == 1 { ([0.0, mix(10.0, 12.0, r4), mix(32.0, 38.0, r3), mix(4.0, 7.0, r2)], [kq, 1.8, mix(1.6, 2.2, r1)]) }
                else { ([1.0, mix(20.0, 24.0, r4), mix(45.0, 52.0, r3), mix(0.0, 2.0, r5)], [kq, 2.4, 2.4]) };
        }
        // the road's plan in metres; its lamps in size units, one every spr = sp x Size m (the line's parts: s over Size)
        let len = hypot(b[0] - a[0], b[1] - a[1]);
        let (t, sp, hl, em, wet) = ([(b[0] - a[0]) / len, (b[1] - a[1]) / len], pp[2], pp[1], pp[3].abs(), pp[3] < 0.0);
        let spr = sp * sz;
        let nmax = (len / spr).floor() as i64;
        let on_land = |k: i64| { let (x, y) = (a[0] + k as f64 * spr * t[0], a[1] + k as f64 * spr * t[1]); wet || z.land(fract(y.atan2(x) / TAU)) > 0.5 }; // a land road stops at the shore
        let mut k0 = 0;
        while k0 <= nmax && !on_land(k0) { k0 += 1; }
        let mut k1 = k0;
        while k1 < nmax && on_land(k1 + 1) { k1 += 1; }
        if k0 > nmax { continue; }
        let (hide_c, tw, s0, s1) = ((i == 1 || i == 3) && z.lamp_c[1] > 0.0, pp[0] != 0.0, k0 as f64 * spr, k1 as f64 * spr);
        c.add(Instance { line: true, o: a, ang: t[1].atan2(t[0]), len, smar: 80.0 * sz, top: em + 4.0, hide_c, ..I0 }); // the embankment, the lamps' light pools
        if em > 0.0 { c.part(Band, [if wet { -70.0 } else { s0 / sz - 20.0 }, 0., if wet { len / sz + 10.0 } else { s1 / sz + 20.0 }, em], [30., if wet { 1e-3 } else { 25.0 }, 1., 0.]); }
        c.part(LRow, [if wet { 0.0 } else { s0 / sz }, em + 0.05, if wet { len / sz } else { s1 / sz }, 3.], [sp, 0., if wet { 0.04 } else { 0.03 } * q[1], q[0]]);
        for k in k0..=k1 {
            let pl = [a[0] + k as f64 * spr * t[0], a[1] + k as f64 * spr * t[1]];
            let (dl, ul) = (hypot(pl[0], pl[1]), fract(pl[1].atan2(pl[0]) / TAU));
            if dl < 420.0 { continue; }
            let mut nr = [-t[1], t[0]];
            if nr[0] * pl[0] + nr[1] * pl[1] < 0.0 { nr = [-nr[0], -nr[1]]; } // the arm reaches over the road, away from the camera
            let ax = (nr[0] * -(ul * TAU).sin() + nr[1] * (ul * TAU).cos()) * q[2];
            let (sx, y, a2, kw) = (if ax < 0.0 { -1.0 } else { 1.0 }, em + hl, ax.abs(), if tw { 1.7 } else { 1.0 });
            c.add(Instance { uc: ul, d: dl, hw: 4.0, top: em + hl + 1.5, hide_c, glow: 1.5, ..I0 });
            c.trap(0.0, em, 0.12 * kw, 0.07 * kw, hl - 0.2, 1.0); // pole
            if !tw { // a bracket arm curving up, the luminaire at its end, its lens facing down
                c.seg([0., y - 0.7, ax * 0.45, y - 0.05], 0.05, 0.05, 1.0); c.seg([ax * 0.45, y - 0.05, ax, y], 0.05, 0.05, 1.0); c.trap(ax + sx * 0.15, y - 0.09, 0.3, 0.3, 0.18, 1.0);
                c.part(Light, [ax + sx * 0.12, y - 0.17, 0.24, 1.7], [0., q[1], 0., q[0]]);
            } else { // a twin-arm mast: one arm over each carriageway
                c.seg([-a2, y, a2, y], 0.07, 0.07, 1.0);
                for sg in [-1.0, 1.0] { c.trap(sg * a2, y - 0.11, 0.4, 0.4, 0.22, 1.0); c.part(Light, [sg * a2, y - 0.19, 0.3, 1.7], [0., q[1], 0., q[0]]); }
            }
        }
    }
}
