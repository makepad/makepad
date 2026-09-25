use std::cell::RefCell;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Unit {
    #[default]
    None,
    Bytes,
    Files,
    Blocks,
    Objects,
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
    /// The components this run installs, in order, and which one this is
    /// (see `plan`); empty when no plan was given.
    pub plan: Vec<String>,
    pub step: usize,
}
struct Total {
    label: String,
    units: u64,
    done: u64,
    step: u64,
    shown: f64,
}
thread_local! {
    static HOOK: RefCell<Option<Box<dyn FnMut(Progress)>>> = const { RefCell::new(None) };
    static PACKAGE: RefCell<Package> = RefCell::new(Package::default());
    static TOTAL: RefCell<Option<Total>> = const { RefCell::new(None) };
    static PLAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn scope<R>(hook: impl FnMut(Progress) + 'static, f: impl FnOnce() -> R) -> R {
    let previous = HOOK.with(|slot| slot.replace(Some(Box::new(hook))));
    struct Restore(Option<Box<dyn FnMut(Progress)>>, Package);
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
pub fn emit(mut progress: Progress) {
    progress.package = PACKAGE.with(|p| p.borrow().clone());
    progress.overall = TOTAL.with(|total| {
        let mut total = total.borrow_mut();
        let total = total.as_mut()?;
        // The running step's own loaded/total, when known, moves the bar
        // within that step's share.
        let within = if progress.total > 0 { (progress.loaded as f64 / progress.total as f64).clamp(0.0, 1.0) } else { 0.0 };
        let fraction = ((total.done as f64 + total.step as f64 * within) / total.units.max(1) as f64).clamp(0.0, 1.0);
        total.shown = total.shown.max(fraction);
        let plan = PLAN.with(|p| p.borrow().clone());
        let step = plan.iter().position(|l| *l == total.label).unwrap_or(0);
        Some(Overall { label: total.label.clone(), fraction: total.shown, bytes: total.units / 2, plan, step })
    });
    HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(progress);
        }
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
    });
}
/// The components the next installs will run, in order ("Step 1 of 3").
/// Only those that actually run are listed; an empty list clears it.
pub fn plan(labels: &[&str]) {
    PLAN.with(|p| *p.borrow_mut() = labels.iter().map(|l| l.to_string()).collect());
}
/// Start a component's bar: `bytes` is the sum of its payload sizes; each
/// is later stepped twice (download, then unpack).
pub fn total_begin(label: &str, bytes: u64) {
    TOTAL.with(|t| *t.borrow_mut() = Some(Total { label: label.into(), units: bytes * 2, done: 0, step: 0, shown: 0.0 }));
}
pub fn total_end() {
    TOTAL.with(|t| *t.borrow_mut() = None);
}
/// The next piece of work, `weight` units (a payload's size), until `step_done`.
pub fn step(weight: u64) {
    TOTAL.with(|t| if let Some(t) = t.borrow_mut().as_mut() { t.step = weight; });
}
pub fn step_done() {
    TOTAL.with(|t| if let Some(t) = t.borrow_mut().as_mut() { t.done += t.step; t.step = 0; });
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
