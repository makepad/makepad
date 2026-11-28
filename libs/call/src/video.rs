//! Video over datagrams: an access unit cut into fragments, a sender's cache of what it sent
//! for resends, key frames coalesced, and the receiver's side: missing sequences found and
//! asked for again, fragments put back into whole frames, and frames handed on only when the
//! frame they predict from was (Fable, WORKCALL Q6).
//!
//! All of it is sans-IO: times are microseconds on the caller's clock.

use crate::wire::{Fragment, NackRange, MAX_FRAGMENT, VIDEO_WINDOW};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How long a sender keeps what it sent, for resends.
pub const CACHE_US: u64 = 1_000_000;
/// The least time between two key frames of one stream; requests inside it share one.
pub const KEY_INTERVAL_US: u64 = 1_000_000;
// Sender key repair relies on the newest key staying cached while the gate holds the next one.
const _: () = assert!(CACHE_US >= KEY_INTERVAL_US);
// A resend reuses its sequence: the replay window holds CACHE_US of full fragments at
// 150 Mbit/s (150 bits a µs), so no cached resend is refused below that rate.
const _: () = assert!(VIDEO_WINDOW as u64 > 150 * CACHE_US / (MAX_FRAGMENT as u64 * 8));
/// A gap younger than this may still be reordering, not loss.
pub const REORDER_US: u64 = 10_000;
/// Give a lost fragment three chances before asking for a key. Resends can still win the race.
const RESEND_CHANCES: u8 = 3;

/// Cuts an access unit into fragments whose sizes differ by at most a byte, none over
/// [`MAX_FRAGMENT`].
pub fn fragments<'a>(au: &'a [u8], frame: u32, base: u16, key: bool, capture_us: u64) -> impl Iterator<Item = Fragment<'a>> {
    fragments_at(au, frame, base, key, capture_us, MAX_FRAGMENT)
}

pub(super) fn fragments_at<'a>(au: &'a [u8], frame: u32, base: u16, key: bool, capture_us: u64, payload_bytes: usize) -> impl Iterator<Item = Fragment<'a>> {
    let count = au.len().div_ceil(payload_bytes).max(1);
    let (size, extra) = (au.len() / count, au.len() % count);
    let flags = if key { crate::wire::video_flags::KEY } else { 0 };
    (0..count).map(move |index| {
        let start = index * size + index.min(extra);
        let end = start + size + usize::from(index < extra);
        Fragment { flags, frame, base, index: index as u16, count: count as u16, capture_us, payload: &au[start..end] }
    })
}

struct CachedDatagram {
    seq: u64,
    at: u64,
    bytes: Vec<u8>,
    /// Two fields per cached datagram track its latest transmission and receipt of any copy.
    newest_hop: Option<u64>,
    seen: bool,
}

/// The datagrams one stream sent to one receiver in the last [`CACHE_US`], by sequence: a
/// resend is the same bytes (only the hop changes).
#[derive(Default)]
pub struct SendCache {
    sent: VecDeque<CachedDatagram>,
    top_seen: Option<u64>,
    /// Sealed sequences restart at zero under a new secret. This orders their number spaces.
    pub(super) epoch: usize,
}

impl SendCache {
    pub(super) fn new(epoch: usize) -> Self {
        Self { epoch, ..Self::default() }
    }

    pub fn push(&mut self, seq: u64, now: u64, datagram: Vec<u8>) {
        while self.sent.front().is_some_and(|entry| now.saturating_sub(entry.at) > CACHE_US) {
            self.sent.pop_front();
        }
        self.sent.push_back(CachedDatagram { seq, at: now, bytes: datagram, newest_hop: None, seen: false });
    }

    fn entry_mut(&mut self, seq: u64) -> Option<&mut CachedDatagram> {
        let index = self.sent.binary_search_by_key(&seq, |entry| entry.seq).ok()?;
        self.sent.get_mut(index)
    }

    pub(super) fn stamped(&mut self, seq: u64, hop: u64) {
        if let Some(entry) = self.entry_mut(seq) {
            entry.newest_hop = Some(hop);
        }
    }

    pub(super) fn mark_seen(&mut self, seq: u64) {
        self.top_seen = Some(self.top_seen.map_or(seq, |top| top.max(seq)));
        if let Some(entry) = self.entry_mut(seq) {
            entry.seen = true;
        }
    }

    /// Only the newest transmission of a still-unseen packet can buy another send.
    pub(super) fn repair(&self, seq: u64, hop: u64, now: u64, key_packet: bool) -> Option<&[u8]> {
        if !key_packet && self.top_seen.is_some_and(|top| seq <= top) {
            return None;
        }
        let index = self.sent.binary_search_by_key(&seq, |entry| entry.seq).ok()?;
        let entry = &self.sent[index];
        (entry.newest_hop == Some(hop) && !entry.seen && now.saturating_sub(entry.at) <= CACHE_US)
            .then_some(entry.bytes.as_slice())
    }

    /// The existing key can answer a key request while all its packets remain available
    /// and feedback has yet to account for at least one of them.
    pub(super) fn key_awaiting_receipt(&self, key: &std::ops::RangeInclusive<u64>, now: u64) -> bool {
        let Ok(i) = self.sent.binary_search_by_key(key.start(), |entry| entry.seq) else { return false };
        let last = i + (*key.end() - *key.start()) as usize;
        now.saturating_sub(self.sent[i].at) <= CACHE_US && self.sent.get(last).is_some_and(|e| e.seq == *key.end())
            && self.sent.range(i..=last).any(|entry| !entry.seen)
    }

