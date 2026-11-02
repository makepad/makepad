//! Surfaces of revolution (`SmSurfOfRevolution`): a generating curve swept about an
//! axis, kept together with its B-spline surface and the polar conversion table
//! between the angular (STEP) and NURB parameterisations of the sweep direction.
//!
//! Upstream: `source/SMLib/src/SmSurfOfRevolution.cpp`,
//! `source/SMLib/inc/SmSurfOfRevolution.h`. The file is split over several plan units
//! (`surface.surf_of_revolution_p1`, `_p2`, `_p3`); each lives in its own submodule and
//! adds methods to [`SurfOfRevolution`] (or free functions).

pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::{
    test_for_iso_circles, DirectrixAtGeneratorParam, IsoCircles, RevolutionIsoCurve,
    SurfOfRevolution,
};
pub use p3::Revolved;
