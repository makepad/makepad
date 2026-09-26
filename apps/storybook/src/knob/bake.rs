//! The bench's JavaScript bakes, in Rust.
//!
//! Everything the knob shader needs that depends only on a style's curves,
//! the material's profile smoothing and depth, and the light: the curves
//! resampled to 256 taps (value and slope at 16 bits, as the bench stores
//! them), the wing's derived constants, the revolve's outline by height for
//! the analytic cast-shadow sweep, the wing as a few knots with its taper,
//! and the revolve's self-shadow over its disc as a 64 x 64 table.
//!
//! None of it depends on a knob's size or value, so every knob of one style
//! under one light shares one bake. The ports follow the bench's functions
//! step for step (`fixTangents`, `polyline`, `resample`, `bakeProfile`,
//! `bakeCurve`, `bakeWing`, `dpIdx`, `bakeShade`), in the same order of
//! operations, so the same inputs give the same taps.
use super::presets::{Anchor, KnobMaterial, KnobStyle};
use std::collections::BTreeSet;

/// Taps per curve.
pub const TAPS: usize = 256;
/// Rows of the curve texture: profile, flute, wing width, wing height, wing
/// section.
pub const DATA_ROWS: usize = 5;
/// Floats in the knot texture: eight outline knots and four wing knots.
pub const KNOT_FLOATS: usize = 48;
/// The self-shadow table's side.
pub const SHADE_N: usize = 64;

#[derive(Clone, Copy, Default)]
struct Pt {
    x: f64,
    y: f64,
    ty: u8,
    ix: f64,
    iy: f64,
    ox: f64,
    oy: f64,
}

fn smooth_type(p: &Pt) -> bool {
    p.ty == 1 || p.ty == 3
}

/// The bench's `fixTangents` for anchors that carry no tangents: automatic
/// tangents, flat at a local extremum (Fritsch-Carlson), a third of the way
/// to each neighbour at most, and the pair rule so a segment's handles never
/// cross in x.
fn fix_tangents(p: &mut [Pt]) {
    let n = p.len();
    for i in 0..n {
        if p[i].ty == 0 {
            p[i].ix = 0.0;
            p[i].iy = 0.0;
            p[i].ox = 0.0;
            p[i].oy = 0.0;
            continue;
        }
        let a2 = p[i.saturating_sub(1)];
        let b2 = p[(i + 1).min(n - 1)];
        let mut ty = (b2.y - a2.y) / 6.0;
        if i > 0 && i < n - 1 && (p[i].y - a2.y) * (b2.y - p[i].y) <= 0.0 {
            ty = 0.0;
        }
        if p[i].ty == 3 {
            ty = 0.0;
        }
        p[i].ox = (b2.x - a2.x) / 6.0;
        p[i].oy = ty;
        p[i].ix = -p[i].ox;
        p[i].iy = -ty;
        let span = 1.0 / 3.0;
        let back = if i > 0 { (p[i].x - p[i - 1].x) * span } else { 1.0 };
        let fwd = if i < n - 1 { (p[i + 1].x - p[i].x) * span } else { 1.0 };
        if smooth_type(&p[i]) {
            if p[i].ty == 3 {
                p[i].oy = 0.0;
            }
            if p[i].ox < 0.0 {
                p[i].ox = 0.0;
                p[i].oy = 0.0;
            }
            let lim = back.min(fwd);
            if p[i].ox > lim {
                let s2 = lim / p[i].ox;
                p[i].ox *= s2;
                p[i].oy *= s2;
            }
            p[i].ix = -p[i].ox;
            p[i].iy = -p[i].oy;
        } else {
            p[i].ox = p[i].ox.min(fwd).max(0.0);
            p[i].ix = p[i].ix.max(-back).min(0.0);
        }
    }
    for i in 0..n.saturating_sub(1) {
        let l = p[i + 1].x - p[i].x;
        let c1 = p[i].ox;
        let c2 = -p[i + 1].ix;
        if c1 + c2 > l && c1 + c2 > 0.0 {
            let f = l / (c1 + c2);
            p[i].ox *= f;
            p[i].oy *= f;
            if smooth_type(&p[i]) {
                p[i].ix = -p[i].ox;
                p[i].iy = -p[i].oy;
            }
            p[i + 1].ix *= f;
            p[i + 1].iy *= f;
            if smooth_type(&p[i + 1]) {
                p[i + 1].ox = -p[i + 1].ix;
                p[i + 1].oy = -p[i + 1].iy;
            }
        }
    }
}

