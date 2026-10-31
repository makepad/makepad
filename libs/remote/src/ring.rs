//! The endpoint's replay ring: one fixed buffer (allocated once, 4 MiB by default) holding the
//! `stream` service's chunks, the oldest dropped to make room, plus the last 256 `watch` events.
//! Each record has a sequence number per kind. Producers (app threads) only copy bytes under a
//! short lock and never wait for a consumer; each subscriber (a service thread per channel) keeps
//! a [`Cursor`], reads what is new and learns from `lost` how much the ring dropped before it got
//! there. A reconnecting client resumes from the last sequence number it saw.

use crate::conn::{Channel, Event};
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub const DEFAULT_RING_BYTES: usize = 4 << 20;
pub const WATCH_KEEP: usize = 256;
/// per record in the stream ring: len u32 + seq u64
const REC_HEAD: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Stream,
    Watch,
}

/// A subscriber's place: the byte position of the next stream record (watch ignores it) and the
/// sequence number of the last record delivered.
#[derive(Clone, Copy, Default, Debug)]
pub struct Cursor {
    pub pos: u64,
    pub seq: u64,
}

pub struct Batch {
    /// records the ring dropped before this reader got to them
    pub lost: u64,
    pub recs: Vec<(u64, Vec<u8>)>,
}

struct Inner {
    buf: Vec<u8>,
    head: u64,
    tail: u64,
    stream_seq: u64,
    /// seq of the oldest record held (stream_seq + 1 when empty)
    oldest: u64,
    stream_lost_oversize: u64,
    watch: VecDeque<(u64, Vec<u8>)>,
    watch_seq: u64,
}

pub struct Ring {
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl Inner {
    fn phys(&self, abs: u64) -> usize {
        (abs % self.buf.len() as u64) as usize
    }

    fn copy_in(&mut self, abs: u64, data: &[u8]) {
        let cap = self.buf.len();
        let p = self.phys(abs);
        let first = data.len().min(cap - p);
        self.buf[p..p + first].copy_from_slice(&data[..first]);
        self.buf[..data.len() - first].copy_from_slice(&data[first..]);
    }

    fn copy_out(&self, abs: u64, out: &mut [u8]) {
        let cap = self.buf.len();
        let p = self.phys(abs);
        let first = out.len().min(cap - p);
        out[..first].copy_from_slice(&self.buf[p..p + first]);
        let rest = out.len() - first;
        out[first..].copy_from_slice(&self.buf[..rest]);
    }

    fn rec_len(&self, abs: u64) -> usize {
        let mut h = [0u8; 4];
        self.copy_out(abs, &mut h);
        u32::from_le_bytes(h) as usize
    }

    fn drop_oldest(&mut self) {
        let len = self.rec_len(self.head);
        self.head += (REC_HEAD + len) as u64;
        self.oldest += 1;
    }
}

impl Ring {
    pub fn new(bytes: usize) -> Ring {
        let cap = bytes.max(REC_HEAD * 2);
        Ring {
            inner: Mutex::new(Inner {
                buf: vec![0u8; cap],
                head: 0,
                tail: 0,
                stream_seq: 0,
                oldest: 1,
                stream_lost_oversize: 0,
                watch: VecDeque::with_capacity(WATCH_KEEP),
                watch_seq: 0,
            }),
            cv: Condvar::new(),
        }
    }

    /// Appends a stream chunk; returns its sequence number. A chunk larger than the ring is not
    /// kept (the next reader sees it in `lost`).
    pub fn push_stream(&self, chunk: &[u8]) -> u64 {
        let mut g = self.inner.lock().unwrap();
        g.stream_seq += 1;
        let seq = g.stream_seq;
        let need = REC_HEAD + chunk.len();
        if need > g.buf.len() {
            // everything before it is gone too: the ring cannot hold a record that size
            g.stream_lost_oversize += 1;
            while g.tail > g.head {
                g.drop_oldest();
            }
            g.oldest = seq + 1;
            return seq;
        }
        while g.buf.len() - ((g.tail - g.head) as usize) < need && g.tail > g.head {
            g.drop_oldest();
        }
        let tail = g.tail;
        let mut head = [0u8; REC_HEAD];
        head[..4].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        head[4..].copy_from_slice(&seq.to_le_bytes());
        g.copy_in(tail, &head);
        g.copy_in(tail + REC_HEAD as u64, chunk);
        g.tail += need as u64;
        drop(g);
        self.cv.notify_all();
        seq
    }

    /// Appends a watch event (one JSON line); returns its sequence number.
    pub fn push_watch(&self, line: &[u8]) -> u64 {
        let mut g = self.inner.lock().unwrap();
        g.watch_seq += 1;
        let seq = g.watch_seq;
        if g.watch.len() == WATCH_KEEP {
            g.watch.pop_front();
        }
        g.watch.push_back((seq, line.to_vec()));
        drop(g);
        self.cv.notify_all();
        seq
    }

    pub fn last_seq(&self, kind: Kind) -> u64 {
        let g = self.inner.lock().unwrap();
        match kind {
            Kind::Stream => g.stream_seq,
            Kind::Watch => g.watch_seq,
        }
    }

    /// Chunks that did not fit the ring at all.
    pub fn oversize_lost(&self) -> u64 {
        self.inner.lock().unwrap().stream_lost_oversize
    }

