//! The video history of a connection: the received bitstream of the last [`WINDOW_NS`] (the
//! encoded pictures as they arrived, never decoded frames), with each picture's host frame
//! number, its times and how its live decode went.
//!
//! It always runs. The decode thread records each packet after decoding it (off the network
//! thread, after the picture went to the view); recording moves the packet's own bytes into the
//! history (no copy). The history starts at a key run (every stream's key frame), so every
//! picture in it decodes: old packets go a key run at a time, once the next key run is itself
//! older than the window. A byte cap bounds it whatever the stream does.
//!
//! Pausing the view takes a [`Snapshot`] (the packets shared, not copied): the live history
//! goes on recording, the snapshot stays as it was for scrubbing and saving.

use std::collections::VecDeque;
use std::sync::Arc;

/// How far back the history reaches (user: "like 30 seconds").
pub const WINDOW_NS: u64 = 30_000_000_000;
/// The most bitstream the history keeps, whatever the window holds.
pub const MAX_BYTES: usize = 1 << 30;
/// The viewer asks for a key frame after this many changing pictures without one, so that any
/// picture of the history decodes from a key frame not far before it (the desktop's streams
/// otherwise have key frames only on request).
pub const KEY_EVERY: u32 = 240;
/// A picture changes something when one of its parts is over this size (the desktop's own
/// measure: smaller pictures change nothing). Only changing pictures count toward [`KEY_EVERY`]:
/// a still desktop gets no key frames (a key frame there would drop its near-lossless refinement
/// for a moment: a visible pulse).
pub const CHANGE_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub struct HistPacket {
    /// Position in the history (counts up from 0 for the connection).
    pub seq: u64,
    pub frame: u32,
    pub stream: u8,
    pub parts: u8,
    /// Where the part sits in the picture (tiles: columns and bands), and its decoded size
    /// (0 until the live decoder had it).
    pub x0: u32,
    pub y0: u32,
    /// `pref::H264` / `pref::HEVC` (the `Video`'s codec).
    pub codec: u8,
    pub size: [u32; 2],
    pub key: bool,
    /// Host clock.
    pub built_ns: u64,
    pub encoded_ns: u64,
    /// This machine's clock.
    pub received_ns: u64,
    /// The live decode of this packet (µs) and whether it failed.
    pub decode_us: u32,
    pub failed: bool,
    pub data: Arc<Vec<u8>>,
}

impl HistPacket {
    pub fn hevc(&self) -> bool {
        self.codec == makepad_desk_link::pref::HEVC
    }

    /// The decoder's codec for this packet.
    pub fn video_codec(&self) -> makepad_widgets::makepad_platform::video_file::StreamVideoCodec {
        use makepad_widgets::makepad_platform::video_file::StreamVideoCodec;
        if self.hevc() { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 }
    }
}

pub struct History {
    packets: VecDeque<HistPacket>,
    /// `seq` of each packet that starts a key run.
    syncs: VecDeque<u64>,
    bytes: usize,
    next_seq: u64,
    last_key: bool,
    window_ns: u64,
    max_bytes: usize,
    /// The stream size (`Welcome` / `Stream`), for saving.
    pub stream_size: [u32; 2],
}

impl Default for History {
    fn default() -> Self {
        Self::new(WINDOW_NS, MAX_BYTES)
    }
}

