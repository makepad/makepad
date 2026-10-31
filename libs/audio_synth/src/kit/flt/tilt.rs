//! The tilt-noise bank (FILTERS.md 11): eleven trapezoidal SVF shelves in a
//! pipelined series (11 samples of latency) turning uniform noise into a
//! tilted spectrum, the scene noise source's "tilt" colour mode.

use super::approx::tan_r;
use super::QUIRK_TILT_BANK_HOST_RATE;
use crate::dsp::Rng;
use crate::kit::core::{BlockLine, BLOCK_OS};
use std::f32::consts::PI;

const SECTIONS: usize = 11;
const K: f32 = 1.86;

#[derive(Clone, Copy, Debug, Default)]
struct Bank {
    st: [[f32; 2]; SECTIONS],
    pipe: [f32; SECTIONS],
}

/// One voice's tilt-noise generator (L, and R in true stereo).
#[derive(Clone, Debug)]
pub struct TiltNoise {
    rng: Rng,
    fs_host: f32,
    fs_os: f32,
    a: [[f32; 3]; SECTIONS],
    m: [[f32; 3]; SECTIONS],
    dm: [[f32; 3]; SECTIONS],
    bank: [Bank; 2],
    att: BlockLine,
    /// Ramps the cube of the level.
    level: BlockLine,
    /// Stereo path only: smoothed side (width) and mid gains, both from 0
    /// toward 1 at 0.25 per block.
    width: BlockLine,
    mid: BlockLine,
    first: bool,
}

/// End-of-block ramp convention: step first, so the target lands on the
/// block's last sample.
#[inline]
fn ramp_next(l: &mut BlockLine) -> f32 {
    l.v += l.dv;
    l.v
}

/// One block of the width / mid smoothers: v <- 0.25 + 0.75 v, ramped.
#[inline]
fn smooth_toward_one(l: &mut BlockLine) {
    let t = 0.25 + 0.75 * l.v;
    l.dv = (t - l.v) / BLOCK_OS as f32;
}

fn shelf_mix(slope: f32) -> [[f32; 3]; SECTIONS] {
    let lo = 10f32.powf(-slope / 20.0);
    let hi = 10f32.powf(slope / 20.0);
    let mut m = [[0.0; 3]; SECTIONS];
    for (i, mi) in m.iter_mut().enumerate() {
        *mi = if i < 6 { [1.0, K * (lo - 1.0), lo * lo - 1.0] } else { [hi * hi, K * (1.0 - hi) * hi, 1.0 - hi * hi] };
    }
    m
}

impl TiltNoise {
    /// A generator for a voice; `start` before the first block.
    pub fn new(seed: u32) -> TiltNoise {
        TiltNoise {
            rng: Rng::new(seed),
            fs_host: 48000.0,
            fs_os: 96000.0,
            a: [[0.0; 3]; SECTIONS],
            m: [[0.0; 3]; SECTIONS],
            dm: [[0.0; 3]; SECTIONS],
            bank: [Bank::default(); 2],
            att: BlockLine::default(),
            level: BlockLine::default(),
            width: BlockLine::default(),
            mid: BlockLine::default(),
            first: true,
        }
    }

    /// The host sample rate (default 48 kHz); the bank runs at twice it.
    pub fn set_sample_rate(&mut self, fs_host: f32) {
        self.fs_host = fs_host;
        self.fs_os = 2.0 * fs_host;
    }

    /// Voice start: corner coefficients, the shelf mix for `tilt`
    /// (-6..6, = noise colour x 6), and the pipeline pre-filled with noise
    /// (draws interleaved L0, R0, L1, R1, ..., R drawn even in mono).
    pub fn start(&mut self, tilt: f32, stereo: bool) {
        let fs = if QUIRK_TILT_BANK_HOST_RATE { self.fs_host } else { self.fs_os };
        for i in 0..SECTIONS {
            let f = 10.0 * (2f32).powi(i as i32 + 1);
            let g = tan_r(PI * f / fs);
            let a1 = 1.0 / (1.0 + g * (g + K));
            let a2 = g * a1;
            self.a[i] = [a1, a2, g * a2];
        }
        self.m = shelf_mix(tilt.clamp(-6.0, 6.0) / 2.0);
        self.dm = [[0.0; 3]; SECTIONS];
        self.bank = [Bank::default(); 2];
        for _ in 0..SECTIONS {
            let xl = self.rng.bipolar();
            let xr = self.rng.bipolar();
            self.step(0, xl);
            if stereo {
                self.step(1, xr);
            }
        }
        self.att = BlockLine::default();
        self.level = BlockLine::default();
        self.width = BlockLine::default();
        self.mid = BlockLine::default();
        self.width.snap(0.0);
        self.mid.snap(0.0);
        self.first = true;
    }

    #[inline]
    fn step(&mut self, ch: usize, x: f32) -> f32 {
        let b = &mut self.bank[ch];
        let out = b.pipe[SECTIONS - 1];
        for i in (0..SECTIONS).rev() {
            let inp = if i == 0 { x } else { b.pipe[i - 1] };
            let [a1, a2, a3] = self.a[i];
            let [m0, m1, m2] = self.m[i];
            let s = &mut b.st[i];
            let v3 = inp - s[1];
            let v1 = a1 * s[0] + a2 * v3;
            let v2 = s[1] + a2 * s[0] + a3 * v3;
            s[0] = 2.0 * v1 - s[0];
            s[1] = 2.0 * v2 - s[1];
            b.pipe[i] = m0 * inp + m1 * v1 + m2 * v2;
        }
        out
    }

