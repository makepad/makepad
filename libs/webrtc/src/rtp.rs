//! RTP in, H.264 access units out (RFC 3550, RFC 6184).
//!
//! Packets go into a reorder buffer by their extended sequence number and
//! come out in order; the stream's first packet sets where it starts. A missing packet is asked for again (a NACK) at once;
//! when it has not come by the time the hole is `reorder_wait` old, it is
//! given up: the access unit it was part of cannot be decoded, and neither
//! can anything after it until the next keyframe, so those are dropped and a
//! keyframe is asked for (a PLI). A decoder never sees a broken picture.
//! At most [`MAX_HOLES`] holes are waited for at once: a wider loss burst,
//! or a sender jumping far ahead, gives them all up at once with one PLI
//! instead of a NACK per hole. An access unit past [`MAX_ACCESS_UNIT`]
//! bytes (a sender that never ends one) is dropped the same way.
//!
//! Depacketizing: single NAL unit packets, STAP-A and FU-A
//! (packetization-mode 1). Each access unit is Annex-B (00 00 00 01 before
//! every NAL unit), complete at the packet with the marker bit, or at the
//! first packet of the next timestamp when the marker was lost.

use std::collections::BTreeMap;

/// What the receiver needs of an RTP packet's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtpHeader {
    pub marker: bool,
    pub payload_type: u8,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    /// The payload is `packet[payload.0..payload.1]` (padding removed).
    pub payload: (usize, usize),
}

/// The header of an RTP version 2 packet; None when it is not one.
pub fn parse(packet: &[u8]) -> Option<RtpHeader> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return None;
    }
    let csrcs = (packet[0] & 0x0f) as usize;
    let mut start = 12 + 4 * csrcs;
    if packet[0] & 0x10 != 0 {
        let words = u16::from_be_bytes([*packet.get(start + 2)?, *packet.get(start + 3)?]) as usize;
        start += 4 + 4 * words;
    }
    let mut end = packet.len();
    if packet[0] & 0x20 != 0 {
        end = end.checked_sub(*packet.last()? as usize)?;
    }
    if start > end {
        return None;
    }
    Some(RtpHeader {
        marker: packet[1] & 0x80 != 0,
        payload_type: packet[1] & 0x7f,
        seq: u16::from_be_bytes([packet[2], packet[3]]),
        timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        ssrc: u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        payload: (start, end),
    })
}

/// RFC 5761: an RTCP packet on the shared port has a packet type of 192..=223.
pub fn is_rtcp(packet: &[u8]) -> bool {
    packet.len() >= 8 && packet[0] >> 6 == 2 && (192..=223).contains(&packet[1])
}

/// One picture, Annex-B.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Vec<u8>,
    pub timestamp: u32,
    /// It holds an IDR slice: decoding can start here.
    pub keyframe: bool,
}

/// What the receiver asks the sender for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Feedback {
    /// These sequence numbers again.
    Nack(Vec<u16>),
    /// A keyframe.
    Pli,
}

struct Held {
    marker: bool,
    timestamp: u32,
    payload: Vec<u8>,
}

/// One H.264 stream (one SSRC) from packets to access units.
pub struct H264Receiver {
    /// Seconds a hole is waited for before it is given up.
    pub reorder_wait: f64,
    /// Seconds between keyframe requests while one is awaited.
    pub pli_every: f64,
    held: BTreeMap<u64, Held>,
    highest: Option<u64>,
    next: Option<u64>,
    /// When each missing sequence number was first missed.
    missing: BTreeMap<u64, f64>,
    nack: Vec<u16>,
    au: Vec<u8>,
    au_timestamp: Option<u32>,
    au_keyframe: bool,
    au_broken: bool,
    fragment: Option<Vec<u8>>,
    want_keyframe: bool,
    last_pli: Option<f64>,
    /// Packets taken, packets given up (for receiver reports).
    pub received: u64,
    pub lost: u64,
}

