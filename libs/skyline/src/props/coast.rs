//! The coast (the page's `pkCoast`): a lighthouse on a headland 40 m inland of a real shoreline (lantern, a faint beam in
//! the haze), and ships with their lights out at sea, one per 10 degree cell of open sea at most, 2.5-11 km out.
use super::kit::{Ctx, Pt::*, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix, TAU};
use crate::zones::Zones;

pub fn build(c: &mut Ctx) {
    let (am, z) = (c.p[P::PrCoast], c.z);
    if am <= 0.0 { return; }
    if z.shore[2] > 0.5 { // land lies toward smaller u from shore[0]
        let (us, side) = if c.rnd(1.0, 1.0, 1930.0) >= 0.5 { (z.shore[0], -1.0) } else { (z.shore[1], 1.0) };
        let d = mix(1800.0, 3200.0, c.rnd(2.0, 1.0, 1930.0));
        let (uc, h, bd) = (fract(us + side * 40.0 * c.k / (TAU * d)), mix(24.0, 40.0, c.rnd(3.0, 1.0, 1930.0)), if c.rnd(4.0, 1.0, 1930.0) < 0.5 { -1.0 } else { 1.0 });
        c.add(Instance { uc, d, base: (-c.eye() / d).max(0.6 * z.rise(uc)) - c.k / d, hw: 800.0, top: h + 80.0, hide_c: true, ..I0 });
        c.trap(0.0, 0.0, 3.6, 2.2, h, 1.0); c.bx(0.0, h - 0.3, h + 1.0, 3.3); c.trap(0.0, h + 0.8, 1.9, 1.9, h + 4.7 - (h + 0.8), 0.55); // tower, gallery, lantern room, cap
        c.part(Ell, [0., h + 4.6, 2.1, 1.5], [0., 1., 1., 0.]);
        c.gable(-side * 9.0, 9.0, 4.0, 7.0); c.light(0.0, h + 2.8, 1.0, 7.0, 1.25); // the keeper's house, the lantern, a beam frozen mid-sweep
        c.part(Beam, [0., h + 2.8, bd / 260.0, -0.03 / 260.0], [1.2 / 260.0, 0.08, 0.10 * (0.3 + 0.7 * c.p[P::Haze]), 1.25]);
    }
    for cf in (0..36).map(|ci| ci as f64) {
        let [s1, s2, s3, s4, s5, s6] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0].map(|k| c.rnd(cf, k, 1931.0));
        let uc = fract((cf + 0.5 + (s2 - 0.5) * 0.5) / 36.0);
        // Size times as large: as many over Size
        if s1 >= am * 0.6 * (1.0 - z.land(uc)) * (1.0 - Zones::in_z(uc, &z.bridge)) * (1.0 - Zones::in_z(uc, &z.fair)) / c.k { continue; }
        let (d, ty) = (mix(2500.0, 11000.0, s3 * s3), if s4 < 0.45 { 0 } else if s4 < 0.8 { 1 } else { 2 }); // cargo / tanker, ferry / cruise ship, small boat
        if !crate::at_sea(c.p, d) { continue; } // nearer than the sea distance: that is the shore
        let ln = [mix(120.0, 240.0, s5), mix(140.0, 280.0, s5), mix(14.0, 30.0, s5)][ty] * mix(0.35, 1.0, s6); // foreshortened by its heading
        let (fb, g, o) = ([mix(8.0, 13.0, s2), 10.0, 2.5][ty], if s6 < 0.5 { -1.0 } else { 1.0 }, 0.5); // g: the bow toward +x or -x
        let (hl, hs) = (0.5 * ln, fb + if ty == 0 { mix(12.0, 20.0, s5) } else { mix(18.0, 32.0, s5) });
        let (xm, hm) = if ty < 2 { (0.25 * ln, hs + 8.0) } else { (0.0, fb + 3.0 + 5.0) };
        c.add(Instance { uc, d, hw: hl + 15.0, top: hm + 2.0, hide_c: true, hide_m: true, seed: (cf * 5.0 + 3.0) as u32, ..I0 });
        // a hull with a raked bow (the waterline ends .6 fb short of the stem), the sheer rising a quarter toward the bow
        c.bx(g * -0.3 * fb, 0.0, fb, hl - 0.3 * fb); c.trap(g * (hl - 0.9 * fb), 0.0, 0.3 * fb, 0.9 * fb, fb, 1.0);
        c.seg([g * 0.6 * hl, fb - 0.125 * fb, g * (hl - 0.125 * fb), 1.125 * fb], 0.125 * fb, 0.125 * fb, 1.0);
        if ty == 0 { // superstructure aft, a funnel, deck lights
            let x0 = -hl + 0.04 * ln;
            let x1 = x0 + 0.13 * ln;
            c.bx(g * 0.5 * (x0 + x1), fb - o, hs, 0.5 * (x1 - x0)); c.bx(g * (x0 + 0.03 * ln), hs - o, hs + 6.0, 0.02 * ln + 1.0);
            for k in 0..4 { c.light(g * (x1 + (hl - x1) * (0.2 + 0.2 * k as f64)), fb + 2.0, 0.4, 1.2, 0.75); }
        } else if ty == 1 { c.bx(g * -0.04 * ln, fb - o, hs, 0.38 * ln); // cabin windows
        c.part(Win, [g * -0.04 * ln, 0.5 * (fb + hs), 0.38 * ln, 0.5 * (hs - fb)], [2.4, 3.2, 0.5 * c.p[P::WinD] + 0.03, 0.85]); }
        else { c.bx(g * -0.1 * ln, fb - o, fb + 3.0, 0.2 * ln); } // a small boat's cabin
        c.bx(g * xm, fb - o, hm, 0.25); c.light(g * xm, hm, 0.4, 2.2, 1.3); // the mast and its masthead light, a red or green sidelight
        c.part(CLight, [g * 0.42 * ln, fb + 1.0, 0.3, 1.2], if g * (s5 - 0.5) > 0.0 { [1., 0.08, 0.04, 0.] } else { [0.1, 1., 0.3, 0.] });
    }
}
