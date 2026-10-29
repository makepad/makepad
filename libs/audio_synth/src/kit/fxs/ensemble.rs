//! Ensemble (type 20): a three-phase chorus ensemble on two LFO families,
//! either through windowed-sinc digital delays or through a model of a
//! bucket-brigade delay chip: an N-stage sample buffer clocked between the
//! audio samples, fed and read through banks of complex one-pole input and
//! output filters (the published combined BBD + filter model), each tap
//! followed by a small wave-digital model of the chip's output stage.
//! EFFECTS-2.md 6.7.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, calc_omega_hz, Biquad, SlotDef, SlotUnit, SlotValue};
use std::sync::OnceLock;

/// The anti-alias cutoff is computed as an angular value (2 pi 400 ...)
/// and then used as Hz; mirrored.
const QUIRK_ANGULAR_CUTOFF_AS_HZ: bool = true;
/// The per-tick update of the filter gains rotates by the pole angle only
/// (the decay magnitude is ignored); mirrored.
const QUIRK_BBD_PHASE_ONLY_ROTATION: bool = true;
const _: () = assert!(QUIRK_ANGULAR_CUTOFF_AS_HZ && QUIRK_BBD_PHASE_ONLY_ROTATION);

const STAGE_NAMES: &[&str] = &["Digital Delay", "BBD 128 Stages", "BBD 256 Stages", "BBD 512 Stages", "BBD 1024 Stages", "BBD 2048 Stages", "BBD 4096 Stages"];

/// log2(0.01) .. log2(20).
const RATE_MIN: f32 = -6.643856;
const RATE_MAX: f32 = 4.321928;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        freq("Anti-Alias Filter", 45.2326),
        fl("Frequency 1", RATE_MIN, RATE_MAX, -2.473931, SlotUnit::Log2Hz),
        percent("Depth 1", 1.0),
        fl("Frequency 2", RATE_MIN, RATE_MAX, 2.464668, SlotUnit::Log2Hz),
        percent("Depth 2", 0.6),
        choice("Type", STAGE_NAMES, 2.0),
        about(fl("Clock Rate", 1.5, 100.0, 40.0, SlotUnit::None), "BBD clock in kHz"),
        percent("Saturation", 0.0),
        percent("Feedback", 0.0),
        bipolar("Width", 1.0),
        percent("Mix", 1.0),
        freq_d("Reconstruction Filter", 45.2326),
    ]
}

