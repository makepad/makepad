use crate::bounding_volume::Aabb;
use crate::math::{Real, Vector, DIM};
use crate::num::{Bounded, Zero};
use crate::query::{PointProjection, PointQuery};
use crate::shape::FeatureId;

impl Aabb {
    fn do_project_local_point(&self, pt: Vector, solid: bool) -> (bool, Vector, Vector) {
        let mins_pt = self.mins - pt;
        let pt_maxs = pt - self.maxs;
        let shift = mins_pt.max(Vector::ZERO) - pt_maxs.max(Vector::ZERO);

        let inside = shift == Vector::ZERO;

        if !inside {
            (false, pt + shift, shift)
        } else if solid {
            (true, pt, shift)
        } else {
            let _max: Real = Bounded::max_value();
            let mut best = -_max;
            let mut is_mins = false;
            let mut best_id = 0;

            for i in 0..DIM {
                let mins_pt_i = mins_pt[i];
                let pt_maxs_i = pt_maxs[i];

                if mins_pt_i < pt_maxs_i {
                    if pt_maxs[i] > best {
                        best_id = i;
                        is_mins = false;
                        best = pt_maxs_i
                    }
                } else if mins_pt_i > best {
                    best_id = i;
                    is_mins = true;
                    best = mins_pt_i
                }
            }

            let mut shift: Vector = Vector::ZERO;

            if is_mins {
                shift[best_id] = best;
            } else {
                shift[best_id] = -best;
            }

            (inside, pt + shift, shift)
        }
    }
}

impl PointQuery for Aabb {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        let (inside, ls_pt, _) = self.do_project_local_point(pt, solid);
        PointProjection::new(inside, ls_pt)
    }

    #[allow(unused_assignments)] // For last_zero_shift which is used only in 3D.
    #[allow(unused_variables)] // For last_zero_shift which is used only in 3D.
    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        let (inside, ls_pt, shift) = self.do_project_local_point(pt, false);
        let proj = PointProjection::new(inside, ls_pt);
        let mut nzero_shifts = 0;
        let mut last_zero_shift = 0;
        let mut last_not_zero_shift = 0;

        for i in 0..DIM {
            if shift[i].is_zero() {
                nzero_shifts += 1;
                last_zero_shift = i;
            } else {
                last_not_zero_shift = i;
            }
        }

        if nzero_shifts == DIM {
            for i in 0..DIM {
                if ls_pt[i] > self.maxs[i] - crate::math::DEFAULT_EPSILON {
                    return (proj, FeatureId::Face(i as u32));
                }
                if ls_pt[i] <= self.mins[i] + crate::math::DEFAULT_EPSILON {
                    return (proj, FeatureId::Face((i + DIM) as u32));
                }
            }

            (proj, FeatureId::Unknown)
        } else if nzero_shifts == DIM - 1 {
            // On a 3D face.
            if ls_pt[last_not_zero_shift] < self.center()[last_not_zero_shift] {
                (proj, FeatureId::Face((last_not_zero_shift + DIM) as u32))
            } else {
                (proj, FeatureId::Face(last_not_zero_shift as u32))
            }
        } else {
            // On a vertex or edge.
            let mut id = 0;
            let center = self.center();

            for i in 0..DIM {
                if ls_pt[i] < center[i] {
                    id |= 1 << i;
                }
            }

            #[cfg(feature = "dim3")]
            {
                if nzero_shifts == 0 {
                    (proj, FeatureId::Vertex(id))
                } else {
                    (proj, FeatureId::Edge((id << 2) | (last_zero_shift as u32)))
                }
            }

            #[cfg(feature = "dim2")]
            {
                (proj, FeatureId::Vertex(id))
            }
        }
    }

    #[inline]
    fn distance_to_local_point(&self, pt: Vector, solid: bool) -> Real {
        let mins_pt = self.mins - pt;
        let pt_maxs = pt - self.maxs;
        let shift = mins_pt.max(pt_maxs).max(Vector::ZERO);

        if solid || shift != Vector::ZERO {
            shift.length()
        } else {
            // TODO: optimize that.
            -pt.distance(self.project_local_point(pt, solid).point)
        }
    }
}
