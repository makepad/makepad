//! Mass properties (mass, inertia, center-of-mass) of shapes.

pub use self::mass_properties::MassProperties;

mod mass_properties;
mod mass_properties_ball;
mod mass_properties_capsule;
#[cfg(feature = "alloc")]
mod mass_properties_compound;
#[cfg(feature = "dim3")]
mod mass_properties_cone;
#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
mod mass_properties_convex_polygon;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
mod mass_properties_convex_polyhedron;
mod mass_properties_cuboid;
mod mass_properties_cylinder;
#[cfg(feature = "dim2")]
mod mass_properties_triangle;
#[cfg(feature = "dim2")]
#[cfg(feature = "alloc")]
mod mass_properties_trimesh2d;
#[cfg(feature = "dim3")]
#[cfg(feature = "alloc")]
mod mass_properties_trimesh3d;

#[cfg(feature = "alloc")]
mod mass_properties_voxels;

/// Free functions for some special-cases of mass-properties computation.
pub mod details {
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    pub use super::mass_properties_convex_polygon::convex_polygon_area_and_center_of_mass;
    #[cfg(feature = "dim2")]
    #[cfg(feature = "alloc")]
    pub use super::mass_properties_trimesh2d::trimesh_area_and_center_of_mass;
    #[cfg(feature = "dim3")]
    #[cfg(feature = "alloc")]
    pub use super::mass_properties_trimesh3d::{
        tetrahedron_unit_inertia_tensor_wrt_point, trimesh_signed_volume_and_center_of_mass,
    };
}
