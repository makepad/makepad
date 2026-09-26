//! Progress events from installer work to whoever shows them.
//!
//! The hook, the running component's bar and the current package are per
//! thread. Work that runs on other threads (the install jobs, downloads, the
//! unpack pool) carries a [`Ctx`] there: its hook is a shared, sendable
//! forwarder, and the component's bar is shared between threads through
//! atomics, so every thread working for one component moves the same bar.
use std::{
    cell::RefCell,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Unit {
    #[default]
    None,
    Bytes,
    Files,
    Blocks,
    Objects,
    /// Crates compiled so far (the total is not known before Cargo finishes).
    Crates,
}
#[derive(Clone, Debug, Default)]
pub struct Package {
    pub group: String,
    pub name: String,
    pub index: usize,
    pub count: usize,
}
#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub detail: String,
    pub loaded: u64,
    pub total: u64,
    pub frac: f32,
    pub unit: Unit,
    pub package: Package,
    /// The component's single forward-only bar, when one is running.
    pub overall: Option<Overall>,
    /// The install job (a row of the work page) the event comes from, when
    /// components install side by side (`jobs::run`); empty otherwise.
    pub row: String,
}
/// One bar for a whole component (Build tools, Windows SDK, Rust, CUDA):
/// its work in bytes is known before it starts, every payload counting once
/// when downloaded and once when unpacked, so the bar only moves forward.
#[derive(Clone, Debug)]
pub struct Overall {
    pub label: String,
    /// 0..=1, never less than an earlier event's.
    pub fraction: f64,
    /// The payload size (half the work units), for "620 / 1480 MB".
    pub bytes: u64,
    /// Bytes received from the network so far, all connections together.
    pub downloaded: u64,
    /// Payloads being downloaded, unpacked and written right now.
    pub downloading: usize,
    pub unpacking: usize,
    pub writing: usize,
}
/// What a thread is doing for its component, counted while a guard lives.
#[derive(Clone, Copy, Debug)]
pub enum Activity {
    Download = 0,
    Unpack = 1,
    Write = 2,
}

/// A component's bar, shared by every thread that works for it.
pub struct Component {
    label: String,
    units: AtomicU64,
    done: AtomicU64,
    downloaded: AtomicU64,
    active: [AtomicUsize; 3],
}
impl Component {
    fn overall(&self) -> Overall {
        let units = self.units.load(Ordering::Relaxed);
        let done = self.done.load(Ordering::Relaxed);
        Overall {
            label: self.label.clone(),
            fraction: (done as f64 / units.max(1) as f64).clamp(0.0, 1.0),
            bytes: units / 2,
            downloaded: self.downloaded.load(Ordering::Relaxed),
            downloading: self.active[0].load(Ordering::Relaxed),
            unpacking: self.active[1].load(Ordering::Relaxed),
            writing: self.active[2].load(Ordering::Relaxed),
        }
    }
    fn add(&self, units: u64) {
        // Never past the whole: a payload larger than its manifest size must
        // not push the bar over 100 %.
        let total = self.units.load(Ordering::Relaxed);
        let _ = self.done.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |d| Some(d.saturating_add(units).min(total)));
    }
}

/// A hook that other threads can call: events from them arrive here.
pub type Forward = Arc<dyn Fn(Progress) + Send + Sync>;
enum Hook {
    Local(Box<dyn FnMut(Progress)>),
    Shared(Forward),
}
/// The piece of work running on this thread within its component's bar:
/// `weight` units, of which `reported` have been added to the bar.
struct Step {
    weight: u64,
    reported: u64,
}
thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static PACKAGE: RefCell<Package> = RefCell::new(Package::default());
    static TOTAL: RefCell<Option<Arc<Component>>> = const { RefCell::new(None) };
    static STEP: RefCell<Option<Step>> = const { RefCell::new(None) };
}

pub fn scope<R>(hook: impl FnMut(Progress) + 'static, f: impl FnOnce() -> R) -> R {
    install(Hook::Local(Box::new(hook)), f)
}
/// Like [`scope`] with a hook other threads may share (see [`Ctx`]).
pub fn forward<R>(hook: Forward, f: impl FnOnce() -> R) -> R {
    install(Hook::Shared(hook), f)
}
fn install<R>(hook: Hook, f: impl FnOnce() -> R) -> R {
    let previous = HOOK.with(|slot| slot.replace(Some(hook)));
    struct Restore(Option<Hook>, Package);
    impl Drop for Restore {
        fn drop(&mut self) {
            HOOK.with(|h| {
                h.replace(self.0.take());
            });
            PACKAGE.with(|p| {
                p.replace(std::mem::take(&mut self.1));
            });
        }
    }
    let _restore = Restore(previous, PACKAGE.with(|p| p.replace(Package::default())));
    f()
}

