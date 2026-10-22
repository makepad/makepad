//! Power lines (the page's `pkPylons`): up to three straight lines on open land, a lattice tower every 300-400 m in
//! true perspective (double circuit or single-circuit cat-head), the wires as parallel sagging spans. Unlit.
use super::kit::{hypot, Ctx, Pt::*, I0, Instance, EYE};
use crate::params::P;
use crate::rng::{fract, mix, TAU};

pub fn build(c: &mut Ctx) {
    let (am, z, sz) = (c.p[P::PrPyl], c.z, c.k);
    if am <= 0.0 { return; }
    for l in 0..3 {
        let (zz, fl) = (z.pyl[l], l as f64);
        if zz[3] < 0.5 || zz[1] < 1.0 || (l == 1 && am <= 0.3) || (l == 2 && am <= 0.65) { continue; }
        let r = |st: f64| z.rnd(fl, st, 1100.0);
        // the line's plan in metres, its towers in size units: one every lsp size units (lspr m); the wires' parts are in
        // size units along the line (s over Size)
        let (p, h, ty, lsp, rc) = (zz[1], 32.0 + 22.0 * r(3.0), r(4.0) >= 0.5, 300.0 + 100.0 * r(1.0), mix(4500.0, zz[2], am));
        let lspr = lsp * sz;
        let (s0, reach, reach2, ins, b0) = (lspr * (r(2.0) - 0.5), rc * (0.88 + 0.24 * r(5.0)), rc * (0.88 + 0.24 * r(6.0)), 0.075 * h, zz[0] * TAU);
        let (na, nb) = (((-(reach * reach - p * p).max(0.0).sqrt() - s0) / lspr).ceil() as i64, (((reach2 * reach2 - p * p).max(0.0).sqrt() - s0) / lspr).floor() as i64);
        let (n0, t) = ([b0.cos(), b0.sin()], [-b0.sin(), b0.cos()]); // the line: nearest point at bearing zz[0], distance p; toward larger bearings
        let has = |n: i64| n >= na && n <= nb && z.open(fract(zz[0] + (s0 + n as f64 * lspr).atan2(p) / TAU)) > 0.5;
        let arms = [0.18, 0.22, 0.16];
        for n in (na..=nb).filter(|&n| has(n)) { // towers
            let sn = s0 + n as f64 * lspr;
            let dj = hypot(p, sn);
            // (the tower's perspective in its size units: its distance and the eye over Size)
            let (uc, pc, lw) = (fract(zz[0] + sn.atan2(p) / TAU), [sn / dj, p / dj, dj / sz, EYE], (0.005 * h).max(0.12));
            // a tower point (across, along the line, height) onto its plane
            let pr = |a: f64, b: f64, y: f64| { let sc = pc[2] / (pc[2] + a * pc[1] + b * pc[0]); [(b * pc[1] - a * pc[0]) * sc, pc[3] + (y - pc[3]) * sc] };
            let pseg = |c: &mut Ctx, a: [f64; 2], b: [f64; 2], w0: f64, w1: f64, o: f64| c.seg([a[0], a[1], b[0], b[1]], w0, w1, o);
            let fb = pc[0].abs() + pc[1];
            c.add(Instance { uc, d: dj, hw: if ty { 0.36 } else { 0.22 } * h + 6.0, top: h + 1.0, hide_c: true, hide_m: true, seed: (n * 7 + l as i64) as u32, ..I0 });
            let (b0, wt, yb) = if ty { (0.13 * h, 0.035 * h, 0.55 * h) } else { (0.115 * h, 0.03 * h, 0.6 * h) };
            for j in 0..3 { // tapered lattice legs
                let (y0, y1, w0, w1) = (yb * j as f64 / 3.0, yb * (j + 1) as f64 / 3.0, mix(b0, wt, j as f64 / 3.0), mix(b0, wt, (j + 1) as f64 / 3.0));
                c.part(Lat, [0., y0, 0., y1], [w0 * fb, w1 * fb, 1.6 * (w0 + w1) * 0.5, lw]);
            }
            if !ty { // double circuit: three crossarms with insulator strings, an earth-wire peak
                c.part(Lat, [0., yb, 0., 0.93 * h], [wt * fb, 0.8 * wt * fb, 2.0 * wt, lw * 0.85]); c.trap(0.0, 0.93 * h, 0.8 * wt * fb, 0.1, 0.07 * h, 0.8);
                for (i, aa) in arms.iter().map(|x| h * x).enumerate() {
                    let ha = h * (0.63 + 0.135 * i as f64);
                    for sg in [-1.0, 1.0] {
                        let (c0, tip) = (pr(0.0, 0.0, ha), pr(sg * aa, 0.0, ha));
                        c.part(Lat, [c0[0], c0[1] + 0.025 * h, tip[0], tip[1] + 0.006 * h], [0.025 * h, 0.006 * h, (tip[0] - c0[0]).abs() / 3.0 + 0.01, lw * 0.8]);
                        pseg(c, pr(sg * 0.94 * aa, 0.0, ha), pr(sg * 0.94 * aa, 0.0, ha - ins), 0.16, 0.16, 0.9);
                    }
                }
            } else { // single-circuit cat-head: V legs, earth-wire peaks, one wide crossarm
                for sg in [-1.0, 1.0] {
                    for bb in [-wt, wt] { pseg(c, pr(sg * wt, bb, yb), pr(sg * 0.2 * h, bb, 0.8 * h), lw * 1.6, lw * 1.4, 0.85); }
                    for x0 in [0.12, 0.22] { pseg(c, pr(sg * x0 * h, 0.0, 0.84 * h), pr(sg * 0.17 * h, 0.0, h), lw * 1.2, lw * 0.8, 0.85); }
                    let (c0, tip) = (pr(0.0, 0.0, 0.8 * h), pr(sg * 0.36 * h, 0.0, 0.8 * h));
                    c.part(Lat, [c0[0], c0[1] + 0.0225 * h, tip[0], tip[1] + 0.01 * h], [0.0225 * h, 0.01 * h, (tip[0] - c0[0]).abs() / 3.0 + 0.01, lw * 0.8]);
                }
                for i in 0..3 { let x = 0.31 * h * (i as f64 - 1.0); pseg(c, pr(x, 0.0, 0.8 * h), pr(x, 0.0, 0.8 * h - ins), 0.16, 0.16, 0.9); }
            }
        }
        // the wires: one line frame per power line, each wire on its own parallel line, spans only between existing towers
        let wires: Vec<[f64; 4]> = if !ty {
            let w = |i: usize, sg: f64| [sg * 0.94 * h * arms[i], h * (0.63 + 0.135 * i as f64) - ins, 0.032 + 0.004 * i as f64, 0.06];
            (0..3).flat_map(|i| [w(i, -1.0), w(i, 1.0)]).chain([[0.0, h, 0.024, 0.025]]).collect()
        }
            else { (0..3).map(|k| [0.31 * h * (k as f64 - 1.0), 0.8 * h - ins, 0.033 + 0.003 * k as f64, 0.06])
                .chain([[-0.17 * h, h, 0.024, 0.025], [0.17 * h, h, 0.024, 0.025]]).collect() };
        let (mut runs, mut r0) = (vec![], None);
        for n in na..=nb + 1 { if n <= nb && has(n) { r0 = r0.or(Some(n)); } else if let Some(a) = r0.take() { if n - 1 > a { runs.push((a as f64 * lspr, (n - 1) as f64 * lspr)); } } }
        if runs.is_empty() { continue; }
        let (smin, smax) = (runs[0].0, runs[runs.len() - 1].1); // s along the wires from the first tower of the first run
        c.add(Instance { line: true, o: [p * n0[0] + (s0 + smin) * t[0], p * n0[1] + (s0 + smin) * t[1]],
            ang: t[1].atan2(t[0]), len: smax - smin, smar: 0.0, top: h + 2.0, hide_c: true, hide_m: true, ..I0 });
        let sgn = if -t[1] * n0[0] + t[0] * n0[1] > 0.0 { 1.0 } else { -1.0 }; // lateral offsets go to the line's left
        for &(a, b) in &runs {
            for (k, w) in wires.iter().enumerate() {
                c.part_c(Arc, [(a - smin) / sz, w[1], (b - smin) / sz, w[1]], [w[2] * lsp * (0.95 + 0.1 * z.rnd(fl * 8.0 + k as f64, 8.0, 1100.0)), w[3], 1., lsp], [sgn * w[0], 0., 1.]);
            }
        }
    }
}