    #[cfg(test)]
    pub(super) fn unseen_sequences(&self) -> impl Iterator<Item = u64> + '_ {
        self.sent.iter().filter(|entry| !entry.seen).map(|entry| entry.seq)
    }

    /// The cached datagrams of `first .. first + count`, oldest first; sequences no longer
    /// cached are left out (the receiver will ask for a key frame).
    pub fn find(&self, first: u64, count: u8) -> impl Iterator<Item = &[u8]> {
        let end = first.saturating_add(count as u64);
        let start = self.sent.partition_point(|entry| entry.seq < first);
        self.sent.range(start..).take_while(move |entry| entry.seq < end).map(|entry| entry.bytes.as_slice())
    }

    pub fn len(&self) -> usize {
        self.sent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
    }
}

/// Whether a stream's encoder should make a key frame now: at most one per
/// [`KEY_INTERVAL_US`], and every request inside that time is answered by the same one.
#[derive(Default)]
pub struct KeyGate {
    last_key: Option<u64>,
    pending: bool,
}

impl KeyGate {
    pub fn request(&mut self) {
        self.pending = true;
    }

    /// A request still waiting is answered another way (resends of the newest key frame).
    pub fn withdraw(&mut self) {
        self.pending = false;
    }

    /// Called before each encode: true when this picture should be a key frame.
    pub fn take(&mut self, now: u64) -> bool {
        self.take_after(now, KEY_INTERVAL_US)
    }

    pub(crate) fn take_after(&mut self, now: u64, interval: u64) -> bool {
        let due = self.pending && self.last_key.is_none_or(|at| now.saturating_sub(at) >= interval);
        if due {
            self.pending = false;
        }
        due
    }

    /// The encoder made a key frame (asked for or not).
    pub fn made_key(&mut self, now: u64) {
        self.last_key = Some(now);
    }
}

/// A whole access unit, ready for the decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub frame: u32,
    pub key: bool,
    pub capture_us: u64,
    pub data: Vec<u8>,
}

struct Partial {
    key: bool,
    key_id: u8,
    first_seq: u64,
    base: u16,
    capture_us: u64,
    expiry_from: u64,
    count: u16,
    buffers: FragmentBuffers,
}

impl Partial {
    fn last_seq(&self) -> u64 {
        self.first_seq + u64::from(self.count) - 1
    }

    fn complete(&self) -> bool {
        self.buffers.arrivals.len() == usize::from(self.count)
    }
}

struct Arrival {
    index: u16,
    len: u16,
    // A frame holds at most u16::MAX fragments of MAX_FRAGMENT bytes.
    offset: u32,
}

/// Only the duplicate bitmap is sized from the advertised count. Payload and offsets
/// grow with the fragments that actually arrive, even when the last one arrives first.
#[derive(Default)]
struct FragmentBuffers {
    data: Vec<u8>,
    arrivals: Vec<Arrival>,
    seen: Vec<u64>,
}

impl FragmentBuffers {
    fn reset(&mut self, count: u16) {
        self.data.clear();
        self.arrivals.clear();
        self.seen.clear();
        let words = usize::from(count).div_ceil(64);
        self.seen.reserve_exact(words); // keep even a reused bitmap within 1,024 words
        self.seen.resize(words, 0);
    }

    fn insert(&mut self, frag: &Fragment) -> bool {
        let index = usize::from(frag.index);
        let word = &mut self.seen[index / 64];
        let mask = 1u64 << (index % 64);
        if *word & mask != 0 {
            return false;
        }
        *word |= mask;
        self.arrivals.push(Arrival {
            index: frag.index,
            offset: self.data.len() as u32,
            len: frag.payload.len() as u16,
        });
        self.data.extend_from_slice(frag.payload);
        true
    }

    fn assemble(&mut self, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(self.data.len());
        self.arrivals.sort_unstable_by_key(|a| a.index);
        for arrival in &self.arrivals {
            let start = arrival.offset as usize;
            out.extend_from_slice(&self.data[start..start + usize::from(arrival.len)]);
        }
    }
}

struct Missing {
    noticed: u64,
    first_due: u64,
    asked: Option<u64>,
    rounds: u8,
    third_ask: Option<u64>,
}

impl Missing {
    fn new(now: u64, first_due: u64) -> Self {
        Self { noticed: now, first_due, asked: None, rounds: 0, third_ask: None }
    }
}

/// A key recovered inside a cleared gap makes the pictures after it worth resending.
struct ClearedGap {
    from: u64,
    through: u64,
    at: u64,
    arrived: BTreeSet<u64>,
}

struct Sequences {
    next: u64,
    missing: BTreeMap<u64, Missing>,
    cleared: Option<ClearedGap>,
}

impl Sequences {
    fn expire_cleared(&mut self, now: u64) {
        if self.cleared.as_ref().is_some_and(|g| now.saturating_sub(g.at) > CACHE_US) {
            self.cleared = None;
        }
    }

    fn delivered_key(&mut self, key: &Partial, now: u64) {
        let last = key.last_seq();
        while self.missing.first_key_value().is_some_and(|(&seq, _)| seq <= last) {
            self.missing.pop_first();
        }
        if let Some(gap) = self.cleared.take_if(|g| key.first_seq >= g.from) {
            // A key beyond the gap supersedes it. Inside it, only the unseen suffix is needed.
            for seq in last + 1..=gap.through {
                if !gap.arrived.contains(&seq) {
                    self.missing.entry(seq).or_insert_with(|| Missing::new(now, now)).first_due = now;
                }
            }
        }
    }
}

/// What a receiver wants the sender to do, gathered at each [`Receiver::poll`].
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Wants {
    pub nacks: Vec<NackRange>,
    pub key: bool,
}

