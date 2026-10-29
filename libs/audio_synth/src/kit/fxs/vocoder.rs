//! Vocoder (type 10): the slot's input is the carrier, the host's audio
//! input the modulator. Up to 20 bands of paired state-variable band-passes
//! on each side; each modulator band's envelope scales its carrier band
//! (the carrier is scaled before its band filter). EFFECTS.md 5.14.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{db_to_linear, SlotDef, SlotValue};

/// The carrier is multiplied by the envelope BEFORE its band filter.
const QUIRK_CARRIER_SCALED_BEFORE_FILTER: bool = true;

const MOD_MODES: &[&str] = &["Monosum", "Left Only", "Right Only", "Stereo"];
/// 12 log2(180 / 440) and 12 log2(7500 / 440).
const MIN_DEFAULT: f32 = -15.474079;
const MAX_DEFAULT: f32 = 49.095467;
const MAX_BANDS: usize = 20;

pub fn slot_defs() -> [SlotDef; 12] {
    [
        db("Gain", -48.0, 48.0, 0.0),
        db("Gate", -96.0, 0.0, -96.0),
        percent("Env Follow", 0.0),
        bipolar("Q", 0.0),
        int("Bands", 4.0, 20.0, 20.0),
        fl("Min Frequency", -36.0, 36.0, MIN_DEFAULT, crate::kit::core::SlotUnit::NoteA440),
        fl("Max Frequency", 0.0, 60.0, MAX_DEFAULT, crate::kit::core::SlotUnit::NoteA440),
        choice("Input", MOD_MODES, 0.0),
        bipolar("Range", 0.0),
        bipolar("Center", 0.0),
        percent("Mix", 1.0),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(rev: i32, p: &mut [SlotValue; 12]) {
    if rev <= 10 {
        p[4] = SlotValue::int(20);
        p[5].v = MIN_DEFAULT;
        p[6].v = MAX_DEFAULT;
        p[8].v = 0.0;
        p[9].v = 0.0;
        p[7] = SlotValue::int(0);
        p[10].v = 1.0;
    }
}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    500
}

/// Two cascaded Chamberlin band-passes (damping q), tuned apart.
#[derive(Clone, Copy, Default)]
struct BandPair {
    f: [f32; 2],
    q: f32,
    s: [[f32; 2]; 2],
}

impl BandPair {
    fn tune(&mut self, hz: f32, spread: f32, q: f32, sr: f32) {
        for (j, m) in [1.0 - spread, 1.0 + spread].into_iter().enumerate() {
            self.f[j] = 2.0 * (std::f32::consts::PI * (hz * m / sr).min(0.25)).sin();
        }
        self.q = q;
    }
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let mut u = x;
        for j in 0..2 {
            let f = self.f[j];
            let [low, band] = &mut self.s[j];
            *low += f * *band;
            let high = self.q * u - *low - self.q * *band;
            *band += f * high;
            u = *band;
        }
        u
    }
}

pub struct Vocoder {
    sr: f32,
    block: u32,
    n: usize,
    car: [[BandPair; MAX_BANDS]; 2],
    modf: [[BandPair; MAX_BANDS]; 2],
    env: [[f32; MAX_BANDS]; 2],
    gain: Line,
}

impl Vocoder {
    pub fn new(sr: f32) -> Vocoder {
        Vocoder {
            sr,
            block: 0,
            n: 20,
            car: [[BandPair::default(); MAX_BANDS]; 2],
            modf: [[BandPair::default(); MAX_BANDS]; 2],
            env: [[0.0; MAX_BANDS]; 2],
            gain: Line::new(),
        }
    }

    pub fn reset(&mut self) {
        *self = Vocoder::new(self.sr);
    }

