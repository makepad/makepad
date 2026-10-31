//! Differential checks for Relax's core::arch modules.
//!
//! The integration tests use 4,096 xorshift inputs plus 17 edge patterns and
//! test immediate boundaries (all selectors for shuffles, blends and permutes).
//! Native x86 calls are feature-gated. Three literal Intel-pseudocode cases per
//! function also run on hosts lacking the feature, including ARM hosts.
//! ARM hosts additionally check the x86 SHA implementation against native NEON SHA.
//! Run the bridge-backed modules with Relax; the legacy rustc harness still needs
//! host adapters for the general SIMD/runtime bridges.
//!
//! `_MM_SHUFFLE` is unstable in rustc 1.98, so stable runs use its documented
//! scalar definition. `--features check-unstable-shuffle` enables the actual
//! stdarch comparison on nightly, or with `RUSTC_BOOTSTRAP=x86_64` for that test
//! crate only. This opt-in changes tests, not the emulated core modules.

pub use core::ptr;
// The source modules now call the compiler SIMD/runtime bridges. Expose those
// when this harness is run by Relax; rustc cannot implement relax-intrinsic ABI.
#[cfg(relax)]
pub use core::{intrinsics_rt, intrinsics_vx};
#[path = "../../core/arch/aarch64.rs"]
pub mod hr_aarch64;
#[path = "../../core/arch/x86_64.rs"]
pub mod hr_x86;