impl Default for H264Receiver {
    fn default() -> Self {
        H264Receiver {
            reorder_wait: 0.15,
            pli_every: 0.5,
            held: BTreeMap::new(),
            highest: None,
            next: None,
            missing: BTreeMap::new(),
            nack: Vec::new(),
            au: Vec::new(),
            au_timestamp: None,
            au_keyframe: false,
            au_broken: false,
            fragment: None,
            want_keyframe: true,
            last_pli: None,
            received: 0,
            lost: 0,
        }
    }
}

const START: [u8; 4] = [0, 0, 0, 1];

/// Holes waited for at once. A keyframe at our bitrate is a few hundred
/// packets at most; past this, asking packet by packet costs more than a
/// keyframe.
pub const MAX_HOLES: u64 = 256;

/// The most bytes of one access unit (and of a fragmented NAL unit in it):
/// far over any phone camera's keyframe.
pub const MAX_ACCESS_UNIT: usize = 4 << 20;

impl H264Receiver {
    /// The highest extended sequence number seen (for receiver reports).
    pub fn highest(&self) -> Option<u64> {
        self.highest
    }

    /// Takes one plain RTP packet of this stream; answers the access units
    /// it completes.
    pub fn push(&mut self, packet: &[u8], now: f64) -> Vec<AccessUnit> {
        let Some(header) = parse(packet) else { return Vec::new() };
        let ext = self.extend(header.seq);
        if self.next.is_some_and(|next| ext < next) || self.held.contains_key(&ext) {
            // Late (already given up) or a duplicate.
            return Vec::new();
        }
        self.received += 1;
        self.missing.remove(&ext);
        if self.highest.is_none_or(|h| ext > h) {
            let from = self.highest.map_or(ext, |h| h + 1);
            if self.missing.len() as u64 + (ext - from) > MAX_HOLES {
                self.resync(ext, ext - from);
            } else {
                for gap in from..ext {
                    self.missing.insert(gap, now);
                    self.nack.push(gap as u16);
                }
            }
            self.highest = Some(ext);
        }
        if self.next.is_none() {
            // The stream begins: its first keyframe is on its way, so the
            // first keyframe request waits one `pli_every`.
            self.next = Some(ext);
            self.last_pli = Some(now);
        }
        let payload = packet[header.payload.0..header.payload.1].to_vec();
        self.held.insert(ext, Held { marker: header.marker, timestamp: header.timestamp, payload });
        self.drain(now)
    }

    /// Gives up holes older than `reorder_wait`; answers what that frees.
    pub fn poll(&mut self, now: f64) -> Vec<AccessUnit> {
        self.drain(now)
    }

    /// NACKs for new holes, and a PLI while a keyframe is awaited.
    pub fn feedback(&mut self, now: f64) -> Vec<Feedback> {
        let mut out = Vec::new();
        if !self.nack.is_empty() {
            out.push(Feedback::Nack(std::mem::take(&mut self.nack)));
        }
        if self.want_keyframe && self.next.is_some() && self.last_pli.is_none_or(|t| now - t >= self.pli_every) {
            self.last_pli = Some(now);
            out.push(Feedback::Pli);
        }
        out
    }

    /// When [`H264Receiver::poll`] or [`H264Receiver::feedback`] next has
    /// something to do: a hole to give up, or a keyframe to ask for again.
    pub fn next_deadline(&self) -> Option<f64> {
        // Holes are missed in sequence order: the first is the oldest.
        let hole = self.missing.first_key_value().map(|(_, since)| since + self.reorder_wait);
        let pli = if self.want_keyframe && self.next.is_some() { Some(self.last_pli.map_or(0.0, |t| t + self.pli_every)) } else { None };
        [hole, pli].into_iter().flatten().reduce(f64::min)
    }

    /// Too many holes to wait for: every one given up, and with them all
    /// held and every picture under way; decoding starts again at the next
    /// keyframe, asked for once `pli_every` allows.
    fn resync(&mut self, at: u64, skipped: u64) {
        self.lost += self.missing.len() as u64 + skipped;
        self.missing.clear();
        self.held.clear();
        self.nack.clear();
        self.next = Some(at);
        self.au = Vec::new();
        self.au_timestamp = None;
        self.au_keyframe = false;
        self.fragment = None;
        self.au_broken = true;
        self.want_keyframe = true;
    }