/// This thread's progress state, to continue on another thread: its hook
/// (only a shared one travels), its component's bar and its package.
#[derive(Clone, Default)]
pub struct Ctx {
    hook: Option<Forward>,
    total: Option<Arc<Component>>,
    package: Package,
}
impl Ctx {
    pub fn current() -> Ctx {
        Ctx {
            hook: HOOK.with(|h| match h.borrow().as_ref() {
                Some(Hook::Shared(hook)) => Some(hook.clone()),
                _ => None,
            }),
            total: TOTAL.with(|t| t.borrow().clone()),
            package: PACKAGE.with(|p| p.borrow().clone()),
        }
    }
    /// Run `f` on this thread as if on the thread the context came from. A
    /// step does not carry over: work on another thread starts its own.
    pub fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let hook = HOOK.with(|h| h.replace(self.hook.clone().map(Hook::Shared)));
        let total = TOTAL.with(|t| t.replace(self.total.clone()));
        let package = PACKAGE.with(|p| p.replace(self.package.clone()));
        let step = STEP.with(|s| s.replace(None));
        struct Restore(Option<Option<Hook>>, Option<Option<Arc<Component>>>, Option<Package>, Option<Option<Step>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                HOOK.with(|h| *h.borrow_mut() = self.0.take().flatten());
                TOTAL.with(|t| *t.borrow_mut() = self.1.take().flatten());
                PACKAGE.with(|p| *p.borrow_mut() = self.2.take().unwrap_or_default());
                STEP.with(|s| *s.borrow_mut() = self.3.take().flatten());
            }
        }
        let _restore = Restore(Some(hook), Some(total), Some(package), Some(step));
        f()
    }
}

