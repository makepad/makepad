//! Duplex segments: the short lengths of string between the bridge and the
//! hitch pins, which no damper touches. They sit on the same bridge as the
//! speaking lengths, so every note sets its neighbourhood's segments
//! ringing, mostly with what its attack leaves in them, and they ring on at
//! their own pitches after the note is damped.
//!
//! One bank of modes per region of the upper long bridge. A segment gains the bridge's velocity once per round trip and
//! gives the bridge back the force of one string, so its strengths are the
//! unison junction's, not free numbers.

use crate::board::{BLOCK, BUSES};
use crate::scale::{KeyScale, KEYS};
use crate::voice::jitter;

const MODES: usize = 16;
/// Segments sing from about middle C up; below that makers mute them with
/// felt, and the bass bridge has none.
const FIRST_REGION: usize = 2;

#[derive(Clone)]
pub struct Duplex {
    /// Per region and mode: rotation (re, im), state (re, im), drive gain.
    modes: Vec<[[f32; 5]; MODES]>,
    /// Bridge force of a region to bridge velocity, and a segment's wave to
    /// the bridge velocity it causes, per region.
    receive: [f32; BUSES],
    cause: [f32; BUSES],
    ringing: [bool; BUSES],
    strength: f32,
}

impl Duplex {
    /// `gains` are the keys' output gains, so a region's segments sound at
    /// the level of the strings they belong to.
    pub fn new(scale: &[KeyScale], gains: &[f32], fs: f64, strength: f64) -> Self {
        let mut duplex = Duplex { modes: vec![[[0.0; 5]; MODES]; BUSES], receive: [0.0; BUSES], cause: [0.0; BUSES], ringing: [false; BUSES], strength: strength as f32 };
        for region in FIRST_REGION..BUSES {
            // The keys nearest this region of the bridge.
            let centre = region as f64 / (BUSES - 1) as f64 * (KEYS - 1) as f64;
            let half = 0.5 * (KEYS - 1) as f64 / (BUSES - 1) as f64;
            let lo = (centre - half).max(20.0);
            let hi = (centre + half).min((KEYS - 1) as f64);
            let (mut r, mut z, mut gain, mut count) = (0.0, 0.0, 0.0, 0.0);
            for m in 0..MODES {
                let index = (lo + (hi - lo) * (m as f64 + 0.5) / MODES as f64) as usize;
                let key = &scale[index.min(KEYS - 1)];
                r += key.bridge_r;
                z += key.z;
                gain += gains[index.min(KEYS - 1)] as f64;
                count += 1.0;
                // A segment is a small-integer fraction of the speaking
                // length, never exactly: makers detune them a little.
                let id = (region * MODES + m) as u32;
                let ratio = [3.0, 4.0, 5.0, 6.0, 8.0][(id as usize * 7 + m) % 5] * (1.0 + 0.03 * jitter(id, 31));
                let f = (key.f1 * ratio).min(0.2 * fs);
                let radius = (-(1.5 + 0.5 * jitter(id, 32)) / fs).exp();
                let w = std::f64::consts::TAU * f / fs;
                duplex.modes[region][m] = [(radius * w.cos()) as f32, (radius * w.sin()) as f32, 0.0, 0.0, (f / fs) as f32];
            }
            let (r, z, gain) = (r / count, z / count, gain / count);
            duplex.receive[region] = (1.0 / r) as f32;
            // three strings per key; 16 modes stand for the region's keys
            let keys = hi - lo;
            duplex.cause[region] = (gain * 3.0 * (keys / MODES as f64) * 2.0 * z / r) as f32;
        }
        duplex
    }

    /// Each region's segment frequencies, Hz (empty below `FIRST_REGION`).
    pub fn frequencies(&self, fs: f64) -> Vec<Vec<f64>> {
        (0..BUSES)
            .map(|region| if region < FIRST_REGION { Vec::new() } else { self.modes[region].iter().map(|m| (m[1] as f64).atan2(m[0] as f64) * fs / std::f64::consts::TAU).collect() })
            .collect()
    }

    pub fn set_strength(&mut self, strength: f64) {
        self.strength = strength.max(0.0) as f32;
    }

    pub fn clear(&mut self) {
        for region in &mut self.modes {
            for mode in region {
                mode[2] = 0.0;
                mode[3] = 0.0;
            }
        }
        self.ringing = [false; BUSES];
    }

    pub fn ringing(&self) -> bool {
        self.ringing.iter().any(|&r| r)
    }

    /// `force[b][..n]` is the bridge force of each region in N; the sound of
    /// a region's segments is added to `bus[at[b]][..n]`, the board's place
    /// for that region.
    pub fn process(&mut self, force: &[[f32; BLOCK]; BUSES], bus: &mut [[f32; BLOCK]], at: &[usize; BUSES], n: usize) {
        for region in FIRST_REGION..BUSES {
            let driven = force[region][..n].iter().any(|&f| f != 0.0);
            if !driven && !self.ringing[region] {
                continue;
            }
            let (receive, cause) = (self.receive[region], self.cause[region]);
            let bus = &mut bus[at[region]];
            let mut energy = 0.0f32;
            for mode in &mut self.modes[region] {
                let [cr, ci, mut re, mut im, gain] = *mode;
                let gain = gain * receive * self.strength;
                for i in 0..n {
                    let nr = cr * re - ci * im + gain * force[region][i];
                    im = ci * re + cr * im;
                    re = nr;
                    bus[i] -= cause * re;
                }
                mode[2] = re;
                mode[3] = im;
                energy += re * re + im * im;
            }
            // at rest once the segments add less than -170 dB to the sound
            self.ringing[region] = energy * cause * cause > 1e-17;
            if !self.ringing[region] {
                for mode in &mut self.modes[region] {
                    mode[2] = 0.0;
                    mode[3] = 0.0;
                }
            }
        }
    }
}
