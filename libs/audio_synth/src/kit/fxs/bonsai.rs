//! Bonsai (type 28): a tape-flavoured chain of trapezoidal one-pole
//! filters and soft clippers: input gain, a distorting bass boost, a
//! tilted tape saturator with a dynamic de-esser, signal-dependent tape
//! hiss, an "age" (dullness) stage and output gain. EFFECTS.md 5.19.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{db_to_linear, SlotDef, SlotValue};

/// The constant named for 4000 Hz is computed at 3000 Hz; mirrored.
const QUIRK_ALPHA_4000_IS_3000: bool = true;
const _: () = assert!(QUIRK_ALPHA_4000_IS_3000);
/// In mono bass-boost mode the boost is added to the left channel only.
const QUIRK_MONO_BOOST_LEFT_ONLY: bool = true;
const _: () = assert!(QUIRK_MONO_BOOST_LEFT_ONLY);
/// The noise generators' seeds are kept as floats between blocks and
/// truncated back to int (losing low bits); mirrored for the same hiss.
const QUIRK_NOISE_SEED_AS_FLOAT: bool = true;

const BOOST_MODES: &[&str] = &["Stereo", "Mono"];
const BIAS_FILTERS: &[&str] = &["Tilt", "Pull Mids"];
const SAT_MODES: &[&str] = &["Gentle", "Normal", "Hard", "Harsh"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        db("Gain", -24.0, 24.0, 0.0),
        deform(extend(db("Bass Boost Amount", -24.0, 24.0, 0.0)), BOOST_MODES),
        percent("Bass Distort", 0.25),
        choice("Bias Filter", BIAS_FILTERS, 0.0),
        choice("Saturation Type", SAT_MODES, 0.0),
        percent("Saturation Amount", 0.25),
        percent("Noise Sensitivity", 0.25),
        db("Noise Gain", -24.0, 24.0, -12.0),
        percent("Dull", 0.0),
        db("Gain (out)", -24.0, 24.0, 0.0),
        percent("Mix", 1.0),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// The tan-approximating one-pole coefficient for `f` Hz.
pub(super) fn alpha(f: f32, sr: f32) -> f32 {
    let x = (f.max(0.001) * std::f32::consts::PI / sr).min(std::f32::consts::FRAC_PI_2);
    let t = ((0.0388452 - 0.0896638 * x) * x + 1.00005) * x / ((0.0404318 - 0.430871 * x) * x + 1.0);
    t / (1.0 + t)
}

/// Trapezoidal one-pole.
#[derive(Clone, Copy, Default)]
struct Lp1 {
    s: f32,
}

impl Lp1 {
    #[inline]
    fn lp(&mut self, x: f32, a: f32) -> f32 {
        let v = (x - self.s) * a;
        let y = v + self.s;
        self.s = v + y;
        y
    }
    #[inline]
    fn hp(&mut self, x: f32, a: f32) -> f32 {
        x - self.lp(x, a)
    }
}

// ---- clippers (level L: x / L, result x L) ----

#[inline]
fn cs(x: f32) -> f32 {
    0.5 * (2.0 * x.abs() + (4.0 * x * x + 1.0).sqrt()).ln() * x.signum()
}
#[inline]
pub(super) fn ct78(x: f32) -> f32 {
    let x2 = x * x;
    x * (2027025.0 + x2 * (270270.0 + x2 * (6930.0 + 36.0 * x2))) / (2027025.0 + x2 * (945945.0 + x2 * (51975.0 + x2 * (630.0 + x2))))
}
#[inline]
fn cf(x: f32) -> f32 {
    let x2 = x * x;
    5.0 * x * (21.0 + 2.0 * x2) / (105.0 + 0.6825 * x2 * (45.0 + 6.0 * x2))
}
#[inline]
fn csin(x: f32) -> f32 {
    (1.0 / (x / 8.0).cosh() + 0.125) * x.sin() + 0.5 * ct78(x / 8.0)
}
#[inline]
fn lvl(f: fn(f32) -> f32, x: f32, l: f32) -> f32 {
    f(x / l) * l
}
#[inline]
fn invsq(x: f32) -> f32 {
    1.0 - (1.0 - x) * (1.0 - x)
}
#[inline]
fn rr(x: f32, a: f32, b: f32) -> f32 {
    a + x * (b - a)
}

struct Alphas {
    a0: f32,
    a10: f32,
    a20: f32,
    a30: f32,
    a50: f32,
    a70: f32,
    a99: f32,
    a100: f32,
    a160: f32,
    a200: f32,
    a500: f32,
    a700: f32,
    a900: f32,
    a1000: f32,
    a1200: f32,
    a1280: f32,
    a1300: f32,
    a2000: f32,
    a3000: f32,
    a4000: f32,
    a4690: f32,
    a6000: f32,
    a8000: f32,
    a11000: f32,
}

impl Alphas {
    fn new(sr: f32) -> Alphas {
        let a = |f| alpha(f, sr);
        Alphas {
            a0: a(0.0),
            a10: a(10.0),
            a20: a(20.0),
            a30: a(30.0),
            a50: a(50.0),
            a70: a(70.0),
            a99: a(99.0),
            a100: a(100.0),
            a160: a(160.0),
            a200: a(200.0),
            a500: a(500.0),
            a700: a(700.0),
            a900: a(900.0),
            a1000: a(1000.0),
            a1200: a(1200.0),
            a1280: a(1280.0),
            a1300: a(1300.0),
            a2000: a(2000.0),
            a3000: a(3000.0),
            a4000: a(3000.0),
            a4690: a(4690.0),
            a6000: a(6000.0),
            a8000: a(8000.0),
            a11000: a(11000.0),
        }
    }
}

/// Per-sample smoothed controls (one-pole at 20 Hz toward block targets).
#[derive(Clone, Copy, Default)]
struct Ctl {
    lp: Lp1,
    t: f32,
}

impl Ctl {
    #[inline]
    fn next(&mut self, a20: f32) -> f32 {
        self.lp.lp(self.t, a20)
    }
}

// Controls, by index.
const C_GIN: usize = 0;
const C_GOUT: usize = 1;
const C_MIX: usize = 2;
const C_L1: usize = 3;
const C_L3: usize = 4;
const C_L4: usize = 5;
const C_L5: usize = 6;
const C_L6: usize = 7;
const C_B: usize = 8;
const C_H: usize = 9;
const C_LEVEL: usize = 10;
const C_PRE: usize = 11;
const C_GA: usize = 12;
const C_LPCOEF: usize = 13;
const C_THR: usize = 14;
const C_SCALE: usize = 15;
const C_AGE_G: usize = 16;
const C_AHC: usize = 17;
const C_ALC: usize = 18;
const C_CLIPL: usize = 19;
const NCTL: usize = 20;

/// One channel's filter states.
#[derive(Clone, Copy, Default)]
struct Chan {
    hp10_in: Lp1,
    // bass boost
    b_hp20: Lp1,
    b_lp50a: Lp1,
    b_lp50b: Lp1,
    b_hp30a: Lp1,
    b_lp200a: Lp1,
    b_lp200b: Lp1,
    b_hp200: Lp1,
    b_lp200c: Lp1,
    b_hp30b: Lp1,
    b_lp500a: Lp1,
    b_lp500b: Lp1,
    // saturation
    t_hp4690: Lp1,
    t_lp1280: Lp1,
    m_hp11000: Lp1,
    m_lp6000: Lp1,
    m_lp10: Lp1,
    m_lp3000: Lp1,
    pt_hp160: Lp1,
    pt_lp99: Lp1,
    pm_hp1300: Lp1,
    pm_lp700: Lp1,
    pm_lp70: Lp1,
    pm_lp900: Lp1,
    de_lp20: Lp1,
    hs4000: Lp1,
    hs8000: Lp1,
    // noise
    n_lp1000: Lp1,
    n_lp50a: Lp1,
    n_lp50b: Lp1,
    u_lp50: Lp1,
    u_lp1000: Lp1,
    a_prev: f32,
    env: Lp1,
    nz_hp500: Lp1,
    no_lp2000: Lp1,
    // age
    g_hs3000: Lp1,
    g_hs2000: Lp1,
    g_ls700: Lp1,
    g_ls1200: Lp1,
    g_hc: Lp1,
    g_lc: Lp1,
    hp10_out: Lp1,
}

impl Chan {
    /// The bass-boost branch y for input u.
    fn bass(&mut self, u: f32, c: &[f32; NCTL], a: &Alphas) -> f32 {
        let br1 = 20.0 * self.b_lp50b.lp(lvl(ct78, self.b_lp50a.lp(self.b_hp20.hp(u, a.a20), a.a50), c[C_L1]), a.a50);
        let rr_ = self.b_lp200b.lp(self.b_lp200a.lp(self.b_hp30a.hp(u, a.a30), a.a200), a.a200);
        let br2 = 5.0 * rr_;
        let br3 = c[C_L3] * self.b_hp200.hp(rr_.clamp(-0.01, 0.01), a.a200);
        let t = 2.0 * self.b_lp200c.lp(lvl(ct78, c[C_L4] * rr_, c[C_L5]), a.a200);
        let s = self.b_hp30b.hp(br3 + t, a.a30);
        self.b_lp500b.lp(lvl(cs, c[C_B] * self.b_lp500a.lp((br1 + br2 + s) / 6.0, a.a500), c[C_L6]), a.a500)
    }

    fn saturate(&mut self, x: f32, c: &[f32; NCTL], a: &Alphas, bias: i32, mode: i32) -> f32 {
        let xs = x * c[C_PRE];
        let into = if bias == 0 {
            xs + 4.73151 * self.t_hp4690.hp(xs, a.a4690) - 0.822243 * self.t_lp1280.lp(xs, a.a1280)
        } else {
            let m = xs + 7.943283 * self.m_hp11000.hp(xs, a.a11000) - 0.177828 * self.m_lp6000.lp(xs, a.a6000);
            1.412538 * (m + self.m_lp10.lp(m, a.a10) - 0.707946 * self.m_lp3000.lp(m, a.a3000))
        };
        let f: fn(f32) -> f32 = match mode {
            1 => ct78,
            2 => cf,
            3 => csin,
            _ => cs,
        };
        let y = lvl(f, into, c[C_LEVEL]);
        let post = if bias == 0 {
            y + -0.851138 * self.pt_hp160.hp(y, a.a160) + 4.73151 * self.pt_lp99.lp(y, a.a99)
        } else {
            let m = y - 0.917276 * self.pm_hp1300.hp(y, a.a1300) + 0.251189 * self.pm_lp700.lp(y, a.a700);
            0.707946 * (m - 3.162278 * self.pm_lp70.lp(m, a.a70) + 2.818383 * self.pm_lp900.lp(m, a.a900))
        };
        let h = c[C_H];
        let z = lvl(cs, h * post, 0.1);
        let g = 1.0 / (1.0 + self.de_lp20.lp((6.0 * z).abs() - (6.0 * xs * h).abs(), a.a20).abs());
        let z = z + g * g * self.hs4000.hp(z, a.a4000);
        z + g * g * g * self.hs8000.hp(z, a.a8000)
    }

    fn hiss(&mut self, u: f32, n: f32, c: &[f32; NCTL], a: &Alphas, sr: f32) -> f32 {
        let nc = n.clamp(-1.0, 1.0);
        let nf = self.n_lp1000.lp(nc, a.a1000);
        let cc = self.n_lp50b.lp(self.n_lp50a.lp(nc, a.a50), a.a50);
        let av = self.u_lp1000.lp(u + self.u_lp50.lp(u, a.a50) + cc, a.a1000);
        let slope = ((av - self.a_prev) * 0.001 * sr).abs();
        self.a_prev = av;
        let e = (slope - c[C_THR]).max(0.0);
        let e = e * e * c[C_SCALE];
        let env = self.env.lp(e, c[C_LPCOEF]);
        let nz = self.nz_hp500.hp(env * nf, a.a500);
        u + self.no_lp2000.lp(lvl(ct78, nz * c[C_GA], 0.1), a.a2000)
    }

    fn age(&mut self, x: f32, c: &[f32; NCTL], a: &Alphas) -> f32 {
        let g = c[C_AGE_G];
        // SPEC? "clip the filtered branch with cS(level) before adding":
        // read as y = x + gain * cS_level(filtered).
        let x = x + -g * lvl(cs, self.g_hs3000.hp(x, a.a3000), 0.02);
        let x = x + g * lvl(cs, self.g_hs2000.hp(x, a.a2000), 0.05);
        let x = x + -g * lvl(cs, self.g_ls700.lp(x, a.a700), 0.075);
        let x = x + g * lvl(cs, self.g_ls1200.lp(x, a.a1200), 0.04);
        let x = self.g_hc.lp(x, c[C_AHC]);
        let x = self.g_lc.hp(x, c[C_ALC]);
        lvl(ct78, x, c[C_CLIPL])
    }
}

/// The 32-bit LCG noise.
#[derive(Clone, Copy)]
struct Lcg {
    seed: f32,
    s: i32,
}

impl Lcg {
    fn new(seed: i32) -> Lcg {
        Lcg { seed: seed as f32, s: seed }
    }
    fn begin_block(&mut self) {
        if QUIRK_NOISE_SEED_AS_FLOAT {
            self.s = self.seed as i32;
        }
    }
    #[inline]
    fn next(&mut self) -> f32 {
        self.s = self.s.wrapping_mul(1103515245).wrapping_add(12345);
        self.s as f32 * 4.6567e-10
    }
    fn end_block(&mut self) {
        self.seed = self.s as f32;
    }
}

pub struct Bonsai {
    sr: f32,
    a: Alphas,
    ctl: [Ctl; NCTL],
    ch: [Chan; 2],
    noise: [Lcg; 2],
}

impl Bonsai {
    pub fn new(sr: f32) -> Bonsai {
        Bonsai { sr, a: Alphas::new(sr), ctl: [Ctl::default(); NCTL], ch: [Chan::default(); 2], noise: [Lcg::new(1), Lcg::new(2)] }
    }

    pub fn reset(&mut self) {
        *self = Bonsai::new(self.sr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}

    fn targets(&mut self, p: &[SlotValue; 12]) {
        let a = &self.a;
        let t = &mut self.ctl;
        t[C_GIN].t = db_to_linear(p[0].v - 24.0);
        t[C_GOUT].t = db_to_linear(p[9].v + 20.0);
        t[C_MIX].t = clamp01(p[10].v);
        let boost = db_to_linear(ext(&p[1], |v| 3.0 * v));
        let dist = 3.0 * clamp01(p[2].v);
        let d1 = dist / 3.0;
        let d2 = dist * dist;
        // SPEC? l2 = rerange01(d2, 0.125, 1) is defined by the spec but used
        // nowhere in the chain; not computed.
        t[C_L1].t = rr(invsq(d1), 0.025, 0.01);
        t[C_L3].t = rr(d2, 0.5, 10.0);
        t[C_L4].t = rr(d2, 1.0, 5.0);
        t[C_L5].t = rr(d1, 0.075, 0.025);
        t[C_L6].t = rr(d1, 0.1, 0.075);
        t[C_B].t = boost;
        let sat = clamp01(p[5].v);
        t[C_H].t = db_to_linear(rr((sat * sat + sat) / 2.0, 0.5, 3.0));
        t[C_LEVEL].t = rr(invsq(sat), 0.15, 0.025);
        t[C_PRE].t = db_to_linear(12.0 * sat);
        let sens = clamp01(p[6].v);
        let si = invsq(sens);
        t[C_GA].t = db_to_linear(p[7].v) / 4.0;
        t[C_LPCOEF].t = rr(si, a.a500, a.a50);
        t[C_THR].t = rr(si, 0.125, 0.0);
        t[C_SCALE].t = rr(sens, 100.0, 50.0);
        let dull = clamp01(p[8].v);
        t[C_AGE_G].t = db_to_linear(rr(invsq(dull), -36.0, 3.0));
        t[C_AHC].t = alpha(rr(invsq(dull), 20000.0, 2000.0), self.sr);
        t[C_ALC].t = rr(dull * dull, a.a0, a.a100);
        t[C_CLIPL].t = rr(invsq(dull), 0.25, 0.075);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.targets(p);
        let mono = p[1].deform == 1;
        let bias = p[3].i().clamp(0, 1);
        let mode = p[4].i().clamp(0, 3);
        let a20 = self.a.a20;
        for n in self.noise.iter_mut() {
            n.begin_block();
        }
        for k in 0..B {
            let mut c = [0f32; NCTL];
            for (i, v) in c.iter_mut().enumerate() {
                *v = self.ctl[i].next(a20);
            }
            let input = [l[k], r[k]];
            let mut x = [0f32; 2];
            for ch in 0..2 {
                x[ch] = self.ch[ch].hp10_in.hp(input[ch] * c[C_GIN], self.a.a10);
            }
            if mono {
                let y = self.ch[0].bass(0.5 * (x[0] + x[1]), &c, &self.a);
                x[0] += y;
            } else {
                for ch in 0..2 {
                    x[ch] += self.ch[ch].bass(x[ch], &c, &self.a);
                }
            }
            let na = self.noise[0].next();
            let nb = self.noise[1].next();
            let nz = [na + 0.5 * nb, na - 0.5 * nb];
            let mut out = [0f32; 2];
            for ch in 0..2 {
                let s = self.ch[ch].saturate(x[ch], &c, &self.a, bias, mode);
                let s = self.ch[ch].hiss(s, nz[ch], &c, &self.a, self.sr);
                let s = self.ch[ch].age(s, &c, &self.a);
                let o = self.ch[ch].hp10_out.hp(s, self.a.a10) * c[C_GOUT];
                out[ch] = input[ch] + (o - input[ch]) * c[C_MIX];
            }
            l[k] = out[0];
            r[k] = out[1];
        }
        for n in self.noise.iter_mut() {
            n.end_block();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    #[test]
    fn clipper_curves() {
        for i in -400..=400 {
            let x = i as f32 * 0.01;
            let x64 = x as f64;
            assert!((cs(x) as f64 - 0.5 * (2.0 * x64).asinh()).abs() < 1e-5, "cs {x}");
            assert!((ct78(x) as f64 - x64.tanh()).abs() < 2e-4, "ct78 {x}");
            let want_cf = 5.0 * x64 * (21.0 + 2.0 * x64 * x64) / (105.0 + 0.6825 * x64 * x64 * (45.0 + 6.0 * x64 * x64));
            assert!((cf(x) as f64 - want_cf).abs() < 1e-5, "cf {x}");
            let want_sin = (1.0 / (x64 / 8.0).cosh() + 0.125) * x64.sin() + 0.5 * (x64 / 8.0).tanh();
            assert!((csin(x) as f64 - want_sin).abs() < 1e-4, "csin {x}");
        }
        // Level scaling.
        assert!((lvl(cs, 0.3, 0.1) - 0.1 * cs(3.0)).abs() < 1e-7);
    }

    #[test]
    fn alpha_is_one_pole_coefficient() {
        // alpha = g / (1 + g) with g ~ tan(pi f / sr).
        let a = alpha(1000.0, 48000.0);
        let g = (std::f64::consts::PI * 1000.0 / 48000.0).tan();
        assert!((a as f64 - g / (1.0 + g)).abs() < 1e-4);
    }

    #[test]
    fn finite_and_bounded() {
        let ctx = FxCtx::new(48000.0);
        for (sat_mode, bias, mono) in [(0, 0, 0), (1, 1, 1), (2, 0, 1), (3, 1, 0)] {
            let mut p = default_values(28);
            p[4].v = sat_mode as f32;
            p[3].v = bias as f32;
            p[1].deform = mono;
            p[1].v = 12.0;
            p[5].v = 0.8;
            p[8].v = 0.5;
            let mut fx = Bonsai::new(48000.0);
            let mut rng = Rng::new(9);
            let mut peak = 0f32;
            for b in 0..600 {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                for k in 0..32 {
                    let n = b * 32 + k;
                    l[k] = if b < 300 { 0.5 * rng.bipolar() } else { 0.5 * (n as f32 * 0.0576).sin() };
                    r[k] = l[k];
                }
                fx.process(&p, &ctx, &mut l, &mut r);
                for x in l.iter().chain(r.iter()) {
                    assert!(x.is_finite());
                    peak = peak.max(x.abs());
                }
            }
            assert!(peak > 1e-3 && peak < 20.0, "{peak}");
        }
    }
}
