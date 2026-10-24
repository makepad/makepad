//! The terrain as the scene reads it: per output column the five ridge elevations, each tree
//! layer's foot line, canopy top and density, the land envelope, the near ground rise, the
//! off-centre factor and the city glow's azimuth part (the page's `sgColumns`), and the single trees
//! (`sgTrees`): one indexed mesh per view, built here, drawn once per picture with max blending into
//! a coverage target with one channel per tree layer (ranges 2, 3, 4 and the front treeline) that
//! the range instances sample.
use crate::params::{P, Params};
use crate::render::{SceneCtx, SceneTarget};
use crate::rng::{fract, mix, smooth, Rng, TAU};
use crate::zones::Zones;
use makepad_widgets::makepad_draw::*;

/// Texels per column: rows 0-4 the ranges (ridge, tree foot, canopy top, density), row 5 the front
/// treeline (-, foot, canopy top, density), row 6 (land, ground rise, off-centre factor t, city glow).
pub const ROWS: usize = 7;
/// The column texture's widest row; more columns wrap into further bands of 7 rows.
pub const TEX_W: usize = 8192;

/// The parameters the columns depend on (with the seed and the view).
pub const COLUMN_KEYS: &[P] =
    &[P::MtH, P::MtW, P::MtC, P::MtRug, P::Land, P::LandC, P::TrD, P::TrH, P::TrCon, P::TrW, P::TrC, P::Treeline, P::CtW, P::CtC, P::CtR, P::Off, P::OffDir, P::Size];

/// Columns beyond the view on each side: the feet of trees reaching in (Size times as far at a larger Size).
pub fn margin(p: &Params, span: f64, w: usize) -> usize {
    (((0.045 * p[P::Size].max(1.0) / (span * TAU / w as f64 * (1.0 - p[P::Off]).max(0.1))).ceil() as usize) + 2).min(2048)
}

/// The off-centre camera (the page's `sgWarp`, bit-exact): the world azimuth the raw view azimuth `ur`
/// sees (the scenery is a ring of radius 1, the camera `off` from its centre toward `offDir`) and
/// the distance factor t that heights and pixel sizes scale by. For the trees and the props.
pub fn warp(p: &Params, ur: f64) -> (f64, f64) {
    let (cx, cy, dx, dy) = (p[P::Off] * (p[P::OffDir] * TAU).cos(), p[P::Off] * (p[P::OffDir] * TAU).sin(), (ur * TAU).cos(), (ur * TAU).sin());
    let cd = cx * dx + cy * dy; let t = -cd + (cd * cd - cx * cx - cy * cy + 1.0).max(1e-4).sqrt();
    (fract((cy + t * dy).atan2(cx + t * dx) / TAU), t)
}

