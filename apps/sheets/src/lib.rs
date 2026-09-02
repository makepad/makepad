//! sheets as a library: the spreadsheet widget, its engine, its one AI
//! tool, and — for a host that links it — the module ([`module`]): what
//! the window manager (and the web superbuild) seats in a tile
//! IN-PROCESS, one instance per isolate, without a `Window` or a process.
//! The standalone binary (`src/main.rs`, feature `standalone`) is the same
//! crate with a `Window` around `MpSheets{}` and the F10 overlay.

pub use makepad_widgets;

pub mod ai;
pub mod docs;
pub mod formula;
pub mod module;
pub mod sheet;
pub mod theme;
pub mod view;

pub use module::{SheetsModule, SHEETS_MODULE};
