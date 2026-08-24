//! THE PLATTER — the VJ video transport.
//!
//! A record player has an angular VELOCITY. Position is only ever the
//! integral of it:
//!
//! ```text
//!     ω(now) = travel · ( slider ⊕ bpm-driven speed ⊕ hand speed )
//!     q     += ω · dt                       unwrapped path coordinate
//!     pos    = map(q)                       loop = sawtooth, bounce = triangle,
//!                                           once = clamp
//!     (pair, t) = locate(pos)               PTS binary search — VFR-exact
//! ```
//!
//! Every velocity term is slew-bounded; NO code path assigns `pos` except
//! [`Transport::seek`] (a cue — a deliberate needle drop) and the map, which
//! is a pure re-expression of the integral. That is what makes a position
//! discontinuity IMPOSSIBLE rather than avoided: there is nothing to
//! discontinue.
//!
//! The four inputs, all as velocities (`local/agent_state/vj-transport-
//! redesign/design-v3-amend.md`):
//!
//! 1. **Play rate** — the speed slider, natural units (1.0 = the source's own
//!    cadence).
//! 2. **Beat lock** — the transport reads the beat clock's RATE (`bpm`), never
//!    its position: one sweep of the range per `beats_per_sweep` beats. A BPM
//!    change of any size reaches the video as a speed ramp. Phase ALIGNMENT
//!    (sweep turns landing on beat edges) is a small bounded velocity TRIM
//!    toward the grid — pitch-riding a turntable into the mix — clamped to
//!    [`TRIM_MAX_FRAC`] of the speed, ramped in over [`TRIM_RAMP_BEATS`].
//!    It never moves `pos`; it only bends ω. Engage and epoch bumps are
//!    absorbed by the same trim over a few beats: no snap, ever.
//! 3. **Scratch** — the hand on the platter. Held, the platter IS the hand
//!    (its velocity, slewed at [`HAND_SLEW`] so a mouse cannot teleport the
//!    speed). Released, the hand blends back into the play/beat velocity
//!    over [`RELEASE_SECS`]. The hand's sign is TRAVEL-relative and the
//!    on-screen direction is re-read from the map's current leg every frame,
//!    so a held wheel reflects at a bounce apex exactly like playback does.
//!    The hand never clamps at the range edges: loop wraps, bounce reflects.
//! 4. **Direction** — `travel` is a sign on ω, integrated from the current
//!    q: pos is continuous, velocity reverses (the one intended C0). Reverse
//!    mode == Loop with travel −1. "Forward" always means +travel.
//!
//! Coordinates are MEDIA SECONDS on the source timeline (design-v2 §6: a
//! frame index is not enough for variable-frame-rate material). The map's
//! origin is the range's first frame; a Loop's length includes the wrap pair
//! (last → first, one frame duration), a Bounce/Once sweep runs first → last.
//!
//! Range edits RESCALE, never teleport (the ratified law): the phase on the
//! sweep is preserved and the position remaps proportionally into the new
//! window. That, too, goes through the map — it is the map's period changing.
//!
//! Pure: no threads, no atomics, no `Cx`, no wall clock. `now` is whatever
//! stamp the presenter hands in (one per display frame, both decks).

use std::fmt;

/// The scratch spring: on release, ω blends hand → play/beat over this
/// long (linear, s: 1 → 0).
pub const RELEASE_SECS: f64 = 0.3;
/// Slew bound on the HELD hand's velocity, in natural-speed units per
/// second: a wheel slammed from rest reaches 2× in 125 ms — jog-wheel
/// inertia, not a mouse teleport.
pub const HAND_SLEW: f64 = 16.0;
/// The beat trim's authority: at most this fraction of the beat-driven
/// speed, either way. Pitch-riding, invisible.
pub const TRIM_MAX_FRAC: f64 = 0.03;
/// The beat trim's time constant, in beats (exponential convergence of the
/// grid error when inside the clamp).
pub const TRIM_TAU_BEATS: f64 = 4.0;
/// The beat trim ramps in over this many beats (C1 — the correction has no
/// step of its own).
pub const TRIM_RAMP_BEATS: f64 = 0.25;
/// A long frame (pause, stall) advances the platter by at most this many
/// display periods: dt = clamp(now − last, 0, MAX_DT_PERIODS · period).
pub const MAX_DT_PERIODS: f64 = 2.0;
/// The tempo-step detector's threshold: a `bpm` change larger than this
/// between two frames is flagged as a [`Events::BPM_STEP`] (the beat clock
/// adopted a new tempo — a tap, a chip, a re-lock). Informational: the
/// transport reacts to every bpm change identically (a speed change).
const BPM_STEP_FRAC: f64 = 0.01;

/// How the map folds the unwrapped path coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Play the range once, hold at its end.
    Once,
    /// Sawtooth: wrap through the (last → first) pair.
    Loop,
    /// Triangle: reflect at both ends.
    Bounce,
}

/// The beat clock as the transport sees it, sampled by the presenter at
/// `now`: a RATE (bpm) and, for the phase trim, the continuous beat
/// position. `epoch` bumps whenever the clock's position was allowed to
/// move (start / tap) — the trim absorbs it, the platter does not snap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatInput {
    pub bpm: f64,
    /// Continuous beat position (beats since the clock's epoch).
    pub beats: f64,
    pub epoch: u64,
}

/// What happened to the transport since the previous frame — every one of
/// these is a sanctioned discontinuity (in velocity, or, for SEEK alone, in
/// position) that a continuity gate exempts.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Events(u32);

impl Events {
    pub const NONE: Events = Events(0);
    /// `seek()` — the only position write.
    pub const SEEK: Events = Events(1 << 0);
    /// travel := −travel.
    pub const FLIP: Events = Events(1 << 1);
    /// The map changed shape (loop/bounce/once); u re-anchored.
    pub const MODE: Events = Events(1 << 2);
    /// The range moved: rescaled, phase preserved.
    pub const RANGE: Events = Events(1 << 3);
    /// A new timeline (clip / cache generation) was bound.
    pub const TIMELINE: Events = Events(1 << 4);
    /// Beat lock engaged or released.
    pub const SYNC: Events = Events(1 << 5);
    /// Beats-per-sweep changed (the chip).
    pub const CADENCE: Events = Events(1 << 6);
    /// The hand took the platter.
    pub const HAND_START: Events = Events(1 << 7);
    /// The hand let go (the spring starts).
    pub const HAND_RELEASE: Events = Events(1 << 8);
    /// The speed slider moved.
    pub const SPEED: Events = Events(1 << 9);
    pub const PAUSE: Events = Events(1 << 10);
    pub const RESUME: Events = Events(1 << 11);
    /// The beat clock's tempo moved by more than [`BPM_STEP_FRAC`] in one
    /// frame (its own adoption step; the transport just follows).
    pub const BPM_STEP: Events = Events(1 << 12);
    /// The beat clock re-started (its position moved); the trim absorbs it.
    pub const EPOCH: Events = Events(1 << 13);
    /// Once mode reached the end of the range and stopped.
    pub const ONCE_END: Events = Events(1 << 14);
    /// The spring landed: the hand is fully out of the velocity.
    pub const HAND_SETTLED: Events = Events(1 << 15);

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn contains(self, e: Events) -> bool {
        self.0 & e.0 == e.0
    }
    pub fn insert(&mut self, e: Events) {
        self.0 |= e.0;
    }
    /// Events that move POSITION or velocity by design — the ones a
    /// per-frame continuity check must exempt. Informational flags
    /// (BPM_STEP, EPOCH, HAND_SETTLED) are not in it: pos and ω stay
    /// continuous through them modulo the beat clock's own step.
    pub fn exempts_continuity(self) -> bool {
        const HARD: u32 = Events::SEEK.0
            | Events::FLIP.0
            | Events::MODE.0
            | Events::RANGE.0
            | Events::TIMELINE.0
            | Events::SYNC.0
            | Events::CADENCE.0
            | Events::HAND_START.0
            | Events::HAND_RELEASE.0
            | Events::SPEED.0
            | Events::PAUSE.0
            | Events::RESUME.0
            | Events::BPM_STEP.0
            | Events::ONCE_END.0;
        self.0 & HARD != 0
    }
}

impl fmt::Debug for Events {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [(Events, &str); 16] = [
            (Events::SEEK, "seek"),
            (Events::FLIP, "flip"),
            (Events::MODE, "mode"),
            (Events::RANGE, "range"),
            (Events::TIMELINE, "timeline"),
            (Events::SYNC, "sync"),
            (Events::CADENCE, "cadence"),
            (Events::HAND_START, "hand_start"),
            (Events::HAND_RELEASE, "hand_release"),
            (Events::SPEED, "speed"),
            (Events::PAUSE, "pause"),
            (Events::RESUME, "resume"),
            (Events::BPM_STEP, "bpm_step"),
            (Events::EPOCH, "epoch"),
            (Events::ONCE_END, "once_end"),
            (Events::HAND_SETTLED, "hand_settled"),
        ];
        let mut first = true;
        for (e, name) in NAMES {
            if self.contains(e) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("-")?;
        }
        Ok(())
    }
}

impl std::ops::BitOr for Events {
    type Output = Events;
    fn bitor(self, rhs: Events) -> Events {
        Events(self.0 | rhs.0)
    }
}

/// The source's frame timeline: presentation stamps in media seconds,
/// ascending, one per resident frame; plus the duration the LAST frame is
/// shown for (the wrap pair's length — the median frame delta, which is
/// also the nominal cadence).
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    pts: Vec<f64>,
    tail: f64,
}

impl Timeline {
    /// Build from ascending stamps. Fewer than one frame is no timeline.
    /// The tail is the median positive delta (1/24 s for a single frame).
    pub fn from_pts(pts: Vec<f64>) -> Option<Timeline> {
        if pts.is_empty() || pts.iter().any(|p| !p.is_finite()) {
            return None;
        }
        if pts.windows(2).any(|w| w[1] < w[0]) {
            return None;
        }
        let mut deltas: Vec<f64> =
            pts.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).collect();
        let tail = if deltas.is_empty() {
            1.0 / 24.0
        } else {
            deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());
            deltas[deltas.len() / 2]
        };
        Some(Timeline { pts, tail })
    }

    /// From the decoder's 100 ns stamps.
    pub fn from_pts_100ns<I: IntoIterator<Item = i64>>(pts: I) -> Option<Timeline> {
        Timeline::from_pts(pts.into_iter().map(|p| p as f64 / 1e7).collect())
    }

    pub fn len(&self) -> usize {
        self.pts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }

    pub fn pts(&self, index: usize) -> f64 {
        self.pts[index.min(self.pts.len() - 1)]
    }

    /// Duration the last frame holds (the wrap pair's length).
    pub fn tail(&self) -> f64 {
        self.tail
    }

    /// Nominal source cadence, frames per second.
    pub fn fps(&self) -> f64 {
        1.0 / self.tail.max(1e-9)
    }

    /// The last frame whose stamp is ≤ `t` (0 before the first).
    pub fn index_at(&self, t: f64) -> usize {
        let n = self.pts.partition_point(|p| *p <= t);
        n.saturating_sub(1).min(self.pts.len() - 1)
    }

    /// The `[lo, hi)` frame window for a media-time window `[t_in, t_out)`,
    /// at least one frame wide — the same rule the trim handles used on the
    /// decoder's stamps: frames whose stamp is inside the window.
    pub fn window(&self, t_in: f64, t_out: f64) -> (usize, usize) {
        let n = self.pts.len();
        let lo = self.pts.partition_point(|p| *p < t_in).min(n - 1);
        let hi = self.pts.partition_point(|p| *p < t_out).clamp(lo + 1, n);
        (lo, hi)
    }
}

