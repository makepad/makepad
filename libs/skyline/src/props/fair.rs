//! The fairground (the page's `pkFair`): everything on one plane 1.15-1.85 km out beside the town (on a pier at the sea
//! or at the town's edge): a Ferris wheel, a roller coaster, a carousel, a drop tower, booths with bulb strings, a pier on
//! piles with its lamps. The ground band first, then each ride with its own lights, then the ground band's lights; the
//! repeated members are one part inside a REP, the trestle is clipped under the track by a CLIP.
use super::kit::{clamp, Ctx, Pt::{self, *}, I0, Instance};
use crate::params::P;
use crate::rng::{fract, mix, smooth, TAU};

/// A part with its placement c (a plane part: (0, 0, 1); the wheel's turned plane: (0, 0, 1/ex)).
type Pa = (Pt, [f64; 4], [f64; 4], [f64; 3]);
const C0: [f64; 3] = [0.0, 0.0, 1.0];
const WARM: f64 = 0.82;
const COOL: f64 = 1.45;
fn bx(x: f64, y0: f64, w: f64, h: f64, w1: f64) -> Pa { (Trap, [x, y0, w, w1], [h, 0., 1., 0.], C0) }
fn sg(xa: f64, ya: f64, xb: f64, yb: f64, w: f64) -> Pa { (Seg, [xa, ya, xb, yb], [w, w, 1., 0.], C0) }
fn lig(x: f64, y: f64, rr: f64, amp: f64, kw: f64) -> Pa { (Light, [x, y, rr, 1.], [0., amp, 0., kw], C0) }
/// A repeat of `list` (b: cell count, kind, start angle), every part with placement cc.
fn rep(o: &mut Vec<Pa>, a: [f64; 4], b: [f64; 3], list: &[Pa], cc: [f64; 3]) {
    o.push((Rep, a, [b[0], b[1], b[2], list.len() as f64], cc)); o.extend(list.iter().map(|q| (q.0, q.1, q.2, cc)));
}
/// The grid points x0 + p i from a to b (b too for the last chunk).
fn grid(x0: f64, p: f64, a: f64, b: f64, last: bool) -> Vec<f64> {
    let (mut i, mut k) = (((a - x0) / p - 1e-9).ceil(), vec![]);
    while x0 + p * i < b - 1e-9 || (last && x0 + p * i <= b + 1e-9) { k.push(x0 + p * i); i += 1.0; }
    k
}
/// A ground-band chunk: its top and parts.
type Chunk = (f64, Vec<Pa>);
/// An instance at fair x (m, away from the town): half-width, top, parts.
struct Fi { x: f64, hw: f64, top: f64, p: Vec<Pa> }

