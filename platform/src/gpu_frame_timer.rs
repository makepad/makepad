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
    // Only the prefix is live; completed slots retain their name buffers.
    open: Vec<(u64, String, f64, f64)>,
    open_len: usize,
    spans: Vec<usize>,
    done: Vec<CompletedFrame>,
    done_head: usize,
    done_len: usize,
    /// The last finished frame's end (frames finish in queue order).
    last_end: f64,
}

struct CompletedFrame {
    timing: GpuFrameTiming,
    // The inactive suffix retains name buffers when pass counts vary.
    pass_count: usize,
}

static STATE: Mutex<State> = Mutex::new(State {
    open: Vec::new(),
    open_len: 0,
    spans: Vec::new(),
    done: Vec::new(),
    done_head: 0,
    done_len: 0,
    last_end: f64::NEG_INFINITY,
});

/// Frames kept for a consumer that takes them late; older ones drop.
const KEEP_FRAMES: usize = 240;
/// Spans of frames that never finish (a pass outside any window) drop
/// past this many.
const KEEP_SPANS: usize = 4096;

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        if let Ok(mut s) = STATE.lock() {
            s.open_len = 0;
            s.done_head = 0;
            s.done_len = 0;
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
        if s.open_len >= KEEP_SPANS {
            s.open_len = 0;
        }
        let index = s.open_len;
        if index == s.open.len() {
            s.open.push((frame, String::new(), start, end));
        }
        let span = &mut s.open[index];
        span.0 = frame;
        span.1.clear();
        span.1.push_str(name);
        span.2 = start;
        span.3 = end;
        s.open_len += 1;
    }
}

/// Every pass of `frame` has been recorded: fold them into one timing.
pub fn finish_frame(frame: u64) {
    if !enabled() {
        return;
    }
    let Ok(mut s) = STATE.lock() else { return };
    let State {
        open, open_len, spans, done, done_head, done_len, last_end,
    } = &mut *s;
    let end = open[..*open_len].iter()
        .filter(|sp| sp.0 == frame)
        .map(|sp| sp.3)
        .reduce(f64::max);
    let Some(end) = end else { return };
    // Older unpresented passes belong to this frame too. Keep indices so
    // the names stay in their reusable slots while the spans are sorted.
    spans.clear();
    spans.extend((0..*open_len).filter(|&i| open[i].0 == frame || open[i].3 <= end));
    let floor = *last_end;
    for &i in spans.iter() {
        open[i].2 = open[i].2.max(floor).min(open[i].3);
    }
    spans.sort_unstable_by(|&a, &b| {
        open[a].2.total_cmp(&open[b].2)
            // Match the stable ordering: this frame first, then stale spans.
            .then_with(|| (open[a].0 != frame).cmp(&(open[b].0 != frame)))
            .then_with(|| a.cmp(&b))
    });
    let index = (*done_head + *done_len) % KEEP_FRAMES;
    if index == done.len() {
        done.push(CompletedFrame {
            timing: GpuFrameTiming::default(),
            pass_count: 0,
        });
    }
    done[index].pass_count = breakdown_into(spans.iter().map(|&i| {
        let sp = &open[i];
        (sp.1.as_str(), sp.2, sp.3, sp.0 != frame)
    }), &mut done[index].timing);
    *last_end = spans.iter().fold(floor, |e, &i| e.max(open[i].3));
    if *done_len == KEEP_FRAMES {
        *done_head = (*done_head + 1) % KEEP_FRAMES;
    } else {
        *done_len += 1;
    }
    // Compact the live prefix without dropping the completed name buffers.
    let mut kept = 0;
    for i in 0..*open_len {
        if open[i].0 != frame && open[i].3 > end {
            open.swap(kept, i);
            kept += 1;
        }
    }
    *open_len = kept;
}

/// The frames finished since the last call, oldest first.
/// Owned output is allocated here; recording and finishing reuse their buffers.
pub fn take_frames() -> Vec<GpuFrameTiming> {
    STATE.lock().map(|mut s| {
        let frames = (0..s.done_len)
            .map(|i| {
                let frame = &s.done[(s.done_head + i) % KEEP_FRAMES];
                GpuFrameTiming {
                    busy_ms: frame.timing.busy_ms,
                    span_ms: frame.timing.span_ms,
                    passes: frame.timing.passes[..frame.pass_count].to_vec(),
                }
            })
            .collect();
        s.done_head = 0;
        s.done_len = 0;
        frames
    }).unwrap_or_default()
}

/// The partition described in the module docs. `spans`: (name, start, end)
/// in seconds; `floor`: the previous frame's end.
pub fn breakdown(mut spans: Vec<(String, f64, f64)>, floor: f64) -> GpuFrameTiming {
    for s in spans.iter_mut() {
        s.1 = s.1.max(floor).min(s.2);
    }
    spans.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut timing = GpuFrameTiming::default();
    breakdown_into(
        spans.iter().map(|(name, start, end)| (name.as_str(), *start, *end, false)),
        &mut timing,
    );
    timing
}

fn breakdown_into<'a>(
    spans: impl Iterator<Item = (&'a str, f64, f64, bool)>,
    timing: &mut GpuFrameTiming,
) -> usize {
    let mut first = None;
    let mut covered = f64::NEG_INFINITY;
    let mut busy = 0.0;
    let mut pass_count = 0;
    for (name, start, end, unpresented) in spans {
        first.get_or_insert(start);
        let own = (end - start.max(covered)).max(0.0);
        covered = covered.max(end);
        busy += own;
        let ms = (own * 1000.0) as f32;
        let prefix = if unpresented { "unpresented " } else { "" };
        match timing.passes[..pass_count].iter_mut().find(|p| {
            p.0.strip_prefix(prefix) == Some(name)
        }) {
            Some(p) => p.1 += ms,
            None => {
                if pass_count == timing.passes.len() {
                    timing.passes.push((String::new(), 0.0));
                }
                let pass = &mut timing.passes[pass_count];
                pass.0.clear();
                pass.0.push_str(prefix);
                pass.0.push_str(name);
                pass.1 = ms;
                pass_count += 1;
            }
        }
    }
    timing.busy_ms = (busy * 1000.0) as f32;
    timing.span_ms = ((covered - first.unwrap_or(0.0)).max(0.0) * 1000.0) as f32;
    pass_count
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