/// Where a media-time position lands on the frame timeline: the pair
/// `(a, b)` it sits between and the fraction `t` of the way from `a` to
/// `b`. In a Loop the last pair is `(hi − 1, lo)` — the wrap pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Locate {
    pub a: usize,
    pub b: usize,
    pub t: f64,
}

/// One frame of the platter, as ONE snapshot: position and the velocity
/// that produced it (never two values from two moments).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    /// Media seconds, inside the range.
    pub pos: f64,
    /// The platter's velocity this frame (media s / s, signed by travel):
    /// `q` advanced by exactly `omega · dt`.
    pub omega: f64,
    /// The on-screen velocity: `omega` folded through the map's leg. This
    /// is what the picture does (a bounce's back leg runs the other way).
    pub screen_vel: f64,
    /// The integration step actually used (clamped).
    pub dt: f64,
    /// Everything that happened since the previous frame.
    pub events: Events,
    /// The map's current leg (true = pos rises with q).
    pub leg_forward: bool,
    /// Once mode has reached the end of its range.
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hand {
    Idle,
    /// The platter is the hand: `v` is the slewed velocity, `target` the
    /// raw wheel reading (travel-relative, natural units).
    Held { v: f64, target: f64 },
    /// The spring: `v` is the hand's velocity at release, `s` runs 1 → 0.
    Releasing { v: f64, s: f64 },
}

/// The platter. See the module docs.
#[derive(Clone, Debug)]
pub struct Transport {
    timeline: Option<Timeline>,
    lo: usize,
    hi: usize,
    mode: Mode,
    /// +1 / −1.
    travel: f64,
    playing: bool,
    /// The unwrapped path coordinate, media seconds from the range origin.
    q: f64,
    /// The play-speed slider, natural units.
    speed: f64,
    /// Beat lock: beats per sweep, when engaged.
    sync: Option<f64>,
    /// The beat trim's current velocity (media s/s, travel-relative).
    trim_v: f64,
    hand: Hand,
    last_now: Option<f64>,
    /// Observed display period (EMA of dt), for the long-frame clamp.
    frame_period: Option<f64>,
    last_beat: Option<BeatInput>,
    /// The grid error measured at the END of the last step — q and β at
    /// the same instant. The trim acts on it one frame later; measuring
    /// it before the integration (q stale, β fresh) locked the sweep a
    /// display frame ahead of the grid.
    grid_err: Option<f64>,
    last_omega: f64,
    events: Events,
    done: bool,
}

impl Default for Transport {
    fn default() -> Self {
        Transport::new()
    }
}

/// Signed distance to the nearest multiple of 1, in (−0.5, 0.5].
fn wrap_half(x: f64) -> f64 {
    let r = x.rem_euclid(1.0);
    if r > 0.5 {
        r - 1.0
    } else {
        r
    }
}

impl Transport {
    pub fn new() -> Self {
        Transport {
            timeline: None,
            lo: 0,
            hi: 0,
            mode: Mode::Loop,
            travel: 1.0,
            playing: true,
            q: 0.0,
            speed: 1.0,
            sync: None,
            trim_v: 0.0,
            hand: Hand::Idle,
            last_now: None,
            frame_period: None,
            last_beat: None,
            grid_err: None,
            last_omega: 0.0,
            events: Events::NONE,
            done: false,
        }
    }

    // ---- the map ----------------------------------------------------------

    /// (origin, sweep length S = last − first, loop length L = S + tail).
    fn geometry(&self) -> Option<(f64, f64, f64)> {
        let tl = self.timeline.as_ref()?;
        if tl.is_empty() || self.hi <= self.lo {
            return None;
        }
        let origin = tl.pts(self.lo);
        let s = (tl.pts(self.hi - 1) - origin).max(0.0);
        let l = (s + tl.tail()).max(1e-9);
        Some((origin, s, l))
    }

    /// One sweep's length in media seconds: what `beats_per_sweep` beats
    /// span. A loop sweeps through the wrap pair; a bounce leg does not.
    fn sweep_len(&self) -> Option<f64> {
        let (_, s, l) = self.geometry()?;
        Some(match self.mode {
            Mode::Loop => l,
            Mode::Bounce | Mode::Once => s.max(1e-9),
        })
    }

    /// `q` → (pos, leg_forward).
    fn map(&self, q: f64) -> (f64, bool) {
        let Some((origin, s, l)) = self.geometry() else {
            return (0.0, true);
        };
        match self.mode {
            Mode::Loop => (origin + q.rem_euclid(l), true),
            Mode::Bounce => {
                if s <= 0.0 {
                    return (origin, true);
                }
                let r = q.rem_euclid(2.0 * s);
                if r < s {
                    (origin + r, true)
                } else {
                    (origin + 2.0 * s - r, false)
                }
            }
            Mode::Once => (origin + q.clamp(0.0, s), true),
        }
    }

    /// The phase on the map's PERIOD, in [0, 1): Loop → q/L, Bounce →
    /// q/(2S) (the leg is part of the phase), Once → q/S clamped. This is
    /// what a range edit preserves.
    fn period_phase(&self) -> f64 {
        let Some((_, s, l)) = self.geometry() else {
            return 0.0;
        };
        match self.mode {
            Mode::Loop => self.q.rem_euclid(l) / l,
            Mode::Bounce => {
                if s <= 0.0 {
                    0.0
                } else {
                    self.q.rem_euclid(2.0 * s) / (2.0 * s)
                }
            }
            Mode::Once => {
                if s <= 0.0 {
                    0.0
                } else {
                    (self.q / s).clamp(0.0, 1.0)
                }
            }
        }
    }

    /// Inverse of [`Self::period_phase`] under the CURRENT geometry.
    fn q_for_period_phase(&self, phase: f64) -> f64 {
        let Some((_, s, l)) = self.geometry() else {
            return 0.0;
        };
        match self.mode {
            Mode::Loop => phase * l,
            Mode::Bounce => phase * 2.0 * s,
            Mode::Once => phase * s,
        }
    }

    /// `pos` → q on the map's forward leg (the re-anchor used by a mode
    /// switch: travel preserved, the new map's own leg).
    fn q_for_pos_forward(&self, pos: f64) -> f64 {
        let Some((origin, s, l)) = self.geometry() else {
            return 0.0;
        };
        let p = pos - origin;
        match self.mode {
            Mode::Loop => p.rem_euclid(l),
            Mode::Bounce | Mode::Once => p.clamp(0.0, s),
        }
    }

    // ---- state readers ----------------------------------------------------

    pub fn timeline(&self) -> Option<&Timeline> {
        self.timeline.as_ref()
    }

