//! NURBS basis functions, knot vectors, and B-spline/NURBS curve and surface
//! evaluation, refinement, fitting and approximation.
//!
//! Upstream: `source/SMLib/{inc,src}/NL_*` (the "NLib" layer: `NL_BasisBasic`,
//! `NL_BasisAdv`, `NL_CrvBasic`, `NL_SrfBasic`, `NL_CrvFit`, ...) and the
//! `SmNurbs*` / `SmBSpline*` wrappers. Units are listed in `plan/modules.toml`.

pub mod b_spline_curve;
pub mod b_spline_curve_n_lib;
pub mod b_spline_surface;
pub mod b_spline_surface_n_lib;
pub mod basis_adv;
pub mod basis_basic;
pub mod bezier;
pub mod crv_adv;
pub mod crv_approx;
pub mod crv_basic;
pub mod crv_common;
pub mod crv_fit;
pub mod crv_geom;
pub mod crv_nurb;
pub mod crv_offset;
pub mod crv_tool;
pub(crate) mod fit_arrays;
pub mod funcs_adv;
pub mod funcs_basic;
pub mod geom;
pub mod iges;
pub(crate) mod index;
pub mod knots;
pub mod math;
pub mod nurbs_crv;
pub mod nurbs_srf;
pub mod spiral;
pub mod srf_adv;
pub mod srf_basic;
pub mod srf_common;
pub mod srf_fit;
pub mod srf_nurb;
pub mod srf_offset;
pub mod srf_tool;
