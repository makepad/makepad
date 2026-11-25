// rejects: invalid argument to a legacy const generic: cannot have const blocks, closures, async blocks or items
#![cfg(target_arch = "x86_64")]
use std::arch::x86_64::{_mm_slli_epi32 as shift, _mm_set1_epi32};

// (a plain fn, not a #[test]: `cargo check` of the crate, without cfg(test), rejects it too)
fn nested_definition_arguments() {
    unsafe {
        let a = _mm_set1_epi32(3);
        let _ = shift(a, (|| 2)());
        let _ = shift(a, const { 2 });
        let _ = shift(a, { const N: i32 = 2; N });
        let _ = shift(a, [2; 1][0]);
    }
}
