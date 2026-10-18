//! Where everything stands on the circle: the five mountain ranges' ridges (summits and roughness)
//! and the placement zones of the city and every prop family (the page's `propZones`).
//!
//! A zone is `[centre azimuth u, half-width in u, nearest distance m, farthest distance m]`;
//! half-width 0 means absent. Everything is seed-deterministic, periodic in u and ported from the
//! page step by step, so zones land where the page puts them.
use crate::params::{P, Params};
use crate::rng::*;

pub type Zone = [f64; 4];

/// Relative height of range k (0 farthest .. 4 nearest).
pub fn mt_scale(k: usize) -> f64 {
    [0.85, 1.15, 1.25, 0.85, 0.50][k]
}
/// How alpine range k may get at full ruggedness.
pub fn mt_alp(k: usize) -> f64 {
    [1.15, 1.25, 1.1, 0.7, 0.25][k]
}
/// Summit cells of range k around the circle.
pub fn mt_cells(k: usize) -> f64 {
    [30.0, 24.0, 20.0, 24.0, 34.0][k]
}
/// Nominal distance (m) of range k: 4 (nearest) 5 km, 3 7.8 km, 2 12.2 km, 1 19 km, 0 30 km.
pub fn mt_dist(k: f64) -> f64 {
    5000.0 * 6f64.powf((4.0 - k) * 0.25)
}
/// Pixel footprint of the zones' ridge model: roughness octaves down to a 16k export's pixel.
pub const MPX: f64 = TAU / 16384.0;

/// The ridge model of the five ranges for one parameter set: per range its peak height, its
/// ruggedness and its summit table (per summit cell: height bound r1^1.5, slope, lean, offset).
pub struct Ridges {
    hk: [f64; 5],
    rk: [f64; 5],
    tab: [Vec<[f64; 4]>; 5],
    menv_s: f64,
    w_a: f64,
    mt_c: f64,
    mt_w: f64,
}

impl Ridges {
    pub fn new(p: &Params, rng: &Rng) -> Self {
        let tab = std::array::from_fn(|k| {
            let ly = 20.0 + k as f64 * 10.0;
            (0..mt_cells(k) as usize)
                .map(|c| {
                    let (r1, r2) = (rng.rnd(c as f64, 2.0, ly), rng.rnd(c as f64, 3.0, ly));
                    [r1 * r1.sqrt(), 0.28 + 0.32 * fract(r1 * 37.7), 0.65 + 0.7 * fract(r2 * 53.1), 0.8 * r2]
                })
                .collect()
        });
        Self {
            hk: std::array::from_fn(|k| p[P::MtH] * mt_scale(k)),
            rk: std::array::from_fn(|k| clamp(p[P::MtRug] * mt_alp(k), 0.0, 1.0)),
            tab,
            menv_s: smooth(0.0, 0.04, p[P::MtW]),
            w_a: clamp(p[P::MtW] * 1.3, 0.0, 1.0),
            mt_c: p[P::MtC],
            mt_w: p[P::MtW],
        }
    }

    /// Peak height (rad) of range k.
    pub fn hk(&self, k: usize) -> f64 {
        self.hk[k]
    }

    /// The massif envelopes at u: the wide one and the core one.
    pub fn envelopes(&self, rng: &Rng, u: f64) -> (f64, f64) {
        (rng.region(u, self.mt_c, self.w_a, 20.0), rng.region(u, self.mt_c + 0.02, self.mt_w, 30.0))
    }

    /// Range k's share of the massif, from the envelopes: far ranges spread wide, near ones keep to the core.
    pub fn menv(&self, k: usize, env: (f64, f64)) -> f64 {
        let t = (4 - k) as f64 / 4.0;
        mix(env.0, env.1, if t < 0.3 { 0.15 } else if t < 0.6 { 0.5 } else { 1.0 }) * self.menv_s
    }

