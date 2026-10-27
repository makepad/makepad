//! Industry (the page's `pkIndustry`): slot 1, seen with the city camera. Three rows of ~95 m lots beside the city,
//! from a power-plant end (cooling towers, boiler houses, chimneys) through factory halls and steel stacks to a chemical
//! end (process units with many small lights, tanks, a flare); every lot an instance, every plume its own just behind it.
use super::kit::{clamp, hypot, Ctx, Pt::{self, *}, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix, smooth, TAU};

/// A part with its extent (|x| + half-width, top).
type Pe = (Pt, [f64; 4], [f64; 4], f64, f64);
/// A lot's parts: bodies, then lights.
#[derive(Default)]
struct Lot { bp: Vec<Pe>, lp: Vec<Pe> }

impl Lot {
    fn pt(&mut self, t: Pt, a: [f64; 4], b: [f64; 4], x: f64, y: f64, li: bool) { if li { &mut self.lp } else { &mut self.bp }.push((t, a, b, x.abs(), y)); }
    fn trap(&mut self, x: f64, y: f64, w0: f64, w1: f64, h: f64, o: f64) { self.pt(Trap, [x, y, w0, w1], [h, 0., o, 0.], x.abs() + w0.max(w1), y + h, false); }
    fn seg(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, w: f64, o: f64) { self.pt(Seg, [x0, y0, x1, y1], [w, w, o, 0.], x0.abs().max(x1.abs()) + w, y0.max(y1) + w, false); }
    fn gable(&mut self, a: [f64; 4], xa: f64, ya: f64, n: f64) { self.pt(Gable, a, [xa, ya, 1., n], a[0].abs().max((a[0] + n * (a[2] - a[0])).abs()), ya, false); }
    fn lamp(&mut self, x: f64, y: f64, r: f64, amp: f64, k: f64, sq: f64) { self.pt(Light, [x, y, r, sq], [0., amp, 0., k], x.abs() + 3.0 * r + 1.0, y + 3.0 * r + 1.0, true); }
    fn beacon(&mut self, x: f64, y: f64) { self.pt(Beacon, [x, y, 0., 0.], [0.; 4], x.abs() + 2.0, y + 2.0, true); }
    /// One lit pane w x h (a window texture that lights it whole).
    fn pane(&mut self, x: f64, y: f64, w: f64, h: f64) { let py = h / 0.45; self.pt(Win, [x, y - 0.025 * py, w, 0.5 * py], [2.0 * w, py, 1., 1.2], x.abs() + w, y + h, true); }
    /// A tapering chimney with a rim band at its top.
    fn chimney(&mut self, x: f64, h: f64, rb: f64, rt: f64) { self.trap(x, 0.0, rb, rt, h, 1.0); self.trap(x, h - 2.2, mix(rb, rt, (h - 2.2) / h) + 0.35, rt + 0.35, 2.2, 1.0); }
}

