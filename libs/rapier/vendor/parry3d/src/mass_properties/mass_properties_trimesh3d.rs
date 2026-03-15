use crate::mass_properties::MassProperties;
use crate::math::{Matrix, Real, Vector, DIM};
use crate::shape::Tetrahedron;
use num::Zero;

impl MassProperties {
    /// Computes the mass properties of a triangle mesh.
    pub fn from_trimesh(
        density: Real,
        vertices: &[Vector],
        indices: &[[u32; DIM]],
    ) -> MassProperties {
        let (volume, com) = trimesh_signed_volume_and_center_of_mass(vertices, indices);

        if volume.is_zero() {
            return MassProperties::zero();
        }

        let mut itot = Matrix::ZERO;

        for t in indices {
            let p2 = vertices[t[0] as usize];
            let p3 = vertices[t[1] as usize];
            let p4 = vertices[t[2] as usize];

            let vol = Tetrahedron::new(com, p2, p3, p4).signed_volume();
            let ipart = tetrahedron_unit_inertia_tensor_wrt_point(com, com, p2, p3, p4);

            itot += ipart * vol;
        }

        let sign = volume.signum();
        Self::with_inertia_matrix(com, volume * density * sign, itot * density * sign)
    }
}

/// Computes the unit inertia tensor of a tetrahedron, with regard to the given `point`.
pub fn tetrahedron_unit_inertia_tensor_wrt_point(
    point: Vector,
    p1: Vector,
    p2: Vector,
    p3: Vector,
    p4: Vector,
) -> Matrix {
    let p1 = p1 - point;
    let p2 = p2 - point;
    let p3 = p3 - point;
    let p4 = p4 - point;

    // Just for readability.
    let x1 = p1[0];
    let y1 = p1[1];
    let z1 = p1[2];
    let x2 = p2[0];
    let y2 = p2[1];
    let z2 = p2[2];
    let x3 = p3[0];
    let y3 = p3[1];
    let z3 = p3[2];
    let x4 = p4[0];
    let y4 = p4[1];
    let z4 = p4[2];

    let diag_x = x1 * x1
        + x1 * x2
        + x2 * x2
        + x1 * x3
        + x2 * x3
        + x3 * x3
        + x1 * x4
        + x2 * x4
        + x3 * x4
        + x4 * x4;
    let diag_y = y1 * y1
        + y1 * y2
        + y2 * y2
        + y1 * y3
        + y2 * y3
        + y3 * y3
        + y1 * y4
        + y2 * y4
        + y3 * y4
        + y4 * y4;
    let diag_z = z1 * z1
        + z1 * z2
        + z2 * z2
        + z1 * z3
        + z2 * z3
        + z3 * z3
        + z1 * z4
        + z2 * z4
        + z3 * z4
        + z4 * z4;

    let a0 = (diag_y + diag_z) * 0.1;
    let b0 = (diag_z + diag_x) * 0.1;
    let c0 = (diag_x + diag_y) * 0.1;

    let a1 = (y1 * z1 * 2.0
        + y2 * z1
        + y3 * z1
        + y4 * z1
        + y1 * z2
        + y2 * z2 * 2.0
        + y3 * z2
        + y4 * z2
        + y1 * z3
        + y2 * z3
        + y3 * z3 * 2.0
        + y4 * z3
        + y1 * z4
        + y2 * z4
        + y3 * z4
        + y4 * z4 * 2.0)
        * 0.05;
    let b1 = (x1 * z1 * 2.0
        + x2 * z1
        + x3 * z1
        + x4 * z1
        + x1 * z2
        + x2 * z2 * 2.0
        + x3 * z2
        + x4 * z2
        + x1 * z3
        + x2 * z3
        + x3 * z3 * 2.0
        + x4 * z3
        + x1 * z4
        + x2 * z4
        + x3 * z4
        + x4 * z4 * 2.0)
        * 0.05;
    let c1 = (x1 * y1 * 2.0
        + x2 * y1
        + x3 * y1
        + x4 * y1
        + x1 * y2
        + x2 * y2 * 2.0
        + x3 * y2
        + x4 * y2
        + x1 * y3
        + x2 * y3
        + x3 * y3 * 2.0
        + x4 * y3
        + x1 * y4
        + x2 * y4
        + x3 * y4
        + x4 * y4 * 2.0)
        * 0.05;

    Matrix::from_cols_array(&[a0, -c1, -b1, -c1, b0, -a1, -b1, -a1, c0])
}

