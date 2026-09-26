//! Where an install's time goes: per component (Build tools, Windows SDK,
//! Rust, CUDA) its wall time and, per phase, the time threads spent in it,
//! the window from its first start to its last end, and the bytes it moved.
//! Written to builder.log (and the CLI's output) after each install, so a
//! slow download is told apart from slow unpacking.
use std::{
    cell::RefCell,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Network transfer, manifests included.
    Fetch,
    /// Hashing downloads and cached files.
    Verify,
    /// Reading cached archives from disk.
    Read,
    /// Inflate, LZX, tar/zip/CAB/MSI parsing.
    Decompress,
    /// Creating and writing installed files (and the download cache).
    Write,
    /// Running the new compiler, waiting for security software.
    Check,
}
const PHASES: [(Phase, &str); 6] = [
    (Phase::Fetch, "fetch"),
    (Phase::Verify, "verify"),
    (Phase::Read, "read"),
    (Phase::Decompress, "decompress"),
    (Phase::Write, "write"),
    (Phase::Check, "check"),
];

#[derive(Default)]
struct Row {
    busy: Duration,
    bytes: u64,
    count: u64,
    first: Option<Instant>,
    last: Option<Instant>,
}
struct Component {
    label: String,
    start: Instant,
    end: Option<Instant>,
    phases: [Row; 6],
}
struct Run {
    start: Instant,
    components: Vec<Component>,
    notes: Vec<String>,
}
static RUN: Mutex<Option<Run>> = Mutex::new(None);

thread_local! {
    static CURRENT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Start counting a new install.
pub fn reset() {
    *lock() = Some(Run { start: Instant::now(), components: Vec::new(), notes: Vec::new() });
}
fn lock() -> std::sync::MutexGuard<'static, Option<Run>> {
    RUN.lock().unwrap_or_else(|e| e.into_inner())
}
fn with_component(label: &str, f: impl FnOnce(&mut Component)) {
    let mut run = lock();
    let Some(run) = run.as_mut() else { return };
    let index = match run.components.iter().position(|c| c.label == label) {
        Some(index) => index,
        None => {
            run.components.push(Component { label: label.into(), start: Instant::now(), end: None, phases: Default::default() });
            run.components.len() - 1
        }
    };
    f(&mut run.components[index]);
}
/// An extra line for the report (download hosts, pool size).
pub fn note(line: String) {
    if let Some(run) = lock().as_mut() {
        run.notes.push(line);
    }
}

/// The component this thread works for, until the guard drops.
pub fn component(label: &str) -> ComponentGuard {
    with_component(label, |c| c.end = None);
    ComponentGuard(CURRENT.with(|c| c.replace(label.into())))
}
pub struct ComponentGuard(String);
impl Drop for ComponentGuard {
    fn drop(&mut self) {
        let label = CURRENT.with(|c| c.replace(std::mem::take(&mut self.0)));
        with_component(&label, |c| c.end = Some(Instant::now()));
    }
}
/// The component of this thread, for work handed to another thread.
pub fn current() -> String {
    CURRENT.with(|c| c.borrow().clone())
}
/// Work on another thread for `label`, without ending its wall time.
pub fn adopt(label: &str) -> AdoptGuard {
    AdoptGuard(CURRENT.with(|c| c.replace(label.into())))
}
pub struct AdoptGuard(String);
impl Drop for AdoptGuard {
    fn drop(&mut self) {
        CURRENT.with(|c| c.replace(std::mem::take(&mut self.0)));
    }
}

/// Time spent in `phase` until the span drops.
pub fn span(phase: Phase) -> Span {
    Span { phase, start: Instant::now(), bytes: 0 }
}
pub struct Span {
    phase: Phase,
    start: Instant,
    bytes: u64,
}
impl Span {
    pub fn bytes(&mut self, bytes: u64) {
        self.bytes += bytes;
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        let label = current();
        let label = if label.is_empty() { "other".to_owned() } else { label };
        let index = PHASES.iter().position(|(p, _)| *p == self.phase).unwrap_or(0);
        let (start, end, bytes) = (self.start, Instant::now(), self.bytes);
        with_component(&label, |c| {
            let row = &mut c.phases[index];
            row.busy += end - start;
            row.bytes += bytes;
            row.count += 1;
            row.first = Some(row.first.map_or(start, |f| f.min(start)));
            row.last = Some(row.last.map_or(end, |l| l.max(end)));
        });
    }
}

/// One line per component: wall time, then per phase the thread time
/// summed over threads, its window when threads overlapped in it, bytes.
pub fn report() -> Vec<String> {
    let run = lock();
    let Some(run) = run.as_ref() else { return Vec::new() };
    let mut out = vec![format!("timing: install {:.1} s", run.start.elapsed().as_secs_f64())];
    for c in &run.components {
        let wall = c.end.unwrap_or_else(Instant::now).duration_since(c.start).as_secs_f64();
        let mut line = format!("timing: {:<12} wall {wall:6.1} s (from {:5.1} s)", c.label, c.start.duration_since(run.start).as_secs_f64());
        for ((_, name), row) in PHASES.iter().zip(&c.phases) {
            if row.count == 0 {
                continue;
            }
            let secs = row.busy.as_secs_f64();
            line += &format!(" | {name} {secs:.1} s");
            if let (Some(first), Some(last)) = (row.first, row.last) {
                let window = last.duration_since(first).as_secs_f64();
                if secs > window + 0.05 {
                    line += &format!(" in {window:.1} s");
                }
            }
            if row.bytes > 0 {
                line += &format!(" {:.0} MB", row.bytes as f64 / 1_048_576.0);
            }
            if *name == "write" {
                line += &format!(" {} files", row.count);
            }
        }
        out.push(line);
    }
    out.extend(run.notes.iter().cloned());
    out.iter().map(|line| crate::plain_paths(line)).collect()
}
