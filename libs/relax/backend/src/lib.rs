//! Relax's shared low-level back end: RIR ([`rir`], including its vector registers and ops), the
//! register allocator ([`regalloc`]), the host's machine-code emitter (`x64` on x86-64, `arm64` on
//! AArch64: each host builds only its own), jump-table lowering ([`switch`]), the C ABI planner ([`cabi`]) and executable
//! memory ([`exec`]), the passes every optimising tier runs ([`passes`]), with the dominator and block-order ([`dom`]), frame-layout ([`frame`]) and bit-op
//! ([`bitops`]) analyses the emitters run. Front ends: Relax's Rust (relax-codegen optimises
//! it first) and Splash kernels. [`probe`] lets a front end observe compile phases.

// Diagnostics never panic: std's print macros panic when a write fails (ENOSPC on the session
// log in a full tmpfs /tmp aborted a fleet Sandbox inside a lazy compile, 2026-10-23), so in this
// crate every print is a write whose error is dropped. Test builds keep std's captured macros.
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprintln {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! eprint {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stderr().write_fmt(format_args!($($t)+)); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! println {
    () => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_all(b"\n"); }};
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!("{}\n", format_args!($($t)+))); }};
}
#[cfg(not(test))]
#[allow(unused_macros)]
macro_rules! print {
    ($($t:tt)+) => {{ use ::std::io::Write as _; let _ = ::std::io::stdout().write_fmt(format_args!($($t)+)); }};
}

#[cfg(target_arch = "aarch64")]
pub mod arm64;
pub mod asm;
pub mod bitops;
pub mod cabi;
pub mod dbgloc;
pub mod dom;
pub mod exec;
pub mod frame;
pub mod legal;
pub mod merge;
pub mod pair256;
pub mod passes;
pub mod probe;
pub mod profile;
pub mod regalloc;
pub mod remark;
pub mod rir;
pub mod stats;
pub mod switch;
pub mod tail;
pub mod wasm;
#[cfg(not(target_arch = "aarch64"))]
pub mod x64;

// This crate's source stamp (build.rs, libs/relax/self_stamp.rs): relax-compiler folds it into the codegen identity.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub unsafe fn reset_statics() {
    dbgloc::reset_statics();
    passes::reset_statics();
    rir::reset_statics();
    stats::reset_statics();
}
