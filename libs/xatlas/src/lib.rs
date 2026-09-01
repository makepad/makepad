//! Rust port of [xatlas](https://github.com/jpcy/xatlas) (`f700c779`).
//!
//! Official Hunyuan-Paint (`xatlas.parametrize`) is:
//! `Create` + `AddMesh` + `Generate` with default chart/pack options, then
//! `uv / (width, height)`. This crate must match the C++ oracle dumps in
//! `oracle/gold/` bit-exactly when built with the same flags
//! (`XA_MULTITHREADED=0`, `XA_DEBUG=0`, `NDEBUG`).

// A bit-exact port keeps upstream's full surface; unused pieces stay to match the C++.
#![allow(dead_code)]

mod atlas;
mod math;
mod mesh;
mod opennl;
mod pack;
mod param;
mod raster;
mod segment;
mod util;

pub use atlas::{
    parametrize, parametrize_with_options, parametrize_with_options_progress,
    parametrize_with_progress, unwrap_mesh, AddMeshError, ChartOptions, PackOptions,
    Parametrize, Vertex,
};
