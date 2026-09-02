use makepad_widgets::{Cx, CxOsApi};

/// Demo monotonic clock. Keep this indirection until `Cx::monotonic_now()`
/// is available on every platform.
pub fn monotonic_now(cx: &Cx) -> f64 {
    cx.seconds_since_app_start()
}