/// The column data of one view: `n = w + 2 m` columns from `u0 - m` pixels, 7 texels each
/// (column i's row r at `i * ROWS + r`).
pub fn columns(z: &Zones, p: &Params, u0: f64, span: f64, w: usize, m: usize) -> Vec<[f32; 4]> {
    let (rng, nc, px0) = (&z.rng, w + 2 * m, span * TAU / w as f64);
    let (cx, cy, mut out) = (p[P::Off] * (p[P::OffDir] * TAU).cos(), p[P::Off] * (p[P::OffDir] * TAU).sin(), vec![[0f32; 4]; nc * ROWS]);
    let (cc, hk, c_d) = (cx * cx + cy * cy, std::array::from_fn::<f64, 5, _>(|k| z.ridge.hk(k)), 28.0 * p[P::CtR]);
    let cdx = (p[P::CtC] * TAU).cos() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).cos();
    let cdy = (p[P::CtC] * TAU).sin() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).sin(); let city_b = fract(cdy.atan2(cdx) / TAU);
    // the conifer share shapes the canopy's roughness (the page's `trC` here is `P.trCon`)
    let (tr_d, tr_c) = (p[P::TrD], p[P::TrCon]);
    // the column part of the forest: undergrowth, the canopy top and its pixel-scale roughness
    let canopy = |u: f64, px: f64, cells: f64, ly: f64, hmax: f64, base: f64, reg: f64| -> [f64; 3] {
        if reg < 0.05 { return [base, base, 0.0]; }
        let (rg, nf, mut b0, mut b1, mut b2) = (smooth(0.05, 0.3, reg), (TAU / (2.0 * px)).floor().max(1.0), 0.0, 0.0, 0.72);
        for (j, f) in [3.0, 11.0, 0.5, 2.0, 6.0, 19.0, 43.0].into_iter().enumerate() {
            let (per, x, oo) = (if j == 2 { (cells * 0.5).floor() } else { cells * f }, u * cells * f, ly + if j < 2 { 50.0 + j as f64 } else { 52.0 + (j - 2) as f64 });
            let (vn, vl) = (rng.pn(x, per, oo), if j >= 4 { rng.pl(x, per, oo) } else { 0.0 });
            match j { 0 | 1 => b0 += [0.07, 0.04][j] * vn, 2 => b1 = vn, 3 => b2 += 0.30 * vn,
                4 | 5 => b2 += [0.22, 0.12][j - 4] * (vn + (vl - vn) * tr_c - 0.5), _ => b2 += 0.16 * tr_c * (vl - 0.5) }
        }
        let tn = rng.pn(fract(u) * nf, nf, ly + 71.0);
        // a closed forest's canopy reaches most of the way up its trees
        let bush = hmax * (0.05 + b0) * (0.35 + 0.65 * tr_d) * rg; let body = hmax * (0.25 + 0.75 * tr_d * tr_d) * (0.55 + 0.45 * b1) * b2 * rg;
        [base, base + bush.max(body) + (0.6 * TAU / nf).min(hmax * 0.04) * (tn - 0.5) * rg, rg]
    };
    for i in 0..nc {
        let ur = fract(u0 + (i as f64 - m as f64 + 0.5) / w as f64 * span); let (dx, dy) = ((ur * TAU).cos(), (ur * TAU).sin());
        let cd = cx * dx + cy * dy; let t = -cd + (cd * cd - cc + 1.0).max(1e-4).sqrt();
        let (uw, px) = (fract((cy + t * dy).atan2(cx + t * dx) / TAU), px0 * t);
        let (env, lenv) = (z.ridge.envelopes(rng, uw), rng.region(uw, p[P::LandC], p[P::Land], 600.0));
        let freg = rng.region(uw, p[P::TrC], p[P::TrW], 450.0) * smooth(0.25, 0.6, lenv); let cenv = rng.region(ur, city_b, p[P::CtW], 270.0);
        let gh = (0.0035 + 0.0025 * rng.fbm(uw, 24.0, 500.0)) * lenv * p[P::Size]; let o = &mut out[i * ROWS..(i + 1) * ROWS];
        for k in 0..5 {
            let menv = z.ridge.menv(k, env); let (h, nz) = z.ridge.mtn_h(rng, uw, k, menv, px); let hr = hk[k] * menv * h;
            let mut row = [hr, 0.0, 0.0, 0.0];
            if k >= 2 {
                let kk = (k - 2) as f64; let t_reg = rng.region(uw, p[P::TrC], p[P::TrW], 400.0 + kk * 10.0);
                let base = hr.min(p[P::Treeline] * p[P::Size] * (0.55 + 0.9 * rng.pn(uw * 17.0, 17.0, 20.0 + k as f64 * 10.0 + 5.0)) * (0.8 + 0.4 * nz));
                let c = canopy(uw, px, tree_cells(p, k - 2), 410.0 + kk * 10.0, tree_hmax(p, k - 2), base, t_reg);
                row = [hr, c[0], c[1], c[2]];
            }
            o[k] = row.map(|v| v as f32);
        }
        let c = canopy(uw, px, tree_cells(p, 3), 400.0, tree_hmax(p, 3), gh, freg); o[5] = [0.0, c[0] as f32, c[1] as f32, c[2] as f32];
        o[6] = [lenv as f32, gh as f32, t as f32, ((0.55 + 0.45 * smooth(0.3, 0.7, rng.fbm(ur, 3.0, 50.0))) * cenv) as f32];
    }
    out
}

/// The tallest single tree of tree layer `l` (0-2 ranges 2-4, 3 the front treeline), rad above its foot: Size times the base's.
pub fn tree_hmax(p: &Params, l: usize) -> f64 { (if l < 3 { p[P::TrH] * (0.7 + 0.3 * l as f64) } else { p[P::TrH] }) * p[P::Size] }

/// Tree layer `l`'s cells round the turn (a single tree each, at most, a row): the base's over Size, whole, so its trees are
/// Size times as wide and as many over Size; the canopy's roughness follows them.
pub fn tree_cells(p: &Params, l: usize) -> f64 { ((if l < 3 { 260.0 + 130.0 * l as f64 } else { 520.0 }) / p[P::Size]).round().max(1.0) }

/// The parameters the trees depend on (with the seed and the pixel size).
const TREE_KEYS: &[P] = &[P::TrD, P::TrH, P::TrCon, P::TrW, P::TrC, P::Land, P::LandC, P::Off, P::OffDir, P::Size];

/// The single trees' mesh in the page's vertex layout ([`TreeVertex`], 9 floats), and how many trees.
#[derive(Default)] pub struct TreeMesh { pub v: Vec<f32>, pub i: Vec<u32>, pub trees: usize }

/// A tree before its mesh: anchor (warped u), height and pixel size there (rad), its cell centre, conifer, id.
#[derive(Clone, Copy)] struct Tree { ut: f64, ht: f64, px: f64, cxc: f64, con: bool, tid: f64 }

