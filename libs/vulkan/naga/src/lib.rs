/*!
Naga can be used to translate source code written in one shading language to another.

# Example

The following example translates WGSL to SPIR-V.
It requires the features `"wgsl-in"` and `"spv-out"` to be enabled.

*/
// If we don't have the required front- and backends, don't try to build this example.
#![cfg_attr(all(feature = "wgsl-in", feature = "spv-out"), doc = "```")]
#![cfg_attr(not(all(feature = "wgsl-in", feature = "spv-out")), doc = "```ignore")]
/*!
let wgsl_source = "
@fragment
fn main_fs() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
";

// Parse the source into a Module.
let module: naga::Module = naga::front::wgsl::parse_str(wgsl_source)?;

// Validate the module.
// Validation can be made less restrictive by changing the ValidationFlags.
let module_info: naga::valid::ModuleInfo =
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .subgroup_stages(naga::valid::ShaderStages::all())
    .subgroup_operations(naga::valid::SubgroupOperationSet::all())
    .validate(&module)?;

// Translate the module.
let words = naga::back::spv::write_vec(
    &module,
    &module_info,
    &naga::back::spv::Options::default(),
    Some(&naga::back::spv::PipelineOptions {
        entry_point: "main_fs".into(),
        shader_stage: naga::ShaderStage::Fragment,
        allow_and_force_point_size: false,
        strict_capabilities: false,
    }),
)?;
assert!(!words.is_empty());

# Ok::<(), Box<dyn core::error::Error>>(())
```
*/

#![allow(
    clippy::new_without_default,
    clippy::unneeded_field_pattern,
    clippy::match_like_matches_macro,
    clippy::collapsible_if,
    clippy::derive_partial_eq_without_eq,
    clippy::needless_borrowed_reference,
    clippy::single_match,
    clippy::enum_variant_names,
    clippy::result_large_err
)]
#![warn(
    trivial_casts,
    trivial_numeric_casts,
    unused_extern_crates,
    unused_qualifications,
    clippy::pattern_type_mismatch,
    clippy::missing_const_for_fn,
    clippy::rest_pat_in_fully_bound_structs,
    clippy::match_wildcard_for_single_variants
)]
#![deny(clippy::exit)]
#![cfg_attr(
    not(test),
    warn(
        clippy::dbg_macro,
        clippy::panic,
        clippy::print_stderr,
        clippy::print_stdout,
        clippy::todo
    )
)]
#![no_std]

#[cfg(std)]
extern crate std;

extern crate alloc;
extern crate self as log;

pub use makepad_error_log;
use alloc::format;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[doc(hidden)]
pub fn __log_redirect(level: Level, args: core::fmt::Arguments<'_>) {
    match level {
        Level::Error => makepad_error_log::error!("{}", args),
        Level::Warn => makepad_error_log::warn!("{}", args),
        Level::Info => makepad_error_log::info!("{}", args),
        Level::Debug => makepad_error_log::debug!("{}", args),
        Level::Trace => makepad_error_log::trace!("{}", args),
    }
}

#[macro_export]
macro_rules! log {
    ($level:expr, $($arg:tt)+) => {{
        let __level = $level;
        if $crate::log_enabled!(__level) {
            $crate::__log_redirect(__level, format_args!($($arg)+));
        }
    }};
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)+) => {
        $crate::log!($crate::Level::Error, $($arg)+)
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)+) => {
        $crate::log!($crate::Level::Warn, $($arg)+)
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)+) => {
        $crate::log!($crate::Level::Info, $($arg)+)
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)+) => {
        $crate::log!($crate::Level::Debug, $($arg)+)
    };
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)+) => {
        $crate::log!($crate::Level::Trace, $($arg)+)
    };
}

#[macro_export]
macro_rules! log_enabled {
    ($level:expr) => {{
        match $level {
            $crate::Level::Error | $crate::Level::Warn => true,
            $crate::Level::Info | $crate::Level::Debug | $crate::Level::Trace => false,
        }
    }};
}

mod arena;
pub mod back;
pub mod common;
pub mod compact;
pub mod diagnostic_filter;
pub mod error;
pub mod front;
pub mod ir;
pub mod keywords;
mod non_max_u32;
pub mod proc;
mod racy_lock;
mod span;
pub mod valid;

use alloc::string::String;

pub use crate::arena::{Arena, Handle, Range, UniqueArena};
pub use crate::span::{SourceLocation, Span, SpanContext, WithSpan};

// TODO: Eliminate this re-export and migrate uses of `crate::Foo` to `use crate::ir; ir::Foo`.
pub use ir::*;

/// Width of a boolean type, in bytes.
pub const BOOL_WIDTH: Bytes = 1;

/// Width of abstract types, in bytes.
pub const ABSTRACT_WIDTH: Bytes = 8;

/// Hash map that is faster but not resilient to DoS attacks.
/// (Similar to rustc_hash::FxHashMap but using hashbrown::HashMap instead of alloc::collections::HashMap.)
/// To construct a new instance: `FastHashMap::default()`
pub type FastHashMap<K, T> =
    hashbrown::HashMap<K, T, core::hash::BuildHasherDefault<rustc_hash::FxHasher>>;

/// Hash set that is faster but not resilient to DoS attacks.
/// (Similar to rustc_hash::FxHashSet but using hashbrown::HashSet instead of alloc::collections::HashMap.)
pub type FastHashSet<K> =
    hashbrown::HashSet<K, core::hash::BuildHasherDefault<rustc_hash::FxHasher>>;

/// Insertion-order-preserving hash set (`IndexSet<K>`), but with the same
/// hasher as `FastHashSet<K>` (faster but not resilient to DoS attacks).
pub type FastIndexSet<K> =
    indexmap::IndexSet<K, core::hash::BuildHasherDefault<rustc_hash::FxHasher>>;

/// Insertion-order-preserving hash map (`IndexMap<K, V>`), but with the same
/// hasher as `FastHashMap<K, V>` (faster but not resilient to DoS attacks).
pub type FastIndexMap<K, V> =
    indexmap::IndexMap<K, V, core::hash::BuildHasherDefault<rustc_hash::FxHasher>>;

/// Map of expressions that have associated variable names
pub(crate) type NamedExpressions = FastIndexMap<Handle<Expression>, String>;
