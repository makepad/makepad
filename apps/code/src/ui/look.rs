//! Small drawing helpers for the look: inks from the palette as colours,
//! the live (breathing) ink, the working spinner and the timer format.

use super::theme::{vec, PALETTE as P};
use makepad_code_agent::event::Touch;
use makepad_widgets::*;

/// The palette as opaque colours, for drawing code.
pub mod ink {
    use super::*;

    pub const TEXT_HI: Vec4f = vec(P.text_hi, 1.0);
    pub const TEXT: Vec4f = vec(P.text, 1.0);
    pub const META: Vec4f = vec(P.meta, 1.0);
    pub const DIM: Vec4f = vec(P.disabled, 1.0);
    pub const AGENT: Vec4f = vec(P.primary, 1.0);
    pub const HUMAN: Vec4f = vec(P.human, 1.0);
    pub const OK: Vec4f = vec(P.ok, 1.0);
    pub const BAD: Vec4f = vec(P.error, 1.0);
    pub const CHANGE: Vec4f = vec(P.edited, 1.0);
    pub const LANE: Vec4f = vec(P.lane, 1.0);
}

/// The same colour at `a`.
pub const fn alpha(c: Vec4f, a: f32) -> Vec4f {
    vec4(c.x, c.y, c.z, a)
}

/// The ink of a file by what the agent did to it.
pub fn touch(how: Touch) -> Vec4f {
    match how {
        Touch::Edited => ink::CHANGE,
        Touch::Created => ink::OK,
        _ => vec(P.read, 1.0),
    }
}

/// The working spinner: a quadrant dot walking clockwise, eight steps a
/// second. Block elements are in the mono face, so it never falls back.
pub fn spinner(clock: f64) -> &'static str {
    const FRAMES: [&str; 4] = ["▘", "▝", "▗", "▖"];
    FRAMES[((clock * 8.0) as usize) % FRAMES.len()]
}

/// `mm:ss` under an hour, `h:mm:ss` beyond.
pub fn elapsed(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// How far a finished tool's resolve scan has run, 0..1; 1 when it has
/// none (still running, restored, or long done).
pub fn resolve_scan(started: Option<std::time::Instant>, took: Option<f32>) -> f32 {
    match (started, took) {
        (Some(s), Some(took)) => ((s.elapsed().as_secs_f32() - took) / RESOLVE_SCAN).clamp(0.0, 1.0),
        _ => 1.0,
    }
}

/// Seconds a tool row's resolve scan runs.
pub const RESOLVE_SCAN: f32 = 0.42;
