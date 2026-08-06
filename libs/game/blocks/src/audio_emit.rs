//! Gameplay-driven sound emission.
//!
//! Blocks know things the script never says out loud: that a wheel is
//! slipping, that a foot just landed, that an engine is at 80% throttle. This
//! module turns that into [`SoundEvent`]s so a game is audible without the AI
//! writing a single cue — the same bargain the rest of the blocks layer makes
//! for physics and AI.
//!
//! Nothing here plays anything. Events are queued and drained by the host,
//! which owns the mixer; that keeps this crate free of audio devices and keeps
//! sound Local tier (game.md) — the events describe the shared simulation, but
//! what a device does with them is its own business.

use makepad_game_audio::director::{Category, SoundEvent};
use makepad_game_audio::materials::Material;
use makepad_math::Vec3f;

/// One queued sound with where it happened, so the host can place it.
#[derive(Clone, Debug, PartialEq)]
pub struct EmittedSound {
    pub event: SoundEvent,
    /// World position; the host converts to gain/pan against its listener.
    pub at: Vec3f,
    /// Audible radius. Engine loops carry further than footsteps.
    pub range: f32,
}

/// Queue of sounds produced during one tick.
#[derive(Clone, Debug, Default)]
pub struct AudioEmitter {
    queue: Vec<EmittedSound>,
    /// Hard cap so a pathological frame cannot grow this without bound; the
    /// director throttles too, but that is a separate budget downstream.
    cap: usize,
}

/// Beyond this many queued sounds in a tick, further ones are dropped. The
/// director will not start more than a handful anyway.
const DEFAULT_CAP: usize = 64;

impl AudioEmitter {
    pub fn new() -> Self {
        Self {
            queue: Vec::new(),
            cap: DEFAULT_CAP,
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Take everything queued this tick. The host calls this after `post_step`.
    pub fn drain(&mut self) -> Vec<EmittedSound> {
        std::mem::take(&mut self.queue)
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }

    fn push(&mut self, sound: EmittedSound) {
        if self.queue.len() < self.cap {
            self.queue.push(sound);
        }
    }

    /// A named cue at a position.
    pub fn cue(&mut self, name: &str, category: Category, at: Vec3f, gain: f32, pitch: f32) {
        self.push(EmittedSound {
            event: SoundEvent::Cue {
                name: name.to_string(),
                category,
                gain: gain.clamp(0.0, 2.0),
                pitch: pitch.clamp(0.25, 4.0),
            },
            at,
            range: 40.0,
        });
    }

    /// A physics contact. `pair_key` must be stable for the same two bodies so
    /// the director can throttle a rattling pair.
    pub fn impact(&mut self, a: Material, b: Material, speed: f32, at: Vec3f, pair_key: u64) {
        self.push(EmittedSound {
            event: SoundEvent::Impact {
                a,
                b,
                speed,
                pair_key,
            },
            at,
            range: 50.0,
        });
    }
}

/// A stable key for a contact between two entities, order-independent so the
/// cooldown applies to the pair rather than to whichever body reported first.
pub fn contact_key(a: u64, b: u64) -> u64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    // A cheap mix; collisions between distinct pairs only cost a shared
    // cooldown, never a wrong sound.
    lo.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ hi.rotate_left(32)
}

/// Footstep timing from a walk cycle.
///
/// Steps land on the half-phase, so a character emits at 0.0 and 0.5 of its
/// stride, and the stride shortens as it speeds up. Driving this from the
/// animation phase rather than a timer keeps feet and sound together when the
/// character slows down or stops mid-stride.
#[derive(Clone, Debug)]
pub struct StepTimer {
    phase: f32,
    /// Half-cycle the last step fired on, so one step fires once. Starts at
    /// -1 so the first footfall lands the moment you start walking rather
    /// than half a stride later.
    last_half: i32,
}

impl Default for StepTimer {
    fn default() -> Self {
        Self {
            phase: 0.0,
            last_half: -1,
        }
    }
}

impl StepTimer {
    /// Advance and report whether a foot just landed.
    ///
    /// `speed` is horizontal ground speed; `stride` is metres per full cycle.
    pub fn advance(&mut self, dt: f32, speed: f32, stride: f32) -> bool {
        if !dt.is_finite() || !speed.is_finite() || speed <= 0.15 || stride <= 0.01 {
            // Standing still: reset so the next step off the mark lands on a
            // fresh phase instead of half-way through one.
            self.phase = 0.0;
            self.last_half = -1;
            return false;
        }
        self.phase += speed * dt / stride;
        if self.phase >= 8.0 {
            // Keep the accumulator small without disturbing the half-phase.
            self.phase %= 1.0;
            self.last_half = -1;
        }
        let half = (self.phase * 2.0).floor() as i32;
        if half != self.last_half {
            self.last_half = half;
            return true;
        }
        false
    }
}

/// Engine note from throttle and speed.
///
/// Pitch tracks speed with a floor so an idling engine still hums, and
/// throttle sets loudness — the combination is what makes acceleration
/// audible rather than just fast.
pub fn engine_note(speed: f32, top_speed: f32, throttle: f32) -> (f32, f32) {
    let s = if top_speed > 0.01 {
        (speed / top_speed).clamp(0.0, 1.5)
    } else {
        0.0
    };
    let pitch = (0.7 + s * 0.9).clamp(0.25, 4.0);
    let gain = (0.25 + throttle.clamp(0.0, 1.0) * 0.5 + s * 0.2).clamp(0.0, 1.0);
    (gain, pitch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    #[test]
    fn contact_keys_are_order_independent() {
        assert_eq!(contact_key(3, 9), contact_key(9, 3));
        assert_ne!(contact_key(3, 9), contact_key(3, 10));
    }

    #[test]
    fn the_queue_is_capped() {
        let mut e = AudioEmitter::new();
        for i in 0..500u64 {
            e.impact(Material::Wood, Material::Stone, 5.0, v(0.0, 0.0, 0.0), i);
        }
        assert!(e.len() <= DEFAULT_CAP, "queue grew to {}", e.len());
    }

    #[test]
    fn draining_empties_the_queue() {
        let mut e = AudioEmitter::new();
        e.cue("jump", Category::Movement, v(1.0, 2.0, 3.0), 1.0, 1.0);
        let out = e.drain();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].at, v(1.0, 2.0, 3.0));
        assert!(e.is_empty());
    }

