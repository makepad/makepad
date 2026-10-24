//! Relax's front end: parsing, the crate graph and name resolution (program), macro expansion (macros, preload,
//! derive, builtins, fmtmac, macro_reg) and immutable Rust source contexts (source_context). Builds on relax-base.
//! The layers below are re-exported, so every module of Relax is `crate::<module>` in each layer.

#![recursion_limit = "4096"]

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
