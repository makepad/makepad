//! Convolution (type 31): an impulse-response reverb whose IR travels in
//! the slot's user data. The IR is resampled to sr x Size, trimmed from
//! its start (or reversed from a point), normalised, then convolved with
//! zero added latency by a stack of uniformly partitioned frequency-domain
//! stages whose partition doubles from one host block upwards (each stage
//! starts at least one partition into the IR, so its result is always
//! ready in time). After it: an optional tilt (two shared-frequency
//! trapezoidal shelves), low and high cut, and the wet/dry mix.
//! EFFECTS-2.md 6.9.

use super::common::*;
use super::{FxCtx, FxUserData};
use crate::kit::core::{calc_omega, Biquad, Lag, SlotDef, SlotUnit, SlotValue};
use crate::kit::table::fft;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        fl("Pre-Delay", 0.0, 1.0, 0.0, SlotUnit::Secs),
        about(fl("Size", 0.5, 2.0, 1.0, SlotUnit::Percent), "IR resample factor; not modulatable"),
        about(fl("Start", 0.0, 0.9, 0.0, SlotUnit::Percent), "trims this fraction of the IR; not modulatable"),
        about(deact(fl("Reverse From", 0.0, 0.5, 0.0, SlotUnit::Percent)), "deactivated = no reverse; not modulatable"),
        freq_d("Tilt Center", 0.0),
        db("Tilt Slope", -12.0, 12.0, 0.0),
        freq_d("Low Cut", -60.0),
        freq_d("High Cut", 70.0),
        percent("Mix", 1.0),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(p: &mut [SlotValue; 12]) {
    for i in [3, 4, 6, 7] {
        p[i].deactivated = true;
    }
}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

/// Without the loaded IR the length is unknown: always run (the instance
/// answers exactly through `Fx::ringout_blocks`).
pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// The largest partition (bounds the work done in one block).
const MAX_PART: usize = 4096;

/// One uniformly partitioned stage covering IR[start, start + parts * size).
struct Stage {
    size: usize,
    start: usize,
    /// Spectra of the IR partitions, (re, im) of length 2 size each.
    h: Vec<(Vec<f32>, Vec<f32>)>,
    /// Frequency-domain delay line of input spectra (ring of parts).
    fdl: Vec<(Vec<f32>, Vec<f32>)>,
    fdl_pos: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    acc_re: Vec<f32>,
    acc_im: Vec<f32>,
}

impl Stage {
    fn new(ir: &[f32], start: usize, size: usize, parts: usize) -> Stage {
        let n = 2 * size;
        let mut h = Vec::with_capacity(parts);
        for j in 0..parts {
            let mut re = vec![0f32; n];
            let mut im = vec![0f32; n];
            for k in 0..size {
                let idx = start + j * size + k;
                if idx < ir.len() {
                    re[k] = ir[idx];
                }
            }
            fft(&mut re, &mut im, false);
            h.push((re, im));
        }
        Stage {
            size,
            start,
            h,
            fdl: (0..parts).map(|_| (vec![0f32; n], vec![0f32; n])).collect(),
            fdl_pos: 0,
            re: vec![0.0; n],
            im: vec![0.0; n],
            acc_re: vec![0.0; n],
            acc_im: vec![0.0; n],
        }
    }

    fn clear(&mut self) {
        for (a, b) in self.fdl.iter_mut() {
            a.iter_mut().for_each(|s| *s = 0.0);
            b.iter_mut().for_each(|s| *s = 0.0);
        }
        self.fdl_pos = 0;
    }

    /// `hist` = the last 2 size input samples (oldest first); returns the
    /// size output samples in `self.re[size..]`.
    fn run(&mut self, hist: impl Fn(usize) -> f32) {
        let n = 2 * self.size;
        for k in 0..n {
            self.re[k] = hist(k);
            self.im[k] = 0.0;
        }
        fft(&mut self.re, &mut self.im, false);
        let parts = self.h.len();
        self.fdl_pos = (self.fdl_pos + parts - 1) % parts;
        {
            let (fr, fi) = &mut self.fdl[self.fdl_pos];
            fr.copy_from_slice(&self.re);
            fi.copy_from_slice(&self.im);
        }
        self.acc_re.iter_mut().for_each(|s| *s = 0.0);
        self.acc_im.iter_mut().for_each(|s| *s = 0.0);
        for j in 0..parts {
            let (xr, xi) = &self.fdl[(self.fdl_pos + j) % parts];
            let (hr, hi) = &self.h[j];
            for k in 0..n {
                self.acc_re[k] += xr[k] * hr[k] - xi[k] * hi[k];
                self.acc_im[k] += xr[k] * hi[k] + xi[k] * hr[k];
            }
        }
        self.re.copy_from_slice(&self.acc_re);
        self.im.copy_from_slice(&self.acc_im);
        fft(&mut self.re, &mut self.im, true);
    }
}