    pub fn range(&self) -> (usize, usize) {
        (self.lo, self.hi)
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn travel_forward(&self) -> bool {
        self.travel > 0.0
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn sync(&self) -> Option<f64> {
        self.sync
    }

    pub fn hand_active(&self) -> bool {
        !matches!(self.hand, Hand::Idle)
    }

    pub fn hand_held(&self) -> bool {
        matches!(self.hand, Hand::Held { .. })
    }

    /// The current position, media seconds (the map of `q`).
    pub fn pos(&self) -> f64 {
        self.map(self.q).0
    }

    /// The unwrapped path coordinate — for traces and tests.
    pub fn q(&self) -> f64 {
        self.q
    }

    /// The map's current leg.
    pub fn leg_forward(&self) -> bool {
        self.map(self.q).1
    }

    /// The direction the PICTURE is moving: travel folded with the leg.
    pub fn screen_forward(&self) -> bool {
        self.leg_forward() == (self.travel > 0.0)
    }

    /// Phase on the SWEEP (0 → 1 per sweep, in the direction of travel).
    pub fn sweep_phase(&self) -> f64 {
        let Some(len) = self.sweep_len() else { return 0.0 };
        (self.travel * self.q / len).rem_euclid(1.0)
    }

    /// The velocity of the last step.
    pub fn omega(&self) -> f64 {
        self.last_omega
    }

    pub fn done(&self) -> bool {
        self.done
    }

    // ---- inputs: map parameters ------------------------------------------

    /// Bind a timeline (a clip, or a rebuilt cache) with its window. If a
    /// timeline was already bound the phase is PRESERVED (rescale, never
    /// teleport); a fresh clip has nothing to preserve and the caller cues
    /// it with [`Self::seek`] to the frame on screen.
    pub fn bind(&mut self, timeline: Timeline, lo: usize, hi: usize) {
        let phase = self.timeline.as_ref().map(|_| self.period_phase());
        let n = timeline.len();
        self.timeline = Some(timeline);
        self.lo = lo.min(n - 1);
        self.hi = hi.clamp(self.lo + 1, n);
        self.q = match phase {
            Some(u) => self.q_for_period_phase(u),
            None => 0.0,
        };
        self.done = false;
        self.events.insert(Events::TIMELINE);
    }

    /// Move the window. Rescales: the phase is untouched and the position
    /// remaps proportionally into the new range.
    pub fn set_range(&mut self, lo: usize, hi: usize) {
        let Some(n) = self.timeline.as_ref().map(|t| t.len()) else { return };
        let lo = lo.min(n - 1);
        let hi = hi.clamp(lo + 1, n);
        if (lo, hi) == (self.lo, self.hi) {
            return;
        }
        let phase = self.period_phase();
        self.lo = lo;
        self.hi = hi;
        self.q = self.q_for_period_phase(phase);
        self.done = false;
        self.events.insert(Events::RANGE);
    }

    /// Change the map's shape. Re-anchors q so the position is unchanged;
    /// travel is preserved (never a velocity sign flip on a mode change —
    /// the on-screen direction follows the new map's leg).
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        let pos = self.pos();
        self.mode = mode;
        self.q = self.q_for_pos_forward(pos);
        self.done = false;
        self.events.insert(Events::MODE);
    }

    // ---- inputs: velocity terms ------------------------------------------

    pub fn set_travel(&mut self, forward: bool) {
        let t = if forward { 1.0 } else { -1.0 };
        if t != self.travel {
            self.travel = t;
            self.done = false;
            self.events.insert(Events::FLIP);
        }
    }

    pub fn flip(&mut self) {
        self.set_travel(self.travel < 0.0);
    }

    pub fn set_playing(&mut self, playing: bool) {
        if playing != self.playing {
            self.playing = playing;
            self.events.insert(if playing { Events::RESUME } else { Events::PAUSE });
        }
    }

    /// The speed slider, natural units, ≥ 0 (direction is `travel`).
    pub fn set_speed(&mut self, speed: f64) {
        let speed = speed.max(0.0);
        if speed.is_finite() && (speed - self.speed).abs() > 1e-12 {
            self.speed = speed;
            self.events.insert(Events::SPEED);
        }
    }

    /// Beat lock on (`Some(beats_per_sweep)`) or off. Engaging never moves
    /// the position: the sweep continues from where it is and the trim
    /// rides it onto the grid.
    pub fn set_sync(&mut self, beats_per_sweep: Option<f64>) {
        let beats = beats_per_sweep.filter(|b| b.is_finite() && *b > 0.0);
        match (self.sync, beats) {
            (None, None) => {}
            (Some(a), Some(b)) if (a - b).abs() < 1e-12 => {}
            (Some(_), Some(_)) => {
                self.sync = beats;
                self.events.insert(Events::CADENCE);
            }
            _ => {
                self.sync = beats;
                self.trim_v = 0.0;
                self.grid_err = None;
                self.events.insert(Events::SYNC);
            }
        }
    }

    /// The hand is on the platter: `v` is the wheel's velocity in natural
    /// units, TRAVEL-relative (+ = along the current travel). Call every
    /// time the wheel reading changes; the platter follows it within
    /// [`HAND_SLEW`].
    pub fn hand_hold(&mut self, v: f64) {
        let v = if v.is_finite() { v } else { 0.0 };
        match self.hand {
            Hand::Held { target, .. } if (target - v).abs() < 1e-12 => {}
            Hand::Held { v: cur, .. } => self.hand = Hand::Held { v: cur, target: v },
            Hand::Releasing { v: cur, s } => {
                // Re-grabbed mid-spring: from where the blend is now.
                let auto = self.auto_velocity_now();
                let now_v = s * cur + (1.0 - s) * auto;
                self.hand = Hand::Held { v: now_v, target: v };
                self.trim_v = 0.0;
                self.events.insert(Events::HAND_START);
            }
            Hand::Idle => {
                // From the platter's current velocity — a grab is a slew,
                // not a step.
                let now_v = self.auto_velocity_now();
                self.hand = Hand::Held { v: now_v, target: v };
                self.trim_v = 0.0;
                self.done = false;
                self.events.insert(Events::HAND_START);
            }
        }
    }

    /// The hand lets go: the spring blends back over [`RELEASE_SECS`].
    pub fn hand_release(&mut self) {
        if let Hand::Held { v, .. } = self.hand {
            self.hand = Hand::Releasing { v, s: 1.0 };
            self.events.insert(Events::HAND_RELEASE);
        }
    }

    /// THE ONLY POSITION WRITER: a cue. Media seconds; the current leg is
    /// kept so a bounce cued on its back leg keeps travelling back.
    pub fn seek(&mut self, pos: f64) {
        let Some((origin, s, l)) = self.geometry() else { return };
        let p = pos - origin;
        let base = match self.mode {
            Mode::Loop => (self.q / l).floor() * l + p.rem_euclid(l),
            Mode::Bounce => {
                let period = 2.0 * s;
                if s <= 0.0 {
                    0.0
                } else {
                    let cycle = (self.q / period).floor() * period;
                    let p = p.clamp(0.0, s);
                    if self.leg_forward() {
                        cycle + p
                    } else {
                        cycle + period - p
                    }
                }
            }
            Mode::Once => p.clamp(0.0, s),
        };
        self.q = base;
        self.done = false;
        self.events.insert(Events::SEEK);
    }

    /// Cue to a frame of the timeline.
    pub fn seek_frame(&mut self, index: usize) {
        if let Some(tl) = self.timeline.as_ref() {
            let t = tl.pts(index);
            self.seek(t);
        }
    }

    // ---- the integrator ---------------------------------------------------

    /// The play/beat velocity as it stands (no trim), natural units ×
    /// media seconds: what a sweep runs at when nothing is holding it.
    fn base_velocity(&self, beat: Option<&BeatInput>) -> f64 {
        match (self.sync, beat, self.sweep_len()) {
            (Some(b), Some(bi), Some(len)) if bi.bpm > 0.0 && bi.bpm.is_finite() => {
                len * bi.bpm / 60.0 / b
            }
            _ => self.speed,
        }
    }

    fn auto_velocity_now(&self) -> f64 {
        self.base_velocity(self.last_beat.as_ref()) + self.trim_v
    }

    /// The sweep's phase error to the beat grid, in BEATS, in (−0.5, 0.5]:
    /// positive = the sweep is behind the grid (speed up to catch it). The
    /// grid is every beat: a `beats_per_sweep` sweep passes a beat at each
    /// 1/B of its phase, and the nearest one is the one it aligns to —
    /// which beat a turn lands on is whatever is nearest at engage.
    pub fn grid_error_beats(&self, beat: &BeatInput) -> Option<f64> {
        let b = self.sync?;
        if self.mode == Mode::Once {
            return None;
        }
        let phase_beats = self.sweep_phase() * b;
        Some(wrap_half(beat.beats - phase_beats))
    }

    /// Advance the platter to `now`. Returns ONE snapshot: the position,
    /// the velocity that produced it, and every event since the last step.
    pub fn advance(&mut self, now: f64, beat: Option<BeatInput>) -> Step {
        // dt: clamped to a couple of display periods so a pause or a stall
        // never advances the platter by the whole gap.
        let raw_dt = match self.last_now {
            Some(last) if now.is_finite() => (now - last).max(0.0),
            _ => 0.0,
        };
        if now.is_finite() {
            self.last_now = Some(now);
        }
        if raw_dt > 0.0 && raw_dt < 0.25 {
            self.frame_period = Some(match self.frame_period {
                Some(fp) => fp + (raw_dt - fp) * 0.05,
                None => raw_dt,
            });
        }
        let dt = match self.frame_period {
            Some(fp) => raw_dt.min(MAX_DT_PERIODS * fp),
            None => raw_dt.min(MAX_DT_PERIODS / 60.0),
        };

        // The beat clock's own steps, flagged for the trace.
        if let (Some(prev), Some(cur)) = (self.last_beat, beat) {
            if prev.bpm > 0.0 && ((cur.bpm / prev.bpm) - 1.0).abs() > BPM_STEP_FRAC {
                self.events.insert(Events::BPM_STEP);
            }
            if cur.epoch != prev.epoch {
                self.events.insert(Events::EPOCH);
            }
        }
        self.last_beat = beat;

        let base = self.base_velocity(beat.as_ref());

        // THE BEAT TRIM: a bounded velocity toward the grid, ramped in over
        // a quarter beat. Zero when aligned, zero while the hand holds the
        // platter, zero without a lock.
        let (trim_target, trim_ramp) = match (self.sync, beat, self.sweep_len()) {
            (Some(b), Some(bi), Some(len))
                if bi.bpm > 0.0
                    && bi.bpm.is_finite()
                    && self.playing
                    && !matches!(self.hand, Hand::Held { .. })
                    && self.mode != Mode::Once =>
            {
                let period = 60.0 / bi.bpm;
                let delta_beats = self.grid_err.unwrap_or(0.0);
                let delta_sweeps = delta_beats / b;
                // sweeps per second, at the beat-driven speed
                let v_beat_sweeps = 1.0 / (b * period);
                let cap = TRIM_MAX_FRAC * v_beat_sweeps;
                let raw = delta_sweeps / (TRIM_TAU_BEATS * period);
                let target = raw.clamp(-cap, cap) * len;
                let ramp = (cap * len) / (TRIM_RAMP_BEATS * period) * dt;
                (target, ramp)
            }
            _ => {
                // Ramp out at the same slope it ramps in with (or snap when
                // there is no beat to size the slope from).
                let ramp = match (self.sync, self.last_beat, self.sweep_len()) {
                    (Some(b), Some(bi), Some(len)) if bi.bpm > 0.0 => {
                        let period = 60.0 / bi.bpm;
                        (TRIM_MAX_FRAC / (b * period) * len) / (TRIM_RAMP_BEATS * period) * dt
                    }
                    _ => f64::INFINITY,
                };
                (0.0, ramp)
            }
        };
        let d = (trim_target - self.trim_v).clamp(-trim_ramp, trim_ramp);
        self.trim_v += if d.is_finite() { d } else { trim_target - self.trim_v };

        // THE HAND.
        let auto = base + self.trim_v;
        let v = match self.hand {
            Hand::Idle => auto,
            Hand::Held { v, target } => {
                let step = HAND_SLEW * dt;
                let nv = v + (target - v).clamp(-step, step);
                self.hand = Hand::Held { v: nv, target };
                nv
            }
            Hand::Releasing { v, s } => {
                let ns = s - dt / RELEASE_SECS;
                if ns <= 0.0 {
                    self.hand = Hand::Idle;
                    self.events.insert(Events::HAND_SETTLED);
                    auto
                } else {
                    self.hand = Hand::Releasing { v, s: ns };
                    ns * v + (1.0 - ns) * auto
                }
            }
        };

        let mut omega = if self.playing && self.timeline.is_some() { self.travel * v } else { 0.0 };

        // INTEGRATE. The only place q moves outside seek()/rescale.
        let mut done = self.done;
        if self.mode == Mode::Once {
            if let Some((_, s, _)) = self.geometry() {
                let next = self.q + omega * dt;
                // The end stop: the platter is held against it. Flagged on
                // every frame the stop intervenes — the first is the
                // (intended) velocity discontinuity of hitting it.
                if next >= s && omega > 0.0 {
                    omega = if dt > 0.0 { (s - self.q) / dt } else { 0.0 };
                    self.q = s;
                    done = true;
                    self.events.insert(Events::ONCE_END);
                } else if next <= 0.0 && omega < 0.0 {
                    omega = if dt > 0.0 { (0.0 - self.q) / dt } else { 0.0 };
                    self.q = 0.0;
                    done = true;
                    self.events.insert(Events::ONCE_END);
                } else {
                    self.q = next;
                    done = false;
                }
            }
        } else {
            self.q += omega * dt;
        }
        self.done = done;
        self.last_omega = omega;
        // The grid error for the NEXT frame's trim: q and β at this instant.
        self.grid_err = beat.as_ref().and_then(|bi| self.grid_error_beats(bi));

        let (pos, leg_forward) = self.map(self.q);
        let events = std::mem::replace(&mut self.events, Events::NONE);
        Step {
            pos,
            omega,
            screen_vel: if leg_forward { omega } else { -omega },
            dt,
            events,
            leg_forward,
            done,
        }
    }

    // ---- the picture: pair lookup ----------------------------------------

    /// `pos` → the pair it sits between and the fraction across it.
    /// Loop: the last pair is the wrap pair `(hi − 1, lo)`, `tail` long.
    /// Bounce/Once: pairs `(k, k + 1)` for `k ∈ [lo, hi − 2]`; the apex is
    /// `(hi − 2, t = 1)`.
    pub fn locate(&self, pos: f64) -> Option<Locate> {
        let tl = self.timeline.as_ref()?;
        let (origin, s, l) = self.geometry()?;
        let (lo, hi) = (self.lo, self.hi);
        let p = pos - origin;
        match self.mode {
            Mode::Loop => {
                let p = p.rem_euclid(l);
                let last = hi - 1;
                let mut a = tl.index_at(origin + p).clamp(lo, last);
                // Guard a stamp exactly at a frame with rounding below it.
                if a > lo && tl.pts(a) - origin > p {
                    a -= 1;
                }
                if a >= last {
                    let t = ((p - s) / tl.tail().max(1e-9)).clamp(0.0, 1.0);
                    Some(Locate { a: last, b: lo, t })
                } else {
                    let (pa, pb) = (tl.pts(a) - origin, tl.pts(a + 1) - origin);
                    let t = ((p - pa) / (pb - pa).max(1e-9)).clamp(0.0, 1.0);
                    Some(Locate { a, b: a + 1, t })
                }
            }
            Mode::Bounce | Mode::Once => {
                if hi - lo < 2 {
                    return Some(Locate { a: lo, b: lo, t: 0.0 });
                }
                let p = p.clamp(0.0, s);
                let mut a = tl.index_at(origin + p).clamp(lo, hi - 2);
                if a > lo && tl.pts(a) - origin > p {
                    a -= 1;
                }
                let (pa, pb) = (tl.pts(a) - origin, tl.pts(a + 1) - origin);
                let t = ((p - pa) / (pb - pa).max(1e-9)).clamp(0.0, 1.0);
                Some(Locate { a, b: a + 1, t })
            }
        }
    }

    /// The single frame nearest to `pos` (the OFF tier's picture).
    pub fn nearest(&self, pos: f64) -> Option<usize> {
        let l = self.locate(pos)?;
        Some(if l.t < 0.5 { l.a } else { l.b })
    }

    /// The pair the platter will be serving `k` pairs from now, PREDICTED
    /// ALONG THE MAP's traversal — a loop wraps through its seam, a bounce
    /// reflects at its ends — never a numeric ±k. This is what a producer
    /// prefetches for. `k` may be fractional; the direction is the current
    /// travel (a paused platter predicts along travel).
    pub fn locate_ahead(&self, k: f64) -> Option<Locate> {
        let tl = self.timeline.as_ref()?;
        let dir = if self.last_omega != 0.0 {
            self.last_omega.signum()
        } else {
            self.travel
        };
        let (pos, _) = self.map(self.q + dir * k * tl.tail());
        self.locate(pos)
    }

    /// On-screen pace in source frames per second for a step: what the
    /// AI-tier gate reads.
    pub fn pace_fps(&self, step: &Step) -> f64 {
        let fps = self.timeline.as_ref().map(|t| t.fps()).unwrap_or(0.0);
        step.screen_vel.abs() * fps
    }
}

// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beat_sync::{BeatClock, BeatTarget};

