//! Per-pass CPU time (MAKEPAD_RUN_PASS_TIMES) and the MAKEPAD_RUN_OPT diagnostic toggles.

use std::cell::Cell;

/// CPU ns per pass (all threads, MAKEPAD_RUN_PASS_TIMES): 0 fold/copy-prop, 1 cse, 2 local copy-prop,
/// 3 dce, 4 jumps+merge, 5 inline, 6 sink, 7 layout, 8 sroa, 9 live_dce (top level), 10 licm,
/// 11 regalloc, 12 emit, 13 verify, 14 compact, 15 thread_known
pub static PASS_NS: [std::sync::atomic::AtomicU64; 16] = [const { std::sync::atomic::AtomicU64::new(0) }; 16];

/// Per-pass timing is opt-in (MAKEPAD_RUN_PASS_TIMES): the thread CPU clock is a syscall.
/// MAKEPAD_RUN_OPT diagnostic toggles beyond OptCfg's fields: `-freecap` (no size allowance for
/// O3 free inlines: R13's 1.9 MB functions, to exercise arm64 branch relaxation),
/// `-costmodel` (rustc MIR cost admission); `+single` turns the single-caller admission on;
/// `-ic` disables the InstCombine subset (nonnull, phi compares, convs, niches, sinking).
pub fn opt_off(options: &relax_session::Options, tok: &str) -> bool {
    options.opt.iter().any(|t| t.trim() == tok)
}

/// `opt_off` for passes called without the options: the same MAKEPAD_RUN_OPT tokens, read once.
pub fn opt_env_off(tok: &str) -> bool {
    static T: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::env::var("MAKEPAD_RUN_OPT").map(|v| v.split(',').map(|t| t.trim().to_string()).collect()).unwrap_or_default()).iter().any(|t| t == tok)
}

pub fn pass_clock() -> u64 {
    if PASS_ON.with(|c| c.get()) {
        if fast_clock() && !crate::probe::cost_enabled() {
            // monotonic (vDSO) clock: per-pass timing of tiny functions without the cost
            // of the thread-CPU syscall dominating them
            static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            T0.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64 + 1
        } else {
            crate::cpu_ns()
        }
    } else {
        0
    }
}

pub fn fast_clock() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var("MAKEPAD_RUN_PASS_TIMES").map_or(false, |v| v == "wall"))
}

thread_local! {
    static PASS_ON: Cell<bool> = Cell::new(std::env::var("MAKEPAD_RUN_PASS_TIMES").is_ok());
    /// this thread's ns per pass (pass_local: one build's split)
    static PASS_LOCAL: Cell<[u64; 16]> = const { Cell::new([0; 16]) };
}

/// Per-pass timing on this thread from now on (a O3 trace times each build's passes).
pub fn pass_timing_on() {
    PASS_ON.with(|c| c.set(true));
}

/// This thread's ns per pass so far (PASS_NAMES order); a build's split is the difference.
pub fn pass_local() -> [u64; 16] {
    PASS_LOCAL.with(|c| c.get())
}

pub fn pass_time(i: usize, t0: u64) {
    if t0 == 0 {
        return;
    }
    let now = pass_clock();
    PASS_NS[i].fetch_add(now - t0, std::sync::atomic::Ordering::Relaxed);
    PASS_LOCAL.with(|c| {
        let mut a = c.get();
        a[i] += now - t0;
        c.set(a);
    });
}

pub const PASS_NAMES: [&str; 16] = ["fold", "cse", "lcopy", "dce", "jumps", "inline", "sink", "layout", "sroa", "live_dce", "licm", "regalloc", "emit", "verify", "compact", "thread_known"];
