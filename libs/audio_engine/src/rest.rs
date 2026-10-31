//! A schedule that makes no sound rests: the device callback fills silence
//! for it without running its graph, and the next thing that can start a
//! sound wakes it in the very buffer it arrives in.
//!
//! The decision is measured, never assumed. A schedule rests only when
//! (1) the sum of what it sends to the outputs has stayed under
//! [`SILENCE`] for a whole hold (long enough for the longest echo it holds:
//! a reverb tail has decayed below audibility by then, an echo has come
//! round), and (2) no source is running: no transport playing, no deck
//! playing, no live input enabled, no generator without an input.
//! It wakes when a control moved, or a feed (notes, pad hits, tempo, FX
//! automation) has something waiting; a command that installs, loads or
//! swaps something is taken before the pass, so it is seen in the same buffer.

/// Peak under which an output is silent: about -100 dBFS, below a 16-bit step.
pub const SILENCE: f32 = 1e-5;

/// Seconds of silence before a schedule rests, with no echo in it.
pub const HOLD_SECONDS: f64 = 1.0;
/// With a delay line or a shader FX (a delay up to
/// `score_player::fx::MAX_DELAY_SECS`, 4 s) in the graph: silence between two
/// echoes can be that long.
pub const ECHO_HOLD_SECONDS: f64 = 5.0;

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Resting is on unless a host turns it off (to compare against an engine
/// that always runs its graph).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// One schedule's resting state, made when the schedule is prepared.
pub struct Rest {
    /// Frames the outputs have been silent for, in a row.
    quiet: u64,
    asleep: bool,
    /// The control bank as it was when the schedule went to rest.
    controls: Box<[u32]>,
}

impl Rest {
    pub fn new(controls: usize) -> Self {
        Self { quiet: 0, asleep: false, controls: vec![0; controls].into_boxed_slice() }
    }

    pub fn asleep(&self) -> bool {
        self.asleep
    }

    pub fn wake(&mut self) {
        self.asleep = false;
        self.quiet = 0;
    }

    /// After a pass of `frames` whose outputs peaked at `peak`: whether the
    /// schedule goes to rest now. `hold` is the frames of silence it needs
    /// (computed lazily, only once a second of silence has passed) and
    /// `quiescent` says no source runs; `snapshot` fills the control values
    /// it will be woken by.
    pub fn after_pass(&mut self, peak: f32, frames: usize, hold: impl FnOnce() -> u64, quiescent: impl FnOnce() -> bool, snapshot: impl FnOnce(&mut [u32])) -> bool {
        if !ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        if !(peak < SILENCE) {
            // (a NaN is not silence either)
            self.quiet = 0;
            return false;
        }
        self.quiet = self.quiet.saturating_add(frames as u64);
        if self.quiet < hold() || !quiescent() {
            return false;
        }
        snapshot(&mut self.controls);
        self.asleep = true;
        true
    }

    /// Before a pass: whether it is skipped. `controls_moved` compares the
    /// bank with the snapshot, `feeds_waiting` looks at the feeds; either
    /// wakes the schedule.
    pub fn skip_pass(&mut self, controls_moved: impl FnOnce(&[u32]) -> bool, feeds_waiting: impl FnOnce() -> bool) -> bool {
        if !self.asleep {
            return false;
        }
        if controls_moved(&self.controls) || feeds_waiting() {
            self.wake();
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: u64 = 48_000;

    fn quiet_for(rest: &mut Rest, passes: usize, peak: f32, quiescent: bool) -> bool {
        let mut slept = false;
        for _ in 0..passes {
            slept = rest.after_pass(peak, 256, || HOLD, || quiescent, |snapshot| snapshot.fill(7));
        }
        slept
    }

    #[test]
    fn silent_and_decayed_rests() {
        let mut rest = Rest::new(4);
        assert!(!quiet_for(&mut rest, 100, 0.0, true), "a hold has not passed yet");
        assert!(!rest.asleep());
        assert!(quiet_for(&mut rest, 100, 0.0, true));
        assert!(rest.asleep());
    }

    #[test]
    fn a_tail_still_audible_keeps_running() {
        let mut rest = Rest::new(4);
        // A tail that decays 1 % per pass stays above silence for a long
        // time: it never rests while it is audible.
        let mut peak = 0.5f32;
        for _ in 0..1000 {
            assert!(!rest.after_pass(peak, 256, || HOLD, || true, |_| {}));
            peak *= 0.99;
            if peak < SILENCE { break; }
        }
        assert!(!rest.asleep());
        // Once below audibility it needs the whole hold, and an echo that
        // returns inside the hold starts the count again.
        assert!(!quiet_for(&mut rest, 150, 0.0, true));
        assert!(!rest.after_pass(0.2, 256, || HOLD, || true, |_| {}));
        assert!(!quiet_for(&mut rest, 150, 0.0, true));
        assert!(quiet_for(&mut rest, 60, 0.0, true));
    }

    #[test]
    fn a_running_source_never_rests() {
        let mut rest = Rest::new(4);
        assert!(!quiet_for(&mut rest, 1000, 0.0, false));
        assert!(!rest.asleep());
    }

    #[test]
    fn a_nan_output_is_not_silence() {
        let mut rest = Rest::new(4);
        assert!(!quiet_for(&mut rest, 1000, f32::NAN, true));
    }

    #[test]
    fn a_new_source_wakes_in_the_same_pass() {
        let mut rest = Rest::new(4);
        assert!(quiet_for(&mut rest, 400, 0.0, true));
        // Nothing changed: the pass is skipped.
        assert!(rest.skip_pass(|snapshot| snapshot.iter().any(|v| *v != 7), || false));
        assert!(rest.asleep());
        // A control moved (play): the pass runs, the count starts over.
        assert!(!rest.skip_pass(|_| true, || false));
        assert!(!rest.asleep());
        assert!(!quiet_for(&mut rest, 10, 0.0, true));
        // A feed (a note) wakes it too.
        assert!(quiet_for(&mut rest, 400, 0.0, true));
        assert!(!rest.skip_pass(|_| false, || true));
        assert!(!rest.asleep());
    }
}
