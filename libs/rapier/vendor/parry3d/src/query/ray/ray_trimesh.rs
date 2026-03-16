use crate::math::Real;
use crate::query::{Ray, RayCast, RayIntersection};
use crate::shape::{CompositeShapeRef, FeatureId, TriMesh};

#[cfg(feature = "dim3")]
pub use ray_cast_with_culling::RayCullingMode;

impl RayCast for TriMesh {
    #[inline]
    fn cast_local_ray(&self, ray: &Ray, max_time_of_impact: Real, solid: bool) -> Option<Real> {
        CompositeShapeRef(self)
            .cast_local_ray(ray, max_time_of_impact, solid)
            .map(|hit| hit.1)
    }

    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        CompositeShapeRef(self)
            .cast_local_ray_and_get_normal(ray, max_time_of_impact, solid)
            .map(|(best, mut res)| {
                // We hit a backface.
                // NOTE: we need this for `TriMesh::is_backface` to work properly.
                if res.feature == FeatureId::Face(1) {
                    res.feature = FeatureId::Face(best + self.indices().len() as u32)
                } else {
                    res.feature = FeatureId::Face(best);
                }
                res
            })
    }
}

// NOTE: implement the ray-cast with culling on its own submodule to facilitate feature gating.
#[cfg(feature = "dim3")]
mod ray_cast_with_culling {
    use crate::math::{Pose, Real, Vector};
    use crate::partitioning::Bvh;
    use crate::query::details::NormalConstraints;
    use crate::query::{Ray, RayIntersection};
    use crate::shape::{
        CompositeShape, CompositeShapeRef, FeatureId, Shape, TriMesh, Triangle, TypedCompositeShape,
    };

    /// Controls which side of a triangle a ray-cast is allowed to hit.
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum RayCullingMode {
        /// Ray-casting won’t hit back faces.
        IgnoreBackfaces,
        /// Ray-casting won’t hit front faces.
        IgnoreFrontfaces,
    }

    impl RayCullingMode {
        fn check(self, tri_normal: Vector, ray_dir: Vector) -> bool {
            match self {
                RayCullingMode::IgnoreBackfaces => tri_normal.dot(ray_dir) < 0.0,
                RayCullingMode::IgnoreFrontfaces => tri_normal.dot(ray_dir) > 0.0,
            }
        }
    }

    /// A utility shape with a `TypedCompositeShape` implementation that skips triangles that
    /// are back-faces or front-faces relative to a given ray and culling mode.
    struct TriMeshWithCulling<'a> {
        trimesh: &'a TriMesh,
        culling: RayCullingMode,
        ray: &'a Ray,
    }

    impl CompositeShape for TriMeshWithCulling<'_> {
        fn map_part_at(
            &self,
            shape_id: u32,
            f: &mut dyn FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>),
        ) {
            let _ = self.map_untyped_part_at(shape_id, f);
        }

        /// Gets the acceleration structure of the composite shape.
        fn bvh(&self) -> &Bvh {
            self.trimesh.bvh()
        }
    }

    impl TypedCompositeShape for TriMeshWithCulling<'_> {
        type PartShape = Triangle;
        type PartNormalConstraints = ();

        #[inline(always)]
        fn map_typed_part_at<T>(
            &self,
            i: u32,
            mut f: impl FnMut(
                Option<&Pose>,
                &Self::PartShape,
                Option<&Self::PartNormalConstraints>,
            ) -> T,
        ) -> Option<T> {
            let tri = self.trimesh.triangle(i);
            let tri_normal = tri.scaled_normal();

            if self.culling.check(tri_normal, self.ray.dir) {
                Some(f(None, &tri, None))
            } else {
                None
            }
        }

        #[inline(always)]
        fn map_untyped_part_at<T>(
            &self,
            i: u32,
            mut f: impl FnMut(Option<&Pose>, &dyn Shape, Option<&dyn NormalConstraints>) -> T,
        ) -> Option<T> {
            let tri = self.trimesh.triangle(i);
            let tri_normal = tri.scaled_normal();

            if self.culling.check(tri_normal, self.ray.dir) {
                Some(f(None, &tri, None))
            } else {
                None
            }
        }
    }

    impl TriMesh {
        /// Casts a ray on this triangle mesh transformed by `m`.
        ///
        /// Hits happening later than `max_time_of_impact` are ignored. In other words, hits are
        /// only searched along the segment `[ray.origin, ray.origin + ray.dir * max_time_of_impact`].
        ///
        /// Hits on back-faces or front-faces are ignored, depending on the given `culling` mode.
        #[inline]
        pub fn cast_ray_with_culling(
            &self,
            m: &Pose,
            ray: &Ray,
            max_time_of_impact: Real,
            culling: RayCullingMode,
        ) -> Option<RayIntersection> {
            let ls_ray = ray.inverse_transform_by(m);
            self.cast_local_ray_with_culling(&ls_ray, max_time_of_impact, culling)
                .map(|inter| inter.transform_by(m))
        }

        /// Casts a ray on this triangle mesh.
        ///
        /// This is the same as [`TriMesh::cast_ray_with_culling`] except that the ray is given
        /// in the mesh’s local-space.
        pub fn cast_local_ray_with_culling(
            &self,
            ray: &Ray,
            max_time_of_impact: Real,
            culling: RayCullingMode,
        ) -> Option<RayIntersection> {
            let mesh_with_culling = TriMeshWithCulling {
                trimesh: self,
                culling,
                ray,
            };
            CompositeShapeRef(&mesh_with_culling)
                .cast_local_ray_and_get_normal(ray, max_time_of_impact, false)
                .map(|(best, mut res)| {
                    // We hit a backface.
                    // NOTE: we need this for `TriMesh::is_backface` to work properly.
                    if res.feature == FeatureId::Face(1) {
                        res.feature = FeatureId::Face(best + self.indices().len() as u32)
                    } else {
                        res.feature = FeatureId::Face(best);
                    }
                    res
                })
        }
    }

    #[cfg(test)]
    mod test {
        use crate::math::Vector;
        use crate::query::{Ray, RayCullingMode};
        use crate::shape::TriMesh;

        #[test]
        fn cast_ray_on_trimesh_with_culling() {
            let vertices = vec![
                Vector::ZERO,
                Vector::new(1.0, 0.0, 0.0),
                Vector::new(0.0, 1.0, 0.0),
            ];
            let indices = vec![[0, 1, 2]];
            let ray_up = Ray::new(Vector::new(0.0, 0.0, -1.0), Vector::new(0.0, 0.0, 1.0));
            let ray_down = Ray::new(Vector::new(0.0, 0.0, 1.0), Vector::new(0.0, 0.0, -1.0));

            let mesh = TriMesh::new(vertices, indices).unwrap();
            assert!(mesh
                .cast_local_ray_with_culling(&ray_up, 1000.0, RayCullingMode::IgnoreFrontfaces)
                .is_some());
            assert!(mesh
                .cast_local_ray_with_culling(&ray_down, 1000.0, RayCullingMode::IgnoreFrontfaces)
                .is_none());

            assert!(mesh
                .cast_local_ray_with_culling(&ray_up, 1000.0, RayCullingMode::IgnoreBackfaces)
                .is_none());
            assert!(mesh
                .cast_local_ray_with_culling(&ray_down, 1000.0, RayCullingMode::IgnoreBackfaces)
                .is_some());
        }
    }
}
