//! Boolean operations (union, intersection, difference) on solids, plus the
//! classification and merge steps they are built from.
//!
//! Upstream: `SmTopologySolver`, `SmMerge`, `SmRegion`, `SmBrepCutting` and related
//! files in `source/SMLib`, and the boolean entry points of `SM_API/SmApiBrep`.

pub mod api_brep;
pub mod merge;
pub mod topology_intersector;