/// One sender's video stream at a receiver.
pub struct Receiver {
    src: u16,
    stream: u8,
    /// Each encryption key has its own sequence space.
    seqs: BTreeMap<u8, Sequences>,
    /// Frames not yet handed on, whole or in part.
    frames: BTreeMap<u32, Partial>,
    spare_data: Vec<Vec<u8>>,
    spare_fragments: Vec<FragmentBuffers>,
    /// The newest frame handed on; the next must predict from it, or be a key frame.
    delivered: Option<u32>,
    /// Only a key frame lets decoding go on (at the start, or after the chain broke).
    need_key: bool,
    /// Packet cost of replacing the chain, learned from the newest complete key frame.
    key_fragments: Option<usize>,
    /// Until this key's capture interval ends, the sender's gate cannot offer a cheaper new key.
    key_capture_us: u64,
    // First evidence of an undelivered share head. Repeating it must not restart the wait.
    advertised: Option<(u32, u64)>,
    pub stats: ReceiverStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiverStats {
    pub fragments: u64,
    /// Repeats, and fragments of frames already handed on or skipped.
    pub late: u64,
    pub frames: u64,
    pub key_frames: u64,
    pub nacked: u64,
    /// Polls that asked for a key frame.
    pub key_requests: u64,
    /// Frames never handed on: passed over by a newer key frame, or too old to decode.
    pub skipped: u64,
}

impl Receiver {
    pub fn new(src: u16, stream: u8) -> Self {
        Receiver {
            src,
            stream,
            seqs: BTreeMap::new(),
            frames: BTreeMap::new(),
            spare_data: Vec::new(),
            spare_fragments: Vec::new(),
            delivered: None,
            need_key: true,
            key_fragments: None,
            key_capture_us: 0,
            advertised: None,
            stats: ReceiverStats::default(),
        }
    }

    pub(super) fn head(&mut self, now: u64, key_id: u8, frame: u32, next: u64) {
        if self.delivered.is_some_and(|d| !newer(frame, d)) { return; }
        if self.advertised.is_some_and(|(head, _)| newer(head, frame)) { return; }
        // On resubscribing mid-stream there is no trustworthy lower bound for an entirely
        // unseen epoch's gap. The held key is cheaper than requesting historical sequences.
        if self.delivered.is_none() && frame != 0 && !self.seqs.contains_key(&key_id) {
            self.need_key = true;
            return;
        }
        let head = self.advertised.get_or_insert((frame, now));
        if newer(frame, head.0) { head.0 = frame; }
        let seqs = self.seqs.entry(key_id).or_insert_with(|| Sequences {
            next: 0, missing: BTreeMap::new(), cleared: None,
        });
        if next > seqs.next {
            for seq in seqs.next.max(next.saturating_sub(VIDEO_WINDOW as u64))..next {
                seqs.missing.insert(seq, Missing::new(now, now));
            }
            seqs.next = next;
        }
    }

    /// Return a whole frame's storage for a later access unit on this stream.
    pub fn recycle(&mut self, data: Vec<u8>) {
        if data.capacity() != 0 {
            self.spare_data.push(data);
        }
    }

    /// A fragment that opened under `key_id` at sequence `seq`. Whole frames that can be decoded
    /// now go to `out`, in order.
    pub fn take(&mut self, key_id: u8, seq: u64, frag: &Fragment, now: u64, out: &mut Vec<Frame>) {
        self.stats.fragments += 1;
        let first_seq = seq.saturating_sub(u64::from(frag.index));
        // Even the first fragment heard can reveal a missing prefix of its picture.
        let seqs = self.seqs.entry(key_id).or_insert_with(|| Sequences {
            next: first_seq, missing: BTreeMap::new(), cleared: None,
        });
        seqs.expire_cleared(now);
        if seq >= seqs.next {
            let gap = seq - seqs.next;
            if self.stream != crate::wire::stream::SHARE && frag.capture_us.saturating_sub(self.key_capture_us) >= KEY_INTERVAL_US
                && self.key_fragments.is_some_and(|count| gap > count as u64)
            {
                self.need_key = true;
                let from = seqs.next;
                let cleared = seqs.cleared.get_or_insert_with(|| ClearedGap {
                    from, through: seq - 1, at: now, arrived: BTreeSet::new(),
                });
                cleared.through = seq - 1;
            } else {
                // A gap wider than the opener's replay window can't be resent into it.
                let from = seqs.next.max(seq.saturating_sub(VIDEO_WINDOW as u64));
                for s in from..seq {
                    seqs.missing.insert(s, Missing::new(now, now.saturating_add(REORDER_US)));
                }
            }
            seqs.next = seq.saturating_add(1);
        } else {
            seqs.missing.remove(&seq);
        }
        // A later clear may extend across packets received after the first gap.
        if let Some(gap) = &mut seqs.cleared {
            if seq >= gap.from {
                gap.arrived.insert(seq);
            }
        }

        if self.delivered.is_some_and(|d| !newer(frag.frame, d)) {
            self.stats.late += 1;
            return;
        }
        if frag.index >= frag.count || frag.payload.len() > MAX_FRAGMENT {
            return;
        }
        let partial = self.frames.entry(frag.frame).or_insert_with(|| {
            let mut buffers = self.spare_fragments.pop().unwrap_or_default();
            buffers.reset(frag.count);
            Partial {
                key: frag.is_key(), key_id, first_seq, base: frag.base,
                capture_us: frag.capture_us, expiry_from: now, count: frag.count, buffers,
            }
        });
        if partial.count != frag.count {
            return; // disagrees with its frame's count: not this frame's
        }
        if !partial.buffers.insert(frag) {
            self.stats.late += 1;
            return;
        }
        // A paced share key can take longer than the resend cache to cross a healthy link.
        // Only a new fragment extends its life. Replays cannot keep a dead frame alive.
        if self.stream == crate::wire::stream::SHARE { partial.expiry_from = now; }
        if partial.complete() {
            self.deliver(now, out);
        }
    }

