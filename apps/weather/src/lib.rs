//! weather as a library: the Open-Meteo model, the sky artwork widget,
//! and — for a host that links it — the module ([`module`]): what the
//! window manager (and the web superbuild) seats in a tile IN-PROCESS,
//! one instance per isolate, without a `Window` or a process. The
//! standalone binary (`src/main.rs`, feature `standalone`) is the same
//! crate with a `Window` around `WeatherView{}` and the F10 overlay.

pub use makepad_widgets;

pub mod ai;
pub mod daily;
pub mod hourly;
pub mod model;
pub mod module;
pub mod parts;
pub mod sky;
pub mod view;

pub use module::{WeatherModule, WEATHER_MODULE};
pub use view::WeatherView;