/// The curve as a polyline, 96 samples a cubic segment.
fn polyline(anchors: &[Anchor]) -> Vec<[f64; 2]> {
    let mut p: Vec<Pt> = anchors.iter().map(|a| Pt { x: a[0], y: a[1], ty: a[2] as u8, ..Pt::default() }).collect();
    fix_tangents(&mut p);
    let mut out = Vec::with_capacity(p.len() * 97);
    for i in 0..p.len().saturating_sub(1) {
        let (p0, p1) = (p[i], p[i + 1]);
        let (c1x, c1y) = (p0.x + p0.ox, p0.y + p0.oy);
        let (c2x, c2y) = (p1.x + p1.ix, p1.y + p1.iy);
        for k in 0..=96 {
            let t = k as f64 / 96.0;
            let u = 1.0 - t;
            out.push([
                u * u * u * p0.x + 3.0 * u * u * t * c1x + 3.0 * u * t * t * c2x + t * t * t * p1.x,
                u * u * u * p0.y + 3.0 * u * u * t * c1y + 3.0 * u * t * t * c2y + t * t * t * p1.y,
            ]);
        }
    }
    out
}

/// The curve resampled to `n` evenly spaced taps over x in 0..1. The
/// polyline's x never runs backwards (the pair rule sees to that), so each
/// tap's search resumes at the segment the last one found, which is the
/// segment the bench's search from the start finds.
pub fn resample(anchors: &[Anchor], n: usize) -> Vec<f64> {
    let pts = polyline(anchors);
    let mut k0 = 0;
    (0..n)
        .map(|i| {
            let r = i as f64 / (n - 1) as f64;
            let mut y = pts[pts.len() - 1][1];
            if r <= pts[0][0] {
                y = pts[0][1];
            } else {
                for k in k0..pts.len() - 1 {
                    if r >= pts[k][0] && r <= pts[k + 1][0] {
                        let t = (r - pts[k][0]) / (pts[k + 1][0] - pts[k][0]).max(1e-6);
                        y = pts[k][1] + t * (pts[k + 1][1] - pts[k][1]);
                        k0 = k;
                        break;
                    }
                }
            }
            y
        })
        .collect()
}

/// A value in 0..1 at the bench's 16 bits.
fn q16(v: f64) -> f64 {
    (v.clamp(0.0, 1.0) * 65535.0).round() / 65535.0
}

/// The slope as the bench encodes it: -dh/dx over +-8, mapped to 0..1.
fn slope_code(sl: f64) -> f64 {
    q16(sl / 16.0 + 0.5)
}

/// Central differences, both ends clamped (the profile's rule).
fn central_slopes(h: &[f64]) -> Vec<f64> {
    let n = h.len();
    (0..n).map(|i| -(h[(i + 1).min(n - 1)] - h[i.saturating_sub(1)]) * (n - 1) as f64 / 2.0).collect()
}

/// Central differences with one-sided ends (`bakeCurve`'s rule).
fn curve_slopes(h: &[f64]) -> Vec<f64> {
    let n = h.len();
    let mut sl = central_slopes(h);
    sl[0] = -(h[1] - h[0]) * (n - 1) as f64;
    sl[n - 1] = -(h[n - 1] - h[n - 2]) * (n - 1) as f64;
    sl
}

/// One row of texels: (slope code, height) at 16 bits each.
fn row_of(sl: &[f64], h: &[f64]) -> Vec<[f64; 2]> {
    sl.iter().zip(h).map(|(s, v)| [slope_code(*s), q16(*v)]).collect()
}

/// `bakeProfile`: the revolve, its slope box-smoothed over `psmooth` taps.
fn bake_profile(prof: &[Anchor], psmooth: f64) -> Vec<[f64; 2]> {
    let h = resample(prof, TAPS);
    let mut sl = central_slopes(&h);
    let w = psmooth.round() as isize;
    if w > 0 {
        let n = TAPS as isize;
        sl = (0..n)
            .map(|i| {
                let mut acc = 0.0;
                let mut cnt = 0.0;
                for k in -w..=w {
                    acc += sl[(i + k).clamp(0, n - 1) as usize];
                    cnt += 1.0;
                }
                acc / cnt
            })
            .collect();
    }
    row_of(&sl, &h)
}

