// Run with `makepad test` and rustc -O --test on each native architecture.
#![allow(dead_code, unused_macros, non_snake_case)]
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64 as hr;
#[cfg(target_arch = "aarch64")]
use core::arch::aarch64 as hr;
include!("../std/check_arch/tests/common/mod.rs");
include!("../std/check_arch/tests/common/differential.rs");
#[allow(dead_code)]
mod sha256_oracle {
    include!("../std/check_arch/tests/common/sha256_oracle.rs");
}
include!("../std/check_arch/tests/common/sha256_cases.rs");
