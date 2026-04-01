use {
    crate::{makepad_math::*, window::WindowId}, //makepad_microserde::*,
    std::cell::Cell,
    std::rc::Rc,
};


/// Safe area insets describing regions of the screen that should not contain
/// interactive content (e.g., notch/Dynamic Island, home indicator, rounded corners).
/// Values are in logical points (not physical pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SafeAreaInsets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowGeom {
    pub dpi_factor: f64,
    pub can_fullscreen: bool,
    pub xr_is_presenting: bool,
    pub is_fullscreen: bool,
    pub is_topmost: bool,
    pub position: Vec2d,
    pub inner_size: Vec2d,
    pub outer_size: Vec2d,
    /// Safe area insets for this window (non-zero on devices with notches,
    /// rounded corners, home indicators, etc.)
    pub safe_area_insets: SafeAreaInsets,
    /// Bounding box of the window-chrome buttons drawn by this window, in logical
    /// pixels with a top-left origin at the top-left corner of the content view
    /// (Y increases downward — Makepad's standard coordinate system).
    ///
    /// **Per-platform values:**
    /// - **macOS** — bounding box of the three traffic-light buttons (close /
    ///   miniaturize / zoom), queried live from the OS via `standardWindowButton:`.
    ///   Buttons sit on the left side of the title bar.
    /// - **Windows** — bounding box of the three Makepad-drawn caption buttons
    ///   (minimize / maximize / close), each 46 × 29 logical px, right-aligned at
    ///   the top of the caption bar.
    /// - **Linux / Wayland with `custom_window_chrome`** — same button layout as
    ///   Windows (right-aligned, 138 × 29 logical px).
    /// - **All other platforms** (X11 with WM decorations, LinuxDirect, Android,
    ///   iOS, Web, …) — zero rect, because the platform either provides its own
    ///   chrome outside the content area or has no title bar at all.
    ///
    /// **How to use this:**
    /// When drawing custom content inside a caption bar (e.g., a title label,
    /// search field, or toolbar), use this rect to determine which region is
    /// occupied by chrome buttons so you can apply the necessary margins and avoid
    /// overdrawing the buttons.  On macOS the occupied region is on the left; on
    /// Windows / Wayland it is on the right.  A zero rect means there are no
    /// chrome buttons to avoid.
    pub window_chrome_buttons: Rect,
}

#[derive(Clone, Debug)]
pub struct WindowGeomChangeEvent {
    pub window_id: WindowId,
    pub old_geom: WindowGeom,
    pub new_geom: WindowGeom,
}

#[derive(Clone, Debug)]
pub struct WindowMovedEvent {
    pub window_id: WindowId,
    pub old_pos: Vec2d,
    pub new_pos: Vec2d,
}

#[derive(Clone, Debug)]
pub struct WindowCloseRequestedEvent {
    pub window_id: WindowId,
    pub accept_close: Rc<Cell<bool>>,
}

#[derive(Clone, Debug)]
pub struct WindowClosedEvent {
    pub window_id: WindowId,
}

#[derive(Clone, Debug)]
pub enum PopupDismissReason {
    FocusLost,
    OutsideClick,
    Escape,
    Compositor,
    ParentClosed,
}

/// Notification that a popup window should be closed.
///
/// The app **must** call `WindowHandle::close()` to actually close the popup.
/// The framework does not auto-close popup windows on dismissal.
///
/// On Wayland the compositor may force-close the surface (`PopupDone`); in
/// that case `PopupDismissed` fires after the surface is already gone.
///
/// Common reasons: `OutsideClick`, `FocusLost`, `Escape`, `Compositor`,
/// `ParentClosed`.
#[derive(Clone, Debug)]
pub struct PopupDismissedEvent {
    pub window_id: WindowId,
    pub reason: PopupDismissReason,
}
/*
#[derive(Clone, Debug)]
pub struct WindowResizeLoopEvent {
    pub was_started: bool,
    pub window_id: WindowId
}*/

#[derive(Clone, Debug, Copy)]
pub enum WindowDragQueryResponse {
    NoAnswer,
    Client,
    Caption,
    SysMenu, // windows only
}

#[derive(Clone, Debug)]
pub struct WindowDragQueryEvent {
    pub window_id: WindowId,
    pub abs: Vec2d,
    pub response: Rc<Cell<WindowDragQueryResponse>>,
}
