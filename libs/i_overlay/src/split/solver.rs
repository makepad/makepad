use crate::core::edge_data::{EdgeDataSplit, OverlayEdgeData};
use crate::core::solver::Solver;
use crate::geom::x_segment::XSegment;
use crate::segm::merge::ShapeSegmentsMerge;
use crate::segm::segment::Segment;
use crate::segm::sort::ShapeSegmentsSort;
use crate::segm::winding::WindingCount;
use crate::split::cross_solver::{CrossSolver, CrossType, EndMask};
use crate::split::line_mark::{LineMark, SortMarkByIndexAndPoint};
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_key_sort::sort::key::SortKey;
use i_tree::{Expiration, LayoutNumber};

pub(crate) struct SplitSolver<I: IntNumber> {
    pub(super) marks: Vec<LineMark<I>>,
}

impl<I: IntNumber> SplitSolver<I> {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        Self { marks: Vec::new() }
    }
}

impl<I> SplitSolver<I>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
{
    #[inline]
    pub(crate) fn split_segments<C: WindingCount, D: OverlayEdgeData<C>>(
        &mut self,
        segments: &mut Vec<Segment<C, I, D>>,
        solver: &Solver,
    ) -> bool {
        let mut store = D::Store::default();
        self.split_segments_with_store(segments, solver, &mut store)
    }

    #[inline]
    pub(crate) fn split_segments_with_store<C: WindingCount, D: OverlayEdgeData<C>>(
        &mut self,
        segments: &mut Vec<Segment<C, I, D>>,
        solver: &Solver,
        store: &mut D::Store,
    ) -> bool {
        if segments.is_empty() {
            return false;
        }

        segments.sort_by_ab(solver.is_parallel_sort_allowed());
        let any_merged = segments.merge_if_needed_with_store(store);
        if segments.is_empty() {
            return true;
        }

        let any_intersection = self.split(segments, solver, store);
        any_merged | any_intersection
    }

    #[inline]
    fn split<C: WindingCount, D: OverlayEdgeData<C>>(
        &mut self,
        segments: &mut Vec<Segment<C, I, D>>,
        solver: &Solver,
        store: &mut D::Store,
    ) -> bool {
        let is_list = solver.is_list_split(segments);
        let snap_radius = solver.snap_radius();
        if is_list {
            return self.list_split(snap_radius, segments, solver, store);
        }

        let is_fragmentation = solver.is_fragmentation_required(segments);

        if is_fragmentation {
            self.fragment_split(snap_radius, segments, solver, store)
        } else {
            self.tree_split(snap_radius, segments, solver, store)
        }
    }

    pub(super) fn cross(
        i: usize,
        j: usize,
        ei: &XSegment<I>,
        ej: &XSegment<I>,
        marks: &mut Vec<LineMark<I>>,
        radius: I::Wide,
    ) -> bool {
        let cross = if let Some(cross) = CrossSolver::<I>::cross(ei, ej, radius) {
            cross
        } else {
            return false;
        };

        match cross.cross_type {
            CrossType::Pure => {
                marks.push(LineMark {
                    index: i,
                    point: cross.point,
                });
                marks.push(LineMark {
                    index: j,
                    point: cross.point,
                });
            }
            CrossType::TargetEnd => {
                marks.push(LineMark {
                    index: j,
                    point: cross.point,
                });
            }
            CrossType::OtherEnd => {
                marks.push(LineMark {
                    index: i,
                    point: cross.point,
                });
            }
            CrossType::Overlay => {
                let mask = CrossSolver::<I>::collinear(ei, ej);
                if mask == 0 {
                    return false;
                }

                if mask.is_target_a() {
                    marks.push(LineMark {
                        index: j,
                        point: ei.a,
                    });
                }

                if mask.is_target_b() {
                    marks.push(LineMark {
                        index: j,
                        point: ei.b,
                    });
                }

                if mask.is_other_a() {
                    marks.push(LineMark {
                        index: i,
                        point: ej.a,
                    });
                }

                if mask.is_other_b() {
                    marks.push(LineMark {
                        index: i,
                        point: ej.b,
                    });
                }
            }
        }

        cross.is_round
    }