/// A zero-latency partitioned convolver for one channel, fed one host
/// block at a time.
struct Convolver {
    stages: Vec<Stage>,
    hist: Vec<f32>,
    hist_mask: usize,
    out: Vec<f32>,
    out_mask: usize,
    t: usize,
}

impl Convolver {
    fn new(ir: &[f32]) -> Convolver {
        let mut stages = Vec::new();
        let mut start = 0usize;
        let mut size = B;
        let len = ir.len().max(1);
        loop {
            let remaining = len.saturating_sub(start);
            if remaining == 0 {
                break;
            }
            if size >= MAX_PART || remaining <= 2 * size {
                let parts = remaining.div_ceil(size);
                stages.push(Stage::new(ir, start, size, parts));
                break;
            }
            stages.push(Stage::new(ir, start, size, 2));
            start += 2 * size;
            size *= 2;
        }
        let hist_len = (2 * size).next_power_of_two();
        let reach = stages.iter().map(|s| s.start + s.size).max().unwrap_or(B) + 2 * B;
        let out_len = reach.next_power_of_two();
        Convolver { stages, hist: vec![0.0; hist_len], hist_mask: hist_len - 1, out: vec![0.0; out_len], out_mask: out_len - 1, t: 0 }
    }

    fn clear(&mut self) {
        for s in self.stages.iter_mut() {
            s.clear();
        }
        self.hist.iter_mut().for_each(|s| *s = 0.0);
        self.out.iter_mut().for_each(|s| *s = 0.0);
    }

    fn process(&mut self, x: &mut [f32; B]) {
        for k in 0..B {
            self.hist[(self.t + k) & self.hist_mask] = x[k];
        }
        let t_end = self.t + B;
        for s in self.stages.iter_mut() {
            if t_end % s.size != 0 {
                continue;
            }
            let n = 2 * s.size;
            let (hist, mask) = (&self.hist, self.hist_mask);
            // The history window ends at t_end (times t_end - 2 size ..).
            s.run(|k| {
                let ti = t_end as i64 - n as i64 + k as i64;
                if ti < 0 {
                    0.0
                } else {
                    hist[ti as usize & mask]
                }
            });
            // y[m] belongs to time t_end - size + m + start.
            for m in 0..s.size {
                let ti = t_end - s.size + m + s.start;
                self.out[ti & self.out_mask] += s.re[s.size + m];
            }
        }
        for k in 0..B {
            let i = (self.t + k) & self.out_mask;
            x[k] = self.out[i];
            self.out[i] = 0.0;
        }
        self.t = t_end;
    }
}

/// Band-limited windowed-sinc resampling by `ratio` = out rate / in rate.
fn resample(x: &[f32], ratio: f64) -> Vec<f32> {
    if (ratio - 1.0).abs() < 1e-9 || x.is_empty() {
        return x.to_vec();
    }
    let cut = ratio.min(1.0);
    let half = (32.0 / cut).ceil() as i64;
    let out_len = (x.len() as f64 * ratio).ceil() as usize;
    let mut y = vec![0f32; out_len];
    for (m, ym) in y.iter_mut().enumerate() {
        let t = m as f64 / ratio;
        let c = t.floor() as i64;
        let mut acc = 0.0f64;
        for n in (c - half + 1)..=(c + half) {
            if n < 0 || n as usize >= x.len() {
                continue;
            }
            let d = t - n as f64;
            let w = 0.5 * (1.0 + (std::f64::consts::PI * d / half as f64).cos());
            let u = cut * d;
            let s = if u.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u) };
            acc += x[n as usize] as f64 * cut * s * w;
        }
        *ym = acc as f32;
    }
    y
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// The IR after resampling, trimming / reversing and normalisation.
pub fn prepare_ir(left: &[f32], right: Option<&[f32]>, ir_sr: f32, sr: f32, size: f32, start: f32, reverse: Option<f32>) -> (Vec<f32>, Vec<f32>) {
    let ratio = (sr as f64 * size as f64) / ir_sr.max(1.0) as f64;
    let mut ch = [resample(left, ratio), resample(right.unwrap_or(left), ratio)];
    for c in ch.iter_mut() {
        let len = c.len();
        let cut = (len as f32 * start) as usize;
        match reverse {
            Some(rv) => {
                c.truncate(len - cut.min(len));
                let from = (c.len() as f32 * rv) as usize;
                c[from.min(len)..].reverse();
            }
            None => {
                c.drain(..cut.min(len));
            }
        }
    }
    let e = ch.iter().map(|c| c.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>()).fold(0.0, f64::max);
    if e > 1e-3 {
        let g = (1.0 / (2.0 * e)).sqrt().min(2.0) as f32;
        for c in ch.iter_mut() {
            c.iter_mut().for_each(|x| *x *= g);
        }
    }
    let [l, r] = ch;
    (l, r)
}

