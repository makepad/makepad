//! Intersections: curve/curve, curve/surface, surface/surface (marching and
//! tracing), face/face and B-rep/B-rep intersection graphs.
//!
//! Upstream: `SmSurfaceIntersector`, `SmSurfaceTracer`, `SmTopologyIntersector`,
//! `SmPolyIntersector`, `SmSSIData` in `source/SMLib`, and
//! `SM_API/SmApiIntersectors`.

pub mod adv_surface_intersector;
pub mod api_intersectors;
pub mod curve_class;
pub mod point_class;
pub mod ray_tracer;
pub mod surface_drop_curve;
pub mod surface_drop_lib;
pub mod surface_intersector;
pub mod surface_silhouette;
pub mod surface_tracer;
pub mod topology_solver;
