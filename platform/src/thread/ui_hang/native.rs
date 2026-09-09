use super::{phase_name, CURRENT};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{atomic::{AtomicBool, AtomicU64, Ordering}, mpsc::{sync_channel, Receiver, SyncSender}, Arc},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod backend;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod backend;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod backend;

#[path = "demangle.rs"]
mod demangle;

const THRESHOLD: Duration = Duration::from_millis(250);
const INTERVAL: Duration = Duration::from_millis(100);

fn configured_threshold() -> Duration {
    std::env::var("MAKEPAD_UI_HANG_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(THRESHOLD)
}

pub(super) struct State {
    origin: Instant,
    threshold: Duration,
    start: AtomicU64,
    pub(super) phase: AtomicU64,
    stop: AtomicBool,
    completions: SyncSender<Completion>,
    lost: AtomicU64,
}
struct Completion { start: u64, nanos: u64, phase: u64 }

impl State {
    pub(super) fn begin(&self) {
        self.start.store(self.origin.elapsed().as_nanos() as u64 + 1, Ordering::Release);
    }
    pub(super) fn end(&self) {
        let start = self.start.swap(0, Ordering::AcqRel);
        let nanos = (self.origin.elapsed().as_nanos() as u64 + 1).saturating_sub(start);
        if nanos >= self.threshold.as_nanos() as u64 {
            let report = Completion { start, nanos, phase: self.phase.load(Ordering::Relaxed) };
            if self.completions.try_send(report).is_err() {
                self.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
struct Registration(Arc<State>);
impl Drop for Registration {
    fn drop(&mut self) {
        CURRENT.with(|p| p.set(std::ptr::null()));
        self.0.stop.store(true, Ordering::Release);
    }
}
thread_local! { static REGISTRATION: RefCell<Option<Registration>> = const { RefCell::new(None) }; }

pub(super) fn initialize() {
    REGISTRATION.with(|registration| {
        if registration.borrow().is_some() { return; }
        let Some(target) = backend::Target::current() else {
            crate::log!("[ui-hang] stack sampler unavailable for this UI thread");
            return;
        };
        let (tx, rx) = sync_channel(16);
        let state = Arc::new(State {
            threshold: configured_threshold(),
            origin: Instant::now(), start: AtomicU64::new(0), phase: AtomicU64::new(0),
            stop: AtomicBool::new(false), completions: tx, lost: AtomicU64::new(0),
        });
        let worker = state.clone();
        match std::thread::Builder::new().name("makepad-ui-watchdog".into()).spawn(move || {
            watch(worker, rx, target, |line| crate::log!("{}", line));
        }) {
            Ok(_) => {
                CURRENT.with(|p| p.set(Arc::as_ptr(&state)));
                *registration.borrow_mut() = Some(Registration(state));
            }
            Err(error) => crate::log!("[ui-hang] cannot start watchdog: {}", error),
        }
    });
}

struct Sample { phase: u64, frames: Vec<usize>, count: usize }

fn watch(state: Arc<State>, rx: Receiver<Completion>, target: backend::Target, mut emit: impl FnMut(String)) {
    let mut samples: HashMap<u64, Vec<Sample>> = HashMap::new();
    loop {
        let tick = Instant::now();
        let start = state.start.load(Ordering::Acquire);
        let elapsed = (state.origin.elapsed().as_nanos() as u64 + 1).saturating_sub(start);
        if start != 0 && elapsed >= state.threshold.as_nanos() as u64 {
            let phase = state.phase.load(Ordering::Relaxed);
            let frames = target.sample();
            if state.start.load(Ordering::Acquire) == start {
                let batch = samples.entry(start).or_default();
                if let Some(sample) = batch.iter_mut().find(|s| s.phase == phase && s.frames == frames) {
                    sample.count += 1;
                } else if batch.len() < 32 {
                    batch.push(Sample { phase, frames, count: 1 });
                }
            }
        }
        while let Ok(done) = rx.try_recv() {
            let mut batch = samples.remove(&done.start).unwrap_or_default();
            batch.sort_by_key(|s| std::cmp::Reverse(s.count));
            let count: usize = batch.iter().map(|s| s.count).sum();
            let phase = batch.first().map_or(done.phase, |s| s.phase);
            let mut line = format!("[ui-hang] {:.0} ms · phase={} detail={} · samples={} · top frames: ",
                done.nanos as f64 / 1e6, phase_name(phase as u32), phase >> 32, count);
            if batch.is_empty() { line.push_str("<event ended before stack sample>"); }
            for (i, sample) in batch.iter().take(4).enumerate() {
                if i > 0 { line.push_str(" | "); }
                line.push_str(&format!("{}x {}: ", sample.count, phase_name(sample.phase as u32)));
                if sample.frames.is_empty() { line.push_str("<stack unavailable>"); }
                for (j, &pc) in sample.frames.iter().take(8).enumerate() {
                    if j > 0 { line.push_str(" ← "); }
                    line.push_str(&target.symbolize(pc));
                }
            }
            emit(line);
        }
        // Bounded even if completion pressure loses a report. Nothing wakes UI.
        if samples.len() > 16 {
            let active = state.start.load(Ordering::Acquire);
            samples.retain(|&key, _| key == active);
        }
        let lost = state.lost.swap(0, Ordering::Relaxed);
        if lost != 0 {
            emit(format!(
                "[ui-hang] dropped {lost} completion reports (queue full)"
            ));
        }
        if state.stop.load(Ordering::Acquire) {
            break;
        }
        std::thread::park_timeout(
            INTERVAL
                .min(state.threshold / 4)
                .max(Duration::from_millis(1))
                .saturating_sub(tick.elapsed()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watchdog_reports_one_sleep_with_phase_and_stack() {
        let target = backend::Target::current().expect("native stack target");
        let (tx, rx) = sync_channel(16);
        let state = Arc::new(State {
            threshold: THRESHOLD,
            origin: Instant::now(), start: AtomicU64::new(0),
            phase: AtomicU64::new(super::super::UiPhase::CodePageInstall as u64),
            stop: AtomicBool::new(false), completions: tx, lost: AtomicU64::new(0),
        });
        let (reports, received) = sync_channel(16);
        let worker = state.clone();
        let thread = std::thread::spawn(move || watch(worker, rx, target, |line| {
            eprintln!("{line}");
            reports.try_send(line).unwrap();
        }));
        state.begin();
        std::thread::sleep(Duration::from_millis(400));
        state.end();
        // Only this deliberate test waits; production UI never does.
        let line = received.recv_timeout(Duration::from_secs(3)).unwrap();
        state.stop.store(true, Ordering::Release);
        thread.thread().unpark();
        thread.join().unwrap();
        let milliseconds: f64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!(milliseconds >= 400.0, "{line}");
        assert!(line.contains("phase=code-page-install"), "{line}");
        assert!(!line.contains("samples=0"), "{line}");
        assert!(!line.contains("<stack unavailable>"), "{line}");
        assert!(received.try_recv().is_err(), "one report per event");
    }
}