    /// Hands on frames in order while each can be decoded; when the chain waits on a frame
    /// that a newer whole key frame makes unnecessary, skips to that key frame.
    fn deliver(&mut self, now: u64, out: &mut Vec<Frame>) {
        self.drain(now, out);
        let newest_key = self.frames.iter().rev().find(|(_, p)| p.key && p.complete()).map(|(f, _)| *f);
        if let Some(key) = newest_key {
            while self.frames.first_key_value().is_some_and(|(&f, _)| f < key) {
                let (_, p) = self.frames.pop_first().unwrap();
                self.spare_fragments.push(p.buffers);
                self.stats.skipped += 1;
            }
            self.drain(now, out);
        }
    }

    fn drain(&mut self, now: u64, out: &mut Vec<Frame>) {
        while let Some((&frame, p)) = self.frames.iter().next() {
            let follows = p.key || self.delivered.is_some_and(|d| frame.wrapping_sub(p.base as u32) == d);
            if !(p.complete() && follows) {
                break;
            }
            let mut p = self.frames.remove(&frame).unwrap();
            if p.key {
                self.key_fragments = Some(usize::from(p.count));
                self.key_capture_us = p.capture_us;
                self.seqs.get_mut(&p.key_id).unwrap().delivered_key(&p, now);
            }
            let mut data = self.spare_data.pop().unwrap_or_default();
            p.buffers.assemble(&mut data);
            self.spare_fragments.push(p.buffers);
            self.stats.frames += 1;
            if p.key {
                self.stats.key_frames += 1;
                self.need_key = false;
            }
            self.delivered = Some(frame);
            if self.advertised.is_some_and(|(head, _)| !newer(head, frame)) { self.advertised = None; }
            out.push(Frame { frame, key: p.key, capture_us: p.capture_us, data });
        }
        if self.frames.is_empty() && self.delivered.is_some() {
            self.need_key = false; // a resend let the chain catch up after all
        }
    }

    /// A key frame is arriving and resends can still complete it: asking for another would
    /// only start a second one behind it.
    pub(crate) fn key_arriving(&self, now: u64, rtt_us: u64) -> bool {
        self.frames.values().any(|p| p.key && !self.should_request_key(p, now, rtt_us))
    }

    /// Only a key frame lets this stream's decoding go on.
    pub fn wants_key(&self) -> bool {
        self.need_key
    }

    /// A quiet tail may still be paced. Only a known gap or expiry of the resend cache gives it up.
    fn should_request_key(&self, p: &Partial, now: u64, rtt_us: u64) -> bool {
        // A delta also needs its predecessors. A key can start at its own first sequence.
        let first = if p.key { p.first_seq } else { 0 };
        let seqs = &self.seqs[&p.key_id];
        let oldest = seqs.missing.range(first..=p.last_seq()).next();
        if self.stream == crate::wire::stream::SHARE {
            return now.saturating_sub(p.expiry_from) > CACHE_US ||
                oldest.is_some_and(|(_, m)| now.saturating_sub(m.noticed) >= share_wait(rtt_us));
        }
        now.saturating_sub(p.expiry_from) > CACHE_US ||
            // An incomplete key at or before a cleared gap cannot replace the lost chain yet.
            (p.key && seqs.cleared.as_ref().is_some_and(|g| p.first_seq <= g.through)) ||
            oldest.is_some_and(|(_, m)| now.saturating_sub(m.noticed) > CACHE_US ||
                m.third_ask.is_some_and(|at| now.saturating_sub(at) >= rtt_us))
    }

    /// The NACKs due now and whether to ask for a key frame. `rtt_us` is the path's round trip.
    pub fn poll(&mut self, now: u64, rtt_us: u64, wants: &mut Wants) {
        for seqs in self.seqs.values_mut() {
            seqs.expire_cleared(now);
        }
        let again = resend_interval(rtt_us);
        if self.frames.first_key_value().is_some_and(|(_, p)| self.should_request_key(p, now, rtt_us)) {
            self.need_key = true;
        }
        for (&key_id, seqs) in self.seqs.iter_mut() {
            // past the sender's cache a resend can't come
            seqs.missing.retain(|_, m| now.saturating_sub(m.noticed) <= CACHE_US);
            let mut run: Option<NackRange> = None;
            for (&seq, m) in seqs.missing.iter_mut() {
                let due = match m.asked {
                    None => now >= m.first_due,
                    Some(at) => now.saturating_sub(at) >= again,
                };
                if !due {
                    continue;
                }
                m.asked = Some(now);
                m.rounds = m.rounds.saturating_add(1);
                if m.rounds == RESEND_CHANCES {
                    m.third_ask = Some(now);
                }
                self.stats.nacked += 1;
                match &mut run {
                    Some(r) if r.first + r.count as u64 == seq && r.count < u8::MAX => r.count += 1,
                    _ => {
                        wants.nacks.extend(run.take());
                        run = Some(NackRange { src: self.src, stream: self.stream, key_id, first: seq, count: 1 });
                    }
                }
            }
            wants.nacks.extend(run);
        }
        if let Some((_, since)) = self.advertised {
            if now.saturating_sub(since) < share_wait(rtt_us) && self.delivered.is_none() { return; }
            self.need_key |= now.saturating_sub(since) >= share_wait(rtt_us);
        }
        if self.need_key {
            // frames older than the sender's cache can't be completed; the key frame passes over
            // the rest
            let before = self.frames.len();
            self.frames.retain(|_, p| {
                if now.saturating_sub(p.expiry_from) <= CACHE_US {
                    return true;
                }
                self.spare_fragments.push(std::mem::take(&mut p.buffers));
                false
            });
            self.stats.skipped += (before - self.frames.len()) as u64;
            // A key frame still arriving (the stream's first, say) is not asked for again, unless
            // it is stuck past what resends can fix.
            let key_coming = self.frames.values().any(|p| p.key && !self.should_request_key(p, now, rtt_us));
            if !key_coming {
                self.stats.key_requests += 1;
                wants.key = true;
            }
        }
    }
}

fn resend_interval(rtt_us: u64) -> u64 {
    rtt_us.saturating_add(rtt_us / 2)
}

/// `a` is after `b` in frame order (frame numbers wrap).
fn share_wait(rtt_us: u64) -> u64 {
    (2 * rtt_us + 100_000).max(250_000)
}

fn newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sender's stream: access units of the given sizes, every `key_every`-th a key frame.
    fn stream(sizes: &[usize]) -> Vec<(u64, Vec<u8>)> {
        let mut seq = 0u64;
        let mut out = Vec::new();
        for (i, &size) in sizes.iter().enumerate() {
            let au: Vec<u8> = (0..size).map(|b| (b * 7 + i) as u8).collect();
            let key = i == 0;
            for f in fragments(&au, i as u32, if key { 0 } else { 1 }, key, i as u64 * 33_000) {
                let mut bytes = Vec::new();
                f.write(&mut bytes);
                out.push((seq, bytes));
                seq += 1;
            }
        }
        out
    }

