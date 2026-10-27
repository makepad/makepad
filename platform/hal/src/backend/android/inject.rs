//! Input into the app from the host side (a Desk Link viewer's mouse and keyboard): events posted
//! to the app's windows as the Activity and the XR thread post them, app-level only (no
//! system-wide Android input). And the windows a host-side tap may show: the host window (the
//! XR panel on a Quest, the Activity's surface on a phone) and the XR stereo window.
//!
//! - Pointer events on the host window go where the Activity's touches and the XR aim rays go:
//!   to the app that borrows the host window (`Shared::emit`, like `xr::point`).
//! - Pointer events on the XR window go to that window of its session.
//! - Keys and text always go to the app that borrows the host window (where the Activity's key
//!   events go; Cx hands them to the focused window and its key focus).

use super::shell::{lock, Shared};
use crate::{clock_ns, session::HOST, Id};

/// One of the app's windows as the core presents it (what `crate::frame_tap::subscribe` takes),
/// its size in pixels and its pixels per window point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppWindow {
    pub session: u32,
    pub window: Id,
    pub px: [u32; 2],
    pub dpi: f32,
    /// The XR stereo window (both eyes side by side), not the host window.
    pub xr: bool,
}

/// An immersive build in an XR session: the host window is the XR panel.
pub fn immersive() -> bool {
    lock(&Shared::global().state).xr.is_some()
}

/// The host window: the XR panel in an immersive build, else the Activity's surface. `None`
/// before it exists (or while the surface has no size).
pub fn host_window() -> Option<AppWindow> {
    let shared = Shared::global();
    let (host, size) = {
        let r = lock(&shared.registry);
        (r.host?, r.state.size)
    };
    let (px, dpi) = match lock(&shared.state).xr {
        Some((px, dpi)) => (px, dpi),
        None => (size, shared.dpi()),
    };
    (px[0] > 0 && px[1] > 0).then_some(AppWindow { session: HOST, window: host, px, dpi, xr: false })
}

/// The XR stereo window, while an app has one (`window_kind::XR`).
pub fn xr_window() -> Option<AppWindow> {
    let (session, window) = lock(&Shared::global().registry).xr_window?;
    let eye = super::xr::eye_px()?;
    Some(AppWindow { session, window, px: [2 * eye[0], eye[1]], dpi: 1., xr: true })
}

/// A pointer event at window point (`x`, `y`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pointer {
    Move,
    /// `button`: Cx's mouse button mask (primary 1, secondary 2, middle 4, back 8, forward 16).
    Down(u32),
    Up(u32),
    /// Wheel motion in points (+dy: content moves down, as the Activity's wheel).
    Scroll { dx: f64, dy: f64 },
    /// The pointer left the window.
    Leave,
}

/// Posts `event` at (`x`, `y`) points of `window`, with modifiers `mods` (`input::MOD_*`).
pub fn pointer(window: &AppWindow, x: f64, y: f64, event: Pointer, mods: u32) {
    let post = |q: &mut crate::buffers::EventQueue, id: Id| match event {
        Pointer::Move => q.mouse_move(id, x, y, 0., 0., mods),
        Pointer::Down(button) => q.mouse_down(id, x, y, button, mods),
        Pointer::Up(button) => q.mouse_up(id, x, y, button, mods),
        Pointer::Scroll { dx, dy } => q.scroll(id, x, y, dx, dy, mods, false, crate::input::scroll::NONE),
        Pointer::Leave => q.mouse_leave(id, x, y),
    };
    let shared = Shared::global();
    if window.xr {
        shared.emit_to(window.session, window.window, clock_ns(), post);
    } else {
        // the host window's events go to the app that borrows it
        shared.emit(clock_ns(), post);
    }
}

/// A key (`key`: the makepad `KeyCode` index) down or up with modifiers `mods`; a key down
/// that types a character (no control/logo held) also types it, as the soft keyboard and a
/// physical one do.
pub fn key(down: bool, key: u32, repeat: bool, mods: u32) {
    let shared = Shared::global();
    if down {
        shared.emit(clock_ns(), |q, id| q.key_down(id, key, repeat, mods));
        let commands = mods & (crate::input::MOD_CONTROL | crate::input::MOD_LOGO) != 0;
        let typed = makepad_key_code::KeyCode::from_index(key).to_char(mods & crate::input::MOD_SHIFT != 0);
        if let Some(c) = typed.filter(|c| !commands && !c.is_control()) {
            let mut bytes = [0; 4];
            let text = c.encode_utf8(&mut bytes);
            shared.emit(clock_ns(), |q, id| q.text(id, crate::input::text::TYPED, text.as_bytes()));
        }
    } else {
        shared.emit(clock_ns(), |q, id| q.key_up(id, key, mods));
    }
}

/// Text typed into the app (a viewer's paste or composed text).
pub fn text(text: &str) {
    Shared::global().emit(clock_ns(), |q, id| q.text(id, crate::input::text::TYPED, text.as_bytes()));
}
