// Contact noise sources that are gated by the instrument's own motion:
// snare wires against the snare-side head, the hi-hat top cymbal bouncing on
// the bottom, and — a different mechanism, the same output — the hand clap.
//
// Snare wires. The wires lie against the resonant head under light tension.
// When the head moves far enough (it is driven through the air column by the
// batter, so it starts a few milliseconds AFTER the stick), the wires lose
// contact and slap back: a burst of broadband impacts twice per cycle of the
// head's motion, gated by how far past the lift-off threshold the head is.
// So the noise is not a free-running burst: it starts late (the reference
// snare's high band peaks 30-40 ms after the strike at soft velocities),
// it follows the resonant head's envelope (a ghost note buzzes for a moment,
// a rim shot for a quarter of a second), it is modulated at 2 f_reso, and it
// disappears entirely for the softest touches, whose head motion never
// clears the threshold. The impacts are injected back into the resonant
// head's modes (which colour them) and into a small set of wire/head
// formant modes (~1-6 kHz), both living in the voice's modal bank.
//
// Hi-hat chatter is the same detector on the top cymbal's low-mode
// displacement: struck hard when closed, the top bounces off the bottom for
// a few tens of milliseconds.
//
// The detector: g = max(|x| - threshold, 0) * scale smoothed with a fast
// attack and a slower release, multiplying seeded white noise. It is NOT
// clamped to 1: the impacts get harder as the head moves further, so the
// wire noise scales with the head's motion (the reference wire share stays
// within a few dB of the tone from ghost notes to rim shots); a high cap
// keeps it bounded in principle.

use crate::util::{one_pole_coeff, Rng};

#[derive(Clone, Copy)]
pub struct RattleDesign {
    /// Lift-off threshold and the slope past it (displacement units of the
    /// bank's `gh` read).
    pub threshold: f32,
    pub scale: f32,
    pub attack_s: f32,
    pub release_s: f32,
    /// Impact force scale (N per unit noise).
    pub gain: f32,
    /// Hard stop after this many ms (0 = none): the hi-hat chatter ends
    /// when the top settles, the wires never stop on their own.
    pub max_ms: f32,
}

#[derive(Clone, Copy)]
pub struct Rattle {
    on: bool,
    rng: Rng,
    env: f32,
    a_att: f32,
    a_rel: f32,
    thr: f32,
    scale: f32,
    gain: f32,
    remaining: u32,
}

impl Rattle {
    pub const fn idle() -> Self {
        Self { on: false, rng: Rng(1), env: 0.0, a_att: 1.0, a_rel: 1.0, thr: 0.0, scale: 0.0, gain: 0.0, remaining: 0 }
    }

    pub fn start(&mut self, d: &RattleDesign, fs: f32, seed: u32) {
        // (attack: the wires' own bouncing motion takes several head cycles
        // to build to full chatter — the reference snare's high band peaks
        // 30-40 ms after the stick at soft and medium strokes)
        self.on = true;
        self.rng = Rng::new(seed);
        self.env = 0.0;
        self.a_att = one_pole_coeff(d.attack_s, fs);
        self.a_rel = one_pole_coeff(d.release_s, fs);
        self.thr = d.threshold;
        self.scale = d.scale;
        self.gain = d.gain;
        self.remaining = if d.max_ms > 0.0 { (d.max_ms * 1e-3 * fs) as u32 } else { u32::MAX };
    }

    #[inline]
    pub fn step(&mut self, x: f32) -> f32 {
        if !self.on {
            return 0.0;
        }
        if self.remaining != u32::MAX {
            if self.remaining == 0 {
                self.on = false;
                return 0.0;
            }
            self.remaining -= 1;
        }
        let g = ((x.abs() - self.thr) * self.scale).clamp(0.0, 50.0);
        let a = if g > self.env { self.a_att } else { self.a_rel };
        self.env += a * (g - self.env);
        if self.env < 1e-6 {
            // keep the generator sequence independent of the gate so the
            // noise is the same stream regardless of when it opens
            self.rng.next_u32();
            return 0.0;
        }
        self.rng.bipolar() * self.env * self.gain
    }
}