    fn feed(r: &mut Receiver, packets: &[(u64, Vec<u8>)], now: u64, out: &mut Vec<Frame>) {
        for (seq, bytes) in packets {
            r.take(0, *seq, &Fragment::read(bytes).unwrap(), now, out);
        }
    }

    #[test]
    fn fragments_are_even_and_put_back_whole() {
        let au = vec![5u8; 3 * MAX_FRAGMENT + 1];
        let parts: Vec<_> = fragments(&au, 1, 0, true, 0).collect();
        assert_eq!(parts.len(), 4, "four fragments");
        let (min, max) = parts.iter().fold((usize::MAX, 0), |(a, b), f| (a.min(f.payload.len()), b.max(f.payload.len())));
        assert!(max - min <= 1 && max <= MAX_FRAGMENT, "near-equal sizes, none too big: {min}..{max}");
        assert_eq!(fragments(&[1, 2, 3], 0, 0, true, 0).count(), 1, "a small unit is one fragment");

        let packets = stream(&[30_000, 4_000, 4_000, 900]);
        let mut r = Receiver::new(1, 2);
        let mut out = Vec::new();
        let mut shuffled = packets.clone();
        shuffled.reverse(); // the worst reordering
        feed(&mut r, &shuffled, 0, &mut out);
        assert_eq!(out.iter().map(|f| f.frame).collect::<Vec<_>>(), vec![0, 1, 2, 3], "every frame, in order, despite reordering");
        for (f, size) in out.iter().zip([30_000, 4_000, 4_000, 900]) {
            assert_eq!(f.data, (0..size).map(|b| (b * 7 + f.frame as usize) as u8).collect::<Vec<u8>>(), "frame {} intact", f.frame);
        }
        feed(&mut r, &packets[..3], 0, &mut out);
        assert_eq!(out.len(), 4, "repeats of handed-on frames change nothing");
        assert_eq!(r.stats.late, 3, "and are counted as late");

        let data = out.remove(0).data;
        let ptr = data.as_ptr();
        r.recycle(data);
        for part in fragments(&au, 4, 1, false, 132_000) {
            r.take(0, packets.len() as u64 + u64::from(part.index), &part, 132_000, &mut out);
        }
        assert_eq!(out.last().unwrap().data.as_ptr(), ptr, "a later frame must reuse the returned access-unit buffer");

        for count in [3, u16::MAX] {
            let mut fresh = Receiver::new(1, 2);
            let tail = Fragment {
                flags: 0, frame: 0, base: 1, index: count - 1, count, capture_us: 0,
                payload: &[0; MAX_FRAGMENT],
            };
            fresh.take(0, u64::from(tail.index), &tail, 0, &mut out);
            let p = &fresh.frames[&tail.frame];
            let bytes = p.buffers.data.capacity();
            let words = p.buffers.seen.capacity();
            assert!(bytes < 2 * MAX_FRAGMENT && words <= 1024,
                "one fragment of {count} reserved {bytes} payload bytes and {words} words for duplicate tracking");
        }
    }

