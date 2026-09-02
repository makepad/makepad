#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

// Geodata's filesystem and network cache code is native-only, so std clocks are safe here.
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
