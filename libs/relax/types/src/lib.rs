//! Relax's type layer: interned types (types), signatures and impls (tcx), type checking (typeck), layouts,
//! constant evaluation, the borrow checker, definition-owned body queries and stable type hashes. Builds on relax-front.
//! The layers below are re-exported, so every module of Relax is `crate::<module>` in each layer.

pub use relax_front::*;

/// Compiler statics held for rollback; forget them before destroying their generations.
pub struct ZoneTaken {
    lazy: borrowck::lazy::ZoneTaken,
    bound_vars: tcx::resolve_bound_vars::ZoneTaken,
}

/// Takes this crate's compiler statics while every compiler thread is quiesced.
pub fn zone_take() -> ZoneTaken {
    ZoneTaken { lazy: borrowck::lazy::zone_take(), bound_vars: tcx::resolve_bound_vars::zone_take() }
}

/// Restores the statics after an aborted swap, forgetting the new generation's values.
pub fn zone_put_back(taken: ZoneTaken) {
    borrowck::lazy::zone_put_back(taken.lazy);
    tcx::resolve_bound_vars::zone_put_back(taken.bound_vars);
}

#[test]
fn zone_reset_fresh_process() {
    const CHILD: &str = "MAKEPAD_RUN_ZONE_RESET_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        std::mem::forget(zone_take());
        std::mem::forget(zone_take());
        return;
    }
    // Other tests use these statics; the reset and its empty-state assertions need isolation.
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "zone_reset_fresh_process"])
        .env(CHILD, "1")
        .status().unwrap();
    assert!(status.success());
}

pub mod types;
pub mod instance;
pub mod consts;
pub mod const_eval;
pub mod tcx;
pub mod type_queries;
pub mod typeck;
pub mod layout;
pub mod const_fold;
pub mod cycle;
pub mod borrowck;
pub mod lint;
pub mod lint_table;
pub mod lint_levels;
pub mod ast_visit;
pub mod lint_pass;
pub mod lint_unreachable;
pub mod lint_liveness;
pub mod lint_early;
pub mod lint_dead;
pub mod lint_imports;
pub mod lint_late;
pub mod lint_late2;
pub mod significant_drop;
pub mod lint_tail_drop;
pub mod lint_recursion;
pub mod lint_nonlocal;
pub mod item_checks;
pub mod feature_gate;
pub mod transmute;
pub mod wfcheck;
pub mod unsafety;
pub mod pattern_analysis;
pub mod stable;
pub mod mir;

/// A process-unique number for test scratch names (a clock alone collides between parallel tests).
#[cfg(test)]
pub(crate) fn unique_test_id() -> u128 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    (nanos << 20) | NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128
}

#[cfg(test)]
mod lang_item_tests;
pub mod infer;

#[cfg(test)]
mod fnhdr_tests;

#[cfg(test)]
mod token_source_tests;
pub mod expr_use;
pub mod upvar;
pub mod region_scope;

pub mod next_solver;

/// A name no two tests of this process share (pid and clock alone collide under parallel test threads).
#[cfg(test)]
pub(crate) fn unique_test_suffix() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}
