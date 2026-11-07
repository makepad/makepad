//! The beat as a picture reads it: one object a visualiser feeds the sound
//! it hears and steps once a frame.
//!
//! [`BeatPulse`] runs the streaming tracker ([`BeatSyncAnalyzer`]) with a
//! published clock ([`BeatClock`]) and reads the tracker's own onset front
//! end, hop by hop, for what a picture moves with between beats: the onset
//! strength and four band levels. Once a frame ([`BeatPulse::frame`]) it
//! publishes a [`BeatFrame`]: the tempo and a confidence, the beat and bar
//! positions, beat and bar events, a decaying pulse on each, the onsets and
//! the levels.
//!
//! * **Predicted, not reacted.** Events come from the clock crossing a
//!   beat, so they land on the beat instead of a hop after the onset that
//!   showed it, and the frame-to-frame motion is smooth.
//! * **Close behind the tracker.** A picture wants the beat where it is
//!   now, not a flywheel's inertia ([`crate::LiveTempo`] keeps that, for
//!   decks): every locked reading steers the clock at the operator's rate,
//!   so the clock takes the tracker's tempo as it is published and closes a
//!   phase difference within about a beat, always as a slew, never a jump.
//!   The tracker's own hysteresis (a new tempo has to repeat before it
//!   locks) is what keeps that steady.
//! * **Only real beats.** Events and pulses come only while the tracker
//!   follows a beat (locked, or holding over a short dropout); the pulses
//!   fade in and out with it. Noise, talk and beatless passages move the
//!   levels and onsets and never fire a beat.
//! * **The bar.** The clock counts four beats to a bar. Which beat is the
//!   one is a vote: each beat's accent (the kick lane's energy around it)
//!   counts toward its place in the bar, and another place takes over only
//!   when it clearly leads, so the bar is stable. Music whose beats are all alike (the same kick
//!   on every beat) has no audible one; its bars are still four beats,
//!   starting on a beat.
//! * **The person's say.** A phase nudge ([`BeatPulse::set_phase_nudge`])
//!   moves everything published on the beat a fraction of a beat earlier
//!   or later (a display that shows late, a taste for the hit a touch
//!   ahead), and a tap on the one ([`BeatPulse::tap_downbeat`]) says which
//!   beat starts the bar when the vote picks another.
//! * **A room microphone.** The front end works in log energy against an
//!   adaptive floor, so a quiet room and a loud one read alike and steady
//!   noise reads as nothing. The levels are auto-gained between a floor
//!   that follows the room's own noise and a ceiling that follows the
//!   loudest recent moment, so they swing 0..1 at any loudness and the
//!   noise alone stays near 0.
//!
//! Cost: the tracker's front end is a few one-pole filters per sample, its
//! tempo analysis runs four times a second, and the levels a few
//! logarithms per 10 ms hop. Nothing allocates after the first push (which
//! makes the tracker), so it may run on the UI thread or a worker.

use crate::onset::{OnsetHop, BANDS, HOPS_PER_SEC};
use crate::realtime::{BeatClock, BeatSnapshot, BeatSyncAnalyzer, BeatTarget};

/// Beats to a bar.
const BAR_BEATS: i64 = 4;
/// The tempo the clock runs at before the first beat.
const FIRST_BPM: f64 = 120.0;
/// The beat pulse falls by e^-this over a beat; the bar pulse by e^-that
/// per beat of its bar.
const PULSE_DECAY: f64 = 5.0;
const BAR_PULSE_DECAY: f64 = 3.0;
/// How quickly the pulses fade in once the tracker follows a beat, and out
/// once it lets go, in seconds.
const PRESENCE_SECS: f64 = 0.3;
/// A composite or lane novelty this strong reads as a full onset.
const ONSET_FULL: f32 = 3.0;
/// The onset envelopes' release, seconds.
const ONSET_RELEASE_SECS: f64 = 0.1;
/// The levels' release, seconds (the attack is immediate).
const LEVEL_RELEASE_SECS: f64 = 0.12;
/// Auto-gain: the ceiling falls this fast after the loudest moment, and
/// the floor rises this fast after the quietest, in dB a second.
const CEILING_FALL_DB: f32 = 4.0;
const FLOOR_RISE_DB: f32 = 2.0;
/// The span between floor and ceiling is at least this, so the room's own
/// noise never reads as full scale, and at most this, so a silence before
/// the music does not leave the floor far below it.
const MIN_SPAN_DB: f32 = 12.0;
const MAX_SPAN_DB: f32 = 40.0;
/// This close above the floor a level still reads 0: the noise's jitter.
const FLOOR_MARGIN_DB: f32 = 3.0;
/// A hop within this many beats of a beat counts toward that beat's accent.
const ACCENT_WINDOW: f64 = 0.2;
/// How much one bar's accent moves its place's vote (the first one sets
/// it).
const VOTE_GAIN: f32 = 0.125;
/// Another place in the bar becomes the one when its vote leads the
/// current one's by this many dB.
const VOTE_LEAD_DB: f32 = 1.0;
/// The levels: the four lanes, then all of them together.
const LEVELS: usize = BANDS + 1;
/// Energy below this is digital silence (-140 dB).
const SILENT_ENERGY: f32 = 1e-14;
/// The phase nudge goes this far either way, in beats.
const MAX_NUDGE: f64 = 0.5;

