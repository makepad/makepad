//! Cones and cylinders (`SmCone`, `SmCylinder`): surfaces of revolution of a line about
//! an axis, with the STEP parameterisation by angle (degrees) and height along the
//! axis.
//!
//! Upstream: `source/SMLib/src/SmCone.cpp`, `source/SMLib/inc/SmCone.h`,
//! `source/SMLib/src/SmCylinder.cpp`, `source/SMLib/inc/SmCylinder.h`. The files are
//! split over several plan units (`surface.cone_p1`, `_p2`, `_p3`); each lives in its
//! own submodule and adds methods to [`Cone`] (or free functions).

pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::{
    total_coincidence_checker, Apex, Coincidence, Cone, ConeCanonical, ConeStepInversion,
    PositivePoint,
};
pub use p2::ellipse_trim_intervals;
