//! Relax's front end: parsing, the crate graph and name resolution (program), macro expansion (macros, preload,
//! derive, builtins, fmtmac, macro_reg) and immutable Rust source contexts (source_context). Builds on relax-base.
//! The layers below are re-exported, so every module of Relax is `crate::<module>` in each layer.

#![recursion_limit = "4096"]

pub use relax_base::*;

/// Compiler statics held for rollback; forget them before destroying their generations.
pub struct ZoneTaken {
    program: program::ZoneTaken,
    builtins: builtins::ZoneTaken,
    macro_reg: macro_reg::ZoneTaken,
}

/// Takes this crate's compiler statics while every compiler thread is quiesced.
pub fn zone_take() -> ZoneTaken {
    ZoneTaken {
        program: program::zone_take(),
        builtins: builtins::zone_take(),
        macro_reg: macro_reg::zone_take(),
    }
}

impl ZoneTaken {
    /// Registers the taken proc-macro graph again in the current (new) generation
    /// (macro_reg::zone_reinstall).
    pub fn reinstall_macro_registry(&self, sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
        macro_reg::zone_reinstall(sess, &self.macro_reg)
    }
}

/// Restores the statics after an aborted swap, forgetting the new generation's values.
pub fn zone_put_back(taken: ZoneTaken) {
    program::zone_put_back(taken.program);
    builtins::zone_put_back(taken.builtins);
    macro_reg::zone_put_back(taken.macro_reg);
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

pub mod lit;
pub mod builtin_attrs;
pub mod parser;
pub mod program;
pub mod def_collector;
pub mod assoc;
pub mod macros;
pub mod macro_check;
pub mod preload;
pub mod crate_image;
pub mod collected;
pub mod builtins;
pub mod crates;
pub mod unitgraph;
pub mod native;
pub mod macro_reg;
pub mod census;
pub mod surface;
pub mod bitflags_native;
pub mod source_context;
pub mod rs_index;
pub mod rs_outline;

pub mod parse_work;
pub mod load_profile;
pub mod exec_profile;

pub mod diagnostics;
pub mod source;
pub mod origin_table;

pub mod lang_items;
pub mod target_features;
pub mod feature_tables;
pub mod features;