    #[test]
    fn a_lost_fragment_is_asked_for_and_a_resend_completes_the_chain() {
        let packets = stream(&[10_000, 3_000, 3_000]);
        let lost = 10; // in frame 1
        let mut r = Receiver::new(1, 2);
        let mut out = Vec::new();
        let (before, after) = packets.split_at(lost);
        feed(&mut r, &before[..1], 0, &mut out);
        let mut first = Wants::default();
        r.poll(500, 40_000, &mut first);
        assert!(!first.key, "a fresh receiver whose first key frame is on its way asks for no other");
        feed(&mut r, &before[1..], 600, &mut out);
        feed(&mut r, &after[1..], 1_000, &mut out);
        assert_eq!(out.len(), 1, "frame 0 only: frame 1 lacks a fragment and frame 2 predicts from it");

        let mut wants = Wants::default();
        r.poll(5_000, 40_000, &mut wants);
        assert!(wants.nacks.is_empty(), "no NACK inside the reorder tolerance");
        r.poll(12_000, 40_000, &mut wants);
        assert_eq!(wants.nacks, vec![NackRange { src: 1, stream: 2, key_id: 0, first: lost as u64, count: 1 }], "the gap is asked for");
        assert!(!wants.key, "no key frame while a resend can still help");

        let mut again = Wants::default();
        r.poll(40_000, 40_000, &mut again);
        assert!(again.nacks.is_empty(), "not asked again before 1.5 RTT");
        r.poll(73_000, 40_000, &mut again);
        assert_eq!(again.nacks.len(), 1, "asked again after 1.5 RTT");

        feed(&mut r, &packets[lost..=lost], 80_000, &mut out);
        assert_eq!(out.iter().map(|f| f.frame).collect::<Vec<_>>(), vec![0, 1, 2], "the resend completes frame 1 and frame 2 follows");
        let mut done = Wants::default();
        r.poll(200_000, 40_000, &mut done);
        assert_eq!(done, Wants::default(), "nothing more to ask for");

        // A whole one-packet picture at 15 fps first becomes visibly missing when picture 2 arrives.
        let pictures: Vec<_> = (0..3).map(|i| fragments(b"picture", i, u16::from(i != 0), i == 0,
            u64::from(i) * 1_000_000 / 15).next().unwrap()).collect();
        let gap = pictures[2].capture_us + 20_000;
        let rtt = 40_000;
        for repaired in [true, false] {
            let mut r = Receiver::new(1, 2);
            let mut out = Vec::new();
            r.take(0, 0, &pictures[0], 20_000, &mut out);
            r.take(0, 2, &pictures[2], gap, &mut out);
            for delay in [10_000, 70_000, 130_000] {
                let mut early = Wants::default();
                r.poll(gap + delay - 1, rtt, &mut early);
                assert_eq!(early, Wants::default(), "15 fps gap asked early at {} µs: {early:?}", delay - 1);
                let mut due = Wants::default();
                r.poll(gap + delay, rtt, &mut due);
                assert_eq!(due, Wants { nacks: vec![NackRange { src: 1, stream: 2, key_id: 0, first: 1, count: 1 }], key: false },
                    "15 fps gap must get precisely one NACK at +{delay} µs");
            }
            let mut waiting = Wants::default();
            r.poll(gap + 169_999, rtt, &mut waiting);
            assert_eq!(waiting, Wants::default(), "third resend still has one RTT to arrive: {waiting:?}");
            if repaired {
                r.take(0, 1, &pictures[1], gap + 169_999, &mut out);
                assert_eq!(out.iter().map(|f| f.frame).collect::<Vec<_>>(), [0, 1, 2], "third chance must recover picture 1 and release picture 2");
            }
            let mut verdict = Wants::default();
            r.poll(gap + 170_000, rtt, &mut verdict);
            assert_eq!(verdict, Wants { key: !repaired, nacks: Vec::new() },
                "one RTT after the third ask, key must depend on recovery (repaired={repaired})");
            if !repaired {
                let mut fourth = Wants::default();
                r.poll(gap + 190_000, rtt, &mut fourth);
                assert_eq!(fourth, Wants { key: true, nacks: vec![NackRange { src: 1, stream: 2, key_id: 0, first: 1, count: 1 }] },
                    "the fourth NACK at +190 ms must race the outstanding key request");
                r.take(0, 1, &pictures[1], gap + 230_000, &mut out);
                assert_eq!(out.iter().map(|f| f.frame).collect::<Vec<_>>(), [0, 1, 2], "a late resend must still complete the chain");
                let mut caught_up = Wants::default();
                r.poll(gap + 250_000, rtt, &mut caught_up);
                assert_eq!(caught_up, Wants::default(), "repair before the replacement key should end both requests");
            }
        }
    }

    #[test]
    fn a_chain_stuck_too_long_asks_for_a_key_frame_and_restarts_at_it() {
        let mut packets = stream(&[10_000, 3_000, 3_000]);
        let lost = packets.iter().position(|(_, b)| Fragment::read(b).unwrap().frame == 1).unwrap();
        packets.remove(lost);
        let mut r = Receiver::new(1, 2);
        let mut out = Vec::new();
        feed(&mut r, &packets, 0, &mut out);
        for at in [10_000, 40_000, 70_000] {
            let mut wants = Wants::default();
            r.poll(at, 20_000, &mut wants);
            assert_eq!(wants.nacks.len(), 1, "the missing fragment must still get each NACK at {at} µs: {wants:?}");
            assert!(!wants.key, "the last NACK still has time to recover the fragment at {at} µs");
        }
        let mut wants = Wants::default();
        r.poll(90_000, 20_000, &mut wants);
        assert!(wants.key, "one 20 ms RTT after the third ask at 70 ms, the stuck chain should request a key");

        // the key frame passes over the frames that waited, and the chain goes on from it
        let next = stream(&[8_000, 2_000]);
        let shifted: Vec<(u64, Vec<u8>)> = next
            .iter()
            .map(|(s, b)| {
                let f = Fragment::read(b).unwrap();
                let mut bytes = Vec::new();
                Fragment { frame: f.frame + 10, ..f }.write(&mut bytes);
                (s + 100, bytes)
            })
            .collect();
        feed(&mut r, &shifted, 190_000, &mut out);
        assert_eq!(out.iter().map(|f| f.frame).collect::<Vec<_>>(), vec![0, 10, 11], "decoding restarts at the key frame");
        assert_eq!(r.stats.skipped, 2, "frames 1 and 2 were passed over");
        let mut after = Wants::default();
        r.poll(200_000, 20_000, &mut after);
        assert!(!after.key, "no more key requests once one arrived");

        // No later sequence reveals the tail. Its fragments may be paced until the cache expires.
        let key = fragments(b"key", 0, 0, true, 0).next().unwrap();
        let data = vec![0; 2 * MAX_FRAGMENT + 1];
        let parts: Vec<_> = fragments(&data, 1, 1, false, 20_000).collect();
        for second_at in [None, Some(180_000)] {
            let mut r = Receiver::new(1, 2);
            let mut out = Vec::new();
            r.take(0, 0, &key, 0, &mut out);
            r.take(0, 1, &parts[0], 40_000, &mut out);
            if let Some(at) = second_at {
                r.take(0, 2, &parts[1], at, &mut out);
            }
            for (age, key_due) in [(230_000, false), (CACHE_US, false), (CACHE_US + 1, true)] {
                let mut wants = Wants::default();
                r.poll(40_000 + age, 40_000, &mut wants);
                assert_eq!(wants, Wants { key: key_due, nacks: vec![] },
                    "unnamed tail at age {age} µs with second fragment at {second_at:?}");
            }
        }
    }

