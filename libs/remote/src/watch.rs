//! The `watch` service: push-only event lines (panic, stopped, patch, ...). The host emits through
//! a [`WatchSink`]; the endpoint's ring keeps the last 256 so a reconnecting client gets the ones
//! it missed (RESUME), and a gap is reported as `{"event":"drop","lost":N}`.
//! Each DATA message is `seq u64 LE | one JSON line`.

use crate::endpoint::Factory;
use crate::ring::{serve_feed, Kind, Ring};
use std::sync::Arc;

#[derive(Clone)]
pub struct WatchSink(pub(crate) Arc<Ring>);

impl WatchSink {
    /// Publishes one event line (no newline); never blocks on a client.
    pub fn emit(&self, line: &str) -> u64 {
        self.0.push_watch(line.as_bytes())
    }
}

fn drop_line(lost: u64) -> Vec<u8> {
    format!("{{\"event\":\"drop\",\"lost\":{lost}}}").into_bytes()
}

/// Args: `from=<seq>` continues after that event; without it the connection's RESUME (if any)
/// decides, else the channel sees events from now on.
pub fn service(ring: Arc<Ring>) -> Factory {
    service_with(ring, || {}, || {})
}

/// As [`service`], calling `opened` when a channel starts following and `closed` when it ends
/// (a host counts its watchers with them).
pub fn service_with(ring: Arc<Ring>, opened: fn(), closed: fn()) -> Factory {
    Arc::new(move |conn, open| {
        let from = open.args.split_whitespace().find_map(|a| a.strip_prefix("from=")?.parse::<u64>().ok()).or_else(|| conn.resume().and_then(|r| r.seqs.iter().find(|s| s.0 == "watch").map(|s| s.1)));
        let cur = match from {
            Some(seq) => ring.after(Kind::Watch, seq),
            None => ring.live(Kind::Watch),
        };
        let ring = ring.clone();
        let hooks = conn.hooks();
        std::thread::Builder::new()
            .name("makepad-remote-watch".into())
            .spawn(move || {
                (hooks.enter)();
                opened();
                serve_feed(&ring, Kind::Watch, open.channel, cur, drop_line);
                closed();
                (hooks.exit)();
            })
            .map(|_| ())
            .map_err(|e| (crate::conn::CLOSE_REFUSED, format!("cannot start the watch thread: {e}")))
    })
}
