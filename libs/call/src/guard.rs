//! Video sender adaptation on the capture clock. Charge every encoded access unit;
//! decide whether to skip the next capture before doing any encoding work.

use std::{collections::VecDeque, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Encoder budget at which this rung can be selected on startup or a climb.
    pub min_ask_bps: u64,
}

impl Rung {
    const fn new(width: u32, height: u32, fps: u32, min_ask_bps: u64) -> Self {
        Self { width, height, fps, min_ask_bps }
    }
}

impl fmt::Display for Rung {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}@{}", self.width, self.height, self.fps)
    }
}

/// Decrease resolution first, then frame rate.
pub const LADDER: [Rung; 7] = [
    Rung::new(1280, 720, 30, 900_000),
    Rung::new(960, 540, 30, 550_000),
    Rung::new(640, 360, 30, 280_000),
    Rung::new(320, 180, 30, 120_000),
    Rung::new(320, 180, 15, 70_000),
    Rung::new(320, 180, 8, 40_000),
    Rung::new(320, 180, 4, 0),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Skipping,
    Overshoot,
    Headroom,
    LowAsk,
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Skipping => "skipping",
            Self::Overshoot => "overshoot",
            Self::Headroom => "headroom",
            Self::LowAsk => "low ask",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub from: Rung,
    pub to: Rung,
    pub why: Why,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    Encode,
    Skip,
}

const SKIP_DELAY_US: u64 = 100_000;
const DOWN_WAIT_US: u64 = 2_000_000;
const STARTUP_UP_WAIT_US: u64 = 2_000_000;
const UP_WAIT_US: u64 = 10_000_000;
const MAX_UP_WAIT_US: u64 = 60_000_000;

/// Leave 80 kbit/s for voice and ten percent of the remainder as headroom.
pub fn video_ask(estimate_bps: u64) -> u64 {
    ((u128::from(estimate_bps.saturating_sub(80_000)) * 9 / 10) as u64).max(1)
}

#[derive(Default)]
struct Window {
    start_us: u64,
    // Both totals use bit-microseconds so ask changes preserve fractional bits.
    budget: u128,
    output: u128,
    due: u64,
    skipped: u64,
    last_skip_us: Option<u64>,
    below_min_since: Option<u64>,
}

/// Cumulative totals through this capture or encoder output. The ask stays in
/// force until the next sample, including across a rolling-window boundary.
struct Sample {
    at: u64,
    ask: u64,
    budget: u128,
    output: u128,
}

pub struct Guard {
    top: usize,
    rung: usize,
    ask_bps: u64,
    video_estimate_bps: f64,
    last_estimate_bps: Option<u64>,
    last_tick_us: u64,
    debt_bit_us: u128,
    /// The part of the debt that the first picture on a rung (the key frame a step or the start
    /// forces) still holds. It drains first, as that picture went out first.
    grace_bit_us: u128,
    last_us: Option<u64>,
    // Scheduled time times fps, so fractional microseconds carry between captures.
    next_due: Option<u128>,
    window: Window,
    history: VecDeque<Sample>,
    first_picture: bool,
    up_wait_us: u64,
    last_step_up: bool,
    steps: VecDeque<Step>,
}

impl Guard {
    /// Highest-quality rung whose minimum the encoder budget meets.
    pub fn rung_for(ask_bps: u64) -> usize {
        LADDER.iter().position(|r| ask_bps >= r.min_ask_bps).unwrap()
    }

    /// Start on this rung and allow adaptation across the whole ladder.
    pub fn new(start: usize) -> Self {
        Self::with_top(start, 0)
    }

    /// Apply a capture-size limit separately from the initial bandwidth choice.
    pub fn with_top(start: usize, top: usize) -> Self {
        let top = top.min(LADDER.len() - 1);
        Self {
            top,
            rung: start.clamp(top, LADDER.len() - 1),
            ask_bps: video_ask(crate::session::START_BPS),
            video_estimate_bps: 0.0,
            last_estimate_bps: None,
            last_tick_us: 0,
            debt_bit_us: 0,
            grace_bit_us: 0,
            last_us: None,
            next_due: None,
            window: Window::default(),
            history: VecDeque::new(),
            first_picture: true,
            up_wait_us: STARTUP_UP_WAIT_US,
            last_step_up: false,
            steps: VecDeque::new(),
        }
    }

