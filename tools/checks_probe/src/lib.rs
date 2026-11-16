//! A std-only probe of the profile every JIT test run devhub drives builds: `devhub test`, the
//! land gate and the tests bucket run it as `makepad test makepad-checks-probe --profile
//! release-checks` (libs/devhub lease/mod.rs JIT_TEST_PROFILE), release with cargo test's checks.
//! A run that drops the profile, or a JIT that drops its checks, is red here.

#[cfg(test)]
mod tests {
    #[test]
    fn devhub_tests_run_with_debug_assertions_and_overflow_checks() {
        assert!(cfg!(debug_assertions), "debug assertions are off: the run lacks the release-checks profile");
        let max: u8 = std::hint::black_box(255);
        let wrapped = std::panic::catch_unwind(|| max + std::hint::black_box(1));
        assert!(wrapped.is_err(), "255u8 + 1 did not panic: overflow checks are off (it gave {wrapped:?})");
    }
}
