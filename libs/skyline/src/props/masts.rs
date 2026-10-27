//! Radio and TV masts (the page's `pkMasts`): guyed lattice masts with red lights up their height, or a concrete TV
//! tower with a lit pod, on the skyline ridge (range 3 or 4) or on open treeless land. Zone: `z.mast_*`.
use super::kit::{Ctx, Pt::*, I0, Instance};
use crate::params::P;

pub fn build(c: &mut Ctx) {
    if c.p[P::PrMast] <= 0.0 { return; }
    for i in (0..4).filter(|&i| c.z.mast_u[i] >= 0.0) {
        let (uc, k, d, h) = (c.z.mast_u[i], c.z.mast_k[i], c.z.mast_d[i], c.z.mast_h[i]);
        let (eye, sz) = (c.eye(), c.k);
        let base = if k < -0.5 { (-eye / d).max(c.z.rise(uc)) - 1.5 * sz / d } else { c.z.ridges(uc)[k as usize].max(-eye / d) - 2.0 * sz / d }; // feet sunk 1.5 (land) / 2 (ridge) size units
        c.add(Instance { uc, d, base, hw: 0.75 * h + 15.0, top: 1.05 * h + 3.0, hide_c: true, hide_m: true, ..I0 });
        if c.rnd(i as f64, 3.0, 1900.0) <= 0.3 && k < -0.5 { // a concrete TV tower on open land: shaft, pod, antenna, lit pod windows
            c.trap(0.0, 0.0, 8.0, 3.5, 0.72 * h, 1.0); c.bx(0.0, 0.7 * h, 0.86 * h, 3.5); c.ell(0.0, 0.72 * h, 13.0, 6.0); c.bx(0.0, 0.8 * h, h, 1.0);
            if c.p[P::WinD] > 0.0 { c.part(LBox, [0., 0.72 * h, 12., 0.8], [0., 0.3 * c.p[P::WinD], 0., 0.8]); }
            c.beacon(0.0, h + 1.0, 1.0, d); c.beacon(0.0, 0.86 * h, 0.8, d);
        } else { // a guyed mast: guy wires down to anchors on both sides, red lights up the mast, a little stronger than the city's
            c.bx(0.0, 0.0, h, 1.2);
            for (yk, xk) in (0..3).map(|g| (h * (0.32 + 0.31 * g as f64), h * (0.30 + 0.12 * g as f64))) {
                c.seg([0., yk, xk, 0.], 0.05, 0.05, 1.0); c.seg([0., yk, -0.85 * xk, 0.], 0.05, 0.05, 1.0); c.beacon(0.0, yk, 0.9, d);
            }
            c.beacon(0.0, h + 0.8, 1.3, d);
        }
    }
}
