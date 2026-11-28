//! A simulated one-way network path on a virtual clock, for tests: latency, jitter (which
//! reorders), random loss, and a bottleneck of a given rate whose queue drops from its tail
//! once it holds more than a given delay, as a router's does.

use crate::wire::Header;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

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

/// A test simulator. Pairing loss and jitter across runs costs one transmission-count entry
/// per distinct parsed packet for the life of the path. Unparsed sends share a single counter.
pub struct Path {
    pub config: PathConfig,
    seed: u64,
    transmissions: BTreeMap<(u16, u8, u8, u64), u64>,
    unparsed: u64,
    period_rng: u64,
    // Whether the current period is bad and when it ends on the virtual clock.
    loss_period: Option<(bool, u128)>,
    busy_until: u64,
    order: u64,
    flight: BinaryHeap<Reverse<(u64, u64, Vec<u8>)>>,
    pub stats: PathStats,
}

// SplitMix64's finalizer. Separate salts give loss and jitter their own draw from the same identity.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

const LOSS_SALT: u64 = 0x243f_6a88_85a3_08d3;
const JITTER_SALT: u64 = 0x1319_8a2e_0370_7344;

impl Path {
    pub fn new(config: PathConfig, seed: u64) -> Path {
        Path { config, seed: mix(seed), transmissions: BTreeMap::new(), unparsed: 0, period_rng: seed,
            loss_period: None, busy_until: 0, order: 0, flight: BinaryHeap::new(), stats: PathStats::default() }
    }

    fn packet_key(&mut self, datagram: &[u8]) -> u64 {
        let Some((h, _)) = Header::parse(datagram) else {
            let index = self.unparsed;
            self.unparsed += 1;
            // The extra bit separates the unparsed stream from every packed header prefix.
            return mix(mix(self.seed ^ (1 << 32)) ^ index);
        };
        let prefix = (u64::from(h.src) << 16) | (u64::from(h.stream) << 8) | u64::from(h.key_id);
        let packet = mix(mix(self.seed ^ prefix) ^ h.seq);
        let count = self.transmissions.entry((h.src, h.stream, h.key_id, h.seq)).or_default();
        let key = mix(packet ^ *count);
        *count += 1;
        key
    }

    fn period_us(&mut self, mean: u64) -> u128 {
        // Inverse exponential CDF using a uniform survival probability in (0, 1].
        self.period_rng = self.period_rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let survival = ((mix(self.period_rng) >> 11) + 1) as f64 / (1u64 << 53) as f64;
        (-(mean as f64) * survival.ln()).ceil().max(1.0) as u128
    }

    fn loss_at(&mut self, now: u64) -> u32 {
        let PathConfig { good_mean_us, bad_mean_us, loss_ppm, burst_loss_ppm, .. } = self.config;
        if bad_mean_us == 0 || good_mean_us == 0 {
            self.loss_period = None;
            return if bad_mean_us == 0 { loss_ppm } else { burst_loss_ppm };
        }
        let now = u128::from(now);
        // The first good period starts at virtual time zero, even if neither end sends yet.
        let (mut bad, mut end) = self.loss_period.unwrap_or_else(|| (false, self.period_us(good_mean_us)));
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
        let key = self.packet_key(&datagram);
        if mix(key ^ LOSS_SALT) % 1_000_000 < u64::from(loss_ppm) {
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
            at += mix(key ^ JITTER_SALT) % self.config.jitter_us;
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
    use crate::wire;

    fn datagram(src: u16, stream: u8, key_id: u8, seq: u64, hop: u16) -> Vec<u8> {
        let mut d = Vec::new();
        Header::begin(&mut d, hop, &Header::aad(src, stream, 0, key_id));
        d.extend_from_slice(&seq.to_be_bytes()[2..]);
        d.resize(wire::HEADER + wire::TAG, 0);
        d
    }

    // Take the only packet in flight so we can compare the exact arrival time, including jitter.
    fn arrival(path: &mut Path, now: u64, d: Vec<u8>) -> Option<u64> {
        path.send(now, d);
        path.flight.pop().map(|Reverse((at, _, _))| at)
    }

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

        let config = PathConfig { jitter_us: 800, loss_ppm: 100_000, good_mean_us: 1_000,
            bad_mean_us: 500, burst_loss_ppm: 900_000, ..Default::default() };
        let (mut quiet, mut busy) = (Path::new(config, 7), Path::new(config, 7));
        let mut paired_arrivals = 0;
        for seq in 0..256 {
            let now = seq * 1_000 + 750;
            for noise in [datagram(2, 1, 0, seq, 0), datagram(1, 2, 0, seq, 0),
                          datagram(1, 1, 1, seq, 0), Vec::new()] {
                arrival(&mut busy, now - 750, noise);
            }
            for attempt in 0..2 {
                let alone = arrival(&mut quiet, now, datagram(1, 1, 0, seq, 0));
                let interleaved = arrival(&mut busy, now, datagram(1, 1, 0, seq, 99));
                assert_eq!(alone, interleaved, "traffic changed loss or jitter for sequence {seq}, transmission {attempt} at {now} µs");
                paired_arrivals += usize::from(alone.is_some());
            }
        }

        let mut p = Path::new(PathConfig { loss_ppm: 100_000, ..Default::default() }, 7);
        let total = 10_000;
        let (mut first_lost, mut both_lost) = (0, 0);
        for seq in 0..total {
            let d = datagram(1, 1, 0, seq, 0);
            let first = arrival(&mut p, seq, d.clone());
            let second = arrival(&mut p, seq, d);
            first_lost += u64::from(first.is_none());
            both_lost += u64::from(first.is_none() && second.is_none());
        }
        let probability = f64::from(p.config.loss_ppm) / 1_000_000.0;
        let within_five_sigma = |lost: u64, n: u64| {
            let expected = n as f64 * probability;
            (lost as f64 - expected).abs() <= 5.0 * (expected * (1.0 - probability)).sqrt()
        };
        assert!(within_five_sigma(first_lost, total), "{first_lost}/{total} first sends lost, outside the 5σ binomial bound for {probability}");
        assert!(within_five_sigma(both_lost, first_lost), "{both_lost}/{first_lost} retries of lost packets also lost, outside the independent 5σ bound for {probability}");
        eprintln!("paired paths: {paired_arrivals}/512 arrived at identical times; first sends lost {first_lost}/{total}, retries of lost packets lost {both_lost}/{first_lost}");
    }
}