/// Computes the volume and center-of-mass of a mesh.
pub fn trimesh_signed_volume_and_center_of_mass(
    vertices: &[Vector],
    indices: &[[u32; DIM]],
) -> (Real, Vector) {
    let geometric_center = crate::utils::center(vertices);

    let mut res = Vector::ZERO;
    let mut vol = 0.0;

    for t in indices {
        let p2 = vertices[t[0] as usize];
        let p3 = vertices[t[1] as usize];
        let p4 = vertices[t[2] as usize];

        let volume = Tetrahedron::new(geometric_center, p2, p3, p4).signed_volume();
        let center = Tetrahedron::new(geometric_center, p2, p3, p4).center();

        res += center * volume;
        vol += volume;
    }

    if vol.is_zero() {
        (vol, geometric_center)
    } else {
        (vol, res / vol)
    }
}

#[cfg(test)]
mod test {
    use crate::math::{Pose, Rotation, Vector, VectorExt};
    use crate::{
        mass_properties::MassProperties,
        shape::{Ball, Capsule, Cone, Cuboid, Cylinder, Shape},
    };
    use std::dbg;

    fn assert_same_principal_inertias(mprops1: &MassProperties, mprops2: &MassProperties) {
        for k in 0..3 {
            let i1 = mprops1.principal_inertia_local_frame
                * (mprops1.principal_inertia()
                    * (mprops1.principal_inertia_local_frame.inverse() * Vector::ith(k, 1.0)));
            let i2 = mprops2.principal_inertia_local_frame
                * (mprops2.principal_inertia()
                    * (mprops2.principal_inertia_local_frame.inverse() * Vector::ith(k, 1.0)));
            assert!(i1.abs_diff_eq(i2, 0.5))
        }
    }

    #[test]
    fn cuboid_as_trimesh_mprops() {
        let cuboid = Cuboid::new(Vector::new(1.0, 2.0, 3.0));

        use crate::shape::Shape;
        let orig_mprops = cuboid.mass_properties(1.0);
        dbg!(orig_mprops.principal_inertia());

        let mut trimesh = cuboid.to_trimesh();
        let mprops = MassProperties::from_trimesh(1.0, &trimesh.0, &trimesh.1);
        assert_relative_eq!(mprops.mass(), 48.0, epsilon = 1.0e-4);
        assert!(
            (mprops.principal_inertia_local_frame * mprops.principal_inertia())
                .abs()
                .abs_diff_eq(Vector::new(208.0, 160.0, 80.0), 1.0e-4)
        );

        // Check after shifting the trimesh off the origin.
        trimesh
            .0
            .iter_mut()
            .for_each(|pt| *pt += Vector::new(30.0, 20.0, 10.0));
        let mprops = MassProperties::from_trimesh(1.0, &trimesh.0, &trimesh.1);
        assert_relative_eq!(mprops.mass(), 48.0, epsilon = 1.0e-4);
        assert!(
            (mprops.principal_inertia_local_frame * mprops.principal_inertia())
                .abs()
                .abs_diff_eq(Vector::new(208.0, 160.0, 80.0), 1.0e-4)
        );
    }

