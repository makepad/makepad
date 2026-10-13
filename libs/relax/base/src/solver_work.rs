//! Opt-in work counts at first present; no wall-clock comparisons on shared hosts.
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};

static ENABLED: OnceLock<bool> = OnceLock::new();
static COUNTS: [AtomicU64; 24] = [const { AtomicU64::new(0) }; 24];
pub const NORMALIZE: usize = 0;
pub const NORMALIZE_SKIPPED: usize = 1;
pub const IMPL_GOALS: usize = 2;
pub const IMPL_PROBES: usize = 3;
pub const IMPL_REJECTED: usize = 4;
pub const RECURSION_SCANS: usize = 5;
pub const ENV_INTERNS: usize = 6;

pub const ENV_HASHES: usize = 7;
pub const ENV_HASH_CLAUSES: usize = 8;
pub const ENV_RESOLVE_CLAUSES: usize = 9;
pub const ENV_CANON_CLAUSES: usize = 10;
pub const CANON_GOALS: usize = 11;
pub const CANON_ENV_SKIPPED: usize = 12;
pub const CANON_PRED_SKIPPED: usize = 13;
pub const CANON_TY_SKIPPED: usize = 14;
pub const RESULT_READS: usize = 15;
pub const RESULT_CLONES: usize = 16;
pub const ANON_TASKS: usize = 17;
pub const ANON_LOCKS: usize = 18;
pub const SOLVER_GLOBAL_LOCKS: usize = 19;
pub const SOLVER_INTERN_LOCKS: usize = 20;
pub const SOLVER_MAP_LOCKS: usize = 21;
pub const ENV_CACHE_LOCKS: usize = 22;
pub const SEARCH_GOALS: usize = 23;

#[inline]
pub fn count(kind: usize) { add(kind, 1); }

#[inline]
pub fn add(kind: usize, amount: u64) {
    if *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_RUN_SOLVER_WORK_TRACE").is_some()) {
        COUNTS[kind].fetch_add(amount, Ordering::Relaxed);
    }
}

pub fn report() {
    if !*ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_RUN_SOLVER_WORK_TRACE").is_some()) { return; }
    let names = ["normalize", "normalize_skipped", "impl_goals", "impl_probes", "impl_rejected", "recursion_scans", "env_interns", "env_hashes", "env_hash_clauses", "env_resolve_clauses", "env_canon_clauses", "canon_goals", "canon_env_skipped", "canon_pred_skipped", "canon_ty_skipped", "result_reads", "result_clones", "anon_tasks", "anon_locks", "solver_global_locks", "solver_intern_locks", "solver_map_locks", "env_cache_locks", "search_goals"];
    for (name, count) in names.iter().zip(&COUNTS) {
        eprintln!("makepad-run solver-work: {name}={}", count.load(Ordering::Relaxed));
    }
}