    /// The highest of the summits of cells ci-2 ..= ci+2 at x (own cell first; a cell whose height
    /// bound cannot beat the best is skipped).
    fn summits(&self, k: usize, x: f64, q: f64, sh: f64) -> f64 {
        let cells = mt_cells(k);
        let t = &self.tab[k];
        let ci = x.floor();
        let (hgm, rr) = (mix(0.4, 1.0, sh), mix(0.05, 0.004, sh));
        let srr = rr.sqrt();
        let mut best: f64 = 0.0;
        for j in [0.0, 1.0, -1.0, 2.0, -2.0] {
            let c = ci + j;
            let row = t[(c - cells * (c / cells).floor()) as usize];
            let hg = row[0] * hgm;
            if hg <= best {
                continue;
            }
            let hw = clamp(hg * q / row[1] * mix(2.4, 1.4, sh), 0.2, 1.5);
            let (asy, dx) = (row[2], x - (c + 0.1 + row[3]));
            let mut d = if dx < 0.0 { -dx / (hw * asy) } else { dx / (hw * (2.0 - asy)) };
            d = (d * d + rr).sqrt() - srr;
            let f = (1.0 - d).max(0.0);
            best = best.max(hg * mix(f * f * (3.0 - 2.0 * f), mix(f, f * f, 0.8 * sh), sh));
        }
        best
    }

    /// The page's `mtnH`: range k's height factor at u (multiply by `hk * menv`) with roughness
    /// octaves down to the pixel footprint `px`. Also returns the second octave's value, which the
    /// tree line of that range follows.
    pub fn mtn_h(&self, rng: &Rng, u: f64, k: usize, menv: f64, px: f64) -> (f64, f64) {
        let (ly, b, rk, h) = (20.0 + k as f64 * 10.0, 2.0 + k as f64, self.rk[k], self.hk[k]);
        let n = 0.667 * rng.pn(u * b, b, ly + 8.0) + 0.333 * rng.pn(u * b * 2.0, b * 2.0, ly + 11.0);
        let alp = smooth(-0.25, 0.25, (n - 0.5) * 1.9 + rk * 1.75 - 0.7 + (menv - 0.5) * 0.9 * (0.4 + rk));
        let swell = smooth(0.3, 0.7, n);
        let cells = mt_cells(k);
        let q = h * cells / TAU;
        let s1 = self.summits(k, u * cells, q, alp);
        let (sm, g, f0) = (mix(0.9, 0.25, alp * rk), mix(0.42, 0.5, alp), (cells * (2.0 - 0.25 * k as f64) + 0.5).floor());
        let (mut f, mut a, mut d, mut fo, mut nz) = (f0, 1.0, 0.0, 0.7 * alp * rk, 0.5);
        for j in 0..12 {
            let w = clamp(TAU / (f * px) * 0.5 - 1.0, 0.0, 1.0);
            if w <= 0.0 {
                break;
            }
            let x = u * f;
            let i = x.floor();
            let mut i1 = i + 1.0;
            if i1 >= f {
                i1 -= f;
            }
            let mut fr = x - i;
            fr = mix(fr, fr * fr * (3.0 - 2.0 * fr), sm.max(1.0 - 0.22 * j as f64));
            let ra = rng.rnd(i, 7.0 + j as f64, ly);
            let mut v = mix(ra, rng.rnd(i1, 7.0 + j as f64, ly), fr);
            v = mix(v, 1.0 - (2.0 * v - 1.0).abs(), fo);
            if j == 1 {
                nz = v;
            }
            d += a * w * (v - 0.5);
            f *= 2.0;
            a *= g;
            fo *= 0.6;
        }
        let s = mix(0.1, 0.6 * mix(0.6, 1.0, rk), alp) * (0.35 + 0.65 * s1);
        ((0.04 + 0.1 * swell + 0.86 * s1 * (0.78 + 0.22 * swell) + s * cells / (f0 * (0.45 * q.max(0.45)).sqrt()) * d).max(0.015), nz)
    }

    /// Ridge elevation (rad) of the five ranges at u, roughness down to `px`.
    pub fn heights(&self, rng: &Rng, u: f64, px: f64) -> [f64; 5] {
        let u = fract(u);
        let env = self.envelopes(rng, u);
        std::array::from_fn(|k| {
            let menv = self.menv(k, env);
            self.hk[k] * menv * self.mtn_h(rng, u, k, menv, px).0
        })
    }
}

