//! makepad-geodata: bulk open-geodata fetching and per-layer overlay database
//! building for the map stack.
//!
//! Every overlay layer becomes its own .mbtiles file (never merged into the
//! base map archive), built from bulk-downloadable open datasets only. The
//! fetch module enforces the politeness rules; the tiler writes standard
//! gzipped MVT the renderer's existing decoder already understands.
//!
//! This is a library so the maps app can embed the same fetch/cache/build
//! machinery later for periodically synced sources (e.g. the NDW charger
//! file); the `geodata` binary is a thin CLI over it.

#[cfg(not(target_arch = "wasm32"))]
mod clock;
#[cfg(not(target_arch = "wasm32"))]
mod http_fetch;

#[cfg(not(target_arch = "wasm32"))]
pub mod fetch;
pub mod geo;
pub mod gpkg;
#[cfg(not(target_arch = "wasm32"))]
pub mod layers;
pub mod mvt;
pub mod png;
#[cfg(not(target_arch = "wasm32"))]
pub mod query;
pub mod knmi_hdf5;
pub mod radar_raster;
pub mod radar_volume;
pub mod terrain_shade;
#[cfg(not(target_arch = "wasm32"))]
pub mod wind;
#[cfg(not(target_arch = "wasm32"))]
pub mod radar;
#[cfg(not(target_arch = "wasm32"))]
pub mod raster;
#[cfg(not(target_arch = "wasm32"))]
pub mod sidecar;
pub mod tiff;
#[cfg(not(target_arch = "wasm32"))]
pub mod spool;
#[cfg(not(target_arch = "wasm32"))]
pub mod tiler;
pub mod wkb;

#[cfg(not(target_arch = "wasm32"))]
pub use fetch::{fetch_source, FetchOptions, FetchOutcome, SourceSpec};
#[cfg(not(target_arch = "wasm32"))]
pub use layers::{find_layer, registry, BuildCtx, BuildReport, Layer};
