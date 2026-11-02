//! Planes (`SmPlane`): an analytic plane `O + u * su * X + v * sv * Y` over a STEP
//! (analytic) domain, kept next to a bilinear B-spline (its NURB representation).
//!
//! Upstream: `source/SMLib/src/SmPlane.cpp`, `source/SMLib/inc/SmPlane.h`.
//! The file is split over the plan units `surface.plane_p1` and `_p2`; each lives in
//! its own submodule and adds methods to [`Plane`].

pub mod p1;
pub mod p2;

pub use p1::{Coincidence, Plane, PlaneDropCurve, PlaneIsoCurve};