pub fn emit(mut progress: Progress) {
    progress.package = PACKAGE.with(|p| p.borrow().clone());
    progress.overall = TOTAL.with(|total| {
        let total = total.borrow();
        let total = total.as_ref()?;
        // The running step's own loaded/total, when known, moves the bar
        // within that step's share.
        if progress.total > 0 {
            let within = (progress.loaded as f64 / progress.total as f64).clamp(0.0, 1.0);
            STEP.with(|s| {
                if let Some(step) = s.borrow_mut().as_mut() {
                    let reached = (step.weight as f64 * within) as u64;
                    if reached > step.reported {
                        total.add(reached - step.reported);
                        step.reported = reached;
                    }
                }
            });
        }
        Some(total.overall())
    });
    deliver(progress);
}
/// Hand an event to this thread's hook as it is (already stamped elsewhere).
pub fn deliver(progress: Progress) {
    HOOK.with(|slot| match slot.borrow_mut().as_mut() {
        Some(Hook::Local(hook)) => hook(progress),
        Some(Hook::Shared(hook)) => hook(progress),
        None => {}
    });
}
pub fn package(group: &str, name: &str, index: usize, count: usize) {
    PACKAGE.with(|p| {
        p.replace(Package {
            group: group.into(),
            name: name.into(),
            index,
            count,
        });
    });
    stage("Preparing", name, 0.0);
}
pub fn stage(stage: &str, detail: &str, frac: f32) {
    emit(Progress {
        stage: stage.into(),
        detail: detail.into(),
        loaded: 0,
        total: 0,
        frac,
        unit: Unit::None,
        package: Package::default(),
        overall: None,
        row: String::new(),
    });
}
pub fn measured(stage: &str, detail: &str, loaded: u64, total: u64, unit: Unit) {
    emit(Progress {
        stage: stage.into(),
        detail: detail.into(),
        loaded,
        total,
        frac: if total > 0 {
            loaded as f32 / total as f32
        } else {
            0.0
        },
        unit,
        package: Package::default(),
        overall: None,
        row: String::new(),
    });
}
/// Start a component's bar: `bytes` is the sum of its payload sizes; each
/// is later stepped twice (download, then unpack).
pub fn total_begin(label: &str, bytes: u64) {
    let component = Component {
        label: label.into(),
        units: AtomicU64::new(bytes * 2),
        done: AtomicU64::new(0),
        downloaded: AtomicU64::new(0),
        active: Default::default(),
    };
    TOTAL.with(|t| *t.borrow_mut() = Some(Arc::new(component)));
    STEP.with(|s| *s.borrow_mut() = None);
}
/// Ends the component's bar and its package: a later step without its own
/// package must not read the last one's "2 of 2" as its fraction.
pub fn total_end() {
    TOTAL.with(|t| *t.borrow_mut() = None);
    STEP.with(|s| *s.borrow_mut() = None);
    PACKAGE.with(|p| p.replace(Package::default()));
}
/// `total_begin` ended when the guard drops, also on an early return.
pub struct TotalGuard;
impl Drop for TotalGuard {
    fn drop(&mut self) {
        total_end();
    }
}
pub fn total(label: &str, bytes: u64) -> TotalGuard {
    total_begin(label, bytes);
    TotalGuard
}
/// The next piece of work on this thread, `weight` units (a payload's
/// size), until `step_done`. Other threads have steps of their own.
pub fn step(weight: u64) {
    STEP.with(|s| *s.borrow_mut() = Some(Step { weight, reported: 0 }));
}
pub fn step_done() {
    let step = STEP.with(|s| s.borrow_mut().take());
    if let Some(step) = step {
        advance(step.weight.saturating_sub(step.reported));
    }
}
/// `units` of the component's work done outside a step (bytes received by
/// a download connection, a payload found in the cache).
pub fn advance(units: u64) {
    TOTAL.with(|t| {
        if let Some(total) = t.borrow().as_ref() {
            total.add(units);
        }
    });
}
/// Bytes received from the network for this component.
pub fn downloaded(bytes: u64) {
    TOTAL.with(|t| {
        if let Some(total) = t.borrow().as_ref() {
            total.downloaded.fetch_add(bytes, Ordering::Relaxed);
        }
    });
}
/// Counts this thread as doing `activity` for its component until the
/// guard drops ("downloading 3 · unpacking 2").
pub fn activity(activity: Activity) -> ActivityGuard {
    let total = TOTAL.with(|t| t.borrow().clone());
    if let Some(total) = &total {
        total.active[activity as usize].fetch_add(1, Ordering::Relaxed);
    }
    ActivityGuard(total, activity)
}
pub struct ActivityGuard(Option<Arc<Component>>, Activity);
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if let Some(total) = &self.0 {
            total.active[self.1 as usize].fetch_sub(1, Ordering::Relaxed);
        }
    }
}
/// The label of the component this thread works for.
pub fn component() -> Option<String> {
    TOTAL.with(|t| t.borrow().as_ref().map(|c| c.label.clone()))
}

pub fn active() -> bool { HOOK.with(|h| h.borrow().is_some()) }
pub fn message(text: &str) {
    if HOOK.with(|h| h.borrow().is_some()) {
        stage("Working", text, 0.0);
    } else {
        println!("{text}");
    }
}
#[macro_export]
macro_rules! setup_note { ($($args:tt)*) => { $crate::progress::message(&format!($($args)*)) }; }

