//! A simulated one-way network path on a virtual clock, for tests: latency, jitter (which
//! reorders), random loss, and a bottleneck of a given rate whose queue drops from its tail
//! once it holds more than a given delay, as a router's does.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, Debug)]
pub struct PathConfig {
    pub latency_us: u64,
    /// Each datagram's extra delay is uniform in `0..jitter_us`.
    pub jitter_us: u64,
    /// Loss probability outside bad periods, in parts per million.
    pub loss_ppm: u32,
    /// Mean length of a good period in µs. Zero gives continuous bad loss when enabled.
    pub good_mean_us: u64,
    /// Mean length of a bad period in µs. Zero disables burst loss.
    pub bad_mean_us: u64,
    /// Loss probability in a bad period, in parts per million.
    pub burst_loss_ppm: u32,
    /// The bottleneck's rate; 0 for none.
    pub rate_bps: u64,
    /// The most delay the bottleneck's queue holds.
    pub queue_us: u64,
}

impl Default for PathConfig {
    fn default() -> Self {
        PathConfig { latency_us: 20_000, jitter_us: 0, loss_ppm: 0, good_mean_us: 0, bad_mean_us: 0,
            burst_loss_ppm: 0, rate_bps: 0, queue_us: 200_000 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathStats {
    pub sent: u64,
    pub lost: u64,
    pub queue_drops: u64,
    pub delivered: u64,
}

pub struct Path {
    pub config: PathConfig,
    rng: u64,
    // Whether the current period is bad and when it ends on the virtual clock.
    loss_period: Option<(bool, u128)>,
    busy_until: u64,
    order: u64,
    flight: BinaryHeap<Reverse<(u64, u64, Vec<u8>)>>,
    pub stats: PathStats,
}

impl Path {
    pub fn new(config: PathConfig, seed: u64) -> Path {
        Path { config, rng: seed | 1, loss_period: None, busy_until: 0, order: 0, flight: BinaryHeap::new(), stats: PathStats::default() }
    }

    fn random(&mut self) -> u64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn period_us(&mut self, mean: u64) -> u128 {
        // Inverse exponential CDF using a uniform survival probability in (0, 1].
        let survival = ((self.random() >> 11) + 1) as f64 / (1u64 << 53) as f64;
        (-(mean as f64) * survival.ln()).ceil().max(1.0) as u128
    }

    fn loss_at(&mut self, now: u64) -> u32 {
        let PathConfig { good_mean_us, bad_mean_us, loss_ppm, burst_loss_ppm, .. } = self.config;
        if bad_mean_us == 0 || good_mean_us == 0 {
            self.loss_period = None;
            return if bad_mean_us == 0 { loss_ppm } else { burst_loss_ppm };
        }
        let now = u128::from(now);
        // Start good on the first send. Advance from each deadline, including across idle gaps.
        let (mut bad, mut end) = self.loss_period.unwrap_or_else(|| (false, now + self.period_us(good_mean_us)));
        while now >= end {
            bad = !bad;
            end += self.period_us(if bad { bad_mean_us } else { good_mean_us });
        }
        self.loss_period = Some((bad, end));
        if bad { burst_loss_ppm } else { loss_ppm }
    }

    pub fn send(&mut self, now: u64, datagram: Vec<u8>) {
        self.stats.sent += 1;
        let loss_ppm = self.loss_at(now);
        if (self.random() % 1_000_000) < u64::from(loss_ppm) {
            self.stats.lost += 1;
            return;
        }
        let mut at = now;
        if self.config.rate_bps > 0 {
            let start = self.busy_until.max(now);
            if start - now > self.config.queue_us {
                self.stats.queue_drops += 1;
                return;
            }
            self.busy_until = start + datagram.len() as u64 * 8 * 1_000_000 / self.config.rate_bps;
            at = self.busy_until;
        }
        at += self.config.latency_us;
        if self.config.jitter_us > 0 {
            at += self.random() % self.config.jitter_us;
        }
        self.order += 1;
        self.flight.push(Reverse((at, self.order, datagram)));
    }

    /// Everything that has arrived by `now`, in arrival order.
    pub fn poll(&mut self, now: u64, out: &mut Vec<Vec<u8>>) {
        while self.flight.peek().is_some_and(|Reverse((at, _, _))| *at <= now) {
            let Reverse((_, _, d)) = self.flight.pop().unwrap();
            self.stats.delivered += 1;
            out.push(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_delays_loses_and_queues_as_configured() {
        let mut p = Path::new(PathConfig { latency_us: 10_000, loss_ppm: 100_000, ..Default::default() }, 7);
        for i in 0..10_000u64 {
            p.send(i, vec![0; 100]);
        }
        let mut out = Vec::new();
        p.poll(10_000 + 9_998, &mut out);
        assert_eq!(out.len() as u64, p.stats.delivered, "delivered counts what came out");
        assert!(p.stats.delivered < p.stats.sent - p.stats.lost, "the last datagrams are still in flight");
        assert!((800..1200).contains(&p.stats.lost), "about 10% lost: {}", p.stats.lost);

        let mut bursts = Path::new(PathConfig { good_mean_us: 1_000, bad_mean_us: 500,
            burst_loss_ppm: 1_000_000, ..Default::default() }, 7);
        for now in (0..20_000).step_by(100) {
            let lost = bursts.stats.lost;
            for _ in 0..3 {
                bursts.send(now, Vec::new());
            }
            let dropped = bursts.stats.lost - lost;
            assert!(dropped == 0 || dropped == 3, "all packets sent at {now} µs must share the loss state, lost {dropped}/3");
        }
        assert!(0 < bursts.stats.lost && bursts.stats.lost < bursts.stats.sent,
            "the path must visit both good and bad periods, lost {} of {}", bursts.stats.lost, bursts.stats.sent);

        // 1 Mbit/s: a 1,250-byte datagram takes 10 ms; a 50 ms queue holds five
        let mut q = Path::new(PathConfig { latency_us: 0, rate_bps: 1_000_000, queue_us: 50_000, ..Default::default() }, 1);
        for _ in 0..10 {
            q.send(0, vec![0; 1250]);
        }
        assert_eq!(q.stats.queue_drops, 4, "a burst beyond the queue is dropped at the tail");
        let mut out = Vec::new();
        q.poll(30_000, &mut out);
        assert_eq!(out.len(), 3, "the bottleneck's rate paces what comes out");
    }
}