/// What a picture reads from the music at one frame.
///
/// Positions run forward only. Events, pulses, the tempo and the
/// confidence are there only while the tracker follows a beat; the
/// positions and phases keep running at the last tempo between beats, for
/// motion that does not stop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BeatFrame {
    /// Beats a minute; 0 without a beat.
    pub tempo: f32,
    /// How sure the tracker is of its grid, 0..1; 0 without a beat.
    pub confidence: f32,
    /// The tracker follows a beat (locked, or holding over a dropout).
    pub locked: bool,
    /// Beats so far plus the phase of this one. Never runs back.
    pub beat_pos: f64,
    /// Bars so far plus the phase of this one. Never runs back.
    pub bar_pos: f64,
    /// 0..1 through this beat (the fraction of `beat_pos`).
    pub beat_phase: f32,
    /// 0..1 through this bar (the fraction of `bar_pos`).
    pub bar_phase: f32,
    /// Which beat of the bar this is; 0 is the one.
    pub beat_in_bar: u32,
    /// A beat started since the last frame.
    pub on_beat: bool,
    /// A bar started since the last frame.
    pub on_bar: bool,
    /// 1 on each beat, falling to near 0 by the next.
    pub beat_pulse: f32,
    /// 1 on each bar's first beat, falling to near 0 within it.
    pub bar_pulse: f32,
    /// Onset strength, 0..1: how sharply the sound rose just now, over
    /// every band.
    pub onset: f32,
    /// The same per band: kick, body, presence, air.
    pub onsets: [f32; BANDS],
    /// Band levels, smoothed and auto-gained, 0..1: kick (below 170 Hz),
    /// body (to 700 Hz), presence (to 2.8 kHz), air.
    pub bands: [f32; BANDS],
    /// All bands together, the same way.
    pub level: f32,
}

impl BeatFrame {
    /// The frame as two four-vectors for a shader, in the names every
    /// runtime uses: `beat` = (beat_pulse, bar_pulse, beat_pos, bar_pos)
    /// and `tempo` = (tempo, beat_conf, onset, beat_in_bar). The phases
    /// are the fractions of the positions.
    pub fn uniforms(&self) -> ([f32; 4], [f32; 4]) {
        (
            [self.beat_pulse, self.bar_pulse, self.beat_pos as f32, self.bar_pos as f32],
            [self.tempo, self.confidence, self.onset, self.beat_in_bar as f32],
        )
    }
}

/// The streaming tracker and its clock, read the way a picture reads them.
pub struct BeatPulse {
    /// The tracker, made by the first push (and again when the rate
    /// changes), and the tempo known from elsewhere it is told.
    tracker: Option<Box<BeatSyncAnalyzer>>,
    prior: Option<f64>,
    /// Its newest reading, and whether it came since the last frame.
    snapshot: BeatSnapshot,
    fresh: bool,
    clock: BeatClock,
    /// Seconds of frames since the start.
    now: f64,
    /// The clock has followed the tracker since it last started.
    followed: bool,
    /// The rate of the last push.
    rate: f64,
    /// Added to the clock's position to make the published one, which
    /// carries on through a restart of the clock.
    offset: f64,
    /// Added to the bars counted from the one, so a new one never moves
    /// the bar back.
    bar_offset: f64,
    /// The clock's position as of the last frame, and the position it
    /// made (the published one before the nudge).
    clock_at: f64,
    raw_beat: f64,
    /// The person's phase nudge, in beats (+ earlier).
    nudge: f64,
    /// The one was tapped: it holds against the vote while the beat is
    /// followed.
    tapped: bool,
    /// The clock's position (without the nudge), the tempo it runs at, and
    /// the tracker's sample count, as of the last frame: how a hop pushed
    /// since is placed on the beat.
    beat_at: f64,
    beats_per_sec: f64,
    sample_at: u64,
    locked: bool,
    /// The place in the bar (a beat number modulo four) that is the one.
    downbeat: i64,
    presence: f32,
    hops: Hops,
    last: BeatFrame,
}

/// What each hop leaves for the next frame: the accents, the levels and
/// the onsets.
struct Hops {
    /// The beat whose accent is being collected, and the kick lane's energy
    /// summed over its hops (a sum, not a peak, so where the hops fall on
    /// the kick does not matter).
    accent_beat: Option<i64>,
    accent: f32,
    /// Each place in the bar's mean accent (dB), once it has had one.
    votes: [Option<f32>; BAR_BEATS as usize],
    /// Smoothed energy, and the auto-gain's floor and ceiling in dB.
    energy: [f32; LEVELS],
    floor: [f32; LEVELS],
    ceiling: [f32; LEVELS],
    /// The newest auto-gained levels.
    gained: [f32; LEVELS],
    leveled: bool,
    /// The strongest novelty per lane, and the composite, since the last
    /// frame.
    onset_new: [f32; LEVELS],
}

