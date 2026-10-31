//! Chorus (type 9): four triangle-modulated taps on one mono delay line,
//! panned across the stereo field, with low/high cut on the wet signal and
//! clipped feedback summed with the mono input.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotValue};

const LEN: usize = 1 << 18;
/// Per-sample lag of each tap's delay time.
const TIME_LAG: f32 = 0.001;
/// The taps read without subtracting the kernel's 6-sample centre, so each
/// voice sits 6 samples later than its nominal time.
const QUIRK_NO_KERNEL_OFFSET: bool = true;
const _: () = assert!(QUIRK_NO_KERNEL_OFFSET);

pub fn slot_defs() -> [SlotDef; 12] {
    [
        envtime("Time", -11.0, -3.0, -6.0),
        lforate("Rate", -2.0),
        percent("Depth", 0.3),
        percent("Feedback", 0.5),
        freq_d("Low Cut", -36.0),
        freq_d("High Cut", 36.0),
        percent("Mix", 1.0),
        width_db("Width"),
        unused(),
        unused(),
        unused(),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 15 {
        p[4].deactivated = false;
        p[5].deactivated = false;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

pub struct Chorus {
    sr: f32,
    line: BlockDelay,
    phase: [f32; 4],
    time: [f32; 4],
    target: [f32; 4],
    pan_l: [f32; 4],
    pan_r: [f32; 4],
    first: bool,
    lp: Biquad,
    hp: Biquad,
    fb: Line,
    mix: Line,
    width: Line,
}

impl Chorus {
    pub fn new(sr: f32) -> Chorus {
        let mut pan_l = [0f32; 4];
        let mut pan_r = [0f32; 4];
        for j in 0..4 {
            let x = 2.0 * (j as f32 / 3.0) - 1.0;
            pan_l[j] = (0.5 - 0.5 * x).max(0.0).sqrt() / 2.0;
            pan_r[j] = (0.5 + 0.5 * x).max(0.0).sqrt() / 2.0;
        }
        Chorus {
            sr,
            line: BlockDelay::new(LEN),
            phase: [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0],
            time: [0.0; 4],
            target: [0.0; 4],
            pan_l,
            pan_r,
            first: true,
            lp: Biquad::new(),
            hp: Biquad::new(),
            fb: Line::new(),
            mix: Line::new(),
            width: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.line.clear();
        self.phase = [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0];
        self.first = true;
        self.lp = Biquad::new();
        self.hp = Biquad::new();
        self.fb = Line::new();
        self.mix = Line::new();
        self.width = Line::new();
    }

    fn control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        let rate = rate_per_block(&p[1], ctx.tsr, self.sr);
        let tm = time_secs(&p[0], ctx.tsr);
        for j in 0..4 {
            self.phase[j] += rate;
            if self.phase[j] >= 1.0 {
                self.phase[j] -= self.phase[j].floor();
            }
            let lfo = (2.0 * (2.0 * self.phase[j] - 1.0).abs() - 1.0) * p[2].v;
            self.target[j] = self.sr * tm * (1.0 + lfo);
            if self.first {
                self.time[j] = self.target[j];
            }
        }
        self.first = false;
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.control(p, ctx);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        self.fb.set_target_smoothed(0.5 * p[3].v * p[3].v * p[3].v);
        self.control(p, ctx);
        let centre = 0.0;

        let mut wl = [0f32; B];
        let mut wr = [0f32; B];
        for k in 0..B {
            let (mut a, mut b) = (0.0, 0.0);
            for j in 0..4 {
                self.time[j] += TIME_LAG * (self.target[j] - self.time[j]);
                let v = self.line.read(self.time[j] - centre, k);
                a += self.pan_l[j] * v;
                b += self.pan_r[j] * v;
            }
            wl[k] = a;
            wr[k] = b;
        }
        if !p[5].deactivated {
            self.lp.lp2b(calc_omega(p[5].v / 12.0, sr), 0.707);
            self.lp.process(&mut wl, &mut wr);
        }
        if !p[4].deactivated {
            self.hp.hp(calc_omega(p[4].v / 12.0, sr), 0.707);
            self.hp.process(&mut wl, &mut wr);
        }
        for k in 0..B {
            let fbs = hardclip(self.fb.at(k) * (wl[k] + wr[k]));
            self.line.write_at(k, fbs + l[k] + r[k]);
        }
        self.line.advance();

        self.width.set_target_smoothed(db_to_linear(p[7].v));
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[6].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    fn render(fx: &mut Chorus, p: &[SlotValue; 12], input: &[f32]) -> (Vec<f32>, Vec<f32>) {
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
    fn unmodulated_taps_land_six_samples_late() {
        let mut p = default_values(9);
        p[2].v = 0.0;
        p[3].v = 0.0;
        p[4].deactivated = true;
        p[5].deactivated = true;
        let mut fx = Chorus::new(48000.0);
        let mut x = vec![0f32; 2048];
        x[0] = 1.0;
        let (l, _) = render(&mut fx, &p, &x);
        let mut best = 0;
        for i in 0..l.len() {
            if l[i].abs() > l[best].abs() {
                best = i;
            }
        }
        // 2^-6 s = 750 samples, + the kernel centre.
        assert!((best as i32 - 756).abs() <= 1, "peak at {best}");
    }

    #[test]
    fn finite_and_feedback_dies_away() {
        let p = default_values(9);
        let mut fx = Chorus::new(48000.0);
        let mut rng = Rng::new(11);
        let n = 48000 * 2;
        let x: Vec<f32> = (0..n)
            .map(|i| if i < 9600 { 0.5 * rng.bipolar() + 0.3 * (i as f32 * 0.03).sin() } else { 0.0 })
            .collect();
        let (l, r) = render(&mut fx, &p, &x);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        let peak = l.iter().fold(0f32, |a, s| a.max(s.abs()));
        let tail = l[n - 4800..].iter().fold(0f32, |a, s| a.max(s.abs()));
        assert!(tail < 1e-4 * peak, "tail {tail} peak {peak}");
    }
}
