//! Sender-side rate estimation for one direction of a call path.

use std::collections::VecDeque;

const GROUP_US: u64 = 5_000;
const TREND_WINDOW: usize = 20;
const DELAY_WINDOW_US: u64 = 10_000_000;
const DELIVERY_WINDOW_US: u64 = 500_000;
const LOSS_INTERVAL_US: u64 = 300_000;
const LOSS_SEED_US: u64 = 2_000_000;
const LOSS_SEED_PACKETS: f64 = 50.0;
const LOSS_DECAY_US: f64 = 10_000_000.0;
const MIN_BPS: u64 = 16_000;

#[derive(Clone, Copy)]
struct Packet {
    send_us: u64,
    arrival_us: u64,
}

impl Packet {
    fn delay_us(self) -> i128 {
        i128::from(self.arrival_us) - i128::from(self.send_us)
    }
}

struct Trend {
    group_start_us: u64,
    group_last: Option<Packet>,
    previous_last: Option<Packet>,
    groups: u32,
    accumulated_us: f64,
    smoothed_us: f64,
    samples: VecDeque<(u64, f64)>,
    slope: f64,
    threshold: f64,
    above_since_us: Option<u64>,
    overuse: bool,
}

impl Trend {
    fn new() -> Self {
        Self {
            group_start_us: 0,
            group_last: None,
            previous_last: None,
            groups: 0,
            accumulated_us: 0.0,
            smoothed_us: 0.0,
            samples: VecDeque::with_capacity(TREND_WINDOW),
            slope: 0.0,
            threshold: 12.5,
            above_since_us: None,
            overuse: false,
        }
    }

    fn record(&mut self, packet: Packet) {
        if let Some(last) = self.group_last.replace(packet) {
            if packet.send_us.saturating_sub(self.group_start_us) <= GROUP_US {
                return;
            }
            self.finish_group(last);
        }
        self.group_start_us = packet.send_us;
    }

    fn finish_group(&mut self, last: Packet) {
        self.groups = (self.groups + 1).min(60);
        let Some(previous) = self.previous_last.replace(last) else { return };
        // The change in transit time is the arrival gap minus the send gap,
        // even when the receiver clock is behind or the packets reordered.
        self.accumulated_us += (last.delay_us() - previous.delay_us()) as f64;
        self.smoothed_us = 0.9 * self.smoothed_us + 0.1 * self.accumulated_us;
        if self.samples.len() == TREND_WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back((last.arrival_us, self.smoothed_us));
        let previous_slope = self.slope;
        self.slope = self.fit_slope();
        let modified = f64::from(self.groups) * self.slope * 4.0;
        if modified > self.threshold {
            let since = *self.above_since_us.get_or_insert(last.send_us);
            self.overuse = last.send_us.saturating_sub(since) > 10_000
                && self.slope >= previous_slope;
        } else {
            self.above_since_us = None;
            self.overuse = false;
        }

        let excess = modified.abs() - self.threshold;
        if excess <= 15.0 {
            let dt_ms = last.arrival_us.saturating_sub(previous.arrival_us) as f64 / 1_000.0;
            let k = if excess < 0.0 { 0.0087 } else { 0.039 };
            self.threshold = (self.threshold + k * excess * dt_ms).clamp(6.0, 600.0);
        }
    }

    fn fit_slope(&self) -> f64 {
        // Subtract a local origin before converting timestamps to floating point.
        let (arrival_origin, delay_origin) = self.samples[0];
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for &(arrival, delay) in &self.samples {
            let x = (i128::from(arrival) - i128::from(arrival_origin)) as f64;
            let y = delay - delay_origin;
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let n = self.samples.len() as f64;
        let variance = n * sxx - sx * sx;
        if variance > 0.0 { (n * sxy - sx * sy) / variance } else { 0.0 }
    }
}

struct LossSeed {
    since_us: u64,
    start_bps: f64,
    hold: bool,
}

#[derive(Clone, Copy, Default)]
struct LossSample {
    lost: u64,
    delivered: u64,
}

impl LossSample {
    fn packets(self) -> u128 {
        u128::from(self.lost) + u128::from(self.delivered)
    }

    fn total(self) -> f64 {
        self.packets() as f64
    }

    fn add(&mut self, other: Self) {
        self.lost = self.lost.saturating_add(other.lost);
        self.delivered = self.delivered.saturating_add(other.delivered);
    }

    fn log_likelihood_ratio(self, reference: f64, higher: f64) -> f64 {
        self.lost as f64 * (higher / reference).ln()
            + self.delivered as f64 * ((1.0 - higher) / (1.0 - reference)).ln()
    }
}

#[derive(Default)]
struct LossWatch {
    sum: f64,
    sample: LossSample,
    tainted: bool,
}

struct LossDip {
    at_us: u64,
    pre_estimate: f64,
    clear_bps: f64,
    pre_startup: bool,
    sent_bps: u64,
    cap: f64,
    confirmed: bool,
    reference: f64,
    high: LossSample,
    low: LossSample,
    sum: f64,
    skip: bool,
}

struct LossSuspect {
    sent_bps: u64,
    needed: u128,
    cleared: u128,
    // Level counts and sample time to restore if a random verdict alarms on retest.
    old_level: Option<(f64, f64, u64)>,
}

/// Cumulative loss alarms, dip outcomes and suspect retests on this estimator.
#[derive(Clone, Copy, Debug, Default)]
pub struct LossEvents {
    pub alarms: u32,
    pub random: u32,
    pub clean: u32,
    pub dirty: u32,
    pub suspects_set: u32,
    pub suspects_cleared: u32,
    /// Random verdicts whose retest restored the previous loss level.
    pub reverted: u32,
}

/// The available bit rate for all traffic on one outgoing path.
pub struct RateEstimator {
    trend: Trend,
    // Increasing transit times in send order. The front is the window minimum.
    minima: VecDeque<(u64, i128)>,
    queue_delay_us: u64,
    // Sorted by receiver time, which can go backwards in send order.
    arrivals: VecDeque<(u64, usize)>,
    delivered_bytes: u128,
    interval: LossSample,
    last_loss_decision_us: Option<u64>,
    interval_dirty: bool,
    loss_seed: Option<LossSeed>,
    watch: LossWatch,
    // Estimate at the last loss decision with a cleared watch and no open dip.
    clear_bps: f64,
    dip: Option<LossDip>,
    suspect: Option<LossSuspect>,
    loss_cap: Option<f64>,
    events: LossEvents,
    // Pool seed packets, then decay both counts together between clean samples.
    level_lost: f64,
    level_total: f64,
    level_at_us: u64,
    estimate: f64,
    startup: bool,
    last_update_us: Option<u64>,
    // Sender time and estimate just before the last delay backoff.
    last_overuse: Option<(u64, f64)>,
}

impl RateEstimator {
    /// Start at the caller's chosen bit rate, with a floor of 16 kbit/s.
    pub fn new(start_bps: u64) -> Self {
        let estimate = start_bps.max(MIN_BPS) as f64;
        Self {
            trend: Trend::new(),
            minima: VecDeque::new(),
            queue_delay_us: 0,
            arrivals: VecDeque::new(),
            delivered_bytes: 0,
            interval: LossSample::default(),
            last_loss_decision_us: None,
            interval_dirty: false,
            loss_seed: None,
            watch: LossWatch::default(),
            clear_bps: estimate,
            dip: None,
            suspect: None,
            loss_cap: None,
            events: LossEvents::default(),
            level_lost: 0.0,
            level_total: 0.0,
            level_at_us: 0,
            estimate,
            startup: true,
            last_update_us: None,
            last_overuse: None,
        }
    }

    /// Record each newly reported packet once, in send order. Send time is local
    /// and arrival time is on the receiver's clock, both in microseconds.
    pub fn on_arrival(&mut self, send_us: u64, arrival_us: u64, bytes: usize) {
        let packet = Packet { send_us, arrival_us };
        self.trend.record(packet);

        let delay = packet.delay_us();
        while self.minima.front().is_some_and(|&(time, _)| send_us.saturating_sub(time) >= DELAY_WINDOW_US) {
            self.minima.pop_front();
        }
        while self.minima.back().is_some_and(|&(_, value)| value >= delay) {
            self.minima.pop_back();
        }
        self.minima.push_back((send_us, delay));
        self.queue_delay_us = (delay - self.minima[0].1).min(i128::from(u64::MAX)) as u64;

        let latest = self.arrivals.back().map_or(arrival_us, |&(time, _)| time);
        if arrival_us < latest {
            if latest - arrival_us >= DELIVERY_WINDOW_US {
                return;
            }
            let index = self.arrivals.partition_point(|&(time, _)| time <= arrival_us);
            self.arrivals.insert(index, (arrival_us, bytes));
        } else {
            while self.arrivals.front().is_some_and(|&(time, _)| arrival_us - time >= DELIVERY_WINDOW_US) {
                self.delivered_bytes -= self.arrivals.pop_front().unwrap().1 as u128;
            }
            self.arrivals.push_back((arrival_us, bytes));
        }
        self.delivered_bytes += bytes as u128;
    }

    /// Add newly settled verdicts to the current loss decision window.
    pub fn on_settled(&mut self, lost: u64, delivered: u64) {
        self.interval.add(LossSample { lost, delivered });
    }