/// `bakeCurve`: any drawn curve, value and slope.
fn bake_curve(pts: &[Anchor]) -> Vec<[f64; 2]> {
    let h = resample(pts, TAPS);
    row_of(&curve_slopes(&h), &h)
}

/// `flatCurve`: a curve whose every texel is the same, decoded, or -1.
fn flat_curve(pts: &[Anchor]) -> [f64; 2] {
    let row = bake_curve(pts);
    if row.iter().all(|t| t == &row[0]) {
        row[0]
    } else {
        [-1.0, -1.0]
    }
}

/// The two-tap lookup the shader does, on a baked row.
fn row_at(row: &[[f64; 2]], x: f64) -> [f64; 2] {
    let xx = x.clamp(0.0, 1.0) * 255.0;
    let i0 = xx.floor();
    let fr = xx - i0;
    let a = row[i0 as usize];
    let b = row[(i0 + 1.0).min(255.0) as usize];
    [a[0] + (b[0] - a[0]) * fr, a[1] + (b[1] - a[1]) * fr]
}

/// Linear lookup in a sampled curve (`at` in `bakeShade`).
fn at(a: &[f64], x: f64) -> f64 {
    let t = x.clamp(0.0, 1.0) * (a.len() - 1) as f64;
    let k = (t.floor() as usize).min(a.len() - 2);
    a[k] + (a[k + 1] - a[k]) * (t - k as f64)
}

/// Douglas-Peucker over several curves sampled at the same points: the
/// indices any of them needs, the tolerance loosened until at most `max_n`
/// remain.
fn dp_idx(curves: &[Vec<[f64; 2]>], tol0: f64, max_n: usize) -> Vec<usize> {
    fn rec(pts: &[[f64; 2]], a: usize, b: usize, tol: f64, keep: &mut BTreeSet<usize>) {
        let (ax, ay, bx, by) = (pts[a][0], pts[a][1], pts[b][0], pts[b][1]);
        let mut l = (bx - ax).hypot(by - ay);
        if l == 0.0 || l.is_nan() {
            l = 1e-9;
        }
        let mut best = None;
        let mut bd = tol;
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let dd = ((bx - ax) * (ay - p[1]) - (ax - p[0]) * (by - ay)).abs() / l;
            if dd > bd {
                bd = dd;
                best = Some(i);
            }
        }
        if let Some(bi) = best {
            keep.insert(bi);
            rec(pts, a, bi, tol, keep);
            rec(pts, bi, b, tol, keep);
        }
    }
    let n = curves[0].len();
    let mut tol = tol0;
    for _ in 0..16 {
        let mut keep = BTreeSet::new();
        keep.insert(0);
        keep.insert(n - 1);
        for pts in curves {
            rec(pts, 0, n - 1, tol, &mut keep);
        }
        if keep.len() <= max_n {
            return keep.into_iter().collect();
        }
        tol *= 1.4;
    }
    vec![0, n - 1]
}

/// What the geometry bake hands the shader beside its curve texture.
#[derive(Clone, Debug, Default)]
pub struct GeomConsts {
    /// The grip's band, in radii: where the profile anchors `ffrom` and
    /// `fto` sit.
    pub flute_r: [f64; 2],
    /// How tall the wing's crest gets, in cap heights.
    pub wing_top: f64,
    /// A constant wing height or width, decoded, or -1.
    pub wing_hc: f64,
    pub wing_wc: [f64; 2],
    /// The width curve's end values, the ends' geometry and radii.
    pub wing_ends: [f64; 2],
    pub wing_geo: [f64; 4],
    pub wing_rc: [f64; 2],
    /// In cut mode, where the cutters reach the ground (-1 never).
    pub wing_thru: f64,
    /// The profile's height at 0.9 R and under a spherical dimple.
    pub prof09: f64,
    pub prof_cut: f64,
    /// Whether the foot is notched (cut through, or fluted to the rim):
    /// the contact ring then reads the outline.
    pub foot_notched: bool,
}

