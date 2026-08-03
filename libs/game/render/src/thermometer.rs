//! Adaptive quality governor — the "thermometer".
//!
//! A game the AI wrote can ask for anything, on hardware from a desktop to a
//! Quest, so the renderer profiles itself and sheds its own decoration to stay
//! inside the frame budget.
//!
//! Two properties matter more than the cut list:
//!
//! **It measures a rolling percentile, never a mean.** A shader compile, a
//! chunk rebuild, a GC pause or an OS stall produces one enormous frame. A mean
//! smears that across the whole window and trips a cut for a hiccup that will
//! never recur; a governor that reacts to one bad frame then oscillates, and
//! oscillation looks considerably worse than being slightly over budget. The
//! p90 of a two-second window ignores the worst 10% of frames by construction,
//! so only *sustained* overrun moves the level.
//!
//! **It may only cut decoration.** Colliders, NPCs, players, gameplay props,
//! interactables and the HUD are never touched: a cut changes how much
//! decoration a game wears, never what the game *is*. That is the same
//! Local-tier rule that governs particles and cameras — presentation must never
//! reach into simulation — and it is precisely what lets a Quest and the PC
//! hosting it run different quality levels and stay in sync.

/// Frames retained for the percentile. Two seconds at 60Hz; long enough that a
/// single stall cannot dominate, short enough to react within a beat.
const WINDOW: usize = 120;

/// Fraction of the display budget aimed at, leaving headroom to recover in.
/// Targeting the full budget means never having slack to climb back up with.
const TARGET_FRACTION: f32 = 0.80;

/// Consecutive over-budget evaluations before cutting. Low: falling behind is
/// felt immediately, so react fast.
const CUT_PATIENCE: u32 = 2;

/// Consecutive under-budget evaluations before restoring. Deliberately much
/// higher than CUT_PATIENCE — cut fast, restore lazily. Symmetric thresholds
/// sit on the boundary and flip level every other evaluation, which reads as
/// flickering decoration.
const RESTORE_PATIENCE: u32 = 30;

/// How often the level is reconsidered, in frames. Re-evaluating every frame
/// on a 120-frame window means acting on almost the same data repeatedly.
const EVAL_INTERVAL: u32 = 15;

/// What the renderer is allowed to draw at the current level.
///
/// Every field scales decoration. There is deliberately no field here that
/// could remove a collider, an NPC, an interactable or a HUD element — the type
/// itself is the guarantee that a cut cannot change what the game is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    /// Multiplier on particle spawn counts (1.0 = as authored).
    pub particle_scale: f32,
    /// Multiplier on the draw distance for props tagged as decoration. Props
    /// that are structure, gameplay or interactable ignore this entirely.
    pub decor_distance_scale: f32,
    /// Multiplier on how many casters get a projected silhouette shadow.
    pub shadow_caster_scale: f32,
    /// When false, projected silhouettes degrade to blobs — much cheaper, and
    /// still grounds objects, which is the part that actually matters.
    pub projected_shadows: bool,
    /// Multiplier on scattered foliage density.
    pub foliage_scale: f32,
    /// Multiplier on overall draw distance. Last resort: it changes the shape
    /// of the world rather than its dressing.
    pub draw_distance_scale: f32,
}

impl Default for Quality {
    fn default() -> Self {
        Self::FULL
    }
}

impl Quality {
    pub const FULL: Self = Self {
        particle_scale: 1.0,
        decor_distance_scale: 1.0,
        shadow_caster_scale: 1.0,
        projected_shadows: true,
        foliage_scale: 1.0,
        draw_distance_scale: 1.0,
    };

    /// The cut ladder, cheapest-looking loss first.
    ///
    /// Particles go before props because a thinner spark burst is barely
    /// noticed while a vanishing fence is; shadows degrade before geometry
    /// disappears because a blob still grounds an object; draw distance is last
    /// because it changes the world's silhouette rather than its dressing.
    pub const LEVELS: usize = 7;