    #[test]
    fn a_chain_stuck_on_a_resend_while_pictures_keep_coming_still_asks_for_its_gaps() {
        // pictures every 33 ms; a fragment of frame 1 is lost and so is its first resend, and a
        // fragment of frame 6 is lost too
        let packets = stream(&[10_000, 3_000, 3_000, 3_000, 3_000, 3_000, 3_000, 3_000, 3_000, 3_000]);
        let in_frame = |frame: u32| packets.iter().position(|(_, b)| Fragment::read(b).unwrap().frame == frame).unwrap();
        let (lost1, lost6) = (in_frame(1), in_frame(6));
        let mut r = Receiver::new(1, 2);
        let mut out = Vec::new();
        let mut asked: Vec<(u64, u64)> = Vec::new();
        for frame in 0..10u32 {
            let now = frame as u64 * 33_000 + 1_000;
            let arriving: Vec<(u64, Vec<u8>)> = packets
                .iter()
                .enumerate()
                .filter(|(i, (_, b))| Fragment::read(b).unwrap().frame == frame && *i != lost1 && *i != lost6)
                .map(|(_, p)| p.clone())
                .collect();
            feed(&mut r, &arriving, now, &mut out);
            let mut wants = Wants::default();
            r.poll(now, 40_000, &mut wants);
            asked.extend(wants.nacks.iter().flat_map(|n| (n.first..n.first + n.count as u64).map(move |s| (now, s))));
        }
        assert_eq!(out.len(), 1, "frame 0 only while frame 1 waits");
        let rounds: Vec<_> = asked.iter().filter(|(_, s)| *s == lost1 as u64).map(|&(at, _)| at).collect();
        assert_eq!(rounds, [67_000, 133_000, 199_000, 265_000], "frame 1 should keep getting NACKs while later pictures arrive: {asked:?}");
        assert!(asked.iter().any(|&(_, s)| s == lost6 as u64), "frame 6's gap is asked for too: {asked:?}");
        feed(&mut r, &packets[lost1..=lost1], 340_000, &mut out);
        feed(&mut r, &packets[lost6..=lost6], 341_000, &mut out);
        assert_eq!(out.iter().map(|f| f.frame).collect::<Vec<_>>(), (0..10).collect::<Vec<_>>(), "the resends complete the chain, in order");
        assert_eq!(r.stats.skipped, 0, "nothing was passed over");
        let mut done = Wants::default();
        r.poll(350_000, 40_000, &mut done);
        assert!(!done.key, "the chain caught up: no key frame wanted");
    }