#[derive(Clone, Copy, PartialEq)]
struct Prepared {
    size: f32,
    start: f32,
    reverse: f32,
    reverse_off: bool,
}

pub struct Convolution {
    sr: f32,
    conv: Option<[Convolver; 2]>,
    ir_len: usize,
    prepared: Option<Prepared>,
    wanted: Option<Prepared>,
    pre: [SincLine; 2],
    pre_lag: Lag,
    pre_used: bool,
    tilt_lo: Svf,
    tilt_hi: Svf,
    lowcut: Biquad,
    highcut: Biquad,
    mix: Line,
}

fn wanted(p: &[SlotValue; 12]) -> Prepared {
    Prepared { size: p[1].v, start: p[2].v, reverse: p[3].v, reverse_off: p[3].deactivated }
}

impl Convolution {
    pub fn new(sr: f32) -> Convolution {
        Convolution {
            sr,
            conv: None,
            ir_len: 0,
            prepared: None,
            wanted: None,
            pre: [SincLine::new(1 << 19), SincLine::new(1 << 19)],
            pre_lag: Lag::new(0.001),
            pre_used: false,
            tilt_lo: Svf::new(),
            tilt_hi: Svf::new(),
            lowcut: Biquad::new(),
            highcut: Biquad::new(),
            mix: Line::new(),
        }
    }

    /// Parse the user data and build the convolver (allocates).
    pub fn prepare(&mut self, p: &[SlotValue; 12], sr: f32, ud: Option<&FxUserData>) {
        self.sr = sr;
        let w = wanted(p);
        self.prepared = Some(w);
        self.wanted = Some(w);
        self.conv = None;
        self.ir_len = 0;
        let Some(ud) = ud else { return };
        let (Some(_name), Some(rate), Some(left)) = (ud.get("irname"), ud.get("samplerate"), ud.get("left")) else { return };
        if rate.len() < 4 || left.len() < 4 {
            return;
        }
        let ir_sr = f32::from_le_bytes([rate[0], rate[1], rate[2], rate[3]]);
        let left = floats(left);
        let right = ud.get("right").map(floats).filter(|r| !r.is_empty());
        let reverse = if w.reverse_off { None } else { Some(w.reverse.clamp(0.0, 0.5)) };
        let (l, r) = prepare_ir(&left, right.as_deref(), ir_sr, sr, w.size.clamp(0.5, 2.0), w.start.clamp(0.0, 0.9), reverse);
        self.ir_len = l.len().max(r.len());
        self.conv = Some([Convolver::new(&l), Convolver::new(&r)]);
    }

    pub fn needs_prepare(&self) -> bool {
        self.wanted != self.prepared
    }

    pub fn ringout(&self, p: &[SlotValue; 12]) -> i32 {
        if self.conv.is_none() {
            return 0;
        }
        let pre = (self.sr * clamp01(p[0].v)).max(self.pre_lag.v);
        ((self.ir_len as f32 + pre) / B as f32).ceil() as i32
    }

    pub fn reset(&mut self) {
        if let Some(c) = self.conv.as_mut() {
            c[0].clear();
            c[1].clear();
        }
        self.pre[0].clear();
        self.pre[1].clear();
        self.pre_lag = Lag::new(0.001);
        self.pre_used = false;
        self.tilt_lo.reset();
        self.tilt_hi.reset();
        self.lowcut.suspend();
        self.highcut.suspend();
        self.mix = Line::new();
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.wanted = Some(wanted(p));
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.wanted = Some(wanted(p));
        let Some(conv) = self.conv.as_mut() else { return };
        let sr = self.sr;
        let (dry_l, dry_r) = (*l, *r);
        let mut wl = *l;
        let mut wr = *r;
        let target = sr * clamp01(p[0].v);
        self.pre_lag.set(target);
        if target > 0.0 {
            self.pre_used = true;
        }
        if self.pre_used {
            for k in 0..B {
                let d = self.pre_lag.process().max(6.0);
                let (a, b) = (self.pre[0].read(d), self.pre[1].read(d));
                self.pre[0].write(wl[k]);
                self.pre[1].write(wr[k]);
                wl[k] = a;
                wr[k] = b;
            }
        }
        conv[0].process(&mut wl);
        conv[1].process(&mut wr);
        if !p[4].deactivated {
            let fc = 440.0 * (p[4].v / 12.0).exp2();
            let a = 10f32.powf(p[5].v / 40.0);
            self.tilt_lo.set(SvfMode::LowShelf, fc, 0.0, 1.0 / a, sr, B);
            self.tilt_hi.set(SvfMode::HighShelf, fc, 0.0, a, sr, B);
            for k in 0..B {
                let (a1, b1) = self.tilt_lo.tick(wl[k], wr[k]);
                let (a2, b2) = self.tilt_hi.tick(a1, b1);
                wl[k] = a2;
                wr[k] = b2;
            }
            self.tilt_lo.settle();
            self.tilt_hi.settle();
        }
        if !p[6].deactivated {
            self.lowcut.hp(calc_omega(p[6].v / 12.0, sr), 0.707);
            self.lowcut.process(&mut wl, &mut wr);
        }
        if !p[7].deactivated {
            self.highcut.lp(calc_omega(p[7].v / 12.0, sr), 0.707);
            self.highcut.process(&mut wl, &mut wr);
        }
        self.mix.set_target_smoothed(clamp01(p[8].v));
        *l = dry_l;
        *r = dry_r;
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx, FxUserData};
    use super::*;
    use crate::dsp::Rng;