    fn learn_loss(&mut self, now_us: u64, sample: LossSample) {
        let elapsed = now_us.saturating_sub(self.level_at_us) as f64;
        let decay = (-elapsed / LOSS_DECAY_US).exp();
        self.level_lost = self.level_lost * decay + sample.lost as f64;
        self.level_total = self.level_total * decay + sample.total();
        self.level_at_us = now_us;
    }

    fn decide_loss(&mut self, now_us: u64, sent_bps: u64) {
        let sample = std::mem::take(&mut self.interval);
        let dirty = std::mem::take(&mut self.interval_dirty);
        if let Some(seed) = &mut self.loss_seed {
            if dirty || sample.total() == 0.0 {
                return;
            }
            self.level_lost += sample.lost as f64;
            self.level_total += sample.total();
            self.level_at_us = now_us;
            seed.hold = sample.lost as f64 / sample.total() >= 0.02;
            if (now_us.saturating_sub(seed.since_us) >= LOSS_SEED_US && self.level_total >= LOSS_SEED_PACKETS)
                || self.estimate >= 1.5 * seed.start_bps
            {
                self.loss_seed = None;
            }
        } else if let Some(dip) = self.dip.take() {
            if dirty {
                self.suspect = None;
                self.events.dirty += 1;
            } else {
                self.judge_dip(now_us, dip, sample);
            }
        } else {
            self.watch_loss(now_us, sample, dirty, sent_bps);
        }
    }

    fn watch_loss(&mut self, now_us: u64, sample: LossSample, dirty: bool, sent_bps: u64) {
        if sample.total() == 0.0 {
            // An empty dirty interval still taints an open run.
            self.watch.tainted |= dirty && self.watch.sum > 0.0;
            return;
        }
        self.watch.tainted |= dirty;
        let mut level = self.random_loss();
        // At the higher hypothesis's ceiling there is no upward contrast left.
        let mut reference = self.upper_loss_level(level).clamp(0.02, 0.95);
        let higher = (reference + 0.05_f64.max(0.5 * reference)).min(0.95);
        self.watch.sum = (self.watch.sum + sample.log_likelihood_ratio(reference, higher)).max(0.0);
        self.watch.sample.add(sample);
        if self.watch.sum == 0.0 {
            let run = std::mem::take(&mut self.watch);
            if !run.tainted {
                self.learn_loss(now_us, run.sample);
                if let Some(suspect) = &mut self.suspect {
                    if 10 * u128::from(sent_bps) >= 9 * u128::from(suspect.sent_bps) {
                        suspect.cleared += run.sample.packets();
                        if suspect.cleared >= suspect.needed {
                            self.suspect = None;
                            self.events.suspects_cleared += 1;
                        }
                    }
                }
            }
        } else if self.watch.sum >= 9.2 {
            let run = std::mem::take(&mut self.watch);
            let suspect = self.suspect.as_ref().filter(|s| 4 * u128::from(sent_bps) >= 3 * u128::from(s.sent_bps));
            let confirms_suspect = suspect.is_some();
            if let Some(saved) = suspect.and_then(|s| s.old_level) {
                (self.level_lost, self.level_total, self.level_at_us) = saved;
                level = self.random_loss();
                reference = self.upper_loss_level(level).clamp(0.02, 0.95);
                self.events.reverted += 1;
            }
            let cap = self.delivered_bps() / (1.0 - level);
            let confirmed = run.tainted || confirms_suspect || self.loss_cap.is_some_and(|last| (0.8 * last..=1.25 * last).contains(&cap));
            if confirmed {
                self.suspect = None;
            }
            self.dip = Some(LossDip { at_us: now_us, pre_estimate: self.estimate, clear_bps: self.clear_bps, pre_startup: self.startup && !confirmed,
                high: run.sample, low: LossSample::default(), reference, sum: 0.0, skip: true, sent_bps, cap, confirmed });
            self.estimate = self.estimate.min(0.85 * cap).max(MIN_BPS as f64);
            self.events.alarms += 1;
        }
    }

    fn retest_dip(&mut self, dip: &LossDip, old_level: Option<(f64, f64, u64)>) {
        self.suspect = Some(LossSuspect {
            sent_bps: dip.sent_bps.min(self.estimate_bps()), needed: (4 * dip.high.packets()).max(100), cleared: 0, old_level,
        });
        self.events.suspects_set += 1;
    }

    fn judge_dip(&mut self, now_us: u64, mut dip: LossDip, sample: LossSample) {
        // Settled verdicts in the first interval still cover packets sent before the cut.
        if dip.skip {
            dip.skip = false;
        } else {
            let high = (dip.high.lost as f64 / dip.high.total()).max(dip.reference + 0.01).min(0.95);
            dip.low.add(sample);
            dip.sum += sample.log_likelihood_ratio(dip.reference, high);
        }
        if dip.sum.abs() < 3.0 && dip.low.total() < 400.0 && now_us.saturating_sub(dip.at_us) < 5_000_000 {
            self.dip = Some(dip);
        } else if dip.sum > 0.0 {
            let saved = (self.level_lost, self.level_total, self.level_at_us);
            self.level_lost = dip.high.lost as f64 + dip.low.lost as f64;
            self.level_total = dip.high.total() + dip.low.total();
            self.level_at_us = now_us;
            self.estimate = dip.pre_estimate;
            self.startup = dip.pre_startup;
            self.retest_dip(&dip, Some(saved));
            if dip.confirmed {
                self.loss_cap = None;
            }
            self.events.random += 1;
        } else {
            if dip.low.total() > 0.0 {
                self.learn_loss(now_us, dip.low);
            }
            if dip.confirmed {
                self.startup = false;
                self.loss_cap = Some(dip.cap);
            } else {
                self.estimate = self.estimate.max(dip.pre_estimate.min(dip.clear_bps));
                self.startup = dip.pre_startup;
                self.retest_dip(&dip, None);
            }
            self.events.clean += 1;
        }
    }

    fn delivered_bps(&self) -> f64 {
        self.delivered_bytes as f64 * (8_000_000.0 / DELIVERY_WINDOW_US as f64)
    }

    /// Update after a report. Sender time and round-trip time are in microseconds.
    /// The sent rate counts all bits sent on this path in the last second.
    pub fn update(&mut self, now_us: u64, rtt_us: u64, sent_bps: u64) -> u64 {
        let previous = self.last_update_us.replace(now_us).unwrap_or(now_us);
        let dt_s = now_us.saturating_sub(previous) as f64 / 1_000_000.0;
        let loss_interval_us = LOSS_INTERVAL_US.saturating_add(rtt_us);
        let overuse = self.trend.overuse || self.queue_delay_us > 100_000;
        let recent_backoff = self.last_overuse.is_some_and(|(at, _)| now_us.saturating_sub(at) < loss_interval_us);
        let was_dipping = self.dip.is_some();
        self.interval_dirty |= overuse || recent_backoff;
        let last_decision = self.last_loss_decision_us.get_or_insert_with(|| {
            self.loss_seed = Some(LossSeed { since_us: now_us, start_bps: self.estimate, hold: false });
            now_us
        });
        if now_us.saturating_sub(*last_decision) >= loss_interval_us {
            *last_decision = now_us;
            self.decide_loss(now_us, sent_bps);
            if self.watch.sum == 0.0 && self.dip.is_none() {
                self.clear_bps = self.estimate;
            }
        }

        let mut next = self.estimate;
        if overuse {
            self.startup = false;
            if self.last_overuse.is_none_or(|(time, _)| now_us.saturating_sub(time) >= rtt_us.max(200_000)) {
                self.last_overuse = Some((now_us, self.estimate));
                next = 0.85 * self.delivered_bps();
            }
        } else if self.dip.is_none() && !was_dipping && !self.loss_seed.as_ref().is_some_and(|seed| seed.hold)
            && (self.queue_delay_us <= 10_000 || self.trend.slope >= 0.0)
        {
            let near_last_rate = self.last_overuse.is_some_and(|(_, rate)| {
                self.estimate >= rate && self.estimate <= 1.1 * rate
            }) || self.loss_cap.is_some_and(|cap| (0.9 * cap..=1.1 * cap).contains(&self.estimate));
            let growth: f64 = if self.startup && self.suspect.is_none() { 2.0 }
                else if !self.startup && near_last_rate { 1.04 } else { 1.08 };
            next *= growth.powf(dt_s);
        }

        // Limit growth to what the sender has exercised, including during startup.
        let growth_ceiling = self.estimate.max(1.5 * sent_bps as f64);
        self.estimate = next.min(growth_ceiling).max(MIN_BPS as f64);
        self.estimate_bps()
    }

    /// The current estimate in bits per second.
    pub fn estimate_bps(&self) -> u64 {
        self.estimate as u64
    }

    /// Fraction of settled packets lost in the pooled clean samples on this path.
    pub fn random_loss(&self) -> f64 {
        if self.level_total > 0.0 { self.level_lost / self.level_total } else { 0.0 }
    }

    fn upper_loss_level(&self, level: f64) -> f64 {
        let se = if self.level_total > 0.0 { (level * (1.0 - level) / self.level_total).sqrt() } else { 0.0 };
        (level + se).min(1.0)
    }