/// Components installing side by side, one row each, from their events
/// (those with a `row`): what the work page and the plain printer show.
#[derive(Default)]
pub struct Rows {
    pub rows: Vec<Row>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum RowState {
    Running,
    Done,
    Failed(String),
    Stopped,
    /// Windows security software still holds the new compiler.
    Waiting,
}
pub struct Row {
    pub label: String,
    pub state: RowState,
    /// Forward-only share of the component's work, when its size is known.
    pub fraction: Option<f64>,
    /// Payload bytes of the component.
    pub bytes: u64,
    /// What it does now: "downloading 3 · unpacking 2", "checking downloads".
    pub doing: String,
    /// Network bytes per second, smoothed.
    pub rate: f64,
    /// Seconds left, once there is a steady pace.
    pub eta: Option<f64>,
    pub started: std::time::Instant,
    pub took: f64,
    sample: Option<(std::time::Instant, u64, f64)>,
    pace: f64,
}
impl Rows {
    /// Update from an event; true when a row started or finished (worth
    /// drawing at once rather than at the next frame).
    pub fn update(&mut self, p: &Progress) -> bool {
        if p.row.is_empty() {
            return false;
        }
        let now = std::time::Instant::now();
        let index = match self.rows.iter().position(|r| r.label == p.row) {
            Some(index) => index,
            None => {
                self.rows.push(Row {
                    label: p.row.clone(),
                    state: RowState::Running,
                    fraction: None,
                    bytes: 0,
                    doing: String::new(),
                    rate: 0.0,
                    eta: None,
                    started: now,
                    took: 0.0,
                    sample: None,
                    pace: 0.0,
                });
                self.rows.len() - 1
            }
        };
        let row = &mut self.rows[index];
        let fresh = row.sample.is_none() && row.fraction.is_none() && row.doing.is_empty();
        let end = match p.stage.as_str() {
            crate::jobs::DONE => Some(RowState::Done),
            crate::jobs::FAILED => Some(RowState::Failed(p.detail.lines().next().unwrap_or_default().to_owned())),
            crate::jobs::STOPPED => Some(RowState::Stopped),
            crate::jobs::WAITING => Some(RowState::Waiting),
            _ => None,
        };
        if let Some(state) = end {
            row.took = now.duration_since(row.started).as_secs_f64();
            if state == RowState::Done {
                row.fraction = Some(1.0);
            }
            row.state = state;
            row.doing.clear();
            row.eta = None;
            return true;
        }
        if row.state != RowState::Running {
            return false;
        }
        if let Some(o) = &p.overall {
            row.fraction = Some(row.fraction.unwrap_or(0.0).max(o.fraction));
            row.bytes = o.bytes;
            let doing: Vec<&str> = [(o.downloading, "downloading"), (o.unpacking, "unpacking"), (o.writing, "writing")]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(_, what)| what)
                .collect();
            row.doing = if doing.is_empty() { phase(&p.stage).into() } else { doing.join(", ") };
            let fraction = row.fraction.unwrap_or(0.0);
            match row.sample {
                None => row.sample = Some((now, o.downloaded, fraction)),
                Some((at, bytes, done)) => {
                    let dt = now.duration_since(at).as_secs_f64();
                    if dt >= 0.5 {
                        let rate = o.downloaded.saturating_sub(bytes) as f64 / dt;
                        row.rate = if row.rate == 0.0 { rate } else { row.rate * 0.6 + rate * 0.4 };
                        let pace = (fraction - done).max(0.0) / dt;
                        row.pace = if row.pace == 0.0 { pace } else { row.pace * 0.7 + pace * 0.3 };
                        row.eta = (row.pace > 0.0 && now.duration_since(row.started).as_secs_f64() > 2.0)
                            .then(|| (1.0 - fraction) / row.pace);
                        row.sample = Some((now, o.downloaded, fraction));
                    }
                }
            }
        } else {
            row.doing = phase(&p.stage).into();
        }
        fresh
    }
    /// A finished row for the log: "done · 459 MB in 31.2 s", or why not.
    pub fn end_line(row: &Row) -> String {
        match &row.state {
            RowState::Done if row.bytes == 0 => format!("ready in {:.1} s", row.took),
            RowState::Done => format!("done · {:.0} MB in {:.1} s", row.bytes as f64 / 1_048_576.0, row.took),
            RowState::Failed(reason) => format!("failed after {:.1} s: {reason}", row.took),
            RowState::Stopped => format!("stopped after {:.1} s: another component failed", row.took),
            RowState::Waiting => "waiting for Windows security to release the compiler".into(),
            RowState::Running => "running".into(),
        }
    }
    /// The row's figures after its bar: "73/173 MB · 28 MB/s · 9 s left ·
    /// downloading", or its one-line summary when done.
    pub fn detail(row: &Row) -> String {
        let mb = |b: f64| b / 1_048_576.0;
        match &row.state {
            RowState::Done if row.bytes > 0 => format!("{:.0} MB · {:.0} s", mb(row.bytes as f64), row.took.max(1.0)),
            RowState::Done => format!("ready · {:.0} s", row.took.max(1.0)),
            RowState::Failed(reason) => reason.clone(),
            RowState::Stopped => "stopped".into(),
            RowState::Waiting => "waiting for Windows security to release the compiler".into(),
            RowState::Running => {
                // Most telling first: a narrow terminal cuts the end.
                let mut parts = Vec::new();
                if row.bytes > 0 {
                    let done = row.fraction.unwrap_or(0.0) * row.bytes as f64;
                    parts.push(format!("{:.0}/{:.0} MB", mb(done), mb(row.bytes as f64)));
                }
                if row.rate >= 65_536.0 && row.doing.starts_with("downloading") {
                    parts.push(format!("{:.0} MB/s", mb(row.rate)));
                }
                if let Some(eta) = row.eta.filter(|e| *e < 3600.0) {
                    parts.push(format!("{:.0} s left", eta.max(1.0)));
                }
                if !row.doing.is_empty() {
                    parts.push(row.doing.clone());
                }
                parts.join(" · ")
            }
        }
    }
}
/// A short word for what a stage does; never a file name.
fn phase(stage: &str) -> &'static str {
    match stage {
        "Download" => "downloading",
        "Verify cache" | "Verify SHA-256" => "checking downloads",
        "Checking compiler" => "checking the compiler",
        "Decompressing" | "Decompressing CAB" | "Unpacking" => "unpacking",
        "Installing" | "Installing SDK" | "Save download" => "writing",
        "Ready" => "finishing",
        _ => "preparing",
    }
}

