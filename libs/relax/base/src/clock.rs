//! The front end's process-wide monotonic wall clock (thread CPU time is relax_backend::cpu_ns,
//! shared with the optimiser so build and pass totals use one calibration).

/// Wall time in ns since the first call of it in this process.
pub fn wall_ns() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

// Why this thread is working (type-checking bodies, parsing lazily): MAKEPAD_RUN_TYCK_STATS /
// MAKEPAD_RUN_LAZY_STATS attribute their numbers by it.
thread_local! { pub static TYS_ORIGIN: std::cell::Cell<&'static str> = const { std::cell::Cell::new("?") }; }

/// Names why this thread type-checks bodies until the guard drops (the previous name is
/// restored): MAKEPAD_RUN_TYCK_STATS / MAKEPAD_RUN_LAZY_STATS attribute checks and lazy parses by it.
pub fn origin(o: &'static str) -> OriginGuard {
    OriginGuard(TYS_ORIGIN.with(|c| c.replace(o)))
}

pub struct OriginGuard(&'static str);

impl Drop for OriginGuard {
    fn drop(&mut self) {
        TYS_ORIGIN.with(|c| c.set(self.0));
    }
}

/// When `makepad run` started (its boot timeline and reports count from here).
static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Starts the run clock (once; later calls keep the first time). A process that continues a
/// session across an exec (see `mark_process_start`) counts from the session's start.
pub fn mark_start() {
    mark_process_start();
    let p = PROCESS_START.get().unwrap();
    let _ = T0.set(if p.2 { p.1 } else { std::time::Instant::now() });
}

/// Milliseconds since `mark_start` (0 before it).
pub fn ms_since_start() -> f64 {
    T0.get().map(|t| t.elapsed().as_secs_f64() * 1e3).unwrap_or(0.0)
}

// Separate from the historical run/phase clock: includes loader setup before cmd_run.
// Each executable marks this at entry, so child launches never inherit their parent's origin.
// (wall start, its Instant, continued across an exec)
static PROCESS_START: std::sync::OnceLock<(std::time::SystemTime, std::time::Instant, bool)> = std::sync::OnceLock::new();

/// The program replaced this process with the loader again (an exec of its own executable:
/// Relax's std sets this on that exec) -- the same session going on, so every clock
/// counts from the first process's start (epoch seconds), and first-frame / main times include
/// both boots. Taken once at entry and removed, so no child launch inherits it.
pub const EXEC_ORIGIN_ENV: &str = "MAKEPAD_RUN_EXEC_ORIGIN_EPOCH";

pub fn mark_process_start() {
    PROCESS_START.get_or_init(|| {
        let (wall, now) = (std::time::SystemTime::now(), std::time::Instant::now());
        let origin = std::env::var(EXEC_ORIGIN_ENV).ok().and_then(|v| v.trim().parse::<f64>().ok());
        if origin.is_some() {
            unsafe { std::env::remove_var(EXEC_ORIGIN_ENV) };
        }
        let back = origin
            .map(|o| std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(o))
            .and_then(|o| wall.duration_since(o).ok())
            .filter(|d| d.as_secs() < 600)
            .and_then(|d| now.checked_sub(d).map(|i| (wall - d, i)));
        match back {
            Some((w, i)) => (w, i, true),
            None => (wall, now, false),
        }
    });
}

/// Epoch seconds at executable entry, for an in-process program's first-present clock.
pub fn process_start_epoch() -> f64 {
    mark_process_start();
    PROCESS_START.get().unwrap().0.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

pub fn ms_since_process_start() -> f64 {
    mark_process_start();
    PROCESS_START.get().unwrap().1.elapsed().as_secs_f64() * 1e3
}