    /// An access unit (or a fragment) past [`MAX_ACCESS_UNIT`]: a sender
    /// that never ends it. Dropped, and a keyframe asked for.
    fn oversize(&mut self) {
        self.au = Vec::new();
        self.fragment = None;
        self.au_broken = true;
        self.want_keyframe = true;
    }

    fn extend(&self, seq: u16) -> u64 {
        let Some(highest) = self.highest else { return (1 << 16) | seq as u64 };
        let base = highest & !0xffff;
        // The candidate nearest the highest seen, across a wrap either way.
        [base.wrapping_sub(1 << 16), base, base + (1 << 16)]
            .into_iter()
            .map(|b| b | seq as u64)
            .min_by_key(|c| c.abs_diff(highest))
            .unwrap_or(base | seq as u64)
    }

    fn drain(&mut self, now: f64) -> Vec<AccessUnit> {
        let mut out = Vec::new();
        while let Some(next) = self.next {
            if let Some(held) = self.held.remove(&next) {
                self.next = Some(next + 1);
                self.depacketize(held, &mut out);
                continue;
            }
            let given_up = match self.missing.get(&next) {
                Some(&since) => now - since >= self.reorder_wait,
                None => false,
            };
            if !given_up || self.held.is_empty() {
                break;
            }
            // The hole is not coming: skip it, and with it the picture it
            // was part of.
            self.missing.remove(&next);
            self.lost += 1;
            self.next = Some(next + 1);
            self.au_broken = true;
            self.fragment = None;
        }
        out
    }

    fn depacketize(&mut self, held: Held, out: &mut Vec<AccessUnit>) {
        if self.au_timestamp.is_some_and(|t| t != held.timestamp) {
            // The marker of the last picture was lost: it ends here.
            self.au_broken = true;
            self.finish(out);
        }
        self.au_timestamp = Some(held.timestamp);
        let p = &held.payload;
        match p.first().map(|b| b & 0x1f) {
            Some(1..=23) => self.nal(p),
            Some(24) => {
                // STAP-A: (16-bit size, NAL unit) repeated.
                let mut at = 1;
                while at + 2 <= p.len() {
                    let size = u16::from_be_bytes([p[at], p[at + 1]]) as usize;
                    let Some(nal) = p.get(at + 2..at + 2 + size) else {
                        self.au_broken = true;
                        break;
                    };
                    self.nal(nal);
                    at += 2 + size;
                }
            }
            Some(28) if p.len() >= 2 => {
                let (start, end) = (p[1] & 0x80 != 0, p[1] & 0x40 != 0);
                if self.au_broken {
                    // The picture is lost already: nothing more of it is kept.
                } else if start {
                    let mut nal = vec![(p[0] & 0xe0) | (p[1] & 0x1f)];
                    nal.extend_from_slice(&p[2..]);
                    self.fragment = Some(nal);
                } else if let Some(fragment) = &mut self.fragment {
                    fragment.extend_from_slice(&p[2..]);
                } else {
                    // A middle or end without its start: its start was lost.
                    self.au_broken = true;
                }
                if self.fragment.as_ref().is_some_and(|f| self.au.len() + START.len() + f.len() > MAX_ACCESS_UNIT) {
                    self.oversize();
                }
                if end {
                    if let Some(nal) = self.fragment.take() {
                        self.nal(&nal);
                    }
                }
            }
            _ => self.au_broken = true,
        }
        if held.marker {
            self.finish(out);
        }
    }

    fn nal(&mut self, nal: &[u8]) {
        if self.au_broken {
            return;
        }
        if self.au.len() + START.len() + nal.len() > MAX_ACCESS_UNIT {
            self.oversize();
            return;
        }
        if nal.first().is_some_and(|b| b & 0x1f == 5) {
            self.au_keyframe = true;
        }
        self.au.extend_from_slice(&START);
        self.au.extend_from_slice(nal);
    }