    /// Once clean packets have pooled, use the z = 2 Wilson upper bound during the
    /// seed so video can grow on evidence of low loss. After the seed, allow one
    /// standard error above the learned loss.
    pub fn video_loss_level(&self) -> Option<f64> {
        let n = self.level_total;
        if n == 0.0 {
            None
        } else if self.loss_seed.is_some() {
            let k = self.level_lost;
            Some((k + 2.0 + 2.0 * (k * (n - k) / n + 1.0).sqrt()) / (n + 4.0))
        } else {
            Some(self.upper_loss_level(self.random_loss()))
        }
    }

    /// Cumulative counts across the estimator's loss decisions.
    pub fn loss_events(&self) -> LossEvents {
        self.events
    }

    /// Transit time above the minimum seen in the last ten seconds of sending, in microseconds.
    pub fn queue_delay_us(&self) -> u64 {
        self.queue_delay_us
    }

    /// Forget the transit-time baseline and reseed loss at the current rate on the next report.
    pub fn reset_delay_base(&mut self) {
        self.minima.clear();
        self.queue_delay_us = 0;
        self.interval = LossSample::default();
        self.interval_dirty = false;
        self.last_loss_decision_us = None;
        self.loss_seed = None;
        self.level_lost = 0.0;
        self.level_total = 0.0;
        self.level_at_us = 0;
        self.watch = LossWatch::default();
        self.clear_bps = self.estimate;
        self.dip = None;
        self.suspect = None;
        self.loss_cap = None;
        // The first report marks the new path's time origin for seeding and growth.
        self.last_update_us = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{Path, PathConfig, PathStats};

    #[test]
    fn trend_distinguishes_constant_delay_from_a_growing_queue() {
        let mut trend = Trend::new();
        let mut group = |i: u64, extra_us: u64| {
            for within in [0, 2_000, 5_000] {
                trend.record(Packet {
                    send_us: 1_000_000 + i * 10_000 + within,
                    arrival_us: 20_000 + i * 10_000 + within + extra_us,
                });
            }
            (trend.slope, trend.overuse)
        };
        for i in 0..65 {
            let (slope, overuse) = group(i, 0);
            assert!(slope.abs() < 1e-10 && !overuse, "constant transit time gave slope {slope}, overuse {overuse}");
        }
        let detected = (1..=12).find(|i| group(64 + i, i * 5_000).1);
        eprintln!("trend detected 5 ms/group growth at group {detected:?}");
        assert!(detected.is_some(), "5 ms more delay per group went undetected for 12 groups");
    }

    #[test]
    fn queue_backoff_uses_receiver_time_for_bytes_and_sender_time_for_rtt() {
        let mut rate = RateEstimator::new(300_000);
        rate.on_arrival(1_000_000, 10_000, 4_000);
        rate.on_arrival(1_200_000, 510_000, 3_000);
        rate.on_arrival(1_210_000, 480_000, 2_000);
        assert_eq!(rate.queue_delay_us(), 260_000, "the unknown clock offset cancels");
        assert_eq!(rate.update(1_300_000, 350_000, 300_000), 68_000,
            "the half-second window includes the reordered packet and expires the oldest");

        rate.on_arrival(1_220_000, 1_030_000, 2_000);
        assert_eq!(rate.update(1_500_000, 350_000, 300_000), 68_000, "a long RTT spaces the cuts");
        assert_eq!(rate.update(1_650_000, 350_000, 300_000), 27_200, "back off once the RTT has passed");
        rate.on_settled(30, 0);
        assert_eq!(rate.update(1_950_000, 350_000, 300_000), 27_200, "loss from the recent queue backoff cannot cut again");

        rate.on_arrival(11_220_000, 11_030_000, 2_000);
        assert_eq!(rate.queue_delay_us(), 0, "the old transit minimum expires after ten seconds");
        assert_eq!(rate.update(11_300_000, 350_000, 0), 27_200, "the skipped loss decision consumed its counts");
        rate.on_arrival(11_230_000, 11_060_000, 2_000);
        assert_eq!(rate.queue_delay_us(), 20_000, "queue delay uses the new minimum");
        rate.reset_delay_base();
        assert_eq!(rate.queue_delay_us(), 0, "reset clears the old queue measurement");
        rate.on_arrival(11_240_000, 11_090_000, 2_000);
        assert_eq!(rate.queue_delay_us(), 0, "the first arrival on the new path sets its baseline");
    }

    #[test]
    fn loss_seed_pools_clean_counts_and_releases_startup_on_time_or_rate() {
        use crate::guard::{video_ask, Guard};

        // For 1/36 the Wilson formula gives 0.145218, rounded here to 0.145.
        let counts = [(0, 8, 333), (0, 12, 250), (1, 12, 361), (0, 16, 200),
                      (0, 24, 143), (1, 24, 207), (1, 36, 145), (10, 50, 333)];
        let readings = counts.map(|(lost, total, expected)| {
            let mut rate = RateEstimator::new(100_000);
            rate.update(0, 0, 0);
            rate.on_settled(lost, total - lost);
            rate.update(LOSS_INTERVAL_US, 0, 0);
            let level = rate.video_loss_level().unwrap_or_else(|| panic!("{lost}/{total} pooled packets must give a seed reading"));
            if (lost, total) == (0, 16) {
                let mut guard = Guard::new(Guard::rung_for(video_ask(100_000)));
                guard.tick(0, 100_000, 0, None);
                guard.tick(LOSS_INTERVAL_US, 200_000, 0, Some(level));
                assert_eq!(guard.ask_bps(), video_ask(200_000), "0/16 at {level} must release video to the full 200000 bit/s estimate");
            }
            (lost, total, expected, level)
        });
        for (lost, total, expected, level) in readings {
            assert_eq!((1000.0 * level).round() as u32, expected, "Wilson seed reading for {lost}/{total} was {level:.12}, expected {expected} thousandths");
        }

        let mut rate = RateEstimator::new(100_000);
        assert_eq!(rate.video_loss_level(), None, "video must wait for the first loss seed before any report");
        let at = |us: u64| 7_000_000 + us;
        rate.update(at(0), 40_000, 100_000);
        rate.on_settled(1, 0);
        rate.update(at(339_999), 40_000, 0);
        assert_eq!(rate.random_loss(), 0.0, "the pending loss must wait 300 ms plus the 40 ms RTT");
        rate.on_settled(0, 19);
        assert_eq!(rate.update(at(340_000), 40_000, 100_000), 100_000, "1/20 loss holds the seed rate");
        assert_eq!(rate.random_loss(), 0.05, "the first decision combines the settled reports into 1/20 loss");
        assert_eq!(rate.update(at(350_000), 40_000, 100_000), 100_000, "the seed hold lasts between decisions");
        rate.on_settled(7, 2);
        assert_eq!(rate.update(at(680_000), 40_000, 100_000), 100_000, "7/9 loss cannot cut the seed rate");
        rate.on_settled(1, 19);
        assert_eq!(rate.update(at(2_000_000), 40_000, 100_000), 100_000, "49 pooled packets still hold at two seconds");
        assert_eq!(rate.random_loss(), 9.0 / 49.0, "seed fractions must be pooled by packet count");
        rate.on_settled(1, 0);
        rate.update(at(2_340_000), 40_000, 0);
        assert_eq!(rate.random_loss(), 0.20, "the fiftieth packet completes a 10/50 seed");
        assert_eq!(rate.video_loss_level(), Some(0.20 + (0.20_f64 * 0.80 / 50.0).sqrt()), "video must use one standard error above 10/50 once seeded");
        rate.on_settled(1, 0);
        assert_eq!(rate.update(at(3_340_000), 40_000, 200_000), 200_000, "startup doubles while the 1/1 loss run stays open");
        assert_eq!(rate.random_loss(), 0.20, "the open run must leave the 10/50 seed alone");
        rate.on_settled(0, 9);
        rate.update(at(3_680_000), 40_000, 0);
        let decay = (-0.134_f64).exp();
        let expected = (10.0 * decay + 1.0) / (50.0 * decay + 10.0);
        assert!((rate.random_loss() - expected).abs() < 1e-12,
            "the cleared 1/10 run must join the seed decayed over 1.34 s, expected {expected}, got {}", rate.random_loss());

        let before_backoff = rate.random_loss();
        rate.on_arrival(0, 0, 1);
        rate.on_arrival(10_000, 120_000, 1);
        assert_eq!(rate.update(at(3_740_000), 40_000, 200_000), MIN_BPS, "delay backoff respects the 16 kbit/s floor");
        rate.on_arrival(130_000, 130_000, 1);
        rate.on_settled(0, 100);
        rate.update(at(4_140_000), 40_000, 0);
        assert_eq!(rate.random_loss(), before_backoff, "a 0/100 interval containing overuse must leave the level alone");

        let mut rate = RateEstimator::new(100_000);
        rate.update(at(0), 40_000, 100_000);
        rate.on_settled(1, 49);
        assert_eq!(rate.update(at(340_000), 40_000, 100_000), 100_000, "50 packets before two seconds still hold at exactly 2%");
        rate.on_settled(0, 1);
        assert_eq!(rate.update(at(1_340_000), 40_000, 100_000), 150_000, "a loss-free interval releases the hold up to 1.5 times sent");
        rate.on_settled(1, 0);
        let bps = rate.update(at(1_680_000), 40_000, 200_000);
        assert!(bps > 150_000, "at 1.5 times start the seed must end early even with 1/1 lost, got {bps} bit/s");
        assert_eq!(rate.random_loss(), 2.0 / 52.0, "the early seed pools all 52 packets at its last decision");
    }

    #[test]
    fn loss_alarm_accumulates_evidence_and_the_dip_distinguishes_random_from_clean() {
        for low_losses in [5, 0] {
            let mut rate = RateEstimator::new(320_000);
            rate.update(0, 0, 0);
            rate.on_settled(1, 99);
            rate.update(2_000_000, 0, 0);
            rate.on_arrival(2_000_000, 2_000_000, 9_900);
            // Taint only the first interval of the confirmed clean case.
            rate.interval_dirty = low_losses == 0;
            for interval in 1..=3 {
                rate.on_settled(5, 35);
                let bps = rate.update(2_000_000 + interval * LOSS_INTERVAL_US, 0, 0);
                let expected = if interval == 3 { 136_000 } else { 320_000 };
                assert_eq!(bps, expected, "5/40 interval {interval} at 1% level, expected {expected} bit/s for low case {low_losses}/40");
                assert_eq!(rate.events.alarms, u32::from(interval == 3), "only the third 5/40 interval should alarm, interval {interval}");
                assert_eq!(rate.random_loss(), 0.01, "open 5/40 run must preserve the 1/100 level on interval {interval}");
            }
            rate.on_settled(40, 0);
            assert_eq!(rate.update(3_200_000, 0, 320_000), 136_000, "skip the mixed 40/40 interval and hold the dip rate");
            rate.on_settled(low_losses, 40 - low_losses);
            let bps = rate.update(3_500_000, 0, 320_000);
            if low_losses == 5 {
                assert_eq!((rate.events.random, rate.events.clean), (1, 0), "5/40 after the cut should give one random verdict");
                assert_eq!(rate.random_loss(), 20.0 / 160.0, "random verdict must replace 1/100 with pooled high and low 20/160");
                assert_eq!(bps, 320_000, "random verdict must restore the pre-dip rate");
                assert!(rate.startup, "the random verdict at {bps} bit/s must preserve startup");
            } else {
                assert_eq!((rate.events.random, rate.events.clean), (0, 1), "0/40 after the cut should give one clean verdict");
                let decay = (-0.15_f64).exp();
                let expected = decay / (100.0 * decay + 40.0);
                assert!((rate.random_loss() - expected).abs() < 1e-12, "clean verdict must learn only 0/40, expected {expected}, got {}", rate.random_loss());
                assert_eq!(bps, 136_000, "confirmed clean verdict must keep the dip rate");
                assert!(!rate.startup, "confirmed clean verdict at {bps} bit/s must end startup");
            }
        }
    }

    #[test]
    fn clean_dips_retest_the_suspected_sent_rate_and_clear_on_qualifying_packets() {
        let mut rate = RateEstimator::new(320_000);
        rate.update(0, 0, 0);
        rate.on_settled(1, 99);
        let mut now = 2_000_000;
        rate.update(now, 0, 0);
        rate.on_arrival(now, now, 9_900);
        let mut report = |rate: &mut RateEstimator, dt, lost, delivered, sent| {
            now += dt;
            rate.on_settled(lost, delivered);
            rate.update(now, 0, sent)
        };
        let suspect = |rate: &RateEstimator| rate.suspect.as_ref().map(|s| (s.sent_bps, s.needed, s.cleared));

        let cut = report(&mut rate, LOSS_INTERVAL_US, 10, 0, 200_000);
        assert_eq!(cut, 136_000, "a first 10/10 alarm must probe at 136000 bit/s");
        report(&mut rate, LOSS_INTERVAL_US, 0, 40, 200_000);
        let restored = report(&mut rate, LOSS_INTERVAL_US, 0, 40, 200_000);
        assert_eq!((restored, rate.startup), (320_000, true), "a first clean verdict must restore the rate and startup");
        assert_eq!(suspect(&rate), Some((200_000, 100, 0)), "the 10-packet alarm needs the 100-packet minimum at its sent rate");
        assert_eq!(report(&mut rate, 1_000_000, 0, 0, 320_000), 345_600, "startup with a suspect must grow 8% in one second");

        report(&mut rate, LOSS_INTERVAL_US, 0, 100, 179_999);
        assert_eq!(suspect(&rate), Some((200_000, 100, 0)), "100 packets below 90% of 200000 must not clear the suspect");
        report(&mut rate, LOSS_INTERVAL_US, 0, 99, 180_000);
        assert_eq!(suspect(&rate), Some((200_000, 100, 99)), "99 qualifying packets must leave one more to clear");
        report(&mut rate, LOSS_INTERVAL_US, 0, 1, 180_000);
        assert_eq!((suspect(&rate), rate.events.suspects_cleared), (None, 1), "the hundredth packet at exactly 90% must clear the suspect");

        report(&mut rate, LOSS_INTERVAL_US, 15, 105, 200_000);
        report(&mut rate, LOSS_INTERVAL_US, 0, 40, 200_000);
        let restored = report(&mut rate, LOSS_INTERVAL_US, 0, 40, 200_000);
        assert_eq!(restored, 345_600, "after clearance a new clean dip must restore the last cleared 345600 bit/s");
        assert_eq!(suspect(&rate), Some((200_000, 480, 0)), "the 120-packet alarm needs four times its high sample");
        let cut = report(&mut rate, LOSS_INTERVAL_US, 15, 105, 150_000);
        assert!(cut < restored, "a second alarm at exactly 75% must dip again, got {cut} from {restored} bit/s");
        assert_eq!(rate.dip.as_ref().map(|d| d.confirmed), Some(true), "the 150000 bit/s alarm must confirm the 200000 bit/s suspect");
        assert_eq!(suspect(&rate), None, "the confirming alarm at {cut} bit/s must remove the suspect");
        report(&mut rate, LOSS_INTERVAL_US, 0, 40, 150_000);
        let settled = report(&mut rate, LOSS_INTERVAL_US, 0, 40, 150_000);
        assert_eq!((settled, rate.startup), (cut, false), "a confirmed clean dip must keep {cut} bit/s and end startup");

        for confirm_retest in [false, true] {
            // During doubling the run's packets settled at very different rates.
            let mut rate = RateEstimator::new(100_000);
            report(&mut rate, 0, 0, 0, 0);
            rate.on_arrival(0, 0, 10_000);
            assert_eq!(report(&mut rate, 2_000_000, 1, 99, 400_000), 400_000, "a completed 1/100 seed must grow to 400000 bit/s");
            assert_eq!(report(&mut rate, 1_000_000, 0, 100, 800_000), 800_000, "the cleared 0/100 interval must double the 400000 bit/s estimate");
            for expected in [1_600_000, 3_200_000] {
                assert_eq!(report(&mut rate, 1_000_000, 4, 36, expected), expected, "startup must still double to {expected} bit/s during the open 4/40 loss run");
            }
            report(&mut rate, LOSS_INTERVAL_US, 4, 36, 1_000_000);
            assert_eq!(rate.dip.as_ref().map(|d| (d.clear_bps, d.pre_estimate, d.confirmed)), Some((400_000.0, 3_200_000.0, false)),
                "the third 4/40 interval must alarm at 3200000 bit/s with the 400000 bit/s cleared decision");
            report(&mut rate, LOSS_INTERVAL_US, 0, 40, 1_000_000);
            assert_eq!(report(&mut rate, LOSS_INTERVAL_US, 0, 40, 1_000_000), 400_000, "the unconfirmed clean dip must restore the cleared 400000 bit/s");
            assert_eq!(suspect(&rate), Some((400_000, 480, 0)), "a 120-packet alarm sent at 1000000 bit/s must retest at the restored 400000 bit/s");

            assert_eq!(report(&mut rate, 1_000_000, 0, 0, 400_000), 432_000, "the suspect retest must grow to 432000 bit/s");
            report(&mut rate, LOSS_INTERVAL_US, 15, 105, 300_000);
            assert_eq!(rate.dip.as_ref().map(|d| d.confirmed), Some(true), "300000 bit/s must confirm the retest at 75% of its reachable rate");
            report(&mut rate, LOSS_INTERVAL_US, 0, 40, 300_000);
            let saved = (rate.level_lost, rate.level_total, rate.level_at_us);
            assert_eq!(report(&mut rate, LOSS_INTERVAL_US, 5, 35, 300_000), 432_000, "a random verdict must return the full 432000 bit/s pre-alarm estimate");
            assert_eq!((rate.events.random, rate.startup), (1, false), "a confirmed random verdict must end doubling at the restored 432000 bit/s");
            assert_eq!(report(&mut rate, 1_000_000, 0, 0, 500_000), 466_560, "one second after the confirmed random verdict must grow 432000 bit/s by 8%");

            assert_eq!(suspect(&rate), Some((300_000, 480, 0)), "the random verdict at 432000 bit/s must retest the lower 300000 bit/s sent rate for 480 packets");
            assert_eq!(rate.suspect.as_ref().and_then(|s| s.old_level), Some(saved), "a random verdict must save its previous loss counts and sample time {saved:?}");
            assert_eq!(rate.random_loss(), 20.0 / 160.0, "the random verdict must pool the 15/120 high and 5/40 low samples");
            if confirm_retest {
                report(&mut rate, LOSS_INTERVAL_US, 40, 0, 225_000);
                assert_eq!((rate.level_lost, rate.level_total, rate.level_at_us), saved, "the alarm at exactly 75% of 300000 bit/s must restore all three saved level fields");
                let expected_cap = 160_000.0 / (1.0 - saved.0 / saved.1);
                assert_eq!(rate.dip.as_ref().map(|d| (d.confirmed, d.reference, d.cap)), Some((true, 0.02, expected_cap)),
                    "the retest alarm must confirm with the old 2% reference and 160000 bit/s delivered divided by the old surviving fraction");
                assert_eq!(rate.events.reverted, 1, "the alarm at 225000 bit/s must count one reverted random verdict");
            } else {
                report(&mut rate, LOSS_INTERVAL_US, 60, 420, 270_000);
                assert_eq!((suspect(&rate), rate.events.suspects_cleared, rate.events.reverted), (None, 1, 0),
                    "480 qualifying packets at 90% of 300000 bit/s must clear the random verdict's suspect");
                assert_eq!(rate.random_loss(), 20.0 / 160.0, "a cleared 60/480 run must preserve the pooled level of 20/160");
            }
        }
    }

    #[test]
    fn confirmed_capacity_slows_growth_confirms_a_nearby_alarm_and_clears_on_random_loss() {
        let mut rate = RateEstimator::new(320_000);
        rate.update(0, 0, 0);
        rate.on_settled(0, 100);
        rate.update(2_000_000, 0, 0);
        rate.on_arrival(2_000_000, 2_000_000, 15_000);
        rate.interval_dirty = true;
        rate.on_settled(10, 30);
        assert_eq!(rate.update(2_300_000, 0, 0), 204_000, "the tainted alarm must dip to 85% of 240000 bit/s");
        rate.update(2_600_000, 0, 0);
        // The delivery window changes before the verdict; the dip remembers the alarm's cap.
        rate.on_arrival(2_800_000, 2_800_000, 12_000);
        rate.on_settled(0, 40);
        assert_eq!(rate.update(2_900_000, 0, 0), 204_000, "the confirmed clean verdict must keep the cut rate");
        assert_eq!(rate.loss_cap, Some(240_000.0), "the saved cap must be the alarm's 240000 bit/s");

        assert_eq!(rate.update(5_900_000, 0, 160_000), 240_000, "the sent ceiling should bring the estimate exactly to the remembered cap");
        assert_eq!(rate.update(6_900_000, 0, 250_000), 249_600, "one second at the remembered cap must grow by 4%");
        assert!(rate.suspect.is_none() && !rate.interval_dirty, "the next alarm must rely on the cap alone at {} bit/s", rate.estimate_bps());
        rate.on_settled(10, 30);
        assert_eq!(rate.update(7_200_000, 0, 0), 163_200, "the new 192000 bit/s cap must give a 163200 bit/s dip");
        assert_eq!(rate.dip.as_ref().map(|d| d.confirmed), Some(true), "192000 is exactly 80% of the saved 240000 bit/s and must confirm");
        rate.update(7_500_000, 0, 0);
        rate.on_settled(10, 30);
        assert_eq!(rate.update(7_800_000, 0, 0), 249_600, "10/40 loss at both rates must restore the pre-dip estimate");
        assert_eq!(rate.loss_cap, None, "a random verdict on the confirmed dip must clear the 240000 bit/s cap");
    }

    #[test]
    fn path_change_restarts_loss_seeding_at_the_current_estimate() {
        // An open watch and a pending dip occur at different points in the retest.
        for pending_dip in [false, true] {
            let mut rate = RateEstimator::new(320_000);
            rate.update(0, 0, 0);
            rate.on_settled(1, 99);
            rate.update(2_000_000, 0, 0);
            rate.interval_dirty = true;
            for (at, bytes) in [(2_000_000, 9_900), (2_900_000, 4_500)] {
                rate.on_arrival(at, at, bytes);
                rate.on_settled(10, 0);
                rate.update(at + 300_000, 0, 90_000);
                rate.update(at + 600_000, 0, 90_000);
                rate.on_settled(0, 40);
                rate.update(at + 900_000, 0, 90_000);
            }
            rate.on_settled(if pending_dip { 10 } else { 1 }, 0);
            rate.update(4_100_000, 0, 0);
            assert_eq!((rate.loss_cap, rate.suspect.is_some(), rate.dip.is_some(), rate.watch.sum > 0.0),
                (Some(160_000.0), true, pending_dip, !pending_dip), "the old path must exercise its cap, suspect and pending evidence");
            rate.on_settled(3, 7);
            rate.interval_dirty = true;
            let before = rate.estimate;
            rate.reset_delay_base();
            assert_eq!(rate.video_loss_level(), None, "the reset must leave video waiting for the new path's seed");
            assert_eq!(rate.estimate, before, "reset must preserve the current estimate, including during a dip");
            assert_eq!(rate.clear_bps, before, "the new path's cleared rate must start at its reset estimate of {before} bit/s");
            assert_eq!((rate.random_loss(), rate.level_lost, rate.level_total, rate.level_at_us), (0.0, 0.0, 0.0, 0), "old level counts must be forgotten");
            assert_eq!((rate.interval.packets(), rate.interval_dirty, rate.last_loss_decision_us), (0, false, None), "old settled verdicts and their window must be forgotten");
            assert_eq!((rate.watch.sum, rate.watch.sample.packets(), rate.watch.tainted), (0.0, 0, false), "reset must discard the open watch run");
            assert_eq!((rate.dip.is_some(), rate.suspect.is_some(), rate.loss_cap), (false, false, None), "reset must forget the old dip, suspect and cap");

            let first = 20_000_000;
            rate.update(first, 0, 1_000_000);
            assert_eq!(rate.estimate, before, "the first new-path report must not grow across the idle gap");
            assert_eq!(rate.loss_seed.as_ref().map(|s| (s.since_us, s.start_bps, s.hold)), Some((first, before, false)),
                "the new seed must use the first report's clock and the exact reset estimate {before}");
            rate.on_settled(5, 45);
            rate.update(first + 1_000_000, 0, 1_000_000);
            assert_eq!(rate.estimate, before, "a lossy 50-packet seed must still hold one second after restarting");
            rate.on_settled(5, 45);
            rate.update(first + 2_000_000, 0, 1_000_000);
            assert_eq!((rate.random_loss(), rate.loss_seed.is_some()), (0.10, false), "two new-path seconds must complete the pooled 10/100 seed");
        }

        let mut results = Vec::new();
        for seed in SEEDS {
            let mut call = Call::new(PathConfig { loss_ppm: 10_000, ..Default::default() }, seed);
            call.app_bps = 4_000_000;
            call.change_path_at(6_000_000, |path| path.loss_ppm = 100_000);
            let before = call.rate.estimate_bps();
            call.rate.reset_delay_base();
            assert_eq!((call.rate.estimate_bps(), call.rate.random_loss()), (before, 0.0), "seed {seed} must start the new path at {before} bit/s with an empty level");
            let (_, level) = call.run_to(15, |_| {});
            call.print_window("path_reset", seed);
            results.push((seed, level));
        }
        for (seed, level) in results {
            assert!((0.07..=0.13).contains(&level), "1% to 10% at reset seed {seed}: 15 s level {level} outside 7–13%");
        }
    }

    const TICK_US: u64 = 5_000;
    const REPORT_US: u64 = 50_000;
    const SEEDS: [u64; 8] = [1, 3, 5, 7, 9, 11, 13, 15];
    const PACKET_BYTES: usize = 1_000;
    const PACKET_BITS: u64 = PACKET_BYTES as u64 * 8;

    struct Report {
        due_us: u64,
        arrivals: Vec<(u64, u64)>,
        lost: u64,
        delivered: u64,
    }

    #[derive(Clone, Copy)]
    struct RateWindow {
        min: u64,
        peak: u64,
        sum: u64,
        ticks: u64,
    }

    impl RateWindow {
        fn new(bps: u64) -> Self {
            Self { min: bps, peak: bps, sum: 0, ticks: 0 }
        }

        fn sample(&mut self, bps: u64) {
            self.min = self.min.min(bps);
            self.peak = self.peak.max(bps);
            self.sum += bps;
            self.ticks += 1;
        }

        fn mean(self) -> u64 {
            self.sum / self.ticks
        }
    }

    #[derive(Default)]
    struct PolicerTrace {
        before_alarm_peak: u64,
        retest_peak: Option<u64>,
        late_peak: u64,
        peak_bps: u64,
        peak_at_us: u64,
        peak_queue_us: u64,
        peak_interval: Option<(u64, u64, LossSample)>,
        previous_decision: PathStats,
        queue_drops_at_cut: u64,
        decisions: Vec<String>,
    }

    impl PolicerTrace {
        fn sample(&mut self, now: u64, rate: &RateEstimator) {
            let bps = rate.estimate_bps();
            if rate.events.alarms == 0 {
                self.before_alarm_peak = self.before_alarm_peak.max(bps);
            } else if now <= 8_000_000 {
                self.retest_peak = Some(self.retest_peak.unwrap_or(0).max(bps));
            }
            if now >= 8_000_000 {
                self.late_peak = self.late_peak.max(bps);
            }
        }

        fn update(&mut self, rate: &mut RateEstimator, now: u64, rtt: u64, sent: u64, path: PathStats) {
            let from = rate.last_loss_decision_us;
            let sample = rate.interval;
            let before = rate.estimate_bps();
            let seed = rate.loss_seed.as_ref().map(|s| (s.since_us, 1.5 * s.start_bps <= rate.estimate));
            let dip = rate.dip.as_ref().map(|d| (d.confirmed, d.skip));
            let events = rate.events;
            let after = rate.update(now, rtt, sent);
            if now <= 5_000_000 && after > self.peak_bps {
                self.peak_bps = after;
                self.peak_at_us = now;
                self.peak_queue_us = rate.queue_delay_us();
                self.peak_interval = None;
            }
            if rate.last_loss_decision_us != Some(now) {
                return;
            }
            let previous_path = std::mem::replace(&mut self.previous_decision, path);
            let Some(from) = from else { return };
            // Decisions include the report at their right endpoint. The last
            // interval containing a peak before 5 s can settle after 5 s.
            if from < self.peak_at_us && self.peak_at_us <= now {
                self.peak_interval = Some((from, now, sample));
            }
            if now > 5_000_000 && dip.is_none() && rate.dip.is_none() {
                return;
            }
            let event = if let Some((since, reached_rate)) = seed {
                if rate.loss_seed.is_some() {
                    "seed".to_owned()
                } else {
                    let timed = now - since >= LOSS_SEED_US && rate.level_total >= LOSS_SEED_PACKETS;
                    format!("seed ended: time+packets={timed} ({} µs, {} packets), 1.5x start={reached_rate}", now - since, rate.level_total)
                }
            } else if let Some((confirmed, skip)) = dip {
                if rate.events.random > events.random {
                    format!("random; restored {after} bit/s, pooled {:.0}/{:.0} lost", rate.level_lost, rate.level_total)
                } else if rate.events.clean > events.clean {
                    if confirmed { "confirmed clean".to_owned() } else { format!("unconfirmed clean; restored {after} bit/s") }
                } else if rate.events.dirty > events.dirty {
                    "dirty; dip dropped".to_owned()
                } else if skip {
                    "dip: skip".to_owned()
                } else {
                    "dip: collecting".to_owned()
                }
            } else if let Some(d) = &rate.dip {
                self.queue_drops_at_cut = path.queue_drops;
                format!("alarm cap={:.0} bit/s clear={:.0} bit/s confirmed={} reverted={}", d.cap, d.clear_bps, d.confirmed, rate.events.reverted != events.reverted)
            } else {
                format!("watch sum={:.2}", rate.watch.sum)
            };
            let mut line = format!("{now} µs {event}, settled {} lost/{} delivered, level {:.3}%, estimate {before}->{after} bit/s, sent {sent} bit/s, queue {} µs, path sent {} random {} queue drops {}",
                sample.lost, sample.delivered, 100.0 * rate.random_loss(), rate.queue_delay_us(),
                path.sent - previous_path.sent, path.lost - previous_path.lost, path.queue_drops - previous_path.queue_drops);
            if dip.is_some() && rate.dip.is_none() {
                use std::fmt::Write;
                write!(line, ", queue drops since cut {}", path.queue_drops - self.queue_drops_at_cut).unwrap();
            }
            self.decisions.push(line);
        }
    }

    struct Call {
        now_us: u64,
        rate: RateEstimator,
        window: RateWindow,
        path: Path,
        app_bps: u64,
        credit: u64,
        drop_hop: Option<usize>,
        policer_trace: Option<PolicerTrace>,
        // Indexed by hop, carrying send time and the receiver's first observation.
        packets: Vec<(u64, Option<u64>)>,
        highest_end: usize,
        last_report_end: usize,
        settled_end: usize,
        incoming: Vec<Vec<u8>>,
        reports: VecDeque<Report>,
    }

    impl Call {
        fn new(config: PathConfig, seed: u64) -> Self {
            let rate = RateEstimator::new(300_000);
            Self {
                now_us: 0,
                window: RateWindow::new(rate.estimate_bps()),
                rate,
                path: Path::new(config, seed),
                app_bps: u64::MAX,
                credit: 0,
                drop_hop: None,
                policer_trace: None,
                packets: Vec::new(),
                highest_end: 0,
                last_report_end: 0,
                settled_end: 0,
                incoming: Vec::new(),
                reports: VecDeque::new(),
            }
        }

        fn step(&mut self) {
            self.now_us += TICK_US;
            // Bit-microseconds of pacing credit carry across both ticks and rate changes.
            self.credit += self.rate.estimate_bps().min(self.app_bps) * TICK_US;
            while self.credit >= PACKET_BITS * 1_000_000 {
                self.credit -= PACKET_BITS * 1_000_000;
                let mut datagram = vec![0; PACKET_BYTES];
                datagram[..8].copy_from_slice(&(self.packets.len() as u64).to_be_bytes());
                self.path.send(self.now_us, datagram);
                self.packets.push((self.now_us, None));
            }
            self.path.poll(self.now_us, &mut self.incoming);
            for datagram in self.incoming.drain(..) {
                let hop = u64::from_be_bytes(datagram[..8].try_into().unwrap()) as usize;
                if self.drop_hop == Some(hop) {
                    continue;
                }
                self.packets[hop].1 = Some(self.now_us);
                self.highest_end = self.highest_end.max(hop + 1);
            }
            if self.now_us % REPORT_US == 0 {
                self.report();
            }
            while self.reports.front().is_some_and(|r| r.due_us <= self.now_us) {
                let report = self.reports.pop_front().unwrap();
                let latency = self.path.config.latency_us;
                let mut rtt = None;
                for (send, arrival) in report.arrivals {
                    let sample = arrival - send + latency;
                    rtt = Some(rtt.map_or(sample, |r: u64| r.min(sample)));
                    self.rate.on_arrival(send, arrival, PACKET_BYTES);
                }
                self.rate.on_settled(report.lost, report.delivered);
                let first = self.packets.partition_point(|&(sent, _)| self.now_us - sent >= 1_000_000);
                let sent_bps = (self.packets.len() - first) as u64 * PACKET_BITS;
                let rtt = rtt.unwrap_or(2 * latency);
                if let Some(trace) = &mut self.policer_trace {
                    trace.update(&mut self.rate, self.now_us, rtt, sent_bps, self.path.stats);
                } else {
                    self.rate.update(self.now_us, rtt, sent_bps);
                }
            }
            self.window.sample(self.rate.estimate_bps());
            if let Some(trace) = &mut self.policer_trace {
                trace.sample(self.now_us, &self.rate);
            }
        }

        fn begin_window(&mut self) {
            self.window = RateWindow::new(self.rate.estimate_bps());
        }

        fn print_window(&self, case: &str, seed: u64) {
            let config = self.path.config;
            eprintln!("{case} seed={seed} ppm={} queue_us={} end_us={} peak={} mean={} level={:.8} {:?}",
                config.loss_ppm, config.queue_us, self.now_us, self.window.peak, self.window.mean(), self.rate.random_loss(), self.rate.events);
        }

        fn run_to(&mut self, second: u64, mut sample: impl FnMut(&Self)) -> (u64, f64) {
            while self.now_us < second * 1_000_000 {
                self.step();
                sample(self);
            }
            (self.rate.estimate_bps(), self.rate.random_loss())
        }

        fn change_path_at(&mut self, at_us: u64, change: impl FnOnce(&mut PathConfig)) {
            while self.now_us + TICK_US < at_us {
                self.step();
            }
            change(&mut self.path.config);
            self.step();
        }

        fn report(&mut self) {
            // As in ARRIVALS, the range ends at the highest observed hop. It appears
            // in two reports, so a packet still in the router is not declared lost.
            let mut arrivals = Vec::new();
            for &(sent, arrived) in &self.packets[self.settled_end..self.highest_end] {
                if let Some(at) = arrived {
                    if at > self.now_us - REPORT_US {
                        arrivals.push((sent, at));
                    }
                }
            }
            let settled = &self.packets[self.settled_end..self.last_report_end];
            let lost = settled.iter().filter(|(_, at)| at.is_none()).count() as u64;
            self.reports.push_back(Report {
                due_us: self.now_us + self.path.config.latency_us,
                arrivals,
                lost,
                delivered: settled.len() as u64 - lost,
            });
            self.settled_end = self.last_report_end;
            self.last_report_end = self.highest_end;
        }
    }

    #[test]
    fn rate_ramps_to_the_bottleneck_and_tracks_a_capacity_drop() {
        let drop_us = 12_000_000;
        let mut call = Call::new(PathConfig { rate_bps: 2_000_000, queue_us: 300_000, ..Default::default() }, 7);
        let mut ramp_us = None;
        let mut cut_us = None;
        let mut sum_bps = 0;
        let mut queues = Vec::new();
        let mut after_one_bps = 0;
        let mut after_three_bps = 0;
        let mut after_three_queue = 0;
        let mut trace = Vec::new();
        while call.now_us < drop_us + 4_000_000 {
            if call.now_us == drop_us {
                call.path.config.rate_bps = 700_000;
            }
            call.step();
            let now = call.now_us;
            let bps = call.rate.estimate_bps();
            let queue = call.rate.queue_delay_us();
            if now <= drop_us && bps >= 1_600_000 {
                ramp_us.get_or_insert(now);
            }
            if (8_000_000..drop_us).contains(&now) && now % REPORT_US == 0 {
                sum_bps += bps;
                queues.push(queue);
            }
            if now > drop_us && bps <= 800_000 && cut_us.is_none() {
                cut_us = Some(now - drop_us);
                eprintln!("first drop crossing at {now} µs, {bps} bit/s, queue {queue} µs");
            }
            if now == drop_us + 1_000_000 {
                after_one_bps = bps;
            }
            if now == drop_us + 3_000_000 {
                after_three_bps = bps;
                after_three_queue = queue;
            }
            if now % 1_000_000 == 0 {
                trace.push((now / 1_000_000, bps, queue));
            }
        }
        queues.sort_unstable();
        let mean_bps = sum_bps / queues.len() as u64;
        let median_queue = (queues[queues.len() / 2 - 1] + queues[queues.len() / 2]) / 2;
        eprintln!("(s, bit/s, queue µs) {trace:?}");
        call.print_window("ramp_and_drop", 7);
        eprintln!("1.6 Mbit/s at {ramp_us:?} µs, mean {mean_bps} bit/s, median queue {median_queue} µs; drop crossed 800 kbit/s after {cut_us:?} µs");
        assert!(ramp_us.is_some_and(|at| at <= 6_000_000), "1.6 Mbit/s should be reached by 6 s, got {ramp_us:?} µs");
        assert!((1_400_000..=2_200_000).contains(&mean_bps), "8–12 s mean on the 2 Mbit/s path was {mean_bps} bit/s");
        assert!(median_queue < 60_000, "8–12 s median queue was {median_queue} µs");
        assert!(after_one_bps <= 800_000, "one second after the drop the estimate was {after_one_bps} bit/s");
        assert!(after_three_bps >= 500_000, "three seconds after the drop the estimate was starved at {after_three_bps} bit/s");
        assert!(after_three_queue < 100_000, "three seconds after the drop the queue was {after_three_queue} µs");
    }

    #[test]
    fn random_loss_without_a_queue_grows_and_is_learned() {
        let mut results = Vec::new();
        for loss_ppm in [10_000, 50_000, 150_000, 250_000, 500_000] {
            for seed in SEEDS {
                let mut call = Call::new(PathConfig { loss_ppm, ..Default::default() }, seed);
                let start = call.rate.estimate_bps();
                let mut previous = start;
                let mut largest_drop = 0;
                let mut sample = |c: &Call| {
                    let bps = c.rate.estimate_bps();
                    if bps < previous {
                        eprintln!("unqueued {loss_ppm} ppm seed {seed} dip at {} µs: {previous} to {bps} bit/s", c.now_us);
                        largest_drop = largest_drop.max(previous - bps);
                    }
                    previous = bps;
                };
                let (six, level6) = call.run_to(6, &mut sample);
                let (ten, level10) = call.run_to(10, sample);
                eprintln!("unqueued {loss_ppm} ppm seed {seed}: 6 s {six} {level6:.6}, 10 s {ten} {level10:.6}");
                call.print_window("unqueued", seed);
                results.push((loss_ppm, seed, start, six, ten, level10, call.window.min, call.window.peak, largest_drop, call.rate.events.alarms));
            }
        }
        for (loss_ppm, seed, start, six, ten, level10, low, peak, largest_drop, alarms) in results {
            assert_eq!(alarms, 0, "{loss_ppm} ppm seed {seed}: steady unqueued loss should raise no alarms, got {alarms}");
            match loss_ppm {
                10_000 => assert!(six > start, "1% loss seed {seed} should grow from {start} by 6 s, got {six} bit/s"),
                50_000 => assert!(peak > 3_000_000, "5% loss seed {seed} should pass 3 Mbit/s within 10 s, peak {peak} bit/s"),
                _ => {
                    assert!(low >= 9 * start / 10, "{loss_ppm} ppm seed {seed} fell below 90% of {start}, min {low} bit/s");
                    assert!(six >= 1_500_000, "{loss_ppm} ppm seed {seed} should reach 1.5 Mbit/s by 6 s, got {six} bit/s");
                    assert!(ten > six, "{loss_ppm} ppm seed {seed} should grow between 6 s and 10 s, got {six} then {ten} bit/s");
                }
            }
            if loss_ppm == 250_000 || loss_ppm == 500_000 {
                let range = if loss_ppm == 250_000 { 0.19..=0.31 } else { 0.40..=0.60 };
                assert!(range.contains(&level10), "{loss_ppm} ppm seed {seed} should learn {range:?} by 10 s, got {level10}");
                assert_eq!(largest_drop, 0, "{loss_ppm} ppm seed {seed} should never cut on the unqueued path, largest cut {largest_drop} bit/s");
            }
        }
    }

    #[test]
    fn short_loss_bursts_allow_growth_without_inflating_the_medium_loss_level() {
        let config = PathConfig { loss_ppm: 10_000, good_mean_us: 3_000_000, bad_mean_us: 100_000,
            burst_loss_ppm: 500_000, ..Default::default() };
        let results = SEEDS.map(|seed| {
            let mut call = Call::new(config, seed);
            let start = call.rate.estimate_bps();
            let (estimate, level) = call.run_to(10, |_| {});
            eprintln!("bursty path seed {seed}: 10 s estimate {estimate} bit/s, level {level:.8}, {:?}", call.rate.loss_events());
            (seed, start, estimate, level)
        });
        for (seed, start, estimate, level) in results {
            assert!(estimate > 2 * start, "bursty path seed {seed}: 10 s estimate {estimate} bit/s must exceed twice the {start} bit/s start");
            assert!(level < 0.10, "bursty path seed {seed}: 10 s learned loss {level:.8} must stay below 10%");
        }
    }

    #[test]
    fn five_percent_loss_settles_at_the_bottleneck() {
        for seed in SEEDS {
            let mut call = Call::new(PathConfig { rate_bps: 2_000_000, queue_us: 300_000, loss_ppm: 50_000, ..Default::default() }, seed);
            let (six, level6) = call.run_to(6, |_| {});
            call.run_to(8, |_| {});
            call.begin_window();
            let mut sum = call.rate.estimate_bps();
            let mut queues = vec![call.rate.queue_delay_us()];
            let mut sample = |c: &Call| {
                if (8_000_000..12_000_000).contains(&c.now_us) && c.now_us % REPORT_US == 0 {
                    sum += c.rate.estimate_bps();
                    queues.push(c.rate.queue_delay_us());
                }
            };
            let (ten, level10) = call.run_to(10, &mut sample);
            call.run_to(12, sample);
            let mean = sum / queues.len() as u64;
            queues.sort_unstable();
            let mid = queues.len() / 2;
            let median = (queues[mid - 1] + queues[mid]) / 2;
            eprintln!("2 Mbit/s 5% loss seed {seed}: 6 s {six} {level6:.6}, 10 s {ten} {level10:.6}, mean {mean} bit/s, median queue {median} µs");
            call.print_window("bottleneck", seed);
            assert!((1_400_000..=2_200_000).contains(&mean), "5% loss seed {seed} should settle within 1.4–2.2 Mbit/s at 8–12 s, mean {mean} bit/s");
            assert!(median < 60_000, "5% loss seed {seed} should keep the 8–12 s median queue below 60 ms, got {median} µs");
        }
    }

    #[test]
    fn losing_the_fourth_hop_preserves_startup() {
        for seed in SEEDS {
            let mut clean = Call::new(PathConfig::default(), seed);
            let (reference, _) = clean.run_to(4, |_| {});
            let mut call = Call::new(PathConfig::default(), seed);
            call.drop_hop = Some(3);
            let (four, _) = call.run_to(4, |_| {});
            let losses = call.packets[..call.settled_end].iter().filter(|(_, at)| at.is_none()).count();
            let (six, level6) = call.run_to(6, |_| {});
            let (ten, level10) = call.run_to(10, |_| {});
            eprintln!("lost fourth hop seed {seed}: 4 s {four} clean {reference}, 6 s {six} {level6:.8}, 10 s {ten} {level10:.8}");
            call.print_window("fourth", seed);
            assert_eq!(losses, 1, "seed {seed} should have settled just the fourth hop as lost by 4 s, got {losses} losses");
            assert!(10 * four >= 7 * reference, "seed {seed} one early loss should retain 70% of the clean 4 s estimate, got {four} vs {reference} bit/s");
        }
    }

    #[test]
    fn a_policer_limits_rate_and_preserves_the_medium_loss_level() {
        let mut results = Vec::new();
        for loss_ppm in [10_000, 150_000] {
            let (start, end) = if loss_ppm == 10_000 { (5, 15) } else { (8, 20) };
            for queue_us in [2_000, 12_000] {
                for seed in SEEDS {
                    let mut call = Call::new(PathConfig { rate_bps: 1_000_000, queue_us, loss_ppm, ..Default::default() }, seed);
                    call.policer_trace = Some(PolicerTrace { peak_bps: call.window.peak, ..Default::default() });
                    call.run_to(5, |_| {});
                    let early_peak = call.window.peak;
                    let (_, mut max_level) = call.run_to(start, |_| {});
                    call.begin_window();
                    let (_, level) = call.run_to(end, |c| {
                        if c.now_us % 1_000_000 == 0 {
                            max_level = max_level.max(c.rate.random_loss());
                        }
                    });
                    call.print_window("policer", seed);
                    eprintln!("policer {loss_ppm} ppm {queue_us} µs seed {seed}: first 5 s peak {early_peak}, max whole-second level {max_level:.8}");
                    let window = call.window;
                    call.run_to(20, |_| {});
                    let trace = call.policer_trace.unwrap();
                    eprintln!("policer {loss_ppm} ppm {queue_us} µs seed {seed}: pre-alarm {}, first alarm to 8 s {:?}, 8–20 s {}, 20 s level {:.8} suspect={} {:?}",
                        trace.before_alarm_peak, trace.retest_peak, trace.late_peak, call.rate.random_loss(), call.rate.suspect.is_some(), call.rate.events);
                    if queue_us == 12_000 || loss_ppm == 150_000 {
                        if let Some((from, to, sample)) = trace.peak_interval {
                            eprintln!("policer {loss_ppm} ppm {queue_us} µs seed {seed}: peak {early_peak} bit/s at {} µs, loss {}/{} in ({from}, {to}] µs, peak queue {} µs",
                                trace.peak_at_us, sample.lost, sample.packets(), trace.peak_queue_us);
                        }
                    }
                    eprintln!("policer {loss_ppm} ppm {queue_us} µs seed {seed} decisions through 5 s and later dips:\n  {}", trace.decisions.join("\n  "));
                    results.push((loss_ppm, queue_us, seed, window.peak, window.mean(), level, max_level, trace.before_alarm_peak, trace.retest_peak));
                }
            }
        }
        for (loss_ppm, queue_us, seed, peak, mean, level, max_level, before_alarm, retest) in results {
            let retest_limit = match (loss_ppm, queue_us) {
                (150_000, 12_000) => Some(1_900_000),
                (10_000, 12_000) => Some(1_400_000),
                (150_000, 2_000) => Some(1_300_000),
                _ => None,
            };
            // An alarm later than 8 s leaves this measurement window empty.
            if let (Some(bps), Some(limit)) = (retest, retest_limit) {
                assert!(bps <= limit, "{loss_ppm} ppm {queue_us} µs seed {seed}: peak from first alarm to 8 s was {bps} bit/s, limit {limit}");
            }
            if (loss_ppm, queue_us) == (150_000, 12_000) {
                assert!(before_alarm <= 2_600_000, "15% loss 12 ms policer seed {seed}: pre-alarm peak {before_alarm} exceeds 2600000 bit/s");
            }
            let (cap, floor) = match (loss_ppm, queue_us) {
                (10_000, 2_000) => (1_300_000, 600_000),
                (10_000, _) => (1_700_000, 900_000),
                (_, 2_000) => (1_400_000, 550_000),
                _ => (1_700_000, 550_000),
            };
            assert!(peak <= cap, "{loss_ppm} ppm {queue_us} µs policer seed {seed}: peak {peak} exceeds {cap} bit/s");
            assert!(mean >= floor, "{loss_ppm} ppm {queue_us} µs policer seed {seed}: mean {mean} below {floor} bit/s");
            if loss_ppm == 10_000 {
                assert!(max_level < 0.04, "1% loss {queue_us} µs policer seed {seed}: peak whole-second level {max_level} must be below 4% from 5 s");
                assert!(level < 0.025, "1% loss {queue_us} µs policer seed {seed}: 15 s level {level} must be below 2.5%");
            } else {
                assert!((0.07..=0.24).contains(&level), "15% loss {queue_us} µs policer seed {seed}: 20 s level {level} outside 7–24%");
            }
        }
    }

    #[test]
    fn a_policer_that_drops_from_four_megabits_limits_the_rate_again_by_fourteen_seconds() {
        let mut results = Vec::new();
        for seed in SEEDS {
            let mut call = Call::new(PathConfig { rate_bps: 4_000_000, queue_us: 12_000, loss_ppm: 10_000, ..Default::default() }, seed);
            call.policer_trace = Some(PolicerTrace::default());
            call.run_to(5, |_| {});
            let early_peak = call.window.peak;
            call.change_path_at(10_000_000, |path| path.rate_bps = 1_500_000);
            let ten = call.rate.estimate_bps();
            call.begin_window();
            let (fourteen, _) = call.run_to(14, |_| {});
            let recovery_peak = call.window.peak;
            call.begin_window();
            let mut tail = vec![fourteen];
            let (_, level) = call.run_to(20, |c| {
                if c.now_us % 1_000_000 == 0 {
                    tail.push(c.rate.estimate_bps());
                }
            });
            call.print_window("policer_drop", seed);
            let trace = call.policer_trace.unwrap();
            eprintln!("dropping policer seed {seed}: pre-alarm {}, first alarm to 8 s {:?}, 8–20 s {}, 14–20 s {}",
                trace.before_alarm_peak, trace.retest_peak, trace.late_peak, call.window.peak);
            if call.rate.events.reverted > 0 || call.window.peak > 2_000_000 || level >= 0.03 {
                eprintln!("dropping policer seed {seed} decisions through 5 s and later dips:\n  {}", trace.decisions.join("\n  "));
                eprintln!("dropping policer seed {seed} estimates each second from 14 to 20 s: {tail:?} bit/s");
            }
            eprintln!("4 to 1.5 Mbit/s seed {seed}: first 5 s peak {early_peak}, 10 s {ten}, 10–14 s peak {recovery_peak}, 14 s {fourteen}");
            results.push((seed, call.window.peak, level));
        }
        for (seed, peak, level) in results {
            assert!(peak <= 2_000_000, "4 to 1.5 Mbit/s policer seed {seed}: 14–20 s peak {peak} bit/s exceeds 2 Mbit/s");
            assert!(level < 0.03, "4 to 1.5 Mbit/s policer seed {seed}: 20 s level {level} must be below 3%");
        }
    }

    #[test]
    fn a_change_in_random_loss_replaces_the_level_and_restores_the_rate() {
        let mut results = Vec::new();
        for seed in SEEDS {
            let mut call = Call::new(PathConfig { loss_ppm: 10_000, ..Default::default() }, seed);
            call.app_bps = 4_000_000;
            call.change_path_at(6_000_000, |path| path.loss_ppm = 100_000);
            let six = call.rate.estimate_bps();
            call.begin_window();
            let (nine, _) = call.run_to(9, |_| {});
            let (_, level10) = call.run_to(10, |_| {});
            let (_, level12) = call.run_to(12, |_| {});
            let (_, level15) = call.run_to(15, |_| {});
            call.print_window("level_change", seed);
            eprintln!("level change seed {seed}: 6 s {six}, 9 s {nine}, minimum {} bit/s, levels at 10/12/15 s {level10:.8}/{level12:.8}/{level15:.8}", call.window.min);
            results.push((seed, six, nine, level15, call.window.min, call.rate.events.reverted));
        }
        for (seed, six, nine, level, low, reverted) in results {
            assert_eq!(reverted, 0, "1% to 10% seed {seed} must keep the learned level after a true level change, got {reverted} reverts");
            assert!((0.07..=0.13).contains(&level), "1% to 10% seed {seed}: 15 s level {level} outside 7–13%");
            assert!(nine >= six, "1% to 10% seed {seed}: 9 s estimate {nine} below the 6 s rate {six} bit/s");
            assert!(100 * low >= 45 * six, "1% to 10% seed {seed}: minimum {low} below 45% of {six} bit/s");
        }
    }

    #[test]
    fn a_two_hundred_millisecond_burst_recovers_its_rate_by_eight_seconds() {
        let mut results = Vec::new();
        for seed in SEEDS {
            let mut call = Call::new(PathConfig { loss_ppm: 10_000, ..Default::default() }, seed);
            call.app_bps = 4_000_000;
            call.change_path_at(6_000_000, |path| path.loss_ppm = 1_000_000);
            let six = call.rate.estimate_bps();
            call.begin_window();
            call.change_path_at(6_200_000, |path| path.loss_ppm = 10_000);
            let (eight, _) = call.run_to(8, |_| {});
            call.print_window("burst", seed);
            eprintln!("burst seed {seed}: 6 s {six}, 8 s {eight}, minimum {} bit/s", call.window.min);
            results.push((seed, six, eight));
        }
        for (seed, six, eight) in results {
            assert!(10 * eight >= 9 * six, "200 ms outage seed {seed}: 8 s estimate {eight} below 90% of the 6 s rate {six} bit/s");
        }
    }

    #[test]
    fn rate_growth_is_limited_by_what_the_application_sends() {
        let mut call = Call::new(PathConfig::default(), 7);
        call.app_bps = 200_000;
        while call.now_us < 6_000_000 {
            call.step();
            let bps = call.rate.estimate_bps();
            assert!(bps <= 300_000, "a 200 kbit/s application grew to {bps} bit/s at {} µs", call.now_us);
        }
        assert_eq!(call.path.stats.sent, 150, "the pacer should send exactly 200 kbit/s over six seconds");
        call.print_window("app_limited", 7);
    }
}
