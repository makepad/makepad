//! The sound engine under every singer: a bank of phase-locked harmonics of
//! one pitch, each at the spectral envelope's level at its frequency (so the
//! formants stay where they are whatever the pitch), plus a bank of noise
//! bands, part of it pulsed with the glottal cycle. It renders fixed blocks
//! of [`BLOCK`] frames; every level ramps across a block, so no parameter
//! change can click. No allocation, no locks.

use crate::bands::*;

pub const BLOCK: usize = 64;
pub const MAX_HARMONICS: usize = 80;
/// The highest harmonic frequency (a voice has little above it; the noise
/// bands carry the air).
pub const HARMONIC_TOP_HZ: f32 = 11_000.0;

/// What one block sounds: the harmonic envelope and gain, the noise bands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub env: [f32; ENV_POINTS],
    /// Linear harmonic gain.
    pub harm: f32,
    /// Linear noise band gains: steady (`noise`) and pulsed with the
    /// glottal cycle (`pulsed`, the aperiodic part of voicing).
    pub noise: [f32; NOISE_BANDS],
    pub pulsed: [f32; NOISE_BANDS],
}

impl Frame {
    pub const SILENT: Frame = Frame { env: [FLOOR_DB; ENV_POINTS], harm: 0.0, noise: [0.0; NOISE_BANDS], pulsed: [0.0; NOISE_BANDS] };
}

#[derive(Clone, Copy)]
struct Band {
    b0: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
    norm: f32,
    rng: u32,
}

pub struct Voicer {
    rate: f32,
    /// Phase of the fundamental (radians, wrapped).
    phase: f64,
    amp: [f32; MAX_HARMONICS],
    count: usize,
    re: [f32; MAX_HARMONICS],
    im: [f32; MAX_HARMONICS],
    rr: [f32; MAX_HARMONICS],
    ri: [f32; MAX_HARMONICS],
    target: [f32; MAX_HARMONICS],
    bands: [Band; NOISE_BANDS],
    gn: [f32; NOISE_BANDS],
    gp: [f32; NOISE_BANDS],
}