    #[test]
    fn primitives_as_trimesh_mprops() {
        let primitives = (
            Cuboid::new(Vector::new(1.0, 2.0, 3.0)),
            Capsule::new_y(2.0, 1.0),
            Cone::new(2.0, 1.0),
            Cylinder::new(2.0, 1.0),
            Ball::new(2.0),
        );
        let mut meshes = [
            primitives.0.to_trimesh(),
            primitives.1.to_trimesh(100, 100),
            primitives.2.to_trimesh(100),
            primitives.3.to_trimesh(100),
            primitives.4.to_trimesh(100, 100),
        ];
        let shapes = [
            &primitives.0 as &dyn Shape,
            &primitives.1 as &dyn Shape,
            &primitives.2 as &dyn Shape,
            &primitives.3 as &dyn Shape,
            &primitives.4 as &dyn Shape,
        ];

        for (shape, mesh) in shapes.iter().zip(meshes.iter_mut()) {
            let shape_mprops = shape.mass_properties(2.0);
            let mesh_mprops = MassProperties::from_trimesh(2.0, &mesh.0, &mesh.1);
            assert_relative_eq!(shape_mprops.mass(), mesh_mprops.mass(), epsilon = 1.0e-1);
            assert_same_principal_inertias(&shape_mprops, &mesh_mprops);
            assert_relative_eq!(
                shape_mprops.local_com,
                mesh_mprops.local_com,
                epsilon = 1.0e-3
            );

            // Now try with a shifted mesh.
            let shift = Vector::new(33.0, 22.0, 11.0);
            mesh.0.iter_mut().for_each(|pt| *pt += shift);

            let mesh_mprops = MassProperties::from_trimesh(2.0, &mesh.0, &mesh.1);

            assert_relative_eq!(shape_mprops.mass(), mesh_mprops.mass(), epsilon = 1.0e-1);
            assert_same_principal_inertias(&shape_mprops, &mesh_mprops);
            assert_relative_eq!(
                shape_mprops.local_com + shift, // The mesh is shifted, so its center-of-mass is shifted too.
                mesh_mprops.local_com,
                epsilon = 1.0e-3
            );
        }
    }

    // Regression test for https://github.com/dimforge/parry/pull/331
    //
    // We compare the angular inertia tensor of a rotated & translated Cuboid, with the
    // inertia tensor of the same cuboid represented as a `TriMesh` with rotated & translated
    // vertices.
    #[test]
    fn rotated_inertia_tensor() {
        let cuboid = Cuboid::new(Vector::new(3.0, 2.0, 1.0));
        let density = 1.0;

        // Compute mass properties with a translated and rotated cuboid.
        let pose = Pose::new(Vector::new(5.0, 2.0, 3.0), Vector::new(0.6, 0.7, 0.8));
        let mprops = cuboid.mass_properties(density);

        // Compute mass properties with a manually transformed cuboid.
        let mut vertices = cuboid.to_trimesh().0;
        let trimesh_origin_mprops =
            MassProperties::from_trimesh(density, &vertices, &cuboid.to_trimesh().1);
        vertices.iter_mut().for_each(|v| *v = pose * *v);
        let trimesh_transformed_mprops =
            MassProperties::from_trimesh(density, &vertices, &cuboid.to_trimesh().1);

        // Compare the mass properties.
        assert_relative_eq!(
            mprops.mass(),
            trimesh_origin_mprops.mass(),
            epsilon = 1.0e-4
        );
        assert_relative_eq!(
            mprops.mass(),
            trimesh_transformed_mprops.mass(),
            epsilon = 1.0e-4
        );
        // compare local_com
        assert_relative_eq!(
            pose * mprops.local_com,
            trimesh_transformed_mprops.local_com,
            epsilon = 1.0e-4
        );
        assert_relative_eq!(
            pose * trimesh_origin_mprops.local_com,
            trimesh_transformed_mprops.local_com,
            epsilon = 1.0e-4
        );
        assert!(trimesh_origin_mprops
            .principal_inertia()
            .abs_diff_eq(mprops.principal_inertia(), 1.0e-4));
        let w1 = trimesh_origin_mprops.world_inv_inertia(&pose.rotation);
        let w2 = trimesh_transformed_mprops.world_inv_inertia(&Rotation::IDENTITY);
        assert_relative_eq!(w1.m11, w2.m11, epsilon = 1.0e-7);
        assert_relative_eq!(w1.m12, w2.m12, epsilon = 1.0e-7);
        assert_relative_eq!(w1.m13, w2.m13, epsilon = 1.0e-7);
        assert_relative_eq!(w1.m22, w2.m22, epsilon = 1.0e-7);
        assert_relative_eq!(w1.m23, w2.m23, epsilon = 1.0e-7);
        assert_relative_eq!(w1.m33, w2.m33, epsilon = 1.0e-7);
    }
}
