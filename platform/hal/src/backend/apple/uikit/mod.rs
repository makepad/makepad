//! The UIKit shell of the HAL: the iOS and tvOS window system. The HAL owns the process main
//! thread and runs `UIApplicationMain` there ([`run_main_loop`]); the shell-neutral half (the
//! ops sessions send, the events the pumping thread collects, the main-loop state) is
//! `super::shell`, shared with the AppKit shell, and the GPU work is the shared Metal core.
//!
//! The app has one `UIWindow` filling the screen. Its root view is the first normal window a
//! session creates; another normal window is a full-screen view above it, a popup a view at its
//! place over its parent. Each of these views is backed by its own `CAMetalLayer`, which the
//! Metal backend renders into ([`Shared::layer`]). A `CADisplayLink` per window announces vsyncs,
//! but only for a window that asked for a frame (`FrameRequest`), so an idle app costs no
//! wake-ups.
//!
//! Moved from os/apple/ios/{ios_app, ios_delegates, ios_text_input, ios_file_dialog}.rs and
//! os/apple/tvos/*: what used to call into Cx writes `RawEvent`s for the session now; the
//! fingers / tap / hit logic is Cx's (`hal_intake`).

mod app;
mod classes;
#[cfg(target_os = "ios")]
mod dialog;
#[cfg(target_os = "ios")]
mod keys;
#[cfg(target_os = "ios")]
mod menu;
#[cfg(target_os = "ios")]
mod text;
#[cfg(target_os = "ios")]
mod test_rotate;
mod window;

pub use app::{host_dark, run_main_loop, stop_main_loop};
use super::shell::{lock, log_enabled, Op, Shared};

/// What `super::shell` needs of UIKit.
pub(super) mod native {
    use crate::types::Id;
    use makepad_apple_sys::ObjcId;

    pub fn run_on_main(f: impl Fn() + Send + 'static) {
        super::app::run_on_main(f)
    }

    pub fn schedule_drain() {
        super::app::schedule_drain()
    }

    pub fn wake_event_loop() {
        super::app::wake_event_loop()
    }

    pub fn window_view(sid: u32, id: Id) -> Option<ObjcId> {
        super::app::app().window_view(sid, id)
    }

    /// A frame reached the screen: the rotation test counts it.
    pub fn presented() {
        #[cfg(target_os = "ios")]
        super::test_rotate::presented()
    }
}
