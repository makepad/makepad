//! Relax's base: what every phase shares and nothing above it -- hashing (fxhash, sip), memory accounting (memstat,
//! zone_alloc), clocks, concurrent containers (conc, arena), the lexer and syntax tree (lexer, ast), cfg, the
//! manifests (toml, resolve), the git read path and source mounts (git, gitstore, vfs, inflate)
//! and the proc-macro token model (pm, pm_tokens).
//! The layers below are re-exported, so every module of Relax is `crate::<module>` in each layer.

#![recursion_limit = "1024"]


// Diagnostics never panic: std's print macros panic when a write fails (ENOSPC on the session
// log in a full tmpfs /tmp aborted a fleet run inside a lazy compile, 2026-10-23), so in this
// crate every print is a write whose error is dropped. Test builds keep std's captured macros.
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprintln {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_all(b"\n"); }};
    ($($t:tt)+) => { $crate::vfs::write_err(format_args!("{}\n", format_args!($($t)+))) };
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprint {
    ($($t:tt)+) => { $crate::vfs::write_err(format_args!($($t)+)) };
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! println {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_all(b"\n"); }};
    ($($t:tt)+) => { $crate::vfs::write_out(format_args!("{}\n", format_args!($($t)+))) };
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! print {
    ($($t:tt)+) => { $crate::vfs::write_out(format_args!($($t)+)) };
}

pub mod fxhash;
pub mod memstat;
pub use relax_session::{memo_local, options, sip, Session};
pub mod conc;
pub mod lockfree;
pub mod pool;
pub mod query;
pub mod preempt;
pub mod stack;
pub mod sstack;
pub mod arena;
pub mod json;
pub mod diag;
pub use relax_lex::lexer;
pub use relax_lex::packed;
pub mod ast;
pub mod toml;
pub mod inflate;
pub mod git;
pub mod gitstore;
pub mod vfs;
pub mod packsrc;
pub mod progress;
pub mod run_state;
pub mod run_threads;
pub mod boot_stats;
pub mod dirs;
pub use relax_lex::tokens as pm_tokens;
pub mod pm;
pub mod progenv;
pub mod compile_time;
pub mod zone_alloc;
pub mod clock;
pub mod relaxed;
pub mod srchash;
pub mod thread_local_mac;
pub mod tokbuf;
pub mod cfg;
pub mod registry;
pub mod resolve;
pub mod sha256;
pub mod std_digest;
pub mod tar;
pub mod image;
pub mod why;
pub mod waits;

pub mod scc;

pub mod solver_work;

/// The command this process runs as, for the lines that tell the user what to type next
/// (`relax inspect <pid> help`): `relax` unless the runner set another at start (the Makepad
/// host sets `makepad`). A property of the process, so reset_statics leaves it.
static TOOL_NAME: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
pub fn set_tool_name(name: &'static str) {
    let _ = TOOL_NAME.set(name);
}
pub fn tool_name() -> &'static str {
    TOOL_NAME.get().copied().unwrap_or("relax")
}

// This crate's source stamp (build.rs, libs/relax/self_stamp.rs): relax-compiler folds it into the codegen identity.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub unsafe fn reset_statics() {
    arena::reset_statics();
    boot_stats::reset_statics();
    clock::reset_statics();
    diag::reset_statics();
    preempt::reset_statics();
    progress::reset_statics();
    query::reset_statics();
    relaxed::reset_statics();
    solver_work::reset_statics();
    vfs::reset_statics();
    why::reset_statics();
    zone_alloc::reset_statics();
}