/// One tree's vertex writer (the page's `V_`): points in tree heights, the clip 0..1 over its cell window.
struct Tf<'a> { m: &'a mut TreeMesh, ut: f64, ht: f64, l: f64, lo: f64, cs: f64 }
impl Tf<'_> {
    /// Vertex (nx, ny) with the coverage terms q = [a, w, bx, by, capsule H]; returns its index.
    fn v(&mut self, nx: f64, ny: f64, q: [f64; 5]) -> u32 {
        self.m.v.extend([self.ut, nx * self.ht, ny * self.ht, q[4], q[0], q[1], q[2], q[3], (nx - self.lo) * self.cs + 4.0 * self.l].map(|x| x as f32));
        (self.m.v.len() / 9 - 1) as u32
    }
    /// The quad from vertex pair p, p + 1 to the next pair a, a + 1.
    fn ribbon(&mut self, p: u32, a: u32) { self.m.i.extend([p, p + 1, a, p + 1, a + 1, a]) }
}

/// The page's `sgTrees`, bit-exact: the trees at raw pixel size `px0` whose anchors lie in the warped arc (lo, hi; lo > hi
/// wraps; None: the whole turn), each layer in anchor order.
pub fn trees(rng: &Rng, p: &Params, px0: f64, arc: Option<(f64, f64)>) -> TreeMesh {
    let (cx, cy, mut mesh) = (p[P::Off] * (p[P::OffDir] * TAU).cos(), p[P::Off] * (p[P::OffDir] * TAU).sin(), TreeMesh::default());
    for l in 0..4 {
        let lf = l as f64;
        let (cells, ly, rly) = if l < 3 { (tree_cells(p, l), 410.0 + lf * 10.0, 400.0 + lf * 10.0) } else { (tree_cells(p, 3), 400.0, 450.0) };
        let (hmax, dprob, mut list) = (tree_hmax(p, l), mix(0.06, 1.0, p[P::TrD] * p[P::TrD]), Vec::new());
        for (row, ci) in (0..2).flat_map(|r| (0..cells as u32).map(move |c| (r as f64, c as f64))) {
            let sd = ly + row * 11.0;
            if rng.rnd(ci, 5.0, sd) >= dprob { continue; }
            let cxc = 0.5 + (rng.rnd(ci, 6.0, sd) - 0.5) * 0.5; let ut = fract((ci + cxc - 0.5 * row) / cells);
            if arc.is_some_and(|(lo, hi)| if lo <= hi { ut < lo || ut > hi } else { ut < lo && ut > hi }) { continue; }
            let mut reg = rng.region(ut, p[P::TrC], p[P::TrW], rly);
            if l == 3 { reg *= smooth(0.25, 0.6, rng.region(ut, p[P::LandC], p[P::Land], 600.0)); }
            if reg <= 0.05 { continue; }
            let ([rh, rs, rv], pn) = ([2.0, 3.0, 4.0].map(|st| rng.rnd(ci, st, sd)), |f: f64, o: f64| rng.pn(ut * f, f, ly + o));
            let env = 0.35 + 0.65 * (0.65 * pn(7.0, 90.0) + 0.35 * pn(14.0, 93.0));
            let clump = 0.3 + 0.7 * smooth(0.30, 0.65, 0.65 * pn(23.0, 95.0) + 0.35 * pn(46.0, 97.0));
            let (size, con) = (0.62 + 0.38 * rh.powf(0.7) * (0.55 + 0.45 * rv), p[P::TrCon] >= rs);
            let ht = hmax * env * mix(clump, 1.0, 0.5 * p[P::TrD]) * size * smooth(0.05, 0.6, reg) * (1.0 - 0.18 * row) * mix(0.8, 1.0, con as u8 as f64);
            let px = px0 * ((ut * TAU).cos() - cx).hypot((ut * TAU).sin() - cy);
            if ht >= px * 0.5 { list.push(Tree { ut, ht, px, cxc, con, tid: ci + cells * row + ly * 7.0 }); }
        }
        list.sort_by(|a, b| a.ut.total_cmp(&b.ut)); mesh.trees += list.len();
        for tr in &list { tree(rng, p, &mut mesh, tr, (lf, TAU / cells, ly)); }
    }
    mesh
}