impl History {
    pub fn new(window_ns: u64, max_bytes: usize) -> Self {
        History { packets: VecDeque::with_capacity(8192), syncs: VecDeque::new(), bytes: 0, next_seq: 0, last_key: false, window_ns, max_bytes, stream_size: [0, 0] }
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The time the history spans (receive clock, ns).
    pub fn span_ns(&self) -> u64 {
        match (self.packets.front(), self.packets.back()) {
            (Some(a), Some(b)) => b.received_ns.saturating_sub(a.received_ns),
            _ => 0,
        }
    }

    /// A new session (reconnect): its streams start over with key frames.
    pub fn session_start(&mut self) {
        self.last_key = false;
    }

    /// Records a packet; returns its `seq` (for [`Self::note`]).
    pub fn push(&mut self, mut p: HistPacket) -> u64 {
        p.seq = self.next_seq;
        self.next_seq += 1;
        if p.key && !self.last_key {
            self.syncs.push_back(p.seq);
        }
        self.last_key = p.key;
        self.bytes += p.data.len();
        let newest = p.received_ns;
        let seq = p.seq;
        self.packets.push_back(p);
        self.evict(newest);
        seq
    }

    /// The live decode of packet `seq`: its time (µs), whether it failed, its decoded size.
    pub fn note(&mut self, seq: u64, decode_us: u32, failed: bool, size: [u32; 2]) {
        let Some(front) = self.packets.front().map(|p| p.seq) else { return };
        if let Some(p) = seq.checked_sub(front).and_then(|i| self.packets.get_mut(i as usize)) {
            p.decode_us = decode_us;
            p.failed = failed;
            p.size = size;
        }
    }

    fn time_of(&self, seq: u64) -> Option<u64> {
        let front = self.packets.front()?.seq;
        self.packets.get((seq - front) as usize).map(|p| p.received_ns)
    }

    fn evict(&mut self, newest: u64) {
        loop {
            let Some(front) = self.packets.front().map(|p| p.seq) else { return };
            while self.syncs.front().is_some_and(|s| *s < front) {
                self.syncs.pop_front();
            }
            let over = self.bytes > self.max_bytes;
            // the next key run after the front: what comes before it can go as a whole
            let cut = self.syncs.iter().copied().find(|s| *s > front);
            match cut {
                Some(cut) if over || self.time_of(cut).is_some_and(|t| t.saturating_add(self.window_ns) <= newest) => {
                    while self.packets.front().is_some_and(|p| p.seq < cut) {
                        let p = self.packets.pop_front().unwrap();
                        self.bytes -= p.data.len();
                    }
                }
                None if over && self.packets.len() > 1 => {
                    // no later key run: the oldest pictures go one by one (the front no longer
                    // decodes until the next key run)
                    let p = self.packets.pop_front().unwrap();
                    self.bytes -= p.data.len();
                }
                _ => return,
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.packets.iter().cloned().collect(), &self.syncs, self.stream_size)
    }
}

/// A picture of a snapshot: its packets (one per tile, or one), its sizes and times.
#[derive(Clone, Debug)]
pub struct PicInfo {
    pub frame: u32,
    /// Its packets: `packets[first..first + count]`.
    pub first: usize,
    pub count: usize,
    pub key: bool,
    pub bytes: usize,
    pub built_ns: u64,
    pub received_ns: u64,
    pub decode_us: u32,
    pub failed: bool,
    /// The picture its decode starts from (the key run at or before it); None: no key run
    /// before it is in the history (it does not decode).
    pub sync: Option<usize>,
}

/// [`Snapshot::flags`] bits.
pub mod flag {
    pub const GAP: u8 = 1;
    pub const SKIP: u8 = 2;
    pub const SPIKE: u8 = 4;
    pub const FAILED: u8 = 8;
}

pub struct Snapshot {
    pub packets: Vec<HistPacket>,
    pub pictures: Vec<PicInfo>,
    pub stream_size: [u32; 2],
    /// Medians over the snapshot (ns, bytes): the picture interval (host clock) and a non-key
    /// picture's size, the yardsticks of the gap and spike marks.
    pub median_dt_ns: u64,
    pub median_bytes: usize,
}

impl Snapshot {
    pub fn new(packets: Vec<HistPacket>, syncs: &VecDeque<u64>, stream_size: [u32; 2]) -> Snapshot {
        let mut pictures: Vec<PicInfo> = Vec::new();
        let mut sync = None;
        for (i, p) in packets.iter().enumerate() {
            // the tiles of one picture arrive together, with the same frame number
            if let Some(last) = pictures.last_mut().filter(|l| l.frame == p.frame && p.parts > 1 && l.count < p.parts as usize) {
                last.count += 1;
                last.key |= p.key;
                last.bytes += p.data.len();
                last.decode_us += p.decode_us;
                last.failed |= p.failed;
                last.received_ns = last.received_ns.max(p.received_ns);
                continue;
            }
            if syncs.binary_search(&p.seq).is_ok() {
                sync = Some(pictures.len());
            }
            pictures.push(PicInfo {
                frame: p.frame,
                first: i,
                count: 1,
                key: p.key,
                bytes: p.data.len(),
                built_ns: p.built_ns,
                received_ns: p.received_ns,
                decode_us: p.decode_us,
                failed: p.failed,
                sync,
            });
        }
        let mut dts: Vec<u64> = pictures.windows(2).map(|w| w[1].built_ns.saturating_sub(w[0].built_ns)).filter(|d| *d > 0).collect();
        dts.sort_unstable();
        let mut sizes: Vec<usize> = pictures.iter().filter(|p| !p.key).map(|p| p.bytes).collect();
        sizes.sort_unstable();
        Snapshot {
            median_dt_ns: dts.get(dts.len() / 2).copied().unwrap_or(0),
            median_bytes: sizes.get(sizes.len() / 2).copied().unwrap_or(0),
            packets,
            pictures,
            stream_size,
        }
    }

    pub fn len(&self) -> usize {
        self.pictures.len()
    }

    pub fn packets_of(&self, i: usize) -> &[HistPacket] {
        let p = &self.pictures[i];
        &self.packets[p.first..p.first + p.count]
    }

    /// Host time from picture `i - 1` to `i` (ns; 0 for the first).
    pub fn dt_ns(&self, i: usize) -> u64 {
        if i == 0 {
            return 0;
        }
        self.pictures[i].built_ns.saturating_sub(self.pictures[i - 1].built_ns)
    }

    /// [`Self::marks`] as bits (no allocation; the timeline asks for every picture).
    pub fn flags(&self, i: usize) -> u8 {
        let p = &self.pictures[i];
        let mut f = 0;
        if p.failed || p.sync.is_none() {
            f |= flag::FAILED;
        }
        if i > 0 {
            let skipped = p.frame.wrapping_sub(self.pictures[i - 1].frame).wrapping_sub(1);
            if skipped > 0 && skipped < u32::MAX / 2 {
                f |= flag::SKIP;
            }
            if self.median_dt_ns > 0 && self.dt_ns(i) > self.median_dt_ns * 2 {
                f |= flag::GAP;
            }
        }
        if !p.key && self.median_bytes > 0 && p.bytes > self.median_bytes * 4 && p.bytes > 16 << 10 {
            f |= flag::SPIKE;
        }
        f
    }

    /// What is unusual about picture `i` (empty: nothing), short words for the timeline and the
    /// clip notes: a gap (over twice the usual interval), a frame-number skip (the desktop made
    /// pictures this viewer did not get), a size spike, a key frame, a failed decode.
    pub fn marks(&self, i: usize) -> Vec<String> {
        let p = &self.pictures[i];
        let mut out = Vec::new();
        if p.key {
            out.push("key".to_string());
        }
        if p.failed {
            out.push("decode failed".to_string());
        }
        if p.sync.is_none() {
            out.push("no key frame before it".to_string());
        }
        if i > 0 {
            let prev = &self.pictures[i - 1];
            let skipped = p.frame.wrapping_sub(prev.frame).wrapping_sub(1);
            if skipped > 0 && skipped < u32::MAX / 2 {
                out.push(format!("{skipped} frame{} skipped", if skipped == 1 { "" } else { "s" }));
            }
            let dt = self.dt_ns(i);
            if self.median_dt_ns > 0 && dt > self.median_dt_ns * 2 {
                out.push(format!("gap {:.1} ms", dt as f64 / 1e6));
            }
        }
        if !p.key && self.median_bytes > 0 && p.bytes > self.median_bytes * 4 && p.bytes > 16 << 10 {
            out.push(format!("size {:.1}x", p.bytes as f64 / self.median_bytes as f64));
        }
        out
    }

    /// The picture nearest to host time `built_ns`.
    pub fn at_time(&self, built_ns: u64) -> usize {
        match self.pictures.binary_search_by_key(&built_ns, |p| p.built_ns) {
            Ok(i) => i,
            Err(i) if i == 0 => 0,
            Err(i) if i >= self.pictures.len() => self.pictures.len().saturating_sub(1),
            Err(i) => {
                if built_ns - self.pictures[i - 1].built_ns <= self.pictures[i].built_ns - built_ns {
                    i - 1
                } else {
                    i
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(frame: u32, stream: u8, parts: u8, key: bool, t_ms: u64, bytes: usize) -> HistPacket {
        HistPacket {
            seq: 0,
            frame,
            stream,
            parts,
            x0: 0,
            y0: stream as u32 * 100,
            codec: makepad_desk_link::pref::H264,
            size: [0, 0],
            key,
            built_ns: t_ms * 1_000_000,
            encoded_ns: t_ms * 1_000_000,
            received_ns: t_ms * 1_000_000,
            decode_us: 10,
            failed: false,
            data: Arc::new(vec![0; bytes]),
        }
    }

    #[test]
    fn evicts_whole_key_runs_past_the_window() {
        // 1 s window, a key frame every 10 pictures, 10 pictures a second
        let mut h = History::new(1_000_000_000, 1 << 30);
        for f in 0..100u32 {
            h.push(packet(f, 0, 1, f % 10 == 0, f as u64 * 100, 10));
        }
        let s = h.snapshot();
        // the front is a key frame and at least the window is kept
        assert!(s.pictures[0].key);
        assert_eq!(s.pictures[0].sync, Some(0));
        assert!(h.span_ns() >= 1_000_000_000);
        assert!(h.span_ns() < 2_000_000_000);
        assert!(s.pictures.iter().all(|p| p.sync.is_some()));
        assert_eq!(s.pictures.last().unwrap().frame, 99);
        assert_eq!(h.bytes(), s.packets.len() * 10);
    }

    #[test]
    fn tiles_group_into_pictures_and_keep_their_key_run() {
        let mut h = History::new(u64::MAX / 4, 1 << 30);
        for f in 0..6u32 {
            for s in 0..2u8 {
                h.push(packet(f, s, 2, f == 0 || f == 3, f as u64 * 8, 100));
            }
        }
        let s = h.snapshot();
        assert_eq!(s.len(), 6);
        assert_eq!(s.pictures[1].count, 2);
        assert_eq!(s.pictures[1].bytes, 200);
        assert_eq!(s.pictures[2].sync, Some(0));
        assert_eq!(s.pictures[4].sync, Some(3));
        assert_eq!(s.packets_of(4).len(), 2);
        assert_eq!(s.packets_of(4)[1].stream, 1);
    }

    #[test]
    fn interleaved_key_run_is_one_sync() {
        // two engines in turn: a key request makes the next two pictures key frames
        let mut h = History::new(u64::MAX / 4, 1 << 30);
        for f in 0..8u32 {
            h.push(packet(f, (f % 2) as u8, 1, f == 0 || f == 1 || f == 4 || f == 5, f as u64 * 8, 50));
        }
        let s = h.snapshot();
        assert_eq!(s.len(), 8);
        assert_eq!(s.pictures[1].sync, Some(0));
        assert_eq!(s.pictures[5].sync, Some(4));
        assert_eq!(s.pictures[7].sync, Some(4));
    }

    #[test]
    fn byte_cap_bounds_the_history() {
        let mut h = History::new(u64::MAX / 4, 1000);
        for f in 0..50u32 {
            h.push(packet(f, 0, 1, f == 0, f as u64, 100));
        }
        assert!(h.bytes() <= 1000);
        let s = h.snapshot();
        // the key frame went: nothing before the next key run decodes
        assert!(s.pictures.iter().all(|p| p.sync.is_none()));
        h.push(packet(50, 0, 1, true, 50, 100));
        let s = h.snapshot();
        assert_eq!(s.pictures.last().unwrap().sync, Some(s.len() - 1));
    }

    #[test]
    fn marks_gaps_skips_and_spikes() {
        let mut h = History::new(u64::MAX / 4, 1 << 30);
        let mut t = 0;
        for f in 0..20u32 {
            t += if f == 10 { 40 } else { 8 };
            let frame = if f >= 15 { f + 2 } else { f };
            h.push(packet(frame, 0, 1, f == 0, t, if f == 12 { 100_000 } else { 5000 }));
        }
        let s = h.snapshot();
        assert_eq!(s.median_dt_ns, 8_000_000);
        assert!(s.marks(10).iter().any(|m| m.starts_with("gap 40.0")));
        assert!(s.marks(12).iter().any(|m| m.starts_with("size")));
        assert!(s.marks(15).iter().any(|m| m == "2 frames skipped"));
        assert!(s.marks(5).is_empty());
        assert_eq!(s.at_time(t * 1_000_000), 19);
        assert_eq!(s.at_time(0), 0);
    }
}
