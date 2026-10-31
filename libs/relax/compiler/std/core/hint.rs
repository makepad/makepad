//! Optimiser hints.

pub const unsafe fn unreachable_unchecked() -> ! {
    crate::intrinsics_rt::unreachable()
}

pub const fn black_box<T>(dummy: T) -> T {
    unsafe { crate::intrinsics_mem::black_box(dummy) }
}

pub fn spin_loop() {
    unsafe { crate::intrinsics_atomic::spin_loop_hint() }
}

pub const unsafe fn assert_unchecked(cond: bool) {
    if !cond {
        crate::intrinsics_rt::unreachable()
    }
}

pub const fn must_use<T>(value: T) -> T {
    value
}

/// `if condition { true_val } else { false_val }` with both evaluated and the other one dropped;
/// real core lowers it to a branchless select (cmov/csel). Relax: a plain select for now.
pub const fn select_unpredictable<T>(condition: bool, true_val: T, false_val: T) -> T {
    if condition { true_val } else { false_val }
}
