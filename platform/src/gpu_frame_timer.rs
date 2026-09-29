//! Whole-frame GPU time, with a per-pass breakdown that sums to it.
//!
//! A window frame is several GPU submissions: its offscreen passes (shadow
//! maps, a 3D scene, post chains) and the window pass that presents. Each
//! pass's own start..end span is not its cost: on Metal a command buffer's
//! `GPUStartTime` is when the GPU picked it up, and later buffers start
//! while earlier ones still run (or wait on them), so per-pass spans overlap
//! and a 1x1 pass can read 5 ms. Summing them over-counts; the frame's
//! first-start..last-end span counts the idle gaps between submissions.
//!
//! This module takes every pass span of one frame (backends call
//! [`record_pass`] from their completion paths, [`finish_frame`] once the
//! presenting pass is done) and reports:
//! * `busy_ms`: the union of the spans, the time the GPU spent on the frame.
//!   Spans are clipped to the end of the previous frame: a frame's first
//!   pass can be picked up while the last frame still runs, and that wait
//!   is the last frame's time, not this one's;
//! * `passes`: that union partitioned by pass. Sorted by start, a pass owns
//!   the part of its span that ends after every earlier span: the time it
//!   extended the frame. The parts sum to `busy_ms` exactly.
//!
//! Passes of a repaint that never presented are folded into the next frame
//! that did, named `unpresented <pass>`: the GPU spent that time too.
//!
//! Off until something calls [`set_enabled`]; disabled backends do no work.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// One finished frame.
#[derive(Clone, Debug, Default)]
pub struct GpuFrameTiming {
    /// GPU busy time of the frame (union of its pass spans), ms.
    pub busy_ms: f32,
    /// First pass start to last pass end, ms (includes idle gaps).
    pub span_ms: f32,
    /// (pass name, ms) in first-start order; the ms sum to `busy_ms`.
    /// Passes that share a name are merged.
    pub passes: Vec<(String, f32)>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);

struct State {
    open: Vec<(u64, String, f64, f64)>,
    done: VecDeque<GpuFrameTiming>,
    /// The last finished frame's end (frames finish in queue order).
    last_end: f64,
}

static STATE: Mutex<State> = Mutex::new(State { open: Vec::new(), done: VecDeque::new(), last_end: f64::NEG_INFINITY });

/// Frames kept for a consumer that takes them late; older ones drop.
const KEEP_FRAMES: usize = 240;
/// Spans of frames that never finish (a pass outside any window) drop
/// past this many.
const KEEP_SPANS: usize = 4096;

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        if let Ok(mut s) = STATE.lock() {
            s.open.clear();
            s.done.clear();
            s.last_end = f64::NEG_INFINITY;
        }
    }
}

#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// One pass of frame `frame` ran on the GPU from `start` to `end` seconds
/// (any clock, the same for every pass of a frame). Any thread.
pub fn record_pass(frame: u64, name: &str, start: f64, end: f64) {
    if !enabled() || !(start.is_finite() && end.is_finite() && start > 0.0 && end >= start) {
        return;
    }
    if let Ok(mut s) = STATE.lock() {
        if s.open.len() >= KEEP_SPANS {
            s.open.clear();
        }
        s.open.push((frame, name.to_string(), start, end));
    }
}

/// Every pass of `frame` has been recorded: fold them into one timing.
pub fn finish_frame(frame: u64) {
    if !enabled() {
        return;
    }
    let Ok(mut s) = STATE.lock() else { return };
    let mut spans = Vec::new();
    s.open.retain(|(f, name, a, b)| {
        if *f == frame {
            spans.push((name.clone(), *a, *b));
            false
        } else {
            true
        }
    });
    if spans.is_empty() {
        return;
    }
    // Passes of repaints that never presented (a window that skipped its
    // present after its offscreen passes were encoded) finished before this
    // frame did: the queue runs in order, so no later frame will claim
    // them. Their GPU time is real; it is shown as "unpresented <pass>".
    let end = spans.iter().fold(f64::NEG_INFINITY, |e, sp| e.max(sp.2));
    s.open.retain(|(_, name, a, b)| {
        if *b <= end {
            spans.push((format!("unpresented {name}"), *a, *b));
            false
        } else {
            true
        }
    });
    let floor = s.last_end;
    s.last_end = spans.iter().fold(floor, |e, sp| e.max(sp.2));
    let timing = breakdown(spans, floor);
    if s.done.len() >= KEEP_FRAMES {
        s.done.pop_front();
    }
    s.done.push_back(timing);
}