impl Default for BeatPulse {
    fn default() -> Self {
        Self::new()
    }
}

impl BeatPulse {
    pub fn new() -> Self {
        BeatPulse {
            tracker: None,
            prior: None,
            snapshot: BeatSnapshot::default(),
            fresh: false,
            clock: BeatClock::new(),
            now: 0.0,
            followed: false,
            rate: 0.0,
            offset: 0.0,
            bar_offset: 0.0,
            clock_at: 0.0,
            raw_beat: 0.0,
            nudge: 0.0,
            tapped: false,
            beat_at: 0.0,
            beats_per_sec: 2.0,
            sample_at: 0,
            locked: false,
            downbeat: 0,
            presence: 0.0,
            hops: Hops {
                accent_beat: None,
                accent: 0.0,
                votes: [None; BAR_BEATS as usize],
                energy: [0.0; LEVELS],
                floor: [0.0; LEVELS],
                ceiling: [0.0; LEVELS],
                gained: [0.0; LEVELS],
                leveled: false,
                onset_new: [0.0; LEVELS],
            },
            last: BeatFrame::default(),
        }
    }

    /// Another source started: forget its beat and its bar (a tapped one
    /// too). The positions carry on, the nudge stays; the levels adapt by
    /// themselves.
    pub fn reset(&mut self) {
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.reset();
            self.snapshot = tracker.snapshot();
        }
        self.hops.votes = [None; BAR_BEATS as usize];
        self.hops.accent_beat = None;
        self.tapped = false;
    }

    /// The person's phase nudge, in beats, -0.5..0.5: positive moves the
    /// beats, the bars, their events and pulses that much earlier, negative
    /// later. A nudge back holds the positions still until the music has
    /// caught up with them; they never run back.
    pub fn set_phase_nudge(&mut self, beats: f64) {
        if beats.is_finite() {
            self.nudge = beats.clamp(-MAX_NUDGE, MAX_NUDGE);
        }
    }

    /// The phase nudge in force, in beats.
    pub fn phase_nudge(&self) -> f64 {
        self.nudge
    }

    /// The person tapped the one, on the music as heard: the music's beat
    /// nearest the last frame (the nudge aside) starts the bar from now on,
    /// whatever the vote says, until the beat is lost or
    /// [`BeatPulse::reset`]. The bars run forward through it. Returns how
    /// far from that beat the tap fell (beats, + after it), or `None` (and
    /// nothing changes) while no beat is followed.
    pub fn tap_downbeat(&mut self) -> Option<f64> {
        if !self.locked {
            return None;
        }
        let nearest = self.raw_beat.round();
        self.move_one((nearest as i64).rem_euclid(BAR_BEATS), self.last.beat_pos);
        self.tapped = true;
        Some(self.raw_beat - nearest)
    }

    /// The bar's first beat becomes `place` (a beat number modulo four),
    /// the bar count stepping on rather than back at `beat_pos`.
    fn move_one(&mut self, place: i64, beat_pos: f64) {
        let before = self.bar_offset + (beat_pos - self.downbeat as f64) / BAR_BEATS as f64;
        let after = self.bar_offset + (beat_pos - place as f64) / BAR_BEATS as f64;
        if after < before {
            self.bar_offset += 1.0;
        }
        self.downbeat = place;
    }

    /// A tempo known from elsewhere (a tag, a whole-file pass), or `None`:
    /// it decides the octave and shortens the lock, and is never published
    /// unmeasured.
    pub fn set_prior(&mut self, bpm: Option<f64>) {
        self.prior = bpm;
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.set_prior(bpm);
        }
    }

    /// The sound since the last push: mono samples at `rate` Hz, in the
    /// order they were heard.
    pub fn push(&mut self, samples: &[f32], rate: f64) {
        if !(rate.is_finite() && rate >= 8_000.0) {
            return;
        }
        if self.tracker.is_none() || (self.rate - rate).abs() > 0.5 {
            let mut tracker = Box::new(BeatSyncAnalyzer::new(rate));
            tracker.set_prior(self.prior);
            self.tracker = Some(tracker);
            self.sample_at = 0;
        }
        self.rate = rate;
        let hop_secs = 1.0 / HOPS_PER_SEC;
        let half_hop = (rate * 0.5 * hop_secs) as u64;
        let (beat_at, beats_per_sec, sample_at, locked) = (self.beat_at, self.beats_per_sec, self.sample_at, self.locked);
        let hops = &mut self.hops;
        let Some(tracker) = self.tracker.as_mut() else { return };
        self.snapshot = tracker.push_mono_with(samples, |hop| {
            hops.level(hop, hop_secs as f32);
            if locked {
                let centre = hop.end_sample.saturating_sub(half_hop);
                let beat = beat_at + centre.saturating_sub(sample_at) as f64 / rate * beats_per_sec;
                hops.accent(beat, hop);
            }
        });
        self.fresh = true;
    }

    /// One frame, `dt` seconds after the last.
    pub fn frame(&mut self, dt: f64) -> BeatFrame {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.now += dt;
        let now = self.now;
        if !self.clock.running() {
            let bpm = if self.followed { self.clock.nominal_bpm() } else { FIRST_BPM };
            self.clock.start(now, BeatTarget::phase_only(0.0, 60.0 / bpm));
        }
        self.clock.advance_to(now);
        let snapshot = self.snapshot;
        let locked = snapshot.is_locked();
        if locked && self.fresh {
            // The newest reading is of the newest sample, which is heard now.
            if let (Some(phase), Some(period)) = (snapshot.phase_at(snapshot.sample_position), snapshot.seconds_per_beat()) {
                self.clock.anchor(now, BeatTarget::phase_only(phase, period));
                self.followed = true;
            }
        } else if !locked && self.followed {
            self.clock.coast(now);
        }
        self.fresh = false;
        let clock = self.clock.position_at(now);
        let running_bpm = self.clock.bpm();
        let bpm = self.clock.nominal_bpm();

        // The clock starts over after a long silence or a reset: the
        // published position carries on from where it was, to the next
        // whole beat at most, and the bar's vote starts over with it.
        if clock < self.clock_at {
            self.offset = (self.raw_beat - clock - 1e-9).ceil();
            self.hops.votes = [None; BAR_BEATS as usize];
            self.hops.accent_beat = None;
        }
        self.clock_at = clock;
        let raw_beat = (self.offset + clock).max(self.raw_beat);
        self.raw_beat = raw_beat;
        // The nudge moves what is published, not the grid the hops are
        // placed on (a beat's accent is the sound around the beat itself).
        let beat_pos = (raw_beat + self.nudge).max(self.last.beat_pos);
        let crossed = beat_pos.floor() > self.last.beat_pos.floor();

        // The one, from the vote unless it was tapped; bars run forward
        // through a change of it.
        self.tapped &= locked;
        let lead = if self.tapped { self.downbeat } else { self.hops.leader(self.downbeat) };
        if lead != self.downbeat {
            self.move_one(lead, beat_pos);
        }
        let bar_pos = (self.bar_offset + (beat_pos - self.downbeat as f64) / BAR_BEATS as f64).max(self.last.bar_pos);
        let new_bar = bar_pos.floor() > self.last.bar_pos.floor();

        let target = if locked { 1.0 } else { 0.0 };
        self.presence += (target - self.presence) * (1.0 - (-dt / PRESENCE_SECS).exp()) as f32;
        let beat_phase = beat_pos.fract();
        let bar_phase = bar_pos.fract();
        let beat_in_bar = ((bar_phase * BAR_BEATS as f64) as u32).min(BAR_BEATS as u32 - 1);
        let in_bar = beat_in_bar as f64 + beat_phase;

        let release = (-dt / ONSET_RELEASE_SECS).exp() as f32;
        let mut onsets = [0.0f32; LEVELS];
        let previous = [self.last.onsets[0], self.last.onsets[1], self.last.onsets[2], self.last.onsets[3], self.last.onset];
        for (i, onset) in onsets.iter_mut().enumerate() {
            let fresh = (self.hops.onset_new[i] / ONSET_FULL).min(1.0);
            *onset = fresh.max(previous[i] * release);
        }
        self.hops.onset_new = [0.0; LEVELS];
        let gained = self.hops.gained;

        let frame = BeatFrame {
            tempo: if locked { bpm as f32 } else { 0.0 },
            confidence: if locked { snapshot.confidence } else { 0.0 },
            locked,
            beat_pos,
            bar_pos,
            beat_phase: beat_phase as f32,
            bar_phase: bar_phase as f32,
            beat_in_bar,
            on_beat: locked && crossed,
            on_bar: locked && crossed && new_bar,
            beat_pulse: self.presence * (-PULSE_DECAY * beat_phase).exp() as f32,
            bar_pulse: self.presence * (-BAR_PULSE_DECAY * in_bar).exp() as f32,
            onset: onsets[BANDS],
            onsets: [onsets[0], onsets[1], onsets[2], onsets[3]],
            bands: [gained[0], gained[1], gained[2], gained[3]],
            level: gained[BANDS],
        };
        self.beat_at = raw_beat;
        self.beats_per_sec = running_bpm / 60.0;
        self.sample_at = snapshot.sample_position;
        self.locked = locked;
        self.last = frame;
        frame
    }

    /// The frame [`BeatPulse::frame`] made last.
    pub fn last(&self) -> BeatFrame {
        self.last
    }
}

