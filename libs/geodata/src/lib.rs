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

pub mod fetch;
pub mod geo;
pub mod gpkg;
pub mod layers;
pub mod mvt;
pub mod png;
pub mod query;
pub mod knmi_hdf5;
pub mod radar_raster;
pub mod terrain_shade;
pub mod wind;
pub mod radar;
pub mod raster;
pub mod sidecar;
pub mod tiff;
pub mod spool;
pub mod tiler;
pub mod wkb;

pub use fetch::{fetch_source, FetchOptions, FetchOutcome, SourceSpec};
pub use layers::{find_layer, registry, BuildCtx, BuildReport, Layer};
