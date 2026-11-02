//! The fillet solver base (`SmFilletSolver`): the data and the solver-independent
//! methods shared by every fillet solver, its nonlinear function objects, and the
//! fillet surface generator types.
//!
//! Upstream: `source/SMLib/src/SmFilletSolver.cpp`, `source/SMLib/inc/SmFilletSolver.h`.

pub mod ball_fit;
pub mod curve_blend_sih;
pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
pub mod p5;

pub mod surfaces;