impl Voicer {
    pub fn new(rate: f32, seed: u32) -> Voicer {
        let bands = std::array::from_fn(|b| {
            let fc = NOISE_CENTERS[b];
            let bw = noise_bandwidth(b);
            let mut band = Band { b0: 0.0, a1: 0.0, a2: 0.0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0, norm: 0.0, rng: 0x9E37_79B9 ^ seed.wrapping_mul(0x85EB_CA6B).wrapping_add(b as u32 * 0x2545_F491) | 1 };
            if fc < rate * 0.45 {
                // A band-pass of 0 dB peak gain, -3 dB at fc ± bw/2.
                let w0 = std::f32::consts::TAU * fc / rate;
                let q = fc / bw;
                let alpha = w0.sin() / (2.0 * q);
                let a0 = 1.0 + alpha;
                band.b0 = alpha / a0;
                band.a1 = -2.0 * w0.cos() / a0;
                band.a2 = (1.0 - alpha) / a0;
                // Its white-noise output RMS is sqrt(pi bw / rate): scale it
                // to one, so a band gain is the band's RMS.
                band.norm = (rate / (std::f32::consts::PI * bw)).sqrt();
            }
            band
        });
        Voicer {
            rate, phase: 0.0, amp: [0.0; MAX_HARMONICS], count: 0, re: [0.0; MAX_HARMONICS], im: [0.0; MAX_HARMONICS],
            rr: [0.0; MAX_HARMONICS], ri: [0.0; MAX_HARMONICS], target: [0.0; MAX_HARMONICS], bands, gn: [0.0; NOISE_BANDS], gp: [0.0; NOISE_BANDS],
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Silent and at rest (every level ramped to zero).
    pub fn is_silent(&self) -> bool {
        self.amp[..self.count].iter().all(|a| *a == 0.0) && self.gn.iter().chain(&self.gp).all(|g| *g == 0.0)
    }

    /// Render one block into `out` (overwritten). `f0` in Hz; the envelope
    /// is read at harmonic frequency / `formant_scale`; `gain` scales all;
    /// harmonic levels are corrected by sqrt(f0 / f0_ref) so a vowel keeps
    /// its loudness when its harmonics thin out at a higher pitch.
    pub fn render_block(&mut self, out: &mut [f32; BLOCK], f0: f32, frame: &Frame, formant_scale: f32, gain: f32, f0_ref: f32) {
        let f0 = f0.clamp(20.0, 2000.0);
        let top = HARMONIC_TOP_HZ.min(self.rate * 0.46);
        let count = ((top / f0) as usize).clamp(1, MAX_HARMONICS);
        let hgain = frame.harm * gain * (f0 / f0_ref.max(20.0)).sqrt();
        let inv_scale = 1.0 / formant_scale.max(0.5);
        for k in 0..MAX_HARMONICS {
            self.target[k] = if k < count && hgain > 0.0 { db_to_lin(env_at(&frame.env, (k + 1) as f32 * f0 * inv_scale)) * hgain } else { 0.0 };
        }
        let active = count.max(self.count);
        self.count = count;

        // Phase-locked harmonics: every one restarts from k times the
        // fundamental's phase, then turns at k times its rate.
        let w = std::f64::consts::TAU * f0 as f64 / self.rate as f64;
        let (s1, c1) = (self.phase.sin() as f32, self.phase.cos() as f32);
        let (rs, rc) = (w.sin() as f32, w.cos() as f32);
        let (mut zr, mut zi, mut qr, mut qi) = (1.0f32, 0.0f32, 1.0f32, 0.0f32);
        for k in 0..active {
            (zr, zi) = (zr * c1 - zi * s1, zr * s1 + zi * c1);
            (qr, qi) = (qr * rc - qi * rs, qr * rs + qi * rc);
            self.re[k] = zr;
            self.im[k] = zi;
            self.rr[k] = qr;
            self.ri[k] = qi;
        }
        let step = 1.0 / BLOCK as f32;
        let mut da = [0.0f32; MAX_HARMONICS];
        for k in 0..active {
            da[k] = (self.target[k] - self.amp[k]) * step;
        }
        let mut gn = self.gn;
        let mut gp = self.gp;
        let mut dn = [0.0f32; NOISE_BANDS];
        let mut dp = [0.0f32; NOISE_BANDS];
        for b in 0..NOISE_BANDS {
            let (tn, tp) = if self.bands[b].norm > 0.0 { (frame.noise[b] * gain, frame.pulsed[b] * gain) } else { (0.0, 0.0) };
            dn[b] = (tn - gn[b]) * step;
            dp[b] = (tp - gp[b]) * step;
        }
        let (re, im, rr, ri, amp) = (&mut self.re[..active], &mut self.im[..active], &self.rr[..active], &self.ri[..active], &mut self.amp[..active]);
        let da = &da[..active];
        for o in out.iter_mut() {
            let mut acc = 0.0f32;
            for k in 0..re.len() {
                let (r, i) = (re[k] * rr[k] - im[k] * ri[k], re[k] * ri[k] + im[k] * rr[k]);
                re[k] = r;
                im[k] = i;
                amp[k] += da[k];
                acc += amp[k] * i;
            }
            // The glottal pulse shape the aperiodic noise follows: peaks
            // once a period, mean power one.
            let c = re[0];
            let pulse = (0.35 + 0.65 * (0.5 + 0.5 * c) * (0.5 + 0.5 * c)) * (1.0 / 0.639_2);
            let (mut steady, mut pulsed) = (0.0f32, 0.0f32);
            for (b, band) in self.bands.iter_mut().enumerate() {
                gn[b] += dn[b];
                gp[b] += dp[b];
                if band.norm == 0.0 {
                    continue;
                }
                band.rng ^= band.rng << 13;
                band.rng ^= band.rng >> 17;
                band.rng ^= band.rng << 5;
                // Uniform, unit variance.
                let x = (band.rng as f32 * (1.0 / 4_294_967_296.0) - 0.5) * 3.464_101_6;
                let y = band.b0 * (x - band.x2) - band.a1 * band.y1 - band.a2 * band.y2;
                band.x2 = band.x1;
                band.x1 = x;
                band.y2 = band.y1;
                band.y1 = y;
                let y = y * band.norm;
                steady += gn[b] * y;
                pulsed += gp[b] * y;
            }
            *o = acc + steady + pulsed * pulse;
        }
        // Land exactly on the targets (no float creep), silence exactly.
        for k in 0..active {
            self.amp[k] = self.target[k];
        }
        for b in 0..NOISE_BANDS {
            self.gn[b] = if self.bands[b].norm > 0.0 { frame.noise[b] * gain } else { 0.0 };
            self.gp[b] = if self.bands[b].norm > 0.0 { frame.pulsed[b] * gain } else { 0.0 };
        }
        self.phase = (self.phase + w * BLOCK as f64) % std::f64::consts::TAU;
    }
}
