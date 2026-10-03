//! Optimiser hints.

pub unsafe fn unreachable_unchecked() -> ! {
    crate::intrinsics_rt::unreachable()
}

pub fn black_box<T>(dummy: T) -> T {
    unsafe { crate::intrinsics_mem::black_box(dummy) }
}

pub fn spin_loop() {
    unsafe { crate::intrinsics_atomic::spin_loop_hint() }
}

pub unsafe fn assert_unchecked(cond: bool) {
    if !cond {
        crate::intrinsics_rt::unreachable()
    }
}

pub fn must_use<T>(value: T) -> T {
    value
}