// SPEC? the default deactivation of the reconstruction filter for a new
// slot is not stated (old patches get it deactivated): active here.
pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 22 {
        p[11].v = 45.2326;
        p[11].deactivated = true;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

// ------------------------------------------------------------ complex ---

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct C {
    re: f64,
    im: f64,
}

impl C {
    const fn new(re: f64, im: f64) -> C {
        C { re, im }
    }
    #[inline]
    fn mul(self, o: C) -> C {
        C::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
    #[inline]
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    #[inline]
    fn scale(self, s: f64) -> C {
        C::new(self.re * s, self.im * s)
    }
    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        C::new((self.re * o.re + self.im * o.im) / d, (self.im * o.re - self.re * o.im) / d)
    }
    #[inline]
    fn exp(self) -> C {
        let m = self.re.exp();
        C::new(m * self.im.cos(), m * self.im.sin())
    }
    #[inline]
    fn cis(a: f64) -> C {
        C::new(a.cos(), a.sin())
    }
}

const IN_ROOTS: [C; 4] = [C::new(-10329.2715, -329.848), C::new(-10329.2715, 329.848), C::new(366.990557, -1811.4318), C::new(366.990557, 1811.4318)];
const IN_POLES: [C; 4] = [C::new(-55482.0, -25082.0), C::new(-55482.0, 25082.0), C::new(-26292.0, -59437.0), C::new(-26292.0, 59437.0)];
const OUT_ROOTS: [C; 4] = [C::new(-11256.0, -99566.0), C::new(-11256.0, 99566.0), C::new(-13802.0, -24606.0), C::new(-13802.0, 24606.0)];
const OUT_POLES: [C; 4] = [C::new(-51468.0, -21437.0), C::new(-51468.0, 21437.0), C::new(-26276.0, -59699.0), C::new(-26276.0, 59699.0)];

/// The BBD's filter banks at one anti-alias frequency.
#[derive(Clone, Copy, Default)]
struct BbdFilters {
    p_in: [C; 4],
    pc_in: [C; 4],
    gin: [C; 4],
    p_out: [C; 4],
    pc_out: [C; 4],
    gout: [C; 4],
    h0: f64,
}

impl BbdFilters {
    fn new(fa: f64, sr: f64) -> BbdFilters {
        let ts = 1.0 / sr;
        let mut f = BbdFilters::default();
        let ki = fa / 9900.0;
        let ko = fa / 9500.0;
        let mut h0 = 0.0;
        for i in 0..4 {
            let (r, p) = (IN_ROOTS[i].scale(ki), IN_POLES[i].scale(ki));
            f.p_in[i] = p;
            f.pc_in[i] = p.scale(ts).exp();
            // SPEC? the input gain folds in the hold-input discretisation
            // r (pc - 1) / p so the state update is x = pc x + u.
            f.gin[i] = r.mul(f.pc_in[i].add(C::new(-1.0, 0.0))).div(p);
            let (r, p) = (OUT_ROOTS[i].scale(ko), OUT_POLES[i].scale(ko));
            f.p_out[i] = p;
            f.pc_out[i] = p.scale(ts).exp();
            f.gout[i] = r.div(p);
            h0 -= r.div(p).re;
        }
        f.h0 = h0;
        f
    }
}

/// One bucket-brigade line: N stages, write and read ticks alternating
/// between the audio samples.
struct Bbd {
    buf: Vec<f32>,
    n: usize,
    ptr: usize,
    tn: f64,
    write_next: bool,
    x_in: [C; 4],
    x_out: [C; 4],
    y_prev: f64,
}

impl Bbd {
    fn new() -> Bbd {
        Bbd { buf: vec![0.0; 4096], n: 512, ptr: 0, tn: 0.0, write_next: true, x_in: [C::default(); 4], x_out: [C::default(); 4], y_prev: 0.0 }
    }
    fn set_stages(&mut self, n: usize) {
        if n != self.n {
            self.n = n;
            self.clear();
        }
    }
    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.ptr = 0;
        self.tn = 0.0;
        self.write_next = true;
        self.x_in = [C::default(); 4];
        self.x_out = [C::default(); 4];
        self.y_prev = 0.0;
    }
    /// One audio sample through a line of `delay` seconds.
    fn tick(&mut self, f: &BbdFilters, u: f32, delay: f64, sr: f64) -> f32 {
        let tick = (delay / (2.0 * self.n as f64) * sr).max(1e-3);
        let mut acc = [C::default(); 4];
        let mut g_in: Option<[C; 4]> = None;
        let mut g_out: Option<[C; 4]> = None;
        let rot = 2.0 * tick / sr;
        while self.tn < 1.0 {
            if self.write_next {
                let g = match g_in {
                    None => std::array::from_fn(|i| f.gin[i].mul(f.p_in[i].scale(self.tn / sr).exp())),
                    Some(g) => std::array::from_fn(|i| g[i].mul(C::cis(f.p_in[i].im * rot))),
                };
                let mut v = 0.0;
                for i in 0..4 {
                    v += g[i].mul(self.x_in[i]).re;
                }
                self.buf[self.ptr] = v as f32;
                g_in = Some(g);
            } else {
                let g = match g_out {
                    None => std::array::from_fn(|i| f.gout[i].mul(f.p_out[i].scale((1.0 - self.tn) / sr).exp())),
                    Some(g) => std::array::from_fn(|i| g[i].mul(C::cis(-f.p_out[i].im * rot))),
                };
                self.ptr = (self.ptr + 1) % self.n;
                let y = self.buf[self.ptr] as f64;
                let d = y - self.y_prev;
                for i in 0..4 {
                    acc[i] = acc[i].add(g[i].scale(d));
                }
                self.y_prev = y;
                g_out = Some(g);
            }
            self.write_next = !self.write_next;
            self.tn += tick;
        }
        self.tn -= 1.0;
        let mut out = f.h0 * self.y_prev;
        for i in 0..4 {
            self.x_in[i] = f.pc_in[i].mul(self.x_in[i]).add(C::new(u as f64, 0.0));
            self.x_out[i] = f.pc_out[i].mul(self.x_out[i]).add(acc[i]);
            out += self.x_out[i].re;
        }
        out as f32
    }
}

// ---------------------------------------------- output stage (WDF) -----

const WDF_ALPHA: f64 = 0.4;

fn current_lut() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        (0..65536)
            .map(|i| {
                let u = -5.0 + 10.0 * i as f64 / 65535.0;
                (u.signum() * 2e-9 * u.abs().powf(0.33)) as f32
            })
            .collect()
    })
}

#[inline]
fn lut_current(u: f64) -> f64 {
    let idx = ((u + 5.0) / 10.0 * 65535.0).clamp(0.0, 65535.0) as usize;
    current_lut()[idx] as f64
}

