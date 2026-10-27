//! Wind turbines (the page's `pkTurbines`): slot 0 on mountain range k, or 4.5 km out on open flat land, or 8 km
//! offshore; one model per farm in up to three rows, rotors facing the farm's wind, most with a red nacelle beacon.
use super::kit::{Ctx, Pt::*, I0, Instance, EYE};
use crate::params::P;
use crate::rng::{fract, hb4, hu, TAU};
use crate::zones::mt_dist;

pub fn build(c: &mut Ctx) {
    let (am, sz) = (c.p[P::PrTurb], c.k);
    if am <= 0.0 { return; }
    let (rows, occ) = (1 + (am > 0.2) as i32 + (am > 0.55) as i32, 0.62 + 0.3 * am);
    for f in 0..2 {
        let z = c.z.turb[f];
        if z[1] <= 0.0 || (f == 1 && am <= 0.45) { continue; }
        let (k, mode, rf) = ((z[2] + 0.5).floor(), z[3], hb4(c.z.rng.pk_hash(f as f64, 1200.0)));
        let h0 = 85.0 + 35.0 * rf[0];
        let r0 = h0 * (0.47 + 0.08 * rf[1]);
        let (cs, dz) = (2.0 * r0 * (3.0 + 0.6 * rf[2]), if mode > 1.5 { if mode < 2.5 { 4500.0 } else { 8000.0 } } else { mt_dist(k) });
        for fr in (0..rows).rev().map(|r| r as f64) {
            // dk: where a turbine of size units subtends what it does (d / Size); the cells of cs size units fill the farm
            let d = dz * (1.0 + if mode > 1.5 { 0.14 } else { 0.09 } * fr);
            let dk = d / sz;
            let (ly, off, jit, xm) = (1201.0 + 4.0 * f as f64 + fr, 0.37 * fr, 1.0 - 2.3 * r0 / cs, (z[1] + 0.003) * TAU * dk);
            for ci in ((-xm / cs + off).floor() - 1.0) as i64..=((xm / cs + off).floor() + 1.0) as i64 {
                let h = c.z.rng.pk_hash(ci as f64, ly);
                let r = hb4(h);
                let xc = (ci as f64 + 0.5 - off + (r[1] - 0.5) * jit) * cs;
                if r[0] > occ || (xc / (TAU * dk)).abs() > z[1] * (0.8 + 0.2 * r[2]) { continue; }
                let (uc, r1, mut base) = (fract(z[0] + xc / (TAU * dk)), hb4(hu(h)), -EYE / dk);
                if mode > 2.5 { if c.z.land(uc) > 0.3 { continue; } } // offshore: standing in the sea
                else if mode > 1.5 || c.p[P::MtW] < 0.04 { if c.z.open(uc) < 0.5 { continue; } } // big open flat land
                else { // gentle ground on the lower half of the range; row 0 on the crest, the others behind it
                    let hr = c.z.ridges(uc)[k as usize];
                    if !c.z.turb_ok(uc, k as usize, d, hr) { continue; }
                    base = (hr - (if fr < 0.5 { 1.5 } else { 8.0 + 25.0 * r1[0] }) / dk).max(base);
                }
                let (sc, yaw, th0) = (0.97 + 0.06 * r[3], (rf[3] - uc) * TAU + (r1[1] - 0.5) * 0.14, r1[2] * TAU);
                let (hh, rr, sy, cy) = (h0 * sc, r0 * sc, yaw.sin(), yaw.cos());
                let (hx, fw, lit) = (6.5 * sy.abs() + 2.0 * cy.abs(), 0.5 + 0.5 * cy.abs(), if fr < 0.5 { 0.6 } else { 0.25 });
                c.add(Instance { slot: 0, range: k as u8, uc, d, base, hw: rr * 1.03 + 8.0, top: hh + rr + 4.0, td: dz, seed: h, glow: 2.0, ..I0 });
                c.trap(0.0, 0.0, 2.2, 1.25, hh, 1.0); c.trap(-2.2 * sy, hh - 1.6, hx, hx, 3.8, 1.0); c.ell(5.0 * sy, hh, 1.6, 1.6); // tower, nacelle, hub
                for i in 0..3 { let th = th0 + i as f64 * 2.0944; c.seg([5.0 * sy, hh, 5.0 * sy + rr * th.cos() * cy, hh + rr * th.sin()], 1.5 * fw, 0.3 * fw, 1.0); } // blades
                if r1[3] < lit { c.part(Beacon, [-3.0 * sy, hh + 2.6, 0., 0.], [0.3, 0., 0., 0.]); }
            }
        }
    }
}
