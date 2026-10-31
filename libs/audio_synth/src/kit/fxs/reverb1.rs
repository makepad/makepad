//! Reverb 1 (type 2): a 16-line feedback delay network with uniform
//! (Householder-style) mixing, damped taps, a mono pre-delay, per-line
//! stereo pans, and a post EQ (low cut, 2-octave peak, high cut) on the
//! wet signal. Four room shapes give four sets of line lengths.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{calc_omega, db_to_linear, Biquad, SlotDef, SlotUnit, SlotValue};

/// The room's line lengths are fixed sample counts: the reference does not
/// scale them by the sample rate, so the room gets smaller at higher rates.
const QUIRK_ROOM_NOT_RATE_SCALED: bool = true;
const _: () = assert!(QUIRK_ROOM_NOT_RATE_SCALED);
/// A synced pre-delay is multiplied by tempo/120 (not divided) in the
/// reference; mirrored so synced patches keep their pre-delay.
const QUIRK_PREDELAY_SYNC_TIMES_TSR: bool = true;
const _: () = assert!(QUIRK_PREDELAY_SYNC_TIMES_TSR);
const LINE: usize = 1 << 15;
const MASK: usize = LINE - 1;

/// Line lengths per shape, fixed point with 8 fractional bits.
const SHAPES: [[i32; 16]; 4] = [
    [1339934, 962710, 1004427, 1103966, 1198575, 1743348, 1033425, 933313, 949407, 1402754, 1379894, 1225304, 1135598, 1402107, 956152, 1137737],
    [1265607, 844703, 856159, 1406425, 786608, 1163557, 1091206, 1129434, 1270379, 896997, 1415393, 782808, 868582, 1234463, 1000336, 968299],
    [1293101, 1334867, 1178781, 1850949, 1663760, 1982922, 1211021, 1824481, 1520266, 1351822, 1102711, 1513696, 1057618, 1671799, 1406360, 1170468],
    [1833435, 2462309, 2711583, 2219764, 1664194, 2109157, 1626137, 1434473, 2271242, 1621375, 1831218, 2640903, 1577737, 1871624, 2439164, 1427343],
];

const SHAPE_NAMES: &[&str] = &["Type 1", "Type 2", "Type 3", "Type 4"];