    fn finish(&mut self, out: &mut Vec<AccessUnit>) {
        let broken = self.au_broken || self.fragment.is_some();
        let au = AccessUnit { data: std::mem::take(&mut self.au), timestamp: self.au_timestamp.unwrap_or(0), keyframe: self.au_keyframe };
        self.au_timestamp = None;
        self.au_keyframe = false;
        self.au_broken = false;
        self.fragment = None;
        if broken || au.data.is_empty() {
            self.want_keyframe = true;
        } else if au.keyframe || !self.want_keyframe {
            self.want_keyframe = false;
            out.push(au);
        }
    }
}

/// The sender's side, for tests: RTP packets of an Annex-B access unit,
/// each NAL unit as one packet when it fits in `mtu`, as FU-A fragments
/// when not, and runs of small ones together as STAP-A.
#[cfg(any(test, feature = "browser"))]
pub fn packetize(au: &[u8], timestamp: u32, seq: &mut u16, ssrc: u32, mtu: usize) -> Vec<Vec<u8>> {
    let nals = split_annex_b(au);
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    let mut stap: Vec<u8> = Vec::new();
    for nal in nals {
        if nal.len() + 3 <= mtu / 4 {
            if stap.is_empty() {
                stap.push((nal[0] & 0xe0) | 24);
            }
            stap.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            stap.extend_from_slice(nal);
            continue;
        }
        if !stap.is_empty() {
            payloads.push(std::mem::take(&mut stap));
        }
        if nal.len() <= mtu {
            payloads.push(nal.to_vec());
            continue;
        }
        let body = &nal[1..];
        let chunks: Vec<&[u8]> = body.chunks(mtu - 2).collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let mut fu = vec![(nal[0] & 0xe0) | 28, nal[0] & 0x1f];
            if i == 0 {
                fu[1] |= 0x80;
            }
            if i == chunks.len() - 1 {
                fu[1] |= 0x40;
            }
            fu.extend_from_slice(chunk);
            payloads.push(fu);
        }
    }
    if !stap.is_empty() {
        payloads.push(stap);
    }
    let last = payloads.len() - 1;
    payloads
        .into_iter()
        .enumerate()
        .map(|(i, payload)| {
            let mut packet = vec![0x80, 102 | if i == last { 0x80 } else { 0 }];
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&ssrc.to_be_bytes());
            packet.extend_from_slice(&payload);
            *seq = seq.wrapping_add(1);
            packet
        })
        .collect()
}

