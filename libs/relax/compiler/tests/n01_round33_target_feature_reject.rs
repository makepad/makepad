// L7: rustc rejects each item below (x86_64 target). Checked as a package with `makepad check`.
#![allow(dead_code)]
#[target_feature(enable = "avx2")] fn avx2_fn() {}
// no avx2 here: TargetFeatureCast -> mismatched types
fn no_features() { let _f: fn() = avx2_fn; }
// not a Rust target feature
#[target_feature(enable = "avx512")] unsafe fn bogus() {}
// unstable target feature (x87_target_feature)
#[target_feature(enable = "x87")] unsafe fn unstable() {}
// internal-only
#[target_feature(enable = "soft-float")] unsafe fn internal() {}
// safe trait method
trait Tr { fn m(&self); }
impl Tr for u8 { #[target_feature(enable = "avx")] fn m(&self) {} }
// the attribute value is split at commas only: " sse2" is not a feature
#[target_feature(enable = "avx, sse2")] unsafe fn spaced() {}
