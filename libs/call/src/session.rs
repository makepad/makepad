//! One participant's media with one other participant over one link, sans-IO.
//!
//! A [`Peer`] seals what this side sends (voice frames, video access units cut into fragments,
//! feedback), numbers every datagram it puts on the link (the hop), and opens what the other
//! side sends: voice and whole video frames come out as [`Event`]s. Feedback runs both ways on
//! its own stream: the arrival time of every hop, reported twice, so each side learns the
//! round trip and its loss without synchronised clocks; the sequences a video receiver is
//! missing, which the sender resends from its cache as the same bytes; and the streams a
//! receiver can't go on without a key frame of.
//!
//! Keys: each sender draws a secret per receiver and sends it over the encrypted stream (not
//! here); [`Peer::install_send_secret`] and [`Peer::install_recv_secret`] take it in. Every
//! stream's keys come from it by HKDF-SHA256 with the call id as salt and
//! `"makepad work v1 media\0" | sender's device key | stream id` as info, so no two streams,
//! senders or calls share a keystream. A secret and its sealers' counters live and die
//! together; a new secret goes in under a new key id.

use crate::rate::{LossEvents, RateEstimator};
use crate::video::{self, KeyGate, SendCache};
use crate::wire::{self, stream, stream_flags, Header, NackRange, Section, Voice, MAX_ARRIVALS, NOT_SEEN};
use makepad_network::p2p::seal::{OpenError, Opener, SealKeys, Sealer};
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::num::NonZeroU8;

/// The HKDF info label of every media key.
pub const MEDIA_INFO: &[u8] = b"makepad work v1 media\0";
const SRC_INFO: &[u8] = b"makepad work v1 call src\0";
/// Feedback cadence while media arrives, and while none does (the heartbeat).
pub const FEEDBACK_US: u64 = 50_000;
pub const IDLE_FEEDBACK_US: u64 = 100_000;
/// The round trip assumed before the first measurement.
pub const DEFAULT_RTT_US: u64 = 100_000;
/// The path's rate assumed before the first measurement.
pub const START_BPS: u64 = 300_000;
const PAD_GAIN: f64 = 1.25;
// Once delivery is confirmed, keep a silent path sampling at about three probes per 500 ms.
const MIN_PROBE_BPS: u64 = 64_000;
const PACE_FACTOR: f64 = 2.5;
const PACE_BURST_US: i128 = 5_000;
/// A late wake (a coarse system timer, a busy thread) still releases the credit that video
/// earned while it waited, up to this much time of it at once.
const PACE_CATCH_UP_US: u64 = 30_000;
/// How long this side remembers when it sent each hop (a report comes within a round trip).
const SENT_MEMORY_US: u64 = 2_000_000;
/// The streams a secret gives keys for.
const STREAMS: [u8; 6] = [stream::VOICE, stream::CAMERA_LOW, stream::CAMERA_HIGH, stream::SHARE, stream::APP_AUDIO, stream::FEEDBACK];

/// Wire cost in bit-microseconds, the same units as bit/s times elapsed µs.
fn pace_cost(bytes: usize) -> i128 {
    bytes as i128 * 8_000_000
}

/// A participant's tag in the datagrams of one call.
pub fn sender_tag(call_id: &[u8], device_pub: &[u8]) -> u16 {
    let mut m = Vec::with_capacity(SRC_INFO.len() + call_id.len() + device_pub.len());
    m.extend_from_slice(SRC_INFO);
    m.extend_from_slice(call_id);
    m.extend_from_slice(device_pub);
    let h = makepad_network::digest::sha256_hash(&m);
    u16::from_be_bytes([h[0], h[1]])
}

/// The keys of one sender's stream under one secret.
pub fn stream_keys(secret: &[u8], call_id: &[u8], sender_pub: &[u8], stream: u8) -> io::Result<SealKeys> {
    SealKeys::derive(secret, call_id, &[MEDIA_INFO, sender_pub, &[stream]])
}

/// What came in from the other side.
#[derive(Debug)]
pub enum Event {
    /// One voice frame, as sent (the caller decodes it and puts it in the jitter buffer).
    Voice { stream: u8, seq: u32, flags: u8, talking: bool, capture_us: u64, payload: Vec<u8> },
    /// One whole access unit that can be decoded now, in order.
    Video { stream: u8, frame: video::Frame },
}

/// Result of trying one link's keys on an incoming datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveOutcome {
    Opened,
    Replayed,
    Refused,
    BadTag,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerStats {
    pub sent: u64,
    pub sent_bytes: u64,
    pub resent: u64,
    /// Of the resends, those sent on arrival-report evidence.
    pub repairs: u64,
    pub feedback_sent: u64,
    /// Full-sized padding datagrams accepted by the outgoing link (included in feedback_sent).
    pub padding_sent: u64,
    pub received: u64,
    pub received_bytes: u64,
    /// Not a media datagram of this call: malformed, another sender's, or under no key we hold.
    pub refused: u64,
    pub replayed: u64,
    pub bad_tag: u64,
    /// This side's hops, as the other side's reports settled them (each hop is reported twice;
    /// the second report decides).
    pub hops_delivered: u64,
    pub hops_lost: u64,
    /// Complete share frames (and keys) released by the pacer, including the last fragment.
    pub share_frames_sent: u64,
    pub share_keys_sent: u64,
    /// Time between queueing the most recent share key and releasing its last fragment.
    pub last_share_key_pacing_us: u64,
    pub share_requests_sent: u64,
    pub share_requests_received: u64,
    /// Why each share request was sent.
    pub share_request_causes: ShareRequestCauses,
}

/// What made the viewer ask for a share key. A request for a picture already asked for and
/// not yet shown counts as a retry, whatever first asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShareRequestCauses {
    /// Watch opened: the first picture.
    pub opening: u64,
    /// The receiver's chain broke, or an advertised frame never came.
    pub gap: u64,
    /// The newest assembled frame didn't come out of the decoder in time.
    pub decode_late: u64,
    /// The decoder reported an error.
    pub decode_error: u64,
    pub retry: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestCause { Opening, Gap, DecodeLate, DecodeError }

struct SentHop {
    at: u64,
    len: usize,
    reports: u8,
    seen: bool,
    repair_queued: bool,
    /// Video stream, key id and sealed sequence. Nonzero video ids keep the optional tag to
    /// 16 bytes per hop on 64-bit targets for the two-second history.
    video: Option<(NonZeroU8, u8, u64)>,
}

struct PacedDatagram {
    queued_at: u64,
    bytes: Vec<u8>,
    /// Stream whose key frame ends with this datagram.
    key_end: Option<u8>,
    share_end: Option<(u8, u32, u64)>,
    // For a paced share resend: whether the sender's loss evidence triggered it.
    resend: Option<bool>,
}

#[derive(Default)]
struct OutStream {
    frame: u32,
    voice_seq: u32,
    key_gate: KeyGate,
    queued_keys: usize,
    /// A key frame went into this stream: the other side can decode what follows it.
    started: bool,
    /// Key id and sealed sequences of the newest complete key frame.
    newest_key: Option<(u8, std::ops::RangeInclusive<u64>)>,
    last_key_sent: Option<u64>,
    /// Per key id: what this stream sent under it lately.
    cache: BTreeMap<u8, SendCache>,
}

impl OutStream {
    /// Repair the newest key, or a tail after it that the receiver cannot yet name.
    fn repair(&mut self, id: u8, seq: u64, hop: u64, now: u64) -> Option<Vec<u8>> {
        let (key_id, key) = self.newest_key.as_ref()?;
        let cache = self.cache.get(&id)?;
        let key_packet = if id == *key_id {
            if seq < *key.start() {
                return None;
            }
            seq <= *key.end()
        } else {
            if cache.epoch < self.cache.get(key_id)?.epoch {
                return None;
            }
            false
        };
        let bytes = cache.repair(seq, hop, now, key_packet)?.to_vec();
        if key_packet {
            self.last_key_sent = Some(now);
            self.key_gate.withdraw();
        }
        Some(bytes)
    }
}

/// The arrival times of the other side's hops, until each was reported twice.
#[derive(Default)]
struct Arrivals {
    /// Unwrapped hop numbers (u16 on the wire, extended against the highest seen).
    highest: Option<u64>,
    times: BTreeMap<u64, u64>,
    /// The first hop the next report covers (where the report before last ended).
    floor: u64,
    /// Where the last report ended.
    last_end: u64,
    since_report: u64,
}

impl Arrivals {
    fn record(&mut self, hop: u16, now: u64) {
        let un = match self.highest {
            None => {
                let un = (1u64 << 32) + hop as u64;
                self.floor = un;
                self.last_end = un;
                un
            }
            Some(h) => (h as i64 + hop.wrapping_sub(h as u16) as i16 as i64) as u64,
        };
        if un < self.floor {
            return; // already reported twice
        }
        self.highest = Some(self.highest.map_or(un, |h| h.max(un)));
        self.times.entry(un).or_insert(now);
        self.since_report += 1;
    }

    /// ARRIVALS sections covering the floor to the highest hop; then moves the floor up.
    fn report(&mut self, now: u64, out: &mut Vec<Section>) {
        let Some(highest) = self.highest else { return };
        let report_time = (now / 100) as u32;
        let mut base = self.floor;
        while base <= highest {
            let end = (base + MAX_ARRIVALS as u64).min(highest + 1);
            let ages = (base..end)
                .map(|hop| match self.times.get(&hop) {
                    Some(&at) => (now.saturating_sub(at) / 100).min(NOT_SEEN as u64 - 1) as u16,
                    None => NOT_SEEN,
                })
                .collect();
            out.push(Section::Arrivals { base: base as u16, report_time, ages });
            base = end;
        }
        self.floor = self.last_end;
        self.last_end = highest + 1;
        let floor = self.floor;
        self.times.retain(|hop, _| *hop >= floor);
        self.since_report = 0;
    }
}

// Retry a single outstanding KEY on the media clock. On a slow path allow a round trip
// and the feedback interval for a key already on its way.
pub(crate) const SHARE_RETRY_US: u64 = 250_000;

/// The viewer's side of a watched share. A still window has no new frames and costs no keys:
/// the sharer repeats its HEAD, which is what reveals a last picture lost whole.
struct WatchedShare {
    pending: bool,
    /// What made `pending`, and whether a request for it has gone out.
    cause: RequestCause,
    asked: bool,
    last_request: Option<u64>,
    // Latest assembled frame and the last decoder progress toward it. A delayed codec
    // can return an older picture while retaining the final update to a still window.
    awaiting_decode: Option<(u32, u64)>,
    decoded: u64,
    errors: u64,
}

struct DeliveryInterval {
    start_us: u64,
    end_us: u64,
    bps: f64,
}

#[derive(Default)]
struct Padding {
    wanted: u64,
    // Receiver clock. After a stalled read, use complete fresh feedback intervals
    // for this attempt so that the accumulated burst cannot set the probe rate.
    last_report: Option<u32>,
    recovery: Option<DeliveryInterval>,
    credit: i128,
    hold_until: u64,
    hold_us: u64,
    // An estimate fall ends an attempt only after padding actually went on the link.
    sent_in_attempt: bool,
    blocked: bool,
}

impl Padding {
    fn observe_report(&mut self, report_time: u32, rate: &RateEstimator) {
        let last = self.last_report.get_or_insert(report_time);
        let end_us = u64::from(report_time) * 100;
        // Late sections can add arrivals to the current interval. Only a newer
        // receiver timestamp can advance it or start recovery.
        if report_time.wrapping_sub(*last) as i32 > 0 {
            let start_us = u64::from(*last) * 100;
            *last = report_time;
            if start_us > end_us {
                self.recovery = None; // The wire clock wrapped.
            } else if self.wanted > rate.estimate_bps() && end_us - start_us > IDLE_FEEDBACK_US
                && rate.has_delivery_gap(start_us, end_us, FEEDBACK_US) {
                // Even the idle heartbeat was late and arrivals stopped. Charge the
                // catch-up batch to the whole interval it could have taken to arrive.
                self.recovery = Some(DeliveryInterval { start_us, end_us, bps: 0.0 });
            } else if let Some(sample) = &mut self.recovery {
                if end_us - sample.end_us >= FEEDBACK_US {
                    sample.start_us = sample.end_us;
                    sample.end_us = end_us;
                }
            }
        }
        if let Some(sample) = &mut self.recovery {
            sample.bps = rate.reported_bps(sample.start_us, sample.end_us);
        }
    }
}

/// This side of a call with one other participant.
pub struct Peer {
    call_id: Vec<u8>,
    my_pub: Vec<u8>,
    their_pub: Vec<u8>,
    my_src: u16,
    their_src: u16,
    send_key: Option<(u8, BTreeMap<u8, Sealer>)>,
    recv_keys: BTreeMap<u8, BTreeMap<u8, Opener>>,
    /// Hops sent so far (the next hop's number, unwrapped).
    hops: u64,
    sent: VecDeque<SentHop>,
    out: BTreeMap<u8, OutStream>,
    in_video: BTreeMap<u8, video::Receiver>,
    arrivals: Arrivals,
    resends: Vec<(Vec<u8>, bool)>,
    next_feedback: u64,
    share_head: Option<(u8, u32, u64)>,
    last_voice: Option<(u64, usize)>,
    next_head: u64,
    receive_share: bool,
    watch: Option<WatchedShare>,
    rtt_us: Option<u64>,
    /// The other side's clock minus this side's, from the report sample with the least round
    /// trip (half of it taken as the way there), and that round trip.
    clock: Option<(u64, i64)>,
    /// What this side's path to the other carries, from the other side's reports.
    rate: RateEstimator,
    /// Sealed video in send order, with the time it entered the queue.
    paced: VecDeque<PacedDatagram>,
    share_paced: VecDeque<PacedDatagram>,
    // Repeated NACKs for a fragment still in the pacer need just one transmission.
    share_resends: std::collections::BTreeSet<(u8, u64)>,
    paced_bytes: usize,
    /// Voice, feedback and resends may spend credit below zero.
    pace_credit: i128,
    pace_at: u64,
    padding: Padding,
    plain: Vec<u8>,
    pub stats: PeerStats,
}

impl Peer {
    /// `my_pub` and `their_pub` are the device keys the link authenticated.
    pub fn new(call_id: &[u8], my_pub: &[u8], their_pub: &[u8]) -> Peer {
        Peer {
            call_id: call_id.to_vec(),
            my_pub: my_pub.to_vec(),
            their_pub: their_pub.to_vec(),
            my_src: sender_tag(call_id, my_pub),
            their_src: sender_tag(call_id, their_pub),
            send_key: None,
            recv_keys: BTreeMap::new(),
            hops: 0,
            sent: VecDeque::new(),
            out: BTreeMap::new(),
            in_video: BTreeMap::new(),
            arrivals: Arrivals::default(),
            resends: Vec::new(),
            next_feedback: 0,
            share_head: None,
            last_voice: None,
            next_head: 0,
            receive_share: true,
            watch: None,
            rtt_us: None,
            clock: None,
            rate: RateEstimator::new(START_BPS),
            paced: VecDeque::new(),
            share_paced: VecDeque::new(),
            share_resends: Default::default(),
            paced_bytes: 0,
            pace_credit: pace_cost(2 * wire::MAX_DATAGRAM),
            pace_at: 0,
            padding: Padding::default(),
            plain: Vec::new(),
            stats: PeerStats::default(),
        }
    }

    /// From now on this side seals under `secret`, as key `key_id`. The secret must be new: the
    /// sealers start their counters at zero.
    pub fn install_send_secret(&mut self, key_id: u8, secret: &[u8; 32]) -> io::Result<()> {
        let mut sealers = BTreeMap::new();
        for s in STREAMS {
            sealers.insert(s, Sealer::new(stream_keys(secret, &self.call_id, &self.my_pub, s)?));
        }
        self.send_key = Some((key_id, sealers));
        Ok(())
    }

    /// The other side's secret under `key_id`; older key ids stay until
    /// [`Peer::drop_recv_secret`] (packets sealed under them may still be on their way).
    pub fn install_recv_secret(&mut self, key_id: u8, secret: &[u8; 32]) -> io::Result<()> {
        let mut openers = BTreeMap::new();
        for s in STREAMS {
            let window = if stream::is_video(s) { wire::VIDEO_WINDOW } else { wire::VOICE_WINDOW };
            openers.insert(s, Opener::new(stream_keys(secret, &self.call_id, &self.their_pub, s)?, window));
        }
        self.recv_keys.insert(key_id, openers);
        Ok(())
    }

    pub fn drop_recv_secret(&mut self, key_id: u8) {
        self.recv_keys.remove(&key_id);
    }

    pub fn rtt_us(&self) -> Option<u64> {
        self.rtt_us
    }

    /// The other side's clock minus this side's, in µs: their capture times on this side's
    /// clock, for latency. Within half the round trip's asymmetry.
    pub fn clock_offset_us(&self) -> Option<i64> {
        self.clock.map(|(_, offset)| offset)
    }

    /// What this side's path to the other carries now, in bits per second (everything sent on
    /// it: media, feedback, resends).
    pub fn estimate_bps(&self) -> u64 {
        self.rate.estimate_bps()
    }