    fn advance(&mut self, now: u64) -> u64 {
        let Some(last) = self.last_us else {
            self.last_us = Some(now);
            self.window.start_us = now;
            return now;
        };
        let budget = u128::from(self.ask_bps) * u128::from(now.saturating_sub(last));
        self.last_us = Some(last.max(now));
        self.window.budget += budget;
        self.debt_bit_us = self.debt_bit_us.saturating_sub(budget);
        self.grace_bit_us = self.grace_bit_us.saturating_sub(budget).min(self.debt_bit_us);
        last.max(now)
    }

    fn sample(&mut self, now: u64) {
        if self.rung == self.top {
            return;
        }
        let cutoff = now.saturating_sub(self.up_wait_us);
        while self.history.get(1).is_some_and(|s| s.at <= cutoff) {
            self.history.pop_front();
        }
        let sample = Sample { at: now, ask: self.ask_bps, budget: self.window.budget, output: self.window.output };
        if let Some(last) = self.history.back_mut().filter(|s| s.at == now) {
            *last = sample;
        } else {
            self.history.push_back(sample);
        }
    }

    fn can_step_up(&self, now: u64) -> bool {
        let Some(cutoff) = now.checked_sub(self.up_wait_us) else { return false };
        if self.rung == self.top || self.ask_bps < LADDER[self.rung - 1].min_ask_bps
            || self.window.start_us > cutoff || self.window.last_skip_us.is_some_and(|at| at > cutoff) {
            return false;
        }
        let Some(base) = self.history.front().filter(|s| s.at <= cutoff) else { return false };
        let budget = self.window.budget - base.budget - u128::from(cutoff - base.at) * u128::from(base.ask);
        self.window.output - base.output <= budget.saturating_add(budget / 10)
    }

    fn adapt(&mut self, now: u64) {
        let age = now.saturating_sub(self.window.start_us);
        if self.last_step_up && age >= UP_WAIT_US {
            self.up_wait_us = self.up_wait_us.min(UP_WAIT_US);
            self.last_step_up = false;
        }
        self.sample(now);
        let low = self.ask_bps < self.rung().min_ask_bps * 4 / 5;
        let low_for = if low {
            now.saturating_sub(*self.window.below_min_since.get_or_insert(now))
        } else {
            self.window.below_min_since = None;
            0
        };
        if age < DOWN_WAIT_US || self.window.due == 0 {
            return;
        }

        let (to, why) = if self.rung + 1 < LADDER.len() && self.window.skipped > self.window.due / 4 {
            (self.rung + 1, Why::Skipping)
        } else if self.rung + 1 < LADDER.len() && self.window.output > self.window.budget.saturating_add(self.window.budget / 2) {
            (self.rung + 1, Why::Overshoot)
        } else if low_for >= DOWN_WAIT_US {
            (self.rung + 1, Why::LowAsk)
        } else if self.can_step_up(now) {
            (self.rung - 1, Why::Headroom)
        } else {
            return;
        };

        let up = to < self.rung;
        if !up && self.up_wait_us == STARTUP_UP_WAIT_US {
            self.up_wait_us = UP_WAIT_US;
        } else if !up && self.last_step_up {
            self.up_wait_us = (2 * self.up_wait_us).min(MAX_UP_WAIT_US);
        }
        self.steps.push_back(Step { from: self.rung(), to: LADDER[to], why });
        self.rung = to;
        self.last_step_up = up;
        self.first_picture = true;
        let below_min_since = (self.ask_bps < self.rung().min_ask_bps * 4 / 5).then_some(now);
        self.window = Window { start_us: now, below_min_since, ..Default::default() };
        self.history.clear();
        self.sample(now);
        self.next_due = None;
    }