impl Hops {
    /// A hop's lane levels into the smoothed, auto-gained ones.
    fn level(&mut self, hop: &OnsetHop, hop_secs: f32) {
        let release = (-hop_secs as f64 / LEVEL_RELEASE_SECS).exp() as f32;
        let mut energy = [0.0f32; LEVELS];
        for band in 0..BANDS {
            energy[band] = hop.band_rms[band] * hop.band_rms[band];
            energy[BANDS] += energy[band];
        }
        for i in 0..LEVELS {
            self.energy[i] = energy[i].max(self.energy[i] * release);
            let db = 10.0 * (self.energy[i] + SILENT_ENERGY).log10();
            if !self.leveled {
                self.floor[i] = db;
                self.ceiling[i] = db;
            }
            self.ceiling[i] = if db > self.ceiling[i] { db } else { self.ceiling[i] - CEILING_FALL_DB * hop_secs };
            self.floor[i] = if db < self.floor[i] { db } else { self.floor[i] + FLOOR_RISE_DB * hop_secs };
            self.floor[i] = self.floor[i].max(self.ceiling[i] - MAX_SPAN_DB).min(self.ceiling[i]);
            let span = (self.ceiling[i] - self.floor[i]).max(MIN_SPAN_DB) - FLOOR_MARGIN_DB;
            self.gained[i] = ((db - self.floor[i] - FLOOR_MARGIN_DB) / span).clamp(0.0, 1.0);
        }
        self.leveled = true;
        for band in 0..BANDS {
            self.onset_new[band] = self.onset_new[band].max(hop.novelty[band]);
        }
        self.onset_new[BANDS] = self.onset_new[BANDS].max(hop.onset);
    }

