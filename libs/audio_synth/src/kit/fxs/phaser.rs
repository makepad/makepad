//! Phaser (type 3): up to 16 allpass stages per channel in a feedback loop,
//! their centres spread over the spectrum and swept by the effects' mod LFO
//! (control tick every 8 blocks), an optional tone tilt, width and mix.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotValue};

const MAX_STAGES: usize = 16;
/// Blocks per control tick.
const TICK: u32 = 8;

const WAVES: [&str; 7] = ["Sine", "Triangle", "Ramp", "Saw", "Square", "Noise", "Sample & Hold"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        bipolar("Center", 0.0),
        bipolar("Feedback", 0.0),
        bipolar("Sharpness", 0.0),
        about(deact(lforate("Rate", 0.0)), "deactivated: the LFO is frozen and the rate knob scrubs its phase"),
        percent("Depth", 0.0),
        percent("Stereo", 0.0),
        percent("Mix", 0.5),
        width_db("Width"),
        about(int("Count", 1.0, 16.0, 4.0), "1 = the legacy single-stage mode"),
        percent("Spread", 0.0),
        choice("Waveform", &WAVES, 0.0),
        deact(bipolar("Tone", 0.0)),
    ]
}

/// SPEC? the default deactivation state of Tone is not given; deactivated
/// is the neutral choice.
pub fn init_defaults(p: &mut [SlotValue; 12]) {
    p[11].deactivated = true;
}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 13 {
        p[8] = SlotValue::int(4);
        p[7].v = 0.0;
    }
    if rev <= 15 {
        p[10] = SlotValue::int(1);
        p[3].deactivated = false;
    }
    if rev <= 17 {
        p[11].v = 0.0;
        p[11].deactivated = true;
    }
    if rev < 30 {
        // Old wave order: 0 Sine, 1 Tri, 2 Ramp, 3 Noise, 4 S&H, 5 Square.
        match p[10].i() {
            3 => {
                p[10] = SlotValue::int(5);
                p[5].v = 1.0;
            }
            4 => {
                p[10] = SlotValue::int(6);
                p[5].v = 1.0;
            }
            5 => p[10] = SlotValue::int(4),
            _ => {}
        }
    }
}

pub fn ringout_blocks(p: &[SlotValue; 12], _sr: f32) -> i32 {
    let fb = p[1].v.abs();
    if fb > 0.9 {
        5000
    } else if fb > 0.5 {
        3000
    } else {
        1000
    }
}

pub struct Phaser {
    sr: f32,
    lfo: ModLfo,
    ap: [[Biquad; MAX_STAGES]; 2],
    lp: Biquad,
    hp: Biquad,
    fb: f32,
    fb_inc: f32,
    d: [f32; 2],
    n: usize,
    blk: u32,
    width: Line,
    mix: Line,
}

impl Phaser {
    pub fn new(sr: f32) -> Self {
        Phaser {
            sr,
            lfo: ModLfo::new(0x9ba5e),
            ap: [[Biquad::new(); MAX_STAGES]; 2],
            lp: Biquad::new(),
            hp: Biquad::new(),
            fb: 0.0,
            fb_inc: 0.0,
            d: [0.0; 2],
            n: 4,
            blk: 0,
            width: Line::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.lfo.reset();
        for ch in &mut self.ap {
            for s in ch.iter_mut() {
                s.suspend();
            }
        }
        self.lp.suspend();
        self.hp.suspend();
        self.fb = 0.0;
        self.fb_inc = 0.0;
        self.d = [0.0; 2];
        self.blk = 0;
        self.width = Line::new();
        self.mix = Line::new();
    }

    fn control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        if self.blk % TICK == 0 {
            let rate = &p[3];
            let (inc, offset) = if rate.deactivated {
                (0.0, ((rate.v + 7.0) / 16.0).clamp(0.0, 1.0))
            } else {
                (rate_linear(-rate.v, self.sr) * tsr_if(rate, ctx.tsr) * TICK as f32, 0.0)
            };
            self.lfo.tick(p[10].i().clamp(0, 6), inc, p[4].v.clamp(0.0, 2.0), offset, clamp01(p[5].v));
            let lv = [self.lfo.value(0), self.lfo.value(1)];
            self.n = p[8].i().clamp(1, MAX_STAGES as i32) as usize;
            let q = 1.0 + 0.8 * p[2].v as f64;
            let n = self.n;
            for ch in 0..2 {
                for i in 0..n {
                    let arg = if n == 1 {
                        2.0 * p[0].v + 1.5 / 12.0 + 2.0 * lv[ch]
                    } else {
                        let centre = (2.0 * (i + 1) as f32 / n as f32).exp2();
                        2.0 * p[0].v + p[9].v * centre + (2.0 / (i + 1) as f32) * lv[ch]
                    };
                    self.ap[ch][i].apf(calc_omega(arg, self.sr), q);
                }
            }
            self.fb_inc = (0.95 * p[1].v - self.fb) / (TICK as usize * B) as f32;
            if !p[11].deactivated {
                let (lpc, hpc) = tone_cuts(p[11].v);
                self.lp.lp(calc_omega(lpc / 12.0 - 2.0, self.sr), 0.707);
                self.hp.hp(calc_omega(hpc / 12.0 - 2.0, self.sr), 0.707);
            }
        }
        self.blk = self.blk.wrapping_add(1);
    }

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.control(p, ctx);
        let mut wl = *l;
        let mut wr = *r;
        let n = self.n;
        for k in 0..B {
            self.fb += self.fb_inc;
            for ch in 0..2 {
                let x = if ch == 0 { wl[k] } else { wr[k] };
                let mut d = (x + self.d[ch] * self.fb).clamp(-32.0, 32.0);
                for s in self.ap[ch][..n].iter_mut() {
                    d = s.tick_mono(d);
                }
                self.d[ch] = d;
                if ch == 0 {
                    wl[k] = d;
                } else {
                    wr[k] = d;
                }
            }
        }
        if !p[11].deactivated {
            self.lp.process(&mut wl, &mut wr);
            self.hp.process(&mut wl, &mut wr);
        }
        self.width.set_target_smoothed(db_to_linear(p[7].v));
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[6].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
        self.control(p, ctx);
        self.fb += self.fb_inc * B as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_zero_no_feedback_is_allpass() {
        let mut p = super::super::default_values(3);
        p[6].v = 1.0;
        let ctx = FxCtx::new(48000.0);
        for hz in [100.0f64, 440.0, 1000.0, 5000.0] {
            let mut fx = Phaser::new(48000.0);
            let (mut ein, mut eout) = (0.0f64, 0.0f64);
            let blocks = 3000;
            for b in 0..blocks {
                let mut l = [0f32; 32];
                for k in 0..32 {
                    let n = (b * 32 + k) as f64;
                    l[k] = (0.5 * (2.0 * std::f64::consts::PI * hz * n / 48000.0).sin()) as f32;
                }
                let input = l;
                let mut r = l;
                fx.process(&p, &ctx, &mut l, &mut r);
                if b >= blocks / 2 {
                    for k in 0..32 {
                        ein += (input[k] as f64).powi(2);
                        eout += (l[k] as f64).powi(2);
                    }
                }
            }
            let ddb = 10.0 * (eout / ein).log10();
            assert!(ddb.abs() < 0.05, "{hz} Hz: {ddb} dB");
        }
    }

    #[test]
    fn migration_maps_old_waves() {
        let mut p = super::super::default_values(3);
        p[10] = SlotValue::int(3);
        migrate(29, &mut p);
        assert_eq!(p[10].i(), 5);
        assert_eq!(p[5].v, 1.0);
    }
}
