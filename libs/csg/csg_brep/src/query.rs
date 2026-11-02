//! Read-only queries on B-reps and geometry: closest point, point
//! classification, bounding boxes, adjacency lookups, manifold checks.
//!
//! Upstream: `SmPointClass`, `SmRayTracer`, `SmExtent*`, `SmPseudoBox`,
//! `SmTopologyTraverser` in `source/SMLib`, and `SM_API/SmApiQueries`.

pub mod api_queries;
pub mod brep;
pub mod edge;
pub mod face;
pub mod r#loop;
pub mod poly;
pub mod poly_properties;
pub mod tess;