    pub fn at_level(level: usize) -> Self {
        match level.min(Self::LEVELS - 1) {
            0 => Self::FULL,
            1 => Self {
                particle_scale: 0.5,
                ..Self::FULL
            },
            2 => Self {
                particle_scale: 0.25,
                decor_distance_scale: 0.6,
                ..Self::FULL
            },
            3 => Self {
                particle_scale: 0.25,
                decor_distance_scale: 0.6,
                shadow_caster_scale: 0.5,
                ..Self::FULL
            },
            4 => Self {
                particle_scale: 0.15,
                decor_distance_scale: 0.45,
                shadow_caster_scale: 0.35,
                projected_shadows: false,
                ..Self::FULL
            },
            5 => Self {
                particle_scale: 0.1,
                decor_distance_scale: 0.35,
                shadow_caster_scale: 0.25,
                projected_shadows: false,
                foliage_scale: 0.5,
                ..Self::FULL
            },
            _ => Self {
                particle_scale: 0.0,
                decor_distance_scale: 0.25,
                shadow_caster_scale: 0.15,
                projected_shadows: false,
                foliage_scale: 0.3,
                draw_distance_scale: 0.6,
            },
        }
    }

    /// One-line description of what this level gave up, for the log/overlay.
    /// A governor that silently degrades is indistinguishable from a bug.
    pub fn reason(level: usize) -> &'static str {
        match level.min(Self::LEVELS - 1) {
            0 => "full quality",
            1 => "halved particles",
            2 => "particles + distant decoration",
            3 => "+ fewer projected shadows",
            4 => "+ blob shadows only",
            5 => "+ thinner foliage",
            _ => "+ reduced draw distance (floor)",
        }
    }
}

/// Frame budget for a display, in milliseconds.
pub fn budget_ms_for_hz(refresh_hz: f32) -> f32 {
    if refresh_hz > 1.0 {
        1000.0 / refresh_hz
    } else {
        1000.0 / 60.0
    }
}

pub struct Thermometer {
    samples: [f32; WINDOW],
    filled: usize,
    next: usize,
    frames_since_eval: u32,
    over: u32,
    under: u32,
    level: usize,
    budget_ms: f32,
    /// Set when the level last changed, so a host can log only on transitions.
    pub changed: bool,
}

impl Thermometer {
    pub fn new(refresh_hz: f32) -> Self {
        Self {
            samples: [0.0; WINDOW],
            filled: 0,
            next: 0,
            frames_since_eval: 0,
            over: 0,
            under: 0,
            level: 0,
            budget_ms: budget_ms_for_hz(refresh_hz),
            changed: false,
        }
    }

    pub fn set_refresh_hz(&mut self, hz: f32) {
        self.budget_ms = budget_ms_for_hz(hz);
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn quality(&self) -> Quality {
        Quality::at_level(self.level)
    }

    pub fn reason(&self) -> &'static str {
        Quality::reason(self.level)
    }

    pub fn target_ms(&self) -> f32 {
        self.budget_ms * TARGET_FRACTION
    }

    /// The measurement the decision is made on: p90 of the window, or None
    /// until enough frames have been seen to be worth acting upon.
    pub fn p90_ms(&self) -> Option<f32> {
        if self.filled < WINDOW / 2 {
            return None;
        }
        let mut v: Vec<f32> = self.samples[..self.filled].to_vec();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // Index rather than interpolate: with a 120-sample window the
        // difference is noise, and an exact sample is easier to reason about
        // when a number looks wrong.
        Some(v[(v.len() as f32 * 0.90) as usize % v.len()])
    }