    /// One 64-sample block of tilted noise: `colour` -1..1, `level` the
    /// noise level value (output gain level^3, the cube ramped across the
    /// block), `stereo` = independent R through the mid/side width stage.
    pub fn block(&mut self, colour: f32, level: f32, stereo: bool, l: &mut [f32; BLOCK_OS], r: &mut [f32; BLOCK_OS]) {
        let tilt = colour.clamp(-1.0, 1.0) * 6.0;
        let new = shelf_mix(tilt / 2.0);
        if self.first {
            self.m = new;
            self.dm = [[0.0; 3]; SECTIONS];
            self.first = false;
        } else {
            for i in 0..SECTIONS {
                for j in 0..3 {
                    self.dm[i][j] = (new[i][j] - self.m[i][j]) / BLOCK_OS as f32;
                }
            }
        }
        let t = if tilt > 0.0 { -4.0 * tilt } else { tilt };
        self.att.set(10f32.powf(t / 20.0), BLOCK_OS);
        self.level.set(level * level * level, BLOCK_OS);
        if stereo {
            smooth_toward_one(&mut self.width);
            smooth_toward_one(&mut self.mid);
        }
        for i in 0..BLOCK_OS {
            let a = ramp_next(&mut self.att);
            let nl = self.rng.bipolar() * a;
            let nr = if stereo { self.rng.bipolar() * a } else { nl };
            let yl = self.step(0, nl);
            let g = ramp_next(&mut self.level);
            if stereo {
                let yr = self.step(1, nr);
                let (w, m) = (ramp_next(&mut self.width), ramp_next(&mut self.mid));
                let mid = 0.5 * (yl + yr) * m;
                let side = 0.5 * (yl - yr) * w;
                l[i] = (mid + side) * g;
                r[i] = (mid - side) * g;
            } else {
                l[i] = yl * g;
                r[i] = yl * g;
            }
            for s in 0..SECTIONS {
                for j in 0..3 {
                    self.m[s][j] += self.dm[s][j];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_draws_interleave_and_r_is_drawn_in_mono() {
        let mut mono = TiltNoise::new(9);
        let mut st = TiltNoise::new(9);
        mono.start(2.0, false);
        st.start(2.0, true);
        // Same draws consumed, L fed the even ones in both.
        assert_eq!(mono.rng.next_u32(), st.rng.next_u32());
        assert_eq!(mono.bank[0].pipe, st.bank[0].pipe);
        let mut by_hand = TiltNoise::new(9);
        by_hand.start(2.0, false);
        by_hand.bank = [Bank::default(); 2];
        by_hand.rng = Rng::new(9);
        for _ in 0..SECTIONS {
            let x = by_hand.rng.bipolar();
            by_hand.rng.bipolar();
            by_hand.step(0, x);
        }
        assert_eq!(by_hand.bank[0].pipe, mono.bank[0].pipe);
    }

    #[test]
    fn level_cube_is_ramped_and_snaps_first() {
        let (mut l, mut r) = ([0.0; BLOCK_OS], [0.0; BLOCK_OS]);
        let (mut l1, mut r1) = ([0.0; BLOCK_OS], [0.0; BLOCK_OS]);
        let mut a = TiltNoise::new(4);
        a.start(0.0, false);
        let mut b = a.clone();
        // First block snaps: gain 0.5^3 throughout.
        a.block(0.0, 0.5, false, &mut l, &mut r);
        b.block(0.0, 1.0, false, &mut l1, &mut r1);
        for k in 0..BLOCK_OS {
            assert!((l[k] - 0.125 * l1[k]).abs() <= 1e-6 * l1[k].abs().max(1e-3), "{k}");
        }
        // Then the cube ramps linearly to its target on the last sample.
        b.level.snap(1.0);
        a.block(0.0, 1.0, false, &mut l, &mut r);
        b.block(0.0, 1.0, false, &mut l1, &mut r1);
        for k in 0..BLOCK_OS {
            let g = 0.125 + 0.875 * (k + 1) as f32 / BLOCK_OS as f32;
            assert!((l[k] - g * l1[k]).abs() <= 1e-5 * l1[k].abs().max(1e-3), "{k} {} {}", l[k], g * l1[k]);
        }
        assert_eq!(l, r);
    }

    #[test]
    fn stereo_width_fades_in_from_zero() {
        let (mut l, mut r) = ([0.0; BLOCK_OS], [0.0; BLOCK_OS]);
        let (mut l1, mut r1) = ([0.0; BLOCK_OS], [0.0; BLOCK_OS]);
        let mut a = TiltNoise::new(5);
        a.start(-3.0, true);
        let mut b = a.clone();
        b.width.snap(1.0);
        b.mid.snap(1.0);
        let mut prev = 0.0f32;
        for blk in 1..=12 {
            a.block(-0.5, 1.0, true, &mut l, &mut r);
            b.block(-0.5, 1.0, true, &mut l1, &mut r1);
            let end = 1.0 - 0.75f32.powi(blk);
            for k in 0..BLOCK_OS {
                let g = prev + (end - prev) * (k + 1) as f32 / BLOCK_OS as f32;
                let scale = l1[k].abs().max(r1[k].abs()).max(1e-6);
                assert!((l[k] - g * l1[k]).abs() <= 1e-5 * scale, "block {blk} {k}");
                assert!((r[k] - g * r1[k]).abs() <= 1e-5 * scale, "block {blk} {k}");
            }
            prev = end;
        }
        assert!(l != r);
    }
}