/// The placement zones and the helpers the prop generators share (the page's `propZones.lib`).
pub struct Zones {
    pub p: Params,
    pub rng: Rng,
    pub ridge: Ridges,
    /// The city core.
    pub core: Zone,
    pub city: Zone,
    pub crane: Zone,
    pub ind: Zone,
    /// x: side of the city the industry is on (+1 toward larger u, -1 smaller, 0 no city).
    pub ind_x: Zone,
    /// The two real shorelines: x where the sea begins toward larger u, y toward smaller u; z 1 when
    /// there are both land and sea.
    pub shore: Zone,
    pub town: Zone,
    /// x: shore azimuth, y: sea side of the town (+1 toward larger u, -1 smaller, 0 inland).
    pub town_x: Zone,
    pub church: Zone,
    pub bridge: Zone,
    /// x: 1 over water, 0 across a valley; y: shore azimuth; z: sea side.
    pub bridge_x: Zone,
    pub fair: Zone,
    /// x: 1 on a pier, 0 at the town edge; y: direction from the town.
    pub fair_x: Zone,
    pub lamp_t: Zone,
    pub lamp_c: Zone,
    /// Power lines: (bearing of the line's nearest point, its distance m, farthest pylon m, 1).
    pub pyl: [Zone; 3],
    /// Wind farms: (centre, half-width, range k they stand on, 1 crest / 2 open flat land / 3 offshore).
    pub turb: [Zone; 2],
    /// Up to four masts: azimuth (-1 none), range k (-1 open land), distance m, height m.
    pub mast_u: Zone,
    pub mast_k: Zone,
    pub mast_d: Zone,
    pub mast_h: Zone,
}

impl Zones {
    pub fn new(p: &Params) -> Self {
        let rng = Rng::new(p.seed);
        let ridge = Ridges::new(p, &rng);
        let mut z = Zones {
            p: p.clone(),
            rng,
            ridge,
            core: [0.0; 4],
            city: [0.0; 4],
            crane: [0.0; 4],
            ind: [0.0; 4],
            ind_x: [0.0; 4],
            shore: [0.0; 4],
            town: [0.0; 4],
            town_x: [0.0; 4],
            church: [0.0; 4],
            bridge: [0.0; 4],
            bridge_x: [0.0; 4],
            fair: [0.0; 4],
            fair_x: [0.0; 4],
            lamp_t: [0.0; 4],
            lamp_c: [0.0; 4],
            pyl: [[0.0; 4]; 3],
            turb: [[0.0; 4]; 2],
            mast_u: [-1.0; 4],
            mast_k: [-1.0; 4],
            mast_d: [3000.0; 4],
            mast_h: [150.0; 4],
        };
        z.place();
        z
    }

