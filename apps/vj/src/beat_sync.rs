//! Dependency-free beat-grid tracking for the VJ input worker.
//!
//! The analyzer deliberately owns fixed-size rings and performs no allocation in
//! [`BeatSyncAnalyzer::push_mono`].  It is intended to live on a worker thread:
//! the platform audio callback can copy/down-mix into a bounded queue, while the
//! worker feeds arbitrary-sized chunks here and publishes the returned `Copy`
//! snapshot.
//!
//! # The clock topology
//!
//! There is exactly ONE published clock — [`BeatClock`] — and everything in the
//! app reads the beat from it: the visuals, the video loop rate-fit, the sprite
//! steppers, the karaoke quantizer, the LED, the program fades. What feeds it is
//! chosen by priority, highest first:
//!
//! 1. **The operator** — a TAP tempo or a "the one is HERE" anchor. They asked;
//!    they win.
//! 2. **The room, when a deck is following it** ([`ClockSource::External`]) —
//!    playing along with another DJ or a live source. The deck chases the
//!    detector, so the detector must be the clock and that deck's own grid must
//!    never be read back, or it would be chasing itself.
//! 3. **The live deck's grid** — the normal show: the DJ system is master, the
//!    grid comes from a whole-file analysis and a device clock, the crossfader
//!    decides which deck leads. While a deck is playing, the loopback detector
//!    is SUPPRESSED: it would only be re-detecting our own output, later and
//!    less certainly than the deck already knows it.
//! 4. **The loopback detector** — VJ standalone, listening to the room.
//!
//! Both directions, one contract: whichever source is in charge, the published
//! time is continuous. Handovers between sources — including a crossfade from
//! one deck to another — are slewed like any other correction, never jumped.
//!
//! ## Protocol seam
//!
//! [`ClockSource`] and [`ClockSink`] are the (deliberately protocol-free) seam
//! where network and hardware sync will plug in later: an adapter publishes
//! tempo/phase/confidence into the ladder as a source, or subscribes to the
//! published clock to drive an outgoing protocol as a sink. Nothing here knows
//! anything about any specific protocol, and nothing here should.

use std::f32::consts::PI;

const HISTORY_HOPS: usize = 4096;
const MAX_ONSETS: usize = 256;
const ANALYSIS_SECONDS: f64 = 12.0;
// A locked deck also keeps a short, recent-only view. The long view is what
// makes mastered/compressed material stable, while the short view prevents a
// previous song from voting for its BPM for most of the 12-second history.
const CHANGE_ANALYSIS_SECONDS: f64 = 3.0;
const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 180.0;
const FILTER_BANDS: usize = 4;
const CROSSOVERS: usize = FILTER_BANDS - 1;

/// How trustworthy and current the tracked grid is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BeatLockState {
    #[default]
    Unlocked,
    Acquiring,
    Locked,
    /// A previously good grid is being extrapolated through a short dropout.
    Holdover,
    /// A previously good grid has gone stale and must not drive hard sync.
    Lost,
}

/// A coherent, trivially-copyable view of the beat grid at one sample instant.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatSnapshot {
    pub sample_rate: f64,
    pub sample_position: u64,
    pub bpm: f64,
    pub beat_period_samples: f64,
    /// Most recent predicted whole-beat boundary at or before `sample_position`.
    pub phase_sample: f64,
    pub beat_index: i64,
    pub confidence: f32,
    pub state: BeatLockState,
    pub last_onset_sample: Option<u64>,
}

impl BeatSnapshot {
    pub fn has_grid(&self) -> bool {
        self.beat_period_samples.is_finite() && self.beat_period_samples > 1.0
    }

    pub fn is_locked(&self) -> bool {
        matches!(self.state, BeatLockState::Locked | BeatLockState::Holdover)
    }

    pub fn seconds_per_beat(&self) -> Option<f64> {
        self.has_grid()
            .then_some(self.beat_period_samples / self.sample_rate)
    }

    /// Fractional beat phase in `[0, 1)` for an absolute sample position.
    pub fn phase_at(&self, sample: u64) -> Option<f64> {
        if !self.has_grid() {
            return None;
        }
        Some((sample as f64 - self.phase_sample).rem_euclid(self.beat_period_samples)
            / self.beat_period_samples)
    }

    /// First strict future boundary of a beat subdivision.
    pub fn next_boundary(&self, after_sample: u64, subdivision: u32) -> Option<u64> {
        next_boundary_sample(
            after_sample,
            self.phase_sample,
            self.beat_period_samples,
            subdivision,
        )
    }
}

