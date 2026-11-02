//! Surface geometry: plane, cylinder, cone, sphere, torus, B-spline, extrusion,
//! revolution, offset and blend surfaces, as a concrete `enum` with evaluation,
//! normals, derivatives and closest point.
//!
//! Upstream: `SmSurface` and its subclasses (`SmPlane`, `SmCylinder`, `SmCone`,
//! `SmSphere`, `SmTorus`, `SmBSplineSurface`, `SmSurfOfExtrusion`,
//! `SmSurfOfRevolution`, `SmOffsetSurface`, `SmBlendSurface`, ...) in `source/SMLib`,
//! and `SM_API/SmApiSurfaces`.

pub mod b_spline_volume;
pub mod b_spline_volume_n_lib;
pub mod bend_volume;
pub mod cone;
pub mod curve_bounded_surface;
pub mod offset_surface;
pub mod plane;
pub mod sphere;
pub mod srf_in_volume;
pub mod surf_of_extrusion;
pub mod surf_of_revolution;
#[allow(clippy::module_inception)]
pub mod surface;
pub mod surface_cache;
pub mod surface_point_solve;
pub mod torus;
pub mod trim_srf_cache;
pub mod twist_volume;
pub mod unbend_volume;
pub mod volume;