    /// A cursor that sees only what is pushed from now on.
    pub fn live(&self, kind: Kind) -> Cursor {
        let g = self.inner.lock().unwrap();
        match kind {
            Kind::Stream => Cursor { pos: g.tail, seq: g.stream_seq },
            Kind::Watch => Cursor { pos: 0, seq: g.watch_seq },
        }
    }

    /// A cursor that continues after record `seq` (a reconnect): whatever the ring still holds
    /// after it is delivered, and `lost` says how many records went missing in between.
    pub fn after(&self, kind: Kind, seq: u64) -> Cursor {
        let g = self.inner.lock().unwrap();
        match kind {
            Kind::Watch => Cursor { pos: 0, seq: seq.min(g.watch_seq) },
            Kind::Stream => {
                if seq >= g.stream_seq {
                    return Cursor { pos: g.tail, seq: g.stream_seq };
                }
                if seq + 1 < g.oldest {
                    return Cursor { pos: 0, seq };
                }
                // walk to the record with seq + 1
                let mut pos = g.head;
                let mut s = g.oldest;
                while s <= seq && pos < g.tail {
                    pos += (REC_HEAD + g.rec_len(pos)) as u64;
                    s += 1;
                }
                Cursor { pos, seq }
            }
        }
    }

    /// Waits until a record newer than `cur` exists (or `timeout`); true when there is one.
    pub fn wait(&self, kind: Kind, cur: &Cursor, timeout: Duration) -> bool {
        let mut g = self.inner.lock().unwrap();
        let newer = |g: &Inner| match kind {
            Kind::Stream => g.stream_seq > cur.seq,
            Kind::Watch => g.watch_seq > cur.seq,
        };
        if newer(&g) {
            return true;
        }
        g = self.cv.wait_timeout(g, timeout).unwrap().0;
        newer(&g)
    }

    /// Reads what is new after `cur` (about `max_bytes` of it) and moves the cursor.
    pub fn read(&self, kind: Kind, cur: &mut Cursor, max_bytes: usize) -> Batch {
        let g = self.inner.lock().unwrap();
        let mut r = Batch { lost: 0, recs: Vec::new() };
        match kind {
            Kind::Watch => {
                let oldest = g.watch.front().map_or(g.watch_seq + 1, |e| e.0);
                if oldest > cur.seq + 1 {
                    r.lost = oldest - 1 - cur.seq;
                }
                let mut bytes = 0;
                for (seq, line) in g.watch.iter().filter(|e| e.0 > cur.seq) {
                    if bytes > 0 && bytes + line.len() > max_bytes {
                        break;
                    }
                    bytes += line.len();
                    r.recs.push((*seq, line.clone()));
                }
                if let Some(last) = r.recs.last() {
                    cur.seq = last.0;
                } else if r.lost > 0 {
                    cur.seq = oldest - 1;
                }
            }
            Kind::Stream => {
                if cur.pos < g.head {
                    r.lost = g.oldest - 1 - cur.seq;
                    cur.pos = g.head;
                    cur.seq = g.oldest - 1;
                }
                let mut bytes = 0;
                while cur.pos < g.tail {
                    let len = g.rec_len(cur.pos);
                    if bytes > 0 && bytes + len > max_bytes {
                        break;
                    }
                    let mut seq = [0u8; 8];
                    g.copy_out(cur.pos + 4, &mut seq);
                    let mut data = vec![0u8; len];
                    g.copy_out(cur.pos + REC_HEAD as u64, &mut data);
                    let seq = u64::from_le_bytes(seq);
                    // chunks too big for the ring leave a gap in the sequence
                    if seq > cur.seq + 1 {
                        r.lost += seq - cur.seq - 1;
                    }
                    cur.pos += (REC_HEAD + len) as u64;
                    cur.seq = seq;
                    bytes += len;
                    r.recs.push((seq, data));
                }
            }
        }
        r
    }
}

/// Serves one `stream` / `watch` channel from the ring until it closes. Every DATA message is
/// `seq u64 LE | record`; `drop_marker(lost)` builds the record that reports a gap. Starts at
/// `cur`.
pub fn serve_feed(ring: &Ring, kind: Kind, ch: Channel, mut cur: Cursor, drop_marker: fn(u64) -> Vec<u8>) {
    let tx = ch.tx();
    loop {
        loop {
            match ch.try_recv() {
                Ok(Event::Closed(..)) | Err(false) => return,
                Ok(_) => {}
                Err(true) => break,
            }
        }
        if !ring.wait(kind, &cur, Duration::from_millis(500)) {
            continue;
        }
        let r = ring.read(kind, &mut cur, 128 << 10);
        let mut out: Vec<(u64, Vec<u8>)> = Vec::with_capacity(r.recs.len() + 1);
        if r.lost > 0 {
            let first = r.recs.first().map_or(cur.seq, |f| f.0 - 1);
            out.push((first, drop_marker(r.lost)));
        }
        out.extend(r.recs);
        for (seq, rec) in out {
            let mut m = Vec::with_capacity(8 + rec.len());
            m.extend_from_slice(&seq.to_le_bytes());
            m.extend_from_slice(&rec);
            if tx.send(m).is_err() {
                return;
            }
        }
    }
}
