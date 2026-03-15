use crate::math::{Real, Vector};
use crate::partitioning::BvhNode;
use crate::query::{Ray, RayCast, RayIntersection};
use crate::shape::{FeatureId, Voxels, VoxelsChunkRef};

impl RayCast for Voxels {
    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        self.chunk_bvh()
            .find_best(
                max_time_of_impact,
                |node: &BvhNode, best_so_far| node.cast_ray(ray, best_so_far),
                |primitive, best_so_far| {
                    let chunk = self.chunk_ref(primitive);
                    chunk.cast_local_ray_and_get_normal(ray, best_so_far, solid)
                },
            )
            .map(|(_chunk_id, hit)| hit)
    }
}

impl<'a> RayCast for VoxelsChunkRef<'a> {
    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        solid: bool,
    ) -> Option<RayIntersection> {
        use num_traits::Bounded;

        let aabb = self.local_aabb();
        let (min_t, mut max_t) = aabb.clip_ray_parameters(ray)?;

        #[cfg(feature = "dim2")]
        let ii = [0, 1];
        #[cfg(feature = "dim3")]
        let ii = [0, 1, 2];

        if min_t > max_time_of_impact {
            return None;
        }

        max_t = max_t.min(max_time_of_impact);
        let clip_ray_a = ray.point_at(min_t);
        let voxel_key_signed = self.voxel_at_point_unchecked(clip_ray_a);
        let mut voxel_key = self.clamp_voxel(voxel_key_signed);
        let [domain_mins, domain_maxs] = self.domain();

        loop {
            let aabb = self.voxel_aabb_unchecked(voxel_key);

            if let Some(voxel) = self.voxel_state(voxel_key) {
                if !voxel.is_empty() {
                    // We hit a voxel!
                    // TODO: if `solid` is false, and we started hitting from the first iteration,
                    //       then we should continue the ray propagation until we reach empty space again.
                    let hit = aabb.cast_local_ray_and_get_normal(ray, max_t, solid);

                    if let Some(mut hit) = hit {
                        // TODO: have the feature id be based on the voxel type?
                        hit.feature = FeatureId::Face(
                            self.flat_id(voxel_key).unwrap_or_else(|| unreachable!()),
                        );
                        return Some(hit);
                    }
                }
            }

            /*
             * Find the next voxel to cast the ray on.
             */
            let toi = ii.map(|i| {
                if ray.dir[i] > 0.0 {
                    let t = (aabb.maxs[i] - ray.origin[i]) / ray.dir[i];
                    if t < 0.0 {
                        (Real::max_value(), true)
                    } else {
                        (t, true)
                    }
                } else if ray.dir[i] < 0.0 {
                    let t = (aabb.mins[i] - ray.origin[i]) / ray.dir[i];
                    if t < 0.0 {
                        (Real::max_value(), false)
                    } else {
                        (t, false)
                    }
                } else {
                    (Real::max_value(), false)
                }
            });

            #[cfg(feature = "dim2")]
            if toi[0].0 > max_t && toi[1].0 > max_t {
                break;
            }

            #[cfg(feature = "dim3")]
            if toi[0].0 > max_t && toi[1].0 > max_t && toi[2].0 > max_t {
                break;
            }

            let imin = Vector::from(toi.map(|t| t.0)).min_position();

            if toi[imin].1 {
                if voxel_key[imin] < domain_maxs[imin] - 1 {
                    voxel_key[imin] += 1;
                } else {
                    // Leaving the shape’s bounds.
                    break;
                }
            } else if voxel_key[imin] > domain_mins[imin] {
                voxel_key[imin] -= 1;
            } else {
                // Leaving the shape’s bounds.
                break;
            }
        }

        None
    }
}
