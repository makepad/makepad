//! Frame work accounting and reusable scatter geometry.
use super::*;

const WINDOW: usize = 120;
const PEAK_WINDOW: usize = 600;
/// Leave 20% of GI's own GPU share for cost variation and asynchronous feedback.
const GPU_BUDGET_AIM_FRACTION: f64 = 0.8;

pub(super) struct StageTimes {
    samples: [(f64, usize); WINDOW],
    next: usize,
    len: usize,
}

impl Default for StageTimes {
    fn default() -> Self { Self { samples: [(0.0, 0); WINDOW], next: 0, len: 0 } }
}

impl StageTimes {
    pub fn record(&mut self, ms: f64, rows: usize) {
        if !ms.is_finite() || ms < 0.0 { return; }
        self.samples[self.next] = (ms, rows);
        self.next = (self.next + 1) % WINDOW;
        self.len = (self.len + 1).min(WINDOW);
    }

    /// Last 120 completed pass measurements, including every completion
    /// when several arrive together. Missing timings are not zero-cost frames.
    pub fn stats(&self) -> (f64, f64, Option<f64>) {
        if self.len == 0 { return (0.0, 0.0, None); }
        let (mut sum, mut peak, mut timed, mut rows) = (0.0, 0.0_f64, 0.0, 0);
        for &(ms, n) in &self.samples[..self.len] {
            sum += ms;
            peak = peak.max(ms);
            if n > 0 { timed += ms; rows += n; }
        }
        (sum / self.len as f64, peak, (rows > 0).then(|| timed / rows as f64))
    }
}

pub(super) struct FrameTimes {
    times: StageTimes,
    peaks: [f64; PEAK_WINDOW],
    next: usize,
}

impl Default for FrameTimes {
    fn default() -> Self { Self { times: StageTimes::default(), peaks: [0.0; PEAK_WINDOW], next: 0 } }
}

impl FrameTimes {
    /// The watcher publishes all four passes together at frame completion,
    /// oldest first. Sum each completion, not the passes' independent maxima.
    /// Unequal streams cannot be paired without platform frame tags: retain
    /// their pass diagnostics but do not invent a partial/zero-cost frame.
    pub fn push(&mut self, passes: [&[f32]; 4], rows: usize) -> bool {
        let count = passes[0].len();
        if passes.iter().any(|p| p.len() != count) { return false; }
        let mut recorded = false;
        for i in 0..count {
            if passes.iter().any(|p| !p[i].is_finite() || p[i] < 0.0) { continue; }
            let sum = passes.iter().map(|p| p[i] as f64).sum();
            self.times.record(sum, rows);
            self.peaks[self.next] = sum;
            self.next = (self.next + 1) % PEAK_WINDOW;
            recorded = true;
        }
        recorded
    }

    pub fn stats(&self) -> (f64, f64, Option<f64>) { self.times.stats() }

    /// Fixed storage and fixed work, including at a window wrap. No query or
    /// log is needed; valid spikes are neither clamped nor filtered out.
    pub fn max600(&self) -> f64 { self.peaks.iter().copied().fold(0.0, f64::max) }
}

#[derive(Default)]
pub(super) struct BatchController {
    /// Filtered throughput, probes/ms. Filtering the throughput makes the
    /// budget-derived cap itself slew slowly, including while warming up.
    rate: f64,
    last_run: Option<f64>,
    cadence: Option<f64>,
}

impl BatchController {
    pub fn aim(budget_ms: f32) -> f64 { budget_ms as f64 * GPU_BUDGET_AIM_FRACTION }

    pub fn new(initial: usize, budget_ms: f32) -> Self {
        Self { rate: initial as f64 / Self::aim(budget_ms), ..Self::default() }
    }

    pub fn cap(&mut self, budget_ms: f32, capacity: usize) -> usize {
        // Do not wind up a hidden rate above what the scheduler can issue.
        let aim = Self::aim(budget_ms);
        self.rate = self.rate.min(capacity as f64 / aim);
        (self.rate * aim).floor().max(1.0).min(capacity as f64) as usize
    }

    pub fn advance(&mut self, now: f64, ms_per_probe: Option<f64>, active: bool) {
        let previous = self.last_run.replace(now);
        if !active { self.cadence = None; return; }
        let Some(previous) = previous else { return; };
        let elapsed = now - previous;
        if !elapsed.is_finite() || elapsed <= 0.0 { return; }
        let cadence = self.cadence.replace(elapsed);
        // No work credit for an idle/skipped frame. Learn the new cadence,
        // but leave this batch unchanged even when a late sample arrives.
        let Some(cadence) = cadence else { return; };
        if elapsed > cadence * 1.5 { return; }
        let Some(cost) = ms_per_probe.filter(|c| c.is_finite() && *c > 0.0) else { return; };
        // At most 10% over any second of continuous rendering (integer
        // batches round down). Both directions use the same logarithmic rate.
        let factor = 1.1_f64.powf(elapsed);
        self.rate = (1.0 / cost).clamp(self.rate / factor, self.rate * factor);
    }
}

/// Three immutable records per batch row. Destination slots come from the
/// batch texture in the vertex shader, so changing the list uploads no
/// scatter instances. Rebuild only for capacity or shader-layout changes.
#[derive(Default)]
pub(super) struct ScatterCache {
    pub instances: Option<makepad_platform::retained_instances::RetainedInstances>,
}

impl ScatterCache {
    pub fn prepare(&mut self, draw: &mut DrawGiScatter, capacity: usize) -> usize {
        let slots = draw.quad.draw_vars.as_slice().len();
        if self.instances.as_ref().is_some_and(|r| r.slots() == slots && r.count() == capacity * 3) {
            return 0;
        }
        let mut data = Vec::with_capacity(capacity * 3 * slots);
        for row in 0..capacity {
            for (region, col, width) in [(0.0, 0.0, 8.0), (1.0, 64.0, 10.0), (2.0, 164.0, 1.0)] {
                draw.gi_tile = vec4(row as f32, col, width, region);
                draw.quad.rect_pos = vec2(0.0, 0.0);
                draw.quad.rect_size = vec2(width, width);
                data.extend_from_slice(draw.quad.draw_vars.as_slice());
            }
        }
        let bytes = data.len() * std::mem::size_of::<f32>();
        self.instances = Some(makepad_platform::retained_instances::RetainedInstances::new(slots, data.into()).expect("whole scatter instances"));
        bytes
    }
}
