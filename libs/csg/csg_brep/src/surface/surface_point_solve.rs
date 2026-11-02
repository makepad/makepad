//! The newer surface-point solvers (`SmSurface::GlobalPointSolve_1` and
//! `LocalPointSolve_1`) and their file-local classes: the problem data
//! (`SmSurfacePointSolver`), the global solver (`SmSurfPtGlobalSolver`) and the local
//! iteration stepper (`SmSurfPtSolveStepper`).
//!
//! Upstream: `source/SMLib/src/SmSurfacePointSolve.cpp`.
//! The file is split over the plan units `surface.surface_point_solve_p1`, `_p2` and
//! `_p3`; each lives in its own submodule and adds methods to the classes of `p1`.

pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::{SurfPtGlobalSolver, SurfPtSolveStepper, SurfacePointSolver};
