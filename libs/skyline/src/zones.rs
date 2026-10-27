//! Where everything stands on the circle: the five ranges' ridges and the placement zones of the city
//! and every prop family (the page's `propZones`, ported operation for operation, so the zones land
//! bit-exactly where the page puts them). A zone is [centre u, half-width u, nearest m, farthest m];
//! half-width 0: absent.
use crate::params::{P, Params};
use crate::rng::*;

pub type Zone = [f64; 4];
const Z0: Zone = [0.0; 4];

/// Range k (0 farthest .. 4 nearest): relative height, alpine share at full ruggedness, summit cells.
pub fn mt_scale(k: usize) -> f64 { [0.85, 1.15, 1.25, 0.85, 0.50][k] }
fn mt_alp(k: usize) -> f64 { [1.15, 1.25, 1.1, 0.7, 0.25][k] }
fn mt_cells(k: usize) -> f64 { [30.0, 24.0, 20.0, 24.0, 34.0][k] }
/// Range k's summit cells at Size `size`: the base's count over Size, whole (at least one), so its summits are Size times
/// as wide and as many over Size.
fn cells_at(k: usize, size: f64) -> f64 { (mt_cells(k) / size).round().max(1.0) }
/// Nominal distance (m) of range k: 30 km (0) .. 5 km (4).
pub fn mt_dist(k: f64) -> f64 { 5000.0 * 6f64.powf((4.0 - k) * 0.25) }
/// The zones' ridge footprint: roughness octaves down to a 16k export's pixel.
pub const MPX: f64 = TAU / 16384.0;

/// The first of the `n` candidates `f(j)` = (score, candidate) with the highest score above -9.
fn argmax<T: Default>(n: usize, mut f: impl FnMut(f64) -> (f64, T)) -> T {
    let (mut best, mut out) = (-9.0, T::default());
    for j in 0..n { let (s, t) = f(j as f64); if s > best { (best, out) = (s, t); } }
    out
}

/// The ridge model of one parameter set: per range its peak height, ruggedness, summit cells and summit table
/// (per summit cell: height bound r1^1.5, slope, lean, offset), so a summit costs no hashing.
pub struct Ridges { hk: [f64; 5], rk: [f64; 5], cells: [f64; 5], tab: [Vec<[f64; 4]>; 5], menv_s: f64, w_a: f64, mt_c: f64, mt_w: f64 }