    /// A hop at `beat` (on the clock's position): near a beat, it
    /// accents that beat. A beat's accent is counted once the hops have
    /// moved on to the next one.
    fn accent(&mut self, beat: f64, hop: &OnsetHop) {
        let nearest = beat.round();
        if (beat - nearest).abs() > ACCENT_WINDOW {
            return;
        }
        let nearest = nearest as i64;
        let energy = hop.band_rms[0] * hop.band_rms[0];
        if self.accent_beat == Some(nearest) {
            self.accent += energy;
            return;
        }
        if let Some(at) = self.accent_beat {
            let place = at.rem_euclid(BAR_BEATS) as usize;
            let accent = 10.0 * (self.accent + SILENT_ENERGY).log10();
            let vote = self.votes[place].get_or_insert(accent);
            *vote += (accent - *vote) * VOTE_GAIN;
        }
        self.accent_beat = Some(nearest);
        self.accent = energy;
    }

    /// The place in the bar that is the one: `current`, unless another
    /// clearly leads it.
    fn leader(&self, current: i64) -> i64 {
        let mut best = (current, self.votes[current as usize]);
        for place in 0..BAR_BEATS {
            if let Some(vote) = self.votes[place as usize] {
                if best.1.is_none_or(|lead| vote > lead) {
                    best = (place, Some(vote));
                }
            }
        }
        match (self.votes[current as usize], best.1) {
            (Some(now), Some(lead)) if lead - now <= VOTE_LEAD_DB => current,
            _ => best.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    /// Counts this thread's allocations, so a test can show a stretch of
    /// work allocated nothing.
    struct Counting;

    thread_local! {
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
            unsafe { System.realloc(ptr, layout, size) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    fn allocations() -> u64 {
        ALLOCATIONS.with(|n| n.get())
    }

    const RATE: f64 = 48_000.0;
    /// Samples a 60 Hz frame brings.
    const FRAME: usize = 800;

    struct Noise(u32);

    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
        }
    }

    /// A band heard through a webcam's microphone in a room: kick, snare
    /// on two and four, eighth-note hats, the first kick of each bar the
    /// loudest; three early reflections, the microphone's bass roll-off
    /// (a one-pole high-pass at 120 Hz), and the room's own noise. The
    /// tempo and the loudness may change over time. Returns the samples
    /// and every beat's time, with whether it starts a bar.
    fn band_in_a_room(seconds: f64, bpm_at: impl Fn(f64) -> f64, gain_at: impl Fn(f64) -> f32, room_noise: f32) -> (Vec<f32>, Vec<(f64, bool)>) {
        let len = (seconds * RATE) as usize;
        let mut dry = vec![0.0f32; len];
        let mut noise = Noise(0x2545_f491);
        let mut beats = Vec::new();
        let mut at = 0.31;
        while at < seconds - 0.3 {
            let index = beats.len();
            let period = 60.0 / bpm_at(at);
            beats.push((at, index % 4 == 0));
            let start = (at * RATE) as usize;
            let accent = [1.0f32, 0.62, 0.8, 0.62][index % 4];
            // Kick: a falling pitch with a click.
            for i in 0..(0.25 * RATE) as usize {
                let t = i as f64 / RATE;
                let phase = std::f64::consts::TAU * (55.0 * t + 95.0 / 25.0 * (1.0 - (-25.0 * t).exp()));
                let click = if t < 0.003 { 0.3 * noise.next() } else { 0.0 };
                dry[start + i] += accent * ((-18.0 * t).exp() as f32 * phase.sin() as f32 + click);
            }
            if index % 2 == 1 {
                for i in 0..(0.12 * RATE) as usize {
                    let t = i as f64 / RATE;
                    let body = 0.3 * (std::f64::consts::TAU * 190.0 * t).sin() as f32;
                    dry[start + i] += 0.55 * (-30.0 * t).exp() as f32 * (0.5 * noise.next() + body);
                }
            }
            for half in 0..2 {
                let hat = start + (half as f64 * 0.5 * period * RATE) as usize;
                let mut previous = 0.0;
                for i in 0..(0.03 * RATE) as usize {
                    let t = i as f64 / RATE;
                    let n = noise.next();
                    if hat + i < len {
                        dry[hat + i] += 0.2 * (-120.0 * t).exp() as f32 * (n - previous);
                    }
                    previous = n;
                }
            }
            at += period;
        }
        let mut wet = dry.clone();
        for (delay, gain) in [(0.023, 0.35f32), (0.041, 0.22), (0.067, 0.12)] {
            let d = (delay * RATE) as usize;
            for i in d..len {
                wet[i] += gain * dry[i - d];
            }
        }
        let alpha = (-std::f64::consts::TAU * 120.0 / RATE).exp() as f32;
        let (mut x1, mut y1) = (0.0f32, 0.0f32);
        for (i, sample) in wet.iter_mut().enumerate() {
            let y = alpha * (y1 + *sample - x1);
            x1 = *sample;
            y1 = y;
            *sample = 0.4 * gain_at(i as f64 / RATE) * y + room_noise * noise.next();
        }
        (wet, beats)
    }

    /// Play `samples` a 60 Hz frame at a time; every frame's reading and
    /// the time at its end.
    fn play(pulse: &mut BeatPulse, samples: &[f32]) -> Vec<(f64, BeatFrame)> {
        let mut heard = 0usize;
        samples
            .chunks(FRAME)
            .map(|chunk| {
                pulse.push(chunk, RATE);
                heard += chunk.len();
                (heard as f64 / RATE, pulse.frame(chunk.len() as f64 / RATE))
            })
            .collect()
    }

    /// For each event from `from` seconds to the last beat: how far it
    /// lands from the nearest beat (seconds, + late), and whether that beat
    /// starts a bar.
    fn landings(frames: &[(f64, BeatFrame)], beats: &[(f64, bool)], from: f64, event: impl Fn(&BeatFrame) -> bool) -> Vec<(f64, bool)> {
        let last = beats.last().unwrap().0 + 0.1;
        frames
            .iter()
            .filter(|(at, f)| *at >= from && *at <= last && event(f))
            .map(|(at, _)| {
                let nearest = beats.iter().min_by(|a, b| (a.0 - at).abs().total_cmp(&(b.0 - at).abs())).unwrap();
                (at - nearest.0, nearest.1)
            })
            .collect()
    }

    /// How many beats fall from `from` seconds to the last one.
    fn beats_from(beats: &[(f64, bool)], from: f64) -> usize {
        beats.iter().filter(|(at, _)| *at >= from).count()
    }

    /// Every event lands within a 60 Hz frame and a hop of its beat
    /// (a little early, or up to a frame and a half late).
    fn assert_on_the_beat(landings: &[(f64, bool)], what: &str) {
        for (error, _) in landings {
            assert!((-0.02..=0.035).contains(error), "{what}: a beat {:.0} ms off", error * 1000.0);
        }
    }

    fn range(frames: &[(f64, BeatFrame)], from: f64, to: f64, value: impl Fn(&BeatFrame) -> f32) -> (f32, f32) {
        let values = frames.iter().filter(|(at, _)| *at >= from && *at < to).map(|(_, f)| value(f));
        values.fold((f32::MAX, f32::MIN), |(low, high), v| (low.min(v), high.max(v)))
    }

    #[test]
    fn beats_and_bars_land_on_the_band_at_several_tempos() {
        for bpm in [92.0, 120.0, 146.0] {
            let what = format!("{bpm} BPM");
            let (samples, beats) = band_in_a_room(18.0, |_| bpm, |_| 1.0, 0.004);
            let mut pulse = BeatPulse::new();
            let frames = play(&mut pulse, &samples);

            let locked_at = frames.iter().find(|(_, f)| f.locked).map(|(at, _)| *at);
            assert!(locked_at.is_some_and(|at| at < 8.0), "{what}: locked at {locked_at:?}");
            let last = frames.last().unwrap().1;
            assert!((last.tempo as f64 - bpm).abs() < bpm * 0.015, "{what}: tempo {}", last.tempo);
            assert!(last.confidence > 0.4, "{what}: confidence {}", last.confidence);

            // From ten seconds on: one event a beat, each on its beat.
            let on_beats = landings(&frames, &beats, 10.0, |f| f.on_beat);
            let expected = beats_from(&beats, 10.0);
            assert!(on_beats.len().abs_diff(expected) <= 1, "{what}: {} beats for {expected}", on_beats.len());
            assert_on_the_beat(&on_beats, &what);
            // The bars start on the one (the loudest kick).
            let on_bars = landings(&frames, &beats, 10.0, |f| f.on_bar);
            assert!(on_bars.len() >= 2, "{what}: {} bars", on_bars.len());
            assert!(on_bars.iter().all(|(_, one)| *one), "{what}: a bar off the one: {on_bars:?}, votes {:?}", pulse.hops.votes);

            // The pulse is up on the beat and down between; the kick lane
            // swings with the kick.
            let (low, high) = range(&frames, 10.0, 18.0, |f| f.beat_pulse);
            assert!(high > 0.8 && low < 0.1, "{what}: pulse {low}..{high}");
            let (low, high) = range(&frames, 10.0, 18.0, |f| f.bands[0]);
            assert!(high > 0.7 && low < 0.4, "{what}: kick level {low}..{high}");
            assert!(
                frames.windows(2).all(|w| w[1].1.beat_pos >= w[0].1.beat_pos && w[1].1.bar_pos >= w[0].1.bar_pos),
                "{what}: a position ran back"
            );
        }
    }

    #[test]
    fn the_frame_loop_allocates_nothing() {
        // A second play of the same music: every path the first one took
        // exists by then (a JIT builds code on first use), and the tracker
        // is made by the first push.
        let (samples, _) = band_in_a_room(14.0, |_| 128.0, |_| 1.0, 0.004);
        play(&mut BeatPulse::new(), &samples);
        let mut pulse = BeatPulse::new();
        pulse.push(&samples[..FRAME], RATE);
        pulse.frame(FRAME as f64 / RATE);
        let before = allocations();
        let mut locked = false;
        for chunk in samples[FRAME..].chunks(FRAME) {
            pulse.push(chunk, RATE);
            locked = pulse.frame(chunk.len() as f64 / RATE).locked;
        }
        let during = allocations() - before;
        assert!(locked, "the music was followed");
        assert_eq!(during, 0, "allocations in the frame loop");
    }

    #[test]
    fn a_new_song_at_another_tempo_is_followed() {
        let (samples, beats) = band_in_a_room(24.0, |t| if t < 11.0 { 100.0 } else { 132.0 }, |_| 1.0, 0.004);
        let mut pulse = BeatPulse::new();
        let frames = play(&mut pulse, &samples);
        let before = frames.iter().rev().find(|(at, _)| *at < 11.0).unwrap().1;
        assert!(before.locked && (before.tempo - 100.0).abs() < 1.5, "before the change: {} BPM", before.tempo);
        let after = frames.last().unwrap().1;
        assert!(after.locked && (after.tempo - 132.0).abs() < 2.0, "after the change: {} BPM", after.tempo);
        let on_beats = landings(&frames, &beats, 19.0, |f| f.on_beat);
        assert!(on_beats.len().abs_diff(beats_from(&beats, 19.0)) <= 1, "{} beats after the change", on_beats.len());
        assert_on_the_beat(&on_beats, "after the change");
    }

    #[test]
    fn room_noise_alone_fires_no_beat() {
        // Hiss and a hum for six seconds, then knocks and clatter at random
        // times as well.
        let len = (16.0 * RATE) as usize;
        let mut noise = Noise(0x9e37_79b9);
        let mut samples: Vec<f32> = (0..len)
            .map(|i| 0.02 * noise.next() + 0.01 * (std::f64::consts::TAU * 50.0 * i as f64 / RATE).sin() as f32)
            .collect();
        let mut at = (6.0 * RATE) as usize;
        while at < len {
            for i in 0..(0.04 * RATE) as usize {
                if at + i < len {
                    samples[at + i] += 0.3 * (-80.0 * i as f32 / RATE as f32).exp() * noise.next();
                }
            }
            at += (0.15 * RATE) as usize + (noise.next().abs() * 0.9 * RATE as f32) as usize;
        }
        let mut pulse = BeatPulse::new();
        let frames = play(&mut pulse, &samples);
        for (at, f) in &frames {
            assert!(!f.on_beat && !f.on_bar && !f.locked, "a beat in noise at {at:.2} s: {f:?}");
            assert!(f.beat_pulse == 0.0 && f.tempo == 0.0 && f.confidence == 0.0, "at {at:.2} s: {f:?}");
        }
        // The hiss alone sits near the floor; a knock reads.
        for band in 0..BANDS {
            let (_, high) = range(&frames, 2.0, 6.0, |f| f.bands[band]);
            assert!(high < 0.35, "band {band} in hiss reads {high}");
        }
        let (_, high) = range(&frames, 6.0, 16.0, |f| f.onset);
        assert!(high > 0.5, "knocks read as onsets: {high}");
    }

    #[test]
    fn a_quiet_start_then_loud_keeps_the_beat_and_the_levels() {
        // 40 dB quieter for the first twelve seconds, then up over half a
        // second.
        let gain = |t: f64| if t < 12.0 { 0.01 } else { (0.01 + (t - 12.0) as f32 * 2.0).min(1.0) };
        let (samples, beats) = band_in_a_room(22.0, |_| 124.0, gain, 0.0002);
        let mut pulse = BeatPulse::new();
        let frames = play(&mut pulse, &samples);
        let locked_at = frames.iter().find(|(_, f)| f.locked).map(|(at, _)| *at);
        assert!(locked_at.is_some_and(|at| at < 9.0), "quiet: locked at {locked_at:?}");
        assert!(frames.iter().filter(|(at, _)| *at > 9.0).all(|(_, f)| f.locked), "the lock was lost through the rise");
        for (from, to, part) in [(8.0, 12.0, "quiet"), (13.5, 22.0, "loud")] {
            let (low, high) = range(&frames, from, to, |f| f.bands[0]);
            assert!(high > 0.7 && low < 0.4, "{part}: kick level {low}..{high}");
        }
        assert_on_the_beat(&landings(&frames, &beats, 13.0, |f| f.on_beat), "after the rise");
    }

    #[test]
    fn a_phase_nudge_moves_the_beats_and_never_runs_back() {
        let (samples, beats) = band_in_a_room(20.0, |_| 120.0, |_| 1.0, 0.004);
        let plain = play(&mut BeatPulse::new(), &samples);
        // A quarter beat earlier from the start: the same grid, ahead.
        let mut early = BeatPulse::new();
        early.set_phase_nudge(0.25);
        assert_eq!(early.phase_nudge(), 0.25);
        let ahead = play(&mut early, &samples);
        for ((at, a), (_, b)) in plain.iter().zip(&ahead).filter(|((at, _), _)| *at > 10.0) {
            assert!((b.beat_pos - a.beat_pos - 0.25).abs() < 1e-6, "at {at:.2} s: {} against {}", b.beat_pos, a.beat_pos);
        }
        let on_beats = landings(&ahead, &beats, 10.0, |f| f.on_beat);
        assert!(on_beats.len().abs_diff(beats_from(&beats, 10.0)) <= 1, "{} beats nudged", on_beats.len());
        for (error, _) in &on_beats {
            assert!((-0.145..=-0.09).contains(error), "a nudged beat {:.0} ms off", error * 1000.0);
        }
        // Turned from well ahead to well behind mid-song (and past the
        // range): held still, then behind, never back.
        let mut turned = BeatPulse::new();
        turned.set_phase_nudge(0.4);
        let mut frames = Vec::new();
        let mut heard = 0usize;
        for chunk in samples.chunks(FRAME) {
            if heard as f64 / RATE >= 12.0 {
                turned.set_phase_nudge(-0.9);
            }
            turned.push(chunk, RATE);
            heard += chunk.len();
            frames.push((heard as f64 / RATE, turned.frame(chunk.len() as f64 / RATE)));
        }
        assert_eq!(turned.phase_nudge(), -0.5, "the nudge stays within half a beat");
        assert!(frames.windows(2).all(|w| w[1].1.beat_pos >= w[0].1.beat_pos && w[1].1.bar_pos >= w[0].1.bar_pos), "a position ran back");
        for ((at, a), (_, b)) in plain.iter().zip(&frames).filter(|((at, _), _)| *at > 14.0) {
            assert!((b.beat_pos - a.beat_pos + 0.5).abs() < 1e-6, "at {at:.2} s: {} against {}", b.beat_pos, a.beat_pos);
        }
    }

    #[test]
    fn a_tap_on_the_one_moves_the_bar_there_and_holds() {
        // The vote finds the band's loudest kick; the person taps the
        // third beat of the bar as the one instead.
        let (samples, beats) = band_in_a_room(24.0, |_| 120.0, |_| 1.0, 0.004);
        let nearest = |at: f64| (0..beats.len()).min_by(|&a, &b| (beats[a].0 - at).abs().total_cmp(&(beats[b].0 - at).abs())).unwrap();
        let mut pulse = BeatPulse::new();
        assert_eq!(pulse.tap_downbeat(), None, "no beat to tap yet");
        let mut frames = Vec::new();
        let mut heard = 0usize;
        let mut tapped_at = None;
        for chunk in samples.chunks(FRAME) {
            pulse.push(chunk, RATE);
            heard += chunk.len();
            let at = heard as f64 / RATE;
            let frame = pulse.frame(chunk.len() as f64 / RATE);
            if tapped_at.is_none() && at > 10.0 && frame.on_beat && nearest(at) % 4 == 2 {
                let off = pulse.tap_downbeat().expect("a beat is followed");
                assert!(off.abs() < 0.1, "the tap fell {off} beats off");
                tapped_at = Some(at);
            }
            frames.push((at, frame));
        }
        let tapped_at = tapped_at.expect("a third beat after ten seconds");
        let last = beats.last().unwrap().0 + 0.1;
        let bars: Vec<usize> = frames.iter().filter(|(at, f)| *at > tapped_at + 0.1 && *at <= last && f.on_bar).map(|(at, _)| nearest(*at)).collect();
        assert!(bars.len() >= 4, "{} bars after the tap", bars.len());
        assert!(bars.iter().all(|i| i % 4 == 2), "bars on beats {bars:?}, not on the tapped place");
        assert!(frames.windows(2).all(|w| w[1].1.bar_pos >= w[0].1.bar_pos), "a bar ran back");
        // A new source forgets the tap: the vote has the one again.
        pulse.reset();
        assert!(!pulse.tapped);
    }
}
