//! Floating-point control flags for flush-to-zero and exception handling.

// This is an RAII structure that enables flushing denormal numbers
// to zero, and automatically resetting previous flags once it is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FlushToZeroDenormalsAreZeroFlags {
    original_flags: u32,
}

impl FlushToZeroDenormalsAreZeroFlags {
    #[cfg(not(all(
        not(feature = "enhanced-determinism"),
        any(target_arch = "x86_64", target_arch = "x86"),
        target_feature = "sse"
    )))]
    pub fn flush_denormal_to_zero() -> Self {
        Self { original_flags: 0 }
    }

    #[cfg(all(
        not(feature = "enhanced-determinism"),
        any(target_arch = "x86", target_arch = "x86_64"),
        target_feature = "sse"
    ))]
    #[allow(deprecated)] // will address that later.
    pub fn flush_denormal_to_zero() -> Self {
        unsafe {
            #[cfg(target_arch = "x86")]
            use std::arch::x86::{_MM_FLUSH_ZERO_ON, _mm_getcsr, _mm_setcsr};
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::{_MM_FLUSH_ZERO_ON, _mm_getcsr, _mm_setcsr};

            // Flush denormals & underflows to zero as this as a significant impact on the solver's performances.
            // To enable this we need to set the bit 15 (given by _MM_FLUSH_ZERO_ON) and the bit 6 (for denormals-are-zero).
            // See https://software.intel.com/content/www/us/en/develop/articles/x87-and-sse-floating-point-assists-in-ia-32-flush-to-zero-ftz-and-denormals-are-zero-daz.html
            let original_flags = _mm_getcsr();
            _mm_setcsr(original_flags | _MM_FLUSH_ZERO_ON | (1 << 6));
            Self { original_flags }
        }
    }
}

#[cfg(all(
    not(feature = "enhanced-determinism"),
    any(target_arch = "x86", target_arch = "x86_64"),
    target_feature = "sse"
))]
impl Drop for FlushToZeroDenormalsAreZeroFlags {
    #[allow(deprecated)] // will address that later.
    fn drop(&mut self) {
        #[cfg(target_arch = "x86")]
        unsafe {
            std::arch::x86::_mm_setcsr(self.original_flags)
        }
        #[cfg(target_arch = "x86_64")]
        unsafe {
            std::arch::x86_64::_mm_setcsr(self.original_flags)
        }
    }
}

/// This is an RAII structure that disables floating point exceptions while
/// it is alive, so that operations which generate NaNs and infinite values
/// intentionally will not trip an exception when debugging problematic
/// code that is generating NaNs and infinite values erroneously.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DisableFloatingPointExceptionsFlags {
    #[cfg(feature = "debug-disable-legitimate-fe-exceptions")]
    // We can't get a precise size for this, because it's of type
    // `fenv_t`, which is a definition that doesn't exist in rust
    // (not even in the libc crate, as of the time of writing.)
    // But since the state is intended to be stored on the stack,
    // 256 bytes should be more than enough.
    original_flags: [u8; 256],
}

#[cfg(feature = "debug-disable-legitimate-fe-exceptions")]
extern "C" {
    fn feholdexcept(env: *mut std::ffi::c_void);
    fn fesetenv(env: *const std::ffi::c_void);
}

impl DisableFloatingPointExceptionsFlags {
    #[cfg(not(feature = "debug-disable-legitimate-fe-exceptions"))]
    #[allow(dead_code)]
    /// Disables floating point exceptions as long as this object is not dropped.
    pub fn disable_floating_point_exceptions() -> Self {
        Self {}
    }

    #[cfg(feature = "debug-disable-legitimate-fe-exceptions")]
    /// Disables floating point exceptions as long as this object is not dropped.
    pub fn disable_floating_point_exceptions() -> Self {
        unsafe {
            let mut original_flags = [0; 256];
            feholdexcept(original_flags.as_mut_ptr() as *mut _);
            Self { original_flags }
        }
    }
}

#[cfg(feature = "debug-disable-legitimate-fe-exceptions")]
impl Drop for DisableFloatingPointExceptionsFlags {
    fn drop(&mut self) {
        unsafe {
            fesetenv(self.original_flags.as_ptr() as *const _);
        }
    }
}