/// The literal 3.14159 is the page's: the saw-tooth roofs' layout depends on it.
#[allow(clippy::approx_constant)]
pub fn build(c: &mut Ctx) {
    let (z, p, zs) = (c.z.ind, c.p, c.z);
    if p[P::PrInd] <= 0.0 || z[1] <= 0.0 { return; }
    let (rnd, wd) = (|id: f64, st: f64, ly: f64| zs.rnd(id, st, ly), p[P::WinD]);
    let h4 = |x: f64, y: f64, k: i32| { let h = zs.rng.hcell(x as i32, y as i32, k); [0, 8, 16, 24].map(|s| ((h >> s) & 255) as f64 * 0.003921569) };
    let cm = 0.8 * p[P::Off] * 28.0 * p[P::CtR] * 70.0;
    let cam = [cm * (p[P::OffDir] * TAU).cos(), cm * (p[P::OffDir] * TAU).sin()]; // the city camera (m)
    // one world wind; the power-plant end; two cooling towers?
    let (wa, zsd, pair) = (rnd(2.0, 9.0, 1400.0) * TAU, if rnd(1.0, 9.0, 1400.0) < 0.5 { -1.0 } else { 1.0 }, rnd(3.0, 9.0, 1400.0) <= 0.65);
    let wind = |uc: f64| wa.cos() * -(uc * TAU).sin() + wa.sin() * (uc * TAU).cos();
    for fr in (0..3).map(|f| f as f64) {
        let rr = mix(z[3], z[2], (fr + 0.5) / 3.0);
        let cbu = 95.0 * c.k / (rr * TAU); // row distance (far to near), lot width (u; 95 size units, so as many over Size)
        let (jct, jfl) = ((-0.62 * zsd * z[1] / cbu).floor(), (0.62 * zsd * z[1] / cbu).floor());
        let jct2 = jct + if (jct + 1.5).abs() * cbu < z[1] { 1.0 } else { -1.0 };
        for j in (-6..=5).map(|j| j as f64) {
            let (cpos, id) = ((j + 0.5) * cbu, fr * 16.0 + j + 6.0);
            if cpos.abs() > z[1] { continue; }
            let hh = h4(id, fr, 1401);
            let (t, spn, ct) = (hh[0], clamp(cpos / z[1], -1.0, 1.0) * zsd, fr == 0.0 && (j == jct || (pair && j == jct2))); // the cooling tower(s); the flare in row 1
            let pick = |v: [(f64, u8); 4], e: u8| v.iter().find(|q| t < q.0).map_or(e, |q| q.1);
            let ty = if ct { 3 } else if fr == 1.0 && j == jfl { 8 } else if spn < -0.33 { pick([(0.1, 0), (0.35, 7), (0.55, 2), (0.85, 1)], 9) }
                else if spn < 0.33 { pick([(0.08, 0), (0.48, 1), (0.65, 2), (0.8, 6)], 9) } else { pick([(0.05, 0), (0.45, 5), (0.75, 4), (0.9, 6)], 1) };
            let (b, rad) = (z[0] + cpos + if ct { 0.0 } else { (hh[1] - 0.5) * 0.45 * cbu }, rr + if ct { 0.0 } else { (hh[2] - 0.5) * 0.55 * (z[3] - z[2]) / 3.0 });
            if ty == 0 || c.z.land(fract(b)) < 0.5 { continue; } // no industry on the water
            let q = [rad * (b * TAU).cos() - cam[0], rad * (b * TAU).sin() - cam[1]];
            let d = hypot(q[0], q[1]);
            if d < 150.0 { continue; }
            let (uc, hid) = (fract(q[1].atan2(q[0]) / TAU), if ct { fr * 16.0 + jct + 6.0 } else { id }); // a pair of cooling towers is built alike
            let (h, g, nk) = (h4(hid, fr + 5.0, 1402), h4(hid, fr + 9.0, 1403), clamp((d / 400.0).ln() / 12.5f64.ln(), 0.0, 1.0));
            let td = 400.0 * 12.5f64.powf(clamp(((0.92 + 0.16 * g[3]) * (0.6 + 0.4 * nk) - 0.6) / 0.4, 0.0, 1.0)); // a little tone variation from lot to lot
            let (mut l, mut pl, mut fl) = (Lot::default(), None, false);
            match ty {
                1 => { // factory hall: north-light saw-tooth or flat roof with roof units; sodium wall lamps, a few lit ribbon windows
                    let (hw, hh) = (22.0 + 20.0 * h[0], 13.0 + 22.0 * h[1] * h[1]);
                    if h[2] >= 0.4 {
                        let (per, th) = (9.0 + 5.0 * h[3], 3.5 + 1.5 * g[0]);
                        let tv = th * smooth(0.35, 0.9, (g[1] * 3.14159 - uc * TAU).cos().abs()); // the teeth show along the line of sight
                        let (hb, n) = (hh + th - tv, (2.0 * hw / per).floor());
                        l.trap(0.0, 0.0, hw, hw, hb, 1.0);
                        if tv > 0.05 && n > 0.0 { l.gable([-hw, hb - 4.0, per - hw, hb], per - hw, hb + tv, n); }
                        if tv > 0.05 && hw - (-hw + n * per) > 0.3 { l.gable([-hw + n * per, hb - 4.0, hw, hb], hw, hb + tv * (hw - (-hw + n * per)) / per, 1.0); }
                    } else { l.trap(0.0, 0.0, hw, hw, hh, 1.0); l.trap((g[2] - 0.5) * hw * 1.2, hh - 2.0, 2.5 + 3.0 * g[3], 2.5 + 3.0 * g[3], 4.8 + 0.8 * g[0], 1.0);
                    l.trap((g[3] - 0.5) * hw * 1.4, hh - 2.0, 1.6, 1.6, 3.8 + 0.6 * g[2], 1.0); }
                    let (sp, mut xk) = (24.0 + 12.0 * g[2], 0.0);
                    while (xk + 0.5) * sp < 2.0 * hw {
                        let lx = (xk + 0.5) * sp - hw; if lx.abs() < hw - 3.0 && rnd(hid * 64.0 + xk, 5.0, 1402.0) < 0.7 { l.lamp(lx, hh - 1.2, 0.3, 1.1, 0.6, 1.0); } xk += 1.0;
                    }
                    if wd > 0.0 {
                        for xi in 0..((2.0 * hw - 8.0) / 6.0).floor() as i64 {
                            if rnd(hid * 64.0 + (xi as f64 * 0.5).floor(), 7.0, 1402.0) < wd * 0.4 { l.pane(-hw + 7.0 + 6.0 * xi as f64, hh - 3.8, 4.6, 1.5); }
                        }
                    }
                    fl = g[3] >= 0.65;
                }
                2 => { // concrete chimney, a service gallery, beacons
                    let (hh, rb) = (58.0 + 92.0 * h[0] * h[0], 3.5 + 2.5 * h[1]);
                    let rt = rb * (0.55 + 0.15 * h[2]);
                    let r6 = mix(rb, rt, 0.86);
                    l.chimney(0.0, hh, rb, rt); l.seg(-r6 - 0.85, 0.86 * hh, r6 + 0.85, 0.86 * hh, 0.35, 1.0); l.beacon(rt * (g[0] - 0.5), hh - 0.8);
                    if hh > 100.0 { l.beacon(mix(rb, rt, 0.5) * if g[1] < 0.5 { -0.8 } else { 0.8 }, hh * 0.5); }
                    if h[3] < 0.45 { pl = Some([0.0, hh, rt * 1.1, 20.0 + 35.0 * g[2], 55.0 + 50.0 * g[3], 0.4, 190.0 + 70.0 * g[0], 0.95]); }
                }
                3 => { // hyperboloid cooling tower: three pieces fitted to its two hyperbolas, overlapping
                    let hh = 100.0 + 35.0 * h[0];
                    let rb = 0.36 * hh * (0.95 + 0.1 * h[1]);
                    let (yt, rt, rtt) = (0.79 * hh, 0.585 * rb, 0.635 * rb);
                    let (bl, bu) = (yt / (rb * rb / (rt * rt) - 1.0).sqrt(), (hh - yt) / (rtt * rtt / (rt * rt) - 1.0).sqrt());
                    let r = |y: f64| rt * (1.0 + (y - yt) * (y - yt) / (if y < yt { bl } else { bu }).powi(2)).sqrt();
                    let ov = 3f64.max(0.004 * d / c.k);
                    for (y0, y1) in [(0.0, 0.5 * yt + ov), (0.5 * yt, yt + ov), (yt, hh)] {
                        l.pt(Trap, [0., y0, r(y0), r(y1)], [y1 - y0, 0.5 * (r(y0) + r(y1)) - r(0.5 * (y0 + y1)), 1., 0.], r(y0).max(r(y1)), y0 + (y1 - y0), false);
                    }
                    l.beacon(1.5 - rtt, hh - 0.7); l.beacon(rtt - 1.5, hh - 0.7);
                    pl = Some([0.0, hh - 2.0, rtt * 0.8, 40.0 + 40.0 * g[2], 120.0, 0.4, 300.0, 1.15]);
                }
                4 => { // storage tanks: low cylinders with conical roofs, or spheres on legs
                    for k in (0..3).map(|k| k as f64) {
                        let (ha, hk) = (rnd(hid * 4.0 + k, 20.0, 1403.0), rnd(hid * 4.0 + k, 21.0, 1403.0));
                        let x = (k - 1.0) * 38.0 + (ha - 0.5) * 5.0;
                        if hk < 0.7 {
                            let (r, ht) = (10.0 + 6.0 * ha, 12.0 + 10.0 * hk); l.gable([x - r, -1.0, x + r, ht], x, ht + 1.8, 1.0);
                            if ha > 0.45 { l.lamp(x + r * 0.55, ht + 1.21, 0.25, 0.9, 0.6, 1.0); }
                        }
                        else {
                            let r = 7.0 + 2.5 * ha;
                            l.pt(Ell, [x, r + 5.0, r, r], [0., 0., 1., 0.], x.abs() + r, 2.0 * r + 5.0, false); l.seg(x - 0.75 * r, -1.0, x - 0.75 * r, r + 5.0, 0.3, 1.0);
                            l.seg(x + 0.75 * r, -1.0, x + 0.75 * r, r + 5.0, 0.3, 1.0);
                            if ha > 0.4 { l.lamp(x, 2.0 * r + 5.6, 0.25, 0.9, 0.6, 1.0); }
                        }
                    }
                    fl = g[3] >= 0.6;
                }
                5 => { process(&mut l, &rnd, hid, &g); fl = h[3] >= 0.7; }
                6 => { // steel stacks braced by a lattice tower
                    let n = if h[2] < 0.5 { 2.0 } else { 3.0 };
                    for k in (0..n as i32).map(|k| k as f64) {
                        let (ha, hk) = (rnd(hid * 4.0 + k, 27.0, 1405.0), rnd(hid * 4.0 + k, 28.0, 1405.0));
                        let (x, hs) = ((k - (n - 1.0) * 0.5) * 7.0, 35.0 + 35.0 * hk);
                        l.trap(x, 0.0, 1.0 + 0.8 * ha, 1.0 + 0.8 * ha, hs, 1.0);
                        if hs > 45.0 && hk > 0.6 { l.beacon(x, hs + 0.4); }
                        if k == 0.0 && ha < 0.4 { pl = Some([x, hs, (1.0 + 0.8 * ha) * 1.3, 8.0 + 10.0 * hk, 40.0, 0.3, 150.0, 1.15]); }
                    }
                    let lw = (n - 1.0) * 3.5 + 2.5;
                    l.pt(Lat, [0., 0., 0., 31.], [lw, lw, 5., 0.22], lw + 0.5, 31.3, false);
                }
                7 => { // boiler house with a chimney beside it and an inclined conveyor on three trestles; tall windows, a few lit
                    let (hw, hh, sd) = (18.0 + 12.0 * h[0], 38.0 + 24.0 * h[1], if g[0] < 0.5 { -1.0 } else { 1.0 });
                    l.trap(0.0, 0.0, hw, hw, hh, 1.0); l.trap(hw * (g[1] - 0.5) * 0.8, hh - 2.0, hw * 0.25, hw * 0.25, 6.0, 1.0);
                    if wd > 0.0 {
                        for iy in 0..((hh - 16.0) / 7.0).floor() as i64 {
                            for ix in 0..((2.0 * hw - 10.0) / 6.0).floor() as i64 {
                                if rnd(hid * 64.0 + ix as f64 + iy as f64 * 9.0, 8.0, 1407.0) < wd * 0.3 { l.pane(-hw + 8.0 + 6.0 * ix as f64, 13.5 + 7.0 * iy as f64, 2.2, 4.0); }
                            }
                        }
                    }
                    let (xc, hc, rb) = (sd * (hw + 9.0), (1.7 * hh + 30.0 * g[1]).min(150.0), 4.5 + 1.5 * g[2]);
                    l.chimney(xc, hc, rb, 0.6 * rb); l.beacon(xc, hc - 0.8);
                    if hc > 100.0 { l.beacon(xc + sd * mix(rb, 0.6 * rb, 0.5) * 0.8, hc * 0.5); }
                    let (ax, bx, by) = (-sd * (hw + 65.0), -sd * hw, 0.72 * hh);
                    l.seg(ax, 0.0, bx, by, 1.3, 1.0);
                    for k in 0..3 { let tt = (k as f64 + 0.5) / 3.0; l.seg(ax + tt * (bx - ax), -1.0, ax + tt * (bx - ax), tt * by, 0.45, 1.0); }
                    if h[2] < 0.7 { pl = Some([xc, hc, rb * 0.66, 25.0 + 30.0 * g[3], 70.0, 0.4, 230.0, 0.95]); }
                }
                8 => { // flare stack in a lattice derrick, a small flame at the tip leaning downwind, its smoke lit by the flame
                    let (hh, fh, wx, mut yb) = (60.0 + 40.0 * h[0], 6.0 + 6.0 * h[1], wind(uc), 11.0);
                    l.trap(0.0, 0.0, 0.7, 0.7, hh, 1.0); l.seg(-6.5, 0.0, -1.2, hh - 3.0, 0.32, 1.0); l.seg(6.5, 0.0, 1.2, hh - 3.0, 0.32, 1.0);
                    while yb <= hh - 3.0 { let xl = mix(6.5, 1.2, yb / (hh - 3.0)); l.seg(-xl, yb, xl, yb, 0.22, 1.0); yb += 11.0; }
                    l.lamp(0.0475 * fh * wx, hh + 0.5 + 0.5 * fh, 0.18 * fh, 1.05, 0.45, 0.45); l.lamp(0.0, hh + 1.5, 0.8, 1.4, 0.45, 1.0);
                    pl = Some([0.0, hh + fh, 1.6, 8.0 + 8.0 * g[2], 40.0, 0.3, -90.0, 0.95]);
                }
                _ => { // a battery of silos (steel with cone roofs, or concrete with a roof gallery), an elevator tower at one end
                    let (st, n) = (g[1] >= 0.5, 3.0 + (4.0 * h[2]).floor());
                    let (r, hh) = if st { (3.0 + 2.0 * h[0], 20.0 + 15.0 * h[1]) } else { (4.0 + 2.0 * h[0], 30.0 + 20.0 * h[1]) };
                    l.gable([-n * r, -1.0, (2.0 - n) * r, hh], (1.0 - n) * r, if st { hh + 0.58 * r } else { hh }, n);
                    if st { l.seg(0.25 - n * r, hh + 0.58 * r + 0.4, n * r - 0.25, hh + 0.58 * r + 0.4, 0.25, 1.0); } else { l.trap(0.0, 0.0, n * r - 2.0, n * r - 2.0, hh + 5.0, 1.0); }
                    let ex = (if g[0] < 0.5 { -1.0 } else { 1.0 }) * (n * r + 2.5);
                    l.trap(ex, 0.0, 2.5, 2.5, hh + 14.0, 1.0);
                    if g[2] < 0.7 { l.lamp(ex, hh + 14.6, 0.3, 1.0, 0.6, 1.0); }
                }
            }
            if fl { // a floodlight mast with two white heads
                let (xf, hf) = ((g[2] * 2.0 - 1.0) * 30.0, 24.0 + 8.0 * g[0]);
                l.seg(xf, -1.0, xf, hf, 0.35, 1.0); l.trap(xf, hf - 0.15, 1.3, 1.3, 0.9, 1.0); l.lamp(xf - 0.7, hf - 0.15, 0.45, 1.8, 1.45, 1.0);
                l.lamp(xf + 0.7, hf - 0.15, 0.45, 1.8, 1.45, 1.0);
            }
            // more parts than an instance holds (96): several instances at its distance, the bodies first, the lights last
            let all: Vec<Vec<Pe>> = if l.bp.len() + l.lp.len() <= 96 { vec![[l.bp, l.lp].concat()] } else { l.bp.chunks(96).chain(l.lp.chunks(96)).map(|q| q.to_vec()).collect() };
            let seed = (hid * 4.0 + fr) as u32;
            for a in all {
                let (hw, top) = (a.iter().map(|q| q.3).fold(f64::MIN, f64::max) + 2.0, a.iter().map(|q| q.4).fold(f64::MIN, f64::max) + 2.0);
                c.add(Instance { slot: 1, view: 1, uc, d, td, seed, glow: 2.0, hw, top, ..I0 });
                for q in a { c.part(q.0, q.1, q.2); }
            }
            if let Some([ex, ey, w0, r, lp, k, fd, sh]) = pl { // the lot's plume, just behind it, in a frame centred on its extent
                let wx = wind(uc);
                let (aw, sg, s1) = (wx.abs().max(0.07), if wx < 0.0 { -1.0 } else { 1.0 }, 600f64.min(fd.abs() * (400.0 * k).ln()));
                let (lo, hi) = ((-1.6 * w0).min((aw - 0.256) * s1 - 1.6 * w0), (aw + 0.256) * s1 + 1.6 * w0);
                let (xa, xb) = (ex + if sg > 0.0 { lo } else { -hi }, ex + if sg > 0.0 { hi } else { -lo });
                let (xm, dp) = (0.5 * (xa + xb), d + 0.5);
                c.add(Instance { slot: 1, view: 1, uc: fract(uc + xm * c.k / (TAU * dp)), d: dp, hw: 0.5 * (xb - xa) + 2.0, top: ey + r + 0.06 * s1 + 1.6 * (w0 + 0.16 * s1) + 2.0, seed, ..I0 });
                c.part_c(Plume, [ex - xm, ey, w0, r], [lp, k, fd, sg * aw], [sh, 0., 1.]);
            }
        }
    }
}