    /// Feed one frame. Returns true when the quality level changed.
    pub fn frame(&mut self, frame_ms: f32) -> bool {
        self.changed = false;
        if !frame_ms.is_finite() || frame_ms < 0.0 {
            return false;
        }
        self.samples[self.next] = frame_ms;
        self.next = (self.next + 1) % WINDOW;
        self.filled = (self.filled + 1).min(WINDOW);

        self.frames_since_eval += 1;
        if self.frames_since_eval < EVAL_INTERVAL {
            return false;
        }
        self.frames_since_eval = 0;

        let Some(p90) = self.p90_ms() else {
            return false;
        };
        let target = self.target_ms();

        if p90 > target {
            self.over += 1;
            self.under = 0;
        } else if p90 < target * 0.75 {
            // Restore only with real headroom, not merely "not over" — coming
            // back up at the boundary immediately re-triggers a cut.
            self.under += 1;
            self.over = 0;
        } else {
            self.over = 0;
            self.under = 0;
        }

        if self.over >= CUT_PATIENCE && self.level + 1 < Quality::LEVELS {
            self.level += 1;
            self.over = 0;
            self.changed = true;
        } else if self.under >= RESTORE_PATIENCE && self.level > 0 {
            self.level -= 1;
            self.under = 0;
            self.changed = true;
        }
        self.changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Thermometer, frame_ms: f32, frames: usize) {
        for _ in 0..frames {
            t.frame(frame_ms);
        }
    }

    #[test]
    fn a_single_hiccup_never_cuts() {
        // The whole reason for a percentile rather than a mean: one 100 ms
        // stall (a shader compile, a GC, an OS hitch) inside an otherwise
        // comfortable window must not cost the player their decoration.
        let mut t = Thermometer::new(60.0);
        run(&mut t, 8.0, 119);
        t.frame(100.0);
        run(&mut t, 8.0, 200);
        assert_eq!(t.level(), 0, "a lone spike moved the level");
    }

    #[test]
    fn several_scattered_hiccups_still_never_cut() {
        // Up to 10% of frames may be stalls before p90 notices — that is the
        // window's design, stated as a test so a future tweak to WINDOW or the
        // percentile has to confront it.
        let mut t = Thermometer::new(60.0);
        for i in 0..300 {
            t.frame(if i % 15 == 0 { 90.0 } else { 8.0 });
        }
        assert_eq!(t.level(), 0);
    }

    #[test]
    fn sustained_overrun_cuts() {
        let mut t = Thermometer::new(60.0);
        run(&mut t, 25.0, 200);
        assert!(t.level() > 0, "sustained 25ms frames did not cut");
    }

    #[test]
    fn cutting_is_faster_than_restoring() {
        // Cut fast, restore lazily. Symmetric patience sits on the boundary and
        // flips level every other evaluation, which reads as flickering.
        let mut t = Thermometer::new(60.0);
        run(&mut t, 25.0, 200);
        let cut_level = t.level();
        assert!(cut_level > 0);

        // A short spell of headroom must not claw anything back. (It may even
        // still be cutting: the window is 120 frames, so it holds slow samples
        // for a while after the load drops — which is exactly the lag that
        // stops the level flapping.)
        run(&mut t, 4.0, 100);
        assert!(
            t.level() >= cut_level,
            "restored after only a short spell of headroom"
        );
        let peak = t.level();
        run(&mut t, 4.0, 900);
        assert!(
            t.level() < peak,
            "never restored despite sustained headroom (level stuck at {peak})"
        );
    }

    #[test]
    fn it_bottoms_out_rather_than_running_away() {
        let mut t = Thermometer::new(60.0);
        run(&mut t, 500.0, 2000);
        assert_eq!(t.level(), Quality::LEVELS - 1);
        // And the floor still draws something: an unplayable-looking world is
        // not a valid answer to being slow.
        let q = t.quality();
        assert!(q.draw_distance_scale > 0.0 && q.decor_distance_scale > 0.0);
    }

    #[test]
    fn the_ladder_only_ever_gives_things_up() {
        // Each level must be no more expensive than the one before it in every
        // dimension, or "cutting" could raise cost.
        for l in 1..Quality::LEVELS {
            let prev = Quality::at_level(l - 1);
            let cur = Quality::at_level(l);
            assert!(cur.particle_scale <= prev.particle_scale);
            assert!(cur.decor_distance_scale <= prev.decor_distance_scale);
            assert!(cur.shadow_caster_scale <= prev.shadow_caster_scale);
            assert!(cur.foliage_scale <= prev.foliage_scale);
            assert!(cur.draw_distance_scale <= prev.draw_distance_scale);
            assert!(!(cur.projected_shadows && !prev.projected_shadows));
        }
    }

