//! Distant lights (the page's `pkFar`): villages far off as clusters of small lights on the horizon, some cells a road of
//! lamps, 5.5-12 km out (across the water up to 1.6 times farther), each lamp on the near rise at its own bearing.
use super::kit::{Ctx, Pt::*, I0, Instance, EYE};
use crate::params::P;
use crate::rng::{fract, mix, smooth, TAU};

pub fn build(c: &mut Ctx) {
    let am = c.p[P::PrFar];
    if am <= 0.0 { return; }
    for cf in (0..72).map(|ci| ci as f64) { // cells of 5 degrees: a road, a village (fewer at sea) or nothing
        let [r1, r2, r3, r4] = [1.0, 2.0, 3.0, 4.0].map(|k| c.rnd(cf, k, 1910.0));
        let uc = fract((cf + 0.5 + (r2 - 0.5) * 0.25) / 72.0);
        let (sea, road) = (1.0 - c.z.land(uc), r1 <= 0.2 * am);
        if !road && r1 > 0.2 * am + 0.55 * am * mix(1.0, 0.5, sea) { continue; }
        // dk: where lights of size units subtend what they do (d / Size): a village keeps its share of its cell, its lamps
        // Size times as large and as many over Size
        let d = mix(5500.0, 12000.0, r3) * mix(1.0, 1.6, sea);
        let dk = d / c.k;
        let (half, hv, sp, b0) = (if road { 0.48 } else { mix(0.12, 0.42, r4 * r4) } * TAU * dk / 72.0,
            if road { 9.0 } else { mix(12.0, 45.0, r4) }, if road { mix(32.0, 45.0, r2) } else { mix(22.0, 40.0, r3) }, -EYE / dk);
        let mut lt = vec![];
        for (j, id) in ((-half / sp).floor() as i64 - 1..=(half / sp).ceil() as i64).map(|j| (j as f64, cf * 1000.0 + j as f64)) {
            let lx = (j + 0.2 + 0.6 * c.rnd(id, 6.0, 1911.0)) * sp; // villages thin out toward their edges; most lights low, a few up the slope
            if c.rnd(id, 5.0, 1911.0) > if road { 0.9 } else { 0.62 * (1.0 - 0.7 * smooth(0.4, 1.0, lx.abs() / half)) } || lx.abs() > half { continue; }
            let ly = if road { hv } else { hv * c.rnd(id, 7.0, 1911.0) * c.rnd(id, 8.0, 1911.0) * (1.0 - 0.5 * lx.abs() / half) };
            lt.push([lx, ly, b0.max(c.z.rise(uc + lx / (TAU * dk))), // sodium or white
                if road { 0.72 } else if c.rnd(id, 9.0, 1911.0) < 0.6 { 0.7 } else { 1.35 }, 3.2 * mix(0.45, 1.0, c.rnd(id, 10.0, 1911.0))]);
        }
        for ch in lt.chunks(12) { // lights only, at most 12 per instance
            let (xc, base) = (0.5 * (ch[0][0] + ch[ch.len() - 1][0]), ch.iter().fold(f64::MAX, |m, l| m.min(l[2])));
            let top = ch.iter().fold(f64::MIN, |m, l| m.max((l[2] - base) * dk + l[1]));
            c.add(Instance { uc: fract(uc + xc / (TAU * dk)), d, base, hw: 0.5 * (ch[ch.len() - 1][0] - ch[0][0]) + 2.0, glow: 40.0, top: top + 4.0, hide_c: true, hide_m: true, ..I0 });
            for l in ch { c.part(Light, [l[0] - xc, (l[2] - base) * dk + l[1], 0.6, 1.], [0., l[4], 0., l[3]]); }
        }
    }
}
