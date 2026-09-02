//! photos as a library: the picture wall widget around `TileGrid`, the
//! library choice, the tools the assistant calls, and — for a host that
//! links it — the module ([`module`]): what the window manager (and the
//! web superbuild later) seats in a tile IN-PROCESS, one instance per
//! isolate, without a `Window` or a process. The standalone binary
//! (`src/main.rs`, feature `standalone`) is the same crate with a `Window`
//! around `PhotosView{}` and the F10 overlay.

pub use makepad_widgets;

pub mod ai;
pub mod library;
pub mod module;
pub mod view;

pub use module::{PhotosModule, PHOTOS_MODULE};
pub use view::PhotosView;
