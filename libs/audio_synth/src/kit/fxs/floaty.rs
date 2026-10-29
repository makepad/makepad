//! Floaty Delay (type 30): a tape-like delay with a variable-speed read
//! head (play rate, reverse, click-smoothing window), a resonant input
//! low-pass and a pitch warp both driven by a sine plus smooth-random
//! modulation, a low-passed soft-clipped feedback path, a DC blocker and
//! an output low cut.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{softclip, SlotDef, SlotUnit, SlotValue};

const LEN: usize = 1 << 19;
const MIN_DELAY: f32 = 12.0;
/// The Time slot keeps a tempo-sync flag but the reference's processing
/// ignores it; mirrored (the time is always 2^v s).
const QUIRK_TIME_IGNORES_SYNC: bool = true;
const _: () = assert!(QUIRK_TIME_IGNORES_SYNC);

pub fn slot_defs() -> [SlotDef; 12] {
    [
        envtime("Time", -5.64386, 3.0, -1.73697),
        about(fl("Playrate", -5.0, 5.0, 1.0, SlotUnit::None), "read speed (negative = reverse)"),
        percent("Feedback", 0.5),
        freq("Cutoff", 0.0),
        percent("Resonance", 0.5),
        fl("Rate", -3.0, 4.0, 0.0, SlotUnit::Log2Hz),
        percent("Width", 0.0),
        percent("Pitch Depth", 0.0),
        percent("Filter Depth", 0.0),
        freq("Low Cut", -60.0),
        percent("Mix", 0.3),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

pub struct FloatyDelay {
    sr: f32,
    line: [SincLine; 2],
    sine: SimpleLfo,
    n1: SimpleLfo,
    n2: SimpleLfo,
    in_lp: [Svf; 2],
    fb_lp: Svf,
    dc: Svf,
    low_cut: Svf,
    base: Line,
    warp: [Line; 2],
    rate: Line,
    fb: Line,
    mix: Line,
    h: f32,
    was_one: bool,
}

#[inline]
fn hz(semis: f32) -> f32 {
    440.0 * (semis / 12.0).exp2()
}

impl FloatyDelay {
    pub fn new(sr: f32) -> FloatyDelay {
        FloatyDelay {
            sr,
            line: [SincLine::new(LEN), SincLine::new(LEN)],
            sine: SimpleLfo::new(0x5eed1),
            n1: SimpleLfo::new(0x5eed2),
            n2: SimpleLfo::new(0x5eed3),
            in_lp: [Svf::new(), Svf::new()],
            fb_lp: Svf::new(),
            dc: Svf::new(),
            low_cut: Svf::new(),
            base: Line::new(),
            warp: [Line::new(), Line::new()],
            rate: Line::new(),
            fb: Line::new(),
            mix: Line::new(),
            h: 0.0,
            was_one: false,
        }
    }

    pub fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
        self.sine = SimpleLfo::new(0x5eed1);
        self.n1 = SimpleLfo::new(0x5eed2);
        self.n2 = SimpleLfo::new(0x5eed3);
        self.in_lp = [Svf::new(), Svf::new()];
        self.fb_lp = Svf::new();
        self.dc = Svf::new();
        self.low_cut = Svf::new();
        self.base = Line::new();
        self.warp = [Line::new(), Line::new()];
        self.rate = Line::new();
        self.fb = Line::new();
        self.mix = Line::new();
        self.h = 0.0;
        self.was_one = false;
    }

    /// Advance the modulators one block; returns (mL, mR).
    fn control(&mut self, p: &[SlotValue; 12]) -> (f32, f32) {
        self.sine.advance(p[5].v, self.sr);
        self.n1.advance(p[5].v + 1.0, self.sr);
        self.n2.advance(p[5].v + 1.0, self.sr);
        let s = self.sine.sine();
        let a = self.n1.smooth_noise();
        let b = self.n2.smooth_noise();
        let w = p[6].v;
        (s + a, s + a * (1.0 - w) + b * w)
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.control(p);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let (ml, mr) = self.control(p);
        for (c, m) in [ml, mr].into_iter().enumerate() {
            let f = hz(p[3].v + m * p[8].v * 24.0).clamp(20.0, 20000.0);
            self.in_lp[c].set(SvfMode::Lp, f, p[4].v, 1.0, sr, B);
        }
        self.fb_lp.set(SvfMode::Lp, hz(p[3].v + 31.02), 0.55, 1.0, sr, B);
        self.dc.set(SvfMode::Hp, 30.0, 0.5, 1.0, sr, B);
        self.low_cut.set(SvfMode::Hp, hz(p[9].v), 0.55, 1.0, sr, B);

        let secs = p[0].v.exp2();
        let base_t = secs.clamp(0.002, 8.0) * sr;
        self.base.set_target(base_t);
        // SPEC? the warp depth formula is read with baseTime in samples, as
        // the text defines it ("... samples added to the read position").
        let span = 0.01225 * (base_t - 0.002) + 0.002;
        self.warp[0].set_target(ml * p[7].v * span);
        self.warp[1].set_target(mr * p[7].v * span);
        self.rate.set_target(p[1].v);
        self.fb.set_target(p[2].v);
        self.mix.set_target(p[10].v);

        let mut wl = [0f32; B];
        let mut wr = [0f32; B];
        for k in 0..B {
            let rate = self.rate.at(k);
            let ra = rate.abs();
            let t = self.base.at(k) * ra;
            if self.h.abs() >= t {
                self.h = 0.0;
            }
            let pos = if rate >= 0.0 { t - self.h } else { self.h };
            let habs = self.h.abs();
            let gain = if rate == 1.0 {
                if !self.was_one {
                    // SPEC? one-sample nudge toward 0 on entering rate 1.
                    self.h = if self.h > 0.0 { (self.h - 1.0).max(0.0) } else { (self.h + 1.0).min(0.0) };
                }
                1.0
            } else {
                let w: f32 = if rate < 0.0 { 512.0 } else { 256.0 };
                w.min(habs).min(t - habs).clamp(0.0, w) / w
            };
            self.was_one = rate == 1.0;
            self.h += if rate >= 0.0 { ra - 1.0 } else { ra + 1.0 };

            let pl = (pos + self.warp[0].at(k)).max(MIN_DELAY);
            let pr = (pos + self.warp[1].at(k)).max(MIN_DELAY);
            let yl = self.line[0].read(pl) * gain;
            let yr = self.line[1].read(pr) * gain;
            // SPEC? the feedback taps the DC-blocked read.
            let (yl, yr) = self.dc.tick(yl, yr);
            wl[k] = yl;
            wr[k] = yr;
            let xl = self.in_lp[0].tick_mono(l[k]);
            let xr = self.in_lp[1].tick_mono(r[k]);
            let fb = self.fb.at(k);
            let (fl_, fr_) = self.fb_lp.tick(xl + fb * yl, xr + fb * yr);
            self.line[0].write(softclip(fl_));
            self.line[1].write(softclip(fr_));
        }
        for k in 0..B {
            let (a, b) = self.low_cut.tick(wl[k], wr[k]);
            wl[k] = a;
            wr[k] = b;
        }
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    fn render(fx: &mut FloatyDelay, p: &[SlotValue; 12], input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let ctx = FxCtx::new(48000.0);
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for chunk in input.chunks(B) {
            let mut l = [0f32; B];
            l[..chunk.len()].copy_from_slice(chunk);
            let mut r = l;
            fx.process(p, &ctx, &mut l, &mut r);
            ol.extend_from_slice(&l);
            or.extend_from_slice(&r);
        }
        (ol, or)
    }

    #[test]
    fn unit_rate_delay_lands_on_the_sample() {
        let mut p = default_values(30);
        p[0].v = 0.1f32.log2();
        p[2].v = 0.0;
        p[3].v = 70.0;
        p[10].v = 1.0;
        let mut fx = FloatyDelay::new(48000.0);
        let mut x = vec![0f32; 8000];
        x[0] = 1.0;
        let (l, _) = render(&mut fx, &p, &x);
        let mut best = 0;
        for i in 0..l.len() {
            if l[i].abs() > l[best].abs() {
                best = i;
            }
        }
        assert!((best as i32 - 4800).abs() <= 3, "peak at {best}");
    }

    #[test]
    fn finite_on_noise_and_sine_at_every_rate() {
        for rate in [1.0f32, 0.5, -1.0, 2.5, 0.0] {
            let mut p = default_values(30);
            p[1].v = rate;
            p[7].v = 0.5;
            p[8].v = 0.5;
            p[6].v = 0.5;
            let mut fx = FloatyDelay::new(48000.0);
            let mut rng = Rng::new(9);
            let x: Vec<f32> = (0..48000).map(|i| if i < 9600 { 0.5 * rng.bipolar() + 0.3 * (i as f32 * 0.02).sin() } else { 0.0 }).collect();
            let (l, r) = render(&mut fx, &p, &x);
            assert!(l.iter().chain(r.iter()).all(|s| s.is_finite() && s.abs() < 10.0), "rate {rate}");
        }
    }
}
