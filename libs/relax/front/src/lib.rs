//! Relax's front end: parsing, the crate graph and name resolution (program), macro expansion (macros, preload,
//! derive, builtins, fmtmac, macro_reg) and immutable Rust source contexts (source_context). Builds on relax-base.
//! The layers below are re-exported, so every module of Relax is `crate::<module>` in each layer.

#![recursion_limit = "4096"]

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

pub use relax_base::*;

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

// This crate's source stamp (build.rs, libs/relax/self_stamp.rs): relax-compiler folds it into the codegen identity.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));