impl Ridges {
    /// At Size k the peaks are k times as high and their cells k times as wide: each base cell's summit goes to the cell
    /// that holds its centre, the tallest where several meet (so the highest peak stands near its place), and a cell none
    /// reaches (k under 1) draws a summit of its own.
    pub fn new(p: &Params, rng: &Rng) -> Self {
        let cells = std::array::from_fn(|k| cells_at(k, p[P::Size]));
        let row = |c: f64, st: f64, k: usize| {
            let (r1, r2) = (rng.rnd(c, st, 20.0 + k as f64 * 10.0), rng.rnd(c, st + 1.0, 20.0 + k as f64 * 10.0));
            [r1 * r1.sqrt(), 0.28 + 0.32 * fract(r1 * 37.7), 0.65 + 0.7 * fract(r2 * 53.1), 0.8 * r2]
        };
        let tab = std::array::from_fn(|k| {
            let n: f64 = cells[k];
            let mut t: Vec<Option<[f64; 4]>> = vec![None; n as usize];
            for c in 0..mt_cells(k) as usize {
                let (i, r) = ((((c as f64 + 0.5) * n / mt_cells(k)).floor() as usize).min(n as usize - 1), row(c as f64, 2.0, k));
                if t[i].is_none_or(|q| r[0] > q[0]) { t[i] = Some(r); }
            }
            t.into_iter().enumerate().map(|(i, r)| r.unwrap_or_else(|| row(i as f64, 4.0, k))).collect()
        });
        Self { hk: std::array::from_fn(|k| p[P::MtH] * mt_scale(k) * p[P::Size]), rk: std::array::from_fn(|k| clamp(p[P::MtRug] * mt_alp(k), 0.0, 1.0)), cells, tab,
            menv_s: smooth(0.0, 0.04, p[P::MtW]), w_a: clamp(p[P::MtW] * 1.3, 0.0, 1.0), mt_c: p[P::MtC], mt_w: p[P::MtW] }
    }
    /// Peak height (rad) of range k.
    pub fn hk(&self, k: usize) -> f64 { self.hk[k] }
    /// The massif's wide and core envelopes at u.
    pub fn envelopes(&self, rng: &Rng, u: f64) -> (f64, f64) { (rng.region(u, self.mt_c, self.w_a, 20.0), rng.region(u, self.mt_c + 0.02, self.mt_w, 30.0)) }
    /// Range k's share of the massif: far ranges spread wide, near ones keep to the core.
    pub fn menv(&self, k: usize, env: (f64, f64)) -> f64 {
        let t = (4 - k) as f64 / 4.0;
        mix(env.0, env.1, if t < 0.3 { 0.15 } else if t < 0.6 { 0.5 } else { 1.0 }) * self.menv_s
    }
    /// The highest summit of cells ci-2 ..= ci+2 at x (a cell whose height bound cannot beat the best is skipped).
    fn summits(&self, k: usize, x: f64, q: f64, sh: f64) -> f64 {
        let (cells, ci, hgm, rr) = (self.cells[k], x.floor(), mix(0.4, 1.0, sh), mix(0.05, 0.004, sh));
        let mut best: f64 = 0.0;
        for c in [ci, ci + 1.0, ci - 1.0, ci + 2.0, ci - 2.0] {
            let row = self.tab[k][(c - cells * (c / cells).floor()) as usize];
            let hg = row[0] * hgm;
            if hg <= best { continue; }
            let (hw, dx) = (clamp(hg * q / row[1] * mix(2.4, 1.4, sh), 0.2, 1.5), x - (c + 0.1 + row[3]));
            let d = if dx < 0.0 { -dx / (hw * row[2]) } else { dx / (hw * (2.0 - row[2])) };
            let f = (1.0 - ((d * d + rr).sqrt() - rr.sqrt())).max(0.0);
            best = best.max(hg * mix(f * f * (3.0 - 2.0 * f), mix(f, f * f, 0.8 * sh), sh));
        }
        best
    }
    /// The page's `mtnH`: range k's height factor at u (times `hk * menv`), roughness octaves down to
    /// the pixel footprint `px`; and the second octave's value, which the range's tree line follows.
    pub fn mtn_h(&self, rng: &Rng, u: f64, k: usize, menv: f64, px: f64) -> (f64, f64) {
        let (ly, b, rk, cells) = (20.0 + k as f64 * 10.0, 2.0 + k as f64, self.rk[k], self.cells[k]);
        let n = 0.667 * rng.pn(u * b, b, ly + 8.0) + 0.333 * rng.pn(u * b * 2.0, b * 2.0, ly + 11.0);
        let alp = smooth(-0.25, 0.25, (n - 0.5) * 1.9 + rk * 1.75 - 0.7 + (menv - 0.5) * 0.9 * (0.4 + rk));
        let (swell, q) = (smooth(0.3, 0.7, n), self.hk[k] * cells / TAU);
        let s1 = self.summits(k, u * cells, q, alp);
        let (sm, g, f0) = (mix(0.9, 0.25, alp * rk), mix(0.42, 0.5, alp), (cells * (2.0 - 0.25 * k as f64) + 0.5).floor());
        let (mut f, mut a, mut d, mut fo, mut nz) = (f0, 1.0, 0.0, 0.7 * alp * rk, 0.5);
        for j in 0..12 {
            let w = clamp(TAU / (f * px) * 0.5 - 1.0, 0.0, 1.0);
            if w <= 0.0 { break; }
            let (x, jf) = (u * f, j as f64);
            let i = x.floor();
            let i1 = if i + 1.0 >= f { i + 1.0 - f } else { i + 1.0 };
            let fr = mix(x - i, (x - i) * (x - i) * (3.0 - 2.0 * (x - i)), sm.max(1.0 - 0.22 * jf));
            let v = mix(rng.rnd(i, 7.0 + jf, ly), rng.rnd(i1, 7.0 + jf, ly), fr);
            let v = mix(v, 1.0 - (2.0 * v - 1.0).abs(), fo);
            if j == 1 { nz = v; }
            (d, f, a, fo) = (d + a * w * (v - 0.5), f * 2.0, a * g, fo * 0.6);
        }
        let s = mix(0.1, 0.6 * mix(0.6, 1.0, rk), alp) * (0.35 + 0.65 * s1);
        ((0.04 + 0.1 * swell + 0.86 * s1 * (0.78 + 0.22 * swell) + s * cells / (f0 * (0.45 * q.max(0.45)).sqrt()) * d).max(0.015), nz)
    }
    /// Ridge elevation (rad) of the five ranges at u, roughness down to `px`.
    pub fn heights(&self, rng: &Rng, u: f64, px: f64) -> [f64; 5] {
        let (u, env) = (fract(u), self.envelopes(rng, fract(u)));
        std::array::from_fn(|k| { let menv = self.menv(k, env); self.hk[k] * menv * self.mtn_h(rng, u, k, menv, px).0 })
    }
}