    fn setup(&mut self, p: &[SlotValue; 12]) {
        let qq = 20.0 * (1.0 + 0.5 * p[3].v.clamp(-1.0, 1.0));
        let spread = 0.4 / qq;
        let q = 1.0 / qq;
        let n = ((p[4].i().clamp(4, 20) / 4) * 4) as usize;
        self.n = n;
        let mut flo = p[5].v.clamp(-36.0, 36.0);
        let mut fhi = p[6].v.clamp(0.0, 60.0);
        if flo > fhi {
            std::mem::swap(&mut flo, &mut fhi);
        }
        let d = (fhi - flo) / (n - 1) as f32;
        let (center, range) = (p[9].v, p[8].v);
        let half = (fhi - flo) / 2.0;
        let mid = half + flo + 0.3 * center * half;
        let m_lo = mid - half * (1.0 + 0.7 * range);
        let mut dm = 2.0 * half * (1.0 + 0.7 * range) / (n - 1) as f32;
        if m_lo + dm * (n - 1) as f32 > 60.0 {
            dm = (60.0 - m_lo) / (n - 1) as f32;
        }
        let same = center == 0.0 && range == 0.0;
        for k in 0..n {
            let fc = 440.0 * (flo / 12.0).exp2() * (k as f32 * d / 12.0).exp2();
            let fm = if same { fc } else { 440.0 * ((m_lo + k as f32 * dm) / 12.0).exp2() };
            for c in 0..2 {
                self.car[c][k].tune(fc, spread, q, self.sr);
                self.modf[c][k].tune(fm, spread, q, self.sr);
            }
        }
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        if self.block % 64 == 0 {
            self.setup(p);
        }
        self.block = self.block.wrapping_add(1);
        let rate = 0.001 * (4.0 * clamp01(p[2].v)).exp2();
        self.gain.set_target_smoothed(db_to_linear(p[0].v + 24.0));
        let g = db_to_linear(p[1].v + p[0].v + 24.0);
        let gate = g * g;
        let mode = p[7].i().clamp(0, 3);
        let mix = p[10].v;
        let zero = [0f32; 32];
        let (ml, mr) = ctx.audio_in.unwrap_or((&zero, &zero));
        let n = self.n;
        for k in 0..B {
            let gk = self.gain.at(k);
            let (m0, m1) = match mode {
                0 => ((ml[k] + mr[k]) * gk, 0.0),
                1 => (ml[k] * gk, 0.0),
                2 => (mr[k] * gk, 0.0),
                _ => (ml[k] * gk, mr[k] * gk),
            };
            let stereo = mode == 3;
            let (mut sl, mut sr) = (0f32, 0f32);
            for b in 0..n {
                let mut a = [0f32; 2];
                for c in 0..(if stereo { 2 } else { 1 }) {
                    let x = if c == 0 { m0 } else { m1 };
                    let m = self.modf[c][b].tick(x);
                    let mut m = (m * m).min(6.0);
                    if m < gate {
                        m = 0.0;
                    }
                    let e = &mut self.env[c][b];
                    *e = *e * (1.0 - rate) + rate * m;
                    a[c] = e.sqrt();
                }
                if !stereo {
                    a[1] = a[0];
                }
                let _ = QUIRK_CARRIER_SCALED_BEFORE_FILTER;
                sl += self.car[0][b].tick(l[k] * a[0]);
                sr += self.car[1][b].tick(r[k] * a[1]);
            }
            l[k] = l[k] * (1.0 - mix) + mix * 4.0 * sl;
            r[k] = r[k] * (1.0 - mix) + mix * 4.0 * sr;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{default_values, FxCtx};
    use super::*;
    use crate::dsp::Rng;

    /// Output RMS of a noise carrier with a sine modulator at `hz`, through
    /// a narrow measuring band-pass at `probe` Hz.
    fn band_energy(hz: f32, probe: f32) -> f32 {
        let p = default_values(10);
        let mut fx = Vocoder::new(48000.0);
        let mut rng = Rng::new(4);
        let mut meas = BandPair::default();
        meas.tune(probe, 0.0, 0.05, 48000.0);
        let mut e = 0.0;
        for b in 0..1500 {
            let mut ml = [0f32; 32];
            for k in 0..32 {
                ml[k] = 0.3 * (std::f32::consts::TAU * hz * (b * 32 + k) as f32 / 48000.0).sin();
            }
            let mr = ml;
            let mut ctx = FxCtx::new(48000.0);
            ctx.audio_in = Some((&ml, &mr));
            let mut l = [0f32; 32];
            let mut r = [0f32; 32];
            for k in 0..32 {
                l[k] = 0.3 * rng.bipolar();
                r[k] = l[k];
            }
            fx.process(&p, &ctx, &mut l, &mut r);
            for x in l.iter() {
                assert!(x.is_finite());
                let y = meas.tick(*x);
                if b > 500 {
                    e += y * y;
                }
            }
        }
        e
    }

    #[test]
    fn output_follows_the_modulator_band() {
        let at_500 = band_energy(500.0, 500.0);
        let at_4k = band_energy(500.0, 4000.0);
        assert!(at_500 > 10.0 * at_4k, "{at_500} {at_4k}");
        let at_4k_m = band_energy(4000.0, 4000.0);
        let at_500_m = band_energy(4000.0, 500.0);
        assert!(at_4k_m > 10.0 * at_500_m, "{at_4k_m} {at_500_m}");
    }

    #[test]
    fn silent_without_modulator() {
        let p = default_values(10);
        let ctx = FxCtx::new(48000.0);
        let mut fx = Vocoder::new(48000.0);
        let mut l = [0.5f32; 32];
        let mut r = [0.5f32; 32];
        fx.process(&p, &ctx, &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|x| x.abs() < 1e-9));
    }
}
