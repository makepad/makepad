//! Compile-fail: fixing value-only intrinsics must not make pointer loads safe.
// rejects: E0133
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(target_arch = "x86_64")]

#[target_feature(enable = "sse2")]
unsafe fn load(bytes: &[u8; 16]) {
    let _ = std::arch::x86_64::_mm_loadu_si128(bytes.as_ptr().cast());
}

fn main() {}
