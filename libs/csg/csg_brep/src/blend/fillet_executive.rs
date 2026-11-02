//! The fillet executive (`SmFilletExecutive`): drives the fillet solvers over a B-rep,
//! and the fillet laws (`SmFilletLaw`) that give the radius along a filleted edge.
//! Split by plan unit.
//!
//! Upstream: `source/SMLib/inc/SmFilletExecutive.h`, `source/SMLib/src/SmFilletExecutive.cpp`.

pub mod api;
pub mod p1;
pub mod p2;
pub mod p3;
pub mod session;
pub mod topology_changes;
