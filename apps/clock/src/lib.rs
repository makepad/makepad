//! clock as a library: the analog face widget, the time wheel, the alarm
//! book and its list widget, local time without a timezone database, and
//! — for a host that links it — the module ([`module`]): what the window
//! manager (and the web superbuild) seats in a tile IN-PROCESS, one
//! instance per isolate, without a `Window` or a process. The standalone
//! binary (`src/main.rs`, feature `standalone`) is the same crate with a
//! `Window` around `ClockView{}` and the F10 overlay.

pub use makepad_widgets;

pub mod ai;
pub mod alarm;
pub mod alarm_list;
pub mod digits;
pub mod face;
pub mod laps;
pub mod local_time;
pub mod module;
pub mod ring;
pub mod tabs;
pub mod view;
pub mod wheel;

pub use module::{ClockModule, CLOCK_MODULE};
pub use view::ClockView;
