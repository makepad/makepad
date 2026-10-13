//! How long each real-time audio block takes against its deadline (the block's length at the
//! device rate). A block that takes longer is an overrun: the device plays what was not ready
//! (a glitch). Counted for every output callback (the `audio_output_box` seam) and any render
//! loop that meters itself; `audio_overruns()` is the total. With `MAKEPAD_AUDIO_DEADLINE=1` a
//! meter prints its totals every 5 s (a diagnostic: printing is off the hot path otherwise).

use makepad_relax::relax;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static OVERRUNS: AtomicU64 = AtomicU64::new(0);

/// Blocks that took longer than their deadline, in all meters since start.
pub fn audio_overruns() -> u64 {
    OVERRUNS.load(Ordering::Relaxed)
}

pub struct DeadlineMeter {
    name: &'static str,
    report: bool,
    blocks: u64,
    over: u64,
    max: f64,
    since: Instant,
    /// the current report window: blocks, overruns, busy, budget
    window: (u64, u64, Duration, Duration),
    /// the window's blocks by load: < 25%, < 50%, < 80%, < 100%, over
    hist: [u64; 5],
    /// the window's blocks that began more than 1.5 budgets after the previous one began (the
    /// thread ran late: the gap is the device's, not the block's), and the longest gap in budgets
    late: u64,
    gap: f64,
    last: Option<Instant>,
}

#[relax(O3, always)]
impl DeadlineMeter {
    pub fn new(name: &'static str) -> DeadlineMeter {
        let report = std::env::var("MAKEPAD_AUDIO_DEADLINE").is_ok_and(|v| v == "1");
        DeadlineMeter { name, report, blocks: 0, over: 0, max: 0.0, since: Instant::now(), window: (0, 0, Duration::ZERO, Duration::ZERO), hist: [0; 5], late: 0, gap: 0.0, last: None }
    }

    /// One block of `frames` at `sample_rate` took `took`.
    pub fn block(&mut self, frames: usize, sample_rate: f64, took: Duration) {
        if frames == 0 || sample_rate <= 0.0 {
            return;
        }
        let budget = Duration::from_secs_f64(frames as f64 / sample_rate);
        self.blocks += 1;
        let load = took.as_secs_f64() / budget.as_secs_f64();
        if load > self.max {
            self.max = load;
        }
        if took > budget {
            self.over += 1;
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        }
        let began = Instant::now() - took;
        if let Some(last) = self.last {
            let gap = began.duration_since(last).as_secs_f64() / budget.as_secs_f64();
            self.late += (gap > 1.5) as u64;
            self.gap = self.gap.max(gap);
        }
        self.last = Some(began);
        self.hist[if load < 0.25 { 0 } else if load < 0.5 { 1 } else if load < 0.8 { 2 } else if load <= 1.0 { 3 } else { 4 }] += 1;
        self.window.0 += 1;
        self.window.1 += (took > budget) as u64;
        self.window.2 += took;
        self.window.3 += budget;
        if self.report && self.since.elapsed() >= Duration::from_secs(5) {
            let (b, o, busy, budget) = self.window;
            eprintln!(
                "audio-deadline {}: last {} blocks: {} over budget, load {:.1}% (busy / budget), max {:.1}%, by load <25/50/80/100/over {:?}; {} began late (longest gap {:.2} budgets); since start {} blocks, {} over",
                self.name, b, o, 100.0 * busy.as_secs_f64() / budget.as_secs_f64().max(1e-9), 100.0 * self.max, self.hist, self.late, self.gap, self.blocks, self.over
            );
            self.hist = [0; 5];
            self.late = 0;
            self.gap = 0.0;
            self.since = Instant::now();
            self.max = 0.0;
            self.window = (0, 0, Duration::ZERO, Duration::ZERO);
        }
    }
}