/// The placement zones and the helpers the prop generators share (the page's `propZones.lib`).
/// The parameters are private: the zones are cached by their own keys only, so a generator reads
/// every other parameter from its own `Params`.
pub struct Zones {
    p: Params,
    pub rng: Rng,
    pub ridge: Ridges,
    /// The city core, the city, its cranes, the industry; `ind_x.x` its side of the city (+1 toward larger u, 0 no city).
    pub core: Zone, pub city: Zone, pub crane: Zone, pub ind: Zone, pub ind_x: Zone,
    /// The real shorelines: x where the sea begins toward larger u, y toward smaller; z 1 with both land and sea.
    pub shore: Zone,
    /// `town_x` (shore u, sea side ±1 or 0 inland), `bridge_x` (1 over water, shore u, sea side),
    /// `fair_x` (1 on a pier, direction from the town).
    pub town: Zone, pub town_x: Zone, pub church: Zone, pub bridge: Zone, pub bridge_x: Zone, pub fair: Zone, pub fair_x: Zone,
    pub lamp_t: Zone, pub lamp_c: Zone,
    /// Power lines (nearest point's bearing, its distance m, farthest pylon m, 1); wind farms (centre,
    /// half-width, range k, 1 crest / 2 open flat land / 3 offshore).
    pub pyl: [Zone; 3], pub turb: [Zone; 2],
    /// Up to four masts: azimuth (-1 none), range k (-1 open land), distance m, height m.
    pub mast_u: Zone, pub mast_k: Zone, pub mast_d: Zone, pub mast_h: Zone,
    /// The lakes on open land (`lakes::place`, after the zones).
    pub lakes: Vec<crate::lakes::Lake>,
}

