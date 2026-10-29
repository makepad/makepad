//! Distortion (type 5): a peaking pre-EQ, a drive gain, a 4x oversampled
//! shaper with feedback and optional high cuts around it, a two-stage
//! half-band decimator, the output gain and a peaking post-EQ. The output
//! fades over the last 320 blocks of its 1600-block ring-out.

use super::common::*;
use crate::kit::shaper::{shape_static, Shaper, FX_SHAPER_SUBLIST};
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, HalfBand, SlotDef, SlotUnit, SlotValue};

const RINGOUT: i32 = 1600;
const FADE: i32 = 320;

/// The reference ramps the stateful shaper's drive by (new - old)/64 per
/// sub-sample although a block has 128 sub-samples (it overshoots to twice
/// the step); mirrored.
const QUIRK_DRIVE_STEP_DIV: f32 = 64.0;
/// The reference feeds the Digital model its input without undoing the
/// drive (every other stateful model gets s / drive); mirrored.
#[allow(dead_code)]
const QUIRK_DIGITAL_INPUT_UNSCALED: bool = true;
/// A block counts as silent input below this peak (the host does not pass
/// the slot's ring-out counter, so the effect keeps its own).
// SPEC? the reference uses the host's input-present flag for the fade counter.
const SILENCE: f32 = 1e-9;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        extend(db("Pre-EQ Gain", -48.0, 48.0, 0.0)),
        freq("Pre-EQ Frequency", 0.0),
        fl("Pre-EQ Bandwidth", 0.0, 5.0, 2.0, SlotUnit::None),
        freq_d("Pre-EQ High Cut", 70.0),
        extend(db("Drive", -24.0, 24.0, 0.0)),
        bipolar("Feedback", 0.0),
        extend(db("Post-EQ Gain", -48.0, 48.0, 0.0)),
        freq("Post-EQ Frequency", 0.0),
        fl("Post-EQ Bandwidth", 0.0, 5.0, 2.0, SlotUnit::None),
        freq_d("Post-EQ High Cut", 70.0),
        db("Gain", -24.0, 24.0, 0.0),
        choice("Model", &MODEL_NAMES, 0.0),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 11 {
        p[11].v = 0.0;
        p[0].extend = false;
        p[6].extend = false;
    }
    if rev <= 15 {
        p[3].deactivated = false;
        p[9].deactivated = false;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    RINGOUT
}

pub struct Distortion {
    sr: f32,
    pre: Biquad,
    post: Biquad,
    lp_pre: Biquad,
    lp_post: Biquad,
    hb1: HalfBand,
    hb2: HalfBand,
    drive: Line,
    gain: Line,
    fb: [f32; 2],
    sh: [Shaper; 2],
    block: u32,
    silent: i32,
    count: u32,
}

