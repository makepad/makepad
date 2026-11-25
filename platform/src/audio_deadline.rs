//! How long each real-time audio block takes against its deadline (the block's length at the
//! device rate). A block that takes longer is an overrun: the device plays what was not ready
//! (a glitch). Counted for every output callback (the `audio_output_box` seam) and any render
//! loop that meters itself; `audio_overruns()` is the total. With `MAKEPAD_AUDIO_DEADLINE=1` a
//! meter publishes a snapshot every 5 s. A single background thread reads and prints the
//! snapshots, so the audio callback never formats or writes a report.
//! Times are seconds of [`crate::monotonic_seconds`]: the web has no `Instant`.

use relax_attr::relax;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

static OVERRUNS: AtomicU64 = AtomicU64::new(0);

/// Blocks that took longer than their deadline, in all meters since start.
pub fn audio_overruns() -> u64 {
    OVERRUNS.load(Ordering::Relaxed)
}

pub struct DeadlineMeter {
    report: Option<Arc<ReportSlot>>,
    blocks: u64,
    over: u64,
    max: f64,
    since: f64,
    /// the current report window: blocks, overruns, busy and budget seconds
    window: (u64, u64, f64, f64),
    /// the window's blocks by load: < 25%, < 50%, < 80%, < 100%, over
    hist: [u64; 5],
    /// the window's blocks that began more than 1.5 budgets after the previous one began (the
    /// thread ran late: the gap is the device's, not the block's), and the longest gap in budgets
    late: u64,
    gap: f64,
    last: Option<f64>,
}

#[relax(O3, always)]
impl DeadlineMeter {
    pub fn new(name: &'static str) -> DeadlineMeter {
        let report = std::env::var("MAKEPAD_AUDIO_DEADLINE").is_ok_and(|v| v == "1")
            .then(|| register_report(name));
        Self::with_report(report, crate::monotonic_seconds())
    }

    fn with_report(report: Option<Arc<ReportSlot>>, since: f64) -> DeadlineMeter {
        DeadlineMeter { report, blocks: 0, over: 0, max: 0.0, since, window: (0, 0, 0.0, 0.0), hist: [0; 5], late: 0, gap: 0.0, last: None }
    }

    /// One block of `frames` at `sample_rate` began at `began` and took `took` (seconds).
    pub fn block(&mut self, frames: usize, sample_rate: f64, began: f64, took: f64) {
        if frames == 0 || sample_rate <= 0.0 {
            return;
        }
        let budget = frames as f64 / sample_rate;
        self.blocks += 1;
        let load = took / budget;
        if load > self.max {
            self.max = load;
        }
        if took > budget {
            self.over += 1;
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(last) = self.last {
            let gap = (began - last) / budget;
            self.late += (gap > 1.5) as u64;
            self.gap = self.gap.max(gap);
        }
        self.last = Some(began);
        self.hist[if load < 0.25 { 0 } else if load < 0.5 { 1 } else if load < 0.8 { 2 } else if load <= 1.0 { 3 } else { 4 }] += 1;
        self.window.0 += 1;
        self.window.1 += (took > budget) as u64;
        self.window.2 += took;
        self.window.3 += budget;
        let now = began + took;
        let Some(slot) = &self.report else { return };
        if now - self.since >= 5.0 {
            slot.publish(self);
            self.hist = [0; 5];
            self.late = 0;
            self.gap = 0.0;
            self.since = now;
            self.max = 0.0;
            self.window = (0, 0, 0.0, 0.0);
        }
    }
}

#[derive(Default)]
struct ReportSlot {
    name: &'static str,
    window_blocks: AtomicU64,
    window_over: AtomicU64,
    busy: AtomicU64,
    budget: AtomicU64,
    max: AtomicU64,
    hist: [AtomicU64; 5],
    late: AtomicU64,
    gap: AtomicU64,
    total_over: AtomicU64,
    // The single writer marks the slot with zero during a write and the strictly
    // increasing lifetime block count when done. All snapshot accesses share one order.
    total_blocks: AtomicU64,
}

impl ReportSlot {
    #[relax(O3, always)]
    fn publish(&self, meter: &DeadlineMeter) {
        use Ordering::SeqCst;
        let (blocks, over, busy, budget) = meter.window;
        self.total_blocks.store(0, SeqCst);
        self.window_blocks.store(blocks, SeqCst);
        self.window_over.store(over, SeqCst);
        self.busy.store(busy.to_bits(), SeqCst);
        self.budget.store(budget.to_bits(), SeqCst);
        self.max.store(meter.max.to_bits(), SeqCst);
        for (bucket, count) in self.hist.iter().zip(meter.hist) {
            bucket.store(count, SeqCst);
        }
        self.late.store(meter.late, SeqCst);
        self.gap.store(meter.gap.to_bits(), SeqCst);
        self.total_over.store(meter.over, SeqCst);
        self.total_blocks.store(meter.blocks, SeqCst);
    }

