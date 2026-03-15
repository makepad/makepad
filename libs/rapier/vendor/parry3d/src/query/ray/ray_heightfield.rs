use crate::math::Real;
#[cfg(feature = "dim2")]
use crate::query;
use crate::query::{Ray, RayCast, RayIntersection};
#[cfg(feature = "dim2")]
use crate::shape::FeatureId;
use crate::shape::HeightField;

#[cfg(feature = "dim2")]
impl RayCast for HeightField {
    #[inline]
    fn cast_local_ray_and_get_normal(
        &self,
        ray: &Ray,
        max_time_of_impact: Real,
        _: bool,
    ) -> Option<RayIntersection> {
        let aabb = self.local_aabb();
        let (min_t, mut max_t) = aabb.clip_ray_parameters(ray)?;

        if min_t > max_time_of_impact {
            return None;
        }

        max_t = max_t.min(max_time_of_impact);

        let clip_ray_a = ray.point_at(min_t);

        // None may happen due to slight numerical errors.
        let mut curr = self.cell_at_point(clip_ray_a).unwrap_or_else(|| {
            if ray.origin.x > 0.0 {
                self.num_cells() - 1
            } else {
                0_usize
            }
        });

        /*
         * Test the segment under the ray.
         */
        if let Some(seg) = self.segment_at(curr) {
            let (s, t) = query::details::closest_points_line_line_parameters(
                ray.origin,
                ray.dir,
                seg.a,
                seg.scaled_direction(),
            );
            if s >= 0.0 && t >= 0.0 && t <= 1.0 {
                // Cast succeeded on the first element!
                let n = seg.normal().unwrap();
                let fid = if n.dot(ray.dir) > 0.0 {
                    // The ray hit the back face.
                    curr + self.num_cells()
                } else {
                    // The ray hit the front face.
                    curr
                };

                return Some(RayIntersection::new(s, n, FeatureId::Face(fid as u32)));
            }
        }

        /*
         * Test other segments in the path of the ray.
         */
        if ray.dir.x == 0.0 {
            return None;
        }

        let right = ray.dir.x > 0.0;
        let cell_width = self.cell_width();
        let start_x = self.start_x();

        while (right && curr < self.num_cells()) || (!right && curr > 0) {
            let curr_param;

            if right {
                curr += 1;
                curr_param = (cell_width * (curr as Real) + start_x - ray.origin.x) / ray.dir.x;
            } else {
                curr_param = (ray.origin.x - cell_width * (curr as Real) - start_x) / ray.dir.x;
                curr -= 1;
            }

            if curr_param >= max_t {
                // The part of the ray after max_t is outside of the heightfield Aabb.
                return None;
            }

            if let Some(seg) = self.segment_at(curr) {
                // TODO: test the y-coordinates (equivalent to an Aabb test) before actually computing the intersection.
                let (s, t) = query::details::closest_points_line_line_parameters(
                    ray.origin,
                    ray.dir,
                    seg.a,
                    seg.scaled_direction(),
                );

                if t >= 0.0 && t <= 1.0 && s <= max_time_of_impact {
                    let n = seg.normal().unwrap();
                    let fid = if n.dot(ray.dir) > 0.0 {
                        // The ray hit the back face.
                        curr + self.num_cells()
                    } else {
                        // The ray hit the front face.
                        curr
                    };
                    return Some(RayIntersection::new(s, n, FeatureId::Face(fid as u32)));
                }
            }
        }

        None
    }
}

#[cfg(feature = "dim3")]
impl RayCast for HeightField {
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
        max_t = max_t.min(max_time_of_impact);
        let clip_ray_a = ray.point_at(min_t);
        let mut cell = match self.cell_at_point(clip_ray_a) {
            Some(cell) => cell,
            // None may happen due to slight numerical errors.
            None => {
                let i = if ray.origin.z > 0.0 {
                    self.nrows() - 1
                } else {
                    0
                };

                let j = if ray.origin.x > 0.0 {
                    self.ncols() - 1
                } else {
                    0
                };

                (i, j)
            }
        };

        loop {
            let tris = self.triangles_at(cell.0, cell.1);
            let inter1 = tris
                .0
                .and_then(|tri| tri.cast_local_ray_and_get_normal(ray, max_time_of_impact, solid));
            let inter2 = tris
                .1
                .and_then(|tri| tri.cast_local_ray_and_get_normal(ray, max_time_of_impact, solid));

            match (inter1, inter2) {
                (Some(mut inter1), Some(mut inter2)) => {
                    if inter1.time_of_impact < inter2.time_of_impact {
                        inter1.feature =
                            self.convert_triangle_feature_id(cell.0, cell.1, true, inter1.feature);
                        return Some(inter1);
                    } else {
                        inter2.feature =
                            self.convert_triangle_feature_id(cell.0, cell.1, false, inter2.feature);
                        return Some(inter2);
                    }
                }
                (Some(mut inter), None) => {
                    inter.feature =
                        self.convert_triangle_feature_id(cell.0, cell.1, true, inter.feature);
                    return Some(inter);
                }
                (None, Some(mut inter)) => {
                    inter.feature =
                        self.convert_triangle_feature_id(cell.0, cell.1, false, inter.feature);
                    return Some(inter);
                }
                (None, None) => {}
            }

            /*
             * Find the next cell to cast the ray on.
             */
            let (toi_x, right) = if ray.dir.x > 0.0 {
                let x = self.x_at(cell.1 + 1);
                ((x - ray.origin.x) / ray.dir.x, true)
            } else if ray.dir.x < 0.0 {
                let x = self.x_at(cell.1);
                ((x - ray.origin.x) / ray.dir.x, false)
            } else {
                (Real::max_value(), false)
            };

            let (toi_z, down) = if ray.dir.z > 0.0 {
                let z = self.z_at(cell.0 + 1);
                ((z - ray.origin.z) / ray.dir.z, true)
            } else if ray.dir.z < 0.0 {
                let z = self.z_at(cell.0);
                ((z - ray.origin.z) / ray.dir.z, false)
            } else {
                (Real::max_value(), false)
            };

            if toi_x > max_t && toi_z > max_t {
                break;
            }

            if toi_x >= 0.0 && toi_x < toi_z {
                if right {
                    cell.1 += 1
                } else if cell.1 > 0 {
                    cell.1 -= 1
                } else {
                    break;
                }
            } else if toi_z >= 0.0 {
                if down {
                    cell.0 += 1
                } else if cell.0 > 0 {
                    cell.0 -= 1
                } else {
                    break;
                }
            } else {
                break;
            }

            if cell.0 >= self.nrows() || cell.1 >= self.ncols() {
                break;
            }
        }

        None
    }
}
