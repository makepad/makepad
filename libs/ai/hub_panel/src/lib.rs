//! The AI hub control panel (the AI Hub app draws it) and the control API client a makepad WM
//! node uses to tell its hub what only the WM knows (`hub_control::FleetHub`).

pub mod hub_control;
pub mod hub_glow;
mod panel;
pub mod tray_glyph;

pub use panel::{HubHit, HubPanel};