    pub(super) fn apply<C: WindingCount, D: OverlayEdgeData<C>>(
        &mut self,
        segments: &mut Vec<Segment<C, I, D>>,
        reusable_buffer: &mut Vec<LineMark<I>>,
        solver: &Solver,
        store: &mut D::Store,
    ) {
        self.marks
            .sort_by_index_and_point(solver.is_parallel_sort_allowed(), reusable_buffer);
        self.marks.dedup();

        segments.reserve(self.marks.len());

        // split segments

        let mut i = 0;
        while i < self.marks.len() {
            let start = i;
            let m0 = self.marks[i];

            i += 1;
            while i < self.marks.len() && self.marks[i].index == m0.index {
                i += 1;
            }

            let s0 = unsafe {
                // SAFETY: m0.index < segments.len() (marks are built from valid segment indices).
                // We take at most one &mut to that element per group. We drop the &mut before any push,
                // so no aliasing or reallocation invalidation can occur.
                segments.get_unchecked_mut(m0.index)
            };

            let count = s0.count;
            let data = s0.data;
            let x_seg = s0.x_segment;

            if start + 1 == i {
                // single split
                let (d0, d1) = data.split(
                    EdgeDataSplit {
                        a: x_seg.a,
                        p: m0.point,
                        b: x_seg.b,
                    },
                    store,
                );
                *s0 = Segment::create_and_validate_with_data(x_seg.a, m0.point, count, d0, store);
                let s1 = Segment::create_and_validate_with_data(m0.point, x_seg.b, count, d1, store);
                segments.push(s1);

                continue;
            }

            // we have several points
            let sub_marks = &mut self.marks[start..i];
            Self::sort_sub_marks(sub_marks, x_seg);

            let m0 = sub_marks[0];
            let (d0, mut rest_data) = data.split(
                EdgeDataSplit {
                    a: x_seg.a,
                    p: m0.point,
                    b: x_seg.b,
                },
                store,
            );
            *s0 = Segment::create_and_validate_with_data(x_seg.a, m0.point, count, d0, store);

            let mut p0 = m0.point;

            for mi in sub_marks.iter().skip(1) {
                let (di, next_data) = rest_data.split(
                    EdgeDataSplit {
                        a: p0,
                        p: mi.point,
                        b: x_seg.b,
                    },
                    store,
                );
                segments.push(Segment::create_and_validate_with_data(
                    p0, mi.point, count, di, store,
                ));
                rest_data = next_data;
                p0 = mi.point;
            }

            segments.push(Segment::create_and_validate_with_data(
                p0, x_seg.b, count, rest_data, store,
            ));
        }

        segments.sort_by_ab(solver.is_parallel_sort_allowed());
        segments.merge_if_needed_with_store(store);
    }

    #[inline]
    fn sort_sub_marks(marks: &mut [LineMark<I>], x_seg: XSegment<I>) {
        let mut j0 = 0;
        let mut j = 1;

        let m0 = marks[0];
        let mut x0 = m0.point.x;
        while j < marks.len() {
            let xi = marks[j].point.x;
            if x0 == xi {
                j += 1;
                continue;
            }

            if j0 + 1 < j {
                let (y0, y1) = Self::y_range(j0, j, x_seg, marks);
                Self::sort_sub_marks_by_y(y0, y1, &mut marks[j0..j]);
            }

            x0 = xi;
            j0 = j;
            j += 1;
        }

        if j0 + 1 < j {
            let (y0, y1) = Self::y_range(j0, j, x_seg, marks);
            Self::sort_sub_marks_by_y(y0, y1, &mut marks[j0..j]);
        }
    }

    #[inline]
    fn y_range(j0: usize, j1: usize, s: XSegment<I>, marks: &[LineMark<I>]) -> (I, I) {
        let y0 = if j0 == 0 { s.a.y } else { marks[j0 - 1].point.y };
        let y1 = if j1 == marks.len() {
            s.b.y
        } else {
            marks[j1].point.y
        };
        (y0, y1)
    }

    #[inline]
    fn sort_sub_marks_by_y(y0: I, y1: I, marks: &mut [LineMark<I>]) {
        // The x-coordinate is the same for every point
        // By default, the range should be sorted in ascending order by the y-coordinate.
        if y0 > y1 {
            // reverse the order to sort the range in descending order by the y-coordinate.
            marks.reverse();
        }
    }
}
