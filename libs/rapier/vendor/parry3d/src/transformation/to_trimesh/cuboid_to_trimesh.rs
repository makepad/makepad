use crate::bounding_volume::Aabb;
use crate::math::Vector;
use crate::shape::Cuboid;
use crate::transformation::utils;
use alloc::vec::Vec;

impl Aabb {
    /// Discretize the boundary of this Aabb as a triangle-mesh.
    pub fn to_trimesh(&self) -> (Vec<Vector>, Vec<[u32; 3]>) {
        let center = self.center();
        let half_extents = self.half_extents();
        let mut cube_mesh = Cuboid::new(half_extents).to_trimesh();
        cube_mesh.0.iter_mut().for_each(|p| *p += center);
        cube_mesh
    }
}

impl Cuboid {
    /// Discretize the boundary of this cuboid as a triangle-mesh.
    pub fn to_trimesh(&self) -> (Vec<Vector>, Vec<[u32; 3]>) {
        let (vtx, idx) = unit_cuboid();
        (utils::scaled(vtx, self.half_extents * 2.0), idx)
    }
}

/// Generates a cuboid shape with a split index buffer.
///
/// The cuboid is centered at the origin, and has its half extents set to 0.5.
fn unit_cuboid() -> (Vec<Vector>, Vec<[u32; 3]>) {
    #[cfg(feature = "dim3")]
    {
        let mut coords = Vec::with_capacity(8);
        let mut faces = Vec::with_capacity(12);

        coords.push(Vector::new(-0.5, -0.5, 0.5));
        coords.push(Vector::new(-0.5, -0.5, -0.5));
        coords.push(Vector::new(0.5, -0.5, -0.5));
        coords.push(Vector::new(0.5, -0.5, 0.5));
        coords.push(Vector::new(-0.5, 0.5, 0.5));
        coords.push(Vector::new(-0.5, 0.5, -0.5));
        coords.push(Vector::new(0.5, 0.5, -0.5));
        coords.push(Vector::new(0.5, 0.5, 0.5));

        faces.push([4, 5, 0]);
        faces.push([5, 1, 0]);

        faces.push([5, 6, 1]);
        faces.push([6, 2, 1]);

        faces.push([6, 7, 3]);
        faces.push([2, 6, 3]);

        faces.push([7, 4, 0]);
        faces.push([3, 7, 0]);

        faces.push([0, 1, 2]);
        faces.push([3, 0, 2]);

        faces.push([7, 6, 5]);
        faces.push([4, 7, 5]);

        (coords, faces)
    }
    #[cfg(feature = "dim2")]
    {
        let mut coords = Vec::with_capacity(8);
        let mut faces = Vec::with_capacity(12);

        coords.push(Vector::new(-0.5, -0.5));
        coords.push(Vector::new(0.5, -0.5));
        coords.push(Vector::new(-0.5, 0.5));
        coords.push(Vector::new(0.5, 0.5));

        faces.push([0, 1, 2]);
        faces.push([2, 1, 3]);

        (coords, faces)
    }
}
