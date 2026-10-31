//! The realtime mode as one object: the streaming tracker steering the
//! published clock, for a caller that just wants "where is the beat now".
//!
//! Feed it the sound as it plays ([`LiveTempo::push`]) and step it with the
//! time that passed ([`LiveTempo::advance`]); it returns a continuous beat
//! position that never jumps or runs backwards. Before anything locks it
//! free-runs at the last tempo it trusted (120 BPM before any), so
//! beat-driven visuals still pulse; a first lock is taken at operator speed
//! (inside about a beat), later corrections are slewed like any other.
//!
//! A known tempo ([`LiveTempo::set_prior`]) — a tag, a whole-file pass —
//! decides the octave and shortens the lock.

use crate::realtime::{BeatClock, BeatSnapshot, BeatSyncAnalyzer, BeatTarget};

/// The free-running tempo before anything was locked.
const DEFAULT_BPM: f64 = 120.0;

pub struct LiveTempo {
    analyzer: Option<Box<BeatSyncAnalyzer>>,
    rate: f64,
    prior: Option<f64>,
    clock: BeatClock,
    /// Seconds of play since the start.
    now: f64,
    snapshot: BeatSnapshot,
    /// A lock the clock has not been told about yet.
    pending: Option<BeatTarget>,
    /// The clock has followed a lock since the last reset.
    followed: bool,
}

impl Default for LiveTempo {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveTempo {
    pub fn new() -> Self {
        LiveTempo {
            analyzer: None,
            rate: 0.0,
            prior: None,
            clock: BeatClock::new(),
            now: 0.0,
            snapshot: BeatSnapshot::default(),
            pending: None,
            followed: false,
        }
    }

    /// Forget the music (another source started); keeps the prior only if
    /// the caller sets it again, since a new source is a new record.
    pub fn reset(&mut self) {
        let now = self.now;
        let position = self.clock.position_at(now);
        let bpm = self.clock.nominal_bpm();
        *self = LiveTempo { now, ..LiveTempo::new() };
        // Keep the published beat continuous through the reset.
        if position > 0.0 {
            self.clock.start(now, BeatTarget::phase_only(position.fract(), 60.0 / bpm.max(1.0)));
        }
    }

    /// A tempo known from elsewhere, or `None`.
    pub fn set_prior(&mut self, bpm: Option<f64>) {
        self.prior = bpm;
        if let Some(analyzer) = self.analyzer.as_mut() {
            analyzer.set_prior(bpm);
        }
    }

    /// Feed mono samples at `rate` Hz, in play order.
    pub fn push(&mut self, samples: &[f32], rate: f64) {
        if !(rate >= 8_000.0) || !rate.is_finite() {
            return;
        }
        if self.analyzer.is_none() || (self.rate - rate).abs() > 0.5 {
            let mut analyzer = Box::new(BeatSyncAnalyzer::new(rate));
            analyzer.set_prior(self.prior);
            self.analyzer = Some(analyzer);
            self.rate = rate;
        }
        let Some(analyzer) = self.analyzer.as_mut() else { return };
        let snapshot = analyzer.push_mono(samples);
        self.follow(snapshot, 0);
    }

    /// Follow a tracker that runs somewhere else (a worker thread, reached
    /// through [`crate::realtime::SnapshotCell`]): `snapshot` is its latest
    /// reading and `lead_samples` how much sound has played since the
    /// sample it was taken at. Only the clock runs here, which costs next
    /// to nothing; [`LiveTempo::push`] is this with the tracker inline.
    pub fn follow(&mut self, snapshot: BeatSnapshot, lead_samples: u64) {
        self.snapshot = snapshot;
        self.pending = None;
        if snapshot.is_locked() {
            let at = snapshot.sample_position.saturating_add(lead_samples);
            if let (Some(phase), Some(period)) = (snapshot.phase_at(at), snapshot.seconds_per_beat()) {
                self.pending = Some(BeatTarget::phase_only(phase, period));
            }
        }
    }

    /// Advance by `dt` seconds of play. Returns the beat position
    /// (continuous, in beats) and the tempo it runs at.
    pub fn advance(&mut self, dt: f64) -> (f64, f64) {
        self.now += dt.max(0.0);
        let now = self.now;
        if !self.clock.running() {
            let bpm = if self.followed { self.clock.nominal_bpm() } else { DEFAULT_BPM };
            self.clock.start(now, BeatTarget::phase_only(0.0, 60.0 / bpm));
        }
        self.clock.advance_to(now);
        match self.pending.take() {
            Some(target) => {
                if !self.followed {
                    // The first lock replaces a guess, not a measurement:
                    // take its tempo at once and its phase inside a beat.
                    self.clock.arm_operator();
                    self.followed = true;
                }
                self.clock.discipline(now, target);
            }
            None if !self.snapshot.is_locked() && self.followed => self.clock.coast(now),
            None => {}
        }
        (self.clock.position_at(now), self.clock.bpm())
    }

    /// The clock follows the music.
    pub fn locked(&self) -> bool {
        self.snapshot.is_locked()
    }

    /// The tempo the clock runs at.
    pub fn bpm(&self) -> f64 {
        if self.clock.running() {
            self.clock.nominal_bpm()
        } else {
            DEFAULT_BPM
        }
    }

    /// How sure the tracker is of its grid, 0..1.
    pub fn confidence(&self) -> f32 {
        self.snapshot.confidence
    }

    /// The tracker's latest reading.
    pub fn snapshot(&self) -> BeatSnapshot {
        self.snapshot
    }
}