/// One tree's triangles (the page's `emitTree`, the old `tree1` shapes): a spruce, or a broadleaf tree of tapered capsule
/// limbs and jagged leaf clumps. Coverage is a signed distance in pixels, d = a / w - |(bx, by)| (a capsule's clamped).
fn tree(rng: &Rng, p: &Params, mesh: &mut TreeMesh, tr: &Tree, (l, cw, ly): (f64, f64, f64)) {
    let Tree { ut, ht, cxc, con, tid, .. } = *tr; let (pxn, maxw) = (tr.px / ht, 1.05 * cw / ht);
    let (m, b, ip) = (0.79 * pxn, if con { 0.6 } else { maxw + 0.3 }, 1.0 / pxn);
    let (lo, hi) = (((-1.0 - cxc) * cw / ht).max(-b), ((2.0 - cxc) * cw / ht).min(b));
    let (mut t, r) = (Tf { m: mesh, ut, ht, l, lo, cs: 1.0 / (hi - lo) }, std::array::from_fn::<f64, 6, _>(|i| rng.rnd(tid, 21.0 + i as f64, ly)));
    let (dens, gt) = (mix(0.25, 1.0, r[2] * r[2]) * mix(0.7, 1.0, p[P::TrD]), |s: f64, n: f64| smooth(1.0, 2.5, s / n));
    if con {
        // a tapering cone, its flanks ragged rows of drooping branch tips; then the leader and the bare trunk
        let (lean, w, cb, nb) = ((r[0] - 0.5) * 0.008, mix(0.15, 0.24, r[3]).min(0.8 * maxw), mix(0.03, 0.14, r[4]), mix(22.0, 34.0, r[1]).floor());
        let (gn, g95, ex, dth) = (gt(ip, nb), gt(ip, 95.0), mix(0.55, 0.8, r[5]), mix(0.6, 1.0, dens));
        let tw = |t: f64, side: f64| {
            let (env, bi) = (w * (1.0 - t).max(0.0).powf(ex), (t * nb).floor());
            if gn <= 0.0 { return env; }
            let (saw, jag) = (rng.rnd(tid * 31.0 + bi, 40.0 + side, ly) * (1.0 - 0.6 * (t * nb - bi)), rng.pl(t * nb * 1.7 + side * 17.3, 1e4, tid + 11.0 + side));
            let mut br = 0.72 + 0.28 * (jag + (saw - jag) * 0.5);
            if g95 > 0.0 { br *= 1.0 + (0.75 + 0.5 * rng.pl(t * 95.0 + side * 5.1, 1e4, tid + 13.0 + side) - 1.0) * g95; }
            if rng.rnd(tid * 31.0 + bi, 44.0 + side, ly) >= dth { br *= 0.6; }
            env * (1.0 + (br - 1.0) * gn)
        };
        let n = if gn <= 0.0 { (ip * (1.0 - cb) / 1.5).ceil().max(3.0) } else { (ip * (1.0 - cb)).ceil().max(4.0) };
        let (mut ts, mut j, mut prev) = (Vec::new(), 1.0, None);
        for tk in (0..=n as usize).map(|k| k as f64 / n) { while gn > 0.0 && j < nb && j / nb <= tk { ts.extend([j / nb - 1e-6, j / nb + 1e-6]); j += 1.0; } ts.push(tk); }
        for tt in ts.into_iter().map(|t| t.clamp(0.0, 1.0)) {
            let (ny, wl, wr) = (cb + tt * (1.0 - cb), tw(tt, 0.0), tw(tt, 1.0)); let ax = lean * ny;
            let a = t.v(ax - wl - m, ny, [wl * ip, 1.0, -(wl + m) * ip, 0.0, 0.0]);
            t.v(ax, ny, [wl * ip, 1.0, 0.0, 0.0, 0.0]); t.v(ax, ny, [wr * ip, 1.0, 0.0, 0.0, 0.0]); t.v(ax + wr + m, ny, [wr * ip, 1.0, (wr + m) * ip, 0.0, 0.0]);
            if let Some(q) = prev { t.ribbon(q, a); t.ribbon(q + 2, a + 2); } prev = Some(a);
        }
        for (ys, hws) in [(&[0.0, 1.0, 1.012][..], &[0.010, 0.005, 0.005 - 0.012][..]), (&[0.0, cb + 0.02], &[0.020, 0.020])] {
            let mut prev = None;
            for (&ny, &hw) in ys.iter().zip(hws) {
                let (ax, e) = (lean * ny, hw.max(0.0) + m); let a = [-1.0, 1.0].map(|s| t.v(ax + s * e, ny, [hw * ip, 1.0, s * e * ip, 0.0, 0.0]))[0];
                if let Some(q) = prev { t.ribbon(q, a); } prev = Some(a);
            }
        }
        return;
    }
    // a broadleaf tree: a wandering trunk, two branches forking at each upper node and curving up, a leaf clump at each
    // branch segment's end
    let (reach, lg, spread, leafy) = ((0.8 * maxw / 0.45).min(1.0), mix(0.13, 0.17, r[3]), mix(0.80, 1.25, r[5]), mix(0.55, 1.0, dens));
    let fw = (6.0 * pxn).max(0.12); let on: [f64; 4] = std::array::from_fn(|o| smooth(1.2 * pxn, 2.5 * pxn, fw * 0.5f64.powi(o as i32)));
    // a tapered capsule as one quad: the shader clamps the along-axis offset (the old `tcap`)
    let caps = |t: &mut Tf, ax: f64, ay: f64, bx: f64, by: f64, ra: f64, rb: f64| {
        let l = Some((bx - ax).hypot(by - ay)).filter(|l| *l > 0.0).unwrap_or(1e-9);
        let (ex, ey, ma, mb, h) = ((bx - ax) / l, (by - ay) / l, ra + m, rb + m, 0.5 * l);
        let mut q = |s: f64, u: f64| t.v(ax + ex * s - ey * u, ay + ey * s + ex * u, [ra * ip, rb * ip, u * ip, (s - h) * ip, (h * ip).max(1e-4)]);
        let a0 = [(-ma, -1.0), (-ma, 1.0), (l + mb, -1.0), (l + mb, 1.0)].map(|(s, u)| q(s, u * ma.max(mb)))[0];
        t.m.i.extend([a0, a0 + 1, a0 + 3, a0, a0 + 3, a0 + 2]);
    };
    // a leaf clump (the old `crown1` + `roughEdge`): a radius per angle displaced by gradient-noise fbm, a fan; one quad
    // of its mean radius when that is under half a pixel
    let crown = |t: &mut Tf, ox: f64, oy: f64, rx: f64, ry: f64, cid: f64| {
        let mr = rx.min(ry); let (k, dl, g) = (mr / pxn, m / mr, [11.0, 23.0, 47.0, 97.0].map(|n| gt(TAU * mr / pxn, n)));
        let n = (TAU * rx.max(ry) / pxn * 1.3).ceil().max([8.0, 36.0, 72.0, 150.0, 300.0][g.iter().rposition(|&x| x > 0.0).map_or(0, |i| i + 1)]).min(400.0);
        let dsp = |x: f64, y: f64| {
            let (mut qx, mut qy, mut sc, mut s) = (x / fw, y / fw, fw, 0.0);
            for (o, &w) in on.iter().enumerate() {
                if w > 0.0 { s += 0.30 * sc * w * rng.tgn(qx, qy, (cid + 40.0 + o as f64 * 5.0) as i32); } (qx, qy, sc) = (1.6 * qx - 1.2 * qy, 1.2 * qx + 1.6 * qy, sc * 0.5);
            }
            s
        };
        let (lod, mut rs, mut ends) = (k < 0.5, 0.0, None); let c0 = if lod { 0 } else { t.v(ox, oy, [0.0; 5]) };
        for vi in 0..n as usize {
            let an = vi as f64 / n; let (ca, sa, pn) = ((an * TAU).cos(), (an * TAU).sin(), |f: f64, o: f64| rng.pn(an * f, f, cid + o));
            let mut r0 = 0.90 + 0.15 * pn(3.0, 4.0) + 0.10 * (pn(7.0, 7.0) - 0.5);
            if g[0] > 0.0 { r0 -= 0.18 * g[0] * (2.0 * pn(11.0, 10.0) - 1.0).abs(); }
            for (gi, f, o, a) in [(1, 23.0, 6.0, 0.22), (2, 47.0, 9.0, 0.18), (3, 97.0, 5.0, 0.12)] { if g[gi] > 0.0 { r0 += a * g[gi] * (rng.pl(an * f, f, cid + o) - 0.5); } }
            let sq = |rad: f64| r0 * (1.0 - 0.18 * smooth(0.0, -0.9, rad * sa)); let mut rad = sq(sq(r0));
            for _ in 0..2 {
                let (rr, rad0) = (sq(rad), rad); let mb = smooth(1.5 * fw, 0.0, ((rr - rad0) * mr).abs());
                rad = if mb > 0.0 { rr + dsp(ox + rx * rad0 * ca, oy + ry * rad0 * sa) * mb / mr } else { rr };
            }
            let (rad, rv) = (rad.max(0.0), (rad.max(0.0) + dl).min(1.5));
            if lod { rs += rad; continue; }
            let id = t.v(ox + rx * rv * ca, oy + ry * rv * sa, [rad * k, 1.0, rv * ca * k, rv * sa * k, 0.0]);
            if let Some((_, prev)) = ends { t.m.i.extend([c0, prev, id]); } ends = Some((ends.map_or(id, |e: (u32, u32)| e.0), id));
        }
        if let Some((first, prev)) = ends { t.m.i.extend([c0, prev, first]); }
        if lod {
            let p0 = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sy)| t.v(ox + rx * 1.5 * sx, oy + ry * 1.5 * sy, [rs / n * k, 1.0, 1.5 * sx * k, 1.5 * sy * k, 0.0]))[0];
            t.m.i.extend([p0, p0 + 1, p0 + 2, p0, p0 + 2, p0 + 3]);
        }
    };
    let (mut px, mut py, mut a, mut w) = (0.0, 0.0, (r[0] - 0.5) * 0.10, mix(0.018, 0.026, r[4]));
    for (i, fi) in [0.0, 1.0, 2.0, 3.0].into_iter().enumerate() {
        let (qx, qy) = (px + a.sin() * lg, py + a.cos() * lg); caps(&mut t, px, py, qx, qy, w, w * 0.8);
        for sdd in 0..[0, 2, 2, 3][i] {
            if sdd == 2 { crown(&mut t, qx, qy + 0.05, 0.15 * leafy * reach + 0.03, 0.12 * leafy + 0.02, tid + 99.0); continue; }
            let (sg, kb, sf) = (if sdd == 0 { -1.0 } else { 1.0 }, tid * 7.0 + (fi * 2.0 + sdd as f64), sdd as f64);
            let (mut ba, mut bx, mut by, mut bw) = (a + sg * mix(0.6, 1.0, rng.rnd(kb, 54.0, ly)) * spread, qx, qy, w * 0.55);
            let mut bl = lg * mix(0.65, 0.95, rng.rnd(kb, 56.0, ly)) * (1.15 - 0.12 * fi) * reach;
            for fj in [0.0, 1.0, 2.0] {
                let (ex, ey) = (bx + ba.sin() * bl, by + ba.cos() * bl); caps(&mut t, bx, by, ex, ey, bw, bw * 0.7);
                let cr = mix(0.075, 0.12, rng.rnd(kb * 3.0 + fj, 57.0, ly)) * leafy * (1.0 - 0.12 * fj);
                crown(&mut t, ex, ey, cr * 1.25, cr, tid + 60.0 + (fi * 6.0 + sf * 3.0 + fj));
                (bx, by, bw, bl, ba) = (ex, ey, bw * 0.7, bl * 0.82, ba + ((rng.rnd(kb * 3.0 + fj, 58.0, ly) - 0.5) * 0.6 - sg * 0.14));
            }
        }
        (px, py, w, a) = (qx, qy, w * 0.78, a + (rng.rnd(tid * 5.0 + fi, 55.0, ly) - 0.5) * 0.30);
    }
}

