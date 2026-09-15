pub mod archive;
pub mod drape;
pub mod geometry;
pub(crate) mod icons;
pub(crate) mod label;
pub mod overlay;
pub mod style;
pub mod tile;
pub mod view;

pub use overlay::{MapMarker, MapPuck, MapRouteOverlay};
pub use view::*;

fn warm_shared_registries() {
    icons::warm_icon_registries();
    tile::warm_tile_registries();
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod bake_report;
