//! Frequency shifter (type 7): a single-sideband (quadrature mix, half-band
//! low-pass, quadrature mix back) shifter inside a feedback delay, with a
//! separate right-channel shift as a multiple of the left one.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{HalfBand, SlotDef, SlotUnit, SlotValue};
use std::f64::consts::FRAC_PI_2;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        about(extend(fl("Left", -10.0, 10.0, 0.0, SlotUnit::None)), "shift x10 Hz (extended: x1000 Hz)"),
        bipolar("Right", 1.0),
        envtime("Time", -8.0, 5.0, -8.0),
        percent("Feedback", 0.0),
        percent("Mix", 1.0),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(p: &[SlotValue; 12], sr: f32) -> i32 {
    // SPEC? "feedback" read as the applied gain Feedback^3.
    let fb = p[3].v.clamp(0.0, 1.0).powi(3);
    let floor = 10f32.powf(-4.8);
    let g = fb.max(floor);
    if g >= 1.0 {
        return -1;
    }
    let time = sr * p[2].v.exp2();
    ((time / B as f32) * (1.0 + floor.ln() / g.ln())).ceil() as i32
}

/// The right oscillators follow the left ones' state when Right is exactly 1.
fn slew(dst: &mut Quadrature, src: &Quadrature) {
    dst.r = 0.01 * src.r + 0.99 * dst.r;
    dst.i = 0.01 * src.i + 0.99 * dst.i;
}

/// Line length (power of two) per channel.
const LINE_LEN: usize = 1 << 18;

pub struct FreqShift {
    sr: f32,
    line: [BlockDelay; 2],
    o1: [Quadrature; 2],
    o2: [Quadrature; 2],
    hb: [HalfBand; 2],
    time: f32,
    time_target: f32,
    fb: Line,
    mix: Line,
    first: bool,
}

impl FreqShift {
    pub fn new(sr: f32) -> FreqShift {
        FreqShift {
            sr,
            line: [BlockDelay::new(LINE_LEN), BlockDelay::new(LINE_LEN)],
            o1: [Quadrature::new(0.0); 2],
            o2: [Quadrature::new(0.0); 2],
            hb: [HalfBand::new(12, true), HalfBand::new(12, true)],
            time: 0.0,
            time_target: 0.0,
            fb: Line::new(),
            mix: Line::new(),
            first: true,
        }
    }

    pub fn reset(&mut self) {
        for l in &mut self.line {
            l.clear();
        }
        for h in &mut self.hb {
            h.reset();
        }
        self.o1 = [Quadrature::new(0.0); 2];
        self.o2 = [Quadrature::new(0.0); 2];
        self.first = true;
    }

    fn control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.fb.set_target(p[3].v.powi(3));
        self.time_target = self.sr * time_secs(&p[2], ctx.tsr) - 6.0;
        if self.first {
            self.time = self.time_target;
            self.first = false;
        }
        let shift = p[0].v * if p[0].extend { 1000.0 } else { 10.0 };
        let w = std::f64::consts::TAU * shift as f64 / self.sr as f64;
        self.o1[0].set_rate(FRAC_PI_2 - w.min(0.0));
        self.o2[0].set_rate(FRAC_PI_2 + w.max(0.0));
        if p[1].v == 1.0 {
            let (a, b) = (self.o1[0], self.o2[0]);
            slew(&mut self.o1[1], &a);
            slew(&mut self.o2[1], &b);
            self.o1[1].set_rate(FRAC_PI_2 - w.min(0.0));
            self.o2[1].set_rate(FRAC_PI_2 + w.max(0.0));
        } else {
            let wr = w * p[1].v as f64;
            self.o1[1].set_rate(FRAC_PI_2 - wr.min(0.0));
            self.o2[1].set_rate(FRAC_PI_2 + wr.max(0.0));
        }
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.control(p, ctx);
        let min_t = (12 + B) as f32;
        let mut times = [0f32; 32];
        for t in times.iter_mut() {
            self.time += 1e-4 * (self.time_target - self.time);
            *t = self.time.max(min_t);
        }
        let mut wet = [[0f32; 32]; 2];
        for c in 0..2 {
            let mut re = [0f32; 32];
            let mut im = [0f32; 32];
            for k in 0..32 {
                let d = self.line[c].read(times[k], k);
                let o = &mut self.o1[c];
                re[k] = d * o.r as f32;
                im[k] = d * o.i as f32;
                o.step();
            }
            self.hb[c].lowpass(&mut re, &mut im);
            for k in 0..32 {
                let o = &mut self.o2[c];
                let v = 2.0 * (re[k] * o.r as f32 + im[k] * o.i as f32);
                o.step();
                wet[c][k] = v;
            }
        }
        for k in 0..32 {
            let fb = self.fb.at(k);
            self.line[0].write_at(k, l[k] + lookup_waveshape(WS_SOFT, wet[0][k] * fb));
            self.line[1].write_at(k, r[k] + lookup_waveshape(WS_SOFT, wet[1][k] * fb));
        }
        self.line[0].advance();
        self.line[1].advance();
        self.mix.set_target_smoothed(p[4].v);
        self.mix.fade_2_blocks(l, r, &wet[0], &wet[1]);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.control(p, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::eq::tests::tone_amp;
    use crate::kit::fxs::{default_values, FxCtx};

    #[test]
    fn shifts_up_by_100_hz() {
        let sr = 48000.0f32;
        let mut p = default_values(7);
        p[0].v = 10.0;
        let mut fx = FreqShift::new(sr);
        let ctx = FxCtx::new(sr);
        let mut out = Vec::new();
        for b in 0..3000 {
            let mut l = [0f32; 32];
            for (k, s) in l.iter_mut().enumerate() {
                let n = (b * 32 + k) as f64;
                *s = 0.5 * (std::f64::consts::TAU * 1000.0 * n / sr as f64).sin() as f32;
            }
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            out.extend_from_slice(&l);
        }
        let tail = &out[out.len() - 24000..];
        let up = tone_amp(tail, 1100.0, sr as f64);
        let orig = tone_amp(tail, 1000.0, sr as f64);
        let down = tone_amp(tail, 900.0, sr as f64);
        assert!((up / 0.5 - 1.0).abs() < 0.02, "{up}");
        assert!(orig < 0.005 && down < 0.005, "{orig} {down}");
    }
}