/// The frames finished since the last call, oldest first.
pub fn take_frames() -> Vec<GpuFrameTiming> {
    STATE.lock().map(|mut s| s.done.drain(..).collect()).unwrap_or_default()
}

/// The partition described in the module docs. `spans`: (name, start, end)
/// in seconds; `floor`: the previous frame's end.
pub fn breakdown(mut spans: Vec<(String, f64, f64)>, floor: f64) -> GpuFrameTiming {
    for s in spans.iter_mut() {
        s.1 = s.1.max(floor).min(s.2);
    }
    spans.sort_by(|a, b| a.1.total_cmp(&b.1));
    let first = spans.first().map_or(0.0, |s| s.1);
    let mut covered = f64::NEG_INFINITY;
    let mut busy = 0.0;
    let mut passes: Vec<(String, f32)> = Vec::new();
    for (name, start, end) in spans {
        let own = (end - start.max(covered)).max(0.0);
        covered = covered.max(end);
        busy += own;
        let ms = (own * 1000.0) as f32;
        match passes.iter_mut().find(|p| p.0 == name) {
            Some(p) => p.1 += ms,
            None => passes.push((name, ms)),
        }
    }
    GpuFrameTiming {
        busy_ms: (busy * 1000.0) as f32,
        span_ms: ((covered - first).max(0.0) * 1000.0) as f32,
        passes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t: &GpuFrameTiming, name: &str) -> f32 {
        t.passes.iter().find(|p| p.0 == name).map_or(-1.0, |p| p.1)
    }

    #[test]
    fn overlapping_spans_partition_the_union() {
        // shadow 0..2 ms, scene picked up at 1 ms but done at 6, a tiny
        // pass "starting" at 1.5 (waiting on the scene) that ends at 6.2,
        // and the window pass 7..8 after a 0.8 ms idle gap.
        let t = breakdown(vec![
            ("window".into(), 0.007, 0.008),
            ("scene".into(), 0.001, 0.006),
            ("shadow".into(), 0.0, 0.002),
            ("adapt".into(), 0.0015, 0.0062),
        ], f64::NEG_INFINITY);
        assert!((ms(&t, "shadow") - 2.0).abs() < 1e-3);
        assert!((ms(&t, "scene") - 4.0).abs() < 1e-3);
        assert!((ms(&t, "adapt") - 0.2).abs() < 1e-3);
        assert!((ms(&t, "window") - 1.0).abs() < 1e-3);
        assert!((t.busy_ms - 7.2).abs() < 1e-3);
        assert!((t.span_ms - 8.0).abs() < 1e-3);
        let sum: f32 = t.passes.iter().map(|p| p.1).sum();
        assert!((sum - t.busy_ms).abs() < 1e-3);
        assert_eq!(t.passes[0].0, "shadow");
    }

    #[test]
    fn same_named_passes_merge() {
        let t = breakdown(vec![
            ("bloom".into(), 0.0, 0.001),
            ("bloom".into(), 0.001, 0.0015),
            ("scene".into(), 0.0015, 0.003),
        ], f64::NEG_INFINITY);
        assert_eq!(t.passes.len(), 2);
        assert!((ms(&t, "bloom") - 1.5).abs() < 1e-3);
    }

    #[test]
    fn a_pass_picked_up_during_the_last_frame_starts_at_its_end() {
        // The shadow pass "started" at 0 while the previous frame ran to 3 ms.
        let t = breakdown(vec![("shadow".into(), 0.0, 0.004), ("scene".into(), 0.001, 0.006)], 0.003);
        assert!((ms(&t, "shadow") - 1.0).abs() < 1e-3);
        assert!((ms(&t, "scene") - 2.0).abs() < 1e-3);
        assert!((t.busy_ms - 3.0).abs() < 1e-3);
        assert!((t.span_ms - 3.0).abs() < 1e-3);
    }
}
