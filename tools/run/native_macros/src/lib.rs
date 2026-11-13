//! Makepad's native proc-macro set: the script and widget derives run natively in the `makepad`
//! binary, which registers `SET` (tools/run lib.rs `HOST`). The `relax` binary links none of it, so
//! there these derives are JIT-compiled and hosted like any proc macro.
use relax_native_macros::{Entry, Function, Package, Set};

mod bindings {
    // the wrappers, adapted from the embedded sources (relax-native-macros gen.rs)
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
// This crate's source stamp (relax-native-macros gen.rs, libs/relax/self_stamp.rs): the codegen
// identity holds it once the set is registered.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));
// The set's packages, as the build read them.
include!(concat!(env!("OUT_DIR"), "/sources.rs"));

/// Makepad's set.
pub static SET: Set = Set { name: "makepad", packages: PACKAGES, entries: bindings::ENTRIES, stamp: SRC_STAMP };