impl Distortion {
    pub fn new(sr: f32) -> Distortion {
        Distortion {
            sr,
            pre: Biquad::new(),
            post: Biquad::new(),
            lp_pre: Biquad::new(),
            lp_post: Biquad::new(),
            hb1: HalfBand::new(6, false),
            hb2: HalfBand::new(6, true),
            drive: Line::new(),
            gain: Line::new(),
            fb: [0.0; 2],
            sh: [Shaper::default(); 2],
            block: 0,
            silent: 0,
            count: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Distortion::new(self.sr);
    }

    fn control(&mut self, p: &[SlotValue; 12]) {
        if self.block % 8 == 0 {
            let sr = self.sr;
            self.pre.peak_eq(calc_omega(oct(p[1].v), sr), p[2].v as f64, ext(&p[0], |v| 3.0 * v) as f64);
            self.post.peak_eq(calc_omega(oct(p[7].v), sr), p[8].v as f64, ext(&p[6], |v| 3.0 * v) as f64);
            // High cuts run at 4x: the -2 octaves compensate; no coefficient lag.
            self.lp_pre.lp2b(calc_omega(oct(p[3].v) - 2.0, sr), 0.707);
            self.lp_pre.instantize();
            self.lp_post.lp2b(calc_omega(oct(p[9].v) - 2.0, sr), 0.707);
            self.lp_post.instantize();
        }
        self.block = self.block.wrapping_add(1);
        self.drive.set_target_smoothed(db_to_linear(ext(&p[4], |v| 5.0 * v)));
        let fade = if self.silent > RINGOUT - FADE { ((RINGOUT - self.silent - 1) as f32 / FADE as f32).clamp(0.0, 1.0) } else { 1.0 };
        self.gain.set_target_smoothed(db_to_linear(p[10].v) * fade);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let loud = l.iter().chain(r.iter()).any(|x| x.abs() > SILENCE);
        if loud {
            self.silent = 0;
        } else {
            self.silent += 1;
        }
        self.control(p);

        let m = p[11].i();
        let model = if (0..8).contains(&m) { m as usize } else { 0 };
        let ws = FX_SHAPER_SUBLIST[model];
        let fb = p[5].v;
        let pre_on = !p[3].deactivated;
        let post_on = !p[9].deactivated;

        self.pre.process(l, r);

        let d0 = self.drive.cur;
        let dd = (self.drive.target - self.drive.cur) / QUIRK_DRIVE_STEP_DIV;
        let mut dnow = d0;
        let dc = if model >= 3 { shape_static(ws, 0.0, d0) } else { 0.0 };

        let mut ol = [0f32; 128];
        let mut or = [0f32; 128];
        for k in 0..B {
            let g = self.drive.at(k);
            let inp = [l[k] * g, r[k] * g];
            let off = if (self.count / 16) % 2 == 0 { 1e-8 } else { -1e-8 };
            self.count = self.count.wrapping_add(1);
            for j in 0..4 {
                let mut s = [inp[0] + fb * self.fb[0], inp[1] + fb * self.fb[1]];
                if pre_on {
                    let (a, b) = self.lp_pre.tick(s[0], s[1]);
                    s = [a, b];
                }
                for c in 0..2 {
                    let y = if model < 3 {
                        lookup_waveshape(ws, s[c])
                    } else {
                        // Digital (model 4) takes s unscaled (QUIRK_DIGITAL_INPUT_UNSCALED).
                        let x = if model == 4 { s[c] } else { s[c] / dnow };
                        self.sh[c].tick(ws, x, dnow) - dc
                    };
                    self.fb[c] = y;
                    s[c] = y + off;
                }
                if post_on {
                    let (a, b) = self.lp_post.tick(s[0], s[1]);
                    s = [a, b];
                }
                ol[4 * k + j] = s[0];
                or[4 * k + j] = s[1];
                dnow += dd;
            }
        }
        self.hb1.decimate(&mut ol, &mut or);
        self.hb2.decimate(&mut ol[..64], &mut or[..64]);
        for k in 0..B {
            let g = self.gain.at(k);
            l[k] = ol[k] * g;
            r[k] = or[k] * g;
        }
        self.post.process(l, r);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.silent += 1;
        self.control(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Rng;

    #[test]
    fn loud_noise_finite_and_bounded_every_model() {
        for model in 0..8 {
            let mut p = super::super::default_values(5);
            p[11].v = model as f32;
            p[4].v = 24.0;
            p[5].v = 0.5;
            let ctx = super::super::FxCtx::new(48000.0);
            let mut fx = Distortion::new(48000.0);
            let mut rng = Rng::new(7);
            for _ in 0..300 {
                let mut l = [0f32; 32];
                let mut r = [0f32; 32];
                for k in 0..32 {
                    l[k] = 4.0 * rng.bipolar();
                    r[k] = 4.0 * rng.bipolar();
                }
                fx.process(&p, &ctx, &mut l, &mut r);
                for k in 0..32 {
                    assert!(l[k].is_finite() && r[k].is_finite(), "model {model}");
                    // Digital is a quantiser (unbounded by design); the
                    // Asymmetric table reaches about -22 at its low end.
                    if model != 4 {
                        assert!(l[k].abs() < 50.0 && r[k].abs() < 50.0, "model {model} {} {}", l[k], r[k]);
                    }
                }
            }
        }
    }

    #[test]
    fn fades_out_at_end_of_ringout() {
        let p = super::super::default_values(5);
        let ctx = super::super::FxCtx::new(48000.0);
        let mut fx = Distortion::new(48000.0);
        for _ in 0..RINGOUT - 2 {
            fx.process_only_control(&p, &ctx);
        }
        assert!(fx.gain.target < 0.05, "{}", fx.gain.target);
    }
}