pub fn slot_defs() -> [SlotDef; 12] {
    [
        envtime("Pre-Delay", -8.0, 5.0, -4.0),
        choice("Room Shape", SHAPE_NAMES, 0.0),
        percent("Size", 0.5),
        fl("Decay Time", -4.0, 6.0, 1.0, SlotUnit::Log2Secs),
        percent("HF Damping", 0.2),
        freq_d("Low Cut", -24.0),
        freq("Peak Freq", 0.0),
        db("Peak Gain", -48.0, 48.0, 0.0),
        // The reference's default 72 lies above the 70 maximum and is clamped
        // to 70 on load; both are above Nyquist at 48 kHz (a pass-through).
        freq_d("High Cut", 70.0),
        percent("Mix", 0.5),
        width_db("Width"),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 15 {
        p[5].deactivated = false;
        p[8].deactivated = false;
    }
}

/// Fixed-point lengths x 2 Size, not scaled by the sample rate
/// (QUIRK_ROOM_NOT_RATE_SCALED).
fn line_lengths(shape: usize, size: f32) -> [i32; 16] {
    let mut l = [0i32; 16];
    for t in 0..16 {
        l[t] = (SHAPES[shape][t] as f32 * 2.0 * size) as i32;
    }
    l
}

/// The -120 dB time plus the longest line, in blocks.
pub fn ringout_blocks(p: &[SlotValue; 12], sr: f32) -> i32 {
    let shape = p[1].i().clamp(0, 3) as usize;
    let max = line_lengths(shape, p[2].v).iter().copied().max().unwrap_or(0);
    ((max as f32 / 256.0 + sr * p[3].v.exp2() * 2.0) / B as f32) as i32
}

pub struct Reverb1 {
    sr: f32,
    lines: Vec<f32>,
    pre: Vec<f32>,
    pos: usize,
    o: [f32; 16],
    len: [i32; 16],
    g: [f32; 16],
    pan_l: [f32; 16],
    pan_r: [f32; 16],
    shape: i32,
    size: f32,
    decay: f32,
    counter: u32,
    first: bool,
    hp: Biquad,
    peak: Biquad,
    lp: Biquad,
    mix: Line,
    width: Line,
}

impl Reverb1 {
    pub fn new(sr: f32) -> Reverb1 {
        let mut pan_l = [0f32; 16];
        let mut pan_r = [0f32; 16];
        for t in 0..16 {
            let xb = 2.0 * (t as f32 / 15.0) - 1.0;
            pan_l[t] = (0.5 - 0.495 * xb).sqrt();
            pan_r[t] = (0.5 + 0.495 * xb).sqrt();
        }
        Reverb1 {
            sr,
            lines: vec![0.0; 16 * LINE],
            pre: vec![0.0; LINE],
            pos: 0,
            o: [0.0; 16],
            len: [0; 16],
            g: [0.0; 16],
            pan_l,
            pan_r,
            shape: -1,
            size: 0.0,
            decay: f32::NAN,
            counter: 0,
            first: true,
            hp: Biquad::new(),
            peak: Biquad::new(),
            lp: Biquad::new(),
            mix: Line::new(),
            width: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.clear();
        self.pre.iter_mut().for_each(|s| *s = 0.0);
        self.pos = 0;
        self.shape = -1;
        self.decay = f32::NAN;
        self.counter = 0;
        self.first = true;
        self.hp = Biquad::new();
        self.peak = Biquad::new();
        self.lp = Biquad::new();
        self.mix = Line::new();
        self.width = Line::new();
    }

    fn clear(&mut self) {
        self.lines.iter_mut().for_each(|s| *s = 0.0);
        self.o = [0.0; 16];
    }

    fn set_gains(&mut self) {
        let t60 = self.sr * self.decay.exp2();
        for t in 0..16 {
            self.g[t] = 0.001f32.powf(self.len[t] as f32 / (256.0 * t60));
        }
    }

    fn update_filters(&mut self, p: &[SlotValue; 12]) {
        let sr = self.sr;
        self.hp.hp(calc_omega(p[5].v / 12.0, sr), 0.5);
        self.peak.peak_eq(calc_omega(p[6].v / 12.0, sr), 2.0, p[7].v as f64);
        self.lp.lp2b(calc_omega(p[8].v / 12.0, sr), 0.5);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        let sr = self.sr;
        let shape = p[1].i().clamp(0, 3);
        let mut lens_changed = false;
        if shape != self.shape {
            self.shape = shape;
            self.size = p[2].v;
            self.clear();
            lens_changed = true;
        } else if self.counter % 32 == 0 && (p[2].v - self.size).abs() > 0.001 {
            self.size = p[2].v;
            lens_changed = true;
        }
        if lens_changed {
            self.len = line_lengths(shape as usize, self.size);
        }
        if lens_changed || p[3].v != self.decay {
            self.decay = p[3].v;
            self.set_gains();
        }
        if self.first || self.counter % 32 == 0 {
            self.update_filters(p);
        }
        self.first = false;
        self.counter = self.counter.wrapping_add(1);

        let sync = tsr_if(&p[0], ctx.tsr);
        let pd = (sr * p[0].v.exp2() * sync) as usize;
        let d = p[4].v.clamp(0.01, 0.99);
        let mut off = [0usize; 16];
        for t in 0..16 {
            off[t] = (self.len[t] / 256).max(0) as usize;
        }

        let mut wl = [0f32; B];
        let mut wr = [0f32; B];
        for k in 0..B {
            let mut sum = 0.0;
            for t in 0..16 {
                let rv = self.lines[t * LINE + (self.pos.wrapping_sub(off[t]) & MASK)];
                self.o[t] = d * self.o[t] + (1.0 - d) * rv;
                sum += self.o[t];
            }
            let s = (-2.0 / 16.0) * sum + self.pre[self.pos.wrapping_sub(pd) & MASK];
            self.pos = (self.pos + 1) & MASK;
            self.pre[self.pos] = 0.5 * (l[k] + r[k]);
            let (mut a, mut b) = (0.0, 0.0);
            for t in 0..16 {
                self.lines[t * LINE + self.pos] = self.g[t] * (s + self.o[t]);
                a += self.o[t] * self.pan_l[t];
                b += self.o[t] * self.pan_r[t];
            }
            wl[k] = a;
            wr[k] = b;
        }

        if !p[5].deactivated {
            self.hp.process(&mut wl, &mut wr);
        }
        self.peak.process(&mut wl, &mut wr);
        if !p[8].deactivated {
            self.lp.process(&mut wl, &mut wr);
        }
        self.width.set_target_smoothed(db_to_linear(p[10].v));
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[9].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    fn render(fx: &mut Reverb1, p: &[SlotValue; 12], input: &[f32]) -> (Vec<f32>, Vec<f32>) {
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

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn tail_decays_window_by_window() {
        let mut p = default_values(2);
        p[9].v = 1.0;
        let mut fx = Reverb1::new(48000.0);
        let mut rng = Rng::new(3);
        let n = 48000 * 4;
        let x: Vec<f32> = (0..n).map(|i| if i < 4800 { 0.5 * rng.bipolar() } else { 0.0 }).collect();
        let (l, r) = render(&mut fx, &p, &x);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        let w = 12000;
        let mut prev = f32::MAX;
        for i in 2..(n / w) {
            let e = rms(&l[i * w..(i + 1) * w]);
            assert!(e < prev, "window {i}: {e} >= {prev}");
            prev = e;
        }
        // 2 s decay time: about -60 dB over 2 s (damping and EQ make it faster).
        let drop = 20.0 * (rms(&l[24000..36000]) / rms(&l[120000..132000])).log10();
        assert!(drop > 20.0, "drop {drop} dB");
    }

    #[test]
    fn line_gains_reach_minus_60_db_at_the_decay_time() {
        let p = default_values(2);
        let mut fx = Reverb1::new(48000.0);
        let mut l = [0f32; B];
        let mut r = [0f32; B];
        fx.process(&p, &FxCtx::new(48000.0), &mut l, &mut r);
        for t in 0..16 {
            let passes = 48000.0 * 2.0 / (fx.len[t] as f32 / 256.0);
            let db = 20.0 * fx.g[t].log10() * passes;
            assert!((db + 60.0).abs() < 0.01, "{t}: {db}");
        }
    }

    #[test]
    fn ringout_covers_the_minus_120_db_time() {
        let p = default_values(2);
        let n = ringout_blocks(&p, 48000.0);
        assert!(n as f32 * B as f32 >= 48000.0 * 4.0);
    }
    #[test]
    fn high_cut_default_is_clamped_to_its_maximum() {
        let d = slot_defs()[8];
        assert_eq!((d.default, d.max), (70.0, 70.0));
        assert_eq!(default_values(super::super::TYPE_NAMES.iter().position(|n| *n == "Reverb 1").unwrap() as u8)[8].v, 70.0);
    }
}