impl Zones {
    pub fn new(p: &Params) -> Self {
        let rng = Rng::new(p.seed);
        let ridge = Ridges::new(p, &rng);
        let mut z = Zones { p: p.clone(), rng, ridge, core: Z0, city: Z0, crane: Z0, ind: Z0, ind_x: Z0, shore: Z0, town: Z0, town_x: Z0,
            church: Z0, bridge: Z0, bridge_x: Z0, fair: Z0, fair_x: Z0, lamp_t: Z0, lamp_c: Z0, pyl: [Z0; 3], turb: [Z0; 2],
            mast_u: [-1.0; 4], mast_k: [-1.0; 4], mast_d: [3000.0; 4], mast_h: [150.0; 4], lakes: Vec::new() };
        z.place();
        z.lakes = crate::lakes::place(&z, p);
        z
    }
    pub fn rnd(&self, id: f64, st: f64, ly: f64) -> f64 { self.rng.rnd(id, st, ly) }
    /// The land envelope at u (1 land, 0 sea), and the same without its wobble.
    pub fn land(&self, u: f64) -> f64 { self.rng.region(u, self.p[P::LandC], self.p[P::Land], 600.0) }
    pub fn land_n(&self, u: f64) -> f64 {
        let (land, h) = (self.p[P::Land], self.p[P::Land] * 0.5);
        smooth(h + 0.035, h - 0.035, wd(u, self.p[P::LandC]).abs()).max(if land >= 0.999 { 1.0 } else { 0.0 }) * smooth(0.0, 0.02, land)
    }
    /// Soft membership of u in z; the clear gap between a zone of half-width hw at c and z (negative:
    /// overlap); c pushed clear of z with a margin of 0.01.
    pub fn in_z(u: f64, z: &Zone) -> f64 { if z[1] > 0.0 { 1.0 - smooth(z[1] - 0.004, z[1] + 0.004, wd(u, z[0]).abs()) } else { 0.0 } }
    pub fn gap(c: f64, hw: f64, z: &Zone) -> f64 { if z[1] > 0.0 { wd(c, z[0]).abs() - hw - z[1] } else { 1.0 } }
    pub fn push(c: f64, hw: f64, z: &Zone) -> f64 { let g = Self::gap(c, hw, z) - 0.01; if g < 0.0 { c - g * sgn(wd(c, z[0]) + 1e-6) } else { c } }
    /// The front treeline's region.
    pub fn tr_n(&self, u: f64) -> f64 { self.rng.region(fract(u), self.p[P::TrC], self.p[P::TrW], 450.0) }
    /// Open land outside the city core, industry, town, fair and bridge: unwobbled, or the shader's `prOpen`.
    pub fn open_n(&self, u: f64) -> f64 { self.free(self.land_n(u), u) }
    pub fn open(&self, u: f64) -> f64 { self.free(self.land(u), u) }
    fn free(&self, x: f64, u: f64) -> f64 { [&self.core, &self.ind, &self.town, &self.fair, &self.bridge].iter().fold(x, |x, z| x * (1.0 - Self::in_z(u, z))) }
    /// Ridge elevations of the five ranges at u (the zones' footprint).
    pub fn ridges(&self, u: f64) -> [f64; 5] { self.ridge.heights(&self.rng, u, MPX) }
    /// The near ground rise at u (rad, the ground band's top); with the front treeline, what hides the foot of something far.
    pub fn rise(&self, u: f64) -> f64 { let u = fract(u); (0.0035 + 0.0025 * self.rng.fbm(u, 24.0, 500.0)) * self.land(u) * self.p[P::Size] }
    fn near_cover(&self, u: f64) -> f64 { self.rise(u) + 0.6 * self.p[P::TrH] * self.p[P::Size] * self.tr_n(u) }
    /// Gentle ground on the lower half of range k at u (the crest 180 m to each side within ~12 degrees),
    /// where a wind turbine may stand; h0 is range k's ridge at u, d its distance.
    pub fn turb_ok(&self, u: f64, k: usize, d: f64, h0: f64) -> bool {
        let du = 180.0 / (TAU * d);
        let (ha, hb) = (self.ridges(u + du)[k], self.ridges(u - du)[k]);
        (ha - h0).abs().max((h0 - hb).abs()) <= 0.21 * TAU * du && h0 <= 0.5 * self.p[P::MtH] * mt_scale(k) * self.p[P::Size]
    }

