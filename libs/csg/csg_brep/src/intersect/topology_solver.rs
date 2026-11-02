//! The topology solver (`SmTopologySolver`): a global solver over spatial trees of
//! topology objects (vertices, edge curves, face surfaces) and the local solvers it
//! runs on pairs of tree objects.
//!
//! Upstream: `source/SMLib/src/SmTopologySolver.cpp`, `source/SMLib/inc/SmTopologySolver.h`.
//! The file is split over the plan units `intersect.topology_solver_p1` and `_p2`;
//! each lives in its own submodule.

pub mod p1;
pub mod p2;
