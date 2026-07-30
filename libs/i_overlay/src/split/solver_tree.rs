use crate::core::edge_data::OverlayEdgeData;
use crate::core::solver::Solver;
use crate::geom::line_range::LineRange;
use crate::geom::x_segment::XSegment;
use crate::segm::segment::Segment;
use crate::segm::winding::WindingCount;
use crate::split::snap_radius::SnapRadius;
use crate::split::solver::SplitSolver;
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_tree::ExpiredVal;
use i_tree::seg::exp::{SegExpCollection, SegRange};
use i_tree::seg::tree::SegExpTree;
use i_tree::{Expiration, LayoutNumber};

#[derive(Clone, Copy)]
struct IdSegment<I: IntNumber> {
    id: usize,
    x_segment: XSegment<I>,
}

impl<I: IntNumber + Expiration> ExpiredVal<I> for IdSegment<I> {
    #[inline]
    fn expiration(&self) -> I {
        self.x_segment.b.x
    }
}

impl<I> SplitSolver<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    pub(super) fn tree_split<C: WindingCount, D: OverlayEdgeData<C>>(
        &mut self,
        snap_radius: SnapRadius,
        segments: &mut Vec<Segment<C, I, D>>,
        solver: &Solver,
        store: &mut D::Store,
    ) -> bool {
        let range: SegRange<I> = if let Some(range) = segments.ver_range() {
            range.into()
        } else {
            return false;
        };
        let mut tree: SegExpTree<I, I, IdSegment<I>> = if let Some(tree) = SegExpTree::new(range) {
            tree
        } else {
            return self.list_split(snap_radius, segments, solver, store);
        };

        let mut reusable_buffer = Vec::new();

        let mut need_to_fix = true;
        let mut any_intersection = false;

        let mut snap_radius = snap_radius;

        while need_to_fix && segments.len() > 2 {
            need_to_fix = false;
            self.marks.clear();

            let radius = snap_radius.radius::<I>();

            for (i, si) in segments.iter().enumerate() {
                let time = si.x_segment.a.x;
                let si_range = si.x_segment.y_range().into();
                for sj in tree.iter_by_range(si_range, time) {
                    let (this_index, scan_index, this, scan) = if si.x_segment < sj.x_segment {
                        (i, sj.id, &si.x_segment, &sj.x_segment)
                    } else {
                        (sj.id, i, &sj.x_segment, &si.x_segment)
                    };

                    let is_round = Self::cross(this_index, scan_index, this, scan, &mut self.marks, radius);

                    need_to_fix = is_round || need_to_fix;
                }

                tree.insert_by_range(si_range, si.id_segment(i));
            }

            if self.marks.is_empty() {
                return any_intersection;
            }

            any_intersection = true;
            tree.clear();

            self.apply(segments, &mut reusable_buffer, solver, store);

            snap_radius.increment();
        }

        any_intersection
    }
}

impl<I: IntNumber> From<LineRange<I>> for SegRange<I> {
    #[inline]
    fn from(value: LineRange<I>) -> Self {
        Self {
            min: value.min,
            max: value.max,
        }
    }
}

trait VerticalRange {
    type Int: IntNumber;

    fn ver_range(&self) -> Option<LineRange<Self::Int>>;
}

impl<I: IntNumber, C: Send, D: Send> VerticalRange for Vec<Segment<C, I, D>> {
    type Int = I;

    fn ver_range(&self) -> Option<LineRange<I>> {
        let mut min_y = self.first()?.x_segment.a.y;
        let mut max_y = min_y;

        for edge in self.iter() {
            min_y = min_y.min(edge.x_segment.a.y);
            max_y = max_y.max(edge.x_segment.a.y);
            min_y = min_y.min(edge.x_segment.b.y);
            max_y = max_y.max(edge.x_segment.b.y);
        }

        Some(LineRange {
            min: min_y,
            max: max_y,
        })
    }
}

impl<I: IntNumber, C: Send, D: Send> Segment<C, I, D> {
    #[inline]
    fn id_segment(&self, id: usize) -> IdSegment<I> {
        IdSegment {
            id,
            x_segment: self.x_segment,
        }
    }
}
