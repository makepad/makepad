//! Where a bake pass says what it is doing.
//!
//! The passes were written as CLI passes: they `println!` a running
//! commentary that is genuinely useful (block counts, tiles/s, per-zoom
//! byte totals) and that anyone debugging a bad archive reads first. An
//! app running the same passes on a worker thread needs those same lines
//! in a window, plus a fraction to drive a bar.
//!
//! So every pass reports through [`report`] / the [`step`], [`note`] and
//! [`tick`] macros instead of `println!`. With no sink installed the line goes to
//! stdout exactly as before (the CLI is unchanged); with one installed it
//! goes to the sink INSTEAD, so a host that draws its own log is not also
//! spamming a terminal it may not have.
//!
//! The sink is process-global because the passes fan work out across
//! worker threads and closures several layers deep; threading a channel
//! through every one of them would be a large diff for no gain. One bake
//! at a time is the only supported shape — [`SinkGuard`] enforces it by
//! refusing to install a second sink while one is live.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// One line of bake commentary.
#[derive(Clone, Debug)]
pub struct Report {
    /// Which pass is speaking, e.g. `"detail"`, `"base"`, `"nav"`.
    pub stage: &'static str,
    /// The line itself, already formatted, without a trailing newline.
    pub line: String,
    /// How far this stage has got, when it knows: 0.0..=1.0.
    pub fraction: Option<f32>,
    /// True for a stage headline (a new pass starting), false for detail
    /// under the current headline. A host with one status line and one log
    /// pane knows which pane each belongs in.
    pub headline: bool,
}

type Sink = Box<dyn Fn(Report) + Send + Sync>;

fn slot() -> &'static Mutex<Option<Sink>> {
    static SLOT: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Fast path for the overwhelmingly common case (CLI, tests): no lock, no
/// allocation of a `Report` that nobody reads.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Sends every [`Report`] to `sink` until the guard is dropped.
///
/// Returns `None` if a sink is already installed: two concurrent bakes
/// would interleave their commentary into one window, and the passes are
/// not built to run twice in a process anyway (each wants the machine).
pub struct SinkGuard(());

impl SinkGuard {
    pub fn install(sink: impl Fn(Report) + Send + Sync + 'static) -> Option<SinkGuard> {
        let mut slot = slot().lock().ok()?;
        if slot.is_some() {
            return None;
        }
        *slot = Some(Box::new(sink));
        INSTALLED.store(true, Ordering::Release);
        Some(SinkGuard(()))
    }
}

impl Drop for SinkGuard {
    fn drop(&mut self) {
        INSTALLED.store(false, Ordering::Release);
        if let Ok(mut slot) = slot().lock() {
            *slot = None;
        }
    }
}

/// Report one line. Goes to the installed sink, or to stdout.
pub fn report(report: Report) {
    if INSTALLED.load(Ordering::Acquire) {
        if let Ok(slot) = slot().lock() {
            if let Some(sink) = slot.as_ref() {
                sink(report);
                return;
            }
        }
    }
    println!("{}", report.line);
}

/// A stage headline: `step!("base", "Phase 2/3: writing archive zooms 0..=14")`.
#[macro_export]
macro_rules! step {
    ($stage:expr, $($arg:tt)*) => {
        $crate::progress::report($crate::progress::Report {
            stage: $stage,
            line: format!($($arg)*),
            fraction: None,
            headline: true,
        })
    };
}

/// Detail under the current headline: `note!("base", "  z12: 82 tiles")`.
#[macro_export]
macro_rules! note {
    ($stage:expr, $($arg:tt)*) => {
        $crate::progress::report($crate::progress::Report {
            stage: $stage,
            line: format!($($arg)*),
            fraction: None,
            headline: false,
        })
    };
}

/// Detail that also knows how far the stage has got — the lines a bar can
/// be driven from: `tick!("base", 0.42, "  z12: 82 tiles")`. Separate from
/// [`note`] rather than an optional argument, so a leading format string is
/// never mistaken for a fraction.
#[macro_export]
macro_rules! tick {
    ($stage:expr, $fraction:expr, $($arg:tt)*) => {
        $crate::progress::report($crate::progress::Report {
            stage: $stage,
            line: format!($($arg)*),
            fraction: Some($fraction as f32),
            headline: false,
        })
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    // One test, not three: the sink is process-global, and cargo runs the
    // tests of a crate in parallel threads of one process.
    #[test]
    fn sink_receives_reports_is_exclusive_and_releases_on_drop() {
        let (tx, rx) = mpsc::channel();
        let guard = SinkGuard::install(move |r| {
            let _ = tx.send(r);
        })
        .expect("no sink installed");
        crate::step!("test", "headline {}", 1);
        crate::tick!("test", 0.5, "detail");
        let first = rx.recv().unwrap();
        assert_eq!(first.line, "headline 1");
        assert!(first.headline);
        let second = rx.recv().unwrap();
        assert_eq!(second.fraction, Some(0.5));
        assert!(!second.headline);
        // Exclusive while live, free once dropped.
        assert!(SinkGuard::install(|_| {}).is_none());
        drop(guard);
        assert!(SinkGuard::install(|_| {}).is_some());
    }
}
