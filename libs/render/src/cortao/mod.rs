//! Circular Occluder Ray-Traced Ambient Occlusion (CORTAO) approximates nearby
//! geometry with discs and traces hemisphere rays against them in screen space.
//! This module is the shared screen-space AO facility for every renderer:
//! camera and pass parameters (`params`), shared occluder tables (`tables`),
//! and a three-stage GPU chain (`pass`, `shaders`).

pub mod params;
pub mod pass;
pub mod shaders;
pub mod tables;

pub use params::*;
pub use pass::*;

/// Register the CORTAO draw types after `makepad_draw::script_mod`.
pub use shaders::script_mod;
