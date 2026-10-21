//! The night skyline: a 360-degree strip of land, mountains, city, trees
//! and props, rendered on the GPU (see `render::Skyline`). Used by the
//! skyline example and as the HDRI generator's skyline layer.
pub use makepad_widgets;
use makepad_widgets::*;

pub mod params;
pub mod rng;
pub mod zones;
pub mod terrain;
pub mod city;
pub mod props;
pub mod shaders;
pub mod post;
pub mod render;
pub mod export;

pub use export::{Band, BandDone, ExportKind};
pub use params::{ParamDef, Params, COUNT, P, PARAMS, SEED_MAX};
pub use render::{SceneTarget, Skyline, ViewSpec};

/// Registers the skyline's shaders and its `mod.draw.Skyline`, in the
/// order they depend on each other. Call after makepad_draw's own.
pub fn script_mod(vm: &mut ScriptVm) {
    shaders::script_mod(vm);
    terrain::script_mod(vm);
    city::script_mod(vm);
    props::kit::script_mod(vm);
    props::script_mod(vm);
    post::script_mod(vm);
    render::script_mod(vm);
}

/// The strip's elevation range in degrees, (top, bottom): rows are linear
/// in elevation, the horizon sits `horizon` of the way down.
pub fn band_range_deg(p: &Params) -> (f64, f64) {
    let vfov = p[P::Vfov];
    let horizon = p[P::Horizon];
    (horizon * vfov, -(1.0 - horizon) * vfov)
}