pub fn build(c: &mut Ctx) {
    let (am, z) = (c.p[P::PrFair], c.z.fair);
    if am <= 0.0 || z[1] <= 0.0 { return; }
    let (r, pier, fs): (Vec<f64>, bool, f64) = ((0..16).map(|i| if i == 0 { 0.0 } else { c.rnd(i as f64, 1.0, 1900.0) }).collect(),
        c.z.fair_x[0] > 0.5, if c.z.fair_x[1] < 0.0 { -1.0 } else { 1.0 });
    // d m out; its rides and booths in size units, which subtend over dk = d / Size what metres do over d
    let d = 1150.0 + 700.0 * r[1];
    let dk = d / c.k;
    let (w, yg, dw, ton, kon) = (z[1] * TAU * dk, if pier { 6.5 } else { 0.0 }, // half-width, the pier deck or the ground
        30.0 + 25.0 * r[2], am >= 0.5 + 0.25 * r[4], am >= 0.2 + 0.25 * r[5]);
    // the layout along x: wheel, coaster and drop tower in a seed order, booths and a carousel in the gaps
    let rw = 0.5 * dw;
    let (ww, wt, yh) = (2.3 * rw + 8.0, if ton { 14.0 } else { 0.0 }, yg + rw + 3.5);
    let lc = if am >= 0.12 + 0.2 * r[3] { clamp(2.0 * w - 8.0 - ww - wt - 12.0, 0.0, 250.0) } else { 0.0 };
    let lc = if lc < 60.0 { 0.0 } else { lc };
    let wc = if lc != 0.0 { lc + 10.0 } else { 0.0 };
    let (x0, o) = (-w + 4.0 + (2.0 * w - 8.0 - ww - wt - wc).max(0.0) * r[6], (r[7] * 2.999).floor());
    let (xw, xc0, xt) = if o < 0.5 { (x0 + 0.5 * ww, x0 + ww, x0 + ww + wc + 0.5 * wt) } else if o < 1.5 { (x0 + wc + 0.5 * ww, x0, x0 + wc + ww + 0.5 * wt) }
        else { (x0 + wt + 0.5 * ww, x0 + wt + ww, x0 + 0.5 * wt) };
    let (xc0, s) = (xc0 + 5.0, |xc: f64| move |v: f64| fs * (v - xc)); // fair x -> part x in the instance centred at xc
    // the ground band (pier, booths) and its lights in x chunks of ~100 m: (top, parts) each
    let xe = if pier { -w - 0.0065 * TAU * dk } else { -w };
    let nf = 1f64.max(((w - xe) / 100.0 + 0.5).floor()) as usize;
    let (cf, mut body) = ((w - xe) / nf as f64, vec![]);
    let (chunk_of, xcj) = (|x: f64| clamp(((x - xe) / cf).floor(), 0.0, nf as f64 - 1.0) as usize, |j: usize| xe + (j as f64 + 0.5) * cf);
    let (mut gb, mut fl): (Vec<Chunk>, Vec<Chunk>) = (vec![(f64::MIN, vec![]); nf], vec![(f64::MIN, vec![]); nf]);
    for j in (0..nf).filter(|_| pier) { // the pier on piles: deck pieces overlapping 1 m, rail, piles every 8 m, lamps on posts every 25 m
        let (a, last, sj) = (xe + j as f64 * cf, j == nf - 1, s(xcj(j)));
        let b = a + cf;
        let row = |p: f64| { let k = grid(0.0, p, a, b, last); ([sj(k[0]), 0.0, fs * p, 0.0], [k.len() as f64, 0.0, 0.0]) };
        let (da, db) = (if j > 0 { a - 0.5 } else { a }, if last { b } else { b + 0.5 });
        let e = &mut gb[j];
        e.0 = e.0.max(yg + 4.6);
        e.1.extend([bx(sj(0.5 * (da + db)), yg - 1.3, 0.5 * (db - da), 1.3, 0.5 * (db - da)), (Arc, [sj(a).min(sj(b)), yg + 1.05, sj(a).max(sj(b)), yg + 1.05], [0., 0.06, 1., 0.], C0)]);
        if !grid(0.0, 8.0, a, b, last).is_empty() { let (ra, rb) = row(8.0); rep(&mut e.1, ra, rb, &[bx(0.0, 0.0, 0.35, yg - 1.3, 0.35)], C0); }
        if !grid(0.0, 25.0, a, b, last).is_empty() {
            let (ra, rb) = row(25.0);
            rep(&mut e.1, ra, rb, &[bx(0.0, yg, 0.08, 4.4, 0.08)], C0);
            fl[j].0 = fl[j].0.max(yg + 5.9); rep(&mut fl[j].1, ra, rb, &[lig(0.0, yg + 4.5, 0.22, 1.6, WARM)], C0);
        }
    }
    if ton { // the drop tower: mast, top cap, ride ring, base; bulbs up both edges, round the ring; a beacon
        let hdt = yg + 55.0 + 35.0 * r[15];
        let (yr, n) = (yg + 6.0 + (hdt - yg - 22.0) * r[9], ((hdt - 5.0 - yg) / 2.6).floor() - 1.0);
        let mut p = vec![bx(0.0, yg, 1.8, hdt - yg, 1.8), bx(0.0, hdt - 5.0, 3.2, 5.0, 3.2), bx(0.0, yr - 1.2, 4.5, 2.4, 4.5), bx(0.0, yg, 5.0, 4.0, 5.0)];
        if n > 0.0 { rep(&mut p, [0., yg + 5.2, 0., 2.6], [n, 0., 0.], &[lig(-1.95, 0.0, 0.1, 0.6, COOL), lig(1.95, 0.0, 0.1, 0.6, COOL)], C0); }
        p.extend([(LRow, [-3., yr - 1.25, 3., 0.1], [1.5, 0., 0.8, WARM], C0), (Beacon, [0., hdt + 0.8, 0., 0.], [0.; 4], C0)]);
        body.push(Fi { x: xt, hw: 5.5, top: hdt + 1.5, p });
    }
    if kon { // the carousel: drum, conical roof, finial; bulbs round the roof's rim and down the cone
        let (xk, rk) = (if r[8] < 0.5 { -w + 12.0 } else { w - 12.0 }, 7.0 + 3.0 * r[9]);
        let (sl, nb, n) = (5.0 / (rk + 1.0), ((rk + 0.7) / 2.0).floor(), ((rk + 0.8) / 1.2).floor());
        let mut p = vec![bx(0.0, yg, rk, 4.2, rk), bx(0.0, yg + 3.9, rk + 1.0, 5.3, 0.0),
            bx(0.0, yg + 8.0, 0.12, 2.6, 0.12), (LRow, [-1.2 * n, yg + 4.25, 1.2 * n, 0.1], [1.2, 0., 0.8, WARM], C0)];
        rep(&mut p, [0., yg + 8.95, -2., -2.0 * sl], [nb + 1.0, 0., 0.], &[lig(0.0, 0.0, 0.09, 0.5, WARM)], C0);
        if nb > 0.0 { rep(&mut p, [2., yg + 8.95 - 2.0 * sl, 2., -2.0 * sl], [nb, 0., 0.], &[lig(0.0, 0.0, 0.09, 0.5, WARM)], C0); }
        body.push(Fi { x: xk, hw: rk + 1.5, top: yg + 10.8, p });
    }
    { // the Ferris wheel: rims, spokes and hub in its plane turned from the view; gondolas hang upright; A-frame legs, brace, platform
        let (ex, ns, gs) = (mix(0.82, 1.0, r[10]), 16.0 + 4.0 * (r[11] * 2.999).floor(), 0.055 * dw + 0.8);
        let (ph, cw, fx, yb) = (if fs > 0.0 { r[12] * TAU } else { std::f64::consts::PI - r[12] * TAU }, [0.0, 0.0, 1.0 / ex], 0.62 * rw * ex, yg + 0.32 * (yh - yg));
        let al = std::f64::consts::PI / ns;
        let mut p = vec![(Ring, [0., yh, rw, 1.], [0.32 + 0.004 * dw, 0., 0., 0.], cw)];
        if dw >= 36.0 { p.push((Ring, [0., yh, 0.91 * rw, 1.], [0.22, 0., 0., 0.], cw)); }
        let (mut spoke, mut rk) = (vec![sg(0.06 * rw, 0.0, rw, 0.0, 0.16)], 1.6);
        while rk <= rw - 1.0 { if rk >= 0.12 * rw { spoke.push(lig(rk, 0.0, 0.12, 0.6, WARM)); } rk += 3.2; } // bulbs along the spokes
        rep(&mut p, [0., yh, 0., 1.], [ns, 1., ph], &spoke, cw);
        p.push((Ell, [0., yh, 0.065 * rw, 0.065 * rw], [0., 0., 1., 0.], cw));
        rep(&mut p, [0., yh - 0.7 * gs, rw, ex], [ns, 2., ph], &[bx(0.0, -0.5 * gs, 0.42 * gs, gs, 0.42 * gs), sg(0.0, 0.0, 0.0, 0.75 * gs, 0.08)], C0);
        p.extend([sg(0.0, yh, -fx, yg, 0.45), sg(0.0, yh, fx, yg, 0.45), sg(-0.68 * fx, yb, 0.68 * fx, yb, 0.3), bx(0.0, yg, 0.45 * rw, 3.4, 0.45 * rw)]);
        let (ca, sa) = (rw * (al / 4.0).cos(), rw * (al / 4.0).sin()); // 4 ns bulbs round the rim, a lit hub
        rep(&mut p, [0., yh, 0., 1.], [2.0 * ns, 1., ph + al / 4.0], &[lig(ca, -sa, 0.16, 1.05, WARM), lig(ca, sa, 0.16, 1.05, if r[13] < 0.5 { WARM } else { COOL })], cw);
        p.push(lig(0.0, yh, 0.45, 1.2, WARM));
        body.push(Fi { x: xw, hw: rw * ex + 0.5 * gs + 1.0, top: yh + rw + 1.2, p });
    }
    if lc != 0.0 { coaster(&mut body, &r, lc, xc0, yg, fs) }
    for cf2 in ((-w / 9.0).floor() as i64..=(w / 9.0).floor() as i64).map(|cc| cc as f64) { // booths along the front, bulb strings under the eaves
        let bxx = (cf2 + 0.5) * 9.0 + (c.rnd(cf2, 1.0, 1901.0) - 0.5) * 2.0;
        if c.rnd(cf2, 2.0, 1901.0) > 0.25 + 0.55 * am || bxx < -w + 3.0 || bxx > w - 3.0 { continue; }
        let (bw, bh, tent) = (2.0 + 1.5 * c.rnd(cf2, 3.0, 1901.0), 3.2 + 1.6 * c.rnd(cf2, 4.0, 1901.0), c.rnd(cf2, 5.0, 1901.0) >= 0.6);
        let (kw, n, j) = (if c.rnd(cf2, 6.0, 1901.0) < 0.5 { WARM } else { COOL }, ((bw - 0.2) / 0.9).floor(), chunk_of(bxx));
        let sx = s(xcj(j))(bxx);
        gb[j].0 = gb[j].0.max(yg + bh + if tent { 2.0 } else { 0.2 }); gb[j].1.push(bx(sx, yg, bw, bh, bw));
        if tent { gb[j].1.push(bx(sx, yg + bh, bw, 1.8, 0.0)); }
        fl[j].0 = fl[j].0.max(yg + bh + 0.2); fl[j].1.push((LRow, [sx - 0.9 * n, yg + bh - 0.35, sx + 0.9 * n, 0.08], [0.9, 0., 1.1, kw], C0));
    }
    let chunks = |v: Vec<Chunk>| v.into_iter().enumerate().filter(|q| !q.1 .1.is_empty()).map(|(j, (top, p))| Fi { x: xcj(j), hw: 0.5 * cf + 5.0, top, p }).collect::<Vec<_>>();
    for f in chunks(gb).into_iter().chain(body).chain(chunks(fl)) {
        c.add(Instance { uc: fract(z[0] + fs * f.x / (TAU * dk)), d, hw: f.hw, top: f.top, glow: 1.0, ..I0 });
        for q in f.p { c.part_c(q.0, q.1, q.2, q.3); }
    }
}

