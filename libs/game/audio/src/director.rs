//! The audio director: gameplay events in, voices out.
//!
//! This is the layer that makes a game audible without the AI writing cues.
//! Blocks and the physics step emit [`SoundEvent`]s describing *what
//! happened*; the director decides which sound that is, picks a fresh variant,
//! enforces the budgets that stop a collapsing stack machine-gunning, and
//! hands the mixer a voice.
//!
//! All selection randomness comes from a [`LocalRng`]. Audio is Local tier: it
//! must never advance the simulation.

use crate::bank::{SampleBank, SampleId};
use crate::materials::{ImpactCurve, ImpactSound, Material, MaterialPair};
use crate::mixer::{Mixer, Priority, VoiceHandle, VoiceSpec};
use crate::rng::LocalRng;
use std::collections::HashMap;

/// Volume groups a player (or a device) can balance independently.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Category {
    /// Physics contacts.
    Impact,
    /// Footsteps, engines, anything a block emits continuously.
    Movement,
    /// Weapons, pickups, scoring.
    Action,
    /// Menu and HUD.
    Ui,
    /// Background texture.
    Ambient,
}

impl Category {
    pub fn all() -> [Category; 5] {
        [
            Category::Impact,
            Category::Movement,
            Category::Action,
            Category::Ui,
            Category::Ambient,
        ]
    }

    fn priority(self) -> Priority {
        match self {
            Category::Action | Category::Impact => Priority::High,
            Category::Movement | Category::Ui => Priority::Normal,
            Category::Ambient => Priority::Low,
        }
    }
}

/// What happened, in gameplay terms. The director owns the translation from
/// this to an actual sound.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundEvent {
    /// Two things touched. `speed` is closing speed at the contact.
    Impact {
        a: Material,
        b: Material,
        speed: f32,
        /// Stable key for the contact, so repeats can be throttled.
        pair_key: u64,
    },
    /// A named cue from a block: `footstep`, `jump`, `land`, `skid`,
    /// `engine-start`, `shoot`, `pickup`, `checkpoint`, `win`.
    Cue {
        name: String,
        category: Category,
        gain: f32,
        pitch: f32,
    },
}

/// A family of interchangeable takes on one sound.
struct Variants {
    samples: Vec<SampleId>,
    /// Index last played, so the next pick can avoid it.
    last: Option<usize>,
}

/// Per-frame and per-pair throttles.
pub struct Budget {
    /// Most sounds started in a single frame.
    pub per_frame: usize,
    /// Seconds a given contact pair must wait before it can sound again.
    pub pair_cooldown: f32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            per_frame: 6,
            pair_cooldown: 0.06,
        }
    }
}

pub struct AudioDirector {
    families: HashMap<String, Variants>,
    volumes: HashMap<Category, f32>,
    rng: LocalRng,
    curve: ImpactCurve,
    budget: Budget,
    /// Contact key -> time it last sounded.
    cooldowns: HashMap<u64, f32>,
    now: f32,
    started_this_frame: usize,
    /// Events refused this frame, for budget tuning.
    dropped: usize,
}

impl AudioDirector {
    pub fn new(seed: u64) -> Self {
        let mut volumes = HashMap::new();
        for c in Category::all() {
            volumes.insert(c, 1.0);
        }
        Self {
            families: HashMap::new(),
            volumes,
            rng: LocalRng::new(seed),
            curve: ImpactCurve::default(),
            budget: Budget::default(),
            cooldowns: HashMap::new(),
            now: 0.0,
            started_this_frame: 0,
            dropped: 0,
        }
    }

    pub fn set_budget(&mut self, budget: Budget) {
        self.budget = budget;
    }

    pub fn set_volume(&mut self, category: Category, gain: f32) {
        self.volumes.insert(category, gain.clamp(0.0, 2.0));
    }

    pub fn dropped_last_frame(&self) -> usize {
        self.dropped
    }

    /// Register a sound family. `name` is a cue name (`footstep`) or a
    /// material key (`impact-wood`); the samples are interchangeable takes.
    pub fn register(&mut self, name: &str, samples: Vec<SampleId>) {
        if samples.is_empty() {
            return;
        }
        self.families.insert(
            name.to_string(),
            Variants {
                samples,
                last: None,
            },
        );
    }

    pub fn knows(&self, name: &str) -> bool {
        self.families.contains_key(name)
    }