#[inline]
fn wright_omega2(x: f64) -> f64 {
    if x < -3.684303659906469 {
        0.0
    } else if x > 1.972967391708859 {
        x
    } else {
        0.5836596684310648 + x * (0.4451353886588814 + x * (0.1126446405111627 + 0.009451797158780131 * x))
    }
}

/// An alpha-transform capacitor port.
#[derive(Clone, Copy, Default)]
struct Cap {
    r: f64,
    a: f64,
    b: f64,
}

impl Cap {
    fn new(c: f64, sr: f64) -> Cap {
        Cap { r: 1.0 / ((1.0 + WDF_ALPHA) * sr * c), a: 0.0, b: 0.0 }
    }
    #[inline]
    fn reflected(&self) -> f64 {
        self.b
    }
    #[inline]
    fn incident(&mut self, a: f64) {
        let b_next = 0.5 * (1.0 + WDF_ALPHA) * a + 0.5 * (1.0 - WDF_ALPHA) * self.b;
        self.a = a;
        self.b = b_next;
    }
    #[inline]
    fn voltage(&self) -> f64 {
        0.5 * (self.a + self.b)
    }
}

/// series(R_gk, parallel(-V_in, parallel(C_gk, series(C_gp, parallel(C_pk,
/// I_s))))) under a diode.
#[derive(Clone, Copy)]
struct OutputStage {
    r_gk: f64,
    r_vin: f64,
    r_is: f64,
    c_gk: Cap,
    c_gp: Cap,
    c_pk: Cap,
    vp: f64,
    // port resistances of the adaptors
    r_p2: f64,
    r_s2: f64,
    r_p1b: f64,
    r_p1: f64,
    r_s1: f64,
}

impl OutputStage {
    fn new(sr: f64) -> OutputStage {
        let mut s = OutputStage {
            r_gk: 2.7e3,
            r_vin: 1e-9,
            r_is: 1e9,
            c_gk: Cap::new(1.6e-12, sr),
            c_gp: Cap::new(1.7e-12, sr),
            c_pk: Cap::new(0.33e-12, sr),
            vp: 0.0,
            r_p2: 0.0,
            r_s2: 0.0,
            r_p1b: 0.0,
            r_p1: 0.0,
            r_s1: 0.0,
        };
        let par = |a: f64, b: f64| a * b / (a + b);
        s.r_p2 = par(s.c_pk.r, s.r_is);
        s.r_s2 = s.c_gp.r + s.r_p2;
        s.r_p1b = par(s.c_gk.r, s.r_s2);
        s.r_p1 = par(s.r_vin, s.r_p1b);
        s.r_s1 = s.r_gk + s.r_p1;
        s
    }
    #[inline]
    fn tick(&mut self, vg: f64, drive: f64) -> f64 {
        let is = lut_current(0.1 * vg - 0.001 * self.vp);
        // up: leaves
        let b_vin = -vg; // the inverted voltage source
        let b_is = self.r_is * is;
        let b_cpk = self.c_pk.reflected();
        let b_cgp = self.c_gp.reflected();
        let b_cgk = self.c_gk.reflected();
        let b_rgk = 0.0;
        let (g_cpk, g_is) = (1.0 / self.c_pk.r, 1.0 / self.r_is);
        let b_p2 = (g_cpk * b_cpk + g_is * b_is) / (g_cpk + g_is);
        let b_s2 = -(b_cgp + b_p2);
        let (g_cgk, g_s2) = (1.0 / self.c_gk.r, 1.0 / self.r_s2);
        let b_p1b = (g_cgk * b_cgk + g_s2 * b_s2) / (g_cgk + g_s2);
        let (g_vin, g_p1b) = (1.0 / self.r_vin, 1.0 / self.r_p1b);
        let b_p1 = (g_vin * b_vin + g_p1b * b_p1b) / (g_vin + g_p1b);
        let b_s1 = -(b_rgk + b_p1);
        // root diode
        let (dis, vt) = (1e-10, 0.02585);
        let r = self.r_s1;
        let a_root = b_s1;
        let x = (r * dis / vt).ln() + (a_root + r * dis) / vt;
        let b_root = a_root + 2.0 * r * dis - 2.0 * vt * wright_omega2(x);
        // down
        let sum = b_root + b_rgk + b_p1;
        let a_p1 = b_p1 - (self.r_p1 / r) * sum;
        let a_p1b = a_p1 + b_p1 - b_p1b;
        let a_cgk = a_p1b + b_p1b - b_cgk;
        let a_s2 = a_p1b + b_p1b - b_s2;
        let sum2 = a_s2 + b_cgp + b_p2;
        let a_cgp = b_cgp - (self.c_gp.r / self.r_s2) * sum2;
        let a_p2 = b_p2 - (self.r_p2 / self.r_s2) * sum2;
        let a_cpk = a_p2 + b_p2 - b_cpk;
        self.c_gk.incident(a_cgk);
        self.c_gp.incident(a_cgp);
        self.c_pk.incident(a_cpk);
        self.vp = self.c_pk.voltage();
        drive * self.vp + (1.0 - drive) * vg
    }
}