    /// Fill unused capacity up to the current estimate until this target is reached.
    /// Zero closes the subscription and forgets its retry interval.
    pub fn set_wanted(&mut self, bps: u64) {
        if self.padding.wanted != bps {
            if bps <= self.estimate_bps() {
                self.padding = Padding::default();
            }
            self.padding.wanted = bps;
        }
    }

    #[cfg(test)]
    pub(crate) fn wanted_bps(&self) -> u64 {
        self.padding.wanted
    }

    /// The random-loss fraction learned on this side's outgoing path.
    pub fn random_loss(&self) -> f64 {
        self.rate.random_loss()
    }

    /// The upper random-loss reading for video, once this path has a clean loss sample.
    pub fn video_loss_level(&self) -> Option<f64> {
        self.rate.video_loss_level()
    }

    /// Cumulative loss alarms, dip outcomes and suspect retests on this path.
    pub fn loss_events(&self) -> LossEvents {
        self.rate.loss_events()
    }

    /// The queue delay the estimator sees on this side's path, µs.
    pub fn queue_delay_us(&self) -> u64 {
        self.rate.queue_delay_us()
    }

    fn pace_bps(&self) -> u64 {
        if self.share_paced.is_empty() { (self.rate.estimate_bps() as f64 * PACE_FACTOR) as u64 }
        else { self.rate.estimate_bps() }
    }

    fn pace_burst(&self) -> i128 {
        (i128::from(self.pace_bps()) * PACE_BURST_US).max(pace_cost(2 * wire::MAX_DATAGRAM))
    }

    /// Total wire rate for an open padding attempt. Media gets first use of this credit.
    fn padding_bps(&self) -> u64 {
        let estimate = self.estimate_bps();
        if self.padding.wanted <= estimate || self.rate.delivered_bps() == 0.0 { return 0; }
        let delivered = self.padding.recovery.as_ref().map_or_else(|| self.rate.delivered_bps(), |sample| sample.bps);
        let paced = ((PAD_GAIN * delivered) as u64).max(MIN_PROBE_BPS);
        estimate.min(paced)
    }

    /// Credit earned while nothing waited is capped at the burst. While video waits, a wake
    /// later than its deadline keeps what the wait earned (up to `PACE_CATCH_UP_US`), so the
    /// pace holds whatever the caller's wake granularity.
    fn refill_pacer(&mut self, now: u64) {
        let padding_bps = self.padding_bps();
        if padding_bps > 0 {
            let elapsed = now.saturating_sub(self.pace_at.max(self.padding.hold_until));
            let earned = i128::from(padding_bps).saturating_mul(i128::from(elapsed));
            self.padding.credit = self.padding.credit.saturating_add(earned).min(pace_cost(2 * wire::MAX_DATAGRAM));
        }
        let pace = i128::from(self.pace_bps());
        let elapsed = i128::from(now.saturating_sub(self.pace_at));
        let cap = match self.next_paced() {
            None => self.pace_burst(),
            Some(head) => {
                let waited = now.saturating_sub(head.queued_at.max(self.pace_at)).min(PACE_CATCH_UP_US);
                (self.pace_burst() + pace * i128::from(waited)).max(self.pace_credit)
            }
        };
        self.pace_credit = self.pace_credit.saturating_add(pace.saturating_mul(elapsed)).min(cap);
        if let Some(head) = self.share_paced.front() {
            // A static window earns no burst against the next microphone frame.
            self.pace_credit = self.pace_credit.min(pace_cost(head.bytes.len()));
        }
        self.pace_at = self.pace_at.max(now);
    }

    /// The local time in µs when the next video or padding datagram has credit.
    pub fn next_send_us(&self) -> Option<u64> {
        let (missing, bps, at) = if let Some(head) = self.next_paced() {
            (pace_cost(head.bytes.len()) - self.pace_credit, self.pace_bps(), self.pace_at)
        } else {
            let bps = self.padding_bps();
            if bps == 0 || self.padding.blocked || self.send_key.is_none() { return None; }
            (pace_cost(wire::MAX_DATAGRAM) - self.padding.credit, bps, self.pace_at.max(self.padding.hold_until))
        };
        let wait = (missing.max(0) as u128).div_ceil(u128::from(bps));
        let due = at.saturating_add(u64::try_from(wait).unwrap_or(u64::MAX));
        Some(self.share_send_at(due))
    }

    /// How long the oldest queued video datagram has waited, in µs (zero if empty).
    pub fn pacer_delay_us(&self, now: u64) -> u64 {
        self.paced.front().into_iter().chain(self.share_paced.front()).map(|d| now.saturating_sub(d.queued_at)).max().unwrap_or(0)
    }

    /// Sealed bytes waiting for pacing credit.
    pub fn paced_bytes(&self) -> usize {
        self.paced_bytes
    }

    // Both stream queues spend the same credit. Voice is immediate, then share, then camera.
    fn next_paced(&self) -> Option<&PacedDatagram> {
        self.share_paced.front().or_else(|| self.paced.front())
    }

    /// Let an in-flight microphone frame finish, and leave the next 20 ms voice slot free.
    /// If the microphone stops, the reservation expires with that slot.
    fn share_send_at(&self, now: u64) -> u64 {
        let (Some(head), Some((voice_at, bytes))) = (self.share_paced.front(), self.last_voice) else { return now };
        let cost_us = |bytes: usize| (bytes as u64 * 8_000_000).div_ceil(self.rate.estimate_bps());
        let next_voice = voice_at.saturating_add(20_000);
        if now >= next_voice || cost_us(bytes) + cost_us(head.bytes.len()) > 20_000 {
            // At very low estimates there may be no whole slot. Ordinary credit still makes progress.
            return now;
        }
        let start = now.max(voice_at.saturating_add(cost_us(bytes)));
        if start.saturating_add(cost_us(head.bytes.len())) > next_voice { next_voice } else { start }
    }

    fn release_paced(&mut self, now: u64, out: &mut Vec<Vec<u8>>) {
        while self.next_paced().is_some_and(|d| pace_cost(d.bytes.len()) <= self.pace_credit) && self.share_send_at(now) <= now {
            let mut d = self.share_paced.pop_front().or_else(|| self.paced.pop_front()).unwrap();
            if let Some(stream) = d.key_end {
                let s = self.out.get_mut(&stream).unwrap();
                s.queued_keys -= 1;
                s.last_key_sent = Some(now);
            }
            if let Some(head) = d.share_end {
                // Publishing earlier would NACK fragments still waiting for pacing credit.
                self.share_head = Some(head);
                self.stats.share_frames_sent += 1;
                if d.key_end.is_some() {
                    self.stats.share_keys_sent += 1;
                    self.stats.last_share_key_pacing_us = now.saturating_sub(d.queued_at);
                }
            }
            if let Some(repair) = d.resend {
                let (h, _) = Header::parse(&d.bytes).unwrap();
                self.share_resends.remove(&(h.key_id, h.seq));
                self.stats.resent += 1;
                self.stats.repairs += u64::from(repair);
            }
            self.paced_bytes -= d.bytes.len();
            self.stamp(&mut d.bytes, now);
            out.push(d.bytes);
        }
        if self.next_paced().is_none() {
            // a catch-up left over once the queue drained is idle credit
            self.pace_credit = self.pace_credit.min(self.pace_burst());
        }
    }

    fn seal(&mut self, stream_id: u8, flags: u8, plaintext: &[u8]) -> io::Result<(u8, u64, Vec<u8>)> {
        let (key_id, sealers) = self.send_key.as_mut().ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no send secret"))?;
        let sealer = sealers.get_mut(&stream_id).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no such stream"))?;
        let aad = Header::aad(self.my_src, stream_id, flags, *key_id);
        let mut d = Vec::with_capacity(wire::HEADER + plaintext.len() + wire::TAG);
        Header::begin(&mut d, 0, &aad);
        let seq = sealer.seal(&aad, plaintext, &mut d)?;
        Ok((*key_id, seq, d))
    }

    /// After refilling for this send time, charge each datagram and record its hop.
    fn stamp(&mut self, d: &mut [u8], now: u64) {
        let hop = self.hops;
        Header::set_hop(d, hop as u16);
        let video = Header::parse(d).and_then(|(h, _)| {
            let id = NonZeroU8::new(h.stream)?;
            stream::is_video(id.get()).then_some((id, h.key_id, h.seq))
        });
        if let Some((stream, key_id, seq)) = video {
            if let Some(cache) = self.out.get_mut(&stream.get()).and_then(|s| s.cache.get_mut(&key_id)) {
                cache.stamped(seq, hop);
            }
        }
        self.record_sent(now, d.len(), video);
    }

    fn record_sent(&mut self, now: u64, len: usize, video: Option<(NonZeroU8, u8, u64)>) {
        self.pace_credit -= pace_cost(len);
        if self.padding_bps() > 0 && now >= self.padding.hold_until {
            self.padding.credit -= pace_cost(len);
        }
        self.hops += 1;
        self.sent.push_back(SentHop { at: now, len, reports: 0, seen: false, repair_queued: false, video });
        while self.sent.front().is_some_and(|s| now.saturating_sub(s.at) > SENT_MEMORY_US) {
            self.sent.pop_front();
        }
        self.stats.sent += 1;
        self.stats.sent_bytes += len as u64;
    }

    /// One voice frame to send now.
    pub fn send_voice(&mut self, now: u64, stream_id: u8, voice: &Voice, talking: bool) -> io::Result<Vec<u8>> {
        if !stream::is_audio(stream_id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an audio stream"));
        }
        let mut plain = std::mem::take(&mut self.plain);
        voice.write(&mut plain);
        let flags = if talking { stream_flags::TALKING } else { 0 };
        let sealed = self.seal(stream_id, flags, &plain);
        self.plain = plain;
        let (_, _, mut d) = sealed?;
        if stream_id == stream::VOICE { self.last_voice = Some((now, d.len())); }
        self.refill_pacer(now);
        self.stamp(&mut d, now);
        Ok(d)
    }

    /// The next voice frame's sequence on this link (the caller's frames may skip none).
    pub fn next_voice_seq(&mut self) -> u32 {
        let s = self.out.entry(stream::VOICE).or_default();
        let seq = s.voice_seq;
        s.voice_seq = seq.wrapping_add(1);
        seq
    }

    /// A coalesced key request, due on this stream's recovery interval.
    pub fn key_frame_due(&mut self, stream_id: u8, now: u64) -> bool {
        let gate = &mut self.out.entry(stream_id).or_default().key_gate;
        if stream_id == stream::SHARE { gate.take_after(now, SHARE_RETRY_US) } else { gate.take(now) }
    }

    pub(crate) fn receive_share(&mut self, watching: bool) {
        self.receive_share = watching;
        if watching {
            self.watch.get_or_insert(WatchedShare { pending: true, cause: RequestCause::Opening, asked: false,
                last_request: None, awaiting_decode: None, decoded: 0, errors: 0 });
        } else {
            self.watch = None;
            self.in_video.remove(&stream::SHARE);
        }
    }

    pub(crate) fn share_queued(&self) -> bool { !self.share_paced.is_empty() }

    pub(crate) fn decoded_share(&mut self, now: u64, decoded: u64, frame: u32, errors: u64) {
        let Some(watch) = &mut self.watch else { return };
        if decoded != watch.decoded {
            watch.decoded = decoded;
            watch.awaiting_decode = watch.awaiting_decode.and_then(|(latest, _)| (latest != frame).then_some((latest, now)));
            watch.pending = false;
        }
        if errors != watch.errors {
            watch.errors = errors;
            watch.pending = true;
            (watch.cause, watch.asked) = (RequestCause::DecodeError, false);
        }
    }

    /// Close just this subscription. Frame numbers and the sealer's sequence continue on reopen.
    pub(crate) fn stop_share(&mut self) {
        self.set_wanted(0);
        self.share_head = None;
        self.share_resends.clear();
        self.paced_bytes -= self.share_paced.drain(..).map(|p| p.bytes.len()).sum::<usize>();
        self.resends.retain(|(d, _)| d[5] & 15 != stream::SHARE);
        if let Some(s) = self.out.get_mut(&stream::SHARE) {
            *s = OutStream { frame: s.frame, ..Default::default() };
        }
    }

