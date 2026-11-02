//! Band-limited steps and ramps as tables: the difference between a
//! windowed-sinc step (and its integral) and the ideal one, for an event at
//! any fraction of a sample. An oscillator writes its plain waveform and
//! adds these around each discontinuity through a short delay line
//! ([`BandLine`]), so any number of events per sample costs only their taps.

use std::sync::LazyLock;

/// Half the taps: the line delays its output by this many samples.
pub const HALF: usize = 12;
pub const TAPS: usize = 2 * HALF;
/// Rows per sample of event position.
const ROWS: usize = 64;
/// Kaiser window shape (about -80 dB side lobes).
const BETA: f64 = 8.0;
/// The band's edge over Nyquist: the transition sits so that what folds below
/// 20 kHz at 48 kHz is already in the stop band.
const EDGE: f64 = 0.92;

pub struct Tables {
    step: [[f32; TAPS]; ROWS + 1],
    ramp: [[f32; TAPS]; ROWS + 1],
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > sum * 1e-12 {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

fn build() -> Tables {
    // The band-limited impulse on a fine grid, and its two integrals.
    let per = 512usize;
    let n = 2 * HALF * per;
    let h = 1.0 / per as f64;
    let w = HALF as f64;
    let impulse = |t: f64| -> f64 {
        let r = t / w;
        if r.abs() >= 1.0 {
            return 0.0;
        }
        let x = std::f64::consts::PI * EDGE * t;
        let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
        sinc * bessel_i0(BETA * (1.0 - r * r).sqrt()) / bessel_i0(BETA)
    };
    let mut s = vec![0.0f64; n + 1];
    for i in 0..n {
        let t = -w + i as f64 * h;
        // Simpson over one grid step.
        s[i + 1] = s[i] + (impulse(t) + 4.0 * impulse(t + h / 2.0) + impulse(t + h)) * h / 6.0;
    }
    let total = s[n];
    for v in s.iter_mut() {
        *v /= total;
    }
    let mut a = vec![0.0f64; n + 1];
    for i in 0..n {
        a[i + 1] = a[i] + (s[i] + s[i + 1]) * h / 2.0;
    }
    let mut t = Tables { step: [[0.0; TAPS]; ROWS + 1], ramp: [[0.0; TAPS]; ROWS + 1] };
    let scale = per / ROWS;
    for r in 0..=ROWS {
        let d = r as f64 / ROWS as f64;
        for k in 0..TAPS {
            let i = (k * per + r * scale).min(n);
            let tau = k as f64 - w + d;
            // At or after the event (a tap's own half), the ideal step has happened.
            let after = k >= HALF;
            t.step[r][k] = (s[i] - if after { 1.0 } else { 0.0 }) as f32;
            t.ramp[r][k] = (a[i] - if after { tau } else { 0.0 }) as f32;
        }
    }
    t
}

static TABLES: LazyLock<Tables> = LazyLock::new(build);

/// Build the tables now (off the audio thread).
pub fn ready() {
    LazyLock::force(&TABLES);
}

/// The delay line an oscillator's plain signal and corrections go through.
#[derive(Clone, Copy, Debug)]
pub struct BandLine {
    line: [f32; TAPS],
    at: usize,
}

impl Default for BandLine {
    fn default() -> Self {
        BandLine { line: [0.0; TAPS], at: 0 }
    }
}

impl BandLine {
    pub fn clear(&mut self) {
        *self = BandLine::default();
    }

    /// A step of `jump` and a slope change of `kink` (per sample) that
    /// happened `ago` samples (0..=1) before the sample being written.
    #[inline]
    pub fn event(&mut self, jump: f32, kink: f32, ago: f32) {
        let x = ago.clamp(0.0, 1.0) * ROWS as f32;
        let row = (x as usize).min(ROWS - 1);
        let f = x - row as f32;
        let t = &*TABLES;
        let (s0, s1, r0, r1) = (&t.step[row], &t.step[row + 1], &t.ramp[row], &t.ramp[row + 1]);
        let mut at = self.at;
        for k in 0..TAPS {
            let step = s0[k] + (s1[k] - s0[k]) * f;
            let ramp = r0[k] + (r1[k] - r0[k]) * f;
            self.line[at] += jump * step + kink * ramp;
            at += 1;
            if at == TAPS {
                at = 0;
            }
        }
    }

    /// Write this sample's plain value; read the delayed, corrected output.
    #[inline]
    pub fn push(&mut self, plain: f32) -> f32 {
        let write = if self.at + HALF >= TAPS { self.at + HALF - TAPS } else { self.at + HALF };
        self.line[write] += plain;
        let out = self.line[self.at];
        self.line[self.at] = 0.0;
        self.at = if self.at + 1 == TAPS { 0 } else { self.at + 1 };
        out
    }
}
