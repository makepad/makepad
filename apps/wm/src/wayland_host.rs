//! Foreign Wayland windows. The WM serves the Wayland protocol itself
//! (libs/wayland_server, no libwayland) and shows each client's toplevel in
//! a tile (`wayland_view.rs`). Everything here is a side track of the WM:
//! the layout, binds and shell see an ordinary client slot.
//!
//! This file is the platform-neutral surface the tile uses; the Linux part
//! (server, textures, process launch) is in `linux.rs`, other systems get
//! `other.rs` where nothing ever maps.

use makepad_app_host::ClientId;
use makepad_widgets::Texture;

#[cfg(target_os = "linux")]
mod catalog;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
use other as imp;

/// (wayland client, wl_surface id).
pub type Surf = (u32, u32);

/// One surface of a window, positioned in window-geometry coordinates.
pub struct Layer {
    pub surf: Surf,
    /// Top-left of the surface, relative to the tile origin (the main window's geometry origin).
    pub pos: (i32, i32),
    /// The texture's size, logical px (buffer scale 1).
    pub size: (i32, i32),
    /// The part of the texture shown (viewport crop), normalised: x, y, w, h.
    pub uv: (f32, f32, f32, f32),
    pub opaque: bool,
    pub popup: bool,
    pub texture: Texture,
}

/// What a tile asks the host to do.
pub enum ViewCmd {
    /// Tell the window its tile size; `activated` when it has the keyboard.
    Configure { client: ClientId, w: i32, h: i32, activated: bool },
    KeyboardFocus(Option<Surf>),
    PointerEnter { surf: Surf, x: f64, y: f64 },
    PointerLeave,
    PointerMotion { x: f64, y: f64 },
    PointerButton { button: u32, pressed: bool },
    PointerAxis { axis: u32, value: f64, v120: i32 },
    Key { evdev: u16, pressed: bool },
    DismissPopups(ClientId),
    /// The display scale (dpi factor): clients are told it as their preferred scale.
    Scale(f64),
    /// Screen rectangle in tile coordinates, so popups are placed inside the screen.
    PopupBounds { x: i32, y: i32, w: i32, h: i32 },
    /// A makepad app copied text (`primary`: selected it): the Wayland clients' selection.
    Selection { primary: bool, text: String },
}

/// The surfaces to draw for a window, bottom to top (the window's own tree, then popups).
pub fn layers(client: ClientId) -> Vec<Layer> { imp::layers(client) }
/// The window's toplevel surface once it is mapped.
pub fn main_surface(client: ClientId) -> Option<Surf> { imp::main_surface(client) }
pub fn send(cmd: ViewCmd) { imp::send(cmd) }
/// Frame callbacks owed to the surfaces of `client` that got new content since the last call.
pub fn frames_presented(client: ClientId) { imp::frames_presented(client) }
/// True for a client id this module created a window for.
pub fn owns(client: ClientId) -> bool { imp::owns(client) }
/// The cursor shape (wp_cursor_shape_v1 number, 0 = none asked) the window's client wants.
pub fn cursor_shape(client: ClientId) -> u32 { imp::cursor_shape(client) }

/// The foreign apps the launcher lists: (launcher id `wl.<name>`, label).
pub fn menu_apps() -> Vec<(String, String)> { imp::menu_apps() }
/// The icon PNG of a listed foreign app.
pub fn icon_file(id: &str) -> Option<std::path::PathBuf> { imp::icon_file(id) }