    /// Call once per capture. An unknown loss level holds video growth while the path seeds. Times are in µs.
    pub fn tick(&mut self, now_us: u64, estimate_bps: u64, pacer_delay_us: u64, loss_level: Option<f64>) -> Tick {
        let now_us = self.advance(now_us);
        // Encoder output can advance the debt clock between captures.
        let elapsed_us = now_us - self.last_tick_us;
        self.last_tick_us = now_us;
        let previous = self.last_estimate_bps.replace(estimate_bps);
        let video_bps = if previous.is_none() || loss_level.is_some_and(|level| level <= 0.20) {
            self.video_estimate_bps = estimate_bps as f64;
            estimate_bps
        } else {
            if loss_level.is_some_and(|level| level >= 0.30) {
                let floor = crate::session::START_BPS as f64;
                if self.video_estimate_bps > floor {
                    self.video_estimate_bps = (self.video_estimate_bps * 0.92_f64.powf(elapsed_us as f64 / 1_000_000.0)).max(floor);
                }
            } else if let (Some(level), Some(last)) = (loss_level, previous.filter(|&last| last != 0 && estimate_bps > last)) {
                let taper = ((0.30 - level) / 0.10).clamp(0.0, 1.0);
                self.video_estimate_bps *= (estimate_bps as f64 / last as f64).powf(taper);
            }
            self.video_estimate_bps = self.video_estimate_bps.min(estimate_bps as f64);
            self.video_estimate_bps as u64
        };
        self.ask_bps = video_ask(video_bps);
        self.adapt(now_us);
        let fps = u128::from(self.rung().fps);
        let now = u128::from(now_us) * fps;
        let due = self.next_due.unwrap_or(now);
        if u128::from(now_us) < due / fps {
            return Tick::Skip;
        }
        // Keep the cadence through small capture jitter; after a missed period start
        // at this capture, so a stalled camera cannot accumulate a burst of due frames.
        let next = due + 1_000_000;
        self.next_due = Some(if next <= now { now + 1_000_000 } else { next });
        self.window.due += 1;
        let limit = u128::from(self.ask_bps) * u128::from(SKIP_DELAY_US);
        let late = pacer_delay_us > SKIP_DELAY_US;
        if self.debt_bit_us > limit || late {
            // The key frame a step forced still skips pictures while it drains, but it is no
            // overload: neither its debt nor the pacer delay while it lasts counts toward a down
            // step, or each up step would undo itself. Any skip still holds back an up step, so
            // an encoder that overshoots can't climb behind its first pictures.
            let grace = self.grace_bit_us > 0;
            if self.debt_bit_us - self.grace_bit_us > limit || (late && !grace) {
                self.window.skipped += 1;
            }
            self.window.last_skip_us = Some(now_us);
            return Tick::Skip;
        }
        Tick::Encode
    }

    /// Charge the complete AU, including key frames. The first AU on a new rung (the key frame
    /// the step forced) is excluded from its output-rate measurement, and its debt from the
    /// overload that steps down; a key frame the receiver asks for counts like any picture.
    pub fn encoded(&mut self, now_us: u64, bytes: usize, _key: bool) {
        let now_us = self.advance(now_us);
        let cost = bytes as u128 * 8_000_000;
        self.debt_bit_us = self.debt_bit_us.saturating_add(cost);
        if std::mem::take(&mut self.first_picture) {
            self.grace_bit_us = self.grace_bit_us.saturating_add(cost);
        } else {
            self.window.output = self.window.output.saturating_add(cost);
        }
        self.sample(now_us);
    }

    pub fn ask_bps(&self) -> u64 {
        self.ask_bps
    }

    pub fn rung(&self) -> Rung {
        LADDER[self.rung]
    }

