//! The terrain as the scene reads it: per output column the five ridge elevations, each tree
//! layer's foot line, canopy top and density, the land envelope, the near ground rise, the
//! off-centre factor and the city glow's azimuth part (the page's `sgColumns`), and the single trees
//! (`sgTrees`), drawn once per view into a coverage target with one channel per tree layer
//! (ranges 2, 3, 4 and the front treeline) that the range instances sample.
//!
//! Owner after the foundation: stage B (the trees). The columns were built by the foundation
//! because the ranges need them; the trees are stubs ([`Trees`], `DrawSkyTrees`).
use crate::params::{P, Params};
use crate::render::SceneCtx;
use crate::rng::{fract, smooth, TAU};
use crate::zones::Zones;
use makepad_widgets::makepad_draw::*;

/// Texels per column: rows 0-4 the ranges (ridge, tree foot, canopy top, density), row 5 the front
/// treeline (-, foot, canopy top, density), row 6 (land, ground rise, off-centre factor t, city glow).
pub const ROWS: usize = 7;
/// The column texture's widest row; more columns wrap into further bands of 7 rows.
pub const TEX_W: usize = 8192;

/// The parameters the columns depend on (with the seed and the view).
pub const COLUMN_KEYS: &[P] = &[
    P::MtH, P::MtW, P::MtC, P::MtRug, P::Land, P::LandC, P::TrD, P::TrH, P::TrCon, P::TrW, P::TrC, P::Treeline, P::CtW, P::CtC,
    P::CtR, P::Off, P::OffDir,
];

/// Columns beyond the view on each side: the feet of trees reaching in.
pub fn margin(p: &Params, span: f64, w: usize) -> usize {
    (((0.045 / (span * TAU / w as f64 * (1.0 - p[P::Off]).max(0.1))).ceil() as usize) + 2).min(2048)
}

/// The off-centre camera (the page's `sgWarp`, bit-exact): the world azimuth the raw view azimuth `ur`
/// sees (the scenery is a ring of radius 1, the camera `off` from its centre toward `offDir`) and
/// the distance factor t that heights and pixel sizes scale by. For the trees and the props.
#[allow(dead_code)]
pub fn warp(p: &Params, ur: f64) -> (f64, f64) {
    let (cx, cy, dx, dy) = (p[P::Off] * (p[P::OffDir] * TAU).cos(), p[P::Off] * (p[P::OffDir] * TAU).sin(), (ur * TAU).cos(), (ur * TAU).sin());
    let cd = cx * dx + cy * dy;
    let t = -cd + (cd * cd - cx * cx - cy * cy + 1.0).max(1e-4).sqrt();
    (fract((cy + t * dy).atan2(cx + t * dx) / TAU), t)
}