    /// Advance the frame clock and reset per-frame budgets.
    pub fn begin_frame(&mut self, dt: f32) {
        self.now += dt.max(0.0);
        self.started_this_frame = 0;
        self.dropped = 0;
        // Forget stale cooldowns so the map cannot grow without bound in a
        // long session.
        let cutoff = self.now - self.budget.pair_cooldown * 4.0;
        self.cooldowns.retain(|_, &mut t| t >= cutoff);
    }

    /// Turn an event into a voice. Returns `None` when the event was
    /// throttled, unknown, or too gentle to hear.
    pub fn emit(
        &mut self,
        event: &SoundEvent,
        placement: Placement,
        bank: &mut SampleBank,
        mixer: &mut Mixer,
    ) -> Option<VoiceHandle> {
        if self.started_this_frame >= self.budget.per_frame {
            self.dropped += 1;
            return None;
        }
        let (family, category, gain, pitch) = match event {
            SoundEvent::Impact {
                a,
                b,
                speed,
                pair_key,
            } => {
                if let Some(&last) = self.cooldowns.get(pair_key) {
                    if self.now - last < self.budget.pair_cooldown {
                        self.dropped += 1;
                        return None;
                    }
                }
                let pair = MaterialPair::new(*a, *b);
                let ImpactSound { gain, pitch } = self.curve.evaluate(*speed, pair)?;
                self.cooldowns.insert(*pair_key, self.now);
                (
                    format!("impact-{}", pair.key()),
                    Category::Impact,
                    gain,
                    pitch,
                )
            }
            SoundEvent::Cue {
                name,
                category,
                gain,
                pitch,
            } => (name.clone(), *category, *gain, *pitch),
        };

        let sample = self.pick(&family)?;
        let volume = self.volumes.get(&category).copied().unwrap_or(1.0);
        // A little pitch scatter so repeated cues do not sound machine-made.
        let jitter = self.rng.range(0.97, 1.03);
        let handle = mixer.play(VoiceSpec {
            sample,
            gain: (gain * volume * placement.gain).clamp(0.0, 2.0),
            pan: placement.pan.clamp(-1.0, 1.0),
            pitch: (pitch * jitter).clamp(0.25, 4.0),
            looping: false,
            priority: category.priority(),
        })?;
        bank.pin(sample);
        self.started_this_frame += 1;
        Some(handle)
    }

    /// Choose a variant, avoiding an immediate repeat.
    fn pick(&mut self, family: &str) -> Option<SampleId> {
        let n = self.families.get(family)?.samples.len();
        if n == 0 {
            return None;
        }
        let choice = if n == 1 {
            0
        } else {
            let last = self.families.get(family).and_then(|v| v.last);
            let mut c = self.rng.below(n);
            if Some(c) == last {
                // One deterministic nudge beats a rejection loop.
                c = (c + 1 + self.rng.below(n - 1)) % n;
            }
            c
        };
        let v = self.families.get_mut(family)?;
        v.last = Some(choice);
        v.samples.get(choice).copied()
    }
}