    pub fn rnd(&self, id: f64, st: f64, ly: f64) -> f64 {
        self.rng.rnd(id, st, ly)
    }
    /// The land envelope at u (1 land, 0 sea).
    pub fn land(&self, u: f64) -> f64 {
        self.rng.region(u, self.p[P::LandC], self.p[P::Land], 600.0)
    }
    /// The land envelope without its wobble.
    pub fn land_n(&self, u: f64) -> f64 {
        let (land, h) = (self.p[P::Land], self.p[P::Land] * 0.5);
        let on = if land >= 0.999 { 1.0 } else { 0.0 };
        smooth(h + 0.035, h - 0.035, wd(u, self.p[P::LandC]).abs()).max(on) * smooth(0.0, 0.02, land)
    }
    /// Soft membership of azimuth u in zone z.
    pub fn in_z(u: f64, z: &Zone) -> f64 {
        if z[1] > 0.0 {
            1.0 - smooth(z[1] - 0.004, z[1] + 0.004, wd(u, z[0]).abs())
        } else {
            0.0
        }
    }
    /// Clear gap between a zone of half-width hw at c and zone z (negative: overlap).
    pub fn gap(c: f64, hw: f64, z: &Zone) -> f64 {
        if z[1] > 0.0 {
            wd(c, z[0]).abs() - hw - z[1]
        } else {
            1.0
        }
    }
    /// c pushed clear of zone z (with a margin of 0.01).
    pub fn push(c: f64, hw: f64, z: &Zone) -> f64 {
        let g = Self::gap(c, hw, z) - 0.01;
        if g < 0.0 {
            c - g * sgn(wd(c, z[0]) + 1e-6)
        } else {
            c
        }
    }
    /// The front treeline's region.
    pub fn tr_n(&self, u: f64) -> f64 {
        self.rng.region(fract(u), self.p[P::TrC], self.p[P::TrW], 450.0)
    }
    /// Open land: no city core, industry, town, fair or bridge (unwobbled land).
    pub fn open_n(&self, u: f64) -> f64 {
        self.free(self.land_n(u), u)
    }
    /// Open land (the shader's `prOpen`), for the prop generators.
    #[allow(dead_code)]
    pub fn open(&self, u: f64) -> f64 {
        self.free(self.land(u), u)
    }
    /// `x` times the share of u outside every placed zone, multiplied in the page's order.
    fn free(&self, mut x: f64, u: f64) -> f64 {
        for z in [&self.core, &self.ind, &self.town, &self.fair, &self.bridge] {
            x *= 1.0 - Self::in_z(u, z);
        }
        x
    }
    /// Ridge elevations of the five ranges at u (the zones' footprint).
    pub fn ridges(&self, u: f64) -> [f64; 5] {
        self.ridge.heights(&self.rng, u, MPX)
    }
    /// Elevation (rad) of the near ground rise at u (the ground band's top).
    pub fn rise(&self, u: f64) -> f64 {
        let u = fract(u);
        (0.0035 + 0.0025 * self.rng.fbm(u, 24.0, 500.0)) * self.land(u)
    }
    /// What hides the foot of something far at u: the near ground rise and the front treeline.
    fn near_cover(&self, us: f64) -> f64 {
        self.rise(us) + 0.6 * self.p[P::TrH] * self.tr_n(us)
    }
    /// Gentle ground on the lower half of range k at u (crest 180 m to each side within ~12 degrees):
    /// where a wind turbine may stand; h0 is range k's ridge at u, d its distance.
    pub fn turb_ok(&self, u: f64, k: usize, d: f64, h0: f64) -> bool {
        let du = 180.0 / (TAU * d);
        let (ha, hb) = (self.ridges(u + du)[k], self.ridges(u - du)[k]);
        (ha - h0).abs().max((h0 - hb).abs()) <= 0.21 * TAU * du && h0 <= 0.5 * self.p[P::MtH] * mt_scale(k)
    }

