use crate::core::edge_data::OverlayEdgeData;
use crate::core::solver::Solver;
use crate::segm::segment::Segment;
use crate::segm::winding::WindingCount;
use crate::split::snap_radius::SnapRadius;
use crate::split::solver::SplitSolver;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_tree::{Expiration, LayoutNumber};

impl<I> SplitSolver<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    pub(super) fn list_split<C: WindingCount, D: OverlayEdgeData<C>>(
        &mut self,
        snap_radius: SnapRadius,
        segments: &mut Vec<Segment<C, I, D>>,
        solver: &Solver,
        store: &mut D::Store,
    ) -> bool {
        let mut need_to_fix = true;

        let mut snap_radius = snap_radius;
        let mut any_intersection = false;
        let mut reusable_buffer = Vec::new();

        while need_to_fix && segments.len() > 1 {
            need_to_fix = false;
            self.marks.clear();

            let radius = snap_radius.radius::<I>();

            for (i, si) in segments.iter().enumerate() {
                let xsi = &si.x_segment;
                let ri = xsi.y_range();
                for (j, sj) in segments.iter().enumerate().skip(i + 1) {
                    let xsj = &sj.x_segment;
                    if xsi.b.x < xsj.a.x {
                        break;
                    }

                    if xsj.is_not_intersect_y_range(&ri) {
                        continue;
                    }

                    let is_round = Self::cross(i, j, xsi, xsj, &mut self.marks, radius);
                    need_to_fix = need_to_fix || is_round
                }
            }

            if self.marks.is_empty() {
                return any_intersection;
            }
            any_intersection = true;
            self.apply(segments, &mut reusable_buffer, solver, store);

            snap_radius.increment();

            if need_to_fix && !solver.is_list_split(segments) {
                // finish with tree solver if edges is become large
                self.tree_split(snap_radius, segments, solver, store);
                return true;
            }
        }

        any_intersection
    }
}