/// Where a sound sits relative to the listener. Computed by the caller (the
/// positional maths lives in `game_script::audio3d`), so this crate does not
/// need to know about world geometry.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub gain: f32,
    pub pan: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::tests::wav;

    fn setup() -> (SampleBank, Mixer, AudioDirector, Vec<SampleId>) {
        let mut bank = SampleBank::new(44100);
        let ids: Vec<SampleId> = (0..4)
            .map(|i| bank.insert(&format!("s{i}"), &wav(2000, 44100)).unwrap())
            .collect();
        let mixer = Mixer::new(44100);
        let director = AudioDirector::new(1234);
        (bank, mixer, director, ids)
    }

    #[test]
    fn an_impact_picks_the_material_family_and_plays() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("impact-wood", ids.clone());
        d.begin_frame(0.016);
        let h = d.emit(
            &SoundEvent::Impact {
                a: Material::Wood,
                b: Material::Wood,
                speed: 5.0,
                pair_key: 1,
            },
            Placement::default(),
            &mut bank,
            &mut mixer,
        );
        assert!(h.is_some(), "impact produced no voice");
        assert_eq!(mixer.active_voices(), 1);
    }

    #[test]
    fn a_gentle_touch_is_silent() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("impact-wood", ids);
        d.begin_frame(0.016);
        let h = d.emit(
            &SoundEvent::Impact {
                a: Material::Wood,
                b: Material::Wood,
                speed: 0.1,
                pair_key: 1,
            },
            Placement::default(),
            &mut bank,
            &mut mixer,
        );
        assert!(h.is_none());
        assert_eq!(mixer.active_voices(), 0);
    }

    #[test]
    fn the_same_contact_is_throttled_by_cooldown() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("impact-metal", ids);
        let ev = SoundEvent::Impact {
            a: Material::Metal,
            b: Material::Metal,
            speed: 6.0,
            pair_key: 42,
        };
        d.begin_frame(0.016);
        assert!(d.emit(&ev, Placement::default(), &mut bank, &mut mixer).is_some());
        // Next frame, still inside the cooldown window.
        d.begin_frame(0.016);
        assert!(d.emit(&ev, Placement::default(), &mut bank, &mut mixer).is_none());
        // After the window it may sound again.
        d.begin_frame(0.2);
        assert!(d.emit(&ev, Placement::default(), &mut bank, &mut mixer).is_some());
    }

    #[test]
    fn a_collapsing_stack_cannot_machine_gun() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("impact-wood", ids);
        d.begin_frame(0.016);
        let mut played = 0;
        // 200 distinct contacts in one frame, as a toppling stack produces.
        for k in 0..200u64 {
            let ev = SoundEvent::Impact {
                a: Material::Wood,
                b: Material::Wood,
                speed: 4.0,
                pair_key: k,
            };
            if d
                .emit(&ev, Placement::default(), &mut bank, &mut mixer)
                .is_some()
            {
                played += 1;
            }
        }
        assert!(played <= 6, "per-frame cap ignored: {played}");
        assert!(d.dropped_last_frame() > 100);
    }

    #[test]
    fn variants_avoid_immediate_repeats() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("footstep", ids);
        let mut seen = Vec::new();
        for i in 0..12 {
            d.begin_frame(0.5);
            let ev = SoundEvent::Cue {
                name: "footstep".into(),
                category: Category::Movement,
                gain: 1.0,
                pitch: 1.0,
            };
            d.emit(&ev, Placement::default(), &mut bank, &mut mixer);
            seen.push(d.families["footstep"].last.unwrap());
            let _ = i;
        }
        for w in seen.windows(2) {
            assert_ne!(w[0], w[1], "the same take played twice in a row");
        }
    }

    #[test]
    fn selection_is_deterministic_for_a_given_seed() {
        let run = || {
            let (mut bank, mut mixer, mut d, ids) = setup();
            d.register("footstep", ids);
            let mut picks = Vec::new();
            for _ in 0..20 {
                d.begin_frame(0.5);
                d.emit(
                    &SoundEvent::Cue {
                        name: "footstep".into(),
                        category: Category::Movement,
                        gain: 1.0,
                        pitch: 1.0,
                    },
                    Placement::default(),
                    &mut bank,
                    &mut mixer,
                );
                picks.push(d.families["footstep"].last.unwrap());
            }
            picks
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn an_unknown_cue_is_ignored_rather_than_faked() {
        let (mut bank, mut mixer, mut d, _) = setup();
        d.begin_frame(0.016);
        let h = d.emit(
            &SoundEvent::Cue {
                name: "no-such-sound".into(),
                category: Category::Action,
                gain: 1.0,
                pitch: 1.0,
            },
            Placement::default(),
            &mut bank,
            &mut mixer,
        );
        assert!(h.is_none());
        assert!(!d.knows("no-such-sound"));
    }

    #[test]
    fn category_volume_scales_the_voice() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("shoot", ids);
        d.set_volume(Category::Action, 0.0);
        d.begin_frame(0.016);
        d.emit(
            &SoundEvent::Cue {
                name: "shoot".into(),
                category: Category::Action,
                gain: 1.0,
                pitch: 1.0,
            },
            Placement::default(),
            &mut bank,
            &mut mixer,
        );
        let out = crate::mixer::render_to_vec(&mut mixer, &bank, 512);
        assert!(
            out.iter().all(|s| s.abs() < 1e-6),
            "muted category still made noise"
        );
    }

    #[test]
    fn cooldown_map_does_not_grow_without_bound() {
        let (mut bank, mut mixer, mut d, ids) = setup();
        d.register("impact-stone", ids);
        for k in 0..500u64 {
            d.begin_frame(0.05);
            let _ = d.emit(
                &SoundEvent::Impact {
                    a: Material::Stone,
                    b: Material::Stone,
                    speed: 5.0,
                    pair_key: k,
                },
                Placement::default(),
                &mut bank,
                &mut mixer,
            );
        }
        assert!(d.cooldowns.len() < 50, "cooldowns leaked: {}", d.cooldowns.len());
    }
}