    fn user_data(ir: &[f32], rate: f32) -> FxUserData {
        let mut ud = FxUserData::default();
        ud.set("irname", b"test".to_vec());
        ud.set("samplerate", rate.to_le_bytes().to_vec());
        ud.set("left", ir.iter().flat_map(|x| x.to_le_bytes()).collect());
        ud
    }

    #[test]
    fn equals_direct_convolution_with_no_latency() {
        for len in [1usize, 100, 5000] {
            let mut rng = Rng::new(len as u32);
            // Small energy so normalisation stays off (E <= 1e-3).
            let scale = (0.5e-3 / len as f32).sqrt();
            let ir: Vec<f32> = (0..len).map(|_| rng.bipolar() * scale).collect();
            let e: f32 = ir.iter().map(|x| x * x).sum();
            assert!(e <= 1e-3);
            let mut fx = Convolution::new(48000.0);
            let p = default_values(31);
            fx.prepare(&p, 48000.0, Some(&user_data(&ir, 48000.0)));
            let ctx = FxCtx::new(48000.0);
            let blocks = (len + 3000) / 32 + 2;
            let input: Vec<f32> = (0..blocks * 32).map(|_| rng.bipolar()).collect();
            let mut out = Vec::new();
            for b in 0..blocks {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                l.copy_from_slice(&input[b * 32..b * 32 + 32]);
                r.copy_from_slice(&l);
                fx.process(&p, &ctx, &mut l, &mut r);
                out.extend_from_slice(&l);
            }
            for n in 0..out.len() {
                let mut y = 0.0f64;
                for k in 0..len.min(n + 1) {
                    y += ir[k] as f64 * input[n - k] as f64;
                }
                assert!((out[n] as f64 - y).abs() < 1e-4, "len {len} n {n}: {} vs {y}", out[n]);
            }
        }
    }

    #[test]
    fn normalisation_rule() {
        let ir = vec![1.0f32; 64];
        let (l, r) = prepare_ir(&ir, None, 48000.0, 48000.0, 1.0, 0.0, None);
        let e: f32 = l.iter().map(|x| x * x).sum();
        // E = 64 -> g = sqrt(1/128): energy 0.5.
        assert!((e - 0.5).abs() < 1e-4, "{e}");
        assert_eq!(l, r);
        let quiet = vec![0.001f32; 64];
        let (q, _) = prepare_ir(&quiet, None, 48000.0, 48000.0, 1.0, 0.0, None);
        assert_eq!(q, quiet);
        let (t, _) = prepare_ir(&[1.0, 2.0, 3.0, 4.0], None, 48000.0, 48000.0, 1.0, 0.5, None);
        assert_eq!(t.len(), 2);
        let (rv, _) = prepare_ir(&[0.001, 0.002, 0.003, 0.004], None, 48000.0, 48000.0, 1.0, 0.0, Some(0.5));
        assert_eq!(rv, vec![0.001, 0.002, 0.004, 0.003]);
    }

    #[test]
    fn passes_through_without_data() {
        let mut fx = Convolution::new(48000.0);
        let p = default_values(31);
        fx.prepare(&p, 48000.0, None);
        let mut l = [0.3f32; 32];
        let mut r = [-0.3f32; 32];
        fx.process(&p, &FxCtx::new(48000.0), &mut l, &mut r);
        assert!(l.iter().all(|x| *x == 0.3) && r.iter().all(|x| *x == -0.3));
        assert_eq!(fx.ringout(&p), 0);
    }

    #[test]
    fn resampler_keeps_a_low_sine() {
        let x: Vec<f32> = (0..4800).map(|n| (std::f32::consts::TAU * 500.0 * n as f32 / 48000.0).sin()).collect();
        let y = resample(&x, 2.0);
        for m in 400..9000 {
            let want = (std::f32::consts::TAU * 500.0 * m as f32 / 96000.0).sin();
            assert!((y[m] - want).abs() < 1e-2, "{m}");
        }
    }
}
