//! Master-bus dynamics: a glue compressor, a look-ahead peak limiter and a
//! ducker, all stereo-linked so the image never wanders.

use crate::dsp::{db_to_gain, finite, gain_to_db};

/// Feed-forward RMS-ish compressor with a soft knee.
#[derive(Clone, Copy, Debug)]
pub struct Compressor {
    pub threshold_db: f32,
    pub ratio: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
    attack: f32,
    release: f32,
    env_db: f32,
}

impl Compressor {
    pub fn new(rate: f32, threshold_db: f32, ratio: f32, attack_s: f32, release_s: f32) -> Self {
        let rate = finite(rate, 48_000.0).max(1.0);
        Compressor {
            threshold_db,
            ratio: ratio.max(1.0),
            knee_db: 6.0,
            makeup_db: 0.0,
            attack: 1.0 - (-1.0 / (attack_s.max(1.0e-4) * rate)).exp(),
            release: 1.0 - (-1.0 / (release_s.max(1.0e-4) * rate)).exp(),
            env_db: -120.0,
        }
    }

    /// Gain reduction (dB, ≤ 0) currently applied.
    pub fn reduction_db(&self) -> f32 {
        -self.curve(self.env_db).max(0.0)
    }

    #[inline]
    fn curve(&self, level_db: f32) -> f32 {
        let over = level_db - self.threshold_db;
        let k = self.knee_db * 0.5;
        let slope = 1.0 - 1.0 / self.ratio;
        if over <= -k {
            0.0
        } else if over >= k {
            over * slope
        } else {
            slope * (over + k) * (over + k) / (4.0 * k)
        }
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let level = gain_to_db(l.abs().max(r.abs()));
        let coef = if level > self.env_db { self.attack } else { self.release };
        self.env_db += (level - self.env_db) * coef;
        let g = db_to_gain(self.makeup_db - self.curve(self.env_db));
        (l * g, r * g)
    }
}

const LOOKAHEAD: usize = 128;

/// Look-ahead brickwall limiter: the gain is computed from the peak of the
/// next ~1.3-2.7 ms (at 96-48 kHz), so transients are caught before they
/// pass, and released smoothly.
#[derive(Clone, Copy, Debug)]
pub struct Limiter {
    pub ceiling: f32,
    buf_l: [f32; LOOKAHEAD],
    buf_r: [f32; LOOKAHEAD],
    at: usize,
    gain: f32,
    release: f32,
    hold_peak: f32,
    hold_left: usize,
}

impl Limiter {
    pub fn new(rate: f32, ceiling_db: f32) -> Self {
        let rate = finite(rate, 48_000.0).max(1.0);
        Limiter {
            ceiling: db_to_gain(ceiling_db),
            buf_l: [0.0; LOOKAHEAD],
            buf_r: [0.0; LOOKAHEAD],
            at: 0,
            gain: 1.0,
            release: 1.0 - (-1.0 / (0.08 * rate)).exp(),
            hold_peak: 0.0,
            hold_left: 0,
        }
    }

    /// The limiter's latency in samples.
    pub const fn latency() -> usize {
        LOOKAHEAD
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let l = finite(l, 0.0);
        let r = finite(r, 0.0);
        let peak = l.abs().max(r.abs());
        // Peak-hold over the look-ahead window (a running max that restarts
        // when the held peak ages out: cheap, conservative).
        if peak >= self.hold_peak || self.hold_left == 0 {
            self.hold_peak = peak;
            self.hold_left = LOOKAHEAD;
        } else {
            self.hold_left -= 1;
        }
        let want = if self.hold_peak > self.ceiling { self.ceiling / self.hold_peak } else { 1.0 };
        if want < self.gain {
            // Attack across the look-ahead so the gain is down in time.
            self.gain += (want - self.gain) * (4.0 / LOOKAHEAD as f32);
            if self.gain < want {
                self.gain = want;
            }
        } else {
            self.gain += (want - self.gain) * self.release;
        }
        let out_l = self.buf_l[self.at];
        let out_r = self.buf_r[self.at];
        self.buf_l[self.at] = l;
        self.buf_r[self.at] = r;
        self.at = (self.at + 1) % LOOKAHEAD;
        // The last line of defence is a hard clip at the ceiling: the
        // smoothed attack can lag a single extreme spike by a hair.
        let c = self.ceiling;
        ((out_l * self.gain).clamp(-c, c), (out_r * self.gain).clamp(-c, c))
    }
}

/// Side-chain ducker: `key` is an envelope 0..1 (voice activity); the bus is
/// pulled down by up to `depth_db` while it is up.
#[derive(Clone, Copy, Debug)]
pub struct Ducker {
    pub depth_db: f32,
    level: f32,
    attack: f32,
    release: f32,
}

impl Ducker {
    pub fn new(rate: f32, depth_db: f32) -> Self {
        let rate = finite(rate, 48_000.0).max(1.0);
        Ducker {
            depth_db,
            level: 0.0,
            attack: 1.0 - (-1.0 / (0.03 * rate)).exp(),
            release: 1.0 - (-1.0 / (0.4 * rate)).exp(),
        }
    }

    #[inline]
    pub fn gain(&mut self, key: f32) -> f32 {
        let key = finite(key, 0.0).clamp(0.0, 1.0);
        let c = if key > self.level { self.attack } else { self.release };
        self.level += (key - self.level) * c;
        db_to_gain(-self.depth_db.abs() * self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limiter_never_passes_the_ceiling() {
        let mut lim = Limiter::new(48_000.0, -1.0);
        let ceiling = db_to_gain(-1.0);
        for i in 0..48_000 {
            let x = if i % 997 == 0 { 8.0 } else { ((i as f32) * 0.05).sin() * 2.0 };
            let (l, r) = lim.process(x, -x);
            assert!(l.abs() <= ceiling + 1.0e-6 && r.abs() <= ceiling + 1.0e-6);
        }
    }

    #[test]
    fn the_compressor_reduces_loud_and_leaves_quiet() {
        let mut c = Compressor::new(48_000.0, -18.0, 4.0, 0.005, 0.1);
        let mut loud = 0.0f32;
        for i in 0..48_000 {
            let x = ((i as f32) * 0.05).sin() * 0.9;
            loud = c.process(x, x).0.abs().max(loud * 0.9999);
        }
        assert!(c.reduction_db() < -6.0);
        let mut c = Compressor::new(48_000.0, -18.0, 4.0, 0.005, 0.1);
        for i in 0..48_000 {
            let x = ((i as f32) * 0.05).sin() * 0.01;
            c.process(x, x);
        }
        assert!(c.reduction_db() > -0.1);
    }
}