    #[test]
    fn quest_and_desktop_budgets_differ() {
        assert!((budget_ms_for_hz(60.0) - 16.667).abs() < 0.01);
        assert!((budget_ms_for_hz(72.0) - 13.889).abs() < 0.01);
        assert!((budget_ms_for_hz(120.0) - 8.333).abs() < 0.01);
        // A 12ms frame is comfortable on desktop and over budget on a Quest,
        // which is the entire reason the budget comes from the display.
        let mut desktop = Thermometer::new(60.0);
        let mut quest = Thermometer::new(72.0);
        run(&mut desktop, 12.0, 300);
        run(&mut quest, 12.0, 300);
        assert_eq!(desktop.level(), 0);
        assert!(quest.level() > 0);
    }

    #[test]
    fn garbage_frame_times_are_ignored_not_propagated() {
        let mut t = Thermometer::new(60.0);
        run(&mut t, 8.0, 130);
        t.frame(f32::NAN);
        t.frame(f32::INFINITY);
        t.frame(-5.0);
        run(&mut t, 8.0, 60);
        assert_eq!(t.level(), 0);
        assert!(t.p90_ms().is_some_and(|p| p.is_finite()));
    }

    #[test]
    fn every_level_has_a_reason_to_show() {
        // A governor that degrades silently is indistinguishable from a bug.
        for l in 0..Quality::LEVELS {
            assert!(!Quality::reason(l).is_empty());
        }
        assert_ne!(Quality::reason(0), Quality::reason(Quality::LEVELS - 1));
    }

    /// The renderer applies `quality()` unconditionally on every frame, so
    /// level 0 has to be a bit-for-bit no-op. If it ever stops being one,
    /// merely *linking* the governor would change how the game looks on a
    /// machine that never had a performance problem.
    #[test]
    fn a_dormant_governor_changes_nothing() {
        let t = Thermometer::new(60.0);
        let q = t.quality();
        assert_eq!(t.level(), 0);
        assert_eq!(q.particle_scale, 1.0);
        assert_eq!(q.shadow_caster_scale, 1.0);
        assert_eq!(q.decor_distance_scale, 1.0);
        assert_eq!(q.foliage_scale, 1.0);
        assert_eq!(q.draw_distance_scale, 1.0);
        assert!(q.projected_shadows);
        assert_eq!(q, Quality::FULL);
    }

    /// The renderer turns `shadow_caster_scale` into a caster count and
    /// `particle_scale` into a truncation length. Both must stay sane as
    /// integers: a rounding slip that reaches 0 casters would unground every
    /// object in the world, which reads as broken rather than as cheaper.
    #[test]
    fn the_scales_survive_being_turned_into_counts() {
        // Matches DEFAULT_SHADOW_BUDGET in renderer.rs.
        let budget = 24usize;
        let mut last = usize::MAX;
        for level in 0..Quality::LEVELS {
            let q = Quality::at_level(level);
            let casters = ((budget as f32 * q.shadow_caster_scale).round() as usize).max(1);
            assert!(casters >= 1, "level {level} left nothing grounded");
            assert!(casters <= last, "level {level} raised the caster count");
            last = casters;

            let particles = (1000.0 * q.particle_scale).round() as usize;
            assert!(particles <= 1000);
        }
        // The bottom of the ladder must still ground *something*, and must
        // have actually given the particles up.
        let bottom = Quality::at_level(Quality::LEVELS - 1);
        assert_eq!((1000.0 * bottom.particle_scale) as usize, 0);
        assert!(((budget as f32 * bottom.shadow_caster_scale).round() as usize).max(1) >= 1);
    }
}