/// Return the first subdivision boundary strictly after `after_sample`.
///
/// Computing from the phase reference instead of incrementing a cached deadline
/// means late UI frames automatically skip every missed boundary.
pub fn next_boundary_sample(
    after_sample: u64,
    phase_sample: f64,
    beat_period_samples: f64,
    subdivision: u32,
) -> Option<u64> {
    if !phase_sample.is_finite()
        || !beat_period_samples.is_finite()
        || beat_period_samples <= 1.0
        || subdivision == 0
    {
        return None;
    }
    let step = beat_period_samples / subdivision as f64;
    if step < 1.0 {
        return None;
    }
    let n = ((after_sample as f64 - phase_sample) / step).floor() + 1.0;
    let boundary = phase_sample + n * step;
    if boundary.is_finite() && boundary >= 0.0 {
        Some(boundary.ceil() as u64)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatFit {
    pub beats: u32,
    /// Source playback multiplier. `1.02` plays two percent faster.
    pub playback_rate: f64,
    pub target_duration_seconds: f64,
    pub relative_rate_error: f64,
    pub confidence: f32,
    pub within_rate_limit: bool,
}

/// Fit a source loop to a musically useful power-of-two beat count.
pub fn fit_loop_to_grid(
    source_duration_seconds: f64,
    bpm: f64,
    max_rate_deviation: f64,
) -> Option<BeatFit> {
    const MUSICAL_LENGTHS: &[u32] = &[1, 2, 4, 8, 16, 32];
    fit_loop_to_beats(
        source_duration_seconds,
        bpm,
        MUSICAL_LENGTHS,
        max_rate_deviation,
    )
}

/// Fit a source loop to one of `allowed_beats` and report whether the needed
/// rate is inside the caller's A/V-safe range.
pub fn fit_loop_to_beats(
    source_duration_seconds: f64,
    bpm: f64,
    allowed_beats: &[u32],
    max_rate_deviation: f64,
) -> Option<BeatFit> {
    if !source_duration_seconds.is_finite()
        || source_duration_seconds <= 0.0
        || !bpm.is_finite()
        || !(MIN_BPM..=MAX_BPM).contains(&bpm)
        || !max_rate_deviation.is_finite()
        || max_rate_deviation < 0.0
    {
        return None;
    }

    let mut best: Option<BeatFit> = None;
    for &beats in allowed_beats {
        if beats == 0 {
            continue;
        }
        let target = beats as f64 * 60.0 / bpm;
        let rate = source_duration_seconds / target;
        let error = (rate - 1.0).abs();
        let within = error <= max_rate_deviation;
        // Below the safe limit confidence stays high. Beyond it, return a useful
        // degraded proposal rather than silently claiming that sync is safe.
        let confidence = if max_rate_deviation > 0.0 && within {
            (1.0 - 0.25 * error / max_rate_deviation) as f32
        } else if error <= f64::EPSILON {
            1.0
        } else {
            (0.5 * (-4.0 * (error - max_rate_deviation).max(0.0)).exp()) as f32
        };
        let fit = BeatFit {
            beats,
            playback_rate: rate,
            target_duration_seconds: target,
            relative_rate_error: error,
            confidence,
            within_rate_limit: within,
        };
        if best
            .as_ref()
            .map(|old| {
                error < old.relative_rate_error - 1e-9
                    || ((error - old.relative_rate_error).abs() <= 1e-9 && beats < old.beats)
            })
            .unwrap_or(true)
        {
            best = Some(fit);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// the published clock
// ---------------------------------------------------------------------------

// TUNING PRIORS — THIS CLOCK IS FOR EDM, ON PURPOSE.
//
// The job is four-on-the-floor at a near-constant tempo, and everything
// below is tuned for that and not for rubato or a live band. Concretely:
// tempo has strong inertia (a locked tempo barely moves, and a sudden
// re-estimate has to hold for seconds before it is believed), corrections
// are spent on PHASE rather than tempo, and a lost lock coasts for a long
// time because an EDM breakdown is exactly where a detector flails and
// exactly where the flywheel has to hold — the drop lands back on the held
// grid, which is the musical truth. Please do not "fix" the inertia to make
// it follow a live drummer: that trade is not the one this app wants.

/// Beats per bar the clock groups by.
pub const CLOCK_BAR_BEATS: f64 = 4.0;
/// An error smaller than this is steady-state drift; anything larger is a
/// disagreement and gets the faster regime.
const CLOCK_TRACK_ERROR: f64 = 0.1;
/// After this long with nothing confident to follow, a coast is no longer a
/// breakdown — it is silence, and the clock stops rather than pretending.
pub const CLOCK_COAST_MAX_SECS: f64 = 60.0;
/// Tempo agreement inside this band is drift, and is eased away rather than
/// adopted. Outside it, the source is claiming a DIFFERENT tempo.
const CLOCK_TEMPO_BAND: f64 = 0.02;
/// How much of a drift disagreement is taken per update — deliberately tiny:
/// tempo is the thing that must not wander.
const CLOCK_TEMPO_GAIN: f64 = 0.01;
/// A different tempo has to hold this long before the clock believes it. A
/// track does not change tempo mid-drop; a detector changes its mind all the
/// time, and octave flips are its favourite way to do it.
const CLOCK_TEMPO_CLAIM_SECS: f64 = 2.5;

/// How much tempo a correction is allowed to spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlewRegime {
    /// Steady-state discipline: a few percent, spread over many beats.
    Track,
    /// A real disagreement — after a coast, or a detector that came back
    /// somewhere else. Faster, still nothing an eye reads as a jump.
    Recapture,
    /// The operator said so: converge inside about a beat.
    Operator,
}

impl SlewRegime {
    /// The time constant, in beats: the outstanding error decays by `1/e`
    /// over this many beats.
    fn converge_beats(self) -> f64 {
        match self {
            SlewRegime::Track => 12.0,
            SlewRegime::Recapture => 4.0,
            SlewRegime::Operator => 1.0,
        }
    }

    /// The hard bound on the effective tempo, as a fraction either side of
    /// nominal. The lower bound never reaches `-1`, which is what keeps the
    /// published position monotonic whatever the error asks for.
    fn bounds(self) -> (f64, f64) {
        match self {
            SlewRegime::Track => (-0.04, 0.04),
            SlewRegime::Recapture => (-0.25, 0.25),
            SlewRegime::Operator => (-0.90, 2.50),
        }
    }
}

/// Who is telling the clock where the beat is, in priority order.
///
/// This is the ladder from the module docs as a value, so the app can say
/// which rung it is on and a future protocol adapter has a rung to sit on.
/// The variants are ordered: a lower one never overrides a higher one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClockSource {
    /// Nothing is driving it — free-running or coasting.
    #[default]
    None,
    /// The loopback detector: the VJ listening to the room.
    Detector,
    /// A playing deck's analysed grid — the DJ system as master.
    Deck,
    /// The room, while a deck is following it (EXT).
    External,
    /// A network or hardware sync adapter (session-sync protocols, MIDI
    /// clock, …). Reserved: no protocol lives in this crate, and none
    /// should. An adapter implements nothing here — it simply feeds
    /// [`BeatTarget`]s in at this rung, and the exact rung is the app's
    /// decision, not the protocol's.
    Net,
    /// The operator: TAP tempo, or "the one is HERE".
    Operator,
}

/// The outgoing half of the seam: something that mirrors the published
/// clock outwards (MIDI clock at 24 ppqn, a network beat protocol, a
/// hardware trigger…).
///
/// The clock does not know or care what a sink does with the time; a sink
/// is handed the same continuous position every other consumer reads, and
/// the epoch so it can resynchronise when the clock genuinely restarts.
/// Deliberately protocol-free: no implementation belongs in this module.
pub trait ClockSink {
    /// Called with the published position (beats), the effective tempo, and
    /// the clock's epoch, at whatever rate the host pumps.
    fn publish(&mut self, position_beats: f64, bpm: f64, epoch: u64);
}

/// Where a source says the beat is, and how fast it is running.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatTarget {
    /// Position inside the window this source can actually vouch for: a
    /// phase in `[0, 1)` from a detector with no downbeat, or a bar position
    /// in `[0, bar)` from a source that knows where the one is.
    pub phase: f64,
    /// `1.0` for phase only; the bar length when the downbeat is meant.
    pub wrap: f64,
    pub period_secs: f64,
}

impl BeatTarget {
    /// A phase-only target: "we are this far into some beat".
    pub fn phase_only(phase: f64, period_secs: f64) -> BeatTarget {
        BeatTarget { phase: phase.rem_euclid(1.0), wrap: 1.0, period_secs }
    }

    /// A bar-aware target: "we are this far into beat `index` of the bar".
    pub fn in_bar(index: u64, phase: f64, period_secs: f64) -> BeatTarget {
        BeatTarget {
            phase: (index as f64 + phase).rem_euclid(CLOCK_BAR_BEATS),
            wrap: CLOCK_BAR_BEATS,
            period_secs,
        }
    }
}

/// The beat clock the whole app runs on: a disciplined oscillator between
/// the raw estimates and every consumer.
///
/// **Contract: published beat time is continuous.** [`BeatClock::position_at`]
/// never jumps and never runs backwards. Every correction — detector drift,
/// a re-lock that came back somewhere else, a deck grid taking over, an
/// operator tap — is spent as a bounded SLEW of the effective tempo until
/// the phase converges, never as a discontinuity. The one exception is an
/// EPOCH: [`BeatClock::start`] is an explicit "there was no clock, now there
/// is one" event, and consumers can see it through [`BeatClock::epoch`].
///
/// Losing the source is not a correction at all. The clock COASTS: it keeps
/// running at the last tempo and phase it was confident about, because a
/// steady clock that is slightly wrong through a breakdown beats one that
/// re-guesses every frame.
///
/// Time is plain monotonic seconds, so the whole discipline is testable
/// without a clock.
#[derive(Clone, Debug)]
pub struct BeatClock {
    now: f64,
    /// Continuous beat position since the epoch. Monotonic, always.
    position: f64,
    /// Nominal seconds per beat — the tempo the clock free-runs at.
    period: f64,
    /// Phase still owed to the source, in beats. Positive = the source is
    /// ahead of us and the clock must run fast to catch up.
    error: f64,
    regime: SlewRegime,
    running: bool,
    coasting: bool,
    /// Seconds spent coasting since the last confident source.
    coasted: f64,
    /// Armed by an operator action: the next target it sees is spent at the
    /// operator rate however small it is.
    operator_armed: bool,
    /// A tempo the source keeps claiming that the clock does not believe
    /// yet: the period, and when the claim started.
    tempo_claim: Option<(f64, f64)>,
    epoch: u64,
}

impl Default for BeatClock {
    fn default() -> Self {
        BeatClock {
            now: 0.0,
            position: 0.0,
            period: 0.5,
            error: 0.0,
            regime: SlewRegime::Track,
            running: false,
            coasting: false,
            coasted: 0.0,
            operator_armed: false,
            tempo_claim: None,
            epoch: 0,
        }
    }
}

impl BeatClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// True while the clock is flying on its own because nothing confident
    /// is available. The bar should say so, and the LED should show it.
    pub fn coasting(&self) -> bool {
        self.coasting
    }

    /// Bumped by [`BeatClock::start`] — the only place the published
    /// position is allowed to move on its own.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The tempo the clock free-runs at, ignoring any correction in flight.
    pub fn nominal_bpm(&self) -> f64 {
        60.0 / self.period.max(1e-6)
    }

    /// The tempo the clock is ACTUALLY running at this instant, correction
    /// included: what a consumer must use to predict the next beat.
    pub fn period_secs(&self) -> f64 {
        self.period / (1.0 + self.slew()).max(1e-3)
    }

    pub fn bpm(&self) -> f64 {
        60.0 / self.period_secs().max(1e-6)
    }

    /// Phase still to be corrected, in beats.
    pub fn error_beats(&self) -> f64 {
        self.error
    }

    /// The beat position at `now`, projected from the last update. This is
    /// the published stream: continuous and non-decreasing in `now`.
    pub fn position_at(&self, now: f64) -> f64 {
        if !self.running {
            return self.position;
        }
        let dt = (now - self.now).max(0.0);
        self.position + dt / self.period.max(1e-6) * (1.0 + self.slew())
    }

    /// Which beat of the bar `position` falls in, and how far into it.
    pub fn bar_phase_at(&self, now: f64) -> (u64, f64) {
        let position = self.position_at(now);
        let index = position.floor().rem_euclid(CLOCK_BAR_BEATS) as u64;
        (index, (position - position.floor()).clamp(0.0, 1.0))
    }

    /// The correction currently being spent, as a fraction of nominal tempo.
    fn slew(&self) -> f64 {
        if !self.running {
            return 0.0;
        }
        let (low, high) = self.regime.bounds();
        (self.error / self.regime.converge_beats()).clamp(low, high)
    }

    /// Run the oscillator forward to `now`, spending whatever correction is
    /// outstanding. Every other method calls this first, so the position is
    /// always advanced before anything is allowed to change the target.
    pub fn advance_to(&mut self, now: f64) {
        if !self.running || !now.is_finite() {
            self.now = now.max(self.now);
            return;
        }
        let dt = (now - self.now).max(0.0);
        self.now = self.now.max(now);
        if dt <= 0.0 {
            return;
        }
        let slew = self.slew();
        let base = dt / self.period.max(1e-6);
        self.position += base * (1.0 + slew);
        // A correction is spent exactly as it is travelled — the extra
        // distance covered IS the error that got closed.
        self.error -= base * slew;
        if self.coasting {
            self.coasted += dt;
            if self.coasted > CLOCK_COAST_MAX_SECS {
                // Not a breakdown any more. Stop rather than pretend.
                self.running = false;
                self.error = 0.0;
            }
        }
    }

    /// THE EPOCH EVENT: there was no clock and now there is one. This is the
    /// only call that may move the published position, and it says so by
    /// bumping [`BeatClock::epoch`].
    pub fn start(&mut self, now: f64, target: BeatTarget) {
        if !(target.period_secs > 0.0) || !target.period_secs.is_finite() {
            return;
        }
        self.now = now;
        self.period = target.period_secs;
        self.position = target.phase;
        self.error = 0.0;
        self.regime = SlewRegime::Track;
        self.running = true;
        self.coasting = false;
        self.coasted = 0.0;
        self.operator_armed = false;
        self.tempo_claim = None;
        self.epoch += 1;
    }

    /// Take a source's tempo, with the inertia the tuning priors call for:
    /// a disagreement inside the drift band is eased away over seconds, and
    /// a genuinely different tempo has to hold before it is believed.
    ///
    /// An operator tempo is not a claim, it is an instruction: it lands.
    fn adopt_tempo(&mut self, now: f64, target: f64, operator: bool) {
        if !(target > 0.0) || !target.is_finite() {
            return;
        }
        if operator {
            self.period = target;
            self.tempo_claim = None;
            return;
        }
        if (target / self.period - 1.0).abs() <= CLOCK_TEMPO_BAND {
            self.period += (target - self.period) * CLOCK_TEMPO_GAIN;
            self.tempo_claim = None;
            return;
        }
        match self.tempo_claim {
            Some((claim, since)) if (target / claim - 1.0).abs() <= CLOCK_TEMPO_BAND => {
                if now - since >= CLOCK_TEMPO_CLAIM_SECS {
                    self.period = target;
                    self.tempo_claim = None;
                }
            }
            _ => self.tempo_claim = Some((target, now)),
        }
    }

    /// Follow a confident source: adopt its tempo (a change of slope, never
    /// of position) and slew the phase toward it.
    pub fn discipline(&mut self, now: f64, target: BeatTarget) {
        self.advance_to(now);
        if !self.running {
            self.start(now, target);
            return;
        }
        let operator = self.operator_armed;
        self.adopt_tempo(now, target.period_secs, operator);
        self.coasting = false;
        self.coasted = 0.0;
        self.error = wrapped_error(target.phase - self.position, target.wrap);
        self.regime = if operator {
            SlewRegime::Operator
        } else if self.error.abs() <= CLOCK_TRACK_ERROR {
            SlewRegime::Track
        } else if self.regime == SlewRegime::Operator {
            // An operator correction runs to completion at operator speed;
            // the pump re-stating the same target must not slow it down.
            SlewRegime::Operator
        } else {
            SlewRegime::Recapture
        };
        self.operator_armed = false;
    }

    /// The operator moved the beat: converge inside about a beat, but as a
    /// slew like everything else, so nothing driven by the phase glitches.
    pub fn anchor(&mut self, now: f64, target: BeatTarget) {
        self.advance_to(now);
        if !self.running {
            self.start(now, target);
            return;
        }
        self.operator_armed = true;
        self.discipline(now, target);
    }

    /// The operator TAPPED: the beat is HERE, exactly. The second
    /// sanctioned discontinuity (after [`BeatClock::start`]): a tap is not
    /// a claim to converge on, it is a declaration, and gliding to it over
    /// a beat defeats the gesture — the phase pins to the press. Consumers
    /// see it as an epoch, same as a fresh start.
    pub fn pin(&mut self, now: f64, target: BeatTarget) {
        self.start(now, target);
    }

    /// Arm the operator regime for the NEXT target the clock sees. RESYNC
    /// has nothing to anchor to at the moment it is pressed — the detector
    /// is only just starting to look — so the intent is carried forward.
    pub fn arm_operator(&mut self) {
        self.operator_armed = true;
    }

    /// Nothing confident to follow. Keep running exactly as we were.
    pub fn coast(&mut self, now: f64) {
        self.advance_to(now);
        if self.running {
            self.coasting = true;
        }
    }

    /// Give up entirely (no source and no history worth flying on).
    pub fn stop(&mut self) {
        self.running = false;
        self.coasting = false;
        self.error = 0.0;
    }
}

/// Shortest signed correction to a target inside a wrap window, biased
/// toward running FAST: a clock that rushes to the next beat reads better
/// than one that stalls, and stalling is the direction with the hard floor.
fn wrapped_error(raw: f64, wrap: f64) -> f64 {
    if !(wrap > 0.0) || !raw.is_finite() {
        return 0.0;
    }
    let low = -wrap * 0.25;
    let mut error = raw.rem_euclid(wrap);
    if error > wrap + low {
        error -= wrap;
    }
    error
}

// ---------------------------------------------------------------------------
// operator tap tempo
// ---------------------------------------------------------------------------

/// A gap longer than this cannot be part of one tempo (24 BPM): the operator
/// stopped and started again, so the run restarts from that tap.
pub const TAP_MAX_GAP_SECS: f64 = 2.5;
/// A gap shorter than this (240 BPM) is a bounce, not a beat. It is IGNORED
/// rather than treated as a restart, so a double-clicked button cannot throw
/// away a run the operator is halfway through.
pub const TAP_MIN_GAP_SECS: f64 = 0.25;
/// Taps kept for the rolling mean — five gaps, about two bars of tapping.
pub const TAP_HISTORY: usize = 6;
/// Taps needed before the run may set TEMPO. Four taps = three gaps: the
/// smallest count that shows a steady interval rather than a single guess.
pub const TAP_TEMPO_TAPS: usize = 4;
/// A gap this far off the run's mean is a different tempo, not a wobble, and
/// restarts the run at that tap.
const TAP_GAP_TOLERANCE: f64 = 0.35;

/// What one tap says about the beat clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TapClock {
    /// The moment of this tap, in the caller's clock. It is a downbeat: the
    /// operator says "one" here.
    pub anchor_secs: f64,
    /// The tapped tempo, once the run is long enough to have one. `None`
    /// means "keep the tempo the clock already runs at and move only the
    /// phase" — one tap is a downbeat, not a tempo.
    pub bpm: Option<f64>,
    /// Taps in the current run, this one included.
    pub taps: usize,
}