/// What the shadow bake hands the shader beside its knot texture and
/// self-shadow table.
#[derive(Clone, Debug, Default)]
pub struct ShadeConsts {
    /// The revolve outline's knots, their count and the height scale.
    pub sil: [[f64; 4]; 8],
    pub sil_n: f64,
    pub sil_s: f64,
    /// The wing's knots, their count, the floor they stand on and the
    /// section's taper.
    pub wk: [[f64; 4]; 4],
    pub wk_n: f64,
    pub wk_b: f64,
    pub wt: [f64; 4],
}

/// A style's geometry, baked: everything that depends on its curves and
/// the material's crease blur, and nothing that depends on the light.
pub struct GeomBake {
    /// `DATA_ROWS` rows of `TAPS` texels, each (slope, value) at 16 bits as
    /// the bench packs them: slope high byte in red and low in blue, value
    /// high in green and low in alpha. BGRA in a u32.
    pub data: Vec<u32>,
    pub consts: GeomConsts,
    /// The profile, wing section, width and height at 256 taps, which the
    /// shadow bake reads.
    h: Vec<f64>,
    pv: Vec<f64>,
    wvs: Vec<f64>,
    hvs: Vec<f64>,
}

/// A style's shadows under one light and depth, baked.
pub struct ShadeBake {
    /// The knots as floats: the outline's eight, then the wing's four.
    pub knots: Vec<f32>,
    /// The self-shadow table, BGRA: .r = .b the 2D measure, .g the 3D one.
    pub shade: Vec<u32>,
    pub consts: ShadeConsts,
}

/// The geometry bake depends on the style and the material's crease blur.
#[derive(Clone, Copy, PartialEq)]
pub struct GeomKey {
    pub style: usize,
    pub psmooth: f64,
}

/// The shadow bake on the style, the knob's depth and the light.
#[derive(Clone, Copy, PartialEq)]
pub struct ShadeKey {
    pub style: usize,
    pub psmooth: f64,
    pub pdepth: f64,
    pub lx: f64,
    pub ly: f64,
    pub lz: f64,
}

impl GeomKey {
    pub fn new(style: usize, m: &KnobMaterial) -> Self {
        Self { style, psmooth: m.psmooth.round() }
    }
}

impl ShadeKey {
    pub fn new(style: usize, m: &KnobMaterial) -> Self {
        Self { style, psmooth: m.psmooth.round(), pdepth: m.pdepth, lx: m.lx, ly: m.ly, lz: m.lz }
    }
}

/// Bake one style's curves and wing (the bench's `bakeProfile`,
/// `bakeFlute`, `bakeWing`).
pub fn bake_geometry(style: &KnobStyle, key: &GeomKey) -> GeomBake {
    let mut c = GeomConsts::default();
    let prof = bake_profile(style.prof, key.psmooth);
    let flute = bake_curve(style.flute);
    let wwid = bake_curve(style.wwid);
    let whgt = bake_curve(style.whgt);
    let wprof = bake_curve(style.wprof);

    // The grip band, from the profile's anchors.
    let fa = style.prof[style.ffrom.min(style.prof.len() - 1)][0];
    let fb = style.prof[style.fto.min(style.prof.len() - 1)][0];
    c.flute_r = [fa.min(fb), fa.max(fb)];

    // bakeWing.
    let hv64 = resample(style.whgt, 64);
    c.wing_top = hv64.iter().fold(0.0f64, |a, b| a.max(*b)) * 2.0;
    c.wing_hc = flat_curve(style.whgt)[1];
    c.wing_wc = flat_curve(style.wwid);
    let wv = resample(style.wwid, 256);
    c.wing_ends = [wv[0], wv[255]];
    let slopes = [(wv[1] - wv[0]) * 255.0, (wv[255] - wv[254]) * 255.0];
    let w2n = style.wwmax * 0.5;
    let hs = (c.wing_ends[0] * w2n).max(0.0);
    let he = (c.wing_ends[1] * w2n).max(0.0);
    let er = style.wendr.clamp(0.0, 1.0);
    let mut ln = (style.wr1 - style.wr0).max(0.001);
    let (mut a0, mut a1, mut ms, mut me, mut rs, mut re) = (style.wr0, style.wr1, 0.0, 0.0, 0.0, 0.0);
    for _ in 0..16 {
        me = slopes[1] * w2n / ln;
        ms = -slopes[0] * w2n / ln;
        let se = (1.0 + me * me).sqrt();
        let ss = (1.0 + ms * ms).sqrt();
        re = er * he * se;
        rs = er * hs * ss;
        a1 = style.wr1 - re * (1.0 + me / se);
        a0 = style.wr0 + rs * (1.0 + ms / ss);
        ln = (a1 - a0).max(0.001);
    }
    c.wing_geo = [a0, a1, ms, me];
    c.wing_rc = [rs, re];
    let pv = resample(style.wprof, 256);
    let dmax = hv64.iter().fold(0.0f64, |a, b| a.max(b * 2.0));
    c.wing_thru = -1.0;
    for (j, v) in pv.iter().enumerate() {
        if 1.0 - dmax * (1.0 - v) <= 0.002 {
            c.wing_thru = j as f64 / 255.0;
            break;
        }
    }

    let h = resample(style.prof, TAPS);
    let hvs = resample(style.whgt, 256);

    // What the shader precomputed per draw in the bench (solidPre).
    c.prof09 = row_at(&prof, 0.9)[1];
    c.prof_cut = row_at(&prof, style.cr.clamp(0.0, 1.0))[1];
    let cut_through = style.cut > 0.5 && style.cf < 0.001 && (style.cut > 1.5 || style.csph < 0.5);
    c.foot_notched = cut_through || (style.flutes >= 1.0 && c.flute_r[1] > 0.9);

    // The curve texture, 16 bits a value as the bench packs it.
    let mut data = vec![0u32; TAPS * DATA_ROWS];
    for (r, row) in [&prof, &flute, &wwid, &whgt, &wprof].iter().enumerate() {
        for (i, t) in row.iter().enumerate() {
            let sv = (t[0] * 65535.0).round() as u32;
            let hv = (t[1] * 65535.0).round() as u32;
            let (red, blue) = (sv >> 8, sv & 255);
            let (green, alpha) = (hv >> 8, hv & 255);
            data[r * TAPS + i] = (alpha << 24) | (red << 16) | (green << 8) | blue;
        }
    }
    GeomBake { data, consts: c, h, pv, wvs: wv, hvs }
}