    /// Queues one encoded access unit and puts the datagrams already due in `out`.
    pub fn send_video(&mut self, now: u64, stream_id: u8, au: &[u8], key: bool, capture_us: u64, out: &mut Vec<Vec<u8>>) -> io::Result<()> {
        let s = self.out.entry(stream_id).or_default();
        if !key && !s.started {
            // Nothing before a key frame can be decoded over there (someone who joins a running
            // call): send them none of it and ask the encoder for a key frame.
            s.key_gate.request();
            return Ok(());
        }
        s.started = true;
        let frame = s.frame;
        s.frame = frame.wrapping_add(1);
        if key {
            s.key_gate.made_key(now);
        }
        let mut plain = std::mem::take(&mut self.plain);
        let mut result = Ok(());
        // On a thin path a full datagram can occupy the wire for longer than a voice frame.
        // Interleave share fragments at half the 20 ms voice period. Large frames still fit
        // the wire's fragment count; camera packetisation is unchanged.
        let size = if stream_id == stream::SHARE {
            let wire_bytes = self.rate.estimate_bps() / 800; // ten milliseconds
            (wire_bytes as usize).saturating_sub(wire::MAX_DATAGRAM - wire::MAX_FRAGMENT)
                .clamp(1, wire::MAX_FRAGMENT).max(au.len().div_ceil(u16::MAX as usize).min(wire::MAX_FRAGMENT))
        } else { wire::MAX_FRAGMENT };
        let parts: Vec<_> = video::fragments_at(au, frame, if key { 0 } else { 1 }, key, capture_us, size).collect();
        let last = parts.len() - 1;
        for (i, f) in parts.iter().enumerate() {
            f.write(&mut plain);
            let mut flags = 0;
            if key && i == 0 {
                flags |= stream_flags::KEY_START;
            }
            if i == last {
                flags |= stream_flags::FRAME_END;
            }
            match self.seal(stream_id, flags, &plain) {
                Ok((key_id, seq, d)) => {
                    let s = self.out.get_mut(&stream_id).unwrap();
                    let epoch = s.cache.len();
                    s.cache.entry(key_id).or_insert_with(|| SendCache::new(epoch)).push(seq, now, d.clone());
                    self.paced_bytes += d.len();
                    let key_end = key && i == last;
                    if key_end {
                        s.newest_key = Some((key_id, seq - i as u64..=seq));
                    }
                    s.queued_keys += usize::from(key_end);
                    let share_end = (stream_id == stream::SHARE && i == last).then_some((key_id, frame, seq + 1));
                    let queue = if stream_id == stream::SHARE { &mut self.share_paced } else { &mut self.paced };
                    queue.push_back(PacedDatagram { queued_at: now, bytes: d, key_end: key_end.then_some(stream_id), share_end, resend: None });
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.plain = plain;
        self.refill_pacer(now);
        self.release_paced(now, out);
        result
    }

    /// One datagram from the other side.
    pub fn receive(&mut self, now: u64, datagram: &[u8], events: &mut Vec<Event>) {
        match self.try_receive(now, datagram, events) {
            ReceiveOutcome::Opened | ReceiveOutcome::Replayed => {}
            ReceiveOutcome::Refused => self.note_refused(),
            ReceiveOutcome::BadTag => self.note_bad_tag(),
        }
    }

    pub fn note_bad_tag(&mut self) { self.stats.bad_tag += 1; }

    pub fn note_refused(&mut self) { self.stats.refused += 1; }

    /// Only authenticated datagrams affect this peer while routing is undecided.
    pub fn try_receive(&mut self, now: u64, datagram: &[u8], events: &mut Vec<Event>) -> ReceiveOutcome {
        let Some((h, sealed)) = Header::parse(datagram) else {
            return ReceiveOutcome::Refused;
        };
        let Some(opener) = (h.src == self.their_src).then(|| self.recv_keys.get_mut(&h.key_id).and_then(|o| o.get_mut(&h.stream))).flatten() else {
            return ReceiveOutcome::Refused;
        };
        let mut plain = std::mem::take(&mut self.plain);
        let outcome = match opener.open(&datagram[wire::AAD], sealed, &mut plain) {
            Ok(seq) => {
                self.stats.received += 1;
                self.stats.received_bytes += datagram.len() as u64;
                self.arrivals.record(h.hop, now);
                self.take(now, &h, seq, &plain, events);
                ReceiveOutcome::Opened
            }
            Err(OpenError::Replayed) => {
                self.stats.replayed += 1;
                ReceiveOutcome::Replayed
            }
            Err(OpenError::BadTag | OpenError::Short) => ReceiveOutcome::BadTag,
        };
        self.plain = plain;
        outcome
    }

    fn take(&mut self, now: u64, h: &Header, seq: u64, plain: &[u8], events: &mut Vec<Event>) {
        match h.stream {
            s if stream::is_audio(s) => {
                if let Some(v) = Voice::read(plain) {
                    events.push(Event::Voice {
                        stream: s,
                        seq: v.seq,
                        flags: v.flags,
                        talking: h.flags & stream_flags::TALKING != 0,
                        capture_us: v.capture_us,
                        payload: v.payload.to_vec(),
                    });
                }
            }
            s if stream::is_video(s) && (s != stream::SHARE || self.receive_share) => {
                if let Some(f) = wire::Fragment::read(plain) {
                    let mut frames = Vec::new();
                    let src = self.their_src;
                    self.in_video.entry(s).or_insert_with(|| video::Receiver::new(src, s)).take(h.key_id, seq, &f, now, &mut frames);
                    if s == stream::SHARE {
                        if let (Some(last), Some(watch)) = (frames.last(), &mut self.watch) {
                            let since = watch.awaiting_decode.map_or(now, |(_, at)| at);
                            watch.awaiting_decode = Some((last.frame, since));
                        }
                    }
                    events.extend(frames.into_iter().map(|frame| Event::Video { stream: s, frame }));
                }
            }
            stream::FEEDBACK => {
                if let Some(sections) = wire::read_feedback(plain) {
                    for section in sections {
                        self.feedback(now, section);
                    }
                }
            }
            _ => {}
        }
    }

    fn feedback(&mut self, now: u64, section: Section) {
        match section {
            Section::Arrivals { base, report_time, ages } => {
                // the report's base, extended against the hops this side has sent
                let behind = (self.hops as u16).wrapping_sub(base) as u64;
                let Some(base) = self.hops.checked_sub(behind) else { return };
                // Credit up to this report was earned at the previous delivery rate and estimate.
                self.refill_pacer(now);
                let first = self.hops - self.sent.len() as u64;
                let mut rtt: Option<u64> = None;
                let mut best: Option<(u64, i64)> = None;
                let (mut lost, mut delivered) = (0u64, 0u64);
                let mut latest_seen_at: Option<u64> = None;
                for (i, age) in ages.iter().copied().enumerate() {
                    let hop = base + i as u64;
                    let Some(rec) = hop.checked_sub(first).and_then(|i| self.sent.get_mut(i as usize)) else { continue };
                    rec.reports = rec.reports.saturating_add(1);
                    if age != NOT_SEEN {
                        latest_seen_at = Some(latest_seen_at.map_or(rec.at, |at| at.max(rec.at)));
                        let arrived = (report_time as i64 - age as i64) * 100;
                        if !rec.seen {
                            // in send order, the first time each is seen
                            self.rate.on_arrival(rec.at, arrived.max(0) as u64, rec.len);
                            if let Some((stream, key_id, seq)) = rec.video {
                                if let Some(cache) = self.out.get_mut(&stream.get()).and_then(|s| s.cache.get_mut(&key_id)) {
                                    cache.mark_seen(seq);
                                }
                            }
                        }
                        rec.seen = true;
                        let sample = now.saturating_sub(rec.at).saturating_sub(age as u64 * 100);
                        rtt = Some(rtt.map_or(sample, |r| r.min(sample)));
                        if best.is_none_or(|(r, _)| sample < r) {
                            best = Some((sample, arrived - rec.at as i64 - (sample / 2) as i64));
                        }
                    }
                    if rec.reports == 2 {
                        if rec.seen {
                            delivered += 1;
                        } else {
                            lost += 1;
                        }
                    }
                }
                // A later stamp seen in this report gives an earlier absent hop time to
                // reorder. Finish marking receipt across the report before testing tails.
                if let Some(cutoff) = latest_seen_at.and_then(|at| at.checked_sub(video::REORDER_US)) {
                    for (i, age) in ages.into_iter().enumerate() {
                        if age != NOT_SEEN {
                            continue;
                        }
                        let hop = base + i as u64;
                        let Some(rec) = hop.checked_sub(first).and_then(|i| self.sent.get_mut(i as usize)) else { continue };
                        if rec.seen || rec.repair_queued || rec.at > cutoff {
                            continue;
                        }
                        let Some((s, id, seq)) = rec.video else { continue };
                        let Some(stream) = self.out.get_mut(&s.get()) else { continue };
                        if let Some(bytes) = stream.repair(id, seq, hop, now) {
                            self.resends.push((bytes, true));
                            rec.repair_queued = true;
                        }
                    }
                }
                self.stats.hops_delivered += delivered;
                self.stats.hops_lost += lost;
                self.rate.on_settled(lost, delivered);
                let (_, sent_bytes) = self.sent_since(now, 1_000_000);
                let rtt_now = rtt.or(self.rtt_us).unwrap_or(DEFAULT_RTT_US);
                let before = self.estimate_bps();
                let after = self.rate.update(now, rtt_now, sent_bytes as u64 * 8);
                if self.padding.wanted != 0 {
                    self.padding.observe_report(report_time, &self.rate);
                    if after >= self.padding.wanted {
                        self.padding.recovery = None;
                        self.padding.hold_us = 0;
                        self.padding.hold_until = 0;
                        self.padding.sent_in_attempt = false;
                        self.padding.credit = 0;
                    } else if after < before && self.padding.sent_in_attempt && !self.rate.in_startup() {
                        self.padding.recovery = None;
                        self.padding.hold_us = (2 * self.padding.hold_us).clamp(10_000_000, 60_000_000);
                        self.padding.hold_until = now.saturating_add(self.padding.hold_us);
                        self.padding.sent_in_attempt = false;
                        self.padding.credit = 0;
                    }
                }
                if let Some(sample) = rtt {
                    self.rtt_us = Some(self.rtt_us.map_or(sample, |r| (7 * r + sample) / 8));
                }
                if let Some((r, offset)) = best {
                    match &mut self.clock {
                        // the floor creeps up 0.1 ms a report, so a changed path takes over
                        Some((floor, o)) => {
                            *floor += 100;
                            if r <= *floor {
                                (*floor, *o) = (r, offset);
                            }
                        }
                        None => self.clock = Some((r, offset)),
                    }
                }
            }
            Section::Nack(ranges) => {
                for NackRange { src, stream: s, key_id, first, count } in ranges {
                    if src != self.my_src {
                        continue;
                    }
                    let Some(stream) = self.out.get_mut(&s) else { continue };
                    let Some(cache) = stream.cache.get(&key_id) else { continue };
                    for d in cache.find(first, count) {
                        if stream.newest_key.as_ref().is_some_and(|(id, seqs)| {
                            *id == key_id && Header::parse(d).is_some_and(|(h, _)| seqs.contains(&h.seq))
                        }) {
                            // the receiver is recovering this key frame by resends: an ask
                            // made before it knew of it is answered by them
                            stream.last_key_sent = Some(now);
                            stream.key_gate.withdraw();
                        }
                        self.resends.push((d.to_vec(), false));
                    }
                }
            }
            Section::Head { stream: s, key_id, frame, next } => {
                if s == stream::SHARE && self.receive_share && self.recv_keys.contains_key(&key_id) {
                    self.in_video.entry(s).or_insert_with(|| video::Receiver::new(self.their_src, s)).head(now, key_id, frame, next);
                }
            }
            Section::Key(keys) => {
                let grace = self.rtt_us.unwrap_or(DEFAULT_RTT_US).saturating_add(FEEDBACK_US);
                for (src, s) in keys {
                    if src == self.my_src {
                        if s == stream::SHARE { self.stats.share_requests_received += 1; }
                        let stream = self.out.entry(s).or_default();
                        if stream.queued_keys == 0 && stream.last_key_sent.is_none_or(|at| now.saturating_sub(at) >= grace) {
                            let repairing_key = stream.newest_key.as_ref().is_some_and(|(id, key)| {
                                stream.cache.get(id).is_some_and(|cache| cache.key_awaiting_receipt(key, now))
                            });
                            if s == stream::SHARE || !repairing_key {
                                stream.key_gate.request();
                            }
                        }
                    }
                }
            }
        }
    }

    /// Releases requested resends, due video, then feedback when due or a receiver has gaps.
    pub fn poll(&mut self, now: u64, out: &mut Vec<Vec<u8>>) {
        self.refill_pacer(now);
        let mut resends = std::mem::take(&mut self.resends);
        for (mut bytes, repair) in resends.drain(..) {
            if bytes[5] & 15 == stream::SHARE {
                let (h, _) = Header::parse(&bytes).unwrap();
                if !self.share_resends.insert((h.key_id, h.seq)) { continue; }
                // A burst of NACKs for a lost key has the same cost to voice as the original.
                self.paced_bytes += bytes.len();
                self.share_paced.push_back(PacedDatagram { queued_at: now, bytes, key_end: None, share_end: None, resend: Some(repair) });
            } else {
                self.stamp(&mut bytes, now);
                out.push(bytes);
                self.stats.resent += 1;
                self.stats.repairs += u64::from(repair);
            }
        }
        self.resends = resends;
        self.release_paced(now, out);
        let rtt = self.rtt_us.unwrap_or(DEFAULT_RTT_US);
        let mut wants = video::Wants::default();
        let (mut share_gap, mut key_arriving) = (false, false);
        for (&s, r) in &mut self.in_video {
            if s == stream::SHARE && self.watch.is_some() {
                let mut repair = video::Wants::default();
                r.poll(now, rtt, &mut repair);
                share_gap = repair.key;
                key_arriving = r.key_arriving(now, rtt);
                wants.nacks.extend(repair.nacks);
            } else {
                r.poll(now, rtt, &mut wants);
            }
        }
        let retry = SHARE_RETRY_US.max(rtt.saturating_add(FEEDBACK_US));
        let request_share = self.watch.as_mut().is_some_and(|w| {
            let late = w.awaiting_decode.is_some_and(|(_, at)| now.saturating_sub(at) >= retry);
            if !w.pending && (share_gap || late) {
                w.pending = true;
                w.asked = false;
                w.cause = if share_gap { RequestCause::Gap } else { RequestCause::DecodeLate };
            }
            // A retry waits while the key asked for is still arriving.
            w.pending && !(w.asked && key_arriving) && w.last_request.is_none_or(|at| now.saturating_sub(at) >= retry)
        });
        let due = now >= self.next_feedback;
        if (!due && wants.nacks.is_empty() && !request_share) || self.send_key.is_none() {
            return;
        }
        let mut sections = Vec::new();
        let active = self.arrivals.since_report > 0;
        self.arrivals.report(now, &mut sections);
        if !wants.nacks.is_empty() {
            sections.push(Section::Nack(wants.nacks));
        }
        if due {
            if let Some((key_id, frame, next)) = self.share_head.filter(|_| now >= self.next_head) {
                sections.push(Section::Head { stream: stream::SHARE, key_id, frame, next });
                self.next_head = now + IDLE_FEEDBACK_US;
            }
            self.next_feedback = now + if active { FEEDBACK_US } else { IDLE_FEEDBACK_US };
        }
        if request_share || (due && wants.key) {
            let mut keys: Vec<_> = self.in_video.iter().filter(|(s, r)|
                due && wants.key && (**s != stream::SHARE || self.watch.is_none()) && r.wants_key())
                .map(|(&s, _)| (self.their_src, s)).collect();
            if request_share {
                let watch = self.watch.as_mut().unwrap();
                watch.last_request = Some(now);
                self.stats.share_requests_sent += 1;
                let causes = &mut self.stats.share_request_causes;
                *match (watch.asked, watch.cause) {
                    (true, _) => &mut causes.retry,
                    (false, RequestCause::Opening) => &mut causes.opening,
                    (false, RequestCause::Gap) => &mut causes.gap,
                    (false, RequestCause::DecodeLate) => &mut causes.decode_late,
                    (false, RequestCause::DecodeError) => &mut causes.decode_error,
                } += 1;
                watch.asked = true;
                keys.push((self.their_src, stream::SHARE));
            }
            sections.push(Section::Key(keys));
        }
        // the first section carries NACKs and KEYs with the first arrivals; more arrivals
        // than one datagram holds go in datagrams of their own
        let (mut first, mut rest): (Vec<Section>, Vec<Section>) = (Vec::new(), Vec::new());
        for s in sections {
            match s {
                Section::Arrivals { .. } if first.iter().any(|f| matches!(f, Section::Arrivals { .. })) => rest.push(s),
                _ => first.push(s),
            }
        }
        let mut plain = std::mem::take(&mut self.plain);
        for group in std::iter::once(first).chain(rest.into_iter().map(|s| vec![s])) {
            wire::write_feedback(&group, &mut plain);
            if plain.len() <= wire::MAX_PLAINTEXT {
                self.send_report(now, &plain, out);
            } else {
                // HEAD and long NACK lists can exceed the room beside a full arrival report.
                // Split on section/range boundaries; each datagram parses on its own.
                for section in group {
                    if let Section::Nack(ranges) = section {
                        for chunk in ranges.chunks((wire::MAX_PLAINTEXT - 4) / 11) {
                            wire::write_feedback(&[Section::Nack(chunk.to_vec())], &mut plain);
                            self.send_report(now, &plain, out);
                        }
                    } else {
                        wire::write_feedback(&[section], &mut plain);
                        self.send_report(now, &plain, out);
                    }
                }
            }
        }
        self.plain = plain;
    }

    fn send_report(&mut self, now: u64, plain: &[u8], out: &mut Vec<Vec<u8>>) {
        if let Ok((_, _, mut d)) = self.seal(stream::FEEDBACK, 0, plain) {
            self.stamp(&mut d, now);
            self.stats.feedback_sent += 1;
            out.push(d);
        }
    }

    /// Report whether the link accepted its pending media and feedback. A full link
    /// wakes for normal traffic again before it tries optional padding.
    pub fn set_padding_blocked(&mut self, blocked: bool) {
        self.padding.blocked = blocked;
    }

    /// After poll and after draining the link's backlog, spend any remaining wire
    /// credit directly on the link. A refused datagram never consumes a hop.
    pub fn send_padding(&mut self, now: u64, mut try_send: impl FnMut(Vec<u8>) -> bool) {
        if self.padding_bps() == 0 || self.padding.blocked
            || now < self.padding.hold_until || self.next_paced().is_some() || self.send_key.is_none() {
            return;
        }
        self.refill_pacer(now);
        while self.padding.credit >= pace_cost(wire::MAX_DATAGRAM) {
            let Ok((_, _, mut d)) = self.seal(stream::FEEDBACK, 0, &wire::PADDING_PLAINTEXT) else { break };
            Header::set_hop(&mut d, self.hops as u16);
            if !try_send(d) {
                self.padding.blocked = true;
                break;
            }
            self.record_sent(now, wire::MAX_DATAGRAM, None);
            self.stats.feedback_sent += 1;
            self.stats.padding_sent += 1;
            self.padding.sent_in_attempt = true;
        }
    }

    /// Return a decoded access unit's storage to the receiver that assembled it.
    pub(crate) fn recycle_video(&mut self, stream: u8, data: Vec<u8>) {
        if let Some(receiver) = self.in_video.get_mut(&stream) {
            receiver.recycle(data);
        }
    }

    /// The video receivers' statistics, by stream.
    pub fn video_stats(&self) -> impl Iterator<Item = (u8, video::ReceiverStats)> + '_ {
        self.in_video.iter().map(|(s, r)| (*s, r.stats))
    }

    /// This side's datagrams sent in the last `window_us` and their bytes (a rate over the
    /// window the caller picks).
    pub fn sent_since(&self, now: u64, window_us: u64) -> (usize, usize) {
        self.sent.iter().rev().take_while(|s| now.saturating_sub(s.at) <= window_us).fold((0, 0), |(n, b), s| (n + 1, b + s.len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::{video_ask, Guard, Rung, Tick, LADDER};
    use crate::sim::{Path, PathConfig};

    const CALL: &[u8] = b"a test call";
    const VOICE_INTERVAL_US: u64 = 20_000;

    fn device(n: u8) -> [u8; 65] {
        let mut k = [4u8; 65];
        k[1] = n;
        k
    }

    struct VideoAttempts {
        original_dropped: bool,
        resent: bool,
    }

    struct Rig {
        a: Peer,
        b: Peer,
        ab: Path,
        ba: Path,
        now: u64,
        voice: bool,
        voice_sent: u32,
        video_sent: u32,
        voice_got: Vec<u32>,
        last_voice_at: Option<u64>,
        max_voice_gap: u64,
        expected_share: Option<Vec<u8>>,
        receiver_paused: bool,
        // Drain time of the bytes the forward bottleneck accepted and its peak wire backlog.
        router_empty_at: u64,
        peak_router_us: u64,
        video_got: Vec<(u32, bool)>,
        shown_at: Vec<u64>,
        keys_made: Vec<(u32, u64)>,
        /// While true, a's datagrams to b are lost.
        outage: bool,
        reverse_outage: bool,
        trace_resends: bool,
        video_attempts: Option<BTreeMap<(u8, u8, u64), VideoAttempts>>,
        drop_once: std::collections::BTreeSet<(u8, u64)>,
        last_drop_us: Option<u64>,
        /// a sizes its pictures from its rate estimate, as an encoder following it would.
        adaptive: bool,
        fixed_picture_bytes: Option<usize>,
        sizes: Vec<usize>,
    }

    fn au(frame: u32, size: usize) -> Vec<u8> {
        (0..size).map(|i| (i as u32).wrapping_mul(31).wrapping_add(frame) as u8).collect()
    }

    impl Rig {
        fn new(path: PathConfig) -> Rig {
            Self::with_seed(path, 11)
        }

        fn with_seed(path: PathConfig, seed: u64) -> Rig {
            let (da, db) = (device(1), device(2));
            let mut a = Peer::new(CALL, &da, &db);
            let mut b = Peer::new(CALL, &db, &da);
            a.install_send_secret(0, &[1; 32]).unwrap();
            b.install_recv_secret(0, &[1; 32]).unwrap();
            b.install_send_secret(0, &[2; 32]).unwrap();
            a.install_recv_secret(0, &[2; 32]).unwrap();
            Rig {
                a,
                b,
                ab: Path::new(path, seed),
                ba: Path::new(path, 2 * seed + 1),
                now: 0,
                voice: true,
                voice_sent: 0,
                video_sent: 0,
                voice_got: Vec::new(),
                last_voice_at: None,
                max_voice_gap: 0,
                expected_share: None,
                receiver_paused: false,
                router_empty_at: 0,
                peak_router_us: 0,
                video_got: Vec::new(),
                shown_at: Vec::new(),
                keys_made: Vec::new(),
                outage: false,
                reverse_outage: false,
                trace_resends: false,
                video_attempts: None,
                drop_once: Default::default(),
                last_drop_us: None,
                adaptive: false,
                fixed_picture_bytes: None,
                sizes: Vec::new(),
            }
        }

        /// a sends voice every 20 ms and video at 30 pictures a second; both poll every ms.
        fn run(&mut self, ms: u64) {
            let mut out = Vec::new();
            let mut events = Vec::new();
            for _ in 0..ms {
                self.tick(&mut out, &mut events, |r, out| {
                    let now = r.now;
                    let key = r.video_sent == 0 || r.a.key_frame_due(stream::CAMERA_HIGH, now);
                    let size = match (r.fixed_picture_bytes, r.adaptive, key) {
                        (Some(bytes), _, _) => bytes * if key { 3 } else { 1 },
                        // 85% of the estimate at 30 pictures a second, a key frame three times that
                        (_, true, _) => (r.a.estimate_bps() * 85 / 100 / 30 / 8) as usize * if key { 3 } else { 1 },
                        (_, false, true) => 20_000,
                        (_, false, false) => 3_000 + (r.video_sent as usize % 7) * 100,
                    };
                    r.send_picture(size.max(400), key, out);
                });
            }
        }

        fn send_picture(&mut self, size: usize, key: bool, out: &mut Vec<Vec<u8>>) {
            if key {
                self.keys_made.push((self.video_sent, self.now));
            }
            self.sizes.push(size);
            self.a.send_video(self.now, stream::CAMERA_HIGH, &au(self.video_sent, size), key, self.now, out).unwrap();
            self.video_sent += 1;
        }

        fn send_at_guard_ask(&mut self, guard: &mut Guard, force_key: bool, out: &mut Vec<Vec<u8>>) {
            let key = force_key || self.video_sent == 0 || self.a.key_frame_due(stream::CAMERA_HIGH, self.now);
            let bytes = (guard.ask_bps() / u64::from(guard.rung().fps) / 8) as usize * if key { 3 } else { 1 };
            guard.encoded(self.now, bytes, key);
            self.send_picture(bytes, key, out);
        }

        fn tick(&mut self, out: &mut Vec<Vec<u8>>, events: &mut Vec<Event>, capture: impl FnOnce(&mut Rig, &mut Vec<Vec<u8>>)) {
            self.step(1_000, out, events, |r, out| {
                if r.now % 33_000 == 0 {
                    capture(r, out);
                }
            });
        }

        fn step(&mut self, dt: u64, out: &mut Vec<Vec<u8>>, events: &mut Vec<Event>, capture: impl FnOnce(&mut Rig, &mut Vec<Vec<u8>>)) {
            self.now += dt;
            let now = self.now;
            if self.voice && now % VOICE_INTERVAL_US == 0 {
                let seq = self.a.next_voice_seq();
                let v = Voice { flags: 0, seq, capture_us: now, payload: &[seq as u8; 100] };
                out.push(self.a.send_voice(now, stream::VOICE, &v, true).unwrap());
                self.voice_sent += 1;
            }
            capture(self, out);
            let first = out.len();
            let before = self.a.stats;
            self.a.poll(now, out);
            let resends = (self.a.stats.resent - before.resent) as usize;
            if self.trace_resends && resends > 0 {
                let bytes: usize = out[first..first + resends].iter().map(Vec::len).sum();
                eprintln!("burst at {now} µs, {} bit/s, {resends} resends ({} sender repairs), {bytes} bytes", self.a.estimate_bps(), self.a.stats.repairs - before.repairs);
            }
            self.a.send_padding(now, |d| { out.push(d); true });
            self.send_forward(out);
            if !self.receiver_paused {
                self.b.poll(now, out);
                for d in out.drain(..) {
                    if !self.reverse_outage {
                        self.ba.send(now, d);
                    }
                }
                self.ab.poll(now, out);
                for d in out.drain(..) {
                    self.b.receive(now, &d, events);
                }
            }
            self.ba.poll(now, out);
            for d in out.drain(..) {
                self.a.receive(now, &d, events);
            }
            for e in events.drain(..) {
                match e {
                    Event::Voice { seq, payload, talking, .. } => {
                        assert!(talking && payload == [seq as u8; 100], "voice frame {seq} intact");
                        if let Some(last) = self.last_voice_at.replace(now) {
                            self.max_voice_gap = self.max_voice_gap.max(now - last);
                        }
                        self.voice_got.push(seq);
                    }
                    Event::Video { stream: stream::SHARE, frame } if self.expected_share.is_some() => {
                        assert_eq!(frame.data, self.expected_share.take().unwrap(), "the complete share picture must arrive");
                    }
                    Event::Video { stream: s, frame } => {
                        assert_eq!(s, stream::CAMERA_HIGH);
                        assert_eq!(frame.data, au(frame.frame, self.sizes[frame.frame as usize]), "picture {} intact", frame.frame);
                        if let Some(&(last, _)) = self.video_got.last() {
                            assert!(frame.frame > last, "pictures in order: {} after {last}", frame.frame);
                        }
                        self.video_got.push((frame.frame, frame.key));
                        self.shown_at.push(now);
                    }
                }
            }
        }

        fn send_forward(&mut self, out: &mut Vec<Vec<u8>>) {
            for d in out.drain(..) {
                if !self.drop_once.is_empty() {
                    let h = Header::parse(&d).unwrap().0;
                    if self.drop_once.remove(&(h.stream, h.seq)) {
                        self.last_drop_us = Some(self.now);
                        continue;
                    }
                }
                if !self.outage {
                    let video = self.video_attempts.as_mut().and_then(|attempts| {
                        let h = Header::parse(&d)?.0;
                        stream::is_video(h.stream).then(|| attempts.entry((h.stream, h.key_id, h.seq)))
                    });
                    let before = self.ab.stats;
                    let bytes = d.len();
                    self.ab.send(self.now, d);
                    if self.ab.config.rate_bps > 0 && self.ab.stats.lost == before.lost && self.ab.stats.queue_drops == before.queue_drops {
                        let service = bytes as u64 * 8_000_000 / self.ab.config.rate_bps;
                        self.router_empty_at = self.router_empty_at.max(self.now) + service;
                        self.peak_router_us = self.peak_router_us.max(self.router_empty_at - self.now);
                    }
                    if let Some(entry) = video {
                        let dropped = self.ab.stats.lost > before.lost || self.ab.stats.queue_drops > before.queue_drops;
                        entry.and_modify(|v| v.resent = true).or_insert(VideoAttempts { original_dropped: dropped, resent: false });
                    }
                }
            }
        }

        fn shown(&self, frame: u32) -> Option<u64> {
            self.video_got.iter().position(|&(id, _)| id == frame).map(|i| self.shown_at[i])
        }

        fn start_still_share(&mut self, out: &mut Vec<Vec<u8>>, events: &mut Vec<Event>) {
            let still = au(0, 1_200);
            self.a.send_video(0, stream::SHARE, &still, true, 0, out).unwrap();
            self.expected_share = Some(still);
            while self.now < 1_000_000 {
                self.step(1_000, out, events, |_, _| {});
            }
            assert!(self.expected_share.is_none(), "the initial still picture should reach the viewer");
        }

        fn report_repair(&self, case: &str, since: u64, shown: Option<u64>, bound: u64) {
            let received = self.b.video_stats().next().unwrap().1;
            eprintln!("{case}: picture {shown:?} µs, delay {:?} µs, bound {bound} µs, {} NACK resends, {} sender repairs, {} NACKed, {} key-request polls, keys made (frame, µs) {:?}",
                shown.map(|at| at - since), self.a.stats.resent - self.a.stats.repairs, self.a.stats.repairs, received.nacked, received.key_requests, self.keys_made);
        }
    }

    #[test]
    fn padding_waits_for_credit_and_only_an_accepted_send_consumes_a_hop() {
        let mut r = Rig::new(PathConfig::default());
        r.a.set_wanted(4_500_000);
        assert_eq!(r.a.next_send_us(), None, "no padding wake before the receiver reports delivery");
        r.a.send_padding(0, |_| panic!("padding before a delivery sample"));
        r.a.rate.on_arrival(0, 0, wire::MAX_DATAGRAM);
        let delivered = wire::MAX_DATAGRAM as u64 * 8 * 2; // One datagram in half a second.
        let paced = ((PAD_GAIN * delivered as f64) as u64).max(MIN_PROBE_BPS);
        let due = (wire::MAX_DATAGRAM as u64 * 8_000_000).div_ceil(paced);
        assert_eq!(r.a.next_send_us(), Some(due));
        r.a.send_padding(due - 1, |_| panic!("padding sent before it had a datagram of credit"));
        let mut refused = 0;
        r.a.send_padding(due, |d| { assert_eq!(d.len(), wire::MAX_DATAGRAM); refused += 1; false });
        assert_eq!((refused, r.a.hops, r.a.stats.sent, r.a.next_send_us()), (1, 0, 0, None));
        r.a.set_padding_blocked(false);
        assert_eq!(r.a.next_send_us(), Some(due));
        r.a.send_padding(due * 10, |d| {
            r.b.receive(due * 10, &d, &mut Vec::new());
            true
        });
        assert_eq!((r.a.hops, r.a.stats.padding_sent, r.b.arrivals.since_report), (2, 2, 2),
            "idle credit buys two reported hops, including after a blocked send");
        assert_eq!(r.a.next_send_us(), Some(due * 11));
        r.a.stop_share();
        assert_eq!(r.a.next_send_us(), None, "closing the subscription cancels its padding wake");
    }

    #[test]
    fn split_and_reordered_feedback_keep_the_same_padding_interval() {
        let mut rate = RateEstimator::new(START_BPS);
        let mut padding = Padding { wanted: 4_500_000, ..Default::default() };
        padding.observe_report(0, &rate);
        rate.on_arrival(100_000, 300_000, 1_200);
        padding.observe_report(3_000, &rate);
        assert_eq!(padding.recovery.as_ref().unwrap().bps, 32_000.0, "the stalled read covers all 300 ms");
        rate.on_arrival(301_000, 303_000, 1_200);
        padding.observe_report(3_030, &rate);
        assert_eq!(padding.recovery.as_ref().unwrap().bps, 32_000.0, "an early report cannot compress the sample to 3 ms");
        padding.observe_report(3_500, &rate);
        assert_eq!(padding.recovery.as_ref().unwrap().bps, 192_000.0, "only the fresh packet counts");
        rate.on_arrival(320_000, 323_000, 1_200);
        padding.observe_report(3_500, &rate);
        padding.observe_report(3_030, &rate);
        assert_eq!(padding.recovery.as_ref().unwrap().bps, 384_000.0, "split and late sections must use the same 50 ms");
    }

    #[test]
    fn a_still_share_fills_the_lan_then_sends_its_large_key() {
        let wanted = 4_500_000;
        let mut r = Rig::new(PathConfig { latency_us: 3_000, ..Default::default() });
        let (mut out, mut events) = (Vec::new(), Vec::new());
        r.start_still_share(&mut out, &mut events);
        assert_eq!(r.a.stats.padding_sent, 0, "an untargeted still share must send zero padding");
        let start = r.now;
        r.a.set_wanted(wanted);
        assert!(r.a.next_send_us().is_some(), "padding must wake the idle video pacer");
        // Keep observing a late crossing for the key and stall measurements. The
        // six-second assertion below uses the actual first crossing.
        let climb_bound = 6_000_000;
        while r.a.estimate_bps() < wanted && r.now < start + 2 * climb_bound {
            r.step(1_000, &mut out, &mut events, |_, _| {});
            let e = r.a.loss_events();
            assert_eq!(e.tested_cuts + e.untested_cuts, 0, "clean LAN backoff at {} µs, {} bit/s, queue {} µs, {e:?}",
                r.now - start, r.a.estimate_bps(), r.a.queue_delay_us());
        }
        let padding = r.a.stats.padding_sent * wire::MAX_DATAGRAM as u64;
        let climb_us = r.now - start;
        eprintln!("LAN climb: {climb_us} µs to {} bit/s, {padding} bytes of padding", r.a.estimate_bps());
        assert!(r.a.estimate_bps() >= wanted, "the still share never reached {wanted} bit/s in {climb_us} µs");
        assert!(padding > 0 && padding as f64 * 8.0 <= 1.5 * wanted as f64 / std::f64::consts::LN_2,
            "LAN climb spent {padding} padding bytes");
        assert_eq!(r.a.next_send_us(), None, "target reached: no padding deadline");
        let reached = r.now;
        while r.now < reached + 1_000_000 {
            r.step(1_000, &mut out, &mut events, |_, _| {});
        }
        let key = au(1, 150_000);
        let at = r.now;
        r.a.send_video(at, stream::SHARE, &key, true, at, &mut out).unwrap();
        r.expected_share = Some(key);
        while r.expected_share.is_some() && r.now < at + 500_000 {
            r.step(1_000, &mut out, &mut events, |_, _| {});
        }
        assert!(r.expected_share.is_none(), "the 150 KB key must reach the viewer within 0.5 s");
        assert_eq!(r.a.stats.padding_sent * wire::MAX_DATAGRAM as u64, padding, "no padding after reaching the panel's rate");
        assert!(r.max_voice_gap <= VOICE_INTERVAL_US, "voice arrival gap {} µs during padding and the key", r.max_voice_gap);
        eprintln!("LAN key arrived after {} µs, max voice gap {} µs", r.now - at, r.max_voice_gap);

        // With only voice and HEADs in the delivery window, pause the receiver, then
        // read the waiting datagrams together before its overdue feedback (as the driver does).
        while r.now < at + 2_000_000 { r.step(1_000, &mut out, &mut events, |_, _| {}); }
        let before = r.a.estimate_bps();
        assert!(before >= wanted);
        let mut low = before;
        r.receiver_paused = true;
        let resume = r.now + 300_000;
        while r.now < resume {
            r.step(1_000, &mut out, &mut events, |_, _| {});
            low = low.min(r.a.estimate_bps());
        }
        r.receiver_paused = false;
        r.ab.poll(resume, &mut out);
        assert!(out.len() >= 300_000 / VOICE_INTERVAL_US as usize, "the receiver must observe the stalled burst");
        for d in out.drain(..) { r.b.receive(resume, &d, &mut events); }
        r.b.poll(resume, &mut out);
        for d in out.drain(..) { r.ba.send(resume, d); }
        let mut recovered_at = None;
        let mut wakes = BTreeMap::new();
        while r.now < resume + 3_000_000 {
            let a = &r.a;
            let pads = a.stats.padding_sent;
            let gate = if a.estimate_bps() >= wanted { "wanted reached" }
                else if a.padding.hold_until > r.now + 1_000 { "hold" }
                else if a.padding.blocked || a.next_paced().is_some() { "backlog" }
                else if a.padding_bps() == 0 { "no delivery" }
                else { "pacing credit" };
            r.step(1_000, &mut out, &mut events, |_, _| {});
            let bps = r.a.estimate_bps();
            let gate = if r.a.stats.padding_sent > pads { "sent padding" } else { gate };
            *wakes.entry(gate).or_insert(0u32) += 1;
            if (low >= wanted && bps < wanted) || (r.now - resume).is_multiple_of(500_000) {
                let delivered = r.a.rate.delivered_bps() as u64;
                let target = r.a.padding_bps();
                let sent_bps = r.a.sent_since(r.now, 1_000_000).1 * 8;
                let exercised = sent_bps + sent_bps / 2;
                eprintln!("stall +{} µs: estimate {bps}, 500 ms delivered {delivered}, pad target {target}, 1.5 × sent {exercised} bit/s, hold remaining {} µs, wakes {wakes:?}",
                    r.now - resume, r.a.padding.hold_until.saturating_sub(r.now));
                wakes.clear();
            }
            low = low.min(bps);
            if bps < wanted { recovered_at = None; }
            else { recovered_at.get_or_insert(r.now - resume); }
        }
        eprintln!("300 ms receiver stall: before {before}, minimum {low}, final {}, at wanted after {recovered_at:?} µs", r.a.estimate_bps());
        assert!(climb_us <= climb_bound, "the still share took {climb_us} µs to reach {wanted} bit/s, limit {climb_bound}");
        assert!(low * 10 >= before * 7, "stall sank {before} bit/s to {low}");
        assert!(recovered_at.is_some(), "still below {wanted} bit/s three seconds after the receiver resumed");
    }

    #[test]
    fn a_voice_free_still_share_climbs_lan_estimate_via_probe_floor() {
        let wanted = 4_500_000;
        let lan = PathConfig { latency_us: 3_000, ..Default::default() };
        let (mut out, mut events) = (Vec::new(), Vec::new());
        let mut r = Rig::new(lan);
        r.voice = false;
        r.start_still_share(&mut out, &mut events);
        let start = r.now;
        r.a.set_wanted(wanted);
        while r.now < start + 8_000_000 && r.a.estimate_bps() < wanted {
            r.step(1_000, &mut out, &mut events, |_, _| {});
        }
        let climb_us = r.now - start;
        let cuts = r.a.loss_events();
        eprintln!("voice-free LAN climb: {climb_us} µs to {} bit/s", r.a.estimate_bps());
        assert!(r.a.estimate_bps() >= wanted, "below {wanted} bit/s after {climb_us} µs");
        assert_eq!(cuts.tested_cuts + cuts.untested_cuts, 0, "clean LAN cut during climb");
        assert!(r.a.stats.padding_sent > 0);
        assert_eq!(r.voice_sent, 0);

        let mut r = Rig::new(PathConfig { rate_bps: 150_000, ..lan });
        r.voice = false;
        r.start_still_share(&mut out, &mut events);
        r.a.set_wanted(wanted);
        let end = r.now + 160_000_000;
        let mut holds = Vec::new();
        while r.now < end {
            let until = r.a.padding.hold_until;
            let sent = r.a.stats.padding_sent;
            r.step(1_000, &mut out, &mut events, |_, _| {});
            if r.now < until {
                assert_eq!(r.a.stats.padding_sent, sent, "padded during hold at {} µs", r.now);
            }
            if r.a.padding.hold_until > until {
                holds.push(r.a.padding.hold_us);
            }
        }
        assert!(holds.starts_with(&[10_000_000, 20_000_000, 40_000_000, 60_000_000]),
            "voice-free 150 kbit/s backoff: {holds:?}");
        assert!(holds.iter().skip(4).all(|&hold| hold == 60_000_000));
        assert_eq!(r.voice_sent, 0);
    }

    #[test]
    fn a_still_share_backs_off_padding_on_a_slow_link() {
        let path = PathConfig { latency_us: 3_000, rate_bps: 150_000, ..Default::default() };
        let (mut out, mut events) = (Vec::new(), Vec::new());
        let mut camera = Rig::new(path);
        while camera.now < 1_000_000 { camera.step(1_000, &mut out, &mut events, |_, _| {}); }
        camera.adaptive = true;
        camera.run(6_000);
        let camera_peak = camera.peak_router_us;

        let mut r = Rig::new(path);
        r.start_still_share(&mut out, &mut events);
        let start = r.now;
        r.a.set_wanted(4_500_000);
        let mut holds = Vec::new();
        let mut after_ten = (0, 0);
        while r.now < start + 120_000_000 {
            let held_until = r.a.padding.hold_until;
            let sent = r.a.stats.padding_sent;
            r.step(1_000, &mut out, &mut events, |_, _| {});
            if r.now < held_until {
                assert_eq!(r.a.stats.padding_sent, sent, "padded during hold at {} µs", r.now);
            }
            if r.a.padding.hold_until > held_until {
                holds.push((r.now - start, r.a.padding.hold_us));
            }
            if r.now == start + 10_000_000 {
                after_ten = (r.a.stats.padding_sent, r.a.stats.sent_bytes);
            }
        }
        let padding = (r.a.stats.padding_sent - after_ten.0) * wire::MAX_DATAGRAM as u64;
        let total = r.a.stats.sent_bytes - after_ten.1;
        let peak = r.peak_router_us;
        let sent = r.voice_sent;
        let drain_by = r.router_empty_at + path.latency_us + 1_000;
        while r.now < drain_by { r.step(1_000, &mut out, &mut events, |_, _| {}); }
        let missing: Vec<_> = (0..sent).filter(|seq| r.voice_got.binary_search(seq).is_err()).collect();
        eprintln!("150 kbit/s: peak router {peak} µs vs camera {camera_peak} µs, after 10 s {padding}/{total} padding bytes, holds {holds:?}, missing voice {missing:?}");
        assert!(peak <= camera_peak, "padding queued {peak} µs against the camera start's {camera_peak} µs");
        assert!(padding * 10 < total, "after 10 s padding spent {padding} of {total} bytes");
        assert_eq!(holds.iter().map(|&(_, duration)| duration).take(4).collect::<Vec<_>>(),
            vec![10_000_000, 20_000_000, 40_000_000, 60_000_000], "failed attempts must reach the one-minute hold");
        assert!(holds.iter().skip(4).all(|&(_, duration)| duration == 60_000_000));
        assert!(missing.is_empty(), "lost voice frames {missing:?} in the 120 s run");
    }

    #[test]
    fn a_share_key_on_a_thin_link_preserves_voice_playout() {
        use crate::driver::codec::{Codecs, TestCodecs};
        use makepad_teamtalk::jitter::{JitterRing, Playout, PlayoutConfig};
        let path = PathConfig { rate_bps: 300_000, ..Default::default() };
        let rung = crate::guard::LADDER[3];
        let mut encoder = TestCodecs.encoder(rung, (crate::guard::video_ask(START_BPS) / 1000) as u32).unwrap();
        let picture = encoder.push(&vec![128; (rung.width*rung.height*3/2) as usize], 0).unwrap().remove(0);
        assert!(picture.key && picture.data.len() > wire::MAX_FRAGMENT, "exercise a fragmented share key");
        let run = |with_key: bool| {
            let mut r = Rig::new(path);
            let ring = JitterRing::new();
            let mut audio = crate::driver::voice::VoiceDecoder::default();
            let mut payload = Vec::new();
            makepad_teamtalk::codec::adpcm_encode_into(&mut Default::default(), 4, &[0.25; 320], &mut payload);
            let mut player = Playout::new(PlayoutConfig { default_frame: 960, ..Default::default() });
            let (mut out, mut events) = (Vec::new(), Vec::new());
            let (mut last, mut wire_gap, mut audible_gap, mut quiet, mut voices, mut keys) = (0, 0, 0, 0, 0, 0);
            for now in (0..2_000_000).step_by(1_000) {
                if now % VOICE_INTERVAL_US == 0 {
                    let seq = r.a.next_voice_seq();
                    let voice = Voice { flags: 0, seq, capture_us: now, payload: &payload };
                    out.push(r.a.send_voice(now, stream::VOICE, &voice, true).unwrap());
                }
                if with_key && now == 1_000_000 {
                    r.a.send_video(now, stream::SHARE, &picture.data, true, now, &mut out).unwrap();
                }
                r.a.poll(now, &mut out);
                for d in out.drain(..) { r.ab.send(now, d); }
                r.b.poll(now, &mut out);
                for d in out.drain(..) { r.ba.send(now, d); }
                r.ba.poll(now, &mut out);
                for d in out.drain(..) { r.a.receive(now, &d, &mut events); }
                r.ab.poll(now, &mut out);
                for d in out.drain(..) { r.b.receive(now, &d, &mut events); }
                for event in events.drain(..) {
                    match event {
                        Event::Voice { seq, flags, payload, .. } => {
                            assert_eq!(seq, voices, "voice sequence continuity with share={with_key}");
                            voices += 1;
                            if now > 500_000 { wire_gap = wire_gap.max(now - last); }
                            last = now;
                            audio.insert(&ring, seq, flags, &payload);
                        }
                        Event::Video { stream: stream::SHARE, frame } if frame.key => { keys += 1; }
                        _ => {}
                    }
                }
                let mut samples = [0.0; 48];
                player.render(&ring, 48_000., &mut samples);
                if now > 500_000 {
                    for sample in samples {
                        quiet = if sample == 0. { quiet + 1 } else { 0 };
                        audible_gap = audible_gap.max(quiet);
                    }
                }
            }
            assert_eq!(keys, usize::from(with_key), "the key must actually traverse the constrained path");
            (audible_gap, wire_gap, player.stats.concealed, voices)
        };
        let control = run(false);
        let share = run(true);
        eprintln!("300 kbit/s, {}-byte key: (silent samples, max arrival gap us, concealed frames, received voice) control {control:?}, share {share:?}", picture.data.len());
        assert_eq!(share.0, control.0, "share key lengthened audible voice gap, samples: {share:?} vs {control:?}");
        assert_eq!(share.1, control.1, "share key lengthened the voice arrival gap: {share:?} vs {control:?}");
        assert_eq!(share.2, control.2, "share key forced voice concealment: {share:?} vs {control:?}");
        assert_eq!(share.3, control.3, "share key prevented a voice frame arriving: {share:?} vs {control:?}");
    }

    #[test]
    fn a_still_shares_head_repairs_its_entire_missing_last_picture() {
        let mut r = Rig::new(PathConfig::default());
        let (mut packets, mut events, mut shown) = (Vec::new(), Vec::new(), Vec::new());
        r.drop_once.insert((stream::SHARE, 1));
        let last_capture = 200_000;
        for now in (0..=last_capture + 250_000).step_by(1_000) {
            r.now = now;
            if now == 0 || now == last_capture {
                r.a.send_video(now, stream::SHARE, &[if now == 0 { 10 } else { 20 }], now == 0, now, &mut packets).unwrap();
            }
            r.a.poll(now, &mut packets);
            r.send_forward(&mut packets);
            r.b.poll(now, &mut packets);
            for d in packets.drain(..) { r.ba.send(now, d); }
            r.ab.poll(now, &mut packets);
            for d in packets.drain(..) { r.b.receive(now, &d, &mut events); }
            r.ba.poll(now, &mut packets);
            for d in packets.drain(..) { r.a.receive(now, &d, &mut events); }
            for event in events.drain(..) {
                if let Event::Video { stream: stream::SHARE, frame } = event { shown.push((now, frame.data)); }
            }
        }
        let receiver = r.b.in_video[&stream::SHARE].stats;
        assert_eq!(r.last_drop_us, Some(last_capture), "the last AU must be lost whole, with no later media");
        assert!(receiver.nacked > 0, "HEAD should expose the missing tail for NACK, stats {receiver:?}");
        assert_eq!(shown.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>(), [&[10][..], &[20][..]], "both original pictures must be shown before give-up: {shown:?}");
        eprintln!("still share: last frame repaired in {} ms, {} NACKs, {} keys shown", (shown[1].0-last_capture)/1000, receiver.nacked, receiver.key_frames);
    }

    #[test]
    fn watching_a_still_share_gets_a_key_in_one_trip_and_recovers_after_loss() {
        use crate::driver::codec::{Codecs, TestCodecs};
        use crate::guard::Rung;

        const LEG: u64 = 20_000;
        const LOSS_AT: u64 = 100_000;
        let mut r = Rig::new(PathConfig { latency_us: LEG, ..Default::default() });
        let rung = Rung { width: 8, height: 8, fps: 30, min_ask_bps: 0, full_bps: 1000 };
        let mut encoder = TestCodecs.encoder(rung, 1).unwrap();
        let mut decoder = TestCodecs.decoder().unwrap();
        let mut pictures = Vec::new();
        let mut packets = Vec::new();
        let mut events = Vec::new();
        let mut decoded = 0;
        let mut displayed = Vec::new();
        let mut originals_lost = std::collections::BTreeSet::new();
        let mut lost_request = false;
        r.b.receive_share(true);
        // No periodic encoder or repaint. Only the initial Watch and recovery KEYs can
        // produce a new key of this held image. Drop three consecutive deltas AND all repairs
        // of them. The fourth delta exposes the hole; also lose the first repair KEY request.
        for now in (0..=LOSS_AT + 1_000_000).step_by(1_000) {
            r.ba.poll(now, &mut packets);
            for packet in packets.drain(..) { r.a.receive(now, &packet, &mut events); }
            r.ab.poll(now, &mut packets);
            for packet in packets.drain(..) { r.b.receive(now, &packet, &mut events); }
            for event in events.drain(..) {
                if let Event::Video { stream: stream::SHARE, frame } = event {
                    decoder.push_into(&frame.data, i64::from(frame.frame), &mut pictures).unwrap();
                    assert_eq!(pictures.len(), 1, "a recovered picture must actually decode");
                    pictures.clear();
                    decoded += 1;
                    r.b.decoded_share(now, decoded, frame.frame, 0);
                    displayed.push((now, frame.key));
                }
            }
            let key = r.a.key_frame_due(stream::SHARE, now);
            let update = [LOSS_AT, LOSS_AT + 33_000, LOSS_AT + 66_000, LOSS_AT + 99_000].contains(&now);
            if key || update {
                if key { encoder.request_key(); }
                let packet = encoder.push(&[90; 96], now as i64 * 10).unwrap().pop().unwrap();
                r.a.send_video(now, stream::SHARE, &packet.data, packet.key, now, &mut packets).unwrap();
            }
            r.a.poll(now, &mut packets);
            for packet in packets.drain(..) {
                let h = Header::parse(&packet).unwrap().0;
                if h.stream == stream::SHARE && (1..=3).contains(&h.seq) {
                    originals_lost.insert(h.seq);
                } else { r.ab.send(now, packet); }
            }
            let before = r.b.stats.share_requests_sent;
            r.b.poll(now, &mut packets);
            if now >= LOSS_AT && !lost_request && before != r.b.stats.share_requests_sent {
                lost_request = true;
                packets.clear();
            }
            for packet in packets.drain(..) { r.ba.send(now, packet); }
        }
        assert_eq!(originals_lost.len(), 3);
        assert!(lost_request, "the first recovery request must be lost");
        assert_eq!(displayed.len(), 2, "only the initial key and the replacement key should decode: {displayed:?}");
        assert_eq!(displayed[0], (2 * LEG, true), "Watch must get its first picture in one RTT");
        let (at, key) = displayed[1];
        assert!(key && at - LOSS_AT < 1_000_000, "the missing run requires a fresh key within one second");
        assert_eq!(r.b.stats.share_requests_sent, 3, "one opening KEY and one lost recovery KEY with a single retry");
        eprintln!("share virtual clock: first picture {} ms (one RTT), three lost frames and a lost request repaired in {} ms",
            displayed[0].0 / 1000, (at - LOSS_AT) / 1000);
    }

    /// A still window, on a fast path and on a thin one where a key of a few kilobytes takes
    /// seconds to cross: Watch asks once, gets one key, and holding the picture costs nothing.
    #[test]
    fn a_still_share_costs_one_key() {
        const KEY_BYTES: usize = 5_000;
        let fast = PathConfig { latency_us: 1_000, ..Default::default() };
        let thin = PathConfig { latency_us: 2_000, rate_bps: 48_000, queue_us: 5_000_000, ..Default::default() };
        for (name, path) in [("fast", fast), ("thin", thin)] {
            let mut r = Rig::new(path);
            let (mut packets, mut events) = (Vec::new(), Vec::new());
            let (mut decoded, mut frame, mut keys_made, mut first_picture) = (0, 0u32, 0, None);
            let mut source_key = false;
            r.b.receive_share(true);
            for now in (0..=20_000_000u64).step_by(1_000) {
                r.ba.poll(now, &mut packets);
                for packet in packets.drain(..) { r.a.receive(now, &packet, &mut events); }
                r.ab.poll(now, &mut packets);
                for packet in packets.drain(..) { r.b.receive(now, &packet, &mut events); }
                for event in events.drain(..) {
                    if let Event::Video { stream: stream::SHARE, frame: f } = event {
                        decoded += 1;
                        first_picture.get_or_insert(now);
                        r.b.decoded_share(now, decoded, f.frame, 0);
                    }
                }
                // The app's source: a still window makes a picture only when asked for a key, and
                // the share takes it only while nothing of the last one waits for pacing.
                source_key |= r.a.key_frame_due(stream::SHARE, now);
                if source_key && !r.a.share_queued() {
                    source_key = false;
                    keys_made += 1;
                    r.a.send_video(now, stream::SHARE, &au(frame, KEY_BYTES), true, now, &mut packets).unwrap();
                    frame += 1;
                }
                r.a.poll(now, &mut packets);
                for packet in packets.drain(..) { r.ab.send(now, packet); }
                r.b.poll(now, &mut packets);
                for packet in packets.drain(..) { r.ba.send(now, packet); }
            }
            let counts = format!("{name}: first picture at {:?} ms, {} requests sent ({:?}), {} keys made, {} decoded",
                first_picture.map(|t| t / 1000), r.b.stats.share_requests_sent, r.b.stats.share_request_causes, keys_made, decoded);
            assert!(first_picture.is_some(), "Watch shows a picture: {counts}");
            assert_eq!((r.b.stats.share_requests_sent, keys_made, decoded), (1, 1, 1),
                "Watch asks once and a still window costs one key: {counts}");
        }
    }

    #[test]
    fn a_still_share_retries_until_the_decoder_publishes_a_picture() {
        let mut r = Rig::new(PathConfig::default());
        let mut packets = Vec::new();
        let mut events = Vec::new();
        r.b.receive_share(true);
        r.b.poll(0, &mut packets);
        packets.clear();
        // The codec accepts a complete key without returning a picture on this push.
        r.a.send_video(20_000, stream::SHARE, &[42], true, 20_000, &mut packets).unwrap();
        for packet in packets.drain(..) { r.b.receive(40_000, &packet, &mut events); }
        assert!(events.iter().any(|e| matches!(e, Event::Video { .. })));
        r.b.decoded_share(40_000, 0, 0, 0);
        r.b.poll(SHARE_RETRY_US - 1, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 1);
        r.b.poll(SHARE_RETRY_US, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 2, "accepting a key without pixels must retry on a still window");
        packets.clear();
        r.a.send_video(270_000, stream::SHARE, &[43], true, 270_000, &mut packets).unwrap();
        for packet in packets.drain(..) { r.b.receive(290_000, &packet, &mut events); }
        r.b.decoded_share(300_000, 1, 0, 0);
        r.b.poll(550_000, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 3, "an older picture cannot acknowledge the most recent update");
        r.b.decoded_share(560_000, 2, 1, 0);
        r.b.poll(810_000, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 3, "publishing the current picture settles the request");
        r.b.decoded_share(820_000, 2, 1, 1);
        r.b.poll(820_000, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 4, "a decoder failure asks again even with no transport gap");
        r.b.decoded_share(900_000, 3, 2, 1);
        r.b.poll(5_900_000, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 4,
            "a still window costs nothing: the sharer's repeated HEAD is what reveals a lost last picture");
        r.b.decoded_share(5_900_000, 3, 2, 2);
        r.b.poll(5_900_000, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 5, "a second decoder failure asks again");
        r.b.receive_share(false);
        r.b.poll(7_000_000, &mut packets);
        assert_eq!(r.b.stats.share_requests_sent, 5, "Close cancels outstanding retries");
    }

    #[test]
    fn lost_one_packet_pictures_recover_at_both_capture_rates() {
        let mut recovery = Vec::new();
        for (fps, ask) in [(15u64, 70_000u64), (30, 120_000)] {
            let (mut lost, mut recovered) = (0, 0);
            for seed in 1..=8 {
                let mut r = Rig::with_seed(PathConfig { loss_ppm: 150_000, ..Default::default() }, seed);
                let mut originals = BTreeMap::new();
                let mut lost_frames = std::collections::BTreeSet::new();
                let (mut next_seq, mut shown) = (0u64, 0);
                let (mut out, mut events) = (Vec::new(), Vec::new());
                let mut last_video_send = 10_000_000;
                while r.now <= last_video_send + video::CACHE_US + 40_000 + IDLE_FEEDBACK_US {
                    let now = r.now;
                    if now < 10_000_000 {
                        if now % 20_000 == 0 {
                            let seq = r.a.next_voice_seq();
                            let v = Voice { flags: 0, seq, capture_us: now, payload: &[seq as u8; 100] };
                            out.push(r.a.send_voice(now, stream::VOICE, &v, true).unwrap());
                        }
                        if now * fps >= u64::from(r.video_sent) * 1_000_000 {
                            let key = r.a.key_frame_due(stream::CAMERA_HIGH, now) || u64::from(r.video_sent) % (2 * fps) == 0;
                            let size = (ask / fps / 8) as usize * if key { 4 } else { 1 };
                            if !key && size <= wire::MAX_FRAGMENT {
                                originals.insert(next_seq, r.video_sent);
                            }
                            next_seq += size.div_ceil(wire::MAX_FRAGMENT) as u64;
                            r.send_picture(size, key, &mut out);
                        }
                    }
                    r.a.poll(now, &mut out);
                    for d in out.drain(..) {
                        let h = Header::parse(&d).unwrap().0;
                        let before = r.ab.stats.lost;
                        r.ab.send(now, d);
                        if h.stream == stream::CAMERA_HIGH {
                            last_video_send = last_video_send.max(now);
                            if let Some(frame) = originals.remove(&h.seq) {
                                if r.ab.stats.lost > before {
                                    lost_frames.insert(frame);
                                }
                            }
                        }
                    }
                    r.b.poll(now, &mut out);
                    for d in out.drain(..) { r.ba.send(now, d); }
                    r.ab.poll(now, &mut out);
                    for d in out.drain(..) { r.b.receive(now, &d, &mut events); }
                    r.ba.poll(now, &mut out);
                    for d in out.drain(..) { r.a.receive(now, &d, &mut events); }
                    for event in events.drain(..) {
                        if let Event::Video { frame, .. } = event {
                            shown += usize::from(lost_frames.contains(&frame.frame));
                        }
                    }
                    r.now += 1_000;
                }
                lost += lost_frames.len();
                recovered += shown;
            }
            assert!(lost > 0, "{fps} fps needs lost originals to measure recovery, got {lost}");
            recovery.push(recovered as f64 / lost as f64);
            eprintln!("{fps} fps, 15% loss: showed {recovered}/{lost} lost one-packet pictures after a resend");
        }
        assert!((recovery[0] - recovery[1]).abs() <= 0.05,
            "lost one-packet pictures should recover within five percentage points at 15 and 30 fps, got {recovery:?}");
    }

    #[test]
    fn a_large_key_frame_is_paced_and_arrives_whole() {
        let mut r = Rig::new(PathConfig::default());
        let picture = au(0, 30_000);
        let mut out = Vec::new();
        let mut events = Vec::new();
        r.a.send_video(0, stream::CAMERA_HIGH, &au(0, 3_000), false, 0, &mut out).unwrap();
        assert!(out.is_empty() && r.a.paced_bytes() == 0, "a picture before the stream's first key frame can't be decoded, so it goes to no one");
        assert!(r.a.key_frame_due(stream::CAMERA_HIGH, 0), "and it asks the encoder for a key frame");
        r.a.send_video(0, stream::CAMERA_HIGH, &picture, true, 0, &mut out).unwrap();
        let initial_bytes: usize = out.iter().map(Vec::len).sum();
        let wire_bytes = initial_bytes + r.a.paced_bytes();
        let pace = START_BPS * 5 / 2;
        let burst_bits = (pace * 5 / 1_000).max(2 * wire::MAX_DATAGRAM as u64 * 8);
        assert!(initial_bytes > 0 && initial_bytes as u64 * 8 <= burst_bits, "first release used {initial_bytes} bytes of burst");

        // Spend beyond the remaining credit with voice. Its hop goes immediately;
        // the same bytes move the head video's deadline back.
        let due = r.a.next_send_us().expect("the large picture must leave a paced tail");
        let v = Voice { flags: 0, seq: 0, capture_us: 0, payload: &[0; 300] };
        let voice = r.a.send_voice(0, stream::VOICE, &v, true).unwrap();
        let voice_wait = (voice.len() as u64 * 8_000_000).div_ceil(pace);
        let after_voice = r.a.next_send_us().expect("voice must leave the video queued");
        assert!(after_voice.abs_diff(due + voice_wait) <= 1, "voice wire time must postpone video by {voice_wait} µs");
        out.push(voice);

        // One picture, no new encoder input or rate reports: the pace stays constant.
        let mut released = 0;
        let mut peak_bits = 0;
        let mut last_us = 0;
        for tick in 0..=1_000 {
            r.now = tick * 1_000;
            let due = r.a.next_send_us();
            r.a.poll(r.now, &mut out);
            let mut bytes = 0;
            for d in out.drain(..) {
                if stream::is_video(Header::parse(&d).unwrap().0.stream) {
                    bytes += d.len();
                }
                r.ab.send(r.now, d);
            }
            let bits = bytes as u64 * 8;
            let limit = burst_bits + pace / 1_000 + wire::MAX_DATAGRAM as u64 * 8;
            assert!(bits <= limit, "tick {tick} released {bits} media bits, limit {limit}");
            if tick > 0 {
                assert_eq!(bytes > 0, due.is_some_and(|at| at <= r.now), "video release must follow its deadline {due:?} at {} µs", r.now);
            }
            peak_bits = peak_bits.max(bits);
            released += bytes;
            if bytes > 0 {
                last_us = r.now;
            }
            assert_eq!(r.a.paced_bytes(), wire_bytes - released, "queued bytes must match the unsent tail");
            let age = if released < wire_bytes { r.now } else { 0 };
            assert_eq!(r.a.pacer_delay_us(r.now), age, "oldest queued fragment must retain its enqueue time");

            r.ab.poll(r.now, &mut out);
            for d in out.drain(..) {
                r.b.receive(r.now, &d, &mut events);
            }
            if events.iter().any(|e| matches!(e, Event::Video { .. })) {
                break;
            }
        }
        let min_us = ((wire_bytes as u64 * 8 - burst_bits) * 1_000_000).div_ceil(pace);
        assert!(last_us >= min_us, "picture finished at {last_us} µs, before its {min_us} µs pacing minimum");
        assert!(events.iter().any(|e| matches!(e, Event::Video { frame, .. } if frame.key && frame.data == picture)), "receiver must get the whole key frame");
        assert_eq!(released, wire_bytes, "every queued video byte must leave");
        assert_eq!(r.a.next_send_us(), None, "a drained pacer has no deadline");
        assert_eq!(r.ab.stats.lost + r.ab.stats.queue_drops, 0, "clean path must drop no datagrams");
        eprintln!("30 kB key frame: {wire_bytes} wire bytes, peak {peak_bits} bits/tick, final release {last_us} µs, received at {} µs", r.now);
    }

    #[test]
    fn a_pacer_woken_late_keeps_its_pace_and_bounds_its_catch_up() {
        // Wakes every 16 ms, as a coarse system timer gives, and one 200 ms stall of the thread.
        let mut r = Rig::new(PathConfig::default());
        r.a.rate = RateEstimator::new(4_000_000);
        let pace = 4_000_000 * 5 / 2;
        let burst_bits = (pace * 5 / 1_000).max(2 * wire::MAX_DATAGRAM as u64 * 8);
        let mut out = Vec::new();
        r.a.send_video(0, stream::CAMERA_HIGH, &au(0, 400_000), true, 0, &mut out).unwrap();
        let wire_bits = (out.iter().map(Vec::len).sum::<usize>() + r.a.paced_bytes()) as u64 * 8;
        out.clear();
        let (wake, stall, stall_at) = (16_000, 200_000, 64_000);
        let release_limit = burst_bits + pace * PACE_CATCH_UP_US / 1_000_000 + wire::MAX_DATAGRAM as u64 * 8;
        let mut now = 0;
        while r.a.paced_bytes() > 0 {
            now += if now == stall_at { stall } else { wake };
            r.a.poll(now, &mut out);
            let bits: u64 = out.drain(..).filter(|d| stream::is_video(Header::parse(d).unwrap().0.stream)).map(|d| d.len() as u64 * 8).sum();
            assert!(bits <= release_limit, "the wake at {now} µs released {bits} bits, beyond the burst and catch-up {release_limit}");
            assert!(now < 2_000_000, "the picture must drain");
        }
        // only the stall's wait beyond the catch-up is lost; the rest of the time keeps the pace
        let paced_us = (wire_bits - burst_bits) * 1_000_000 / pace;
        let limit_us = paced_us + (stall - PACE_CATCH_UP_US) + 2 * wake;
        assert!(now <= limit_us, "a {}-bit picture took {now} µs at 16 ms wakes, beyond {limit_us} µs", wire_bits);
    }

    #[test]
    fn paced_video_ramps_on_a_clean_path() {
        let mut r = Rig::new(PathConfig::default());
        r.adaptive = true;
        let mut estimates = Vec::new();
        let mut max_path_delay = 0;
        let mut max_pacer_delay = 0;
        for _ in 0..4_000 {
            r.run(1);
            max_path_delay = max_path_delay.max(r.a.queue_delay_us());
            max_pacer_delay = max_pacer_delay.max(r.a.pacer_delay_us(r.now));
            if r.now % 1_000_000 == 0 {
                estimates.push(r.a.estimate_bps());
            }
        }
        eprintln!("clean path estimates each second: {estimates:?} bit/s, peak path queue {max_path_delay} µs, peak pacer age {max_pacer_delay} µs");
        assert!(r.a.estimate_bps() >= 3 * START_BPS, "estimate must at least triple in four seconds: {estimates:?}");
        assert!(max_path_delay < 20_000, "release stamps must keep measured path queue below 20 ms, got {max_path_delay} µs");
        assert!(max_pacer_delay < 20_000, "adaptive pictures must drain within 20 ms, oldest was {max_pacer_delay} µs");
    }

    #[test]
    fn a_clean_path_carries_everything_and_measures_its_round_trip() {
        let mut r = Rig::new(PathConfig { latency_us: 20_000, ..Default::default() });
        r.run(2_000);
        assert_eq!(r.voice_got.len() as u32, r.voice_sent - 1, "every voice frame but the one in flight");
        let delivered = r.video_got.len() as u32;
        assert!(delivered >= r.video_sent - 1, "every picture but the one in flight: {delivered} of {}", r.video_sent);
        assert_eq!(r.video_got.iter().filter(|(_, k)| *k).count(), 1, "one key frame: nothing asked for another");
        let rtt = r.a.rtt_us().expect("a measured the round trip");
        assert!((40_000..42_000).contains(&rtt), "a's round trip {rtt} µs on a 2 × 20 ms path");
        assert!(r.b.rtt_us().is_some_and(|t| (40_000..42_000).contains(&t)), "b measures it from feedback alone: {:?}", r.b.rtt_us());
        assert_eq!((r.a.stats.hops_lost, r.a.stats.resent), (0, 0), "nothing lost, nothing resent");
        let offset = r.a.clock_offset_us().expect("a estimated b's clock");
        assert!(offset.abs() <= 500, "one clock on both sides, a symmetric path: offset {offset} µs");
        assert!(r.a.stats.hops_delivered > 250, "a's hops are settled by b's reports: {}", r.a.stats.hops_delivered);
        assert!(r.a.stats.feedback_sent >= 15 && r.b.stats.feedback_sent >= 35, "feedback flows: a {} (idle), b {}", r.a.stats.feedback_sent, r.b.stats.feedback_sent);

        // Stop capturing pictures so the final in-flight fragments and their reports can arrive.
        let (mut out, mut events) = (Vec::new(), Vec::new());
        let report_by = r.now + IDLE_FEEDBACK_US + rtt;
        while r.now < report_by {
            r.tick(&mut out, &mut events, |_, _| {});
        }
        let cache = &r.a.out[&stream::CAMERA_HIGH].cache[&0];
        let unseen: Vec<_> = cache.unseen_sequences().collect();
        assert!(!cache.is_empty() && unseen.is_empty(), "clean arrival reports must mark every cached sequence seen: {} cached, unseen {unseen:?}", cache.len());
        assert_eq!(r.a.stats.repairs, 0, "clean arrival reports must trigger zero sender repairs");
    }

    #[test]
    fn loss_is_measured_and_video_recovers_by_resends() {
        let path = PathConfig { latency_us: 20_000, jitter_us: 4_000, loss_ppm: 50_000, ..Default::default() };
        for seed in [1, 3, 5, 7, 9, 11, 13, 15] {
            let mut r = Rig::with_seed(path, seed);
            r.video_attempts = Some(BTreeMap::new());
            // Two fragments per delta, six per key, about 70% of the initial pace on the wire.
            r.fixed_picture_bytes = Some(2_000);
            r.run(3_000);
            eprintln!("5% loss seed {seed}: {}/{} pictures, {}/{} voice frames, {} resends, {} lost and {} delivered hops",
                r.video_got.len(), r.video_sent, r.voice_got.len(), r.voice_sent, r.a.stats.resent, r.a.stats.hops_lost, r.a.stats.hops_delivered);
            let voice = r.voice_got.len() as f64 / r.voice_sent as f64;
            assert!((0.90..0.99).contains(&voice), "seed {seed}: voice is not resent: {voice:.3} arrived");
            let video = r.video_got.len() as f64 / r.video_sent as f64;
            assert!(video >= 0.95, "seed {seed}: {video:.3} pictures handed on by 3 s, {} resends", r.a.stats.resent);
            let (lost, delivered) = (r.a.stats.hops_lost as f64, r.a.stats.hops_delivered as f64);
            let loss = lost / (lost + delivered);
            assert!((0.03..0.08).contains(&loss), "seed {seed}: a's loss estimate {loss:.3} on a 5% path");
            assert!(r.a.rtt_us().is_some_and(|t| (40_000..50_000).contains(&t)), "seed {seed}: round trip under jitter: {:?} µs", r.a.rtt_us());

            // The final picture is captured at 2.970 s. Let its cached resends reach the receiver.
            let deadline = r.now + video::CACHE_US + path.latency_us + path.jitter_us;
            let (mut out, mut events) = (Vec::new(), Vec::new());
            while r.video_got.len() < r.video_sent as usize && r.now < deadline {
                r.step(1_000, &mut out, &mut events, |_, _| {});
            }
            // The draw decides how many originals were lost, so check a resend for each one.
            let attempts = r.video_attempts.as_ref().unwrap();
            let dropped = attempts.values().filter(|v| v.original_dropped).count();
            let missing: Vec<_> = attempts.iter().filter(|(_, v)| v.original_dropped && !v.resent).map(|(id, _)| id).collect();
            eprintln!("seed {seed}: {}/{dropped} lost video originals resent, {} total resends, {}/{} pictures shown by {} µs",
                dropped - missing.len(), r.a.stats.resent, r.video_got.len(), r.video_sent, r.now);
            assert!(missing.is_empty(), "seed {seed}: {dropped} video originals dropped, never resent (stream, key, seq): {missing:?}");
            assert_eq!((r.video_sent, r.video_got.len()), (90, 90), "seed {seed}: all 90 encoded pictures should be shown in order");
        }
    }

    #[test]
    fn an_outage_asks_for_a_key_frame_and_the_picture_comes_back() {
        let mut r = Rig::new(PathConfig { latency_us: 10_000, ..Default::default() });
        r.run(1_000);
        let before = r.video_got.len();
        let outage_start = r.now;
        r.outage = true;
        r.run(1_500);
        let returned = r.now;
        // A queued original already occupies cache space, but was never sent into the gap.
        let cache = &r.a.out[&stream::CAMERA_HIGH].cache[&0];
        let cached = r.a.sent.iter().filter(|h| h.at > outage_start)
            .filter_map(|h| h.video.map(|(_, _, seq)| seq))
            .filter(|&seq| cache.find(seq, 1).next().is_some())
            .collect::<std::collections::BTreeSet<_>>().len();
        let before_resends = r.a.stats;
        let at_return = r.video_got.len();
        r.outage = false;
        r.trace_resends = true;
        r.run(1_000);
        let recovery = r.shown_at.get(at_return).map(|at| at - returned);
        let resends = r.a.stats.resent - before_resends.resent;
        let repairs = r.a.stats.repairs - before_resends.repairs;
        let bound = IDLE_FEEDBACK_US + FEEDBACK_US + 4 * r.ab.config.latency_us;
        eprintln!("O: keys made (frame, µs) {:?}, first after return {:?} at {:?} (+{recovery:?} µs, bound {bound}), {} shown in recovery second, {} NACK resends, {} sender repairs, {cached} cached",
            r.keys_made, r.video_got.get(at_return), r.shown_at.get(at_return), r.video_got.len() - at_return, resends - repairs, repairs);
        let keys: Vec<u32> = r.video_got.iter().filter(|(_, k)| *k).map(|(f, _)| *f).collect();
        assert_eq!(keys.len(), 2, "a second key frame, asked for after the outage: {keys:?}");
        let after = r.video_got.len() - before;
        assert!(after >= 25, "pictures again within a second of the outage: {after}");
        let gen = r.b.video_stats().next().unwrap().1;
        assert!(gen.key_requests > 0 && gen.skipped > 0, "b asked for the key frame and passed over what it couldn't use: {gen:?}");
        assert!(r.keys_made.len() == 2 && r.keys_made[1].1 < returned, "O needs exactly two keys, the second before {returned} µs: {:?}", r.keys_made);
        let key_fragments = r.sizes[r.keys_made[1].0 as usize].div_ceil(wire::MAX_FRAGMENT);
        assert!(recovery.is_some_and(|us| us <= bound), "O recovery {recovery:?} µs exceeded {bound} µs");
        assert!((key_fragments as u64..cached as u64).contains(&resends), "O resent {resends} packets after return, expected at least {key_fragments} and fewer than {cached}");
        assert!(r.video_got.len() - at_return >= 25, "O showed only {} pictures in the recovery second", r.video_got.len() - at_return);
    }

    #[test]
    fn after_a_two_way_outage_a_new_key_starts_the_chain_without_nacking_the_gap() {
        let mut r = Rig::new(PathConfig { latency_us: 10_000, ..Default::default() });
        r.run(1_000);
        r.outage = true;
        r.reverse_outage = true;
        r.run(1_500);
        let returned = r.now;
        let shown_before = r.video_got.len();
        r.outage = false;
        r.reverse_outage = false;
        let end = returned + 1_000_000;
        while r.keys_made.len() == 1 && r.now < end {
            r.run(1);
        }
        let pace = r.a.pace_bps();
        r.run((end - r.now) / 1_000);
        assert_eq!(r.keys_made.len(), 2, "O2 should encode two keys across the outage: {:?}", r.keys_made);
        let (frame, captured) = r.keys_made[1];
        let size = r.sizes[frame as usize];
        let wire_bytes = size + size.div_ceil(wire::MAX_FRAGMENT) * (wire::MAX_DATAGRAM - wire::MAX_FRAGMENT);
        let pacing_us = (wire_bytes as u64 * 8_000_000).div_ceil(pace);
        let bound = IDLE_FEEDBACK_US + 4 * r.ab.config.latency_us + 33_000 + pacing_us;
        let first = r.shown_at.get(shown_before).copied();
        r.report_repair("O2", returned, first, bound);
        assert!(captured > returned, "O2 replacement key captured at {captured} µs should follow return at {returned}");
        let received = r.b.video_stats().next().unwrap().1;
        assert_eq!(received.key_frames, 2, "O2 must show both keys: {received:?}");
        assert_eq!(received.nacked, 0, "O2 must leave the cleared outage gap alone: {received:?}");
        assert!(first == r.shown(frame) && first.is_some_and(|t| t - returned <= bound), "O2 first picture {first:?}, replacement key {:?}, return {returned}, bound {bound} µs", r.shown(frame));
    }

    // After REORDER_US, the next voice hop leaves within one voice interval and reaches
    // the receiver half an RTT later. Its arrival report leaves within FEEDBACK_US.
    // Half an RTT later the sender has that evidence and repairs at once. The repair
    // takes a final half RTT to reach the receiver.
    fn sender_repair_bound_us(rtt: u64) -> u64 {
        video::REORDER_US + VOICE_INTERVAL_US + FEEDBACK_US + 3 * rtt / 2
    }

    #[test]
    fn a_lost_mid_call_key_is_repaired_with_or_without_the_following_delta() {
        const KEY_AT: u64 = 3_000_000;
        const INTERVAL: u64 = 33_000;
        let mut results = Vec::new();
        for drop_delta in [false, true] {
            let case = if drop_delta { "K2" } else { "K" };
            let mut r = Rig::new(PathConfig::default());
            let rtt = 2 * r.ab.config.latency_us;
            let bound = sender_repair_bound_us(rtt);
            let (mut out, mut events) = (Vec::new(), Vec::new());
            let (mut frame, mut asks_before) = (0, 0);
            while r.now < KEY_AT + video::KEY_INTERVAL_US + INTERVAL {
                r.step(1_000, &mut out, &mut events, |r, out| {
                    if r.now.abs_diff(KEY_AT) % INTERVAL != 0 {
                        return;
                    }
                    let forced = r.now == KEY_AT;
                    let key = r.video_sent == 0 || forced || r.a.key_frame_due(stream::CAMERA_HIGH, r.now);
                    if forced {
                        frame = r.video_sent;
                        asks_before = r.b.video_stats().next().unwrap().1.key_requests;
                    }
                    r.send_picture(if key { 2 * wire::MAX_FRAGMENT } else { 700 }, key, out);
                    if forced {
                        let (_, seqs) = r.a.out[&stream::CAMERA_HIGH].newest_key.as_ref().unwrap();
                        let end = *seqs.end() + u64::from(drop_delta);
                        r.drop_once.extend((*seqs.start()..=end).map(|seq| (stream::CAMERA_HIGH, seq)));
                    }
                });
            }
            let shown = r.shown(frame);
            let asks = r.b.video_stats().next().unwrap().1.key_requests - asks_before;
            let last_drop = r.last_drop_us.expect("the forced key was lost");
            r.report_repair(case, last_drop, shown, bound);
            results.push((case, drop_delta, shown, last_drop, bound, asks, r.keys_made, r.video_got));
        }
        for (case, drop_delta, _, _, _, asks, keys, got) in &results {
            assert_eq!(keys.len(), 2, "{case} must recover the forced key without encoding another: {keys:?}");
            assert_eq!(*asks > 0, *drop_delta, "{case} key-request polls after loss: {asks}, following delta dropped: {drop_delta}");
            let key = keys[1].0;
            assert!(got.contains(&(key + 1, false)) && got.contains(&(key + 2, false)), "{case} missing pictures after key {key}");
        }
        for (case, _, shown, last_drop, bound, _, _, _) in results {
            assert!(shown.is_some_and(|t| t <= last_drop + bound), "{case} key shown at {shown:?} µs, last lost send {last_drop}, bound {bound} µs");
        }
    }

    #[test]
    fn voice_reports_repair_an_unexposed_video_tail_at_both_round_trips() {
        let mut results = Vec::new();
        for rtt in [20_000, 300_000] {
            let mut r = Rig::new(PathConfig { latency_us: rtt / 2, ..Default::default() });
            r.fixed_picture_bytes = Some(700);
            r.run(3_000);
            let capture = r.now;
            let frame = r.video_sent;
            let first: usize = r.sizes.iter().map(|n| n.div_ceil(wire::MAX_FRAGMENT)).sum();
            r.drop_once.extend([first + 1, first + 2].map(|seq| (stream::CAMERA_HIGH, seq as u64)));
            let (mut out, mut events) = (Vec::new(), Vec::new());
            r.send_picture(3 * wire::MAX_FRAGMENT, false, &mut out);
            r.send_forward(&mut out);
            while r.now < capture + 2_000_000 {
                r.tick(&mut out, &mut events, |_, _| {});
            }
            let bound = sender_repair_bound_us(rtt);
            let shown = r.shown(frame);
            let received = r.b.video_stats().next().unwrap().1;
            let last_drop = r.last_drop_us.expect("the picture's tail was lost");
            r.report_repair(&format!("T RTT {rtt}"), last_drop, shown, bound);
            results.push((rtt, shown, last_drop, bound, received, r.a.stats));
        }
        for (rtt, _, _, _, received, sent) in &results {
            assert_eq!((received.key_requests, received.nacked), (0, 0), "T RTT {rtt} µs must need no key or NACK");
            assert_eq!((sent.resent, sent.repairs), (2, 2), "T RTT {rtt} µs must repair exactly the two lost tail fragments");
        }
        for (rtt, shown, last_drop, bound, _, _) in results {
            assert!(shown.is_some_and(|t| t <= last_drop + bound), "T RTT {rtt} µs, picture shown {shown:?}, last lost send {last_drop}, bound {bound} µs");
        }
    }

    #[test]
    fn a_paced_key_needs_no_repair_when_only_a_voice_hop_is_lost() {
        let mut r = Rig::new(PathConfig { latency_us: 50, ..Default::default() });
        let (mut out, mut events) = (Vec::new(), Vec::new());
        r.drop_once.insert((stream::VOICE, 0));
        r.send_picture(20_000, true, &mut out);
        r.send_forward(&mut out);
        while r.now < 500_000 {
            r.step(50, &mut out, &mut events, |_, _| {});
        }
        let received = r.b.video_stats().next().unwrap().1;
        eprintln!("P: key 0 captured at 0, shown {:?} µs, {} resends, {} sender repairs", r.shown(0), r.a.stats.resent, r.a.stats.repairs);
        assert_eq!((received.frames, received.fragments), (1, 18), "P should show the entire 18-fragment paced key: {received:?}");
        assert_eq!((received.key_requests, received.nacked), (0, 0), "P saw a pacing pause as a lost fragment: {received:?}");
        assert_eq!((r.a.stats.resent, r.a.stats.repairs), (0, 0), "P a lost voice hop triggered video repair: {:?}", r.a.stats);
        assert_eq!(r.a.stats.hops_lost, 1, "P must settle one lost voice hop");
    }

    #[test]
    fn a_burst_after_a_young_key_recovers_before_the_gate_reopens() {
        let mut r = Rig::new(PathConfig { latency_us: 10_000, ..Default::default() });
        r.fixed_picture_bytes = Some(1_000);
        r.run(260);
        assert!(r.shown(0).is_some(), "Y needs the first key before the burst");
        r.outage = true;
        r.run(300);
        let returned = r.now;
        let shown_before = r.shown_at.len();
        r.outage = false;
        r.trace_resends = true;
        r.run(470);
        let first = r.shown_at.get(shown_before).copied();
        let bound = video::REORDER_US + 4 * r.ab.config.latency_us;
        r.report_repair("Y", returned, first, bound);
        assert_eq!(r.keys_made.len(), 1, "Y must recover on the young key before its gate reopens: {:?}", r.keys_made);
        assert!(first.is_some_and(|at| at - returned <= bound), "Y first picture {first:?} after return at {returned} exceeded {bound} µs");
    }

    #[test]
    fn the_estimate_follows_a_bottleneck_and_keeps_its_queue_short() {
        let mut r = Rig::new(PathConfig { latency_us: 20_000, rate_bps: 1_000_000, queue_us: 300_000, ..Default::default() });
        r.adaptive = true;
        r.run(6_000);
        let (got, est) = (r.video_got.len(), r.a.estimate_bps());
        assert!((600_000..1_300_000).contains(&est), "a's estimate {est} bit/s on a 1 Mbit/s path");
        r.run(4_000);
        let est = r.a.estimate_bps();
        assert!((600_000..1_300_000).contains(&est), "and it stays there: {est} bit/s");
        let later = r.video_got.len() - got;
        assert!(later >= 100, "pictures keep coming at the rate the path holds: {later} in 4 s");
        assert!(r.ab.stats.queue_drops * 50 < r.ab.stats.sent, "the bottleneck's queue rarely overflows: {:?}", r.ab.stats);
        assert!(r.a.queue_delay_us() < 150_000, "a short queue: {} µs", r.a.queue_delay_us());
    }

    struct GuardedCall {
        rig: Rig,
        // End rung, mean estimate, bits sent and captures skipped in each second.
        seconds: Vec<(Rung, u64, u64, u32)>,
        steps: Vec<(u64, Rung)>,
        max_gap_us: u64,
        max_pacer_us: u64,
        max_queue_us: u64,
        reached_800k: Option<u64>,
        first_release_us: Option<u64>,
        up_to_640: u32,
    }

    fn guarded_call(path: PathConfig, seconds: u64, start: usize, model: impl Fn(f64, u32, u64, u32, bool) -> usize) -> GuardedCall {
        let mut r = Rig::new(path);
        let mut guard = Guard::new(start);
        let mut needs_key = true;
        let mut first_known_capture = None;
        let mut first_release_us = None;
        let mut steps = Vec::new();
        let mut trace = Vec::new();
        let mut last_picture = 2_000_000;
        let mut max_gap_us = 0;
        let mut max_pacer_us = 0;
        let mut max_queue_us = 0;
        let mut reached_800k = None;
        let mut up_to_640 = 0;
        let mut out = Vec::new();
        let mut events = Vec::new();
        for _ in 0..seconds {
            let mut skips = 0;
            let mut estimates = 0;
            let sent_before = r.a.stats.sent_bytes;
            for _ in 0..1_000 {
                let pictures_before = r.video_got.len();
                r.tick(&mut out, &mut events, |r, out| {
                    let before = guard.rung();
                    let level = r.a.video_loss_level();
                    let decision = guard.tick(r.now, r.a.estimate_bps(), r.a.pacer_delay_us(r.now), level);
                    if let Some(level) = level {
                        first_known_capture.get_or_insert((r.now, r.a.estimate_bps(), guard.ask_bps()));
                        if level <= 0.20 {
                            first_release_us.get_or_insert(r.now);
                        }
                    }
                    let rung = guard.rung();
                    let step = guard.take_step();
                    assert_eq!(step.map(|s| (s.from, s.to)), (before != rung).then_some((before, rung)), "each rung change must be reported");
                    assert!(guard.take_step().is_none(), "a step must be consumed only once");
                    if step.is_some() {
                        if rung == LADDER[2] && before.width < rung.width {
                            up_to_640 += 1;
                        }
                        steps.push((r.now, rung));
                        needs_key = true;
                    }
                    if decision == Tick::Skip {
                        skips += 1;
                        return;
                    }
                    let requested = r.a.key_frame_due(stream::CAMERA_HIGH, r.now);
                    let key = std::mem::take(&mut needs_key) || requested;
                    let pixels = rung.width * rung.height * rung.fps;
                    let scale = (f64::from(pixels) / (1280.0 * 720.0 * 30.0)).powf(0.75);
                    let bytes = model(scale, rung.fps, guard.ask_bps(), r.video_sent, key);
                    guard.encoded(r.now, bytes, key);
                    r.send_picture(bytes, key, out);
                });
                if r.now >= 2_000_000 {
                    max_gap_us = max_gap_us.max(r.now - last_picture);
                    if r.video_got.len() > pictures_before {
                        last_picture = r.now;
                    }
                }
                if !steps.is_empty() {
                    max_pacer_us = max_pacer_us.max(r.a.pacer_delay_us(r.now));
                }
                estimates += r.a.estimate_bps();
                max_queue_us = max_queue_us.max(r.a.queue_delay_us());
                if r.a.estimate_bps() >= 800_000 {
                    reached_800k.get_or_insert(r.now);
                }
            }
            trace.push((guard.rung(), estimates / 1_000, (r.a.stats.sent_bytes - sent_before) * 8, skips));
        }
        let mut report = String::new();
        use std::fmt::Write;
        for (i, (rung, mean, sent, skips)) in trace.iter().enumerate() {
            writeln!(report, "{} {rung} {mean} {sent} {skips}", i + 1).unwrap();
        }
        eprintln!("guarded path {path:?}\ns rung mean_bit/s sent_bit/s skips\n{report}steps {steps:?}, max delivery gap {max_gap_us} µs, peak pacer age after step {max_pacer_us} µs, {}/{} pictures, {} resends, {} lost/{} delivered hops, RTT {:?} µs",
            r.video_got.len(), r.video_sent, r.a.stats.resent, r.a.stats.hops_lost, r.a.stats.hops_delivered, r.a.rtt_us());
        eprintln!("peak path queue {max_queue_us} µs, 800 kbit/s at {reached_800k:?} µs, {up_to_640} probes into 640x360@30, first known capture (µs, estimate, ask) {first_known_capture:?}, first reading at or under 20% at {first_release_us:?} µs");
        GuardedCall { rig: r, seconds: trace, steps, max_gap_us, max_pacer_us, max_queue_us, reached_800k, first_release_us, up_to_640 }
    }

    #[test]
    fn guard_adapts_an_encoder_with_a_cost_floor_on_a_bottleneck() {
        let path = PathConfig { rate_bps: 1_000_000, queue_us: 300_000, latency_us: 20_000, ..Default::default() };
        let call = guarded_call(path, 30, Guard::rung_for(video_ask(START_BPS)), |scale, fps, ask, _, key| {
            let bits = (4_000_000.0 * scale).max(ask as f64) / f64::from(fps);
            (bits / 8.0 * if key { 3.0 } else { 1.0 }).round() as usize
        });
        for (second, &(_, mean, sent, _)) in call.seconds.iter().enumerate().skip(5) {
            assert!((6 * mean..=12 * mean).contains(&(10 * sent)), "second {} sent {sent} bit/s against mean estimate {mean}", second + 1);
        }
        assert!(call.max_queue_us < 100_000, "peak measured path queue was {} µs", call.max_queue_us);
        assert!(call.first_release_us.is_some_and(|at| at <= 1_500_000), "the first capture with loss at or under 20% must come by 1.5 s, got {:?} µs", call.first_release_us);
        assert!(call.reached_800k.is_some_and(|at| at <= 6_000_000), "must reach 800 kbit/s by 6 s, reached at {:?} µs", call.reached_800k);
        assert!(call.up_to_640 <= 3, "too many upward probes into 640x360@30: {:?}", call.steps);
        assert!(call.max_gap_us <= 1_000_000, "longest picture gap after 2 s was {} µs", call.max_gap_us);
    }

    #[test]
    fn guard_keeps_oversized_pictures_moving_under_five_percent_loss() {
        let path = PathConfig { latency_us: 20_000, jitter_us: 4_000, loss_ppm: 50_000, ..Default::default() };
        let call = guarded_call(path, 10, 0, |scale, _, _, n, key| {
            let bytes = if key { 20_000 } else { 3_000 + (n % 7) * 100 };
            (f64::from(bytes) * scale).round() as usize
        });
        let r = &call.rig;
        assert!(call.seconds.iter().any(|s| s.3 > 0) && !call.steps.is_empty(), "the oversized source must skip captures and step down");
        assert!(call.max_gap_us <= 1_000_000, "longest picture gap after 2 s was {} µs", call.max_gap_us);
        assert!(100 * r.video_got.len() >= 95 * r.video_sent as usize, "only {}/{} encoded pictures arrived", r.video_got.len(), r.video_sent);
        assert!(r.a.stats.resent > 0, "lost fragments must trigger resends");
        let lost = r.a.stats.hops_lost;
        let total = lost + r.a.stats.hops_delivered;
        assert!((3 * total..=8 * total).contains(&(100 * lost)) && total > 0, "measured {lost} lost of {total} settled hops");
        assert!(r.a.rtt_us().is_some_and(|us| (40_000..50_000).contains(&us)), "round trip under jitter was {:?} µs", r.a.rtt_us());
        assert!(call.max_pacer_us < 200_000, "oldest paced fragment after the first step reached {} µs", call.max_pacer_us);
    }

    #[test]
    fn heavy_random_loss_keeps_video_at_its_start_while_stream_capacity_grows() {
        let start_ask = video_ask(START_BPS);
        let start = Guard::rung_for(start_ask);
        let mut results = Vec::new();
        for loss_percent in [40, 30, 25] {
            let path = PathConfig { loss_ppm: loss_percent * 10_000, ..Default::default() };
            let seconds = if loss_percent == 30 { 40 } else { 10 };
            for seed in [1, 3, 5, 7, 9, 11, 13, 15].into_iter().chain((loss_percent == 30).then_some(211)) {
                let mut r = Rig::with_seed(path, seed);
                let mut guard = Guard::new(start);
                let (mut low, mut high) = (start_ask, start_ask);
                let mut first_outside = None;
                let mut best = guard.rung();
                let (mut out, mut events) = (Vec::new(), Vec::new());
                // The driver gives the stream the estimate left after the last second of media.
                let stream_cap = |r: &Rig| r.a.estimate_bps().saturating_sub(8 * r.a.sent_since(r.now, 1_000_000).1 as u64).max(64_000);
                let mut cap_at_two = 0;
                let (mut est, mut cap) = (0, 0);
                let mut last_second_peak = 0;
                let mut asks = Vec::new();
                while r.now < seconds * 1_000_000 {
                    r.tick(&mut out, &mut events, |r, out| {
                        let tick = guard.tick(r.now, r.a.estimate_bps(), r.a.pacer_delay_us(r.now), r.a.video_loss_level());
                        low = low.min(guard.ask_bps());
                        high = high.max(guard.ask_bps());
                        if !(9 * start_ask..=11 * start_ask).contains(&(10 * guard.ask_bps())) {
                            first_outside.get_or_insert((r.now, r.a.estimate_bps(), r.a.random_loss(), r.a.video_loss_level(), guard.ask_bps()));
                        }
                        if guard.rung().min_ask_bps > best.min_ask_bps {
                            best = guard.rung();
                        }
                        if tick == Tick::Encode {
                            r.send_at_guard_ask(&mut guard, false, out);
                        }
                    });
                    if r.now >= 39_000_000 {
                        last_second_peak = last_second_peak.max(guard.ask_bps());
                    }
                    if r.now == 2_000_000 {
                        cap_at_two = stream_cap(&r);
                    }
                    if r.now == 10_000_000 {
                        est = r.a.estimate_bps();
                        cap = stream_cap(&r);
                    }
                    if seed == 211 && matches!(r.now, 10_000_000 | 20_000_000 | 40_000_000) {
                        asks.push(guard.ask_bps());
                    }
                }
                if loss_percent == 30 {
                    eprintln!("30% seed {seed}: 40 s ask {}, last-second peak {last_second_peak}", guard.ask_bps());
                    assert!(2 * last_second_peak <= 3 * start_ask, "30% seed {seed}, last-second peak {last_second_peak} exceeded 1.5 times the start ask {start_ask}");
                }
                if seed == 211 {
                    // The receiver keeps gaps behind a young key, and the sender repairs a lost key instead of making another.
                    // Seed 211 asks for 69 keys/min, down from about 325, and its video ask is back at its start at all three checks.
                    assert_eq!(asks, [198_000, 198_000, 198_000], "30% seed 211 asks at 10, 20 and 40 s, bit/s: {asks:?}");
                }
                let peak_ratio = high as f64 / start_ask as f64;
                eprintln!("{loss_percent}% session seed {seed}: 10 s estimate {est}, stream cap {cap_at_two} at 2 s -> {cap} at 10 s, ask {low}..{high} final {}, peak/start {peak_ratio:.6}, rung {} highest {best}, level {:.6}, video level {:?}, first outside hold (µs, estimate, level, video level, ask) {first_outside:?}", guard.ask_bps(), guard.rung(), r.a.random_loss(), r.a.video_loss_level());
                results.push((loss_percent, seed, est, cap_at_two, cap, low, high, best));
            }
        }
        for (loss_percent, seed, est, cap_at_two, cap, low, high, best) in results {
            if loss_percent == 25 {
                continue;
            }
            assert!(est > START_BPS, "{loss_percent}% session seed {seed} should grow from {START_BPS} bit/s, 10 s estimate {est}");
            assert!(cap > cap_at_two, "{loss_percent}% session seed {seed} should grow the stream cap, 2 s {cap_at_two}, 10 s {cap} bit/s");
            assert!(10 * low >= 9 * start_ask, "{loss_percent}% session seed {seed} fell below 0.9 times its {start_ask} bit/s ask, saw {low}..{high}");
            if loss_percent == 40 {
                assert!(10 * high <= 11 * start_ask, "40% session seed {seed} exceeded 1.1 times its {start_ask} bit/s ask, saw {low}..{high}");
                assert_eq!(best, LADDER[start], "40% session seed {seed} climbed from {} to {best}", LADDER[start]);
            }
        }
    }

    #[test]
    fn sustained_heavy_loss_returns_a_settled_video_ask_to_its_start() {
        let path = PathConfig { rate_bps: 2_000_000, queue_us: 300_000, loss_ppm: 10_000, ..Default::default() };
        let mut r = Rig::new(path);
        let start = video_ask(START_BPS);
        let mut guard = Guard::new(Guard::rung_for(start));
        let (mut out, mut events) = (Vec::new(), Vec::new());
        let mut needs_key = false;
        let mut last_tick = 0;
        let mut reading = None;
        let mut falling_from = None;
        let mut checked = 0;
        let mut floor_at = None;
        let mut first_heavy = None;
        let mut final_peak = 0;
        while r.now < 40_000_000 {
            if r.now == 10_000_000 {
                r.ab.config.loss_ppm = 350_000;
                r.ba.config.loss_ppm = 350_000;
                assert!(guard.ask_bps() > 2 * start, "the 1% path should enter the loss step above twice the starting ask {start}, got {}", guard.ask_bps());
            }
            r.tick(&mut out, &mut events, |r, out| {
                let before = guard.ask_bps();
                let estimate = r.a.estimate_bps();
                reading = r.a.video_loss_level();
                let tick = guard.tick(r.now, estimate, r.a.pacer_delay_us(r.now), reading);
                let ask = guard.ask_bps();
                if let Some(step) = guard.take_step() {
                    eprintln!("loss step at {} µs: {} -> {}, {}, ask {ask}", r.now, step.from, step.to, step.why);
                    needs_key = true;
                }
                if tick == Tick::Encode {
                    r.send_at_guard_ask(&mut guard, std::mem::take(&mut needs_key), out);
                }
                if r.now > 10_000_000 {
                    if reading.is_some_and(|level| level >= 0.30) {
                        first_heavy.get_or_insert((r.now, reading, before, ask));
                    }
                    assert!(ask >= start, "loss step at {} µs fell below the starting ask {start} to {ask}, path {estimate}", r.now);
                    if ask == start {
                        floor_at.get_or_insert(r.now);
                    }
                    // Measure uninterrupted heavy readings above the floor with room on the path.
                    if reading.is_some_and(|level| level >= 0.30) && ask > start && video_ask(estimate) >= before {
                        let (at, initial) = *falling_from.get_or_insert((last_tick, before));
                        if r.now - at >= 1_000_000 {
                            let video = 80_000.0 + initial as f64 / 0.9;
                            let expected = video_ask((video * 0.92_f64.powf((r.now - at) as f64 / 1_000_000.0)) as u64);
                            // The initial and final asks both round to whole bit/s.
                            assert!(ask.abs_diff(expected) <= 2, "8%/s video fall from ask {initial} at {at} µs: expected ask {expected} ±2 bit/s at {}, got {ask}", r.now);
                            checked += 1;
                            falling_from = None;
                        }
                    } else {
                        falling_from = None;
                    }
                }
                last_tick = r.now;
            });
            if r.now >= 39_000_000 {
                final_peak = final_peak.max(guard.ask_bps());
            }
            if r.now % 1_000_000 == 0 {
                eprintln!("loss step second {} reading {reading:?} ask {} rung {} path {}", r.now / 1_000_000, guard.ask_bps(), guard.rung(), r.a.estimate_bps());
            }
        }
        eprintln!("loss step: first heavy capture (µs, reading, previous ask, ask) {first_heavy:?}, {checked} one-second falls, starting ask reached at {floor_at:?} µs, final-second peak {final_peak}");
        assert!(checked > 0, "the 35% path should show a full second of 8%/s video fall, saw {checked} windows");
        assert_eq!(final_peak, start, "the 35% path must hold the starting ask {start} throughout its last second, saw {final_peak}");
    }

    #[test]
    fn short_estimate_dips_skip_pictures_without_stepping_down() {
        let base = 1_080_000;
        let start = Guard::rung_for(video_ask(base));
        let bytes = (video_ask(base) / u64::from(LADDER[start].fps) / 8) as usize;
        for cut in [15, 50] {
            for duration in [700_000, 1_400_000] {
                let mut r = Rig::new(PathConfig::default());
                r.a.rate = RateEstimator::new(base);
                let mut guard = Guard::new(start);
                let (mut out, mut events) = (Vec::new(), Vec::new());
                let (mut skips, mut captures) = (0, 0);
                let mut first_step = None;
                // An established rung sees one dip, followed by a full down-step window at the old rate.
                let dip = 2_000_000..2_000_000 + duration;
                while r.now < dip.end + 2_000_000 {
                    r.tick(&mut out, &mut events, |r, out| {
                        let dipping = dip.contains(&r.now);
                        let estimate = if dipping { base * (100 - cut) / 100 } else { base };
                        let tick = guard.tick(r.now, estimate, r.a.pacer_delay_us(r.now), r.a.video_loss_level());
                        if dipping {
                            captures += 1;
                            skips += u32::from(tick == Tick::Skip);
                        }
                        if let Some(step) = guard.take_step() {
                            first_step.get_or_insert((r.now, step));
                        }
                        if tick == Tick::Encode {
                            let key = r.video_sent == 0 || r.a.key_frame_due(stream::CAMERA_HIGH, r.now);
                            // Keep charging the pre-dip picture size while the lower ask drains its debt.
                            guard.encoded(r.now, bytes, key);
                            r.send_picture(bytes, key, out);
                        }
                    });
                }
                eprintln!("guard dip {cut}% for {duration} µs: skipped {skips}/{captures} captures during dip, first step {first_step:?}, final rung {}", guard.rung());
                assert!(first_step.is_none(), "{cut}% dip for {duration} µs must preserve {}, first step {first_step:?} (skipped {skips}/{captures} captures during dip)", LADDER[start]);
            }
        }
    }

    #[test]
    fn foreign_replayed_and_forged_datagrams_are_refused() {
        let mut r = Rig::new(PathConfig::default());
        let v = Voice { flags: 0, seq: 0, capture_us: 0, payload: b"hi" };
        let d = r.a.send_voice(0, stream::VOICE, &v, false).unwrap();
        let mut events = Vec::new();
        r.b.receive(0, &d, &mut events);
        assert_eq!(events.len(), 1, "the true datagram opens");
        r.b.receive(0, &d, &mut events);
        assert_eq!((events.len(), r.b.stats.replayed), (1, 1), "a replay is refused");

        let d = r.a.send_voice(0, stream::VOICE, &v, false).unwrap();
        for byte in 3..d.len() {
            let mut forged = d.clone();
            forged[byte] ^= 0x04;
            r.b.receive(0, &forged, &mut events);
        }
        assert_eq!(events.len(), 1, "no flipped bit from the source tag to the tag gets through");
        assert_eq!(r.b.stats.bad_tag + r.b.stats.refused, (d.len() - 3) as u64, "each one refused");
        let mut hop = d.clone();
        Header::set_hop(&mut hop, 999);
        r.b.receive(0, &hop, &mut events);
        assert_eq!(events.len(), 2, "the hop is outside the tag: a forwarder may rewrite it");

        let own = r.b.send_voice(0, stream::VOICE, &v, false).unwrap();
        r.b.receive(0, &own, &mut events);
        assert_eq!(events.len(), 2, "a datagram of one's own is not the other side's");
        let other = sender_tag(b"another call", &device(1));
        assert_ne!(other, r.a.my_src, "the source tag depends on the call");

        // One second at 150 Mbit/s advances 150,000,000 / (1,152 × 8) = 16,276 full fragments.
        let cache_distance = 150_000_000 / (wire::MAX_FRAGMENT as u64 * 8);
        let window = wire::VIDEO_WINDOW as u64;
        let newest_seq = cache_distance.max(window);
        let mut early = Vec::new();
        let mut expired = Vec::new();
        for _ in 0..newest_seq {
            let (_, seq, packet) = r.a.seal(stream::CAMERA_HIGH, 0, &[]).unwrap();
            if newest_seq - seq == cache_distance {
                early = packet;
            } else if newest_seq - seq == window {
                expired = packet;
            }
        }
        let (_, _, newest) = r.a.seal(stream::CAMERA_HIGH, 0, &[]).unwrap();
        let received = r.b.stats.received;
        r.b.receive(video::CACHE_US, &newest, &mut events);
        assert_eq!(r.b.stats.received, received + 1, "the newest video datagram must open at sequence {newest_seq}");
        r.b.receive(video::CACHE_US, &early, &mut events);
        assert_eq!(r.b.stats.received, received + 2, "a cached video datagram {cache_distance} sequences behind the newest must open");
        let replayed = r.b.stats.replayed;
        r.b.receive(video::CACHE_US, &expired, &mut events);
        assert_eq!(r.b.stats.replayed, replayed + 1, "an unseen video datagram {window} sequences behind the newest must be refused as a replay");
    }
}