    fn read(&self, last_blocks: u64) -> Option<Report> {
        use Ordering::SeqCst;
        loop {
            let total_blocks = self.total_blocks.load(SeqCst);
            if total_blocks == 0 || total_blocks == last_blocks {
                return None;
            }
            let report = Report {
                name: self.name,
                window_blocks: self.window_blocks.load(SeqCst),
                window_over: self.window_over.load(SeqCst),
                busy: f64::from_bits(self.busy.load(SeqCst)),
                budget: f64::from_bits(self.budget.load(SeqCst)),
                max: f64::from_bits(self.max.load(SeqCst)),
                hist: self.hist.each_ref().map(|bucket| bucket.load(SeqCst)),
                late: self.late.load(SeqCst),
                gap: f64::from_bits(self.gap.load(SeqCst)),
                total_blocks,
                total_over: self.total_over.load(SeqCst),
            };
            if self.total_blocks.load(SeqCst) == total_blocks {
                return Some(report);
            }
        }
    }
}

struct Report {
    name: &'static str,
    window_blocks: u64,
    window_over: u64,
    busy: f64,
    budget: f64,
    max: f64,
    hist: [u64; 5],
    late: u64,
    gap: f64,
    total_blocks: u64,
    total_over: u64,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,
            "audio-deadline {}: last {} blocks: {} over budget, load {:.1}% (busy / budget), max {:.1}%, by load <25/50/80/100/over {:?}; {} began late (longest gap {:.2} budgets); since start {} blocks, {} over",
            self.name, self.window_blocks, self.window_over, 100.0 * self.busy / self.budget.max(1e-9),
            100.0 * self.max, self.hist, self.late, self.gap, self.total_blocks, self.total_over
        )
    }
}

fn register_report(name: &'static str) -> Arc<ReportSlot> {
    static NEW_SLOTS: Mutex<Vec<Arc<ReportSlot>>> = Mutex::new(Vec::new());
    static START: Once = Once::new();

    let slot = Arc::new(ReportSlot { name, ..Default::default() });
    NEW_SLOTS.lock().unwrap().push(slot.clone());
    START.call_once(|| {
        let spawned = std::thread::Builder::new().name("audio-deadline".into()).spawn(|| {
            let mut slots = Vec::new();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let new_slots = std::mem::take(&mut *NEW_SLOTS.lock().unwrap());
                slots.extend(new_slots.into_iter().map(|slot| (slot, 0)));
                slots.retain_mut(|(slot, last_blocks)| {
                    // Check before reading so a meter dropped during the read gets one
                    // more pass, and a meter already dropped gets its final window printed.
                    let meter_alive = Arc::strong_count(slot) > 1;
                    if let Some(report) = slot.read(*last_blocks) {
                        *last_blocks = report.total_blocks;
                        eprintln!("{report}");
                    }
                    meter_alive
                });
            }
        });
        if let Err(error) = spawned {
            eprintln!("audio-deadline: reports are off, the logger thread did not start: {error}");
        }
    });
    slot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_five_second_windows() {
        let slot = Arc::new(ReportSlot { name: "test", ..Default::default() });
        let mut meter = DeadlineMeter::with_report(Some(slot.clone()), 0.0);
        for (began, took) in [(0.0, 0.125), (1.0, 0.25), (2.0, 0.5), (3.0, 1.0)] {
            meter.block(8, 8.0, began, took);
        }
        assert!(slot.read(0).is_none(), "published before five seconds");
        meter.block(8, 8.0, 4.0, 1.25);
        let first = slot.read(0).expect("first window");
        assert_eq!((first.busy, first.budget), (3.125, 5.0));
        assert_eq!(first.to_string(), "audio-deadline test: last 5 blocks: 1 over budget, load 62.5% (busy / budget), max 125.0%, by load <25/50/80/100/over [1, 1, 1, 1, 1]; 0 began late (longest gap 1.00 budgets); since start 5 blocks, 1 over");

        // The gap from the previous window's last block to this one's first is late.
        for began in [6.0, 7.0, 8.0, 9.0] {
            meter.block(8, 8.0, began, 0.25);
        }
        assert!(slot.read(first.total_blocks).is_none(), "repeated or early window");
        meter.block(8, 8.0, 10.0, 0.25);
        let second = slot.read(first.total_blocks).expect("window at exactly five seconds");
        assert_eq!((second.busy, second.budget), (1.25, 5.0));
        assert_eq!(second.to_string(), "audio-deadline test: last 5 blocks: 0 over budget, load 25.0% (busy / budget), max 25.0%, by load <25/50/80/100/over [0, 5, 0, 0, 0]; 1 began late (longest gap 2.00 budgets); since start 10 blocks, 1 over");
        assert!(slot.read(second.total_blocks).is_none(), "repeated final window");
    }
}