/// Operator tap tempo: the last few tap instants and the tempo/phase they
/// imply.
///
/// Time is plain monotonic seconds so the whole rule set is testable without
/// a clock; the caller converts to whatever instant type its beat grid uses.
#[derive(Clone, Debug, Default)]
pub struct TapTempo {
    /// Tap times, oldest first, at most [`TAP_HISTORY`].
    taps: Vec<f64>,
}

impl TapTempo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tap. Returns the clock it implies, or `None` when the tap
    /// was a bounce inside [`TAP_MIN_GAP_SECS`] and was ignored.
    pub fn tap(&mut self, at_secs: f64) -> Option<TapClock> {
        if !at_secs.is_finite() {
            return None;
        }
        let restart = match self.taps.last().copied() {
            None => true,
            Some(last) => {
                let gap = at_secs - last;
                if gap < TAP_MIN_GAP_SECS {
                    // A bounce: not a beat, and not a reason to lose the run.
                    return None;
                }
                gap > TAP_MAX_GAP_SECS || self.is_outlier(gap)
            }
        };
        if restart {
            self.taps.clear();
        }
        self.taps.push(at_secs);
        if self.taps.len() > TAP_HISTORY {
            self.taps.remove(0);
        }
        Some(TapClock { anchor_secs: at_secs, bpm: self.bpm(), taps: self.taps.len() })
    }

    /// The tempo of the current run, once it has [`TAP_TEMPO_TAPS`] taps.
    pub fn bpm(&self) -> Option<f64> {
        if self.taps.len() < TAP_TEMPO_TAPS {
            return None;
        }
        let mean = self.mean_gap()?;
        let bpm = 60.0 / mean;
        bpm.is_finite().then_some(bpm.clamp(60.0 / TAP_MAX_GAP_SECS, 60.0 / TAP_MIN_GAP_SECS))
    }

    pub fn taps(&self) -> usize {
        self.taps.len()
    }

    pub fn clear(&mut self) {
        self.taps.clear();
    }

    /// Mean of the gaps currently kept, or `None` with fewer than two taps.
    fn mean_gap(&self) -> Option<f64> {
        if self.taps.len() < 2 {
            return None;
        }
        let span = self.taps.last()? - self.taps.first()?;
        Some(span / (self.taps.len() - 1) as f64)
    }

    /// A gap that disagrees with the run's mean by more than the tolerance is
    /// a new tempo (half/double time included), not a wobble in this one.
    fn is_outlier(&self, gap: f64) -> bool {
        match self.mean_gap() {
            None => false,
            Some(mean) if mean > 0.0 => {
                let ratio = gap / mean;
                ratio < 1.0 - TAP_GAP_TOLERANCE || ratio > 1.0 + TAP_GAP_TOLERANCE
            }
            Some(_) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Onset {
    sample: u64,
    strength: f32,
}

#[derive(Clone, Copy, Debug)]
struct TempoEstimate {
    period: f64,
    phase_origin: f64,
    phase_quality: f32,
    support: usize,
    aligned_support: usize,
    best_score: f32,
    confidence: f32,
    recent: bool,
}

/// Streaming mono transient/tempo/phase tracker.
pub struct BeatSyncAnalyzer {
    sample_rate: f64,
    hop_size: usize,
    sample_clock: u64,
    hop_samples: usize,
    hop_square_sum: f64,
    hop_band_square_sum: [f64; FILTER_BANDS],
    previous_input: f32,
    dc_output: f32,
    dc_coefficient: f32,
    lowpass_state: [f32; CROSSOVERS],
    lowpass_alpha: [f32; CROSSOVERS],
    band_previous_log_energy: [f32; FILTER_BANDS],
    band_flux_mean: [f32; FILTER_BANDS],
    band_flux_variance: [f32; FILTER_BANDS],
    band_novelty_tail: [f32; FILTER_BANDS],
    peak_left: f32,
    peak_middle: f32,
    peak_middle_sample: u64,
    last_onset_sample: Option<u64>,
    onset_history: [f32; HISTORY_HOPS],
    band_onset_history: [[f32; HISTORY_HOPS]; FILTER_BANDS],
    history_write: usize,
    history_count: usize,
    onsets: [Onset; MAX_ONSETS],
    onset_write: usize,
    onset_count: usize,
    hops_since_analysis: usize,
    grid_origin: f64,
    period_samples: f64,
    confidence: f32,
    candidate_period: f64,
    candidate_confidence: f32,
    candidate_streak: u8,
    change_period: f64,
    change_phase_origin: f64,
    change_confidence: f32,
    change_streak: u8,
    change_misses: u8,
    change_published: bool,
    change_previous_period: f64,
    change_previous_origin: f64,
    change_previous_confidence: f32,
    change_previous_state: BeatLockState,
    state: BeatLockState,
    ever_locked: bool,
}

impl BeatSyncAnalyzer {
    pub fn new(sample_rate: f64) -> Self {
        assert!(sample_rate.is_finite() && sample_rate >= 8_000.0);
        let dc_coefficient = (-2.0 * PI * 25.0 / sample_rate as f32).exp();
        let mut lowpass_alpha = [0.0; CROSSOVERS];
        for (alpha, cutoff) in lowpass_alpha.iter_mut().zip([170.0f32, 700.0, 2_800.0]) {
            let cutoff = cutoff.min(sample_rate as f32 * 0.4);
            *alpha = 1.0 - (-2.0 * PI * cutoff / sample_rate as f32).exp();
        }
        Self {
            sample_rate,
            hop_size: (sample_rate / 100.0).round().max(32.0) as usize,
            sample_clock: 0,
            hop_samples: 0,
            hop_square_sum: 0.0,
            hop_band_square_sum: [0.0; FILTER_BANDS],
            previous_input: 0.0,
            dc_output: 0.0,
            dc_coefficient,
            lowpass_state: [0.0; CROSSOVERS],
            lowpass_alpha,
            band_previous_log_energy: [0.0; FILTER_BANDS],
            band_flux_mean: [0.0; FILTER_BANDS],
            band_flux_variance: [1e-6; FILTER_BANDS],
            band_novelty_tail: [0.0; FILTER_BANDS],
            peak_left: 0.0,
            peak_middle: 0.0,
            peak_middle_sample: 0,
            last_onset_sample: None,
            onset_history: [0.0; HISTORY_HOPS],
            band_onset_history: [[0.0; HISTORY_HOPS]; FILTER_BANDS],
            history_write: 0,
            history_count: 0,
            onsets: [Onset::default(); MAX_ONSETS],
            onset_write: 0,
            onset_count: 0,
            hops_since_analysis: 0,
            grid_origin: 0.0,
            period_samples: 0.0,
            confidence: 0.0,
            candidate_period: 0.0,
            candidate_confidence: 0.0,
            candidate_streak: 0,
            change_period: 0.0,
            change_phase_origin: 0.0,
            change_confidence: 0.0,
            change_streak: 0,
            change_misses: 0,
            change_published: false,
            change_previous_period: 0.0,
            change_previous_origin: 0.0,
            change_previous_confidence: 0.0,
            change_previous_state: BeatLockState::Unlocked,
            state: BeatLockState::Unlocked,
            ever_locked: false,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.sample_rate);
    }

    /// Feed arbitrary callback-sized mono PCM. Non-finite samples are ignored.
    /// This method performs no heap allocation.
    pub fn push_mono(&mut self, samples: &[f32]) -> BeatSnapshot {
        for &sample in samples {
            let sample = if sample.is_finite() {
                sample.clamp(-8.0, 8.0)
            } else {
                0.0
            };
            // A DC blocker followed by three one-pole crossovers is cheap
            // enough to run sample-by-sample, and is much more revealing than
            // broadband RMS on mastered material. Kick, body, presence and
            // high-frequency transients each get an independent novelty lane.
            let dc = sample - self.previous_input + self.dc_coefficient * self.dc_output;
            self.previous_input = sample;
            self.dc_output = dc;
            for index in 0..CROSSOVERS {
                self.lowpass_state[index] +=
                    self.lowpass_alpha[index] * (dc - self.lowpass_state[index]);
            }
            let bands = [
                self.lowpass_state[0],
                self.lowpass_state[1] - self.lowpass_state[0],
                self.lowpass_state[2] - self.lowpass_state[1],
                dc - self.lowpass_state[2],
            ];
            self.hop_square_sum += (sample as f64) * (sample as f64);
            for (sum, band) in self.hop_band_square_sum.iter_mut().zip(bands) {
                *sum += (band as f64) * (band as f64);
            }
            self.hop_samples += 1;
            self.sample_clock = self.sample_clock.saturating_add(1);
            if self.hop_samples == self.hop_size {
                self.finish_hop();
            }
        }
        self.snapshot()
    }

    pub fn snapshot(&self) -> BeatSnapshot {
        let (phase_sample, beat_index) = if self.period_samples > 1.0 {
            let index = ((self.sample_clock as f64 - self.grid_origin) / self.period_samples)
                .floor() as i64;
            (
                self.grid_origin + index as f64 * self.period_samples,
                index,
            )
        } else {
            (0.0, 0)
        };
        BeatSnapshot {
            sample_rate: self.sample_rate,
            sample_position: self.sample_clock,
            bpm: if self.period_samples > 1.0 {
                60.0 * self.sample_rate / self.period_samples
            } else {
                0.0
            },
            beat_period_samples: self.period_samples,
            phase_sample,
            beat_index,
            confidence: self.confidence.clamp(0.0, 1.0),
            state: self.state,
            last_onset_sample: self.last_onset_sample,
        }
    }

    fn finish_hop(&mut self) {
        let first_hop = self.history_count == 0;
        let _broadband_rms = (self.hop_square_sum / self.hop_samples as f64).sqrt() as f32;
        self.hop_square_sum = 0.0;
        let hop_samples = self.hop_samples;
        self.hop_samples = 0;

        let mut band_novelty = [0.0f32; FILTER_BANDS];
        let weights = [1.30f32, 1.05, 0.90, 0.65];
        let mut novelty_square_sum = 0.0;
        let mut weight_sum = 0.0;
        for band in 0..FILTER_BANDS {
            let rms = (self.hop_band_square_sum[band] / hop_samples as f64).sqrt() as f32;
            self.hop_band_square_sum[band] = 0.0;

            // Log compression makes a 2 dB transient useful whether system
            // audio arrives at -35 dBFS or has been limited close to full
            // scale. Each lane adapts to its own stationary flux distribution.
            let log_energy = (1.0 + 96.0 * rms).ln();
            let flux = if first_hop {
                0.0
            } else {
                (log_energy - self.band_previous_log_energy[band]).max(0.0)
            };
            self.band_previous_log_energy[band] = log_energy;

            let deviation = self.band_flux_variance[band].max(1e-8).sqrt();
            let threshold = self.band_flux_mean[band] + 0.55 * deviation + 0.0015;
            let novelty = ((flux - threshold).max(0.0) / (deviation + 0.006)).min(6.0);

            // Let a transient occupy two or three 10 ms cells. This makes the
            // lag correlation tolerant of non-integral beat periods without
            // delaying the local-maximum timestamp used for phase.
            self.band_novelty_tail[band] = novelty.max(self.band_novelty_tail[band] * 0.42);
            band_novelty[band] = self.band_novelty_tail[band];
            novelty_square_sum += weights[band] * band_novelty[band] * band_novelty[band];
            weight_sum += weights[band];

            // Do not let a single large kick raise the floor enough to hide
            // the following snare. The clipped value still follows changing
            // programme material within roughly half a second.
            let robust_flux = flux.min(self.band_flux_mean[band] + 2.5 * deviation + 0.018);
            let delta = robust_flux - self.band_flux_mean[band];
            self.band_flux_mean[band] += 0.020 * delta;
            self.band_flux_variance[band] =
                (0.980 * self.band_flux_variance[band] + 0.020 * delta * delta).max(1e-8);
        }
        let onset_value = (novelty_square_sum / weight_sum).sqrt().min(6.0);

        self.onset_history[self.history_write] = onset_value;
        for band in 0..FILTER_BANDS {
            self.band_onset_history[band][self.history_write] = band_novelty[band];
        }
        self.history_write = (self.history_write + 1) % HISTORY_HOPS;
        self.history_count = (self.history_count + 1).min(HISTORY_HOPS);

        if self.peak_middle > self.peak_left
            && self.peak_middle >= onset_value
            && self.peak_middle > 0.18
        {
            let refractory = (self.sample_rate * 0.09) as u64;
            if self
                .last_onset_sample
                .map(|last| self.peak_middle_sample.saturating_sub(last) >= refractory)
                .unwrap_or(true)
            {
                self.record_onset(self.peak_middle_sample, self.peak_middle);
            }
        }
        self.peak_left = self.peak_middle;
        self.peak_middle = onset_value;
        self.peak_middle_sample = self.sample_clock.saturating_sub((self.hop_size / 2) as u64);

        self.hops_since_analysis += 1;
        let analysis_interval = (self.sample_rate / self.hop_size as f64 * 0.25)
            .round()
            .max(1.0) as usize;
        if self.hops_since_analysis >= analysis_interval {
            self.hops_since_analysis = 0;
            self.analyze_grid();
        }
        self.update_dropout_state();
    }

    fn record_onset(&mut self, sample: u64, strength: f32) {
        self.onsets[self.onset_write] = Onset { sample, strength };
        self.onset_write = (self.onset_write + 1) % MAX_ONSETS;
        self.onset_count = (self.onset_count + 1).min(MAX_ONSETS);
        self.last_onset_sample = Some(sample);
        if matches!(self.state, BeatLockState::Unlocked | BeatLockState::Lost) {
            self.state = BeatLockState::Acquiring;
        }
    }

    fn history(&self, logical_index: usize) -> f32 {
        let oldest = (self.history_write + HISTORY_HOPS - self.history_count) % HISTORY_HOPS;
        self.onset_history[(oldest + logical_index) % HISTORY_HOPS]
    }

    fn band_history(&self, band: usize, logical_index: usize) -> f32 {
        let oldest = (self.history_write + HISTORY_HOPS - self.history_count) % HISTORY_HOPS;
        self.band_onset_history[band][(oldest + logical_index) % HISTORY_HOPS]
    }

    fn onset(&self, logical_index: usize) -> Onset {
        let oldest = (self.onset_write + MAX_ONSETS - self.onset_count) % MAX_ONSETS;
        self.onsets[(oldest + logical_index) % MAX_ONSETS]
    }

    fn correlation_at(&self, band: Option<usize>, lag: usize, used: usize) -> f32 {
        if lag >= used || used - lag < 16 {
            return 0.0;
        }
        let offset = self.history_count - used;
        let count = used - lag;
        let mut sx = 0.0f64;
        let mut sy = 0.0f64;
        let mut sxx = 0.0f64;
        let mut syy = 0.0f64;
        let mut sxy = 0.0f64;
        for i in lag..used {
            let x = band
                .map(|band| self.band_history(band, offset + i))
                .unwrap_or_else(|| self.history(offset + i)) as f64;
            let y = band
                .map(|band| self.band_history(band, offset + i - lag))
                .unwrap_or_else(|| self.history(offset + i - lag)) as f64;
            sx += x;
            sy += y;
            sxx += x * x;
            syy += y * y;
            sxy += x * y;
        }
        let n = count as f64;
        let covariance = sxy - sx * sy / n;
        let vx = (sxx - sx * sx / n).max(0.0);
        let vy = (syy - sy * sy / n).max(0.0);
        if vx <= 1e-12 || vy <= 1e-12 {
            0.0
        } else {
            (covariance / (vx * vy).sqrt()).clamp(-1.0, 1.0) as f32
        }
    }

    fn direct_tempo_score(&self, lag: usize, used: usize) -> f32 {
        let composite = self.correlation_at(None, lag, used).max(0.0);
        let mut strongest = 0.0f32;
        let mut second = 0.0f32;
        for band in 0..FILTER_BANDS {
            let score = self.correlation_at(Some(band), lag, used).max(0.0);
            if score > strongest {
                second = strongest;
                strongest = score;
            } else if score > second {
                second = score;
            }
        }

        // Separate band correlations resolve the classic kick/snare ambiguity:
        // their combined envelope repeats every half beat, while each timbral
        // lane repeats at the intended beat period.
        (0.24 * composite + 0.55 * strongest + 0.21 * second).clamp(0.0, 1.0)
    }

    fn tempo_score_at(&self, lag: usize, used: usize) -> f32 {
        let direct = self.direct_tempo_score(lag, used);
        let second = self.direct_tempo_score(lag * 2, used);
        let third = self.direct_tempo_score(lag * 3, used);
        (0.82 * direct + 0.12 * second + 0.06 * third).clamp(0.0, 1.0)
    }

    fn refine_period(&self, initial_period: f64, oldest_allowed: u64) -> f64 {
        let mut period = initial_period;
        // Pairwise IOIs retain useful whole-beat intervals even when adjacent
        // detected events are eighth-note hats or alternating kick/snare hits.
        for _ in 0..2 {
            let mut sum = 0.0;
            let mut weight_sum = 0.0;
            for later_index in 0..self.onset_count {
                let later = self.onset(later_index);
                if later.sample < oldest_allowed {
                    continue;
                }
                for earlier_index in 0..later_index {
                    let earlier = self.onset(earlier_index);
                    if earlier.sample < oldest_allowed {
                        continue;
                    }
                    let difference = later.sample.saturating_sub(earlier.sample) as f64;
                    let multiple = (difference / period).round();
                    if !(1.0..=16.0).contains(&multiple) {
                        continue;
                    }
                    let estimate = difference / multiple;
                    if (estimate / period - 1.0).abs() <= 0.105 {
                        let strength = later.strength.min(earlier.strength).clamp(0.05, 4.0);
                        let weight = strength as f64 / multiple.sqrt();
                        sum += estimate * weight;
                        weight_sum += weight;
                    }
                }
            }
            if weight_sum > 0.0 {
                period = sum / weight_sum;
            }
        }
        period
    }

    fn estimate_tempo(&self, used: usize) -> Option<TempoEstimate> {
        if used > self.history_count || used < 16 || self.onset_count < 4 {
            return None;
        }
        let hop_rate = self.sample_rate / self.hop_size as f64;
        let min_lag = (60.0 * hop_rate / MAX_BPM).floor().max(2.0) as usize;
        let max_lag = (60.0 * hop_rate / MIN_BPM).ceil() as usize;
        let mut best_lag = 0usize;
        let mut best_score = 0.0f32;
        let mut runner_up = 0.0f32;
        for lag in min_lag..=max_lag.min(used / 2) {
            let score = self.tempo_score_at(lag, used);
            if score > best_score {
                if best_lag.abs_diff(lag) > 3 {
                    runner_up = best_score.max(runner_up);
                }
                best_score = score;
                best_lag = lag;
            } else if best_lag.abs_diff(lag) > 3 {
                runner_up = runner_up.max(score);
            }
        }
        if best_lag == 0 {
            return None;
        }

        // Autocorrelation cannot intrinsically distinguish a period from an
        // equally regular two-period bar pattern. Only when an octave mate is
        // genuinely competitive, use a broad musical-rate prior centred at
        // 120 BPM as the tie-breaker (90 beats 180; 150 beats 75).
        for octave_lag in [best_lag / 2, best_lag.saturating_mul(2)] {
            if octave_lag < min_lag || octave_lag > max_lag.min(used / 2) {
                continue;
            }
            let octave_score = self.tempo_score_at(octave_lag, used);
            if octave_score < 0.72 * best_score {
                continue;
            }
            let best_bpm = 60.0 * hop_rate / best_lag as f64;
            let octave_bpm = 60.0 * hop_rate / octave_lag as f64;
            let best_prior = (-0.5 * ((best_bpm / 120.0).ln() / 0.38).powi(2)).exp();
            let octave_prior = (-0.5 * ((octave_bpm / 120.0).ln() / 0.38).powi(2)).exp();
            if octave_prior > best_prior {
                runner_up = runner_up.max(best_score);
                best_lag = octave_lag;
                best_score = octave_score;
            }
        }

        let oldest_allowed = self
            .sample_clock
            .saturating_sub((used * self.hop_size) as u64);
        let period = self.refine_period(best_lag as f64 * self.hop_size as f64, oldest_allowed);
        let bpm = 60.0 * self.sample_rate / period;
        if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
            return None;
        }

        let (phase_origin, phase_quality, support, aligned_support) =
            self.estimate_phase(period, oldest_allowed);
        let separation = ((best_score - runner_up).max(0.0) / best_score.max(1e-5)).min(1.0);
        let support_quality = (aligned_support as f32 / 6.0).min(1.0);
        let estimate_confidence = ((0.70 * best_score + 0.30 * phase_quality)
            * (0.80 + 0.20 * support_quality)
            * (0.92 + 0.08 * separation))
            .clamp(0.0, 1.0);

        let recent = self
            .last_onset_sample
            .map(|last| self.sample_clock.saturating_sub(last) as f64 <= 2.25 * period)
            .unwrap_or(false);
        Some(TempoEstimate {
            period,
            phase_origin,
            phase_quality,
            support,
            aligned_support,
            best_score,
            confidence: estimate_confidence,
            recent,
        })
    }

    fn analyze_grid(&mut self) {
        let hop_rate = self.sample_rate / self.hop_size as f64;
        let used = self
            .history_count
            .min((ANALYSIS_SECONDS * hop_rate).round() as usize);
        if used < (3.0 * hop_rate) as usize || self.onset_count < 4 {
            return;
        }

        let long_estimate = self.estimate_tempo(used);
        if self.ever_locked {
            let recent_hops = (CHANGE_ANALYSIS_SECONDS * hop_rate).round() as usize;
            if self.history_count >= recent_hops {
                let recent_estimate = self.estimate_tempo(recent_hops);
                if self.track_tempo_change(recent_estimate, recent_hops) {
                    return;
                }
            }
        }
        let Some(estimate) = long_estimate else {
            return;
        };
        self.apply_tempo_estimate(estimate);
    }

    fn track_tempo_change(
        &mut self,
        estimate: Option<TempoEstimate>,
        recent_hops: usize,
    ) -> bool {
        let credible = estimate.is_some_and(|estimate| {
            estimate.best_score >= 0.24
                && estimate.confidence >= 0.30
                && estimate.phase_quality >= 0.18
                && estimate.aligned_support >= 3
                && estimate.support >= 4
                && estimate.recent
        });

        if !credible {
            if self.change_streak > 0 {
                self.change_misses = self.change_misses.saturating_add(1);
                let miss_limit = if self.change_published { 8 } else { 2 };
                if self.change_misses >= miss_limit {
                    self.cancel_change_candidate();
                }
            }
            return self.change_streak > 0;
        }
        let estimate = estimate.unwrap();

        if self.change_streak == 0 {
            if self.period_samples <= 1.0
                || (estimate.period / self.period_samples - 1.0).abs() < 0.14
            {
                return false;
            }

            // The recent window must actively prefer the alternative over the
            // established grid. This is the key guard against treating a quiet
            // breakdown or a newly prominent off-beat layer as a song change.
            let locked_lag = (self.period_samples / self.hop_size as f64)
                .round()
                .max(1.0) as usize;
            let locked_score = self.tempo_score_at(locked_lag, recent_hops);
            let beats_locked_grid = estimate.best_score >= locked_score + 0.035
                || estimate.best_score >= locked_score * 1.12;
            if !beats_locked_grid {
                return false;
            }

            let ratio = estimate.period / self.period_samples;
            let octave_jump = (1.82..=2.18).contains(&ratio)
                || ((1.0 / 2.18)..=(1.0 / 1.82)).contains(&ratio);
            if octave_jump
                && estimate.confidence < (self.confidence * 1.25).max(0.74)
            {
                return false;
            }

            self.change_period = estimate.period;
            self.change_phase_origin = estimate.phase_origin;
            self.change_confidence = estimate.confidence;
            self.change_streak = 1;
            self.change_misses = 0;
            self.change_previous_period = self.period_samples;
            self.change_previous_origin = self.grid_origin;
            self.change_previous_confidence = self.confidence;
            self.change_previous_state = self.state;
        } else if (estimate.period / self.change_period - 1.0).abs() <= 0.065 {
            self.change_period = self.change_period * 0.62 + estimate.period * 0.38;
            self.change_phase_origin = estimate.phase_origin;
            self.change_confidence =
                self.change_confidence * 0.55 + estimate.confidence * 0.45;
            self.change_streak = self.change_streak.saturating_add(1);
            self.change_misses = 0;
        } else {
            self.change_misses = self.change_misses.saturating_add(1);
            let miss_limit = if self.change_published { 8 } else { 2 };
            if self.change_misses >= miss_limit {
                self.cancel_change_candidate();
            }
            return self.change_streak > 0;
        }

        if self.change_streak >= 2 && self.change_confidence >= 0.32 {
            self.period_samples = self.change_period;
            self.grid_origin = self.change_phase_origin;
            self.confidence = self.change_confidence;
            self.state = BeatLockState::Acquiring;
            self.change_published = true;
        }

        if self.change_streak >= 7
            && self.change_confidence >= 0.36
            && estimate.best_score >= 0.24
            && estimate.phase_quality >= 0.18
        {
            self.period_samples = self.change_period;
            self.grid_origin = self.change_phase_origin;
            self.confidence = self.change_confidence;
            self.state = BeatLockState::Locked;
            self.candidate_period = self.change_period;
            self.candidate_confidence = self.change_confidence;
            self.candidate_streak = 5;
            self.trim_tempo_history(CHANGE_ANALYSIS_SECONDS);
            self.clear_change_candidate();
        }
        true
    }

    fn clear_change_candidate(&mut self) {
        self.change_period = 0.0;
        self.change_phase_origin = 0.0;
        self.change_confidence = 0.0;
        self.change_streak = 0;
        self.change_misses = 0;
        self.change_published = false;
        self.change_previous_period = 0.0;
        self.change_previous_origin = 0.0;
        self.change_previous_confidence = 0.0;
        self.change_previous_state = BeatLockState::Unlocked;
    }

    fn cancel_change_candidate(&mut self) {
        if self.change_published && self.change_previous_period > 1.0 {
            self.period_samples = self.change_previous_period;
            self.grid_origin = self.change_previous_origin;
            self.confidence = self.change_previous_confidence;
            self.state = self.change_previous_state;
        }
        self.clear_change_candidate();
    }

    fn trim_tempo_history(&mut self, seconds: f64) {
        let keep_hops = (seconds * self.sample_rate / self.hop_size as f64).round() as usize;
        self.history_count = self.history_count.min(keep_hops);
        let oldest_allowed = self
            .sample_clock
            .saturating_sub((seconds * self.sample_rate) as u64);
        let mut keep_onsets = 0usize;
        for index in 0..self.onset_count {
            if self.onset(index).sample >= oldest_allowed {
                keep_onsets += 1;
            }
        }
        self.onset_count = keep_onsets;
    }

    fn apply_tempo_estimate(&mut self, estimate: TempoEstimate) {
        let TempoEstimate {
            period,
            phase_origin,
            phase_quality,
            support,
            aligned_support,
            best_score,
            confidence: estimate_confidence,
            recent,
        } = estimate;
        // Tempo stability and phase quality are related but distinct. A dense
        // eighth-note texture can have a very stable quarter-note recurrence
        // while producing many off-grid local peaks; do not reset the tempo
        // streak merely because those peaks dilute the phase histogram.
        let credible = best_score >= 0.20 && estimate_confidence >= 0.27 && recent;
        if !credible {
            if !self.ever_locked {
                self.candidate_streak = self.candidate_streak.saturating_sub(1);
                self.candidate_confidence = estimate_confidence;
                if support > 0 {
                    self.state = BeatLockState::Acquiring;
                }
            }
            return;
        }

        if self.candidate_period > 1.0 && (period / self.candidate_period - 1.0).abs() <= 0.065 {
            self.candidate_period = self.candidate_period * 0.65 + period * 0.35;
            self.candidate_streak = self.candidate_streak.saturating_add(1);
        } else {
            self.candidate_period = period;
            self.candidate_streak = 1;
        }
        self.candidate_confidence = if self.candidate_streak > 1 {
            self.candidate_confidence * 0.55 + estimate_confidence * 0.45
        } else {
            estimate_confidence
        };

        // Publish a deliberately provisional grid after two consistent
        // analyses. The UI can show a useful BPM around 3--5 seconds while
        // transport-changing callers continue to gate on `is_locked()`.
        if self.candidate_streak >= 2
            && phase_quality >= 0.18
            && aligned_support >= 2
            && !self.ever_locked
        {
            self.period_samples = self.candidate_period;
            self.grid_origin = phase_origin;
            self.confidence = self.candidate_confidence;
            self.state = BeatLockState::Acquiring;
        }

        let lock_ready = self.candidate_streak >= 5
            && best_score >= 0.22
            && phase_quality >= 0.18
            && aligned_support >= 2
            && self.candidate_confidence >= 0.36;
        if lock_ready {
            let tracked_period = self.candidate_period;
            if self.ever_locked && self.period_samples > 1.0 {
                let ratio = tracked_period / self.period_samples;
                let octave_jump = (1.82..=2.18).contains(&ratio)
                    || ((1.0 / 2.18)..=(1.0 / 1.82)).contains(&ratio);
                // Half/double-time peaks are both mathematically legitimate.
                // Once phase is established, require substantially stronger
                // evidence before changing octave instead of letting a newly
                // prominent hat or half-time snare flip the displayed BPM.
                if octave_jump
                    && self.candidate_confidence < (self.confidence * 1.25).max(0.72)
                {
                    self.state = BeatLockState::Locked;
                    return;
                }
            }
            if self.period_samples > 1.0
                && (tracked_period / self.period_samples - 1.0).abs() < 0.12
                && self.ever_locked
            {
                self.period_samples = self.period_samples * 0.82 + tracked_period * 0.18;
                let now = self.sample_clock as f64;
                let predicted = self.grid_origin
                    + ((now - self.grid_origin) / self.period_samples).round()
                        * self.period_samples;
                let measured = phase_origin
                    + ((now - phase_origin) / self.period_samples).round()
                        * self.period_samples;
                let correction = (measured - predicted)
                    .clamp(-0.18 * self.period_samples, 0.18 * self.period_samples);
                self.grid_origin += correction * 0.28;
            } else {
                self.period_samples = tracked_period;
                self.grid_origin = phase_origin;
            }
            self.confidence = if self.ever_locked {
                self.confidence * 0.72 + self.candidate_confidence * 0.28
            } else {
                self.candidate_confidence
            };
            self.state = BeatLockState::Locked;
            self.ever_locked = true;
        }
    }

    fn estimate_phase(
        &self,
        period: f64,
        oldest_allowed: u64,
    ) -> (f64, f32, usize, usize) {
        let mut best_origin = 0.0;
        let mut best_score = 0.0f64;
        let mut best_aligned = 0usize;
        let mut total_strength = 0.0f64;
        let mut support = 0usize;
        for i in 0..self.onset_count {
            let onset = self.onset(i);
            if onset.sample >= oldest_allowed {
                total_strength += onset.strength.max(1e-5) as f64;
                support += 1;
            }
        }
        if support == 0 || total_strength <= 0.0 {
            return (0.0, 0.0, 0, 0);
        }
        let sigma = period * 0.075;
        for candidate_index in 0..self.onset_count {
            let candidate = self.onset(candidate_index);
            if candidate.sample < oldest_allowed {
                continue;
            }
            let origin = candidate.sample as f64;
            let mut score = 0.0;
            let mut aligned = 0usize;
            for i in 0..self.onset_count {
                let onset = self.onset(i);
                if onset.sample < oldest_allowed {
                    continue;
                }
                let remainder = (onset.sample as f64 - origin).rem_euclid(period);
                let distance = remainder.min(period - remainder);
                let kernel = (-0.5 * (distance / sigma).powi(2)).exp();
                score += kernel * onset.strength.max(1e-5) as f64;
                if distance <= 0.12 * period {
                    aligned += 1;
                }
            }
            if score > best_score {
                best_score = score;
                best_origin = origin;
                best_aligned = aligned;
            }
        }
        (
            best_origin,
            (best_score / total_strength).clamp(0.0, 1.0) as f32,
            support,
            best_aligned,
        )
    }

    fn update_dropout_state(&mut self) {
        if !self.ever_locked || self.period_samples <= 1.0 {
            return;
        }
        let age_beats = self
            .last_onset_sample
            .map(|last| self.sample_clock.saturating_sub(last) as f64 / self.period_samples)
            .unwrap_or(f64::INFINITY);
        if age_beats > 8.0 {
            self.state = BeatLockState::Lost;
            self.confidence = 0.0;
        } else if age_beats > 2.5 {
            self.state = BeatLockState::Holdover;
            let hold = ((8.0 - age_beats) / 5.5).clamp(0.0, 1.0) as f32;
            self.confidence = self.confidence.min(hold);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click_track(sample_rate: usize, bpm: f64, seconds: f64) -> Vec<f32> {
        let len = (sample_rate as f64 * seconds) as usize;
        let mut samples = vec![0.0; len];
        let period = 60.0 * sample_rate as f64 / bpm;
        let mut beat = (0.23 * sample_rate as f64) as usize;
        while beat < len {
            for i in 0..(sample_rate / 500).max(4) {
                if beat + i < len {
                    samples[beat + i] = (1.0 - i as f32 / (sample_rate / 500).max(4) as f32)
                        .max(0.0);
                }
            }
            beat = (beat as f64 + period).round() as usize;
        }
        samples
    }

    fn compressed_music_mix(sample_rate: usize, bpm: f64, seconds: f64) -> Vec<f32> {
        let len = (sample_rate as f64 * seconds) as usize;
        let period = 60.0 * sample_rate as f64 / bpm;
        let half_period = period * 0.5;
        let start = 0.19 * sample_rate as f64;
        let mut seed = 0x93c4_6a7du32;
        let mut previous_noise = 0.0f32;
        let mut samples = Vec::with_capacity(len);
        for index in 0..len {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            let high_noise = noise - previous_noise;
            previous_noise = noise;

            let time = index as f64 / sample_rate as f64;
            let mut sample = 0.10 * (2.0 * std::f64::consts::PI * 196.0 * time).sin() as f32
                + 0.07 * (2.0 * std::f64::consts::PI * 293.67 * time).sin() as f32
                + 0.018 * noise;
            if index as f64 >= start {
                let position = index as f64 - start;
                let beat_index = (position / period).floor() as usize;
                let beat_seconds = (position - beat_index as f64 * period) / sample_rate as f64;
                let half_index = (position / half_period).floor() as usize;
                let half_seconds =
                    (position - half_index as f64 * half_period) / sample_rate as f64;

                let kick_accent = [1.0f32, 0.72, 0.86, 0.68][beat_index & 3];
                if beat_seconds < 0.20 {
                    let envelope = (-22.0 * beat_seconds).exp() as f32;
                    let sweep_phase = 2.0
                        * std::f64::consts::PI
                        * (78.0 * beat_seconds - 65.0 * beat_seconds * beat_seconds);
                    sample += 0.42 * kick_accent * envelope * sweep_phase.sin() as f32;
                }

                // Backbeat snare and eighth-note hats deliberately make the
                // broadband envelope busier than the intended quarter-note
                // grid. Their spectra remain independently periodic.
                if beat_index & 1 == 1 && beat_seconds < 0.11 {
                    let envelope = (-30.0 * beat_seconds).exp() as f32;
                    sample += 0.24 * envelope * high_noise
                        + 0.08
                            * envelope
                            * (2.0 * std::f64::consts::PI * 185.0 * time).sin() as f32;
                }
                if half_seconds < 0.032 {
                    let envelope = (-95.0 * half_seconds).exp() as f32;
                    let accent = if half_index & 1 == 0 { 0.10 } else { 0.075 };
                    sample += accent * envelope * high_noise;
                }

                let bass_envelope = 0.42 + 0.58 * (-7.0 * beat_seconds).exp() as f32;
                let bass_frequency = [55.0, 55.0, 65.41, 49.0][(beat_index / 4) & 3];
                sample += 0.16
                    * bass_envelope
                    * (2.0 * std::f64::consts::PI * bass_frequency * time).sin() as f32;
            }

            // A hard soft clip leaves little broadband level movement, like a
            // loud master or a system-capture path with gain normalization.
            samples.push((2.9 * sample).tanh() * 0.42);
        }
        samples
    }

    fn first_grid_and_lock(
        samples: &[f32],
        sample_rate: usize,
    ) -> (Option<f64>, Option<f64>, BeatSnapshot) {
        let mut analyzer = BeatSyncAnalyzer::new(sample_rate as f64);
        let mut first_grid = None;
        let mut first_lock = None;
        for chunk in samples.chunks(317) {
            let snapshot = analyzer.push_mono(chunk);
            let seconds = snapshot.sample_position as f64 / sample_rate as f64;
            if snapshot.has_grid() && first_grid.is_none() {
                first_grid = Some(seconds);
            }
            if snapshot.is_locked() && first_lock.is_none() {
                first_lock = Some(seconds);
            }
        }
        (first_grid, first_lock, analyzer.snapshot())
    }

    fn analyze_chunks(samples: &[f32], sample_rate: usize, chunks: &[usize]) -> BeatSnapshot {
        let mut analyzer = BeatSyncAnalyzer::new(sample_rate as f64);
        let mut offset = 0;
        let mut chunk_index = 0;
        while offset < samples.len() {
            let size = chunks[chunk_index % chunks.len()];
            let end = (offset + size).min(samples.len());
            analyzer.push_mono(&samples[offset..end]);
            offset = end;
            chunk_index += 1;
        }
        analyzer.snapshot()
    }

    fn tempo_change_timing(
        old_bpm: f64,
        new_bpm: f64,
    ) -> (Option<f64>, Option<f64>, BeatSnapshot) {
        const SAMPLE_RATE: usize = 48_000;
        let mut analyzer = BeatSyncAnalyzer::new(SAMPLE_RATE as f64);
        let old_song = compressed_music_mix(SAMPLE_RATE, old_bpm, 10.0);
        for chunk in old_song.chunks(317) {
            analyzer.push_mono(chunk);
        }
        let old_snapshot = analyzer.snapshot();
        assert!(old_snapshot.is_locked(), "old_bpm={old_bpm} {old_snapshot:?}");
        assert!(
            (old_snapshot.bpm - old_bpm).abs() < 1.8,
            "old_bpm={old_bpm} {old_snapshot:?}"
        );

        let new_song = compressed_music_mix(SAMPLE_RATE, new_bpm, 6.0);
        let mut consumed = 0usize;
        let mut first_provisional = None;
        let mut first_lock = None;
        for chunk in new_song.chunks(317) {
            let snapshot = analyzer.push_mono(chunk);
            consumed += chunk.len();
            let seconds = consumed as f64 / SAMPLE_RATE as f64;
            let new_tempo = (snapshot.bpm - new_bpm).abs() < 2.0;
            if new_tempo
                && snapshot.state == BeatLockState::Acquiring
                && first_provisional.is_none()
            {
                first_provisional = Some(seconds);
            }
            if new_tempo && snapshot.is_locked() && first_lock.is_none() {
                first_lock = Some(seconds);
            }
        }
        (first_provisional, first_lock, analyzer.snapshot())
    }

    #[test]
    fn locks_synthetic_tempos_at_both_sample_rates() {
        for &rate in &[44_100usize, 48_000] {
            for &bpm in &[90.0, 120.0, 150.0] {
                let samples = click_track(rate, bpm, 14.0);
                let snapshot = analyze_chunks(&samples, rate, &[127, 511, 64, 997]);
                assert!(
                    snapshot.is_locked(),
                    "rate={rate} bpm={bpm} snapshot={snapshot:?}"
                );
                assert!(
                    (snapshot.bpm - bpm).abs() < 1.6,
                    "rate={rate} bpm={bpm} snapshot={snapshot:?}"
                );
                assert!(snapshot.confidence > 0.50, "{snapshot:?}");
            }
        }
    }

    #[test]
    fn publishes_early_bpm_and_locks_compressed_music_by_eight_seconds() {
        for &bpm in &[90.0, 120.0, 150.0] {
            let samples = compressed_music_mix(48_000, bpm, 8.25);
            let (first_grid, first_lock, snapshot) = first_grid_and_lock(&samples, 48_000);
            assert!(
                first_grid.is_some_and(|seconds| seconds <= 5.0),
                "bpm={bpm} first_grid={first_grid:?} first_lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                first_lock.is_some_and(|seconds| seconds <= 8.0),
                "bpm={bpm} first_grid={first_grid:?} first_lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                (snapshot.bpm - bpm).abs() < 1.8,
                "bpm={bpm} first_grid={first_grid:?} first_lock={first_lock:?} snapshot={snapshot:?}"
            );
        }
    }

    #[test]
    fn reacquires_abrupt_compressed_music_tempo_changes_quickly() {
        for &(old_bpm, new_bpm) in &[(90.0, 140.0), (120.0, 85.0)] {
            let (first_provisional, first_lock, snapshot) =
                tempo_change_timing(old_bpm, new_bpm);
            assert!(
                first_provisional.is_some_and(|seconds| seconds <= 3.25),
                "{old_bpm}->{new_bpm} provisional={first_provisional:?} lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                first_lock.is_some_and(|seconds| seconds <= 5.25),
                "{old_bpm}->{new_bpm} provisional={first_provisional:?} lock={first_lock:?} snapshot={snapshot:?}"
            );
            assert!(
                (snapshot.bpm - new_bpm).abs() < 2.0,
                "{old_bpm}->{new_bpm} snapshot={snapshot:?}"
            );
        }
    }

    #[test]
    fn ordinary_breakdown_preserves_the_established_grid() {
        const SAMPLE_RATE: usize = 48_000;
        let mut analyzer = BeatSyncAnalyzer::new(SAMPLE_RATE as f64);
        analyzer.push_mono(&compressed_music_mix(SAMPLE_RATE, 120.0, 10.0));
        let before = analyzer.snapshot();
        assert!(before.is_locked(), "{before:?}");

        let mut breakdown = Vec::with_capacity(SAMPLE_RATE * 2);
        for sample in 0..SAMPLE_RATE * 2 {
            let seconds = sample as f64 / SAMPLE_RATE as f64;
            breakdown.push(
                (0.16 * (2.0 * std::f64::consts::PI * 220.0 * seconds).sin()) as f32,
            );
        }
        for chunk in breakdown.chunks(317) {
            let snapshot = analyzer.push_mono(chunk);
            assert!(snapshot.has_grid(), "{snapshot:?}");
            assert!((snapshot.bpm - 120.0).abs() < 1.8, "{snapshot:?}");
            assert!(
                matches!(snapshot.state, BeatLockState::Locked | BeatLockState::Holdover),
                "{snapshot:?}"
            );
        }

        analyzer.push_mono(&compressed_music_mix(SAMPLE_RATE, 120.0, 3.0));
        let after = analyzer.snapshot();
        assert!(after.is_locked(), "{after:?}");
        assert!((after.bpm - 120.0).abs() < 1.8, "{after:?}");
    }

    #[test]
    fn arbitrary_chunking_keeps_the_same_grid() {
        let samples = click_track(48_000, 120.0, 13.0);
        let regular = analyze_chunks(&samples, 48_000, &[480]);
        let odd = analyze_chunks(&samples, 48_000, &[1, 37, 1024, 79, 333]);
        assert!((regular.bpm - odd.bpm).abs() < 0.01, "{regular:?} {odd:?}");
        assert!(
            (regular.phase_sample - odd.phase_sample).abs() <= 1.0,
            "{regular:?} {odd:?}"
        );
    }

    #[test]
    fn silence_and_stationary_noise_do_not_lock() {
        let silence = vec![0.0; 48_000 * 12];
        let snapshot = analyze_chunks(&silence, 48_000, &[256]);
        assert!(!snapshot.is_locked(), "{snapshot:?}");

        let mut seed = 0x1234_5678u32;
        let mut noise = vec![0.0; 48_000 * 12];
        for sample in &mut noise {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *sample = (((seed >> 8) as f32 / 16_777_216.0) - 0.5) * 0.04;
        }
        let snapshot = analyze_chunks(&noise, 48_000, &[197, 503]);
        assert!(!snapshot.is_locked(), "{snapshot:?}");
    }

    #[test]
    fn a_locked_grid_holds_over_then_expires() {
        let mut analyzer = BeatSyncAnalyzer::new(48_000.0);
        let clicks = click_track(48_000, 120.0, 12.0);
        analyzer.push_mono(&clicks);
        assert_eq!(analyzer.snapshot().state, BeatLockState::Locked);

        analyzer.push_mono(&vec![0.0; 48_000 * 2]);
        let holdover = analyzer.snapshot();
        assert_eq!(holdover.state, BeatLockState::Holdover, "{holdover:?}");
        assert!(holdover.has_grid());

        analyzer.push_mono(&vec![0.0; 48_000 * 3]);
        let lost = analyzer.snapshot();
        assert_eq!(lost.state, BeatLockState::Lost, "{lost:?}");
        assert!(!lost.is_locked());
    }

    #[test]
    fn missed_boundary_is_advanced_not_replayed() {
        let boundary = next_boundary_sample(12_345, 100.0, 1_000.0, 1).unwrap();
        assert_eq!(boundary, 13_100);
        let eighth = next_boundary_sample(12_345, 100.0, 1_000.0, 2).unwrap();
        assert_eq!(eighth, 12_600);
    }

    #[test]
    fn two_seconds_at_120_is_four_beats() {
        let fit = fit_loop_to_grid(2.0, 120.0, 0.08).unwrap();
        assert_eq!(fit.beats, 4);
        assert!((fit.playback_rate - 1.0).abs() < 1e-9);
        assert!(fit.within_rate_limit);
    }

    #[test]
    fn unsafe_rate_is_reported_as_degraded() {
        let fit = fit_loop_to_grid(3.3, 120.0, 0.08).unwrap();
        assert_eq!(fit.beats, 8);
        assert!(!fit.within_rate_limit, "{fit:?}");
        assert!(fit.confidence < 0.5, "{fit:?}");
    }

    // ---- the published clock ----------------------------------------------

    /// Sample the published stream between two updates and assert the
    /// contract: never backwards, never faster or slower than the slew
    /// bounds allow. Returns the last sampled position.
    fn walk(clock: &BeatClock, from: f64, to: f64, last: &mut f64) {
        let step = 0.005;
        let mut at = from;
        while at <= to {
            let position = clock.position_at(at);
            assert!(position >= *last - 1e-9, "went backwards at {at}: {position} < {last}");
            let travelled = position - *last;
            // 3.5x nominal is the widest any regime may open the throttle.
            let bound = step * clock.nominal_bpm() / 60.0;
            assert!(
                travelled <= bound * 3.5 + 1e-9,
                "jumped at {at}: {travelled} beats in {step}s (bound {bound})"
            );
            *last = position;
            at += step;
        }
    }

    #[test]
    fn the_published_phase_never_jumps_through_a_hostile_sequence() {
        let period = 60.0 / 128.0;
        let mut clock = BeatClock::new();
        let mut at = 0.0;
        clock.start(at, BeatTarget::phase_only(0.0, period));
        assert_eq!(clock.epoch(), 1);
        let mut last = clock.position_at(at);

        // 1. A confident detector, drifting slightly late.
        for _ in 0..40 {
            let next = at + 0.05;
            walk(&clock, at, next, &mut last);
            let phase = ((next / period) - 0.02).rem_euclid(1.0);
            clock.discipline(next, BeatTarget::phase_only(phase, period));
            at = next;
        }
        // 2. The lock collapses (a breakdown): the clock coasts.
        for _ in 0..80 {
            let next = at + 0.05;
            walk(&clock, at, next, &mut last);
            clock.coast(next);
            at = next;
        }
        assert!(clock.coasting() && clock.running());
        // 3. It comes back somewhere ELSE — a third of a beat off.
        for _ in 0..60 {
            let next = at + 0.05;
            walk(&clock, at, next, &mut last);
            let phase = ((next / period) + 0.33).rem_euclid(1.0);
            clock.discipline(next, BeatTarget::phase_only(phase, period));
            at = next;
        }
        // 4. The operator disagrees with all of it and taps the one.
        walk(&clock, at, at + 0.05, &mut last);
        at += 0.05;
        clock.anchor(at, BeatTarget::in_bar(0, 0.0, period));
        for _ in 0..60 {
            let next = at + 0.05;
            walk(&clock, at, next, &mut last);
            at = next;
        }
        // One epoch for the whole sequence: everything else was a slew.
        assert_eq!(clock.epoch(), 1);
    }

    #[test]
    fn a_lost_lock_coasts_at_the_tempo_it_had() {
        let period = 60.0 / 130.0;
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, period));
        clock.discipline(1.0, BeatTarget::phase_only((1.0 / period).rem_euclid(1.0), period));
        let before = clock.position_at(2.0);
        let bpm = clock.nominal_bpm();
        // Sixteen seconds of nothing — a long breakdown.
        clock.coast(2.0);
        for step in 1..=160 {
            clock.coast(2.0 + step as f64 * 0.1);
        }
        assert!(clock.running() && clock.coasting());
        assert!((clock.nominal_bpm() - bpm).abs() < 1e-9, "a coast must not move the tempo");
        // The grid the drop lands on is exactly the one that was held.
        let after = clock.position_at(18.0);
        assert!(((after - before) - 16.0 / period).abs() < 1e-6, "{after} {before}");
    }

    #[test]
    fn a_regained_lock_is_eased_not_snapped() {
        let period = 0.5;
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, period));
        clock.coast(4.0);
        // Back, a third of a beat late.
        let target = (clock.position_at(4.0) + 0.3).rem_euclid(1.0);
        clock.discipline(4.0, BeatTarget::phase_only(target, period));
        assert!((clock.error_beats() - 0.3).abs() < 1e-6, "{}", clock.error_beats());
        // Not closed at once...
        clock.advance_to(4.05);
        assert!(clock.error_beats() > 0.2, "{}", clock.error_beats());
        // ...but closed over a handful of bars, and never above the bound.
        let mut at = 4.05;
        while at < 12.0 {
            at += 0.05;
            clock.advance_to(at);
            assert!(clock.bpm() <= 120.0 * 1.26, "{}", clock.bpm());
        }
        assert!(clock.error_beats().abs() < 0.02, "{}", clock.error_beats());
    }

    #[test]
    fn an_operator_anchor_converges_inside_about_a_beat() {
        let period = 0.5;
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::in_bar(0, 0.0, period));
        clock.advance_to(1.1);
        // "The one is HERE" — nearly two beats from where the clock thinks.
        clock.anchor(1.1, BeatTarget::in_bar(0, 0.0, period));
        let error = clock.error_beats();
        assert!(error.abs() > 1.0, "a real disagreement to close: {error}");
        clock.advance_to(1.1 + period);
        // One beat later most of it is gone (1/e of a time constant)...
        assert!(clock.error_beats().abs() < error.abs() * 0.42, "{}", clock.error_beats());
        clock.advance_to(1.1 + period * 4.0);
        // ...and it is done inside a bar.
        assert!(clock.error_beats().abs() < 0.05, "{}", clock.error_beats());
    }

    #[test]
    fn a_tap_during_a_coast_re_anchors_at_once() {
        let period = 0.5;
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, period));
        for step in 1..=40 {
            clock.coast(step as f64 * 0.1);
        }
        assert!(clock.coasting());
        clock.anchor(4.0, BeatTarget::in_bar(0, 0.0, period));
        assert!(!clock.coasting(), "an operator anchor ends the coast");
        assert_eq!(clock.epoch(), 1, "and it is a slew, not an epoch");
    }

    #[test]
    fn tempo_has_inertia_and_a_new_one_must_hold() {
        let period = 60.0 / 128.0;
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, period));
        // A single octave-flipped estimate is a detector wobble: ignored.
        let half = period * 2.0;
        let mut at = 0.0;
        for _ in 0..8 {
            at += 0.05;
            clock.discipline(at, BeatTarget::phase_only(0.0, half));
        }
        assert!((clock.nominal_bpm() - 128.0).abs() < 0.5, "{}", clock.nominal_bpm());
        // Held for seconds, it is a real tempo change and lands.
        while at < CLOCK_TEMPO_CLAIM_SECS + 0.2 {
            at += 0.05;
            clock.discipline(at, BeatTarget::phase_only(0.0, half));
        }
        assert!((clock.nominal_bpm() - 64.0).abs() < 0.5, "{}", clock.nominal_bpm());
    }

    #[test]
    fn a_silent_coast_eventually_stops_instead_of_pretending() {
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::phase_only(0.0, 0.5));
        let mut at = 0.0;
        while at < CLOCK_COAST_MAX_SECS + 1.0 {
            at += 0.1;
            clock.coast(at);
        }
        assert!(!clock.running(), "a minute of silence is not a breakdown");
        // ...and the next confident source is an explicit new epoch.
        clock.discipline(at + 0.1, BeatTarget::phase_only(0.0, 0.5));
        assert!(clock.running());
        assert_eq!(clock.epoch(), 2);
    }

    #[test]
    fn a_deck_grid_drives_the_published_clock_and_a_crossfade_slews() {
        // The normal show: the DJ system is master. A deck's grid goes in,
        // and the published clock IS that grid — tempo and phase.
        let deck_a = 60.0 / 133.1;
        let mut clock = BeatClock::new();
        let mut at = 0.0;
        clock.start(at, BeatTarget::in_bar(0, 0.0, deck_a));
        for _ in 0..200 {
            at += 0.05;
            let beats = at / deck_a;
            clock.discipline(
                at,
                BeatTarget::in_bar(
                    (beats.floor() as u64).rem_euclid(4),
                    beats - beats.floor(),
                    deck_a,
                ),
            );
        }
        assert!((clock.bpm() - 133.1).abs() < 0.05, "{}", clock.bpm());
        let published = clock.position_at(at);
        assert!((published - at / deck_a).abs() < 0.05, "{published} vs {}", at / deck_a);

        // The crossfader hands over to a 104.9 deck. Tempo moves (a change
        // of slope), position does NOT (a change of value would be a jump).
        let deck_b = 60.0 / 104.9;
        let before = clock.position_at(at);
        let target_at = |at: f64| {
            let beats = at / deck_b;
            BeatTarget::in_bar((beats.floor() as u64).rem_euclid(4), beats - beats.floor(), deck_b)
        };
        clock.discipline(at, target_at(at));
        assert!(
            (clock.position_at(at) - before).abs() < 1e-9,
            "the handover moved the published position"
        );
        let mut last = before;
        let handover = at;
        while at < handover + 14.0 {
            at += 0.05;
            walk(&clock, at - 0.05, at, &mut last);
            clock.discipline(at, target_at(at));
        }
        assert!((clock.bpm() - 104.9).abs() < 1.0, "{}", clock.bpm());
        assert_eq!(clock.epoch(), 1, "a crossfade is a slew, never an epoch");
    }

    #[test]
    fn a_bar_aware_target_corrects_the_downbeat_too() {
        let period = 0.5;
        let mut clock = BeatClock::new();
        clock.start(0.0, BeatTarget::in_bar(0, 0.0, period));
        clock.advance_to(1.0); // two beats in: bar position 2
        let (index, phase) = clock.bar_phase_at(1.0);
        assert_eq!(index, 2);
        assert!(phase < 1e-9);
        // The deck says that beat is actually the one.
        clock.discipline(1.0, BeatTarget::in_bar(0, 0.0, period));
        assert!((clock.error_beats() - 2.0).abs() < 1e-6, "{}", clock.error_beats());
    }

    // ---- tap tempo --------------------------------------------------------

    #[test]
    fn one_tap_is_a_downbeat_without_a_tempo() {
        let mut tap = TapTempo::new();
        let clock = tap.tap(10.0).expect("a first tap is always a tap");
        assert_eq!(clock.taps, 1);
        assert_eq!(clock.anchor_secs, 10.0);
        // One tap says WHERE the one is, never how fast the music runs.
        assert_eq!(clock.bpm, None);
        assert_eq!(tap.bpm(), None);
    }

    #[test]
    fn four_taps_set_bpm_and_phase() {
        let mut tap = TapTempo::new();
        // 128 BPM = 0.46875 s per beat.
        let period = 60.0 / 128.0;
        let mut clock = None;
        for index in 0..4 {
            clock = tap.tap(5.0 + index as f64 * period);
        }
        let clock = clock.expect("rhythmic taps are kept");
        assert_eq!(clock.taps, 4);
        assert_eq!(clock.anchor_secs, 5.0 + 3.0 * period);
        let bpm = clock.bpm.expect("four taps make a tempo");
        assert!((bpm - 128.0).abs() < 1e-6, "{bpm}");
        // The third tap must NOT have been enough.
        let mut short = TapTempo::new();
        for index in 0..3 {
            short.tap(5.0 + index as f64 * period);
        }
        assert_eq!(short.bpm(), None);
    }

    #[test]
    fn taps_keep_updating_as_the_run_continues() {
        let mut tap = TapTempo::new();
        let mut at = 0.0;
        for _ in 0..4 {
            tap.tap(at);
            at += 0.5;
        }
        assert!((tap.bpm().unwrap() - 120.0).abs() < 1e-6);
        // Drifting slightly faster inside the tolerance: the rolling mean
        // follows instead of restarting.
        at -= 0.5;
        for _ in 0..4 {
            at += 0.46;
            tap.tap(at);
        }
        assert_eq!(tap.taps(), TAP_HISTORY);
        let bpm = tap.bpm().unwrap();
        assert!(bpm > 120.0 && bpm < 131.0, "{bpm}");
    }

    #[test]
    fn a_long_gap_restarts_the_run() {
        let mut tap = TapTempo::new();
        for index in 0..4 {
            tap.tap(index as f64 * 0.5);
        }
        assert!(tap.bpm().is_some());
        // Hands off the button for three seconds: the next tap is a fresh
        // downbeat with no tempo behind it.
        let clock = tap.tap(1.5 + TAP_MAX_GAP_SECS + 0.5).unwrap();
        assert_eq!(clock.taps, 1);
        assert_eq!(clock.bpm, None);
    }

    #[test]
    fn a_wildly_different_gap_restarts_the_run() {
        let mut tap = TapTempo::new();
        for index in 0..4 {
            tap.tap(index as f64 * 0.5);
        }
        // Half time: a new tempo, not a wobble in this one.
        let clock = tap.tap(1.5 + 1.0).unwrap();
        assert_eq!(clock.taps, 1, "{clock:?}");
        assert_eq!(clock.bpm, None);
    }

    #[test]
    fn a_bounce_is_ignored_not_a_restart() {
        let mut tap = TapTempo::new();
        for index in 0..4 {
            tap.tap(index as f64 * 0.5);
        }
        assert_eq!(tap.taps(), 4);
        // A double-fired click 30 ms later must not throw the run away.
        assert_eq!(tap.tap(1.53), None);
        assert_eq!(tap.taps(), 4);
        assert!((tap.bpm().unwrap() - 120.0).abs() < 1e-6);
    }

    #[test]
    fn tap_bpm_stays_inside_the_plausible_band() {
        let mut tap = TapTempo::new();
        // Right at the slow limit: gaps of exactly TAP_MAX_GAP_SECS are kept.
        for index in 0..4 {
            tap.tap(index as f64 * TAP_MAX_GAP_SECS);
        }
        let bpm = tap.bpm().unwrap();
        assert!((bpm - 24.0).abs() < 1e-6, "{bpm}");
        assert!(bpm >= 60.0 / TAP_MAX_GAP_SECS && bpm <= 60.0 / TAP_MIN_GAP_SECS);
    }
}