/// A process unit: distillation columns with platforms, an open steel structure on a 6 m grid with bracing and vessels in
/// some bays, many small lamps, a pipe rack on its bents.
fn process(l: &mut Lot, rnd: &dyn Fn(f64, f64, f64) -> f64, hid: f64, g: &[f64; 4]) {
    let sh = 20.0 * (g[0] - 0.5);
    for k in (0..3).map(|k| k as f64) {
        let (ha, hk) = (rnd(hid * 4.0 + k, 22.0, 1404.0), rnd(hid * 4.0 + k, 23.0, 1404.0));
        let (x, r, hc, mut lv) = ((k - 1.0) * 13.0 + (ha - 0.5) * 4.0 + sh, 1.4 + 1.6 * ha, 24.0 + 36.0 * hk * hk, 8.0);
        l.seg(x, -1.0, x, hc, r, 1.0); // a column with a round cap, platforms every 8 m, a lamp on some
        while lv < hc - 2.0 {
            l.seg(x - r - 1.05, lv, x + r + 1.05, lv, 0.25, 1.0);
            if rnd(hid * 16.0 + k * 4.0 + lv, 24.0, 1404.0) < 0.5 { l.lamp(x + if ha < 0.5 { -1.0 } else { 1.0 } * (r + 1.0), lv + 0.8, 0.22, 0.9, 0.6, 1.0); } lv += 8.0;
        }
        if hc > 45.0 { l.beacon(x, hc + r + 0.3); }
    }
    let (sx, sw, hs, mut y) = (-sh + (g[1] - 0.5) * 6.0, 9.0 + 5.0 * g[2], 20.0 + 12.0 * g[3], 6.0);
    let (xr, yt) = (sw + 0.4, hs + 0.3);
    while y <= yt + 0.2 { l.seg(sx - sw + 0.3, y, sx + sw - 0.3, y, 0.3, 1.0); y += 6.0; } // floor beams, columns, outer posts, bracing (y = x + 6k)
    for k in (-3..=3).map(|k| k as f64) { if 6.0 * k.abs() < xr && (6.0 * k.abs() - sw).abs() > 0.7 { l.seg(sx + 6.0 * k, -1.0, sx + 6.0 * k, hs, 0.3, 1.0); } }
    l.seg(sx - sw, -1.0, sx - sw, hs, 0.35, 1.0); l.seg(sx + sw, -1.0, sx + sw, hs, 0.35, 1.0);
    for k in (-12..=8).map(|k| k as f64 * 6.0) { let (xa, xb) = ((-xr).max(-k), xr.min(hs - k)); if xb - xa > 0.5 { l.seg(sx + xa, xa + k, sx + xb, xb + k, 0.22, 0.8); } }
    let (by1, bx0, bx1) = ((yt / 6.0).floor(), (-xr / 6.0).floor(), (xr / 6.0).floor());
    let eq = |bx: f64, by: f64| bx >= bx0 && bx <= bx1 && by >= 0.0 && by <= by1 && rnd(hid * 64.0 + bx + by * 8.0, 25.0, 1404.0) >= 0.62;
    let o = |c: bool| if c { 1.5 } else { 0.0 };
    for by in (0..=by1 as i64).map(|b| b as f64) { for bx in (bx0 as i64..=bx1 as i64).map(|b| b as f64).filter(|&bx| eq(bx, by)) { // vessels and equipment in some bays
        let (x0, x1) = ((6.0 * bx).max(-xr) - o(eq(bx - 1.0, by)), (6.0 * bx + 6.0).min(xr) + o(eq(bx + 1.0, by)));
        let (y0, y1) = (if by != 0.0 { 6.0 * by - o(eq(bx, by - 1.0)) } else { -1.0 }, (6.0 * by + 6.0).min(yt) + o(eq(bx, by + 1.0)));
        if x1 > x0 && y1 > y0 { l.trap(sx + 0.5 * (x0 + x1), y0, 0.5 * (x1 - x0), 0.5 * (x1 - x0), y1 - y0, 0.85); }
    } }
    let mut iy = 1.0;
    while iy * 6.0 - 0.6 < hs { // lamps on the structure's levels
        for ix in (-4..4).map(|ix| ix as f64) {
            let (lx, hl) = ((ix + 0.5) * 4.5, rnd(hid * 256.0 + ix + iy * 16.0, 26.0, 1404.0));
            if lx.abs() < sw && hl < 0.45 { l.lamp(sx + lx, iy * 6.0 - 0.6, 0.22, 0.8, if hl < 0.36 { 0.6 } else { 1.45 }, 1.0); }
        }
        iy += 1.0;
    }
    l.seg(-44.4, 11.0, 44.4, 11.0, 0.6, 1.0); l.seg(-44.55, 13.5, 44.55, 13.5, 0.45, 1.0); // a pipe rack on its bents
    for i in 0..7 { let x = -44.0 + i as f64 * 88.0 / 6.0; l.seg(x, -1.0, x, 13.5, 0.2, 1.0); }
}
