use std::ptr;

use hilog_sys as hilog;

#[test]
fn link_smoke() {
    unsafe {
        let _ = hilog::OH_LOG_IsLoggable(0, ptr::null(), hilog::LogLevel::LOG_DEBUG);
    }
}