// ------------------------------------------------------------ effect ---

/// Three sine outputs at 0, 1/3, 2/3 of a cycle, ramped per sample.
#[derive(Default)]
struct TriLfo {
    ph: f32,
    out: [Line; 3],
}

impl TriLfo {
    fn tick(&mut self, rate: f32, depth: f32, sr: f32) {
        self.ph += rate_linear(-rate.clamp(-7.64386, 5.322), sr);
        self.ph -= self.ph.floor();
        for j in 0..3 {
            self.out[j].set_target(sine_turns(self.ph + j as f32 / 3.0) * depth);
        }
    }
}

pub struct Ensemble {
    sr: f32,
    lfo1: TriLfo,
    lfo2: TriLfo,
    lines: [SincLine; 2],
    aa: Biquad,
    bbd: [Bbd; 4],
    stage: [OutputStage; 4],
    filters: BbdFilters,
    filt_fa: f64,
    block: u32,
    dc1: Biquad,
    dc2: Biquad,
    recon1: Biquad,
    recon2: Biquad,
    fb: [f32; 2],
    width: Line,
    mix: Line,
    ty: i32,
}

impl Ensemble {
    pub fn new(sr: f32) -> Ensemble {
        let srd = sr as f64;
        Ensemble {
            sr,
            lfo1: TriLfo::default(),
            lfo2: TriLfo::default(),
            lines: [SincLine::new(8192), SincLine::new(8192)],
            aa: Biquad::new(),
            bbd: std::array::from_fn(|_| Bbd::new()),
            stage: [OutputStage::new(srd); 4],
            filters: BbdFilters::new(9900.0, srd),
            filt_fa: -1.0,
            block: 0,
            dc1: Biquad::new(),
            dc2: Biquad::new(),
            recon1: Biquad::new(),
            recon2: Biquad::new(),
            fb: [0.0; 2],
            width: Line::new(),
            mix: Line::new(),
            ty: -1,
        }
    }

    pub fn reset(&mut self) {
        *self = Ensemble::new(self.sr);
    }

