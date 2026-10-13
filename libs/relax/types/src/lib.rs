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
pub mod borrowck;
pub mod lint;
pub mod item_checks;
pub mod unsafety;
pub mod pattern_analysis;
pub mod stable;
pub mod mir;

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