    fn place(&mut self) {
        let p = self.p.clone();
        let (ct_c, ct_w) = (p[P::CtC], p[P::CtW]);
        let d = 28.0 * p[P::CtR] * 70.0;
        let has_c = ct_w >= 0.001;
        let (sea, lnd) = (p[P::Land] < 0.96, p[P::Land] > 0.04);
        let hc = |x: f64| if has_c { x } else { 0.0 };
        self.core = [ct_c, hc(ct_w * 0.25).max(if has_c { 0.02 } else { 0.0 }), 0.0, 0.0];
        let cw = hc((ct_w * 0.5 + 0.045).min(0.5));
        self.city = [ct_c, cw, 0.5 * d, 1.55 * d];
        self.crane = if has_c { [ct_c, ct_w * 0.5 + 0.02, 0.5 * d, 1.55 * d] } else { [0.0; 4] };

        // industry: beside the city on the side with more land, at the city's distance
        let hw_i = 0.014 + 0.026 * p[P::PrInd];
        let o_i = hc(cw + 0.01 + hw_i);
        let s_i = if self.land(fract(ct_c + o_i)) + 0.05 * self.rnd(1.0, 1.0, 1000.0) >= self.land(fract(ct_c - o_i)) + 0.025 { 1.0 } else { -1.0 };
        self.ind = [fract(ct_c + s_i * o_i), hw_i, 0.55 * d, 1.2 * d];
        self.ind_x = [hc(s_i), 0.0, 0.0, 0.0];

        // the real shorelines near the nominal ones (bisection on the land region)
        let shore_at = |z: &Self, s: f64, sd: f64| {
            let h = p[P::Land] * 0.5;
            let (mut lo, mut hi) = (s - sd * h.min(0.11), s + sd * (0.5 - h).min(0.11));
            for _ in 0..10 {
                let m = 0.5 * (lo + hi);
                if z.land(fract(m)) > 0.5 {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            fract(0.5 * (lo + hi))
        };
        let (mut s_r, mut s_l) = (0.0, 0.0);
        if sea && lnd {
            s_r = shore_at(self, fract(p[P::LandC] + p[P::Land] * 0.5), 1.0);
            s_l = shore_at(self, fract(p[P::LandC] - p[P::Land] * 0.5), -1.0);
        }
        self.shore = [s_r, s_l, if sea && lnd { 1.0 } else { 0.0 }, 0.0];

        // town: on the shore farther from the city and the industry, else the best land away from them
        let hw_t = 0.02 + 0.045 * p[P::PrTown];
        let (mut c_t, mut sh_t, mut sd_t) = (0.0, 0.0, 0.0);
        if sea && lnd {
            let g_r = Self::gap(s_r, hw_t, &self.city).min(Self::gap(s_r, hw_t, &self.ind));
            let g_l = Self::gap(s_l, hw_t, &self.city).min(Self::gap(s_l, hw_t, &self.ind));
            let r = g_r + 0.03 * self.rnd(2.0, 1.0, 1000.0) >= g_l + 0.015;
            c_t = if r { s_r } else { s_l };
            sh_t = c_t;
            sd_t = if r { 1.0 } else { -1.0 };
        } else {
            let mut best = -9.0;
            for j in 0..12 {
                let c = (j as f64 + self.rnd(j as f64, 3.0, 1000.0)) / 12.0;
                let s = 0.3 * self.land_n(c) + Self::gap(c, hw_t, &self.city).min(Self::gap(c, hw_t, &self.ind)).min(0.25);
                if s > best {
                    best = s;
                    c_t = c;
                }
            }
        }
        c_t = fract(Self::push(Self::push(Self::push(c_t, hw_t, &self.city), hw_t, &self.ind), hw_t, &self.city));
        if wd(c_t, sh_t).abs() > 0.8 * hw_t {
            sd_t = 0.0;
        }
        self.town = [c_t, hw_t, 1200.0, 2600.0];
        self.town_x = [sh_t, sd_t, 0.0, 0.0];
        self.church = [c_t, hw_t * 0.85, 1200.0, 2600.0];

        // bridge: from the other shore out over the water (or beyond the town's waterfront), else
        // across a valley on the freest land; still no room: shorter
        let mut hw_b = 0.02 + 0.025 * p[P::PrBridge];
        let (mut c_b, mut sh_b, mut sd_b, mut g_b) = (0.0, 0.0, 0.0, -9.0);
        let gap3 = |z: &Self, c: f64, hw: f64| Self::gap(c, hw, &z.city).min(Self::gap(c, hw, &z.ind)).min(Self::gap(c, hw, &z.town));
        if sea && lnd {
            let (mut s_o, mut sd_o) = if sd_t > 0.0 { (s_l, -1.0) } else { (s_r, 1.0) };
            if sd_t == 0.0 {
                s_o = if wd(s_r, c_t).abs() > wd(s_l, c_t).abs() { s_r } else { s_l };
                sd_o = if s_o == s_r { 1.0 } else { -1.0 };
            }
            let c1 = s_o + sd_o * hw_b * 0.5;
            let c2 = sh_t + sd_t * (hw_t + hw_b + 0.006);
            let g1 = gap3(self, c1, hw_b);
            let g2 = if sd_t == 0.0 { -9.0 } else { gap3(self, c2, hw_b) };
            let o = g1 >= g2.min(0.0);
            c_b = if o { c1 } else { c2 };
            sh_b = if o { s_o } else { sh_t };
            sd_b = if o { sd_o } else { sd_t };
            g_b = if o { g1 } else { g2 };
        }
        if g_b < 0.0 {
            let mut best = -9.0;
            for j in 0..12 {
                let c = (j as f64 + self.rnd(j as f64, 4.0, 1000.0)) / 12.0;
                let g = gap3(self, c, hw_b);
                let s = 0.3 * self.land_n(c) + g.min(0.25);
                if s > best {
                    best = s;
                    c_b = c;
                    g_b = g;
                }
            }
            sh_b = 0.0;
            sd_b = 0.0;
        }
        if g_b < 0.0 {
            hw_b = (hw_b + g_b).max(0.008);
        }
        c_b = fract(c_b);
        let w_b = if sea && lnd && self.land(c_b) < 0.5 { 1.0 } else { 0.0 };
        self.bridge = [c_b, hw_b, 2200.0, 4500.0];
        self.bridge_x = [w_b, if w_b > 0.0 { sh_b } else { 0.0 }, if w_b > 0.0 { sd_b } else { 0.0 }, 0.0];

        // fairground: on a pier beyond the town's waterfront when free, else at the town's far end
        let mut hw_f = 0.012 + 0.012 * p[P::PrFair];
        let o_f = hw_t + hw_f + 0.006;
        let c_p = c_t + sd_t * o_f;
        let pier = sd_t != 0.0
            && Self::gap(c_p, hw_f, &self.bridge).min(Self::gap(c_p, hw_f, &self.city)).min(Self::gap(c_p, hw_f, &self.ind)) >= 0.0;
        let fs = if pier {
            sd_t
        } else if sd_t != 0.0 {
            -sd_t
        } else if has_c {
            sgn(wd(c_t, ct_c) + 1e-6)
        } else {
            1.0
        };
        let c_f = fract(Self::push(Self::push(Self::push(c_t + fs * o_f, hw_f, &self.bridge), hw_f, &self.city), hw_f, &self.ind));
        let g_f = Self::gap(c_f, hw_f, &self.bridge)
            .min(Self::gap(c_f, hw_f, &self.city))
            .min(Self::gap(c_f, hw_f, &self.ind).min(Self::gap(c_f, hw_f, &self.town)));
        hw_f = (hw_f + g_f.min(0.0)).max(0.006);
        self.fair = [c_f, hw_f, 1100.0, 1900.0];
        self.fair_x = [if pier { 1.0 } else { 0.0 }, fs, 0.0, 0.0];

        // street-lamp rows: toward / along the town and along the city's near edge
        self.lamp_t = [c_t, hw_t + 0.012, 400.0, 2600.0];
        self.lamp_c = if has_c { [ct_c, ct_w * 0.5 + 0.02, 400.0, 450f64.max(0.5 * d)] } else { [0.0; 4] };

        // power lines: open land outside the front treeline, apart from each other
        let pyl_site = |z: &Self, i: f64, pa: f64, pb: f64| {
            let (mut best, mut c0) = (-9.0, 0.0);
            for j in 0..16 {
                let jf = j as f64;
                let c = fract((jf + 0.5 + 0.8 * (z.rnd(jf, i, 1010.0) - 0.5)) / 16.0);
                let o_s = (z.open_n(c) + 0.5 * (z.open_n(c - 0.04) + z.open_n(c + 0.04))) * 0.5;
                let mut s = o_s * (1.0 - 0.6 * Self::in_z(c, &z.city)) + 0.45 * z.rnd(jf, i + 3.0, 1010.0)
                    - 0.9 * (z.tr_n(c) + 0.5 * (z.tr_n(c - 0.04) + z.tr_n(c + 0.04))) * 0.5
                    - if o_s < 0.3 { 1.5 } else { 0.0 };
                for q in [pa, pb] {
                    if q >= 0.0 {
                        s -= 0.8 * (1.0 - wd(c, q).abs() / 0.12).max(0.0);
                    }
                }
                if s > best {
                    best = s;
                    c0 = c;
                }
            }
            c0
        };
        let p0 = pyl_site(self, 0.0, -1.0, -1.0);
        let p1 = pyl_site(self, 1.0, p0, -1.0);
        let p2 = pyl_site(self, 2.0, p0, p1);
        for (i, c) in [p0, p1, p2].into_iter().enumerate() {
            self.pyl[i] = [c, 1100.0 + 1300.0 * self.rnd(i as f64, 5.0, 1000.0), 9000.0, 1.0];
        }

        // wind farms: on a crest where range k is the skyline, on open flat land, or offshore
        let hw_w = 0.015 + 0.03 * p[P::PrTurb];
        let mt = p[P::MtW] >= 0.04;
        let farm_size = |z: &Self, f: f64, dist: f64| {
            let h0 = 85.0 + 35.0 * z.rnd(f, 1.0, 1200.0);
            let r0 = h0 * (0.47 + 0.08 * z.rnd(f, 2.0, 1200.0));
            (h0 + 0.5 * r0) / dist
        };
        let farm_vis = |z: &Self, f: f64, c: f64, k: usize| {
            let dk = mt_dist(k as f64);
            let top = farm_size(z, f, dk);
            let (mut v, mut rh) = (0.0, 0.0);
            for i in 0..9 {
                let us = c + (i as f64 / 4.0 - 1.0) * hw_w * 0.85;
                let r = z.ridges(us);
                let mut o = z.near_cover(us);
                for (kk, rk) in r.iter().enumerate() {
                    if kk != k {
                        o = o.max(*rk);
                    }
                }
                if z.turb_ok(us, k, dk, r[k]) {
                    v += clamp((r[k] + top - o.max(r[k])) / top, 0.0, 1.0);
                }
                rh += r[k];
            }
            (v / 9.0, rh / 9.0)
        };
        let flat_vis = |z: &Self, f: f64, c: f64, dist: f64| {
            let top = farm_size(z, f, dist);
            let mut v = 0.0;
            for i in 0..9 {
                let us = c + (i as f64 / 4.0 - 1.0) * hw_w * 0.85;
                let o = z.near_cover(us).max(z.ridges(us).into_iter().fold(f64::MIN, f64::max));
                v += clamp((top - o) / top, 0.0, 1.0);
            }
            v / 9.0
        };
        let turb_site = |z: &Self, f: f64, other: f64| {
            let (mut best, mut c0, mut k0, mut m0) = (-9.0, 0.0, 4.0, 1.0);
            let sep = |c: f64| if other >= 0.0 { 30.0 * (wd(c, other).abs() - 2.0 * hw_w - 0.01).min(0.0) } else { 0.0 };
            let city_z = [ct_c, if has_c { (ct_w * 0.3).max(0.02) } else { 0.0 }, 0.0, 0.0];
            let city = |c: f64| Self::in_z(c, &city_z);
            for j in 0..8 {
                let jf = j as f64;
                let c = if mt {
                    fract(p[P::MtC] + (2.0 * z.rnd(jf, f + 7.0, 1000.0) - 1.0) * (p[P::MtW] * 0.6 - hw_w).max(0.0))
                } else {
                    fract((jf + z.rnd(jf, f + 7.0, 1000.0)) / 8.0)
                };
                let s0 = 0.5 * z.rnd(jf, f + 9.0, 1000.0) + (if mt { 0.0 } else { z.open_n(c) }) - city(c) + sep(c);
                if !mt {
                    if s0 > best {
                        (best, c0, k0, m0) = (s0, c, 4.0, 2.0);
                    }
                    continue;
                }
                for k in 2..5 {
                    let (vis, rh) = farm_vis(z, f, c, k);
                    let s = s0 + 1.5 * vis + 0.3 * smooth(0.0, 0.02, rh) + 0.08 * (k - 2) as f64 + 0.3 * z.rnd(jf * 3.0 + k as f64, f + 11.0, 1000.0);
                    if s > best {
                        (best, c0, k0, m0) = (s, c, k as f64, 1.0);
                    }
                }
            }
            for j in 0..12 {
                let jf = j as f64;
                let c = fract((jf + z.rnd(jf, f + 21.0, 1000.0)) / 12.0);
                let (mut open, mut sea) = (1.0f64, 1.0f64);
                for i in 0..5 {
                    let us = fract(c + (i as f64 / 2.0 - 1.0) * hw_w);
                    open = open.min(z.open_n(us));
                    sea = sea.min((1.0 - z.land_n(us)) * (1.0 - Self::in_z(us, &z.bridge)) * (1.0 - Self::in_z(us, &z.fair)));
                }
                let pref = 0.45 * z.rnd(jf, f + 23.0, 1000.0) + 0.2 * z.rnd(f, 29.0, 1000.0);
                if open > 0.6 {
                    let s = 0.5 * z.rnd(jf, f + 25.0, 1000.0) + 1.5 * flat_vis(z, f, c, 4500.0) * open + pref - city(c) + sep(c);
                    if s > best {
                        (best, c0, k0, m0) = (s, c, 4.0, 2.0);
                    }
                }
                if sea > 0.8 {
                    let s = 0.5 * z.rnd(jf, f + 27.0, 1000.0) + 1.5 * flat_vis(z, f, c, 8000.0) * sea + pref + sep(c);
                    if s > best {
                        (best, c0, k0, m0) = (s, c, 4.0, 3.0);
                    }
                }
            }
            [c0, k0, m0]
        };
        let f0 = turb_site(self, 0.0, -1.0);
        let f1 = turb_site(self, 1.0, f0[0]);
        self.turb = [[f0[0], hw_w, f0[1], f0[2]], [f1[0], hw_w, f1[1], f1[2]]];

        // radio / TV masts: on the skyline ridge (range 3 or 4 where it is the front range) or on
        // open treeless land
        let n_m = if p[P::PrMast] > 0.0 { 1 + (p[P::PrMast] * 3.99).floor() as usize } else { 0 };
        for i in 0..n_m {
            let fi = i as f64;
            let mut best = -9.0;
            for j in 0..16 {
                let jf = j as f64;
                let c = fract((jf + self.rnd(jf, fi + 40.0, 1000.0)) / 16.0);
                let r = self.ridges(c);
                let o = self.near_cover(c);
                let mut s = 0.4 * self.rnd(jf, fi + 41.0, 1000.0) - 1.5 * Self::in_z(c, &self.core);
                for q in 0..i {
                    s += 6.0 * (wd(c, self.mast_u[q]).abs() - 0.05).min(0.0);
                }
                let k = if r[4] > o + 0.002 && r[4] >= r[3] - 0.001 {
                    4.0
                } else if r[3] > o + 0.002 && r[3] > r[4] {
                    3.0
                } else {
                    -1.0
                };
                if k < 0.0 {
                    if r.into_iter().fold(f64::MIN, f64::max) > o + 0.002 {
                        s -= 1.0;
                    }
                    s += self.open_n(c) - self.tr_n(c);
                } else {
                    s += 0.8;
                }
                if s > best {
                    best = s;
                    self.mast_u[i] = c;
                    self.mast_k[i] = k;
                }
            }
            self.mast_d[i] = if self.mast_k[i] < 0.0 { 2500.0 + 2000.0 * self.rnd(fi, 42.0, 1000.0) } else { mt_dist(self.mast_k[i]) };
            self.mast_h[i] = if self.mast_k[i] < 0.0 { 120.0 + 180.0 * self.rnd(fi, 43.0, 1000.0) } else { 50.0 + 90.0 * self.rnd(fi, 43.0, 1000.0) };
        }
    }

    /// Every zone by the page's uniform name (for checks against the page and for the shaders).
    #[allow(dead_code)] // the layout check in tests/checks.rs
    pub fn named(&self) -> Vec<(&'static str, Zone)> {
        vec![
            ("uZCity", self.city),
            ("uZCrane", self.crane),
            ("uZInd", self.ind),
            ("uZIndX", self.ind_x),
            ("uZShore", self.shore),
            ("uZTown", self.town),
            ("uZTownX", self.town_x),
            ("uZChurch", self.church),
            ("uZBridge", self.bridge),
            ("uZBridgeX", self.bridge_x),
            ("uZFair", self.fair),
            ("uZFairX", self.fair_x),
            ("uZLampT", self.lamp_t),
            ("uZLampC", self.lamp_c),
            ("uZPyl0", self.pyl[0]),
            ("uZPyl1", self.pyl[1]),
            ("uZPyl2", self.pyl[2]),
            ("uZTurb0", self.turb[0]),
            ("uZTurb1", self.turb[1]),
            ("uZMastU", self.mast_u),
            ("uZMastK", self.mast_k),
            ("uZMastD", self.mast_d),
            ("uZMastH", self.mast_h),
        ]
    }
}