#[cfg(any(test, feature = "browser"))]
fn split_annex_b(au: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 4 <= au.len() {
        if au[i..i + 4] == START {
            starts.push(i + 4);
            i += 4;
        } else {
            i += 1;
        }
    }
    starts.iter().enumerate().map(|(k, &s)| &au[s..starts.get(k + 1).map_or(au.len(), |&n| n - 4)]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in access unit: NAL units of the given types and sizes,
    /// filled so that no start code appears inside.
    fn au(nals: &[(u8, usize)], salt: u8) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, &(kind, size)) in nals.iter().enumerate() {
            out.extend_from_slice(&START);
            out.push(0x60 | kind);
            out.extend((1..size).map(|i| ((i * 7 + k * 13 + salt as usize) % 251 + 2) as u8));
        }
        out
    }

    fn stream() -> Vec<AccessUnit> {
        let mut aus = Vec::new();
        for i in 0..12u32 {
            let keyframe = i % 6 == 0;
            let nals: Vec<(u8, usize)> = if keyframe { vec![(7, 12), (8, 5), (5, 4000)] } else { vec![(1, 300 + 200 * (i as usize % 4))] };
            aus.push(AccessUnit { data: au(&nals, i as u8), timestamp: 90_000 / 8 * i, keyframe });
        }
        aus
    }

    #[test]
    fn packets_become_the_pictures_they_were_made_of() {
        let aus = stream();
        // Across a sequence number wrap, every packet after the first (which
        // sets where the stream starts) reordered with its neighbour: the
        // same pictures come out, in order.
        let mut seq = 65530u16;
        let mut packets: Vec<Vec<u8>> = aus.iter().flat_map(|a| packetize(&a.data, a.timestamp, &mut seq, 7, 1200)).collect();
        let fragments = packets.iter().filter(|p| p[12] & 0x1f == 28).count();
        assert!(fragments >= 6, "each keyframe split into FU-A fragments ({fragments})");
        assert!(packets.iter().any(|p| p[12] & 0x1f == 24), "parameter sets aggregated in a STAP-A");
        for pair in packets[1..].chunks_mut(2) {
            pair.reverse();
        }
        let mut rx = H264Receiver::default();
        let mut got = Vec::new();
        for (i, p) in packets.iter().enumerate() {
            got.extend(rx.push(p, i as f64 * 0.001));
        }
        assert_eq!(got, aus, "reordered packets across the wrap give back every picture");
        assert_eq!(rx.lost, 0);
        let fb = rx.feedback(1.0);
        assert!(!fb.contains(&Feedback::Pli), "no keyframe asked for: the stream began with one");

        // One FU-A fragment of the second keyframe lost: the pictures from
        // there to the next keyframe are dropped, the hole NACKed at once,
        // a keyframe asked for once it is given up.
        let mut seq = 100u16;
        let per_au: Vec<Vec<Vec<u8>>> = aus.iter().map(|a| packetize(&a.data, a.timestamp, &mut seq, 7, 1200)).collect();
        let mut rx = H264Receiver::default();
        let mut got = Vec::new();
        let mut now = 0.0;
        let lost_seq = parse(&per_au[6][2]).unwrap().seq;
        for (k, packets) in per_au.iter().enumerate() {
            for (j, p) in packets.iter().enumerate() {
                if k == 6 && j == 2 {
                    continue;
                }
                now += 0.01;
                got.extend(rx.push(p, now));
            }
            if k == 6 {
                assert_eq!(rx.feedback(now), vec![Feedback::Nack(vec![lost_seq])], "the hole is asked for at once");
            }
            now += 0.1;
            got.extend(rx.poll(now));
        }
        assert!(rx.feedback(now).contains(&Feedback::Pli), "a keyframe is asked for after the loss");
        assert_eq!(got, aus[..6].to_vec(), "nothing from the broken keyframe on is handed out (its P frames depend on it)");
        assert_eq!(rx.lost, 1);

        // The keyframe that answers the PLI starts the pictures again.
        let next = AccessUnit { data: au(&[(7, 12), (8, 5), (5, 3000)], 99), timestamp: 999_000, keyframe: true };
        let mut got = Vec::new();
        for p in packetize(&next.data, next.timestamp, &mut seq, 7, 1200) {
            got.extend(rx.push(&p, now + 0.2));
        }
        assert_eq!(got, vec![next]);
    }

    #[test]
    fn headers_and_demultiplexing() {
        // Version 2, padding, one CSRC, an extension of one word, marker, PT 102.
        let mut p = vec![0xb1, 0x80 | 102, 0x12, 0x34, 0, 0, 0, 9, 0, 0, 0, 7, 0, 0, 0, 5, 0xbe, 0xde, 0, 1, 1, 2, 3, 4];
        p.extend_from_slice(&[0x41, 0xaa, 0, 0, 3]);
        let h = parse(&p).unwrap();
        assert_eq!((h.marker, h.payload_type, h.seq, h.timestamp, h.ssrc), (true, 102, 0x1234, 9, 7));
        assert_eq!(&p[h.payload.0..h.payload.1], &[0x41, 0xaa], "CSRCs, the extension and the padding skipped");
        assert!(parse(&p[..10]).is_none() && parse(&[0x40; 20]).is_none(), "short packets and other versions refused");
        assert!(is_rtcp(&[0x81, 201, 0, 7, 0, 0, 0, 1]) && !is_rtcp(&p), "RTCP packet types 192..=223");
    }

    /// Hostile senders cost bounded memory and feedback: a jump far ahead
    /// gives the holes up with one PLI and no NACKs, holes waited for never
    /// pass [`MAX_HOLES`], and an access unit that never ends is dropped at
    /// [`MAX_ACCESS_UNIT`]. Decoding picks up at the next keyframe.
    #[test]
    fn hostile_senders_cost_bounded_memory_and_feedback() {
        let aus = stream();
        let raw = |seq: u16, timestamp: u32, marker: bool, payload: &[u8]| -> Vec<u8> {
            let mut p = vec![0x80, (marker as u8) << 7 | 102];
            p.extend_from_slice(&seq.to_be_bytes());
            p.extend_from_slice(&timestamp.to_be_bytes());
            p.extend_from_slice(&7u32.to_be_bytes());
            p.extend_from_slice(payload);
            p
        };
        let mut rx = H264Receiver::default();
        let mut seq = 100u16;
        let mut got = Vec::new();
        for p in packetize(&aus[0].data, aus[0].timestamp, &mut seq, 7, 1200) {
            got.extend(rx.push(&p, 0.0));
        }
        assert_eq!(got, vec![aus[0].clone()], "the stream starts with its keyframe");

        // Far ahead: no hole is waited for, nothing is NACKed, one PLI.
        rx.push(&raw(seq.wrapping_add(30_000), 9_000, true, &[0x41, 1, 2, 3]), 0.01);
        assert!(rx.missing.is_empty() && rx.held.is_empty(), "the holes given up at once");
        assert!(rx.feedback(0.02).is_empty(), "no NACK for them");
        assert_eq!(rx.feedback(0.6), vec![Feedback::Pli], "a keyframe asked for instead");
        // Steps just short of the bound, then over it: never more holes than that.
        let mut seq = seq.wrapping_add(30_001);
        for (k, step) in [200u16, 50, 200, 255, 300].into_iter().enumerate() {
            seq = seq.wrapping_add(step);
            rx.push(&raw(seq, 9_000 + k as u32, true, &[0x41, 1]), 1.0);
            assert!(rx.missing.len() as u64 <= MAX_HOLES, "{} holes waited for", rx.missing.len());
            let nacked: usize = rx.feedback(1.0).iter().map(|f| if let Feedback::Nack(n) = f { n.len() } else { 0 }).sum();
            assert!(nacked as u64 <= MAX_HOLES, "{nacked} NACKed at once");
        }

        // An access unit that never ends, as fragments and as whole NAL units.
        let mut fu = vec![0x7c, 0x85];
        fu.extend_from_slice(&[9; 1000]);
        let middle = [&[0x7c, 0x05][..], &[9; 1000]].concat();
        let whole = [&[0x41][..], &[9; 1000]].concat();
        for (timestamp, first, rest) in [(20_000, &fu, &middle), (21_000, &whole, &whole)] {
            seq = seq.wrapping_add(1);
            rx.push(&raw(seq, timestamp, false, first), 2.0);
            for _ in 0..(MAX_ACCESS_UNIT / 1000 + 100) {
                seq = seq.wrapping_add(1);
                assert!(rx.push(&raw(seq, timestamp, false, rest), 2.0).is_empty());
            }
            let held = rx.au.len() + rx.fragment.as_ref().map_or(0, Vec::len);
            assert!(held <= MAX_ACCESS_UNIT && rx.want_keyframe, "dropped at the bound ({held} bytes held), a keyframe wanted");
        }
        let mut got = Vec::new();
        seq = seq.wrapping_add(1);
        for p in packetize(&aus[6].data, 30_000, &mut seq, 7, 1200) {
            got.extend(rx.push(&p, 3.0));
        }
        assert_eq!(got.len(), 1, "decoding picks up at the keyframe");
        assert!(got[0].keyframe && got[0].data == aus[6].data);
    }
}