    #[test]
    fn a_gap_is_cleared_only_if_a_new_key_is_cheaper_and_available() {
        // Four pictures a second: 250 ms between captures, with no sequence gap.
        let mut packets = Vec::new();
        let mut seq = 0u64;
        for frame in 0..12u32 {
            let au: Vec<u8> = (0..2_000).map(|b| (b * 7 + frame as usize) as u8).collect();
            for f in fragments(&au, frame, if frame == 0 { 0 } else { 1 }, frame == 0, frame as u64 * 250_000) {
                let mut bytes = Vec::new();
                f.write(&mut bytes);
                packets.push((seq, frame, bytes));
                seq += 1;
            }
        }
        let mut r = Receiver::new(1, 2);
        let mut out = Vec::new();
        let feed_frame = |r: &mut Receiver, frame: u32, out: &mut Vec<Frame>| {
            for (seq, _, bytes) in packets.iter().filter(|(_, f, _)| *f == frame) {
                r.take(0, *seq, &Fragment::read(bytes).unwrap(), frame as u64 * 250_000 + 1_000, out);
            }
        };
        for frame in 0..6 {
            feed_frame(&mut r, frame, &mut out);
            let mut wants = Wants::default();
            r.poll(frame as u64 * 250_000 + 2_000, 20_000, &mut wants);
            assert!(!wants.key, "picture {frame}, 250 ms after the last, is no outage");
        }
        assert_eq!(out.len(), 6, "every picture handed on");
        // pictures 6 to 8 are lost whole (750 ms): the next one is an outage
        feed_frame(&mut r, 9, &mut out);
        let mut wants = Wants::default();
        r.poll(9 * 250_000 + 2_000, 20_000, &mut wants);
        assert!(wants.key && wants.nacks.is_empty(), "three whole pictures lost over 750 ms: a key frame, no NACKs: {wants:?}");

        // The replacement cost changes from one packet to three at the second key.
        let key_data = vec![0; 2 * MAX_FRAGMENT + 1];
        for (gap, span, outage) in [(2u8, 3_000_000, false), (3, 3_000_000, false),
            (4, KEY_INTERVAL_US - 1, false), (4, KEY_INTERVAL_US, true)] {
            let mut r = Receiver::new(1, 2);
            let mut out = Vec::new();
            let first = fragments(b"first", 0, 0, true, 0).next().unwrap();
            r.take(0, 0, &first, 0, &mut out);
            for (i, f) in fragments(&key_data, 1, 0, true, 1_000).enumerate() {
                r.take(0, 1 + i as u64, &f, 1_000, &mut out);
            }
            let next = fragments(b"delta", 2 + u32::from(gap), 1, false, 1_000 + span).next().unwrap();
            r.take(0, 4 + u64::from(gap), &next, 1_000 + span, &mut out);
            let mut wants = Wants::default();
            r.poll(1_000 + span + REORDER_US, 40_000, &mut wants);
            let nacks = if outage { vec![] } else { vec![NackRange { src: 1, stream: 2, key_id: 0, first: 4, count: gap }] };
            assert_eq!(wants, Wants { key: outage, nacks }, "{gap} missing packets versus a three-packet key, span {span} µs");
        }

        let mut fresh = Receiver::new(1, 2);
        let mut out = Vec::new();
        let middle = fragments(&key_data, 0, 0, true, 0).nth(1).unwrap();
        fresh.take(0, 1, &middle, 0, &mut out);
        let later = fragments(b"delta", 5, 1, false, 200_000).next().unwrap();
        fresh.take(0, 8, &later, 200_000, &mut out);
        let mut wants = Wants::default();
        fresh.poll(200_000 + REORDER_US, 40_000, &mut wants);
        assert_eq!(wants, Wants { key: false, nacks: vec![
            NackRange { src: 1, stream: 2, key_id: 0, first: 0, count: 1 },
            NackRange { src: 1, stream: 2, key_id: 0, first: 2, count: 6 },
        ] }, "before a complete key, ask for its missing first fragment and the later gap");

        // Two clears straddle packets 5 and 8, which have arrived by the time the key is whole.
        let mut r = Receiver::new(1, 2);
        let mut out = Vec::new();
        let first = fragments(b"first", 0, 0, true, 0).next().unwrap();
        r.take(0, 0, &first, 0, &mut out);
        for seq in [8, 5, 12] {
            let f = fragments(b"delta", seq as u32, 1, false, KEY_INTERVAL_US + seq).next().unwrap();
            r.take(0, seq, &f, KEY_INTERVAL_US + 20, &mut out);
        }
        let parts: Vec<_> = fragments(&key_data[..MAX_FRAGMENT + 1], 4, 0, true, KEY_INTERVAL_US).collect();
        r.take(0, 3, &parts[0], KEY_INTERVAL_US + 30, &mut out);
        let mut partial = Wants::default();
        r.poll(KEY_INTERVAL_US + 30, 40_000, &mut partial);
        assert_eq!(partial, Wants { key: true, nacks: vec![] }, "a partial key inside the cleared gap cannot answer the key request");
        r.take(0, 4, &parts[1], KEY_INTERVAL_US + 40, &mut out);
        let mut repaired = Wants::default();
        r.poll(KEY_INTERVAL_US + 40, 40_000, &mut repaired);
        assert_eq!(repaired, Wants { key: false, nacks: vec![
            NackRange { src: 1, stream: 2, key_id: 0, first: 6, count: 2 },
            NackRange { src: 1, stream: 2, key_id: 0, first: 9, count: 3 },
        ] }, "the recovered key must expose only the unseen suffix of both clears, due immediately");

        // A key after the gap supersedes it; a key arriving past the cache cannot repair its suffix.
        for (key_seq, delay) in [(9, 0), (3, CACHE_US + 1)] {
            let mut r = Receiver::new(1, 2);
            let mut out = Vec::new();
            r.take(0, 0, &first, 0, &mut out);
            let delta = fragments(b"delta", 8, 1, false, KEY_INTERVAL_US).next().unwrap();
            r.take(0, 8, &delta, KEY_INTERVAL_US, &mut out);
            let key = fragments(b"key", key_seq as u32, 0, true, KEY_INTERVAL_US).next().unwrap();
            let now = KEY_INTERVAL_US + delay;
            r.take(0, key_seq, &key, now, &mut out);
            let mut wants = Wants::default();
            r.poll(now, 40_000, &mut wants);
            assert!(r.seqs[&0].cleared.is_none() && wants.nacks.is_empty(),
                "key at {key_seq} after {delay} µs must discard the cleared range without requesting a suffix: {wants:?}");
        }
    }

    #[test]
    fn the_sender_cache_finds_ranges_and_forgets_after_a_second() {
        let mut c = SendCache::default();
        for seq in 0..10u64 {
            c.push(seq, seq * 100_000, vec![seq as u8]);
        }
        assert_eq!(c.find(3, 3).collect::<Vec<_>>(), vec![&[3u8][..], &[4], &[5]], "a range");
        c.push(10, 1_500_000, vec![10]);
        assert_eq!(c.find(0, 5).count(), 0, "older than a second: gone");
        assert_eq!(c.find(5, 255).count(), 6, "5 to 10 are still there");

        let mut g = KeyGate::default();
        assert!(!g.take(0), "no key frame unasked");
        g.request();
        assert!(g.take(0), "the first request is answered at once");
        g.made_key(0);
        g.request();
        g.request();
        assert!(!g.take(500_000), "inside a second, requests wait");
        assert!(g.take(1_000_000), "and share the next key frame");
        g.made_key(1_000_000);
        assert!(!g.take(3_000_000), "both were answered by it");
    }

    #[test]
    fn a_paced_share_key_keeps_its_received_fragments_while_making_progress() {
        let bytes = vec![91; MAX_FRAGMENT * 4];
        let mut receiver = Receiver::new(2, crate::wire::stream::SHARE);
        let mut complete = Vec::new();
        for (index, part) in fragments(&bytes, 0, 0, true, 0).enumerate() {
            let now = index as u64 * 400_000;
            receiver.poll(now, 20_000, &mut Wants::default());
            receiver.take(0, index as u64, &part, now, &mut complete);
        }
        assert_eq!(complete.len(), 1, "all fragments arrived in order over 1.2 s, without loss");
        assert_eq!(complete[0].data, bytes);
        assert_eq!(receiver.stats.skipped, 0);
    }
}
