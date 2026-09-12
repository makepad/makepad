pub use makepad_apple_sys::*;

pub(crate) mod ui_hang_sys;

/// Foundation `NSActivityIdleSystemSleepDisabled` (`1ULL << 20`).
#[allow(non_upper_case_globals)]
pub const NSActivityIdleSystemSleepDisabled: u64 = 1 << 20;
/// Foundation `NSActivityUserInitiated` (`0x00FFFFFFULL | NSActivityIdleSystemSleepDisabled`).
#[allow(non_upper_case_globals)]
pub const NSActivityUserInitiated: u64 = 0x00FFFFFF | NSActivityIdleSystemSleepDisabled;