/// A hook for plain output (no terminal UI): side-by-side components print
/// one line each when they start and finish and every few seconds between;
/// other steps print when their stage changes.
pub fn printer() -> impl FnMut(Progress) + 'static {
    let mut rows = Rows::default();
    let mut printed: Vec<(String, std::time::Instant)> = Vec::new();
    let mut last_stage = String::new();
    let begun = std::time::Instant::now();
    move |p: Progress| {
        if p.row.is_empty() {
            let stage = format!("{}: {}", p.stage, p.detail);
            if p.stage != "Working" && p.stage != "Download" && stage != last_stage {
                println!("{:6.1} {}", begun.elapsed().as_secs_f64(), crate::plain_paths(&stage));
                last_stage = stage;
            }
            return;
        }
        let changed = rows.update(&p);
        let Some(row) = rows.rows.iter().find(|r| r.label == p.row) else { return };
        let now = std::time::Instant::now();
        let due = match printed.iter().position(|(l, _)| *l == row.label) {
            Some(i) => changed || now.duration_since(printed[i].1).as_secs_f64() >= 2.0,
            None => true,
        };
        if !due {
            return;
        }
        match printed.iter_mut().find(|(l, _)| *l == row.label) {
            Some(entry) => entry.1 = now,
            None => printed.push((row.label.clone(), now)),
        }
        let mark = match row.state {
            RowState::Running => "  ",
            RowState::Done => "✓ ",
            RowState::Failed(_) => "✗ ",
            RowState::Stopped | RowState::Waiting => "- ",
        };
        let percent = match (&row.state, row.fraction) {
            (RowState::Running, Some(f)) => format!("{:3.0}%  ", f * 100.0),
            (RowState::Running, None) => "      ".into(),
            _ => String::new(),
        };
        println!("{:6.1} {mark}{:<12} {percent}{}", begun.elapsed().as_secs_f64(), row.label, crate::plain_paths(&Rows::detail(row)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component's bar sums real byte weights, moves within a step by the
    /// step's own loaded/total, and never goes backwards when the next
    /// payload's download starts again from zero.
    #[test]
    fn component_total_only_moves_forward() {
        let seen = std::rc::Rc::new(RefCell::new(Vec::new()));
        let log = seen.clone();
        scope(
            move |p| log.borrow_mut().push(p.overall.map(|o| (o.fraction, o.bytes))),
            || {
                total_begin("Build tools", 300);
                step(100);
                measured("Download", "a", 50, 100, Unit::Bytes);
                step_done();
                step(100);
                measured("Extract", "a", 0, 0, Unit::Files);
                step_done();
                step(200);
                measured("Download", "b", 0, 200, Unit::Bytes);
                measured("Download", "b", 200, 200, Unit::Bytes);
                step_done();
                total_end();
                measured("Download", "c", 1, 2, Unit::Bytes);
            },
        );
        let seen = seen.borrow();
        let fractions: Vec<f64> = seen.iter().flatten().map(|(f, _)| *f).collect();
        assert_eq!(seen[0], Some((50.0 / 600.0, 300)));
        assert!(fractions.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(fractions.last(), Some(&(400.0 / 600.0)));
        assert_eq!(seen.last(), Some(&None));
    }
}