    #[test]
    fn footsteps_fire_twice_per_stride() {
        let mut t = StepTimer::default();
        // 2 m/s over a 2 m stride: one full cycle per second, two steps.
        let mut steps = 0;
        for _ in 0..60 {
            if t.advance(1.0 / 60.0, 2.0, 2.0) {
                steps += 1;
            }
        }
        assert!((2..=3).contains(&steps), "expected ~2 steps, got {steps}");
    }

    #[test]
    fn faster_movement_makes_more_steps() {
        let count = |speed: f32| {
            let mut t = StepTimer::default();
            (0..120)
                .filter(|_| t.advance(1.0 / 60.0, speed, 2.0))
                .count()
        };
        assert!(count(6.0) > count(2.0));
    }

    #[test]
    fn standing_still_makes_no_steps() {
        let mut t = StepTimer::default();
        for _ in 0..300 {
            assert!(!t.advance(1.0 / 60.0, 0.0, 2.0));
        }
        // Nonsense inputs are refused rather than dividing by zero.
        assert!(!t.advance(f32::NAN, 5.0, 2.0));
        assert!(!t.advance(0.016, 5.0, 0.0));
    }

    #[test]
    fn step_phase_does_not_drift_over_a_long_walk() {
        let mut t = StepTimer::default();
        let steps = (0..60 * 60).filter(|_| t.advance(1.0 / 60.0, 2.0, 2.0)).count();
        // 60 seconds at one cycle per second is about 120 steps.
        assert!((115..=125).contains(&steps), "drifted to {steps}");
    }

    #[test]
    fn engine_note_rises_with_speed_and_throttle() {
        let (idle_g, idle_p) = engine_note(0.0, 30.0, 0.0);
        let (mid_g, mid_p) = engine_note(15.0, 30.0, 0.5);
        let (fast_g, fast_p) = engine_note(30.0, 30.0, 1.0);
        assert!(idle_p < mid_p && mid_p < fast_p);
        assert!(idle_g < mid_g && mid_g < fast_g);
        // An idling engine is still audible.
        assert!(idle_g > 0.0);
        // Nothing runs away with a silly top speed.
        let (g, p) = engine_note(500.0, 0.0, 2.0);
        assert!(g <= 1.0 && (0.25..=4.0).contains(&p));
    }
}
