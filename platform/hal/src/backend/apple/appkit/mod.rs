//! The AppKit shell of the HAL: the macOS window system. The HAL owns the process main thread
//! and runs `NSApp` there ([`run_main_loop`]); every session's Cx runs on its own thread and
//! talks to the shell only through the HAL: window and OS commands become [`Op`]s the main
//! thread applies, input and window events become `RawEvent`s the pumping thread collects
//! ([`Shared::drain_events`]). Nothing here waits for the other side: the main thread never
//! takes the HAL's state lock, the pumping thread never calls AppKit.
//!
//! One `CAMetalLayer` per window: the Metal backend renders into the layer's drawables and
//! presents them ([`Shared::layer`]); a display link per window announces vsyncs, but only for a
//! window that asked for a frame (`FrameRequest`), so an idle app costs no wake-ups. OS nested
//! loops (live resize, menu tracking, modal panels) block the main thread only: the sessions
//! keep running and presenting, and ops keep arriving because they ride the main dispatch
//! queue, which runs in every run-loop mode.
//!
//! Moved from os/apple/macos/{macos_app, macos_window, macos_delegates, macos_event, macos_ime}.rs
//! and the key tables of apple_util.rs; the producer side of `cocoa_event_callback` (what became
//! of an NSEvent) writes `RawEvent`s here, the fingers / hover / tap logic is Cx's
//! (`hal_intake`). The shell-neutral half (ops, the event inbox, the main-loop state) is
//! `super::shell`, shared with the UIKit shell.

mod app;
mod classes;
mod clipboard;
mod ime;
mod keys;
mod main_lag;
mod screen;
mod test_resize;
mod window;

pub use app::{host_dark, run_main_loop, stop_main_loop};
pub(crate) use app::wake_idle_main;
use super::shell::{lock, log_enabled, Op, Shared};

/// What `super::shell` needs of AppKit.
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

    /// A frame reached the screen: the live-resize test counts it.
    pub fn presented() {
        super::test_resize::paint()
    }
}

