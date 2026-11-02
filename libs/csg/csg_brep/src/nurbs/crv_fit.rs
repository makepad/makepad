//! NURBS curve fitting: parametrisation of data points, knot vectors for
//! interpolation and approximation, global interpolation (optionally with end or
//! point derivatives) and least-squares approximation.
//!
//! Upstream: `source/SMLib/src/NL_CrvFit.cpp` (declarations in
//! `source/SMLib/inc/NL_CrvFit.h`). The file is split over units `nlib.crv_fit_p1`,
//! `nlib.crv_fit_p2`, `nlib.crv_fit_p3` and `nlib.crv_fit_p4`, one submodule each.

pub mod p1;
pub mod p2;
pub mod p3;
pub mod p4;