/// The trees' vertex (the page's `aP`, `aV`, `aC`): anchor (warped u), offset x, y (rad, warped; y above the layer's foot
/// line), capsule half length h (px); coverage terms a, w, bx, by (px); clip 0..1 over the tree's cell window + 4 x layer.
#[derive(Clone, Script, ScriptHook)] #[repr(C)]
pub struct TreeVertex { #[live] an: f32, #[live] x: f32, #[live] y: f32, #[live] h: f32, #[live] a: f32, #[live] w: f32, #[live] bx: f32, #[live] by: f32, #[live] c: f32 }

fn tree_vertex(vm: &mut ScriptVm) -> ScriptValue { let v = TreeVertex::script_pod(vm).expect("TreeVertex pod"); vm.heap_mut().pod_type_name_set(v, id_lut!(TreeVertex)); v.into() }
/// A placeholder that fixes the vertex format; each view binds its own mesh.
fn tree_geom(vm: &mut ScriptVm) -> ScriptValue {
    Geometry::new_borrowed(vm.cx_mut().shared_geometry(id!(SkyTreeGeom), |cx| { let g = Geometry::new(cx); g.update(cx, vec![0, 1, 2], vec![0.0; 27]); g })).into_script_handle(vm)
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.sky_geom = { TreeVertex: #(tree_vertex(vm)) TreeGeom: #(tree_geom(vm)) }

    // The single trees (the page's TVS / TFS): each vertex placed through the off-centre warp on its layer's foot line at
    // its own column, so every tree follows the ridge; its coverage through the old 1.5 px ramp into its layer's channel,
    // max-blended (the union of the old per-pixel max). `copy` 1 is the full strip's second copy across the seam.
    mod.draw.DrawSkyTrees = mod.std.set_type_default() do #(DrawSkyTrees::script_shader(vm)){
        ..mod.draw.DrawQuad,
        blend_op: @Max
        geom: vertex_buffer(mod.sky_geom.TreeVertex, mod.sky_geom.TreeGeom)
        view_size: uniform(vec2(1.0, 1.0)) view_span: uniform(vec2(0.0, 1.0)) view_horizon: uniform(0.8) view_pad: uniform(0.0)
        col_geom: uniform(vec4(1.0, 7.0, 0.0, 1.0)) off: uniform(0.0) off_dir: uniform(0.55) cols: texture_2d(float)
        v_v: varying(vec4f) v_c: varying(vec3f)
        // the camera's bearing (u) and distance factor of world azimuth uw
        raw_of: fn(uw: float, c: vec2) -> vec2 {
            let q = vec2(cos(uw * 6.2831853), sin(uw * 6.2831853)) - c; return vec2(atan2(q.y, q.x) / 6.2831853, length(q))
        }
        // the tree foot line in column row r under pass column x
        foot: fn(x: float, r: float) -> float {
            let c = clamp(x + self.col_geom.z, 0.0, self.col_geom.w - 1.0); let q = floor(c / self.col_geom.x)
            return self.cols.sample_nearest(vec2((c - q * self.col_geom.x + 0.5) / self.col_geom.x, (r + 7.0 * q + 0.5) / self.col_geom.y)).y
        }
        vertex: fn(){
            let c = self.off * vec2(cos(self.off_dir * 6.2831853), sin(self.off_dir * 6.2831853)); let sp = self.view_span; let sz = self.view_size - 2.0 * self.view_pad
            let ra = self.raw_of(self.geom.an, c); let rv = self.raw_of(self.geom.an + self.geom.x / 6.2831853, c)
            var da = fract(ra.x - sp.x - 0.5 * sp.y + 0.5) - 0.5
            if self.copy > 0.5 { da = da - sign(da) }
            let xs = ((da + fract(rv.x - ra.x + 0.5) - 0.5) / sp.y + 0.5) * sz.x + self.view_pad; let x0 = floor(xs - 0.5); let l = floor(self.geom.c * 0.25 + 0.25)
            let b = mix(self.foot(x0, l + 2.0), self.foot(x0 + 1.0, l + 2.0), xs - 0.5 - x0)
            let ys = self.view_horizon * sz.y - (b + self.geom.y) / (rv.y * sp.y * 6.2831853 / sz.x) + self.view_pad
            self.v_v = vec4(self.geom.a, self.geom.w, self.geom.bx, self.geom.by); self.v_c = vec3(self.geom.c - 4.0 * l, l, self.geom.h)
            self.world = self.draw_list.view_transform * vec4(xs, ys, self.draw_depth + self.draw_call.zbias, 1.0)
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        fragment: fn(){
            let v = self.v_v; let h = self.v_c.z; let s = clamp(v.w, 0.0 - h, h); var d = 1000.0
            if h > 0.0 { d = mix(v.x, v.y, 0.5 + 0.5 * s / h) - length(vec2(v.z, v.w - s)) }
            if h <= 0.0 && v.y >= 0.001 { d = v.x / v.y - length(v.zw) }
            let cv = clamp(d / 1.5 + 0.5, 0.0, 1.0) * step(0.0, self.v_c.x) * (1.0 - step(1.0, self.v_c.x))
            self.fb0 = cv * max(1.0 - abs(vec4(0.0, 1.0, 2.0, 3.0) - floor(self.v_c.y + 0.5)), vec4(0.0))
        }
    }

    mod.draw.SkyTreesBase = #(Trees::script_component(vm))
    mod.draw.SkyTrees = mod.std.set_type_default() do mod.draw.SkyTreesBase{ draw_trees: mod.draw.DrawSkyTrees{} }
}

#[derive(Script, ScriptHook)] #[repr(C)]
pub struct DrawSkyTrees { #[deref] pub draw_super: DrawQuad, #[live] pub copy: f32 }

/// The last meshes `trees` built (by their key, with the pixel size, and the arc they hold), so the Skylines of one
/// process that draw the same trees build them once: a host's strip in two parts (`SkyLight::plan`) re-renders both at
/// each step of a held Size drag, where the trees are most of a step's CPU (plan 6's final review, M7). The two newest.
static SHARED_TREES: std::sync::Mutex<Vec<(u64, [u64; 2], std::sync::Arc<TreeMesh>)>> = std::sync::Mutex::new(Vec::new());

/// The mesh `build` makes for trees `key` over arc `arc`, from SHARED_TREES when it holds it.
fn shared_trees(key: u64, arc: [f64; 2], build: impl FnOnce() -> TreeMesh) -> std::sync::Arc<TreeMesh> {
    let arc = arc.map(f64::to_bits);
    let hit = SHARED_TREES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, a, _)| *k == key && *a == arc).map(|(.., m)| m.clone());
    if let Some(m) = hit { return m; }
    let m = std::sync::Arc::new(build());
    let mut last = SHARED_TREES.lock().unwrap_or_else(|e| e.into_inner());
    last.retain(|(k, a, _)| !(*k == key && *a == arc));
    last.push((key, arc, m.clone()));
    if last.len() > 2 { last.remove(0); }
    m
}

/// Forgets the shared meshes (`SHARED_TREES`), so the next view builds its trees: a cold measure's.
pub fn forget_shared_trees() { SHARED_TREES.lock().unwrap_or_else(|e| e.into_inner()).clear(); }

/// The single trees of a view: the coverage target; the mesh (rebuilt when the trees' parameters or the pixel size change,
/// or the view leaves the arc it holds) with its key, arc (start, length) and index count; the last build's trees,
/// vertices, triangles and CPU ms.
#[derive(Script, ScriptHook)]
pub struct Trees { #[live] pub draw_trees: DrawSkyTrees, #[rust] target: Option<SceneTarget>,
    #[rust] pub mesh: Option<(u64, [f64; 2], Geometry, usize)>, #[rust] pub stats: (usize, usize, usize, f64) }

impl Trees {
    /// Draws the trees into their coverage target, a pass the scene pass `consumer` samples. Called only when the
    /// view's picture changes. A narrow view builds the trees of twice its arc (room to pan, the page's `sgGeom`).
    pub fn draw_pass(&mut self, cx: &mut Cx2d, sc: &SceneCtx, consumer: DrawPassId) {
        let (v, p, px0) = (sc.view, sc.p, sc.view.px0());
        let arc = |ua: f64, ub: f64| { let a = warp(p, fract(ua)).0; let w = fract(warp(p, fract(ub)).0 - a) + 0.024; if w >= 1.0 { [0.0, 1.0] } else { [a - 0.012, w] } };
        let (part, key) = (v.span < 0.5, p.hash(TREE_KEYS, &[px0])); let want = if part { arc(v.u0, v.u0 + v.span) } else { [0.0, 1.0] };
        if !self.mesh.as_ref().is_some_and(|(k, a, ..)| *k == key && (a[1] >= 1.0 || fract(want[0] - a[0]) + want[1] <= a[1])) {
            let (t0, ga) = (Cx::monotonic_now(), if part { arc(v.u0 - 0.5 * v.span, v.u0 + 1.5 * v.span) } else { [0.0, 1.0] });
            let m = shared_trees(key, ga, || trees(&sc.z.rng, p, px0, (ga[1] < 1.0).then(|| (fract(ga[0]), fract(ga[0] + ga[1])))));
            let (geom, n) = (self.mesh.take().map_or_else(|| Geometry::new(cx), |m| m.2), m.i.len());
            self.stats = (m.trees, m.v.len() / 9, n / 3, 0.0); geom.update(cx, m.i.clone(), m.v.clone());
            (self.stats.3, self.mesh) = ((Cx::monotonic_now() - t0) * 1000.0, Some((key, ga, geom, n)));
        }
        let Self { draw_trees: dt, target, mesh, .. } = self;
        let t = target.get_or_insert_with(|| SceneTarget::new(cx, "skyline_trees", false));
        cx.attach_child_pass(t.pass.draw_pass_id(), consumer, None);
        t.record(cx, v.w, v.h, |cx| if let Some((_, _, geom, _)) = mesh.as_ref().filter(|m| m.3 > 0) {
            sc.set_uniforms(cx, &mut dt.draw_vars); dt.draw_vars.geometry_id = Some(geom.geometry_id());
            for copy in 0..if part { 1 } else { 2 } { dt.copy = copy as f32; cx.add_instance(&dt.draw_vars); }
        });
    }

