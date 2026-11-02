//! The standard fillet solvers (constant radius, surface/surface, constant distance,
//! curve based and variable radius) and the linear, circular and blend-curve cross
//! section fillet surface generators.
//!
//! Upstream: `source/SMLib/src/SmFilletStandardSolver.cpp`,
//! `source/SMLib/inc/SmFilletStandardSolver.h`. The files are split over several plan
//! units (`blend.fillet_standard_solver_p1`, `_p2`); each lives in its own submodule.

pub mod p1;
pub mod p2;
pub mod surface_surface;
