//! Mass properties: length, area, volume, centroid and moments, via numeric
//! integration over curves, faces and solids.
//!
//! Upstream: `SmIntegrator`, `SmGauss`, `SmEdgeProps`, `SmFaceProps`, `SmLoopProps`,
//! `SmVertexProps`, `SmObjProps` in `source/SMLib`, and the area/volume queries of
//! `SM_API/SmApiQueries`.

pub mod integrator;