    /// The coverage target (one channel per tree layer), once there is one; its pass.
    pub fn coverage(&self) -> Option<&Texture> { self.target.as_ref().map(|t| &t.color) }
    pub fn pass_id(&self) -> Option<DrawPassId> { self.target.as_ref().map(|t| t.pass.draw_pass_id()) }
    /// Lets go of the target (an export core between jobs; the mesh stays: it is small and slow to build).
    pub fn release(&mut self) { self.target = None; }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree layer's cells and tallest tree follow Size (half the cells and twice the height at 2), the base's at Size 1;
    /// the columns' canopy of the front treeline stands twice as high at Size 2 (its mean over the forest's columns, +-10 %).
    #[test]
    fn the_trees_follow_size() {
        let mut p = Params::default();
        assert_eq!([0, 1, 2, 3].map(|l| tree_cells(&p, l)), [260.0, 390.0, 520.0, 520.0]);
        assert_eq!([0, 3].map(|l| tree_hmax(&p, l)), [p[P::TrH] * 0.7, p[P::TrH]]);
        let canopy = |p: &Params| {
            let z = Zones::new(p);
            let c = columns(&z, p, 0.0, 1.0, 1024, margin(p, 1.0, 1024));
            let rows: Vec<[f32; 4]> = c.chunks(ROWS).map(|r| r[5]).filter(|r| r[3] > 0.5).collect();
            rows.iter().map(|r| (r[2] - r[1]) as f64).sum::<f64>() / rows.len() as f64
        };
        let one = canopy(&p);
        p.set(P::Size as usize, 2.0);
        assert_eq!([0, 1, 2, 3].map(|l| tree_cells(&p, l)), [130.0, 195.0, 260.0, 260.0]);
        assert_eq!(tree_hmax(&p, 3), 2.0 * p[P::TrH]);
        let two = canopy(&p);
        assert!((two / one / 2.0 - 1.0).abs() < 0.1, "the front canopy x{}", two / one);
    }
}