    /// xorshift64* — deterministic churn.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed.max(1))
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545F4914F6CDD1D)
        }
        fn f(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (hi - lo) * self.f()
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn chance(&mut self, p: f64) -> bool {
            self.f() < p
        }
    }

    fn cfr(n: usize, fps: f64) -> Timeline {
        Timeline::from_pts((0..n).map(|i| i as f64 / fps).collect()).unwrap()
    }

    /// A transport on a constant-rate clip, whole range, given mode.
    fn platter(n: usize, fps: f64, mode: Mode) -> Transport {
        let mut t = Transport::new();
        t.bind(cfr(n, fps), 0, n);
        t.set_mode(mode);
        t.advance(0.0, None);
        t
    }

    fn beat(bpm: f64, at: f64) -> BeatInput {
        BeatInput { bpm, beats: at * bpm / 60.0, epoch: 1 }
    }

    /// Shortest signed distance on a loop of length `l`.
    fn wrapped(d: f64, l: f64) -> f64 {
        let r = d.rem_euclid(l);
        if r > l * 0.5 {
            r - l
        } else {
            r
        }
    }

    // ---- the continuity gate under random churn ---------------------------

    /// THE CONTINUITY GATE. Random mode/range/sync/scratch/flip/BPM-step
    /// sequences on a mixed display clock. On every event-free frame the
    /// position moved by exactly the velocity the snapshot reports (the
    /// integral law: Δq == ω·dt; Δpos == the map of it), and the velocity
    /// itself moved no faster than the slew bounds allow. The literal
    /// design gate |Δpos − ω_prev·dt| ≤ max(0.05·|ω|·dt, 1e-6) is scored
    /// with ω_prev = the velocity that produced the step, and its worst
    /// ratio is printed.
    #[test]
    fn continuity_gate_holds_under_random_churn() {
        let mut worst_literal = 0.0f64;
        let mut frames = 0usize;
        let mut exempt = 0usize;
        for seed in 1..=24u64 {
            let mut rng = Rng::new(seed * 7919);
            let fps = [12.0, 24.0, 25.0, 30.0, 60.0][rng.below(5)];
            let n = 8 + rng.below(120);
            let mut t = Transport::new();
            // Some clips are VFR: jitter the stamps a little.
            let pts: Vec<f64> = (0..n)
                .map(|i| i as f64 / fps + if rng.chance(0.3) { rng.range(-0.2, 0.2) / fps } else { 0.0 })
                .collect();
            let mut pts = pts;
            pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            t.bind(Timeline::from_pts(pts).unwrap(), 0, n);
            let mut clock = BeatClock::new();
            let mut bpm = rng.range(80.0, 160.0);
            clock.start(0.0, BeatTarget::phase_only(0.0, 60.0 / bpm));
            let display = [60.0, 120.0, 30.0][rng.below(3)];
            let mut now = 0.0;
            t.advance(now, None);
            let mut prev: Option<Step> = None;
            let mut prev_q = t.q();
            for _ in 0..4000 {
                frames += 1;
                // Churn.
                let mut expect_event = false;
                if rng.chance(0.01) {
                    t.set_mode([Mode::Loop, Mode::Bounce, Mode::Once][rng.below(3)]);
                    expect_event = true;
                }
                if rng.chance(0.01) {
                    let lo = rng.below(n - 1);
                    let hi = lo + 2 + rng.below(n - lo - 1);
                    t.set_range(lo, hi.min(n));
                    expect_event = true;
                }
                if rng.chance(0.005) {
                    t.set_sync(if rng.chance(0.7) {
                        Some([1.0, 2.0, 4.0, 8.0, 16.0][rng.below(5)])
                    } else {
                        None
                    });
                    expect_event = true;
                }
                if rng.chance(0.005) {
                    t.flip();
                    expect_event = true;
                }
                if rng.chance(0.003) {
                    let (origin, _, _) = t.geometry().unwrap();
                    t.seek(origin + rng.range(0.0, n as f64 / fps));
                    expect_event = true;
                }
                if rng.chance(0.004) {
                    // The beat clock steps its tempo (a tap).
                    bpm = rng.range(80.0, 160.0);
                    clock.anchor(now, BeatTarget::phase_only(clock.position_at(now), 60.0 / bpm));
                }
                if rng.chance(0.002) {
                    // An epoch: the clock's position is allowed to move.
                    clock.pin(now, BeatTarget::phase_only(rng.range(0.0, 4.0), 60.0 / bpm));
                }
                if rng.chance(0.004) {
                    t.set_playing(rng.chance(0.8));
                    expect_event = true;
                }
                match (t.hand_held(), rng.chance(0.01)) {
                    (false, true) => {
                        t.hand_hold(rng.range(-2.0, 2.0));
                        expect_event = true;
                    }
                    (true, true) => {
                        t.hand_release();
                        expect_event = true;
                    }
                    (true, false) => {
                        // The wheel moves a little every frame.
                        if let Hand::Held { target, .. } = t.hand {
                            t.hand_hold((target + rng.range(-0.1, 0.1)).clamp(-2.5, 2.5));
                        }
                    }
                    _ => {}
                }
                // Display clock: nominal with jitter, an occasional stall.
                let jitter = rng.range(-0.15, 0.15) / display;
                let dt = if rng.chance(0.01) { 0.2 } else { 1.0 / display + jitter };
                now += dt.max(0.0);
                prev_q = t.q();
                clock.advance_to(now);
                let bi = BeatInput {
                    bpm: clock.bpm(),
                    beats: clock.position_at(now),
                    epoch: clock.epoch(),
                };
                let step = t.advance(now, Some(bi));
                // The integral law, always: q moved by exactly ω·dt (Once's
                // end clamp reports the clamped ω). `prev_q` was read after
                // the churn — a seek/mode/range re-anchors q by design.
                let dq = t.q() - prev_q;
                assert!(
                    (dq - step.omega * step.dt).abs() <= 1e-9 * (1.0 + t.q().abs()),
                    "seed {seed}: Δq {dq} != ω·dt {} (events {:?})",
                    step.omega * step.dt,
                    step.events
                );
                if let Some(p) = prev {
                    let exempt_frame = step.events.exempts_continuity() || expect_event;
                    if exempt_frame {
                        exempt += 1;
                    } else {
                        // Δpos is the map of ω·dt: on a loop, the wrapped
                        // distance; on a bounce, reflected (|Δpos| ≤ |ω|dt
                        // and exact away from the apex); once: exact.
                        let dpos = step.pos - p.pos;
                        let (_, s, l) = t.geometry().unwrap();
                        let expect = step.omega * step.dt;
                        // A step longer than half the map's period (a
                        // two-frame range under a fast hand) folds more
                        // than once: the wrapped distance is ambiguous
                        // there and only the q-space law above applies.
                        let ok = match t.mode() {
                            Mode::Loop if expect.abs() >= 0.5 * l => true,
                            Mode::Bounce if expect.abs() >= 0.5 * s => true,
                            Mode::Loop => (wrapped(dpos, l) - expect).abs() <= 1e-9 + 1e-9 * l,
                            Mode::Bounce => {
                                let same_leg = step.leg_forward == p.leg_forward;
                                if same_leg {
                                    let sign = if step.leg_forward { 1.0 } else { -1.0 };
                                    (dpos - sign * expect).abs() <= 1e-9 + 1e-9 * s
                                } else {
                                    dpos.abs() <= expect.abs() + 1e-9
                                }
                            }
                            Mode::Once => (dpos - expect).abs() <= 1e-9,
                        };
                        assert!(
                            ok,
                            "seed {seed} {:?}: Δpos {dpos:.6} vs ω·dt {expect:.6} (leg {}→{})",
                            t.mode(),
                            p.leg_forward,
                            step.leg_forward
                        );
                        // Velocity slew: the hand's bound + the spring +
                        // the trim ramp + the beat clock's own slew.
                        let period = 60.0 / bi.bpm;
                        let len = t.sweep_len().unwrap();
                        let trim_cap = t
                            .sync()
                            .map(|b| TRIM_MAX_FRAC / (b * period) * len)
                            .unwrap_or(0.0);
                        let bound = HAND_SLEW * step.dt
                            + trim_cap / (TRIM_RAMP_BEATS * period) * step.dt
                            + (step.omega.abs() + p.omega.abs()) * (step.dt / RELEASE_SECS + 0.1)
                            + 1e-9;
                        assert!(
                            (step.omega - p.omega).abs() <= bound,
                            "seed {seed}: ω stepped {} → {} in one frame (bound {bound}) events {:?}",
                            p.omega,
                            step.omega,
                            step.events
                        );
                        // The literal design gate, scored.
                        let lit = match t.mode() {
                            Mode::Loop if expect.abs() >= 0.5 * l => 0.0,
                            Mode::Bounce if expect.abs() >= 0.5 * s => 0.0,
                            Mode::Loop => (wrapped(dpos, l) - expect).abs(),
                            Mode::Bounce if step.leg_forward != p.leg_forward => 0.0,
                            Mode::Bounce => {
                                let sign = if step.leg_forward { 1.0 } else { -1.0 };
                                (dpos - sign * expect).abs()
                            }
                            Mode::Once => (dpos - expect).abs(),
                        };
                        let allow = (0.05 * step.omega.abs() * step.dt).max(1e-6);
                        worst_literal = worst_literal.max(lit / allow);
                    }
                }
                // The position is always inside the range.
                let (origin, s, l) = t.geometry().unwrap();
                let hi_edge = match t.mode() {
                    Mode::Loop => origin + l,
                    _ => origin + s,
                };
                assert!(
                    step.pos >= origin - 1e-9 && step.pos <= hi_edge + 1e-9,
                    "seed {seed}: pos {} escaped [{origin}, {hi_edge}]",
                    step.pos
                );
                // And locate() always answers with a real pair.
                let loc = t.locate(step.pos).unwrap();
                let (lo, hi) = t.range();
                assert!(loc.a >= lo && loc.a < hi && loc.b >= lo && loc.b < hi);
                assert!((0.0..=1.0).contains(&loc.t));
                prev = Some(step);
            }
        }
        println!(
            "continuity gate: {frames} frames, {exempt} event frames exempt, worst literal-gate ratio {worst_literal:.3e} (1.0 = the bound)"
        );
        assert!(worst_literal <= 1.0, "literal gate violated: ratio {worst_literal}");
    }

    // ---- bounce apex and loop seam ----------------------------------------

    /// Bounce is C0 with the travel flip at the apex: no frame moves the
    /// picture farther than |ω|·dt, and the on-screen direction reverses
    /// exactly when pos reaches the range's end — while ω keeps its sign.
    #[test]
    fn bounce_is_c0_and_flips_screen_direction_at_the_apex() {
        let n = 25;
        let fps = 24.0;
        let mut t = platter(n, fps, Mode::Bounce);
        let s = (n - 1) as f64 / fps;
        let mut now = 0.0;
        let mut prev = t.advance(now, None);
        let mut turns = Vec::new();
        for i in 0..2000 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            assert!(step.omega > 0.0, "ω keeps its sign through a bounce");
            let dpos = step.pos - prev.pos;
            assert!(dpos.abs() <= step.omega * step.dt + 1e-12, "frame {i}: |Δpos| {dpos} > ω·dt");
            if step.leg_forward != prev.leg_forward {
                // A turn happened where the map says: at an end.
                let at_end = (step.pos - s).abs() < step.omega * step.dt + 1e-9
                    || step.pos.abs() < step.omega * step.dt + 1e-9;
                assert!(at_end, "frame {i}: leg flipped at pos {} (s {s})", step.pos);
                turns.push(i);
                assert_eq!(step.screen_vel.signum(), -prev.screen_vel.signum());
            } else {
                let sign = if step.leg_forward { 1.0 } else { -1.0 };
                assert!((dpos - sign * step.omega * step.dt).abs() < 1e-12);
            }
            prev = step;
        }
        // One leg = S seconds = 60·S display frames.
        let leg = (s * 60.0).round() as usize;
        for w in turns.windows(2) {
            assert!((w[1] - w[0]) as i64 - leg as i64 <= 1 && (w[1] - w[0]) as i64 - leg as i64 >= -1);
        }
        assert!(turns.len() >= 2);
    }

    /// The loop seam serves the WRAP PAIR (last → first) with t running
    /// 0 → 1 across it, and the wrapped Δpos is exactly ω·dt.
    #[test]
    fn loop_seam_serves_the_wrap_pair() {
        let n = 10;
        let fps = 10.0; // one frame per 0.1 s: 6 display frames per pair
        let mut t = platter(n, fps, Mode::Loop);
        let l = n as f64 / fps;
        let mut now = 0.0;
        let mut prev = t.advance(now, None);
        let mut seen_wrap = false;
        let mut prev_loc = t.locate(prev.pos).unwrap();
        for _ in 0..600 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            let loc = t.locate(step.pos).unwrap();
            assert!((wrapped(step.pos - prev.pos, l) - step.omega * step.dt).abs() < 1e-9);
            if loc.a == n - 1 {
                assert_eq!(loc.b, 0, "the wrap pair is (last, first)");
                seen_wrap = true;
            }
            // Pair walk is contiguous: same pair, or the next one.
            let next_of = |a: usize| if a + 1 >= n { 0 } else { a + 1 };
            assert!(
                loc.a == prev_loc.a || loc.a == next_of(prev_loc.a),
                "pair jumped {} -> {}",
                prev_loc.a,
                loc.a
            );
            if loc.a == prev_loc.a {
                assert!(loc.t >= prev_loc.t - 1e-9, "t ran backwards inside a pair");
            }
            prev = step;
            prev_loc = loc;
        }
        assert!(seen_wrap);
    }

    // ---- the BPM step -----------------------------------------------------

    /// THE BPM-CHANGE TEST: step the beat clock 120 → 140 mid-sweep. The
    /// position is continuous (no sample moves more than one frame's
    /// worth), ω ramps with the clock's own slew (the only step is the
    /// clock's tempo adoption, flagged), and the sweep re-aligns to the
    /// grid within the trim bound.
    #[test]
    fn bpm_step_120_to_140_is_a_speed_ramp_never_a_jump() {
        for mode in [Mode::Loop, Mode::Bounce] {
            let n = 49;
            let fps = 24.0;
            let mut t = platter(n, fps, mode);
            let b = 4.0;
            t.set_sync(Some(b));
            let mut clock = BeatClock::new();
            clock.start(0.0, BeatTarget::phase_only(0.0, 0.5));
            let mut now = 0.0;
            let sample = |clock: &BeatClock, now: f64| BeatInput {
                bpm: clock.bpm(),
                beats: clock.position_at(now),
                epoch: clock.epoch(),
            };
            let mut prev = t.advance(now, Some(sample(&clock, now)));
            // Let it lock first.
            for _ in 0..(60 * 12) {
                now += 1.0 / 60.0;
                clock.advance_to(now);
                prev = t.advance(now, Some(sample(&clock, now)));
            }
            let err0 = t.grid_error_beats(&sample(&clock, now)).unwrap();
            assert!(err0.abs() < 0.02, "{mode:?}: not locked before the step: {err0}");
            // Mid-sweep: the tap. Operator regime — the tempo lands. And
            // the operator says the beat is HERE, 0.3 beats from where the
            // clock had it (an epoch: the clock's position moves; the
            // platter's may not — the trim absorbs the 0.3 beats).
            let phase = clock.position_at(now);
            clock.anchor(now, BeatTarget::phase_only(phase, 60.0 / 140.0));
            clock.pin(now, BeatTarget::phase_only(phase + 0.3, 60.0 / 140.0));
            let mut stepped = 0;
            let mut worst_dpos = 0.0f64;
            let mut worst_rel = 0.0f64;
            let mut err_trace: Vec<(f64, f64)> = Vec::new();
            let (origin, s, l) = t.geometry().unwrap();
            let mut prev_bpm = 120.0;
            for i in 0..(60 * 30) {
                now += 1.0 / 60.0;
                clock.advance_to(now);
                let bi = sample(&clock, now);
                let step = t.advance(now, Some(bi));
                let dpos = match mode {
                    Mode::Loop => wrapped(step.pos - prev.pos, l),
                    _ => step.pos - prev.pos,
                };
                worst_dpos = worst_dpos.max(dpos.abs());
                // Never more than one frame's worth in one display frame
                // (at 140 bpm, B=4: 48 frames per 1.71 s = 28 f/s → 0.47
                // frame per display frame).
                assert!(dpos.abs() <= 1.0 / fps, "{mode:?} frame {i}: Δpos {dpos} > one frame");
                let rel = ((step.omega / prev.omega) - 1.0).abs();
                let clock_rel = (bi.bpm / prev_bpm - 1.0).abs();
                if step.events.contains(Events::BPM_STEP) {
                    stepped += 1;
                    // The transport's step IS the clock's step, nothing more.
                    assert!(rel <= clock_rel + 0.04, "{mode:?}: ω stepped {rel} vs clock {clock_rel}");
                } else {
                    // Within the clock's Track slew (4%) plus the trim ramp.
                    assert!(rel <= 0.05, "{mode:?} frame {i}: |Δω/ω| {rel} > 5% without a clock step ({:?})", step.events);
                    worst_rel = worst_rel.max(rel);
                }
                prev_bpm = bi.bpm;
                err_trace.push((bi.beats, t.grid_error_beats(&bi).unwrap()));
                assert!(step.pos >= origin - 1e-9 && step.pos <= origin + s.max(l) + 1e-9);
                prev = step;
            }
            assert!(stepped >= 1, "{mode:?}: the tempo step was not seen");
            // The pin moved the clock 0.3 beats; the platter did not move
            // (checked per frame above) — the error appeared in the trace.
            let peak_err = err_trace.iter().map(|(_, e)| e.abs()).fold(0.0, f64::max);
            assert!(peak_err > 0.25, "{mode:?}: the epoch's offset never reached the trim: {peak_err}");
            // Re-alignment: the grid error converges, and never faster
            // than the trim bound (3% of the beat speed → per beat the
            // phase moves ≤ 0.03/B sweeps = 0.03 beats).
            let end_err = err_trace.last().unwrap().1;
            assert!(end_err.abs() < 0.02, "{mode:?}: grid error after 30 s: {end_err}");
            let mut worst_slope = 0.0f64;
            for w in err_trace.windows(60) {
                let (b0, e0) = w[0];
                let (b1, e1) = w[w.len() - 1];
                let db = b1 - b0;
                if db > 0.0 {
                    let slope = wrap_half(e1 - e0).abs() / db;
                    worst_slope = worst_slope.max(slope);
                }
            }
            println!(
                "bpm step {mode:?}: worst |Δpos| {:.4} frames, worst |Δω/ω| off-step {:.4}, re-align slope {:.4} beats/beat, final err {:.4}",
                worst_dpos * fps,
                worst_rel,
                worst_slope,
                end_err
            );
            assert!(worst_slope <= TRIM_MAX_FRAC + 0.005, "{mode:?}: re-aligned faster than the trim bound: {worst_slope}");
        }
    }

    /// A detector-style tempo change (not an operator) is believed by the
    /// clock only after its claim holds — and when it lands, the transport
    /// follows it as a speed change with the position continuous.
    #[test]
    fn detector_tempo_change_is_continuous_too() {
        let mut t = platter(49, 24.0, Mode::Loop);
        t.set_sync(Some(4.0));
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, 0.5));
        let mut now = 0.0;
        let l = 49.0 / 24.0;
        let mut prev = t.advance(now, Some(beat(120.0, 0.0)));
        for i in 0..(60 * 20) {
            now += 1.0 / 60.0;
            clock.advance_to(now);
            if i > 60 * 3 {
                clock.discipline(now, BeatTarget::phase_only(clock.position_at(now), 60.0 / 140.0));
            }
            let bi = BeatInput { bpm: clock.bpm(), beats: clock.position_at(now), epoch: clock.epoch() };
            let step = t.advance(now, Some(bi));
            let dpos = wrapped(step.pos - prev.pos, l);
            assert!((dpos - step.omega * step.dt).abs() < 1e-9, "pos is the integral, always");
            assert!(dpos.abs() < 1.0 / 24.0);
            prev = step;
        }
        assert!((clock.bpm() - 140.0).abs() < 1.0, "the clock adopted the claim: {}", clock.bpm());
        assert!((t.omega() - l * 140.0 / 60.0 / 4.0).abs() < l * 0.05, "ω follows bpm: {}", t.omega());
    }

    // ---- scratch ----------------------------------------------------------

    /// THE SCRATCH TEST: a synthetic wheel trace across a bounce apex and a
    /// loop seam. Continuity throughout; the hand keeps its sign but the
    /// picture's direction follows the leg — reflecting at the apex,
    /// wrapping at the seam.
    #[test]
    fn scratch_wheel_follows_the_leg_across_apex_and_seam() {
        // Bounce: hold the wheel at +1.5 (along travel) through the apex.
        let n = 13;
        let fps = 24.0;
        let s = (n - 1) as f64 / fps;
        let mut t = platter(n, fps, Mode::Bounce);
        t.seek(s * 0.7);
        let mut now = 0.0;
        let mut prev = t.advance(now, None);
        t.hand_hold(1.5);
        let mut flipped = false;
        for i in 0..240 {
            now += 1.0 / 60.0;
            // A little wobble, always positive: the hand never lets go.
            t.hand_hold(1.5 + 0.3 * (i as f64 * 0.2).sin());
            let step = t.advance(now, None);
            assert!(step.omega > 0.0, "the hand's sign is the hand's");
            let dpos = step.pos - prev.pos;
            assert!(dpos.abs() <= step.omega * step.dt + 1e-12, "frame {i}: |Δpos| > |ω|dt during a scratch");
            if step.leg_forward != prev.leg_forward {
                flipped = true;
                assert!((step.pos - s).abs() <= step.omega * step.dt || step.pos <= step.omega * step.dt);
            } else {
                let sign = if step.leg_forward { 1.0 } else { -1.0 };
                assert!((dpos - sign * step.omega * step.dt).abs() < 1e-12);
            }
            prev = step;
        }
        assert!(flipped, "the wheel never reflected at the apex");

        // Loop: wheel BACKWARD through the seam (pos 0 → wraps to the end).
        let mut t = platter(n, fps, Mode::Loop);
        let l = n as f64 / fps;
        t.seek(0.05);
        let mut now = 0.0;
        t.advance(now, None);
        t.hand_hold(-1.0);
        // A grab is a slew: give the hand its 125 ms to take the platter.
        for _ in 0..10 {
            now += 1.0 / 60.0;
            t.advance(now, None);
        }
        let mut prev = t.advance(now, None);
        let mut wrapped_once = false;
        for i in 0..120 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            assert!(step.omega <= 0.0);
            let dpos = wrapped(step.pos - prev.pos, l);
            assert!((dpos - step.omega * step.dt).abs() < 1e-9, "frame {i}: seam broke continuity");
            if step.pos > prev.pos {
                wrapped_once = true;
                let loc = t.locate(step.pos).unwrap();
                assert_eq!((loc.a, loc.b), (n - 1, 0), "backward through the seam lands in the wrap pair");
            }
            prev = step;
        }
        assert!(wrapped_once);

        // Reverse mode (travel −1): a POSITIVE wheel moves the picture
        // backward — forward is the current travel direction.
        let mut t = platter(n, fps, Mode::Loop);
        t.set_travel(false);
        t.seek(0.3);
        let mut now = 0.0;
        let p0 = t.advance(now, None).pos;
        t.hand_hold(1.0);
        for _ in 0..6 {
            now += 1.0 / 60.0;
            t.advance(now, None);
        }
        assert!(t.pos() < p0, "a positive wheel on a reversed platter runs backward");
        assert!(!t.screen_forward());
    }

    /// Release is a spring: the platter blends back to its own speed over
    /// 300 ms, smoothly, and lands exactly on it.
    #[test]
    fn hand_release_springs_back_over_300ms() {
        let mut t = platter(48, 24.0, Mode::Loop);
        let mut now = 0.0;
        t.advance(now, None);
        t.hand_hold(-2.0);
        for _ in 0..60 {
            now += 1.0 / 60.0;
            t.advance(now, None);
        }
        assert!((t.omega() + 2.0).abs() < 1e-9, "held: the platter is the hand ({})", t.omega());
        t.hand_release();
        let mut prev = t.omega();
        let mut settled_at = None;
        for i in 0..60 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            assert!(step.omega >= prev - 1e-9, "the spring is monotonic");
            assert!((step.omega - prev).abs() <= 3.0 * (1.0 / 60.0) / RELEASE_SECS + 1e-9);
            if step.events.contains(Events::HAND_SETTLED) {
                settled_at = Some(i);
            }
            prev = step.omega;
        }
        let settled = settled_at.expect("the spring landed");
        assert!((17..=19).contains(&settled), "landed at frame {settled}, expected ~18 (0.3 s)");
        assert!((t.omega() - 1.0).abs() < 1e-12, "back on the slider: {}", t.omega());
    }

    // ---- the six ported sweep tests ---------------------------------------

    /// One direction sweep = one beat step, at ANY range width: the time
    /// between turns depends only on beats-per-sweep and the tempo, never
    /// on the window.
    #[test]
    fn one_sweep_is_one_beat_step_at_any_range_width() {
        for mode in [Mode::Loop, Mode::Bounce] {
            let mut cadences = Vec::new();
            for (lo, hi) in [(10usize, 14usize), (0, 400)] {
                let mut t = Transport::new();
                t.bind(cfr(400, 24.0), lo, hi);
                t.set_mode(mode);
                t.set_sync(Some(1.0));
                let mut now = 0.0;
                let mut prev = t.advance(now, Some(beat(120.0, now)));
                let mut turns = Vec::new();
                for i in 0..1200 {
                    now += 1.0 / 60.0;
                    let step = t.advance(now, Some(beat(120.0, now)));
                    let turned = match mode {
                        Mode::Loop => step.pos < prev.pos,
                        _ => step.leg_forward != prev.leg_forward,
                    };
                    if turned {
                        turns.push(i);
                    }
                    prev = step;
                }
                let gaps: Vec<usize> = turns.windows(2).map(|w| w[1] - w[0]).collect();
                assert!(gaps.len() > 10, "{mode:?} {lo}..{hi}: no cadence");
                cadences.push(gaps);
            }
            // 0.5 s beat at 60 Hz = 30 display frames per sweep, ±1 for
            // frame quantization, at both widths.
            for gaps in &cadences {
                for g in gaps {
                    assert!((*g as i64 - 30).abs() <= 1, "{mode:?}: sweep took {g} frames, law says 30");
                }
            }
        }
    }

    /// Fractional beat steps keep an exact long-run cadence: the turn
    /// costs zero time (overshoot carried through the map), so nothing
    /// accumulates.
    #[test]
    fn fractional_beat_steps_keep_exact_long_run_cadence() {
        let mut t = platter(60, 24.0, Mode::Bounce);
        t.set_sync(Some(1.0));
        let bpm = 133.7; // 0.4487 s: 26.9 display frames — nothing divides
        let mut now = 0.0;
        let mut prev = t.advance(now, Some(beat(bpm, now)));
        let mut turns: Vec<f64> = Vec::new();
        for _ in 0..20_000 {
            now += 1.0 / 60.0;
            let step = t.advance(now, Some(beat(bpm, now)));
            if step.leg_forward != prev.leg_forward {
                turns.push(now);
            }
            prev = step;
        }
        let measured = (turns.last().unwrap() - turns[0]) / (turns.len() - 1) as f64;
        let expect = 60.0 / bpm;
        assert!(
            (measured - expect).abs() < 1e-3 * expect,
            "cadence drifted: measured {measured:.5} s/sweep, law says {expect:.5}"
        );
    }

    /// The chip is CADENCE: beats-per-sweep scales the time per sweep and
    /// nothing else — the map, and the position at a given phase, are
    /// untouched.
    #[test]
    fn chip_changes_cadence_only() {
        for (beats, want_frames) in [(0.5f64, 15usize), (1.0, 30), (2.0, 60), (4.0, 120)] {
            let mut t = platter(100, 24.0, Mode::Loop);
            t.set_sync(Some(beats));
            let mut now = 0.0;
            let mut prev = t.advance(now, Some(beat(120.0, now)));
            let mut turns = Vec::new();
            for i in 0..1500 {
                now += 1.0 / 60.0;
                let step = t.advance(now, Some(beat(120.0, now)));
                if step.pos < prev.pos {
                    turns.push(i);
                }
                prev = step;
            }
            for w in turns.windows(2) {
                assert!(
                    ((w[1] - w[0]) as i64 - want_frames as i64).abs() <= 1,
                    "chip {beats}: sweep took {} frames, expected {want_frames}",
                    w[1] - w[0]
                );
            }
        }
        // The mapping is untouched by the chip: mid-sweep is mid-range.
        for beats in [1.0, 4.0, 16.0] {
            let mut t = platter(101, 24.0, Mode::Loop);
            t.set_sync(Some(beats));
            t.q = 0.5 * t.sweep_len().unwrap();
            let loc = t.locate(t.pos()).unwrap();
            assert_eq!(loc.a, 50, "chip {beats} moved the map");
        }
    }

    /// A LIVE TRIM RESCALES, never teleports: the phase is the state, so
    /// the position remaps proportionally into the new window and the
    /// motion carries on. The mirrored bounce leg remaps the same way;
    /// degenerate windows never escape or panic.
    #[test]
    fn trim_rescales_the_sweep_without_teleport() {
        let fps = 24.0;
        // Loop, mid-sweep (phase 0.5): the position is the middle of ANY
        // window.
        let mut t = platter(101, fps, Mode::Loop);
        t.q = 0.5 * t.sweep_len().unwrap();
        let mid = |t: &Transport| {
            let (origin, _, l) = t.geometry().unwrap();
            (t.pos() - origin) / l
        };
        assert!((mid(&t) - 0.5).abs() < 1e-9);
        t.set_range(20, 41);
        assert!((mid(&t) - 0.5).abs() < 1e-9, "20..41: {}", mid(&t));
        assert_eq!(t.locate(t.pos()).unwrap().a, 30);
        t.set_range(10, 12);
        assert!((mid(&t) - 0.5).abs() < 1e-9);
        assert_eq!(t.locate(t.pos()).unwrap().a, 11);
        // Bounce on the mirrored leg, a quarter in: pos = 75% of the window.
        let mut t = platter(101, fps, Mode::Bounce);
        let s = t.sweep_len().unwrap();
        t.q = s + 0.25 * s; // back leg, 25% in
        assert!(!t.leg_forward());
        let frac = |t: &Transport| {
            let (origin, s, _) = t.geometry().unwrap();
            (t.pos() - origin) / s
        };
        assert!((frac(&t) - 0.75).abs() < 1e-9);
        assert_eq!(t.locate(t.pos()).unwrap().a, 75);
        t.set_range(20, 41);
        assert!(!t.leg_forward(), "the leg survives a trim");
        assert!((frac(&t) - 0.75).abs() < 1e-9, "20..41: {}", frac(&t));
        assert_eq!(t.locate(t.pos()).unwrap().a, 35);
        // Degenerate windows.
        let mut t = platter(6, fps, Mode::Bounce);
        t.set_range(5, 6);
        t.q = 0.7;
        let loc = t.locate(t.pos()).unwrap();
        assert_eq!((loc.a, loc.b), (5, 5));
        for _ in 0..10 {
            t.advance(0.1, None);
        }
        let mut t = platter(9, fps, Mode::Loop);
        t.set_range(3, 9);
        let (origin, _, l) = t.geometry().unwrap();
        t.q = l - 1e-9;
        assert_eq!(t.locate(t.pos()).unwrap().a, 8);
        assert!(t.pos() >= origin);
    }

    /// Bounce alternates direction each sweep, wrap restarts forward — and
    /// the apex never dwells: no two consecutive display frames stand
    /// still (a reflection can halve one frame's step, never more).
    #[test]
    fn bounce_alternates_and_never_pauses_at_the_apex() {
        let mut t = platter(48, 24.0, Mode::Bounce);
        t.set_sync(Some(1.0));
        let mut now = 0.0;
        let mut prev = t.advance(now, Some(beat(120.0, now)));
        let mut dirs = Vec::new();
        let mut still = 0;
        let mut worst_still = 0;
        for _ in 0..1200 {
            now += 1.0 / 60.0;
            let step = t.advance(now, Some(beat(120.0, now)));
            if step.leg_forward != prev.leg_forward {
                dirs.push(step.leg_forward);
            }
            let moved = (step.pos - prev.pos).abs();
            if moved < 0.5 * step.omega.abs() * step.dt {
                still += 1;
                worst_still = worst_still.max(still);
            } else {
                still = 0;
            }
            prev = step;
        }
        for w in dirs.windows(2) {
            assert_ne!(w[0], w[1], "bounce failed to alternate");
        }
        assert!(dirs.len() >= 30);
        assert!(worst_still <= 1, "the platter stood still for {worst_still} frames — a pause");
        // Loop mode: always forward.
        let mut t = platter(48, 24.0, Mode::Loop);
        t.set_sync(Some(1.0));
        let mut now = 0.0;
        for _ in 0..600 {
            now += 1.0 / 60.0;
            let step = t.advance(now, Some(beat(120.0, now)));
            assert!(step.leg_forward && step.screen_vel > 0.0, "a wrap-mode sweep ran backward");
        }
    }

    /// The trim is the beat lock's ONLY corrective authority: bounded to
    /// 3% of the beat speed, zero when aligned, and it converges an
    /// engage offset onto the grid over a few beats — never a snap. A
    /// 4-beat sweep passes a beat at every quarter of its phase: 0.25 IS
    /// the grid (no correction), 0.30 pulls back.
    #[test]
    fn trim_is_bounded_zero_when_aligned_and_convergent() {
        // Zero when aligned, bounded everywhere.
        for b in [1.0f64, 2.0, 4.0, 8.0] {
            let mut t = platter(49, 24.0, Mode::Loop);
            t.set_sync(Some(b));
            let len = t.sweep_len().unwrap();
            let v_beat = len * 120.0 / 60.0 / b;
            let mut now = 0.0;
            t.advance(now, Some(beat(120.0, now)));
            assert_eq!(t.grid_error_beats(&beat(120.0, now)).unwrap(), 0.0);
            for phase in [0.01f64, 0.13, 0.35, 0.49, 0.5, 0.77, 0.99] {
                t.q = phase * len;
                // Let the ramp reach its target.
                for _ in 0..120 {
                    now += 1.0 / 60.0;
                    // Freeze the clock at a beat edge so the error is the
                    // phase's own; measure the trim's magnitude.
                    t.q = phase * len;
                    t.advance(now, Some(BeatInput { bpm: 120.0, beats: 0.0, epoch: 1 }));
                }
                assert!(
                    t.trim_v.abs() <= TRIM_MAX_FRAC * v_beat + 1e-12,
                    "b {b} phase {phase}: trim {} out of authority ({})",
                    t.trim_v,
                    TRIM_MAX_FRAC * v_beat
                );
            }
        }
        // 0.25 IS the grid on a 4-beat sweep; 0.30 pulls back.
        let mut t = platter(49, 24.0, Mode::Loop);
        t.set_sync(Some(4.0));
        let len = t.sweep_len().unwrap();
        t.q = 0.25 * len;
        assert_eq!(t.grid_error_beats(&BeatInput { bpm: 120.0, beats: 0.0, epoch: 1 }).unwrap(), 0.0);
        t.q = 0.30 * len;
        assert!(t.grid_error_beats(&BeatInput { bpm: 120.0, beats: 0.0, epoch: 1 }).unwrap() < 0.0);
        // Convergence: engage 8% of a sweep off the grid on a 1-beat sweep
        // and walk onto it — monotonically, within the bound, no snap.
        let mut t = platter(49, 24.0, Mode::Loop);
        t.set_sync(Some(1.0));
        let len = t.sweep_len().unwrap();
        let mut now = 0.0;
        t.advance(now, Some(beat(120.0, now)));
        t.q = 0.08 * len;
        let mut err = t.grid_error_beats(&beat(120.0, now)).unwrap();
        assert!((err + 0.08).abs() < 1e-9, "engage error {err}");
        let mut locked_at = None;
        let mut prev_pos = t.pos();
        for _ in 0..(60 * 20) {
            now += 1.0 / 60.0;
            let step = t.advance(now, Some(beat(120.0, now)));
            let d = wrapped(step.pos - prev_pos, len);
            assert!((d - step.omega * step.dt).abs() < 1e-9, "a nudge moved the position");
            prev_pos = step.pos;
            let e = t.grid_error_beats(&beat(120.0, now)).unwrap();
            assert!(e.abs() <= err.abs() + 1e-6, "error grew: {err} -> {e}");
            err = e;
            // Locked = within 1% of a beat (5 ms at 120: a third of a
            // display frame). Inside the clamp the law is a 4-beat time
            // constant: ln(8) · 4 ≈ 8.3 beats from 8% off.
            if locked_at.is_none() && e.abs() < 0.01 {
                locked_at = Some(now);
            }
        }
        let locked = locked_at.expect("never locked");
        println!("trim: an 8%-off engage on a 1-beat sweep locked (<1% beat) in {:.1} beats", locked * 2.0);
        assert!(locked * 2.0 < 12.0, "took {:.1} beats to lock", locked * 2.0);
        // And the worst case: half a beat off, any chip. Bounded by the 3%
        // authority: (0.5 − 0.12) beats / (0.03 beats per beat) ≈ 13 beats
        // at the clamp, then the exponential tail (~10 more to 1%).
        for b in [1.0, 4.0, 16.0] {
            let mut t = platter(49, 24.0, Mode::Loop);
            t.set_sync(Some(b));
            let len = t.sweep_len().unwrap();
            let mut now = 0.0;
            t.advance(now, Some(beat(120.0, now)));
            t.q = 0.5 / b * len;
            let mut locked_at = None;
            for _ in 0..(60 * 60) {
                now += 1.0 / 60.0;
                t.advance(now, Some(beat(120.0, now)));
                let e = t.grid_error_beats(&beat(120.0, now)).unwrap();
                if e.abs() < 0.01 {
                    locked_at = Some(now);
                    break;
                }
            }
            let beats = locked_at.expect("never locked") * 2.0;
            println!("trim: a half-beat engage offset at chip {b} locked (<1% beat) in {beats:.1} beats");
            assert!(beats < 30.0);
        }
    }

    // ---- laws -------------------------------------------------------------

    /// seek() is the only position writer: every other input leaves pos
    /// exactly where it is (range edits preserve the PHASE instead).
    #[test]
    fn seek_is_the_only_position_writer() {
        let mut t = platter(49, 24.0, Mode::Bounce);
        let mut now = 0.0;
        t.seek(0.7);
        for _ in 0..20 {
            now += 1.0 / 60.0;
            t.advance(now, Some(beat(120.0, now)));
        }
        let p = t.pos();
        t.set_mode(Mode::Loop);
        assert_eq!(t.pos(), p);
        t.set_mode(Mode::Once);
        assert_eq!(t.pos(), p);
        t.set_mode(Mode::Bounce);
        assert_eq!(t.pos(), p);
        t.set_sync(Some(4.0));
        assert_eq!(t.pos(), p);
        t.set_sync(Some(2.0));
        assert_eq!(t.pos(), p);
        t.set_sync(None);
        assert_eq!(t.pos(), p);
        t.flip();
        assert_eq!(t.pos(), p);
        t.flip();
        assert_eq!(t.pos(), p);
        t.set_speed(2.0);
        assert_eq!(t.pos(), p);
        t.hand_hold(1.0);
        assert_eq!(t.pos(), p);
        t.hand_release();
        assert_eq!(t.pos(), p);
        t.set_playing(false);
        assert_eq!(t.pos(), p);
        // A zero-dt advance moves nothing either.
        t.set_playing(true);
        let step = t.advance(now, Some(beat(120.0, now)));
        assert_eq!(step.pos, p);
        assert_eq!(step.dt, 0.0);
        // Range: phase preserved, position rescaled — by the map, not a
        // write.
        let u = t.period_phase();
        t.set_range(10, 30);
        assert!((t.period_phase() - u).abs() < 1e-12);
        // Rebind (a rebuilt cache): phase preserved too.
        let u = t.period_phase();
        t.bind(cfr(97, 48.0), 0, 97);
        assert!((t.period_phase() - u).abs() < 1e-12);
        // And seek: moves it, says so.
        t.seek(0.1);
        assert!((t.pos() - 0.1).abs() < 1e-12);
        let step = t.advance(now, None);
        assert!(step.events.contains(Events::SEEK));
    }

    #[test]
    fn once_plays_to_the_end_and_holds() {
        let mut t = platter(25, 24.0, Mode::Once);
        let s = 24.0 / 24.0;
        let mut now = 0.0;
        let mut ended = None;
        for i in 0..120 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            if step.events.contains(Events::ONCE_END) {
                if ended.is_none() {
                    ended = Some(i);
                    // The end frame reports the clamped partial step.
                    assert!(step.omega >= 0.0 && step.omega <= 1.0);
                } else {
                    // Held against the stop: flagged, motionless.
                    assert_eq!(step.omega, 0.0);
                }
            } else {
                assert!(ended.is_none(), "the stop stopped saying so");
            }
            if ended.is_some() {
                assert_eq!(step.pos, s);
                assert!(step.done);
            }
        }
        assert!((59..=60).contains(&ended.unwrap()), "ended at frame {:?}", ended);
        // The apex pair.
        let loc = t.locate(t.pos()).unwrap();
        assert_eq!((loc.a, loc.b), (23, 24));
        assert_eq!(loc.t, 1.0);
        // A flip plays it back.
        t.flip();
        for _ in 0..30 {
            now += 1.0 / 60.0;
            t.advance(now, None);
        }
        assert!(t.pos() < s && t.pos() > 0.0);
        assert!(!t.done());
    }

    #[test]
    fn paused_platter_does_not_move_and_long_frames_are_clamped() {
        let mut t = platter(48, 24.0, Mode::Loop);
        let mut now = 0.0;
        for _ in 0..30 {
            now += 1.0 / 60.0;
            t.advance(now, None);
        }
        t.set_playing(false);
        let p = t.pos();
        for _ in 0..30 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            assert_eq!(step.pos, p);
            assert_eq!(step.omega, 0.0);
        }
        t.set_playing(true);
        // A 2 s gap advances at most two display periods.
        now += 2.0;
        let step = t.advance(now, None);
        assert!(step.dt <= MAX_DT_PERIODS / 60.0 * 1.01, "dt {} not clamped", step.dt);
        assert!((step.pos - p - step.dt).abs() < 1e-9);
    }

    #[test]
    fn locate_is_exact_on_a_vfr_timeline() {
        let pts = vec![0.0, 0.04, 0.08, 0.20, 0.22, 0.30];
        let tl = Timeline::from_pts(pts.clone()).unwrap();
        assert!((tl.tail() - 0.04).abs() < 1e-12, "median delta");
        let mut t = Transport::new();
        t.bind(tl, 0, 6);
        t.set_mode(Mode::Bounce);
        for (k, p) in pts.iter().enumerate().take(5) {
            let loc = t.locate(*p).unwrap();
            assert_eq!((loc.a, loc.b), (k, k + 1));
            assert!(loc.t.abs() < 1e-12);
        }
        let loc = t.locate(0.14).unwrap();
        assert_eq!((loc.a, loc.b), (2, 3));
        assert!((loc.t - 0.5).abs() < 1e-9, "half-way through a long VFR gap");
        let loc = t.locate(0.21).unwrap();
        assert_eq!((loc.a, loc.b), (3, 4));
        assert!((loc.t - 0.5).abs() < 1e-9);
        // Loop: the wrap pair is `tail` long.
        t.set_mode(Mode::Loop);
        let loc = t.locate(0.32).unwrap();
        assert_eq!((loc.a, loc.b), (5, 0));
        assert!((loc.t - 0.5).abs() < 1e-9);
        assert_eq!(t.nearest(0.31).unwrap(), 5);
        assert_eq!(t.nearest(0.335).unwrap(), 0);
        // The window helper matches the decoder-stamp rule.
        assert_eq!(t.timeline().unwrap().window(0.05, 0.25), (2, 5));
        assert_eq!(t.timeline().unwrap().window(0.0, 1.0), (0, 6));
        assert_eq!(t.timeline().unwrap().window(0.5, 0.6), (5, 6));
    }

    /// TWO DECKS, ONE LAW (design-v2 §8 step 7): two transports fed the
    /// identical input sequence — the same stamps, the same beat clock,
    /// the same churn — produce BIT-IDENTICAL state series. There is no
    /// hidden per-deck state, no wall clock, no allocation ordering: a
    /// deck cannot perturb its neighbour because nothing they touch is
    /// shared.
    #[test]
    fn two_decks_with_identical_inputs_are_bit_identical() {
        let mut rng = Rng::new(0xD0C5);
        let build = || {
            let mut t = Transport::new();
            t.bind(cfr(72, 12.0), 0, 72);
            t
        };
        let (mut a, mut b) = (build(), build());
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, 0.5));
        let mut now = 0.0;
        for step in 0..8000 {
            // The same churn to both, decided once.
            if rng.chance(0.01) {
                let m = [Mode::Loop, Mode::Bounce, Mode::Once][rng.below(3)];
                a.set_mode(m);
                b.set_mode(m);
            }
            if rng.chance(0.008) {
                let s = if rng.chance(0.7) { Some(4.0) } else { None };
                a.set_sync(s);
                b.set_sync(s);
            }
            if rng.chance(0.005) {
                a.flip();
                b.flip();
            }
            if rng.chance(0.01) {
                let v = rng.range(-2.0, 2.0);
                a.hand_hold(v);
                b.hand_hold(v);
            } else if rng.chance(0.01) {
                a.hand_release();
                b.hand_release();
            }
            if rng.chance(0.003) {
                let p = rng.range(0.0, 5.9);
                a.seek(p);
                b.seek(p);
            }
            now += 1.0 / 60.0 + rng.range(-0.001, 0.001);
            clock.advance_to(now);
            let bi = BeatInput {
                bpm: clock.bpm(),
                beats: clock.position_at(now),
                epoch: clock.epoch(),
            };
            let sa = a.advance(now, Some(bi));
            let sb = b.advance(now, Some(bi));
            assert!(
                sa == sb && a.q().to_bits() == b.q().to_bits(),
                "step {step}: decks diverged: {sa:?} vs {sb:?}"
            );
        }
    }

    /// Prefetch prediction follows the MAP: through a loop's seam (the
    /// wrap pair, then the head), reflected at a bounce apex, and along
    /// the current travel when reversed.
    #[test]
    fn locate_ahead_follows_the_traversal() {
        // Loop, forward, standing in the second-to-last pair.
        let mut t = platter(10, 10.0, Mode::Loop);
        t.seek(0.85); // pair 8
        t.advance(0.0, None);
        t.advance(1.0 / 60.0, None);
        assert_eq!(t.locate(t.pos()).unwrap().a, 8);
        assert_eq!(t.locate_ahead(1.0).unwrap().a, 9, "into the wrap pair");
        assert_eq!(t.locate_ahead(2.0).unwrap().a, 0, "through the seam");
        assert_eq!(t.locate_ahead(3.0).unwrap().a, 1);
        // Reverse (travel −1): ahead means the OTHER way.
        let mut t = platter(10, 10.0, Mode::Loop);
        t.set_travel(false);
        t.seek(0.15); // pair 1
        t.advance(0.0, None);
        t.advance(1.0 / 60.0, None);
        assert_eq!(t.locate_ahead(1.0).unwrap().a, 0);
        assert_eq!(t.locate_ahead(2.0).unwrap().a, 9, "backward through the seam");
        // Bounce, forward, near the apex: reflected, never past the end.
        let mut t = platter(10, 10.0, Mode::Bounce);
        t.seek(0.75); // pair 7 of 0..=8
        t.advance(0.0, None);
        t.advance(1.0 / 60.0, None);
        assert_eq!(t.locate_ahead(1.0).unwrap().a, 8);
        let back = t.locate_ahead(3.0).unwrap();
        assert_eq!(back.a, 7, "reflected at the apex");
        assert!(t.locate_ahead(9.0).unwrap().a <= 8);
    }

    #[test]
    fn a_grab_is_a_slew_not_a_step() {
        let mut t = platter(48, 24.0, Mode::Loop);
        let mut now = 0.0;
        t.advance(now, None);
        t.hand_hold(-2.0);
        let mut prev = 1.0;
        for _ in 0..30 {
            now += 1.0 / 60.0;
            let step = t.advance(now, None);
            assert!((step.omega - prev).abs() <= HAND_SLEW / 60.0 + 1e-9);
            prev = step.omega;
        }
        assert!((t.omega() + 2.0).abs() < 1e-9, "reached the wheel: {}", t.omega());
    }
}
