//! Turning an OpenStreetMap PBF extract into everything a map app needs to
//! draw and navigate a region offline:
//!
//! * [`native::convert_detail`] — pass 1-4, the bounded-memory spool store:
//!   every tagged element, every tag, clipped to z14 MVT tiles.
//! * [`native::convert_base`] — the styled z0..=14 base archive (plus the
//!   all-tag detail layers at z14) written from that store.
//! * [`nav_build::nav_build`] — `<basename>.graph` (routing) and
//!   `<basename>.search` (places/POIs/streets) from one PBF scan.
//! * [`testmap`] — the recipe that chains those into a runnable city-sized
//!   test map, for a first run with no archives on disk at all.
//!
//! The passes are the same code whether the `makepad-map-tiles` CLI runs
//! them from a shell or an app runs them on a worker thread; [`progress`]
//! is how the latter gets the lines the former prints to stdout.

pub mod nav_build;
pub mod native;
pub mod progress;
pub mod testmap;
pub mod versatiles;
