//! The `stream` service skeleton: binary chunks `tag u8 | len varint | payload` pushed by a
//! host-side [`StreamSink`] (the gpuprof / profilemode registry defines the tags; this module
//! defines only [`TAG_DROP`]). Chunks go through the endpoint's ring, so a slow or absent client
//! never makes the producer wait: the ring drops its oldest chunks and the client is told how
//! many with a DROP chunk. Each DATA message is `seq u64 LE | chunk`.

use crate::endpoint::Factory;
use crate::ring::{serve_feed, Kind, Ring};
use std::sync::Arc;

/// `payload = lost varint`: chunks the ring dropped before the reader got to them.
pub const TAG_DROP: u8 = 0xFF;

pub fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// (value, bytes used)
pub fn read_varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, &x) in b.iter().enumerate().take(10) {
        v |= ((x & 0x7f) as u64) << (7 * i);
        if x & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

/// Splits one chunk: (tag, payload).
pub fn parse_chunk(b: &[u8]) -> Option<(u8, &[u8])> {
    let tag = *b.first()?;
    let (len, n) = read_varint(&b[1..])?;
    let end = 1 + n + len as usize;
    (end <= b.len()).then(|| (tag, &b[1 + n..end]))
}

#[derive(Clone)]
pub struct StreamSink(pub(crate) Arc<Ring>);

impl StreamSink {
    /// Publishes one chunk; never blocks on a client. Returns its sequence number.
    pub fn push(&self, tag: u8, payload: &[u8]) -> u64 {
        let mut c = Vec::with_capacity(payload.len() + 6);
        c.push(tag);
        write_varint(&mut c, payload.len() as u64);
        c.extend_from_slice(payload);
        self.0.push_stream(&c)
    }
}

fn drop_chunk(lost: u64) -> Vec<u8> {
    let mut p = Vec::new();
    write_varint(&mut p, lost);
    let mut c = vec![TAG_DROP];
    write_varint(&mut c, p.len() as u64);
    c.extend_from_slice(&p);
    c
}

/// Args: `from=<seq>` continues after that chunk; without it the connection's RESUME (if any)
/// decides, else the channel sees chunks from now on.
pub fn service(ring: Arc<Ring>) -> Factory {
    Arc::new(move |conn, open| {
        let from = open.args.split_whitespace().find_map(|a| a.strip_prefix("from=")?.parse::<u64>().ok()).or_else(|| conn.resume().and_then(|r| r.seqs.iter().find(|s| s.0 == "stream").map(|s| s.1)));
        let cur = match from {
            Some(seq) => ring.after(Kind::Stream, seq),
            None => ring.live(Kind::Stream),
        };
        let ring = ring.clone();
        let hooks = conn.hooks();
        std::thread::Builder::new()
            .name("makepad-remote-stream".into())
            .spawn(move || {
                (hooks.enter)();
                serve_feed(&ring, Kind::Stream, open.channel, cur, drop_chunk);
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (crate::conn::CLOSE_REFUSED, format!("cannot start the stream thread: {e}")))
    })
}