    fn place(&mut self) {
        let p = self.p.clone();
        let (ct_c, ct_w, d, has_c, sl) = (p[P::CtC], p[P::CtW], 28.0 * p[P::CtR] * 70.0, p[P::CtW] >= 0.001, p[P::Land] < 0.96 && p[P::Land] > 0.04);
        let hc = |x: f64| if has_c { x } else { 0.0 };
        let cw = hc((ct_w * 0.5 + 0.045).min(0.5));
        (self.core, self.city) = ([ct_c, hc((ct_w * 0.25).max(0.02)), 0.0, 0.0], [ct_c, cw, 0.5 * d, 1.55 * d]);
        self.crane = if has_c { [ct_c, ct_w * 0.5 + 0.02, 0.5 * d, 1.55 * d] } else { Z0 };
        // industry: beside the city on the side with more land, at the city's distance
        let hw_i = 0.014 + 0.026 * p[P::PrInd];
        let o_i = hc(cw + 0.01 + hw_i);
        let s_i = if self.land(fract(ct_c + o_i)) + 0.05 * self.rnd(1.0, 1.0, 1000.0) >= self.land(fract(ct_c - o_i)) + 0.025 { 1.0 } else { -1.0 };
        (self.ind, self.ind_x) = ([fract(ct_c + s_i * o_i), hw_i, 0.55 * d, 1.2 * d], [hc(s_i), 0.0, 0.0, 0.0]);
        // the real shorelines near the nominal ones (bisection on the land region); sd 1: the sea lies toward larger u
        let shore_at = |s: f64, sd: f64| {
            let h = p[P::Land] * 0.5;
            let (mut lo, mut hi) = (s - sd * h.min(0.11), s + sd * (0.5 - h).min(0.11));
            for _ in 0..10 { let m = 0.5 * (lo + hi); if self.land(fract(m)) > 0.5 { lo = m } else { hi = m } }
            fract(0.5 * (lo + hi))
        };
        let (s_r, s_l) = if sl { (shore_at(fract(p[P::LandC] + p[P::Land] * 0.5), 1.0), shore_at(fract(p[P::LandC] - p[P::Land] * 0.5), -1.0)) } else { (0.0, 0.0) };
        self.shore = [s_r, s_l, if sl { 1.0 } else { 0.0 }, 0.0];
        // town: on the shore farther from the city and the industry (waterfront), else the best land away from them
        let hw_t = 0.02 + 0.045 * p[P::PrTown];
        let gap2 = |z: &Self, c: f64, hw: f64| Self::gap(c, hw, &z.city).min(Self::gap(c, hw, &z.ind));
        let (mut c_t, sh_t, mut sd_t) = if !sl {
            (argmax(12, |j| { let c = (j + self.rnd(j, 3.0, 1000.0)) / 12.0; (0.3 * self.land_n(c) + gap2(self, c, hw_t).min(0.25), c) }), 0.0, 0.0)
        } else if gap2(self, s_r, hw_t) + 0.03 * self.rnd(2.0, 1.0, 1000.0) >= gap2(self, s_l, hw_t) + 0.015 { (s_r, s_r, 1.0) } else { (s_l, s_l, -1.0) };
        c_t = fract(Self::push(Self::push(Self::push(c_t, hw_t, &self.city), hw_t, &self.ind), hw_t, &self.city));
        if wd(c_t, sh_t).abs() > 0.8 * hw_t { sd_t = 0.0; } // pushed away from the shore: an inland town
        (self.town, self.town_x, self.church) = ([c_t, hw_t, 1200.0, 2600.0], [sh_t, sd_t, 0.0, 0.0], [c_t, hw_t * 0.85, 1200.0, 2600.0]);
        // bridge: from the other shore out over the water (or beyond the town's waterfront when that shore is
        // taken), else across a valley on the freest land; still no room: shorter
        let mut hw_b = 0.02 + 0.025 * p[P::PrBridge];
        let gap3 = |z: &Self, c: f64, hw: f64| gap2(z, c, hw).min(Self::gap(c, hw, &z.town));
        let (mut c_b, mut sh_b, mut sd_b, mut g_b) = (0.0, 0.0, 0.0, -9.0);
        if sl {
            let (mut s_o, mut sd_o) = if sd_t > 0.0 { (s_l, -1.0) } else { (s_r, 1.0) };
            if sd_t == 0.0 { s_o = if wd(s_r, c_t).abs() > wd(s_l, c_t).abs() { s_r } else { s_l }; sd_o = if s_o == s_r { 1.0 } else { -1.0 }; }
            let (c1, c2) = (s_o + sd_o * hw_b * 0.5, sh_t + sd_t * (hw_t + hw_b + 0.006));
            let (g1, g2) = (gap3(self, c1, hw_b), if sd_t == 0.0 { -9.0 } else { gap3(self, c2, hw_b) });
            (c_b, sh_b, sd_b, g_b) = if g1 >= g2.min(0.0) { (c1, s_o, sd_o, g1) } else { (c2, sh_t, sd_t, g2) };
        }
        if g_b < 0.0 {
            c_b = argmax(12, |j| { let c = (j + self.rnd(j, 4.0, 1000.0)) / 12.0; (0.3 * self.land_n(c) + gap3(self, c, hw_b).min(0.25), c) });
            (g_b, sh_b, sd_b) = (gap3(self, c_b, hw_b), 0.0, 0.0);
            if g_b < 0.0 { hw_b = (hw_b + g_b).max(0.008); }
        }
        c_b = fract(c_b);
        self.bridge = [c_b, hw_b, 2200.0, 4500.0];
        self.bridge_x = if sl && self.land(c_b) < 0.5 { [1.0, sh_b, sd_b, 0.0] } else { Z0 };
        // fairground: on a pier beyond the town's waterfront when that is free, else at the town's end away from the water / the city
        let mut hw_f = 0.012 + 0.012 * p[P::PrFair];
        let o_f = hw_t + hw_f + 0.006;
        let gap_bci = |z: &Self, c: f64, hw: f64| Self::gap(c, hw, &z.bridge).min(Self::gap(c, hw, &z.city)).min(Self::gap(c, hw, &z.ind));
        let pier = sd_t != 0.0 && gap_bci(self, c_t + sd_t * o_f, hw_f) >= 0.0;
        let fs = if pier { sd_t } else if sd_t != 0.0 { -sd_t } else if has_c { sgn(wd(c_t, ct_c) + 1e-6) } else { 1.0 };
        let c_f = fract(Self::push(Self::push(Self::push(c_t + fs * o_f, hw_f, &self.bridge), hw_f, &self.city), hw_f, &self.ind));
        hw_f = (hw_f + gap_bci(self, c_f, hw_f).min(Self::gap(c_f, hw_f, &self.town)).min(0.0)).max(0.006);
        (self.fair, self.fair_x) = ([c_f, hw_f, 1100.0, 1900.0], [if pier { 1.0 } else { 0.0 }, fs, 0.0, 0.0]);
        // street-lamp rows: roads toward / along the town and along the city's near edge, from 400 m out
        self.lamp_t = [c_t, hw_t + 0.012, 400.0, 2600.0];
        self.lamp_c = if has_c { [ct_c, ct_w * 0.5 + 0.02, 400.0, 450f64.max(0.5 * d)] } else { Z0 };
        // power lines: open land outside the front treeline (which would hide a line 1-2.4 km out), never
        // the sea or a zone, apart from each other
        let near = |f: &dyn Fn(f64) -> f64, c: f64| f(c) + 0.5 * (f(c - 0.04) + f(c + 0.04));
        let pyl_site = |i: f64, pa: f64, pb: f64| argmax(16, |j| {
            let c = fract((j + 0.5 + 0.8 * (self.rnd(j, i, 1010.0) - 0.5)) / 16.0);
            let o_s = near(&|u| self.open_n(u), c) * 0.5;
            let mut s = o_s * (1.0 - 0.6 * Self::in_z(c, &self.city)) + 0.45 * self.rnd(j, i + 3.0, 1010.0) - 0.9 * near(&|u| self.tr_n(u), c) * 0.5
                - if o_s < 0.3 { 1.5 } else { 0.0 };
            for q in [pa, pb] { if q >= 0.0 { s -= 0.8 * (1.0 - wd(c, q).abs() / 0.12).max(0.0); } }
            (s, c)
        });
        let p0 = pyl_site(0.0, -1.0, -1.0);
        let p1 = pyl_site(1.0, p0, -1.0);
        let p2 = pyl_site(2.0, p0, p1);
        self.pyl = std::array::from_fn(|i| [[p0, p1, p2][i], 1100.0 + 1300.0 * self.rnd(i as f64, 5.0, 1000.0), 9000.0, 1.0]);
        // wind farms: on a crest where range k is the skyline (neither hidden by a nearer range nor lost against
        // a taller farther one), on open flat land 4.5 km out or offshore 8 km out
        let (hw_w, mt) = (0.015 + 0.03 * p[P::PrTurb], p[P::MtW] >= 0.04);
        // a farm's top (hub + half rotor) over its distance
        let top = |f: f64, dist: f64| { let h0 = 85.0 + 35.0 * self.rnd(f, 1.0, 1200.0); (h0 + 0.5 * (h0 * (0.47 + 0.08 * self.rnd(f, 2.0, 1200.0)))) * p[P::Size] / dist };
        // the visible share of a farm on range k around c (the part of its turbines above every other ridge, the
        // near ground rise and the front treeline, where a turbine may stand), and range k's mean ridge there
        let farm_vis = |f: f64, c: f64, k: usize| {
            let (dk, mut v, mut rh) = (mt_dist(k as f64), 0.0, 0.0);
            let t = top(f, dk);
            for i in 0..9 {
                let us = c + (i as f64 / 4.0 - 1.0) * hw_w * 0.85;
                let r = self.ridges(us);
                let o = (0..5).filter(|&kk| kk != k).fold(self.near_cover(us), |o, kk| o.max(r[kk]));
                if self.turb_ok(us, k, dk, r[k]) { v += clamp((r[k] + t - o.max(r[k])) / t, 0.0, 1.0); }
                rh += r[k];
            }
            (v / 9.0, rh / 9.0)
        };
        // a flat or offshore farm dist m out: the share of its turbines clear of any mountain on that bearing
        let flat_vis = |f: f64, c: f64, dist: f64| {
            let (t, mut v) = (top(f, dist), 0.0);
            for i in 0..9 {
                let us = c + (i as f64 / 4.0 - 1.0) * hw_w * 0.85;
                v += clamp((t - self.near_cover(us).max(self.ridges(us).into_iter().fold(f64::MIN, f64::max))) / t, 0.0, 1.0);
            }
            v / 9.0
        };
        let city_z = [ct_c, hc((ct_w * 0.3).max(0.02)), 0.0, 0.0];
        let turb_site = |f: f64, other: f64| {
            // the second farm keeps clear of the first
            let sep = |c: f64| if other >= 0.0 { 30.0 * (wd(c, other).abs() - 2.0 * hw_w - 0.01).min(0.0) } else { 0.0 };
            let (mut best, mut site) = (-9.0, [0.0, 4.0, 1.0]);
            let mut take = |s: f64, t: [f64; 3]| if s > best { (best, site) = (s, t) };
            for j in 0..8 {
                let jf = j as f64;
                let c = if mt { fract(p[P::MtC] + (2.0 * self.rnd(jf, f + 7.0, 1000.0) - 1.0) * (p[P::MtW] * 0.6 - hw_w).max(0.0)) } else { fract((jf + self.rnd(jf, f + 7.0, 1000.0)) / 8.0) };
                let s0 = 0.5 * self.rnd(jf, f + 9.0, 1000.0) + (if mt { 0.0 } else { self.open_n(c) }) - Self::in_z(c, &city_z) + sep(c);
                if !mt { take(s0, [c, 4.0, 2.0]); continue; }
                for k in 2..5 { // on a real crest; nearer ranges a little preferred
                    let (vis, rh) = farm_vis(f, c, k);
                    take(s0 + 1.5 * vis + 0.3 * smooth(0.0, 0.02, rh) + 0.08 * (k - 2) as f64 + 0.3 * self.rnd(jf * 3.0 + k as f64, f + 11.0, 1000.0), [c, k as f64, 1.0]);
                }
            }
            // open flat land or open sea anywhere on the circle, competing with the crests (a seeded preference mixes the kinds)
            for j in 0..12 {
                let jf = j as f64;
                let c = fract((jf + self.rnd(jf, f + 21.0, 1000.0)) / 12.0);
                let (mut open, mut wet) = (1.0f64, 1.0f64);
                for i in 0..5 {
                    let us = fract(c + (i as f64 / 2.0 - 1.0) * hw_w);
                    (open, wet) = (open.min(self.open_n(us)), wet.min((1.0 - self.land_n(us)) * (1.0 - Self::in_z(us, &self.bridge)) * (1.0 - Self::in_z(us, &self.fair))));
                }
                let pref = 0.45 * self.rnd(jf, f + 23.0, 1000.0) + 0.2 * self.rnd(f, 29.0, 1000.0);
                if open > 0.6 { take(0.5 * self.rnd(jf, f + 25.0, 1000.0) + 1.5 * flat_vis(f, c, 4500.0) * open + pref - Self::in_z(c, &city_z) + sep(c), [c, 4.0, 2.0]); }
                if wet > 0.8 { take(0.5 * self.rnd(jf, f + 27.0, 1000.0) + 1.5 * flat_vis(f, c, 8000.0) * wet + pref + sep(c), [c, 4.0, 3.0]); }
            }
            site
        };
        let f0 = turb_site(0.0, -1.0);
        let f1 = turb_site(1.0, f0[0]);
        self.turb = [[f0[0], hw_w, f0[1], f0[2]], [f1[0], hw_w, f1[1], f1[2]]];
        // radio / TV masts: on the skyline ridge (range 3 or 4 where it is the front range) or on open treeless land
        // as many over Size (one to four)
        let n_m = if p[P::PrMast] > 0.0 { ((1.0 + (p[P::PrMast] * 3.99).floor()) / p[P::Size]).round().clamp(1.0, 4.0) as usize } else { 0 };
        for i in 0..n_m {
            let fi = i as f64;
            let (c, k) = argmax(16, |j| {
                let c = fract((j + self.rnd(j, fi + 40.0, 1000.0)) / 16.0);
                let (r, o) = (self.ridges(c), self.near_cover(c));
                let mut s = 0.4 * self.rnd(j, fi + 41.0, 1000.0) - 1.5 * Self::in_z(c, &self.core);
                for q in 0..i { s += 6.0 * (wd(c, self.mast_u[q]).abs() - 0.05).min(0.0); }
                let k = if r[4] > o + 0.002 && r[4] >= r[3] - 0.001 { 4.0 } else if r[3] > o + 0.002 && r[3] > r[4] { 3.0 } else { -1.0 };
                if k >= 0.0 { s += 0.8; } else {
                    if r.into_iter().fold(f64::MIN, f64::max) > o + 0.002 { s -= 1.0; } // a farther range behind it: dark on dark
                    s += self.open_n(c) - self.tr_n(c);
                }
                (s, (c, k))
            });
            (self.mast_u[i], self.mast_k[i]) = (c, k);
            (self.mast_d[i], self.mast_h[i]) = if k < 0.0 { (2500.0 + 2000.0 * self.rnd(fi, 42.0, 1000.0), 120.0 + 180.0 * self.rnd(fi, 43.0, 1000.0)) }
                else { (mt_dist(k), 50.0 + 90.0 * self.rnd(fi, 43.0, 1000.0)) };
        }
    }