// ---------------------------------------------------------------------------
// Hand clap
// ---------------------------------------------------------------------------
//
// A clap is several impacts, not one: two hands never meet flat, and a group
// of hands never meet at once, so the onset is a flam of 3-5 short bursts
// ~8-12 ms apart (Repp 1987 measured single-clap durations of 5-10 ms with
// spectral peaks between 1 and 3 kHz set by the cupped-hand cavity). Each
// burst is white noise under a 0.3 ms attack / ~2.5 ms decay; after the last
// one a quieter noise tail decays over ~40 ms (the room and the hands'
// after-motion). The bursts drive a small bank of body formants (0.9-9 kHz,
// low Q) in the voice's modal bank; velocity adds a burst and opens the
// higher formants.

#[derive(Clone, Copy)]
pub struct ClapDesign {
    pub burst_decay_s: f32,
    pub burst_spacing_s: f32,
    pub spacing_jitter_s: f32,
    pub tail_level: f32,
    pub tail_decay_s: f32,
    pub gain: f32,
}

#[derive(Clone, Copy)]
pub struct Clap {
    on: bool,
    rng: Rng,
    t: u32,
    n_bursts: u32,
    next_burst: u32,
    bursts_done: u32,
    spacing: f32,
    jitter: f32,
    env: f32,
    burst_a: f32,
    tail_env: f32,
    tail_a: f32,
    tail_level: f32,
    gain: f32,
    amp_scale: f32,
    fs: f32,
    end: u32,
}

impl Clap {
    pub const fn idle() -> Self {
        Self {
            on: false,
            rng: Rng(1),
            t: 0,
            n_bursts: 0,
            next_burst: 0,
            bursts_done: 0,
            spacing: 0.0,
            jitter: 0.0,
            env: 0.0,
            burst_a: 0.0,
            tail_env: 0.0,
            tail_a: 0.0,
            tail_level: 0.0,
            gain: 0.0,
            amp_scale: 1.0,
            fs: 48000.0,
            end: 0,
        }
    }

    pub fn start(&mut self, d: &ClapDesign, fs: f32, velocity: f32, seed: u32) {
        self.on = true;
        self.rng = Rng::new(seed);
        self.t = 0;
        self.n_bursts = 3 + (velocity * 1.6).round() as u32;
        self.next_burst = 0;
        self.bursts_done = 0;
        self.spacing = d.burst_spacing_s * fs;
        self.jitter = d.spacing_jitter_s * fs;
        self.env = 0.0;
        self.burst_a = (-1.0 / (d.burst_decay_s * fs)).exp();
        self.tail_env = 0.0;
        self.tail_a = (-1.0 / (d.tail_decay_s * fs)).exp();
        self.tail_level = d.tail_level;
        self.gain = d.gain * velocity.clamp(0.0, 1.0).powf(1.3);
        self.amp_scale = 1.0;
        self.fs = fs;
        self.end = u32::MAX;
    }

    #[inline]
    pub fn step(&mut self) -> f32 {
        if !self.on {
            return 0.0;
        }
        if self.bursts_done < self.n_bursts && self.t >= self.next_burst {
            // a new hand pair lands: jittered amplitude, next time jittered
            self.env = 0.7 + 0.3 * self.rng.unit();
            self.bursts_done += 1;
            let j = self.jitter * self.rng.bipolar();
            self.next_burst = self.t + (self.spacing + j).max(2.0) as u32;
            if self.bursts_done == self.n_bursts {
                self.tail_env = self.tail_level;
                self.end = self.t + (0.4 * self.fs) as u32;
            }
        }
        self.t += 1;
        let n = self.rng.bipolar();
        let out = n * (self.env + self.tail_env) * self.gain;
        self.env *= self.burst_a;
        self.tail_env *= self.tail_a;
        if self.t >= self.end {
            self.on = false;
        }
        out
    }
}
