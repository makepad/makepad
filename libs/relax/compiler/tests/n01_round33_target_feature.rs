// L7: #[target_feature] collection (implied features) and adjust_target_feature_sig, as rustc.
#[cfg(target_arch = "x86_64")]
mod x {
    #[target_feature(enable = "avx2")]
    fn avx2_fn() -> u32 { 2 }
    #[target_feature(enable = "avx")]
    fn avx_fn() -> u32 { 1 }
    // avx is implied by avx2: both coerce to safe fn pointers inside an avx2 body
    #[target_feature(enable = "avx2")]
    fn caller() -> u32 {
        let f: fn() -> u32 = avx_fn;
        let g: fn() -> u32 = avx2_fn;
        let c = || { let h: fn() -> u32 = avx_fn; h() };
        f() + g() + c() - 1
    }
    #[target_feature(enable = "sse4.2,popcnt", enable = "bmi1")]
    unsafe fn multi() -> u32 { 3 }
    pub fn run() -> u32 {
        // a safe #[target_feature] fn is an unsafe fn pointer anywhere
        let u: unsafe fn() -> u32 = avx2_fn;
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("popcnt") && std::is_x86_feature_detected!("bmi1") {
            unsafe { caller() + u() + multi() - 2 }
        } else { 6 }
    }
}
#[cfg(not(target_arch = "x86_64"))]
mod x { pub fn run() -> u32 { 6 } }

#[test]
fn target_feature_safety() { assert_eq!(x::run(), 6); }