/// The column data of one view: `n = w + 2 m` columns from `u0 - m` pixels, 7 texels each
/// (column i's row r at `i * ROWS + r`).
pub fn columns(z: &Zones, p: &Params, u0: f64, span: f64, w: usize, m: usize) -> Vec<[f32; 4]> {
    let rng = &z.rng;
    let nc = w + 2 * m;
    let mut out = vec![[0f32; 4]; nc * ROWS];
    let px0 = span * TAU / w as f64;
    let (cx, cy) = (p[P::Off] * (p[P::OffDir] * TAU).cos(), p[P::Off] * (p[P::OffDir] * TAU).sin());
    let cc = cx * cx + cy * cy;
    let hk: [f64; 5] = std::array::from_fn(|k| z.ridge.hk(k));
    let c_d = 28.0 * p[P::CtR];
    let cdx = (p[P::CtC] * TAU).cos() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).cos();
    let cdy = (p[P::CtC] * TAU).sin() * c_d - 0.8 * p[P::Off] * c_d * (p[P::OffDir] * TAU).sin();
    let city_b = fract(cdy.atan2(cdx) / TAU);
    // the conifer share shapes the canopy's roughness (the page's `trC` here is `P.trCon`)
    let (tr_d, tr_c) = (p[P::TrD], p[P::TrCon]);
    // the column part of the forest: undergrowth, the canopy top and its pixel-scale roughness
    let canopy = |u: f64, px: f64, cells: f64, ly: f64, hmax: f64, base: f64, reg: f64| -> [f64; 3] {
        if reg < 0.05 { return [base, base, 0.0]; }
        let rg = smooth(0.05, 0.3, reg);
        let nf = (TAU / (2.0 * px)).floor().max(1.0);
        let (mut b0, mut b1, mut b2) = (0.0, 0.0, 0.72);
        for (j, f) in [3.0, 11.0, 0.5, 2.0, 6.0, 19.0, 43.0].into_iter().enumerate() {
            let per = if j == 2 { (cells * 0.5).floor() } else { cells * f };
            let x = u * cells * f;
            let oo = ly + if j < 2 { 50.0 + j as f64 } else { 52.0 + (j - 2) as f64 };
            let vn = rng.pn(x, per, oo);
            let vl = if j >= 4 { rng.pl(x, per, oo) } else { 0.0 };
            match j {
                0 => b0 += 0.07 * vn,
                1 => b0 += 0.04 * vn,
                2 => b1 = vn,
                3 => b2 += 0.30 * vn,
                4 | 5 => b2 += (if j == 4 { 0.22 } else { 0.12 }) * (vn + (vl - vn) * tr_c - 0.5),
                _ => b2 += 0.16 * tr_c * (vl - 0.5),
            }
        }
        let tn = rng.pn(fract(u) * nf, nf, ly + 71.0);
        // a closed forest's canopy reaches most of the way up its trees
        let bush = hmax * (0.05 + b0) * (0.35 + 0.65 * tr_d) * rg;
        let body = hmax * (0.25 + 0.75 * tr_d * tr_d) * (0.55 + 0.45 * b1) * b2 * rg;
        [base, base + bush.max(body) + (0.6 * TAU / nf).min(hmax * 0.04) * (tn - 0.5) * rg, rg]
    };
    for i in 0..nc {
        let x = i as f64 - m as f64;
        let ur = fract(u0 + (x + 0.5) / w as f64 * span);
        let th = ur * TAU;
        let (dx, dy) = (th.cos(), th.sin());
        let cd = cx * dx + cy * dy;
        let t = -cd + (cd * cd - cc + 1.0).max(1e-4).sqrt();
        let uw = fract((cy + t * dy).atan2(cx + t * dx) / TAU);
        let px = px0 * t;
        let env = z.ridge.envelopes(rng, uw);
        let lenv = rng.region(uw, p[P::LandC], p[P::Land], 600.0);
        let freg = rng.region(uw, p[P::TrC], p[P::TrW], 450.0) * smooth(0.25, 0.6, lenv);
        let cenv = rng.region(ur, city_b, p[P::CtW], 270.0);
        let gh = (0.0035 + 0.0025 * rng.fbm(uw, 24.0, 500.0)) * lenv;
        let o = &mut out[i * ROWS..(i + 1) * ROWS];
        for k in 0..5 {
            let menv = z.ridge.menv(k, env);
            let (h, nz) = z.ridge.mtn_h(rng, uw, k, menv, px);
            let hr = hk[k] * menv * h;
            let mut row = [hr, 0.0, 0.0, 0.0];
            if k >= 2 {
                let kk = (k - 2) as f64;
                let t_reg = rng.region(uw, p[P::TrC], p[P::TrW], 400.0 + kk * 10.0);
                let base = hr.min(p[P::Treeline] * (0.55 + 0.9 * rng.pn(uw * 17.0, 17.0, 20.0 + k as f64 * 10.0 + 5.0)) * (0.8 + 0.4 * nz));
                let c = canopy(uw, px, 260.0 + 130.0 * kk, 410.0 + kk * 10.0, p[P::TrH] * (0.7 + 0.3 * kk), base, t_reg);
                row = [hr, c[0], c[1], c[2]];
            }
            o[k] = row.map(|v| v as f32);
        }
        let c = canopy(uw, px, 520.0, 400.0, p[P::TrH], gh, freg);
        o[5] = [0.0, c[0] as f32, c[1] as f32, c[2] as f32];
        o[6] = [lenv as f32, gh as f32, t as f32, ((0.55 + 0.45 * smooth(0.3, 0.7, rng.fbm(ur, 3.0, 50.0))) * cenv) as f32];
    }
    out
}

/// The tallest single tree of tree layer `l` (0-2 ranges 2-4, 3 the front treeline), rad above its foot.
pub fn tree_hmax(p: &Params, l: usize) -> f64 {
    if l < 3 {
        p[P::TrH] * (0.7 + 0.3 * l as f64)
    } else {
        p[P::TrH]
    }
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The single trees' coverage, one channel per tree layer, drawn with max blending (stage B).
    mod.draw.DrawSkyTrees = mod.std.set_type_default() do #(DrawSkyTrees::script_shader(vm)){
        ..mod.draw.DrawQuad,
        pixel: fn(){
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.draw.SkyTreesBase = #(Trees::script_component(vm))
    mod.draw.SkyTrees = mod.std.set_type_default() do mod.draw.SkyTreesBase{
        draw_trees: mod.draw.DrawSkyTrees{}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyTrees { #[deref] pub draw_super: DrawQuad }

/// The single trees of a view: their mesh (rebuilt when the trees' parameters or the pixel size
/// change) and their coverage target. Stub until stage B: no single trees, so the forests show as
/// their canopy bands.
#[derive(Script, ScriptHook)]
pub struct Trees { #[live] pub draw_trees: DrawSkyTrees }

impl Trees {
    /// Draws the trees into their coverage target, a pass the scene pass `consumer` samples
    /// (attach it with `cx.attach_child_pass(trees, consumer, None)`). Called only when the view's
    /// picture changes.
    pub fn draw_pass(&mut self, _cx: &mut Cx2d, _sc: &SceneCtx, _consumer: DrawPassId) {}

    /// The coverage target (one channel per tree layer), once there is one.
    pub fn coverage(&self) -> Option<&Texture> { None }
}