/// Bake one style's shadows under one light (the bench's `bakeShade`): the
/// revolve's outline by height, the wing's knots and taper, and the
/// self-shadow table.
pub fn bake_shade(style: &KnobStyle, geom: &GeomBake, key: &ShadeKey) -> ShadeBake {
    let mut c = ShadeConsts::default();
    let g = &geom.consts;
    let (h, pv, wvs, hvs) = (&geom.h, &geom.pv, &geom.wvs, &geom.hvs);
    // bakeShade.
    let np = TAPS;
    let ll = key.lx.hypot(key.ly);
    let tanel = key.lz.max(0.02).max(0.05) / ll.max(0.05);
    let zs = key.pdepth.max(0.001) / tanel;
    let has_wing = style.wr1 - style.wr0 > 0.01;
    let cut_mode = has_wing && style.wmode > 0.5;
    let mut zclip = 1e9;
    c.wt = [1.0; 4];
    if has_wing && !cut_mode {
        let t_of = |f: f64| -> f64 {
            for k in (0..256).rev() {
                if pv[k] >= f - 1e-6 {
                    return k as f64 / 255.0;
                }
            }
            0.0
        };
        c.wt = [t_of(0.0), t_of(0.33), t_of(0.67), t_of(0.95).max(0.05)];
        let (ga0, ga1) = (g.wing_geo[0], g.wing_geo[1]);
        let la = (ga1 - ga0).max(1e-3);
        let lh = (style.wr1 - style.wr0).max(1e-3);
        let w_at = |al: f64| (at(wvs, (al - ga0) / la) * style.wwmax * 0.5).max(0.004);
        let z_at = |al: f64| at(hvs, (al - style.wr0) / lh) * 2.0 + style.wbase;
        let mut s0 = style.wr0 + 0.9 * w_at(style.wr0);
        let mut s1 = style.wr1 - 0.9 * w_at(style.wr1);
        if s1 < s0 {
            s0 = (style.wr0 + style.wr1) / 2.0;
            s1 = s0;
        }
        let wp: Vec<[f64; 3]> = (0..=32)
            .map(|i| {
                let al = s0 + (s1 - s0) * i as f64 / 32.0;
                [al, w_at(al), z_at(al)]
            })
            .collect();
        let ki = dp_idx(
            &[wp.iter().map(|p| [p[0], p[1]]).collect(), wp.iter().map(|p| [p[0], p[2] * zs]).collect()],
            0.01,
            4,
        );
        for (n, &k) in ki.iter().enumerate() {
            let a = ki[n.saturating_sub(1)];
            let b = ki[(n + 1).min(ki.len() - 1)];
            let m = if b > a { (wp[b][1] - wp[a][1]) / (wp[b][0] - wp[a][0]).max(1e-6) } else { 0.0 };
            c.wk[n] = [wp[k][0], wp[k][1] / (1.0 + m * m).sqrt(), wp[k][2], 0.0];
        }
        c.wk_n = ki.len() as f64;
        c.wk_b = 0.0;
    } else if cut_mode {
        let depth = at(hvs, 0.5) * 2.0;
        let gap = at(wvs, 0.5) * style.wwmax * 0.5;
        let mut xr = 0.16;
        for (i, v) in pv.iter().enumerate() {
            if *v <= 0.25 {
                xr = i as f64 / 255.0;
                break;
            }
        }
        zclip = (1.0 - depth * (1.0 - at(pv, xr))).max(0.0);
        let ztop = h[0];
        let mut rtop = 0.0;
        for i in (0..np).rev() {
            if h[i] >= ztop - 0.02 {
                rtop = i as f64 / (np - 1) as f64;
                break;
            }
        }
        let half = (rtop - 0.5 * gap).max(0.02);
        let x_of = |z: f64| -> f64 {
            let v = 1.0 - (1.0 - z) / depth.max(1e-3);
            for (k, p) in pv.iter().enumerate() {
                if *p <= v {
                    return k as f64 / 255.0;
                }
            }
            1.0
        };
        let wb = gap + xr;
        c.wt = [
            1.0,
            (gap + x_of(zclip + (1.0 - zclip) / 3.0)) / wb,
            (gap + x_of(zclip + 2.0 * (1.0 - zclip) / 3.0)) / wb,
            (gap / wb).max(0.05),
        ];
        c.wk[0] = [-half, wb, ztop, 0.0];
        c.wk[1] = [half, wb, ztop, 0.0];
        c.wk_n = 2.0;
        c.wk_b = zclip;
    }

    // The revolve's outline by height, at most six knots.
    let hc: Vec<f64> = h.iter().map(|v| v.min(zclip)).collect();
    let zmax = hc.iter().fold(0.0f64, |a, b| a.max(*b));
    let sp: Vec<[f64; 2]> = (0..=96)
        .map(|j| {
            let z = zmax * j as f64 / 96.0;
            let mut r = 0.0;
            for i in (0..np).rev() {
                if hc[i] >= z - 1e-6 {
                    r = i as f64 / (np - 1) as f64;
                    if i < np - 1 && hc[i] > hc[i + 1] {
                        r += ((hc[i] - z) / (hc[i] - hc[i + 1])).min(1.0) / (np - 1) as f64;
                    }
                    break;
                }
            }
            [z * zs, r]
        })
        .collect();
    let ks = dp_idx(&[sp.clone()], 0.012, 6);
    c.sil_n = ks.len() as f64;
    c.sil_s = zs;
    for i in 0..8 {
        let ka = sp[ks[i.min(ks.len() - 1)]];
        let kb = sp[ks[(i + 1).min(ks.len() - 1)]];
        let l = kb[0] - ka[0];
        let dr = ka[1] - kb[1];
        let (mut sb, mut cb) = (0.0, 0.0);
        if l > dr.abs() + 1e-4 {
            sb = dr / l;
            cb = (1.0 - sb * sb).sqrt();
        }
        c.sil[i] = [ka[0], ka[1], sb, cb];
    }

    // The self-shadow over the disc.
    let n = SHADE_N;
    let (dx, dy) = if ll > 1e-4 { (key.lx / ll, key.ly / ll) } else { (0.0, -1.0) };
    let kz = tanel / key.pdepth.max(0.001);
    let ztop2 = g.wing_top.max(1.0);
    let m_steps = 24;
    let lz3 = key.lz.max(0.02);
    let ln3 = (key.lx * key.lx + key.ly * key.ly + lz3 * lz3).sqrt();
    let l3 = [key.lx / ln3, key.ly / ln3, lz3 / ln3];
    let hh_at = |r: f64| -> f64 {
        if r >= 1.0 {
            return hc[np - 1] - (r - 1.0) * 8.0;
        }
        let t = r * (np - 1) as f64;
        let k = t.floor() as usize;
        hc[k] + (hc[k + 1] - hc[k]) * (t - k as f64)
    };
    let mut shade = vec![0u32; n * n];
    for j in 0..n {
        for i in 0..n {
            let x = (i as f64 + 0.5) / n as f64 * 2.0 - 1.0;
            let y = (j as f64 + 0.5) / n as f64 * 2.0 - 1.0;
            let rr = x.hypot(y);
            let (mut v, mut v3) = (0.0, 0.0);
            if rr < 1.0 + 2.0 / n as f64 {
                let zp = hh_at(rr.min(1.0));
                let tb = ((ztop2 - zp) / kz).max(0.01).min(1.2);
                let mut occ = 0.0;
                for m in 0..m_steps {
                    let fi = (m as f64 + 0.5) / m_steps as f64;
                    let t = tb * fi * fi.sqrt();
                    let (qx, qy) = (x + dx * t, y + dy * t);
                    occ += ((hh_at((qx * qx + qy * qy).sqrt()) - zp - t * kz) / 0.25).clamp(0.0, 1.0);
                }
                v = (occ / m_steps as f64 / 0.375).clamp(0.0, 1.0);
                let hh = 56.0 * key.pdepth.max(0.001);
                let mut res: f64 = 1.0;
                let tm = (hh * g.wing_top.max(1.0) * 1.1 + 1.0 - zp * hh) / l3[2];
                for m in 0..48 {
                    let t3 = tm * (m as f64 + 0.5) / 48.0;
                    let hx = x * 56.0 + l3[0] * t3;
                    let hy = y * 56.0 + l3[1] * t3;
                    let clear = zp * hh + l3[2] * t3 - (hh_at((hx * hx + hy * hy).sqrt() / 56.0) * hh).max(0.0)
                        + (t3 * 0.04).max(0.5);
                    res = res.min(6.0 * clear / t3.max(1.0));
                }
                let res = res.clamp(0.0, 1.0);
                v3 = 1.0 - res * res * (3.0 - 2.0 * res);
            }
            let a = (v * 255.0).round() as u32;
            let g = (v3 * 255.0).round() as u32;
            // BGRA in a u32: A R G B from the top byte. Red and blue carry
            // the same value, so a swizzled read reads the same.
            shade[j * n + i] = (255 << 24) | (a << 16) | (g << 8) | a;
        }
    }

    let mut knots = vec![0.0f32; KNOT_FLOATS];
    for (i, k) in c.sil.iter().chain(c.wk.iter()).enumerate() {
        for (lane, v) in k.iter().enumerate() {
            knots[i * 4 + lane] = *v as f32;
        }
    }
    ShadeBake { knots, shade, consts: c }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The linear value of an ink's channels.
pub fn ink(c: u32) -> [f64; 3] {
    [((c >> 24) & 255) as f64 / 255.0, ((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0]
}

/// `envHDR2` for the straight-up reflection with no footprint, at one
/// roughness: the environment's brightness a flat face sees, which every
/// reflection is measured against. The bench works it out per vertex.
fn env_up(m: &KnobMaterial, rgh: f64) -> [f64; 3] {
    let rv = [0.0, 0.0, 1.0];
    let light = [m.lx, m.ly, m.lz.max(0.02)];
    let kl = norm3(light);
    let (kvis, fp) = (1.0, 0.0);
    let w = 0.03 + rgh * 0.45;
    let en = 1.0 / (1.0 + rgh * 3.0);
    let wf = (w * w + fp * fp).sqrt();
    let ground = ink(m.ground);
    let rect_d = |dir: [f64; 3], hs: [f64; 2], rc: f64| -> f64 {
        let rlu = norm3({
            let c = cross3([0.0, 0.0, 1.0], dir);
            [c[0] + 0.0001, c[1], c[2]]
        });
        let rlv = cross3(dir, rlu);
        let rlc = dot3(rv, dir);
        if rlc < 0.05 {
            return 1e9;
        }
        let q = [(dot3(rv, rlu) / rlc).abs() - hs[0] + rc, (dot3(rv, rlv) / rlc).abs() - hs[1] + rc];
        q[0].max(0.0).hypot(q[1].max(0.0)) + q[0].max(q[1]).min(0.0) - rc
    };
    let rect_cov = |d: f64, w: f64| 1.0 - smoothstep(-w, w, d);
    let mix3 = |a: [f64; 3], b: [f64; 3], t: f64| {
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    };
    if m.envk > 1.5 {
        let s = rv[2].clamp(0.0, 1.0).sqrt();
        let sky = mix3([1.5, 0.97 * 1.5, 0.92 * 1.5], [0.28, 0.46, 0.85], s);
        let gk = 0.7 + 0.3 * (-rv[2] * 3.0).clamp(0.0, 1.0);
        let gnd = [0.16 * gk, 0.145 * gk, 0.13 * gk];
        let sun = [40.0 * m.li * kvis, 0.94 * 40.0 * m.li * kvis, 0.84 * 40.0 * m.li * kvis];
        let ck = 1.0 - dot3(rv, kl);
        let ks = 900.0 + (20.0 - 900.0) * rgh;
        let ksf = ks / (1.0 + 0.5 * ks * fp * fp);
        let base = mix3(gnd, sky, smoothstep(-0.5 * w, 0.5 * w, rv[2]));
        let k = en * (ksf / ks) * (-ck * ksf).exp();
        return [base[0] + sun[0] * k, base[1] + sun[1] * k, base[2] + sun[2] * k];
    }
    if m.envk > 0.5 {
        let fc = mix3([0.55; 3], ground, 0.3);
        let floor_c = [fc[0] * 0.75, fc[1] * 0.75, fc[2] * 0.75];
        let wall_c = [0.03 + 0.6 * smoothstep(0.0, 0.9, rv[2]); 3];
        let dt = rect_d([0.0, 0.0, 1.0], [0.36, 0.36], 0.36);
        let dk = rect_d(kl, [0.42, 0.26], 0.08);
        let mut ec = mix3(floor_c, wall_c, smoothstep(-0.4 * w, 0.4 * w, rv[2]));
        let add = 1.1 * en * rect_cov(dt, wf) + 3.5 * m.li * en * kvis * rect_cov(dk, wf);
        let az0 = kl[1].atan2(kl[0]);
        let mut strips = 0.0;
        for si in 0..3 {
            let az = az0 + 1.5708 * (si as f64 + 1.0);
            let ds = rect_d(norm3([az.cos(), az.sin(), 0.35]), [0.05, 0.8], 0.02);
            strips += 5.0 * en * (0.05 + w) / (0.05 + wf) * rect_cov(ds, wf);
        }
        let hz = (rv[2] - 0.12).abs();
        let lw = w * 0.6 + 0.01;
        let lwf = (lw * lw + fp * fp).sqrt();
        let line = 2.5 * en * lw / lwf * (1.0 - smoothstep(0.0, lwf, hz));
        for c in ec.iter_mut() {
            *c += add + strips + line;
        }
        return ec;
    }
    let wall = mix3([0.5; 3], ground, 0.5);
    let k = (0.25 + 0.5 * smoothstep(-0.2, 1.0, rv[2])) * (0.2 + 0.8 * smoothstep(-0.3, 0.05, rv[2]));
    let dk = rect_d(kl, [0.42, 0.26], 0.08);
    let df = rect_d(norm3([-kl[0] + 0.0001, -kl[1], 0.45]), [0.07, 0.55], 0.0);
    let add = 7.0 * m.li * en * kvis * rect_cov(dk, wf) + 2.2 * en * (0.07 + w) / (0.07 + wf) * rect_cov(df, wf);
    [wall[0] * k + add, wall[1] * k + add, wall[2] * k + add]
}

/// The bench's `vEnvRef`: the luminance a flat face reflects, and the clear
/// coat's reflection straight up, which the reflections subtract so a flat
/// face keeps its own colour.
pub fn env_ref(m: &KnobMaterial) -> [f64; 4] {
    if m.env < 0.001 {
        return [0.05, 0.0, 0.0, 0.0];
    }
    let e0 = env_up(m, m.rough.clamp(0.0, 1.0));
    let ec = if m.coat > 0.001 { env_up(m, m.coatr.clamp(0.0, 1.0)) } else { [0.0; 3] };
    let lum = (e0[0] * 0.2126 + e0[1] * 0.7152 + e0[2] * 0.0722).max(0.05);
    [lum, ec[0], ec[1], ec[2]]
}
