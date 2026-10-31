//! Stage's DSP, now a re-export of Makepad's shared synthesis crate
//! (`libs/audio_synth`), where the Ironfish synth and the SVF moved so the
//! games and Stage share one synth system.
//!
//! [`ironfish`] is the self-contained Ironfish synth; [`svf`] is the
//! trapezoidal state-variable filter; [`loudness`] is the one BS.1770
//! loudness meter; [`grain`] is the two-grain time stretch.

pub use makepad_audio_synth::{ironfish, svf};

pub mod grain;
pub mod loudness;