/// The roller coaster: a lift hill, a first drop and three humps; the double rails as parabola pieces fitted to the track;
/// steel columns every 9 m with braces in alternate bays, or a wooden trestle of 5 m lattice bays clipped under the track;
/// the station; bulbs every 4 m. In 18 m chunks.
fn coaster(body: &mut Vec<Fi>, r: &[f64], lc: f64, xc0: f64, yg: f64, fs: f64) {
    let (h, steel, bulbs) = (30.0 + 25.0 * r[14], r[15] < 0.5, r[13] >= 0.5);
    let prof = |x: f64| {
        let f = (x - xc0) / lc;
        let (g1, g2, g3) = ((f - 0.53) / 0.065, (f - 0.70) / 0.055, (f - 0.85) / 0.045);
        yg + 4.5 + (h - 4.5) * smooth(0.07, 0.30, f) * (1.0 - smooth(0.30, 0.41, f)) + h * (0.62 * (-g1 * g1).exp() + 0.40 * (-g2 * g2).exp() + 0.22 * (-g3 * g3).exp())
    };
    let low = |x: f64| { let s = 0.5 * (prof(x + 1.0) - prof(x - 1.0)); prof(x) - 1.35 * (1.0 + s * s).sqrt() }; // the lower rail 1.35 m below along the normal
    let fit = |f: &dyn Fn(f64) -> f64, xa: f64, xb: f64| {
        let (ya, yb) = (f(xa), f(xb));
        let sag = 0.5 * (ya + yb) - f(0.5 * (xa + xb));
        [ya, yb, sag, (1..8).map(|i| i as f64 / 8.0).fold(0.0, |e: f64, u| e.max((ya + (yb - ya) * u - 4.0 * sag * u * (1.0 - u) - f(xa + (xb - xa) * u)).abs()))]
    };
    fn split(fit: &dyn Fn(f64, f64) -> ([f64; 4], [f64; 4]), xa: f64, xb: f64, out: &mut Vec<(f64, f64, [f64; 4], [f64; 4])>) {
        let (a, b) = fit(xa, xb);
        if (a[3] > 0.1 || b[3] > 0.1) && xb - xa > 1.0 { split(fit, xa, 0.5 * (xa + xb), out); split(fit, 0.5 * (xa + xb), xb, out); } else { out.push((xa, xb, a, b)); }
    }
    let (both, nc) = (|xa: f64, xb: f64| (fit(&prof, xa, xb), fit(&low, xa, xb)), 1f64.max((lc / 18.0 + 0.5).floor()));
    let (cl, kmax) = (lc / nc, (lc / 5.0).ceil() - 1.0);
    for ci in 0..nc as i64 {
        let c0 = xc0 + ci as f64 * cl;
        let (c1, last, mut segs, mut p) = (c0 + cl, ci == nc as i64 - 1, vec![], vec![]);
        let xc = 0.5 * (c0 + c1);
        let sx = |v: f64| fs * (v - xc);
        split(&both, c0, c1, &mut segs);
        let (mut top, mut x) = (yg + 6.0, c0);
        while x <= c1 { top = top.max(prof(x)); x += 0.5; }
        for (xa, xb, a, b) in segs {
            let (sw, s_a, s_b) = (fs < 0.0, sx(xa).min(sx(xb)), sx(xa).max(sx(xb)));
            let arc = |e: &[f64; 4], dy: f64| [s_a, if sw { e[1] } else { e[0] } + dy, s_b, if sw { e[0] } else { e[1] } + dy];
            p.extend([(Arc, arc(&a, 0.0), [a[2], 0.28, 1., 0.], C0), (Arc, arc(&b, 0.0), [b[2], 0.22, 1., 0.], C0)]);
            if !steel { // lattice bays every 5 m, under the track less 1.4 m
                let (k0, k1) = (0f64.max(((xa - xc0) / 5.0).floor()), kmax.min(((xb - xc0 - 1e-6) / 5.0).floor()));
                p.push((Clip, arc(&a, -1.4), [a[2], 0., 0., 2.], C0));
                rep(&mut p, [sx(xc0 + 2.5 + 5.0 * k0), 0., fs * 5.0, 0.], [k1 - k0 + 1.0, 0., 0.],
                    &[(Lat, [0., yg, 0., a[0].max(a[1]).max(0.5 * (a[0] + a[1]) - a[2])], [2.5, 2.5, 5., 0.18], C0)], C0);
            }
        }
        if steel { // columns every 9 m; braces every 9 m up in alternate bays, 6 m under the track
            p.extend(grid(xc0, 9.0, c0, c1, last).into_iter().map(|x| bx(sx(x), yg, 0.32, prof(x) - 1.4 - yg, 0.32)));
            let mut m = ((c0 - xc0) / 18.0).floor();
            while xc0 + 18.0 * m < c1 {
                let (ba, bb) = ((xc0 + 18.0 * m).max(c0), (xc0 + 18.0 * m + 9.0).min(c1));
                m += 1.0;
                if bb <= ba { continue; }
                let n = ((bb - ba) / 0.25).ceil() as usize;
                let (xs, mut y): (Vec<f64>, f64) = ((0..=n).map(|i| (ba + i as f64 * 0.25).min(bb)).collect(), yg + 9.0);
                while y < yg + h {
                    let mut i0: Option<usize> = None;
                    for i in 0..=n + 1 {
                        let ok = i <= n && prof(xs[i]) - 6.0 > y;
                        if ok && i0.is_none() { i0 = Some(i); } else if !ok { if let Some(a) = i0.take() { if i - 1 > a { p.push(sg(sx(xs[a]), y, sx(xs[i - 1]), y, 0.2)); } } }
                    }
                    y += 9.0;
                }
            }
        }
        if ci == 0 { p.push(bx(sx(xc0 + 0.035 * lc), yg, 0.035 * lc, 6.0, 0.035 * lc)); } // the station
        if bulbs { p.extend(grid(xc0, 4.0, c0, c1, last).into_iter().map(|x| lig(sx(x), prof(x) + 0.6, 0.1, 0.7, COOL))); }
        body.push(Fi { x: xc, hw: 0.5 * cl + 1.0, top: top + 1.3, p });
    }
}
