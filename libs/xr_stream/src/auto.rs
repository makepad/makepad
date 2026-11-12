//! Auto: the stream's seamless levers set from what the stream measures. Only levers that never
//! drop the picture: the bitrate cap, the anti-aliasing and the server's eye render scale (the
//! encoded size stays). The frame rate is never changed (a re-link); when the server cannot keep
//! up at the lowest settings, Auto suggests a lower one. One change at a time, then a dwell; a
//! lever lowered for a problem is not raised again for a minute, and a raise needs the whole
//! window clean with room to spare (no flapping). The user's manual value of a setting wins
//! (only settings left on Auto move).
//!
//! The signals, per 10 s window (two server windows, ten headset reports): the server's pictures
//! built per second against the display rate and its GPU time (render + pack) against the frame
//! budget; the headset's repeats by cause (late: the link or the decode; server skipped: the
//! server's build; queue cap), pictures shown; the link's round trip against its idle one.
//!
//! The levers' values and what Auto did are here on every target (the panel shows them); the
//! controller that steps them (`control`) is where a stream is served, not on the web.

use crate::settings_types::StreamAa;

#[cfg(not(target_arch = "wasm32"))]
mod control;

/// Which lever a setting left on Auto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lever {
    Bitrate,
    Aa,
    RenderScale,
}

/// The levers' values Auto holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoValues {
    pub kbps: u32,
    pub aa: StreamAa,
    pub render_scale_pct: u32,
}

impl Default for AutoValues {
    fn default() -> Self {
        AutoValues { kbps: 80_000, aa: crate::settings_types::STREAM_AA_DEFAULT, render_scale_pct: 100 }
    }
}

/// The controller (the caption's XR control steps it once a second).
#[derive(Default)]
pub struct XrAuto {
    pub values: AutoValues,
    /// What Auto did last and why (for the panel); empty until it changed something.
    pub note: String,
    /// A suggestion Auto does not act on itself (a lower frame rate).
    pub suggestion: String,
    #[cfg(not(target_arch = "wasm32"))]
    pace: control::Pace,
}
