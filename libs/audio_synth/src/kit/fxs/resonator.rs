//! Resonator (type 17): three parallel resonant filters (low, mid, high
//! band) borrowed from the voice filter set, run 2x oversampled with an
//! asymmetric pre-shaper and a soft clip after every band is summed.

use super::common::*;
use crate::kit::flt::FilterUnit;
use super::FxCtx;
use crate::kit::core::{amp_to_linear, db_to_linear, HalfBand, Lag, SlotDef, SlotUnit, SlotValue};

/// Band ranges (semitones vs A440): 60..300, 300..1500, 1500..7500 Hz.
const BANDS: [(f32, f32); 3] = [(-34.4936, -6.6304), (-6.6304, 21.2342), (21.2342, 49.0984)];
/// The extended range of every band: 60..7500 Hz.
const FULL: (f32, f32) = (-34.4936, 49.0984);

pub fn slot_defs() -> [SlotDef; 12] {
    let band = |name, i: usize, def| extend(fl(name, BANDS[i].0, BANDS[i].1, def, SlotUnit::NoteA440));
    let res = |name| about(extend(percent(name, 0.75)), "extended: no clamping of the resonance");
    let gain = |name| fl(name, 0.0, 1.0, 0.7937, SlotUnit::Amp);
    [
        about(band("Frequency 1", 0, -18.6305), "extended: 60..7500 Hz"),
        res("Resonance 1"),
        gain("Gain 1"),
        about(band("Frequency 2", 1, 8.038216), "extended: 60..7500 Hz"),
        res("Resonance 2"),
        gain("Gain 2"),
        about(band("Frequency 3", 2, 35.90135), "extended: 60..7500 Hz"),
        res("Resonance 3"),
        gain("Gain 3"),
        choice("Mode", &["Lowpass", "Bandpass", "Bandpass+Notch", "Highpass"], 1.0),
        db("Gain", -48.0, 48.0, 0.0),
        percent("Mix", 1.0),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

pub struct Resonator {
    sr: f32,
    up: HalfBand,
    down: HalfBand,
    units: [[FilterUnit; 3]; 2],
    cut: [Lag; 3],
    res: [Lag; 3],
    gain: [Lag; 3],
    out_gain: Line,
    mix: Line,
}

impl Resonator {
    pub fn new(sr: f32) -> Self {
        Resonator {
            sr,
            up: HalfBand::new(12, true),
            down: HalfBand::new(12, true),
            units: Default::default(),
            cut: [Lag::new(0.004); 3],
            res: [Lag::new(0.004); 3],
            gain: [Lag::new(0.004); 3],
            out_gain: Line::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        self.up = HalfBand::new(12, true);
        self.down = HalfBand::new(12, true);
        self.units.iter_mut().flatten().for_each(|u| u.reset());
        self.cut = [Lag::new(0.004); 3];
        self.res = [Lag::new(0.004); 3];
        self.gain = [Lag::new(0.004); 3];
        self.out_gain = Line::new();
        self.mix = Line::new();
    }

    fn targets(&mut self, p: &[SlotValue; 12]) {
        for i in 0..3 {
            let f = &p[3 * i];
            let (lo, hi) = if f.extend { FULL } else { BANDS[i] };
            self.cut[i].set(f.v.clamp(lo, hi));
            let r = &p[3 * i + 1];
            self.res[i].set(if r.extend { r.v } else { clamp01(r.v) });
            self.gain[i].set(amp_to_linear(p[3 * i + 2].v));
        }
    }

    pub fn process(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.targets(p);
        let mode = p[9].i().clamp(0, 3);
        // Resonance scaled down to keep the bands from self-oscillating.
        let (ty, sub, rs) = match mode {
            0 => (1u8, 1u8, 0.75f32),
            3 => (4, 1, 0.75),
            _ => (6, 3, 0.9),
        };
        let fs = 2.0 * self.sr;
        for ch in 0..2 {
            for i in 0..3 {
                self.units[ch][i].set_block(ty, sub, self.cut[i].v, self.res[i].v * rs, fs, true);
            }
        }
        let mut ol = [0f32; 64];
        let mut or = [0f32; 64];
        // SPEC? the spec gives no gain compensation for the interpolator
        // here (the Waveshaper says the reference's one halves the level).
        self.up.upsample_into(&l[..], &r[..], &mut ol, &mut or, 0.5);
        for k in 0..B {
            let mut g = [0f32; 3];
            for i in 0..3 {
                self.cut[i].process();
                self.res[i].process();
                g[i] = self.gain[i].process();
            }
            for j in [2 * k, 2 * k + 1] {
                for ch in 0..2 {
                    let buf = if ch == 0 { &mut ol } else { &mut or };
                    let x = lookup_waveshape(WS_ASYM, buf[j]);
                    let mut acc = 0.0;
                    for i in 0..3 {
                        let y = self.units[ch][i].tick(x) * g[i];
                        if mode == 2 && i == 1 {
                            acc -= y;
                        } else {
                            acc += y;
                        }
                        acc = lookup_waveshape(WS_SOFT, acc);
                    }
                    buf[j] = acc;
                }
            }
        }
        let mut wl = [0f32; 32];
        let mut wr = [0f32; 32];
        self.down.decimate_into(&ol, &or, &mut wl, &mut wr);
        self.out_gain.set_target_smoothed(db_to_linear(p[10].v));
        self.out_gain.multiply_2_blocks(&mut wl, &mut wr);
        self.mix.set_target_smoothed(p[11].v.clamp(-1.0, 1.0));
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, p: &[SlotValue; 12], _ctx: &FxCtx) {
        self.targets(p);
        for i in 0..3 {
            for _ in 0..B {
                self.cut[i].process();
                self.res[i].process();
                self.gain[i].process();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(hz: f64) -> f64 {
        let p = super::super::default_values(17);
        let ctx = FxCtx::new(48000.0);
        let mut fx = Resonator::new(48000.0);
        let (mut sc, mut cc) = (0.0f64, 0.0f64);
        let blocks = 3000;
        for b in 0..blocks {
            let mut l = [0f32; 32];
            for k in 0..32 {
                let n = (b * 32 + k) as f64;
                l[k] = (0.01 * (2.0 * std::f64::consts::PI * hz * n / 48000.0).sin()) as f32;
            }
            let mut r = l;
            fx.process(&p, &ctx, &mut l, &mut r);
            if b >= blocks / 2 {
                for k in 0..32 {
                    let ph = 2.0 * std::f64::consts::PI * hz * (b * 32 + k) as f64 / 48000.0;
                    assert!(l[k].is_finite());
                    sc += l[k] as f64 * ph.sin();
                    cc += l[k] as f64 * ph.cos();
                }
            }
        }
        (sc * sc + cc * cc).sqrt()
    }

    #[test]
    fn bandpass_peaks_at_band_frequencies() {
        let peaks = [150.0, 700.0, 3500.0].map(gain_at);
        let dips = [60.0, 324.0, 1565.0, 9000.0].map(gain_at);
        assert!(peaks[0] > 2.0 * dips[0] && peaks[0] > 2.0 * dips[1], "{peaks:?} {dips:?}");
        assert!(peaks[1] > 2.0 * dips[1] && peaks[1] > 2.0 * dips[2], "{peaks:?} {dips:?}");
        assert!(peaks[2] > 2.0 * dips[2] && peaks[2] > 2.0 * dips[3], "{peaks:?} {dips:?}");
    }
}
