//! Farms (the page's `pkFarms`) on open treeless land: a farmhouse with a lit window, a barn, a silo, sometimes a water
//! tower, a yard light on its pole. Cells of 4 degrees, 900-2600 m out, where the land is open and has no front treeline.
use super::kit::{Ctx, Pt::*, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix};

pub fn build(c: &mut Ctx) {
    let am = c.p[P::PrFarm];
    if am <= 0.0 { return; }
    for cf in (0..90).map(|ci| ci as f64) {
        let [r1, r2, r3, r4, r5, r6] = [1.0, 2.0, 3.0, 4.0, 5.0, 7.0].map(|k| c.rnd(cf, k, 1920.0));
        let uc = fract((cf + 0.5 + (r2 - 0.5) * 0.5) / 90.0);
        if r1 > am * 0.7 * c.z.open(uc) * (1.0 - c.z.tr_n(uc)) / c.k { continue; } // Size times as large: as many over Size
        let (d, sg, wh, hw) = (mix(900.0, 2600.0, r3), if r4 < 0.5 { -1.0 } else { 1.0 }, mix(9.0, 14.0, r5), mix(5.0, 6.5, r2));
        let (xb, wb, hb, rs, hs, wt, ht) = (sg * mix(24.0, 38.0, r6), mix(16.0, 28.0, r4), mix(5.5, 8.0, r5), mix(2.6, 3.8, r3), mix(12.0, 22.0, r4), r5 > 0.75, mix(24.0, 34.0, r3));
        let (xs, xt) = (xb + sg * (0.5 * wb + 5.0), -sg * mix(30.0, 55.0, r2));
        let hwi = (xb.abs() + 0.5 * wb).max(xs.abs() + rs).max(if wt { xt.abs() + 7.0 } else { 0.0 }) + 2.0;
        c.add(Instance { uc, d, base: (-c.eye() / d).max(c.z.rise(uc)) - 1.5 * c.k / d, hw: hwi, // the foot sunk 1.5 size units
            top: if wt { ht + 9.0 } else { (hs + rs).max(hb + 6.5).max(11.2) + 1.0 }, hide_c: true, ..I0 });
        c.gable(0.0, wh, hw, hw + mix(3.0, 4.5, r3)); c.bx(0.25 * wh, hw + 1.4, hw + 4.6, 0.5); c.gable(xb, wb, hb, hb + mix(4.0, 6.5, r2)); // farmhouse, its chimney, the barn
        c.bx(xs, 0.0, hs, rs); c.ell(xs, hs, rs, rs / 1.6); // silo and its dome
        if wt { c.bx(xt, 0.0, ht, 1.5); c.ell(xt, ht + 3.5, 7.0, 4.5); } // sometimes a water tower
        if c.rnd(cf, 6.0, 1920.0) <= 1.3 * c.p[P::WinD] {
            c.part(LBox, [-0.2 * wh, 2.6, 0.6, 0.6], [0., 0.35, 0., 0.8]); if r3 >= 0.5 { c.part(LBox, [0.18 * wh, 2.6, 0.6, 0.6], [0., 0.35, 0., 0.8]); }
        }
        if r2 >= 0.3 { c.trap(0.5 * xb, 0.0, 0.08, 0.06, 6.9, 1.0); c.light(0.5 * xb, 7.0, 0.35, 1.4, 0.72); } // a sodium yard light on its pole
    }
}