    /// Every zone by the page's uniform name (for the checks against the page).
    #[allow(dead_code)] // tests/checks.rs
    pub fn named(&self) -> Vec<(&'static str, Zone)> {
        vec![("uZCity", self.city), ("uZCrane", self.crane), ("uZInd", self.ind), ("uZIndX", self.ind_x), ("uZShore", self.shore), ("uZTown", self.town),
            ("uZTownX", self.town_x), ("uZChurch", self.church), ("uZBridge", self.bridge), ("uZBridgeX", self.bridge_x), ("uZFair", self.fair),
            ("uZFairX", self.fair_x), ("uZLampT", self.lamp_t), ("uZLampC", self.lamp_c), ("uZPyl0", self.pyl[0]), ("uZPyl1", self.pyl[1]),
            ("uZPyl2", self.pyl[2]), ("uZTurb0", self.turb[0]), ("uZTurb1", self.turb[1]), ("uZMastU", self.mast_u), ("uZMastK", self.mast_k),
            ("uZMastD", self.mast_d), ("uZMastH", self.mast_h)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At Size 1 the summit table is the base's (a summit a cell, the page's draws); at Size 2 each range has half its cells
    /// (rounded), keeps its highest summit and stands twice as high; at Size 0.5 twice the cells, every base summit kept
    /// and one drawn afresh between each two.
    #[test]
    fn the_summits_follow_size() {
        let p = Params::default();
        let rng = Rng::new(p.seed);
        let at = |k: f64| { let mut q = p.clone(); q.set(P::Size as usize, k); Ridges::new(&q, &rng) };
        let (one, two, half) = (at(1.0), at(2.0), at(0.5));
        let top = |r: &Ridges, k: usize| r.tab[k].iter().map(|t| t[0]).fold(0.0, f64::max);
        for k in 0..5 {
            let r1 = rng.rnd(3.0, 2.0, 20.0 + k as f64 * 10.0);
            assert_eq!((one.tab[k].len(), one.tab[k][3][0]), (mt_cells(k) as usize, r1 * r1.sqrt()), "range {k} at Size 1");
            assert_eq!((two.tab[k].len(), half.tab[k].len()), ((mt_cells(k) / 2.0).round() as usize, 2 * mt_cells(k) as usize), "range {k}");
            assert_eq!(top(&two, k), top(&one, k), "range {k} keeps its highest summit");
            assert!(top(&half, k) >= top(&one, k), "range {k}: Size 0.5 keeps it among more");
            assert!(one.tab[k].iter().all(|r| half.tab[k].contains(r)), "range {k}: Size 0.5 keeps every summit");
            assert_eq!((two.hk(k), half.hk(k)), (2.0 * one.hk(k), 0.5 * one.hk(k)));
        }
    }
}