    fn lfos(&mut self, p: &[SlotValue; 12]) {
        self.lfo1.tick(p[1].v, clamp01(p[2].v), self.sr);
        self.lfo2.tick(p[3].v, clamp01(p[4].v), self.sr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.lfos(p);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let srd = sr as f64;
        self.lfos(p);
        let ty = p[5].i().clamp(0, 6);
        let digital = ty == 0;
        let n = if digital { 512 } else { 128usize << (ty - 1) };
        if ty != self.ty {
            for b in self.bbd.iter_mut() {
                b.set_stages(n);
            }
            self.ty = ty;
        }
        let c = p[6].v.max(1.0) as f64;
        let t0 = n as f64 / (2.0 * c);
        let scale = 0.95 * t0 / (0.6 + 0.2);
        let (d1, d2) = (scale * 0.6, scale * 0.2);
        let fa_raw = 2.0 * std::f64::consts::PI * 400.0;
        let fa = (fa_raw * (p[0].v as f64 / 12.0).exp2() * (c * 0.01).powf(0.75)).min(25000.0);
        if digital {
            self.aa.lp(calc_omega_hz(fa as f32, sr), 0.7071);
        } else if self.block % 4 == 0 || self.filt_fa < 0.0 {
            if fa != self.filt_fa {
                self.filters = BbdFilters::new(fa, srd);
                self.filt_fa = fa;
            }
        }
        self.block = self.block.wrapping_add(1);
        let kfb = if digital { 1.0 } else { 0.2 };
        let fb_gain = 0.49 * (kfb * clamp01(p[8].v)).sqrt();
        let w50 = calc_omega_hz(50.0, sr);
        self.dc1.hp(w50, 1.0 / 0.7654);
        self.dc1.instantize();
        self.dc2.hp(w50, 1.0 / 1.8478);
        self.dc2.instantize();
        let drive = clamp01(p[7].v) as f64;
        let bbd_gain = 10f64.powf(-8.0 / 20.0);
        let (dry_l, dry_r) = (*l, *r);

        for k in 0..B {
            let t: [f64; 3] = std::array::from_fn(|j| d1 * self.lfo1.out[j].at(k) as f64 + d2 * self.lfo2.out[j].at(k) as f64 + t0);
            let input = [l[k], r[k]];
            let mut taps = [[0f32; 2]; 2];
            let mut x = [0f32; 2];
            for ch in 0..2 {
                x[ch] = lookup_waveshape(WS_SOFT, 0.75 * input[ch] + self.fb[ch]);
            }
            if digital {
                let (xl, xr) = self.aa.tick(x[0], x[1]);
                x = [xl, xr];
            }
            for ch in 0..2 {
                let tt = [t[ch], t[ch + 1]];
                for j in 0..2 {
                    taps[ch][j] = if digital {
                        if j == 0 {
                            self.lines[ch].write(x[ch]);
                        }
                        self.lines[ch].read((tt[j] * srd / 1000.0) as f32)
                    } else {
                        let v = self.bbd[2 * ch + j].tick(&self.filters, x[ch], tt[j] / 1000.0, srd);
                        (v as f64 * bbd_gain) as f32
                    };
                }
            }
            let (f0, f1) = self.dc1.tick(fb_gain * (taps[0][0] + taps[0][1]), fb_gain * (taps[1][0] + taps[1][1]));
            let (f0, f1) = self.dc2.tick(f0, f1);
            self.fb = [f0, f1];
            let mut out = [0f32; 2];
            for ch in 0..2 {
                for j in 0..2 {
                    out[ch] += self.stage[2 * ch + j].tick(taps[ch][j] as f64, drive) as f32;
                }
            }
            l[k] = out[0];
            r[k] = out[1];
        }
        let mut wet_l = *l;
        let mut wet_r = *r;
        if !p[11].deactivated {
            let w = calc_omega(p[11].v / 12.0, sr);
            self.recon1.lp(w, 0.7071);
            self.recon2.lp(w, 0.7071);
            self.recon1.process(&mut wet_l, &mut wet_r);
            self.recon2.process(&mut wet_l, &mut wet_r);
        }
        self.width.set_target_smoothed(p[9].v.clamp(-1.0, 1.0));
        apply_width(&mut wet_l, &mut wet_r, &self.width);
        self.mix.set_target_smoothed(p[10].v.clamp(-1.0, 1.0));
        *l = dry_l;
        *r = dry_r;
        self.mix.fade_2_blocks(l, r, &wet_l, &wet_r);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    fn render(p: &[SlotValue; 12], blocks: usize) -> (Vec<f32>, Vec<f32>) {
        let ctx = FxCtx::new(48000.0);
        let mut fx = Ensemble::new(48000.0);
        let mut rng = Rng::new(5);
        let (mut inp, mut out) = (Vec::new(), Vec::new());
        for b in 0..blocks {
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                let n = b * 32 + k;
                l[k] = 0.3 * (std::f32::consts::TAU * 330.0 * n as f32 / 48000.0).sin() + 0.1 * rng.bipolar();
                r[k] = l[k];
            }
            inp.extend_from_slice(&l);
            fx.process(p, &ctx, &mut l, &mut r);
            assert!(l.iter().chain(r.iter()).all(|x| x.is_finite() && x.abs() < 10.0));
            out.extend_from_slice(&l);
        }
        (inp, out)
    }

    #[test]
    fn both_modes_finite_and_modulated() {
        for ty in [0.0f32, 1.0, 3.0, 6.0] {
            let mut p = default_values(20);
            p[5].v = ty;
            p[8].v = 0.5;
            let (inp, out) = render(&p, 1500);
            let e_out: f32 = out[24000..].iter().map(|x| x * x).sum();
            let diff: f32 = out[24000..].iter().zip(&inp[24000..]).map(|(a, b)| (a - b) * (a - b)).sum();
            assert!(e_out > 1e-2, "type {ty}: silent");
            assert!(diff > 1e-3 * e_out, "type {ty}: no delay effect");
        }
    }

    #[test]
    fn bbd_line_delays_by_its_time() {
        // A slow step through one BBD line arrives after about the delay.
        let f = BbdFilters::new(9900.0, 48000.0);
        let mut b = Bbd::new();
        b.set_stages(512);
        let delay = 0.005;
        let mut first = None;
        for n in 0..2000 {
            let y = b.tick(&f, if n >= 100 { 1.0 } else { 0.0 }, delay, 48000.0);
            if first.is_none() && y.abs() > 0.2 {
                first = Some(n);
            }
        }
        let n = first.expect("no output") as f32 - 100.0;
        assert!((n - 240.0).abs() < 12.0, "{n}");
    }
}
