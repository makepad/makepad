//! Sweeps: extrusion, revolution, translational and rotational sweeps, skinning
//! (lofting) of profiles into surfaces and solids.
//!
//! Upstream: `SmSweepGeometryCreation`, `SmTransSweepGeometry`,
//! `SmRotationalSweepGeometry`, `SmTopologySweep` in `source/SMLib`, and the sweep
//! entry points of `SM_API/SmApiBrep` and `SM_API/SmApiSurfaces`.

pub mod analytic_revolution;
pub mod api_brep;
pub mod api_surfaces;
pub mod rotational_sweep_geometry;
pub mod topology_sweep;
pub mod trans_sweep_geometry;
