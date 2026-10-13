//! core::arch: native vector lowering for hot 128-bit SIMD operations, with scalar
//! implementations for the remaining APIs. Differential tests compare against stdarch.

pub mod aarch64;
pub mod x86_64;

pub mod x86 {
    pub use super::x86_64::*; // the same module under both names, as in core
}
