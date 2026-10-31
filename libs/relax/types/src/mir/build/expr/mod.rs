//! rustc_mir_build/src/builder/expr/*: expression lowering entry points. Frozen seam (I1): the signatures
//! are what `matches/`, statement and block lowering call. One file per rustc file; owner per file:
//! lane B: `as_place`, `as_temp`, `stmt`, `category`; lane E: `as_operand`, `as_rvalue`, `as_constant`, `into`.
pub mod as_constant;
pub mod as_operand;
pub mod as_place;
pub mod as_rvalue;
pub mod as_temp;
pub mod into;
pub mod stmt;
pub mod category;
