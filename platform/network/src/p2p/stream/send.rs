//! One outstanding queue holds both unsent and in-flight segments. A retransmission
//! changes the packet number and its RTT sample, while keeping the segment number.
use super::wire::{expand, Packet, ACK, SEGMENT};
use std::collections::{BTreeMap, VecDeque};

pub(super) const SLOTS: u16 = 128;
pub(super) const MAX_RTO: u64 = 8_000;
const INITIAL_WINDOW: usize = 10;

#[cfg_attr(test, derive(Clone, PartialEq))]
struct Segment {
    bytes: Vec<u8>, // empty for FIN
    packet: Option<u64>,
}

#[cfg_attr(test, derive(Clone, PartialEq))]
pub(super) struct Sender {
    queue: VecDeque<Segment>,
    base: u64,
    sent: u64, // exclusive high water mark, including during recovery
    pub written_bytes: u64,
    pub acked_bytes: u64,
    peer_edge: u64,
    samples: BTreeMap<u64, u64>,
    srtt: Option<u64>,
    variance: u64,
    pub rto: u64,
    timer: Option<u64>,
    cwnd: usize,
    threshold: usize,
    growth: usize,
    dupacks: usize,
    recover: Option<u64>,
    retry: bool,
    pub fin_acked: bool,
}

impl Sender {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            base: 0,
            sent: 0,
            written_bytes: 0,
            acked_bytes: 0,
            peer_edge: u64::from(SLOTS),
            samples: BTreeMap::new(),
            srtt: None,
            variance: 0,
            rto: 1000,
            timer: None,
            cwnd: INITIAL_WINDOW,
            threshold: usize::MAX,
            growth: 0,
            dupacks: 0,
            recover: None,
            retry: false,
            fin_acked: false,
        }
    }

    pub fn write(&mut self, mut bytes: &[u8]) -> usize {
        let size = bytes.len();
        if let Some(tail) = self.queue.back_mut() {
            if tail.packet.is_none() {
                let take = bytes.len().min(SEGMENT - tail.bytes.len());
                tail.bytes.extend_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
            }
        }
        for chunk in bytes.chunks(SEGMENT).take(self.cwnd.min(usize::from(SLOTS)).saturating_sub(self.queue.len())) {
            self.queue.push_back(Segment { bytes: chunk.to_vec(), packet: None });
            bytes = &bytes[chunk.len()..];
        }
        let accepted = size - bytes.len();
        self.written_bytes += accepted as u64;
        accepted
    }

    pub fn finish(&mut self) {
        self.queue.push_back(Segment { bytes: Vec::new(), packet: None });
    }

    fn flight(&self) -> usize {
        (self.sent - self.base) as usize
    }

    pub fn unsent_bytes(&self) -> usize {
        self.queue.iter().skip(self.flight()).map(|segment| segment.bytes.len()).sum()
    }

    fn can_send(&self) -> bool {
        self.flight() < self.queue.len() && self.flight() < self.cwnd && self.sent < self.peer_edge
    }

    pub fn blocked_on_credit(&self) -> bool {
        !self.queue.is_empty() && self.base == self.peer_edge
    }

    /// When the sender next has something to do. New data may go from `new_at` (now, unless
    /// a rate cap holds it); a repair or a due timer goes at once.
    pub fn wake(&self, now: u64, new_at: u64) -> Option<u64> {
        if self.retry {
            Some(now)
        } else if self.can_send() {
            Some(self.timer.map_or(new_at, |t| t.min(new_at)).max(now))
        } else {
            self.timer
        }
    }

    fn sample(&mut self, echo: u64, now: u64) -> bool {
        let Some(at) = self.samples.remove(&echo) else { return false };
        let rtt = now.saturating_sub(at).max(1);
        if let Some(srtt) = self.srtt {
            self.variance = (3 * self.variance + srtt.abs_diff(rtt)) / 4;
            self.srtt = Some((7 * srtt + rtt) / 8);
        } else {
            self.srtt = Some(rtt);
            self.variance = rtt / 2;
        }
        self.rto = (self.srtt.unwrap() + (4 * self.variance).max(1)).clamp(200, MAX_RTO);
        true
    }

    pub fn acknowledge(&mut self, packet: &Packet<'_>, now: u64) {
        let Some(ack) = expand(packet.ack, self.base) else { return };
        if ack < self.base || ack > self.sent {
            return;
        }
        let old_edge = self.peer_edge;
        self.peer_edge = self.peer_edge.max(ack + u64::from(packet.credit));
        let sampled = self.sample(packet.echo, now);
        if ack == self.base {
            // Count only distinct received data packets. Window updates, probes,
            // duplicate wire packets and old ACKs cannot trigger fast recovery.
            if sampled && packet.kind == ACK && old_edge == self.peer_edge && self.flight() > 0 {
                self.dupacks += 1;
                if self.recover.is_some() {
                    self.cwnd += 1;
                } else if self.dupacks == 3 {
                    self.threshold = (self.flight() / 2).max(2);
                    self.cwnd = self.threshold + 3;
                    self.recover = Some(self.sent);
                    self.retry = true;
                }
            }
            return;
        }
        let count = (ack - self.base) as usize;
        for seg in self.queue.drain(..count) {
            self.acked_bytes += seg.bytes.len() as u64;
            if let Some(pn) = seg.packet {
                self.samples.remove(&pn);
            }
            self.fin_acked |= seg.bytes.is_empty();
        }
        self.base = ack;
        self.timer = (self.sent > ack).then(|| now + self.rto);
        self.dupacks = 0;
        self.retry = false;
        match self.recover {
            Some(end) if ack < end => {
                // NewReno partial ACK: stay in recovery and repair the next hole.
                self.cwnd = self.cwnd.saturating_sub(count).max(1) + 1;
                self.retry = true;
            }
            Some(_) => {
                self.cwnd = self.threshold;
                self.recover = None;
                self.growth = 0;
            }
            None if self.cwnd < self.threshold => self.cwnd += 1,
            None => {
                self.growth += count;
                if self.growth >= self.cwnd {
                    self.growth -= self.cwnd;
                    self.cwnd += 1;
                }
            }
        }
    }

    /// The next segment to send: a timed-out or repaired one first, then new data when
    /// `new_ok` (a rate cap can hold new data back, never a repair).
    pub fn transmit(&mut self, now: u64, pn: u64, new_ok: bool) -> Option<(u64, &[u8])> {
        let seq = if self.timer.is_some_and(|at| at <= now) {
            self.threshold = (self.flight() / 2).max(2);
            self.cwnd = 1;
            self.growth = 0;
            self.dupacks = 0;
            self.recover = None;
            self.rto = (2 * self.rto).min(MAX_RTO);
            self.base
        } else if self.retry {
            self.base
        } else if new_ok && self.can_send() {
            self.sent += 1;
            self.sent - 1
        } else {
            return None;
        };
        self.retry = false;
        if self.timer.is_none() || seq == self.base {
            self.timer = Some(now + self.rto);
        }
        let seg = &mut self.queue[(seq - self.base) as usize];
        if let Some(old) = seg.packet.replace(pn) {
            self.samples.remove(&old);
        }
        self.samples.insert(pn, now);
        Some((seq, &seg.bytes))
    }
}