    pub fn take_step(&mut self) -> Option<Step> {
        self.steps.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESTIMATE: u64 = 880_000;
    const ASK: u64 = 720_000;

    /// Arrival time of a 30 fps capture on the virtual microsecond clock.
    fn capture(n: u64) -> u64 {
        n * 1_000_000 / 30
    }

    #[test]
    fn learned_loss_tapers_growth_of_the_video_estimate() {
        let start = crate::session::START_BPS;
        for (level, factor) in [(0.20, 2.0), (0.25, 2.0_f64.sqrt()), (0.30, 1.0)] {
            let mut guard = Guard::new(Guard::rung_for(video_ask(start)));
            guard.tick(0, start, 0, Some(level));
            guard.tick(capture(1), 2 * start, 0, Some(level));
            let expected = start as f64 * factor;
            let video = guard.video_estimate_bps;
            let ask = guard.ask_bps();
            eprintln!("taper level {level}, estimate {start}->{} bit/s, video {video:.6} bit/s, ask {ask} bit/s", 2 * start);
            assert!((video - expected).abs() <= 1e-9, "level {level} should multiply video by {factor}, expected {expected}, got {video}");
            assert_eq!(ask, video_ask(expected as u64), "level {level} should budget video from {expected} bit/s, got {ask}");
            guard.tick(capture(2), start / 2, 0, Some(level));
            assert_eq!(guard.ask_bps(), video_ask(start / 2), "level {level} must respect the fall to {} bit/s, got ask {}", start / 2, guard.ask_bps());
        }

        let mut guard = Guard::new(Guard::rung_for(video_ask(ESTIMATE)));
        for (n, estimate, expected) in [(0, ESTIMATE, ESTIMATE), (1, 2 * ESTIMATE, ESTIMATE),
            (2, ESTIMATE, ESTIMATE), (3, ESTIMATE / 2, ESTIMATE / 2)] {
            guard.tick(capture(n), estimate, 0, None);
            assert_eq!(guard.video_estimate_bps, expected as f64, "unseeded capture {n}, path {estimate} bit/s, expected video {expected} bit/s");
            assert_eq!(guard.ask_bps(), video_ask(expected), "unseeded capture {n} should budget from {expected} bit/s, got ask {}", guard.ask_bps());
        }

        let mut guard = Guard::new(Guard::rung_for(video_ask(2 * start)));
        let base = (2 * start) as f64;
        guard.tick(0, 2 * start, 0, Some(0.35));
        guard.encoded(400_000, 100, true);
        for (at, reading, expected) in [
            (500_000, Some(0.35), base * 0.92_f64.sqrt()),
            (2_000_000, Some(0.35), base * 0.92_f64.powi(2)),
            (3_000_000, Some(0.29), base * 0.92_f64.powi(2)),
            (4_000_000, None, base * 0.92_f64.powi(2)),
            (12_000_000, Some(0.30), start as f64),
            (13_000_000, Some(0.35), start as f64),
        ] {
            guard.tick(at, 2 * start, 0, reading);
            assert!((guard.video_estimate_bps - expected).abs() < 1e-8,
                "capture at {at} µs with reading {reading:?} should give {expected} bit/s, got {}", guard.video_estimate_bps);
        }
        guard.tick(14_000_000, start / 2, 0, Some(0.35));
        guard.tick(15_000_000, 2 * start, 0, Some(0.35));
        assert_eq!(guard.video_estimate_bps, (start / 2) as f64, "heavy loss should preserve the lower path cut to {} bit/s", start / 2);
    }

    #[test]
    fn fifty_percent_random_loss_holds_the_video_ask_and_rung() {
        let start = crate::session::START_BPS;
        let ask = video_ask(start);
        let rung = Guard::rung_for(ask);
        let mut guard = Guard::new(rung);
        let (mut low, mut high) = (ask, ask);
        for n in 0..=300 {
            let now = capture(n);
            let estimate = start + start * now / 1_000_000;
            if guard.tick(now, estimate, 0, Some(0.50)) == Tick::Encode {
                guard.encoded(now, (guard.ask_bps() / 30 / 8) as usize, n == 0);
            }
            low = low.min(guard.ask_bps());
            high = high.max(guard.ask_bps());
            assert_eq!(guard.rung(), LADDER[rung], "50% loss changed rung at {now} µs with estimate {estimate} and ask {}", guard.ask_bps());
        }
        eprintln!("50% loss over 10 s: estimate {start}->{} bit/s, ask {low}..{high} bit/s, rung {}", guard.last_estimate_bps.unwrap(), guard.rung());
        assert!(10 * low >= 9 * ask && 10 * high <= 11 * ask, "50% loss should hold the {ask} bit/s ask over 10 s, saw {low}..{high}");
    }

    #[test]
    fn skipping_limits_triple_sized_pictures_before_the_first_down_step() {
        let mut guard = Guard::new(0);
        let bytes = (3 * ASK / 30 / 8) as usize;
        let mut encoded = 0;
        for n in 0..60 {
            let now = capture(n);
            if guard.tick(now, ESTIMATE, 0, Some(0.0)) == Tick::Encode {
                guard.encoded(now, bytes, n == 0);
                encoded += 1;
            }
            assert_eq!(guard.take_step(), None, "must observe two seconds before stepping down");
        }
        let sent_bps = encoded * bytes as u64 * 8 / 2;
        assert_eq!(guard.ask_bps(), ASK, "encoder ask must reserve voice and leave 10% headroom");
        assert!(sent_bps.abs_diff(ASK) <= bytes as u64 * 8, "3x source sent {sent_bps} bit/s for an ask of {ASK}");
        guard.tick(capture(60), ESTIMATE, 0, Some(0.0));
        let step = guard.take_step().expect("sustained skipping must step down at two seconds");
        assert_eq!(step, Step { from: LADDER[0], to: LADDER[1], why: Why::Skipping }, "first down step and reason");
        assert_eq!(guard.take_step(), None, "each down step is consumed once");
        eprintln!("3x encoder: {encoded}/60 encoded, {sent_bps} bit/s for {ASK} bit/s; step at 2 s");
    }

    #[test]
    fn decimation_keeps_its_phase_and_a_delayed_capture_cannot_build_up_encodes() {
        let mut guard = Guard::with_top(4, 4);
        let mut encoded = 0;
        for n in 0..120 {
            // Alternate a few ms of jitter on captures that are due. The nominal
            // 15 fps deadline must keep its phase through the late arrivals.
            let now = capture(n) + if n % 4 == 0 && n != 0 { 3_000 } else { 0 };
            let tick = guard.tick(now, ESTIMATE, 0, Some(0.0));
            assert_eq!(tick == Tick::Encode, n % 2 == 0, "15 fps schedule drifted at 30 fps capture {n}");
            if tick == Tick::Encode {
                guard.encoded(now, 100, n == 0);
                encoded += 1;
            }
        }
        assert_eq!(encoded, 60, "15 fps must encode half of 120 captures");
        assert_eq!(guard.rung(), LADDER[4], "fps decimation must not count as congestion skips");
        assert_eq!(guard.tick(8_000_000, 0, SKIP_DELAY_US + 1, Some(0.0)), Tick::Skip, "pacer delay above 100 ms must skip even after a long idle");
        assert_eq!(guard.ask_bps(), 1, "the video ask must remain positive below the voice reserve");
        assert_eq!(guard.tick(8_000_001, ESTIMATE, 0, Some(0.0)), Tick::Skip, "missed periods cannot be replayed back to back");
        assert_eq!(guard.tick(8_066_666, ESTIMATE, SKIP_DELAY_US, Some(0.0)), Tick::Encode, "100 ms of pacer delay still allows a due capture");
    }

    #[test]
    fn compares_encoded_bits_with_the_budget_while_the_ask_changes() {
        let mut guard = Guard::new(0);
        assert_eq!(guard.tick(0, ESTIMATE, 0, Some(0.0)), Tick::Encode, "first capture is due");
        guard.encoded(0, 300_000, true);
        // The first key is 2.4 Mbit against a 1.44 Mbit two-second budget.
        // A much larger new ask allows another capture with its remaining debt.
        assert_eq!(guard.tick(2_000_000, 16_080_000, 0, Some(0.0)), Tick::Encode, "a due capture within the new debt allowance should encode");
        assert_eq!(guard.take_step(), None, "the first key alone must not trigger output overshoot");
        guard.encoded(2_000_000, 900_000, false);
        assert_eq!(guard.tick(3_000_000, ESTIMATE, 0, Some(0.0)), Tick::Encode, "the previous high ask must drain the encoder debt");
        assert_eq!(guard.take_step(), None, "a falling ask must not shrink the budget retroactively");
        guard.encoded(3_000_000, 3_000_000, true);
        guard.tick(capture(91), ESTIMATE, 0, Some(0.0));
        assert_eq!(guard.take_step().map(|s| s.why), Some(Why::Overshoot), "large later key must trigger the output criterion");
    }

    #[test]
    fn failed_up_steps_back_off_and_a_held_up_step_resets_the_wait() {
        let mut guard = Guard::new(0);
        let mut changes = Vec::new();
        let mut last_change = 0;
        let mut ups = 0;
        for n in 0..30 * 240 {
            let now = capture(n);
            let fail_top = guard.rung() == LADDER[0] && (ups < 5 || now - last_change >= 11_000_000);
            let decision = guard.tick(now, 1_080_000, if fail_top { SKIP_DELAY_US + 1 } else { 0 }, Some(0.0));
            if let Some(step) = guard.take_step() {
                assert_eq!(guard.take_step(), None, "backoff transitions must be reported exactly once");
                if step.why == Why::Headroom {
                    ups += 1;
                    changes.push(now - last_change);
                }
                last_change = now;
            }
            if decision == Tick::Encode {
                guard.encoded(now, 100, now == last_change);
            }
            if ups == 6 {
                break;
            }
        }
        let expected = [10, 20, 40, 60, 60, 10];
        assert_eq!(changes.len(), expected.len(), "all failed probes and the held probe must lead to recovery: {changes:?}");
        for (wait, seconds) in changes.iter().zip(expected) {
            assert!((seconds * 1_000_000..=seconds * 1_000_000 + 34_000).contains(wait), "expected {seconds} s probe wait, got {wait} µs");
        }
        eprintln!("waits before upward steps: {changes:?} µs");
    }

    #[test]
    fn an_encoder_meeting_its_rising_ask_climbs_to_the_top() {
        let start = Guard::rung_for(200_000);
        assert_eq!(start, 3, "200 kbit/s should start at 320x180@30");
        let mut guard = Guard::new(start);
        let mut steps = Vec::new();
        for n in 0..=300 {
            let now = capture(n);
            let ask = 200_000 + 1_300_000 * now.min(6_000_000) / 6_000_000;
            let estimate = 80_000 + (10 * ask).div_ceil(9);
            assert_eq!(guard.tick(now, estimate, 0, Some(0.0)), Tick::Encode, "on-budget capture {n} should encode");
            assert_eq!(guard.ask_bps(), ask, "model and encoder must agree on the ask");
            if let Some(step) = guard.take_step() {
                assert_eq!(step.why, Why::Headroom, "a sender meeting its rising ask must only climb");
                steps.push((now, step.to));
            }
            assert!(guard.take_step().is_none(), "each ramp step must be reported once");
            guard.encoded(now, (ask as f64 / 30.0 / 8.0).round() as usize, n == 0);
        }
        assert_eq!(steps.iter().map(|s| s.1).collect::<Vec<_>>(), [LADDER[2], LADDER[1], LADDER[0]], "must climb all three rungs by 10 s");
        eprintln!("on-budget ramp: {steps:?}");
    }

    #[test]
    fn a_low_ask_must_persist_below_the_down_threshold() {
        for (i, rung) in LADDER.iter().enumerate() {
            assert_eq!(Guard::rung_for(rung.min_ask_bps), i, "minimum for {rung} includes equality");
            if rung.min_ask_bps != 0 {
                assert_eq!(Guard::rung_for(rung.min_ask_bps - 1), i + 1, "just below the minimum for {rung}");
            }
        }
        let mut guard = Guard::new(2);
        // An ask of exactly 80% of 280 kbit/s (224 kbit/s) interrupts a low interval just
        // before it matures.
        for (now, estimate) in [(0, 300_000), (1_999_999, 328_889), (2_000_000, 300_000), (3_999_999, 300_000)] {
            guard.tick(now, estimate, 0, Some(0.0));
            assert!(guard.take_step().is_none(), "short low-ask interval triggered a step at {now}");
        }
        guard.tick(4_000_000, 300_000, 0, Some(0.0));
        assert_eq!(guard.take_step(), Some(Step { from: LADDER[2], to: LADDER[3], why: Why::LowAsk }), "two continuous seconds below 80% must step down");
    }

    #[test]
    fn an_up_step_key_frame_alone_does_not_step_back_down() {
        // An encoder that fills half its ask climbs from 320x180@8 (each climb 2 s after the
        // skips behind the last key frame end). The key frame at each up step
        // is 600 kbit, 0.8 s of the ask: it skips the pictures behind it while it drains (and
        // holds the pacer for 150 ms), which would be over 25% of the 2 s window. Charged as
        // overload it would step straight back down; a picture that size the encoder makes on
        // its own does step down.
        let run = |big_on_its_own: bool| {
            let mut guard = Guard::new(5);
            let (mut steps, mut since_step, mut big_at, mut skipped_behind_key) = (Vec::new(), 0u32, None, 0);
            for n in 0..30 * 12 {
                let now = capture(n);
                // the big picture holds the pacer for its first 150 ms
                let delay = if big_at.is_some_and(|at: u64| now - at < 150_000) { SKIP_DELAY_US + 50_000 } else { 0 };
                let tick = guard.tick(now, ESTIMATE, delay, Some(0.0));
                if let Some(step) = guard.take_step() {
                    steps.push((now, step));
                    since_step = 0;
                }
                if tick == Tick::Skip && steps.last().is_some_and(|&(at, _): &(u64, Step)| now - at < 800_000) {
                    skipped_behind_key += 1;
                }
                if tick == Tick::Encode {
                    let regular = (ASK / u64::from(guard.rung().fps) / 2 / 8) as usize;
                    let stepped = !steps.is_empty() && since_step == 0;
                    let big = if big_on_its_own { steps.len() == 1 && since_step == 15 } else { stepped };
                    if big {
                        big_at = Some(now);
                    }
                    guard.encoded(now, if big { 75_000 } else { regular }, stepped);
                    since_step += 1;
                }
            }
            (steps, skipped_behind_key)
        };
        let (steps, skipped) = run(false);
        eprintln!("big key at each up step: {} skipped behind keys, steps {:?}", skipped, steps.iter().map(|(at, s)| (*at, s.to)).collect::<Vec<_>>());
        assert!(skipped > 0, "the key frames still skip the pictures behind them while they drain");
        assert!(steps.iter().all(|(_, s)| s.why == Why::Headroom), "an up step's key frame alone must not step back down: {steps:?}");
        assert_eq!(steps.iter().map(|(_, s)| s.to).collect::<Vec<_>>(), [LADDER[4], LADDER[3], LADDER[2], LADDER[1]], "the climb goes on through its key frames");
        let (steps, _) = run(true);
        assert_eq!(steps.get(1).map(|(_, s)| (s.to, s.why)), Some((LADDER[5], Why::Skipping)), "the same picture made on its own is overload: {steps:?}");
    }

    #[test]
    fn floor_keeps_sending_as_oversized_pictures_drain() {
        let mut guard = Guard::new(LADDER.len() - 1);
        let bytes = (100 * ASK / 30 / 8) as usize;
        let mut last = 0;
        let mut longest = 0;
        let mut pictures = 0;
        for n in 0..=30 * 80 {
            let now = capture(n);
            if guard.tick(now, ESTIMATE, 0, Some(0.0)) == Tick::Encode {
                longest = longest.max(now - last);
                last = now;
                guard.encoded(now, bytes, pictures == 0);
                pictures += 1;
            }
        }
        eprintln!("floor, 100x the 30 fps frame budget: {pictures} pictures in 80 s, longest gap {longest} µs");
        assert!(pictures > 1 && guard.take_step().is_none(), "floor must continue producing pictures without stepping off the ladder");
        assert!(longest.max(80_000_000 - last) <= 5_000_000, "floor picture gap {longest} µs for 100x the 30 fps budget exceeded five seconds");
    }
}
